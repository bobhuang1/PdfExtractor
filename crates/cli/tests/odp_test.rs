//! End-to-end tests of the OpenDocument presentation reader.
//!
//! `q3_review_impress.odp` is LibreOffice Impress' conversion of
//! `q3_review.pptx`, so the fixture is what a real producer writes: three
//! slides, a title and bulleted body, a table, a picture, speaker notes, three
//! comments and an embedded bar chart. Its stem is the conversion's own, so it
//! cannot be taken for the deck whose Docling reference is keyed by `q3_review`.
//!
//! `chart_types_impress.odp` is the chart-family fixture: one packaged slide
//! holding a pie, a line with two series, a scatter whose x-values are in its
//! domain, and a bar chart whose cached `local-table` was removed. The
//! end-to-end test holds the first three to their plotted grids and the last to
//! not being invented.
//!
//! `master_animations_impress.odp` carries a master-page background shape, a
//! whole-page bar-wipe transition and two shape effects, so the transition and
//! the effects are checked as separate notes-layer items.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/q3_review_impress.odp")
}

fn master_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/master_animations_impress.odp")
}

fn chart_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/chart_types_impress.odp")
}

struct Workspace {
    path: PathBuf,
}

impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pdfe-odp-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self { path }
    }

    fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }

    fn write(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.join(name);
        std::fs::write(&path, data).expect("write fixture");
        path
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args(args)
        .env("RUST_LOG", "info")
        .output()
        .expect("spawn the CLI binary")
}

fn run_ok(args: &[&str]) -> Output {
    let out = run(args);
    assert!(
        out.status.success(),
        "CLI failed: args={args:?}\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn json_of(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("missing output {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()))
}

fn page_numbers(doc: &serde_json::Value) -> Vec<i64> {
    let mut pages: Vec<i64> = doc["pages"]
        .as_object()
        .expect("a pages map")
        .values()
        .map(|page| page["page_no"].as_i64().expect("a page number"))
        .collect();
    pages.sort_unstable();
    pages
}

fn chart_cells(chart: &serde_json::Value) -> Vec<(u64, u64, &str)> {
    chart["meta"]["tabular_chart"]["chart_data"]["table_cells"]
        .as_array()
        .expect("the chart's cells")
        .iter()
        .map(|cell| {
            (
                cell["start_row_offset_idx"].as_u64().expect("a row"),
                cell["start_col_offset_idx"].as_u64().expect("a column"),
                cell["text"].as_str().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn an_odp_reads_as_slides() {
    let ws = Workspace::new("deck");
    let output = ws.join("q3_review_impress.json");
    run_ok(&[
        "-i",
        fixture().to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
    ]);
    let doc = json_of(&output);

    assert_eq!(doc["name"], "q3_review_impress");
    assert_eq!(
        doc["origin"]["mimetype"],
        "application/vnd.oasis.opendocument.presentation"
    );
    assert_eq!(page_numbers(&doc), [1, 2, 3]);
    assert_eq!(doc["pages"]["1"]["size"]["width"], 9_144_000.0);
    assert_eq!(doc["pages"]["1"]["size"]["height"], 6_858_000.0);

    let groups = doc["groups"].as_array().expect("groups");
    assert_eq!(
        groups
            .iter()
            .filter(|group| group["label"] == "chapter")
            .map(|group| group["name"].as_str().expect("a group name"))
            .collect::<Vec<_>>(),
        ["slide-0", "slide-1", "slide-2"]
    );
    assert_eq!(
        groups
            .iter()
            .filter(|group| group["label"] == "comment_section")
            .map(|group| group["name"].as_str().expect("a group name"))
            .collect::<Vec<_>>(),
        ["comment-slide1-1", "comment-slide1-2", "comment-slide3-1"]
    );

    let texts = doc["texts"].as_array().expect("texts");
    let find = |text: &str| {
        texts
            .iter()
            .find(|item| item["text"] == text)
            .unwrap_or_else(|| panic!("no text item {text:?} in {texts:#?}"))
    };
    assert_eq!(find("Q3 Review")["label"], "title");
    assert_eq!(
        find("Revenue grew across every region.")["label"],
        "list_item"
    );
    assert_eq!(
        find("Revenue grew across every region.")["prov"][0]["page_no"],
        1
    );
    assert_eq!(find("Regional performance")["label"], "title");
    assert_eq!(find("Where the growth came from")["prov"][0]["page_no"], 3);

    let tables = doc["tables"].as_array().expect("tables");
    assert_eq!(tables.len(), 1);
    let data = &tables[0]["data"];
    assert_eq!(data["num_rows"], 4);
    assert_eq!(data["num_cols"], 3);
    let cells = data["table_cells"].as_array().expect("cells");
    let cell = |text: &str| {
        cells
            .iter()
            .find(|cell| cell["text"] == text)
            .unwrap_or_else(|| panic!("no cell {text:?} in {cells:#?}"))
    };
    assert_eq!(cell("Region")["column_header"], true);
    assert_eq!(cell("Q3")["start_col_offset_idx"], 2);
    assert_eq!(cell("145")["start_row_offset_idx"], 1);

    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(pictures.len(), 2, "the picture and the chart");
    let image = &pictures[0]["image"];
    assert_eq!(image["mimetype"], "image/png");
    let uri = image["uri"].as_str().expect("a picture uri");
    assert!(
        ws.join(uri).is_file(),
        "the picture {uri} was not written beside the output"
    );

    let chart = &pictures[1];
    assert_eq!(
        chart["meta"]["classification"]["predictions"][0]["class_name"],
        "bar_chart"
    );
    assert!(chart["image"].is_null(), "a chart was written as an image");
    assert_eq!(chart["prov"][0]["page_no"], 3);
    assert_eq!(find("Units by region")["label"], "caption");

    // The chart's numbers are the data it plots, with its series across the
    // header row and its categories down the first column.
    let chart_data = &chart["meta"]["tabular_chart"]["chart_data"];
    assert_eq!(chart_data["num_rows"], 4);
    assert_eq!(chart_data["num_cols"], 3);
    let chart_cells = chart_data["table_cells"].as_array().expect("cells");
    let chart_cell = |text: &str| {
        chart_cells
            .iter()
            .find(|cell| cell["text"] == text)
            .unwrap_or_else(|| panic!("no chart cell {text:?} in {chart_cells:#?}"))
    };
    assert_eq!(chart_cell("Q2")["start_row_offset_idx"], 0);
    assert_eq!(chart_cell("Q2")["start_col_offset_idx"], 1);
    assert_eq!(chart_cell("145")["start_row_offset_idx"], 1);
    assert_eq!(chart_cell("145")["start_col_offset_idx"], 2);
    assert_eq!(chart_cell("Asia Pacific")["start_row_offset_idx"], 3);
    assert!(
        chart_cells.iter().all(|cell| cell["text"] != "NaN"),
        "a missing plotted point is a gap, not the word for it: {chart_cells:#?}"
    );

    let notes: Vec<&serde_json::Value> = texts
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .collect();
    assert_eq!(notes.len(), 5, "two speaker notes and three comments");
    assert!(notes.iter().any(|note| note["text"]
        .as_str()
        .is_some_and(|text| text.contains("Open with the revenue line"))));
    assert!(notes.iter().any(|note| note["text"]
        .as_str()
        .is_some_and(|text| text.contains("Say the picture is the growth by quarter"))));
    let comment = |text: &str| {
        notes
            .iter()
            .find(|note| note["text"] == text)
            .unwrap_or_else(|| panic!("no comment {text:?} in {notes:#?}"))
    };
    assert_eq!(
        comment(
            "[author: Ada Lovelace, time: 2026-10-18T09:15:00]: \
             Check the Q3 numbers against the ledger."
        )["prov"],
        serde_json::json!([]),
        "a comment is about the slide, not a place on it"
    );
    comment(
        "[author: Grace Hopper, time: 2026-10-18T10:30:00]: \
         Add the regional split to slide 2.",
    );
    comment("[author: Grace Hopper, time: 2026-10-18T16:05:00]: Ship this one.");
}

#[test]
fn chart_families_are_read_as_data_and_a_table_less_chart_is_left_out() {
    let ws = Workspace::new("chart-types");
    let output = ws.join("chart_types_impress.json");
    run_ok(&[
        "-i",
        chart_fixture().to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
        "--to",
        "json",
        "--to",
        "md",
        "--to",
        "html",
    ]);
    let doc = json_of(&output);

    assert_eq!(doc["name"], "chart_types_impress");
    assert_eq!(
        doc["origin"]["mimetype"],
        "application/vnd.oasis.opendocument.presentation"
    );
    assert_eq!(page_numbers(&doc), [1]);
    assert_eq!(doc["pages"]["1"]["size"]["width"], 12_191_760.0);
    assert_eq!(doc["pages"]["1"]["size"]["height"], 6_858_000.0);

    let groups = doc["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["label"], "chapter");
    assert_eq!(groups[0]["name"], "slide-0");

    let texts = doc["texts"].as_array().expect("texts");
    let find = |text: &str| {
        texts
            .iter()
            .find(|item| item["text"] == text)
            .unwrap_or_else(|| panic!("no text item {text:?} in {texts:#?}"))
    };
    assert_eq!(find("Chart families")["label"], "title");
    assert_eq!(find("Revenue by region")["label"], "caption");
    assert_eq!(find("Quarterly trend")["label"], "caption");
    assert_eq!(find("Signal by sample")["label"], "caption");
    assert_eq!(find("Scatter chart with x/y points")["label"], "paragraph");
    assert_eq!(
        find("Chart without a local table")["label"],
        "paragraph",
        "the table-less chart's caption is still slide text"
    );
    assert!(
        texts.iter().all(|item| item["text"] != "No cached data"),
        "the table-less chart's own title was emitted: {texts:#?}"
    );

    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(
        pictures.len(),
        3,
        "the pie, line and scatter are charts; the table-less bar is not: {pictures:#?}"
    );
    assert!(
        pictures.iter().all(|picture| picture["image"].is_null()),
        "a chart was written as an image: {pictures:#?}"
    );
    assert!(
        pictures
            .iter()
            .all(|picture| picture["prov"][0]["page_no"] == 1),
        "a chart was placed on another page: {pictures:#?}"
    );

    let pie = &pictures[0];
    assert_eq!(
        pie["meta"]["classification"]["predictions"][0]["class_name"],
        "pie_chart"
    );
    let pie_data = &pie["meta"]["tabular_chart"]["chart_data"];
    assert_eq!(pie_data["num_rows"], 4);
    assert_eq!(pie_data["num_cols"], 2);
    let pie_cells = chart_cells(pie);
    for cell in [
        (0, 0, ""),
        (0, 1, "Revenue"),
        (1, 0, "North America"),
        (1, 1, "120"),
        (2, 0, "Europe"),
        (2, 1, "98"),
        (3, 0, "Asia Pacific"),
        (3, 1, "74"),
    ] {
        assert!(
            pie_cells.contains(&cell),
            "{cell:?} is missing from the pie: {pie_cells:?}"
        );
    }
    let pie_caption = pie["captions"][0]["$ref"]
        .as_str()
        .expect("the pie's caption");
    assert_eq!(
        texts
            .iter()
            .find(|item| item["self_ref"] == pie_caption)
            .expect("the pie's caption item")["text"],
        "Revenue by region"
    );

    let line = &pictures[1];
    assert_eq!(
        line["meta"]["classification"]["predictions"][0]["class_name"],
        "line_chart"
    );
    let line_data = &line["meta"]["tabular_chart"]["chart_data"];
    assert_eq!(line_data["num_rows"], 4);
    assert_eq!(line_data["num_cols"], 3);
    let line_cells = chart_cells(line);
    for cell in [
        (0, 0, ""),
        (0, 1, "North America"),
        (0, 2, "Europe"),
        (1, 0, "Q2"),
        (1, 1, "120"),
        (1, 2, "98"),
        (2, 0, "Q3"),
        (2, 1, "145"),
        (2, 2, "121"),
        (3, 0, "Q4"),
        (3, 1, "150"),
        (3, 2, "130"),
    ] {
        assert!(
            line_cells.contains(&cell),
            "{cell:?} is missing from the line: {line_cells:?}"
        );
    }
    let line_caption = line["captions"][0]["$ref"]
        .as_str()
        .expect("the line's caption");
    assert_eq!(
        texts
            .iter()
            .find(|item| item["self_ref"] == line_caption)
            .expect("the line's caption item")["text"],
        "Quarterly trend"
    );

    let scatter = &pictures[2];
    assert_eq!(
        scatter["meta"]["classification"]["predictions"][0]["class_name"],
        "scatter_chart"
    );
    let scatter_data = &scatter["meta"]["tabular_chart"]["chart_data"];
    assert_eq!(scatter_data["num_rows"], 4);
    assert_eq!(scatter_data["num_cols"], 2);
    let scatter_cells = chart_cells(scatter);
    for cell in [
        (0, 0, ""),
        (0, 1, "Samples"),
        (1, 0, "1"),
        (1, 1, "2"),
        (2, 0, "2"),
        (2, 1, "4"),
        (3, 0, "3"),
        (3, 1, "3"),
    ] {
        assert!(
            scatter_cells.contains(&cell),
            "{cell:?} is missing from the scatter: {scatter_cells:?}"
        );
    }
    let scatter_caption = scatter["captions"][0]["$ref"]
        .as_str()
        .expect("the scatter's caption");
    assert_eq!(
        texts
            .iter()
            .find(|item| item["self_ref"] == scatter_caption)
            .expect("the scatter's caption item")["text"],
        "Signal by sample"
    );

    // The table-less bar has a frame and a caption but no chart picture, and
    // its own title was not invented from the caption beside it.
    assert!(
        pictures.iter().all(|picture| {
            picture["meta"]["classification"]["predictions"][0]["class_name"] != "bar_chart"
        }),
        "a bar chart was read where its local-table was removed: {pictures:#?}"
    );

    let markdown =
        std::fs::read_to_string(ws.join("chart_types_impress.md")).expect("the Markdown");
    assert!(markdown.contains("Revenue by region"), "{markdown}");
    assert!(markdown.contains("Quarterly trend"), "{markdown}");
    assert!(markdown.contains("Signal by sample"), "{markdown}");
    assert!(
        markdown.contains("Scatter chart with x/y points"),
        "{markdown}"
    );
    assert!(
        markdown.contains("Chart without a local table"),
        "{markdown}"
    );
    assert!(!markdown.contains("No cached data"), "{markdown}");
    assert!(
        !markdown.contains("!["),
        "a chart became an image: {markdown}"
    );

    let html = std::fs::read_to_string(ws.join("chart_types_impress.html")).expect("the HTML");
    assert!(html.contains("Signal by sample"), "{html}");
    assert!(html.contains("Scatter chart with x/y points"), "{html}");
    assert!(!html.contains("<img"), "a chart became an image: {html}");
    assert!(
        !ws.join("chart_types_impress_images").exists(),
        "a chart wrote an image directory"
    );
}

#[test]
fn master_backgrounds_and_animations_are_represented() {
    let ws = Workspace::new("master");
    let output = ws.join("master.json");
    run_ok(&[
        "-i",
        master_fixture().to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
    ]);
    let doc = json_of(&output);

    assert_eq!(page_numbers(&doc), [1, 2]);
    let texts = doc["texts"].as_array().expect("texts");

    // The master page's regular shape is inherited by the slide that shows
    // background objects, and not by the slide that hides them.
    let background: Vec<&serde_json::Value> = texts
        .iter()
        .filter(|item| item["text"] == "Confidential \u{2014} internal use only")
        .collect();
    assert_eq!(
        background.len(),
        1,
        "the master background belongs to exactly one slide: {texts:#?}"
    );
    assert_eq!(background[0]["prov"][0]["page_no"], 1);
    assert!(texts
        .iter()
        .any(|item| item["text"] == "Background objects are hidden on this slide."));

    // Docling has no transition or animation item, so the transition is one
    // notes-layer item and each effect is another.
    let notes: Vec<&serde_json::Value> = texts
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .collect();
    assert_eq!(
        notes.len(),
        3,
        "one transition and one notes item per animation effect"
    );
    assert_eq!(notes[0]["text"], "Transition: barWipe (duration 2s)");
    assert_eq!(
        notes[1]["text"],
        "Animation: set visibility to visible on \"Body\" (begin 0s, duration 0.5s)"
    );
    assert_eq!(
        notes[2]["text"],
        "Animation: set visibility to visible on \"Title\" (begin 0.5s, duration 0.5s)"
    );
    assert!(notes.iter().all(|note| note["prov"][0]["page_no"] == 1));
}

#[test]
fn a_page_range_selects_slides() {
    let ws = Workspace::new("range");
    let output = ws.join("range.json");
    run_ok(&[
        "-i",
        fixture().to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
        "--page-range",
        "2",
    ]);
    let doc = json_of(&output);

    assert_eq!(page_numbers(&doc), [2]);
    let texts = doc["texts"].as_array().expect("texts");
    assert!(texts
        .iter()
        .any(|item| item["text"] == "Regional performance"));
    assert!(!texts.iter().any(|item| item["text"] == "Q3 Review"));

    let past = ws.join("past.json");
    let out = run(&[
        "-i",
        fixture().to_str().expect("a UTF-8 path"),
        "-o",
        past.to_str().expect("a UTF-8 path"),
        "--page-range",
        "9",
    ]);
    assert!(!out.status.success(), "a range past the deck was accepted");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("names no slide") && stderr.contains("(the deck has 3 slide(s); 1-3)"),
        "the refusal does not name the slides there are: {stderr}"
    );
    assert!(
        !past.exists(),
        "a refused range still wrote {}",
        past.display()
    );
}

#[test]
fn a_page_range_keeps_only_the_selected_slides_pictures_and_notes() {
    let ws = Workspace::new("range-items");
    let output = ws.join("range.json");
    run_ok(&[
        "-i",
        fixture().to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
        "--page-range",
        "3",
    ]);
    let doc = json_of(&output);

    // Only the selected slide's chapter and comment group survive.
    assert_eq!(page_numbers(&doc), [3]);
    let groups = doc["groups"].as_array().expect("groups");
    assert_eq!(
        groups
            .iter()
            .filter(|group| group["label"] == "chapter")
            .map(|group| group["name"].as_str().expect("a group name"))
            .collect::<Vec<_>>(),
        ["slide-2"]
    );
    assert_eq!(
        groups
            .iter()
            .filter(|group| group["label"] == "comment_section")
            .map(|group| group["name"].as_str().expect("a group name"))
            .collect::<Vec<_>>(),
        ["comment-slide3-1"]
    );

    // The selected slide has one speaker note and one comment. The note and
    // comment from slide 1, and the skipped slide's table, must not leak in.
    let texts = doc["texts"].as_array().expect("texts");
    let notes: Vec<&serde_json::Value> = texts
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .collect();
    assert_eq!(notes.len(), 2, "a skipped slide's note or comment survived");
    assert!(notes.iter().any(|note| note["text"]
        .as_str()
        .is_some_and(|text| text.contains("Say the picture is the growth by quarter"))));
    assert!(notes
        .iter()
        .any(|note| note["text"]
            == "[author: Grace Hopper, time: 2026-10-18T16:05:00]: Ship this one."));
    for skipped in [
        "Open with the revenue line, then the regions.",
        "Check the Q3 numbers against the ledger.",
        "Add the regional split to slide 2.",
        "Q3 Review",
        "Regional performance",
    ] {
        assert!(
            !texts.iter().any(|item| item["text"] == skipped),
            "skipped slide text {skipped:?} survived"
        );
    }
    assert!(
        texts.iter().all(|item| item["prov"]
            .as_array()
            .is_none_or(|prov| prov.iter().all(|place| place["page_no"] == 3))),
        "a text item points at a skipped slide: {texts:#?}"
    );
    assert!(
        doc["tables"].as_array().is_some_and(Vec::is_empty),
        "the skipped slide's table survived"
    );

    // The selected slide's picture and chart are the only two pictures; both
    // point at page 3, and only the picture has a file beside the JSON.
    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(pictures.len(), 2, "a skipped slide's picture survived");
    assert!(pictures.iter().all(|picture| picture["prov"]
        .as_array()
        .is_some_and(|prov| prov.len() == 1 && prov[0]["page_no"] == 3)));
    let uri = pictures[0]["image"]["uri"]
        .as_str()
        .expect("the selected slide's picture");
    assert!(ws.join(uri).is_file(), "the picture {uri} was not written");
    assert!(pictures[1]["image"].is_null(), "the chart became an image");
}

/// A multi-slide selection keeps every selected slide's items, in the deck's order
/// rather than the range's. The range is deliberately reversed: slide 3's picture
/// and chart, and both selected slides' notes and comments, must come back in deck
/// order, and the unselected slide's table must not come back at all.
#[test]
fn a_page_range_keeps_every_selected_slides_items_in_deck_order() {
    let ws = Workspace::new("range-multi");
    let output = ws.join("range.json");
    run_ok(&[
        "-i",
        fixture().to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
        "--page-range",
        "3,1",
    ]);
    let doc = json_of(&output);

    assert_eq!(page_numbers(&doc), [1, 3]);
    let groups = doc["groups"].as_array().expect("groups");
    assert_eq!(
        groups
            .iter()
            .filter(|group| group["label"] == "chapter")
            .map(|group| group["name"].as_str().expect("a group name"))
            .collect::<Vec<_>>(),
        ["slide-0", "slide-2"],
        "the selected slides are not in deck order"
    );
    assert_eq!(
        groups
            .iter()
            .filter(|group| group["label"] == "comment_section")
            .map(|group| group["name"].as_str().expect("a group name"))
            .collect::<Vec<_>>(),
        ["comment-slide1-1", "comment-slide1-2", "comment-slide3-1"],
        "the selected comments are not in deck order"
    );

    let texts = doc["texts"].as_array().expect("texts");
    let find = |text: &str| {
        texts
            .iter()
            .find(|item| item["text"] == text)
            .unwrap_or_else(|| panic!("no text item {text:?} in {texts:#?}"))
    };
    assert_eq!(find("Q3 Review")["prov"][0]["page_no"], 1);
    assert_eq!(find("Where the growth came from")["prov"][0]["page_no"], 3);
    assert!(
        !texts
            .iter()
            .any(|item| item["text"] == "Regional performance"),
        "the unselected slide's title survived: {texts:#?}"
    );

    // Both slides' speaker notes and comments are kept, slide by slide: the
    // first slide's note, its two comments, the third slide's note, its comment.
    let notes: Vec<&serde_json::Value> = texts
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .collect();
    assert_eq!(
        notes.len(),
        5,
        "a selected slide's note or comment was lost"
    );
    let note_text =
        |note: &serde_json::Value| note["text"].as_str().unwrap_or_default().to_string();
    assert!(
        note_text(notes[0]).starts_with("Open with the revenue line"),
        "the first slide's note is not first: {notes:#?}"
    );
    assert!(
        note_text(notes[1]).contains("Check the Q3 numbers against the ledger.")
            && note_text(notes[2]).contains("Add the regional split to slide 2."),
        "the first slide's comments are not beside its note: {notes:#?}"
    );
    assert!(
        note_text(notes[3]).starts_with("Say the picture is the growth by quarter"),
        "the third slide's note is not next: {notes:#?}"
    );
    assert!(
        note_text(notes[4]).contains("Ship this one."),
        "the third slide's comment is not last: {notes:#?}"
    );
    assert_eq!(notes[0]["prov"][0]["page_no"], 1);
    assert_eq!(notes[3]["prov"][0]["page_no"], 3);

    // The third slide's picture and chart are kept, in deck order; the first
    // slide has neither, and the second slide is not selected at all.
    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(pictures.len(), 2, "a selected slide's picture was lost");
    assert!(
        pictures.iter().all(|picture| picture["prov"]
            .as_array()
            .is_some_and(|prov| prov.len() == 1 && prov[0]["page_no"] == 3)),
        "a picture points at another slide: {pictures:#?}"
    );
    assert_eq!(pictures[0]["image"]["mimetype"], "image/png");
    let uri = pictures[0]["image"]["uri"]
        .as_str()
        .expect("the selected slide's picture");
    assert!(ws.join(uri).is_file(), "the picture {uri} was not written");
    assert!(pictures[1]["image"].is_null(), "the chart became an image");
    assert_eq!(
        pictures[1]["meta"]["classification"]["predictions"][0]["class_name"],
        "bar_chart"
    );

    assert!(
        doc["tables"].as_array().is_some_and(Vec::is_empty),
        "the unselected slide's table survived"
    );
}

#[test]
fn odp_is_offered_under_its_own_family() {
    let out = run_ok(&["--list-input-formats"]);
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--list-input-formats is JSON");
    assert!(
        value["input_formats"]
            .as_array()
            .expect("a format list")
            .iter()
            .any(|name| name == ".odp"),
        "the accepted formats do not name .odp: {value}"
    );
    let family = value["families"]
        .as_array()
        .expect("families")
        .iter()
        .find(|family| family["name"] == "OpenDocument")
        .expect("an OpenDocument family");
    assert!(
        family["input_formats"]
            .as_array()
            .expect("the family's formats")
            .iter()
            .any(|name| name == ".odp"),
        "the OpenDocument family does not hold odp: {family}"
    );
}

#[test]
fn a_file_that_is_not_a_package_is_refused_as_such() {
    let ws = Workspace::new("broken");
    let broken = ws.write("broken.odp", b"this is not a package at all");
    let output = ws.join("broken.json");
    let out = run(&[
        "-i",
        broken.to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
    ]);
    assert!(!out.status.success(), "a non-package was converted");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not an OpenDocument (ZIP) package"),
        "the refusal does not name the package: {stderr}"
    );
    assert!(
        !output.exists(),
        "a refused file still wrote {}",
        output.display()
    );
}
