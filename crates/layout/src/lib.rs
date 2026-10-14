//! Layout analysis: lines, blocks, columns, reading order, labels, tables.
//!
//! Input is the flat span list a content stream produces — one span per text
//! run, in drawing order. Output is the ordered list of elements a reader would
//! perceive, which is what the Docling JSON should contain.
//!
//! The pipeline is:
//!
//! 1. **Rows** — spans that share a baseline band are merged. A bold word inside
//!    a sentence, and the cells of a table row, each become one row.
//! 2. **Runs** — a row is cut wherever adjacent spans are more than a space
//!    apart. This is what keeps two columns apart: their lines share a baseline,
//!    so a column gutter has to be recognised inside the row, not between rows.
//! 3. **Tables** — a run of consecutive rows with the same number of aligned
//!    runs is a table, found *before* the cut so that no vertical whitespace
//!    band can tear its columns apart. The page's own **rules** are part of the
//!    question, because the two things they are read for look alike: a note block
//!    at the foot of a page is a marker column and a text column, two rows of it,
//!    and a number in the first — which is exactly a two-column table. The rule
//!    the page drew above its notes is not part of any table it appears to
//!    introduce, so a candidate standing in the page's foot *under a rule* is
//!    left as the rows it is and reaches the footnote pass (step 8).
//! 4. **Recursive XY-cut** — the page is split on whitespace bands that no run
//!    crosses; a vertical band between two groups of runs is a column gutter.
//!    Leaves are visited in reading order (a split along x gives left column
//!    then right).
//! 5. **Paragraphs** — within a leaf, a line that stops short of the right edge
//!    *while the next starts full* ends a paragraph, which is how justified and
//!    ragged text both signal it. A first-line indent starts one too.
//! 6. **Labels** — font size relative to the body size separates headings, and
//!    literal prefixes separate captions and list items. A sideways stamp in the
//!    page's margin is furniture, not a heading.
//!
//! Pictures are interleaved into the reading order rather than appended, so a
//! figure lands where it appears on the page.
//!
//! 7. **Furniture** — a second pass over the whole document ([`mark_furniture`])
//!    labels the running heads, footers and page numbers, which no single page
//!    can be asked about: a page number is one only because the next page has
//!    one too.
//!
//! 8. **Footnotes** — a third pass over the whole document ([`mark_footnotes`])
//!    labels the notes a page sets at its foot, by the two marks that make one:
//!    the **rule** the page drew above them and the **marker** a note opens
//!    with, which the body of the page also uses. It runs *after* the furniture,
//!    and cannot take a page number from it: a note is the document's text (it
//!    is the label `footnote`, not a furniture label) and a page number is the
//!    page's.

use std::collections::HashMap;
use std::ops::Range;

mod furniture;

pub use furniture::{
    is_furniture_label, mark_footnotes, mark_furniture, PageElements, FOOTNOTE_BAND, FURNITURE_BAND,
};

/// A text run as extracted from a content stream, in points, top-left origin.
#[derive(Debug, Clone)]
pub struct Span {
    pub text: String,
    pub bbox: [f64; 4],
    pub font_size: f64,
    /// The run is rotated or sheared. It then shares no baseline with the
    /// horizontal text around it, however much their boxes overlap.
    pub rotated: bool,
}

/// One element in reading order.
#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Text {
        text: String,
        label: String,
        /// Heading depth, when a numbered heading made it discoverable.
        level: Option<i32>,
        /// List bullet or numbering, removed from `text`.
        marker: Option<String>,
        bbox: [f64; 4],
    },
    Table {
        rows: Vec<Vec<String>>,
        bbox: [f64; 4],
    },
    /// `index` refers to the caller's own picture list.
    Picture { index: usize, bbox: [f64; 4] },
}

/// How far apart two spans may sit and still be one run.
///
/// Below a column gutter and well above a word space, so a table cell or a
/// neighbouring column becomes its own run. Prose never approaches it.
const RUN_GAP_EM: f64 = 1.5;

/// A longer cell than this makes a row prose: tables hold short entries, prose
/// columns hold sentences, and the two are otherwise geometrically identical.
const MAX_CELL_CHARS: usize = 25;

/// How far a column's left edge may wander and still count as one column.
const COLUMN_TOLERANCE: f64 = 6.0;

/// How much of the group's height each side of a suspected gutter has to cover
/// before the split is read as two columns.
///
/// Below this, the "gutter" is only the left margin: a page-footer stamp in the
/// margin would then be read *before* the heading at the top of the page, which
/// is exactly backwards.
const COLUMN_COVERAGE: f64 = 0.25;

/// How far into a page's left or right edge a sideways block may stand and still
/// count as its margin.
///
/// A rotated block *inside* the text is a table's cells or a sideways heading and
/// is read as the document; one standing in the outer edge is a stamp, which is
/// the page's own — `arxiv_attention.pdf`'s `arXiv:1706.03762v7` begins 3% in from
/// the left of a 612 pt page. Docling files those as `page_header`, and so do we.
const MARGIN_STAMP: f64 = 0.1;

/// One span inside a row, with the geometry a table needs.
#[derive(Debug, Clone)]
struct Cell {
    left: f64,
    right: f64,
    text: String,
}

/// A baseline band: every span that shares a line on the page.
#[derive(Debug, Clone)]
struct Row {
    bbox: [f64; 4],
    font_size: f64,
    /// Every span in the row was drawn rotated or sheared. Such a row is
    /// furniture — a margin stamp, a watermark — and never the document title.
    rotated: bool,
    /// The row's spans, left to right, so cell x positions are real geometry
    /// rather than a guess from character counts.
    cells: Vec<Cell>,
}

impl Row {
    fn top(&self) -> f64 {
        self.bbox[1]
    }
    fn bottom(&self) -> f64 {
        self.bbox[3]
    }
}

/// One horizontally contiguous part of a row: the unit reading order works on.
#[derive(Debug, Clone)]
struct Run {
    bbox: [f64; 4],
    font_size: f64,
    text: String,
    /// The run is rotated or sheared, inherited from its row.
    rotated: bool,
    /// Index of the row this run came from, so runs can be grouped into lines.
    row: usize,
}

impl Run {
    fn left(&self) -> f64 {
        self.bbox[0]
    }
    fn top(&self) -> f64 {
        self.bbox[1]
    }
}

/// A unit of reading order: either a whole table or a single run.
#[derive(Debug, Clone)]
struct Block {
    bbox: [f64; 4],
    kind: BlockKind,
}

#[derive(Debug, Clone)]
enum BlockKind {
    Table(Vec<Vec<String>>),
    Run(Run),
    /// The rows of a note block the page's rule marks out: one vector per *row*, so that
    /// each note reaches the paragraphing on its own. They stay one block because they
    /// are one thing on the page (the rule says so, and the XY-cut must not separate
    /// them), and one paragraph per note is what keeps the second note's marker at the
    /// front of the second note.
    Notes(Vec<Vec<Run>>),
}

fn union_bbox(items: impl Iterator<Item = [f64; 4]>) -> [f64; 4] {
    let mut bbox = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for item in items {
        bbox[0] = bbox[0].min(item[0]);
        bbox[1] = bbox[1].min(item[1]);
        bbox[2] = bbox[2].max(item[2]);
        bbox[3] = bbox[3].max(item[3]);
    }
    bbox
}

/// Join a row of adjacent spans, restoring the spaces a producer expressed by
/// repositioning text rather than by writing a space character.
fn join_cells(cells: &[&Cell], font_size: f64) -> String {
    let mut text = String::new();
    let mut previous_right: Option<f64> = None;
    for cell in cells {
        if let Some(right) = previous_right {
            if cell.left - right > 0.2 * font_size && !text.ends_with(' ') {
                text.push(' ');
            }
        }
        text.push_str(&cell.text);
        previous_right = Some(cell.right);
    }
    text.trim_end().to_string()
}

/// Merge spans that share a baseline into rows.
fn to_rows(spans: &[Span]) -> Vec<Row> {
    let mut ordered: Vec<&Span> = spans.iter().filter(|s| !s.text.trim().is_empty()).collect();
    ordered.sort_by(|a, b| {
        a.bbox[1]
            .partial_cmp(&b.bbox[1])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.bbox[0]
                    .partial_cmp(&b.bbox[0])
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let mut rows: Vec<Vec<&Span>> = Vec::new();
    for span in ordered {
        let joined = rows.last_mut().is_some_and(|row| {
            let top = row.iter().map(|s| s.bbox[1]).fold(f64::INFINITY, f64::min);
            let bottom = row
                .iter()
                .map(|s| s.bbox[3])
                .fold(f64::NEG_INFINITY, f64::max);
            let overlap = bottom.min(span.bbox[3]) - top.max(span.bbox[1]);
            let smallest = (bottom - top).min(span.bbox[3] - span.bbox[1]);
            // A sideways run crosses a line of body text without being part of
            // it, however much their boxes overlap: only runs drawn the same way
            // share a row.
            overlap > 0.5 * smallest
                && overlap > 0.0
                && row.iter().all(|s| s.rotated == span.rotated)
        });
        if joined {
            rows.last_mut().expect("checked").push(span);
        } else {
            rows.push(vec![span]);
        }
    }

    rows.into_iter()
        .map(|mut row| {
            row.sort_by(|a, b| {
                a.bbox[0]
                    .partial_cmp(&b.bbox[0])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let bbox = union_bbox(row.iter().map(|s| s.bbox));
            let font_size = row
                .iter()
                .map(|s| s.font_size)
                .fold(0.0_f64, f64::max)
                .max(1.0);
            let cells: Vec<Cell> = row
                .iter()
                .map(|s| Cell {
                    left: s.bbox[0],
                    right: s.bbox[2],
                    text: s.text.trim().to_string(),
                })
                .collect();
            let rotated = row.iter().all(|s| s.rotated);
            Row {
                bbox,
                font_size,
                rotated,
                cells,
            }
        })
        .collect()
}

/// Cut a row into runs wherever a gap that only a layout boundary produces
/// appears. A column gutter lives here: the neighbouring column's line shares
/// this baseline, so nothing between rows can detect it.
fn runs_of(row: &Row, row_index: usize) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut current: Vec<&Cell> = Vec::new();
    let break_at = RUN_GAP_EM * row.font_size;
    for cell in &row.cells {
        let far_enough = current
            .last()
            .is_some_and(|previous| cell.left - previous.right >= break_at);
        if far_enough {
            runs.push(finish_run(&current, row, row_index));
            current.clear();
        }
        current.push(cell);
    }
    if !current.is_empty() {
        runs.push(finish_run(&current, row, row_index));
    }
    runs
}

fn finish_run(cells: &[&Cell], row: &Row, row_index: usize) -> Run {
    Run {
        bbox: union_bbox(
            cells
                .iter()
                .map(|c| [c.left, row.bbox[1], c.right, row.bbox[3]]),
        ),
        font_size: row.font_size,
        text: join_cells(cells, row.font_size),
        rotated: row.rotated,
        row: row_index,
    }
}

/// Could this row be a table row? Short entries, at least two of them.
fn is_table_row(row: &Row) -> bool {
    row.cells.len() >= 2
        && row.cells.iter().all(|c| !c.text.is_empty())
        && row
            .cells
            .iter()
            .any(|c| c.text.chars().count() <= MAX_CELL_CHARS)
}

/// Does `row` continue the table that `first` started?
fn continues_table(first: &Row, previous: &Row, row: &Row) -> bool {
    if row.cells.len() != first.cells.len() || row.cells.iter().any(|c| c.text.is_empty()) {
        return false;
    }
    // A table is contiguous: the next row cannot be half a page away.
    if row.top() - previous.bottom() > 3.0 * first.font_size {
        return false;
    }
    first
        .cells
        .iter()
        .zip(&row.cells)
        .all(|(a, b)| (a.left - b.left).abs() <= COLUMN_TOLERANCE)
}

/// Does a cell begin with a number? A column of those is what makes a grid a
/// table rather than a paragraph with a hanging indent.
fn is_numeric_cell(text: &str) -> bool {
    text.split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit())
}

/// Whether a run of rows is a **note block** rather than the grid it looks like: a
/// marker column — every row opening with a cell of one or two characters, the
/// superscript a note is referred to by (`3`, `∗`, `†`) — and text beside it.
///
/// Which is what a note block *is*: the marker is not a column of data, it is the mark
/// that ties the note to the text it is a note to, and the rows after it are lines of the
/// page's text rather than records. A grid whose first column is a description and whose
/// second is an amount (`irs_f1040.pdf`'s field lines, `8 Additional income from Schedule
/// 1, line 10 . . . | 8`) is not a note block however much the page's rules make it look
/// like one, and keeps its grid.
fn is_note_block(rows: &[Row]) -> bool {
    rows.iter()
        .all(|row| row.cells.len() >= 2 && is_marker_cell(&row.cells[0].text))
        && rows.iter().any(|row| {
            row.cells[1..]
                .iter()
                .map(|cell| cell.text.chars().count())
                .sum::<usize>()
                >= 16
        })
}

/// Whether a cell holds the marker a note opens with rather than a word.
///
/// One or two characters: a superscript number up to `99`, or the symbol a document uses
/// for its notes — including one our reading got wrong (`†` that came out as `y`, which
/// is the marker column all the same).
fn is_marker_cell(text: &str) -> bool {
    let marker = text.trim();
    !marker.is_empty() && marker.chars().count() <= 2 && !marker.contains(char::is_whitespace)
}

/// What a run of rows turned out to be, once the page's own marks had their say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rows {
    /// A table: a grid whose columns the XY-cut must not tear apart.
    Table,
    /// The notes a page set at its foot: rows that look like a grid and are not one, and
    /// that the paragraphing must not merge into each other.
    Notes,
}

/// The runs of rows layout has to handle as a unit, in top-down order: tables, and the
/// note blocks that look exactly like them.
///
/// `rules` and `page_size` are the page's own marks, and they are what keeps a page's
/// **notes** out of the table list. A note block is a marker column and a text column, two
/// rows of it, with a number in the first — which is a two-column table by every test
/// below, and is why the numbers alone cannot decide it (`math_seq2seq.pdf` page 4's
/// `3 | http://www.statmt.org/…` and `4 | Implementations are available…`). What separates
/// the two readings is the rule the page drew above its notes: it stands where a table's
/// top border would, and a page does not draw one above a table. A candidate standing in
/// the page's foot *under a rule* is therefore the page's notes — handed on as rows, to be
/// cut into runs and reach the footnote pass, which reads that same rule.
fn groupings_in(
    rows: &[Row],
    rules: &[[f64; 4]],
    page_size: [f64; 2],
) -> Vec<(Range<usize>, Rows)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let mut end = i;
        if is_table_row(&rows[i]) {
            end = i + 1;
            while end < rows.len() && continues_table(&rows[i], &rows[end - 1], &rows[end]) {
                end += 1;
            }
        }
        // Two aligned rows are also what a footnote marker and its text look
        // like (`†  Work performed...`). A real two-row table carries at least
        // one number; missing a small table costs its grid, inventing one
        // corrupts prose.
        let enough = end - i >= 3
            || (end - i == 2
                && rows[i..end]
                    .iter()
                    .any(|r| r.cells.iter().any(|c| is_numeric_cell(&c.text))));
        let ruled_note = enough && is_note_block(&rows[i..end]) && {
            let bbox = union_bbox(rows[i..end].iter().map(|r| r.bbox));
            furniture::in_the_foot(bbox, page_size[1])
                && furniture::rule_above(bbox, rules, page_size[1])
        };
        match (enough, ruled_note) {
            (false, _) => i += 1,
            (true, false) => {
                found.push((i..end, Rows::Table));
                i = end;
            }
            // The rows are consumed either way: nothing inside a note block is the start
            // of a table.
            (true, true) => {
                found.push((i..end, Rows::Notes));
                i = end;
            }
        }
    }
    found
}

/// An uncovered band on one axis that splits blocks into two non-empty groups.
fn find_split(blocks: &[&Block], axis: Axis, min_gap: f64) -> Option<(Vec<usize>, Vec<usize>)> {
    let (start, end) = match axis {
        Axis::X => (0, 2),
        Axis::Y => (1, 3),
    };
    let mut bands: Vec<(f64, f64)> = Vec::new();
    for a in blocks {
        for b in blocks {
            let (far, near) = (a.bbox[end], b.bbox[start]);
            if far < near && near - far >= min_gap {
                bands.push((far, near));
            }
        }
    }
    // Widest band first: a column gutter beats an incidental gap.
    bands.sort_by(|a, b| {
        (b.1 - b.0)
            .partial_cmp(&(a.1 - a.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (lo, hi) in bands {
        // The band has to be *uncovered*: no block may straddle it. Without this
        // check a band measured between two non-adjacent rows would cut a
        // paragraph in half at a gap no whitespace actually occupies.
        if blocks
            .iter()
            .any(|b| b.bbox[start] < hi && b.bbox[end] > lo)
        {
            continue;
        }
        let mid = (lo + hi) / 2.0;
        let mut before = Vec::new();
        let mut after = Vec::new();
        for (i, block) in blocks.iter().enumerate() {
            if block.bbox[end] <= mid {
                before.push(i);
            } else if block.bbox[start] >= mid {
                after.push(i);
            }
        }
        if !before.is_empty() && !after.is_empty() && before.len() + after.len() == blocks.len() {
            return Some((before, after));
        }
    }
    None
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

/// Do both sides of a vertical split span enough of the group's height to be
/// columns rather than a margin stamp beside the body text?
fn both_cover_the_height(before: &[usize], after: &[usize], refs: &[&Block]) -> bool {
    let extent = |side: &[usize]| {
        let top = side
            .iter()
            .map(|i| refs[*i].bbox[1])
            .fold(f64::INFINITY, f64::min);
        let bottom = side
            .iter()
            .map(|i| refs[*i].bbox[3])
            .fold(f64::NEG_INFINITY, f64::max);
        (bottom - top).max(0.0)
    };
    let total = extent(&(0..refs.len()).collect::<Vec<_>>());
    total > 0.0
        && extent(before) >= COLUMN_COVERAGE * total
        && extent(after) >= COLUMN_COVERAGE * total
}

/// Recursive XY-cut. Returns groups of block indices in reading order.
fn xy_cut(blocks: &[Block], min_x_gap: f64, min_y_gap: f64, depth: usize) -> Vec<Vec<usize>> {
    fn recurse(
        blocks: &[Block],
        indices: Vec<usize>,
        min_x_gap: f64,
        min_y_gap: f64,
        depth: usize,
        out: &mut Vec<Vec<usize>>,
    ) {
        if indices.len() <= 1 || depth > 24 {
            out.push(indices);
            return;
        }
        let refs: Vec<&Block> = indices.iter().map(|i| &blocks[*i]).collect();
        // Columns first: a gutter that no run crosses is the strongest signal,
        // but only when both sides of it actually run down the page.
        let split = find_split(&refs, Axis::X, min_x_gap)
            .filter(|(before, after)| both_cover_the_height(before, after, &refs))
            .or_else(|| find_split(&refs, Axis::Y, min_y_gap));
        match split {
            Some((before, after)) => {
                let take = |group: Vec<usize>| {
                    group
                        .into_iter()
                        .map(|local| indices[local])
                        .collect::<Vec<_>>()
                };
                recurse(blocks, take(before), min_x_gap, min_y_gap, depth + 1, out);
                recurse(blocks, take(after), min_x_gap, min_y_gap, depth + 1, out);
            }
            None => out.push(indices),
        }
    }

    let mut out = Vec::new();
    recurse(
        blocks,
        (0..blocks.len()).collect(),
        min_x_gap,
        min_y_gap,
        depth,
        &mut out,
    );
    out
}

/// Strip a leading bullet or numbering, returning the marker and the rest.
fn split_marker(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim_start();
    let bullets = [
        '\u{2022}', '\u{25cf}', '\u{25aa}', '\u{2013}', '\u{2014}', '*', '-',
    ];
    if let Some(first) = trimmed.chars().next() {
        if bullets.contains(&first) {
            let rest = trimmed[first.len_utf8()..].trim_start();
            if !rest.is_empty() {
                return Some((first.to_string(), rest.to_string()));
            }
        }
    }
    // Numbering such as `1.`, `2)`, `iv.`, and the multi-level `1.2` that needs
    // no trailing separator. At most three digits, so a year like `2019.` is not
    // mistaken for a list number.
    let word: String = trimmed.chars().take_while(|c| !c.is_whitespace()).collect();
    if word.len() >= 2 && word.len() <= 8 {
        let digits = word.chars().filter(|c| c.is_ascii_digit()).count();
        let core = word.trim_end_matches(['.', ')', '(']);
        // A numbering segment does not start with a zero, so a decimal such as
        // `0.9` on a chart axis is not mistaken for a list number.
        let segments: Vec<&str> = core.split('.').collect();
        let numbered_segments = segments
            .iter()
            .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) && !s.starts_with('0'));
        let dotted = !core.is_empty()
            && digits <= 3
            && numbered_segments
            && core.chars().all(|c| c.is_ascii_digit() || c == '.');
        let roman =
            digits == 0 && !core.is_empty() && core.chars().all(|c| "ivxlcdmIVXLCDM".contains(c));
        let terminates = word.ends_with('.') || word.ends_with(')');
        // A dotted prefix is self-terminating: `1.2 Method` needs no dot after.
        if (dotted && (terminates || core.contains('.'))) || (roman && terminates) {
            let rest = trimmed[word.len()..].trim_start();
            if !rest.is_empty() {
                return Some((word, rest.to_string()));
            }
        }
    }
    None
}

/// Heading depth from a numbering prefix like `2.1 ` or `III.`.
///
/// Depth is the number of dot-separated parts, not the number of digits: `10.2`
/// is a second-level heading.
fn heading_level(marker: Option<&str>) -> i32 {
    let marker = marker.unwrap_or_default();
    let parts = marker
        .trim_end_matches(['.', ')', '('])
        .split('.')
        .filter(|p| !p.trim().is_empty())
        .count();
    parts.max(1) as i32
}

/// Is this marker a section number rather than a list number?
///
/// `3.1` numbers a subsection; `1.` or a bullet numbers an item. Inside a
/// paragraph of prose the distinction has to come from the marker, because a
/// section heading and a list item are otherwise the same size.
fn is_section_number(marker: &str) -> bool {
    let trimmed = marker.trim_end_matches(['.', ')', '(']);
    let parts: Vec<&str> = trimmed.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn starts_with_word(text: &str, words: &[&str]) -> bool {
    let lower = text.trim_start().to_ascii_lowercase();
    words.iter().any(|w| {
        // `is_none_or` is newer than this crate's MSRV, and `map_or(true, ..)`
        // trips a newer clippy lint, so the match says exactly what is meant.
        lower.starts_with(w)
            && !matches!(lower[w.len()..].chars().next(), Some(c) if c.is_alphanumeric())
    })
}

/// Join a paragraph's lines, undoing end-of-line hyphenation.
fn join_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for line in lines {
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(text);
            continue;
        }
        // `hyphen-` + `ation` -> `hyphenation`; only when the continuation is
        // lowercase, so `well-Known` style compounds are left alone.
        if out.ends_with('-') && text.chars().next().is_some_and(|c| c.is_lowercase()) {
            out.pop();
            out.push_str(text);
        } else {
            out.push(' ');
            out.push_str(text);
        }
    }
    out
}

/// A row for paragraph purposes: the runs sharing a baseline, joined.
struct LogicalLine {
    bbox: [f64; 4],
    font_size: f64,
    text: String,
    rotated: bool,
}

/// Group a leaf's runs into paragraphs, then label them.
///
/// `page_no`, `page_max` and `page_height` decide whether a block may be the
/// document title, and `title_taken` makes sure only the first one is.
fn paragraphs_in(
    runs: &[&Run],
    body_size: f64,
    page_no: i32,
    page_max: f64,
    page_size: [f64; 2],
    title_taken: &mut bool,
) -> Vec<Element> {
    if runs.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<&Run> = runs.to_vec();
    sorted.sort_by(|a, b| {
        a.row.cmp(&b.row).then(
            a.left()
                .partial_cmp(&b.left())
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    // Runs that share a row are one visual line even when a gutter-looking gap
    // sits between them: the cut already decided that no column separates them.
    let mut lines: Vec<LogicalLine> = Vec::new();
    for run in sorted {
        match lines.last_mut() {
            Some(last) if last.bbox[1] == run.top() => {
                last.bbox = union_bbox([last.bbox, run.bbox].into_iter());
                last.font_size = last.font_size.max(run.font_size);
                last.rotated |= run.rotated;
                if !last.text.ends_with(' ') && !run.text.starts_with(' ') {
                    last.text.push(' ');
                }
                last.text.push_str(&run.text);
            }
            _ => lines.push(LogicalLine {
                bbox: run.bbox,
                font_size: run.font_size,
                text: run.text.clone(),
                rotated: run.rotated,
            }),
        }
    }
    if lines.is_empty() {
        return Vec::new();
    }

    let leaf_right = lines
        .iter()
        .map(|l| l.bbox[2])
        .fold(f64::NEG_INFINITY, f64::max);
    let leaf_left = lines
        .iter()
        .map(|l| l.bbox[0])
        .fold(f64::INFINITY, f64::min);
    let full_width =
        |line: &LogicalLine| line.bbox[2] >= leaf_right - 1.5 * line.font_size.max(4.0);

    let mut groups: Vec<Vec<&LogicalLine>> = vec![vec![&lines[0]]];
    for pair in lines.windows(2) {
        let (prev, next) = (&pair[0], &pair[1]);
        // A paragraph ends where a line stops short *and the next one starts
        // full* — that is the only reliable signal in both justified and ragged
        // text. A short line followed by another short line is just the end of
        // the leaf.
        let ends_paragraph = !full_width(prev) && full_width(next);
        // A first-line indent is small and deliberate. A large shift in the left
        // edge is a centred line or a different column, and those must not be
        // read as a paragraph of their own.
        let indent = next.bbox[0] - leaf_left;
        let indented = indent > 0.5 * prev.font_size && indent < 4.0 * prev.font_size;
        let size_changed = (next.font_size - prev.font_size).abs() > 0.5;
        // Every bullet or number is its own item.
        let starts_item = split_marker(&next.text).is_some();
        if ends_paragraph || indented || size_changed || starts_item {
            groups.push(vec![next]);
        } else {
            groups.last_mut().expect("a group").push(next);
        }
    }

    let mut out = Vec::new();
    for group in groups {
        let bbox = union_bbox(group.iter().map(|l| l.bbox));
        let page_height = page_size[1];
        let font_size = group.iter().map(|l| l.font_size).fold(0.0_f64, f64::max);
        let raw = join_lines(&group.iter().map(|l| l.text.clone()).collect::<Vec<_>>());
        if raw.trim().is_empty() {
            continue;
        }

        let (marker, text) = match split_marker(&raw) {
            Some((marker, rest)) => (Some(marker), rest),
            None => (None, raw),
        };

        // A sideways stamp standing in the page's own margin is the page's, not
        // the document's, whatever size it is set at: `arxiv_attention.pdf`'s
        // arXiv stamp is 20 pt, larger than the title beside it. Docling files
        // those as page furniture, and this is the only thing on a page that
        // argues for it without a second page to compare with.
        let in_margin = bbox[0] >= page_size[0] * (1.0 - MARGIN_STAMP)
            || bbox[2] <= page_size[0] * MARGIN_STAMP;
        // Order matters: a numbered heading is a heading, not a list item, so
        // the size test has to come before the marker test.
        let label = if group.iter().all(|l| l.rotated) && in_margin {
            "page_header"
        } else if font_size > body_size * 1.15 && text.chars().count() < 200 {
            // The document title is the first block set at the page's largest
            // size, and it must not be numbered: `1.2 Method` is structure.
            // It also has to be in the upper third, set horizontally, and made
            // of words: a margin stamp is not the title even when it outranks it
            // (`arxiv_attention.pdf` carries a sideways 20 pt arXiv stamp above
            // its 17 pt title), and the year an IRS form prints in 24 pt
            // (`2025`) is a stamp too.
            let is_title = page_no == 1
                && !*title_taken
                && marker.is_none()
                && font_size >= page_max - 0.01
                && bbox[1] <= page_height / 3.0
                && page_max > 0.0
                && text.chars().any(char::is_alphabetic)
                && group.iter().all(|l| !l.rotated);
            if is_title {
                *title_taken = true;
                "title"
            } else {
                "section_header"
            }
        } else if starts_with_word(
            &text,
            &["figure", "fig.", "table", "tab.", "listing", "algorithm"],
        ) {
            "caption"
        } else if let Some(marker) = marker.as_deref() {
            // `3.1 Encoder and Decoder Stacks` is a heading, not an item, even
            // when the page sets it at body size.
            if is_section_number(marker) {
                "section_header"
            } else {
                "list_item"
            }
        } else {
            "paragraph"
        };

        let level = (label == "section_header").then(|| heading_level(marker.as_deref()));
        // A heading's numbering is structure, its bullet is not: keep the marker
        // only where it genuinely labels the item.
        let marker = match label {
            "section_header" | "list_item" => marker,
            _ => None,
        };

        out.push(Element::Text {
            text,
            label: label.to_string(),
            level,
            marker,
            bbox,
        });
    }
    out
}

/// Keep a box inside the page rectangle.
///
/// Content legitimately reaches past the MediaBox (cropped artwork, a producer's
/// over-estimated glyph advance), and a box that leaves the page is not usable
/// by anything downstream.
fn clamp(b: [f64; 4], page: [f64; 2]) -> [f64; 4] {
    let (w, h) = (page[0].max(1.0), page[1].max(1.0));
    [
        b[0].clamp(0.0, w),
        b[1].clamp(0.0, h),
        b[2].clamp(0.0, w),
        b[3].clamp(0.0, h),
    ]
}

/// Analyse one page.
///
/// `pictures` are the page's picture boxes; each is returned as
/// [`Element::Picture`] carrying its index, placed in reading order.
///
/// No rules: this is the shape a page with nothing drawn on it takes, which is what the
/// tests of everything but tables want.
pub fn analyze(spans: &[Span], page_size: [f64; 2], pictures: &[[f64; 4]]) -> Vec<Element> {
    analyze_page(spans, page_size, pictures, &[], 1)
}

/// Analyse one page, knowing which page it is.
///
/// The page number only affects labelling: only the first page can carry the
/// document title. `rules` are the horizontal rules the page drew, `[l, t, r, b]`
/// with a top-left origin — empty for a producer that publishes no drawing, and
/// empty is what every flow format has. They are asked about a table candidate in the
/// page's foot, where the rule above a note stands where a table's top border would:
/// see [`groupings_in`].
pub fn analyze_page(
    spans: &[Span],
    page_size: [f64; 2],
    pictures: &[[f64; 4]],
    rules: &[[f64; 4]],
    page_no: i32,
) -> Vec<Element> {
    let rows = to_rows(spans);
    if rows.is_empty() {
        return pictures
            .iter()
            .enumerate()
            .map(|(index, bbox)| Element::Picture {
                index,
                bbox: clamp(*bbox, page_size),
            })
            .collect();
    }

    // Body size is the most common row size weighted by length: that is what
    // most of the text looks like, which is what "normal" means here.
    let mut weight: HashMap<u32, usize> = HashMap::new();
    for row in &rows {
        let key = (row.font_size * 10.0).round() as u32;
        let chars: usize = row
            .cells
            .iter()
            .map(|c| c.text.chars().count().max(1))
            .sum();
        *weight.entry(key).or_insert(0) += chars;
    }
    let body_key = weight
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(k, _)| *k)
        .unwrap_or(100);
    let body_size = (body_key as f64 / 10.0).max(1.0);

    // Tables first: their columns must not be torn apart by the XY-cut.
    let groupings = groupings_in(&rows, rules, page_size);
    let mut blocks: Vec<Block> = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        if let Some((range, kind)) = groupings.iter().find(|(r, _)| r.start == index) {
            let bbox = union_bbox(rows[range.clone()].iter().map(|r| r.bbox));
            let block = match kind {
                Rows::Table => {
                    let data: Vec<Vec<String>> = rows[range.clone()]
                        .iter()
                        .map(|row| row.cells.iter().map(|c| c.text.clone()).collect())
                        .collect();
                    Block {
                        bbox,
                        kind: BlockKind::Table(data),
                    }
                }
                Rows::Notes => Block {
                    bbox,
                    kind: BlockKind::Notes(
                        rows[range.clone()]
                            .iter()
                            .enumerate()
                            .map(|(offset, row)| runs_of(row, range.start + offset))
                            .collect(),
                    ),
                },
            };
            blocks.push(block);
            index = range.end;
        } else {
            for run in runs_of(&rows[index], index) {
                blocks.push(Block {
                    bbox: run.bbox,
                    kind: BlockKind::Run(run),
                });
            }
            index += 1;
        }
    }

    let min_x_gap = body_size.max(9.0);
    let min_y_gap = body_size * 0.8;
    // The title's size is the largest *in the upper third of the page*, and only
    // among horizontal rows: a rotated margin stamp must not raise the bar above
    // the real title's size, however large it is set.
    let title_zone = page_size[1] / 3.0;
    let page_max = rows
        .iter()
        .filter(|r| r.top() <= title_zone && !r.rotated)
        .map(|r| r.font_size)
        .fold(0.0_f64, f64::max);
    // Only one block per document is the title, so the decision is made once and
    // then remembered across leaves.
    let mut title_taken = false;
    let mut placed = vec![false; pictures.len()];
    let mut out: Vec<Element> = Vec::new();

    for group in xy_cut(&blocks, min_x_gap, min_y_gap, 0) {
        let mut members: Vec<&Block> = group.iter().map(|i| &blocks[*i]).collect();
        members.sort_by(|a, b| {
            a.bbox[1]
                .partial_cmp(&b.bbox[1])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.bbox[0]
                        .partial_cmp(&b.bbox[0])
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });
        let group_bbox = union_bbox(members.iter().map(|b| b.bbox));

        // Pictures are emitted before the first block that starts below them: a
        // figure introduces the text that follows it. Anything still unclaimed
        // after the last group was genuinely below all the text.
        let mut here: Vec<usize> = (0..pictures.len())
            .filter(|i| !placed[*i] && pictures[*i][1] < group_bbox[3])
            .collect();
        here.sort_by(|a, b| {
            pictures[*a][1]
                .partial_cmp(&pictures[*b][1])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for index in &here {
            placed[*index] = true;
        }
        for index in here {
            out.push(Element::Picture {
                index,
                bbox: clamp(pictures[index], page_size),
            });
        }

        let mut pending: Vec<&Run> = Vec::new();
        for block in members {
            match &block.kind {
                BlockKind::Run(run) => pending.push(run),
                BlockKind::Table(data) => {
                    out.extend(paragraphs_in(
                        &pending,
                        body_size,
                        page_no,
                        page_max,
                        page_size,
                        &mut title_taken,
                    ));
                    pending.clear();
                    out.push(Element::Table {
                        rows: data.clone(),
                        bbox: clamp(block.bbox, page_size),
                    });
                }
                // Each row is one note, so each row is handed over on its own: two
                // notes joined into one paragraph would read as one note with the
                // second's marker buried in the middle of the text, where nothing
                // looks for it.
                BlockKind::Notes(note_rows) => {
                    out.extend(paragraphs_in(
                        &pending,
                        body_size,
                        page_no,
                        page_max,
                        page_size,
                        &mut title_taken,
                    ));
                    pending.clear();
                    for row in note_rows {
                        let row: Vec<&Run> = row.iter().collect();
                        out.extend(paragraphs_in(
                            &row,
                            body_size,
                            page_no,
                            page_max,
                            page_size,
                            &mut title_taken,
                        ));
                    }
                }
            }
        }
        out.extend(paragraphs_in(
            &pending,
            body_size,
            page_no,
            page_max,
            page_size,
            &mut title_taken,
        ));
    }

    // Anything the XY-cut never assigned (a picture set entirely in the margins)
    // still needs a place; keeping it is better than dropping it.
    for index in (0..pictures.len()).filter(|i| !placed[*i]) {
        out.push(Element::Picture {
            index,
            bbox: clamp(pictures[index], page_size),
        });
    }

    for element in &mut out {
        match element {
            Element::Text { bbox, .. }
            | Element::Table { bbox, .. }
            | Element::Picture { bbox, .. } => {
                *bbox = clamp(*bbox, page_size);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, l: f64, t: f64, r: f64, b: f64, size: f64) -> Span {
        Span {
            text: text.to_string(),
            bbox: [l, t, r, b],
            font_size: size,
            rotated: false,
        }
    }

    fn rotated_span(text: &str, l: f64, t: f64, r: f64, b: f64, size: f64) -> Span {
        Span {
            text: text.to_string(),
            bbox: [l, t, r, b],
            font_size: size,
            rotated: true,
        }
    }

    fn texts(elements: &[Element]) -> Vec<(String, String)> {
        elements
            .iter()
            .filter_map(|e| match e {
                Element::Text { text, label, .. } => Some((text.clone(), label.clone())),
                _ => None,
            })
            .collect()
    }

    fn kinds(elements: &[Element]) -> Vec<&str> {
        elements
            .iter()
            .map(|e| match e {
                Element::Text { .. } => "text",
                Element::Table { .. } => "table",
                Element::Picture { .. } => "picture",
            })
            .collect()
    }

    #[test]
    fn merges_spans_that_share_a_baseline_into_one_line() {
        let spans = vec![
            span("Hello", 72.0, 100.0, 100.0, 112.0, 12.0),
            span("World", 103.0, 100.0, 133.0, 112.0, 12.0),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        assert_eq!(
            texts(&out),
            vec![("Hello World".to_string(), "paragraph".to_string())]
        );
    }

    #[test]
    fn joins_lines_into_a_paragraph_and_undoes_hyphenation() {
        // Two full-width lines then a short one: one paragraph, in justified
        // text as in ragged text.
        let spans = vec![
            span("the emul-", 72.0, 100.0, 540.0, 112.0, 12.0),
            span("ation worked", 72.0, 114.0, 540.0, 126.0, 12.0),
            span("and finished here.", 72.0, 128.0, 300.0, 140.0, 12.0),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        assert_eq!(
            out.len(),
            1,
            "expected one paragraph, got {:?}",
            texts(&out)
        );
        match &out[0] {
            Element::Text { text, .. } => {
                assert!(text.contains("emulation worked"), "{text}");
                assert!(text.contains("and finished here."), "{text}");
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn a_short_line_followed_by_a_full_one_ends_a_paragraph() {
        let spans = vec![
            span(
                "first paragraph runs to the margin",
                72.0,
                100.0,
                540.0,
                112.0,
                12.0,
            ),
            span("and ends short.", 72.0, 114.0, 200.0, 126.0, 12.0),
            span(
                "second paragraph starts at the margin",
                72.0,
                130.0,
                540.0,
                142.0,
                12.0,
            ),
            span("and continues here", 72.0, 144.0, 540.0, 156.0, 12.0),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        assert_eq!(got.len(), 2, "expected two paragraphs, got {got:?}");
        assert_eq!(
            got[0].0,
            "first paragraph runs to the margin and ends short."
        );
        assert_eq!(
            got[1].0,
            "second paragraph starts at the margin and continues here"
        );
    }

    #[test]
    fn two_columns_are_read_left_column_first() {
        // Real prose lines, unequal column lengths: a uniform grid would be a
        // table, so the fixture must not be one.
        let mut spans = Vec::new();
        let left = [
            "The left column carries a full sentence of prose.",
            "Its second line keeps running to the same right edge.",
            "A third line continues the same paragraph here.",
            "The fourth line carries on as the sentence unwinds.",
            "And the last line of this column stops early.",
        ];
        for (row, text) in left.iter().enumerate() {
            let y = 100.0 + row as f64 * 14.0;
            spans.push(span(text, 72.0, y, 290.0, y + 12.0, 12.0));
        }
        let right = [
            "The right column is a paragraph of its own.",
            "Its second line is a little longer than that.",
            "Its third and final line ends the column.",
        ];
        for (row, text) in right.iter().enumerate() {
            let y = 100.0 + row as f64 * 14.0;
            spans.push(span(text, 320.0, y, 540.0, y + 12.0, 12.0));
        }
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        assert_eq!(got.len(), 2, "expected two column blocks, got {got:?}");
        assert!(
            got[0].0.starts_with("The left column"),
            "left column first: {:?}",
            got[0]
        );
        assert!(
            got[0].0.contains("stops early"),
            "left column read as a unit: {:?}",
            got[0]
        );
        assert!(
            !got[0].0.contains("right column"),
            "columns interleaved: {:?}",
            got[0]
        );
        assert!(got[1].0.starts_with("The right column"), "{:?}", got[1]);
        assert!(got[1].0.contains("ends the column"), "{:?}", got[1]);
    }

    #[test]
    fn a_bigger_font_becomes_a_heading_and_a_numbered_one_keeps_its_level() {
        let spans = vec![
            span("1.2 Method", 72.0, 80.0, 200.0, 98.0, 18.0),
            span(
                "Body text at the body size, running on.",
                72.0,
                110.0,
                540.0,
                122.0,
                12.0,
            ),
            span(
                "More body text at the body size.",
                72.0,
                124.0,
                540.0,
                136.0,
                12.0,
            ),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        assert_eq!(got[0].1, "section_header", "{got:?}");
        assert_eq!(got[0].0, "Method", "the number should be kept as a marker");
        match &out[0] {
            Element::Text { marker, level, .. } => {
                assert_eq!(marker.as_deref(), Some("1.2"));
                assert_eq!(*level, Some(2), "`1.2` is a second-level heading");
            }
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!(got[1].1, "paragraph");
    }

    #[test]
    fn an_unnumbered_first_heading_is_the_title() {
        let spans = vec![
            span("ACME Supplies", 60.0, 60.0, 313.0, 92.0, 20.0),
            span(
                "Body text at the body size, running on.",
                60.0,
                120.0,
                540.0,
                132.0,
                12.0,
            ),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        assert_eq!(texts(&out)[0].1, "title", "{:?}", texts(&out));
    }

    #[test]
    fn bullets_become_list_items_with_their_marker() {
        let spans = vec![
            span("\u{2022} first thing", 72.0, 100.0, 300.0, 112.0, 12.0),
            span("\u{2022} second thing", 72.0, 114.0, 300.0, 126.0, 12.0),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        assert_eq!(got.len(), 2, "each bullet is its own item: {got:?}");
        assert!(got.iter().all(|(_, label)| label == "list_item"), "{got:?}");
        assert_eq!(got[0].0, "first thing");
    }

    #[test]
    fn a_first_line_indent_starts_a_paragraph() {
        // Paragraph two begins two ems in; both paragraphs are otherwise flush
        // to the same margin.
        let spans = vec![
            span(
                "the first paragraph runs all the way out",
                72.0,
                100.0,
                540.0,
                112.0,
                12.0,
            ),
            span(
                "and carries on to its second line",
                72.0,
                114.0,
                540.0,
                126.0,
                12.0,
            ),
            span(
                "a new paragraph starts indented here",
                96.0,
                128.0,
                540.0,
                140.0,
                12.0,
            ),
            span(
                "and continues flush to the margin",
                72.0,
                142.0,
                540.0,
                154.0,
                12.0,
            ),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        assert_eq!(got.len(), 2, "expected two paragraphs, got {got:?}");
        assert_eq!(
            got[0].0,
            "the first paragraph runs all the way out and carries on to its second line"
        );
        assert_eq!(
            got[1].0,
            "a new paragraph starts indented here and continues flush to the margin"
        );
    }

    #[test]
    fn a_centred_line_is_not_an_indent() {
        // A narrow, centred line of a paragraph is far from the left edge, but it
        // is a continuation, not a new paragraph.
        let spans = vec![
            span(
                "a line that runs the full width of the measure",
                124.0,
                100.0,
                488.0,
                112.0,
                12.0,
            ),
            span(
                "more of the same line of text here",
                124.0,
                114.0,
                488.0,
                126.0,
                12.0,
            ),
            span("and it ends centred.", 268.0, 128.0, 346.0, 140.0, 12.0),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        assert_eq!(
            out.len(),
            1,
            "expected one paragraph, got {:?}",
            texts(&out)
        );
    }

    #[test]
    fn a_sub_numbered_line_is_a_section_not_a_list_item() {
        // Papers set section headings at body size, so the numbering is the only
        // signal that `3.1` introduces a section rather than an item.
        let spans = vec![
            span(
                "3.1 Encoder and Decoder Stacks",
                108.0,
                100.0,
                300.0,
                112.0,
                12.0,
            ),
            span(
                "1. first item of a real list",
                108.0,
                130.0,
                300.0,
                142.0,
                12.0,
            ),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        assert_eq!(got[0].1, "section_header", "{got:?}");
        assert_eq!(got[0].0, "Encoder and Decoder Stacks");
        assert_eq!(got[1].1, "list_item", "{got:?}");
        match &out[0] {
            Element::Text { marker, level, .. } => {
                assert_eq!(marker.as_deref(), Some("3.1"));
                assert_eq!(*level, Some(2), "`3.1` is a second-level heading");
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn heading_depth_counts_parts_not_digits() {
        assert_eq!(heading_level(Some("3.1")), 2);
        assert_eq!(heading_level(Some("10.2")), 2);
        assert_eq!(heading_level(Some("2.1.3")), 3);
        assert_eq!(heading_level(Some("III.")), 1);
        assert_eq!(heading_level(Some("4)")), 1);
    }

    #[test]
    fn a_decimal_number_is_not_a_list_marker() {
        // Chart axis labels look exactly like multi-level numbering, so the
        // leading zero in `0.9` is what tells them apart.
        for text in ["0.9 without batch norm", "0.5 1.0 1.5 velocity"] {
            let spans = vec![span(text, 72.0, 100.0, 300.0, 112.0, 12.0)];
            let out = analyze(&spans, [612.0, 792.0], &[]);
            assert_eq!(texts(&out)[0].1, "paragraph", "{text:?} became a list item");
        }
    }

    #[test]
    fn a_year_is_not_a_list_marker() {
        let spans = vec![span(
            "2019. Results improved substantially.",
            72.0,
            100.0,
            540.0,
            112.0,
            12.0,
        )];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        assert_eq!(texts(&out)[0].1, "paragraph", "{:?}", texts(&out));
    }

    #[test]
    fn a_rotated_margin_stamp_does_not_join_the_line_it_crosses() {
        // A sideways stamp in the left margin overlaps a line of body text
        // vertically *and* horizontally, so only the rotation itself separates
        // them - and joining them would corrupt both.
        let spans = vec![
            rotated_span(
                "arXiv:1706.03762v7 [cs.CL] 2 Aug 2023",
                32.0,
                520.0,
                44.0,
                700.0,
                20.0,
            ),
            span(
                "the abstract runs across the measure here",
                108.0,
                531.0,
                461.0,
                543.0,
                12.0,
            ),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        assert_eq!(got.len(), 2, "the stamp must stand alone: {got:?}");
        assert!(
            got.iter().any(|(t, _)| t.starts_with("arXiv:1706")),
            "the stamp was swallowed: {got:?}"
        );
        assert!(
            got.iter().any(|(t, _)| t.starts_with("the abstract")),
            "the abstract line was lost: {got:?}"
        );
    }

    #[test]
    fn a_lone_year_is_not_the_title() {
        // The IRS form prints `2025` at 24 pt above everything else. It is a
        // stamp on the page, not the document's name.
        let spans = vec![
            span("2025", 260.0, 40.0, 330.0, 66.0, 24.0),
            span(
                "Department of the Treasury",
                60.0,
                100.0,
                300.0,
                112.0,
                12.0,
            ),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        assert!(
            got.iter().all(|(_, l)| l != "title"),
            "a year must not be titled: {got:?}"
        );
    }

    #[test]
    fn a_margin_stamp_does_not_reorder_the_page() {
        // Page 3 of `basicapi.pdf`: a heading at the top, a footer line halfway
        // down, and the producer's stamp in the far left bottom corner. A
        // vertical split would treat that stamp as a left column and read it
        // *before* the heading, so the split has to be refused.
        let spans = vec![
            span("Paragraph 1.1", 45.35, 77.88, 175.35, 97.88, 20.0),
            span("page 3 / 3", 389.28, 813.49, 595.28, 825.49, 12.0),
            span(
                "Powered by TCPDF (www.tcpdf.org)",
                2.83,
                839.66,
                18.83,
                840.66,
                1.0,
            ),
        ];
        let out = analyze(&spans, [595.28, 842.0], &[]);
        let got: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert_eq!(
            got,
            vec![
                "Paragraph 1.1".to_string(),
                "page 3 / 3".to_string(),
                "Powered by TCPDF (www.tcpdf.org)".to_string(),
            ],
            "the page is not in reading order"
        );
    }

    #[test]
    fn a_rotated_stamp_is_never_the_title() {
        // Page 1 of the arXiv paper carries a 20 pt stamp down the left margin
        // and a 17 pt title across the top. The stamp is the largest type on the
        // page and its box reaches into the upper third, so only its rotation
        // keeps it out of the title slot.
        let spans = vec![
            rotated_span(
                "arXiv:1706.03762v7 [cs.CL] 2 Aug 2023",
                32.0,
                100.0,
                44.0,
                500.0,
                20.0,
            ),
            span(
                "Attention Is All You Need",
                211.0,
                145.0,
                400.0,
                162.0,
                17.0,
            ),
            span(
                "The dominant sequence transduction models are based on complex",
                108.0,
                300.0,
                504.0,
                312.0,
                10.0,
            ),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        let title = got.iter().find(|(_, l)| *l == "title");
        assert_eq!(
            title.map(|(t, _)| t.as_str()),
            Some("Attention Is All You Need"),
            "the sideways stamp was titled: {got:?}"
        );
        // And it is the page's, not the document's: Docling files this stamp as
        // `page_header`, and reading it as a section heading put a 20 pt margin
        // note in the document's outline.
        assert!(
            got.iter()
                .any(|(t, l)| t.starts_with("arXiv:1706") && *l == "page_header"),
            "the stamp must survive as the page's furniture: {got:?}"
        );
    }

    #[test]
    fn a_sideways_block_inside_the_text_is_still_the_document() {
        // Rotation alone does not make furniture: a rotated table's cells and a
        // sideways heading are read as text, and only a block in the page's own
        // outer margin is the page's.
        let spans = vec![
            rotated_span("Table 4: hyperparameters", 200.0, 100.0, 212.0, 400.0, 12.0),
            span(
                "The dominant sequence transduction models are based on complex",
                108.0,
                500.0,
                504.0,
                512.0,
                10.0,
            ),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        // Which label it gets is the size rule's business; what matters here is
        // that the page's margin did not claim it.
        assert!(
            got.iter()
                .any(|(t, l)| t.starts_with("Table 4") && *l != "page_header"),
            "a rotated block inside the text is not page furniture: {got:?}"
        );
    }

    #[test]
    fn figure_captions_are_labelled() {
        let spans = vec![span(
            "Figure 3: architecture of the model.",
            72.0,
            500.0,
            300.0,
            512.0,
            10.0,
        )];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        assert_eq!(texts(&out)[0].1, "caption");
    }

    #[test]
    fn a_picture_lands_before_the_text_below_it() {
        let spans = vec![
            span("text above the figure", 72.0, 100.0, 540.0, 112.0, 12.0),
            span("text below the figure", 72.0, 400.0, 540.0, 412.0, 12.0),
        ];
        let pictures = [[100.0, 200.0, 300.0, 350.0]];
        let out = analyze(&spans, [612.0, 792.0], &pictures);
        assert_eq!(kinds(&out), vec!["text", "picture", "text"], "{out:?}");
    }

    #[test]
    fn a_picture_that_no_text_group_covers_is_still_emitted() {
        let spans = vec![span("only text", 72.0, 100.0, 200.0, 112.0, 12.0)];
        let pictures = [[500.0, 700.0, 560.0, 760.0]];
        let out = analyze(&spans, [612.0, 792.0], &pictures);
        assert!(out
            .iter()
            .any(|e| matches!(e, Element::Picture { index: 0, .. })));
    }

    #[test]
    fn aligned_cells_become_a_table() {
        // One span per cell, which is what a table row actually produces.
        let mut spans = Vec::new();
        for (row, cells) in [
            ["Name", "Qty", "Price"],
            ["Widget", "4", "2.50"],
            ["Gadget", "1", "9.00"],
        ]
        .iter()
        .enumerate()
        {
            let y = 100.0 + row as f64 * 14.0;
            for (left, text) in [(72.0, cells[0]), (200.0, cells[1]), (300.0, cells[2])] {
                spans.push(span(text, left, y, left + 40.0, y + 12.0, 12.0));
            }
        }
        let out = analyze(&spans, [612.0, 792.0], &[]);
        match &out[0] {
            Element::Table { rows, .. } => {
                assert_eq!(rows.len(), 3, "{rows:?}");
                assert_eq!(rows[0], vec!["Name", "Qty", "Price"], "{rows:?}");
                assert_eq!(rows[1][0], "Widget");
                assert_eq!(rows[2][2], "9.00");
            }
            other => panic!("expected a table, got {other:?}"),
        }
    }

    #[test]
    fn a_two_row_marker_block_is_not_a_table() {
        // The dagger footnotes on page 1 of the arXiv paper are two rows with an
        // aligned second column and no number anywhere: a hanging indent.
        let mut spans = Vec::new();
        for (row, (marker, text)) in [
            ("\u{2020}", "Work performed while at Google Brain."),
            ("\u{2021}", "Work performed while at Google Research."),
        ]
        .iter()
        .enumerate()
        {
            let y = 100.0 + row as f64 * 14.0;
            spans.push(span(marker, 108.0, y, 113.0, y + 12.0, 12.0));
            spans.push(span(text, 118.0, y, 300.0, y + 12.0, 12.0));
        }
        let out = analyze(&spans, [612.0, 792.0], &[]);
        assert!(
            out.iter().all(|e| matches!(e, Element::Text { .. })),
            "a footnote block became a grid: {out:?}"
        );
    }

    /// The rows of a note block, as `math_seq2seq.pdf` page 4 publishes them: a marker
    /// column and a text column, two rows, each with a number in it.
    fn a_note_block() -> Vec<Span> {
        vec![
            span("3", 120.7, 698.3, 124.0, 704.1, 6.0),
            span(
                "http://www.statmt.org/wmt14/translation-task.html",
                126.0,
                699.0,
                390.5,
                708.1,
                9.0,
            ),
            span("4", 120.7, 709.2, 124.0, 715.0, 6.0),
            span(
                "Implementations are available at https://github.com/lisa-groundhog/GroundHog .",
                126.0,
                710.0,
                479.0,
                719.0,
                9.0,
            ),
        ]
    }

    /// The rule the page draws above those notes.
    const NOTE_RULE: [[f64; 4]; 1] = [[108.0, 697.4, 251.5, 697.4]];

    #[test]
    fn a_note_block_under_the_pages_rule_is_not_a_table() {
        // Every test a table is found by says this is one: two aligned rows, a number in
        // each. The page's own mark is the one thing that says otherwise — it drew a rule
        // over these rows, and a page does not draw one over a table.
        let out = analyze_page(&a_note_block(), [612.0, 792.0], &[], &NOTE_RULE, 4);
        assert!(
            out.iter().all(|e| matches!(e, Element::Text { .. })),
            "a note block became a grid: {out:?}"
        );
        // And two notes stay two notes: a note row is one line of the page's text, and
        // merged with the next the second note's marker ends up in the middle of a
        // paragraph, where nothing looks for it.
        let notes: Vec<&str> = out
            .iter()
            .filter_map(|e| match e {
                Element::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].starts_with("3 http://"), "{notes:?}");
        assert!(notes[1].starts_with("4 Implementations"), "{notes:?}");
    }

    #[test]
    fn the_same_rows_with_no_rule_over_them_are_a_table() {
        // The rule is what decides it, and nothing else: the numbers that make this a
        // table by the ordinary tests are in the rows either way.
        let out = analyze_page(&a_note_block(), [612.0, 792.0], &[], &[], 4);
        assert!(
            matches!(out.first(), Some(Element::Table { .. })),
            "expected a table with no rule to say otherwise: {out:?}"
        );
    }

    #[test]
    fn a_rule_above_a_note_block_high_on_the_page_does_not_make_it_notes() {
        // The rule has to be the page's *foot* rule: a note is set at the foot of a page,
        // and a table with a rule over it in the body is still a table.
        let mut spans = a_note_block();
        for span in &mut spans {
            span.bbox[1] -= 500.0;
            span.bbox[3] -= 500.0;
        }
        let rules = [[108.0, 197.4, 251.5, 197.4]];
        let out = analyze_page(&spans, [612.0, 792.0], &[], &rules, 4);
        assert!(
            matches!(out.first(), Some(Element::Table { .. })),
            "a table in the body was read as notes: {out:?}"
        );
    }

    #[test]
    fn a_two_row_table_with_a_number_is_a_table() {
        let mut spans = Vec::new();
        for (row, cells) in [["Metric", "Value"], ["Accuracy", "0.94"]]
            .iter()
            .enumerate()
        {
            let y = 100.0 + row as f64 * 14.0;
            for (left, text) in [(72.0, cells[0]), (300.0, cells[1])] {
                spans.push(span(text, left, y, left + 40.0, y + 12.0, 12.0));
            }
        }
        let out = analyze(&spans, [612.0, 792.0], &[]);
        match &out[0] {
            Element::Table { rows, .. } => {
                assert_eq!(rows.len(), 2, "{rows:?}");
                assert_eq!(rows[1], vec!["Accuracy", "0.94"], "{rows:?}");
            }
            other => panic!("expected a table, got {other:?}"),
        }
    }

    #[test]
    fn a_single_span_with_wide_spaces_is_not_a_table() {
        let spans = vec![
            span("Name    Qty    Price", 72.0, 100.0, 400.0, 112.0, 12.0),
            span("Widget    4    2.50", 72.0, 114.0, 400.0, 126.0, 12.0),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        assert!(
            out.iter().all(|e| matches!(e, Element::Text { .. })),
            "{out:?}"
        );
    }

    #[test]
    fn prose_is_never_mistaken_for_a_table() {
        let spans = vec![
            span(
                "This is an ordinary sentence of prose text.",
                72.0,
                100.0,
                540.0,
                112.0,
                12.0,
            ),
            span(
                "And another one that follows it directly.",
                72.0,
                114.0,
                540.0,
                126.0,
                12.0,
            ),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        assert_eq!(kinds(&out), vec!["text"], "prose became a grid: {out:?}");
        assert_eq!(
            texts(&out)[0].0,
            "This is an ordinary sentence of prose text. And another one that follows it directly."
        );
    }

    #[test]
    fn a_page_with_no_text_keeps_its_pictures() {
        let out = analyze(&[], [612.0, 792.0], &[[0.0, 0.0, 612.0, 792.0]]);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Element::Picture { index: 0, .. }));
    }

    #[test]
    fn a_scan_with_ocr_lines_produces_readable_blocks() {
        // The invoice fixture: a heading, then widely spaced detail lines.
        let spans = vec![
            span("ACME Supplies", 57.6, 62.9, 313.0, 92.6, 20.0),
            span("Invoice Number 12345", 59.5, 119.0, 300.0, 133.4, 12.0),
            span("Date 2026-10-12", 59.5, 157.4, 236.2, 171.8, 12.0),
            span("Widget assembly 42.00", 58.1, 224.6, 323.5, 243.4, 12.0),
            span("Delivery charge 8.00", 59.5, 258.2, 301.4, 277.0, 12.0),
            span("Total Due 50.00", 57.6, 330.7, 259.7, 348.5, 12.0),
        ];
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        // The page's own leading is wider than a paragraph break, so each line is
        // its own block — but nothing is glued together or split mid-line.
        assert_eq!(
            got[0].1, "title",
            "the big first line is the title: {got:?}"
        );
        assert_eq!(got[0].0, "ACME Supplies");
        assert_eq!(
            got.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            vec![
                "ACME Supplies",
                "Invoice Number 12345",
                "Date 2026-10-12",
                "Widget assembly 42.00",
                "Delivery charge 8.00",
                "Total Due 50.00",
            ],
            "{got:?}"
        );
    }

    #[test]
    fn tightly_spaced_ocr_lines_become_one_paragraph() {
        // The same invoice re-OCR'd with normal single spacing: 12 pt text on a
        // 14 pt pitch leaves a 2 pt gap, which is not a paragraph break.
        let spans = (0..4)
            .map(|row| {
                let y = 100.0 + row as f64 * 14.0;
                span(
                    "a tightly spaced line of scanned text",
                    60.0,
                    y,
                    400.0,
                    y + 12.0,
                    12.0,
                )
            })
            .collect::<Vec<_>>();
        let out = analyze(&spans, [612.0, 792.0], &[]);
        let got = texts(&out);
        assert_eq!(got.len(), 1, "expected one block, got {got:?}");
        assert_eq!(
            got[0].0,
            "a tightly spaced line of scanned text a tightly spaced line of scanned text a tightly spaced line of scanned text a tightly spaced line of scanned text"
        );
    }
}
