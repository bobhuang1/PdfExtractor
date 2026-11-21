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
        stderr.contains("names no sheet") && stderr.contains("(the workbook has 3 sheet(s); 1-3)"),
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
        assert_eq!(
            page_numbers(&doc),
            vec![2],
            "{tag}: the range kept more than slide 2"
        );
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
            stderr.contains("names no slide") && stderr.contains("(the deck has 3 slide(s); 1-3)"),
            "{tag}: the refusal does not name the slides there are: {stderr}"
        );
        assert!(!none.exists(), "{tag}: a refused range still wrote a file");
    }
}

/// The prose writers see the same selection: `1,3` renders the first and third
/// slides, with both slides' speaker notes and the third slide's picture, and
/// nothing from the unnamed second slide. The picture is written beside the
/// prose and both formats point at that file.
#[test]
fn selected_slides_keep_their_notes_and_pictures_in_markdown_and_html() {
    let ws = Workspace::new("rendered-selection");
    let output = ws.join("deck.json");
    run_ok(&[
        "-i",
        fixture("test-data/q3_review.pptx").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--page-range",
        "1,3",
        "--to",
        "md",
        "--to",
        "html",
        "--include-notes",
        "--ocr",
        "never",
    ]);

    let markdown = std::fs::read_to_string(ws.join("deck.md")).expect("the Markdown");
    let html = std::fs::read_to_string(ws.join("deck.html")).expect("the HTML");
    let first_note = "Open with the revenue line, then the regions.";
    let third_note = "Say the picture is the growth by quarter; do not read the table aloud.";
    let picture = "deck_images/slide3_1.png";

    for reading in [&markdown, &html] {
        assert!(
            reading.contains("Q3 Review"),
            "slide 1 is missing:\n{reading}"
        );
        assert!(
            reading.contains("Where the growth came from"),
            "slide 3 is missing:\n{reading}"
        );
        assert!(
            reading.contains(first_note) && reading.contains(third_note),
            "a selected slide's notes are missing:\n{reading}"
        );
        assert!(
            !reading.contains("Regional performance"),
            "slide 2 was rendered too:\n{reading}"
        );
    }
    assert!(
        markdown.contains(&format!("![Image]({picture})")),
        "the Markdown does not link the selected slide's picture:\n{markdown}"
    );
    assert!(
        html.contains(&format!("<img src=\"{picture}\" alt=\"Image\">")),
        "the HTML does not show the selected slide's picture:\n{html}"
    );
    assert!(
        html.contains("<aside class=\"notes\">"),
        "the HTML notes are not marked as notes:\n{html}"
    );
    let image = std::fs::read(ws.join(picture)).expect("the extracted slide picture");
    assert!(
        image.starts_with(b"\x89PNG\r\n\x1a\n") && image.len() > 8,
        "the picture beside the prose is not a PNG"
    );
}

/// A span keeps every selected slide in deck order when one of them has neither
/// speaker notes nor pictures. In `q3_review.pptx` the middle slide is body text
/// and a table only; the chapters still run 1, 2, 3, and the notes and picture on
/// slide 3 stay after slide 2 rather than moving up beside slide 1's notes.
#[test]
fn a_slide_without_notes_or_pictures_keeps_its_place_in_a_span() {
    let ws = Workspace::new("span-empty-slide");
    let output = ws.join("deck.json");
    run_ok(&[
        "-i",
        fixture("test-data/q3_review.pptx").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--page-range",
        "1-3",
        "--to",
        "json",
        "--to",
        "md",
        "--include-notes",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    assert_eq!(page_numbers(&doc), vec![1, 2, 3]);
    let groups = doc["groups"].as_array().expect("a group list");
    let chapters: Vec<&str> = groups
        .iter()
        .filter(|group| group["label"] == "chapter")
        .map(|group| group["name"].as_str().expect("a chapter name"))
        .collect();
    assert_eq!(
        chapters,
        vec!["slide-0", "slide-1", "slide-2"],
        "the empty middle slide disturbed the deck order"
    );

    // The middle chapter really is the selected slide without notes or pictures:
    // its own members are body text and a table, while the remarks and picture
    // groups stay with the slides that hold them.
    let middle = groups
        .iter()
        .find(|group| group["name"] == "slide-1")
        .expect("slide 2's chapter");
    for child in middle["children"].as_array().expect("chapter children") {
        let reference = child["$ref"].as_str().expect("a reference");
        let (kind, index) = reference
            .strip_prefix("#/")
            .expect("a document reference")
            .split_once('/')
            .expect("a kind and an index");
        let index: usize = index.parse().expect("an index");
        match kind {
            "texts" => assert_ne!(
                doc["texts"][index]["content_layer"], "notes",
                "slide 2's chapter holds a note: {reference}"
            ),
            "pictures" => panic!("slide 2's chapter holds a picture: {reference}"),
            _ => {}
        }
    }

    let markdown = std::fs::read_to_string(ws.join("deck.md")).expect("the Markdown");
    let positions: Vec<usize> = chapters
        .iter()
        .map(|name| {
            markdown
                .find(&format!("group chapter body {name}"))
                .unwrap_or_else(|| panic!("{name} is not marked in:\n{markdown}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "Markdown rendered the chapters out of order: {positions:?}"
    );
    let picture = markdown
        .find("![Image](deck_images/slide3_1.png)")
        .expect("slide 3's picture");
    assert!(
        picture > positions[2]
            && markdown.find("Say the picture is the growth").unwrap() > positions[2],
        "slide 3's notes or picture moved before the middle slide:\n{markdown}"
    );
}

/// A legacy `.ppt` is read at all only through the bridge, and the range still
/// selects among the slides Impress writes: `1,3` keeps the first and the third,
/// each with its own number, and nothing of slide 2 comes through. The bridged
/// package keeps the original file's identity too. Skipped where LibreOffice is
/// not installed, like the other bridge tests.
#[test]
fn a_bridged_deck_is_read_as_the_named_slides() {
    let Some(soffice) = soffice_binary() else {
        skipped(
            "the bridged page-range test",
            "no soffice on PATH, so nothing was asserted",
        );
        return;
    };
    let ws = Workspace::new("bridged-slides");
    let output = ws.join("slides-1-3.json");
    let out = run_with_soffice(
        &soffice,
        &[
            "-i",
            fixture("test-data/legacy_deck.ppt").to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--page-range",
            "1,3",
            "--ocr",
            "never",
        ],
    );
    assert!(
        out.status.success(),
        "the bridged deck failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let doc = json_of(&output);
    assert_eq!(page_numbers(&doc), vec![1, 3]);
    let texts: Vec<&str> = doc["texts"]
        .as_array()
        .expect("texts")
        .iter()
        .filter_map(|item| item["text"].as_str())
        .collect();
    assert!(texts.contains(&"Q3 Review"), "{texts:?}");
    assert!(texts.contains(&"Where the growth came from"), "{texts:?}");
    assert!(
        !texts.contains(&"Regional performance") && !texts.contains(&"Open the dashboard."),
        "slide 2 was read too: {texts:?}"
    );

    // Every item belongs to one of the kept slides, and the slide that was not
    // named has no group at all.
    for item in doc["texts"].as_array().expect("texts") {
        for prov in item["prov"].as_array().expect("prov") {
            let page = prov["page_no"].as_i64().expect("a page number");
            assert!(page == 1 || page == 3, "an item on page {page}: {item:#?}");
        }
    }
    let groups: Vec<&str> = doc["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .filter_map(|group| group["name"].as_str())
        .collect();
    assert!(
        groups.contains(&"slide-0") && groups.contains(&"slide-2"),
        "{groups:?}"
    );
    assert!(
        !groups.contains(&"slide-1"),
        "the unnamed slide has a group: {groups:?}"
    );

    // The conversion is how the words were reached; the document is the file
    // the user handed over.
    assert_eq!(doc["name"], "legacy_deck.ppt");
    assert_eq!(doc["origin"]["filename"], "legacy_deck.ppt");
    assert_eq!(doc["origin"]["mimetype"], "application/vnd.ms-powerpoint");

    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(logs.contains("read through LibreOffice as pptx"), "{logs}");
    assert!(
        logs.contains("reading 2 of 3 slide(s) (1,3)"),
        "the bridged run did not say which slides it read: {logs}"
    );
}

/// A range past the last page is refused with the count the bridged reader
/// found: a workbook's sheets and a deck's slides are counted after LibreOffice
/// has written the package that reader opens, and the refusal names the unit and
/// the range. Skipped where LibreOffice is not installed.
#[test]
fn a_bridged_range_past_the_end_names_the_sheets_and_slides_there_are() {
    let Some(soffice) = soffice_binary() else {
        skipped(
            "the bridged out-of-range test",
            "no soffice on PATH, so nothing was asserted",
        );
        return;
    };
    let ws = Workspace::new("bridged-past-end");
    for (tag, relative, refusal, count) in [
        (
            "xls",
            "test-data/ledger.xls",
            "names no sheet",
            "(the workbook has 3 sheet(s); 1-3)",
        ),
        (
            "ppt",
            "test-data/legacy_deck.ppt",
            "names no slide",
            "(the deck has 3 slide(s); 1-3)",
        ),
    ] {
        let output = ws.join(&format!("{tag}-none.json"));
        let out = run_with_soffice(
            &soffice,
            &[
                "-i",
                fixture(relative).to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--page-range",
                "9",
                "--ocr",
                "never",
            ],
        );
        assert!(
            !out.status.success(),
            "{tag}: a range past the last page was accepted"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("--page-range 9") && stderr.contains(refusal),
            "{tag}: the refusal does not name the range and the unit: {stderr}"
        );
        assert!(
            stderr.contains(count),
            "{tag}: the refusal does not name the count: {stderr}"
        );
        assert!(!output.exists(), "{tag}: a refused range wrote a file");
    }
}

/// LibreOffice's `soffice`, from `PDFE_SOFFICE` or `PATH`; `None` where the
/// bridge cannot be run, so the bridged test skips rather than failing the
/// machine. The lookup is the CLI's own.
fn soffice_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PDFE_SOFFICE").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let search = std::env::var_os("PATH")?;
    std::env::split_paths(&search)
        .flat_map(|directory| ["soffice", "libreoffice"].map(|name| directory.join(name)))
        .find(|candidate| candidate.is_file())
}

/// Run the CLI with `PDFE_SOFFICE` naming the LibreOffice this machine has.
fn run_with_soffice(soffice: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args(args)
        .env("RUST_LOG", "info")
        .env("PDFE_SOFFICE", soffice)
        .output()
        .expect("spawn the CLI binary")
}

/// Say that a test asserted nothing, where `cargo test` will show it.
///
/// The harness captures a passing test's output unless the run was given
/// `--nocapture`, so a skip said that way would be invisible and a bridgeless
/// runner's green would look like a bridged deck that was read.
fn skipped(what: &str, why: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout(), "SKIPPED: {what} — {why}");
}

/// Every provenance record in a document, wherever it sits.
fn provenance_records(value: &serde_json::Value, records: &mut Vec<serde_json::Value>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(prov) = map.get("prov").and_then(serde_json::Value::as_array) {
                records.extend(prov.iter().cloned());
            }
            for value in map.values() {
                provenance_records(value, records);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                provenance_records(item, records);
            }
        }
        _ => {}
    }
}

/// An animation's frames are its pages: the range selects among the frames,
/// and each keeps the number it has in the file. A GIF and an animated WebP
/// are the same reading, so a new animated format only adds a row.
#[test]
#[cfg_attr(
    not(feature = "ocr"),
    ignore = "reads text by OCR: needs the `ocr` feature (Tesseract)"
)]
fn an_animations_frames_are_pages_of_their_own() {
    let ws = Workspace::new("frames");
    for (tag, relative, label) in [
        ("gif", "test-data/images/turning.gif", "FRAME"),
        ("webp", "test-data/images/turning.webp", "WEBP"),
    ] {
        let output = ws.join(&format!("{tag}.json"));
        let out = run_ok(&[
            "-i",
            fixture(relative).to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--page-range",
            "2-3",
            "--ocr",
            "always",
        ]);

        let doc = json_of(&output);
        assert_eq!(
            page_numbers(&doc),
            vec![2, 3],
            "{tag}: the wrong frames were kept"
        );
        let texts = doc["texts"].as_array().expect("texts");
        let words = texts
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        for kept in [format!("{label} TWO"), format!("{label} THREE")] {
            assert!(
                words.contains(kept.as_str()),
                "{tag}: the selected frame's words {kept:?} are missing: {words:?}"
            );
        }
        assert!(
            !words.contains(&format!("{label} ONE")),
            "{tag}: a skipped frame's words survived: {words:?}"
        );
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            logs.contains("reading 2 of 3 frames (2-3)"),
            "{tag}: the log does not name the frames it read: {logs}"
        );
    }
}

/// A range that names one page of an image sequence keeps that page's number in
/// the file: the second frame or directory is page 2, not the first page of a
/// one-page document, and no provenance record points anywhere else. GIF and
/// animated WebP frames and a TIFF's directories are the same reading, so a new
/// frame format only adds a row.
#[test]
#[cfg_attr(
    not(feature = "ocr"),
    ignore = "reads text by OCR: needs the `ocr` feature (Tesseract)"
)]
fn a_single_page_keeps_its_original_page_number() {
    let ws = Workspace::new("one-page");
    for (tag, relative, label, unit) in [
        ("gif", "test-data/images/turning.gif", "FRAME", "frames"),
        ("webp", "test-data/images/turning.webp", "WEBP", "frames"),
        ("tiff", "test-data/images/pages.tiff", "PAGE", "directories"),
    ] {
        let output = ws.join(&format!("{tag}-page2.json"));
        let out = run_ok(&[
            "-i",
            fixture(relative).to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--page-range",
            "2",
            "--ocr",
            "always",
        ]);

        let doc = json_of(&output);
        assert_eq!(
            page_numbers(&doc),
            vec![2],
            "{tag}: the wrong page was kept"
        );

        let mut records = Vec::new();
        provenance_records(&doc, &mut records);
        assert!(
            !records.is_empty(),
            "{tag}: the range produced no provenance to check: {doc:#?}"
        );
        for record in &records {
            assert_eq!(
                record["page_no"], 2,
                "{tag}: a provenance record points at another page: {record:#?}"
            );
        }

        // Prove the page read is the one the range named, not merely page 2 of
        // a document that started again at 1.
        let texts = doc["texts"].as_array().expect("texts");
        let words = texts
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        let kept = format!("{label} TWO");
        let skipped = [format!("{label} ONE"), format!("{label} THREE")];
        assert!(
            words.contains(kept.as_str()),
            "{tag}: page 2's words are missing: {words:?}"
        );
        assert!(
            skipped
                .iter()
                .all(|skipped| !words.contains(skipped.as_str())),
            "{tag}: a skipped page's words survived: {words:?}"
        );

        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let single = format!("reading 1 of 3 {unit} (2)");
        assert!(
            logs.contains(single.as_str()),
            "{tag}: the log does not name the single page it read: {logs}"
        );
    }
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
            stderr.contains("names no page") && stderr.contains("(the file has 3 page(s); 1-3)"),
            "{tag}: the refusal does not name the pages there are: {stderr}"
        );
        assert!(!output.exists(), "{tag}: a refused range wrote a file");
    }
}

/// An out-of-order selection does not reorder the frames: `3,1` reads frames 1
/// and 3 in the file's order, each with its own page number, and leaves frame 2
/// out. The log keeps the range as the caller wrote it. A GIF's and an animated
/// WebP's frames and a TIFF's directories are the same reading, so a new frame
/// format only adds a row.
#[test]
#[cfg_attr(
    not(feature = "ocr"),
    ignore = "reads text by OCR: needs the `ocr` feature (Tesseract)"
)]
fn out_of_order_frames_keep_file_order_and_their_page_numbers() {
    let ws = Workspace::new("frames-3-1");
    for (tag, relative, label, unit) in [
        ("gif", "test-data/images/turning.gif", "FRAME", "frames"),
        ("webp", "test-data/images/turning.webp", "WEBP", "frames"),
        ("tiff", "test-data/images/pages.tiff", "PAGE", "directories"),
    ] {
        let output = ws.join(&format!("{tag}.json"));
        let out = run_ok(&[
            "-i",
            fixture(relative).to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--page-range",
            "3,1",
            "--ocr",
            "always",
        ]);

        let doc = json_of(&output);
        assert_eq!(
            page_numbers(&doc),
            vec![1, 3],
            "{tag}: the selected pages do not keep their file numbers"
        );
        let prefix = format!("{label} ");
        let pages: Vec<(i64, &str)> = doc["texts"]
            .as_array()
            .expect("texts")
            .iter()
            .filter_map(|item| {
                let text = item["text"].as_str()?;
                if !text.starts_with(prefix.as_str()) {
                    return None;
                }
                let page = item["prov"].as_array()?.first()?["page_no"].as_i64()?;
                Some((page, text))
            })
            .collect();
        let one = format!("{label} ONE");
        let three = format!("{label} THREE");
        assert_eq!(
            pages,
            vec![(1, one.as_str()), (3, three.as_str())],
            "{tag}: the pages are not in file order with their original numbers"
        );
        for item in doc["texts"].as_array().expect("texts") {
            for prov in item["prov"].as_array().expect("prov") {
                let page = prov["page_no"].as_i64().expect("a page number");
                assert!(
                    page == 1 || page == 3,
                    "{tag}: an item points at skipped page 2: {item:#?}"
                );
            }
        }

        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            logs.contains(&format!("reading 2 of 3 {unit} (3,1)")),
            "{tag}: the log does not keep the range as written: {logs}"
        );
    }
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
