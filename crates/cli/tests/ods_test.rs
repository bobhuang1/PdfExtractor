//! End-to-end tests of the OpenDocument spreadsheet reader.
//!
//! `inventory_calc.ods` is LibreOffice Calc's conversion of `inventory.xlsx` (written
//! by `test-data/make_inventory_ods.py`), so the fixture is what a real producer
//! writes: three sheets (one hidden), a merge, an empty row that splits a sheet
//! into two data regions, numbers, a boolean, a date and a cached formula.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn test_data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data")
        .join(name)
}

fn fixture() -> PathBuf {
    test_data("inventory_calc.ods")
}

/// A scratch directory removed when the test ends.
struct Workspace {
    path: PathBuf,
}

impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pdfe-ods-{}-{tag}", std::process::id()));
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
        .unwrap_or_else(|e| panic!("missing output {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
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

/// Every cell of every table, searched by its text.
fn cell<'a>(doc: &'a serde_json::Value, text: &str) -> &'a serde_json::Value {
    doc["tables"]
        .as_array()
        .expect("a table list")
        .iter()
        .flat_map(|table| {
            table["data"]["table_cells"]
                .as_array()
                .expect("cells")
                .iter()
        })
        .find(|cell| cell["text"] == text)
        .unwrap_or_else(|| panic!("no cell {text:?} in {:#?}", doc["tables"]))
}

#[test]
fn an_ods_reads_as_sheets_of_tables() {
    let ws = Workspace::new("inventory");
    let output = ws.join("inventory.json");
    run_ok(&[
        "-i",
        fixture().to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
    ]);
    let doc = json_of(&output);

    // Docling names a workbook by its stem and keeps the file name in origin.
    assert_eq!(doc["name"], "inventory_calc");
    assert_eq!(doc["origin"]["filename"], "inventory_calc.ods");
    assert_eq!(
        doc["origin"]["mimetype"],
        "application/vnd.oasis.opendocument.spreadsheet"
    );
    // A sheet is a page, and the workbook has three of them in its own order.
    assert_eq!(page_numbers(&doc), vec![1, 2, 3]);

    let groups = doc["groups"].as_array().expect("groups");
    let sheets: Vec<(&str, &str)> = groups
        .iter()
        .filter(|group| group["label"] == "sheet")
        .map(|group| {
            (
                group["name"].as_str().expect("a sheet name"),
                group["content_layer"].as_str().expect("a layer"),
            )
        })
        .collect();
    assert_eq!(
        sheets,
        [
            ("Inventory", "body"),
            ("Notes", "body"),
            ("Hidden", "invisible")
        ],
        "the hidden sheet stays in the document on the invisible layer"
    );

    // The empty row splits Inventory into two tables, and the filler Calc
    // writes for the rest of the sheet is not a column.
    assert_eq!(doc["tables"].as_array().expect("tables").len(), 4);
    let item = cell(&doc, "Item");
    assert_eq!(item["column_header"], true);
    let shipping = cell(&doc, "Shipping included");
    assert_eq!(shipping["col_span"], 2);
    assert_eq!(shipping["start_row_offset_idx"], 0);
    assert_eq!(shipping["start_col_offset_idx"], 1);
    assert_eq!(cell(&doc, "15")["text"], "15", "a formula's cached result");
    assert_eq!(
        cell(&doc, "True")["text"],
        "True",
        "a boolean reads its value, which Docling spells `True`"
    );
    // The row ends at its last cell: the empty slot after `15` is padding rather
    // than a cell, exactly as Docling's own reading of this workbook has it.
    let shipping = doc["tables"]
        .as_array()
        .expect("tables")
        .iter()
        .find(|table| {
            table["data"]["table_cells"]
                .as_array()
                .is_some_and(|cells| cells.iter().any(|cell| cell["text"] == "Shipping included"))
        })
        .expect("the table holding the merged header");
    assert_eq!(
        shipping["data"]["table_cells"]
            .as_array()
            .expect("the table's cells")
            .len(),
        4,
        "the empty slot at the end of the `Total` row is not a cell"
    );
    assert_eq!(
        cell(&doc, "2024-03-05")["text"],
        "2024-03-05",
        "a date reads as the text the reader sees"
    );
}

#[test]
fn ods_is_offered_under_its_own_family() {
    let out = run_ok(&["--list-input-formats"]);
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--list-input-formats is JSON");
    assert!(
        value["input_formats"]
            .as_array()
            .expect("a format list")
            .iter()
            .any(|name| name == ".ods"),
        "the accepted formats do not name .ods: {value}"
    );
    let family = value["families"]
        .as_array()
        .expect("families")
        .iter()
        .find(|family| family["name"] == "OpenDocument")
        .expect("an OpenDocument family");
    let names: Vec<&str> = family["input_formats"]
        .as_array()
        .expect("the family's formats")
        .iter()
        .filter_map(|name| name.as_str())
        .collect();
    assert_eq!(names, [".odt", ".ods", ".odp"]);
}

#[test]
fn a_page_range_selects_a_sheet() {
    let ws = Workspace::new("range");
    run_ok(&[
        "-i",
        fixture().to_str().expect("a UTF-8 path"),
        "-o",
        ws.join("notes.json").to_str().expect("a UTF-8 path"),
        "--page-range",
        "2",
    ]);
    let doc = json_of(&ws.join("notes.json"));
    assert_eq!(page_numbers(&doc), vec![2]);
    assert_eq!(
        doc["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .filter(|group| group["label"] == "sheet")
            .map(|group| group["name"].as_str().expect("a sheet name"))
            .collect::<Vec<_>>(),
        ["Notes"]
    );

    let out = run(&[
        "-i",
        fixture().to_str().expect("a UTF-8 path"),
        "-o",
        ws.join("none.json").to_str().expect("a UTF-8 path"),
        "--page-range",
        "9",
    ]);
    assert!(!out.status.success(), "a range past the last sheet passed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("names no sheet") && stderr.contains("(the workbook has 3 sheet(s); 1-3)"),
        "the refusal does not name the sheets there are: {stderr}"
    );
    assert!(!ws.join("none.json").exists(), "a refused run wrote a file");
}

fn assert_bbox(picture: &Value, want: [f64; 4]) {
    let bbox = &picture["prov"][0]["bbox"];
    let got = [
        bbox["l"].as_f64().expect("a left edge"),
        bbox["t"].as_f64().expect("a top edge"),
        bbox["r"].as_f64().expect("a right edge"),
        bbox["b"].as_f64().expect("a bottom edge"),
    ];
    for (got, want) in got.iter().zip(want) {
        assert!((got - want).abs() < 1e-9, "{got} is not {want}: {got:?}");
    }
}

#[test]
fn an_ods_sheet_reads_the_pictures_it_draws() {
    let ws = Workspace::new("pictures");
    let path = test_data("worksheet_pictures_calc.ods");
    let output = ws.join("worksheet_pictures_calc.json");
    run_ok(&[
        "-i",
        path.to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(
        pictures.len(),
        5,
        "the four raster pictures on Pictures and the BMP on Summary: {pictures:#?}"
    );

    // The BMP's frame sits in A9 and its 60 × 36 picture is handed on as a PNG.
    assert_eq!(pictures[0]["prov"][0]["page_no"].as_i64(), Some(1));
    assert_bbox(&pictures[0], [0.0, 8.0, 0.9372, 9.79904]);
    assert_eq!(pictures[0]["image"]["mimetype"], "image/png");
    let uri = pictures[0]["image"]["uri"]
        .as_str()
        .expect("a written picture");
    let drawn = image::load_from_memory(&std::fs::read(ws.join(uri)).expect("the written PNG"))
        .expect("the written PNG decodes")
        .to_rgb8();
    let expected = image::load_from_memory(&std::fs::read(test_data("images/chart.bmp")).unwrap())
        .expect("the fixture BMP decodes")
        .to_rgb8();
    assert_eq!(drawn.dimensions(), expected.dimensions());
    assert_eq!(drawn.as_raw(), expected.as_raw());

    // The JPEG is carried through as a JPEG, and the Summary sheet's picture
    // stays on its own page rather than moving with the first sheet's.
    assert_eq!(pictures[3]["image"]["mimetype"], "image/jpeg");
    assert!(pictures[3]["image"]["uri"]
        .as_str()
        .is_some_and(|uri| uri.ends_with(".jpg")));
    assert_eq!(pictures[4]["prov"][0]["page_no"].as_i64(), Some(2));
    assert_bbox(&pictures[4], [0.0, 2.0, 0.9372, 3.79904]);
}

#[test]
fn an_ods_sheet_chart_is_its_data_rather_than_a_picture() {
    let ws = Workspace::new("charts");
    let path = test_data("chart_sheet_calc.ods");
    let output = ws.join("chart_sheet_calc.json");
    run_ok(&[
        "-i",
        path.to_str().expect("a UTF-8 path"),
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
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(pictures.len(), 2, "both sheets' charts: {pictures:#?}");

    let bar = &pictures[0];
    assert_eq!(
        bar["meta"]["classification"]["predictions"][0]["class_name"],
        "bar_chart"
    );
    assert!(bar["image"].is_null(), "a chart was written as an image");
    assert_eq!(bar["prov"][0]["page_no"].as_i64(), Some(1));
    assert_eq!(bar["prov"][0]["bbox"]["l"].as_f64(), Some(4.0));
    assert_eq!(bar["prov"][0]["bbox"]["t"].as_f64(), Some(1.0));

    let data = &bar["meta"]["tabular_chart"]["chart_data"];
    assert_eq!(data["num_rows"].as_u64(), Some(4), "{data:#?}");
    assert_eq!(data["num_cols"].as_u64(), Some(3), "{data:#?}");
    let cells: Vec<(u64, u64, &str)> = data["table_cells"]
        .as_array()
        .expect("the chart's cells")
        .iter()
        .map(|cell| {
            (
                cell["start_row_offset_idx"].as_u64().unwrap(),
                cell["start_col_offset_idx"].as_u64().unwrap(),
                cell["text"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    for cell in [
        (0, 0, ""),
        (0, 1, "Q1"),
        (0, 2, "Q2"),
        (1, 0, "North"),
        (1, 1, "120"),
        (1, 2, "140"),
        (2, 0, "South"),
        (2, 1, "90"),
        (2, 2, "95"),
        (3, 0, "West"),
        (3, 1, "60"),
        (3, 2, "80"),
    ] {
        assert!(cells.contains(&cell), "{cell:?} is missing: {cells:?}");
    }

    let caption = bar["captions"][0]["$ref"].as_str().expect("a caption ref");
    let caption_item = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .find(|item| item["self_ref"] == caption)
        .expect("the caption item");
    assert_eq!(caption_item["text"], "Revenue by region");
    assert_eq!(caption_item["label"], "caption");

    let line = &pictures[1];
    assert_eq!(
        line["meta"]["classification"]["predictions"][0]["class_name"],
        "line_chart"
    );
    assert_eq!(
        line["meta"]["tabular_chart"]["chart_data"]["num_cols"].as_u64(),
        Some(2)
    );
    assert_eq!(line["prov"][0]["page_no"].as_i64(), Some(2));
    assert!(line["captions"]
        .as_array()
        .is_none_or(|list| list.is_empty()));
    assert!(line["image"].is_null());

    let markdown = std::fs::read_to_string(ws.join("chart_sheet_calc.md")).expect("the Markdown");
    assert!(markdown.contains("Revenue by region"), "{markdown}");
    assert!(
        !markdown.contains("!["),
        "a chart became an image: {markdown}"
    );
    let html = std::fs::read_to_string(ws.join("chart_sheet_calc.html")).expect("the HTML");
    assert!(!html.contains("<img"), "a chart became an image: {html}");
    assert!(!ws.join("chart_sheet_calc_images").exists());
}

/// A chart's ranges name the sheet they read, and the chart sheet's own cells are
/// not a stand-in for them: this workbook anchors the chart on `Charts`, plots
/// `Figures`, and puts deliberately wrong numbers on `Charts` so a reading that
/// ignored the sheet in a range would be visible in the table.
#[test]
fn a_chart_on_one_sheet_plots_cells_from_another() {
    let ws = Workspace::new("cross-sheet-chart");
    let path = test_data("chart_cross_sheet_calc.ods");
    let output = ws.join("chart_cross_sheet_calc.json");
    run_ok(&[
        "-i",
        path.to_str().expect("a UTF-8 path"),
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
    assert_eq!(page_numbers(&doc), [1, 2]);

    // Both sheets are still pages of their own, and the chart sheet keeps its
    // decoy cells as that sheet's table.
    let tables = doc["tables"].as_array().expect("a table list");
    assert_eq!(tables.len(), 2, "{tables:#?}");
    assert_eq!(cell(&doc, "999")["text"], "999");
    assert_eq!(cell(&doc, "North")["text"], "North");

    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(pictures.len(), 1, "the chart on Charts: {pictures:#?}");
    let chart = &pictures[0];
    assert_eq!(
        chart["meta"]["classification"]["predictions"][0]["class_name"],
        "bar_chart"
    );
    assert!(chart["image"].is_null(), "a chart was written as an image");
    assert_eq!(chart["prov"][0]["page_no"].as_i64(), Some(2));
    assert_eq!(chart["prov"][0]["bbox"]["l"].as_f64(), Some(4.0));
    assert_eq!(chart["prov"][0]["bbox"]["t"].as_f64(), Some(1.0));

    let data = &chart["meta"]["tabular_chart"]["chart_data"];
    assert_eq!(data["num_rows"].as_u64(), Some(4), "{data:#?}");
    assert_eq!(data["num_cols"].as_u64(), Some(3), "{data:#?}");
    let cells: Vec<(u64, u64, &str)> = data["table_cells"]
        .as_array()
        .expect("the chart's cells")
        .iter()
        .map(|cell| {
            (
                cell["start_row_offset_idx"].as_u64().unwrap(),
                cell["start_col_offset_idx"].as_u64().unwrap(),
                cell["text"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    for cell in [
        (0, 1, "Q1"),
        (0, 2, "Q2"),
        (1, 0, "North"),
        (1, 1, "120"),
        (1, 2, "140"),
        (2, 0, "South"),
        (2, 1, "90"),
        (2, 2, "95"),
        (3, 0, "West"),
        (3, 1, "60"),
        (3, 2, "80"),
    ] {
        assert!(cells.contains(&cell), "{cell:?} is missing: {cells:?}");
    }
    assert!(
        cells
            .iter()
            .all(|(_, _, text)| *text != "Wrong" && !text.contains("999")),
        "the chart plotted the chart sheet's decoys: {cells:?}"
    );

    let caption = chart["captions"][0]["$ref"]
        .as_str()
        .expect("a caption ref");
    let caption_item = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .find(|item| item["self_ref"] == caption)
        .expect("the caption item");
    assert_eq!(caption_item["text"], "Cross-sheet revenue");
    assert_eq!(caption_item["label"], "caption");

    let markdown =
        std::fs::read_to_string(ws.join("chart_cross_sheet_calc.md")).expect("the Markdown");
    assert!(markdown.contains("Cross-sheet revenue"), "{markdown}");
    assert!(
        !markdown.contains("!["),
        "a chart became an image: {markdown}"
    );
    let html = std::fs::read_to_string(ws.join("chart_cross_sheet_calc.html")).expect("the HTML");
    assert!(html.contains("Cross-sheet revenue"), "{html}");
    assert!(!html.contains("<img"), "a chart became an image: {html}");
    assert!(!ws.join("chart_cross_sheet_calc_images").exists());
}

#[test]
fn a_file_that_is_not_a_package_is_refused_as_such() {
    let ws = Workspace::new("broken");
    let broken = ws.write("broken.ods", b"this is not a package at all");
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

/// A cell's comment is about the sheet rather than part of its grid: the
/// workbook's comments land on the notes layer, one `comment_section` group per
/// cell, named for the cell, and the cell keeps only its text — so a reading
/// that did not ask for notes never sees a comment pretending to be a cell.
#[test]
fn a_sheets_cell_comments_land_on_the_notes_layer() {
    let ws = Workspace::new("annotations");
    let path = test_data("annotations_calc.ods");
    let output = ws.join("annotations_calc.json");
    run_ok(&[
        "-i",
        path.to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
        "--to",
        "json",
        "--to",
        "md",
    ]);
    let doc = json_of(&output);

    let notes: Vec<&str> = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .filter_map(|item| item["text"].as_str())
        .collect();
    assert_eq!(
        notes,
        [
            "[author: Ada Lovelace, time: 2024-03-05T00:00:00]: Checked against the invoice.",
            "[author: Grace Hopper, time: 2024-03-06T00:00:00]: An empty cell's remark.",
        ],
        "the cell comments are on the notes layer, with the author and time the file states"
    );

    let body: Vec<&str> = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .filter(|item| item["content_layer"] != "notes")
        .filter_map(|item| item["text"].as_str())
        .collect();
    assert!(
        body.iter()
            .all(|text| !text.contains("Checked against the invoice.")),
        "a comment was written as a cell's text: {body:?}"
    );

    let comments: Vec<(&str, &str)> = doc["groups"]
        .as_array()
        .expect("a group list")
        .iter()
        .filter(|group| group["label"] == "comment_section")
        .map(|group| {
            (
                group["name"].as_str().expect("a comment name"),
                group["content_layer"].as_str().expect("a layer"),
            )
        })
        .collect();
    assert_eq!(
        comments,
        [
            ("comment-sheet1-1-1", "notes"),
            ("comment-sheet1-3-2", "notes"),
        ],
        "each comment has its own notes-layer group, named for the cell"
    );

    // A reading that leaves the notes layer out does not show the remarks; one
    // that asks for it writes each as a comment, not as a cell of the sheet.
    let markdown = std::fs::read_to_string(ws.join("annotations_calc.md"))
        .expect("the Markdown without notes");
    assert!(
        !markdown.contains("Checked against the invoice."),
        "a comment was read without --include-notes: {markdown}"
    );

    run_ok(&[
        "-i",
        path.to_str().expect("a UTF-8 path"),
        "-o",
        ws.join("with_notes.md").to_str().expect("a UTF-8 path"),
        "--to",
        "md",
        "--include-notes",
    ]);
    let with_notes =
        std::fs::read_to_string(ws.join("with_notes.md")).expect("the Markdown with notes");
    assert!(
        with_notes.contains(
            "> **Comment:** [author: Ada Lovelace, time: 2024-03-05T00:00:00]: \
             Checked against the invoice."
        ),
        "the comment is not written as a remark: {with_notes}"
    );
}
