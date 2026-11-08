//! End-to-end tests of the OpenDocument text reader.
//!
//! `brief_odt.odt` is LibreOffice Writer's conversion of `brief.docx` (written by
//! `test-data/make_brief_odt.py`), so the fixture is what a real producer
//! writes: a title, a heading, a link, two lists, a table with a row span and a
//! column span, and a picture.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/brief_odt.odt")
}

/// A scratch directory removed when the test ends.
struct Workspace {
    path: PathBuf,
}

impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pdfe-odt-{}-{tag}", std::process::id()));
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

#[test]
fn an_odt_reads_as_the_flow_it_is() {
    let ws = Workspace::new("brief");
    let output = ws.join("brief.json");
    run_ok(&[
        "-i",
        fixture().to_str().expect("a UTF-8 path"),
        "-o",
        output.to_str().expect("a UTF-8 path"),
    ]);
    let doc = json_of(&output);

    assert_eq!(doc["name"], "brief_odt.odt");
    assert_eq!(
        doc["origin"]["mimetype"],
        "application/vnd.oasis.opendocument.text"
    );
    // ODF is flow-based: a page style states a rectangle, but no item sits on a
    // page, so the document publishes none.
    assert!(doc["pages"].as_object().expect("a pages map").is_empty());

    let texts = doc["texts"].as_array().expect("a text list");
    let find = |text: &str| {
        texts
            .iter()
            .find(|item| item["text"] == text)
            .unwrap_or_else(|| panic!("no text item {text:?} in {texts:#?}"))
    };
    assert_eq!(find("Quarterly Brief")["label"], "title");
    assert_eq!(find("Summary")["label"], "section_header");
    assert_eq!(find("Summary")["level"], 1);
    assert_eq!(find("Picture")["label"], "section_header");
    assert_eq!(find("Picture")["level"], 2);
    assert_eq!(
        find("The quarter closed ahead of plan. The filing explains why.")["hyperlink"],
        "https://example.com/filing"
    );
    let items: Vec<&str> = texts
        .iter()
        .filter(|item| item["label"] == "list_item")
        .map(|item| item["text"].as_str().expect("list item text"))
        .collect();
    assert_eq!(
        items,
        [
            "First bullet",
            "Second bullet",
            "Numbered one",
            "Numbered two"
        ]
    );

    let tables = doc["tables"].as_array().expect("a table list");
    assert_eq!(tables.len(), 1);
    let data = &tables[0]["data"];
    assert_eq!(data["num_rows"], 3);
    assert_eq!(data["num_cols"], 3);
    let cells = data["table_cells"].as_array().expect("cells");
    let cell = |text: &str| {
        cells
            .iter()
            .find(|cell| cell["text"] == text)
            .unwrap_or_else(|| panic!("no cell {text:?} in {cells:#?}"))
    };
    assert_eq!(cell("Revenue")["col_span"], 2);
    assert_eq!(cell("Revenue")["start_col_offset_idx"], 1);
    assert_eq!(cell("North\nSouth")["row_span"], 2);
    assert_eq!(cell("Region")["column_header"], true);
    assert_eq!(cell("42")["start_col_offset_idx"], 1);

    let pictures = doc["pictures"].as_array().expect("a picture list");
    assert_eq!(pictures.len(), 1);
    let image = &pictures[0]["image"];
    assert_eq!(image["mimetype"], "image/jpeg");
    assert_eq!(image["size"]["width"], 620.0);
    assert_eq!(image["size"]["height"], 350.0);
    // The frame places 620 pixels across 2 inches, which is 310 dpi.
    assert_eq!(image["dpi"], 310);
    let uri = image["uri"].as_str().expect("a picture uri");
    assert!(
        ws.join(uri).is_file(),
        "the picture {uri} was not written beside the output"
    );
}

#[test]
fn odt_is_offered_under_its_own_family() {
    let out = run_ok(&["--list-input-formats"]);
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--list-input-formats is JSON");
    assert!(
        value["input_formats"]
            .as_array()
            .expect("a format list")
            .iter()
            .any(|name| name == ".odt"),
        "the accepted formats do not name .odt: {value}"
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
            .any(|name| name == ".odt"),
        "the OpenDocument family does not hold odt: {family}"
    );
}

#[test]
fn a_file_that_is_not_a_package_is_refused_as_such() {
    let ws = Workspace::new("broken");
    let broken = ws.write("broken.odt", b"this is not a package at all");
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
