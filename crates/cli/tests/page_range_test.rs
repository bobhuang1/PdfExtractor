//! End-to-end tests of `--page-range`.
//!
//! The real binary runs over `basicapi.pdf`, a three-page document whose pages
//! say `page N / 3` at their foot, so the tests can see both halves of the
//! contract: only the named pages are read, and the furniture pass is given only
//! the pages that were kept.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/real")
        .join(name)
}

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// A scratch directory removed when the test ends.
struct Workspace {
    path: PathBuf,
}

impl Workspace {
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("pdfe-page-range-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self { path }
    }

    fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
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

/// The page numbers a document holds, in the order the map names them.
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

fn labels(doc: &serde_json::Value) -> Vec<String> {
    doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["label"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Only the named page is in the document, and it keeps its own number.
#[test]
fn only_the_named_page_is_in_the_document() {
    let ws = Workspace::new("one");
    run_ok(&[
        "-i",
        corpus("basicapi.pdf").to_str().unwrap(),
        "-o",
        ws.join("page2.json").to_str().unwrap(),
        "--page-range",
        "2",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("page2.json"));
    assert_eq!(page_numbers(&doc), vec![2]);
    assert_eq!(doc["pages"]["2"]["page_no"], 2);
    for item in doc["texts"].as_array().expect("texts") {
        for prov in item["prov"].as_array().expect("prov") {
            assert_eq!(prov["page_no"], 2);
        }
    }
}

/// A span reads every page in it, and nothing outside it.
#[test]
fn a_span_reads_every_page_in_it() {
    let ws = Workspace::new("span");
    run_ok(&[
        "-i",
        corpus("basicapi.pdf").to_str().unwrap(),
        "-o",
        ws.join("pages.json").to_str().unwrap(),
        "--page-range",
        "2-3",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("pages.json"));
    assert_eq!(page_numbers(&doc), vec![2, 3]);
}

/// The furniture pass is run over the kept pages, not the file's: `page 1 / 3`
/// is furniture on a document of three pages (it repeats), and is body text on
/// a document of one — so reading page 1 alone must not label it a page footer.
#[test]
fn the_furniture_pass_sees_only_the_pages_kept() {
    let ws = Workspace::new("furniture");
    let input = corpus("basicapi.pdf");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        ws.join("one.json").to_str().unwrap(),
        "--page-range",
        "1",
        "--ocr",
        "never",
    ]);
    let one = labels(&json_of(&ws.join("one.json")));
    assert!(
        !one.iter().any(|label| label == "page_footer"),
        "a footer was invented among the kept pages: {one:?}"
    );

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        ws.join("all.json").to_str().unwrap(),
        "--ocr",
        "never",
    ]);
    let all = labels(&json_of(&ws.join("all.json")));
    assert!(
        all.iter().any(|label| label == "page_footer"),
        "the corpus document's page numbers lost their label: {all:?}"
    );
}

/// A range that names no page of the file fails, naming the range and how many
/// pages there were.
#[test]
fn a_range_that_names_no_page_fails_with_the_count() {
    let ws = Workspace::new("toolarge");
    let out = run(&[
        "-i",
        corpus("basicapi.pdf").to_str().unwrap(),
        "-o",
        ws.join("none.json").to_str().unwrap(),
        "--page-range",
        "9",
        "--ocr",
        "never",
    ]);

    assert!(!out.status.success(), "a range past the end was accepted");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("names no page"), "{stderr}");
    assert!(
        stderr.contains("3 page"),
        "the count is not named: {stderr}"
    );
    assert!(!ws.join("none.json").exists(), "a refused run wrote a file");
}

/// A range that cannot be read is refused by the argument parser, before any
/// conversion.
#[test]
fn an_unreadable_range_is_refused_before_conversion() {
    let ws = Workspace::new("syntax");
    for spec in ["abc", "0", "3-1", "1,,2", ""] {
        let out = run(&[
            "-i",
            corpus("basicapi.pdf").to_str().unwrap(),
            "-o",
            ws.join("bad.json").to_str().unwrap(),
            "--page-range",
            spec,
        ]);
        assert!(!out.status.success(), "the parser accepted {spec:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("page-range"), "{spec:?} gave {stderr}");
        assert!(!ws.join("bad.json").exists(), "{spec:?} wrote a file");
    }
}

/// A workbook's sheets are its pages: the range selects among them, each sheet
/// keeps the number it has in the workbook, and a range past the last sheet
/// fails naming the sheets there were.
#[test]
fn a_workbook_is_read_as_the_sheets_named() {
    let ws = Workspace::new("sheets");
    let input = fixture("test-data/inventory.xlsx");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        ws.join("sheet2.json").to_str().unwrap(),
        "--page-range",
        "2",
        "--ocr",
        "never",
    ]);
    let doc = json_of(&ws.join("sheet2.json"));
    assert_eq!(page_numbers(&doc), vec![2]);

    let out = run(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        ws.join("none.json").to_str().unwrap(),
        "--page-range",
        "9",
    ]);
    assert!(
        !out.status.success(),
        "a range past the last sheet was accepted"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("names no sheet")
            && stderr.contains("(the workbook has 3 sheet(s); 1-3)"),
        "the refusal does not name the sheets there are: {stderr}"
    );
    assert!(!ws.join("none.json").exists(), "a refused run wrote a file");
}

/// A presentation's slides are its pages too: the range selects among them, each
/// keeps the number it has in the deck, and a range past the last slide fails
/// naming the slides there were. Both presentation families are read the same
/// way — `q3_review.pptx` and Impress' conversion of it.
#[test]
fn a_presentation_is_read_as_the_slides_named() {
    let ws = Workspace::new("slides");
    for (tag, input) in [
        ("pptx", fixture("test-data/q3_review.pptx")),
        ("odp", fixture("test-data/q3_review_impress.odp")),
    ] {
        let kept = ws.join(&format!("{tag}-slide2.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            kept.to_str().unwrap(),
            "--page-range",
            "2",
            "--ocr",
            "never",
        ]);
        let doc = json_of(&kept);
        assert_eq!(page_numbers(&doc), vec![2], "{tag}: the range kept more than slide 2");
        let texts = doc["texts"].as_array().expect("texts");
        assert!(
            texts
                .iter()
                .any(|item| item["text"] == "Regional performance"),
            "{tag}: slide 2's title is missing"
        );
        assert!(
            !texts.iter().any(|item| item["text"] == "Q3 Review"),
            "{tag}: slide 1 was read too"
        );

        let none = ws.join(&format!("{tag}-none.json"));
        let out = run(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            none.to_str().unwrap(),
            "--page-range",
            "9",
        ]);
        assert!(
            !out.status.success(),
            "{tag}: a range past the deck was accepted"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("names no slide")
                && stderr.contains("(the deck has 3 slide(s); 1-3)"),
            "{tag}: the refusal does not name the slides there are: {stderr}"
        );
        assert!(!none.exists(), "{tag}: a refused range still wrote a file");
    }
}

/// An animation's frames are its pages: the range selects among the frames,
/// and each keeps the number it has in the file.
#[test]
fn an_animations_frames_are_pages_of_their_own() {
    let ws = Workspace::new("frames");
    run_ok(&[
        "-i",
        fixture("test-data/images/turning.gif").to_str().unwrap(),
        "-o",
        ws.join("frames.json").to_str().unwrap(),
        "--page-range",
        "2-3",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("frames.json"));
    assert_eq!(page_numbers(&doc), vec![2, 3]);
}

/// A range past the last frame fails naming how many pages the file has, in the
/// unit the reader counted them: an image's frames are pages, whether the file
/// is a GIF, an animated WebP or a TIFF's directories.
#[test]
fn an_out_of_range_frame_names_the_count_and_its_page_unit() {
    let ws = Workspace::new("frames-out-of-range");
    for (tag, relative) in [
        ("gif", "test-data/images/turning.gif"),
        ("webp", "test-data/images/turning.webp"),
        ("tiff", "test-data/images/pages.tiff"),
    ] {
        let output = ws.join(&format!("{tag}.json"));
        let out = run(&[
            "-i",
            fixture(relative).to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--page-range",
            "9",
            "--ocr",
            "never",
        ]);
        assert!(
            !out.status.success(),
            "{tag}: a range past the last frame was accepted"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("names no page")
                && stderr.contains("(the file has 3 page(s); 1-3)"),
            "{tag}: the refusal does not name the pages there are: {stderr}"
        );
        assert!(!output.exists(), "{tag}: a refused range wrote a file");
    }
}

/// A TIFF's directories are its pages — a scanner's batch is a page per sheet —
/// and each keeps the number it has in the file.
#[test]
fn a_tiffs_directories_are_pages_of_their_own() {
    let ws = Workspace::new("tiff");
    run_ok(&[
        "-i",
        fixture("test-data/images/pages.tiff").to_str().unwrap(),
        "-o",
        ws.join("tiff.json").to_str().unwrap(),
        "--page-range",
        "2",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("tiff.json"));
    assert_eq!(page_numbers(&doc), vec![2]);
}

/// An animated WebP's frames are pages as a GIF's are: the range selects among
/// the frames, and each keeps the number it has in the file.
#[test]
fn an_animated_webps_frames_are_pages_of_their_own() {
    let ws = Workspace::new("webp");
    run_ok(&[
        "-i",
        fixture("test-data/images/turning.webp").to_str().unwrap(),
        "-o",
        ws.join("webp.json").to_str().unwrap(),
        "--page-range",
        "2-3",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("webp.json"));
    assert_eq!(page_numbers(&doc), vec![2, 3]);
}

/// A format with no pages is read whole, and says so rather than pretending the
/// range applied.
#[test]
fn a_format_without_pages_is_read_whole_and_says_so() {
    let ws = Workspace::new("flow");
    let input = ws.join("notes.md");
    std::fs::write(&input, b"# Title\n\nBody.\n").expect("write fixture");

    let out = run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "--page-range",
        "2",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("notes.json"));
    assert_eq!(doc["name"], "notes.md");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        logs.contains("read whole"),
        "the run did not say what it did: {logs}"
    );
}
