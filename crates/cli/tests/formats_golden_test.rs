//! Golden tests for the HTML, text and YAML exports.
//!
//! Three more renderings of the same documents the Markdown goldens pin,
//! each compared byte for byte with a committed expectation under
//! `test-data/golden/{html,text,yaml}/`, each produced by the real binary —
//! so the goldens cover the writer *and* the CLI wiring that reaches it. As
//! with every golden in this repository, a change that moves any byte of any
//! document fails here first, and the diff of the golden files is the review:
//!
//!     UPDATE_GOLDEN_FORMATS=1 cargo test -p pdfextractor-cli --test formats_golden_test
//!
//! OCR is off for every case — recognised text depends on the installed
//! Tesseract's language data, which would make byte-exact expectations flaky
//! on machines that are otherwise correct — and `--to text` runs with its
//! documented default, hyphenation joining on.

mod common;

use common::{Pins, Sweep};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Documents whose renderings are pinned, byte for byte: the documents the
/// Markdown goldens share, plus `pictures.md`, whose picture the three other
/// writers each have to place — so one document's formats and its Markdown can
/// be reviewed side by side — `chart_deck.pptx`, whose only picture is a
/// drawing with no file, so the writers have nothing to place and must not
/// invent a link to nothing, `social_cards.html`, whose head facts are not
/// words in the body at all, so the HTML golden is what pins them, and
/// `picture_names.html`, whose picture's file name carries a space and an
/// ampersand, so the HTML golden pins the name the writer escapes.
const CASES: &[&str] = &[
    "hello.pdf",            // the smallest fixture: one heading-sized line
    "dummy.pdf",            // a one-paragraph PDF
    "calibre_demo.docx",    // prose, lists, tables and pictures in one document
    "math-accents.docx",    // Office Math, which becomes `$$` in Markdown
    "irs_f1040.pdf",        // a long form: many tables, columns and furniture
    "flow.html",            // a flow format: headings, lists, code and a table
    "late_header.html",     // a table whose header row is declared below a caption
    "social_cards.html",    // the head's description, canonical URL and social cards
    "picture_names.html",   // a picture named with a space and an ampersand
    "fenced.md",            // Markdown in, HTML/text/YAML out
    "pictures.md",          // Markdown in with a picture: its file is copied and referenced
    "merged_cells.docx",    // a DOCX whose table merges cells: rowspan/colspan in HTML
    "merged_table.rtf",     // the same merged table as a word processor's own RTF export
    "link_annotations.pdf", // a PDF whose link is an `<a href>` in the HTML
    "chart_deck.pptx",      // a deck whose only picture is a chart: no file to place
    "rtf_bitmap.rtf",       // a word processor's own RTF export, control words and all
];

/// What is pinned and under which name: the `--to` value (never the
/// extension, so a format whose file name changes still fails the same way)
/// and the file extension it is written under.
const FORMATS: &[(&str, &str)] = &[("html", "html"), ("text", "txt"), ("yaml", "yaml")];

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn input_of(name: &str) -> PathBuf {
    let root = repository_root().join("test-data");
    match name {
        "hello.pdf"
        | "flow.html"
        | "late_header.html"
        | "social_cards.html"
        | "picture_names.html"
        | "fenced.md"
        | "pictures.md"
        | "merged_cells.docx"
        | "merged_table.rtf"
        | "link_annotations.pdf"
        | "chart_deck.pptx"
        | "rtf_bitmap.rtf" => root.join(name),
        _ => root.join("real").join(name),
    }
}

fn stem_of(name: &str) -> String {
    name.rsplit_once('.').unwrap_or((name, "")).0.to_string()
}

fn golden_of(format: &str, ext: &str, name: &str) -> PathBuf {
    repository_root()
        .join("test-data/golden")
        .join(format)
        .join(format!("{}.{}", stem_of(name), ext))
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn scratch(tag: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("pdfe-fmt-{}-{}-{tag}", std::process::id(), unique));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create scratch dir");
    path
}

/// Convert one document with the real binary and read `extension` back.
/// Extra arguments (for the stdout tests) are appended, so every conversion
/// here goes through the interface a reader uses.
fn convert(name: &str, format: &str, extension: &str, extra: &[&str]) -> (String, PathBuf) {
    let dir = scratch(&stem_of(name));
    let input = input_of(name);
    let output = dir.join(format!("{}.{}", stem_of(name), extension));

    let mut args = vec![
        "-i",
        input.to_str().expect("input path is utf-8"),
        "-o",
        output.to_str().expect("output path is utf-8"),
        "--to",
        format,
        "--ocr",
        "never",
    ];
    args.extend_from_slice(extra);
    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args(&args)
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "converting {name} with --to {format} failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    let written = std::fs::read_to_string(&output).expect("read what was written");
    (written, dir)
}

/// The first differing line with a little context, so a failure says what
/// changed rather than only that something did.
fn difference(golden: &str, actual: &str) -> String {
    let golden_lines: Vec<&str> = golden.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    for index in 0..golden_lines.len().max(actual_lines.len()) {
        let want = golden_lines.get(index).copied();
        let got = actual_lines.get(index).copied();
        if want != got {
            let from = index.saturating_sub(2);
            let context = |lines: &[&str]| {
                (from..(index + 3).min(lines.len()))
                    .map(|i| format!("{:>5} {}", i + 1, lines[i]))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            return format!(
                "first difference at line {}:\ngolden:\n{}\nactual:\n{}",
                index + 1,
                context(&golden_lines),
                context(&actual_lines)
            );
        }
    }
    format!(
        "the lines agree but the endings do not (golden {} bytes, actual {} bytes)",
        golden.len(),
        actual.len()
    )
}

/// Compare one rendering with its golden, or rewrite every golden when
/// `UPDATE_GOLDEN_FORMATS` is set.
///
/// The files the rendering names beside itself are pinned with it: a golden
/// names its assets — HTML's `<img src>`, YAML's `uri:`, a Markdown reference in
/// a text rendering — and each one is discovered from that text and held to its
/// committed bytes, so a new picture is covered without a test of its own.
/// Returns the golden's text, for the set-wide pin sweep.
fn assert_matches_golden(format: &str, ext: &str, name: &str) -> String {
    let (actual, dir) = convert(name, format, ext, &[]);
    let golden_path = golden_of(format, ext, name);
    let pins = Pins {
        label: &format!("{name} as {format}"),
        produced: &dir,
        pinned: golden_path.parent().expect("golden has a directory"),
        update: std::env::var_os("UPDATE_GOLDEN_FORMATS").is_some(),
        regenerate:
            "UPDATE_GOLDEN_FORMATS=1 cargo test -p pdfextractor-cli --test formats_golden_test",
    };

    let golden = if pins.update {
        std::fs::create_dir_all(pins.pinned).expect("create the golden directory");
        std::fs::write(&golden_path, &actual).expect("write the golden file");
        pins.check(&actual);
        actual
    } else {
        let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
            panic!(
                "missing golden {}: {e}\nregenerate with {}",
                golden_path.display(),
                pins.regenerate
            )
        });
        assert!(
            golden == actual,
            "{name} as {format} changed:\n{}",
            difference(&golden, &actual)
        );
        pins.check(&golden);
        golden
    };
    let _ = std::fs::remove_dir_all(&dir);
    golden
}

#[test]
fn html_text_and_yaml_match_the_golden_files() {
    let regenerate =
        "UPDATE_GOLDEN_FORMATS=1 cargo test -p pdfextractor-cli --test formats_golden_test";
    // What each format's goldens name, so one sweep per format can tell a pin
    // from a leftover once all of that format's documents have been read.
    let mut goldens: BTreeMap<&str, Vec<String>> = BTreeMap::new();

    for (format, ext) in FORMATS {
        for name in CASES {
            goldens
                .entry(*format)
                .or_default()
                .push(assert_matches_golden(format, ext, name));
        }
    }

    for (format, goldens) in goldens {
        Sweep {
            pinned: &repository_root().join("test-data/golden").join(format),
            update: std::env::var_os("UPDATE_GOLDEN_FORMATS").is_some(),
            regenerate,
        }
        .check(&goldens);
    }
}

/// A page that names its picture is only a page with a picture in it if the file
/// is there too: the HTML golden for `pictures.md` is written beside the copy,
/// and the copy is the JPEG the Markdown named, byte for byte. The markup alone
/// would still "reference" a picture that no browser could draw, and a re-encoded
/// copy would still load, so both are checked and not just the `<img src>`.
///
/// The name is the page's own, `images/scan.jpg`: the file keeps the path the
/// document gave it, so a page read back names the file it came with rather than
/// one this run invented.
#[test]
fn the_html_golden_picture_is_copied_beside_the_page() {
    let (html, dir) = convert("pictures.md", "html", "html", &[]);

    assert!(
        html.contains("<img src=\"images/scan.jpg\" alt=\"Image\">"),
        "the page no longer names the picture beside it:\n{html}"
    );

    // Beside the page, where a relative `src` resolves: the copy, not the
    // fixture's own directory.
    let copied = std::fs::read(dir.join("images/scan.jpg")).unwrap_or_else(|e| {
        panic!(
            "the picture the page names was not written beside it ({}): {e}",
            dir.join("images/scan.jpg").display()
        )
    });
    let source = std::fs::read(repository_root().join("test-data/images/scan.jpg"))
        .expect("the picture the Markdown names");
    assert_eq!(
        copied, source,
        "the picture beside the HTML is not the JPEG the Markdown named"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The social-card fixture is here for what a body reading cannot see: the
/// committed HTML golden carries the page's description, canonical URL and
/// every card line, escaped and in the order the head stated them. Reading the
/// golden rather than a fresh rendering is deliberate — a regeneration that
/// dropped the head would still match itself, and this is what fails instead.
#[test]
fn the_social_cards_golden_pins_the_head_facts() {
    let html = std::fs::read_to_string(golden_of("html", "html", "social_cards.html"))
        .expect("the social-card HTML golden");
    let expected = [
        "<title>Q3 review</title>",
        r#"<meta name="description" content="Quarterly results &amp; outlook">"#,
        r#"<link rel="canonical" href="https://example.com/reports/q3?quarter=3&amp;region=eu">"#,
        r#"<meta property="og:title" content="Q3 &amp; costs">"#,
        r#"<meta property="og:type" content="article">"#,
        r#"<meta property="og:url" content="https://example.com/reports/q3?quarter=3&amp;region=eu">"#,
        r#"<meta property="og:image" content="https://example.com/cards/q3.png">"#,
        r#"<meta property="og:image:width" content="1200">"#,
        r#"<meta property="og:image:height" content="630">"#,
        r#"<meta property="og:image:alt" content="A chart of Q3 &amp; costs">"#,
        r#"<meta name="twitter:card" content="summary_large_image">"#,
        r#"<meta name="twitter:title" content="Q3 &amp; costs">"#,
        r#"<meta name="twitter:image" content="https://example.com/cards/q3.png">"#,
    ];

    let mut cursor = 0;
    for line in expected {
        let offset = html[cursor..].find(line).unwrap_or_else(|| {
            panic!("the golden is missing {line:?}, or moved it before the line above")
        });
        cursor += offset + line.len();
    }
    // The head is a statement about the page, not words on it.
    for line in expected.iter().skip(1) {
        let content = line
            .split_once("content=\"")
            .or_else(|| line.split_once("href=\""))
            .map(|(_, value)| value.trim_end_matches("\">"))
            .unwrap_or_default();
        assert!(
            content.is_empty() || !html.contains(&format!("<p>{content}</p>")),
            "{line:?} leaked into the body"
        );
    }
}

#[test]
fn every_format_asked_for_is_written_as_a_sibling() {
    // The acceptance line of W1: `-i doc.pdf --to md --to html` writes both
    // — and so do the rest, beside one stem — with each file byte-identical
    // to the golden its own dedicated test compares.
    let dir = scratch("siblings");
    let input = input_of("hello.pdf");
    let stem = dir.join("hello");

    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "-o",
            stem.with_extension("json").to_str().expect("utf-8"),
            "--to",
            "json",
            "--to",
            "md",
            "--to",
            "html",
            "--to",
            "text",
            "--to",
            "yaml",
            "--ocr",
            "never",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    // One file per format, all beside the stem, and none overwriting another.
    let mut written: Vec<String> = std::fs::read_dir(&dir)
        .expect("list the output directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(
        written,
        vec![
            "hello.html".to_string(),
            "hello.json".to_string(),
            "hello.md".to_string(),
            "hello.txt".to_string(),
            "hello.yaml".to_string(),
        ],
        "wrote {written:?}"
    );

    // Each is the golden one: the same document, written five ways.
    for (format, ext) in FORMATS {
        let golden_path = golden_of(format, ext, "hello.pdf");
        let golden = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("missing golden {}: {e}", golden_path.display()));
        let sibling = std::fs::read_to_string(stem.with_extension(ext))
            .unwrap_or_else(|e| panic!("read hello.{ext}: {e}"));
        assert_eq!(sibling, golden, "hello.{ext} is not its own golden");
    }
    let md_path = golden_of("markdown", "md", "hello.pdf");
    let md = std::fs::read_to_string(&md_path)
        .unwrap_or_else(|e| panic!("missing golden {}: {e}", md_path.display()));
    assert_eq!(
        std::fs::read_to_string(stem.with_extension("md")).expect("hello.md"),
        md
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// One stdout case: the format to ask for alone, and the predicate the whole
/// of stdout must satisfy — the stream is the document, first byte to last.
type StdoutCase = (&'static str, fn(&str) -> bool);

#[test]
fn stdout_holds_whatever_one_format_was_asked_for() {
    // `-o -` with a single format is an explicit contract: the stream is the
    // document, first byte to last, in any of the five formats.
    let cases: &[StdoutCase] = &[
        ("html", |out| {
            out.starts_with("<!DOCTYPE html>") && out.ends_with("</html>\n")
        }),
        ("text", |out| out == "Hello World\n"),
        ("yaml", |out| {
            out.starts_with("body:") && out.contains("schema_name: DoclingDocument")
        }),
    ];
    for (format, holds) in cases {
        let input = input_of("hello.pdf");
        let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
            .args([
                "-i",
                input.to_str().expect("utf-8"),
                "-o",
                "-",
                "--to",
                format,
                "--ocr",
                "never",
            ])
            .output()
            .expect("run the CLI binary");
        assert!(
            result.status.success(),
            "--to {format} failed\nstderr={}",
            String::from_utf8_lossy(&result.stderr)
        );
        let stdout = String::from_utf8(result.stdout).expect("stdout is UTF-8");
        assert!(holds(&stdout), "--to {format} wrote {stdout:?}");
    }
}

#[test]
fn two_formats_still_refuse_stdout() {
    // The contract is format-generic: whatever the pair is, stdout can only
    // hold one document and the run says so before converting anything.
    let input = input_of("hello.pdf");
    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "-o",
            "-",
            "--to",
            "html",
            "--to",
            "text",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(!result.status.success(), "two formats must refuse stdout");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("one format"), "stderr={stderr}");
}

/// Convert a text page with the real binary and read the text it writes.
fn text_of(input: &Path, json: &Path) -> String {
    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "-o",
            json.to_str().expect("utf-8"),
            "--to",
            "json",
            "--to",
            "text",
            "--ocr",
            "never",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "converting {} to text failed\nstderr={}",
        input.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::read_to_string(json.with_extension("txt")).expect("read the text output")
}

/// Every committed text golden is read back by the real binary and rendered as
/// text again, and the result must be the golden **byte for byte** — the gate
/// the Markdown goldens already have. Plain text states exactly one boundary,
/// the blank line, so the reader takes a run of non-empty lines as one block: a
/// single line is a paragraph, a longer run is written back verbatim, and the
/// writer's pipe grid is read as the table it is. Without that, a table's rows
/// would become paragraphs and the file would gain a blank line between every
/// row on each save, and a code block would lose its indentation.
///
/// A second pass then reads what the first wrote and renders it again, which
/// proves the re-render is itself stable rather than agreeing by accident.
#[test]
fn every_text_golden_is_a_fixed_point() {
    let golden_dir = repository_root().join("test-data/golden/text");
    let mut goldens: Vec<PathBuf> = std::fs::read_dir(&golden_dir)
        .expect("list the text goldens")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
        .collect();
    goldens.sort();
    assert!(
        !goldens.is_empty(),
        "no text goldens in {}",
        golden_dir.display()
    );

    for golden in goldens {
        let stem = golden
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a golden file name");
        let expected = std::fs::read_to_string(&golden).expect("read the golden");
        let dir = scratch(&format!("text-fixed-{stem}"));
        let input = dir.join("doc.txt");
        std::fs::copy(&golden, &input).expect("copy the golden");

        let first = text_of(&input, &dir.join("first.json"));
        assert!(
            !first.trim().is_empty(),
            "{stem}: the golden rendered to nothing"
        );
        assert!(
            first == expected,
            "{stem}: re-reading the golden changed its text:\n{}",
            difference(&expected, &first)
        );

        let again = dir.join("first.txt");
        let second = text_of(&again, &dir.join("second.json"));
        assert!(
            first == second,
            "{stem}: its text is not a fixed point:\n{}",
            difference(&first, &second)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Every committed HTML golden is read back by the real binary and rendered as
/// HTML again, and the page it re-renders must be the golden **byte for byte** —
/// the gate the Markdown and text goldens already have, now on the rendering
/// with the most structure to lose: nested lists, merged tables, asides, MathML
/// and the `<img>` a picture is placed with.
///
/// The pictures a page names are copied beside it first, where its own `src`
/// resolves, because a golden that names a picture is only the same page if the
/// file is there to be read: a missing file would drop the picture, and the diff
/// would be about the copy rather than about the page. The notes layer is read
/// in, as it is for the Markdown gate, so a page carrying a deck's remarks would
/// give them back rather than reading the asides as prose.
#[test]
fn every_html_golden_is_a_fixed_point() {
    let golden_dir = repository_root().join("test-data/golden/html");
    let mut goldens: Vec<PathBuf> = std::fs::read_dir(&golden_dir)
        .expect("list the HTML goldens")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "html"))
        .collect();
    goldens.sort();
    assert!(
        !goldens.is_empty(),
        "no HTML goldens in {}",
        golden_dir.display()
    );

    for golden in goldens {
        let stem = golden
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a golden file name");
        let expected = std::fs::read_to_string(&golden).expect("read the golden");
        let dir = scratch(&format!("html-fixed-{stem}"));
        let input = dir.join("doc.html");
        std::fs::copy(&golden, &input).expect("copy the golden");

        // Every file the page names is part of it, at the relative path its own
        // `src` resolves against — which is also the path it must come back as.
        for reference in common::referenced_assets(&expected) {
            let source = golden_dir.join(&reference);
            if !source.exists() {
                continue;
            }
            let target = dir.join(&reference);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("create the picture directory");
            }
            std::fs::copy(&source, &target).expect("copy the picture beside the page");
        }

        let again = dir.join("again.html");
        let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
            .args([
                "-i",
                input.to_str().expect("utf-8"),
                "-o",
                again.to_str().expect("utf-8"),
                "--to",
                "html",
                "--include-notes",
                "--ocr",
                "never",
            ])
            .output()
            .expect("run the CLI binary");
        assert!(
            result.status.success(),
            "{stem}: re-reading the golden failed\nstderr={}",
            String::from_utf8_lossy(&result.stderr)
        );

        let rendered = std::fs::read_to_string(&again).expect("the page written again");
        assert!(
            rendered == expected,
            "{stem}: re-reading the golden changed its HTML:\n{}",
            difference(&expected, &rendered)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A YAML golden is the whole document in another notation rather than a
/// reading of it, so its fixed point is the model's: the file is parsed back
/// into a `DoclingDocument` and rendered again, and the result must be the
/// golden **byte for byte**. That is where a key that moves, a `self_ref`
/// rewritten twice, or a `pages` map that comes out in another order would
/// show; a comparison against a fresh conversion would not, because the
/// conversion would have been re-pinned with it.
#[test]
fn every_yaml_golden_is_a_fixed_point() {
    let golden_dir = repository_root().join("test-data/golden/yaml");
    let mut goldens: Vec<PathBuf> = std::fs::read_dir(&golden_dir)
        .expect("list the YAML goldens")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"))
        .collect();
    goldens.sort();
    assert!(
        !goldens.is_empty(),
        "no YAML goldens in {}",
        golden_dir.display()
    );

    for golden in goldens {
        let stem = golden
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a golden file name");
        let expected = std::fs::read_to_string(&golden).expect("read the golden");
        let doc: pdfextractor_core::DoclingDocument = serde_yaml::from_str(&expected)
            .unwrap_or_else(|e| panic!("{stem}: the golden is not the document model: {e}"));
        let rendered = pdfextractor_md_exporter::yaml::render(&doc)
            .unwrap_or_else(|e| panic!("{stem}: could not render it again: {e}"));
        assert!(
            rendered == expected,
            "{stem}: its YAML is not a fixed point:\n{}",
            difference(&expected, &rendered)
        );

        // And it settles: the re-rendered file renders to itself.
        let again: pdfextractor_core::DoclingDocument =
            serde_yaml::from_str(&rendered).expect("the re-rendered file parses");
        let second = pdfextractor_md_exporter::yaml::render(&again).expect("render again");
        assert!(
            second == rendered,
            "{stem}: the YAML changed on its second pass:\n{}",
            difference(&rendered, &second)
        );
    }
}
