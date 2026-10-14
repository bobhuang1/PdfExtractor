//! The Compare mode against the real corpus.
//!
//! These tests run the CLI over documents from `test-data/real` and compare what
//! it wrote with the committed Docling references in `test-data/golden/docling`,
//! through the very function the window calls. The unit tests in `diff` cover the
//! alignment and its verdicts on documents built by hand; this is the check that
//! the mode reads our output and a real reference, and that the numbers it shows
//! mean what the panel says they mean.
//!
//! They skip (loudly) when the CLI has not been built, the way the golden suite
//! skips without `DOCLING_PY`: what is being tested is the window's comparison,
//! not the extractor, and the extractor has its own suites.

use pdfextractor_ui::diff;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The command line binary, or `None` when it has not been built.
fn locate_cli() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PDFEXTRACTOR_CLI") {
        return Some(PathBuf::from(path));
    }
    let candidate = repository_root().join("target/debug/pdfextractor-cli");
    candidate.exists().then_some(candidate)
}

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("pdfe-ui-compare-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Convert one corpus document and compare it with the committed reference.
///
/// `None` when the CLI is not built, so a caller can skip.
fn convert_and_compare(name: &str, scratch: &Scratch) -> Option<diff::Report> {
    let cli = locate_cli()?;
    let root = repository_root();
    let input = root.join("test-data/real").join(name);
    assert!(input.exists(), "missing corpus file {}", input.display());
    let ours = scratch.0.join(format!("{name}.json"));

    let output = Command::new(&cli)
        .args(["--input", &input.to_string_lossy()])
        .args(["--output", &ours.to_string_lossy()])
        .args(["--ocr", "never"])
        .output()
        .expect("run the CLI");
    assert!(
        output.status.success(),
        "converting {name} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(ours.exists(), "{name} produced no output");

    Some(diff::compare_document(
        &input.to_string_lossy(),
        &ours,
        &root.join("test-data/golden/docling"),
    ))
}

/// The box a placed row's note names, in points: `… box 396,490–590,497: …`.
fn note_box(note: &str) -> Option<(f64, f64, f64, f64)> {
    let rest = note.split("box ").nth(1)?;
    let box_ = rest.split(':').next()?;
    let (into, _) = box_.split_once('–')?;
    let (left, top) = into.split_once(',')?;
    let (right, bottom) = box_.split_once('–')?.1.split_once(',')?;
    Some((
        left.trim().parse().ok()?,
        top.trim().parse().ok()?,
        right.trim().parse().ok()?,
        bottom.trim().parse().ok()?,
    ))
}

/// Print what a comparison found, so a failure can be read without re-running.
fn show(name: &str, report: &diff::Report) {
    println!(
        "{name}: object {} rows, agreement {:.3}, pages {}/{}, truncated {}",
        report.rows.len(),
        report.summary.text_agreement,
        report.summary.pages_ours,
        report.summary.pages_theirs,
        report.summary.truncated
    );
    println!(
        "    same {} · notation {} · differs {} · only ours {} · only the reference {} · paired by position {}",
        report.summary.same,
        report.summary.notation,
        report.summary.differs,
        report.summary.ours_only,
        report.summary.theirs_only,
        report.summary.paired_by_position
    );
    for note in &report.summary.notes {
        println!("    note: {note}");
    }
    for row in report
        .rows
        .iter()
        .filter(|row| row.verdict != diff::Verdict::Same)
    {
        println!(
            "    {:?} {:?}: {} / {} ({:?})",
            row.verdict,
            row.kind,
            row.ours.first().map_or("-".to_string(), |b| b.text.clone()),
            row.theirs
                .first()
                .map_or("-".to_string(), |b| b.text.clone()),
            row.note
        );
    }
}

/// The equations of `math-nary.docx`: Docling writes three, in LaTeX, and we
/// write the same three in Unicode. Both readings have the equations, so the
/// panel says "notation" three times rather than "differs" or "missing".
#[test]
fn three_equations_are_three_notation_rows() {
    let scratch = Scratch::new("nary");
    let Some(report) = convert_and_compare("math-nary.docx", &scratch) else {
        println!("skipped: build the CLI (`cargo build -p pdfextractor-cli`) to run this");
        return;
    };
    show("math-nary", &report);

    assert_eq!(report.problem, None, "{:?}", report.problem);
    assert_eq!(report.summary.notation, 3, "three equations are three rows");
    assert_eq!(report.summary.differs, 0);
    assert_eq!(report.summary.ours_only, 0);
    assert_eq!(report.summary.theirs_only, 0);
    assert_eq!(report.summary.text_agreement, 1.0);
}

/// `VariousPictures.docx` is the document whose drawings this project rasterises:
/// a PNG, a JPEG and three metafiles. Docling renders the same five, so every
/// picture the reference has must have one of ours beside it — the failure this
/// guards is a drawing reported as a box with no image, which would show as a
/// picture only the reference has.
///
/// The rows are differences of *scale*, not of reading: the two tools publish
/// their own dpi, and a metafile rasterised from its declared size lands within
/// a point or two of Docling's render of the same drawing. Asserting the pairing
/// rather than the verdict is the point: a picture row that is paired is a
/// picture both sides found, whatever size they found it at.
#[test]
fn every_reference_picture_is_answered_by_one_of_ours() {
    let scratch = Scratch::new("pictures");
    let Some(report) = convert_and_compare("VariousPictures.docx", &scratch) else {
        println!("skipped: build the CLI (`cargo build -p pdfextractor-cli`) to run this");
        return;
    };
    show("VariousPictures", &report);

    let pictures: Vec<&diff::Row> = report
        .rows
        .iter()
        .filter(|row| row.kind == diff::Kind::Picture)
        .collect();
    assert_eq!(
        pictures.len(),
        5,
        "expected five picture rows: {pictures:#?}"
    );
    for row in pictures {
        assert!(
            !row.ours.is_empty() && !row.theirs.is_empty(),
            "a picture is missing a counterpart: {row:#?}"
        );
        assert!(
            row.ours[0].text.contains("image/"),
            "our side of a drawing must be an image: {row:#?}"
        );
    }
    // The five one-sided rows are the reference's empty text items — the items
    // Docling writes around a picture area with no characters in them. They carry
    // nothing to compare, so they are shown as the reference's own reading rather
    // than quietly dropped; nothing else may be one-sided.
    let one_sided: Vec<&diff::Row> = report
        .rows
        .iter()
        .filter(|row| {
            matches!(
                row.verdict,
                diff::Verdict::OursOnly | diff::Verdict::TheirsOnly
            )
        })
        .collect();
    assert_eq!(one_sided.len(), 5, "{one_sided:#?}");
    for row in one_sided {
        let block = row
            .theirs
            .first()
            .expect("only the reference's side is lonely");
        assert_eq!(block.kind, diff::Kind::Text);
        assert!(
            block.text.is_empty(),
            "a one-sided row must be empty: {row:#?}"
        );
    }
    assert_eq!(report.summary.text_agreement, 1.0);
}

/// `mathtype.docx` holds one equation as a binary OLE object, whose *preview* is
/// the only picture of the equation either side has. The two readings disagree
/// about where it sits — Docling puts the preview before the text, we put it
/// after — and an alignment never pairs across a crossing, so each side's picture
/// stands alone beside the same text. Both rows carry the size each tool read the
/// preview at, which is what tells a reader they are the same drawing.
#[test]
fn an_embedded_equations_preview_is_shown_by_both_sides() {
    let scratch = Scratch::new("mathtype");
    let Some(report) = convert_and_compare("mathtype.docx", &scratch) else {
        println!("skipped: build the CLI (`cargo build -p pdfextractor-cli`) to run this");
        return;
    };
    show("mathtype", &report);

    assert_eq!(report.problem, None, "{:?}", report.problem);
    assert_eq!(
        report.summary.same, 1,
        "the prose is the same on both sides"
    );
    assert_eq!(report.summary.differs, 0, "{:#?}", report.rows);

    let pictures: Vec<&diff::Row> = report
        .rows
        .iter()
        .filter(|row| row.kind == diff::Kind::Picture)
        .collect();
    assert_eq!(pictures.len(), 2, "{pictures:#?}");
    assert_eq!(pictures[0].verdict, diff::Verdict::TheirsOnly);
    assert_eq!(pictures[1].verdict, diff::Verdict::OursOnly);
    let ours = &pictures[1].ours[0];
    assert!(
        ours.text.contains("image/png"),
        "our side must be the rasterised preview: {ours:#?}"
    );
    assert_eq!(report.summary.text_agreement, 1.0);
}

/// The three corpus documents whose references carry geometry: Docling's PDF
/// backend publishes a page and a box for every item, and so does ours, which is
/// what lets the panel join two readings of the same thing that the two tools
/// put in a different order. Flow formats (every DOCX here) publish none, so
/// this is where the rule can be seen working on real documents at all.
#[test]
fn pdfs_are_compared_with_the_boxes_both_producers_published() {
    let scratch = Scratch::new("pdf-boxes");
    let mut joined = 0usize;
    let mut placed = 0usize;
    let mut placed_items = 0usize;
    let mut page_pictures = 0usize;
    let mut placed_boxes = 0usize;
    let mut compared = 0usize;
    for name in ["tracemonkey.pdf", "arxiv_attention.pdf", "irs_f1040.pdf"] {
        let Some(report) = convert_and_compare(name, &scratch) else {
            println!("skipped: build the CLI (`cargo build -p pdfextractor-cli`) to run this");
            return;
        };
        show(name, &report);
        assert_eq!(report.problem, None, "{name}: {:?}", report.problem);
        compared += 1;
        joined += report.summary.paired_by_position;

        // Both partitions published boxes, so every block either side found
        // carries the page it was found on...
        for row in &report.rows {
            for block in row.ours.iter().chain(row.theirs.iter()) {
                assert!(
                    block.page.is_some(),
                    "{name}: a block with no page: {row:#?}"
                );
            }
        }
        // ...and a row joined by position is a pair, says so, and is not also
        // counted as two one-sided rows.
        for row in &report.rows {
            let note = row.note.as_deref().unwrap_or_default();
            if !note.contains("paired by position") {
                continue;
            }
            assert!(
                !row.ours.is_empty() && !row.theirs.is_empty(),
                "{name}: a joined row must have both sides: {row:#?}"
            );
            assert!(
                !matches!(
                    row.verdict,
                    diff::Verdict::OursOnly | diff::Verdict::TheirsOnly
                ),
                "{name}: a joined row is not one-sided: {row:#?}"
            );
            assert!(note.contains("page "), "{name}: {note}");
        }
        // And a row that stands for a *place* rather than a comparison is the
        // reference's items with no letters in them at all, standing where our reading
        // read no block: it names the extent they cover, and it is not in either of the
        // one-sided columns. Items of one line of a form are one row — that is the whole
        // of what the count of a form's leaders should cost a reader — so a placed row
        // holds one item or the run of them Docling wrote along one line.
        for row in &report.rows {
            if row.verdict != diff::Verdict::Placed {
                continue;
            }
            placed += 1;
            placed_items += row.theirs.len();
            assert!(
                row.ours.is_empty(),
                "{name}: a placed row has no side of ours: {row:#?}"
            );
            assert!(!row.theirs.is_empty(), "{name}: {row:#?}");
            for (index, item) in row.theirs.iter().enumerate() {
                assert!(
                    !item.text.chars().any(char::is_alphanumeric),
                    "{name}: a placed item has letters in it: {item:#?}"
                );
                assert!(
                    item.page.is_some(),
                    "{name}: a placed item with no page: {item:#?}"
                );
                assert_eq!(
                    item.page, row.theirs[0].page,
                    "{name}: a placed row stands on one page, and item {index} does not: {row:#?}"
                );
            }
            let note = row.note.as_deref().unwrap_or_default();
            assert!(note.contains("no letters"), "{name}: {note}");
            assert!(note.contains("box "), "{name}: {note}");
            assert!(note.contains("page "), "{name}: {note}");
            let placement = row
                .placement
                .unwrap_or_else(|| panic!("{name}: a placed row carries no box: {row:#?}"));
            // The box the note names in points is the one the panel draws in shares of
            // the page — the same extent, read two ways.
            let (left, top, right, bottom) =
                note_box(note).unwrap_or_else(|| panic!("{name}: the note names no box: {note}"));
            let near = |one: f64, other: f64| (one - other).abs() < 0.5;
            let (drawn_left, drawn_top) = (
                placement.x * placement.width,
                placement.y * placement.height,
            );
            let (drawn_right, drawn_bottom) = (
                (placement.x + placement.w) * placement.width,
                (placement.y + placement.h) * placement.height,
            );
            assert!(
                near(drawn_left, left)
                    && near(drawn_top, top)
                    && near(drawn_right, right)
                    && near(drawn_bottom, bottom),
                "{name}: the note names {left},{top}–{right},{bottom} and the panel draws \
                 {drawn_left},{drawn_top}–{drawn_right},{drawn_bottom}: {row:#?}"
            );
            if row.theirs.len() > 1 {
                // A run is one line of one page, and a line is a small share of a page's
                // height: the extent covers the leaders of that line and the rule under
                // them, and nothing that could have been a second line.
                assert!(
                    note.starts_with(&format!(
                        "{} items with no letters in them along one line of page {}",
                        row.theirs.len(),
                        row.theirs[0].page.expect("a placed item stands on a page")
                    )),
                    "{name}: a run names how many items it stands for: {note}"
                );
                assert!(
                    bottom - top <= 0.05 * placement.height,
                    "{name}: a run of {} items covers {} pt of a {} pt page: {note}",
                    row.theirs.len(),
                    bottom - top,
                    placement.height
                );
            }
            // ...and it carries the box the panel *draws*, as fractions of the page it
            // was found on: inside the page's own shape, and never empty, or there is
            // nothing for a reader to see.
            let placement = row
                .placement
                .unwrap_or_else(|| panic!("{name}: a placed row carries no box: {row:#?}"));
            assert_eq!(
                placement.page,
                row.theirs[0].page.expect("a placed item stands on a page"),
                "{name}: {placement:?}"
            );
            assert!(
                placement.width > 100.0 && placement.height > 100.0,
                "{name}: the outline must be the page's own size in points: {placement:?}"
            );
            assert!(
                (0.0..=1.0).contains(&placement.x)
                    && (0.0..=1.0).contains(&placement.y)
                    && placement.w > 0.0
                    && placement.h > 0.0
                    && placement.x + placement.w <= 1.0 + 1e-9
                    && placement.y + placement.h <= 1.0 + 1e-9,
                "{name}: the box must lie inside the page: {placement:?}"
            );
        }
        assert_eq!(
            report.summary.placed,
            report
                .rows
                .iter()
                .filter(|row| row.verdict == diff::Verdict::Placed)
                .count(),
            "{name}: the placed count must be the placed rows"
        );

        // And the per-page view: the placed items of the document grouped by the page
        // they lie on, so a form's leader pattern is one picture rather than one
        // thumbnail per row. It is built before the rows are cut, so it holds every
        // placed item even where the panel's row list does not, every box lies inside
        // the page it is drawn on, and the pages come in order, one each.
        let placed_items: usize = report
            .rows
            .iter()
            .filter(|row| row.verdict == diff::Verdict::Placed)
            .map(|row| row.theirs.len())
            .sum();
        let drawn: usize = report
            .placed_pages
            .iter()
            .map(|page| page.boxes.len())
            .sum();
        page_pictures += report.placed_pages.len();
        placed_boxes += drawn;
        assert!(
            drawn >= placed_items,
            "{name}: the page view must hold every placed item: {drawn} < {placed_items}"
        );
        if report.summary.truncated == 0 {
            assert_eq!(
                drawn, placed_items,
                "{name}: with no row cut, the picture is every placed item exactly once"
            );
        }
        for (index, page) in report.placed_pages.iter().enumerate() {
            assert!(page.page >= 1, "{name}: {page:#?}");
            if index > 0 {
                assert!(
                    report.placed_pages[index - 1].page < page.page,
                    "{name}: a page appears once, in order: {:#?}",
                    report.placed_pages
                );
            }
            assert!(!page.boxes.is_empty(), "{name}: {page:#?}");
            assert!(
                page.width > 100.0 && page.height > 100.0,
                "{name}: the outline must be the page's own size in points: {page:#?}"
            );
            for box_ in &page.boxes {
                assert!(
                    (0.0..=1.0).contains(&box_.x)
                        && (0.0..=1.0).contains(&box_.y)
                        && box_.w > 0.0
                        && box_.h > 0.0
                        && box_.x + box_.w <= 1.0 + 1e-9
                        && box_.y + box_.h <= 1.0 + 1e-9,
                    "{name}: a box must lie inside the page it is drawn on: {box_:?}"
                );
            }
        }
    }
    assert_eq!(compared, 3);
    // Not a hand-built document: on these three, the two readings really do
    // order some of the same things differently, and only the boxes can say so.
    assert!(
        joined > 0,
        "no row was paired by position in any of the three references"
    );
    // ...and a form's dotted leaders really are answered: `irs_f1040` is the form,
    // and its remaining leaders are places now rather than rows of text we missed.
    assert!(
        placed > 0,
        "no row was placed in any of the three references"
    );
    // ...and those places are drawn together, page by page: fewer pictures than rows,
    // because a line of a form is one row, and a page's lines are one picture.
    assert!(
        page_pictures > 0,
        "no page was drawn for the placed items of the three references"
    );
    assert!(
        placed_boxes >= placed_items,
        "the pictures must hold every placed item: {placed_boxes} < {placed_items}"
    );
    println!(
        "{joined} row(s) paired by position and {placed} placed row(s) standing for \
         {placed_items} item(s), drawn as {page_pictures} page(s) holding {placed_boxes} box(es), \
         across the three references"
    );
}

/// A document the reference folder has nothing for is reported as exactly that,
/// naming the folder it was looked in — a mode that silently shows nothing
/// cannot be told from a mode that is broken, and the corpus is where a reader
/// will have pointed the window when they see it.
#[test]
fn a_document_with_no_reference_says_where_it_looked() {
    let root = repository_root();
    let folder = root.join("test-data/golden/docling");
    let report = diff::compare_document(
        "/docs/not-in-the-corpus.pdf",
        Path::new("/nonexistent/ours.json"),
        &folder,
    );

    let problem = report
        .problem
        .expect("a missing reference must be reported");
    assert!(
        problem.contains("no reference for this document"),
        "{problem}"
    );
    assert!(problem.contains("test-data/golden/docling"), "{problem}");
    assert!(report.rows.is_empty());
    assert_eq!(report.summary.rows, 0);
    assert_eq!(report.reference, folder.display().to_string());
}
