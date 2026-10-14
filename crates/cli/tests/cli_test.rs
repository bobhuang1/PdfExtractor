//! End-to-end tests of the command-line interface.
//!
//! Each test runs the real `pdfextractor-cli` binary as a child process and
//! inspects the files it wrote, so the argument parsing, format dispatch, output
//! rules and exit codes are all exercised exactly as a user would hit them.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A scratch directory removed when the test ends.
struct Workspace {
    path: PathBuf,
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

impl Workspace {
    fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("pdfe-cli-{}-{}-{tag}", std::process::id(), unique));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self { path }
    }

    fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&path, contents).expect("write fixture");
        path
    }

    fn copy_from_corpus(&self, name: &str, to: &str) -> PathBuf {
        let source = repository_root().join("test-data/real").join(name);
        let bytes = std::fs::read(&source)
            .unwrap_or_else(|e| panic!("missing corpus file {}: {e}", source.display()));
        self.write(to, &bytes)
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

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Run the CLI with `args` and return the completed process.
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

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Progress notes and warnings are emitted as `tracing` records; errors are
/// written to stderr. Tests that look for a message check both streams so they
/// do not depend on which one carries it.
fn logs_of(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The JSON records on stdout, in order.
///
/// Every line of a `--progress` run is a record, so a line that is not JSON is
/// a failure of the stream itself rather than something to skip over.
fn records_of(out: &Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("progress line is not JSON ({e}): {line}"))
        })
        .collect()
}

fn json_of(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("missing output {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

const MARKDOWN: &[u8] = b"# Title\n\nBody paragraph.\n";
const HTML: &[u8] = b"<h1>Heading</h1><p>Hello &amp; world.</p>";
const RTF: &[u8] = b"{\\rtf1\\ansi First\\par Second\\par}";
const TXT: &[u8] = b"line one\nline two\n";
const CSV: &[u8] = b"a,b\n1,2\n";

/// Without `-o`, output is `<input stem>.json` beside the input.
#[test]
fn single_file_writes_json_beside_the_input() {
    let ws = Workspace::new("single");
    let input = ws.write("doc.md", MARKDOWN);

    let out = run_ok(&["-i", input.to_str().unwrap()]);

    let json = ws.join("doc.json");
    assert!(json.exists(), "no output; stderr={}", stderr_of(&out));
    let doc = json_of(&json);
    assert_eq!(doc["origin"]["mimetype"], "text/markdown");
    assert_eq!(doc["name"], "doc.md");
}

/// `--ocr never` must keep the text layer of a real PDF.
#[test]
fn explicit_output_path_is_honoured() {
    let ws = Workspace::new("explicit");
    let input = ws.copy_from_corpus("dummy.pdf", "dummy.pdf");
    let output = ws.join("custom/out.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);

    let doc = json_of(&output);
    assert_eq!(doc["origin"]["mimetype"], "application/pdf");
}

/// A directory is walked recursively by default and the output tree mirrors it.
#[test]
fn directory_recurses_and_mirrors_the_tree() {
    let ws = Workspace::new("dir");
    ws.write("top.md", MARKDOWN);
    ws.write("nested/deep.txt", TXT);
    let out_dir = ws.join("out");

    run_ok(&[
        "-i",
        ws.path.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);

    // `top.json` mirrors `top.md`; `nested/deep.json` mirrors the subdirectory.
    for relative in ["top.json", "nested/deep.json"] {
        let path = out_dir.join(relative);
        assert!(path.exists(), "missing {}", path.display());
        assert_eq!(json_of(&path)["schema_name"], "DoclingDocument");
    }
}

/// `--recursive false` must not descend into subdirectories.
#[test]
fn recursive_false_stays_at_the_top_level() {
    let ws = Workspace::new("norec");
    ws.write("top.md", MARKDOWN);
    ws.write("nested/deep.txt", TXT);
    let out_dir = ws.join("out");

    run_ok(&[
        "-i",
        ws.path.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
        "--recursive",
        "false",
    ]);

    assert!(out_dir.join("top.json").exists());
    assert!(
        !out_dir.join("nested/deep.json").exists(),
        "recursion was not disabled"
    );
}

/// Two inputs with the same stem but different extensions must not overwrite
/// each other; the colliding names keep their full input name.
#[test]
fn colliding_output_names_are_disambiguated() {
    let ws = Workspace::new("collide");
    ws.write("report.txt", TXT);
    ws.write("report.csv", CSV);
    let out_dir = ws.join("out");

    run_ok(&[
        "-i",
        ws.path.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);

    let txt = out_dir.join("report.txt.json");
    let csv = out_dir.join("report.csv.json");
    assert!(txt.exists() && csv.exists(), "one document was dropped");
    assert_eq!(json_of(&txt)["origin"]["mimetype"], "text/plain");
    assert_eq!(json_of(&csv)["origin"]["mimetype"], "text/csv");
}

/// Every added format is dispatched to its own parser with the right mimetype.
#[test]
fn each_new_format_gets_its_own_parser_and_mimetype() {
    let ws = Workspace::new("formats");
    let cases = [
        ("a.md", MARKDOWN, "text/markdown"),
        ("a.html", HTML, "text/html"),
        ("a.rtf", RTF, "application/rtf"),
        ("a.txt", TXT, "text/plain"),
        ("a.csv", CSV, "text/csv"),
    ];
    let out_dir = ws.join("out");
    for (name, bytes, _) in cases {
        ws.write(name, bytes);
    }

    run_ok(&[
        "-i",
        ws.path.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);

    for (name, _, mime) in cases {
        let path = out_dir.join(format!("{name}.json"));
        assert!(path.exists(), "{name} produced no output");
        let doc = json_of(&path);
        assert_eq!(doc["origin"]["mimetype"], mime, "wrong mimetype for {name}");
        assert!(
            !doc["texts"].as_array().unwrap().is_empty()
                || !doc["tables"].as_array().unwrap().is_empty(),
            "{name} produced an empty document"
        );
    }

    // Markdown keeps its heading; CSV keeps its grid.
    let md = json_of(&out_dir.join("a.md.json"));
    assert_eq!(md["texts"][0]["label"], "section_header");
    let csv = json_of(&out_dir.join("a.csv.json"));
    assert_eq!(csv["tables"][0]["data"]["num_rows"], 2);
}

/// A flow-based document has no page rectangle to point at, so it reports no
/// page rather than one it cannot support: a DOCX declares a paper size in
/// `w:sectPr`, but Word repaginates as the text is edited and no block sits on a
/// page. Docling publishes no pages for one either (bug 46).
#[test]
fn flow_documents_report_no_page_and_pdfs_report_theirs() {
    let ws = Workspace::new("pages");
    let text = ws.write("a.txt", TXT);
    let docx = ws.copy_from_corpus("chartex.docx", "charts.docx");
    let pdf = ws.copy_from_corpus("dummy.pdf", "dummy.pdf");

    for input in [&text, &docx, &pdf] {
        run_ok(&["-i", input.to_str().unwrap()]);
    }

    let flow = json_of(&ws.join("a.json"));
    assert_eq!(
        flow["pages"],
        serde_json::json!({}),
        "a text file grew a page"
    );
    let word = json_of(&ws.join("charts.json"));
    assert_eq!(word["pages"], serde_json::json!({}), "a DOCX grew a page");
    assert!(
        !word["texts"].as_array().unwrap().is_empty(),
        "the DOCX produced no items"
    );
    let paged = json_of(&ws.join("dummy.json"));
    assert!(
        paged["pages"].as_object().unwrap().contains_key("1"),
        "the PDF lost its page: {}",
        paged["pages"]
    );
}

/// A page's own furniture comes out labelled, with its text still on the page.
///
/// `basicapi.pdf` prints `page 1 / 3` along the foot of each of its three pages.
/// Those are the page's, not the document's — a consumer that wants the text
/// should not have to read past them — so they are marked as the furniture layer
/// while staying where the page printed them.
#[test]
fn a_page_number_comes_out_as_the_pages_own_furniture() {
    let ws = Workspace::new("furniture");
    let input = ws.copy_from_corpus("basicapi.pdf", "basicapi.pdf");

    run_ok(&["-i", input.to_str().unwrap(), "--ocr", "never"]);

    let doc = json_of(&ws.join("basicapi.json"));
    let texts = doc["texts"].as_array().expect("text items");
    let mut numbers = Vec::new();
    for item in texts {
        if item["label"] == "page_footer" {
            numbers.push(item["text"].as_str().unwrap_or_default().to_string());
            assert_eq!(item["content_layer"], "furniture", "a footer is the page's");
            assert_eq!(
                item["parent"]["$ref"], "#/body",
                "but it stands on the page"
            );
        }
    }
    assert_eq!(numbers, ["page 1 / 3", "page 2 / 3", "page 3 / 3"]);

    // The document's own text is untouched, and nothing was lost with them.
    assert!(
        texts
            .iter()
            .any(|t| t["text"] == "Chapter 1" && t["label"] == "section_header"),
        "the document's own heading changed: {texts:?}"
    );
    let body: Vec<&str> = doc["body"]["children"]
        .as_array()
        .expect("body children")
        .iter()
        .map(|child| child["$ref"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        body.len(),
        texts.len(),
        "a page's furniture must stay in the body order rather than be dropped"
    );
}

/// A legacy `.doc` converts instead of failing.
#[test]
fn legacy_doc_converts() {
    let ws = Workspace::new("doc");
    let input = ws.copy_from_corpus("two_images.doc", "two.doc");

    run_ok(&["-i", input.to_str().unwrap()]);

    let doc = json_of(&ws.join("two.json"));
    assert_eq!(doc["origin"]["mimetype"], "application/msword");
    assert_eq!(doc["pictures"].as_array().unwrap().len(), 2);
    let texts: Vec<&str> = doc["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["text"].as_str().unwrap())
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("the jpg file")),
        "{texts:?}"
    );
}

/// The file a picture names, checked against the media type the reference
/// claims. Returns that media type.
///
/// A payload that is not the format it is named is a lie the whole document
/// inherits: an EMF written under a `.png` name was bug 14.
fn read_named_image<'a>(ws: &Workspace, picture: &'a serde_json::Value) -> &'a str {
    let image = &picture["image"];
    let mime = image["mimetype"]
        .as_str()
        .unwrap_or_else(|| panic!("a picture with no image: {picture}"));
    let uri = image["uri"]
        .as_str()
        .unwrap_or_else(|| panic!("an image with no file: {picture}"));
    let bytes = std::fs::read(ws.join(uri)).unwrap_or_else(|e| panic!("{uri}: {e}"));
    match mime {
        "image/png" => assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "{uri} is named a PNG and is not one"
        ),
        "image/jpeg" => assert_eq!(&bytes[..2], &[0xff, 0xd8], "{uri}"),
        other => panic!("unexpected media type {other} in {uri}"),
    }
    mime
}

/// `drawings_images` holds no metafile: everything the document drew has been
/// played, and nothing is left behind unplayed.
fn assert_no_drawing_files_left(dir: &Path) {
    for entry in std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.ends_with(".emf") && !name.ends_with(".wmf"),
            "an unplayed drawing was written out: {name}"
        );
    }
}

/// A drawing Word stored as a metafile (`image/emf`, `image/wmf`) is played
/// into a PNG, so a picture that used to be a box with no image is one the
/// document names in an `image` reference — and the file written beside the
/// JSON really is the format that reference claims.
#[test]
fn a_vector_drawing_becomes_a_named_png() {
    let ws = Workspace::new("vector");
    let input = ws.copy_from_corpus("VariousPictures.docx", "pictures.docx");

    run_ok(&["-i", input.to_str().unwrap()]);

    let doc = json_of(&ws.join("pictures.json"));
    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(pictures.len(), 5, "expected the five drawings");

    // Three drawings (two EMF, one WMF) join the PNG the document already had.
    let pngs = pictures
        .iter()
        .filter(|picture| read_named_image(&ws, picture) == "image/png")
        .count();
    assert_eq!(pngs, 4, "the drawings were not rasterised");

    assert_no_drawing_files_left(&ws.join("pictures_images"));
}

/// An embedded object (`w:object`, a MathType equation or a pasted drawing)
/// draws a metafile preview through `v:imagedata`, and in `mathtype.docx` that
/// preview is the equation's only picture. It is played like any other drawing,
/// at the size the object declares.
#[test]
fn an_embedded_objects_preview_becomes_a_named_png() {
    let ws = Workspace::new("object");
    let input = ws.copy_from_corpus("mathtype.docx", "equation.docx");

    run_ok(&["-i", input.to_str().unwrap()]);

    let doc = json_of(&ws.join("equation.json"));
    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(pictures.len(), 1, "the preview is the equation's picture");
    assert_eq!(read_named_image(&ws, &pictures[0]), "image/png");

    // `w:object/@w:dxaOrig` is in twips: 617 x 566 is 30.85 x 28.3 pt, which at
    // the 144 dpi a drawing is rasterised at is a 62 x 57 pixel PNG.
    assert_eq!(pictures[0]["image"]["size"]["width"], 62.0);
    assert_eq!(pictures[0]["image"]["size"]["height"], 57.0);

    let texts: Vec<&str> = doc["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["text"].as_str().unwrap())
        .collect();
    assert!(texts.iter().any(|t| t.contains("Before")), "{texts:?}");

    assert_no_drawing_files_left(&ws.join("equation_images"));
}

/// Several documents convert in parallel without interference.
#[test]
fn parallel_conversion_writes_every_document() {
    let ws = Workspace::new("parallel");
    for i in 0..6 {
        ws.write(&format!("doc{i}.txt", i = i), TXT);
    }
    let out_dir = ws.join("out");

    run_ok(&[
        "-i",
        ws.path.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
        "-p",
        "4",
    ]);

    for i in 0..6 {
        assert!(
            out_dir.join(format!("doc{i}.json", i = i)).exists(),
            "doc{i} was not written",
            i = i
        );
    }
}

/// `--progress` is what the desktop window renders: the queue first, then one
/// record per finished document, numbered in the order they finish.
#[test]
fn progress_streams_the_plan_then_every_document() {
    let ws = Workspace::new("progress");
    ws.write("a.md", MARKDOWN);
    ws.write("nested/b.txt", TXT);
    let out_dir = ws.join("out");

    let out = run_ok(&[
        "-i",
        ws.path.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
        "--progress",
        "-p",
        "2",
    ]);
    let records = records_of(&out);

    // One line per document plus the plan, and nothing else on stdout.
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(records[0]["kind"], "plan");
    assert_eq!(records[0]["total"], 2);
    let inputs = records[0]["inputs"].as_array().expect("a list of inputs");
    assert_eq!(inputs.len(), 2, "the plan must list the whole queue");

    let progress: Vec<&serde_json::Value> = records
        .iter()
        .filter(|record| record["kind"] == "progress")
        .collect();
    assert_eq!(progress.len(), 2, "one record per document");
    for (i, record) in progress.iter().enumerate() {
        assert_eq!(record["ok"], true, "{record}");
        assert_eq!(record["done"], i + 1, "the count must run in order");
        assert_eq!(record["total"], 2);
        let output = record["output"].as_str().expect("the output path");
        assert!(
            Path::new(output).exists(),
            "{output} was reported but not written"
        );
    }

    // Every planned document is reported exactly once. The *order* is not the
    // plan's: records arrive as documents finish, and with two workers on two
    // tiny files either may finish first. That is what `done` is for.
    let mut reported: Vec<&str> = progress
        .iter()
        .map(|record| record["input"].as_str().expect("the input path"))
        .collect();
    reported.sort_unstable();
    let mut planned: Vec<&str> = inputs.iter().map(|v| v.as_str().unwrap()).collect();
    planned.sort_unstable();
    assert_eq!(
        reported, planned,
        "the stream skipped or repeated a document"
    );
}

/// A document that fails is reported as its own record, and the rest of the run
/// still happens.
#[test]
fn a_failed_document_appears_in_the_progress_stream() {
    let ws = Workspace::new("progressfail");
    // A `.docx` that is not a ZIP package: a supported extension whose
    // conversion really does fail, which is the case this test is about.
    let broken = ws.write("broken.docx", b"this is not a zip package at all");
    ws.write("good.md", MARKDOWN);
    let out_dir = ws.join("out");

    let out = run(&[
        "-i",
        ws.path.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
        "--progress",
    ]);
    assert!(!out.status.success(), "a failed document must fail the run");
    let records = records_of(&out);

    let failed: Vec<&serde_json::Value> = records
        .iter()
        .filter(|record| record["kind"] == "progress" && record["ok"] == false)
        .collect();
    assert_eq!(failed.len(), 1, "{records:?}");
    assert_eq!(failed[0]["input"], broken.to_string_lossy().as_ref());
    assert!(
        !failed[0]["error"].as_str().unwrap_or_default().is_empty(),
        "a failure must carry its reason: {}",
        failed[0]
    );
    // A failure still counts as a finished document, exactly once.
    let mut done: Vec<u64> = records
        .iter()
        .filter(|record| record["kind"] == "progress")
        .map(|record| record["done"].as_u64().expect("a count"))
        .collect();
    done.sort_unstable();
    assert_eq!(done, vec![1, 2], "{records:?}");

    let converted = records
        .iter()
        .filter(|record| record["kind"] == "progress" && record["ok"] == true)
        .count();
    assert_eq!(converted, 1, "the other document was abandoned");
    assert!(
        out_dir.join("good.json").exists(),
        "the document that could be converted was not written"
    );
}

/// The records are opt-in: a run without `--progress` keeps its stdout clean,
/// so nothing downstream has to filter JSON out of it.
#[test]
fn progress_records_are_opt_in() {
    let ws = Workspace::new("nojson");
    let input = ws.write("doc.md", MARKDOWN);

    let out = run_ok(&["-i", input.to_str().unwrap()]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("\"kind\""),
        "plain output became machine output: {stdout}"
    );
}

/// Asking for more parallelism than the spec's practical ceiling clamps with a
/// warning, and still succeeds.
#[test]
fn an_excessive_parallelism_is_clamped() {
    let ws = Workspace::new("clamp");
    ws.write("doc.md", MARKDOWN);

    let out = run_ok(&["-i", ws.path.to_str().unwrap(), "-p", "1000"]);
    assert!(
        logs_of(&out).contains("clamping"),
        "expected a clamp warning, got: {}",
        logs_of(&out)
    );
}

/// Flow formats have no page geometry, so `--preview` is skipped with a note and
/// no HTML is written.
#[test]
fn preview_is_skipped_for_flow_formats() {
    let ws = Workspace::new("preview");
    let input = ws.write("doc.md", MARKDOWN);

    let out = run_ok(&["-i", input.to_str().unwrap(), "--preview"]);

    assert!(
        logs_of(&out).contains("flow-based"),
        "expected the skip note, got: {}",
        logs_of(&out)
    );
    assert!(
        !ws.join("doc.preview.html").exists(),
        "an empty preview was written anyway"
    );
    assert!(ws.join("doc.json").exists(), "the conversion did not run");
}

/// A PDF still gets its overlay.
#[test]
fn preview_is_written_for_a_pdf() {
    let ws = Workspace::new("pdfpreview");
    let input = ws.copy_from_corpus("dummy.pdf", "dummy.pdf");
    let output = ws.join("dummy.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--preview",
    ]);

    let preview = ws.join("dummy.preview.html");
    assert!(preview.exists(), "no preview was written");
    let html = std::fs::read_to_string(&preview).expect("read preview");
    assert!(html.contains("<html"), "preview is not HTML");
}

/// A file the extractor does not understand fails with a clear error.
#[test]
fn an_unsupported_file_errors() {
    let ws = Workspace::new("unsupported");
    let input = ws.write("thing.xyz", b"nonsense");

    let out = run(&["-i", input.to_str().unwrap()]);

    assert!(!out.status.success(), "expected a non-zero exit");
    assert!(
        stderr_of(&out).contains("unsupported extension"),
        "unclear error: {}",
        stderr_of(&out)
    );
}

/// A directory with nothing convertible is an error, not a silent success.
#[test]
fn an_empty_directory_errors() {
    let ws = Workspace::new("empty");

    let out = run(&["-i", ws.path.to_str().unwrap()]);

    assert!(!out.status.success(), "expected a non-zero exit");
    assert!(
        stderr_of(&out).contains("no supported documents"),
        "unclear error: {}",
        stderr_of(&out)
    );
}

/// A missing input path is reported before anything else happens.
#[test]
fn a_missing_input_errors() {
    let out = run(&["-i", "/definitely/not/here.pdf"]);
    assert!(!out.status.success(), "expected a non-zero exit");
    assert!(
        stderr_of(&out).contains("does not exist"),
        "unclear error: {}",
        stderr_of(&out)
    );
}
