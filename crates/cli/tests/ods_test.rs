//! End-to-end tests of the OpenDocument spreadsheet reader.
//!
//! `inventory_calc.ods` is LibreOffice Calc's conversion of `inventory.xlsx` (written
//! by `test-data/make_inventory_ods.py`), so the fixture is what a real producer
//! writes: three sheets (one hidden), a merge, an empty row that splits a sheet
//! into two data regions, numbers, a boolean, a date and a cached formula.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/inventory_calc.ods")
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
    assert_eq!(cell(&doc, "TRUE")["text"], "TRUE");
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
    assert_eq!(names, [".odt", ".ods"]);
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
    assert!(stderr.contains("names no sheet"), "{stderr}");
    assert!(stderr.contains("sheet(s)"), "{stderr}");
    assert!(!ws.join("none.json").exists(), "a refused run wrote a file");
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
