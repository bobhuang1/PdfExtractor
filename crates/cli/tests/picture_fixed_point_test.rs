//! A fixed-point gate for the pictures of every golden document.
//!
//! A writer has three ways to show a picture: point at the file beside the page
//! (the default, `referenced`), carry the picture's own bytes in the page as a
//! `data:` URI (`--image-export-mode embedded`), or mark the spot with an
//! invisible comment and name nothing (`placeholder`). The first two are one
//! picture seen two ways, and this gate holds every committed golden to that.
//! Each golden is rendered both ways; the embedded page is read back and the
//! document is rendered embedded again. Three things must hold, and they are
//! one fact from three sides:
//!
//! * the embedded page carries each picture's **bytes**, not a re-encoding and
//!   not merely the right number of pictures;
//! * reading it back decodes those bytes into pictures, and the files that
//!   re-read document writes are byte for byte the referenced render's;
//! * rendering the re-read document embedded reproduces the embedded page, so
//!   the round trip is a fixed point rather than a re-render that settles
//!   somewhere else.
//!
//! Both writers with an image mode are walked — the Markdown goldens and the
//! HTML goldens — so a golden joins the gate by being committed. The pages are
//! compared entire, `<head>` and `<title>` included: the title is the
//! document's own, so reading a page back and rendering it again is the same
//! bytes rather than the same page under the new file's name.
//!
//! Every golden is also rendered `placeholder`. That page has to mark every
//! picture the document holds — a drawing with no bytes is a picture too, and
//! the referenced page writes nothing for it, so the mark is the only trace it
//! leaves — and no image syntax may survive: no file reference, no `data:` URI,
//! no `![…](…)` or `<img>`. Where every picture does have a file, the page is
//! held to something stronger than the count: the referenced page with each
//! picture block replaced by the mark, byte for byte, so a mark cannot count
//! correctly in the wrong place. Where a picture has no bytes — `chart_deck.pptx`
//! is the fixture whose only picture is a chart, a drawing of its data with no
//! media part anywhere in the package — there is no block to stand in for, and
//! the correspondence is drawn between the pages' blocks instead: setting the
//! marks aside must leave the referenced page's blocks, so a mark can neither
//! stand over a word nor hide one.
//!
//! The placeholder pass reads the document a golden was rendered *from*, when
//! that document is in the repository, rather than the golden page. A referenced
//! page has already forgotten a drawing with no bytes — it wrote no block to
//! point at — so a golden read back cannot say the mark belongs there; the
//! source still can. The golden itself remains the input for the other three
//! renders, and for a document that is not beside the corpus.
//!
//! Like every golden gate here, the real binary does the work and OCR is off:
//! recognised text depends on the installed Tesseract's language data.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The two writers that take an image mode, and the golden directory each is
/// pinned under.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Markdown,
    Html,
}

impl Kind {
    /// The directory under `test-data/golden` these goldens sit in.
    fn directory(self) -> &'static str {
        match self {
            Kind::Markdown => "markdown",
            Kind::Html => "html",
        }
    }

    /// The extension the goldens and their renderings are written under.
    fn extension(self) -> &'static str {
        match self {
            Kind::Markdown => "md",
            Kind::Html => "html",
        }
    }

    /// The `--to` value that asks for this writer.
    fn format(self) -> &'static str {
        match self {
            Kind::Markdown => "md",
            Kind::Html => "html",
        }
    }

    /// How a failure names this writer.
    fn label(self) -> &'static str {
        match self {
            Kind::Markdown => "Markdown",
            Kind::Html => "HTML",
        }
    }
}

/// One picture of a render: what the document says about it, and the bytes the
/// run wrote beside the page.
///
/// `width` and `height` are the reported size's `f64` bits, so a picture is
/// ordered and compared exactly as the document states it (`Ord` is what makes
/// the comparison a multiset rather than an order-sensitive list).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Picture {
    mimetype: String,
    width: u64,
    height: u64,
    bytes: Vec<u8>,
}

/// The size, type and byte count of each picture — a failure's readable form,
/// where dumping the bytes would bury the fact that moved.
fn describe(pictures: &[Picture]) -> String {
    if pictures.is_empty() {
        return "no pictures".to_string();
    }
    pictures
        .iter()
        .map(|picture| {
            format!(
                "{} {}x{} ({} bytes)",
                picture.mimetype,
                f64::from_bits(picture.width),
                f64::from_bits(picture.height),
                picture.bytes.len()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a document says about its pictures, in a stable order: the side of the
/// comparison that must agree even before the bytes do.
fn facts(pictures: &[Picture]) -> Vec<(String, u64, u64)> {
    let mut facts: Vec<(String, u64, u64)> = pictures
        .iter()
        .map(|picture| (picture.mimetype.clone(), picture.width, picture.height))
        .collect();
    facts.sort();
    facts
}

/// The bytes of the `data:` URIs a rendered page carries, in page order.
///
/// Both writers spell a picture as `data:image/<type>;base64,<payload>`, in a
/// Markdown destination ending at `)` or an HTML attribute ending at `"`. The
/// payload's alphabet has neither, so the URI ends at whichever comes first.
fn embedded_payloads(page: &str) -> Vec<Vec<u8>> {
    let mut payloads = Vec::new();
    let mut rest = page;
    while let Some(at) = rest.find("data:image/") {
        let after = &rest[at..];
        let end = after.find([')', '"', '\'']).unwrap_or(after.len());
        let uri = &after[..end];
        let (header, payload) = uri
            .split_once(',')
            .unwrap_or_else(|| panic!("a data URI with no payload: {uri}"));
        assert!(
            header.to_ascii_lowercase().ends_with(";base64"),
            "a picture carried in a form this gate does not read: {header}"
        );
        payloads.push(
            STANDARD
                .decode(payload)
                .unwrap_or_else(|e| panic!("a data URI whose payload does not decode: {e}")),
        );
        rest = &after[end..];
    }
    payloads
}

/// The first differing line with a little context, so a failure says what
/// moved rather than only that something did.
fn difference(want: &str, got: &str) -> String {
    let want_lines: Vec<&str> = want.lines().collect();
    let got_lines: Vec<&str> = got.lines().collect();
    for index in 0..want_lines.len().max(got_lines.len()) {
        let expected = want_lines.get(index).copied();
        let actual = got_lines.get(index).copied();
        if expected != actual {
            let from = index.saturating_sub(2);
            let context = |lines: &[&str]| {
                (from..(index + 3).min(lines.len()))
                    .map(|i| format!("{:>5} {}", i + 1, lines[i]))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            return format!(
                "first difference at line {}:\nembedded page:\n{}\nre-rendered page:\n{}",
                index + 1,
                context(&want_lines),
                context(&got_lines)
            );
        }
    }
    format!(
        "the lines agree but the endings do not ({} bytes re-rendered, {} embedded)",
        got.len(),
        want.len()
    )
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A directory of this test's own, removed by the caller when it is done.
fn scratch() -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("pdfe-picture-gate-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create scratch dir");
    path
}

/// Every committed golden document of one writer, in a stable order: the gate
/// discovers its cases from the golden directory, so a committed golden is
/// covered without a list to keep in step.
fn golden_documents(kind: Kind) -> Vec<PathBuf> {
    let directory = repository_root()
        .join("test-data/golden")
        .join(kind.directory());
    let mut documents: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|e| panic!("list {}: {e}", directory.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension == kind.extension())
        })
        .collect();
    documents.sort();
    documents
}

/// The document a golden was rendered from, when it is in the repository: the
/// corpus first, then the fixtures beside it, found by the file stem the golden
/// is named after — the same lookup the Docling comparison uses, so a document
/// joins both gates by being committed.
fn source_document(stem: &str) -> Option<PathBuf> {
    let root = repository_root().join("test-data");
    for directory in [root.join("real"), root] {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let found = entries.flatten().map(|entry| entry.path()).find(|path| {
            path.is_file()
                && path
                    .file_stem()
                    .is_some_and(|name| name.to_string_lossy() == stem)
        });
        if let Some(path) = found {
            return Some(path);
        }
    }
    None
}

/// The mark both writers leave where a picture stood in `placeholder` mode.
///
/// Taken from the writer rather than written out here: the gate is about what
/// a run actually writes, and a second spelling could agree with itself while
/// the pages stopped agreeing with the gate.
const PLACEHOLDER: &str = pdfextractor_md_exporter::IMAGE_PLACEHOLDER;

/// Convert `input` with the real binary and read the JSON it wrote.
///
/// `image_mode` is the `--image-export-mode` value to ask for, or `None` for
/// the default `referenced`; the JSON is asked for either way, because that is
/// where a picture's media type and size are stated.
fn convert(input: &Path, output: &Path, kind: Kind, image_mode: Option<&str>) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"));
    command.args([
        "-i",
        input.to_str().expect("input path is utf-8"),
        "-o",
        output.to_str().expect("output path is utf-8"),
        "--to",
        "json",
        "--to",
        kind.format(),
        "--ocr",
        "never",
        // The goldens are generated with the notes layer, and a deck's
        // picture may sit in a remark: reading without it would leave that
        // picture out of the round trip.
        "--include-notes",
    ]);
    if let Some(mode) = image_mode {
        command.args(["--image-export-mode", mode]);
    }
    let result = command.output().expect("run the CLI binary");
    assert!(
        result.status.success(),
        "converting {} with --image-export-mode {} failed\nstdout={}\nstderr={}",
        input.display(),
        image_mode.unwrap_or("referenced"),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_str(&std::fs::read_to_string(output).expect("the JSON written"))
        .expect("valid JSON")
}

/// The pictures a run's document holds, with the bytes the run wrote for them
/// (resolved against the directory the output sits in).
fn pictures_of(document: &serde_json::Value, directory: &Path) -> Vec<Picture> {
    let mut pictures = Vec::new();
    for picture in document["pictures"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let image = &picture["image"];
        // A drawing with no bytes is no picture to carry, and writes no
        // `data:` URI either, so it is not part of this comparison.
        let Some(uri) = image["uri"].as_str() else {
            continue;
        };
        let bytes = std::fs::read(directory.join(uri)).unwrap_or_else(|e| {
            panic!("the picture {uri} the JSON names was not written beside it: {e}")
        });
        pictures.push(Picture {
            mimetype: image["mimetype"].as_str().unwrap_or_default().to_string(),
            width: image["size"]["width"]
                .as_f64()
                .unwrap_or_default()
                .to_bits(),
            height: image["size"]["height"]
                .as_f64()
                .unwrap_or_default()
                .to_bits(),
            bytes,
        });
    }
    pictures
}

/// Every golden document is rendered referenced and embedded, the embedded
/// page is read back, and the pictures must be the referenced render's — the
/// same count, the same media types and sizes, the same bytes — with the
/// re-render reproducing the embedded page.
#[test]
fn every_golden_renders_embedded_to_its_referenced_pictures() {
    let root = scratch();
    let mut with_pictures: BTreeSet<String> = BTreeSet::new();
    let mut with_titles: BTreeSet<String> = BTreeSet::new();
    let mut with_byteless: BTreeSet<String> = BTreeSet::new();
    let mut total_pictures = 0usize;

    for kind in [Kind::Markdown, Kind::Html] {
        let documents = golden_documents(kind);
        assert!(!documents.is_empty(), "no {} goldens to hold", kind.label());

        for golden in documents {
            let stem = golden
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("a golden file name");
            // A directory of its own, so this document's pictures cannot be
            // mistaken for another's — and so the input's own referenced files
            // stay where the golden names them.
            let directory = root.join(format!("{}-{stem}", kind.directory()));
            std::fs::create_dir_all(&directory).expect("create the case directory");

            // The referenced render: the default mode, which writes each
            // picture beside the page and names it there.
            let referenced = convert(&golden, &directory.join("referenced.json"), kind, None);
            let referenced_pictures = pictures_of(&referenced, &directory);
            let referenced_page =
                std::fs::read_to_string(directory.join(format!("referenced.{}", kind.extension())))
                    .expect("the referenced page");

            // The embedded render of the same document: the same pictures,
            // carried in the page itself.
            let embedded = convert(
                &golden,
                &directory.join("embedded.json"),
                kind,
                Some("embedded"),
            );
            let embedded_pictures = pictures_of(&embedded, &directory);
            let embedded_page =
                std::fs::read_to_string(directory.join(format!("embedded.{}", kind.extension())))
                    .expect("the embedded page");

            assert_eq!(
                facts(&embedded_pictures),
                facts(&referenced_pictures),
                "{} {stem}: the embedded render reports different pictures\n  embedded:   {}\n  referenced: {}",
                kind.label(),
                describe(&embedded_pictures),
                describe(&referenced_pictures)
            );

            // The page carries the referenced render's bytes, in a `data:`
            // URI, and names no file: a page that still pointed at `…_images/`
            // would need the files beside it to show anything at all.
            let mut carried = embedded_payloads(&embedded_page);
            let mut referenced_bytes: Vec<Vec<u8>> = referenced_pictures
                .iter()
                .map(|picture| picture.bytes.clone())
                .collect();
            carried.sort();
            referenced_bytes.sort();
            assert_eq!(
                carried.len(),
                referenced_bytes.len(),
                "{} {stem}: the embedded page carries {} picture(s), the referenced render wrote {}\n  embedded:   {}\n  referenced: {}",
                kind.label(),
                carried.len(),
                referenced_bytes.len(),
                describe(&embedded_pictures),
                describe(&referenced_pictures)
            );
            for (index, (carried, referenced)) in carried.iter().zip(&referenced_bytes).enumerate()
            {
                assert!(
                    carried == referenced,
                    "{} {stem}: picture {index} of the embedded page is not the bytes the referenced render wrote ({} bytes embedded, {} referenced)",
                    kind.label(),
                    carried.len(),
                    referenced.len()
                );
            }
            assert!(
                !embedded_page.contains("_images/"),
                "{} {stem}: the embedded page still names a picture file:\n{embedded_page}",
                kind.label()
            );

            // Reading it back has to decode those bytes into pictures again,
            // and the copy the reader writes must be the referenced render's.
            let page_path = directory.join(format!("embedded.{}", kind.extension()));
            let back = convert(
                &page_path,
                &directory.join("back.json"),
                kind,
                Some("embedded"),
            );
            let back_pictures = pictures_of(&back, &directory);
            assert_eq!(
                facts(&back_pictures),
                facts(&referenced_pictures),
                "{} {stem}: reading its embedded page back changed the pictures\n  read back:  {}\n  referenced: {}",
                kind.label(),
                describe(&back_pictures),
                describe(&referenced_pictures)
            );
            let mut back_bytes: Vec<Vec<u8>> = back_pictures
                .iter()
                .map(|picture| picture.bytes.clone())
                .collect();
            back_bytes.sort();
            assert!(
                back_bytes == referenced_bytes,
                "{} {stem}: the pictures read back are not the referenced render's\n  read back:  {}\n  referenced: {}",
                kind.label(),
                describe(&back_pictures),
                describe(&referenced_pictures)
            );

            // And the page that came back is the page that was read: an
            // embedded picture is carried as written rather than encoded again,
            // so the second rendering settles instead of drifting.
            let back_page =
                std::fs::read_to_string(directory.join(format!("back.{}", kind.extension())))
                    .expect("the re-rendered page");
            // The page's title is the document's, so it comes back with the
            // page: a titled golden here is what proves that half of the gate.
            let titled = !described_title(&embedded_page).is_empty();

            let (wanted, re_rendered) = (embedded_page, back_page);
            assert!(
                re_rendered == wanted,
                "{} {stem}: its embedded page is not a fixed point\n{}",
                kind.label(),
                difference(&wanted, &re_rendered)
            );

            // The placeholder render: a page that names nothing, for a reader
            // who wants the words and the places pictures stood. It is asked of
            // the document the golden was rendered from where that document is
            // in the repository, because a referenced page has already
            // forgotten a drawing with no bytes — it wrote no block to point
            // at — so the golden alone cannot say the mark belongs there.
            let source = source_document(stem);
            let placeholder_input = source.as_deref().unwrap_or(&golden);
            let placeholder_referenced = match &source {
                Some(_) => convert(
                    placeholder_input,
                    &directory.join("source.json"),
                    kind,
                    None,
                ),
                None => referenced.clone(),
            };
            let placeholder_referenced_pictures = pictures_of(&placeholder_referenced, &directory);
            let placeholder_referenced_page = match &source {
                Some(_) => {
                    std::fs::read_to_string(directory.join(format!("source.{}", kind.extension())))
                        .expect("the referenced page of the document the golden came from")
                }
                None => referenced_page.clone(),
            };
            convert(
                placeholder_input,
                &directory.join("placeholder.json"),
                kind,
                Some("placeholder"),
            );
            let placeholder_page = std::fs::read_to_string(
                directory.join(format!("placeholder.{}", kind.extension())),
            )
            .expect("the placeholder page");
            let held = picture_count(&placeholder_referenced);
            let marked = placeholder_page.matches(PLACEHOLDER).count();
            assert_eq!(
                marked,
                held,
                "{} {stem}: the placeholder page marks {marked} picture(s), the document holds {held}\n{placeholder_page}",
                kind.label()
            );
            for syntax in image_syntax(kind) {
                assert!(
                    !placeholder_page.contains(syntax),
                    "{} {stem}: the placeholder page keeps image syntax `{syntax}`\n{placeholder_page}",
                    kind.label()
                );
            }
            if placeholder_referenced_pictures.len() == held {
                // Stronger than the count where it can be drawn: the marks are
                // where the pictures were.
                let replaced = without_pictures(&placeholder_referenced_page, kind);
                assert!(
                    replaced == placeholder_page,
                    "{} {stem}: the placeholder page is not the referenced page with its pictures marked\n{}",
                    kind.label(),
                    difference(&replaced, &placeholder_page)
                );
            } else {
                // A drawing with no bytes wrote no block, so its mark has no
                // block to stand in for: it is an insertion, and the two pages
                // are held together block by block instead, so the mark still
                // has to sit between exactly the blocks the drawing sat
                // between and no word may be lost, gained or moved.
                assert_marks_stand_where_the_pictures_did(
                    &format!("{} {stem}", kind.label()),
                    kind,
                    &placeholder_referenced_page,
                    &placeholder_page,
                );
                with_byteless.insert(format!("{}/{stem}", kind.directory()));
            }

            if titled {
                with_titles.insert(format!("{}/{stem}", kind.directory()));
            }

            if !referenced_pictures.is_empty() {
                with_pictures.insert(format!("{}/{stem}", kind.directory()));
                total_pictures += referenced_pictures.len();
            }
        }
    }

    let _ = std::fs::remove_dir_all(&root);

    // Without this the gate could pass by having nothing to hold: the goldens
    // that carry pictures must still carry them, and both writers must be in
    // the set.
    assert!(
        total_pictures > 0,
        "no golden document yielded a picture, so the gate checked nothing"
    );
    for expected in [
        "markdown/pictures",
        "markdown/calibre_demo",
        "markdown/q3_review",
        "html/pictures",
        "html/calibre_demo",
    ] {
        assert!(
            with_pictures.contains(expected),
            "{expected} yields no picture any more; the embedded gate is not covering it"
        );
    }
    // A mark that replaces no block is the branch a referenced page cannot show,
    // so without a document whose picture has no bytes the block-for-block
    // correspondence would never run: the fixture that carries one must keep it.
    for expected in ["markdown/chart_deck", "html/chart_deck"] {
        assert!(
            with_byteless.contains(expected),
            "{expected} no longer has a picture without bytes, so the placeholder \
             correspondence for one is not covered; covered: {with_byteless:?}"
        );
    }
    // The page's title is compared along with the rest, so the gate must have a
    // titled document to compare: an untitled one would agree about an empty
    // title on both sides and prove nothing about where the title comes from.
    assert!(
        with_titles.contains("html/pictures"),
        "html/pictures writes no title any more, so the title half of the fixed \
         point is not covered; titled: {with_titles:?}"
    );
}

/// The words a rendered page puts in its `<title>`, or `""` for none.
fn described_title(page: &str) -> String {
    page.split_once("<title>")
        .and_then(|(_, rest)| rest.split_once("</title>"))
        .map(|(title, _)| title.to_string())
        .unwrap_or_default()
}

/// How many pictures the document holds, including a drawing with no bytes —
/// the placeholder page marks that one too, though no other mode shows it.
fn picture_count(document: &serde_json::Value) -> usize {
    document["pictures"].as_array().map_or(0, Vec::len)
}

/// The image syntax a placeholder page must not keep: the file reference, the
/// carried bytes, and the element or link each writer would have written.
fn image_syntax(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Markdown => &["![Image](", "_images/", "data:image/"],
        Kind::Html => &["<img", "_images/", "data:image/"],
    }
}

/// The page with every picture block replaced by the placeholder mark, which is
/// what the placeholder render has to be.
fn without_pictures(page: &str, kind: Kind) -> String {
    let mut out = String::with_capacity(page.len());
    for line in page.split_inclusive('\n') {
        if is_picture_block(line.trim_end_matches('\n'), kind) {
            out.push_str(PLACEHOLDER);
            if line.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

/// A page as its blocks: one per blank-line-separated paragraph in Markdown
/// (the writer separates blocks with exactly one blank line), one per line in
/// HTML (the writer puts each block on its own line).
///
/// The correspondence for a picture with no bytes is drawn between these lists
/// rather than between the pages' bytes, because the mark it gains replaces no
/// block: the referenced page wrote nothing for it. Comparing lists keeps the
/// block separator out of the question, which is what makes the two sides
/// comparable at all.
fn blocks(page: &str, kind: Kind) -> Vec<String> {
    match kind {
        Kind::Markdown => page
            .split("\n\n")
            .map(|block| block.trim_end_matches('\n').to_string())
            .filter(|block| !block.is_empty())
            .collect(),
        Kind::Html => page
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

/// Walk the referenced page's blocks beside the placeholder page's and require
/// that the only difference is a mark where a picture was: a mark may stand in
/// for a picture block, and a mark may be inserted where a drawing with no
/// bytes stood, but every other block has to be the same block in the same
/// place. The walk is what makes the check positional rather than a count of
/// two sets — a mark at the top of the page for a picture at the bottom fails
/// here, because the blocks between them no longer line up.
fn assert_marks_stand_where_the_pictures_did(
    case: &str,
    kind: Kind,
    referenced_page: &str,
    placeholder_page: &str,
) {
    let referenced = blocks(referenced_page, kind);
    let placeholder = blocks(placeholder_page, kind);
    let (mut at, mut marked) = (0usize, 0usize);
    while at < referenced.len() || marked < placeholder.len() {
        let picture = referenced.get(at);
        let mark = placeholder.get(marked);
        let is_mark = mark.is_some_and(|block| block.trim() == PLACEHOLDER);
        match (picture, mark) {
            // The same block on both pages: words, headings, group markers, and
            // whatever the writer put around them.
            (Some(picture), Some(text)) if picture == text => {
                at += 1;
                marked += 1;
            }
            // A mark in place of the block a picture with a file left behind.
            (Some(picture), Some(_)) if is_picture_block(picture, kind) && is_mark => {
                at += 1;
                marked += 1;
            }
            // A mark inserted where a drawing with no bytes stood: the
            // referenced page wrote no block there to stand in for.
            (Some(_), Some(_)) if is_mark => marked += 1,
            (None, Some(_)) if is_mark => marked += 1,
            _ => panic!(
                "{case}: the placeholder page does not stand where the referenced page put its pictures\n\
                 --- referenced ---\n{}\n--- placeholder ---\n{}",
                near(&referenced, at),
                near(&placeholder, marked)
            ),
        }
    }
}

/// The blocks around an index, so a failure shows where the walk stopped
/// rather than the whole page.
fn near(blocks: &[String], at: usize) -> String {
    let from = at.saturating_sub(2);
    let to = (at + 3).min(blocks.len());
    if from == to {
        return "(nothing left)".to_string();
    }
    (from..to)
        .map(|index| {
            let arrow = if index == at { "→" } else { " " };
            format!("{arrow} {index}: {}", blocks[index])
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a line is one picture and nothing else, as each writer spells it.
fn is_picture_block(line: &str, kind: Kind) -> bool {
    match kind {
        Kind::Markdown => line.starts_with("![Image](") && line.ends_with(')'),
        Kind::Html => line.starts_with("<p><img src=\"") && line.ends_with("</p>"),
    }
}

#[cfg(test)]
mod correspondence {
    use super::*;

    /// The mark a drawing with no bytes gains stands between the blocks it sat
    /// between on the referenced page — which wrote nothing for it.
    #[test]
    fn a_drawings_mark_may_be_inserted_between_the_blocks_it_sat_between() {
        assert_marks_stand_where_the_pictures_did(
            "case",
            Kind::Markdown,
            "# Title\n\nWords.\n\nLast words.",
            "# Title\n\nWords.\n\n<!-- image -->\n\nLast words.",
        );
    }

    /// A picture with a file leaves a block, and the mark has to take its place:
    /// a page that marks a different spot is not the same page with the
    /// picture's block marked.
    #[test]
    #[should_panic(expected = "does not stand where the referenced page put its pictures")]
    fn a_mark_at_the_top_is_not_a_mark_where_the_picture_stood() {
        assert_marks_stand_where_the_pictures_did(
            "case",
            Kind::Markdown,
            "# Title\n\nWords.\n\n![Image](picture.png)",
            "# Title\n\n<!-- image -->\n\nWords.",
        );
    }

    /// Nothing may hide under a mark: a page that lost a block has a mark with
    /// no block to stand in for.
    #[test]
    #[should_panic(expected = "does not stand where the referenced page put its pictures")]
    fn a_mark_cannot_stand_in_for_a_lost_paragraph() {
        assert_marks_stand_where_the_pictures_did(
            "case",
            Kind::Markdown,
            "# Title\n\nWords.",
            "# Title\n\n<!-- image -->",
        );
    }

    /// The HTML walk reads one block per line, so a mark between two elements is
    /// an insertion there too, not a swallowed paragraph.
    #[test]
    #[should_panic(expected = "does not stand where the referenced page put its pictures")]
    fn an_html_mark_cannot_replace_a_paragraph() {
        assert_marks_stand_where_the_pictures_did(
            "case",
            Kind::Html,
            "<body>\n<p>Words.</p>\n</body>",
            "<body>\n<!-- image -->\n</body>",
        );
    }
}
