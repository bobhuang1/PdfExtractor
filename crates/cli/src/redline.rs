//! Two-document redline: compare two versions of one PDF or DOCX and write a
//! page-aware HTML report.
//!
//! The alignment is not done here. Both versions are converted to the same
//! Docling document the rest of the tool writes, and the rows come from
//! [`pdfextractor_ui::diff::compare_documents`] — the engine the Compare panel
//! and the `--compare` gate already use. What this module adds is the *other*
//! direction of the same comparison: instead of holding one reading against a
//! Docling reference, it holds an older version against a newer one, and gives a
//! reader a page-by-page redline.
//!
//! A row that differs is cut into words for presentation only, with the words
//! that left marked `<del>` and the words that arrived marked `<ins>`. The
//! alignment still decides which blocks belong together; the word diff never
//! changes a verdict or a page.
//!
//! Pages come from the same provenance the comparison already reads. A PDF
//! publishes a page and a box for every item, so its report is grouped by page.
//! A DOCX is flow-based by design — Word repaginates as the text is edited — and
//! publishes no page for its items, so its report is one clearly labelled
//! *Document flow* section rather than an invented pagination.

use pdfextractor_core::DoclingDocument;
use pdfextractor_ui::diff::{
    Block, Kind, Placement, Row, Summary, TableGrid, TableGridCell, Verdict,
};
use std::path::Path;

/// Render a redline report from two already-converted documents.
///
/// `old` is the earlier version and `new` the later one; `family` is what the
/// report calls the format (`PDF` or `DOCX`). The old/new names are the file
/// names a reader handed the command line, not paths, so the page is portable.
pub fn render(
    old: &DoclingDocument,
    new: &DoclingDocument,
    old_name: &str,
    new_name: &str,
    family: &str,
) -> anyhow::Result<String> {
    let old_value = serde_json::to_value(old)?;
    let new_value = serde_json::to_value(new)?;
    let (mut rows, summary, _placed_pages) =
        pdfextractor_ui::diff::compare_documents(&old_value, &new_value);
    // The Compare engine's `Contained` verdict means "the reference's finer
    // reading lies wholly inside one block of ours" — a paragraph Docling cut
    // into line items. That is not a claim the two versions say the same thing:
    // a deletion at the end of a block leaves the new text wholly inside the old
    // one. For a redline, a contained row whose words actually differ is a
    // changed row, so it must not be folded away as segmentation.
    promote_changed_containment(&mut rows);
    Ok(Report {
        old_name,
        new_name,
        family,
        rows: &rows,
        summary: &summary,
    }
    .html())
}

/// Write a rendered report to `path`, through a sibling temporary file so a
/// stopped run leaves either the whole report or none of it.
pub fn write(path: &Path, html: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "redline.html".to_string());
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temp, html)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

/// One page section: the old page and the new page its rows were read from.
///
/// Both are absent for a flow document. The pair is kept rather than one
/// number because a change can move between pages, and the page heading is the
/// place a reader sees that.
struct Section {
    old: Option<i64>,
    new: Option<i64>,
    rows: Vec<usize>,
}

struct PreviewBox {
    placement: Placement,
    label: String,
    kind: Kind,
    class: &'static str,
    detail: String,
}

struct Report<'a> {
    old_name: &'a str,
    new_name: &'a str,
    family: &'a str,
    rows: &'a [Row],
    summary: &'a Summary,
}

impl Report<'_> {
    fn html(&self) -> String {
        let sections = self.sections();
        let mut out = String::new();
        out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
        out.push_str("<meta charset=\"utf-8\">\n");
        out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        out.push_str(&format!(
            "<title>{} redline against {}</title>\n",
            escape(self.new_name),
            escape(self.old_name)
        ));
        out.push_str("<style>\n");
        out.push_str(STYLE);
        out.push_str("</style>\n</head>\n<body>\n");
        out.push_str("<header class=\"masthead\">\n");
        out.push_str("<p class=\"eyebrow\">Redline report</p>\n");
        out.push_str(&format!(
            "<h1>{} <span class=\"against\">against</span> {}</h1>\n",
            escape(self.new_name),
            escape(self.old_name)
        ));
        let page_meta = if self.summary.pages_ours == 0 && self.summary.pages_theirs == 0 {
            "flow document (no fixed pagination)".to_string()
        } else {
            format!(
                "{} page(s) before · {} page(s) after",
                self.summary.pages_ours, self.summary.pages_theirs
            )
        };
        out.push_str(&format!(
            "<p class=\"meta\">{} · {page_meta}</p>\n",
            escape(self.family)
        ));
        out.push_str(&self.summary_cards());
        out.push_str("</header>\n");
        out.push_str(&self.page_navigation(&sections));
        out.push_str("<main>\n");
        if sections.is_empty() {
            out.push_str("<p class=\"empty-report\">The comparison found no blocks to show.</p>\n");
        }
        for section in &sections {
            out.push_str(&self.section_html(section));
        }
        out.push_str("</main>\n");
        out.push_str(
            "<footer class=\"foot\">Compared with the same block alignment as the Compare panel. ",
        );
        out.push_str("Red text was removed; green text was added.</footer>\n");
        out.push_str("</body>\n</html>\n");
        out
    }

    /// Every page in the two readings, in reading order.
    ///
    /// A change that moved from page 3 to page 5 is one section keyed by the
    /// pair, not two sections that would show the same change twice. Pages with
    /// nothing but unchanged blocks are kept: a redline a reader can use has to
    /// show where it is in the document, not only where the changes are.
    fn sections(&self) -> Vec<Section> {
        let mut grouped: std::collections::BTreeMap<(Option<i64>, Option<i64>), Vec<usize>> =
            std::collections::BTreeMap::new();
        for (index, row) in self.rows.iter().enumerate() {
            let old = first_page(&row.ours);
            let new = first_page(&row.theirs);
            grouped.entry((old, new)).or_default().push(index);
        }
        let mut sections: Vec<Section> = grouped
            .into_iter()
            .map(|((old, new), rows)| Section { old, new, rows })
            .collect();
        sections.sort_by_key(|section| {
            (
                section.old.or(section.new).unwrap_or(i64::MAX),
                section.old.unwrap_or(i64::MAX),
                section.new.unwrap_or(i64::MAX),
            )
        });
        sections
    }

    fn summary_cards(&self) -> String {
        let changed = self
            .rows
            .iter()
            .filter(|row| matches!(row.verdict, Verdict::Differs | Verdict::Notation))
            .count();
        let added = self
            .rows
            .iter()
            .filter(|row| row.verdict == Verdict::TheirsOnly)
            .count();
        let removed = self
            .rows
            .iter()
            .filter(|row| row.verdict == Verdict::OursOnly)
            .count();
        let placed = self
            .rows
            .iter()
            .filter(|row| row.verdict == Verdict::Placed)
            .count();
        let unchanged = self
            .rows
            .iter()
            .filter(|row| matches!(row.verdict, Verdict::Same | Verdict::Contained))
            .count();
        let mut cards = String::from("<div class=\"summary\" aria-label=\"Summary\">\n");
        for (label, value, class) in [
            ("changed", changed, "changed"),
            ("added", added, "added"),
            ("removed", removed, "removed"),
            ("placed", placed, "placed"),
            ("unchanged", unchanged, "same"),
        ] {
            cards.push_str(&format!(
                "<div class=\"metric {class}\"><strong>{value}</strong><span>{label}</span></div>\n"
            ));
        }
        cards.push_str(&format!(
            "<div class=\"metric agreement\"><strong>{:.1}%</strong><span>new text held by old</span></div>\n",
            self.summary.text_agreement * 100.0
        ));
        cards.push_str("</div>\n");
        if self.summary.truncated > 0 {
            cards.push_str(&format!(
                "<p class=\"warning\">This report stops at the comparison budget: {} block(s)/row(s) are not shown.</p>\n",
                self.summary.truncated
            ));
        }
        if !self.summary.notes.is_empty() {
            cards.push_str("<details class=\"notes\"><summary>Structural notes</summary><ul>\n");
            for note in &self.summary.notes {
                cards.push_str(&format!("<li>{}</li>\n", escape(note)));
            }
            cards.push_str("</ul></details>\n");
        }
        cards
    }

    fn page_navigation(&self, sections: &[Section]) -> String {
        let mut nav = String::from("<nav class=\"pages\" aria-label=\"Pages\">\n");
        nav.push_str("<span class=\"pages-label\">Jump to</span>\n");
        for section in sections {
            let changes = section
                .rows
                .iter()
                .filter(|index| is_change(&self.rows[**index]))
                .count();
            nav.push_str(&format!(
                "<a href=\"#{}\" data-changes=\"{changes}\">{}<span>{changes} change{}</span></a>\n",
                anchor(section),
                page_label(section),
                if changes == 1 { "" } else { "s" }
            ));
        }
        nav.push_str("</nav>\n");
        nav
    }

    fn section_html(&self, section: &Section) -> String {
        let changed = section
            .rows
            .iter()
            .filter(|index| is_change(&self.rows[**index]))
            .count();
        let unchanged = section.rows.len().saturating_sub(changed);
        let mut out = format!(
            "<section class=\"page\" id=\"{}\" data-old-page=\"{}\" data-new-page=\"{}\">\n",
            anchor(section),
            section
                .old
                .map_or_else(|| "none".to_string(), |page| page.to_string()),
            section
                .new
                .map_or_else(|| "none".to_string(), |page| page.to_string())
        );
        out.push_str("<div class=\"page-head\">\n");
        out.push_str(&format!("<h2>{}</h2>\n", page_label(section)));
        out.push_str(&format!(
            "<span>{} change{} · {} unchanged</span>\n",
            changed,
            if changed == 1 { "" } else { "s" },
            unchanged
        ));
        out.push_str("</div>\n");
        out.push_str(&self.page_previews(section));
        if changed == 0 {
            out.push_str("<p class=\"quiet\">No changes on this page.</p>\n");
        }
        for index in &section.rows {
            if is_change(&self.rows[*index]) {
                out.push_str(&self.row_html(&self.rows[*index]));
            }
        }
        if unchanged > 0 {
            out.push_str(&format!(
                "<details class=\"unchanged\"><summary>{} unchanged block{}</summary>\n",
                unchanged,
                if unchanged == 1 { "" } else { "s" }
            ));
            for index in &section.rows {
                if !is_change(&self.rows[*index]) {
                    out.push_str(&self.row_html(&self.rows[*index]));
                }
            }
            out.push_str("</details>\n");
        }
        out.push_str("</section>\n");
        out
    }

    /// Draw the before and after pages from the placement boxes both PDFs
    /// published. The preview is a page outline, not a raster render: it is the
    /// geometry the comparison actually used, so a moved or resized picture is
    /// visible even on a machine with no PDF renderer.
    fn page_previews(&self, section: &Section) -> String {
        let before = self.page_preview(section, true);
        let after = self.page_preview(section, false);
        if before.is_empty() && after.is_empty() {
            return String::new();
        }
        let mut out = String::from("<div class=\"page-previews\">\n");
        out.push_str(
            "<p class=\"preview-note\">Placement boxes as published by the PDFs. Amber is changed, red is removed, green is added; pictures are the violet outlined boxes.</p>\n",
        );
        out.push_str(&before);
        out.push_str(&after);
        out.push_str("</div>\n");
        out
    }

    fn page_preview(&self, section: &Section, before: bool) -> String {
        let page = if before { section.old } else { section.new };
        let Some(page) = page else {
            return String::new();
        };
        let mut boxes = Vec::new();
        for index in &section.rows {
            let row = &self.rows[*index];
            let blocks = if before { &row.ours } else { &row.theirs };
            boxes.extend(preview_boxes(row, blocks));
        }
        boxes.retain(|item| item.placement.page == page);
        if boxes.is_empty() {
            return String::new();
        }
        let side = if before { "Before" } else { "After" };
        let svg = preview_svg(&boxes, &format!("{side} page {page} layout"));
        format!(
            "<figure class=\"page-preview-figure\"><figcaption>{side} · page {page}</figcaption>{svg}</figure>\n"
        )
    }

    /// A picture row gets its own tight before/after pair even when the page
    /// preview also contains it: the page says where the picture sits, while the
    /// pair makes a move or resize readable without hunting for its box.
    fn picture_preview(&self, row: &Row) -> String {
        if row.kind != Kind::Picture {
            return String::new();
        }
        let before = picture_side(row, true);
        let after = picture_side(row, false);
        if before.is_empty() && after.is_empty() {
            return String::new();
        }
        format!(
            "<div class=\"picture-previews\"><p class=\"preview-note\">Picture placement</p>{before}{after}</div>\n"
        )
    }

    fn row_html(&self, row: &Row) -> String {
        let class = row_class(row);
        let old_text = text_of(&row.ours);
        let new_text = text_of(&row.theirs);
        let (before, after) = if row.kind == Kind::Table {
            let old_table = row
                .ours
                .iter()
                .find_map(|block| block.table.clone())
                .unwrap_or_else(|| TableGrid::from_rows(&pipe_table_rows(&old_text)));
            let new_table = row
                .theirs
                .iter()
                .find_map(|block| block.table.clone())
                .unwrap_or_else(|| TableGrid::from_rows(&pipe_table_rows(&new_text)));
            match (row.ours.is_empty(), row.theirs.is_empty()) {
                (true, false) => (
                    empty(),
                    table_diff(&TableGrid::from_rows(&[]), &new_table).1,
                ),
                (false, true) => (
                    table_diff(&old_table, &TableGrid::from_rows(&[])).0,
                    empty(),
                ),
                _ => table_diff(&old_table, &new_table),
            }
        } else {
            match row.verdict {
                Verdict::Differs => word_diff(&old_text, &new_text),
                Verdict::Notation => (
                    plain_or_empty(&old_text, "notation"),
                    plain_or_empty(&new_text, "notation"),
                ),
                Verdict::OursOnly => (format!("<del>{}</del>", escape(&old_text)), empty()),
                Verdict::TheirsOnly => (empty(), format!("<ins>{}</ins>", escape(&new_text))),
                _ => (
                    plain_or_empty(&old_text, "same"),
                    plain_or_empty(&new_text, "same"),
                ),
            }
        };
        let kind_class = if row.kind == Kind::Table {
            " table"
        } else {
            ""
        };
        let mut out = format!("<article class=\"row {class}{kind_class}\">\n");
        out.push_str(&format!(
            "<div class=\"row-head\"><span class=\"kind\">{}</span><span class=\"label\">{}</span><span class=\"verdict\">{}</span>",
            kind_label(row.kind),
            escape(&row.kind_label()),
            verdict_label(row.verdict)
        ));
        if let (Some(old), Some(new)) = (first_page(&row.ours), first_page(&row.theirs)) {
            if old != new {
                out.push_str(&format!(
                    "<span class=\"moved\">moved: page {old} → page {new}</span>"
                ));
            }
        }
        out.push_str("</div>\n");
        out.push_str(&format!(
            "<div class=\"side before\"><h3>Before</h3><div class=\"text\">{before}</div></div>\n"
        ));
        out.push_str(&format!(
            "<div class=\"side after\"><h3>After</h3><div class=\"text\">{after}</div></div>\n"
        ));
        out.push_str(&self.picture_preview(row));
        if let Some(note) = &row.note {
            out.push_str(&format!("<p class=\"row-note\">{}</p>\n", escape(note)));
        }
        if let Some(placement) = row.placement {
            out.push_str(&format!(
                "<p class=\"row-note\">Placed on page {} · box {:.1}%, {:.1}% · {:.1}% × {:.1}%</p>\n",
                placement.page,
                placement.x * 100.0,
                placement.y * 100.0,
                placement.w * 100.0,
                placement.h * 100.0
            ));
        }
        out.push_str("</article>\n");
        out
    }
}

/// A block's first page, if it published one.
fn first_page(blocks: &[Block]) -> Option<i64> {
    blocks.iter().find_map(|block| block.page)
}

fn row_class(row: &Row) -> &'static str {
    match row.verdict {
        Verdict::Same => "same",
        Verdict::Notation => "notation",
        Verdict::Differs => "differs",
        Verdict::Contained => "contained",
        Verdict::OursOnly => "removed",
        Verdict::TheirsOnly => "added",
        Verdict::Placed => "placed",
    }
}

fn preview_boxes(row: &Row, blocks: &[Block]) -> Vec<PreviewBox> {
    blocks
        .iter()
        .filter_map(|block| {
            let placement = block.placement()?;
            Some(PreviewBox {
                placement,
                label: block.label.clone(),
                kind: block.kind,
                class: row_class(row),
                detail: block.text.clone(),
            })
        })
        .collect()
}

fn picture_side(row: &Row, before: bool) -> String {
    let blocks = if before { &row.ours } else { &row.theirs };
    let mut boxes = preview_boxes(row, blocks);
    if boxes.is_empty() {
        return String::new();
    }
    let page = boxes[0].placement.page;
    boxes.retain(|item| item.placement.page == page);
    let side = if before { "Before" } else { "After" };
    let svg = preview_svg(&boxes, &format!("{side} picture page {page}"));
    format!(
        "<figure class=\"picture-preview-figure\"><figcaption>{side} · page {page}</figcaption>{svg}</figure>\n"
    )
}

/// A page outline with every block on it, in the page's own point coordinates.
fn preview_svg(boxes: &[PreviewBox], label: &str) -> String {
    let width = boxes[0].placement.width;
    let height = boxes[0].placement.height;
    let mut out = format!(
        "<svg class=\"page-preview\" viewBox=\"0 0 {width:.2} {height:.2}\" role=\"img\" aria-label=\"{}\">",
        escape(label)
    );
    out.push_str(&format!(
        "<rect class=\"preview-sheet\" x=\"0\" y=\"0\" width=\"{width:.2}\" height=\"{height:.2}\"/>"
    ));
    for item in boxes {
        let placement = item.placement;
        let x = placement.x * placement.width;
        let y = placement.y * placement.height;
        let w = placement.w * placement.width;
        let h = placement.h * placement.height;
        let kind = if item.kind == Kind::Picture {
            " picture"
        } else {
            ""
        };
        let title = if item.detail.is_empty() {
            item.label.clone()
        } else {
            format!("{} · {}", item.label, item.detail)
        };
        out.push_str(&format!(
            "<rect class=\"preview-box {}{}\" x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\"><title>{}</title></rect>",
            item.class,
            kind,
            escape(&title)
        ));
    }
    out.push_str("</svg>");
    out
}

/// The words of one side of a row, with a blank line between blocks when the
/// other reading cut one block into several.
fn text_of(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A table's pipe-delimited fallback form, for a block built without its grid.
fn pipe_table_rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                vec![String::new()]
            } else {
                line.split(" | ").map(str::to_string).collect()
            }
        })
        .collect()
}

/// Render both sides of a table, diffing one cell at a time.
///
/// A merged cell is written once with its `rowspan` and `colspan`; the slots it
/// covers are `None` in the grid and produce no cell. Missing rows and cells
/// are still blank cells, so the columns stay under the values they describe.
/// Rows are aligned by their words rather than by their index, so inserting a
/// row does not move every later row's changed-cell markers onto its neighbour.
fn table_diff(old: &TableGrid, new: &TableGrid) -> (String, String) {
    let columns = old.columns.max(new.columns);
    if old.rows.is_empty() && new.rows.is_empty() || columns == 0 {
        return (empty(), empty());
    }
    let mut before = String::from("<table class=\"diff-table\">");
    let mut after = String::from("<table class=\"diff-table\">");
    for (old_row, new_row) in table_row_alignment(old, new) {
        before.push_str("<tr>");
        after.push_str("<tr>");
        for column in 0..columns {
            let old_cell = old_row.and_then(|row| old.cell(row, column));
            let new_cell = new_row.and_then(|row| new.cell(row, column));
            if old_cell.is_none() && new_cell.is_none() {
                continue;
            }
            let (old_markup, new_markup) = cell_diff(
                old_cell.map(|cell| cell.text.as_str()),
                new_cell.map(|cell| cell.text.as_str()),
            );
            if let Some(cell) = old_cell {
                before.push_str(&table_cell_html(cell, &old_markup));
            }
            if let Some(cell) = new_cell {
                after.push_str(&table_cell_html(cell, &new_markup));
            }
        }
        before.push_str("</tr>");
        after.push_str("</tr>");
    }
    before.push_str("</table>");
    after.push_str("</table>");
    (before, after)
}

/// Align the two tables' rows, keeping unchanged rows on the same value when a
/// row is inserted or removed.
///
/// Exact rows are the anchors. The rows between two anchors are then paired in
/// order where they resemble one another, so a changed row still gets a word
/// diff and a genuinely new row is marked as a whole-row insertion. The row
/// count product is capped before the quadratic table is allocated; a table too
/// large to align falls back to positional rows.
fn table_row_alignment(old: &TableGrid, new: &TableGrid) -> Vec<(Option<usize>, Option<usize>)> {
    let old_rows = old.rows.len();
    let new_rows = new.rows.len();
    if old_rows == 0 || new_rows == 0 {
        return (0..old_rows.max(new_rows))
            .map(|row| {
                (
                    (row < old_rows).then_some(row),
                    (row < new_rows).then_some(row),
                )
            })
            .collect();
    }
    if old_rows.saturating_mul(new_rows) > MAX_TABLE_ROW_ALIGNMENT {
        return (0..old_rows.max(new_rows))
            .map(|row| {
                (
                    (row < old_rows).then_some(row),
                    (row < new_rows).then_some(row),
                )
            })
            .collect();
    }
    let old_keys = old
        .rows
        .iter()
        .map(|row| table_row_key(row))
        .collect::<Vec<_>>();
    let new_keys = new
        .rows
        .iter()
        .map(|row| table_row_key(row))
        .collect::<Vec<_>>();
    let mut lcs = vec![vec![0usize; new_rows + 1]; old_rows + 1];
    for old_row in (0..old_rows).rev() {
        for new_row in (0..new_rows).rev() {
            lcs[old_row][new_row] = if old_keys[old_row] == new_keys[new_row] {
                lcs[old_row + 1][new_row + 1] + 1
            } else {
                lcs[old_row + 1][new_row].max(lcs[old_row][new_row + 1])
            };
        }
    }
    let mut anchors = Vec::new();
    let (mut old_row, mut new_row) = (0, 0);
    while old_row < old_rows && new_row < new_rows {
        if old_keys[old_row] == new_keys[new_row] {
            anchors.push((old_row, new_row));
            old_row += 1;
            new_row += 1;
        } else if lcs[old_row + 1][new_row] >= lcs[old_row][new_row + 1] {
            old_row += 1;
        } else {
            new_row += 1;
        }
    }
    let mut pairs = Vec::new();
    let (mut old_start, mut new_start) = (0, 0);
    for (anchor_old, anchor_new) in anchors
        .into_iter()
        .chain(std::iter::once((old_rows, new_rows)))
    {
        align_table_row_gap(
            &mut pairs, old, new, old_start, anchor_old, new_start, anchor_new,
        );
        if anchor_old < old_rows && anchor_new < new_rows {
            pairs.push((Some(anchor_old), Some(anchor_new)));
        }
        old_start = anchor_old + 1;
        new_start = anchor_new + 1;
    }
    pairs
}

/// The rows between two exact anchors, paired in order. A row is paired with a
/// later neighbour when that neighbour is the one whose values match; the rows
/// skipped over are insertions or removals of their own.
fn align_table_row_gap(
    pairs: &mut Vec<(Option<usize>, Option<usize>)>,
    old: &TableGrid,
    new: &TableGrid,
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
) {
    let (mut old_row, mut new_row) = (old_start, new_start);
    while old_row < old_end && new_row < new_end {
        if table_rows_similar(&old.rows[old_row], &new.rows[new_row]) {
            pairs.push((Some(old_row), Some(new_row)));
            old_row += 1;
            new_row += 1;
        } else if old_row + 1 < old_end
            && table_rows_similar(&old.rows[old_row + 1], &new.rows[new_row])
        {
            pairs.push((Some(old_row), None));
            old_row += 1;
        } else if new_row + 1 < new_end
            && table_rows_similar(&old.rows[old_row], &new.rows[new_row + 1])
        {
            pairs.push((None, Some(new_row)));
            new_row += 1;
        } else {
            pairs.push((Some(old_row), Some(new_row)));
            old_row += 1;
            new_row += 1;
        }
    }
    while old_row < old_end {
        pairs.push((Some(old_row), None));
        old_row += 1;
    }
    while new_row < new_end {
        pairs.push((None, Some(new_row)));
        new_row += 1;
    }
}

fn table_row_key(row: &[Option<TableGridCell>]) -> Vec<(usize, String, usize, usize)> {
    row.iter()
        .enumerate()
        .filter_map(|(column, cell)| {
            cell.as_ref()
                .map(|cell| (column, cell.text.clone(), cell.row_span, cell.col_span))
        })
        .collect()
}

/// Whether two unanchored rows describe the same row with a cell changed: at
/// least half of the non-empty values are still the same. A wholly different row
/// is left to be marked as an insertion or a removal rather than paired.
fn table_rows_similar(old: &[Option<TableGridCell>], new: &[Option<TableGridCell>]) -> bool {
    let old_texts = old
        .iter()
        .flatten()
        .map(|cell| cell.text.as_str())
        .collect::<Vec<_>>();
    let new_texts = new
        .iter()
        .flatten()
        .map(|cell| cell.text.as_str())
        .collect::<Vec<_>>();
    let same = old_texts
        .iter()
        .zip(new_texts.iter())
        .filter(|(old, new)| !old.is_empty() && old == new)
        .count();
    let non_empty = old_texts
        .iter()
        .filter(|text| !text.is_empty())
        .count()
        .max(new_texts.iter().filter(|text| !text.is_empty()).count());
    non_empty == 0 || same.saturating_mul(2) >= non_empty
}

/// The largest old×new row rectangle worth aligning. Beyond it, positional rows
/// keep the report finite rather than allocating an enormous table.
const MAX_TABLE_ROW_ALIGNMENT: usize = 1_000_000;

/// One table cell, with the spans its layout declares.
fn table_cell_html(cell: &TableGridCell, text: &str) -> String {
    let mut out = String::from("<td");
    if cell.row_span > 1 {
        out.push_str(&format!(" rowspan=\"{}\"", cell.row_span));
    }
    if cell.col_span > 1 {
        out.push_str(&format!(" colspan=\"{}\"", cell.col_span));
    }
    out.push('>');
    out.push_str(text);
    out.push_str("</td>");
    out
}

/// One row/column position of two tables, with its own word diff.
fn cell_diff(old: Option<&str>, new: Option<&str>) -> (String, String) {
    match (old, new) {
        (Some(old), Some(new)) if old.is_empty() && new.is_empty() => {
            (String::new(), String::new())
        }
        (Some(old), Some(new)) => word_diff(old, new),
        (Some(old), None) => (marked_cell(old, "del"), String::new()),
        (None, Some(new)) => (String::new(), marked_cell(new, "ins")),
        (None, None) => (String::new(), String::new()),
    }
}

fn marked_cell(text: &str, tag: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!("<{tag}>{}</{tag}>", escape(text))
    }
}

/// A contained row whose words differ is a content change.
///
/// `Contained` is the Compare engine's shape for "a reference item is a run of
/// our words" — useful when the reference read the same paragraph at a finer
/// grain, wrong when the new version simply deleted the tail of a block. The
/// alignment stays exactly as the engine left it; only the verdict is promoted,
/// and the old note (which called the words the same) is replaced with the fact.
fn promote_changed_containment(rows: &mut [Row]) {
    for row in rows {
        if row.verdict != Verdict::Contained {
            continue;
        }
        let old = normalised(&text_of(&row.ours));
        let new = normalised(&text_of(&row.theirs));
        if old != new {
            row.verdict = Verdict::Differs;
            row.note = Some("aligned block, but its words changed".to_string());
        }
    }
}

fn normalised(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_change(row: &Row) -> bool {
    matches!(
        row.verdict,
        Verdict::Differs
            | Verdict::Notation
            | Verdict::OursOnly
            | Verdict::TheirsOnly
            | Verdict::Placed
    )
}

fn anchor(section: &Section) -> String {
    match (section.old, section.new) {
        (Some(old), Some(new)) if old == new => format!("page-{old}"),
        (Some(old), Some(new)) => format!("page-{old}-to-{new}"),
        (Some(old), None) => format!("page-{old}-removed"),
        (None, Some(new)) => format!("page-{new}-added"),
        (None, None) => "flow".to_string(),
    }
}

fn page_label(section: &Section) -> String {
    match (section.old, section.new) {
        (Some(old), Some(new)) if old == new => format!("Page {old}"),
        (Some(old), Some(new)) => format!("Page {old} → {new}"),
        (Some(old), None) => format!("Page {old} (removed)"),
        (None, Some(new)) => format!("Page {new} (added)"),
        (None, None) => "Document flow".to_string(),
    }
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "text",
        Kind::Table => "table",
        Kind::Picture => "picture",
    }
}

fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Same => "unchanged",
        Verdict::Notation => "notation differs",
        Verdict::Differs => "changed",
        Verdict::Contained => "same words, different blocks",
        Verdict::OursOnly => "removed",
        Verdict::TheirsOnly => "added",
        Verdict::Placed => "placed",
    }
}

fn empty() -> String {
    "<span class=\"empty\">—</span>".to_string()
}

fn plain_or_empty(text: &str, class: &str) -> String {
    if text.is_empty() {
        empty()
    } else {
        format!("<span class=\"{class}\">{}</span>", escape(text))
    }
}

/// Cut two texts into words and mark what left and what arrived.
///
/// The result is `(before, after)`: the old side holds the unchanged words and
/// the deleted ones, the new side the unchanged words and the inserted ones.
/// That is what a two-column redline needs; a single interleaved stream would
/// put a deletion and its replacement in one cell and lose the comparison.
fn word_diff(old: &str, new: &str) -> (String, String) {
    let old_words = words(old);
    let new_words = words(new);
    // The rows are clipped by the comparison engine, but a table can still be
    // hundreds of words; a quadratic table on a pathological block is not worth
    // a browser's patience. Above this, the whole sides are marked instead.
    if old_words.len().saturating_mul(new_words.len()) > 250_000 {
        return (
            format!("<del>{}</del>", escape(old)),
            format!("<ins>{}</ins>", escape(new)),
        );
    }
    let mut lcs = vec![vec![0usize; new_words.len() + 1]; old_words.len() + 1];
    for i in (0..old_words.len()).rev() {
        for j in (0..new_words.len()).rev() {
            lcs[i][j] = if old_words[i] == new_words[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut pieces = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < old_words.len() && j < new_words.len() {
        if old_words[i] == new_words[j] {
            push_piece(&mut pieces, Piece::Same(old_words[i].clone()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            push_piece(&mut pieces, Piece::Deleted(old_words[i].clone()));
            i += 1;
        } else {
            push_piece(&mut pieces, Piece::Inserted(new_words[j].clone()));
            j += 1;
        }
    }
    while i < old_words.len() {
        push_piece(&mut pieces, Piece::Deleted(old_words[i].clone()));
        i += 1;
    }
    while j < new_words.len() {
        push_piece(&mut pieces, Piece::Inserted(new_words[j].clone()));
        j += 1;
    }

    let mut before = String::new();
    let mut after = String::new();
    for piece in pieces {
        match piece {
            Piece::Same(word) => {
                before.push_str(&escape(&word));
                after.push_str(&escape(&word));
            }
            Piece::Deleted(word) => push_marked(&mut before, "del", &word),
            Piece::Inserted(word) => push_marked(&mut after, "ins", &word),
        }
    }
    if before.is_empty() {
        before = empty();
    }
    if after.is_empty() {
        after = empty();
    }
    (before, after)
}

#[derive(Clone)]
enum Piece {
    Same(String),
    Deleted(String),
    Inserted(String),
}

/// Push one marked word run, keeping its leading whitespace outside the mark.
///
/// A deleted or inserted phrase often begins with the space that separated it
/// from the preceding unchanged word. Highlighting that space makes the two
/// runs look joined at the seam in the rendered report, so the space is emitted
/// before the `<del>`/`<ins>` element instead.
fn push_marked(out: &mut String, tag: &str, word: &str) {
    let trimmed = word.trim_start();
    out.push_str(&word[..word.len() - trimmed.len()]);
    if !trimmed.is_empty() {
        out.push_str(&format!("<{tag}>{}</{tag}>", escape(trimmed)));
    }
}

fn push_piece(pieces: &mut Vec<Piece>, piece: Piece) {
    match (pieces.last_mut(), piece) {
        (Some(Piece::Same(last)), Piece::Same(word))
        | (Some(Piece::Deleted(last)), Piece::Deleted(word))
        | (Some(Piece::Inserted(last)), Piece::Inserted(word)) => last.push_str(&word),
        (_, piece) => pieces.push(piece),
    }
}

/// Words and the whitespace between them, so a redline keeps its spacing when
/// only one word in a sentence changed.
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut whitespace: Option<bool> = None;
    for (index, character) in text.char_indices() {
        let is_space = character.is_whitespace();
        if whitespace.is_some_and(|was| was != is_space) {
            out.push(text[start..index].to_string());
            start = index;
        }
        whitespace = Some(is_space);
    }
    if start < text.len() {
        out.push(text[start..].to_string());
    }
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
    out
}

/// A small accessor so the row header can name a block's own label without
/// exposing the private geometry the comparison keeps on `Block`.
trait KindLabel {
    fn kind_label(&self) -> String;
}

impl KindLabel for Row {
    fn kind_label(&self) -> String {
        self.ours
            .first()
            .or_else(|| self.theirs.first())
            .map(|block| block.label.clone())
            .unwrap_or_else(|| kind_label(self.kind).to_string())
    }
}

const STYLE: &str = r#"
:root {
  color-scheme: light;
  --ink: #18222d;
  --muted: #5a6875;
  --line: #d8dee5;
  --paper: #f6f7f9;
  --panel: #ffffff;
  --add: #e8f5ec;
  --add-ink: #11653a;
  --del: #fdecec;
  --del-ink: #9d1c1c;
  --change: #fff7e2;
  --change-ink: #7a5100;
  --accent: #245c8a;
  --radius: 10px;
}
* { box-sizing: border-box; }
body {
  margin: 0 auto;
  max-width: 1380px;
  padding: 28px clamp(16px, 4vw, 44px) 56px;
  font: 15px/1.55 ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
  color: var(--ink);
  background: var(--paper);
}
.masthead { padding: 6px 0 22px; border-bottom: 1px solid var(--line); }
.eyebrow { margin: 0 0 6px; color: var(--accent); font-size: 12px; font-weight: 700; letter-spacing: .12em; text-transform: uppercase; }
h1 { margin: 0; font-size: clamp(24px, 3.4vw, 40px); line-height: 1.15; overflow-wrap: anywhere; }
h1 .against { color: var(--muted); font-size: .55em; font-weight: 500; padding: 0 .2em; }
.meta { margin: 10px 0 0; color: var(--muted); }
.summary { display: grid; grid-template-columns: repeat(auto-fit, minmax(118px, 1fr)); gap: 10px; margin: 22px 0 0; }
.metric { background: var(--panel); border: 1px solid var(--line); border-top: 3px solid var(--line); border-radius: var(--radius); padding: 10px 12px; }
.metric strong { display: block; font-size: 23px; line-height: 1.2; }
.metric span { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: .04em; }
.metric.changed { border-top-color: #c58a00; }
.metric.added { border-top-color: var(--add-ink); }
.metric.removed { border-top-color: var(--del-ink); }
.metric.placed { border-top-color: #6b5aa6; }
.metric.same { border-top-color: #7b8794; }
.metric.agreement { border-top-color: var(--accent); }
.warning { margin: 12px 0 0; padding: 9px 12px; border-radius: 8px; background: var(--change); color: var(--change-ink); }
.notes { margin: 12px 0 0; color: var(--muted); }
.notes ul { margin: 6px 0 0; padding-left: 20px; }
.pages { display: flex; flex-wrap: wrap; align-items: center; gap: 7px; margin: 22px 0 0; padding: 10px 0 14px; border-bottom: 1px solid var(--line); }
.pages-label { color: var(--muted); font-size: 12px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; margin-right: 4px; }
.pages a { color: var(--accent); text-decoration: none; background: var(--panel); border: 1px solid var(--line); border-radius: 999px; padding: 5px 10px; font-size: 13px; }
.pages a:hover, .pages a:focus { border-color: var(--accent); }
.pages a span { color: var(--muted); margin-left: 5px; font-size: 11px; }
main { padding-top: 8px; }
.page { padding: 26px 0 8px; border-bottom: 1px solid var(--line); }
.page-head { display: flex; align-items: baseline; justify-content: space-between; gap: 16px; margin-bottom: 14px; }
.page-head h2 { margin: 0; font-size: 21px; }
.page-head span { color: var(--muted); font-size: 13px; }
.quiet { color: var(--muted); font-style: italic; margin: 0 0 12px; }
.row { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); background: var(--panel); border: 1px solid var(--line); border-left: 4px solid #7b8794; border-radius: var(--radius); margin: 0 0 14px; overflow: hidden; break-inside: avoid; }
.row.differs { border-left-color: #c58a00; }
.row.added { border-left-color: var(--add-ink); }
.row.removed { border-left-color: var(--del-ink); }
.row.notation { border-left-color: #6b5aa6; }
.row.placed { border-left-color: #6b5aa6; }
.row.same, .row.contained { border-left-color: #9aa5b1; }
.row-head { grid-column: 1 / -1; display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 8px 13px; border-bottom: 1px solid var(--line); background: #fbfcfd; font-size: 12px; }
.kind { font-weight: 700; text-transform: uppercase; letter-spacing: .06em; color: var(--muted); }
.label { color: var(--muted); }
.verdict { margin-left: auto; border-radius: 999px; padding: 2px 8px; background: #eef1f4; color: var(--muted); font-weight: 700; }
.row.differs .verdict { background: var(--change); color: var(--change-ink); }
.row.added .verdict { background: var(--add); color: var(--add-ink); }
.row.removed .verdict { background: var(--del); color: var(--del-ink); }
.moved { color: var(--change-ink); background: var(--change); padding: 2px 7px; border-radius: 999px; }
.page-previews, .picture-previews { grid-column: 1 / -1; display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); align-items: start; gap: 12px; padding: 12px 14px; border-top: 1px dashed var(--line); background: #fbfcfd; }
.page-previews { background: #f7f9fb; }
.preview-note { grid-column: 1 / -1; margin: 0; color: var(--muted); font-size: 12px; }
.page-preview-figure, .picture-preview-figure { margin: 0; padding: 8px; background: var(--panel); border: 1px solid var(--line); border-radius: 8px; }
.page-previews .page-preview-figure:only-of-type, .picture-previews .picture-preview-figure:only-of-type { grid-column: 1 / -1; }
.page-preview-figure figcaption, .picture-preview-figure figcaption { margin: 0 0 6px; color: var(--muted); font-size: 11px; font-weight: 700; letter-spacing: .05em; text-transform: uppercase; }
.page-preview { display: block; width: auto; height: 300px; max-width: 100%; margin: 0 auto; background: #fff; }
.preview-sheet { fill: #fff; stroke: var(--line); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.preview-box { fill: rgba(36, 92, 138, .04); stroke: #5f7f9c; stroke-width: 1; vector-effect: non-scaling-stroke; }
.preview-box.differs, .preview-box.notation { fill: rgba(197, 138, 0, .14); stroke: #c58a00; stroke-width: 1.5; }
.preview-box.added, .preview-box.placed { fill: rgba(17, 101, 58, .12); stroke: var(--add-ink); stroke-width: 1.5; }
.preview-box.removed { fill: rgba(157, 28, 28, .12); stroke: var(--del-ink); stroke-width: 1.5; }
.preview-box.same, .preview-box.contained { stroke: #6f8aa2; }
.preview-box.picture { stroke: #7e22ce; stroke-width: 1.8; stroke-dasharray: 4 2; fill: rgba(126, 34, 206, .1); }
.side { min-width: 0; padding: 12px 14px 14px; }
.side h3 { margin: 0 0 6px; color: var(--muted); font-size: 11px; letter-spacing: .1em; text-transform: uppercase; }
.text { white-space: pre-wrap; overflow-wrap: anywhere; }
.text table.diff-table { width: 100%; border-collapse: collapse; table-layout: fixed; white-space: normal; }
.text table.diff-table td { border: 1px solid var(--line); padding: 5px 7px; vertical-align: top; white-space: pre-wrap; overflow-wrap: anywhere; }
.row .side + .side { border-left: 1px solid var(--line); }
del { background: var(--del); color: var(--del-ink); text-decoration: line-through; text-decoration-thickness: 1px; }
ins { background: var(--add); color: var(--add-ink); text-decoration: none; }
.empty { color: #a7b0ba; }
.row-note { grid-column: 1 / -1; margin: 0; padding: 8px 14px 10px; border-top: 1px dashed var(--line); color: var(--muted); font-size: 13px; }
.unchanged { margin: 12px 0 0; color: var(--muted); }
.unchanged summary { cursor: pointer; font-weight: 600; padding: 6px 0; }
.unchanged .row { margin-top: 10px; }
.empty-report { color: var(--muted); padding: 24px 0; }
.foot { margin-top: 30px; color: var(--muted); font-size: 12px; }
@media (max-width: 760px) {
  .row { grid-template-columns: minmax(0, 1fr); }
  .row .side + .side { border-left: 0; border-top: 1px solid var(--line); }
  .page-previews, .picture-previews { grid-template-columns: minmax(0, 1fr); }
  .page-preview { width: 100%; height: auto; max-height: 300px; }
  .text table.diff-table { font-size: 14px; }
  .text table.diff-table td { padding: 4px 5px; }
  .page-head { display: block; }
  .page-head span { display: block; margin-top: 3px; }
}
@media print {
  body { max-width: none; padding: 0; background: #fff; }
  .pages { display: none; }
  .row, .metric, .page-preview-figure, .picture-preview-figure { break-inside: avoid; }
  .unchanged { display: none; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_diff_marks_only_the_changed_words() {
        let (before, after) = word_diff("The quick brown fox", "The quick red fox");
        assert!(before.contains("<del>brown</del>"), "{before}");
        assert!(after.contains("<ins>red</ins>"), "{after}");
        assert!(before.contains("The quick"), "{before}");
        assert!(after.contains("The quick"), "{after}");
        assert!(!before.contains("<ins>"), "{before}");
        assert!(!after.contains("<del>"), "{after}");
    }

    #[test]
    fn word_diff_keeps_the_spacing_between_unchanged_words() {
        let (before, after) = word_diff("a b c", "a x c");
        assert_eq!(before, "a <del>b</del> c");
        assert_eq!(after, "a <ins>x</ins> c");
    }

    #[test]
    fn word_diff_keeps_a_leading_space_outside_the_mark() {
        let (before, _) = word_diff("Supply chain Supply remains tight.", "Supply chain");
        assert_eq!(before, "Supply chain <del>Supply remains tight.</del>");
    }

    #[test]
    fn word_diff_marks_a_whole_side_when_nothing_is_shared() {
        let (before, after) = word_diff("old words", "new words");
        assert!(before.contains("<del>old</del>"), "{before}");
        assert!(after.contains("<ins>new</ins>"), "{after}");
        assert!(before.contains("words"), "{before}");
        assert!(after.contains("words"), "{after}");
    }

    #[test]
    fn html_text_is_escaped() {
        assert_eq!(
            escape("<script>alert('&')</script>"),
            "&lt;script&gt;alert(&#39;&amp;&#39;)&lt;/script&gt;"
        );
    }

    #[test]
    fn page_labels_name_both_sides_of_a_move() {
        assert_eq!(
            page_label(&Section {
                old: Some(3),
                new: Some(5),
                rows: Vec::new()
            }),
            "Page 3 → 5"
        );
        assert_eq!(
            page_label(&Section {
                old: Some(3),
                new: None,
                rows: Vec::new()
            }),
            "Page 3 (removed)"
        );
        assert_eq!(
            page_label(&Section {
                old: None,
                new: None,
                rows: Vec::new()
            }),
            "Document flow"
        );
    }
}
