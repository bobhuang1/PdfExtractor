//! End-to-end tests of the command-line interface.
//!
//! Each test runs the real `pdfextractor-cli` binary as a child process and
//! inspects the files it wrote, so the argument parsing, format dispatch, output
//! rules and exit codes are all exercised exactly as a user would hit them.

use base64::{engine::general_purpose::STANDARD, Engine as _};
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

/// A committed fixture from `test-data/`, by path inside it.
fn fixture(name: &str) -> Vec<u8> {
    let path = repository_root().join("test-data").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()))
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

    // Markdown keeps its heading — the only `#` in the file is the title, as
    // Docling's own Markdown backend reads it; CSV keeps its grid.
    let md = json_of(&out_dir.join("a.md.json"));
    assert_eq!(md["texts"][0]["label"], "title");
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
        "image/bmp" => assert_eq!(&bytes[..2], b"BM", "{uri} is named a BMP and is not one"),
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

/// An RTF `\pict` group is a picture, not a run of hex: the reader turns its
/// payload back into the bytes the file carried, and the conversion writes them
/// beside the JSON and names them from the Markdown, so a render of an RTF keeps
/// its images.
#[test]
fn an_rtf_picture_becomes_a_named_png() {
    // A 2×2 red PNG, written here rather than committed as a binary fixture.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xfd,
        0xd4, 0x9a, 0x73, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x44, 0x0c, 0x10, 0x0a, 0x00, 0x1f, 0xee, 0x03, 0xfd, 0x8b, 0x5f, 0x14,
        0xd4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let ws = Workspace::new("rtf-picture");
    let hex: String = TINY_PNG.iter().map(|byte| format!("{byte:02x}")).collect();
    // The shape LibreOffice writes: a picture-property group first, then the
    // blip's own type and the payload as hex.
    let rtf = format!(
        "{{\\rtf1\\ansi Before {{\\pict{{\\*\\picprop}}\\pngblip\\picw2\\pich2 {hex}}} after\\par}}"
    );
    let input = ws.write("illustrated.rtf", rtf.as_bytes());

    run_ok(&["-i", input.to_str().unwrap(), "--to", "json", "--to", "md"]);

    let doc = json_of(&ws.join("illustrated.json"));
    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(pictures.len(), 1, "the picture was dropped: {pictures:#?}");
    assert_eq!(read_named_image(&ws, &pictures[0]), "image/png");
    assert_eq!(pictures[0]["image"]["size"]["width"], 2.0);
    assert_eq!(pictures[0]["image"]["size"]["height"], 2.0);
    assert_eq!(
        std::fs::read(ws.join("illustrated_images/img1.png"))
            .expect("the picture written beside the JSON"),
        TINY_PNG,
        "the picture's bytes were not carried through"
    );

    // The words around it are still words, and the picture stands between them.
    let texts: Vec<&str> = doc["texts"]
        .as_array()
        .expect("texts")
        .iter()
        .map(|item| item["text"].as_str().expect("a text item"))
        .collect();
    assert_eq!(texts, vec!["Before", "after"]);
    let markdown = std::fs::read_to_string(ws.join("illustrated.md")).expect("the Markdown");
    assert!(
        markdown.contains("![Image](illustrated_images/img1.png)"),
        "{markdown}"
    );
}

/// Word's oldest pictures are `\wbitmap` (or `\dibitmap`) groups whose payload
/// is a DIB — the header and pixels with no `BM` file header, because the RTF
/// file was the container. Wrapping it in that header is what turns one into a
/// picture a reader can open: one `image/bmp` item, sized from the DIB's own
/// header, written as a `.bmp` whose bytes are the payload behind a file header
/// that names where they start.
#[test]
fn an_old_word_bitmap_becomes_a_readable_bmp() {
    // A 3 × 2 24-bit DIB: a `BITMAPINFOHEADER`, then two rows of three pixels
    // padded to four bytes — what Word 6 and 95 wrote for a picture.
    let mut dib = Vec::new();
    dib.extend_from_slice(&40u32.to_le_bytes()); // header size
    dib.extend_from_slice(&3i32.to_le_bytes()); // width
    dib.extend_from_slice(&2i32.to_le_bytes()); // height
    dib.extend_from_slice(&1u16.to_le_bytes()); // planes
    dib.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
    dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    dib.extend_from_slice(&12u32.to_le_bytes()); // pixel data size
    dib.extend_from_slice(&2835i32.to_le_bytes()); // x pixels per metre
    dib.extend_from_slice(&2835i32.to_le_bytes()); // y pixels per metre
    dib.extend_from_slice(&[0u8; 8]); // palette entries used, important
    for _ in 0..2 {
        dib.extend_from_slice(&[0x20, 0x60, 0xa0, 0x20, 0x60, 0xa0, 0x20, 0x60, 0xa0]);
        dib.extend_from_slice(&[0, 0, 0]);
    }

    let ws = Workspace::new("rtf-wbitmap");
    let hex: String = dib.iter().map(|byte| format!("{byte:02x}")).collect();
    let rtf = format!("{{\\rtf1\\ansi Before {{\\pict\\wbitmap0 {hex}}} after\\par}}");
    let input = ws.write("old_word.rtf", rtf.as_bytes());

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("old_word.json"));
    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(pictures.len(), 1, "the bitmap was dropped: {pictures:#?}");
    assert_eq!(read_named_image(&ws, &pictures[0]), "image/bmp");
    assert_eq!(pictures[0]["image"]["size"]["width"], 3.0);
    assert_eq!(pictures[0]["image"]["size"]["height"], 2.0);

    // The words around it are still words.
    let texts: Vec<&str> = doc["texts"]
        .as_array()
        .expect("texts")
        .iter()
        .map(|item| item["text"].as_str().expect("a text item"))
        .collect();
    assert_eq!(texts, vec!["Before", "after"]);

    // The file beside the JSON is a BMP file: the DIB's own bytes behind the
    // header that says how long the file is and where the pixels begin.
    let uri = pictures[0]["image"]["uri"].as_str().expect("an image uri");
    assert!(
        uri.ends_with(".bmp"),
        "the picture is not written as a BMP: {uri}"
    );
    let written = std::fs::read(ws.join(uri)).expect("the picture beside the JSON");
    let word = |at: usize| u32::from_le_bytes(written[at..at + 4].try_into().expect("a word"));
    assert_eq!(word(2) as usize, written.len(), "the file's size is wrong");
    assert_eq!(word(10), 14 + 40, "the pixels do not start past the header");
    assert_eq!(&written[14..], &dib[..], "the DIB's bytes were rewritten");
    // A header that parses is not yet a picture: the pixels have to come out
    // the size the DIB declared.
    let decoded = image::load_from_memory(&written).expect("the wrapped BMP decodes");
    assert_eq!((decoded.width(), decoded.height()), (3, 2));
    assert_eq!(
        decoded.to_rgb8().get_pixel(0, 0).0,
        [0xa0, 0x60, 0x20],
        "the pixel colour is not the DIB's (BGR) bytes"
    );

    let markdown = std::fs::read_to_string(ws.join("old_word.md")).expect("the Markdown");
    assert!(
        markdown.contains(&format!("![Image]({uri})")),
        "the render does not name the picture: {markdown}"
    );
}

/// A committed RTF in Word's shape, converted the way a reader would: the
/// bitmap it carries has to reach the output as the BMP it came from.
///
/// `word_bitmap.rtf` is a whole document — a font table, a paragraph,
/// `\pict\wbitmap0` with the DIB's hex, a paragraph after — showing
/// `test-data/images/rtf_bitmap.bmp` (24 × 16, six colour blocks). Word writes
/// the DIB with no `BITMAPFILEHEADER`, so the conversion has to put one back,
/// and the file beside the JSON has to be the source's own bytes: the wrapper
/// states the same length and pixel offset, and the DIB behind it is copied
/// rather than rewritten. No word processor on a test machine writes this shape
/// any more — see `test-data/make_rtf_fixtures.py` — so the fixture is authored
/// to Word's format, and this test is what makes it worth having.
#[test]
fn a_word_style_rtf_bitmap_is_written_as_the_bmp_it_embeds() {
    let ws = Workspace::new("rtf-word-bitmap");
    let source = fixture("images/rtf_bitmap.bmp");
    let input = ws.write("word_bitmap.rtf", &fixture("word_bitmap.rtf"));

    run_ok(&["-i", input.to_str().unwrap(), "--ocr", "never"]);

    let doc = json_of(&ws.join("word_bitmap.json"));
    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(pictures.len(), 1, "the bitmap was dropped: {pictures:#?}");
    assert_eq!(read_named_image(&ws, &pictures[0]), "image/bmp");

    let uri = pictures[0]["image"]["uri"].as_str().expect("an image uri");
    assert!(uri.ends_with(".bmp"), "not a BMP: {uri}");
    let written = std::fs::read(ws.join(uri)).expect("the picture beside the JSON");
    assert_eq!(
        written, source,
        "{uri} is not the bitmap the RTF embedded: the wrapper rewrote a byte"
    );

    // A readable file is not yet a picture: the page facts come from the
    // header, and the resolution the DIB declares is the picture's.
    let page = pdfextractor_image_parser::ImageDocument::parse_bytes(&written)
        .expect("the file beside the JSON is a picture");
    assert_eq!(page.mimetype, "image/bmp");
    assert_eq!((page.width, page.height), (24, 16));
    // The DIB states 2835 pixels per metre, which is 72 dpi to a fraction.
    let (x, y) = page.dpi.expect("the DIB declares a resolution");
    assert!(
        (x - 72.0).abs() < 0.1 && (y - 72.0).abs() < 0.1,
        "{x}x{y} dpi"
    );

    // The words around it are still words.
    let texts: Vec<&str> = doc["texts"]
        .as_array()
        .expect("texts")
        .iter()
        .map(|item| item["text"].as_str().expect("a text item"))
        .collect();
    assert_eq!(
        texts,
        vec![
            "A picture below, written the way Word writes one.",
            "After the picture.",
        ]
    );
}

/// A word processor's own RTF, and the same bitmap through it.
///
/// `rtf_bitmap.rtf` was written by LibreOffice from a DOCX holding the source
/// bitmap, and LibreOffice spells an embedded raster `\pngblip` whatever file
/// it came from — so the picture reaches the output as a PNG. What has to hold
/// either way is that the file beside the JSON opens and shows the source's
/// pixels: a payload written out without being understood would name a picture
/// no reader could open, and one decoded wrongly would show the wrong picture.
#[test]
fn a_word_processor_rtf_picture_is_written_readably_beside_the_json() {
    let ws = Workspace::new("rtf-libreoffice-bitmap");
    let source = image::load_from_memory(&fixture("images/rtf_bitmap.bmp"))
        .expect("the source bitmap decodes")
        .to_rgba8();
    let input = ws.write("rtf_bitmap.rtf", &fixture("rtf_bitmap.rtf"));

    run_ok(&["-i", input.to_str().unwrap(), "--ocr", "never"]);

    let doc = json_of(&ws.join("rtf_bitmap.json"));
    let pictures = doc["pictures"].as_array().expect("pictures");
    assert_eq!(pictures.len(), 1, "the picture was dropped: {pictures:#?}");
    assert_eq!(read_named_image(&ws, &pictures[0]), "image/png");
    assert_eq!(pictures[0]["image"]["size"]["width"], 24.0);
    assert_eq!(pictures[0]["image"]["size"]["height"], 16.0);

    let uri = pictures[0]["image"]["uri"].as_str().expect("an image uri");
    let written = std::fs::read(ws.join(uri)).expect("the picture beside the JSON");
    let page = pdfextractor_image_parser::ImageDocument::parse_bytes(&written)
        .expect("the file beside the JSON is a picture");
    assert_eq!((page.width, page.height), (24, 16));

    // Readable is not enough: the pixels are the bitmap's own, so the picture
    // a word processor embedded survived the trip unchanged.
    let read = image::load_from_memory(&written)
        .expect("the picture beside the JSON decodes")
        .to_rgba8();
    assert_eq!(read.dimensions(), source.dimensions());
    assert!(
        read.pixels()
            .zip(source.pixels())
            .all(|(read, source)| read == source),
        "the picture LibreOffice embedded did not survive the conversion"
    );

    // And the document is the word processor's: its heading, its paragraph, its
    // picture, its closing paragraph.
    let texts: Vec<&str> = doc["texts"]
        .as_array()
        .expect("texts")
        .iter()
        .map(|item| item["text"].as_str().expect("a text item"))
        .collect();
    assert_eq!(
        texts,
        vec![
            "A word processor's bitmap",
            "The picture below is embedded as a bitmap.",
            "And a paragraph after it.",
        ]
    );
}

/// An RTF table is a table and an RTF heading is a heading: `\trowd` rows make
/// a grid of cells instead of one line of tabs, and a paragraph under a style
/// the stylesheet calls a heading is labelled at its depth instead of reading
/// as ordinary prose. The Markdown says the same thing, because both readings
/// come from the same model.
#[test]
fn an_rtf_table_and_headings_keep_their_structure() {
    let ws = Workspace::new("rtf-table");
    let rtf = r#"{\rtf1\ansi{\stylesheet{\s1\outlinelevel0 heading 1;}}
\pard\s1 Quarterly Brief\par
\trowd\cellx2000\cellx4000
\pard\intbl Year\cell Revenue\cell\row
\trowd\cellx2000\cellx4000
\pard\intbl 2024\cell 120\cell\row
\pard After\par}"#;
    let input = ws.write("brief.rtf", rtf.as_bytes());

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("brief.json"));
    let texts = doc["texts"].as_array().expect("texts");
    let heading = texts
        .iter()
        .find(|item| item["text"] == "Quarterly Brief")
        .expect("the heading");
    assert_eq!(heading["label"], "section_header", "{heading:#?}");
    assert_eq!(heading["level"], 1, "{heading:#?}");
    assert!(
        texts.iter().any(|item| item["text"] == "After"),
        "the paragraph after the table was lost: {texts:#?}"
    );

    let tables = doc["tables"].as_array().expect("tables");
    assert_eq!(tables.len(), 1, "the table was flattened: {doc:#?}");
    let data = &tables[0]["data"];
    assert_eq!(
        (data["num_rows"].as_u64(), data["num_cols"].as_u64()),
        (Some(2), Some(2))
    );
    let cells: Vec<(u64, u64, &str)> = data["table_cells"]
        .as_array()
        .expect("cells")
        .iter()
        .map(|cell| {
            (
                cell["start_row_offset_idx"].as_u64().expect("a row"),
                cell["start_col_offset_idx"].as_u64().expect("a column"),
                cell["text"].as_str().expect("words"),
            )
        })
        .collect();
    assert_eq!(
        cells,
        vec![
            (0, 0, "Year"),
            (0, 1, "Revenue"),
            (1, 0, "2024"),
            (1, 1, "120"),
        ]
    );

    let markdown = std::fs::read_to_string(ws.join("brief.md")).expect("the Markdown");
    assert!(markdown.contains("## Quarterly Brief"), "{markdown}");
    assert!(markdown.contains("| Year"), "{markdown}");
    assert!(markdown.contains("| 2024"), "{markdown}");
    assert!(markdown.contains("| 120"), "{markdown}");
    assert!(
        !markdown.contains("Year\tRevenue"),
        "the table is still a line of tabs: {markdown}"
    );
}

/// `\trhdr` is the table's own statement of which row heads its columns, and
/// it reaches the document model: a table that labels a group above its column
/// names heads the names, not the group. The row the document marked is the
/// one whose cells claim `column_header`, and row 0 claims nothing — every row
/// of the table is still there, in its place.
#[test]
fn an_rtf_header_row_is_the_tables_header_not_row_zero() {
    let ws = Workspace::new("rtf-header-row");
    let rtf = r#"{\rtf1\ansi
\trowd\cellx2000\cellx4000
\pard\intbl Group\cell Result\cell\row
\trowd\trhdr\cellx2000\cellx4000
\pard\intbl Year\cell Revenue\cell\row
\trowd\cellx2000\cellx4000
\pard\intbl 2024\cell 120\cell\row}"#;
    let input = ws.write("grouped.rtf", rtf.as_bytes());

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("grouped.json"));
    let tables = doc["tables"].as_array().expect("tables");
    assert_eq!(tables.len(), 1, "{doc:#?}");
    let cells: Vec<(u64, &str, bool)> = tables[0]["data"]["table_cells"]
        .as_array()
        .expect("cells")
        .iter()
        .map(|cell| {
            (
                cell["start_row_offset_idx"].as_u64().expect("a row"),
                cell["text"].as_str().expect("words"),
                cell["column_header"].as_bool().expect("a flag"),
            )
        })
        .collect();
    assert_eq!(
        cells,
        vec![
            (0, "Group", false),
            (0, "Result", false),
            (1, "Year", true),
            (1, "Revenue", true),
            (2, "2024", false),
            (2, "120", false),
        ]
    );

    let markdown = std::fs::read_to_string(ws.join("grouped.md")).expect("the Markdown");
    for words in ["Group", "Result", "Year", "Revenue", "2024", "120"] {
        assert!(markdown.contains(words), "{words} is missing: {markdown}");
    }
}

/// `merged_table.rtf` is a word processor's own export of a brief: a title, a
/// `Heading 1` over a table, a `Heading 2` under it, and a table that merges
/// both ways. It therefore holds the spellings LibreOffice actually writes — a
/// horizontally merged cell as one cell ending at the far boundary rather than
/// a run of `\clmgf`s, a vertical merge stated with `\clvmgf`/`\clvmrg`, and
/// headings named in the stylesheet with no `\outlinelevel` anywhere — which is
/// what makes it a gate rather than another hand-written sample.
///
/// The structure is pinned end to end: the heading depths and the paragraphs
/// around them, the grid's shape, and every cell's words, spans and header flag.
/// The HTML render is the merge seen whole (`rowspan`/`colspan`), and the
/// flattened Markdown keeps the covered slot's words out of the table instead
/// of repeating them, so a span that silently became two cells fails here as
/// well as in the JSON.
#[test]
fn the_libreoffice_table_fixture_keeps_its_merges_and_headings() {
    let ws = Workspace::new("rtf-merged-table");
    let input = ws.write("merged_table.rtf", &fixture("merged_table.rtf"));

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&ws.join("merged_table.json"));
    assert_eq!(doc["origin"]["mimetype"], "application/rtf");

    let items: Vec<(&str, &str, Option<i64>)> = doc["texts"]
        .as_array()
        .expect("texts")
        .iter()
        .map(|item| {
            (
                item["text"].as_str().expect("words"),
                item["label"].as_str().expect("a label"),
                item["level"].as_i64(),
            )
        })
        .collect();
    assert_eq!(
        items,
        vec![
            ("Quarterly Brief", "title", None),
            (
                "Revenue is grouped by region; the quarter cells merge across the year.",
                "paragraph",
                None
            ),
            ("Revenue by region", "section_header", Some(1)),
            ("Figures in thousands.", "paragraph", None),
            ("Notes", "section_header", Some(2)),
            ("All figures are unaudited.", "paragraph", None),
        ],
        "{doc:#?}"
    );

    let tables = doc["tables"].as_array().expect("tables");
    assert_eq!(tables.len(), 1, "{doc:#?}");
    let data = &tables[0]["data"];
    assert_eq!(
        (data["num_rows"].as_u64(), data["num_cols"].as_u64()),
        (Some(4), Some(3)),
        "{data:#?}"
    );
    // The cells that *are* the table: `North` covers row 2's first slot and
    // `250` covers the total row's second and third, so neither has a cell of
    // its own there and the list is shorter than rows × columns.
    let cells: Vec<(u64, u64, u64, u64, &str, bool)> = data["table_cells"]
        .as_array()
        .expect("cells")
        .iter()
        .map(|cell| {
            (
                cell["start_row_offset_idx"].as_u64().expect("a row"),
                cell["start_col_offset_idx"].as_u64().expect("a column"),
                cell["row_span"].as_u64().expect("a row span"),
                cell["col_span"].as_u64().expect("a column span"),
                cell["text"].as_str().expect("words"),
                cell["column_header"].as_bool().expect("a header flag"),
            )
        })
        .collect();
    assert_eq!(
        cells,
        vec![
            (0, 0, 1, 1, "Region", true),
            (0, 1, 1, 1, "Q1", true),
            (0, 2, 1, 1, "Q2", true),
            (1, 0, 2, 1, "North", false),
            (1, 1, 1, 1, "120", false),
            (1, 2, 1, 1, "140", false),
            (2, 1, 1, 1, "H1", false),
            (2, 2, 1, 1, "H2", false),
            (3, 0, 1, 1, "Total", false),
            (3, 1, 1, 2, "250", false),
        ]
    );

    let html = std::fs::read_to_string(ws.join("merged_table.html")).expect("the HTML");
    assert!(html.contains("<th>Region</th>"), "{html}");
    assert!(
        html.contains("<td rowspan=\"2\">North</td>"),
        "the vertical merge is not one cell: {html}"
    );
    assert!(
        html.contains("<td colspan=\"2\">250</td>"),
        "the horizontal merge is not one cell: {html}"
    );
    // The heading depths the reader found are the ones the writers render.
    assert!(html.contains("<h2>Revenue by region</h2>"), "{html}");
    assert!(html.contains("<h3>Notes</h3>"), "{html}");

    let markdown = std::fs::read_to_string(ws.join("merged_table.md")).expect("the Markdown");
    assert!(markdown.contains("# Quarterly Brief"), "{markdown}");
    assert!(markdown.contains("## Revenue by region"), "{markdown}");
    assert!(markdown.contains("### Notes"), "{markdown}");
    assert!(
        markdown.contains("| North  | 120 | 140 |"),
        "the row under the merged cell is not where it belongs: {markdown}"
    );
    assert!(
        markdown.contains("| Total  | 250 |     |"),
        "the covered column was filled in rather than flattened: {markdown}"
    );
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
    // An ELF header: binary, NULs in the first eight bytes, and no document
    // format this tool knows either by name or by signature.
    let mut binary = vec![0x7F, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00];
    binary.extend_from_slice(&[0; 8]);
    let input = ws.write("thing.xyz", &binary);

    let out = run(&["-i", input.to_str().unwrap()]);

    assert!(!out.status.success(), "expected a non-zero exit");
    let error = stderr_of(&out);
    assert!(
        error.contains("unsupported extension: xyz"),
        "unclear error: {error}"
    );
    // Nothing is known about `.xyz` or its bytes, so nothing is promised about
    // it either — but the refusal still says the bytes were looked at.
    assert!(
        error.contains("the bytes are a binary file of no format this reads"),
        "the bytes are not accounted for: {error}"
    );
    assert!(
        !error.contains("picture format this tool carries"),
        "a stranger was explained as a carried picture: {error}"
    );
}

/// A name the tool refuses is not the whole answer when the bytes are a format
/// it reads: a PDF called `report.xyz` is a PDF, and the refusal has to say so
/// and name the extension that would read it. The answer comes from the bytes,
/// not a guess about the name, so a plain-text file, an RTF, an HTML page and a
/// picture are each named as what they are.
#[test]
fn a_refused_name_is_explained_by_the_files_bytes() {
    let ws = Workspace::new("refused-name");
    let corpus = |name: &str| {
        let path = repository_root().join("test-data/real").join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()))
    };

    for (name, bytes, is, advice) in [
        (
            "report.xyz",
            corpus("dummy.pdf"),
            "the bytes are a PDF",
            "rename the file to .pdf",
        ),
        (
            "letter.xyz",
            std::fs::read(repository_root().join("test-data/brief.docx"))
                .expect("the DOCX fixture"),
            "the bytes are a Word document (DOCX)",
            "rename the file to .docx",
        ),
        (
            "scan.xyz",
            std::fs::read(repository_root().join("test-data/invoice_scan.png"))
                .expect("the PNG fixture"),
            "the bytes are a PNG image",
            "rename the file to .png",
        ),
        (
            "notes.xyz",
            b"just words\n".to_vec(),
            "the bytes are plain text",
            "rename the file to .txt",
        ),
        (
            "page.xyz",
            b"<!DOCTYPE html><html><body>x</body></html>".to_vec(),
            "the bytes are an HTML page",
            "rename the file to .html",
        ),
    ] {
        let input = ws.write(name, &bytes);
        let out = run(&["-i", input.to_str().unwrap()]);

        assert!(!out.status.success(), "{name}: expected a non-zero exit");
        let error = stderr_of(&out);
        assert!(
            error.contains("unsupported extension: xyz"),
            "{name}: the name is not named: {error}"
        );
        assert!(error.contains(is), "{name}: {is} is not said: {error}");
        assert!(
            error.contains(advice),
            "{name}: {advice} is not offered: {error}"
        );
    }

    // A format the tool knows and cannot read is named as itself, and no
    // rename is promised, because none would help.
    let epub = {
        let mut zip = Vec::new();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut zip));
        writer
            .start_file(
                "META-INF/container.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("add the container part");
        std::io::Write::write_all(&mut writer, b"<container/>").expect("write the part");
        writer.finish().expect("finish the package");
        zip
    };
    let input = ws.write("book.xyz", &epub);
    let out = run(&["-i", input.to_str().unwrap()]);
    assert!(!out.status.success(), "an EPUB is not read");
    let error = stderr_of(&out);
    assert!(
        error.contains("the bytes are an EPUB book — not read yet"),
        "the format is not named: {error}"
    );
    assert!(
        !error.contains("rename"),
        "a format with no reader was told to rename it: {error}"
    );
}

/// An SVG or a metafile is a format the extractor carries — it writes one out
/// when a document embeds it — but neither is a page on its own. The refusal has
/// to say that much: that the format is known, what it is called, and which
/// picture formats a standalone file has to be instead. A name the tool knows
/// nothing about keeps the plain message.
#[test]
fn a_carried_picture_that_is_not_a_page_is_explained() {
    let ws = Workspace::new("carried-picture");
    let svg = std::fs::read(repository_root().join("test-data/images/diagram.svg"))
        .expect("the SVG fixture");

    for (name, bytes, mimetype) in [
        ("diagram.svg", svg.as_slice(), "image/svg+xml"),
        // A metafile is certainly a picture this carries; the message is decided
        // by the extension, so the bytes need not be one.
        ("drawing.emf", b"not a metafile".as_slice(), "image/emf"),
        ("drawing.wmf", b"not a metafile".as_slice(), "image/wmf"),
    ] {
        let input = ws.write(name, bytes);
        let out = run(&["-i", input.to_str().unwrap()]);

        assert!(!out.status.success(), "{name}: expected a non-zero exit");
        let error = stderr_of(&out);
        let extension = name.rsplit('.').next().unwrap();
        assert!(
            error.contains(&format!("unsupported extension: {extension}")),
            "{name}: unclear error: {error}"
        );
        assert!(
            error.contains("picture format this tool carries"),
            "{name}: the refusal reads as an unknown format: {error}"
        );
        assert!(
            error.contains(mimetype),
            "{name}: the format is not named: {error}"
        );
        assert!(
            error.contains(&format!(".{extension} is not read as a page on its own")),
            "{name}: the refusal does not say what is wrong: {error}"
        );
        // The way out: the picture formats that are pages, which are accepted —
        // a GIF among them, since its screen is a page and its frames are the
        // pages.
        for extension in ["png", "jpg", "jp2", "gif", "bmp", "tif", "webp"] {
            assert!(
                error.contains(&format!(".{extension}")),
                "{name}: {extension} is not offered as a way to read it: {error}"
            );
        }
    }
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
    // An empty folder holds nothing to name, so nothing is claimed about it.
    assert!(
        !stderr_of(&out).contains("it does hold"),
        "an empty folder was said to hold something: {}",
        stderr_of(&out)
    );
}

/// A folder of SVGs and metafiles is not an empty folder: the run refuses it,
/// and the refusal names the pictures it passed over rather than leaving the
/// user to wonder why `no supported documents` came back for files that are
/// there. The walk is the run's own, so `--recursive false` tells only the top
/// level and the default names a nested picture too; the count and the singular
/// read right either way. A GIF is not among them: it is a page in itself.
#[test]
fn a_directory_of_carried_pictures_names_what_it_holds() {
    let ws = Workspace::new("carried-folder");
    let svg = std::fs::read(repository_root().join("test-data/images/diagram.svg"))
        .expect("the SVG fixture");
    ws.write("diagram.svg", &svg);
    // Not metafiles, but the extensions are what the walk reads them by.
    ws.write("drawing.wmf", b"not a metafile");
    ws.write("nested/drawing.emf", b"not a metafile");

    let shallow = run(&["-i", ws.path.to_str().unwrap(), "--recursive", "false"]);
    assert!(!shallow.status.success(), "expected a non-zero exit");
    let error = stderr_of(&shallow);
    assert!(
        error.contains("no supported documents"),
        "the accepted set is not named: {error}"
    );
    assert!(
        error.contains("it does hold 2 pictures this tool carries but does not read as pages")
            && error.contains("diagram.svg")
            && error.contains("drawing.wmf"),
        "the pictures it passed over are not named: {error}"
    );
    assert!(
        !error.contains("drawing.emf"),
        "a nested file was named by a top-level walk: {error}"
    );

    // The default walk descends, so the nested metafile is named too.
    let deep = run(&["-i", ws.path.to_str().unwrap()]);
    assert!(!deep.status.success(), "expected a non-zero exit");
    let error = stderr_of(&deep);
    assert!(
        error.contains("it does hold 3 pictures") && error.contains("drawing.emf"),
        "the recursive walk does not name what it found: {error}"
    );

    // One picture is named in the singular, as one fact rather than a listing.
    let alone = Workspace::new("carried-one");
    alone.write("diagram.svg", &svg);
    let out = run(&["-i", alone.path.to_str().unwrap()]);
    let error = stderr_of(&out);
    assert!(
        error.contains("it does hold one picture this tool carries but does not read as a page")
            && error.contains("diagram.svg"),
        "a single picture is not named as one: {error}"
    );
}

/// `--help` names the input formats a run accepts, and names exactly the list
/// the empty-directory error prints: both are generated from the one accepted
/// set, so the help cannot promise a format the run refuses or miss one it
/// takes.
#[test]
fn the_help_names_the_accepted_input_formats() {
    // The list as the run enforces it: an empty directory reports the accepted
    // set back in place of the documents it found.
    let ws = Workspace::new("help-formats");
    let refused = run(&["-i", ws.path.to_str().unwrap()]);
    assert!(!refused.status.success(), "expected a non-zero exit");
    let error = stderr_of(&refused);
    let accepted = error
        .split_once("no supported documents (")
        .and_then(|(_, rest)| rest.split_once(") found under"))
        .map(|(list, _)| list.to_string())
        .unwrap_or_else(|| panic!("the error names no formats: {error}"));
    assert!(
        !accepted.is_empty(),
        "the error names an empty list: {error}"
    );

    // Short and long help both name the set, grouped by family: every accepted
    // extension is there under its own name or beside its aliases
    // (`.jpg/.jpeg`), and the mark adding a `*` does not change the name.
    for flag in ["--help", "-h"] {
        let out = run(&[flag]);
        assert!(out.status.success(), "{flag} failed: {}", stderr_of(&out));
        let help = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            help.contains("Accepted input formats"),
            "{flag} names no formats:\n{help}"
        );
        let named: Vec<&str> = help
            .split(|character: char| {
                character.is_whitespace() || character == ',' || character == '/'
            })
            .map(|token| token.trim_end_matches('*').trim_end_matches(':'))
            .collect();
        for extension in accepted.split(", ") {
            assert!(
                named.contains(&extension),
                "{flag} does not name {extension}:\n{help}"
            );
        }
        // A name the run refuses is not promised by the grouped list either.
        for refused in [".svg", ".emf", ".wmf"] {
            assert!(
                !named.contains(&refused),
                "{flag} promises {refused}:\n{help}"
            );
        }
    }

    // A picture the tool carries but does not read as a page is not promised
    // as an input, and a carried page format is.
    let named: Vec<&str> = accepted.split(", ").collect();
    for extension in [
        ".png", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff", ".webp", ".gif",
    ] {
        assert!(
            named.contains(&extension),
            "{extension} is read as a page but not promised: {accepted}"
        );
    }
    for extension in [".svg", ".emf", ".wmf"] {
        assert!(
            !named.contains(&extension),
            "{extension} is not a page but is promised: {accepted}"
        );
    }
}

/// The accepted set is not only prose: `--list-input-formats` prints it as JSON
/// and exits, needing no input path at all. A window or a script reads the same
/// list `--help` and the empty-folder refusal print — the run's own accepted
/// set — so the three cannot disagree, and nothing else is written to the
/// streams a caller is reading.
#[test]
fn the_accepted_input_formats_are_a_machine_readable_list() {
    let out = run(&["--list-input-formats"]);
    assert!(
        out.status.success(),
        "the list is not a run: {}",
        stderr_of(&out)
    );
    assert_eq!(
        stderr_of(&out),
        "",
        "the list carried a log line: {}",
        stderr_of(&out)
    );
    let listed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("the flag prints JSON");

    // The same set, in the same order, as the help and the refusal name it.
    let ws = Workspace::new("input-formats-json");
    let refused = run(&["-i", ws.path.to_str().unwrap()]);
    let error = stderr_of(&refused);
    let accepted = error
        .split_once("no supported documents (")
        .and_then(|(_, rest)| rest.split_once(") found under"))
        .map(|(list, _)| list.to_string())
        .unwrap_or_else(|| panic!("the error names no formats: {error}"));
    assert_eq!(
        listed["input_formats"],
        serde_json::json!(accepted.split(", ").collect::<Vec<_>>()),
        "the JSON list and the refusal disagree: {listed}"
    );

    // The flag is the only way to run without an input: without either, the run
    // says what is missing rather than converting nothing.
    let missing = run(&[]);
    assert!(!missing.status.success(), "a run with no input succeeded");
    assert!(
        stderr_of(&missing).contains("-i, --input <PATH> is required"),
        "the missing input is not named: {}",
        stderr_of(&missing)
    );
}

/// The dotted names of a JSON array of strings, as the flag prints them.
fn dotted_names(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of names: {value}"))
        .iter()
        .map(|name| name.as_str().expect("a name").to_string())
        .collect()
}

/// The flag's set is grouped the way `--help` lists it as well: one family per
/// heading, in the help's order, with every accepted name in exactly one of
/// them. A picker built from the JSON therefore offers the groups the help
/// describes, and a name the run takes cannot go missing from them.
#[test]
fn the_accepted_formats_are_grouped_by_the_family_the_help_names_them_under() {
    let out = run(&["--list-input-formats"]);
    assert!(
        out.status.success(),
        "the list is not a run: {}",
        stderr_of(&out)
    );
    assert_eq!(
        stderr_of(&out),
        "",
        "the list carried a log line: {}",
        stderr_of(&out)
    );
    let listed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("the flag prints JSON");
    let flat = dotted_names(&listed["input_formats"]);
    let families = listed["families"]
        .as_array()
        .expect("the groups are a list");

    // The help's headings, in the order it prints them.
    let headings: Vec<&str> = families
        .iter()
        .map(|family| family["name"].as_str().expect("a family is named"))
        .collect();
    assert_eq!(
        headings,
        vec![
            "PDF",
            "Word",
            "Excel",
            "PowerPoint",
            "Works/WordPerfect",
            "Text and markup",
            "Pictures",
        ],
        "the families are not the help's headings"
    );

    // Every name in exactly one family, and the families together are the flat
    // list: nothing accepted is left out of a group, and nothing is in two.
    let mut grouped: Vec<String> = Vec::new();
    for family in families {
        let heading = family["name"].as_str().expect("a name");
        for name in dotted_names(&family["input_formats"]) {
            assert!(
                name.starts_with('.') && name.len() > 1,
                "{heading} holds {name:?}, which is not a dotted extension"
            );
            assert!(
                !grouped.contains(&name),
                "{name} is in two families: {grouped:?}"
            );
            grouped.push(name);
        }
        assert!(
            !family["input_formats"].as_array().unwrap().is_empty(),
            "{heading} has no formats"
        );
    }
    let sorted = |mut names: Vec<String>| {
        names.sort();
        names
    };
    assert_eq!(
        sorted(grouped.clone()),
        sorted(flat.clone()),
        "the groups are not the accepted set"
    );

    // The groups are the help's own lines: every grouped name is on the line of
    // its heading, so a picker's group is the block a reader finds in the help.
    let help = String::from_utf8_lossy(&run(&["--help"]).stdout).into_owned();
    for family in families {
        let heading = family["name"].as_str().expect("a name");
        let marker = format!("  {heading}:");
        let line = help
            .lines()
            .find(|line| line.contains(&marker))
            .unwrap_or_else(|| panic!("the help has no {heading} line:\n{help}"));
        for name in dotted_names(&family["input_formats"]) {
            assert!(
                line.contains(&name),
                "{name} is under {heading} but not on its help line: {line}"
            );
        }
    }

    // A picture's aliases are names of their own, because a file can be a
    // `.jpeg` or a `.tiff` while the encoding is written `.jpg` or `.tif`.
    let pictures = dotted_names(&families.last().expect("a last family")["input_formats"]);
    for name in [
        ".png", ".jpg", ".jpeg", ".jp2", ".j2c", ".j2k", ".jpc", ".jpf", ".tiff",
    ] {
        assert!(
            pictures.iter().any(|listed| listed == name),
            "{name} is a page but not offered under Pictures: {pictures:?}"
        );
    }
}

/// The console's formats are asked of it, never kept in a list of its own: the
/// window runs the CLI's `--list-input-formats` and parses the answer, so a
/// reader added to the gate shows up in the window with no change on that side.
/// This is the two halves of that contract — what the CLI prints and what the
/// window reads — held together against the same binary.
#[test]
fn the_window_reads_the_accepted_formats_from_the_cli() {
    let program = env!("CARGO_BIN_EXE_pdfextractor-cli");
    let asked = pdfextractor_ui::ask_input_formats(program)
        .unwrap_or_else(|problem| panic!("the window could not ask the CLI: {problem}"));

    // The flag's own answer, read separately: the same names, in the same order.
    let out = run(&["--list-input-formats"]);
    let listed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("the flag prints JSON");
    let expected: Vec<String> = listed["input_formats"]
        .as_array()
        .expect("an array of names")
        .iter()
        .map(|name| name.as_str().expect("a name").to_string())
        .collect();

    assert_eq!(asked.names, expected, "the window reads a different set");
    // Named as a file is, so the page can print them and the chooser filter by
    // them without either re-spelling a name.
    assert!(
        asked.names.iter().all(|name| name.starts_with('.')),
        "a format is not dotted: {:?}",
        asked.names
    );
    assert!(
        asked.names.iter().any(|name| name == ".docx"),
        "a format a run takes is missing: {:?}",
        asked.names
    );
    assert_eq!(
        pdfextractor_ui::bare_extensions(&asked.names),
        expected
            .iter()
            .map(|name| name.trim_start_matches('.').to_string())
            .collect::<Vec<_>>(),
        "the chooser filter is not the same names"
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

/// `/Rotate`: a scan the document turns before display is read the way it is
/// shown. `scanned_rotated.pdf` is the scanned invoice captured sideways —
/// its JPEG stored 90° counter-clockwise, its page `/Rotate 90` — which is
/// how scanners actually hand over such pages. The bitmap is turned with the
/// page before Tesseract sees it, so the same invoice comes back upright,
/// with its boxes in the turned page's space.
#[test]
fn a_rotated_scan_is_read_upright() {
    let ws = Workspace::new("rotated");
    let input = repository_root().join("test-data/scanned_rotated.pdf");
    let output = ws.join("scan.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "auto",
    ]);

    let doc = json_of(&output);
    let page = &doc["pages"]["1"];
    assert_eq!(page["size"]["width"], 792.0, "the page is turned landscape");
    assert_eq!(page["size"]["height"], 612.0);

    let text = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|t| t["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    for needle in ["ACME", "12345", "Total Due"] {
        assert!(text.contains(needle), "missing {needle:?} in: {text}");
    }

    // Every recognised box lies inside the turned page: the mapping is in the
    // space the page is displayed in, not the space the file holds.
    for text in doc["texts"].as_array().expect("a text list") {
        for prov in text["prov"].as_array().expect("provenance") {
            let bbox = &prov["bbox"];
            let (l, t, r, b) = (
                bbox["l"].as_f64().unwrap(),
                bbox["t"].as_f64().unwrap(),
                bbox["r"].as_f64().unwrap(),
                bbox["b"].as_f64().unwrap(),
            );
            assert!(
                l >= 0.0 && t >= 0.0 && r <= 792.5 && b <= 612.5,
                "box outside the page: {bbox}"
            );
        }
    }
}

/// A one-page PDF whose page is this JPEG, drawn to fill it.
///
/// The bytes are assembled here rather than committed as an opaque fixture, so
/// what the test relies on is visible: one image XObject, no text operators, no
/// `/Rotate` on the page, and nothing else that could say which way up it is.
fn pdf_of_image(jpeg: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (page_width, page_height) = (595.0, 842.0);
    let mut objects: Vec<Vec<u8>> = Vec::new();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    objects.push(
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_width} {page_height}] \
             /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>"
        )
        .into_bytes(),
    );
    let content = format!("q {page_width} 0 0 {page_height} 0 0 cm /Im0 Do Q");
    let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    stream.extend_from_slice(content.as_bytes());
    stream.extend_from_slice(b"\nendstream");
    objects.push(stream);
    let mut image = format!(
        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
        jpeg.len()
    )
    .into_bytes();
    image.extend_from_slice(jpeg);
    image.extend_from_slice(b"\nendstream");
    objects.push(image);

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

/// A scanned page that is sideways in its pixels, with nothing in the file to
/// say so: no `/Rotate` on the page and no EXIF tag in the JPEG. The page has no
/// text layer at all, so the picture is the page, and turning it upright is the
/// only way its text can be read.
#[test]
fn a_sideways_scan_with_no_page_rotation_is_read_upright() {
    let ws = Workspace::new("sideways-scan");
    let jpeg = std::fs::read(repository_root().join("test-data/photos/07_sideways.jpg"))
        .expect("read the sideways fixture");
    let input = ws.write("sideways.pdf", &pdf_of_image(&jpeg, 1754, 1240));
    let output = ws.join("sideways.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "auto",
    ]);

    let doc = json_of(&output);
    // The page is the one the PDF declares; the bitmap is turned inside it.
    assert_eq!(doc["pages"]["1"]["size"]["width"], 595.0);
    assert_eq!(doc["pages"]["1"]["size"]["height"], 842.0);

    let text = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|t| t["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    for needle in ["ACME", "12345", "Total Due"] {
        assert!(text.contains(needle), "missing {needle:?} in: {text}");
    }

    // Upright lines run across the page, and every box is in the page the PDF
    // declares: the turn happened before recognition, in the target space.
    for item in doc["texts"].as_array().expect("a text list") {
        let bbox = &item["prov"][0]["bbox"];
        let (l, t, r, b) = (
            bbox["l"].as_f64().unwrap(),
            bbox["t"].as_f64().unwrap(),
            bbox["r"].as_f64().unwrap(),
            bbox["b"].as_f64().unwrap(),
        );
        assert!(r - l > b - t, "a sideways line box: {bbox}");
        assert!(
            l >= 0.0 && t >= 0.0 && r <= 595.5 && b <= 842.5,
            "box outside the page: {bbox}"
        );
    }
}

/// A DOCX or PPTX can embed a picture the camera stored sideways, with only
/// its EXIF tag saying how Word or PowerPoint displayed it: upright. The
/// package keeps the stored pixels, so the reader has to turn them — the
/// picture written beside the JSON and the size in the model must be the one
/// the office application showed.
#[test]
fn an_exif_tagged_picture_in_an_office_file_is_turned_upright() {
    let ws = Workspace::new("oriented-picture");
    for fixture in ["oriented_picture.docx", "oriented_picture.pptx"] {
        let input = repository_root().join("test-data").join(fixture);
        let output = ws.join(&format!("{fixture}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ]);

        let doc = json_of(&output);
        let picture = doc["pictures"]
            .as_array()
            .and_then(|pictures| pictures.first())
            .unwrap_or_else(|| panic!("{fixture}: no picture: {doc:#?}"));
        let image = &picture["image"];
        assert_eq!(image["mimetype"], "image/jpeg", "{fixture}");
        // The stored grid is 1754 x 1240; the picture the tag describes is the
        // portrait one Word and PowerPoint displayed.
        assert_eq!(image["size"]["width"], 1240.0, "{fixture}");
        assert_eq!(image["size"]["height"], 1754.0, "{fixture}");

        let uri = image["uri"].as_str().expect("an image uri");
        let written = output.parent().unwrap().join(uri);
        let bytes = std::fs::read(&written)
            .unwrap_or_else(|e| panic!("{fixture}: missing {}: {e}", written.display()));
        assert!(
            !bytes.windows(6).any(|window| window == b"Exif\0\0"),
            "{fixture}: the written picture still carries the tag"
        );
        let decoded = image::load_from_memory(&bytes)
            .unwrap_or_else(|e| panic!("{fixture}: the picture does not decode: {e}"));
        assert_eq!(
            (decoded.width(), decoded.height()),
            (1240, 1754),
            "{fixture}: the written picture is upright"
        );
    }
}

/// Whether `png` is a picture of `images/diagram.svg`: its pale panel with the
/// blue line across it, painted on every pixel the panel covers.
///
/// A grey box (the raster fallback of the same drawing) or a solid colour is
/// not: the stroke is what a picture of this drawing has and no thumbnail does.
fn assert_rendered_from_the_diagram(label: &str, png: &[u8]) -> (u32, u32) {
    let drawn = image::load_from_memory(png)
        .unwrap_or_else(|e| panic!("{label}: the picture does not decode: {e}"))
        .to_rgba8();
    let (width, height) = drawn.dimensions();
    let mut fill = 0usize;
    let mut ink = 0usize;
    for pixel in drawn.pixels() {
        let [r, g, b, a] = pixel.0;
        if a == 0 {
            continue;
        }
        if (220..=245).contains(&r) && (228..=250).contains(&g) && b >= 235 {
            fill += 1;
        }
        // The stroke is #2f6db5 at two pixels wide, so most of its pixels are
        // blends of it and the fill behind it.
        if i32::from(b) > i32::from(r) + 40 && b > 150 {
            ink += 1;
        }
    }
    let painted = (width as usize * height as usize).max(1);
    assert!(
        fill * 2 >= painted,
        "{label}: {width}x{height} is not a drawing of the fixture's SVG: \
         {fill} of {painted} pixels carry its panel"
    );
    assert!(
        ink >= 20,
        "{label}: {width}x{height} has the panel but none of its stroke: {ink} ink pixels"
    );
    (width, height)
}

/// A package's media type comes from the shared picture list, which carries a
/// GIF and an SVG as well as the raster formats Word usually stores. A Word
/// package and a PowerPoint package each embedding both encodings must give two
/// pictures per conversion, and each one is placed at the size the format
/// states in its own header or root tag. A GIF is decoded and written as PNG,
/// as every raster picture in an office package is; an SVG is a drawing rather
/// than an encoding, so it is played at the size the package places the picture
/// at — 144 dpi, the resolution a metafile in the same package is played at —
/// and written as a PNG too. A box with no image, or vector markup left in the
/// document for the next reader to draw, is the failure this guards.
#[test]
fn a_gif_and_an_svg_embedded_in_office_files_keep_their_size_and_place() {
    let ws = Workspace::new("gif-svg-office");
    let gif = std::fs::read(repository_root().join("test-data/images/chart.gif"))
        .expect("the GIF fixture");
    for fixture in ["gif_svg_pictures.docx", "gif_svg_pictures.pptx"] {
        let input = repository_root().join("test-data").join(fixture);
        let output = ws.join(&format!("{fixture}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--to",
            "json",
            "--to",
            "html",
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(pictures.len(), 2, "{fixture}: {pictures:#?}");

        let by_size = |width: f64, height: f64| {
            pictures
                .iter()
                .find(|picture| {
                    picture["image"]["size"]["width"] == width
                        && picture["image"]["size"]["height"] == height
                })
                .unwrap_or_else(|| {
                    panic!("{fixture}: no {width} × {height} picture: {pictures:#?}")
                })
        };

        // The GIF's pixels, re-encoded as the PNG an office reader leaves.
        let image = &by_size(60.0, 36.0)["image"];
        assert_eq!(image["mimetype"], "image/png", "{fixture}: the GIF");
        let uri = image["uri"].as_str().expect("an image uri");
        assert!(
            uri.ends_with(".png"),
            "{fixture}: the GIF was written as {uri}"
        );
        let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        let expected =
            pdfextractor_image_parser::transcode_to_png(&gif).expect("the GIF fixture decodes");
        assert_eq!(
            written, expected.data,
            "{fixture}: {uri} is not the transcode of the GIF it came from"
        );

        // The drawing, played at the size the package places it: 72 × 43
        // points is 144 × 86 pixels, and those pixels are the drawing.
        let image = &by_size(144.0, 86.0)["image"];
        assert_eq!(image["mimetype"], "image/png", "{fixture}: the drawing");
        assert_eq!(
            image["dpi"].as_u64(),
            Some(144),
            "{fixture}: the drawing was not played at 144 dpi"
        );
        let uri = image["uri"].as_str().expect("an image uri");
        assert!(
            uri.ends_with(".png"),
            "{fixture}: the drawing was written as {uri}"
        );
        let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        assert_eq!(
            assert_rendered_from_the_diagram(fixture, &written),
            (144, 86),
            "{fixture}: {uri}"
        );

        // The writer names each picture by the file it was given: a picture the
        // model holds is a picture the page points at.
        let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
        assert_eq!(
            html.matches("<img src=\"").count(),
            2,
            "{fixture}: a picture is missing from the render:\n{html}"
        );
        assert_eq!(
            html.matches(".png\"").count(),
            2,
            "{fixture}: a picture is still markup in the render:\n{html}"
        );
        assert!(
            !html.contains(".svg\""),
            "{fixture}: the render still points at a drawing rather than its picture:\n{html}"
        );

        let words = doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        for sentence in ["Before the pictures.", "After the pictures."] {
            assert!(
                words.contains(sentence),
                "{fixture}: {sentence} did not survive: {words}"
            );
        }
    }
}

/// A list item's marker can be a picture rather than a glyph: Word names one
/// with `w:lvlPicBulletId` -> `w:numPicBullet`, PowerPoint with an `a:buBlip`
/// on the paragraph. Either way the bullet is a media part, so it has to leave
/// as a picture of its own — and a drawing named the way both applications name
/// an SVG (a raster fallback on the `a:blip`, the drawing in an `asvg:svgBlip`
/// beside it) has to be played, at the size the bullet is drawn in: 18 points in
/// Word's `wp:extent` is 36 pixels at 144 dpi, and 24 points of PowerPoint type
/// size is 48. The raster bullet beside it is a picture already and is kept as
/// stored. A list item whose bullet is still vector markup, or a grey fallback
/// thumbnail where the drawing was, is the failure this guards.
#[test]
fn a_picture_bullet_in_a_list_is_a_picture_of_its_drawing() {
    let ws = Workspace::new("picture-bullets");
    for (fixture, drawn) in [
        ("picture_bullets.docx", (36, 36)),
        ("picture_bullets.pptx", (48, 48)),
    ] {
        let input = repository_root().join("test-data").join(fixture);
        let output = ws.join(&format!("{fixture}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--to",
            "json",
            "--to",
            "html",
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        let texts = doc["texts"].as_array().expect("a text list");
        assert_eq!(texts.len(), 2, "{fixture}: {texts:#?}");
        for (item, words) in texts
            .iter()
            .zip(["Drawn bullet item", "Painted bullet item"])
        {
            assert_eq!(item["text"], words, "{fixture}");
            assert_eq!(item["label"], "list_item", "{fixture}");
            assert!(
                item["marker"].as_str().unwrap_or_default().is_empty(),
                "{fixture}: the picture bullet left a glyph marker beside the words: {item}"
            );
        }

        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(pictures.len(), 2, "{fixture}: {pictures:#?}");

        // The drawing, played at the size the bullet is drawn in.
        let image = &pictures[0]["image"];
        assert_eq!(image["mimetype"], "image/png", "{fixture}: the drawing");
        assert_eq!(
            image["dpi"].as_u64(),
            Some(144),
            "{fixture}: the drawing was not played at 144 dpi"
        );
        let uri = image["uri"].as_str().expect("an image uri");
        assert!(
            uri.ends_with(".png"),
            "{fixture}: the picture bullet was written as {uri}"
        );
        let written = std::fs::read(ws.join(uri)).expect("the bullet's picture");
        assert_eq!(
            assert_rendered_from_the_diagram(fixture, &written),
            drawn,
            "{fixture}: {uri}"
        );

        // The raster bullet: the pixels it stored, not re-encoded.
        let image = &pictures[1]["image"];
        assert_eq!(
            image["mimetype"], "image/png",
            "{fixture}: the raster bullet"
        );
        let uri = image["uri"].as_str().expect("an image uri");
        assert!(
            uri.ends_with(".png"),
            "{fixture}: the raster bullet was written as {uri}"
        );
        let written = std::fs::read(ws.join(uri)).expect("the raster bullet's picture");
        let decoded = image::load_from_memory(&written)
            .expect("the raster bullet decodes")
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (24, 16), "{fixture}");
        assert_eq!(
            decoded.get_pixel(0, 0),
            &image::Rgba([192, 32, 32, 255]),
            "{fixture}"
        );

        // The render points at both pictures, and at nothing still vector.
        let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
        assert_eq!(
            html.matches("<img src=\"").count(),
            2,
            "{fixture}: a bullet is missing from the render:\n{html}"
        );
        assert!(
            !html.contains(".svg\""),
            "{fixture}: the render still points at a drawing rather than its picture:\n{html}"
        );
    }
}

/// A GIF can hold an animation, and its frames are pages, not one picture: each
/// frame is what a reader sees for a moment, drawn on the file's logical
/// screen. Stopping at the first frame lost two pages of three, so the fixture
/// says its own name on every frame and each page is held to its own words —
/// and to no other frame's. The pages are all the screen's size, at the 72 dpi
/// a GIF's missing resolution means.
#[test]
fn an_animated_gif_is_a_page_per_frame() {
    let ws = Workspace::new("animated-gif");
    let bytes = std::fs::read(repository_root().join("test-data/images/turning.gif"))
        .expect("the animation fixture");
    let input = ws.write("turning.gif", &bytes);
    let output = ws.join("turning.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "always",
    ]);

    let doc = json_of(&output);
    let pages = doc["pages"].as_object().expect("a page list");
    assert_eq!(pages.len(), 3, "{pages:?}");
    for page in 1..=3 {
        let size = &doc["pages"][page.to_string()]["size"];
        assert_eq!(size["width"], 480.0, "page {page}");
        assert_eq!(size["height"], 160.0, "page {page}");
    }

    let words_on = |page: i64| -> String {
        doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .filter(|item| item["prov"][0]["page_no"].as_i64() == Some(page))
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let frames = [(1, "FRAME ONE"), (2, "FRAME TWO"), (3, "FRAME THREE")];
    for (page, own) in frames {
        let words = words_on(page);
        assert!(
            words.contains(own),
            "page {page} should show {own}, and shows {words:?}"
        );
        for (other_page, other) in frames {
            if other_page != page {
                assert!(
                    !words.contains(other),
                    "page {page} also shows frame {other_page}'s words: {words:?}"
                );
            }
        }
    }
}

/// An animated WebP is the same strip as an animated GIF: its frames are pages,
/// not one picture, and each is drawn on the canvas the `ANIM` chunk states.
/// Stopping at the first frame would lose two pages of three, so the fixture
/// says its own words on every frame and each page is held to its own words —
/// and to no other frame's. The pages are the canvas's size, at the 72 dpi a
/// WebP's missing resolution means.
#[test]
fn an_animated_webp_is_a_page_per_frame() {
    let ws = Workspace::new("animated-webp");
    let bytes = std::fs::read(repository_root().join("test-data/images/turning.webp"))
        .expect("the animation fixture");
    let input = ws.write("turning.webp", &bytes);
    let output = ws.join("turning.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "always",
    ]);

    let doc = json_of(&output);
    assert_eq!(doc["origin"]["mimetype"], "image/webp");
    let pages = doc["pages"].as_object().expect("a page list");
    assert_eq!(pages.len(), 3, "{pages:?}");
    for page in 1..=3 {
        let size = &doc["pages"][page.to_string()]["size"];
        assert_eq!(size["width"], 480.0, "page {page}");
        assert_eq!(size["height"], 160.0, "page {page}");
    }

    let words_on = |page: i64| -> String {
        doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .filter(|item| item["prov"][0]["page_no"].as_i64() == Some(page))
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let frames = [(1, "WEBP ONE"), (2, "WEBP TWO"), (3, "WEBP THREE")];
    for (page, own) in frames {
        let words = words_on(page);
        assert!(
            words.contains(own),
            "page {page} should show {own}, and shows {words:?}"
        );
        for (other_page, other) in frames {
            if other_page != page {
                assert!(
                    !words.contains(other),
                    "page {page} also shows frame {other_page}'s words: {words:?}"
                );
            }
        }
    }
}

/// An animated WebP's EXIF tag is the file's, so it turns the whole strip: every
/// frame of an animation was stored the same way, because they are one picture.
/// `turning_turned.webp` is the three pages of `turning.webp` stored a quarter
/// turn anticlockwise on a 160 × 480 canvas, with tag 6 saying to turn them back.
/// A reader that applied the tag to the frame it measured — the first — and not
/// to the rest would report page one upright and pages two and three on their
/// side, at the stored grid and with their words unreadable; each sheet says its
/// own words, so a page turned by the wrong tag, or left unturned, shows the
/// wrong words or none rather than passing quietly.
///
/// The pages are held to the upright 480 × 160 the tag describes, to each page's
/// own two lines and no other page's, and to line boxes that are horizontal and
/// inside the page — which only upright pixels give.
#[test]
fn an_animated_webps_exif_orientation_turns_every_frame() {
    let ws = Workspace::new("turned-animated-webp");
    let bytes = std::fs::read(repository_root().join("test-data/images/turning_turned.webp"))
        .expect("the turned animation fixture");
    let input = ws.write("turning_turned.webp", &bytes);
    let output = ws.join("turning_turned.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "always",
    ]);

    let doc = json_of(&output);
    assert_eq!(doc["origin"]["mimetype"], "image/webp");
    let pages = doc["pages"].as_object().expect("a page list");
    assert_eq!(pages.len(), 3, "{pages:?}");
    for page in 1..=3 {
        let size = &doc["pages"][page.to_string()]["size"];
        assert_eq!(
            (
                size["width"].as_f64().expect("a width"),
                size["height"].as_f64().expect("a height"),
            ),
            (480.0, 160.0),
            "page {page} is not the upright frame: {size}"
        );
    }

    let frames = [
        (1, "WEBP ONE", "Page 1 of 3"),
        (2, "WEBP TWO", "Page 2 of 3"),
        (3, "WEBP THREE", "Page 3 of 3"),
    ];
    for (page, words, subline) in frames {
        let on_page: Vec<&serde_json::Value> = doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .filter(|item| item["prov"][0]["page_no"].as_i64() == Some(page))
            .collect();
        let reading = on_page
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            reading.contains(words),
            "page {page} should show {words}, and shows {reading:?}"
        );
        assert!(
            reading.contains(subline),
            "page {page} should show {subline}, and shows {reading:?}"
        );
        for (other_page, other_words, other_subline) in frames {
            if other_page == page {
                continue;
            }
            assert!(
                !reading.contains(other_words) && !reading.contains(other_subline),
                "page {page} also shows page {other_page}'s words: {reading:?}"
            );
        }
        for item in on_page {
            let bbox = &item["prov"][0]["bbox"];
            let (left, top, right, bottom) = (
                bbox["l"].as_f64().expect("a left edge"),
                bbox["t"].as_f64().expect("a top edge"),
                bbox["r"].as_f64().expect("a right edge"),
                bbox["b"].as_f64().expect("a bottom edge"),
            );
            assert!(
                right - left > bottom - top,
                "page {page} has a sideways line box, so recognition saw the stored grid: {bbox}"
            );
            assert!(
                left >= 0.0 && top >= 0.0 && right <= 480.5 && bottom <= 160.5,
                "page {page} has a box outside the upright page: {bbox}"
            );
        }
    }
}

/// A TIFF is a stack too: `pages.tiff` is a three-sheet batch — the shape a
/// scanner's feeder writes, one image file directory per sheet — and each
/// directory is a page a reader turns, at its own size. A reader that stopped at
/// the first directory would report one page and lose the other two, so the
/// fixture says its own words on every sheet, and the last sheet is a different
/// shape so a page measured from the first one cannot pass. A one-directory TIFF
/// is still the one page it always was.
#[test]
fn a_multiframe_tiff_is_a_page_per_frame() {
    let ws = Workspace::new("multipage-tiff");
    let bytes = std::fs::read(repository_root().join("test-data/images/pages.tiff"))
        .expect("the multi-page fixture");
    let input = ws.write("pages.tiff", &bytes);
    let output = ws.join("pages.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "always",
    ]);

    let doc = json_of(&output);
    assert_eq!(doc["origin"]["mimetype"], "image/tiff");
    let pages = doc["pages"].as_object().expect("a page list");
    assert_eq!(pages.len(), 3, "{pages:?}");
    let sizes: Vec<(f64, f64)> = (1..=3)
        .map(|page| {
            let size = &doc["pages"][page.to_string()]["size"];
            (
                size["width"].as_f64().expect("a width"),
                size["height"].as_f64().expect("a height"),
            )
        })
        .collect();
    // Each sheet at its own pixel grid at the 72 dpi the file declares: one
    // point per pixel, and the third sheet is narrower and taller.
    assert_eq!(
        sizes,
        vec![(480.0, 160.0), (480.0, 160.0), (432.0, 192.0)],
        "{pages:?}"
    );

    let words_on = |page: i64| -> String {
        doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .filter(|item| item["prov"][0]["page_no"].as_i64() == Some(page))
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let sheets = [(1, "PAGE ONE"), (2, "PAGE TWO"), (3, "PAGE THREE")];
    for (page, own) in sheets {
        let words = words_on(page);
        assert!(
            words.contains(own),
            "page {page} should show {own}, and shows {words:?}"
        );
        for (other_page, other) in sheets {
            if other_page != page {
                assert!(
                    !words.contains(other),
                    "page {page} also shows page {other_page}'s words: {words:?}"
                );
            }
        }
    }

    // The still TIFF is one page, at the 150 dpi its header declares, exactly as
    // it was before a stack was a stack.
    let still = repository_root().join("test-data/images/scan.tiff");
    let still_out = ws.join("scan.json");
    run_ok(&[
        "-i",
        still.to_str().unwrap(),
        "-o",
        still_out.to_str().unwrap(),
        "--ocr",
        "never",
    ]);
    let doc = json_of(&still_out);
    assert_eq!(
        doc["pages"].as_object().expect("a page list").len(),
        1,
        "a one-directory TIFF became more than one page"
    );
    // 620 × 350 pixels at 150 dpi: the grid its own resolution entry states.
    assert_eq!(doc["pages"]["1"]["size"]["width"], 297.6);
    assert_eq!(doc["pages"]["1"]["size"]["height"], 168.0);
}

/// Each directory of a TIFF states how its own sheet is stored. A batch whose
/// first sheet is a quarter turn must not turn the rest with it: here the first
/// sheet's stored 6 × 4 grid is tagged 6 (a 4 × 6 page), the second is stored
/// upright at 5 × 3 and tagged 1, and the third is stored 6 × 4 and tagged 8
/// (another 4 × 6 page). Pages are their own upright grids, not the first
/// sheet's shape applied to every directory.
#[test]
fn a_tiffs_sheets_are_turned_by_their_own_orientation_tags() {
    use tiff::encoder::colortype::RGB8;
    use tiff::encoder::Rational;
    use tiff::tags::{ResolutionUnit, Tag};

    let mut encoded = std::io::Cursor::new(Vec::new());
    {
        let mut tiff = tiff::encoder::TiffEncoder::new(&mut encoded).expect("a TIFF encoder");
        for (width, height, orientation) in [(6u32, 4u32, 6u16), (5, 3, 1), (6, 4, 8)] {
            let mut page = tiff
                .new_image::<RGB8>(width, height)
                .expect("a directory per sheet");
            page.resolution(ResolutionUnit::Inch, Rational { n: 72, d: 1 });
            page.encoder()
                .write_tag(Tag::Orientation, orientation)
                .expect("the sheet's own orientation");
            page.write_data(&vec![0x80; (width * height * 3) as usize])
                .expect("the sheet's pixels");
        }
    }

    let ws = Workspace::new("oriented-tiff");
    let input = ws.write("oriented.tiff", &encoded.into_inner());
    let output = ws.join("oriented.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pages = doc["pages"].as_object().expect("a page list");
    assert_eq!(pages.len(), 3, "{pages:?}");
    let sizes: Vec<(f64, f64)> = (1..=3)
        .map(|page| {
            let size = &doc["pages"][page.to_string()]["size"];
            (
                size["width"].as_f64().expect("a width"),
                size["height"].as_f64().expect("a height"),
            )
        })
        .collect();
    // 72 dpi, so one pixel is one point. The first and third sheets are on
    // their side and become 4 × 6; the second is already upright at 5 × 3 and
    // must not be turned by the first sheet's tag.
    assert_eq!(sizes, vec![(4.0, 6.0), (5.0, 3.0), (4.0, 6.0)], "{pages:?}");
}

/// The same batch fed the other way up: `pages_turned.tiff` stores its first
/// sheet upright and tagged 1, its second a quarter turn the other way and
/// tagged 6, and its third the opposite quarter and tagged 8. Every page is the
/// same 480 × 160 once turned, but two of the three are stored as 160 × 480.
///
/// Recognition reads pixels, not tags, so a single page's turn would cost the
/// others their text: this is the end-to-end half of the rule the synthetic
/// fixture above pins, with words a recogniser can actually read. It holds
///
/// * the pages: three, each at the upright 480 × 160 and not the stored grid
///   two of them arrived in;
/// * the words: each page's own two lines, and no other page's, so a sheet
///   turned by its neighbour's tag fails by showing the wrong words rather than
///   by silence; and
/// * the boxes: every line horizontal and inside the page, which is only true
///   if the engine was handed upright pixels — sideways ones give no such
///   lines.
#[test]
fn a_tiffs_turned_sheets_are_read_upright_by_ocr() {
    let ws = Workspace::new("turned-tiff-ocr");
    let bytes = std::fs::read(repository_root().join("test-data/images/pages_turned.tiff"))
        .expect("the turned multi-page fixture");
    let input = ws.write("pages_turned.tiff", &bytes);
    let output = ws.join("pages_turned.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "always",
    ]);

    let doc = json_of(&output);
    assert_eq!(doc["origin"]["mimetype"], "image/tiff");
    let pages = doc["pages"].as_object().expect("a page list");
    assert_eq!(pages.len(), 3, "{pages:?}");
    for page in 1..=3 {
        let size = &doc["pages"][page.to_string()]["size"];
        assert_eq!(
            (
                size["width"].as_f64().expect("a width"),
                size["height"].as_f64().expect("a height"),
            ),
            (480.0, 160.0),
            "page {page} is not the upright sheet: {size}"
        );
    }

    let sheets = [
        (1, "PAGE ONE", "Sheet 1 of 3"),
        (2, "PAGE TWO", "Sheet 2 of 3"),
        (3, "PAGE THREE", "Sheet 3 of 3"),
    ];
    for (page, words, subline) in sheets {
        let on_page: Vec<&serde_json::Value> = doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .filter(|item| item["prov"][0]["page_no"].as_i64() == Some(page))
            .collect();
        let reading = on_page
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            reading.contains(words),
            "page {page} should show {words}, and shows {reading:?}"
        );
        assert!(
            reading.contains(subline),
            "page {page} should show {subline}, and shows {reading:?}"
        );
        for (other_page, other_words, other_subline) in sheets {
            if other_page == page {
                continue;
            }
            assert!(
                !reading.contains(other_words) && !reading.contains(other_subline),
                "page {page} also shows page {other_page}'s words: {reading:?}"
            );
        }
        for item in on_page {
            let bbox = &item["prov"][0]["bbox"];
            let (left, top, right, bottom) = (
                bbox["l"].as_f64().expect("a left edge"),
                bbox["t"].as_f64().expect("a top edge"),
                bbox["r"].as_f64().expect("a right edge"),
                bbox["b"].as_f64().expect("a bottom edge"),
            );
            assert!(
                right - left > bottom - top,
                "page {page} has a sideways line box, so recognition saw the stored grid: {bbox}"
            );
            assert!(
                left >= 0.0 && top >= 0.0 && right <= 480.5 && bottom <= 160.5,
                "page {page} has a box outside the upright page: {bbox}"
            );
        }
    }
}

/// Word and PowerPoint 2016 and later state an SVG picture twice: the `a:blip`'s
/// own `r:embed` names a PNG fallback, for readers that cannot draw SVG, and the
/// drawing itself is an `asvg:svgBlip` extension beside it. The fallback is what
/// an old reader shows, not the picture the file asks for — its resolution was
/// chosen for that old reader — so the conversion must take the drawing and
/// render it, not hand on the thumbnail beside it.
///
/// Both fixtures are a PNG picture completed the way Word completes one: the
/// fallback part, a second media part for the drawing, a relationship reaching
/// it, and the extension in the drawing part. The test first holds the fixture
/// to that shape — a package that lost its `svgBlip` would otherwise pass by
/// testing the fallback alone — then the conversion to a picture drawn from the
/// drawing, at the frame the document places it in: the fallback is 40 × 24 at
/// its own grid, so a 144 × 86 picture with the diagram's panel and stroke on it
/// cannot be that thumbnail.
#[test]
fn an_svg_picture_stated_in_an_svg_blip_is_the_drawing_not_its_png_fallback() {
    let ws = Workspace::new("svg-blip");

    for (fixture, drawing, fallback) in [
        (
            "svg_blip_pictures.docx",
            "word/document.xml",
            "word/media/image1.png",
        ),
        (
            "svg_blip_pictures.pptx",
            "ppt/slides/slide1.xml",
            "ppt/media/image1.png",
        ),
    ] {
        let input = repository_root().join("test-data").join(fixture);
        let bytes = std::fs::read(&input).expect("the office fixture");
        assert_picture_parts_are_declared_and_related(&package_parts(&bytes, fixture), fixture);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
            .unwrap_or_else(|e| panic!("{fixture}: not a ZIP: {e}"));
        let mut read_part = |name: &str| {
            let mut part = archive
                .by_name(name)
                .unwrap_or_else(|e| panic!("{fixture}: no {name}: {e}"));
            let mut data = String::new();
            std::io::Read::read_to_string(&mut part, &mut data).expect("read the part");
            data
        };
        let drawing_xml = read_part(drawing);
        assert!(
            drawing_xml.contains("svgBlip") && drawing_xml.contains("r:embed"),
            "{fixture}: the drawing does not state an SVG beside its raster"
        );
        drop(read_part);
        assert!(
            archive.by_name(fallback).is_ok(),
            "{fixture}: the PNG fallback the extension sits beside is gone"
        );
        drop(archive);

        let output = ws.join(&format!("{fixture}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--to",
            "json",
            "--to",
            "html",
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(pictures.len(), 1, "{fixture}: {pictures:#?}");
        let image = &pictures[0]["image"];
        assert_eq!(
            image["mimetype"], "image/png",
            "{fixture}: the drawing was left as markup: {image:#?}"
        );
        let uri = image["uri"].as_str().expect("an image uri");
        assert!(
            uri.ends_with(".png"),
            "{fixture}: the drawing was written as {uri}"
        );
        let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        assert_eq!(
            assert_rendered_from_the_diagram(fixture, &written),
            (144, 86),
            "{fixture}: {uri} is not the drawing the package carries"
        );

        let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
        assert!(
            html.matches("<img src=\"").count() == 1 && html.contains(".png\""),
            "{fixture}: the render does not point at the picture:\n{html}"
        );
        let words = doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        for sentence in ["Before the picture.", "After the picture."] {
            assert!(
                words.contains(sentence),
                "{fixture}: {sentence} did not survive: {words}"
            );
        }
    }
}

/// The same drawing can sit in a package two ways, and both mean the same
/// picture: Word's way — the raster on the picture's own `a:blip`, the drawing in
/// an `asvg:svgBlip` extension beside it — and LibreOffice's, where the picture's
/// own relationship names the SVG and there is no raster at all. A reader that
/// knew only the first spelling reported Word's SVG as its raster twin; one that
/// knew only the second missed the extension. Both are read, so the same document
/// has to come out of both.
///
/// Each pair is written from the same sentences and the same 40 × 24 drawing, so
/// this test writes each variant into a workspace of its own under one name — the
/// pictures a conversion names are named after it — and holds the two JSON
/// documents and the two HTML renders to equality, apart from the input's hash,
/// which is the one thing that must differ because the bytes do.
#[test]
fn the_same_svg_converts_the_same_however_it_is_stored() {
    for (word, embedded, drawing_part, rels_part) in [
        (
            "svg_blip_pictures.docx",
            "svg_embed_pictures.docx",
            "word/document.xml",
            "word/_rels/document.xml.rels",
        ),
        (
            "svg_blip_pictures.pptx",
            "svg_embed_pictures.pptx",
            "ppt/slides/slide1.xml",
            "ppt/slides/_rels/slide1.xml.rels",
        ),
    ] {
        let extension = word
            .rsplit_once('.')
            .map(|(_, extension)| extension)
            .expect("a fixture with an extension");
        let name = format!("svg_picture.{extension}");

        // Hold each fixture to its own spelling before comparing the two, or the
        // equality could hold by both files being the same package twice.
        let spelling = |label: &str| {
            let bytes = fixture(label);
            assert_picture_parts_are_declared_and_related(&package_parts(&bytes, label), label);
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
                .unwrap_or_else(|e| panic!("{label}: not a ZIP: {e}"));
            let mut read = |part: &str| {
                let mut text = String::new();
                std::io::Read::read_to_string(
                    &mut archive
                        .by_name(part)
                        .unwrap_or_else(|e| panic!("{label}: no {part}: {e}")),
                    &mut text,
                )
                .expect("read the part");
                text
            };
            (read(drawing_part), read(rels_part))
        };
        let (word_drawing, word_rels) = spelling(word);
        let (embedded_drawing, embedded_rels) = spelling(embedded);
        assert!(
            word_drawing.contains("svgBlip") && word_rels.contains("image1.png"),
            "{word}: the drawing is not stated beside a raster fallback"
        );
        assert!(
            !embedded_drawing.contains("svgBlip") && embedded_rels.contains("diagram.svg"),
            "{embedded}: the drawing is not the picture's own part"
        );

        // One name for both, so both conversions name their pictures alike.
        let word_ws = Workspace::new("svg-storage-word");
        let embedded_ws = Workspace::new("svg-storage-embedded");
        let word_input = word_ws.write(&name, &fixture(word));
        let embedded_input = embedded_ws.write(&name, &fixture(embedded));

        let convert = |ws: &Workspace, input: &Path| {
            let output = ws.join(&format!("{name}.json"));
            run_ok(&[
                "-i",
                input.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--to",
                "json",
                "--to",
                "html",
                "--ocr",
                "never",
            ]);
            let doc = json_of(&output);
            (output, doc)
        };
        let (word_output, mut word_doc) = convert(&word_ws, &word_input);
        let (embedded_output, mut embedded_doc) = convert(&embedded_ws, &embedded_input);

        let hash = |doc: &serde_json::Value| doc["origin"]["binary_hash"].clone();
        assert_ne!(
            hash(&word_doc),
            hash(&embedded_doc),
            "{word}: both fixtures are the same bytes, so storage is not what is tested"
        );
        for doc in [&mut word_doc, &mut embedded_doc] {
            doc["origin"]
                .as_object_mut()
                .expect("an origin")
                .remove("binary_hash");
        }
        assert_eq!(
            word_doc, embedded_doc,
            "{word}: where the drawing is stored changed the document"
        );

        let word_html = std::fs::read_to_string(word_output.with_extension("html"))
            .expect("the Word-stored render");
        let embedded_html = std::fs::read_to_string(embedded_output.with_extension("html"))
            .expect("the embedded render");
        assert_eq!(
            word_html, embedded_html,
            "{word}: the two storages render differently"
        );
    }
}

/// An SVG is authored against its own viewport and placed in whatever frame the
/// document gives it. When the two have different proportions the drawing's own
/// `preserveAspectRatio` decides how they meet: the default `xMidYMid meet`
/// keeps the drawing's shape and leaves the rest of the frame transparent,
/// while `none` stretches the two axes independently. Every other SVG fixture
/// here states no attribute, so those all ask the default; this one asks `none`.
///
/// `svg_stretch_picture.docx` places a 40 × 24 drawing split red and blue in a
/// 2.5 × 0.5 in frame — five to one, where the drawing is five to three. The
/// rendered picture is held to the frame and its pixels: 180 × 36 points at
/// 144 dpi is 360 × 72, the halves are still left and right, and the pixels a
/// default fit would have left as margin at either side are the drawing's own
/// ink. A reader that ignored the attribute would centre a 120-pixel-wide
/// drawing in a 360-pixel frame, which the two margin samples catch.
#[test]
fn an_svg_stating_none_is_stretched_to_its_frame() {
    let file = "svg_stretch_picture.docx";
    let bytes = fixture(file);
    // The fixture's claim first: the drawing it carries states the non-default
    // fit, so a package that quietly lost the attribute cannot pass this test by
    // looking like a default fit.
    let parts = package_parts(&bytes, file);
    let drawing = parts
        .iter()
        .find(|(name, _)| name.ends_with(".svg"))
        .map(|(_, drawing)| String::from_utf8_lossy(drawing).to_string())
        .unwrap_or_else(|| panic!("{file}: the package carries no drawing"));
    assert!(
        drawing.contains(r#"preserveAspectRatio="none""#),
        "{file}: the drawing does not state the fit this test is about: {drawing}"
    );

    let ws = Workspace::new("svg-stretch");
    let input = ws.write(file, &bytes);
    let output = ws.join("svg_stretch_picture.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(pictures.len(), 1, "{pictures:#?}");
    let image = &pictures[0]["image"];
    assert_eq!(image["mimetype"], "image/png");
    assert_eq!(image["dpi"].as_u64(), Some(144));
    assert_eq!(image["size"]["width"].as_f64(), Some(360.0), "{image:#?}");
    assert_eq!(image["size"]["height"].as_f64(), Some(72.0), "{image:#?}");
    let uri = image["uri"].as_str().expect("an image uri");
    assert!(uri.ends_with(".png"), "the drawing was written as {uri}");
    let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
    let drawn = image::load_from_memory(&written)
        .unwrap_or_else(|e| panic!("{uri} does not decode: {e}"))
        .to_rgba8();
    assert_eq!(
        drawn.dimensions(),
        (360, 72),
        "{uri} is not the frame's grid"
    );

    const RED: [u8; 4] = [192, 57, 43, 255];
    const BLUE: [u8; 4] = [47, 109, 181, 255];
    for (x, y, wanted, why) in [
        (2, 2, RED, "the frame's corner is not the red half"),
        (
            30,
            36,
            RED,
            "the margin a default fit leaves on the left is not the drawing's ink",
        ),
        (90, 36, RED, "the left half is not red"),
        (270, 36, BLUE, "the right half is not blue"),
        (
            330,
            36,
            BLUE,
            "the margin a default fit leaves on the right is not the drawing's ink",
        ),
        (
            357,
            69,
            BLUE,
            "the frame's opposite corner is not the blue half",
        ),
    ] {
        assert_eq!(drawn.get_pixel(x, y).0, wanted, "{uri}: {why}");
    }
}

/// A picture stated twice without an `asvg:svgBlip`: the drawing in an
/// `mc:AlternateContent`'s `mc:Choice`, and, in `mc:Fallback`, the copy an older
/// reader shows — a VML `v:imagedata` in Word, a second `p:pic` in PowerPoint.
/// Only the choice is the picture; taking the fallback too would put the picture
/// on the page twice, and taking it instead would report a grey box where the
/// file draws a diagram.
///
/// Each package's second picture is what a fallback is for: its choice names a
/// drawing the package does not hold, so the orange 60 × 36 raster in the
/// fallback is the picture the document means and must not be lost to the
/// choice's empty promise.
///
/// The test first holds each fixture to that shape — a package that lost its
/// `mc:Fallback` would otherwise pass by testing the choice alone — then the
/// conversion to one picture per drawing, and to the fallback where it is the
/// picture.
#[test]
fn a_picture_in_an_alternate_content_choice_is_the_drawing_not_its_fallback() {
    let ws = Workspace::new("mc-svg");
    for (fixture, drawing, fallback_copy) in [
        (
            "mc_svg_pictures.docx",
            "word/document.xml",
            // The VML copy of each picture: one `v:imagedata` per fallback.
            ("v:imagedata", 2),
        ),
        (
            "mc_svg_pictures.pptx",
            "ppt/slides/slide1.xml",
            // The choice's picture and the fallback's, for both pictures.
            ("<p:pic>", 4),
        ),
    ] {
        let input = repository_root().join("test-data").join(fixture);
        let bytes = std::fs::read(&input).expect("the office fixture");
        assert_picture_parts_are_declared_and_related(&package_parts(&bytes, fixture), fixture);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
            .unwrap_or_else(|e| panic!("{fixture}: not a ZIP: {e}"));
        let mut drawing_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name(drawing)
                .unwrap_or_else(|e| panic!("{fixture}: no {drawing}: {e}")),
            &mut drawing_xml,
        )
        .expect("read the drawing part");
        drop(archive);
        assert_eq!(
            drawing_xml.matches("<mc:AlternateContent>").count(),
            2,
            "{fixture}: the pictures are not stated as alternate content"
        );
        assert_eq!(
            drawing_xml.matches("<mc:Fallback>").count(),
            2,
            "{fixture}: an alternate content with no fallback is not the case tested"
        );
        assert_eq!(
            drawing_xml.matches(fallback_copy.0).count(),
            fallback_copy.1,
            "{fixture}: the fallback does not carry its own copy of the picture"
        );

        let output = ws.join(&format!("{fixture}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--to",
            "json",
            "--to",
            "html",
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(
            pictures.len(),
            2,
            "{fixture}: the choice and its fallback are not two pictures, and each \
             picture is one: {pictures:#?}"
        );

        // The drawing the package holds, played at the picture's own frame, in
        // place of the grey box beside it.
        let image = &pictures[0]["image"];
        assert_eq!(
            (
                image["size"]["width"].as_f64(),
                image["size"]["height"].as_f64()
            ),
            (Some(80.0), Some(48.0)),
            "{fixture}: the first picture is not the drawing's size: {image:#?}"
        );
        assert_eq!(
            image["mimetype"], "image/png",
            "{fixture}: the drawing was left as markup: {image:#?}"
        );
        let uri = image["uri"].as_str().expect("an image uri");
        assert!(
            uri.ends_with(".png"),
            "{fixture}: the drawing was written as {uri}"
        );
        let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        assert_eq!(
            assert_rendered_from_the_diagram(fixture, &written),
            (80, 48),
            "{fixture}: {uri} is not the drawing the package carries"
        );

        // The fallback, where the choice names a part that is not there.
        let image = &pictures[1]["image"];
        assert_eq!(
            (
                image["size"]["width"].as_f64(),
                image["size"]["height"].as_f64()
            ),
            (Some(60.0), Some(36.0)),
            "{fixture}: the fallback is the second picture, at its own size: {image:#?}"
        );
        assert_eq!(
            image["mimetype"], "image/png",
            "{fixture}: the fallback picture is not a raster"
        );
        let uri = image["uri"].as_str().expect("an image uri");
        let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        let raster = image::load_from_memory(&written)
            .expect("the fallback decodes")
            .to_rgba8();
        assert_eq!(raster.dimensions(), (60, 36), "{fixture}: {uri}");
        assert_eq!(
            raster.get_pixel(0, 0).0,
            [224, 140, 40, 255],
            "{fixture}: {uri} is not the orange box the fallback carries"
        );

        let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
        assert_eq!(
            html.matches("<img src=\"").count(),
            2,
            "{fixture}: the render does not show one picture per drawing:\n{html}"
        );

        // The fallback's words are the choice's words read a second time.
        let texts = doc["texts"].as_array().expect("a text list");
        let words = texts
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        for sentence in ["Before the pictures.", "After the pictures."] {
            assert_eq!(
                words.matches(sentence).count(),
                1,
                "{fixture}: {sentence} is not the page's once: {words}"
            );
        }
    }
}

/// A picture can be stated for an older reader only: an `mc:AlternateContent`
/// with no `mc:Choice`, only `mc:Fallback`. The fallback is then the picture,
/// not a duplicate of one beside it. Each fixture states two 60 × 36 orange
/// rasters that way, with a sentence before and after each: one in the body, and
/// one inside a container the reader has to walk into rather than meet in the
/// body — a table's cell in the DOCX, a group on the slide — so alternate content
/// is read wherever the picture stands and not only where a paragraph of the body
/// starts.
///
/// The test first holds each package to that shape — two `mc:AlternateContent`s,
/// two `mc:Fallback`s, no `mc:Choice`, one of them in the body and one in the
/// container, each fallback carrying its picture — because a fixture that grew a
/// choice would otherwise pass by testing it — and then to the conversion: two
/// 60 × 36 `image/png`s written beside the JSON with the orange raster's own
/// pixels, two `<img>`s in the render, the container's picture read between the
/// sentences that frame the container, and each sentence once.
#[test]
fn a_fallback_only_picture_is_the_fallback() {
    /// Where a fixture's second picture is stated: the container the reader has
    /// to walk into, rather than a paragraph of the body.
    struct PictureIn {
        fixture: &'static str,
        /// The part the body's pictures are stated in.
        drawing: &'static str,
        /// What a fallback branch names its picture with.
        fallback_copy: &'static str,
        /// How many fallback branches the part states: the body's picture and
        /// every container's. The DOCX has four here — the body, a cell, a cell
        /// in a nested table and a text box — and one more in its header part.
        fallbacks: usize,
        /// How many pictures the conversion reads from the package: the body's
        /// and each container's, the header's furniture picture included.
        pictures: usize,
        /// How many of them a rendering of the document shows: furniture is not
        /// body content, so a header's picture is read but not rendered.
        rendered: usize,
        /// The sentences the fixture states, each exactly once.
        sentences: &'static [&'static str],
        /// The element the second picture is written inside, and its close: a
        /// table's cell in the DOCX, a group on the slide. The first picture is
        /// outside it, in the body.
        open: &'static str,
        close: &'static str,
        /// What the fixture's own sentences call the container.
        name: &'static str,
    }

    let ws = Workspace::new("mc-fallback");
    for container in [
        PictureIn {
            fixture: "mc_fallback_pictures.docx",
            drawing: "word/document.xml",
            fallback_copy: "v:imagedata",
            fallbacks: 4,
            pictures: 5,
            rendered: 4,
            sentences: &[
                "Before the picture.",
                "After the picture.",
                "Before the table picture.",
                "After the table picture.",
                "Before the nested table picture.",
                "After the nested table picture.",
                "Before the text box picture.",
                "After the text box picture.",
                "The header.",
            ],
            open: "<w:tc>",
            close: "</w:tc>",
            name: "table",
        },
        PictureIn {
            fixture: "mc_fallback_pictures.pptx",
            drawing: "ppt/slides/slide1.xml",
            fallback_copy: "<p:pic>",
            fallbacks: 2,
            pictures: 2,
            rendered: 2,
            sentences: &[
                "Before the picture.",
                "After the picture.",
                "Before the group picture.",
                "After the group picture.",
            ],
            open: "<p:grpSp>",
            close: "</p:grpSp>",
            name: "group",
        },
    ] {
        let PictureIn {
            fixture,
            drawing,
            fallback_copy,
            fallbacks,
            pictures: picture_count,
            rendered,
            sentences,
            open,
            close,
            name,
        } = container;
        let input = repository_root().join("test-data").join(fixture);
        let bytes = std::fs::read(&input).expect("the office fixture");
        assert_picture_parts_are_declared_and_related(&package_parts(&bytes, fixture), fixture);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
            .unwrap_or_else(|e| panic!("{fixture}: not a ZIP: {e}"));
        let mut drawing_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name(drawing)
                .unwrap_or_else(|e| panic!("{fixture}: no {drawing}: {e}")),
            &mut drawing_xml,
        )
        .expect("read the drawing part");
        drop(archive);

        assert_eq!(
            drawing_xml.matches("<mc:AlternateContent>").count(),
            fallbacks,
            "{fixture}: the pictures are not stated as alternate content"
        );
        assert_eq!(
            drawing_xml.matches("<mc:Choice>").count(),
            0,
            "{fixture}: a choice is stated beside a fallback, so the pictures are not \
             fallback-only"
        );
        assert_eq!(
            drawing_xml.matches("<mc:Fallback>").count(),
            fallbacks,
            "{fixture}: the fallback is not each picture's only branch"
        );

        // One picture is the body's own; the other is stated inside the
        // container, where a reader that stops at the body's paragraphs never
        // reaches it.
        let (before_the_container, rest) = drawing_xml
            .split_once(open)
            .unwrap_or_else(|| panic!("{fixture}: no {open} holds the second picture"));
        let inside_the_container = rest
            .split_once(close)
            .map(|(inside, _)| inside)
            .unwrap_or_else(|| panic!("{fixture}: {open} is never closed"));
        for (place, xml) in [
            ("in the body".to_string(), before_the_container),
            (format!("in the {name}"), inside_the_container),
        ] {
            assert_eq!(
                xml.matches("<mc:AlternateContent>").count(),
                1,
                "{fixture}: the picture {place} is not stated as alternate content"
            );
            let fallback = xml
                .split("<mc:Fallback>")
                .nth(1)
                .and_then(|rest| rest.split("</mc:Fallback>").next())
                .unwrap_or_else(|| panic!("{fixture}: the picture {place} has no fallback"));
            assert!(
                fallback.contains(fallback_copy),
                "{fixture}: the fallback {place} does not carry the picture"
            );
        }

        let output = ws.join(&format!("{fixture}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--to",
            "json",
            "--to",
            "html",
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(
            pictures.len(),
            picture_count,
            "{fixture}: every fallback the package states is a picture of the document: \
             {pictures:#?}"
        );
        for picture in pictures {
            let image = &picture["image"];
            assert_eq!(
                (
                    image["size"]["width"].as_f64(),
                    image["size"]["height"].as_f64()
                ),
                (Some(60.0), Some(36.0)),
                "{fixture}: a fallback is not its own size: {image:#?}"
            );
            assert_eq!(
                image["mimetype"], "image/png",
                "{fixture}: a fallback is not a raster: {image:#?}"
            );
            let uri = image["uri"].as_str().expect("an image uri");
            let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
            let raster = image::load_from_memory(&written)
                .expect("a fallback decodes")
                .to_rgba8();
            assert_eq!(raster.dimensions(), (60, 36), "{fixture}: {uri}");
            assert_eq!(
                raster.get_pixel(0, 0).0,
                [224, 140, 40, 255],
                "{fixture}: {uri} is not the orange box a fallback carries"
            );
        }

        let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
        assert_eq!(
            html.matches("<img src=\"").count(),
            rendered,
            "{fixture}: the render does not show every body picture:\n{html}"
        );
        let texts = doc["texts"].as_array().expect("a text list");
        let words = texts
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        let before = format!("Before the {name} picture.");
        let after = format!("After the {name} picture.");
        for sentence in sentences {
            assert_eq!(
                words.matches(sentence).count(),
                1,
                "{fixture}: {sentence} is not the page's once: {words}"
            );
        }

        // The container's picture is read where the container stands: between
        // the two sentences that frame it, not somewhere else in the document.
        let order = reading_order(&doc);
        let position = |reference: String| {
            order
                .iter()
                .position(|item| *item == reference)
                .unwrap_or_else(|| panic!("{fixture}: {reference} is not in the document"))
        };
        let text_at = |sentence: &str| {
            let index = texts
                .iter()
                .position(|item| item["text"].as_str() == Some(sentence))
                .unwrap_or_else(|| panic!("{fixture}: the document does not state {sentence}"));
            position(format!("#/texts/{index}"))
        };
        if fixture.ends_with(".docx") {
            // A Word cell holds its own picture: the table stands between the
            // sentences that frame it, and the picture is in the group that
            // cell references — not a body item beside the table, which would
            // be a picture the cell does not hold.
            let group = doc["groups"]
                .as_array()
                .expect("a group list")
                .iter()
                .find(|group| {
                    group["children"].as_array().is_some_and(|children| {
                        children.iter().any(|child| child["$ref"] == "#/pictures/1")
                    })
                })
                .unwrap_or_else(|| {
                    panic!("{fixture}: no group holds the cell's picture: {doc:#?}")
                });
            let cell = doc["tables"]
                .as_array()
                .expect("a table list")
                .iter()
                .flat_map(|table| {
                    table["data"]["table_cells"]
                        .as_array()
                        .into_iter()
                        .flatten()
                })
                .find(|cell| cell["ref"]["$ref"] == group["self_ref"])
                .unwrap_or_else(|| {
                    panic!("{fixture}: no cell references the group holding the picture")
                });
            assert_eq!(
                cell["text"], "",
                "{fixture}: the cell holding the picture is not the empty cell it was"
            );
            let table = group["parent"]["$ref"]
                .as_str()
                .unwrap_or_else(|| panic!("{fixture}: the group has no parent"));
            let table_at = position(table.to_string());
            assert!(
                text_at(&before) < table_at && table_at < text_at(&after),
                "{fixture}: the table holding the cell's picture is not read between the \
                 sentences around it"
            );
        } else {
            let second_picture = position("#/pictures/1".to_string());
            assert!(
                text_at(&before) < second_picture && second_picture < text_at(&after),
                "{fixture}: the {name}'s picture is not read between the sentences around it"
            );
        }
    }
}

/// The `ordinal`-th `open … close` element in `xml`, counting from zero.
///
/// The container a picture is stated in is not always the first of its kind —
/// the DOCX has a table in a table, so the cell that holds the nested picture is
/// the third `<w:tc>` — and the markup between two containers is exactly what a
/// test that only looked at the first one would mistake for it.
fn nth_element<'a>(xml: &'a str, open: &str, close: &str, ordinal: usize) -> &'a str {
    let mut search = 0;
    let mut start = 0;
    for _ in 0..=ordinal {
        let found = xml[search..]
            .find(open)
            .map(|index| search + index)
            .unwrap_or_else(|| panic!("no {open} number {ordinal}"));
        start = found;
        search = found + open.len();
    }
    // The element can hold another of its own kind — the nested table's cell is
    // inside the outer cell — so its end is the close that takes the depth back
    // to zero, not the first close after it.
    let mut depth = 0usize;
    let mut cursor = start;
    loop {
        let next_open = xml[cursor..].find(open).map(|index| cursor + index);
        let next_close = xml[cursor..].find(close).map(|index| cursor + index);
        match (next_open, next_close) {
            (Some(opened), Some(closed)) if opened < closed => {
                depth += 1;
                cursor = opened + open.len();
            }
            (_, Some(closed)) => {
                depth -= 1;
                if depth == 0 {
                    return &xml[start + open.len()..closed];
                }
                cursor = closed + close.len();
            }
            (Some(_), None) => panic!("{open} number {ordinal} is never closed"),
            (None, None) => panic!("{open} number {ordinal} is never closed"),
        }
    }
}

/// A fallback-only picture is read wherever a Word document states it: a cell
/// of a table, a cell of a table inside another table's cell, a text box's own
/// content, and a header. Each is a container the reader has to walk into, and
/// each is stated only in `mc:Fallback` — no `mc:Choice` beside it — so a reader
/// that stops at the body's paragraphs, or that reads only the choice, loses it.
///
/// The fixture puts the same 60 × 36 orange raster in all four places and in the
/// body, so the test can follow one picture through each container: the tables'
/// cells become rich cells holding the picture in a group under the table, the
/// text box's picture is read where the box stands in the body, and the header's
/// is furniture. The outer table is the one whose cell holds the nested table —
/// a Word cell is flow content, and a table in it is a block the reader reports
/// in its own right — so its cell carries the words and the nested table follows
/// it; the nested picture is the nested table's cell's.
#[test]
fn a_fallback_only_picture_in_a_nested_table_a_text_box_and_a_header_is_read() {
    let name = "mc_fallback_pictures.docx";
    let bytes = fixture(name);
    assert_picture_parts_are_declared_and_related(&package_parts(&bytes, name), name);

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
        .unwrap_or_else(|e| panic!("{name}: not a ZIP: {e}"));
    let mut parts = std::collections::HashMap::new();
    for part in ["word/document.xml", "word/header1.xml"] {
        let mut xml = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name(part)
                .unwrap_or_else(|e| panic!("{name}: no {part}: {e}")),
            &mut xml,
        )
        .expect("read the part");
        parts.insert(part, xml);
    }
    drop(archive);
    let document = &parts["word/document.xml"];
    let header = &parts["word/header1.xml"];

    // Where each fallback stands in the fixture itself, before the reading is
    // asked anything: a cell of the outer table, the third cell (the nested
    // table's, inside the outer cell), the text box's content, and the header.
    assert_eq!(document.matches("<w:tc>").count(), 3, "{document}");
    let (before_the_tables, _) = document
        .split_once("<w:tc>")
        .unwrap_or_else(|| panic!("{name}: no table cell"));
    assert_eq!(
        before_the_tables.matches("<mc:Fallback>").count(),
        1,
        "{name}: the body's picture is not outside every container"
    );
    let outer_cell = nth_element(document, "<w:tc>", "</w:tc>", 1);
    let nested_cell = nth_element(document, "<w:tc>", "</w:tc>", 2);
    assert!(
        outer_cell.contains(nested_cell) && outer_cell.contains("<w:tbl>"),
        "{name}: the nested table is not inside the outer cell"
    );
    for (place, xml, fallbacks) in [
        (
            "the table's cell",
            nth_element(document, "<w:tc>", "</w:tc>", 0),
            1,
        ),
        ("the outer cell", outer_cell, 1),
        ("the nested table's cell", nested_cell, 1),
        (
            "the text box",
            nth_element(document, "<w:txbxContent>", "</w:txbxContent>", 0),
            1,
        ),
    ] {
        assert_eq!(
            xml.matches("<mc:Fallback>").count(),
            fallbacks,
            "{name}: the picture in {place} is not stated where the test says it is"
        );
    }
    assert_eq!(
        header.matches("<mc:Fallback>").count(),
        1,
        "{name}: the header does not state a fallback picture"
    );
    assert!(
        header.contains("v:imagedata"),
        "{name}: the header's fallback does not name its picture"
    );
    assert_eq!(
        document.matches("<mc:Choice>").count() + header.matches("<mc:Choice>").count(),
        0,
        "{name}: a choice is stated beside a fallback, so a picture is not fallback-only"
    );

    let ws = Workspace::new("mc-fallback-containers");
    let output = ws.join("out.json");
    run_ok(&[
        "-i",
        repository_root()
            .join("test-data")
            .join(name)
            .to_str()
            .unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    let doc = json_of(&output);

    // Five pictures: the body's, the table's, the nested table's, the text
    // box's, and the header's — the last on the furniture layer.
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(pictures.len(), 5, "{pictures:#?}");
    for (index, picture) in pictures.iter().enumerate() {
        let image = &picture["image"];
        assert_eq!(
            (
                image["size"]["width"].as_f64(),
                image["size"]["height"].as_f64(),
            ),
            (Some(60.0), Some(36.0)),
            "picture {index} is not the fallback's size: {image:#?}"
        );
        let layer = if index == 4 { "furniture" } else { "body" };
        assert_eq!(picture["content_layer"], layer, "picture {index}");
        let uri = image["uri"].as_str().expect("an image uri");
        let raster = image::load_from_memory(
            &std::fs::read(ws.join(uri)).expect("the picture written beside the JSON"),
        )
        .expect("the fallback decodes")
        .to_rgba8();
        assert_eq!(
            raster.get_pixel(0, 0).0,
            [224, 140, 40, 255],
            "picture {index} ({uri}) is not the orange box a fallback carries"
        );
    }

    // The two table pictures are their cells': each is in a group whose parent
    // is the table, among the table's children, and the cell that references it
    // holds no words of its own. The outer table is the third, and its cell says
    // what the fixture says it says.
    let tables = doc["tables"].as_array().expect("a table list");
    assert_eq!(tables.len(), 3, "{tables:#?}");
    for (table_index, picture_index) in [(0usize, 1usize), (1, 2)] {
        let table = &tables[table_index];
        let group = doc["groups"]
            .as_array()
            .expect("a group list")
            .iter()
            .find(|group| {
                group["children"].as_array().is_some_and(|children| {
                    children
                        .iter()
                        .any(|child| child["$ref"] == format!("#/pictures/{picture_index}"))
                })
            })
            .unwrap_or_else(|| panic!("no group holds picture {picture_index}: {doc:#?}"));
        assert_eq!(
            group["parent"]["$ref"],
            format!("#/tables/{table_index}"),
            "the group holding picture {picture_index} is not the table's"
        );
        assert!(
            table["children"].as_array().is_some_and(|children| {
                children
                    .iter()
                    .any(|child| child["$ref"] == group["self_ref"])
            }),
            "the table does not list the group holding picture {picture_index}"
        );
        let cell = table["data"]["table_cells"]
            .as_array()
            .expect("a cell list")
            .iter()
            .find(|cell| cell["ref"]["$ref"] == group["self_ref"])
            .unwrap_or_else(|| {
                panic!("no cell references the group holding picture {picture_index}")
            });
        assert_eq!(
            cell["text"], "",
            "the cell holding picture {picture_index} is not the empty cell it was"
        );
    }
    let outer = &tables[2]["data"]["table_cells"];
    assert_eq!(
        outer.as_array().map(Vec::len),
        Some(1),
        "the outer table is not the one-cell table it was"
    );
    assert_eq!(outer[0]["text"], "The outer cell.", "{outer:#?}");
    assert!(
        outer[0].get("ref").is_none(),
        "the outer cell holds a table, not a picture"
    );

    // The text box's picture and the table that holds the nested one stand
    // between the sentences that frame them, in the body's own order.
    let order = reading_order(&doc);
    let position = |reference: String| {
        order
            .iter()
            .position(|item| *item == reference)
            .unwrap_or_else(|| panic!("{reference} is not in the body"))
    };
    let texts = doc["texts"].as_array().expect("a text list");
    let text_at = |sentence: &str| {
        let index = texts
            .iter()
            .position(|item| item["text"].as_str() == Some(sentence))
            .unwrap_or_else(|| panic!("the document does not state {sentence}"));
        position(format!("#/texts/{index}"))
    };
    let text_box = position("#/pictures/3".to_string());
    assert!(
        text_at("Before the text box picture.") < text_box
            && text_box < text_at("After the text box picture."),
        "the text box's picture is not read between the sentences around it"
    );
    let nested_table = position("#/tables/1".to_string());
    assert!(
        text_at("Before the nested table picture.") < nested_table
            && nested_table < text_at("After the nested table picture."),
        "the nested table's picture is not read between the sentences around it"
    );

    // The header's picture is furniture, after the header's own words — the
    // header is not body content and does not join the body's order.
    let furniture = doc["furniture"]["children"]
        .as_array()
        .expect("a furniture list");
    let header_words = furniture
        .iter()
        .position(|child| {
            child["$ref"]
                .as_str()
                .and_then(|reference| reference.strip_prefix("#/texts/"))
                .and_then(|index| index.parse::<usize>().ok())
                .is_some_and(|index| texts[index]["text"] == "The header.")
        })
        .unwrap_or_else(|| panic!("the header's words are not furniture: {furniture:#?}"));
    let header_picture = furniture
        .iter()
        .position(|child| child["$ref"] == "#/pictures/4")
        .unwrap_or_else(|| panic!("the header's picture is not furniture: {furniture:#?}"));
    assert!(
        header_words < header_picture,
        "the header's picture is not read after its words: {furniture:#?}"
    );
    assert!(
        !order.iter().any(|item| item == "#/pictures/4"),
        "the header's picture joined the body's order"
    );

    // The header's picture is furniture, so the rendering of the body shows the
    // four body pictures and not the header's — the JSON above is where the
    // header's is held.
    let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
    assert_eq!(
        html.matches("<img src=\"").count(),
        4,
        "the render does not show every body fallback:\n{html}"
    );
}

/// A shape can be filled with a picture rather than a colour, and the fill
/// names its media the way a picture does: the raster on the `a:blip`, the
/// drawing in an `asvg:svgBlip` extension beside it. PowerPoint states such a
/// shape as a `p:sp` whose `p:spPr` holds the `a:blipFill`; Word states it as a
/// `wps:wsp` in `w:drawing`. The fill belongs to the shape, so the shape is
/// painted whole: the picture is drawn through the shape's geometry, under its
/// line, and what leaves the reader is one picture of what the page shows rather
/// than the fill's bare image standing where the shape is.
///
/// Each fixture holds two filled shapes: one filled with
/// `images/diagram.svg`, whose raster fallback is a grey box deliberately unlike
/// the drawing, and which carries the words "Inside the fill."; and one filled
/// with an orange 60 × 36 raster and no drawing at all, so the path a file with
/// no SVG takes is covered too.
///
/// The test first holds each fixture to its shape — two `a:blipFill`s, and one
/// `svgBlip` — then the conversion to two pictures: the drawing drawn into the
/// shape's frame, so a 288 × 173 picture with the diagram's panel and stroke on
/// it, and the raster drawn into its shape's 60 × 36 points, so a 120 × 72 one.
/// Neither reaches the frame's corner, because the shape is a rounded rectangle
/// and the fill stops at its outline: the picture is the shape, not the image a
/// box was standing in for. The shape's words are read once beside it.
#[test]
fn a_shape_filled_with_a_picture_is_painted_into_the_shape() {
    let ws = Workspace::new("blip-fill");
    for (fixture, drawing) in [
        ("blip_fill_shapes.docx", "word/document.xml"),
        ("blip_fill_shapes.pptx", "ppt/slides/slide1.xml"),
    ] {
        let input = repository_root().join("test-data").join(fixture);
        let bytes = std::fs::read(&input).expect("the office fixture");
        assert_picture_parts_are_declared_and_related(&package_parts(&bytes, fixture), fixture);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
            .unwrap_or_else(|e| panic!("{fixture}: not a ZIP: {e}"));
        let mut drawing_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive
                .by_name(drawing)
                .unwrap_or_else(|e| panic!("{fixture}: no {drawing}: {e}")),
            &mut drawing_xml,
        )
        .expect("read the drawing part");
        drop(archive);
        assert_eq!(
            drawing_xml.matches("<a:blipFill>").count(),
            2,
            "{fixture}: the shapes are not filled with pictures"
        );
        assert_eq!(
            drawing_xml.matches("svgBlip").count(),
            1,
            "{fixture}: exactly one fill states a drawing beside its raster"
        );

        let output = ws.join(&format!("{fixture}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--to",
            "json",
            "--to",
            "html",
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(
            pictures.len(),
            2,
            "{fixture}: the two fills are two pictures: {pictures:#?}"
        );

        // The fill that states a drawing is the drawing rendered, not the box
        // beside it.
        let image = &pictures[0]["image"];
        assert_eq!(
            image["mimetype"], "image/png",
            "{fixture}: the drawing was left as markup: {image:#?}"
        );
        let uri = image["uri"].as_str().expect("an image uri");
        assert!(
            uri.ends_with(".png"),
            "{fixture}: the drawing was written as {uri}"
        );
        let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        assert_eq!(
            assert_rendered_from_the_diagram(fixture, &written),
            (288, 173),
            "{fixture}: {uri} is not the drawing the fill states"
        );
        // The fill belongs to the shape: the drawing is painted through its
        // rounded rectangle, so the frame's corner is the page, not the panel.
        let painted = image::load_from_memory(&written)
            .expect("the painted shape decodes")
            .to_rgba8();
        assert_eq!(
            painted.get_pixel(0, 0).0,
            [255, 255, 255, 255],
            "{fixture}: {uri} is the fill's picture laid down as a box, not the shape"
        );

        // The fill that states only a raster is painted into its shape too, at
        // the size the page places it.
        let image = &pictures[1]["image"];
        assert_eq!(
            (
                image["size"]["width"].as_f64(),
                image["size"]["height"].as_f64()
            ),
            (Some(120.0), Some(72.0)),
            "{fixture}: the raster fill is not the shape's 60 × 36 points at 144 dpi: {image:#?}"
        );
        assert_eq!(
            image["mimetype"], "image/png",
            "{fixture}: the raster fill is not a raster"
        );
        let uri = image["uri"].as_str().expect("an image uri");
        let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        let raster = image::load_from_memory(&written)
            .expect("the painted shape decodes")
            .to_rgba8();
        // 60 × 36 points at 144 dpi, whatever the fill's own grid was.
        assert_eq!(raster.dimensions(), (120, 72), "{fixture}: {uri}");
        assert_eq!(
            raster.get_pixel(60, 36).0,
            [224, 140, 40, 255],
            "{fixture}: {uri} is not the orange box the fill carries"
        );
        assert_eq!(
            raster.get_pixel(0, 0).0,
            [255, 255, 255, 255],
            "{fixture}: {uri} is the fill's picture laid down as a box, not the shape"
        );

        let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
        assert_eq!(
            html.matches("<img src=\"").count(),
            2,
            "{fixture}: the render does not show one picture per fill:\n{html}"
        );

        // The shape's words are the page's, once, beside its fill.
        let words = doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        for sentence in [
            "Before the filled shapes.",
            "Inside the fill.",
            "After the filled shapes.",
        ] {
            assert_eq!(
                words.matches(sentence).count(),
                1,
                "{fixture}: {sentence} is not the page's once: {words}"
            );
        }
    }
}

/// Every part of a package, as its name and its bytes.
fn package_parts(bytes: &[u8], label: &str) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .unwrap_or_else(|e| panic!("{label}: not a ZIP: {e}"));
    let mut parts = Vec::new();
    for index in 0..archive.len() {
        let mut part = archive
            .by_index(index)
            .unwrap_or_else(|e| panic!("{label}: part {index}: {e}"));
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut part, &mut data).expect("read a package part");
        parts.push((part.name().to_string(), data));
    }
    parts
}

/// Open a package with headless LibreOffice, write it back, and return what it
/// wrote — the package itself, and every part of it.
///
/// The caller has already found `soffice`. A profile of its own, as the CLI's
/// own bridge uses: a LibreOffice the user has open keeps the default one
/// locked, and a locked profile makes `--headless` exit without converting
/// anything.
fn resave_with_libreoffice(
    soffice: &Path,
    ws: &Workspace,
    fixture: &str,
    bytes: &[u8],
) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let extension = fixture
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or_else(|| panic!("{fixture}: a fixture with no extension"));
    let input = ws.write(fixture, bytes);
    let out = ws.join("resaved");
    std::fs::create_dir_all(&out).expect("the output directory");
    let profile = ws.join("profile");
    let run = Command::new(soffice)
        .arg("--headless")
        .arg("--norestore")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.to_string_lossy()
        ))
        .arg("--convert-to")
        .arg(extension)
        .arg("--outdir")
        .arg(&out)
        .arg(&input)
        .output()
        .expect("spawn soffice");
    assert!(
        run.status.success(),
        "{fixture}: LibreOffice could not re-save it ({}): {}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    let resaved = out.join(fixture);
    let package = std::fs::read(&resaved)
        .unwrap_or_else(|e| panic!("{fixture}: LibreOffice wrote no {}: {e}", resaved.display()));
    let parts = package_parts(&package, fixture);
    (package, parts)
}

/// One picture this project's reader took from a package: what it says the
/// picture is, how big it is, and the bytes it wrote beside the JSON.
#[derive(Debug, PartialEq)]
struct ReadPicture {
    mimetype: String,
    size: (f64, f64),
    bytes: Vec<u8>,
}

impl ReadPicture {
    /// The picture as pixels, or `None` where it is markup rather than a raster
    /// (`image` decodes no SVG).
    fn decoded(&self) -> Option<image::RgbaImage> {
        image::load_from_memory(&self.bytes)
            .ok()
            .map(|drawn| drawn.to_rgba8())
    }
}

/// Whether two pictures are the same picture: the same type at the same size,
/// with the same pixels where both are rasters and the same bytes where they are
/// markup. A PNG another writer re-encoded is the same picture; a different
/// drawing is not.
fn the_same_picture(one: &ReadPicture, other: &ReadPicture) -> bool {
    if one.mimetype != other.mimetype || one.size != other.size {
        return false;
    }
    match (one.decoded(), other.decoded()) {
        (Some(drawn), Some(other_drawn)) => drawn == other_drawn,
        _ => one.bytes == other.bytes,
    }
}

/// Convert a package with the CLI and return the pictures it reported, in the
/// order the document states them.
///
/// `tag` keeps two conversions of the same fixture apart inside one workspace;
/// the input keeps the fixture's own name, so the reader is handed the same
/// extension it would be given by a user.
fn pictures_the_reader_takes(
    ws: &Workspace,
    fixture: &str,
    package: &[u8],
    tag: &str,
) -> Vec<ReadPicture> {
    let input = ws.write(&format!("{tag}/{fixture}"), package);
    let output = ws.join(&format!("{tag}/{fixture}.json"));
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    doc["pictures"]
        .as_array()
        .expect("a pictures list")
        .iter()
        .map(|picture| {
            let image = &picture["image"];
            let uri = image["uri"].as_str().unwrap_or_else(|| {
                panic!("{fixture}: a picture the reader left without an image: {picture:#?}")
            });
            ReadPicture {
                mimetype: image["mimetype"]
                    .as_str()
                    .expect("a picture's mimetype")
                    .to_string(),
                size: (
                    image["size"]["width"].as_f64().expect("a picture's width"),
                    image["size"]["height"]
                        .as_f64()
                        .expect("a picture's height"),
                ),
                // A picture is written beside the JSON, so its URI is relative
                // to the JSON's own directory.
                bytes: std::fs::read(ws.join(tag).join(uri))
                    .unwrap_or_else(|e| panic!("{fixture}: {uri} was not written: {e}")),
            }
        })
        .collect()
}

/// Assert that this reader reads `resaved` as the same pictures it reads from
/// the fixture `before` — a re-save is only a re-save if both readers agree on
/// what the document holds.
fn assert_the_reader_agrees(fixture: &str, before: &[ReadPicture], after: &[ReadPicture]) {
    assert_eq!(
        after.len(),
        before.len(),
        "{fixture}: the re-saved package gives {} picture(s) where the fixture gives {}:\n\
         re-saved: {after:#?}\nfixture: {before:#?}",
        after.len(),
        before.len()
    );
    for (index, (one, other)) in before.iter().zip(after).enumerate() {
        assert!(
            the_same_picture(one, other),
            "{fixture}: picture {index} of the re-saved package is not the fixture's:\n\
             re-saved: {other:#?}\nfixture: {one:#?}"
        );
    }
}

/// A page's picture background as the JSON names it: the media type, the size in
/// points, the resolution, and the bytes written beside the JSON.
#[derive(Debug, PartialEq)]
struct Backdrop {
    mimetype: String,
    size: (f64, f64),
    dpi: u64,
    bytes: Vec<u8>,
}

/// What this reader makes of a deck's pages: each page's backdrop in page order,
/// `None` where the page states none, the pictures it reports as items, and the
/// document's words.
#[derive(Debug, PartialEq)]
struct PageBackdrops {
    pages: Vec<Option<Backdrop>>,
    pictures: usize,
    words: Vec<String>,
}

/// Convert a package with the CLI and return the backdrop each page states.
///
/// A backdrop is the page's `image`, not a picture item, so this reads `pages`
/// rather than `pictures`: a deck whose background came back as a shape would be
/// a different reading of the same page, which is why the item count is carried
/// with it.
fn backdrops_the_reader_takes(
    ws: &Workspace,
    fixture: &str,
    package: &[u8],
    tag: &str,
) -> PageBackdrops {
    let input = ws.write(&format!("{tag}/{fixture}"), package);
    let output = ws.join(&format!("{tag}/{fixture}.json"));
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pages = doc["pages"].as_object().expect("a pages map");
    let mut numbers: Vec<u32> = pages
        .keys()
        .map(|number| number.parse().expect("a page number"))
        .collect();
    numbers.sort_unstable();
    let pages = numbers
        .into_iter()
        .map(|number| {
            pages[&number.to_string()]["image"]
                .as_object()
                .map(|image| {
                    let uri = image["uri"].as_str().unwrap_or_else(|| {
                        panic!("{fixture}: a page image with no file: {image:#?}")
                    });
                    Backdrop {
                        mimetype: image["mimetype"]
                            .as_str()
                            .expect("a page image's mimetype")
                            .to_string(),
                        size: (
                            image["size"]["width"]
                                .as_f64()
                                .expect("a page image's width"),
                            image["size"]["height"]
                                .as_f64()
                                .expect("a page image's height"),
                        ),
                        dpi: image["dpi"].as_u64().expect("a page image's resolution"),
                        bytes: std::fs::read(ws.join(tag).join(uri))
                            .unwrap_or_else(|e| panic!("{fixture}: {uri} was not written: {e}")),
                    }
                })
        })
        .collect();

    PageBackdrops {
        pages,
        pictures: doc["pictures"].as_array().expect("a pictures list").len(),
        words: doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default().to_string())
            .collect(),
    }
}

/// Whether some part's relationships reach `name`, by the file name the target
/// ends with. A part nothing points at is a file left in the package, not a
/// picture the document holds.
fn reached_by_a_relationship(parts: &[(String, Vec<u8>)], name: &str) -> bool {
    let file_name = name.rsplit('/').next().unwrap_or(name);
    parts
        .iter()
        .filter(|(part, _)| part.ends_with(".rels"))
        .any(|(_, data)| {
            String::from_utf8_lossy(data)
                .split("Target=\"")
                .skip(1)
                .filter_map(|target| target.split('"').next())
                .any(|target| target.ends_with(file_name))
        })
}

/// Whether a package states a part's media type as `mime` — by an `<Override>`
/// naming the part, or a `<Default>` for its extension. Both are declarations of
/// the same fact; which one a writer picks is its business.
///
/// The type is matched as the whole `ContentType` value rather than as a
/// substring of the declaration, so `image/svg` does not stand in for
/// `image/svg+xml`: a package naming a type that is not the registered one has
/// not declared the part.
fn declared_as(parts: &[(String, Vec<u8>)], name: &str, mime: &str) -> bool {
    let Some((_, data)) = parts.iter().find(|(part, _)| part == "[Content_Types].xml") else {
        return false;
    };
    let types = String::from_utf8_lossy(data);
    let path = format!("/{}", name.replace('\\', "/"));
    let states = |declaration: &str| declaration.contains(&format!("ContentType=\"{mime}\""));
    if types.split("<Override ").any(|declaration| {
        declaration.contains(&format!("PartName=\"{path}\"")) && states(declaration)
    }) {
        return true;
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension);
    extension.is_some_and(|extension| {
        types.split("<Default ").any(|declaration| {
            declaration.contains(&format!("Extension=\"{extension}\"")) && states(declaration)
        })
    })
}

/// Every picture part a package holds, held to the package's own statements: a
/// media part whose media type no `[Content_Types].xml` entry declares, or that
/// no relationship reaches, is a file in a ZIP rather than a picture the
/// document holds. A fixture whose parts a test opened by name would otherwise
/// pass after the declaration or the relationship that makes them pictures went
/// missing, because the test never asked the package whether they were.
///
/// The extensions are the ones these fixtures carry; an extension this does not
/// know is not a picture part it can judge, and is left alone rather than
/// guessed at.
fn assert_picture_parts_are_declared_and_related(parts: &[(String, Vec<u8>)], label: &str) {
    let seen = picture_parts_declared_and_related(parts, label);
    assert!(
        seen > 0,
        "{label}: the package holds no picture part to hold to its statements"
    );
}

/// The same walk as the assertion above, without the requirement that the
/// package hold a picture at all — a workbook of tables is a package with no
/// media part to judge. It answers how many parts it judged, so a caller walking
/// a corpus can tell a fixture with pictures from one without.
fn picture_parts_declared_and_related(parts: &[(String, Vec<u8>)], label: &str) -> usize {
    let mut seen = 0;
    for (name, _) in parts {
        let Some((_, extension)) = name.rsplit_once('.') else {
            continue;
        };
        let mime = match extension.to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "bmp" => "image/bmp",
            "tif" | "tiff" => "image/tiff",
            "emf" => "image/x-emf",
            "wmf" => "image/x-wmf",
            _ => continue,
        };
        if !["word/media/", "ppt/media/", "xl/media/"]
            .iter()
            .any(|directory| name.starts_with(directory))
        {
            continue;
        }
        seen += 1;
        assert!(
            declared_as(parts, name, mime),
            "{label}: {name} is in the package but declared as no {mime} part"
        );
        assert!(
            reached_by_a_relationship(parts, name),
            "{label}: {name} is in the package but no relationship reaches it"
        );
    }
    seen
}

/// Every `.docx`, `.pptx` and `.xlsx` below `directory`, at any depth: the
/// fixtures a parser test reads sit both in `test-data` and in its `real/`
/// corpus of application-written files.
fn fixture_packages(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|e| panic!("missing {}: {e}", directory.display()));
    for entry in entries {
        let path = entry.expect("a fixture entry").path();
        if path.is_dir() {
            found.extend(fixture_packages(&path));
        } else if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("docx" | "pptx" | "xlsx")
        ) {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// Every committed OOXML fixture, held to the same two statements as the tests
/// that open a part by name.
///
/// The parser tests take a fixture's pictures through the reader, which resolves
/// a picture by relationship: a media part no relationship reaches shows up
/// there as a missing picture, so that statement is exercised by the reader's
/// own answer. The declaration is not: no reader in this project consults
/// `[Content_Types].xml` to paint a picture, so a fixture whose media part names
/// no media type reads exactly the same and its test passes. Walking the corpus
/// asks the package itself, so the fixtures those tests read are held to both
/// statements whether or not the test can see them.
#[test]
fn every_fixture_package_declares_and_relates_its_media_parts() {
    let root = repository_root();
    let mut with_pictures = 0;
    let mut judged = 0;
    for path in fixture_packages(&root.join("test-data")) {
        let label = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {label}: {e}"));
        let seen = picture_parts_declared_and_related(&package_parts(&bytes, &label), &label);
        if seen > 0 {
            with_pictures += 1;
            judged += seen;
        }
    }
    assert!(
        with_pictures > 0 && judged > 0,
        "no fixture package holds a picture part to hold to its statements"
    );
}

/// The references a document states, in the order a reader reads them: the
/// body's children, with each chapter group opened where it stands — a slide is
/// one — so a picture inside a group is placed against the words around the
/// group rather than missing from the order.
fn reading_order(doc: &serde_json::Value) -> Vec<String> {
    fn walk(doc: &serde_json::Value, node: &serde_json::Value, order: &mut Vec<String>) {
        let Some(children) = node["children"].as_array() else {
            return;
        };
        for child in children {
            let Some(reference) = child["$ref"].as_str() else {
                continue;
            };
            order.push(reference.to_string());
            if let Some(index) = reference
                .strip_prefix("#/groups/")
                .and_then(|index| index.parse::<usize>().ok())
            {
                walk(doc, &doc["groups"][index], order);
            }
        }
    }
    let mut order = Vec::new();
    walk(doc, &doc["body"], &mut order);
    order
}

/// The hand-written SVG part in the two fixtures is one an office application
/// accepts, not just a file this project happens to read.
///
/// The generator writes the SVG into a package python-docx and python-pptx
/// cannot place one in, so the part is assembled by hand: a renamed media part,
/// a relationship pointing at it and a content-type declaration. Whether that
/// is a package Word or PowerPoint would open is not something this project's
/// own reader can answer, so LibreOffice is asked: it opens each fixture and
/// writes it back, and what it writes is held to the picture parts it kept,
/// which say they were understood rather than merely tolerated —
///
/// * the drawing is still a part of its own, byte for byte the fixture's;
/// * the package still declares that part as `image/svg…`, and a relationship
///   still reaches it;
/// * a raster drawn *from the SVG's markup* is written beside it as the
///   fallback Word's SVG scheme carries: at least the 40 × 24 the drawing
///   states, with its pale fill and blue stroke on it, declared `image/png` and
///   reached by a relationship;
/// * the GIF the document places is still declared `image/gif` and reached by a
///   relationship; and
/// * this reader, handed the package the application wrote, takes the same two
///   pictures from it — the GIF's pixels and the SVG's own markup — because the
///   fallback beside the drawing is a part, not a third picture.
///
/// The raster is the proof of acceptance: a rasteriser read the `<rect>` and the
/// `<polyline>` and painted them. Skipped where there is no LibreOffice, like the
/// other bridge tests.
#[test]
fn libreoffice_opens_and_resaves_the_svg_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the fixture re-save test");
        return;
    };
    let svg = std::fs::read(repository_root().join("test-data/images/diagram.svg"))
        .expect("the SVG fixture");

    for fixture in ["gif_svg_pictures.docx", "gif_svg_pictures.pptx"] {
        let bytes = std::fs::read(repository_root().join("test-data").join(fixture))
            .expect("the office fixture");
        let ws = Workspace::new(fixture);
        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, fixture, &bytes);

        let named = |extension: &str| -> Vec<(String, Vec<u8>)> {
            parts
                .iter()
                .filter(|(name, _)| name.ends_with(extension))
                .cloned()
                .collect()
        };

        // The drawing: one part, the fixture's own bytes. LibreOffice copies a
        // vector part through rather than re-writing the markup.
        let svg_parts = named(".svg");
        assert_eq!(
            svg_parts.len(),
            1,
            "{fixture}: the re-saved package holds {} SVG parts: {:#?}",
            svg_parts.len(),
            parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        let (svg_name, svg_bytes) = &svg_parts[0];
        assert_eq!(
            svg_bytes.as_slice(),
            svg.as_slice(),
            "{fixture}: {svg_name} is not the drawing the fixture carries"
        );

        // The package still says what that part is, and the drawing still
        // reaches it: a part nothing points at is a file the application put
        // there, not a picture it kept.
        assert!(
            declared_as(&parts, svg_name, "image/svg"),
            "{fixture}: {svg_name} is not declared as an SVG part"
        );
        assert!(
            reached_by_a_relationship(&parts, svg_name),
            "{fixture}: no relationship reaches {svg_name}"
        );

        // The fallback: a raster the application drew from the markup, at the
        // drawing's own size and with its two colours on it.
        let mut fallbacks = 0;
        for (name, data) in named(".png") {
            // A part the application could not draw from the markup is left
            // empty; it is counted as no raster rather than as a bad image, so
            // the failure says what went wrong.
            if data.is_empty() {
                continue;
            }
            let drawn = image::load_from_memory(&data)
                .unwrap_or_else(|e| panic!("{fixture}: {name} does not decode: {e}"))
                .to_rgb8();
            if drawn.width() < 40 || drawn.height() < 24 {
                continue;
            }
            let mut fill = 0;
            let mut ink = 0;
            for pixel in drawn.pixels() {
                let [r, g, b] = pixel.0;
                if (220..=245).contains(&r) && (228..=250).contains(&g) && b >= 235 {
                    fill += 1;
                }
                // The stroke is #2f6db5 at two pixels wide, so most of its
                // pixels are blends of it and the fill behind it.
                if i32::from(b) > i32::from(r) + 40 && b > 150 {
                    ink += 1;
                }
            }
            assert!(
                fill >= 300 && ink >= 20,
                "{fixture}: {name} ({}x{}) is not a drawing of the fixture's SVG: \
                 {fill} fill pixels, {ink} stroke pixels",
                drawn.width(),
                drawn.height()
            );
            assert!(
                declared_as(&parts, &name, "image/png"),
                "{fixture}: {name} is not declared as an image/png part"
            );
            assert!(
                reached_by_a_relationship(&parts, &name),
                "{fixture}: no relationship reaches {name}"
            );
            fallbacks += 1;
        }
        assert_eq!(
            fallbacks, 1,
            "{fixture}: expected one raster drawn from the SVG, found {fallbacks}"
        );

        // The raster picture is still there too — this is a re-save of both.
        let gif_parts = named(".gif");
        assert_eq!(
            gif_parts.len(),
            1,
            "{fixture}: the GIF part did not survive the re-save"
        );
        let (gif_name, _) = &gif_parts[0];
        assert!(
            declared_as(&parts, gif_name, "image/gif"),
            "{fixture}: {gif_name} is not declared as a GIF part"
        );
        assert!(
            reached_by_a_relationship(&parts, gif_name),
            "{fixture}: no relationship reaches {gif_name}"
        );

        // And the package the application wrote is one this reader reads as the
        // fixture: the GIF's pixels and the SVG's own markup at the sizes the
        // document states. The application's PNG fallback sits beside the
        // drawing as a part of its own; it is not a third picture, and the
        // drawing is still the picture the file asks for.
        let before = pictures_the_reader_takes(&ws, fixture, &bytes, "before");
        let after = pictures_the_reader_takes(&ws, fixture, &resaved, "after");
        assert_the_reader_agrees(fixture, &before, &after);
    }
}

/// LibreOffice accepts the `mc:AlternateContent` fixture packages, where a
/// picture is stated twice: the drawing in an `mc:Choice`, and the copy an older
/// reader shows in `mc:Fallback`.
///
/// Neither office library writes that form, so whether an application opens the
/// package rather than merely tolerating it is a question this project's own
/// reader cannot answer. LibreOffice is asked: it opens each fixture and writes
/// it back, and what it writes is held to
///
/// * the document around the pictures: the sentences before and after them are
///   still there, so the package was read rather than refused;
/// * the drawing: still a part of its own, byte for byte the fixture's, declared
///   an `image/svg…` part with a relationship reaching it;
/// * the raster the application drew from that markup, beside the drawing as the
///   fallback Word's SVG scheme carries: at least the picture's 40 × 24, with the
///   diagram's pale panel and blue stroke on it, declared `image/png` and reached
///   by a relationship; and
/// * the second picture, whose choice names a drawing the package does not hold:
///   the application drops it rather than showing the orange raster of its
///   fallback, so no part of the 60 × 36 grid comes back. That is stated, not
///   hidden — a LibreOffice that honoured that fallback fails here, which is the
///   moment to say so.
///
/// This reader, handed what the application wrote, takes one picture from it: the
/// drawing, at the 80 × 48 its 40 × 24 points state, with the fixture's own
/// pixels. The raster the application drew is a part beside the drawing, not a
/// second picture, and the second picture is gone because no part of it is there.
#[test]
fn libreoffice_opens_and_resaves_the_mc_svg_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the alternate-content re-save test");
        return;
    };
    let svg = std::fs::read(repository_root().join("test-data/images/diagram.svg"))
        .expect("the SVG fixture");

    for fixture in ["mc_svg_pictures.docx", "mc_svg_pictures.pptx"] {
        let bytes = std::fs::read(repository_root().join("test-data").join(fixture))
            .expect("the office fixture");
        let ws = Workspace::new(&format!("mc-svg-resave-{fixture}"));
        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, fixture, &bytes);

        let body = parts
            .iter()
            .find(|(name, _)| name.ends_with("document.xml") || name.ends_with("slide1.xml"))
            .map(|(_, data)| String::from_utf8_lossy(data).to_string())
            .unwrap_or_else(|| panic!("{fixture}: no document body in the re-saved package"));
        for sentence in ["Before the pictures.", "After the pictures."] {
            assert!(
                body.contains(sentence),
                "{fixture}: {sentence} did not survive the re-save"
            );
        }

        let named = |extension: &str| -> Vec<(String, Vec<u8>)> {
            parts
                .iter()
                .filter(|(name, _)| name.ends_with(extension))
                .cloned()
                .collect()
        };

        // The drawing: one part, the fixture's own bytes. LibreOffice copies a
        // vector part through rather than re-writing the markup.
        let svg_parts = named(".svg");
        assert_eq!(
            svg_parts.len(),
            1,
            "{fixture}: the re-saved package holds {} SVG parts: {:#?}",
            svg_parts.len(),
            parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        let (svg_name, svg_bytes) = &svg_parts[0];
        assert_eq!(
            svg_bytes.as_slice(),
            svg.as_slice(),
            "{fixture}: {svg_name} is not the drawing the fixture carries"
        );
        assert!(
            declared_as(&parts, svg_name, "image/svg"),
            "{fixture}: {svg_name} is not declared as an SVG part"
        );
        assert!(
            reached_by_a_relationship(&parts, svg_name),
            "{fixture}: no relationship reaches {svg_name}"
        );

        // The raster the application drew from the markup, on the picture's own
        // frame and declared for what it is.
        let mut rasters = Vec::new();
        for (name, data) in named(".png") {
            let (width, height) = image::load_from_memory(&data)
                .unwrap_or_else(|e| panic!("{fixture}: {name} does not decode: {e}"))
                .to_rgb8()
                .dimensions();
            assert!(
                (width, height) != (60, 36),
                "{fixture}: {name} ({width}x{height}) is the second picture's orange \
                 fallback, so the application now shows the copy beside a choice it \
                 cannot use; this note is out of date"
            );
            assert!(
                width >= 40 && height >= 24,
                "{fixture}: {name} ({width}x{height}) is smaller than the picture it \
                 stands for"
            );
            assert_rendered_from_the_diagram(&format!("{fixture}: {name}"), &data);
            assert!(
                declared_as(&parts, &name, "image/png"),
                "{fixture}: {name} is not declared as an image/png part"
            );
            assert!(
                reached_by_a_relationship(&parts, &name),
                "{fixture}: no relationship reaches {name}"
            );
            rasters.push(name);
        }
        assert_eq!(
            rasters.len(),
            1,
            "{fixture}: expected one raster drawn from the drawing, found {rasters:?}"
        );

        // This reader, on what the application wrote: the drawing, at the fixture's
        // own frame with its own pixels, and nothing of the picture the application
        // dropped.
        let before = pictures_the_reader_takes(&ws, fixture, &bytes, "before");
        let after = pictures_the_reader_takes(&ws, fixture, &resaved, "after");
        assert_eq!(
            before.len(),
            2,
            "{fixture}: the fixture is not the pair this expects: {before:#?}"
        );
        assert_eq!(
            after.len(),
            1,
            "{fixture}: the re-saved package gives {} picture(s): {after:#?}",
            after.len()
        );
        assert!(
            the_same_picture(&before[0], &after[0]),
            "{fixture}: the drawing the application kept is not the fixture's:\n\
             re-saved: {:#?}\nfixture: {:#?}",
            after[0],
            before[0]
        );
        assert!(
            after.iter().all(|picture| picture.size != (60.0, 36.0)),
            "{fixture}: the reader found the second picture in a package that dropped it"
        );
    }
}

/// LibreOffice accepts the `mc_fallback_pictures` fixture packages, where the
/// picture has no `mc:Choice` at all: only the `mc:Fallback` an older reader
/// shows. The fallback is therefore the only branch that can supply the 60 × 36
/// orange raster, and an application that reads it proves the fallback-only
/// form rather than this reader proving it to itself. Each package states two
/// that way, one in the body and one inside a container — a table's cell in the
/// DOCX, a group on the slide — so the container is read by the application too.
///
/// It opens each fixture and writes it back, and what it writes is held to
///
/// * the sentences around both pictures: still there, so the package was read
///   rather than refused;
/// * both pictures: the application kept the body's and the container's. Its
///   DOCX export writes a derived `mc:Choice` beside each original
///   `mc:Fallback`, whose `v:imagedata` still names the picture, and the second
///   is still written inside the cell the picture stood in; its PPTX export
///   writes each fallback as a `p:pic` — the container's inside its `p:grpSp` —
///   because Impress has no fallback branch to keep once it has imported the
///   picture;
/// * the raster: one 60 × 36 `image/png`, declared `image/png` and reached by a
///   relationship, with the orange box's pixels — a picture nothing but the
///   fallback could have supplied, and one part both pictures are stored as; and
/// * this reader, on what the application wrote: the same two 60 × 36 pictures
///   with the same pixels the fixture gave.
#[test]
fn libreoffice_opens_and_resaves_the_fallback_only_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the fallback-only re-save test");
        return;
    };

    for (file, body_part, keeps_a_fallback_branch, fallbacks, sentences) in [
        (
            "mc_fallback_pictures.docx",
            "word/document.xml",
            true,
            4,
            &[
                "Before the picture.",
                "After the picture.",
                "Before the table picture.",
                "After the table picture.",
                "Before the nested table picture.",
                "After the nested table picture.",
                "Before the text box picture.",
                "After the text box picture.",
            ][..],
        ),
        (
            "mc_fallback_pictures.pptx",
            "ppt/slides/slide1.xml",
            false,
            2,
            &[
                "Before the picture.",
                "After the picture.",
                "Before the group picture.",
                "After the group picture.",
            ][..],
        ),
    ] {
        let bytes = fixture(file);
        let ws = Workspace::new(&format!("fallback-resave-{file}"));
        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, file, &bytes);

        let body = parts
            .iter()
            .find(|(name, _)| name == body_part)
            .map(|(_, data)| String::from_utf8_lossy(data).to_string())
            .unwrap_or_else(|| panic!("{file}: no {body_part} in the re-saved package"));
        for sentence in sentences {
            assert!(
                body.contains(sentence),
                "{file}: {sentence} did not survive the re-save"
            );
        }

        if keeps_a_fallback_branch {
            assert_eq!(
                body.matches("<mc:Fallback>").count(),
                fallbacks,
                "{file}: the application did not keep every fallback branch"
            );
            assert_eq!(
                body.matches("<v:imagedata").count(),
                fallbacks,
                "{file}: a fallback it kept does not name its picture"
            );
            // The picture that stood in a cell is still the cell's rather than
            // one the application moved out into the body.
            let cell_start = body
                .find("<w:tc>")
                .unwrap_or_else(|| panic!("{file}: the table's cell did not survive the re-save"));
            let cell = &body[cell_start..];
            let cell_end = cell
                .find("</w:tc>")
                .unwrap_or_else(|| panic!("{file}: the table's cell is never closed"));
            assert!(
                cell[..cell_end].contains("<v:imagedata"),
                "{file}: the cell's fallback is not inside the cell the picture stood in"
            );
            // So are the nested table's cell and the text box's own content.
            let nested = nth_element(&body, "<w:tc>", "</w:tc>", 2);
            assert!(
                nested.contains("<v:imagedata"),
                "{file}: the nested table's fallback is not inside its cell"
            );
            let text_box = nth_element(&body, "<w:txbxContent>", "</w:txbxContent>", 0);
            assert!(
                text_box.contains("<v:imagedata"),
                "{file}: the text box's fallback is not inside the text box"
            );
            // The header is its own part, and the application keeps the picture
            // and the fallback there too.
            let header = parts
                .iter()
                .filter(|(name, _)| name.starts_with("word/header"))
                .map(|(name, data)| (name, String::from_utf8_lossy(data).to_string()))
                .find(|(_, xml)| xml.contains("<v:imagedata"))
                .unwrap_or_else(|| panic!("{file}: no header part kept the picture"));
            assert_eq!(
                header.1.matches("<mc:Fallback>").count(),
                1,
                "{file}: {} does not hold one fallback",
                header.0
            );
            assert!(
                header.1.contains("The header."),
                "{file}: {} lost the header's words",
                header.0
            );
        } else {
            assert_eq!(
                body.matches("<p:pic>").count(),
                2,
                "{file}: the two fallbacks are not the slide's two pictures"
            );
            // The group the second picture stood in was kept, with the picture
            // still inside it.
            assert_eq!(
                body.matches("<p:grpSp>").count(),
                1,
                "{file}: the group did not survive the re-save"
            );
            let (_, group) = body
                .split_once("<p:grpSp>")
                .unwrap_or_else(|| panic!("{file}: no group in the re-saved slide"));
            let group = group
                .split_once("</p:grpSp>")
                .map(|(group, _)| group)
                .unwrap_or_else(|| panic!("{file}: the group is never closed"));
            assert!(
                group.contains("<p:pic>"),
                "{file}: the group's picture is not inside the group"
            );
        }

        let rasters: Vec<(&String, image::RgbaImage)> = parts
            .iter()
            .filter(|(name, _)| name.ends_with(".png"))
            .map(|(name, data)| {
                let drawn = image::load_from_memory(data)
                    .unwrap_or_else(|e| panic!("{file}: {name} does not decode: {e}"))
                    .to_rgba8();
                (name, drawn)
            })
            .collect();
        assert_eq!(
            rasters.len(),
            1,
            "{file}: expected the fallback's one raster, found {:?}",
            rasters.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        let (name, raster) = &rasters[0];
        assert_eq!(
            raster.dimensions(),
            (60, 36),
            "{file}: {name} is not the fallback's grid"
        );
        assert_eq!(
            raster.get_pixel(0, 0).0,
            [224, 140, 40, 255],
            "{file}: {name} is not the orange box the fallback carries"
        );
        assert!(
            declared_as(&parts, name, "image/png"),
            "{file}: {name} is not declared as an image/png part"
        );
        assert!(
            reached_by_a_relationship(&parts, name),
            "{file}: no relationship reaches {name}"
        );

        let before = pictures_the_reader_takes(&ws, file, &bytes, "before");
        let after = pictures_the_reader_takes(&ws, file, &resaved, "after");
        if keeps_a_fallback_branch {
            // The application writes the header into the default and first-page
            // (or even-page) headers, so the header's picture is copied into two
            // parts and this reader takes both copies: one picture more than the
            // fixture holds, and the same picture as the one it copies.
            assert_eq!(
                after.len(),
                before.len() + 1,
                "{file}: the re-saved package gives {} picture(s) where the fixture gives {}",
                after.len(),
                before.len()
            );
            assert!(
                the_same_picture(&before[4], &after[5]),
                "{file}: the header's second copy is not the fixture's header picture"
            );
            assert_the_reader_agrees(file, &before, &after[..before.len()]);
        } else {
            assert_the_reader_agrees(file, &before, &after);
        }
    }
}

/// LibreOffice accepts the `svg_embed_pictures` fixture packages, where the
/// drawing is the picture's own media part — its `r:embed`, with no raster beside
/// it. That is LibreOffice's own spelling of an SVG picture, so a fixture that
/// states one that way is asking whether the application reads a package whose
/// picture is a drawing rather than a raster.
///
/// It opens each fixture and writes it back, and what it writes is held to
///
/// * the sentences around the picture: still there, so the package was read
///   rather than refused;
/// * the drawing: still a part of its own, byte for byte the fixture's, declared
///   an `image/svg…` part with a relationship reaching it;
/// * the raster the application drew from that markup, beside the drawing as the
///   fallback Word's SVG scheme carries: at least the picture's 40 × 24, with the
///   diagram's pale panel and blue stroke on it, declared `image/png` and reached
///   by a relationship; and
/// * this reader, on what the application wrote: the same one picture, at the
///   144 × 86 the package places it in, with the same pixels. The raster the
///   application added is a part beside the drawing, not a second picture.
#[test]
fn libreoffice_opens_and_resaves_the_svg_embed_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the embedded-SVG re-save test");
        return;
    };
    let svg = std::fs::read(repository_root().join("test-data/images/diagram.svg"))
        .expect("the SVG fixture");

    for fixture in ["svg_embed_pictures.docx", "svg_embed_pictures.pptx"] {
        let bytes = std::fs::read(repository_root().join("test-data").join(fixture))
            .expect("the office fixture");
        let ws = Workspace::new(&format!("svg-embed-resave-{fixture}"));
        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, fixture, &bytes);

        let body = parts
            .iter()
            .find(|(name, _)| name.ends_with("document.xml") || name.ends_with("slide1.xml"))
            .map(|(_, data)| String::from_utf8_lossy(data).to_string())
            .unwrap_or_else(|| panic!("{fixture}: no document body in the re-saved package"));
        for sentence in ["Before the picture.", "After the picture."] {
            assert!(
                body.contains(sentence),
                "{fixture}: {sentence} did not survive the re-save"
            );
        }

        let named = |extension: &str| -> Vec<(String, Vec<u8>)> {
            parts
                .iter()
                .filter(|(name, _)| name.ends_with(extension))
                .cloned()
                .collect()
        };

        // The drawing: one part, the fixture's own bytes. LibreOffice copies a
        // vector part through rather than re-writing the markup.
        let svg_parts = named(".svg");
        assert_eq!(
            svg_parts.len(),
            1,
            "{fixture}: the re-saved package holds {} SVG parts: {:#?}",
            svg_parts.len(),
            parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        let (svg_name, svg_bytes) = &svg_parts[0];
        assert_eq!(
            svg_bytes.as_slice(),
            svg.as_slice(),
            "{fixture}: {svg_name} is not the drawing the fixture carries"
        );
        assert!(
            declared_as(&parts, svg_name, "image/svg"),
            "{fixture}: {svg_name} is not declared as an SVG part"
        );
        assert!(
            reached_by_a_relationship(&parts, svg_name),
            "{fixture}: no relationship reaches {svg_name}"
        );

        // The raster the application drew from the markup, beside the drawing as
        // its fallback: one part, on the picture's own frame, declared for what it
        // is.
        let mut rasters = Vec::new();
        for (name, data) in named(".png") {
            let (width, height) = image::load_from_memory(&data)
                .unwrap_or_else(|e| panic!("{fixture}: {name} does not decode: {e}"))
                .to_rgb8()
                .dimensions();
            assert!(
                width >= 40 && height >= 24,
                "{fixture}: {name} ({width}x{height}) is smaller than the picture it \
                 stands for"
            );
            assert_rendered_from_the_diagram(&format!("{fixture}: {name}"), &data);
            assert!(
                declared_as(&parts, &name, "image/png"),
                "{fixture}: {name} is not declared as an image/png part"
            );
            assert!(
                reached_by_a_relationship(&parts, &name),
                "{fixture}: no relationship reaches {name}"
            );
            rasters.push(name);
        }
        assert_eq!(
            rasters.len(),
            1,
            "{fixture}: expected one raster drawn from the drawing, found {rasters:?}"
        );

        // This reader, on what the application wrote: the same picture, at the same
        // frame with the same pixels. The raster beside the drawing is a part, not a
        // second picture.
        let before = pictures_the_reader_takes(&ws, fixture, &bytes, "before");
        let after = pictures_the_reader_takes(&ws, fixture, &resaved, "after");
        assert_eq!(
            before.len(),
            1,
            "{fixture}: the fixture is not the single drawing this expects: {before:#?}"
        );
        assert_the_reader_agrees(fixture, &before, &after);
    }
}

/// LibreOffice accepts the `svg_blip_pictures` fixture packages, where Word's
/// own spelling of an SVG picture is used: a PNG fallback on the picture's
/// `a:blip`, the drawing in an `asvg:svgBlip` extension beside it. The embedded
/// SVG fixture proves the other spelling — the drawing as the picture's own
/// media part — so this asks the application the question that form cannot:
/// whether a package written the way Word writes one is read as a picture rather
/// than as its fallback.
///
/// It opens each fixture and writes it back, and what it writes is held to
///
/// * the fixture itself: both spellings were present before the re-save — the
///   `svgBlip` extension and the 40 × 24 grey fallback beside it — so a fixture
///   that lost the extension would not pass by proving the fallback alone;
/// * the drawing: still a part of its own, byte for byte the fixture's, declared
///   an `image/svg…` part with a relationship reaching it;
/// * the raster the application drew from that markup, beside the drawing as the
///   fallback Word's scheme carries: at least the picture's 40 × 24, with the
///   diagram's pale panel and blue stroke on it, declared `image/png` and reached
///   by a relationship; and
/// * this reader, on what the application wrote: the same one picture, at the
///   same 144 × 86 frame with the same pixels, so the fallback is a part beside
///   the drawing rather than a second picture.
#[test]
fn libreoffice_opens_and_resaves_the_svg_blip_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the svgBlip re-save test");
        return;
    };
    let svg = std::fs::read(repository_root().join("test-data/images/diagram.svg"))
        .expect("the SVG fixture");

    for file in ["svg_blip_pictures.docx", "svg_blip_pictures.pptx"] {
        let bytes = fixture(file);
        let ws = Workspace::new(&format!("svg-blip-resave-{file}"));

        // The fixture is Word's form, not the embedded form the other re-save
        // proves: both the extension and the fallback it sits beside are there
        // before the application is asked anything.
        let original = package_parts(&bytes, file);
        let original_body = original
            .iter()
            .find(|(name, _)| name.ends_with("document.xml") || name.ends_with("slide1.xml"))
            .map(|(_, data)| String::from_utf8_lossy(data).to_string())
            .unwrap_or_else(|| panic!("{file}: no document body in the fixture"));
        assert_eq!(
            original_body.matches("<asvg:svgBlip").count(),
            1,
            "{file}: the fixture does not state the drawing as an svgBlip"
        );
        let original_rasters = original
            .iter()
            .filter(|(name, _)| name.ends_with(".png"))
            .count();
        assert_eq!(
            original_rasters, 1,
            "{file}: the fixture does not carry the fallback the extension sits beside"
        );

        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, file, &bytes);

        let body = parts
            .iter()
            .find(|(name, _)| name.ends_with("document.xml") || name.ends_with("slide1.xml"))
            .map(|(_, data)| String::from_utf8_lossy(data).to_string())
            .unwrap_or_else(|| panic!("{file}: no document body in the re-saved package"));
        for sentence in ["Before the picture.", "After the picture."] {
            assert!(
                body.contains(sentence),
                "{file}: {sentence} did not survive the re-save"
            );
        }

        let named = |extension: &str| -> Vec<(String, Vec<u8>)> {
            parts
                .iter()
                .filter(|(name, _)| name.ends_with(extension))
                .cloned()
                .collect()
        };

        let svg_parts = named(".svg");
        assert_eq!(
            svg_parts.len(),
            1,
            "{file}: the re-saved package holds {} SVG parts: {:#?}",
            svg_parts.len(),
            parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        let (svg_name, svg_bytes) = &svg_parts[0];
        assert_eq!(
            svg_bytes.as_slice(),
            svg.as_slice(),
            "{file}: {svg_name} is not the drawing the fixture carries"
        );
        assert!(
            declared_as(&parts, svg_name, "image/svg"),
            "{file}: {svg_name} is not declared as an SVG part"
        );
        assert!(
            reached_by_a_relationship(&parts, svg_name),
            "{file}: no relationship reaches {svg_name}"
        );

        let png_parts = named(".png");
        assert_eq!(
            png_parts.len(),
            1,
            "{file}: expected one raster drawn from the drawing, found {png_parts:#?}"
        );
        let (png_name, png_data) = &png_parts[0];
        let (width, height) =
            assert_rendered_from_the_diagram(&format!("{file}: {png_name}"), png_data);
        assert!(
            width >= 40 && height >= 24,
            "{file}: {png_name} ({width}x{height}) is smaller than the picture it stands for"
        );
        assert!(
            declared_as(&parts, png_name, "image/png"),
            "{file}: {png_name} is not declared as an image/png part"
        );
        assert!(
            reached_by_a_relationship(&parts, png_name),
            "{file}: no relationship reaches {png_name}"
        );

        let before = pictures_the_reader_takes(&ws, file, &bytes, "before");
        let after = pictures_the_reader_takes(&ws, file, &resaved, "after");
        assert_eq!(
            before.len(),
            1,
            "{file}: the fixture is not the single drawing this expects: {before:#?}"
        );
        assert_eq!(
            after.len(),
            1,
            "{file}: the re-saved package gives {} picture(s): {after:#?}",
            after.len()
        );
        assert_eq!(
            before[0].size,
            (144.0, 86.0),
            "{file}: the fixture is not the picture this expects"
        );
        assert_the_reader_agrees(file, &before, &after);
    }
}

/// LibreOffice keeps an oriented picture's pixels and its EXIF tag, so the
/// picture this reader turns upright is still the picture the application shows.
///
/// The fixture embeds `photos/06_rotated.jpg`, whose stored pixels are 90°
/// clockwise from the page and whose orientation tag says to turn them back.
/// Both office applications display it upright without rewriting the pixels;
/// the question a re-save raises is whether the application preserves that tag
/// or turns the bytes itself. It opens each fixture and writes it back, and what
/// it writes is held to
///
/// * the picture: one media part with the fixture's own bytes, under the `.jpeg`
///   name LibreOffice gives it (`image1.jpg` comes back as `image1.jpeg`), so
///   the tag and the stored pixels are both still there — a re-encode that
///   rotated the pixels and dropped the tag would fail here;
/// * that part: declared `image/jpeg` and reached by a relationship, so it is a
///   picture the document holds rather than a file left beside it; and
/// * this reader, on what the application wrote: one picture at the 1240 × 1754
///   upright frame the fixture gives, with the same pixels.
#[test]
fn libreoffice_opens_and_resaves_the_oriented_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the oriented-picture re-save test");
        return;
    };
    let jpeg = std::fs::read(repository_root().join("test-data/photos/06_rotated.jpg"))
        .expect("the oriented JPEG fixture");

    for file in ["oriented_picture.docx", "oriented_picture.pptx"] {
        let bytes = fixture(file);
        let ws = Workspace::new(&format!("oriented-resave-{file}"));
        let before = pictures_the_reader_takes(&ws, file, &bytes, "before");
        assert_eq!(
            before.len(),
            1,
            "{file}: the fixture is not the single oriented picture this expects: {before:#?}"
        );
        assert_eq!(
            before[0].size,
            (1240.0, 1754.0),
            "{file}: the fixture's picture is not upright"
        );

        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, file, &bytes);
        let kept: Vec<&String> = parts
            .iter()
            .filter(|(_, data)| data.as_slice() == jpeg.as_slice())
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            kept.len(),
            1,
            "{file}: the JPEG picture came back {kept:?} times; the parts are {:#?}",
            parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        let name = kept[0];
        assert!(
            declared_as(&parts, name, "image/jpeg"),
            "{file}: {name} is not declared as an image/jpeg part"
        );
        assert!(
            reached_by_a_relationship(&parts, name),
            "{file}: no relationship reaches {name}"
        );

        let after = pictures_the_reader_takes(&ws, file, &resaved, "after");
        assert_the_reader_agrees(file, &before, &after);
    }
}

/// LibreOffice accepts the `picture_bullets` fixture packages, where a list
/// item's marker is a picture rather than a character: `w:numPicBullet` named by
/// a level in Word, `a:buBlip` on a paragraph in PowerPoint. Each fixture holds
/// two such bullets — one the drawing behind a grey raster fallback, one a plain
/// raster — so the re-save asks whether an application keeps a picture that is
/// not a picture of its own but the ink at the start of a line.
///
/// It opens each fixture and writes it back. The two formats answer differently:
///
/// * the DOCX keeps the painted bullet: one `w:numPicBullet`, now a VML
///   `v:imagedata`, at its 24 × 16 raster and red pixels, declared `image/png`
///   and reached by a relationship. The drawn bullet is gone — the numbering part
///   still has a bullet level, but no picture for it and no `svgBlip` — which is
///   stated rather than smoothed over; this reader takes the one picture that is
///   left, and it is the fixture's painted raster;
/// * the PPTX keeps both: two `a:buBlip`s and two PNG parts, the drawn one
///   rasterised by the application at 41 × 25 with the diagram's panel and stroke
///   on it, and the painted one at its 24 × 16 red pixels, each declared and
///   reached. This reader takes both, the painted one unchanged and the drawn one
///   as the application drew it.
#[test]
fn libreoffice_opens_and_resaves_the_picture_bullet_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the picture-bullet re-save test");
        return;
    };

    for file in ["picture_bullets.docx", "picture_bullets.pptx"] {
        let bytes = fixture(file);
        let ws = Workspace::new(&format!("picture-bullets-resave-{file}"));
        let before = pictures_the_reader_takes(&ws, file, &bytes, "before");
        assert_eq!(
            before.len(),
            2,
            "{file}: the fixture is not the two picture bullets this expects: {before:#?}"
        );
        assert_eq!(
            before[0].size,
            if file.ends_with(".docx") {
                (36.0, 36.0)
            } else {
                (48.0, 48.0)
            },
            "{file}: the fixture's drawn bullet is not the size its format states"
        );
        assert_eq!(
            before[1].size,
            (24.0, 16.0),
            "{file}: the fixture's painted bullet is not its raster's grid"
        );

        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, file, &bytes);
        let body = parts
            .iter()
            .find(|(name, _)| name.ends_with("document.xml") || name.ends_with("slide1.xml"))
            .map(|(_, data)| String::from_utf8_lossy(data).to_string())
            .unwrap_or_else(|| panic!("{file}: no document body in the re-saved package"));
        for item in ["Drawn bullet item", "Painted bullet item"] {
            assert!(
                body.contains(item),
                "{file}: {item} did not survive the re-save"
            );
        }

        if file.ends_with(".docx") {
            let numbering = parts
                .iter()
                .find(|(name, _)| name == "word/numbering.xml")
                .map(|(_, data)| String::from_utf8_lossy(data).to_string())
                .unwrap_or_else(|| panic!("{file}: no numbering part in the re-saved package"));
            assert_eq!(
                numbering.matches("<w:numPicBullet ").count(),
                1,
                "{file}: expected the one picture bullet the application kept"
            );
            assert_eq!(
                numbering.matches("<w:lvlPicBulletId").count(),
                1,
                "{file}: the kept picture bullet is not named by a list level"
            );
            assert!(
                numbering.contains("<v:imagedata"),
                "{file}: the kept bullet does not name its raster"
            );
            assert!(
                !numbering.contains("<asvg:svgBlip"),
                "{file}: the application kept the drawn bullet; this note is out of date"
            );

            let rasters: Vec<(&String, image::RgbaImage)> = parts
                .iter()
                .filter(|(name, _)| name.ends_with(".png"))
                .map(|(name, data)| {
                    let drawn = image::load_from_memory(data)
                        .unwrap_or_else(|e| panic!("{file}: {name} does not decode: {e}"))
                        .to_rgba8();
                    (name, drawn)
                })
                .collect();
            assert_eq!(
                rasters.len(),
                1,
                "{file}: expected the painted bullet's one raster, found {:?}",
                rasters.iter().map(|(name, _)| name).collect::<Vec<_>>()
            );
            let (name, raster) = &rasters[0];
            assert_eq!(
                raster.dimensions(),
                (24, 16),
                "{file}: {name} is not its grid"
            );
            assert_eq!(
                raster.get_pixel(12, 8).0,
                [192, 32, 32, 255],
                "{file}: {name} is not the painted bullet's red block"
            );
            assert!(
                declared_as(&parts, name, "image/png"),
                "{file}: {name} is not declared as an image/png part"
            );
            assert!(
                reached_by_a_relationship(&parts, name),
                "{file}: no relationship reaches {name}"
            );

            let after = pictures_the_reader_takes(&ws, file, &resaved, "after");
            assert_eq!(
                after.len(),
                1,
                "{file}: the re-saved package gives {} picture(s): {after:#?}",
                after.len()
            );
            assert!(
                the_same_picture(&before[1], &after[0]),
                "{file}: the picture the application kept is not the fixture's painted bullet:\n\
                 re-saved: {:#?}\nfixture: {:#?}",
                after[0],
                before[1]
            );
        } else {
            assert_eq!(
                body.matches("<a:buBlip>").count(),
                2,
                "{file}: the application did not keep both picture bullets"
            );

            let mut drawn = 0;
            let mut painted = 0;
            for (name, data) in parts.iter().filter(|(name, _)| name.ends_with(".png")) {
                let raster = image::load_from_memory(data)
                    .unwrap_or_else(|e| panic!("{file}: {name} does not decode: {e}"))
                    .to_rgba8();
                if raster.dimensions() == (24, 16) {
                    assert_eq!(
                        raster.get_pixel(12, 8).0,
                        [192, 32, 32, 255],
                        "{file}: {name} is not the painted bullet's red block"
                    );
                    painted += 1;
                } else {
                    assert_eq!(
                        raster.dimensions(),
                        (41, 25),
                        "{file}: {name} is not the application's drawing of the bullet"
                    );
                    assert_rendered_from_the_diagram(&format!("{file}: {name}"), data);
                    drawn += 1;
                }
                assert!(
                    declared_as(&parts, name, "image/png"),
                    "{file}: {name} is not declared as an image/png part"
                );
                assert!(
                    reached_by_a_relationship(&parts, name),
                    "{file}: no relationship reaches {name}"
                );
            }
            assert_eq!(
                (drawn, painted),
                (1, 1),
                "{file}: expected one drawn and one painted bullet, found {drawn} and {painted}"
            );

            let after = pictures_the_reader_takes(&ws, file, &resaved, "after");
            assert_eq!(
                after.len(),
                2,
                "{file}: the re-saved package gives {} picture(s): {after:#?}",
                after.len()
            );
            assert_eq!(
                after[0].size,
                (41.0, 25.0),
                "{file}: the drawn bullet is not the raster the application wrote"
            );
            assert_rendered_from_the_diagram(
                &format!("{file}: re-saved drawn bullet"),
                &after[0].bytes,
            );
            assert!(
                the_same_picture(&before[1], &after[1]),
                "{file}: the painted bullet is not the fixture's:\n\
                 re-saved: {:#?}\nfixture: {:#?}",
                after[1],
                before[1]
            );
        }
    }
}

/// LibreOffice accepts the PowerPoint half of `blip_fill_shapes`, where a shape
/// is filled with a picture rather than a colour: the raster on the shape's
/// `a:blip`, and the drawing in an `asvg:svgBlip` extension beside it. The DOCX
/// half is not in this test because LibreOffice 24.2 refuses to load it (its log
/// says `source file could not be loaded` and writes no re-saved package), so the
/// PPTX is the one that can answer whether an office application keeps a picture
/// that is part of a shape rather than a picture of its own.
///
/// It opens the deck and writes it back, and what it writes is held to
///
/// * the sentences around the filled shapes: still there, so the deck was read
///   rather than refused;
/// * the drawing: still a part of its own, byte for byte the fixture's, declared
///   an `image/svg…` part with a relationship reaching it;
/// * the raster the application drew from that markup beside the drawing, with
///   the diagram's pale panel and stroke on it, declared `image/png` and reached
///   by a relationship;
/// * the orange 60 × 36 raster that fills the second shape, byte for byte the
///   fixture's, declared and reached; and
/// * this reader, on what the application wrote: two pictures, one painted from
///   the drawing and one from the orange raster. The application adds explicit
///   line and shadow properties to the shapes on export, so this reader paints
///   them into canvases about 13 pixels larger than the fixture's (301 × 185 and
///   133 × 85 rather than 288 × 173 and 120 × 72); the fills themselves are what
///   the re-save asks about, and both are still there.
#[test]
fn libreoffice_opens_and_resaves_the_blip_fill_fixture_package() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the picture-fill re-save test");
        return;
    };
    let svg = std::fs::read(repository_root().join("test-data/images/diagram.svg"))
        .expect("the SVG fixture");
    let file = "blip_fill_shapes.pptx";
    let bytes = fixture(file);
    let ws = Workspace::new("blip-fill-resave");

    // The orange fill is the fixture's own part; read it from the package so the
    // application's copy can be held to the bytes that went in.
    let original = package_parts(&bytes, file);
    let orange = original
        .iter()
        .find(|(name, data)| {
            name.ends_with(".png")
                && image::load_from_memory(data)
                    .map(|drawn| drawn.to_rgba8().dimensions() == (60, 36))
                    .unwrap_or(false)
        })
        .map(|(_, data)| data.clone())
        .unwrap_or_else(|| panic!("{file}: no 60 x 36 raster to fill the second shape"));

    let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, file, &bytes);
    let body = parts
        .iter()
        .find(|(name, _)| name == "ppt/slides/slide1.xml")
        .map(|(_, data)| String::from_utf8_lossy(data).to_string())
        .unwrap_or_else(|| panic!("{file}: no slide body in the re-saved package"));
    for sentence in [
        "Before the filled shapes.",
        "Inside the fill.",
        "After the filled shapes.",
    ] {
        assert!(
            body.contains(sentence),
            "{file}: {sentence} did not survive the re-save"
        );
    }

    let svg_parts: Vec<&String> = parts
        .iter()
        .filter(|(name, _)| name.ends_with(".svg"))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        svg_parts.len(),
        1,
        "{file}: the re-saved package holds {} SVG parts: {:#?}",
        svg_parts.len(),
        parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    let svg_name = svg_parts[0];
    let svg_bytes = &parts
        .iter()
        .find(|(name, _)| name == svg_name)
        .expect("the SVG part")
        .1;
    assert_eq!(
        svg_bytes.as_slice(),
        svg.as_slice(),
        "{file}: {svg_name} is not the drawing the fixture carries"
    );
    assert!(
        declared_as(&parts, svg_name, "image/svg"),
        "{file}: {svg_name} is not declared as an SVG part"
    );
    assert!(
        reached_by_a_relationship(&parts, svg_name),
        "{file}: no relationship reaches {svg_name}"
    );

    let orange_parts: Vec<&String> = parts
        .iter()
        .filter(|(_, data)| data.as_slice() == orange.as_slice())
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        orange_parts.len(),
        1,
        "{file}: the orange fill came back {orange_parts:?} times; the parts are {:#?}",
        parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    let orange_name = orange_parts[0];
    assert!(
        declared_as(&parts, orange_name, "image/png"),
        "{file}: {orange_name} is not declared as an image/png part"
    );
    assert!(
        reached_by_a_relationship(&parts, orange_name),
        "{file}: no relationship reaches {orange_name}"
    );

    let mut fallbacks = 0;
    for (name, data) in parts.iter().filter(|(name, _)| name.ends_with(".png")) {
        if data.as_slice() == orange.as_slice() {
            continue;
        }
        assert_rendered_from_the_diagram(&format!("{file}: {name}"), data);
        assert!(
            declared_as(&parts, name, "image/png"),
            "{file}: {name} is not declared as an image/png part"
        );
        assert!(
            reached_by_a_relationship(&parts, name),
            "{file}: no relationship reaches {name}"
        );
        fallbacks += 1;
    }
    assert_eq!(
        fallbacks, 1,
        "{file}: expected one raster drawn from the SVG, found {fallbacks}"
    );

    let before = pictures_the_reader_takes(&ws, file, &bytes, "before");
    let after = pictures_the_reader_takes(&ws, file, &resaved, "after");
    assert_eq!(
        before.len(),
        2,
        "{file}: the fixture is not the two filled shapes this expects: {before:#?}"
    );
    assert_eq!(
        after.len(),
        2,
        "{file}: the re-saved package gives {} picture(s): {after:#?}",
        after.len()
    );
    assert_rendered_from_the_diagram(&format!("{file}: re-saved drawn fill"), &after[0].bytes);
    let painted = after[1].decoded().expect("the painted fill is a raster");
    assert_eq!(
        painted
            .get_pixel(painted.width() / 2, painted.height() / 2)
            .0,
        [224, 140, 40, 255],
        "{file}: the second shape is not filled with the orange raster"
    );
}

/// LibreOffice accepts the `worksheet_pictures` workbook, where each sheet's
/// drawing names its own media. The workbook is the fixture of a second package
/// format, and of six encodings in one package: a BMP, a GIF, a PNG, a JPEG, a
/// bare JPEG 2000 codestream and a JP2 container, with the BMP drawn on a second
/// sheet as well.
///
/// It opens the workbook and writes it back, and what it writes is held to
///
/// * each picture: still a media part with the fixture's own bytes — the BMP and
///   GIF carried through, the PNG and JPEG likewise — so the application read
///   the four encodings it has filters for rather than dropping the ones it
///   cannot draw;
/// * the fifth and sixth: neither JPEG 2000 spelling is anywhere in the re-saved
///   package, which is the application's own limit rather than a failure of the
///   workbook — LibreOffice has no JPEG 2000 import filter, so a codestream and
///   a container are both dropped whole;
/// * each part declared its `image/…` type and reached by a relationship, so the
///   sheets hold them as pictures rather than leaving files in the package;
/// * both sheets' drawings: `drawing1.xml` and `drawing2.xml` are still there, so
///   the second sheet's picture was not lost with the first one; and
/// * this reader, on what the application wrote: the five pictures it kept, at
///   the same sizes and with the same pixels as the fixture's, in the fixture's
///   order.
#[test]
fn libreoffice_opens_and_resaves_the_worksheet_picture_fixture() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the worksheet-picture re-save test");
        return;
    };
    let file = "worksheet_pictures.xlsx";
    let bytes = fixture(file);
    let ws = Workspace::new("worksheet-pictures-resave");
    let source = [
        (fixture("images/chart.bmp"), "image/bmp"),
        (fixture("images/chart.gif"), "image/gif"),
        (fixture("invoice_scan.png"), "image/png"),
        (fixture("images/scan.jpg"), "image/jpeg"),
    ];

    let before = pictures_the_reader_takes(&ws, file, &bytes, "before");
    assert_eq!(
        before.len(),
        7,
        "{file}: the fixture is not the seven pictures this expects: {before:#?}"
    );

    let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, file, &bytes);
    let jpeg2000 = [fixture("images/scan.jpc"), fixture("images/scan.jp2")];
    for part in &jpeg2000 {
        assert!(
            parts
                .iter()
                .all(|(_, data)| data.as_slice() != part.as_slice()),
            "{file}: a JPEG 2000 part survived, so this note is out of date"
        );
    }
    for (data, mime) in &source {
        let kept: Vec<&String> = parts
            .iter()
            .filter(|(_, part)| part.as_slice() == data.as_slice())
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            kept.len(),
            1,
            "{file}: the {mime} picture came back {kept:?} times; the parts are {:#?}",
            parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        let name = kept[0];
        assert!(
            declared_as(&parts, name, mime),
            "{file}: {name} is not declared as {mime}"
        );
        assert!(
            reached_by_a_relationship(&parts, name),
            "{file}: no relationship reaches {name}"
        );
    }

    let media = parts
        .iter()
        .filter(|(name, _)| name.starts_with("xl/media/"))
        .count();
    assert_eq!(
        media,
        4,
        "{file}: expected the four media parts, found {media}: {:#?}",
        parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    for drawing in ["xl/drawings/drawing1.xml", "xl/drawings/drawing2.xml"] {
        assert!(
            parts.iter().any(|(name, _)| name == drawing),
            "{file}: the re-saved workbook has no {drawing}"
        );
    }

    // The fifth and sixth pictures of the fixture's reading are the two JPEG
    // 2000s, identified by what the reader made of them rather than by their
    // place in the list: the file written for each is the pixels its part
    // decodes to, at its own grid.
    let dropped = [4usize, 5];
    for (index, part) in dropped.iter().zip(&jpeg2000) {
        let decoded =
            pdfextractor_image_parser::transcode_to_png(part).expect("the JPEG 2000 decodes");
        let as_read = ReadPicture {
            mimetype: "image/png".to_string(),
            size: (decoded.width as f64, decoded.height as f64),
            bytes: decoded.data,
        };
        assert!(
            the_same_picture(&before[*index], &as_read),
            "{file}: picture {index} is not the JPEG 2000 this test drops: {:#?}",
            before[*index]
        );
    }

    // The re-saved workbook is the fixture's reading two pictures short — the
    // two the application has no filter for — and the five it kept are the
    // fixture's own, at the same sizes and with the same pixels.
    let kept: Vec<ReadPicture> = before
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !dropped.contains(index))
        .map(|(_, picture)| picture)
        .collect();
    let after = pictures_the_reader_takes(&ws, file, &resaved, "after");
    assert_the_reader_agrees(file, &kept, &after);
}

/// LibreOffice keeps `headerPic.docx`'s only picture, which lives in the
/// document's header rather than in its body — and writes it into a second
/// header as well. The real-world fixture is the one place a picture is reached
/// through a separate header part instead of the document part, so the re-save
/// asks whether an application keeps that part and its relationship.
///
/// It opens the document and writes it back, and what it writes is held to
///
/// * the picture: one media part with the fixture's own JPEG bytes, declared
///   `image/jpeg` and reached by a relationship from a header part — the body
///   never names it;
/// * the header parts: more than one, which is the application writing the
///   header it read beside another; and
/// * this reader, on what the application wrote: two pictures at the fixture's
///   80 × 80 frame and with its pixels, the original and the copy. The
///   duplication is stated rather than smoothed over: an application that writes
///   only the one header would fail here, which is the moment to say the note is
///   out of date.
#[test]
fn libreoffice_opens_and_resaves_the_header_picture_fixture() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the header-picture re-save test");
        return;
    };
    let file = "headerPic.docx";
    let bytes = std::fs::read(repository_root().join("test-data/real/headerPic.docx"))
        .expect("the header-picture fixture");
    let ws = Workspace::new("header-picture-resave");

    let original = package_parts(&bytes, file);
    let source = original
        .iter()
        .find(|(name, _)| name.starts_with("word/media/"))
        .map(|(_, data)| data.clone())
        .unwrap_or_else(|| panic!("{file}: the fixture carries no media part"));
    let before = pictures_the_reader_takes(&ws, file, &bytes, "before");
    assert_eq!(
        before.len(),
        1,
        "{file}: the fixture is not the single header picture this expects: {before:#?}"
    );
    assert_eq!(
        before[0].size,
        (80.0, 80.0),
        "{file}: the header picture is not its fixture's frame"
    );

    let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, file, &bytes);
    let kept: Vec<&String> = parts
        .iter()
        .filter(|(_, data)| data.as_slice() == source.as_slice())
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        kept.len(),
        1,
        "{file}: the header picture came back {kept:?} times; the parts are {:#?}",
        parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    let name = kept[0];
    assert!(
        declared_as(&parts, name, "image/jpeg"),
        "{file}: {name} is not declared as an image/jpeg part"
    );
    assert!(
        reached_by_a_relationship(&parts, name),
        "{file}: no relationship reaches {name}"
    );
    let headers = parts
        .iter()
        .filter(|(name, _)| name.starts_with("word/header") && name.ends_with(".xml"))
        .count();
    assert!(
        headers > 1,
        "{file}: the application wrote {headers} header part(s), not the copy this test states"
    );

    let after = pictures_the_reader_takes(&ws, file, &resaved, "after");
    assert_eq!(
        after.len(),
        2,
        "{file}: the re-saved package gives {} picture(s), not the original and its copy: {after:#?}",
        after.len()
    );
    assert!(
        the_same_picture(&before[0], &after[0]),
        "{file}: the header picture is not the fixture's:\n\
         re-saved: {:#?}\nfixture: {:#?}",
        after[0],
        before[0]
    );
    assert!(
        the_same_picture(&before[0], &after[1]),
        "{file}: the duplicated header picture is not the fixture's:\n\
         re-saved: {:#?}\nfixture: {:#?}",
        after[1],
        before[0]
    );
}

/// LibreOffice accepts `background_deck.pptx`, whose first slide paints a
/// photograph as its background rather than placing it as a shape: the picture is
/// `p:cSld/p:bg/p:bgPr/a:blipFill`, outside the shape tree, and the second slide
/// states no `p:bg` at all. A backdrop has no frame of its own, so what a re-save
/// asks is whether the picture survives where the file put it.
///
/// It opens the fixture and writes it back, and what it writes is held to
///
/// * the sentences: each still in its own slide, so the deck was read rather than
///   refused;
/// * the backdrop: still the first slide's one `p:bg`, holding an `a:blipFill` that
///   names a picture, and still nothing on the second slide. An application that
///   dropped the fill, or promoted it to a full-slide `p:pic`, fails here;
/// * the photograph: one `image/jpeg` part, declared and reached by a
///   relationship, byte for byte the fixture's `images/scan.jpg` — the application
///   copies a photograph through rather than re-encoding it; and
/// * this reader, on what the application wrote: the same pages, the first backed
///   by the same 620 × 350 `image/jpeg` at the same 62 dpi and the second by
///   nothing, no picture reported as an item, and the same words as the fixture
///   gave.
#[test]
fn libreoffice_opens_and_resaves_the_picture_background_deck() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the picture-background re-save test");
        return;
    };
    let file = "background_deck.pptx";
    let bytes = fixture(file);
    let ws = Workspace::new("background-resave");
    let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, file, &bytes);

    let slide = |name: &str| -> String {
        parts
            .iter()
            .find(|(part, _)| part == name)
            .map(|(_, data)| String::from_utf8_lossy(data).to_string())
            .unwrap_or_else(|| panic!("{file}: no {name} in the re-saved package"))
    };
    let first = slide("ppt/slides/slide1.xml");
    let second = slide("ppt/slides/slide2.xml");
    assert!(
        first.contains("A photograph behind the words."),
        "{file}: the first slide's sentence did not survive the re-save"
    );
    assert!(
        second.contains("No picture behind these words."),
        "{file}: the second slide's sentence did not survive the re-save"
    );

    assert_eq!(
        first.matches("<p:bg>").count(),
        1,
        "{file}: the first slide does not state one background"
    );
    let background = first
        .split_once("<p:bg>")
        .and_then(|(_, rest)| rest.split_once("</p:bg>"))
        .map(|(background, _)| background)
        .unwrap_or_else(|| panic!("{file}: the background is never closed"));
    assert!(
        background.contains("<a:blipFill") && background.contains("<a:blip "),
        "{file}: the background the application kept names no picture: {background}"
    );
    assert!(
        !first.contains("<p:pic>"),
        "{file}: the backdrop became a picture shape rather than a background"
    );
    assert!(
        !second.contains("<p:bg>"),
        "{file}: the second slide was given a background it never stated"
    );

    let photograph = fixture("images/scan.jpg");
    let photographs: Vec<&String> = parts
        .iter()
        .filter(|(name, _)| name.ends_with(".jpg") || name.ends_with(".jpeg"))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        photographs.len(),
        1,
        "{file}: the fixture's one photograph came back as {photographs:?}; the parts are {:#?}",
        parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    let name = photographs[0];
    assert_eq!(
        parts
            .iter()
            .find(|(part, _)| part == name)
            .map(|(_, data)| data),
        Some(&photograph),
        "{file}: {name} is not the photograph the fixture carries"
    );
    assert!(
        declared_as(&parts, name, "image/jpeg"),
        "{file}: {name} is not declared as an image/jpeg part"
    );
    assert!(
        reached_by_a_relationship(&parts, name),
        "{file}: no relationship reaches {name}"
    );

    let before = backdrops_the_reader_takes(&ws, file, &bytes, "before");
    let after = backdrops_the_reader_takes(&ws, file, &resaved, "after");
    assert_eq!(
        after.pages.len(),
        2,
        "{file}: the re-saved deck is not two slides: {after:#?}"
    );
    let backdrop = after.pages[0]
        .as_ref()
        .unwrap_or_else(|| panic!("{file}: the first slide lost its background: {after:#?}"));
    assert_eq!(
        backdrop.mimetype, "image/jpeg",
        "{file}: the backdrop is not the photograph"
    );
    assert_eq!(
        backdrop.size,
        (620.0, 350.0),
        "{file}: the backdrop is not the 620 × 350 scan"
    );
    assert_eq!(
        backdrop.dpi, 62,
        "{file}: 620 pixels across 720 pt is 62 dpi"
    );
    assert_eq!(
        backdrop.bytes, photograph,
        "{file}: the page image is not the photograph's own bytes"
    );
    assert_eq!(
        after.pages[1], None,
        "{file}: the second slide gained a backdrop"
    );
    assert_eq!(
        after.pictures, 0,
        "{file}: the backdrop was reported as a picture item"
    );
    assert_eq!(
        after, before,
        "{file}: the re-saved deck is not the fixture's reading"
    );
}

/// LibreOffice accepts `master_background_deck.pptx`, whose photograph is the
/// slide *master's* background rather than any slide's: none of the three slides
/// states a picture background, and the first two state no `p:bg` at all. A
/// deck's photographic backdrop is usually kept this way — one picture behind
/// every slide — so what a re-save asks is whether the application keeps it
/// where the deck put it, rather than copying it onto each slide.
///
/// It opens the fixture and writes it back, and what it writes is held to
///
/// * the sentences: each still in its own slide, so the deck was read rather than
///   refused;
/// * the inheritance: neither of the first two slides states a `p:bg` of its own,
///   so their backdrop is still the master's rather than a copy flattened onto
///   them. The third slide states a colour of its own — a `p:bg` with an
///   `a:solidFill` and no `a:blipFill` — so it keeps saying what is behind it
///   instead of being given the photograph as well;
/// * the master: it still states the photograph as its own `p:bg`, holding an
///   `a:blipFill` that names a picture, and still draws no `p:pic` for it. Every
///   background a master states is asked the same question, and at least one
///   must be there — the application's export writes more than one master, and
///   which of them a slide's layout points at is its business, not this test's;
/// * the photograph: one `image/jpeg` part, declared and reached by a
///   relationship, byte for byte the fixture's `images/scan.jpg` — the
///   application copies a photograph through rather than re-encoding it; and
/// * this reader, on what the application wrote: the same three pages, the first
///   two backed by the same 620 × 350 `image/jpeg` at 62 dpi and the third by
///   nothing, no picture reported as an item, and the same words as the fixture
///   gave. That half is the test of the inheritance itself: the re-saved deck
///   names the photograph in the master only, so a reader that looked at the
///   slides alone would come back with three bare pages.
#[test]
fn libreoffice_opens_and_resaves_the_master_picture_background_deck() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the master picture-background re-save test");
        return;
    };
    let file = "master_background_deck.pptx";
    let bytes = fixture(file);
    let ws = Workspace::new("master-background-resave");
    let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, file, &bytes);

    let part = |name: &str| -> String {
        parts
            .iter()
            .find(|(part, _)| part == name)
            .map(|(_, data)| String::from_utf8_lossy(data).to_string())
            .unwrap_or_else(|| panic!("{file}: no {name} in the re-saved package"))
    };
    let background = |xml: &str| -> String {
        xml.split_once("<p:bg>")
            .and_then(|(_, rest)| rest.split_once("</p:bg>"))
            .map(|(background, _)| background.to_string())
            .unwrap_or_else(|| panic!("{file}: a background is never closed: {xml}"))
    };
    let first = part("ppt/slides/slide1.xml");
    let second = part("ppt/slides/slide2.xml");
    let third = part("ppt/slides/slide3.xml");
    assert!(
        first.contains("A photograph behind these words."),
        "{file}: the first slide's sentence did not survive the re-save"
    );
    assert!(
        second.contains("The same photograph behind these words."),
        "{file}: the second slide's sentence did not survive the re-save"
    );
    assert!(
        third.contains("A colour behind these words."),
        "{file}: the third slide's sentence did not survive the re-save"
    );
    for (name, xml) in [("slide1.xml", &first), ("slide2.xml", &second)] {
        assert!(
            !xml.contains("<p:bg>"),
            "{file}: {name} was given the master's background as its own"
        );
        assert!(
            !xml.contains("<p:pic>"),
            "{file}: {name} draws the backdrop as a picture shape"
        );
    }
    assert_eq!(
        third.matches("<p:bg>").count(),
        1,
        "{file}: the third slide does not state one background"
    );
    let colour = background(&third);
    assert!(
        colour.contains("<a:solidFill") && !colour.contains("<a:blipFill"),
        "{file}: the third slide's own colour became the master's photograph: {colour}"
    );

    let masters: Vec<(String, String)> = parts
        .iter()
        .filter(|(name, _)| {
            name.starts_with("ppt/slideMasters/slideMaster") && name.ends_with(".xml")
        })
        .map(|(name, data)| (name.clone(), String::from_utf8_lossy(data).to_string()))
        .collect();
    assert!(
        !masters.is_empty(),
        "{file}: the re-saved package has no slide master"
    );
    let mut stated = 0;
    for (name, xml) in &masters {
        assert!(
            !xml.contains("<p:pic>"),
            "{file}: {name} draws the backdrop as a picture shape"
        );
        for rest in xml.split("<p:bg>").skip(1) {
            let kept = rest.split("</p:bg>").next().unwrap_or_default();
            stated += 1;
            assert!(
                kept.contains("<a:blipFill") && kept.contains("<a:blip "),
                "{file}: the background {name} kept names no picture: {kept}"
            );
        }
    }
    assert!(
        stated > 0,
        "{file}: no master states the photograph's background"
    );

    let photograph = fixture("images/scan.jpg");
    let photographs: Vec<&String> = parts
        .iter()
        .filter(|(name, _)| name.ends_with(".jpg") || name.ends_with(".jpeg"))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        photographs.len(),
        1,
        "{file}: the fixture's one photograph came back as {photographs:?}; the parts are {:#?}",
        parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    let name = photographs[0];
    assert_eq!(
        parts
            .iter()
            .find(|(part, _)| part == name)
            .map(|(_, data)| data),
        Some(&photograph),
        "{file}: {name} is not the photograph the fixture carries"
    );
    assert!(
        declared_as(&parts, name, "image/jpeg"),
        "{file}: {name} is not declared as an image/jpeg part"
    );
    assert!(
        reached_by_a_relationship(&parts, name),
        "{file}: no relationship reaches {name}"
    );

    let before = backdrops_the_reader_takes(&ws, file, &bytes, "before");
    let after = backdrops_the_reader_takes(&ws, file, &resaved, "after");
    assert_eq!(
        after.pages.len(),
        3,
        "{file}: the re-saved deck is not three slides: {after:#?}"
    );
    for (index, page) in after.pages.iter().take(2).enumerate() {
        let backdrop = page.as_ref().unwrap_or_else(|| {
            panic!(
                "{file}: slide {} lost the master's background: {after:#?}",
                index + 1
            )
        });
        assert_eq!(
            backdrop.mimetype, "image/jpeg",
            "{file}: the backdrop is not the photograph"
        );
        assert_eq!(
            backdrop.size,
            (620.0, 350.0),
            "{file}: the backdrop is not the 620 × 350 scan"
        );
        assert_eq!(
            backdrop.dpi, 62,
            "{file}: 620 pixels across 720 pt is 62 dpi"
        );
        assert_eq!(
            backdrop.bytes, photograph,
            "{file}: the page image is not the photograph's own bytes"
        );
    }
    assert_eq!(
        after.pages[2], None,
        "{file}: the third slide's colour became a picture"
    );
    assert_eq!(
        after.pictures, 0,
        "{file}: the backdrop was reported as a picture item"
    );
    assert_eq!(
        after, before,
        "{file}: the re-saved deck is not the fixture's reading"
    );
}

/// LibreOffice accepts the BMP and TIFF fixture packages as packages that carry
/// pictures, not just files this project happens to read.
///
/// The generator places a PNG and renames its part to the raster's own bytes, so
/// whether the package states the format properly is a question only an office
/// application can answer. It opens each fixture and writes it back, and what it
/// writes is held to the pictures:
///
/// * each drawing is still a media part with the fixture's own bytes — one BMP,
///   one TIFF (`image2.tiff` comes back as `image2.tif`: the name is the
///   application's, the bytes are the picture's);
/// * a relationship still reaches each part, so the document holds it as a
///   picture rather than leaving a file in the package;
/// * the package still declares each part with its `image/…` type; and
/// * this reader, handed the re-saved package, takes the same two PNG pictures
///   from it, at the grids their headers state, with the same pixels.
///
/// A part dropped, or a BMP re-drawn as a PNG, fails here: the fixture would
/// then be a file only this reader accepts.
#[test]
fn libreoffice_opens_and_resaves_the_bmp_and_tiff_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the BMP/TIFF re-save test");
        return;
    };
    let bmp = std::fs::read(repository_root().join("test-data/images/chart.bmp"))
        .expect("the BMP fixture");
    let tiff = std::fs::read(repository_root().join("test-data/images/chart.tiff"))
        .expect("the TIFF fixture");

    for fixture in ["bmp_tiff_pictures.docx", "bmp_tiff_pictures.pptx"] {
        let bytes = std::fs::read(repository_root().join("test-data").join(fixture))
            .expect("the office fixture");
        let ws = Workspace::new(fixture);
        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, fixture, &bytes);

        for (source, mime) in [
            (bmp.as_slice(), "image/bmp"),
            (tiff.as_slice(), "image/tiff"),
        ] {
            let kept: Vec<&String> = parts
                .iter()
                .filter(|(_, data)| data.as_slice() == source)
                .map(|(name, _)| name)
                .collect();
            assert_eq!(
                kept.len(),
                1,
                "{fixture}: the {mime} picture came back {kept:?} times; the parts are {:#?}",
                parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
            );
            let name = kept[0];
            assert!(
                reached_by_a_relationship(&parts, name),
                "{fixture}: no relationship reaches {name}"
            );
            assert!(
                declared_as(&parts, name, mime),
                "{fixture}: {name} is not declared as {mime}"
            );
        }

        // The re-saved package goes back through this reader, which has to find
        // the same two pictures in it: both encodings transcoded to PNG at the
        // grids their headers state.
        let before = pictures_the_reader_takes(&ws, fixture, &bytes, "before");
        let after = pictures_the_reader_takes(&ws, fixture, &resaved, "after");
        assert_the_reader_agrees(fixture, &before, &after);
    }
}

/// LibreOffice opens the WebP and JPEG 2000 fixture packages, and draws one of
/// the encodings: the WebP, which it decodes, and neither spelling of the JPEG
/// 2000, for which it has no filter at all.
///
/// The generator places all three pictures by hand — python-docx and python-pptx
/// can place none of the encodings — so whether an office application accepts
/// the parts is a question this project's own reader cannot answer. LibreOffice
/// is asked: it opens each fixture and writes it back, and what it writes is
/// held to
/// * the document around the pictures: the sentences before and after them are
///   still there, so the package was read rather than refused;
/// * the picture it understood: one raster of the chart's own 60 × 36 grid, with
///   the three bars' colours on it, and the WebP's bytes nowhere in the package —
///   a PNG drawn from the pixels, not the WebP under a `.png` name — declared as
///   an `image/png` part with a relationship reaching it, so the document holds
///   it as a picture rather than leaving a file in the package;
/// * the encodings it did not: no part is either JPEG 2000 — the `diagram.jp2`
///   container or the `diagram.jpc` codestream — and no raster is the 40 × 24 or
///   36 × 22 grid they state. LibreOffice has no JPEG 2000 import filter — a
///   standalone JPEG 2000 does not load as a picture either — so both spellings
///   are the half of the fixture only this project's reader reads; and
/// * this reader, on the re-saved package, takes exactly what the application
///   kept: one picture, the chart at its 60 × 36 grid with the fixture's own
///   pixels (the application re-encoded the WebP, so the bytes differ and the
///   drawing does not), and no JPEG 2000 at all.
///
/// That last assertion is deliberately a limitation stated: a LibreOffice that
/// gains a JPEG 2000 filter fails here, which is the moment to say so instead.
#[test]
fn libreoffice_opens_and_resaves_the_webp_and_jp2_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the WebP/JPEG 2000 re-save test");
        return;
    };
    let webp = std::fs::read(repository_root().join("test-data/images/chart.webp"))
        .expect("the WebP fixture");
    let jp2 = std::fs::read(repository_root().join("test-data/images/diagram.jp2"))
        .expect("the JPEG 2000 fixture");
    let jpc = std::fs::read(repository_root().join("test-data/images/diagram.jpc"))
        .expect("the bare JPEG 2000 codestream fixture");

    for fixture in ["webp_jp2_pictures.docx", "webp_jp2_pictures.pptx"] {
        let bytes = std::fs::read(repository_root().join("test-data").join(fixture))
            .expect("the office fixture");
        let ws = Workspace::new(&format!("webp-jp2-resave-{fixture}"));
        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, fixture, &bytes);

        let body = parts
            .iter()
            .find(|(name, _)| name.ends_with("document.xml") || name.ends_with("slide1.xml"))
            .map(|(_, data)| String::from_utf8_lossy(data).to_string())
            .unwrap_or_else(|| panic!("{fixture}: no document body in the re-saved package"));
        for sentence in ["Before the pictures.", "After the pictures."] {
            assert!(
                body.contains(sentence),
                "{fixture}: {sentence} did not survive the re-save"
            );
        }

        // The WebP: a raster the application drew, at the picture's own grid.
        let rasters: Vec<(String, image::RgbImage)> = parts
            .iter()
            .filter(|(name, _)| name.ends_with(".png"))
            .map(|(name, data)| {
                let drawn = image::load_from_memory(data)
                    .unwrap_or_else(|e| panic!("{fixture}: {name} does not decode: {e}"))
                    .to_rgb8();
                (name.clone(), drawn)
            })
            .collect();
        assert_eq!(
            rasters.len(),
            1,
            "{fixture}: expected one raster, found {:?}",
            rasters.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        let (name, chart) = &rasters[0];
        assert_eq!(
            (chart.width(), chart.height()),
            (60, 36),
            "{fixture}: {name} is not the WebP's grid"
        );
        assert!(
            declared_as(&parts, name, "image/png"),
            "{fixture}: {name} is not declared as an image/png part"
        );
        assert!(
            reached_by_a_relationship(&parts, name),
            "{fixture}: no relationship reaches {name}"
        );
        for (bar, colour) in [
            ("blue", [47, 109, 181]),
            ("green", [109, 163, 58]),
            ("gold", [224, 178, 59]),
        ] {
            let pixels = chart.pixels().filter(|pixel| pixel.0 == colour).count();
            assert!(
                pixels >= 100,
                "{fixture}: {name} holds {pixels} {bar} pixels; the chart's bars were not drawn"
            );
        }
        assert!(
            parts.iter().all(|(_, data)| data != &webp),
            "{fixture}: the WebP's own bytes came back as a picture part"
        );

        // The JPEG 2000: both spellings are the half LibreOffice has no filter
        // for.
        for source in [&jp2, &jpc] {
            assert!(
                parts.iter().all(|(_, data)| data != source),
                "{fixture}: a JPEG 2000 part survived, so this note is out of date"
            );
        }
        assert!(
            rasters
                .iter()
                .all(|(_, drawn)| !matches!(drawn.dimensions(), (40, 24) | (36, 22))),
            "{fixture}: a raster of a JPEG 2000's grid came back, so this note is out of date"
        );

        // This reader, on the same re-saved package, takes one picture out of it
        // where the fixture gives three — the same reading the application's
        // part list just showed. The one it takes is the chart, at its own grid
        // with its own pixels: the application re-encoded the WebP as a PNG, so
        // the bytes differ while the drawing does not. The JPEG 2000s are not
        // there, because no part of them is.
        let before = pictures_the_reader_takes(&ws, fixture, &bytes, "before");
        let after = pictures_the_reader_takes(&ws, fixture, &resaved, "after");
        assert_eq!(
            before.len(),
            3,
            "{fixture}: the fixture is not the three pictures this expects: {before:#?}"
        );
        assert_eq!(
            after.len(),
            1,
            "{fixture}: the re-saved package gives {} picture(s): {after:#?}",
            after.len()
        );
        assert!(
            the_same_picture(&before[0], &after[0]),
            "{fixture}: the chart the application drew is not the fixture's:\n\
             re-saved: {:#?}\nfixture: {:#?}",
            after[0],
            before[0]
        );
        assert!(
            after
                .iter()
                .all(|picture| !matches!(picture.size, (40.0, 24.0) | (36.0, 22.0))),
            "{fixture}: the reader found a JPEG 2000 in a package that dropped it"
        );
    }
}

/// The metafile fixtures are documents another producer wrote, and the drawings
/// in them are metafiles — `image/emf` and `image/wmf`, the vector art Word
/// saved — which are played into PNGs by `crates/metafile`. Whether the packages
/// are ones an office application accepts is asked of LibreOffice, which opens
/// and re-writes each, and what it writes is held to the drawings:
///
/// * every metafile of the original is still a part of the re-saved package,
///   byte for byte — an application copies a metafile through rather than
///   re-writing it;
/// * a relationship still reaches each one, so it is a drawing the document
///   holds rather than a file left in the package;
/// * the package still declares each part as an `image/…` part; and
/// * this reader, handed the re-saved package, plays the drawings the
///   application copied through into the same pictures, at the same sizes, byte
///   for byte — the one it re-drew instead is named in the test.
///
/// The name is the application's, not the picture's: LibreOffice writes the two
/// EMFs of `VariousPictures.docx` back under `.wmf` names while keeping their
/// bytes, and declares them by that name — so the parts are matched by content,
/// and the declaration is held to the name it gave them rather than to the
/// encoding inside.
///
/// `mathtype.docx` is deliberately not here: its only drawing is a MathType OLE
/// object's preview, and LibreOffice's DOCX export drops the object whole, so a
/// re-save has nothing to say about that part. The conversion test
/// (`an_embedded_objects_preview_becomes_a_named_png`) is where it is proven.
#[test]
fn libreoffice_opens_and_resaves_the_metafile_fixture_packages() {
    let Some(soffice) = soffice_binary() else {
        eprintln!("no soffice on PATH; skipping the metafile re-save test");
        return;
    };

    let mut checked = 0;
    // How many of the fixture's pictures the application itself re-draws rather
    // than copying the part through: `drawing.docx`'s 137 × 77 logo comes back
    // at 136 × 76, its frame re-derived from the document it was placed in.
    for (fixture, rewritten) in [("VariousPictures.docx", 0usize), ("drawing.docx", 1usize)] {
        let bytes = std::fs::read(repository_root().join("test-data/real").join(fixture))
            .unwrap_or_else(|e| panic!("missing corpus file {fixture}: {e}"));
        let drawings: Vec<(String, Vec<u8>)> = package_parts(&bytes, fixture)
            .into_iter()
            .filter(|(name, _)| name.ends_with(".emf") || name.ends_with(".wmf"))
            .collect();
        assert!(
            !drawings.is_empty(),
            "{fixture}: the fixture carries no metafile, so nothing is tested"
        );

        let ws = Workspace::new(&format!("metafile-{fixture}"));
        let (resaved, parts) = resave_with_libreoffice(&soffice, &ws, fixture, &bytes);

        for (original, data) in &drawings {
            let kept: Vec<&String> = parts
                .iter()
                .filter(|(_, part)| part == data)
                .map(|(name, _)| name)
                .collect();
            assert_eq!(
                kept.len(),
                1,
                "{fixture}: {original} came back {kept:?} times; the parts are {:#?}",
                parts.iter().map(|(name, _)| name).collect::<Vec<_>>()
            );
            let name = kept[0];
            assert!(
                reached_by_a_relationship(&parts, name),
                "{fixture}: no relationship reaches {name}"
            );
            let declared = if name.ends_with(".wmf") {
                declared_as(&parts, name, "image/x-wmf") || declared_as(&parts, name, "image/wmf")
            } else {
                declared_as(&parts, name, "image/emf") || declared_as(&parts, name, "image/x-emf")
            };
            assert!(
                declared,
                "{fixture}: {name} is not declared as a metafile part"
            );
            checked += 1;
        }

        // Then the re-saved package goes through this reader, and the drawings
        // the application copied through have to come out as the pictures the
        // fixture gave — played from the same metafiles, at the same sizes. The
        // one the application re-drew is named, so a fixture that starts
        // differing somewhere else fails here rather than passing on a count.
        let before = pictures_the_reader_takes(&ws, fixture, &bytes, "before");
        let after = pictures_the_reader_takes(&ws, fixture, &resaved, "after");
        let differing: Vec<&ReadPicture> = before
            .iter()
            .filter(|picture| !after.iter().any(|other| the_same_picture(picture, other)))
            .collect();
        assert_eq!(
            differing.len(),
            rewritten,
            "{fixture}: {} drawing(s) came out of the re-saved package as something else:\n\
             re-saved: {after:#?}\nfixture: {before:#?}",
            differing.len()
        );
        if rewritten > 0 {
            assert_eq!(
                differing[0].size,
                (137.0, 77.0),
                "{fixture}: the drawing the application re-drew is not the one this test describes"
            );
        }
    }
    // The corpus holds five metafile drawings; the fifth is `mathtype.docx`'s
    // dropped preview. A loop that quietly stopped covering a fixture would
    // still pass every assertion above — it would just check nothing.
    assert_eq!(checked, 4, "the metafile fixtures lost a drawing");
}

/// A BMP and a TIFF are formats a package can carry, but not the shape Docling
/// leaves a picture in: its own Word and PowerPoint readers open every picture
/// with Pillow and write it back as PNG. A package of each kind carrying both
/// must therefore give two `image/png` pictures per conversion, each at the
/// grid its own header states, with a real PNG written beside the JSON — the
/// header's pixels, not its bytes under a `.png` name — and a render that names
/// both files. A BMP or a TIFF left in the model is the failure this guards.
#[test]
fn a_bmp_and_a_tiff_embedded_in_office_files_become_png_pictures() {
    let ws = Workspace::new("bmp-tiff-office");
    let bmp = std::fs::read(repository_root().join("test-data/images/chart.bmp"))
        .expect("the BMP fixture");
    let tiff = std::fs::read(repository_root().join("test-data/images/chart.tiff"))
        .expect("the TIFF fixture");

    for fixture in ["bmp_tiff_pictures.docx", "bmp_tiff_pictures.pptx"] {
        let input = repository_root().join("test-data").join(fixture);
        let output = ws.join(&format!("{fixture}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--to",
            "json",
            "--to",
            "html",
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(
            pictures.len(),
            2,
            "{fixture}: a picture was lost: {pictures:#?}"
        );

        // Both are found by size, because both are now the same media type:
        // that they are both PNG is the point.
        for (source, (width, height)) in [
            (bmp.as_slice(), (60.0, 36.0)),
            (tiff.as_slice(), (40.0, 24.0)),
        ] {
            let picture = pictures
                .iter()
                .find(|picture| {
                    picture["image"]["size"]["width"] == width
                        && picture["image"]["size"]["height"] == height
                })
                .unwrap_or_else(|| {
                    panic!("{fixture}: no {width} × {height} picture: {pictures:#?}")
                });
            let image = &picture["image"];
            assert_eq!(
                image["mimetype"], "image/png",
                "{fixture}: a picture kept an encoding Docling's office readers never keep"
            );
            let uri = image["uri"].as_str().expect("an image uri");
            assert!(
                uri.ends_with(".png"),
                "{fixture}: the picture was written as {uri}"
            );
            let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
            let expected = pdfextractor_image_parser::transcode_to_png(source)
                .unwrap_or_else(|| panic!("{fixture}: the fixture does not decode"));
            assert_eq!(
                written, expected.data,
                "{fixture}: {uri} is not the transcode of the picture it came from"
            );
        }

        // The render names both: a picture the model holds is one a reader can
        // see, not a box with no image.
        let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
        assert_eq!(
            html.matches("<img src=\"").count(),
            2,
            "{fixture}: a picture is missing from the render:\n{html}"
        );
        assert_eq!(
            html.matches(".png\"").count(),
            2,
            "{fixture}: a picture is not named as a PNG in the render:\n{html}"
        );
    }
}

/// A WebP and both spellings of a JPEG 2000 are encodings the readers decode but
/// Docling cannot name in an `image` reference, so a Word or PowerPoint package
/// embedding one has to hand on a PNG. A package of each kind carrying all three
/// must give three pictures per conversion, each `image/png` at the size its own
/// encoding states, with a real PNG written beside the JSON — the picture's own
/// pixels, not a blank — and a render that names all three files. An
/// `image/webp` or `image/jp2` left in the model is the failure this guards.
///
/// The `.jpc` is a bare codestream rather than a JP2 under another name: it has
/// no `ihdr`, so the 36 × 22 grid the test looks for can only have come from
/// its `SIZ` marker. A reader that went by the extension and assumed a container
/// would have no header to read.
#[test]
fn a_webp_and_both_jpeg_2000_spellings_embedded_in_office_files_become_png_pictures() {
    let ws = Workspace::new("webp-jp2-office");
    let webp = std::fs::read(repository_root().join("test-data/images/chart.webp"))
        .expect("the WebP fixture");
    let jp2 = std::fs::read(repository_root().join("test-data/images/diagram.jp2"))
        .expect("the JPEG 2000 fixture");
    let jpc = std::fs::read(repository_root().join("test-data/images/diagram.jpc"))
        .expect("the bare JPEG 2000 codestream fixture");

    for fixture in ["webp_jp2_pictures.docx", "webp_jp2_pictures.pptx"] {
        let input = repository_root().join("test-data").join(fixture);
        let output = ws.join(&format!("{fixture}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--to",
            "json",
            "--to",
            "html",
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(
            pictures.len(),
            3,
            "{fixture}: a picture was lost: {pictures:#?}"
        );

        // Each is found by its own grid, because all three are now the same
        // media type: that they are all PNG is the point.
        for (source, (width, height)) in [
            (webp.as_slice(), (60.0, 36.0)),
            (jp2.as_slice(), (40.0, 24.0)),
            (jpc.as_slice(), (36.0, 22.0)),
        ] {
            let picture = pictures
                .iter()
                .find(|picture| {
                    picture["image"]["size"]["width"] == width
                        && picture["image"]["size"]["height"] == height
                })
                .unwrap_or_else(|| {
                    panic!("{fixture}: no {width} × {height} picture: {pictures:#?}")
                });
            let image = &picture["image"];
            assert_eq!(
                image["mimetype"], "image/png",
                "{fixture}: a picture Docling cannot name reached the model"
            );
            let uri = image["uri"].as_str().expect("an image uri");
            assert!(
                uri.ends_with(".png"),
                "{fixture}: the picture was written as {uri}"
            );
            let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
            assert_eq!(
                &written[..8],
                &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
                "{fixture}: {uri} is not a PNG"
            );
            let decoded = image::load_from_memory(&written)
                .unwrap_or_else(|e| panic!("{fixture}: {uri} does not decode: {e}"))
                .to_rgb8();
            assert_eq!(
                (decoded.width() as f64, decoded.height() as f64),
                (width, height),
                "{fixture}: {uri} is the encoding's grid"
            );
            // The pixels are the drawing's: a lossless WebP and a lossless JP2
            // decode to this transcode, so an empty or invented PNG fails.
            let expected = pdfextractor_image_parser::transcode_to_png(source)
                .unwrap_or_else(|| panic!("{fixture}: the fixture picture does not decode"));
            assert_eq!(
                written, expected.data,
                "{fixture}: {uri} is not the transcode of the picture it came from"
            );
        }

        // The render names all three: a picture the model holds is one a reader
        // can see, not a box with no image.
        let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
        assert_eq!(
            html.matches("<img src=\"").count(),
            3,
            "{fixture}: a picture is missing from the render:\n{html}"
        );
        assert_eq!(
            html.matches(".png\"").count(),
            3,
            "{fixture}: the render does not name three PNGs:\n{html}"
        );

        let words = doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        for sentence in ["Before the pictures.", "After the pictures."] {
            assert!(
                words.contains(sentence),
                "{fixture}: {sentence} did not survive: {words}"
            );
        }
    }
}

/// An older Word file states its shapes in VML, in `w:pict` groups with no image
/// part behind them: Word paints the shape from the markup, so a conversion has
/// to paint it too. A shape, a group of shapes and a WordArt therefore each
/// become a picture at the size the file gives them, written beside the JSON and
/// named in the render. So do the ways an autoshape is more than a flat box: a
/// `v:fill type="gradient"` ramps two colours across the shape, a `v:shadow`
/// puts a grey copy of the silhouette under it, a `v:stroke` `endarrow` puts a
/// filled head on the end of a line, and VML's `focus` and `focussize` move
/// where a ramp's colours sit — the first putting the second colour in the
/// middle of the blend, the second holding the first colour flat across a disc.
/// The Word horizontal rule in the same fixture is a paragraph's line, not a
/// drawing, and must not become a ninth picture.
///
/// The box and the group are the VML a DOCX writer emits, so the group's oval is
/// placed in the group's `coordsize` (at `1200` of `2000`, i.e. 60 of its 100 pt)
/// and the box is the 119.95 pt the writer states, not a round number.
#[test]
fn a_vml_shape_and_a_wordart_become_pictures() {
    let ws = Workspace::new("vml-shapes");
    let input = repository_root().join("test-data/vml_shapes.docx");
    let output = ws.join("vml_shapes.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(
        pictures.len(),
        8,
        "the horizontal rule must not be a picture: {pictures:#?}"
    );
    let sizes: Vec<(f64, f64)> = pictures
        .iter()
        .map(|picture| {
            let size = &picture["image"]["size"];
            (
                size["width"].as_f64().expect("a width"),
                size["height"].as_f64().expect("a height"),
            )
        })
        .collect();
    // The drawings are rasterised at 144 dpi, two device pixels per point, so a
    // 120 × 60 pt box is 240 × 120 pixels — the size the model carries, with the
    // resolution that makes it 120 × 60 points on the page. The shadowed 80 × 40
    // pt box carries its 5 pt shadow past the frame, and the 100 pt line carries
    // its arrowhead's 6 pt reach on every side. The focused ramp is its 120 × 60
    // pt box, and the focus-sized radial fill its 80 × 80 pt square.
    assert_eq!(
        sizes,
        vec![
            (240.0, 120.0),
            (200.0, 100.0),
            (360.0, 80.0),
            (240.0, 120.0),
            (170.0, 90.0),
            (228.0, 28.0),
            (240.0, 120.0),
            (160.0, 160.0),
        ]
    );

    for picture in pictures {
        let image = &picture["image"];
        assert_eq!(
            image["mimetype"], "image/png",
            "a drawing is carried as pixels"
        );
        assert_eq!(image["dpi"], 144, "the drawings are rasterised at 144 dpi");
        let uri = image["uri"].as_str().expect("an image uri");
        let bytes = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        let drawn = image::load_from_memory(&bytes).expect("the drawing decodes");
        assert_eq!(
            (drawn.width() as f64, drawn.height() as f64),
            (
                image["size"]["width"].as_f64().expect("a width"),
                image["size"]["height"].as_f64().expect("a height"),
            ),
            "{uri}"
        );
    }

    // The drawings are the file's: the rounded box has its fill and a corner the
    // radius leaves white, the group has its two shapes, the WordArt its red
    // letters.
    let first = image::load_from_memory(
        &std::fs::read(ws.join(pictures[0]["image"]["uri"].as_str().unwrap())).expect("the box"),
    )
    .expect("the box decodes")
    .to_rgba8();
    assert_eq!(
        first.get_pixel(120, 60).0,
        [21, 96, 130, 255],
        "the box's fill"
    );
    assert_eq!(
        first.get_pixel(0, 0).0,
        [255, 255, 255, 255],
        "the rounded corner"
    );

    let group = image::load_from_memory(
        &std::fs::read(ws.join(pictures[1]["image"]["uri"].as_str().unwrap())).expect("the group"),
    )
    .expect("the group decodes")
    .to_rgba8();
    assert_eq!(
        group.get_pixel(160, 50).0,
        [109, 163, 58, 255],
        "the oval's fill"
    );
    assert_eq!(
        group.get_pixel(20, 50).0,
        [255, 255, 255, 255],
        "inside the box only"
    );

    let wordart = image::load_from_memory(
        &std::fs::read(ws.join(pictures[2]["image"]["uri"].as_str().unwrap()))
            .expect("the WordArt"),
    )
    .expect("the WordArt decodes")
    .to_rgba8();
    let letters = wordart
        .pixels()
        .filter(|pixel| pixel.0 == [192, 0, 0, 255])
        .count();
    assert!(
        letters > 1000,
        "the WordArt's letters drew {letters} pixel(s)"
    );

    // The gradient: red at its left edge, blue at its right, the mix between.
    let ramp = image::load_from_memory(
        &std::fs::read(ws.join(pictures[3]["image"]["uri"].as_str().unwrap())).expect("the ramp"),
    )
    .expect("the ramp decodes")
    .to_rgba8();
    let start = ramp.get_pixel(2, 60).0;
    let end = ramp.get_pixel(237, 60).0;
    let middle = ramp.get_pixel(120, 60).0;
    assert!(
        start[0] > 230 && start[2] < 25,
        "the ramp starts red: {start:?}"
    );
    assert!(end[2] > 230 && end[0] < 25, "it ends blue: {end:?}");
    assert!(
        (i32::from(middle[0]) - 127).abs() < 12 && (i32::from(middle[2]) - 127).abs() < 12,
        "and is halfway between the stops in the middle: {middle:?}"
    );

    // The shadow: the box where it is, the grey copy past its edge, and nothing
    // above or left of the offset it falls at.
    let shade = image::load_from_memory(
        &std::fs::read(ws.join(pictures[4]["image"]["uri"].as_str().unwrap())).expect("the box"),
    )
    .expect("the shadowed box decodes")
    .to_rgba8();
    assert_eq!(
        shade.get_pixel(30, 20).0,
        [109, 163, 58, 255],
        "the box's fill"
    );
    assert_eq!(
        shade.get_pixel(168, 8).0,
        [255, 255, 255, 255],
        "right of the box and above the shadow, where neither is"
    );
    let cast = shade.get_pixel(130, 80).0;
    assert!(
        cast[0].abs_diff(cast[1]) < 4 && cast[1].abs_diff(cast[2]) < 4,
        "the shadow is grey, not the box's colour: {cast:?}"
    );

    // The connector's head: ink up to its tip, wider than the 2 pt line, and
    // nothing past the tip.
    let arrow = image::load_from_memory(
        &std::fs::read(ws.join(pictures[5]["image"]["uri"].as_str().unwrap())).expect("the line"),
    )
    .expect("the arrowed line decodes")
    .to_rgba8();
    assert_eq!(arrow.get_pixel(100, 14).0, [31, 78, 121, 255], "the line");
    assert_eq!(
        arrow.get_pixel(206, 11).0,
        [31, 78, 121, 255],
        "inside the head, past the line's edge"
    );
    assert_eq!(
        arrow.get_pixel(220, 14).0,
        [255, 255, 255, 255],
        "past the tip"
    );

    // The focused ramp: `focus` is where the blend's middle sits, so the same
    // red-blue pair runs red, blue, red across the box — the second stop sits
    // where an unfocused ramp would be half-mixed, and both ends are the first
    // colour. A reader that dropped `focus` would leave the middle half-mixed
    // and the right edge blue, which is what these three points rule out.
    let focused = image::load_from_memory(
        &std::fs::read(ws.join(pictures[6]["image"]["uri"].as_str().unwrap()))
            .expect("the focused ramp"),
    )
    .expect("the focused ramp decodes")
    .to_rgba8();
    let left = focused.get_pixel(2, 60).0;
    let middle = focused.get_pixel(120, 60).0;
    let right = focused.get_pixel(237, 60).0;
    assert!(
        left[0] > 240 && left[2] < 10,
        "the focused ramp starts red: {left:?}"
    );
    assert!(
        middle[2] > 240 && middle[0] < 10,
        "its second colour sits in the middle of the blend: {middle:?}"
    );
    assert!(
        right[0] > 240 && right[2] < 10,
        "and it runs back to the first colour at the other end: {right:?}"
    );
    let quarter = focused.get_pixel(60, 60).0;
    assert!(
        (i32::from(quarter[0]) - 127).abs() < 12 && (i32::from(quarter[2]) - 127).abs() < 12,
        "and halfway between its stops a quarter of the way along: {quarter:?}"
    );

    // The focus-sized radial fill: `focussize="50%,50%"` on an 80 pt box makes
    // the first colour a 40 pt disc rather than a point, so the points inside
    // it are exactly the first colour — where a point-focused ramp would
    // already have blended most of them — and the ramp from the disc's edge
    // still reaches the second colour at the frame's.
    let radial = image::load_from_memory(
        &std::fs::read(ws.join(pictures[7]["image"]["uri"].as_str().unwrap()))
            .expect("the focused radial"),
    )
    .expect("the focused radial decodes")
    .to_rgba8();
    for (x, y) in [(80, 80), (61, 61), (99, 61), (61, 99), (99, 99)] {
        assert_eq!(
            radial.get_pixel(x, y).0,
            [192, 57, 43, 255],
            "the focus region is flat at ({x}, {y})"
        );
    }
    let beyond = radial.get_pixel(150, 80).0;
    assert!(
        beyond[2] > 130 && beyond[0] < 130,
        "past the disc the ramp has moved toward the second colour: {beyond:?}"
    );
    let edge = radial.get_pixel(159, 80).0;
    assert!(
        edge[2] > 165 && edge[0] < 70,
        "and reaches it at the frame's edge: {edge:?}"
    );

    // The render names the pictures, and the prose around them is unchanged.
    let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
    assert_eq!(
        html.matches("<img src=\"").count(),
        8,
        "a picture is missing from the render:\n{html}"
    );
    let words = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    for sentence in ["Before the shapes.", "After the shapes."] {
        assert!(
            words.contains(sentence),
            "{sentence} did not survive: {words}"
        );
    }
}

/// A slide states each autoshape as markup too — a `p:spPr` naming a preset
/// outline — so the pictures a conversion writes are the shapes the deck shows.
/// The fixture holds one shape for every preset the painter covers, written by
/// python-pptx (`test-data/make_preset_deck.py`), so a preset that stops being
/// painted leaves a hole here as well as in the package reader's own test: all
/// 68 have to reach the JSON, each with its own PNG written beside it, at the
/// size of its frame and with ink on it.
#[test]
fn every_preset_shape_of_a_deck_is_exported() {
    let ws = Workspace::new("preset-deck");
    let input = repository_root().join("test-data/preset_deck.pptx");
    let output = ws.join("preset_deck.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(
        pictures.len(),
        68,
        "a preset shape was left unpainted: {pictures:#?}"
    );

    let mut uris: Vec<&str> = Vec::new();
    for (index, picture) in pictures.iter().enumerate() {
        let carried = &picture["image"];
        assert_eq!(carried["mimetype"], "image/png", "picture {index}");
        assert_eq!(
            carried["dpi"], 144,
            "picture {index} is not rasterised like a metafile"
        );
        // Each preset is a 90 × 50 pt cell at two device pixels per point; the
        // elbow and the curve span two cells and are as tall as one.
        let expected = if index >= 66 {
            (360.0, 100.0)
        } else {
            (180.0, 100.0)
        };
        let size = &carried["size"];
        assert_eq!(
            (
                size["width"].as_f64().expect("a width"),
                size["height"].as_f64().expect("a height"),
            ),
            expected,
            "picture {index} is not the size of its frame"
        );

        let uri = carried["uri"].as_str().expect("an image uri");
        assert!(
            uri.starts_with("preset_deck_images/"),
            "picture {index} names {uri}, not a file beside the JSON"
        );
        uris.push(uri);
        let bytes = std::fs::read(ws.join(uri)).unwrap_or_else(|e| {
            panic!("picture {index} was not written beside the JSON ({uri}): {e}")
        });
        let drawn = image::load_from_memory(&bytes)
            .unwrap_or_else(|e| panic!("picture {index} does not decode: {e}"))
            .to_rgba8();
        assert_eq!(
            (drawn.width(), drawn.height()),
            (expected.0 as u32, expected.1 as u32),
            "picture {index} is not the size its entry states"
        );
        assert!(
            drawn.pixels().any(|pixel| pixel.0[..3] != [255, 255, 255]),
            "picture {index} is a blank canvas"
        );
    }

    // One file per shape: no two pictures share a file, and the directory holds
    // nothing the model does not name.
    uris.sort_unstable();
    uris.dedup();
    assert_eq!(uris.len(), 68, "two shapes were written to one file");
    let written = std::fs::read_dir(ws.join("preset_deck_images"))
        .expect("the picture directory")
        .count();
    assert_eq!(written, 68, "the directory holds files no picture names");

    // The render names every one of them too, so the pictures are exported and
    // not only counted in the model.
    let html = std::fs::read_to_string(ws.join("preset_deck.html")).expect("the HTML written");
    assert_eq!(
        html.matches("<img src=\"preset_deck_images/").count(),
        68,
        "a picture is missing from the render"
    );
}

/// A modern Word file states a shape in DrawingML, as a `wps:wsp` inside
/// `mc:AlternateContent` with a VML copy of the same shape in the Fallback and no
/// image part behind either. A package holding a rounded box, a group of shapes
/// and a five-pointed star must give three pictures — the drawing, not the
/// fallback beside it, and not both — each at the size the drawing states and
/// drawn at the metafiles' 144 dpi. A shape Word paints cannot be nothing in the
/// model.
#[test]
fn a_drawingml_shape_is_painted_from_its_own_markup() {
    let ws = Workspace::new("drawingml-shapes");
    let input = repository_root().join("test-data/drawingml_shapes.docx");
    let output = ws.join("drawingml_shapes.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(
        pictures.len(),
        3,
        "a shape Word paints was left out: {pictures:#?}"
    );
    let sizes: Vec<(f64, f64)> = pictures
        .iter()
        .map(|picture| {
            let size = &picture["image"]["size"];
            (
                size["width"].as_f64().expect("a width"),
                size["height"].as_f64().expect("a height"),
            )
        })
        .collect();
    // 120 × 60, 100 × 50 and 80 × 80 points, two device pixels per point.
    assert_eq!(sizes, vec![(240.0, 120.0), (200.0, 100.0), (160.0, 160.0)]);
    for picture in pictures {
        let image = &picture["image"];
        assert_eq!(image["mimetype"], "image/png");
        assert_eq!(image["dpi"], 144, "a shape is rasterised like a metafile");
    }

    let image = |index: usize| {
        image::load_from_memory(
            &std::fs::read(ws.join(pictures[index]["image"]["uri"].as_str().unwrap()))
                .expect("the picture written beside the JSON"),
        )
        .expect("the drawing decodes")
        .to_rgba8()
    };

    // The box: its fill in the middle, and the rounded corner left white.
    let box_ = image(0);
    assert_eq!(box_.get_pixel(120, 60).0, [21, 96, 130, 255], "the fill");
    assert_eq!(
        box_.get_pixel(0, 0).0,
        [255, 255, 255, 255],
        "the rounded corner"
    );

    // The group: its child frame places the oval on the right, inside the
    // rectangle outline the child draws.
    let group = image(1);
    assert_eq!(
        group.get_pixel(160, 50).0,
        [109, 163, 58, 255],
        "the oval's fill"
    );
    assert_eq!(
        group.get_pixel(1, 1).0,
        [47, 109, 181, 255],
        "the rectangle's outline"
    );

    // The star: the fill it states, inside the points and not between them.
    let star = image(2);
    assert_eq!(
        star.get_pixel(80, 80).0,
        [79, 129, 189, 255],
        "the star's fill"
    );
    assert_eq!(
        star.get_pixel(4, 4).0,
        [255, 255, 255, 255],
        "between two points"
    );

    // The render names all three, and the prose around them is unchanged.
    let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
    assert_eq!(
        html.matches("<img src=\"").count(),
        3,
        "a picture is missing from the render:\n{html}"
    );
    let words = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    for sentence in ["Before the shapes.", "After the shapes."] {
        assert!(
            words.contains(sentence),
            "{sentence} did not survive: {words}"
        );
    }
}

/// A slide's shape is shaded the way its own markup states, not flushed to one
/// colour: its fill can be a gradient ramped across it, the box can cast a soft
/// shadow past its frame, and a connector's line can carry an arrowhead at
/// either end. Each is markup in the shape's `p:spPr`, and PowerPoint paints all
/// three, so a reader that stopped at a solid fill and a plain line drew a slide
/// that is not the one on screen.
///
/// `shaded_deck.pptx` holds one of each, written by python-pptx: a `180 × 90` pt
/// box filled with a red-to-blue `a:lin` gradient, a `120 × 60` pt box with an
/// `a:outerShdw` under it, and two `120` pt connectors, one with a triangle at
/// its tail and one with a stealth head at each end.
#[test]
fn a_slide_shape_is_shaded_the_way_its_markup_states() {
    let ws = Workspace::new("shaded-deck");
    let input = repository_root().join("test-data/shaded_deck.pptx");
    let output = ws.join("shaded_deck.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(
        pictures.len(),
        4,
        "a shaded shape was left out: {pictures:#?}"
    );
    let sizes: Vec<(f64, f64)> = pictures
        .iter()
        .map(|picture| {
            let size = &picture["image"]["size"];
            (
                size["width"].as_f64().expect("a width"),
                size["height"].as_f64().expect("a height"),
            )
        })
        .collect();
    // 180 × 90 pt of gradient box; 120 × 60 of box with a 5 pt shadow at 45°
    // over a 6 pt blur, so its ink is 132 × 72; and two 120 pt connectors whose
    // 6 pt heads make their boxes 134 × 14 pt. Two pixels to the point.
    assert_eq!(
        sizes,
        vec![(360.0, 180.0), (264.0, 144.0), (268.0, 28.0), (268.0, 28.0)]
    );

    let image = |index: usize| {
        image::load_from_memory(
            &std::fs::read(ws.join(pictures[index]["image"]["uri"].as_str().unwrap()))
                .expect("the picture written beside the JSON"),
        )
        .expect("the shape decodes")
        .to_rgba8()
    };

    // The gradient: red at its left edge, blue at its right, the mix between.
    let ramp = image(0);
    let start = ramp.get_pixel(2, 90).0;
    let end = ramp.get_pixel(357, 90).0;
    let middle = ramp.get_pixel(180, 90).0;
    assert!(
        start[0] > 230 && start[2] < 25,
        "the ramp starts red: {start:?}"
    );
    assert!(end[2] > 230 && end[0] < 25, "it ends blue: {end:?}");
    assert!(
        (i32::from(middle[0]) - 127).abs() < 8 && (i32::from(middle[2]) - 127).abs() < 8,
        "and is halfway between the two stops in the middle: {middle:?}"
    );

    // The shadow: the box where it is, the shadow past its frame, and nothing
    // above or left of it, which is where a 45° shadow does not fall.
    let shade = image(1);
    assert_eq!(
        shade.get_pixel(130, 72).0,
        [109, 163, 58, 255],
        "the box's fill"
    );
    assert_eq!(
        shade.get_pixel(3, 3).0,
        [255, 255, 255, 255],
        "above and left of the box, where the shadow is not"
    );
    let shadow = shade.get_pixel(250, 130).0;
    assert!(
        shadow[0].abs_diff(shadow[1]) < 4 && shadow[1].abs_diff(shadow[2]) < 4,
        "the shadow is grey, not the box's colour: {shadow:?}"
    );
    assert!(
        (190..=235).contains(&shadow[0]),
        "and it is the soft grey #000000 at 40% puts there: {shadow:?}"
    );
    let fringe = shade.get_pixel(258, 140).0;
    assert!(
        fringe[0] > 245,
        "the blurred edge fades out past the silhouette: {fringe:?}"
    );

    // The connector's triangle: ink up to its tip, wider than the 2 pt line,
    // and nothing past the tip.
    let triangle = image(2);
    assert_eq!(triangle.get_pixel(40, 13).0, [31, 78, 121, 255], "the line");
    assert_eq!(
        triangle.get_pixel(244, 13).0,
        [31, 78, 121, 255],
        "the head's filled middle"
    );
    assert_eq!(
        triangle.get_pixel(244, 9).0,
        [31, 78, 121, 255],
        "a corner of the head, past the line's edge"
    );
    assert_eq!(
        triangle.get_pixel(262, 13).0,
        [255, 255, 255, 255],
        "past the tip"
    );

    // The stealth: the same head with its base notched back toward the tip, so
    // the two corners are ink and the middle between them is not — and the
    // stroke stopped at the base rather than running through the head.
    let stealth = image(3);
    assert_eq!(
        stealth.get_pixel(244, 9).0,
        [31, 78, 121, 255],
        "a corner of the head"
    );
    assert_eq!(
        stealth.get_pixel(244, 13).0,
        [255, 255, 255, 255],
        "the notch cut into the head's base"
    );
    assert_eq!(
        stealth.get_pixel(246, 13).0,
        [31, 78, 121, 255],
        "and the head's body nearer the tip"
    );

    // The render names all four, and the slide's words are still its text.
    let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
    assert_eq!(
        html.matches("<img src=\"").count(),
        4,
        "a shaded shape is missing from the render:\n{html}"
    );
    let words = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        words.contains("Gradients, shadows and arrowheads."),
        "the slide's words did not survive: {words}"
    );
}

/// A PowerPoint slide's shapes are painted the way a Word document's are: an
/// autoshape, a connector and a group have no image part behind them, so each is
/// drawn by its own markup — at the metafiles' 144 dpi, in the colours the slide
/// states and the theme it inherits — while the picture the deck embeds stays the
/// media part it is. A text box with no fill and no line puts no ink down and is
/// no picture; the words of a shape are the slide's text either way.
#[test]
fn a_slides_shapes_are_painted_even_with_no_image_part() {
    let ws = Workspace::new("shape-deck");
    let input = repository_root().join("test-data/shape_deck.pptx");
    let output = ws.join("shape_deck.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(
        pictures.len(),
        5,
        "a shape PowerPoint paints was left out: {pictures:#?}"
    );
    let sizes: Vec<(f64, f64)> = pictures
        .iter()
        .map(|picture| {
            let size = &picture["image"]["size"];
            (
                size["width"].as_f64().expect("a width"),
                size["height"].as_f64().expect("a height"),
            )
        })
        .collect();
    // The box, the star, the connector (its stroke's 2 pt), the group, and the
    // scan at its own grid.
    assert_eq!(
        sizes,
        vec![
            (240.0, 120.0),
            (160.0, 160.0),
            (436.0, 4.0),
            (144.0, 72.0),
            (620.0, 350.0)
        ]
    );
    for picture in &pictures[..4] {
        assert_eq!(picture["image"]["mimetype"], "image/png");
    }
    assert_eq!(
        pictures[0]["image"]["dpi"].as_u64(),
        Some(144),
        "a shape is rasterised like a metafile"
    );

    let image = |index: usize| {
        image::load_from_memory(
            &std::fs::read(ws.join(pictures[index]["image"]["uri"].as_str().unwrap()))
                .expect("the picture written beside the JSON"),
        )
        .expect("the drawing decodes")
        .to_rgba8()
    };

    // The box: its fill in the middle, and the rounded corner left white.
    let box_ = image(0);
    assert_eq!(box_.get_pixel(120, 60).0, [21, 96, 130, 255], "the fill");
    assert_eq!(
        box_.get_pixel(0, 0).0,
        [255, 255, 255, 255],
        "the rounded corner"
    );

    // The star states no colour of its own: it is the theme's accent, which the
    // slide reaches through its layout and master.
    let star = image(1);
    assert_eq!(
        star.get_pixel(80, 80).0,
        [79, 129, 189, 255],
        "the theme's accent"
    );
    assert_eq!(
        star.get_pixel(4, 4).0,
        [255, 255, 255, 255],
        "between two points"
    );

    // The connector: one line, in the blue it states.
    let line = image(2);
    assert_eq!(line.get_pixel(218, 2).0, [31, 78, 121, 255], "the line");

    // The group: its coordinate space places the oval on the right, inside the
    // rectangle its other child outlines.
    let group = image(3);
    assert_eq!(
        group.get_pixel(110, 36).0,
        [109, 163, 58, 255],
        "the oval's fill"
    );
    assert_eq!(
        group.get_pixel(14, 36).0,
        [255, 255, 255, 255],
        "left of it"
    );

    // The embedded picture was not painted again: it is the media part.
    let written =
        std::fs::read(ws.join(pictures[4]["image"]["uri"].as_str().unwrap())).expect("the scan");
    assert_eq!(
        written,
        fixture("invoice_scan.png"),
        "an embedded picture was re-encoded"
    );

    // The words are the slide's text, not ink: three of them, and the render
    // names all five pictures.
    let words = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    for sentence in [
        "Before the shapes.",
        "A box with words.",
        "Words with no ink behind them.",
    ] {
        assert!(
            words.contains(sentence),
            "{sentence} did not survive: {words}"
        );
    }
    let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
    assert_eq!(
        html.matches("<img src=\"").count(),
        5,
        "a picture is missing from the render:\n{html}"
    );
}

/// A slide's picture background is the page's image, not a picture item.
///
/// `background_deck.pptx` states `images/scan.jpg` in its first slide's
/// `p:cSld/p:bg/p:bgPr/a:blipFill`, outside the shape tree; the second slide has
/// no `p:bg` at all. The extract writes the backdrop beside the JSON and names
/// it in `pages["1"].image` at its own 620 × 350 grid, with the resolution its
/// stretch across the 10-inch page implies. It is not in `pictures`: a backdrop
/// is behind the page rather than a block on it. The second page has no image,
/// and both slides' sentences are still their text.
#[test]
fn a_slides_picture_background_is_the_pages_image() {
    let ws = Workspace::new("background-deck");
    let input = repository_root().join("test-data/background_deck.pptx");
    let output = ws.join("background.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pages = doc["pages"].as_object().expect("a pages map");
    let image = &pages["1"]["image"];
    assert_eq!(image["mimetype"], "image/jpeg");
    assert_eq!(image["dpi"].as_u64(), Some(62), "620 px across 720 pt");
    assert_eq!(image["size"]["width"].as_f64(), Some(620.0));
    assert_eq!(image["size"]["height"].as_f64(), Some(350.0));
    let uri = image["uri"].as_str().expect("the backdrop's file");
    assert!(
        uri.ends_with("slide1_background.jpg"),
        "the page image is not named for the slide it backs: {uri}"
    );
    assert_eq!(
        std::fs::read(ws.join(uri)).expect("the backdrop written beside the JSON"),
        fixture("images/scan.jpg"),
        "the page image is not the background's own JPEG bytes"
    );

    assert_eq!(
        pages["2"].get("image"),
        None,
        "a slide with no background was given one"
    );
    assert_eq!(
        doc["pictures"].as_array().expect("a pictures list").len(),
        0,
        "the backdrop was reported as a picture item as well as the page's image"
    );

    let words = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    for sentence in [
        "A photograph behind the words.",
        "No picture behind these words.",
    ] {
        assert!(
            words.contains(sentence),
            "{sentence} did not survive: {words}"
        );
    }
}

/// `--ocr-lang` and `--psm` are checked before work starts, and a real mode
/// still recognises a real scan.
#[test]
fn ocr_languages_and_page_segmentation_are_checked() {
    let ws = Workspace::new("ocrflags");
    let hello = ws.copy_from_corpus("../hello.pdf", "hello.pdf");

    // A language list with nothing in it names no engine at all.
    let out = run(&["-i", hello.to_str().unwrap(), "--ocr-lang", " , + "]);
    assert!(!out.status.success(), "an empty language list was accepted");
    assert!(
        stderr_of(&out).contains("--ocr-lang"),
        "unclear error: {}",
        stderr_of(&out)
    );

    // The mode is a Tesseract mode, and Tesseract's stop at 13.
    let out = run(&["-i", hello.to_str().unwrap(), "--psm", "14"]);
    assert!(!out.status.success(), "psm 14 was accepted");

    // A chosen mode recognises the scan: PSM 6 is one block of text, which is
    // what the fixture's page is.
    let scan = repository_root().join("test-data/scanned.pdf");
    let output = ws.join("scan.json");
    run_ok(&[
        "-i",
        scan.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--psm",
        "6",
    ]);
    let doc = json_of(&output);
    let text = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|t| t["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(text.contains("ACME"), "--psm 6 lost the scan: {text}");
}

/// A `/Link` annotation's URI reaches `TextItem.hyperlink` — annotation read,
/// rectangle flipped, runs joined to it, field set on the item.
#[test]
fn pdf_link_annotations_reach_the_json() {
    let ws = Workspace::new("pdf-link");
    let fixture = repository_root().join("test-data/link_annotations.pdf");
    let bytes =
        std::fs::read(&fixture).unwrap_or_else(|e| panic!("missing {}: {e}", fixture.display()));
    let input = ws.write("links.pdf", &bytes);
    let output = ws.join("links.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);

    let doc = json_of(&output);
    let texts = doc["texts"].as_array().expect("a text list");
    let linked: Vec<&serde_json::Value> = texts
        .iter()
        .filter(|t| t["hyperlink"].is_string())
        .collect();
    assert_eq!(linked.len(), 1, "{texts:#?}");
    assert_eq!(linked[0]["hyperlink"], "https://example.com/spec");
    assert_eq!(linked[0]["text"], "Visit the spec page");

    // The other page's text stands under no link rectangle and says nothing
    // about links at all — the field stays out of the JSON entirely.
    let plain: Vec<&serde_json::Value> = texts
        .iter()
        .filter(|t| !t["hyperlink"].is_string())
        .collect();
    assert_eq!(plain.len(), 1, "{texts:#?}");
    assert_eq!(plain[0]["text"], "Second page");
}

/// A PDF states a JPEG 2000 as an image XObject with `/Filter /JPXDecode`, and
/// the stream behind that filter is either of the two things a `.jp2` file is
/// made of: the container around the codestream, or the bare codestream. Docling
/// cannot name `image/jp2` in an image reference, so a picture left as one had
/// its bytes written beside the JSON and no `image` on the item at all — the box
/// with no image that an image reference exists to avoid.
///
/// Both forms are decoded, so both are PNG pictures: `image/png`, a `.png` file
/// beside the JSON, the codestream's own 620 × 350 grid, and a picture that is
/// the scan rather than a blank. The page draws the same drawing twice, once in
/// each form, which is what makes the two files a byte-for-byte comparison.
#[test]
fn jpx_pictures_in_a_pdf_become_named_pngs() {
    let ws = Workspace::new("jpx");
    let fixture = repository_root().join("test-data/jpx_picture.pdf");
    let bytes =
        std::fs::read(&fixture).unwrap_or_else(|e| panic!("missing {}: {e}", fixture.display()));
    let input = ws.write("jpx.pdf", &bytes);
    let output = ws.join("jpx.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(pictures.len(), 2, "{pictures:#?}");

    let mut written = Vec::new();
    for (index, picture) in pictures.iter().enumerate() {
        let image = &picture["image"];
        assert!(
            !image.is_null(),
            "picture {index} has no image: {picture:#?}"
        );
        assert_eq!(image["mimetype"], "image/png", "picture {index}");
        assert_eq!(image["size"]["width"], 620.0, "picture {index}");
        assert_eq!(image["size"]["height"], 350.0, "picture {index}");
        // 620 pixels over the 300 pt box it is drawn in, not the codestream's
        // own (absent) capture resolution.
        assert_eq!(image["dpi"], 149, "picture {index}");
        let uri = image["uri"].as_str().expect("an image uri");
        assert!(
            uri.ends_with(".png"),
            "picture {index} was written as {uri}"
        );
        written.push(std::fs::read(ws.join(uri)).expect("the picture written beside the JSON"));
    }

    // The two streams are one drawing: the container's codestream and the bare
    // codestream decode to the same PNG.
    assert_eq!(written[0], written[1], "the two forms decoded differently");
    let decoded = image::load_from_memory(&written[0])
        .expect("the written picture decodes")
        .to_rgb8();
    assert_eq!(decoded.dimensions(), (620, 350));
    let first = decoded.get_pixel(0, 0);
    assert!(
        decoded.pixels().any(|pixel| pixel != first),
        "the picture is one flat colour"
    );

    // The render names both files, and no JPEG 2000 reached it.
    let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
    assert_eq!(
        html.matches("<img src=\"").count(),
        2,
        "a picture is missing from the render:\n{html}"
    );
    assert!(
        !html.contains(".jp2"),
        "a JPEG 2000 reached the render:\n{html}"
    );

    // Nothing in the image directory is still a JPEG 2000 either.
    for entry in std::fs::read_dir(ws.join("jpx_images"))
        .expect("the image directory")
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(name.ends_with(".png"), "a picture was written as {name}");
    }
}

/// A workbook's sheets become pages, groups and tables: the shape Docling's
/// own XLSX backend produces, read out of the JSON the binary wrote.
#[test]
fn a_workbook_becomes_pages_groups_and_tables() {
    let ws = Workspace::new("xlsx");
    let fixture = repository_root().join("test-data/inventory.xlsx");
    let bytes =
        std::fs::read(&fixture).unwrap_or_else(|e| panic!("missing {}: {e}", fixture.display()));
    let input = ws.write("inventory.xlsx", &bytes);
    let output = ws.join("inventory.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);

    let doc = json_of(&output);
    assert_eq!(doc["name"], "inventory");
    assert_eq!(
        doc["origin"]["mimetype"],
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    );
    assert_eq!(doc["origin"]["filename"], "inventory.xlsx");

    // One page per worksheet, sized in cell indices: the sheet's largest right
    // and bottom edge, not a printed page.
    let sizes: Vec<(f64, f64)> = ["1", "2", "3"]
        .iter()
        .map(|page| {
            let size = &doc["pages"][page]["size"];
            (
                size["width"].as_f64().unwrap(),
                size["height"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(sizes, vec![(4.0, 6.0), (2.0, 3.0), (1.0, 2.0)]);

    // A sheet group per worksheet, the hidden one on the invisible layer; the
    // body lists the groups, and each group lists its own tables.
    let groups = doc["groups"].as_array().expect("a group list");
    let described: Vec<(&str, &str, &str)> = groups
        .iter()
        .map(|group| {
            (
                group["name"].as_str().unwrap_or_default(),
                group["content_layer"].as_str().unwrap_or_default(),
                group["label"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        described,
        vec![
            ("Inventory", "body", "sheet"),
            ("Notes", "body", "sheet"),
            ("Hidden", "invisible", "sheet"),
        ]
    );
    let body_children: Vec<&str> = doc["body"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|child| child["$ref"].as_str().unwrap())
        .collect();
    assert_eq!(
        body_children,
        vec!["#/groups/0", "#/groups/1", "#/groups/2"]
    );
    for (index, group) in groups.iter().enumerate() {
        assert_eq!(group["self_ref"], format!("#/groups/{index}"));
        assert_eq!(group["parent"]["$ref"], "#/body");
    }

    // Five tables: the header block, the merged note, the lone "Total" cell,
    // the Notes sheet, and the hidden sheet's.
    let tables = doc["tables"].as_array().expect("a table list");
    assert_eq!(tables.len(), 5, "{tables:#?}");
    let boxes: Vec<(i64, [f64; 4])> = tables
        .iter()
        .map(|table| {
            let prov = &table["prov"][0];
            let bbox = &prov["bbox"];
            (
                prov["page_no"].as_i64().unwrap(),
                [
                    bbox["l"].as_f64().unwrap(),
                    bbox["t"].as_f64().unwrap(),
                    bbox["r"].as_f64().unwrap(),
                    bbox["b"].as_f64().unwrap(),
                ],
            )
        })
        .collect();
    assert_eq!(
        boxes,
        vec![
            (1, [0.0, 0.0, 4.0, 3.0]),
            (1, [1.0, 4.0, 3.0, 5.0]),
            (1, [0.0, 5.0, 1.0, 6.0]),
            (2, [0.0, 0.0, 2.0, 3.0]),
            (3, [0.0, 0.0, 1.0, 2.0]),
        ]
    );
    // The tables sit inside their sheet's group, not directly under the body.
    for (index, table) in tables.iter().enumerate() {
        let group = if index < 3 {
            0
        } else if index == 3 {
            1
        } else {
            2
        };
        assert_eq!(table["parent"]["$ref"], format!("#/groups/{group}"));
        let children = doc["groups"][group]["children"].as_array().unwrap();
        assert!(children
            .iter()
            .any(|child| child["$ref"] == format!("#/tables/{index}")));
    }
    assert_eq!(tables[4]["content_layer"], "invisible");

    // A merged range is one cell covering both its slots; the slot it covers
    // is not a cell of the table at all.
    let table = &tables[1]["data"];
    assert_eq!(table["num_rows"], 1);
    assert_eq!(table["num_cols"], 2);
    let cells = table["table_cells"].as_array().unwrap();
    assert_eq!(cells.len(), 1, "{cells:#?}");
    assert_eq!(cells[0]["text"], "Shipping included");
    assert_eq!(cells[0]["col_span"], 2);

    // A date is the text a reader sees, not the serial the file stores, and
    // the first row of a table is its header — while no worksheet cell labels
    // its own row.
    let notes = tables[3]["data"]["table_cells"].as_array().unwrap();
    let updated = notes
        .iter()
        .find(|cell| cell["start_row_offset_idx"] == 2 && cell["start_col_offset_idx"] == 1)
        .expect("the date cell");
    assert_eq!(updated["text"], "2024-03-05 00:00:00");
    for cell in notes {
        assert_eq!(cell["column_header"], cell["start_row_offset_idx"] == 0);
        assert_eq!(cell["row_header"], false);
    }
}

/// A workbook's pictures are handed on the way a Word or PowerPoint package's
/// are: a PNG and a JPEG travel untouched, and a BMP, a GIF and both spellings of
/// a JPEG 2000 — the bare `.jpc` codestream and the `.jp2` container around one —
/// encodings Docling's own readers never keep, are decoded and written back as
/// PNGs beside the JSON. Every picture is placed in cells on the sheet that
/// draws it, and the render names each file.
///
/// The written PNGs are held to the pixels of the parts they came from, not to
/// their headers: a channel swap, a palette read wrong or a row order reversed
/// would pass a size check and fails this one. The two JPEG 2000s are the
/// pictures whose bytes no picture crate reads unaided — a bare codestream states
/// its grid in the stream's own `SIZ` marker and the container in its `jp2h`
/// box, neither of which `image` opens — so a reader that merely named the part
/// would place a box with no ink in it, and the pixels are what say each was
/// decoded.
#[test]
fn a_workbook_that_draws_pictures_writes_them_beside_the_json() {
    let ws = Workspace::new("xlsx-pictures");
    let path = repository_root().join("test-data/worksheet_pictures.xlsx");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    let input = ws.write("worksheet_pictures.xlsx", &bytes);
    let output = ws.join("worksheet_pictures.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(pictures.len(), 7, "a picture was lost: {pictures:#?}");

    // In the order the workbook draws them, each on its own sheet's page and in
    // the cells its anchor names — the BMP in the first column of row 9, the
    // scan spilling 0.1995 of a cell into the next column.
    let placed: Vec<(i64, [f64; 4])> = pictures
        .iter()
        .map(|picture| {
            let prov = &picture["prov"][0];
            let bbox = &prov["bbox"];
            (
                prov["page_no"].as_i64().unwrap(),
                [
                    bbox["l"].as_f64().unwrap(),
                    bbox["t"].as_f64().unwrap(),
                    bbox["r"].as_f64().unwrap(),
                    bbox["b"].as_f64().unwrap(),
                ],
            )
        })
        .collect();
    fn close(actual: (i64, [f64; 4]), expected: (i64, [f64; 4])) {
        assert_eq!(actual.0, expected.0, "{actual:?} is not {expected:?}");
        for (got, want) in actual.1.iter().zip(expected.1) {
            assert!((got - want).abs() < 1e-9, "{actual:?} is not {expected:?}");
        }
    }
    close(placed[0], (1, [0.0, 8.0, 571_500.0 / 609_600.0, 9.8]));
    close(placed[1], (1, [2.0, 8.0, 2.0 + 571_500.0 / 609_600.0, 9.8]));
    close(
        placed[2],
        (
            1,
            [
                5.0,
                8.0,
                11.0 + 121_608.0 / 609_600.0,
                19.0 + 37_924.0 / 190_500.0,
            ],
        ),
    );
    close(
        placed[3],
        (
            1,
            [
                5.0,
                22.0,
                11.0 + 121_920.0 / 609_600.0,
                33.0 + 38_100.0 / 190_500.0,
            ],
        ),
    );
    close(
        placed[4],
        (
            1,
            [
                5.0,
                36.0,
                11.0 + 121_920.0 / 609_600.0,
                47.0 + 38_100.0 / 190_500.0,
            ],
        ),
    );
    close(
        placed[5],
        (
            1,
            [
                5.0,
                50.0,
                11.0 + 121_920.0 / 609_600.0,
                61.0 + 38_100.0 / 190_500.0,
            ],
        ),
    );
    close(placed[6], (2, [0.0, 2.0, 571_500.0 / 609_600.0, 3.8]));

    // The sheet's page is its own extent in cells, pictures included: the
    // lowest drawing is the JP2 container, which ends at row 61.2.
    let size = &doc["pages"]["1"]["size"];
    assert!(
        (size["width"].as_f64().unwrap() - 11.2).abs() < 1e-9,
        "{size:#?}"
    );
    assert!(
        (size["height"].as_f64().unwrap() - 61.2).abs() < 1e-9,
        "{size:#?}"
    );

    // Each picture in turn: what it was handed on as, and the file beside the
    // JSON held to the pixels the workbook's part decodes to. A JPEG is carried
    // through, so its file is the part itself; the rest are written back as
    // PNGs. The `.jpc` is a bare codestream, which `image` decodes only through
    // the image parser's registration, so its pixels are taken the way the CLI
    // takes them.
    let bmp = fixture("images/chart.bmp");
    let gif = fixture("images/chart.gif");
    let png = fixture("invoice_scan.png");
    let jpeg = fixture("images/scan.jpg");
    let jpc = fixture("images/scan.jpc");
    let jpc_png = pdfextractor_image_parser::transcode_to_png(&jpc)
        .expect("the JPEG 2000 codestream decodes")
        .data;
    let jp2 = fixture("images/scan.jp2");
    let jp2_png = pdfextractor_image_parser::transcode_to_png(&jp2)
        .expect("the JP2 container decodes")
        .data;
    for (picture, (source, pixels_from, mime, written_as)) in pictures.iter().zip([
        (bmp.as_slice(), bmp.as_slice(), "image/png", "png"),
        (gif.as_slice(), gif.as_slice(), "image/png", "png"),
        (png.as_slice(), png.as_slice(), "image/png", "png"),
        (jpeg.as_slice(), jpeg.as_slice(), "image/jpeg", "jpg"),
        (jpc.as_slice(), jpc_png.as_slice(), "image/png", "png"),
        (jp2.as_slice(), jp2_png.as_slice(), "image/png", "png"),
        (bmp.as_slice(), bmp.as_slice(), "image/png", "png"),
    ]) {
        let image = &picture["image"];
        assert_eq!(image["mimetype"], mime, "{image:#?}");
        let uri = image["uri"].as_str().expect("an image uri");
        assert!(
            uri.ends_with(&format!(".{written_as}")) && uri.contains("_images/"),
            "the picture was written as {uri}"
        );
        let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        if written_as == "jpg" {
            // A JPEG is carried through as itself, so the file is the part.
            assert_eq!(written, source, "{uri} is not the JPEG the workbook holds");
            continue;
        }
        let drawn = image::load_from_memory(&written)
            .unwrap_or_else(|e| panic!("{uri} does not decode: {e}"))
            .to_rgb8();
        let expected = image::load_from_memory(pixels_from)
            .unwrap_or_else(|e| panic!("the fixture's picture does not decode: {e}"))
            .to_rgb8();
        assert_eq!(
            drawn.dimensions(),
            expected.dimensions(),
            "{uri} is not the picture's grid"
        );
        assert!(
            drawn.as_raw() == expected.as_raw(),
            "{uri} does not hold the picture's own pixels"
        );
        assert_eq!(
            image["size"]["width"].as_f64(),
            Some(expected.width() as f64),
            "{image:#?}"
        );
        assert_eq!(
            image["size"]["height"].as_f64(),
            Some(expected.height() as f64),
            "{image:#?}"
        );
    }

    // A PNG is not re-encoded: the file is the part's own bytes.
    let kept = std::fs::read(ws.join(pictures[2]["image"]["uri"].as_str().expect("an image uri")))
        .expect("the written PNG");
    assert_eq!(
        kept, png,
        "a PNG was decoded and re-encoded rather than carried through"
    );

    // The render names all seven files, so a picture the model holds is one a
    // reader can see.
    let html = std::fs::read_to_string(output.with_extension("html")).expect("the HTML");
    assert_eq!(
        html.matches("<img src=\"").count(),
        7,
        "a picture is missing from the render:\n{html}"
    );
}

/// A chart a sheet draws is a drawing of data, not an image part, so a workbook
/// converts it the way a deck converts one: a `picture` item classified as the
/// chart it is, holding the numbers it plots as its `tabular_chart`, with the
/// chart's own title as a `caption` item and **no image** written for it. The
/// fixture is written by `openpyxl` (see `test-data/make_chart_sheet_xlsx.py`),
/// which leaves the numbers in the cells and only references in the chart part —
/// so the data reaching the document is the workbook's own, read through the
/// references rather than taken from a cache the writer never made.
#[test]
fn a_workbook_chart_is_its_data_rather_than_a_picture() {
    let ws = Workspace::new("xlsx-chart");
    let path = repository_root().join("test-data/chart_sheet.xlsx");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    let input = ws.write("chart_sheet.xlsx", &bytes);
    let output = ws.join("chart_sheet.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(pictures.len(), 2, "both sheets' charts: {pictures:#?}");

    // The bar chart: which family it is, and the numbers it plots.
    let bar = &pictures[0];
    assert_eq!(
        bar["meta"]["classification"]["predictions"][0]["class_name"],
        "bar_chart"
    );
    let data = &bar["meta"]["tabular_chart"]["chart_data"];
    assert_eq!(data["num_rows"], 4, "{data:#?}");
    assert_eq!(data["num_cols"], 3, "{data:#?}");
    let cells: Vec<(usize, usize, &str)> = data["table_cells"]
        .as_array()
        .expect("the chart's cells")
        .iter()
        .map(|cell| {
            (
                cell["start_row_offset_idx"].as_u64().unwrap() as usize,
                cell["start_col_offset_idx"].as_u64().unwrap() as usize,
                cell["text"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    // A header row naming the series over one column each, then the categories
    // down the first column with the plotted numbers beside them.
    let wanted = [
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
    ];
    for cell in wanted {
        assert!(cells.contains(&cell), "{cell:?} is missing: {cells:?}");
    }

    // The title is the caption, and the picture points at it.
    let caption = bar["captions"][0]["$ref"].as_str().expect("a caption ref");
    let texts = doc["texts"].as_array().expect("a text list");
    let caption_item = texts
        .iter()
        .find(|item| item["self_ref"] == caption)
        .expect("the caption item");
    assert_eq!(caption_item["text"], "Revenue by region");
    assert_eq!(caption_item["label"], "caption");

    // No image: a chart is not a file, and nothing was written for it.
    assert!(bar["image"].is_null(), "{bar:#?}");
    assert_eq!(bar["prov"][0]["page_no"], 1);
    assert_eq!(bar["prov"][0]["bbox"]["l"], 4.0);

    // The second sheet's chart has no title, so no caption — and it is still a
    // picture of its data rather than nothing.
    let line = &pictures[1];
    assert_eq!(
        line["meta"]["classification"]["predictions"][0]["class_name"],
        "line_chart"
    );
    assert_eq!(line["meta"]["tabular_chart"]["chart_data"]["num_cols"], 2);
    assert!(line["captions"]
        .as_array()
        .is_none_or(|list| list.is_empty()));

    // The render says the caption and writes no image for either chart.
    let markdown = std::fs::read_to_string(ws.join("chart_sheet.md")).expect("the Markdown");
    assert!(
        markdown.contains("Revenue by region"),
        "the chart's title is not in the Markdown:\n{markdown}"
    );
    assert!(
        !markdown.contains("!["),
        "the Markdown links a chart as a picture:\n{markdown}"
    );
    let html = std::fs::read_to_string(ws.join("chart_sheet.html")).expect("the HTML");
    assert!(
        !html.contains("<img"),
        "the HTML shows a chart as an image:\n{html}"
    );
    assert!(
        !ws.join("chart_sheet_images").exists(),
        "an image was written for a chart"
    );
}

/// A deck's slides become pages, `chapter` groups and the shapes on them: the
/// shape Docling's own PPTX backend produces, read out of the JSON the binary
/// wrote.
#[test]
fn a_presentation_becomes_pages_groups_and_shapes() {
    let ws = Workspace::new("pptx");
    let fixture = repository_root().join("test-data/q3_review.pptx");
    let bytes =
        std::fs::read(&fixture).unwrap_or_else(|e| panic!("missing {}: {e}", fixture.display()));
    let input = ws.write("q3_review.pptx", &bytes);
    let output = ws.join("q3_review.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);

    let doc = json_of(&output);
    assert_eq!(doc["name"], "q3_review");
    assert_eq!(
        doc["origin"]["mimetype"],
        "application/vnd.openxmlformats-officedocument.presentationml.presentation"
    );
    assert_eq!(doc["origin"]["filename"], "q3_review.pptx");

    // A slide is a page, and the page is the deck's own size in EMU.
    let sizes: Vec<(f64, f64)> = ["1", "2", "3"]
        .iter()
        .map(|page| {
            let size = &doc["pages"][page]["size"];
            (
                size["width"].as_f64().unwrap(),
                size["height"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(sizes, vec![(9144000.0, 6858000.0); 3]);

    // A chapter group per slide, listed by the body; the slide's items are the
    // group's children, not the body's. The comment groups sit beside them and
    // are the notes-layer test's subject.
    let groups = doc["groups"].as_array().expect("a group list");
    let described: Vec<(&str, &str, &str)> = groups
        .iter()
        .filter(|group| group["label"] == "chapter")
        .map(|group| {
            (
                group["name"].as_str().unwrap_or_default(),
                group["label"].as_str().unwrap_or_default(),
                group["content_layer"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        described,
        vec![
            ("slide-0", "chapter", "body"),
            ("slide-1", "chapter", "body"),
            ("slide-2", "chapter", "body"),
        ]
    );
    // The body lists the slides and the comments in the order the deck is read:
    // slide 1, the two comments on it, then the other two slides and the last
    // comment.
    let body_children: Vec<&str> = doc["body"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|child| child["$ref"].as_str().unwrap())
        .collect();
    assert_eq!(
        body_children,
        vec![
            "#/groups/0",
            "#/groups/1",
            "#/groups/2",
            "#/groups/3",
            "#/groups/4",
            "#/groups/5"
        ]
    );

    // Titles are titles; the body placeholder's paragraphs are list items
    // because the slide *master* draws bullets, not because the slide says so;
    // the numbered text box counts itself.
    let texts = doc["texts"].as_array().expect("a text list");
    let summarised: Vec<(&str, &str, Option<&str>, Option<bool>)> = texts
        .iter()
        .filter(|text| text["content_layer"] == "body")
        .map(|text| {
            (
                text["label"].as_str().unwrap_or_default(),
                text["text"].as_str().unwrap_or_default(),
                text["marker"].as_str(),
                text["enumerated"].as_bool(),
            )
        })
        .collect();
    assert_eq!(
        summarised,
        vec![
            ("title", "Q3 Review", None, None),
            (
                "list_item",
                "Revenue grew across every region.",
                Some(""),
                Some(false)
            ),
            (
                "list_item",
                "North America led with 145 units.",
                Some(""),
                Some(false)
            ),
            (
                "list_item",
                "Europe followed at 121 units.",
                Some(""),
                Some(false)
            ),
            ("paragraph", "Prepared for the board.", None, None),
            ("title", "Regional performance", None, None),
            ("list_item", "Open the dashboard.", Some("1."), Some(true)),
            ("list_item", "Filter by region.", Some("2."), Some(true)),
            ("list_item", "Export the summary.", Some("3."), Some(true)),
            ("title", "Where the growth came from", None, None),
            (
                "paragraph",
                "Revenue grew 12% year over year (constant currency)",
                None,
                None
            ),
            // A chart's own title is a caption, not body prose.
            ("caption", "Units by region", None, None),
        ]
    );
    // Every slide item sits in its own slide's group, and on that page: the
    // group is named for the slide, whatever index the comment groups give it.
    let group_name = |reference: &str| -> String {
        let index: usize = reference
            .trim_start_matches("#/groups/")
            .parse()
            .expect("a group reference");
        doc["groups"][index]["name"]
            .as_str()
            .expect("a named group")
            .to_string()
    };
    let mut page = 1i64;
    let mut seen_on_page = 0usize;
    for text in texts.iter().filter(|text| text["content_layer"] == "body") {
        // Five items on the first slide, four on the second, three on the last
        // — the prose, the picture and the chart's title.
        let per_page = [5usize, 4, 3];
        if seen_on_page == per_page[(page - 1) as usize] {
            page += 1;
            seen_on_page = 0;
        }
        seen_on_page += 1;
        assert_eq!(text["prov"][0]["page_no"], page);
        assert_eq!(
            group_name(text["parent"]["$ref"].as_str().unwrap()),
            format!("slide-{}", page - 1),
            "{}",
            text["text"]
        );
    }

    // The graphic frame's table: four rows, three columns, and the merged
    // “Total” cell covering two of them — with no cell under it.
    let tables = doc["tables"].as_array().expect("a table list");
    assert_eq!(tables.len(), 1, "{tables:#?}");
    assert_eq!(
        group_name(tables[0]["parent"]["$ref"].as_str().unwrap()),
        "slide-1"
    );
    let data = &tables[0]["data"];
    assert_eq!(
        (data["num_rows"].as_u64(), data["num_cols"].as_u64()),
        (Some(4), Some(3))
    );
    let cells = data["table_cells"].as_array().unwrap();
    assert_eq!(cells.len(), 11, "{cells:#?}");
    let total = cells
        .iter()
        .find(|cell| cell["start_row_offset_idx"] == 3 && cell["start_col_offset_idx"] == 0)
        .expect("the merged total cell");
    assert_eq!(total["text"], "Total\n218");
    assert_eq!(total["col_span"], 2);
    for cell in cells {
        assert_eq!(cell["column_header"], cell["start_row_offset_idx"] == 0);
    }

    // The picture was reached through the slide's relationship, written beside
    // the JSON, and kept with the pixel size and the place it was drawn in.
    let pictures = doc["pictures"].as_array().expect("a picture list");
    assert_eq!(pictures.len(), 2, "{pictures:#?}");
    assert_eq!(
        group_name(pictures[0]["parent"]["$ref"].as_str().unwrap()),
        "slide-2"
    );
    assert_eq!(pictures[0]["image"]["mimetype"], "image/png");
    assert_eq!(
        (
            pictures[0]["image"]["size"]["width"].as_f64(),
            pictures[0]["image"]["size"]["height"].as_f64()
        ),
        (Some(160.0), Some(80.0))
    );
    assert_eq!(
        pictures[0]["prov"][0]["bbox"]["l"].as_f64(),
        Some(5669280.0)
    );
    let image = ws.join(pictures[0]["image"]["uri"].as_str().expect("an image uri"));
    assert!(image.is_file(), "missing {}", image.display());

    // The chart is a picture of data, not of pixels: it has no image at all,
    // says what kind of chart it is, and carries the numbers it plots. Its
    // title was added as the caption and the picture points back at it.
    let chart = pictures
        .iter()
        .find(|picture| picture.get("image").is_none())
        .expect("the chart picture");
    assert_eq!(
        group_name(chart["parent"]["$ref"].as_str().unwrap()),
        "slide-2"
    );
    assert_eq!(
        chart["meta"]["classification"]["predictions"][0]["class_name"],
        "bar_chart"
    );
    let caption_ref = chart["captions"][0]["$ref"].as_str().expect("a caption");
    let caption = &texts[caption_ref
        .trim_start_matches("#/texts/")
        .parse::<usize>()
        .expect("a text reference")];
    assert_eq!(caption["label"], "caption");
    assert_eq!(caption["text"], "Units by region");
    assert_eq!(caption["prov"][0]["page_no"], 3);

    let data = &chart["meta"]["tabular_chart"]["chart_data"];
    assert_eq!(
        (data["num_rows"].as_u64(), data["num_cols"].as_u64()),
        (Some(4), Some(3))
    );
    let cells = data["table_cells"].as_array().expect("chart cells");
    assert_eq!(cells.len(), 12, "{cells:#?}");
    let cell = |row: u64, col: u64| -> &str {
        cells
            .iter()
            .find(|cell| cell["start_row_offset_idx"] == row && cell["start_col_offset_idx"] == col)
            .unwrap_or_else(|| panic!("no chart cell at {row},{col}"))["text"]
            .as_str()
            .expect("cell text")
    };
    // A header row names the series; the categories run down the first column.
    // The missing Q3 point is a gap, not a zero, and its row keeps its label.
    assert_eq!(
        [
            cell(0, 0),
            cell(0, 1),
            cell(0, 2),
            cell(1, 0),
            cell(1, 1),
            cell(1, 2),
            cell(3, 0),
            cell(3, 2),
        ],
        [
            "",
            "Q2",
            "Q3",
            "North America",
            "120",
            "145",
            "Asia Pacific",
            ""
        ]
    );
    for cell in cells {
        assert_eq!(cell["column_header"], cell["start_row_offset_idx"] == 0);
        assert_eq!(
            cell["row_header"],
            cell["start_col_offset_idx"] == 0 && cell["start_row_offset_idx"] != 0
        );
    }
}

/// A deck's speaker notes and its comments are the notes layer: read from the
/// notes part and the comments part, kept out of the slide's own content, and
/// recorded the way Docling records them — the notes on their slide's page, a
/// comment in a `comment_section` group of its own with what the deck says
/// about it in front of the words.
#[test]
fn speaker_notes_and_comments_are_the_notes_layer() {
    let ws = Workspace::new("pptx-notes");
    let fixture = repository_root().join("test-data/q3_review.pptx");
    let bytes = std::fs::read(&fixture).expect("the deck fixture");
    let input = ws.write("q3_review.pptx", &bytes);
    let output = ws.join("q3_review.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let texts = doc["texts"].as_array().expect("a text list");
    let notes: Vec<&serde_json::Value> = texts
        .iter()
        .filter(|text| text["content_layer"] == "notes")
        .collect();
    let words: Vec<&str> = notes
        .iter()
        .map(|note| note["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        words,
        vec![
            "Open with the revenue line, then the regions.\nThe board asked for the unit numbers first.",
            "[author: Ada Lovelace (AL), time: 2026-10-18T09:15:00.000]: \
             Check the Q3 numbers against the ledger.",
            "[author: Grace Hopper (GH), time: 2026-10-18T10:30:00.000]: \
             Add the regional split to slide 2.",
            "Say the picture is the growth by quarter; do not read the table aloud.",
            "[author: Grace Hopper (GH), time: 2026-10-18T16:05:00.000]: Ship this one.",
        ],
        "{notes:#?}"
    );
    // Every one of them is a `text` item, and every one is on the notes layer:
    // a deck's own remarks are not slide content.
    for note in &notes {
        assert_eq!(note["label"], "text");
    }
    assert!(
        !texts.iter().any(|text| text["content_layer"] == "body"
            && text["text"]
                .as_str()
                .is_some_and(|text| text.contains("Ada Lovelace"))),
        "a comment was read as slide content"
    );

    // The notes are the slide's: its page, its group, and the empty box Docling
    // gives them (a note is nowhere on the slide).
    assert_eq!(notes[0]["prov"][0]["page_no"], 1);
    assert_eq!(notes[0]["prov"][0]["bbox"]["r"], 0.0);
    assert_eq!(notes[0]["prov"][0]["charspan"][1], 89);
    assert_eq!(notes[3]["prov"][0]["page_no"], 3);
    let slide_group = |reference: &str| -> String {
        let index: usize = reference
            .trim_start_matches("#/groups/")
            .parse()
            .expect("a group reference");
        doc["groups"][index]["name"].as_str().unwrap().to_string()
    };
    assert_eq!(
        slide_group(notes[0]["parent"]["$ref"].as_str().unwrap()),
        "slide-0"
    );
    assert_eq!(
        slide_group(notes[3]["parent"]["$ref"].as_str().unwrap()),
        "slide-2"
    );

    // A comment is about the slide rather than a place on it: it has no
    // provenance, and it sits in its own `comment_section` group under the
    // body, named after the slide and the comment's own id.
    let groups = doc["groups"].as_array().expect("a group list");
    let comment_groups: Vec<(&str, &str)> = groups
        .iter()
        .filter(|group| group["label"] == "comment_section")
        .map(|group| {
            (
                group["name"].as_str().unwrap(),
                group["content_layer"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        comment_groups,
        vec![
            ("comment-slide1-1", "notes"),
            ("comment-slide1-2", "notes"),
            ("comment-slide3-1", "notes"),
        ]
    );
    // Each comment is the only child of its group, the group names the body,
    // and the body lists it: one comment is one node of the document.
    let listed_by_body = |reference: &str| -> bool {
        doc["body"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .any(|child| child["$ref"] == reference)
    };
    for name in ["comment-slide1-1", "comment-slide1-2", "comment-slide3-1"] {
        let index = groups
            .iter()
            .position(|group| group["name"] == name)
            .unwrap_or_else(|| panic!("no group named {name}"));
        let group = &groups[index];
        assert_eq!(group["parent"]["$ref"], "#/body");
        assert!(listed_by_body(&format!("#/groups/{index}")));
        let children = group["children"].as_array().unwrap();
        assert_eq!(children.len(), 1, "one item per comment: {group:#?}");
        let item = children[0]["$ref"].as_str().unwrap();
        let number: usize = item.trim_start_matches("#/texts/").parse().unwrap();
        assert_eq!(texts[number]["parent"]["$ref"], format!("#/groups/{index}"));
        assert_eq!(texts[number]["content_layer"], "notes");
        assert!(
            texts[number]["prov"].as_array().unwrap().is_empty(),
            "a comment is nowhere on the slide: {}",
            texts[number]
        );
    }

    // A reading has no use for either: the Markdown of the deck is its slides.
    let markdown = ws.join("q3_review.md");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        markdown.to_str().unwrap(),
        "--to",
        "md",
        "--ocr",
        "never",
    ]);
    let markdown = std::fs::read_to_string(&markdown).expect("the Markdown");
    for absent in ["Ada Lovelace", "Ship this one", "Say the picture"] {
        assert!(
            !markdown.contains(absent),
            "{absent} reached the Markdown:\n{markdown}"
        );
    }
    assert!(markdown.contains("Q3 Review"), "{markdown}");
}

/// `--include-notes` puts the notes layer into all three prose writers. Off,
/// each is a reading of the slides alone; on, the deck's remarks join it, and
/// the slides themselves are unchanged either way.
#[test]
fn notes_join_the_reading_when_asked() {
    let ws = Workspace::new("pptx-notes-export");
    let fixture = repository_root().join("test-data/q3_review.pptx");
    let bytes = std::fs::read(&fixture).expect("the deck fixture");
    let input = ws.write("q3_review.pptx", &bytes);
    let input = input.to_str().unwrap();

    let plain = ws.join("plain.json");
    run_ok(&[
        "-i",
        input,
        "-o",
        plain.to_str().unwrap(),
        "--to",
        "md",
        "--to",
        "html",
        "--to",
        "text",
        "--ocr",
        "never",
    ]);
    let with_notes = ws.join("with-notes.json");
    run_ok(&[
        "-i",
        input,
        "-o",
        with_notes.to_str().unwrap(),
        "--to",
        "md",
        "--to",
        "html",
        "--to",
        "text",
        "--include-notes",
        "--ocr",
        "never",
    ]);

    for extension in ["md", "html", "txt"] {
        let read = |stem: &str| {
            let path = ws.join(&format!("{stem}.{extension}"));
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("missing {}: {e}", path.display()))
        };
        let slides_only = read("plain");
        let everything = read("with-notes");

        for reading in [&slides_only, &everything] {
            assert!(
                reading.contains("Q3 Review"),
                "{extension}: the slides left the reading:\n{reading}"
            );
        }
        for absent in [
            "Open with the revenue line",
            "Ada Lovelace",
            "Say the picture",
        ] {
            assert!(
                !slides_only.contains(absent),
                "{extension}: {absent} reached the slide-only reading:\n{slides_only}"
            );
            assert!(
                everything.contains(absent),
                "{extension}: {absent} is missing from the reading with notes:\n{everything}"
            );
        }

        // The included remarks are marked as remarks, so a reader can tell
        // them from the slide: a labelled quote in Markdown, an aside in
        // HTML — and plain text stays plain.
        match extension {
            "md" => {
                assert!(
                    everything.contains("> **Notes:**"),
                    "{extension}: the speaker notes are not marked:\n{everything}"
                );
                // A comment has its own label, so it is not read as one more
                // thing the presenter said.
                assert!(
                    everything.contains("> **Comment:**"),
                    "{extension}: the comments are not marked as comments:\n{everything}"
                );
                assert!(
                    !slides_only.contains("**Notes:**") && !slides_only.contains("**Comment:**"),
                    "{extension}: a slide-only reading grew a marker:\n{slides_only}"
                );
            }
            "html" => {
                assert!(
                    everything.contains("<aside class=\"notes\">")
                        && everything.contains("<strong>Notes:</strong>"),
                    "{extension}: the speaker notes are not marked:\n{everything}"
                );
                assert!(
                    everything.contains("<aside class=\"notes comment\">")
                        && everything.contains("<strong>Comment:</strong>"),
                    "{extension}: the comments are not marked as comments:\n{everything}"
                );
                assert!(
                    !slides_only.contains("class=\"notes"),
                    "{extension}: a slide-only reading grew a marker:\n{slides_only}"
                );
            }
            _ => assert!(
                !everything.contains("Notes:"),
                "{extension}: plain text grew a marker:\n{everything}"
            ),
        }
    }

    // The same flag governs the stdout form, which is a different call path.
    let out = run_ok(&[
        "-i",
        input,
        "--to",
        "md",
        "-o",
        "-",
        "--include-notes",
        "--ocr",
        "never",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Open with the revenue line") && stdout.contains("Ada Lovelace"),
        "stdout omitted the notes: {stdout}"
    );
}

/// Markdown out with the notes, then Markdown back in: a labelled quote is the
/// deck's remark, and a reader that took it for prose would make it a slide.
/// It has to come back on the notes layer, and rendering it again has to be
/// the same Markdown rather than a document that grew a paragraph.
///
/// The notes alone are not enough: a file that flattened every slide into one
/// list would still keep them. The writer's group markers have to bring the
/// deck's grouping back exactly — each slide's words and its notes in the
/// slide's `chapter` group, each comment in its own `comment_section` group,
/// with the names and layers the deck gave them.
#[test]
fn notes_survive_the_markdown_round_trip() {
    let ws = Workspace::new("pptx-notes-round-trip");
    let fixture = repository_root().join("test-data/q3_review.pptx");
    let bytes = std::fs::read(&fixture).expect("the deck fixture");
    let input = ws.write("q3_review.pptx", &bytes);

    let deck_md = ws.join("deck.md");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        deck_md.to_str().unwrap(),
        "--to",
        "md",
        "--include-notes",
        "--ocr",
        "never",
    ]);
    // The deck itself, for the grouping the Markdown has to carry: its slides
    // and each comment's own section, as the PPTX reader built them.
    let native_output = ws.join("native.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        native_output.to_str().unwrap(),
        "--ocr",
        "never",
    ]);
    let first = std::fs::read_to_string(&deck_md).expect("the deck's Markdown");
    assert!(
        first.contains("> **Notes:**"),
        "the deck has no marked remarks to read back:\n{first}"
    );

    // The Markdown is read under another stem: its own outputs go beside it,
    // and writing them over the input would compare the file with itself.
    let source = ws.write("source.md", first.as_bytes());
    let output = ws.join("round_trip.json");
    run_ok(&[
        "-i",
        source.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--include-notes",
        "--ocr",
        "never",
    ]);

    // What each group holds: its own identity, the layer it states, and its
    // members' words — every text, and a marker for each table and picture. A
    // picture with no image is not a marker: the Markdown writer cannot point
    // at one, so the round trip is not expected to bring it back (the deck's
    // chart is one), while an image-backed picture has to.
    let grouping = |document: &serde_json::Value| -> Vec<serde_json::Value> {
        document["groups"]
            .as_array()
            .expect("a group list")
            .iter()
            .map(|group| {
                let members: Vec<String> = group["children"]
                    .as_array()
                    .expect("group children")
                    .iter()
                    .map(|child| {
                        let path = child["$ref"].as_str().expect("a reference");
                        let (kind, index) = path
                            .strip_prefix("#/")
                            .expect("a document reference")
                            .split_once('/')
                            .expect("a kind and an index");
                        let item = &document[kind][index.parse::<usize>().expect("an index")];
                        match kind {
                            "texts" => item["text"].as_str().unwrap_or_default().to_string(),
                            "tables" => "table".to_string(),
                            "pictures" if !item["image"].is_null() => "picture".to_string(),
                            _ => String::new(),
                        }
                    })
                    .filter(|member| !member.is_empty())
                    .collect();
                serde_json::json!({
                    "label": group["label"],
                    "name": group["name"],
                    "layer": group["content_layer"],
                    "members": members,
                })
            })
            .collect()
    };

    let doc = json_of(&output);
    let native = json_of(&native_output);
    assert_eq!(
        grouping(&doc),
        grouping(&native),
        "the slides and the comments are no longer grouped the way the deck had them"
    );
    let texts = doc["texts"].as_array().expect("a text list");
    let notes: Vec<&str> = texts
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .filter_map(|item| item["text"].as_str())
        .collect();
    for remark in [
        "Open with the revenue line",
        "Ada Lovelace",
        "Say the picture",
    ] {
        assert!(
            notes.iter().any(|note| note.contains(remark)),
            "{remark} did not come back on the notes layer: {notes:#?}"
        );
        assert!(
            !texts.iter().any(|item| {
                item["content_layer"] == "body"
                    && item["text"]
                        .as_str()
                        .is_some_and(|text| text.contains(remark))
            }),
            "{remark} came back as slide content: {texts:#?}"
        );
    }

    // The remark lines alone are compared: the pictures are copied to a new
    // directory and the deck's list markers are re-read by a separate rule, so
    // the whole file is not expected to be byte-identical here.
    let note_lines = |markdown: &str| -> Vec<String> {
        markdown
            .lines()
            .filter(|line| line.starts_with("> **Notes:**") || line.starts_with("> **Comment:**"))
            .map(str::to_string)
            .collect()
    };
    let first_notes = note_lines(&first);
    assert!(!first_notes.is_empty(), "no remarks to follow: {first}");
    let second =
        std::fs::read_to_string(ws.join("round_trip.md")).expect("the re-rendered Markdown");
    assert_eq!(
        first_notes,
        note_lines(&second),
        "the remarks changed on the way back through Markdown"
    );
}

/// HTML out with the notes, then HTML back in: `<aside class="notes">` is the
/// writer's mark, and a reader that took the aside for prose would make the
/// deck's remarks paragraphs of the document. They have to return on the notes
/// layer, and rendering them again has to be the same asides.
#[test]
fn notes_survive_the_html_round_trip() {
    let ws = Workspace::new("pptx-notes-html-round-trip");
    let fixture = repository_root().join("test-data/q3_review.pptx");
    let bytes = std::fs::read(&fixture).expect("the deck fixture");
    let input = ws.write("q3_review.pptx", &bytes);

    let deck_html = ws.join("deck.html");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        deck_html.to_str().unwrap(),
        "--to",
        "html",
        "--include-notes",
        "--ocr",
        "never",
    ]);
    let first = std::fs::read_to_string(&deck_html).expect("the deck's HTML");

    // The HTML is read under another stem, so its own outputs do not overwrite
    // the file being read.
    let source = ws.write("source.html", first.as_bytes());
    let output = ws.join("round_trip.json");
    run_ok(&[
        "-i",
        source.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--include-notes",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    let texts = doc["texts"].as_array().expect("a text list");
    let notes: Vec<&str> = texts
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .filter_map(|item| item["text"].as_str())
        .collect();
    for remark in [
        "Open with the revenue line",
        "Ada Lovelace",
        "Say the picture",
    ] {
        assert!(
            notes.iter().any(|note| note.contains(remark)),
            "{remark} did not come back on the notes layer: {notes:#?}"
        );
        assert!(
            !texts.iter().any(|item| {
                item["content_layer"] == "body"
                    && item["text"]
                        .as_str()
                        .is_some_and(|text| text.contains(remark))
            }),
            "{remark} came back as slide content: {texts:#?}"
        );
    }

    // The aside blocks alone are compared: the images are copied to a new
    // directory and list markers are re-read by a separate rule, so the whole
    // file is not expected to be byte-identical here.
    let asides = |html: &str| -> Vec<String> {
        let lines: Vec<&str> = html.lines().collect();
        lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.trim().starts_with("<aside class=\"notes"))
            .map(|(i, _)| {
                let end = lines[i..]
                    .iter()
                    .position(|line| line.trim() == "</aside>")
                    .map(|offset| i + offset + 1)
                    .unwrap_or(lines.len());
                lines[i..end].join("\n")
            })
            .collect()
    };
    let first_asides = asides(&first);
    assert!(!first_asides.is_empty(), "no remarks to follow:\n{first}");
    let second = std::fs::read_to_string(ws.join("round_trip.html")).expect("the re-rendered HTML");
    assert_eq!(
        first_asides,
        asides(&second),
        "the remarks changed on the way back through HTML"
    );
}

/// A page's head says more about it than its title: the description a search
/// engine shows, and the URL the page calls canonical. Neither is content, and
/// Docling has no field for either — so they travel beside the model, the JSON
/// never names them, and the HTML writer puts them back. That is what makes a
/// page read as HTML and written as HTML the page it was.
#[test]
fn a_pages_description_and_canonical_url_survive_the_html_round_trip() {
    let ws = Workspace::new("html-head-round-trip");
    let page = ws.write(
        "page.html",
        b"<html><head><title>Q3 report</title>\n\
          <meta name=\"description\" content=\"Q3 &amp; costs, in short.\">\n\
          <link rel=\"canonical\" href=\"https://example.com/reports/q3\">\n\
          </head><body><h1>Q3 report</h1><p>Revenue rose.</p></body></html>",
    );

    let first = ws.join("first.html");
    run_ok(&[
        "-i",
        page.to_str().unwrap(),
        "-o",
        first.to_str().unwrap(),
        "--to",
        "html",
    ]);
    let written = std::fs::read_to_string(&first).expect("the page written from HTML");
    assert!(
        written.contains("<meta name=\"description\" content=\"Q3 &amp; costs, in short.\">"),
        "the description was lost: {written}"
    );
    assert!(
        written.contains("<link rel=\"canonical\" href=\"https://example.com/reports/q3\">"),
        "the canonical URL was lost: {written}"
    );

    // The document in between does not carry them: they are not Docling's
    // fields, so a consumer reading the JSON sees the document Docling itself
    // would make of the page — as it does for the words of a quotation.
    let json_path = ws.join("page.json");
    run_ok(&[
        "-i",
        page.to_str().unwrap(),
        "-o",
        json_path.to_str().unwrap(),
    ]);
    let doc = json_of(&json_path).to_string();
    assert!(
        !doc.contains("canonical") && !doc.contains("costs, in short"),
        "a head fact reached the Docling document: {doc}"
    );

    // Read back and written again: the same head, so the same page byte for
    // byte.
    let source = ws.write("source.html", written.as_bytes());
    let second = ws.join("second.html");
    run_ok(&[
        "-i",
        source.to_str().unwrap(),
        "-o",
        second.to_str().unwrap(),
        "--to",
        "html",
    ]);
    let again = std::fs::read_to_string(&second).expect("the page written again");
    assert_eq!(
        again, written,
        "the head changed on the way back through HTML"
    );
}

/// A page's head also states what a link to it looks like: Open Graph's keys in
/// `property`, the Twitter card's in `name`. None of it is content, none of it is
/// Docling's, and none of it may be lost — a page read as HTML and written as
/// HTML keeps the preview it came with, down to the order a card's image and
/// that image's width were stated in.
#[test]
fn a_pages_social_cards_survive_the_html_round_trip() {
    let ws = Workspace::new("html-cards-round-trip");
    let page = ws.write(
        "page.html",
        b"<html><head><title>Q3 report</title>\n\
          <meta name=\"description\" content=\"Q3 &amp; costs, in short.\">\n\
          <meta property=\"og:title\" content=\"Q3 &amp; costs\">\n\
          <meta property=\"og:image\" content=\"https://example.com/q3.png\">\n\
          <meta property=\"og:image:width\" content=\"1200\">\n\
          <meta name=\"twitter:card\" content=\"summary_large_image\">\n\
          <meta name=\"twitter:site\" content=\"@example\">\n\
          </head><body><h1>Q3 report</h1><p>Revenue rose.</p></body></html>",
    );

    let first = ws.join("first.html");
    run_ok(&[
        "-i",
        page.to_str().unwrap(),
        "-o",
        first.to_str().unwrap(),
        "--to",
        "html",
    ]);
    let written = std::fs::read_to_string(&first).expect("the page written from HTML");
    for line in [
        "<meta property=\"og:title\" content=\"Q3 &amp; costs\">",
        "<meta property=\"og:image\" content=\"https://example.com/q3.png\">",
        "<meta property=\"og:image:width\" content=\"1200\">",
        "<meta name=\"twitter:card\" content=\"summary_large_image\">",
        "<meta name=\"twitter:site\" content=\"@example\">",
    ] {
        assert!(written.contains(line), "lost {line}: {written}");
    }
    // The image's width stays the line after the image it describes: a card's
    // statements are read in order.
    let image = written.find("og:image\"").expect("the image");
    let width = written.find("og:image:width").expect("the width");
    assert!(image < width, "the width moved above its image: {written}");

    // The document in between does not carry a link preview: a consumer reading
    // the JSON sees the document Docling itself would make of the page.
    let json_path = ws.join("page.json");
    run_ok(&[
        "-i",
        page.to_str().unwrap(),
        "-o",
        json_path.to_str().unwrap(),
    ]);
    let doc = json_of(&json_path).to_string();
    assert!(
        !doc.contains("og:title") && !doc.contains("twitter:card"),
        "a card reached the Docling document: {doc}"
    );

    // Read back and written again: the same head, so the same page byte for
    // byte.
    let source = ws.write("source.html", written.as_bytes());
    let second = ws.join("second.html");
    run_ok(&[
        "-i",
        source.to_str().unwrap(),
        "-o",
        second.to_str().unwrap(),
        "--to",
        "html",
    ]);
    let again = std::fs::read_to_string(&second).expect("the page written again");
    assert_eq!(
        again, written,
        "the cards changed on the way back through HTML"
    );
}

/// Markdown has no head, so a page's head facts travel as YAML front matter: the
/// writer states them there, the reader puts them back beside the model, and the
/// page written from that Markdown has the same head as the original. A second
/// Markdown rendering must be the first, so the front matter is a fixed point
/// with the rest of the file.
#[test]
fn a_pages_social_cards_survive_the_markdown_round_trip() {
    let ws = Workspace::new("markdown-cards-round-trip");
    let page = ws.write("page.html", &fixture("social_cards.html"));
    let markdown = ws.join("page.md");
    run_ok(&[
        "-i",
        page.to_str().unwrap(),
        "-o",
        markdown.to_str().unwrap(),
        "--to",
        "md",
        "--ocr",
        "never",
        "--include-notes",
    ]);
    let written = std::fs::read_to_string(&markdown).expect("the Markdown written");
    for line in [
        "---\n",
        "description: Quarterly results & outlook\n",
        "canonical: https://example.com/reports/q3?quarter=3&region=eu\n",
        "social:\n",
        "- key: og:title\n  content: Q3 & costs\n",
        "- key: og:image\n  content: https://example.com/cards/q3.png\n",
        "- key: twitter:card\n  content: summary_large_image\n",
    ] {
        assert!(written.contains(line), "lost {line:?}: {written}");
    }

    // Writing the Markdown again is the same file: the front matter reads back
    // into the document instead of becoming prose.
    let again = ws.join("again.md");
    run_ok(&[
        "-i",
        markdown.to_str().unwrap(),
        "-o",
        again.to_str().unwrap(),
        "--to",
        "md",
        "--ocr",
        "never",
        "--include-notes",
    ]);
    assert_eq!(
        std::fs::read_to_string(&again).expect("the second Markdown"),
        written,
        "the head facts changed on the way back through Markdown"
    );

    // The page written from the Markdown has the head the original page had.
    let from_markdown = ws.join("from-markdown.html");
    run_ok(&[
        "-i",
        markdown.to_str().unwrap(),
        "-o",
        from_markdown.to_str().unwrap(),
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    let direct = ws.join("direct.html");
    run_ok(&[
        "-i",
        page.to_str().unwrap(),
        "-o",
        direct.to_str().unwrap(),
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    assert_eq!(
        std::fs::read_to_string(&from_markdown).expect("the HTML from Markdown"),
        std::fs::read_to_string(&direct).expect("the HTML from the page"),
        "the head changed on the way from HTML through Markdown"
    );
}

/// A page's head can be cards and nothing else: no description, no canonical
/// URL. What travels as YAML front matter is then the card list alone, and the
/// reader has to take it without the fields beside it — an empty `description:`
/// or a canonical URL the page never named would be a statement it never made,
/// and it would show up in the page written from the Markdown.
#[test]
fn a_page_whose_head_is_only_social_cards_keeps_them_through_markdown() {
    let ws = Workspace::new("markdown-cards-only");
    let page = ws.write(
        "page.html",
        b"<html><head><title>Release notes</title>\n\
          <meta property=\"og:title\" content=\"Release notes &amp; changes\">\n\
          <meta property=\"og:type\" content=\"article\">\n\
          <meta property=\"og:image\" content=\"https://example.com/notes.png\">\n\
          <meta property=\"og:image:alt\" content=\"A page of notes\">\n\
          <meta name=\"twitter:card\" content=\"summary\">\n\
          <meta name=\"twitter:site\" content=\"@example\">\n\
          </head><body><h1>Release notes</h1><p>Two changes.</p></body></html>",
    );

    let markdown = ws.join("page.md");
    run_ok(&[
        "-i",
        page.to_str().unwrap(),
        "-o",
        markdown.to_str().unwrap(),
        "--to",
        "md",
        "--ocr",
        "never",
    ]);
    let written = std::fs::read_to_string(&markdown).expect("the Markdown written");
    let (front, body) = written
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n\n"))
        .expect("a front matter and a blank line before the body");
    assert_eq!(body, "# Release notes\n\nTwo changes.\n");
    // Only the cards: the page stated neither of the two fields beside them.
    for absent in ["description:", "canonical:"] {
        assert!(
            !front.lines().any(|line| line.starts_with(absent)),
            "the front matter names {absent} though the page never stated it:\n{front}"
        );
    }
    assert!(
        front.starts_with("social:\n- key: og:title\n  content: Release notes & changes\n"),
        "{front}"
    );
    // Order is part of what the head said: the alt stays after its image, the
    // Twitter keys after the Open Graph ones — and a value YAML would read as
    // something else is quoted on the way out (`@example` is not a plain
    // scalar).
    for card in [
        "- key: og:image\n  content: https://example.com/notes.png\n",
        "- key: og:image:alt\n  content: A page of notes\n",
        "- key: twitter:card\n  content: summary\n",
        // The block ends at the closing `---`, so the last card carries no
        // trailing newline of its own.
        "- key: twitter:site\n  content: '@example'",
    ] {
        assert!(front.contains(card), "lost {card:?}:\n{front}");
    }
    let image = front.find("og:image\n").expect("the image");
    let alt = front.find("og:image:alt\n").expect("its alt");
    assert!(image < alt, "the alt moved above its image:\n{front}");

    // Reading it back renders the same file: the cards are the document's head
    // rather than a paragraph at the top of the page.
    let again = ws.join("again.md");
    run_ok(&[
        "-i",
        markdown.to_str().unwrap(),
        "-o",
        again.to_str().unwrap(),
        "--to",
        "md",
        "--ocr",
        "never",
    ]);
    assert_eq!(
        std::fs::read_to_string(&again).expect("the second Markdown"),
        written,
        "the cards changed on the way back through Markdown"
    );

    // The page written from the Markdown has the head the original stated — the
    // cards in `property` and `name` — and none of the lines the page never had.
    let from_markdown = ws.join("from-markdown.html");
    run_ok(&[
        "-i",
        markdown.to_str().unwrap(),
        "-o",
        from_markdown.to_str().unwrap(),
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    let head = std::fs::read_to_string(&from_markdown).expect("the HTML from the Markdown");
    for line in [
        "<meta property=\"og:title\" content=\"Release notes &amp; changes\">",
        "<meta property=\"og:image:alt\" content=\"A page of notes\">",
        "<meta name=\"twitter:card\" content=\"summary\">",
        "<meta name=\"twitter:site\" content=\"@example\">",
    ] {
        assert!(head.contains(line), "lost {line}: {head}");
    }
    assert!(
        !head.contains("name=\"description\"") && !head.contains("rel=\"canonical\""),
        "the page grew a head line it never had: {head}"
    );

    // Byte for byte the page the original gives, so nothing else moved either.
    let direct = ws.join("direct.html");
    run_ok(&[
        "-i",
        page.to_str().unwrap(),
        "-o",
        direct.to_str().unwrap(),
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    assert_eq!(
        head,
        std::fs::read_to_string(&direct).expect("the HTML from the page"),
        "the head changed on the way from HTML through Markdown"
    );
}

/// An ordinary block quote is a quotation, not merely the same words a
/// paragraph would have: Markdown says so with `>`, HTML with `<blockquote>`.
/// Reading a page has to keep the fact and writing one has to put it back —
/// while the JSON, which is Docling's, still reports the paragraph Docling
/// itself would have made of it.
#[test]
fn a_block_quote_survives_the_markdown_and_html_round_trips() {
    let ws = Workspace::new("block-quote");
    let markdown = "> The medium is the message.\n\nA paragraph.\n";
    let source = ws.write("quote.md", markdown.as_bytes());
    let output = ws.join("rendered.json");
    run_ok(&[
        "-i",
        source.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    // Docling has no block quote, so the document claims none: the words are a
    // paragraph, and no field Docling would reject is written for it.
    let doc = json_of(&output);
    assert_eq!(doc["texts"][0]["label"], "paragraph", "{doc:#?}");
    assert!(
        doc["texts"][0].get("quote").is_none(),
        "the quote leaked into Docling's model: {:#?}",
        doc["texts"][0]
    );

    let written = std::fs::read_to_string(ws.join("rendered.md")).expect("the Markdown");
    assert_eq!(written, markdown, "the quote was flattened");

    let html = std::fs::read_to_string(ws.join("rendered.html")).expect("the HTML");
    assert!(
        html.contains("<blockquote><p>The medium is the message.</p></blockquote>"),
        "the page lost the quote: {html}"
    );

    // The page read back is the quote it was.
    let back = ws.join("back.json");
    run_ok(&[
        "-i",
        ws.join("rendered.html").to_str().unwrap(),
        "-o",
        back.to_str().unwrap(),
        "--to",
        "md",
        "--ocr",
        "never",
    ]);
    let reread = std::fs::read_to_string(ws.join("back.md")).expect("the Markdown from HTML");
    assert_eq!(
        reread, markdown,
        "the quote did not survive the HTML round trip"
    );
}

/// The page's `<title>` is the document's own title, not the name of the file
/// that was read, so an HTML page written out, read back under another name and
/// written again is the same bytes. A document with no title gets an empty one
/// rather than a filename that would change on the next reading.
#[test]
fn an_html_page_is_a_fixed_point_down_to_its_title() {
    let ws = Workspace::new("html-title");

    let titled = ws.write("brief.md", b"# Quarterly Brief\n\nBody paragraph.\n");
    run_ok(&[
        "-i",
        titled.to_str().unwrap(),
        "-o",
        ws.join("first.json").to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    let first = std::fs::read_to_string(ws.join("first.html")).expect("the HTML");
    assert!(
        first.contains("<title>Quarterly Brief</title>"),
        "the page is not titled by the document: {first}"
    );
    assert!(
        !first.contains("brief.md"),
        "the title is the input's name: {first}"
    );

    // The same page under a different name, written again: the title is the
    // document's, so nothing moves.
    let copy = ws.write("under_another_name.html", first.as_bytes());
    run_ok(&[
        "-i",
        copy.to_str().unwrap(),
        "-o",
        ws.join("second.json").to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    let second = std::fs::read_to_string(ws.join("second.html")).expect("the re-rendered HTML");
    assert_eq!(first, second, "the page changed when it was read back");

    // No title to take: the page says nothing rather than borrowing a name.
    let untitled = ws.write("untitled.md", b"Body paragraph only.\n");
    run_ok(&[
        "-i",
        untitled.to_str().unwrap(),
        "-o",
        ws.join("plain.json").to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    let plain = std::fs::read_to_string(ws.join("plain.html")).expect("the HTML");
    assert!(plain.contains("<title></title>"), "{plain}");
    assert!(!plain.contains("untitled.md"), "{plain}");

    // A page whose title lives only in the head, with no `h1` in the body: the
    // head's words are the document's title, so the page written back keeps
    // them instead of emptying its `<title>`, and the page it writes is itself
    // a fixed point when it is read again.
    let head_only = ws.write(
        "head_only_page.html",
        b"<!DOCTYPE html>\n<html>\n<head><title>Head only</title></head>\n<body>\n\
          <p>Body paragraph.</p>\n</body>\n</html>\n",
    );
    run_ok(&[
        "-i",
        head_only.to_str().unwrap(),
        "-o",
        ws.join("head_title.json").to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    let titled = std::fs::read_to_string(ws.join("head_title.html")).expect("the HTML");
    assert!(
        titled.contains("<title>Head only</title>"),
        "the head's title was lost: {titled}"
    );
    assert!(
        titled.contains("<h1>Head only</h1>"),
        "the title is not an item of the document: {titled}"
    );
    let again = ws.write("head_title_copy.html", titled.as_bytes());
    run_ok(&[
        "-i",
        again.to_str().unwrap(),
        "-o",
        ws.join("head_title_again.json").to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    assert_eq!(
        titled,
        std::fs::read_to_string(ws.join("head_title_again.html")).expect("the re-rendered HTML"),
        "the page changed when its head-titled page was read back"
    );
}

/// A remark's hyperlink belongs to the notes item, and both prose writers put
/// it around the paragraph the item opens with — Markdown as
/// `[words](url)`, HTML as `<a href>words</a>`. Reading either back has to keep
/// the address on the item, not only the words, or the next render turns the
/// link into its brackets (Markdown) or drops it (HTML).
#[test]
fn a_linked_remark_survives_the_markdown_and_html_round_trips() {
    let ws = Workspace::new("linked-remark");

    let markdown = "<!-- pdfextractor:group comment_section notes comment-slide1-1 -->\n\n\
                    > **Comment:** [the ledger](https://example.com/ledger)\n\n\
                    <!-- pdfextractor:/group -->\n";
    let source = ws.write("remark.md", markdown.as_bytes());
    let output = ws.join("rendered.json");
    run_ok(&[
        "-i",
        source.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--include-notes",
        "--ocr",
        "never",
    ]);
    let doc = json_of(&output);
    let notes: Vec<&serde_json::Value> = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .collect();
    assert_eq!(notes.len(), 1, "{notes:#?}");
    assert_eq!(notes[0]["text"], "the ledger");
    assert_eq!(notes[0]["hyperlink"], "https://example.com/ledger");
    // The remark is still the comment's own section, not a body-level item.
    assert_eq!(notes[0]["parent"]["$ref"], "#/groups/0");
    assert_eq!(doc["groups"][0]["label"], "comment_section");
    let reread = std::fs::read_to_string(ws.join("rendered.md")).expect("the re-rendered Markdown");
    assert_eq!(
        reread, markdown,
        "the remark's link changed on the way back through Markdown"
    );

    let html = "<aside class=\"notes comment\">\n\
                <p><strong>Comment:</strong> <a href=\"https://example.com/ledger\">the ledger</a></p>\n\
                </aside>\n";
    let source = ws.write("remark.html", html.as_bytes());
    let output = ws.join("rendered_html.json");
    run_ok(&[
        "-i",
        source.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--include-notes",
        "--ocr",
        "never",
    ]);
    let doc = json_of(&output);
    let notes: Vec<&serde_json::Value> = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .collect();
    assert_eq!(notes.len(), 1, "{notes:#?}");
    assert_eq!(notes[0]["text"], "the ledger");
    assert_eq!(notes[0]["hyperlink"], "https://example.com/ledger");
    let reread =
        std::fs::read_to_string(ws.join("rendered_html.html")).expect("the re-rendered HTML");
    assert!(
        reread.contains(
            "<p><strong>Comment:</strong> <a href=\"https://example.com/ledger\">the ledger</a></p>"
        ),
        "the remark's link changed on the way back through HTML:\n{reread}"
    );
}

/// An HTML `<img>` with a local source is a picture item: the file is read from
/// beside the page and copied beside the output, exactly as a Markdown
/// `![alt](source)` is, and the words around it stay words in order. The file
/// keeps the name the page gave it, which is what lets a page be written back as
/// the page it was.
#[test]
fn an_html_picture_is_copied_like_a_markdown_one() {
    let ws = Workspace::new("html-picture");
    let image = repository_root().join("test-data/images/scan.jpg");
    let bytes = std::fs::read(&image).expect("the image fixture");
    ws.write("scan.jpg", &bytes);
    let input = ws.write(
        "page.html",
        b"<!DOCTYPE html><html><body><h1>Illustrated</h1><p>Before <img src=\"scan.jpg\" alt=\"a scan\"> after.</p></body></html>",
    );

    let json = ws.join("page.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&json);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(
        pictures.len(),
        1,
        "one image in, one picture out: {pictures:#?}"
    );
    assert_eq!(pictures[0]["image"]["mimetype"], "image/jpeg");
    assert_eq!(pictures[0]["image"]["uri"], "scan.jpg");
    assert_eq!(
        std::fs::read(ws.join("scan.jpg")).expect("the copied image"),
        bytes,
        "the picture was not copied byte for byte"
    );

    let texts: Vec<&str> = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect();
    assert!(
        texts.contains(&"Before") && texts.contains(&"after."),
        "the words around the picture did not survive: {texts:?}"
    );

    let markdown = std::fs::read_to_string(ws.join("page.md")).expect("the Markdown");
    assert!(
        markdown.contains("![Image](scan.jpg)"),
        "the writer did not name the copied image:\n{markdown}"
    );
}

/// A flow page's picture keeps the name the page gave it even when that name
/// holds a space or a character HTML must escape. The page written from the
/// document names the same file — `&` as `&amp;`, the apostrophe as its
/// numeric entity — so reading that page again and writing it once more is the
/// page's own bytes, which is what makes it a fixed point. The Markdown
/// rendering names the same file in the `<…>` form that holds a space, and
/// reading that back settles there too instead of turning the picture into
/// its markup.
#[test]
fn a_picture_keeps_a_name_with_a_space_and_escapes() {
    let ws = Workspace::new("html-picture-name");
    let bytes = fixture("images/scan.jpg");
    let name = "images/scan & Bob's notes.jpg";
    ws.write(name, &bytes);
    let input = ws.write(
        "page.html",
        b"<!DOCTYPE html><html><head><title>Names</title></head><body><h1>Names</h1>\
          <p><img src=\"images/scan &amp; Bob's notes.jpg\" alt=\"a scan\"></p></body></html>",
    );

    let json = ws.join("first.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&json);
    assert_eq!(
        doc["pictures"][0]["image"]["uri"], name,
        "the picture did not keep the page's own name: {:#?}",
        doc["pictures"]
    );
    assert_eq!(
        std::fs::read(ws.join(name)).expect("the copied picture"),
        bytes,
        "the picture was not kept byte for byte"
    );

    let html = std::fs::read_to_string(ws.join("first.html")).expect("the HTML");
    assert!(
        html.contains("<p><img src=\"images/scan &amp; Bob&#39;s notes.jpg\" alt=\"Image\"></p>"),
        "the page does not name the file it came with:\n{html}"
    );
    assert!(
        !ws.join("first_images").exists(),
        "the picture was renamed into this run's own file"
    );

    // The page is its own bytes when it is read back and written again.
    let copy = ws.write("again.html", html.as_bytes());
    run_ok(&[
        "-i",
        copy.to_str().unwrap(),
        "-o",
        ws.join("second.json").to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);
    assert_eq!(
        html,
        std::fs::read_to_string(ws.join("second.html")).expect("the re-rendered HTML"),
        "the page stopped being its own bytes"
    );

    // The same name in Markdown: the angle form is what keeps the space from
    // reading as a title, and reading it back gives the same line again.
    let markdown = std::fs::read_to_string(ws.join("first.md")).expect("the Markdown");
    assert!(
        markdown.contains("![Image](<images/scan & Bob's notes.jpg>)"),
        "the Markdown did not name the file as one address:\n{markdown}"
    );
    let md_copy = ws.write("again.md", markdown.as_bytes());
    run_ok(&[
        "-i",
        md_copy.to_str().unwrap(),
        "-o",
        ws.join("third.json").to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);
    assert_eq!(
        markdown,
        std::fs::read_to_string(ws.join("third.md")).expect("the re-rendered Markdown"),
        "the Markdown stopped naming the file it came with"
    );
}

/// A `<picture>` names several files for one image, and the source copied beside
/// the output is the one the page's conditions leave usable: a `media` feature
/// query and a non-image `type` both disqualify their source, so the `img`'s own
/// `src` is the picture — its file copied byte for byte, even though the
/// conditional candidates name files that exist.
#[test]
fn a_responsive_html_picture_copies_the_chosen_source() {
    let ws = Workspace::new("html-responsive-picture");
    let image = repository_root().join("test-data/images/scan.jpg");
    let bytes = std::fs::read(&image).expect("the image fixture");
    // The two candidates the page passes over are real files with different
    // bytes, so a reader that ignored `media` or `type` would copy one of them.
    ws.write("wide.png", b"not the picture");
    ws.write("page2.html", b"<p>not the picture either</p>");
    ws.write("fallback.jpg", &bytes);
    let input = ws.write(
        "page.html",
        b"<!DOCTYPE html><html><body><p><picture><source media=\"(min-width: 800px)\" srcset=\"wide.png 1x\"><source type=\"text/html\" srcset=\"page2.html\"><img src=\"fallback.jpg\" alt=\"a responsive scan\"></picture></p></body></html>",
    );

    let json = ws.join("page.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&json);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(pictures.len(), 1, "one picture out: {pictures:#?}");
    assert_eq!(pictures[0]["image"]["mimetype"], "image/jpeg");
    assert_eq!(pictures[0]["image"]["uri"], "fallback.jpg");
    assert_eq!(
        std::fs::read(ws.join("fallback.jpg")).expect("the copied image"),
        bytes,
        "the usable source was not the one copied"
    );
    let markdown = std::fs::read_to_string(ws.join("page.md")).expect("the Markdown");
    assert!(
        markdown.contains("![Image](fallback.jpg)"),
        "the writer did not name the chosen image:\n{markdown}"
    );
}

/// A formula is typeset in the HTML the binary writes: MathML the browser draws
/// on its own, with the equation's TeX in the annotation beside it — so the page
/// shows an equation and still reads back as the formula it came from, display
/// and inline alike.
#[test]
fn an_html_formula_is_typeset_and_reads_back() {
    let ws = Workspace::new("html-mathml-formula");
    let input = ws.write(
        "page.md",
        b"$$E = mc^2$$\n\nThe area is $a^2$ and that is all.\n",
    );
    let json = ws.join("page.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let html = std::fs::read_to_string(ws.join("page.html")).expect("the HTML");
    assert!(
        html.contains(
            "<div class=\"formula\"><math style=\"display: block; margin: 0.5em 0\" display=\"block\" xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow><mi>E</mi><mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow><annotation encoding=\"application/x-tex\">E = mc^2</annotation></semantics></math></div>"
        ),
        "the display formula was not typeset:\n{html}"
    );
    assert!(
        html.contains(
            "<span class=\"formula\"><math display=\"inline\" xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow><msup><mi>a</mi><mn>2</mn></msup></mrow><annotation encoding=\"application/x-tex\">a^2</annotation></semantics></math></span>"
        ),
        "the inline formula was not typeset where it stands:\n{html}"
    );

    // The page is an input too: reading it back must give the same two items,
    // not the MathML's tokens or an empty formula.
    let back = ws.join("back.json");
    run_ok(&[
        "-i",
        ws.join("page.html").to_str().unwrap(),
        "-o",
        back.to_str().unwrap(),
        "--to",
        "json",
        "--ocr",
        "never",
    ]);
    let doc = json_of(&back);
    let texts: Vec<(&str, &str)> = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| {
            (
                item["label"].as_str().unwrap_or_default(),
                item["text"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert!(
        texts.contains(&("formula", "E = mc^2")),
        "the display formula did not read back: {texts:?}"
    );
    assert!(
        texts.contains(&("paragraph", "The area is $a^2$ and that is all.")),
        "the sentence around the inline formula did not read back: {texts:?}"
    );
}

/// A table's merged cells are placed, not flattened: the HTML reader keeps a
/// `colspan`/`rowspan` as the span the cell covers, so the model holds one cell
/// where the page wrote one — and the page is rendered back with the same
/// merges instead of an empty slot beside a cell that only looked wider.
#[test]
fn an_html_merged_table_keeps_its_spans() {
    let ws = Workspace::new("html-merged-table");
    let input = ws.write(
        "page.html",
        b"<html><body><table>\n\
<tr><th>Class</th><th>Student</th><th colspan=\"2\">Results</th></tr>\n\
<tr><td rowspan=\"2\">A</td><td>Alice</td><td>71</td><td>84</td></tr>\n\
<tr><td>Bob</td><td>65</td><td>79</td></tr>\n\
</table></body></html>\n",
    );
    let json = ws.join("page.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&json);
    let table = &doc["tables"].as_array().expect("a table list")[0]["data"];
    assert_eq!(table["num_rows"], 3, "the grid's height: {table}");
    assert_eq!(table["num_cols"], 4, "the grid's width: {table}");
    let cells = table["table_cells"].as_array().expect("a cell list");
    let placed: Vec<(&str, u64, u64, u64, u64)> = cells
        .iter()
        .map(|cell| {
            (
                cell["text"].as_str().unwrap_or_default(),
                cell["start_row_offset_idx"].as_u64().unwrap_or_default(),
                cell["start_col_offset_idx"].as_u64().unwrap_or_default(),
                cell["row_span"].as_u64().unwrap_or_default(),
                cell["col_span"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    assert!(
        placed.contains(&("Results", 0, 2, 1, 2)),
        "the two-column header was flattened: {placed:?}"
    );
    assert!(
        placed.contains(&("A", 1, 0, 2, 1)),
        "the two-row cell was flattened: {placed:?}"
    );
    // The slots the merges cover hold no cell at all — that is what a span
    // means in the model — so `Bob` starts the third row one column in and the
    // header row has three cells rather than four.
    assert!(
        !placed.iter().any(|cell| cell.1 == 2 && cell.2 == 0),
        "a cell was placed in a slot a merge covers: {placed:?}"
    );
    assert_eq!(
        placed.len(),
        10,
        "a merge made extra cells instead of covering slots: {placed:?}"
    );

    let html = std::fs::read_to_string(ws.join("page.html")).expect("the HTML");
    assert!(
        html.contains("<th colspan=\"2\">Results</th>"),
        "the merged header lost its span:\n{html}"
    );
    assert!(
        html.contains("<td rowspan=\"2\">A</td>"),
        "the merged cell lost its span:\n{html}"
    );
    assert!(
        html.contains("<tr><td>Bob</td><td>65</td><td>79</td></tr>"),
        "the row under the merge did not begin at its first free column:\n{html}"
    );
}

/// An HTML table's `<th>` row is the table's own statement of which row heads
/// its columns, and it reaches the document model: a table that writes a group
/// title above its column names heads the names, not the title. The cells of
/// the row holding the `<th>`s are the ones that claim `column_header`; the
/// title row claims nothing, and every row is still there in its place. Both
/// writers then put that row where the format keeps a header — the pipe table's
/// first line and the page's `<thead>` — so the render names the columns instead
/// of inventing an empty header row for them.
#[test]
fn an_html_header_row_below_the_first_is_the_tables_header() {
    let ws = Workspace::new("html-header-row");
    let input = ws.write(
        "page.html",
        b"<html><body><table>\n\
<tr><td colspan=\"2\">Monthly totals</td></tr>\n\
<tr><th>Month</th><th>Total</th></tr>\n\
<tr><td>January</td><td>10</td></tr>\n\
</table></body></html>\n",
    );
    let json = ws.join("page.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&json);
    let cells: Vec<(u64, &str, bool)> = doc["tables"].as_array().expect("tables")[0]["data"]
        ["table_cells"]
        .as_array()
        .expect("cells")
        .iter()
        .map(|cell| {
            (
                cell["start_row_offset_idx"].as_u64().expect("a row"),
                cell["text"].as_str().expect("words"),
                cell["column_header"].as_bool().expect("a header flag"),
            )
        })
        .collect();
    assert_eq!(
        cells,
        vec![
            (0, "Monthly totals", false),
            (1, "Month", true),
            (1, "Total", true),
            (2, "January", false),
            (2, "10", false),
        ],
        "{doc:#?}"
    );

    let markdown = std::fs::read_to_string(ws.join("page.md")).expect("the Markdown");
    assert_eq!(
        markdown,
        "| Month          | Total |\n\
         |----------------|-------|\n\
         | Monthly totals |       |\n\
         | January        | 10    |\n",
        "the names head the pipe table, with no empty header row"
    );

    let html = std::fs::read_to_string(ws.join("page.html")).expect("the HTML");
    assert!(
        html.contains("<thead>\n<tr><th>Month</th><th>Total</th></tr>\n</thead>"),
        "the marked row is not the page's head:\n{html}"
    );
    assert!(
        html.contains("<tbody>\n<tr><td colspan=\"2\">Monthly totals</td></tr>\n"),
        "the title row did not follow the header into the body:\n{html}"
    );
}

/// A display formula written on one line in Markdown is the same formula item
/// the fenced form makes, and is written back in the fenced form.
#[test]
fn a_one_line_markdown_display_formula_is_a_formula_item() {
    let ws = Workspace::new("markdown-one-line-formula");
    let input = ws.write("page.md", b"$$E = mc^2$$\n\nAfter.\n");

    let json = ws.join("page.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&json);
    let formula = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .find(|item| item["label"] == "formula")
        .expect("a formula item");
    assert_eq!(formula["text"], "E = mc^2");

    let markdown = std::fs::read_to_string(ws.join("page.md")).expect("the Markdown");
    assert!(
        markdown.contains("$$\nE = mc^2\n$$"),
        "the formula was not written back as a display block:\n{markdown}"
    );
    assert!(
        markdown.contains("After."),
        "the prose after the formula was lost:\n{markdown}"
    );
}

/// A display formula written inside a quote or a list item is lifted into a
/// formula item of its own, and the writer emits it as the ordinary fenced
/// block — the container markup does not survive the lift.
#[test]
fn display_formulas_inside_markdown_blocks_are_lifted() {
    let ws = Workspace::new("markdown-lifted-formula");
    let input = ws.write("page.md", b"> $$E = mc^2$$\n\n- $$\n  a + b\n  = c\n  $$\n");

    let json = ws.join("page.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&json);
    let formulas: Vec<&str> = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .filter(|item| item["label"] == "formula")
        .map(|item| item["text"].as_str().expect("formula text"))
        .collect();
    assert_eq!(formulas, vec!["E = mc^2", "a + b\n= c"]);
    assert_eq!(
        doc["texts"].as_array().expect("a text list").len(),
        2,
        "the formula stayed the container's words: {doc:#?}"
    );

    let markdown = std::fs::read_to_string(ws.join("page.md")).expect("the Markdown");
    assert_eq!(markdown, "$$\nE = mc^2\n$$\n\n$$\na + b\n= c\n$$\n");
}

/// Inline math in Markdown prose is a formula span, not a block: the sentence
/// stays one paragraph in the document and in every rendering, the Markdown
/// keeps its dollars, and the HTML marks the formula where it stood.
#[test]
fn inline_markdown_math_stays_in_its_sentence() {
    let ws = Workspace::new("markdown-inline-formula");
    let input = ws.write("page.md", b"The area is $a^2$ and $b^2$.\n");

    let json = ws.join("page.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&json);
    let texts = doc["texts"].as_array().expect("a text list");
    assert_eq!(texts.len(), 1, "the sentence was split: {texts:#?}");
    assert_eq!(texts[0]["label"], "paragraph");
    assert_eq!(texts[0]["text"], "The area is $a^2$ and $b^2$.");

    let markdown = std::fs::read_to_string(ws.join("page.md")).expect("the Markdown");
    assert_eq!(markdown, "The area is $a^2$ and $b^2$.\n");

    // The spans are where the formulas stood, and each holds the typeset
    // equation with its own TeX in the annotation.
    let html = std::fs::read_to_string(ws.join("page.html")).expect("the HTML");
    for (latex, body) in [
        ("a^2", "<msup><mi>a</mi><mn>2</mn></msup>"),
        ("b^2", "<msup><mi>b</mi><mn>2</mn></msup>"),
    ] {
        let span = format!(
            "<span class=\"formula\"><math display=\"inline\" xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow>{body}</mrow><annotation encoding=\"application/x-tex\">{latex}</annotation></semantics></math></span>"
        );
        assert!(
            html.contains(&span),
            "{latex} was not typeset where it stood:\n{html}"
        );
    }
    assert!(
        html.contains("</span> and <span class=\"formula\">"),
        "the sentence was not kept around the formulas:\n{html}"
    );
    assert_eq!(
        html.matches("<p>").count(),
        1,
        "the sentence was split: {html}"
    );
}

/// Inline math inside a quote or a list item is treated like any other prose:
/// the container keeps its words, the dollars stay in the Markdown, and the
/// page typesets the formula where it stood instead of printing the
/// delimiters. A quoted remark is the notes layer, so it is asked for.
#[test]
fn inline_markdown_math_inside_quotes_and_lists_is_a_span() {
    let ws = Workspace::new("markdown-inline-formula-containers");
    let input = ws.write(
        "page.md",
        b"> The area is $a^2$ across.\n\n- The perimeter is $2\\pi r$ around.\n\n> **Notes:** the slope is $m x + b$.\n",
    );

    let json = ws.join("page.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--to",
        "html",
        "--include-notes",
        "--ocr",
        "never",
    ]);

    let doc = json_of(&json);
    let texts = doc["texts"].as_array().expect("a text list");
    let kept: Vec<(&str, &str)> = texts
        .iter()
        .map(|item| {
            (
                item["label"].as_str().expect("a label"),
                item["text"].as_str().expect("text"),
            )
        })
        .collect();
    assert_eq!(
        kept,
        vec![
            ("paragraph", "The area is $a^2$ across."),
            ("list_item", "The perimeter is $2\\pi r$ around."),
            // A quoted remark is the notes layer, and Docling labels its item
            // `text`: the layer, not the label, is what says it is a remark.
            ("text", "the slope is $m x + b$."),
        ],
        "a container split its sentence or lost the dollars: {texts:#?}"
    );

    // The quotation stays quoted — Docling has no block quote item, but the
    // fact travels beside the model and the writer says it again — and the list
    // item and the remark keep their own markup. In every case the dollars are
    // still the sentence's own.
    let markdown = std::fs::read_to_string(ws.join("page.md")).expect("the Markdown");
    assert_eq!(
        markdown,
        "> The area is $a^2$ across.\n\n- The perimeter is $2\\pi r$ around.\n\n> **Notes:** the slope is $m x + b$.\n"
    );

    let html = std::fs::read_to_string(ws.join("page.html")).expect("the HTML");
    // Each formula is typeset in the element its container made — a `<p>`, an
    // `<li>` and the note's aside — and no dollar survives on the page.
    for (container, latex, body) in [
        (
            "<blockquote><p>The area is ",
            "a^2",
            "<msup><mi>a</mi><mn>2</mn></msup>",
        ),
        (
            "<li>The perimeter is ",
            "2\\pi r",
            "<mn>2</mn><mi>π</mi><mi>r</mi>",
        ),
        (
            "<p><strong>Notes:</strong> the slope is ",
            "m x + b",
            "<mi>m</mi><mi>x</mi><mo>+</mo><mi>b</mi>",
        ),
    ] {
        let span = format!(
            "{container}<span class=\"formula\"><math display=\"inline\" xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow>{body}</mrow><annotation encoding=\"application/x-tex\">{latex}</annotation></semantics></math></span>"
        );
        assert!(
            html.contains(&span),
            "{latex} was not typeset where it stood:\n{html}"
        );
    }
    assert_eq!(
        html.matches('$').count(),
        0,
        "the Markdown delimiters reached the page:\n{html}"
    );
}

/// `--image-export-mode` decides how the prose writers treat a picture:
/// `referenced` (the default) points at the image written beside the document,
/// `embedded` carries its bytes in the page as a base64 `data:` URI, and
/// `placeholder` marks the spot and writes no link. A deck with one real
/// picture and one image-less drawing pins all three.
#[test]
fn the_image_export_mode_decides_how_a_picture_travels() {
    let ws = Workspace::new("image-export-mode");
    let fixture = repository_root().join("test-data/q3_review.pptx");
    let bytes = std::fs::read(&fixture).expect("the deck fixture");
    let input = ws.write("q3_review.pptx", &bytes);
    let input = input.to_str().unwrap();

    let convert = |stem: &str, extra: &[&str]| {
        let output = ws.join(&format!("{stem}.json"));
        let mut args = vec![
            "-i",
            input,
            "-o",
            output.to_str().unwrap(),
            "--to",
            "md",
            "--to",
            "html",
            "--ocr",
            "never",
        ];
        args.extend_from_slice(extra);
        run_ok(&args);
        ["md", "html"].map(|extension| {
            std::fs::read_to_string(ws.join(&format!("{stem}.{extension}")))
                .expect("the page written")
        })
    };

    for page in convert("referenced", &[]) {
        assert!(
            page.contains("referenced_images/slide3_1.png"),
            "the default no longer references the image file:\n{page}"
        );
        assert!(!page.contains("data:image"), "{page}");
        assert!(!page.contains("<!-- image -->"), "{page}");
    }

    for page in convert("embedded", &["--image-export-mode", "embedded"]) {
        assert!(
            page.contains("data:image/png;base64,"),
            "the embedded page carries no picture bytes:\n{page}"
        );
        assert!(
            !page.contains("embedded_images/"),
            "the embedded page still points at a file:\n{page}"
        );
    }

    for page in convert("placeholder", &["--image-export-mode", "placeholder"]) {
        assert!(
            page.contains("<!-- image -->"),
            "the placeholder page does not mark the spot:\n{page}"
        );
        assert!(!page.contains("data:image"), "{page}");
        assert!(!page.contains("_images/"), "{page}");
    }
}

/// `pictures.md` names a JPEG that lives beside it, so it is the flow-format
/// counterpart of the deck's picture: the same two modes are asserted for the
/// same two writers. What only a flow format can show is that the picture is
/// the file the Markdown named — the embedded page is decoded and held to
/// `test-data/images/scan.jpg` **byte for byte**, so a re-encode (which would
/// still decode), a PNG behind a JPEG label, or a payload that is not the
/// picture at all fails here. The Markdown must carry exactly the payload the
/// HTML does: one picture, one spelling of it, in both readings.
#[test]
fn pictures_md_embeds_and_placeholds_its_jpeg_verbatim() {
    let ws = Workspace::new("pictures-image-mode");
    // The fixture is converted where it lives: its `images/scan.jpg` is a path
    // relative to it, and a copy without the JPEG beside it is a document whose
    // picture cannot be reached, not the one this fixture describes.
    let input = repository_root().join("test-data/pictures.md");
    let input = input.to_str().expect("a UTF-8 path");
    let source = std::fs::read(repository_root().join("test-data/images/scan.jpg"))
        .expect("the JPEG the Markdown names");

    let convert = |stem: &str, mode: &str| {
        let output = ws.join(&format!("{stem}.json"));
        run_ok(&[
            "-i",
            input,
            "-o",
            output.to_str().unwrap(),
            "--to",
            "md",
            "--to",
            "html",
            "--image-export-mode",
            mode,
            "--ocr",
            "never",
        ]);
        ["md", "html"].map(|extension| {
            std::fs::read_to_string(ws.join(&format!("{stem}.{extension}")))
                .expect("the page written")
        })
    };

    let [markdown, html] = convert("embedded", "embedded");

    // The HTML carries the picture in a `data:` URI; its payload is the JPEG's
    // own bytes, not a re-encoding and not another format behind a JPEG label.
    let prefix = "data:image/jpeg;base64,";
    let (in_html, in_markdown) = (
        html.matches(prefix).count(),
        markdown.matches(prefix).count(),
    );
    assert_eq!(
        in_html, 1,
        "the embedded page carries the picture {in_html} times:\n{html}"
    );
    assert_eq!(
        in_markdown, 1,
        "the embedded Markdown carries the picture {in_markdown} times:\n{markdown}"
    );
    let start = html.find(prefix).expect("the embedded picture") + prefix.len();
    let end = start + html[start..].find('"').expect("the URI's end");
    let payload = &html[start..end];
    let embedded = STANDARD.decode(payload).expect("the page's base64");
    if embedded != source {
        // The bytes are the assertion, so the failure names where they part
        // rather than printing both pictures.
        let where_it_parts = embedded
            .iter()
            .zip(&source)
            .position(|(page, file)| page != file);
        panic!(
            "the embedded page does not carry the JPEG's own bytes: \
             {} bytes became {}{}",
            source.len(),
            embedded.len(),
            where_it_parts
                .map(|at| format!(", first differing at byte {at}"))
                .unwrap_or_default()
        );
    }

    // The Markdown carries the same bytes where the picture stood, and names no
    // file: the alt text and the payload are the whole link.
    assert!(
        markdown.contains(&format!("![Image]({prefix}{payload})")),
        "the Markdown does not carry the same picture bytes:\n{markdown}"
    );
    for page in [&markdown, &html] {
        assert!(
            !page.contains("_images/"),
            "the embedded page still points at a file:\n{page}"
        );
        assert!(
            !page.contains("<!-- image -->"),
            "the embedded page marked the spot instead of carrying the picture:\n{page}"
        );
        for words in [
            "The figure below is stored beside this file, not inside it.",
            "A paragraph after the picture, so the order is checked as well.",
        ] {
            assert!(
                page.contains(words),
                "the picture took its words: {words}\n{page}"
            );
        }
    }

    let [markdown, html] = convert("placeholder", "placeholder");

    // The spot is marked and nothing claims to be a picture that is not there.
    for page in [&markdown, &html] {
        assert!(
            page.contains("<!-- image -->"),
            "the placeholder page does not mark the spot:\n{page}"
        );
        assert!(!page.contains("data:image"), "{page}");
        assert!(!page.contains("_images/"), "{page}");
    }

    // The mark is where the picture stood, between the sentence before it and
    // the paragraph after it.
    for page in [&markdown, &html] {
        let before = page
            .find("The figure below is stored beside this file, not inside it.")
            .expect("the sentence before the picture");
        let spot = page.find("<!-- image -->").expect("the mark");
        let after = page
            .find("A paragraph after the picture, so the order is checked as well.")
            .expect("the sentence after the picture");
        assert!(
            before < spot && spot < after,
            "the placeholder is not where the picture stood:\n{page}"
        );
    }
}

/// A render written embedded carries the picture in the page instead of beside
/// it. Reading those pages back — the Markdown and the HTML alike — has to
/// decode the `data:` URI into a picture of the same media type and the same
/// bytes, so the embed mode is a way to ship the page, not a way to lose the
/// image. The bytes the first pass wrote beside the document are the ones the
/// page carries; reading it back has to give those bytes, not a re-encode.
#[test]
fn an_embedded_render_reads_back_as_the_picture_it_carried() {
    let ws = Workspace::new("embedded-picture-round-trip");
    let input = repository_root().join("test-data/pictures.md");
    let input = input.to_str().expect("a UTF-8 path");
    let output = ws.join("embedded.json");
    run_ok(&[
        "-i",
        input,
        "-o",
        output.to_str().unwrap(),
        "--to",
        "md",
        "--to",
        "html",
        "--image-export-mode",
        "embedded",
        "--ocr",
        "never",
    ]);
    // The source page names `images/scan.jpg`, so that is the name its picture
    // keeps beside the output.
    let picture = std::fs::read(ws.join("images/scan.jpg")).expect("the extracted JPEG");

    for extension in ["md", "html"] {
        let page = ws.join(&format!("embedded.{extension}"));
        let page_text = std::fs::read_to_string(&page).expect("the embedded page");
        assert!(
            page_text.contains("data:image/jpeg;base64,"),
            "the {extension} page carries no picture bytes:\n{page_text}"
        );

        // The page is read into a new stem, so its own outputs do not overwrite
        // the file being read.
        let json = ws.join(&format!("back-{extension}.json"));
        run_ok(&[
            "-i",
            page.to_str().unwrap(),
            "-o",
            json.to_str().unwrap(),
            "--to",
            "json",
            "--ocr",
            "never",
        ]);

        let doc = json_of(&json);
        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(
            pictures.len(),
            1,
            "{extension} lost the embedded picture: {pictures:#?}"
        );
        assert_eq!(
            pictures[0]["image"]["mimetype"], "image/jpeg",
            "{extension}"
        );
        let written = pictures[0]["image"]["uri"]
            .as_str()
            .expect("the picture's uri");
        assert_eq!(
            std::fs::read(ws.join(written)).expect("the picture read back"),
            picture,
            "{extension}: the embedded bytes did not survive the reading"
        );

        // The URI is the page's syntax, not its words: the surrounding prose is
        // still there and the payload is not.
        let words: String = doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .filter_map(|item| item["text"].as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !words.contains("data:image"),
            "{extension} kept the URI as words: {words}"
        );
        for sentence in [
            "The figure below is stored beside this file, not inside it.",
            "A paragraph after the picture, so the order is checked as well.",
        ] {
            assert!(
                words.contains(sentence),
                "{extension}: the words around the picture did not survive: {words}"
            );
        }
    }
}

/// A page written with `--image-export-mode placeholder` marks each picture with
/// `<!-- image -->` and carries no bytes of it. Reading that page has to give
/// the picture back — an item with nothing behind it — rather than the words
/// the mark is spelled with, or the next render has prose where a picture was.
/// Both readers, and the page as its own bytes: read in the mode it was written
/// in, the mark is written again, so the picture survives the round trip that
/// the mark used to erase.
#[test]
fn a_placeholder_page_reads_back_as_a_picture_with_no_bytes() {
    // The reader's mark and the writer's are one string; neither crate can see
    // the other, so this is where they are held together.
    assert_eq!(
        pdfextractor_text_parser::IMAGE_PLACEHOLDER,
        pdfextractor_md_exporter::IMAGE_PLACEHOLDER,
        "the readers look for a different mark than the writers write"
    );

    let ws = Workspace::new("placeholder-picture-round-trip");
    // The fixture names `images/scan.jpg` beside it, which the placeholder mode
    // does not need: that is the point of the mode.
    let input = repository_root().join("test-data/pictures.md");
    let input = input.to_str().expect("a UTF-8 path");
    let output = ws.join("placeholder.json");
    run_ok(&[
        "-i",
        input,
        "-o",
        output.to_str().unwrap(),
        "--to",
        "md",
        "--to",
        "html",
        "--image-export-mode",
        "placeholder",
        "--ocr",
        "never",
    ]);

    for extension in ["md", "html"] {
        let page = ws.join(&format!("placeholder.{extension}"));
        let page_text = std::fs::read_to_string(&page).expect("the placeholder page");
        assert!(
            page_text.contains("<!-- image -->"),
            "the {extension} page does not mark the spot:\n{page_text}"
        );
        assert!(
            !page_text.contains("data:image"),
            "the {extension} page carries bytes the mode leaves out:\n{page_text}"
        );

        let json = ws.join(&format!("back-{extension}.json"));
        run_ok(&[
            "-i",
            page.to_str().unwrap(),
            "-o",
            json.to_str().unwrap(),
            "--to",
            "json",
            "--ocr",
            "never",
        ]);

        // The mark is a picture: one item, with no image behind it, because the
        // page carried none.
        let doc = json_of(&json);
        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(
            pictures.len(),
            1,
            "{extension} lost the picture the page marked: {pictures:#?}"
        );
        assert!(
            pictures[0]["image"].is_null(),
            "{extension}: the mark claimed an image the page did not carry: {pictures:#?}"
        );

        // The mark is the page's syntax, not its words, and the prose around it
        // is still the prose the page held.
        let words: String = doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .filter_map(|item| item["text"].as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !words.contains("<!-- image -->"),
            "{extension} kept the mark as words: {words}"
        );
        for sentence in [
            "The figure below is stored beside this file, not inside it.",
            "A paragraph after the picture, so the order is checked as well.",
        ] {
            assert!(
                words.contains(sentence),
                "{extension}: the words around the picture did not survive: {words}"
            );
        }

        // Written again in the mode it was written in, the page is the page it
        // was: the picture is still a picture and still marked in place.
        let again = ws.join(&format!("again-{extension}.{extension}"));
        run_ok(&[
            "-i",
            page.to_str().unwrap(),
            "-o",
            again.to_str().unwrap(),
            "--to",
            extension,
            "--image-export-mode",
            "placeholder",
            "--ocr",
            "never",
        ]);
        assert_eq!(
            std::fs::read_to_string(&again).expect("the page written again"),
            page_text,
            "{extension}: the placeholder page is not its own bytes"
        );
    }
}

/// A picture in every format the model can carry survives a real conversion and
/// an HTML round trip: the file written beside the document is the source file
/// byte for byte, the document names each picture with its own media type and
/// the size that format states, and reading the rendered page back gives the
/// same pictures and the same bytes.
///
/// The list is [`pdfextractor_picture_formats::PICTURE_FORMATS`] narrowed to the
/// types Docling can name in an `ImageRef` and whose bytes it carries unchanged.
/// A WebP or a JPEG 2000 is outside it, but not because it stays a box with no
/// image any more: it is transcoded to PNG on extraction, which is what
/// `a_webp_or_jp2_picture_becomes_a_named_png` measures, so a round trip of its
/// own bytes is no longer the fact to hold it to.
/// Two of the six are formats no parser here decodes — a GIF and an SVG — so
/// their size comes from their own bytes, which is what lets them travel at all.
#[test]
fn a_picture_in_every_carried_format_survives_a_conversion_and_an_html_round_trip() {
    use pdfextractor_picture_formats::PICTURE_FORMATS;

    let ws = Workspace::new("picture-formats");
    let corpus = |name: &str| {
        let path = repository_root().join("test-data/images").join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()))
    };

    // One source file per carried format, with the size that format states:
    // the scans from the corpus where they exist, and the two encodings written
    // here — both are carried by their bytes alone.
    let gif = STANDARD
        .decode("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7")
        .expect("the 1x1 GIF's base64");
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="12"><rect width="10" height="12" fill="#336"/></svg>"##;
    let sources: Vec<(&str, Vec<u8>, (u32, u32))> = vec![
        (
            "image/png",
            std::fs::read(repository_root().join("test-data/invoice_scan.png"))
                .expect("the PNG fixture"),
            (620, 350),
        ),
        ("image/jpeg", corpus("scan.jpg"), (620, 350)),
        ("image/gif", gif, (1, 1)),
        ("image/bmp", corpus("scan.bmp"), (620, 350)),
        ("image/tiff", corpus("scan.tiff"), (620, 350)),
        ("image/svg+xml", svg.to_vec(), (10, 12)),
    ];

    // Every carried format the model can name has a source here: a format added
    // to the list later must bring its own fixture to this test rather than be
    // passed over in silence.
    let carried: Vec<&str> = PICTURE_FORMATS
        .iter()
        .filter(|format| pdfextractor_core::is_representable_media_type(format.mime))
        .map(|format| format.mime)
        .collect();
    let covered: Vec<&str> = sources.iter().map(|(mime, ..)| *mime).collect();
    assert_eq!(carried, covered, "a carried format has no picture here");

    let mut page = String::from("Before.\n\n");
    for (mime, bytes, _) in &sources {
        let extension = pdfextractor_picture_formats::extension_for_mime(mime)
            .unwrap_or_else(|| panic!("{mime} has no extension"));
        ws.write(&format!("picture.{extension}"), bytes);
        page.push_str(&format!("![{extension}](picture.{extension})\n\n"));
    }
    page.push_str("After.\n");
    let input = ws.write("page.md", page.as_bytes());

    let output = ws.join("out.json");
    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "html",
        "--ocr",
        "never",
    ]);

    // The conversion wrote each picture beside the document, named it by its own
    // media type and the size its bytes state, and kept every byte.
    assert_pictures(&json_of(&output), &ws, &sources, "the conversion");
    let html = std::fs::read_to_string(ws.join("out.html")).expect("the page");
    assert_eq!(
        html.matches("<img src=\"").count(),
        sources.len(),
        "a picture is missing from the page:\n{html}"
    );

    // The page read back is the same pictures, byte for byte.
    let back = ws.join("back.json");
    run_ok(&[
        "-i",
        ws.join("out.html").to_str().unwrap(),
        "-o",
        back.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "md",
        "--ocr",
        "never",
    ]);
    assert_pictures(&json_of(&back), &ws, &sources, "the HTML round trip");
    let markdown = std::fs::read_to_string(ws.join("back.md")).expect("the Markdown");
    assert_eq!(
        markdown.matches("![Image](").count(),
        sources.len(),
        "a picture left the page on the way back:\n{markdown}"
    );
}

/// A WebP or a JPEG 2000 cannot be named in an `ImageRef` — Docling's registry
/// has no entry for either — so a picture in one of them is decoded and written
/// out as PNG, exactly as a Word metafile already is. The picture that used to
/// be a box with no image is one the document names, at the pixel grid its
/// decoder read, pointing at a file that really is the PNG it claims.
#[test]
fn a_webp_or_jp2_picture_becomes_a_named_png() {
    let ws = Workspace::new("named-png");
    let corpus = |name: &str| {
        let path = repository_root().join("test-data/images").join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()))
    };

    for extension in ["webp", "jp2"] {
        let source = corpus(&format!("scan.{extension}"));
        ws.write(&format!("picture.{extension}"), &source);
        let input = ws.write(
            &format!("{extension}.md"),
            format!("![scan](picture.{extension})\n").as_bytes(),
        );
        let output = ws.join(&format!("{extension}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        let pictures = doc["pictures"].as_array().expect("a picture list");
        assert_eq!(pictures.len(), 1, "{extension}: {doc:#?}");
        assert_eq!(
            read_named_image(&ws, &pictures[0]),
            "image/png",
            "{extension}: the picture was not transcoded"
        );
        let image = &pictures[0]["image"];
        assert_eq!(image["size"]["width"], 620.0, "{extension}");
        assert_eq!(image["size"]["height"], 350.0, "{extension}");

        // The file the reference names is a PNG of the scan's grid, not just
        // bytes renamed: the decoder has to read it back at the same size.
        let uri = image["uri"].as_str().expect("an image uri");
        let written = std::fs::read(ws.join(uri)).expect("the picture written beside the JSON");
        let decoded = image::load_from_memory(&written)
            .unwrap_or_else(|e| panic!("{extension}: the written picture does not decode: {e}"));
        assert_eq!(
            (decoded.width(), decoded.height()),
            (620, 350),
            "{extension}"
        );
    }
}

/// Every picture of `doc` is the source at its place: the same media type, the
/// size that format states, and the same bytes in the file it points at.
fn assert_pictures(
    doc: &serde_json::Value,
    ws: &Workspace,
    sources: &[(&str, Vec<u8>, (u32, u32))],
    what: &str,
) {
    let pictures = doc["pictures"].as_array().expect("a picture list");
    assert_eq!(pictures.len(), sources.len(), "{what}: {doc:#?}");
    for (picture, (mime, bytes, (width, height))) in pictures.iter().zip(sources) {
        let image = picture
            .get("image")
            .unwrap_or_else(|| panic!("{what}: {mime} kept no image: {picture:#?}"));
        assert_eq!(&image["mimetype"], mime, "{what}: {image:#?}");
        assert_eq!(image["size"]["width"], *width as f64, "{what}: {mime}");
        assert_eq!(image["size"]["height"], *height as f64, "{what}: {mime}");
        let uri = image["uri"].as_str().expect("a uri");
        let written =
            std::fs::read(ws.join(uri)).unwrap_or_else(|e| panic!("{what}: missing {uri}: {e}"));
        assert_eq!(&written, bytes, "{what}: {mime}'s bytes changed");
    }
}

/// A plain image is one page, and its text comes from OCR. The page is the
/// pixel grid at the resolution the file declares (150 dpi), not an arbitrary
/// canvas; the origin names the image's own media type.
#[test]
fn an_image_becomes_one_page_read_by_ocr() {
    let ws = Workspace::new("image");
    let fixture = repository_root().join("test-data/invoice_scan.png");
    let bytes =
        std::fs::read(&fixture).unwrap_or_else(|e| panic!("missing {}: {e}", fixture.display()));
    let input = ws.write("invoice_scan.png", &bytes);
    let output = ws.join("invoice_scan.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);

    let doc = json_of(&output);
    assert_eq!(doc["name"], "invoice_scan");
    assert_eq!(doc["origin"]["mimetype"], "image/png");
    assert_eq!(doc["origin"]["filename"], "invoice_scan.png");

    // One image, one page: 620 x 350 pixels at 150 dpi.
    let pages = doc["pages"].as_object().expect("a page map");
    assert_eq!(pages.len(), 1, "{pages:#?}");
    let size = &doc["pages"]["1"]["size"];
    let width = size["width"].as_f64().expect("a width");
    let height = size["height"].as_f64().expect("a height");
    assert!((width - 297.575).abs() < 0.1, "width {width}");
    assert!((height - 167.986).abs() < 0.1, "height {height}");

    // The OCR lines are the page's text, and every box is measured on it.
    let texts = doc["texts"].as_array().expect("a text list");
    let joined = texts
        .iter()
        .map(|text| text["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    for needle in ["ACME SUPPLIES", "Invoice 12345", "Total Due 42.50"] {
        assert!(joined.contains(needle), "missing {needle:?} in: {joined}");
    }
    for text in texts {
        let bbox = &text["prov"][0]["bbox"];
        let (l, t, r, b) = (
            bbox["l"].as_f64().unwrap(),
            bbox["t"].as_f64().unwrap(),
            bbox["r"].as_f64().unwrap(),
            bbox["b"].as_f64().unwrap(),
        );
        assert!(
            l >= 0.0 && t >= 0.0 && r <= width + 0.5 && b <= height + 0.5,
            "box outside the page: {bbox}"
        );
    }
}

/// Every encoding the reader accepts names its own media type and writes its
/// own page. WebP carries no resolution and a JP2 states none of its own, so
/// their pages are one pixel per point; so is a JPEG 2000 codestream, which has
/// no container to hold a resolution box — the same 620 × 350 grid as its JP2,
/// at the 72 dpi default. The JP2 that does carry a capture resolution box is
/// placed at the 150 dpi it states, like the others.
#[test]
fn every_image_encoding_writes_its_own_page() {
    let ws = Workspace::new("image-formats");
    for (file, mimetype, width, height) in [
        ("scan.jpg", "image/jpeg", 297.6, 168.0),
        ("scan.tiff", "image/tiff", 297.6, 168.0),
        ("scan.bmp", "image/bmp", 297.5756, 167.9862),
        ("scan.webp", "image/webp", 620.0, 350.0),
        ("scan.jp2", "image/jp2", 620.0, 350.0),
        // The same picture without its JP2 container: the page is the grid its
        // `SIZ` marker states.
        ("scan.j2c", "image/jp2", 620.0, 350.0),
        // The same codestream under its other name, and the 150 dpi container
        // under a name codestreams also carry: the header is read from the
        // bytes, so `.jpc` is the 620 × 350 grid and `.jpf` the 150 dpi one.
        ("scan.jpc", "image/jp2", 620.0, 350.0),
        ("scan.jpf", "image/jp2", 297.575, 167.986),
        ("scan_150dpi.jp2", "image/jp2", 297.575, 167.986),
        // A GIF states no resolution either: its screen is the page, one pixel
        // per point. This one is a still, so it is one page.
        ("scan.gif", "image/gif", 620.0, 350.0),
    ] {
        let byte_path = repository_root().join("test-data/images").join(file);
        let bytes = std::fs::read(&byte_path)
            .unwrap_or_else(|e| panic!("missing {}: {e}", byte_path.display()));
        let input = ws.write(file, &bytes);
        let output = ws.join(&format!("{file}.json"));

        // `--ocr never` still writes the page: OCR is what would have read it,
        // and the flag says not to. The page's facts do not depend on OCR.
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--ocr",
            "never",
        ]);

        let doc = json_of(&output);
        assert_eq!(doc["origin"]["mimetype"], mimetype, "{file}");
        assert_eq!(doc["origin"]["filename"], file, "{file}");
        assert_eq!(
            doc["pages"].as_object().map(|pages| pages.len()),
            Some(1),
            "{file}: {}",
            doc["pages"]
        );
        let size = &doc["pages"]["1"]["size"];
        let (w, h) = (
            size["width"].as_f64().unwrap(),
            size["height"].as_f64().unwrap(),
        );
        assert!((w - width).abs() < 0.1, "{file}: width {w}");
        assert!((h - height).abs() < 0.1, "{file}: height {h}");
        assert!(
            doc["texts"].as_array().is_some_and(Vec::is_empty),
            "{file}: --ocr never wrote text"
        );
    }
}

/// A standalone JPEG 2000 is a document the way a WebP is: one page, its text
/// from OCR, the origin naming its own media type — and that holds for both of
/// its spellings, the JP2 container and the bare codestream (`.j2c`, `.jpc`). A
/// GIF that is a still is one too. The five files are one scan in four
/// encodings, so the documents must agree on everything the encoding does not
/// change — the page, the boxes and the words — while each keeps its own media
/// type and file name. A JPEG 2000 is not decoded by the `image` crate unless
/// `hayro-jpeg2000` has registered itself, and a GIF is not decoded by
/// Tesseract's loader, so this also proves the OCR path found the pixels.
#[test]
fn a_jp2_or_a_gif_is_one_page_read_by_ocr_like_a_webp() {
    let ws = Workspace::new("jp2-page");
    let mut documents = Vec::new();

    for (file, mimetype) in [
        ("scan.webp", "image/webp"),
        ("scan.jp2", "image/jp2"),
        ("scan.j2c", "image/jp2"),
        ("scan.jpc", "image/jp2"),
        ("scan.gif", "image/gif"),
    ] {
        let bytes = std::fs::read(repository_root().join("test-data/images").join(file))
            .unwrap_or_else(|e| panic!("missing {file}: {e}"));
        let input = ws.write(file, &bytes);
        let output = ws.join(&format!("{file}.json"));
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--ocr",
            "always",
        ]);

        let doc = json_of(&output);
        assert_eq!(doc["origin"]["mimetype"], mimetype, "{file}");
        assert_eq!(doc["origin"]["filename"], file, "{file}");
        assert_eq!(doc["name"], "scan", "{file}");
        assert_eq!(
            doc["pages"].as_object().map(|pages| pages.len()),
            Some(1),
            "{file}: {}",
            doc["pages"]
        );
        let size = &doc["pages"]["1"]["size"];
        assert_eq!(size["width"], 620.0, "{file}");
        assert_eq!(size["height"], 350.0, "{file}");
        documents.push((file, doc));
    }

    let (first_file, first) = &documents[0];
    let texts = |doc: &serde_json::Value| -> Vec<(String, String, [f64; 4])> {
        doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .map(|item| {
                let bbox = &item["prov"][0]["bbox"];
                (
                    item["label"].as_str().unwrap_or_default().to_string(),
                    item["text"].as_str().unwrap_or_default().to_string(),
                    [
                        bbox["l"].as_f64().unwrap(),
                        bbox["t"].as_f64().unwrap(),
                        bbox["r"].as_f64().unwrap(),
                        bbox["b"].as_f64().unwrap(),
                    ],
                )
            })
            .collect()
    };
    let first_texts = texts(first);
    assert!(
        first_texts.len() >= 3,
        "{first_file}: OCR found no page text: {first_texts:?}"
    );
    // The pixels are the same drawing, but the WebP fixture is lossy while the
    // JP2 is lossless and the GIF quantises the palette, and one blurred glyph
    // can change a full stop: the words are compared without their punctuation,
    // the boxes with the line.
    let letters = |text: &str| -> String {
        text.chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    for (other_file, other) in &documents[1..] {
        let other_texts = texts(other);
        assert_eq!(
            first_texts.len(),
            other_texts.len(),
            "{first_file} and {other_file} are one scan in two encodings:\n\
             {first_texts:#?}\n{other_texts:#?}"
        );
        for ((label, text, bbox), (other_label, other_text, other_bbox)) in
            first_texts.iter().zip(&other_texts)
        {
            assert_eq!(label, other_label, "{first_file} vs {other_file}");
            assert_eq!(
                letters(text),
                letters(other_text),
                "{first_file} vs {other_file}"
            );
            for (one, other) in bbox.iter().zip(other_bbox) {
                assert!(
                    (one - other).abs() < 0.5,
                    "{first_file} vs {other_file}: {bbox:?} and {other_bbox:?}"
                );
            }
        }
        let words = other_texts
            .iter()
            .map(|(_, text, _)| text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        for needle in ["ACME SUPPLIES", "Invoice 12345", "Total Due 42.50"] {
            assert!(
                words.contains(needle),
                "{other_file}: missing {needle:?} in: {words}"
            );
        }
    }
}

/// The CLI with the bridge switched off: `PDFE_SOFFICE` names a file that is
/// not there, so a run that must not reach LibreOffice says so out loud.
fn run_without_libreoffice(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args(args)
        .env("RUST_LOG", "info")
        .env("PDFE_SOFFICE", "/definitely/not/a/soffice")
        .output()
        .expect("spawn the CLI binary")
}

/// `.wps` has named three unrelated formats, so the bytes decide which reader
/// runs. A DOCX saved under that name is read as what it is — and the bridge is
/// off, so nothing but content detection can have made this work.
#[test]
fn a_docx_named_wps_is_read_as_a_docx() {
    let ws = Workspace::new("wps-docx");
    let source = repository_root().join("test-data/merged_cells.docx");
    let bytes = std::fs::read(&source).expect("the DOCX fixture");
    let input = ws.write("report.wps", &bytes);
    let output = ws.join("report.json");

    let out = run_without_libreoffice(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let doc = json_of(&output);
    // The document is the file the user handed over, whatever its name says.
    assert_eq!(doc["name"], "report.wps");
    assert_eq!(doc["origin"]["filename"], "report.wps");
    assert_eq!(
        doc["origin"]["mimetype"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    assert!(
        !doc["tables"].as_array().unwrap().is_empty(),
        "the DOCX's merged table did not come through: {doc:#?}"
    );
}

/// A Word binary called `.wps` is a different case: the extension says Works,
/// the bytes say Word 97, and the Word reader is the one that reads it.
#[test]
fn a_word_binary_named_wps_is_read_by_the_word_reader() {
    let ws = Workspace::new("wps-doc");
    let source = repository_root().join("test-data/real/equation.doc");
    let bytes = std::fs::read(&source).expect("the .doc fixture");
    let input = ws.write("letter.wps", &bytes);
    let output = ws.join("letter.json");

    let out = run_without_libreoffice(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let doc = json_of(&output);
    assert_eq!(doc["name"], "letter.wps");
    assert_eq!(doc["origin"]["mimetype"], "application/msword");
}

/// A Works compound file has no reader here, and the failure says what it is,
/// what it would need, and what the user can do instead.
#[test]
fn a_works_document_names_libreoffice_when_it_is_absent() {
    let ws = Workspace::new("works-missing");
    let mut bytes = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    for stream in ["CompObj", "Contents"] {
        bytes.extend(stream.encode_utf16().flat_map(u16::to_le_bytes));
        bytes.extend_from_slice(&[0, 0]);
    }
    let input = ws.write("old.wps", &bytes);

    let out = run_without_libreoffice(&["-i", input.to_str().unwrap()]);

    assert!(!out.status.success(), "a Works file was accepted");
    let logs = logs_of(&out);
    assert!(
        logs.contains("Microsoft Works document"),
        "the error does not name the format: {logs}"
    );
    assert!(
        logs.contains("LibreOffice"),
        "the error does not name what it needs: {logs}"
    );
    assert!(
        logs.contains("re-save it as .docx"),
        "the error offers no way out: {logs}"
    );
    // The path is said once, by the run's own error line, not twice.
    assert_eq!(logs.matches("old.wps").count(), 1, "{logs}");
}

/// The Works family is read through LibreOffice, so the test needs a real
/// document and a LibreOffice to read it with.
///
/// No sample is committed: libwps' corpus states no licence for its files, and
/// a fixture this repository cannot license is one it cannot hold. Point
/// `PDFE_WORKS_SAMPLES` at a directory of Works documents — the libwps
/// OSS-Fuzz seed corpus works:
///
/// ```text
/// PDFE_WORKS_SAMPLES=/tmp/wpsprobe/seeds/wps cargo test -p pdfextractor-cli
/// ```
#[test]
fn works_documents_are_read_through_libreoffice() {
    let Ok(directory) = std::env::var("PDFE_WORKS_SAMPLES") else {
        eprintln!("PDFE_WORKS_SAMPLES is not set; skipping the LibreOffice bridge test");
        return;
    };
    let search = std::env::var_os("PATH").unwrap_or_default();
    let has_soffice = std::env::split_paths(&search).any(|directory| {
        ["soffice", "libreoffice"]
            .iter()
            .any(|name| directory.join(name).is_file())
    });
    if !has_soffice {
        eprintln!("no soffice on PATH; skipping the LibreOffice bridge test");
        return;
    }

    let mut samples: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|e| panic!("read {directory}: {e}"))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("wps"))
        })
        .collect();
    samples.sort();
    assert!(!samples.is_empty(), "no .wps under {directory}");
    samples.truncate(3);

    let ws = Workspace::new("works-bridge");
    let mut content = 0;
    for sample in &samples {
        let bytes = std::fs::read(sample).expect("the sample");
        let input = ws.write("old.wps", &bytes);
        let output = ws.join("old.json");
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--ocr",
            "never",
        ]);
        let doc = json_of(&output);
        assert_eq!(
            doc["origin"]["mimetype"], "application/vnd.ms-works",
            "a Works file was given another format's identity"
        );
        assert_eq!(doc["origin"]["filename"], "old.wps");
        assert_eq!(doc["name"], "old.wps");
        let texts = doc["texts"].as_array().unwrap().len();
        let tables = doc["tables"].as_array().unwrap().len();
        content += usize::from(texts + tables > 0);
    }
    assert!(
        content > 0,
        "none of the {} samples produced any content",
        samples.len()
    );
}

/// Whether LibreOffice is installed, so the bridge tests can say they skipped
/// rather than fail on a machine that has no reason to have it.
fn libreoffice_on_path() -> bool {
    soffice_binary().is_some()
}

/// LibreOffice's executable, if there is one to run: `PDFE_SOFFICE` names it
/// outright — as the CLI's own bridge allows — and `PATH` is searched
/// otherwise.
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

/// The hash the CLI stamps into `origin.binary_hash` — FNV-1a, 64-bit — so a
/// bridged document's identity can be held to the bytes the user handed over.
fn fnv1a(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// The document without what belongs to the package rather than to the deck.
///
/// The bridged and the native file are different packages that say the same
/// thing, and three differences come from the container, not the deck:
///
/// * Their identity — name, MIME type, hash — is asserted on its own, because
///   the bridge must keep the file the user handed over.
/// * Impress re-lays shapes out when it writes its package, so a box can move
///   by a point or more (and a text frame it re-fits can move a lot). The
///   boxes are therefore dropped from the comparison; every item, its page and
///   its character span stay.
/// * Impress assigns its own comment ids, so two comments PowerPoint numbers 1
///   and 2 can both be 1. A `comment_section` group's name is the comment's id;
///   the slide part of the name is kept and the id dropped.
///
/// The extracted image directories are named after each output file, so a
/// picture's `uri` is compared by file name. Everything else — pages, groups,
/// items, labels, layers, words, table cells, picture media — has to match.
fn deck_view(document: &serde_json::Value) -> serde_json::Value {
    let mut view = document.clone();
    let object = view.as_object_mut().expect("a document object");
    object.remove("origin");
    object.insert(
        "name".to_string(),
        serde_json::Value::String("deck".to_string()),
    );

    if let Some(groups) = object.get_mut("groups").and_then(|g| g.as_array_mut()) {
        for group in groups {
            if group["label"] == "comment_section" {
                if let Some(name) = group["name"].as_str() {
                    if let Some((slide, _id)) = name.rsplit_once('-') {
                        group["name"] = serde_json::Value::String(slide.to_string());
                    }
                }
            }
        }
    }

    for section in ["texts", "tables", "pictures"] {
        let Some(items) = object.get_mut(section).and_then(|s| s.as_array_mut()) else {
            continue;
        };
        for item in items {
            if let Some(prov) = item.get_mut("prov").and_then(|p| p.as_array_mut()) {
                for entry in prov {
                    if let Some(entry) = entry.as_object_mut() {
                        entry.remove("bbox");
                    }
                }
            }
            if section == "pictures" {
                if let Some(uri) = item.get_mut("image").and_then(|image| image.get_mut("uri")) {
                    if let Some(name) = uri.as_str().and_then(|u| u.rsplit('/').next()) {
                        *uri = serde_json::Value::String(name.to_string());
                    }
                }
            }
        }
    }
    view
}

/// The workbook without what belongs to the package rather than to the sheet.
///
/// The bridged and the native file are different packages that say the same
/// thing, and only their identity comes from the container: `origin`, and the
/// name, which is the original file's for a bridged workbook and the stem for
/// a native one. That is all this view removes, and it can remove so little
/// because a workbook's geometry is content — a worksheet is a page measured
/// in cell indices and a table's box is its anchor's cell — so unlike Impress'
/// re-laid-out boxes there is nothing here for the writer to move. Every sheet,
/// its layer, every table, cell, span, word and cached value has to match.
fn sheet_view(document: &serde_json::Value) -> serde_json::Value {
    let mut view = document.clone();
    let object = view.as_object_mut().expect("a document object");
    object.remove("origin");
    object.insert(
        "name".to_string(),
        serde_json::Value::String("workbook".to_string()),
    );
    view
}

/// The document without what belongs to the container rather than to the file.
///
/// A `.doc` and the `.docx` it was written from are different containers that
/// say the same thing, and only two differences come from the container:
///
/// * Their identity — name, MIME type, hash — is asserted on its own, because
///   the bridge must keep the file the user handed over.
/// * The extracted image directory is named after each output file, so a
///   picture's `uri` is compared by file name, as `deck_view` compares it.
///
/// That is all. A flow document publishes no page geometry, so there is no box
/// for a writer to re-lay out and nothing else to excuse: every item, label,
/// marker, level, word, link, table cell, span and picture has to agree.
fn document_view(document: &serde_json::Value) -> serde_json::Value {
    let mut view = document.clone();
    let object = view.as_object_mut().expect("a document object");
    object.remove("origin");
    object.insert(
        "name".to_string(),
        serde_json::Value::String("document".to_string()),
    );
    if let Some(pictures) = object.get_mut("pictures").and_then(|p| p.as_array_mut()) {
        for picture in pictures {
            if let Some(uri) = picture
                .get_mut("image")
                .and_then(|image| image.get_mut("uri"))
            {
                if let Some(name) = uri.as_str().and_then(|u| u.rsplit('/').next()) {
                    *uri = serde_json::Value::String(name.to_string());
                }
            }
        }
    }
    view
}

/// The first place two values part, as a path a reader can follow —
/// `texts[4].text: "one" became "two"` says what a full dump does not.
fn first_difference(
    first: &serde_json::Value,
    second: &serde_json::Value,
    path: &str,
) -> Option<String> {
    match (first, second) {
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: {} item(s) became {}", a.len(), b.len()));
            }
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                if let Some(difference) = first_difference(x, y, &format!("{path}[{index}]")) {
                    return Some(difference);
                }
            }
            None
        }
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for key in a.keys().chain(b.keys().filter(|key| !a.contains_key(*key))) {
                let x = a.get(key).unwrap_or(&serde_json::Value::Null);
                let y = b.get(key).unwrap_or(&serde_json::Value::Null);
                if let Some(difference) = first_difference(x, y, &format!("{path}.{key}")) {
                    return Some(difference);
                }
            }
            None
        }
        _ => (first != second).then(|| format!("{path}: {first} became {second}")),
    }
}

/// A legacy `.ppt` is a compound file whose record stream nothing here reads.
/// Impress writes a package the PPTX reader does read, and the result is the
/// deck the user handed over: three slides, their text, their table, their
/// picture and their **speaker notes**, all under the original file's name,
/// MIME type and hash. The notes matter in their own right: the notes parts
/// Impress writes are the same shape a native deck's are, so a bridged legacy
/// presentation keeps its remarks as notes-layer items rather than losing them
/// at the format boundary. The whole converted document is then held to the
/// native deck's, not only its notes: the bridge is a way of reaching the
/// bytes, and reaching them must not change what they say.
#[test]
fn a_legacy_ppt_is_read_through_libreoffice_as_a_deck() {
    if !libreoffice_on_path() {
        eprintln!("no soffice on PATH; skipping the .ppt bridge test");
        return;
    }

    let ws = Workspace::new("ppt-bridge");
    let source = repository_root().join("test-data/legacy_deck.ppt");
    let bytes = std::fs::read(&source).expect("the legacy deck fixture");
    let input = ws.write("legacy_deck.ppt", &bytes);
    let output = ws.join("legacy_deck.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    // Identity is the file the user handed over, not the package Impress wrote:
    // the name, the PowerPoint MIME type and the hash of the original bytes.
    assert_eq!(doc["name"], "legacy_deck.ppt");
    assert_eq!(doc["origin"]["filename"], "legacy_deck.ppt");
    assert_eq!(doc["origin"]["mimetype"], "application/vnd.ms-powerpoint");
    assert_eq!(
        doc["origin"]["binary_hash"].as_u64(),
        Some(fnv1a(&bytes)),
        "the hash is the converted package's, not the original's"
    );

    // The bridge feeds the PPTX reader, so a slide is a page and its shapes
    // are the items: three slides, their words, their table, their picture.
    assert_eq!(
        doc["pages"].as_object().map(serde_json::Map::len),
        Some(3),
        "{doc:#?}"
    );
    let texts: Vec<&str> = doc["texts"]
        .as_array()
        .expect("texts")
        .iter()
        .filter_map(|item| item["text"].as_str())
        .collect();
    assert!(texts.contains(&"Q3 Review"), "{texts:?}");
    assert!(
        !doc["tables"].as_array().expect("tables").is_empty(),
        "the deck's table did not come through: {doc:#?}"
    );

    // The deck's own remarks survive the bridge: the notes are items on the
    // notes layer, on the page of the slide they belong to, not slide content.
    let notes: Vec<&serde_json::Value> = doc["texts"]
        .as_array()
        .expect("texts")
        .iter()
        .filter(|item| item["content_layer"] == "notes")
        .collect();
    let note = |words: &str| -> &serde_json::Value {
        notes
            .iter()
            .copied()
            .find(|item| {
                item["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(words))
            })
            .unwrap_or_else(|| panic!("no note saying {words:?}: {notes:#?}"))
    };
    let opening = note("Open with the revenue line");
    assert_eq!(
        opening["text"],
        "Open with the revenue line, then the regions.\n\
         The board asked for the unit numbers first."
    );
    assert_eq!(opening["label"], "text");
    assert_eq!(opening["prov"][0]["page_no"], 1);
    assert_eq!(
        note("Say the picture is the growth")["prov"][0]["page_no"],
        3
    );
    assert!(
        !doc["texts"]
            .as_array()
            .expect("texts")
            .iter()
            .any(|item| item["content_layer"] == "body"
                && item["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("Say the picture is the growth"))),
        "a note was read as slide content: {texts:?}"
    );

    // The bridge must not change the deck. The two packages differ in what is
    // the container's — identity, re-laid-out boxes, comment ids, image
    // directories — and `deck_view` removes exactly those; every page, group,
    // item, label, layer, word, cell and picture has to be the same.
    let native_input = repository_root().join("test-data/q3_review.pptx");
    let native_output = ws.join("native.json");
    run_ok(&[
        "-i",
        native_input.to_str().unwrap(),
        "-o",
        native_output.to_str().unwrap(),
        "--ocr",
        "never",
    ]);
    let native = json_of(&native_output);
    let (bridged_view, native_view) = (deck_view(&doc), deck_view(&native));
    if bridged_view != native_view {
        let where_it_parts = first_difference(&bridged_view, &native_view, "document")
            .unwrap_or_else(|| "the views differ".to_string());
        panic!("the bridge changed the converted deck:\n  {where_it_parts}");
    }

    // The picture is the same image in both readings, byte for byte; only the
    // directory each output named it in differs.
    let picture_uri = |document: &serde_json::Value| -> String {
        document["pictures"][0]["image"]["uri"]
            .as_str()
            .expect("a picture with an image")
            .to_string()
    };
    assert_eq!(
        std::fs::read(ws.join(&picture_uri(&doc))).expect("the bridged picture"),
        std::fs::read(ws.join(&picture_uri(&native))).expect("the native picture"),
        "the bridge changed the picture's bytes"
    );

    // A picture is extracted beside the JSON, under the output's own name, so
    // the bridged deck is as complete as a native one.
    let pictures = doc["pictures"].as_array().expect("pictures");
    let uri = pictures
        .first()
        .and_then(|picture| picture["image"]["uri"].as_str())
        .expect("a picture with an image");
    assert!(ws.join(uri).is_file(), "missing the extracted {uri}");
}

/// A comment's time is written at PowerPoint's precision, and the bridge is the
/// reason that rule exists: Impress rewrites the moment with nine fractional
/// digits, so `09:15:12.125` arrives as `09:15:12.125000000`. The fixture's
/// comments carry **non-zero** milliseconds — the `.ppt` generator puts back
/// the value Impress' writer zeroes — so this holds the cut to three digits
/// rather than to a fraction of zeroes: a `comment_timestamp` that zeroed the
/// fraction, or left the package's spelling alone, would fail here.
///
/// Both sides of the pair are converted, and the whole documents are held to
/// each other the way `legacy_deck.ppt` and `q3_review.pptx` are: `deck_view`
/// removes only what belongs to the container — identity, the boxes Impress
/// re-lays out, its own comment ids — so every page, group, item, label, layer
/// and word has to be the same, and the milliseconds are then stated on their
/// own. That makes this the second document carried whole across the bridge,
/// which is what keeps a field the bridge drops from hiding behind the one
/// thing this fixture was built to show.
#[test]
fn a_bridged_comment_time_keeps_its_milliseconds() {
    if !libreoffice_on_path() {
        eprintln!("no soffice on PATH; skipping the comment-time bridge test");
        return;
    }

    let ws = Workspace::new("comment-times");
    let convert = |fixture: &str, output: &str| -> serde_json::Value {
        let source = repository_root().join("test-data").join(fixture);
        let path = ws.join(output);
        run_ok(&[
            "-i",
            source.to_str().unwrap(),
            "-o",
            path.to_str().unwrap(),
            "--ocr",
            "never",
        ]);
        json_of(&path)
    };
    let notes = |document: &serde_json::Value| -> Vec<String> {
        document["texts"]
            .as_array()
            .expect("texts")
            .iter()
            .filter(|item| item["content_layer"] == "notes")
            .map(|item| item["text"].as_str().expect("a comment's text").to_string())
            .collect()
    };

    let bridged = convert("comment_times.ppt", "bridged.json");
    // Identity is the file the user handed over, not the package Impress wrote:
    // the name, the PowerPoint MIME type and the hash of the original bytes.
    let source = std::fs::read(repository_root().join("test-data/comment_times.ppt"))
        .expect("the comment-time fixture");
    assert_eq!(bridged["name"], "comment_times.ppt");
    assert_eq!(bridged["origin"]["filename"], "comment_times.ppt");
    assert_eq!(
        bridged["origin"]["mimetype"], "application/vnd.ms-powerpoint",
        "the fixture did not go through the bridge"
    );
    assert_eq!(
        bridged["origin"]["binary_hash"].as_u64(),
        Some(fnv1a(&source)),
        "the hash is the converted package's, not the original's"
    );
    let native = convert("comment_times.pptx", "native.json");

    // One deck in two containers: the bridge may reach the bytes, it may not
    // change what they say. Only the container's own differences are removed
    // by `deck_view`, so this fails on anything else the bridge loses — an
    // item, a label, a layer, a word, a page, a group — and not only on the
    // comment times the rest of this test is about.
    let (bridged_view, native_view) = (deck_view(&bridged), deck_view(&native));
    if bridged_view != native_view {
        let where_it_parts = first_difference(&bridged_view, &native_view, "document")
            .unwrap_or_else(|| "the views differ".to_string());
        panic!("the bridge changed the comment-time deck:\n  {where_it_parts}");
    }
    assert_eq!(
        bridged["pages"].as_object().map(serde_json::Map::len),
        Some(1),
        "the fixture is one slide: {bridged:#?}"
    );

    let (bridged_notes, native_notes) = (notes(&bridged), notes(&native));
    assert_eq!(
        bridged_notes, native_notes,
        "the bridge changed what a comment's time says"
    );

    // The milliseconds are the deck's, not the bridge's: both readings say the
    // two values the fixture gave them, and neither keeps the nine digits.
    assert_eq!(bridged_notes.len(), 2, "{bridged_notes:#?}");
    for time in ["2026-10-18T09:15:12.125", "2026-10-18T16:05:42.750"] {
        assert!(
            bridged_notes
                .iter()
                .any(|note| note.contains(&format!("time: {time}]"))),
            "no note says {time}: {bridged_notes:#?}"
        );
        assert!(
            !bridged_notes
                .iter()
                .any(|note| note.contains(&format!("{time}0"))),
            "the notes layer kept the package's {time}0: {bridged_notes:#?}"
        );
    }
}

/// A legacy `.xls` is a compound file whose BIFF stream nothing here reads.
/// Calc writes the package the XLSX reader does read, and the result is the
/// workbook the user handed over: three worksheets, their tables, their merged
/// ranges, the cached result of their formula, their date and their hidden
/// sheet on the `invisible` layer, all under the original file's name, MIME
/// type and hash.
///
/// The whole converted document is then held to the native workbook's, the way
/// the legacy deck is: `sheet_view` removes only the container's identity, so
/// every sheet, its layer, every table, cell, span, word and cached value has
/// to be the same. That is what makes the bridge's spreadsheet side a
/// whole-document gate rather than a check of the few cells a fixture was built
/// to show.
#[test]
fn a_legacy_xls_is_read_through_libreoffice_as_a_workbook() {
    if !libreoffice_on_path() {
        eprintln!("no soffice on PATH; skipping the .xls bridge test");
        return;
    }

    let ws = Workspace::new("xls-bridge");
    let source = repository_root().join("test-data/ledger.xls");
    let bytes = std::fs::read(&source).expect("the legacy workbook fixture");
    let input = ws.write("ledger.xls", &bytes);
    let output = ws.join("ledger.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    // Identity is the file the user handed over, not the package Calc wrote:
    // the name, the Excel MIME type and the hash of the original bytes.
    assert_eq!(doc["name"], "ledger.xls");
    assert_eq!(doc["origin"]["filename"], "ledger.xls");
    assert_eq!(
        doc["origin"]["mimetype"], "application/vnd.ms-excel",
        "the fixture did not go through the bridge"
    );
    assert_eq!(
        doc["origin"]["binary_hash"].as_u64(),
        Some(fnv1a(&bytes)),
        "the hash is the converted package's, not the original's"
    );

    // The bridge feeds the XLSX reader, so a worksheet is a page and its cells
    // are the tables: three sheets, the hidden one on the invisible layer.
    assert_eq!(
        doc["pages"].as_object().map(serde_json::Map::len),
        Some(3),
        "{doc:#?}"
    );
    let described: Vec<(&str, &str, &str)> = doc["groups"]
        .as_array()
        .expect("a group list")
        .iter()
        .map(|group| {
            (
                group["name"].as_str().unwrap_or_default(),
                group["content_layer"].as_str().unwrap_or_default(),
                group["label"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        described,
        vec![
            ("Ledger", "body", "sheet"),
            ("Notes", "body", "sheet"),
            ("Hidden", "invisible", "sheet"),
        ]
    );

    let native_input = repository_root().join("test-data/ledger.xlsx");
    let native_output = ws.join("native.json");
    run_ok(&[
        "-i",
        native_input.to_str().unwrap(),
        "-o",
        native_output.to_str().unwrap(),
        "--ocr",
        "never",
    ]);
    let native = json_of(&native_output);

    // One workbook in two containers: the bridge may reach the bytes, it may
    // not change what they say. Only identity is removed by `sheet_view`, so
    // this fails on anything else the bridge loses — a sheet, its layer, a
    // table, a cell, a span, a word, a cached value — and not only on the
    // cells the rest of this test is about.
    let (bridged_view, native_view) = (sheet_view(&doc), sheet_view(&native));
    if bridged_view != native_view {
        let where_it_parts = first_difference(&bridged_view, &native_view, "document")
            .unwrap_or_else(|| "the views differ".to_string());
        panic!("the bridge changed the workbook:\n  {where_it_parts}");
    }

    // The facts the fixture was built to carry are stated on their own after
    // the whole-document comparison: the merged ranges keep their spans with
    // the covered slot absent, the formula's cached result is what a reader
    // sees, and the date is text rather than its serial number.
    let tables = doc["tables"].as_array().expect("a table list");
    let cell = |words: &str| -> &serde_json::Value {
        tables
            .iter()
            .flat_map(|table| table["data"]["table_cells"].as_array().expect("cells"))
            .find(|cell| cell["text"] == words)
            .unwrap_or_else(|| panic!("no cell saying {words:?}: {doc:#?}"))
    };
    assert_eq!(
        cell("Shipping included")["col_span"],
        2,
        "the column merge did not come through"
    );
    assert_eq!(
        cell("owner")["row_span"],
        2,
        "the row merge did not come through"
    );
    let notes_table = tables
        .iter()
        .find(|table| {
            table["data"]["table_cells"]
                .as_array()
                .is_some_and(|cells| cells.iter().any(|cell| cell["text"] == "owner"))
        })
        .expect("the Notes sheet's table");
    let notes_cells = notes_table["data"]["table_cells"]
        .as_array()
        .expect("cells");
    assert!(
        !notes_cells
            .iter()
            .any(|cell| cell["start_row_offset_idx"] == 2 && cell["start_col_offset_idx"] == 0),
        "the slot the row merge covers was written as a cell: {notes_cells:#?}"
    );
    assert_eq!(cell("15")["start_col_offset_idx"], 1);
    assert_eq!(cell("2024-03-05 00:00:00")["start_col_offset_idx"], 1);
}

/// A legacy `.doc` is a compound file whose binary stream the built-in reader
/// reads as words alone: it has no paragraph properties, so a heading is a
/// paragraph, a list keeps no marker, and a table is tabs. LibreOffice's Writer
/// reads the same file and writes the package the DOCX reader reads whole, so a
/// `.doc` is bridged to `docx` where LibreOffice is installed — and the document
/// the user handed over is then the document they get: headings, a hyperlink,
/// bulleted and numbered lists with their markers, a table with a column and a
/// row merge, and a picture, all under the original file's name,
/// `application/msword` and hash.
///
/// The whole converted document is then held to the native `brief.docx`'s, the
/// way the deck and workbook pairs are: `document_view` removes only the
/// container's identity and the directory each output names its pictures in, so
/// every item, label, marker, level, word, link, table cell, span and picture
/// has to agree. The gate cannot pass on the built-in reading: it is written
/// over the very features that reading loses, so a bridge that silently stopped
/// working would fail here rather than hide behind the words.
#[test]
fn a_legacy_doc_is_read_through_libreoffice_as_a_document() {
    if !libreoffice_on_path() {
        eprintln!("no soffice on PATH; skipping the .doc bridge test");
        return;
    }

    let ws = Workspace::new("doc-bridge");
    let source = repository_root().join("test-data/legacy_brief.doc");
    let bytes = std::fs::read(&source).expect("the legacy document fixture");
    let input = ws.write("legacy_brief.doc", &bytes);
    let output = ws.join("legacy_brief.json");

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--ocr",
        "never",
    ]);

    let doc = json_of(&output);
    // Identity is the file the user handed over, not the package Writer wrote:
    // the name, the Word MIME type and the hash of the original bytes.
    assert_eq!(doc["name"], "legacy_brief.doc");
    assert_eq!(doc["origin"]["filename"], "legacy_brief.doc");
    assert_eq!(
        doc["origin"]["mimetype"], "application/msword",
        "the fixture did not keep its own identity"
    );
    assert_eq!(
        doc["origin"]["binary_hash"].as_u64(),
        Some(fnv1a(&bytes)),
        "the hash is the converted package's, not the original's"
    );

    // The bridge feeds the DOCX reader, so the fixture's features are the
    // things that reader reads and the built-in one cannot: a heading is a
    // section header, the two lists keep their markers, the table keeps both
    // merges, and the picture is a picture rather than a placeholder.
    let texts = doc["texts"].as_array().expect("a text list");
    assert_eq!(texts.len(), 12, "{doc:#?}");
    let labelled = |words: &str, label: &str| {
        assert!(
            texts
                .iter()
                .any(|item| item["text"] == words && item["label"] == label),
            "no {label} saying {words:?}: {texts:#?}"
        );
    };
    labelled("Quarterly Brief", "section_header");
    labelled("Summary", "section_header");
    labelled("Supply is tight.", "paragraph");
    let link = texts
        .iter()
        .find(|item| item["text"] == "The filing")
        .expect("the hyperlink's words");
    assert_eq!(link["hyperlink"], "https://example.com/filing");
    let markers: Vec<&str> = texts
        .iter()
        .filter(|item| item["label"] == "list_item")
        .filter_map(|item| item["marker"].as_str())
        .collect();
    assert_eq!(markers, vec!["•", "•", "1.", "2."], "{texts:#?}");

    let tables = doc["tables"].as_array().expect("a table list");
    assert_eq!(tables.len(), 1, "{doc:#?}");
    let cells = tables[0]["data"]["table_cells"].as_array().expect("cells");
    let cell = |words: &str| -> &serde_json::Value {
        cells
            .iter()
            .find(|cell| cell["text"] == words)
            .unwrap_or_else(|| panic!("no cell saying {words:?}: {doc:#?}"))
    };
    assert_eq!(cell("Revenue")["col_span"], 2, "the column merge");
    assert_eq!(cell("North South")["row_span"], 2, "the row merge");
    assert_eq!(
        tables[0]["data"]["num_cols"], 3,
        "the bridged table lost a column"
    );

    let pictures = doc["pictures"].as_array().expect("a picture list");
    assert_eq!(pictures.len(), 1, "{doc:#?}");
    let uri = pictures[0]["image"]["uri"]
        .as_str()
        .expect("the bridged picture keeps its image");
    assert!(ws.join(uri).is_file(), "missing the extracted {uri}");

    // One document in two containers: the bridge may reach the bytes, it may
    // not change what they say. Only identity and the image directory are
    // removed by `document_view`, so this fails on anything else the bridge
    // loses — an item, a label, a marker, a level, a word, a link, a cell, a
    // span, a picture.
    let native_input = repository_root().join("test-data/brief.docx");
    let native_output = ws.join("native.json");
    run_ok(&[
        "-i",
        native_input.to_str().unwrap(),
        "-o",
        native_output.to_str().unwrap(),
        "--ocr",
        "never",
    ]);
    let native = json_of(&native_output);
    let (bridged_view, native_view) = (document_view(&doc), document_view(&native));
    if bridged_view != native_view {
        let where_it_parts = first_difference(&bridged_view, &native_view, "document")
            .unwrap_or_else(|| "the views differ".to_string());
        panic!("the bridge changed the converted document:\n  {where_it_parts}");
    }

    // The picture is the same image in both readings, byte for byte; only the
    // directory each output named it in differs.
    let picture_uri = |document: &serde_json::Value| -> String {
        document["pictures"][0]["image"]["uri"]
            .as_str()
            .expect("a picture with an image")
            .to_string()
    };
    assert_eq!(
        std::fs::read(ws.join(&picture_uri(&doc))).expect("the bridged picture"),
        std::fs::read(ws.join(&picture_uri(&native))).expect("the native picture"),
        "the bridge changed the picture's bytes"
    );
}

/// Without LibreOffice a legacy workbook names what it is, what it needs, and
/// the one action that needs nothing installed.
#[test]
fn a_legacy_xls_names_libreoffice_when_it_is_absent() {
    let ws = Workspace::new("xls-missing");
    let mut bytes = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    for stream in ["Workbook", "Book"] {
        bytes.extend(stream.encode_utf16().flat_map(u16::to_le_bytes));
        bytes.extend_from_slice(&[0, 0]);
    }
    let input = ws.write("old.xls", &bytes);

    let out = run_without_libreoffice(&["-i", input.to_str().unwrap()]);

    assert!(!out.status.success(), "a legacy workbook was accepted");
    let logs = logs_of(&out);
    assert!(
        logs.contains("legacy Excel workbook"),
        "the error does not name the format: {logs}"
    );
    assert!(
        logs.contains("LibreOffice"),
        "the error does not name what it needs: {logs}"
    );
    assert!(
        logs.contains("re-save it as .xlsx"),
        "the error offers no way out: {logs}"
    );
    // The path is said once, by the run's own error line, not twice.
    assert_eq!(logs.matches("old.xls").count(), 1, "{logs}");
}

/// The bridge is only how the bytes are reached: a `.ppt` that is really a
/// PPTX package is read by the PPTX reader without LibreOffice at all.
#[test]
fn a_pptx_named_ppt_is_read_as_a_pptx() {
    let ws = Workspace::new("ppt-named-pptx");
    let source = repository_root().join("test-data/q3_review.pptx");
    let bytes = std::fs::read(&source).expect("the deck fixture");
    let input = ws.write("deck.ppt", &bytes);
    let output = ws.join("deck.json");

    let out = run_without_libreoffice(&[
        "-i",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let doc = json_of(&output);
    // A native deck is named by its stem; the file name is the origin's.
    assert_eq!(doc["name"], "deck");
    assert_eq!(doc["origin"]["filename"], "deck.ppt");
    assert_eq!(
        doc["origin"]["mimetype"],
        "application/vnd.openxmlformats-officedocument.presentationml.presentation"
    );
    assert_eq!(doc["pages"].as_object().map(serde_json::Map::len), Some(3));
}

/// Without LibreOffice a legacy deck names what it is, what it needs, and the
/// one action that needs nothing installed.
#[test]
fn a_legacy_ppt_names_libreoffice_when_it_is_absent() {
    let ws = Workspace::new("ppt-missing");
    let mut bytes = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    for stream in ["PowerPoint Document", "Current User"] {
        bytes.extend(stream.encode_utf16().flat_map(u16::to_le_bytes));
        bytes.extend_from_slice(&[0, 0]);
    }
    let input = ws.write("old.ppt", &bytes);

    let out = run_without_libreoffice(&["-i", input.to_str().unwrap()]);

    assert!(!out.status.success(), "a legacy deck was accepted");
    let logs = logs_of(&out);
    assert!(
        logs.contains("legacy PowerPoint presentation"),
        "the error does not name the format: {logs}"
    );
    assert!(
        logs.contains("LibreOffice"),
        "the error does not name what it needs: {logs}"
    );
    assert!(
        logs.contains("re-save it as .pptx"),
        "the error offers no way out: {logs}"
    );
    // The path is said once, by the run's own error line, not twice.
    assert_eq!(logs.matches("old.ppt").count(), 1, "{logs}");
}
