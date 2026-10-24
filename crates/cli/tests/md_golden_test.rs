//! Golden tests for the Markdown export (`--to md`).
//!
//! Each document is converted by the real binary and compared, byte for byte,
//! with a committed expectation under `test-data/golden/markdown/`, in the
//! same spirit as everything else this repository pins: a change to the
//! renderer that alters any document's Markdown fails here first, and the
//! diff of the golden files *is* the review.
//!
//! OCR is off for every case. Recognised text depends on the installed
//! Tesseract and its language data, which would make a byte-exact golden
//! flaky on machines that are otherwise correct. A golden is regenerated
//! deliberately, and the diff reviewed, with:
//!
//!     UPDATE_GOLDEN_MD=1 cargo test -p pdfextractor-cli --test md_golden_test

mod common;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use common::{Pins, Sweep};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Documents whose Markdown is pinned, byte for byte. Each has a full text
/// layer, so `--ocr never` costs none of their content.
const CASES: &[&str] = &[
    "hello.pdf",               // the smallest fixture: one heading-sized line
    "dummy.pdf",               // a one-paragraph PDF
    "calibre_demo.docx",       // prose, lists, tables and pictures in one document
    "math-accents.docx",       // Office Math, which becomes `$$` display blocks
    "irs_f1040.pdf",           // a long form: many tables, columns and furniture
    "flow.html",               // a flow format: headings, lists, code and a table
    "late_header.html",        // a table whose header row is declared below a caption
    "late_merged_header.html", // a late header row whose own cells merge
    "social_cards.html",       // a page's head facts, carried as YAML front matter
    "picture_names.html",      // a picture named with a space and an ampersand
    "fenced.md",               // Markdown in, Markdown out: a fence survives the trip
    "pictures.md",             // Markdown in with a picture: its file is copied and referenced
    "merged_cells.docx", // a DOCX whose table merges cells: spans kept in HTML, flattened here
    "merged_table.rtf",  // the same merged table as a word processor's own RTF export
    "link_annotations.pdf", // a PDF whose link is rendered as `[text](url)`
    "q3_review.pptx",    // a deck: its slide and comment groups are marked
    "chart_deck.pptx",   // a deck whose only picture is a chart, so nothing is written for it
    "inventory.xlsx",    // a workbook: a `sheet` group per worksheet, a merge, a hidden sheet
    "rtf_bitmap.rtf",    // a word processor's own RTF export: a heading, paragraphs, a picture
];

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn input_of(name: &str) -> PathBuf {
    let root = repository_root().join("test-data");
    match name {
        // The small purpose-built fixtures live beside `test-data/`; every
        // other case is a corpus document under `real/`.
        "hello.pdf"
        | "flow.html"
        | "late_header.html"
        | "late_merged_header.html"
        | "social_cards.html"
        | "picture_names.html"
        | "fenced.md"
        | "pictures.md"
        | "merged_cells.docx"
        | "merged_table.rtf"
        | "link_annotations.pdf"
        | "q3_review.pptx"
        | "chart_deck.pptx"
        | "inventory.xlsx"
        | "rtf_bitmap.rtf" => root.join(name),
        _ => root.join("real").join(name),
    }
}

fn stem_of(name: &str) -> String {
    name.rsplit_once('.').unwrap_or((name, "")).0.to_string()
}

fn golden_of(name: &str) -> PathBuf {
    repository_root()
        .join("test-data/golden/markdown")
        .join(format!("{}.md", stem_of(name)))
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn scratch(tag: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("pdfe-md-{}-{}-{tag}", std::process::id(), unique));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create scratch dir");
    path
}

/// Convert one document to Markdown with the real binary and read it back.
fn convert_to_markdown(name: &str) -> String {
    let dir = scratch(&stem_of(name));
    let md = convert_to_markdown_in(name, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    md
}

/// The same conversion into a caller's directory, which is left in place: a
/// picture the writer copied beside the Markdown is part of what the golden
/// promises, so it has to still be there to read.
fn convert_to_markdown_in(name: &str, dir: &std::path::Path) -> String {
    let input = input_of(name);
    let output = dir.join(format!("{}.md", stem_of(name)));

    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("input path is utf-8"),
            "-o",
            output.to_str().expect("output path is utf-8"),
            "--to",
            "md",
            "--ocr",
            "never",
            // The goldens carry a deck's remarks too: without the notes layer
            // the round-trip gate would only ever exercise body content, and
            // a note that turned into a paragraph would never show up in a
            // golden at all.
            "--include-notes",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "converting {name} failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    std::fs::read_to_string(&output).expect("read the Markdown written")
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

/// Each case is converted into a directory of its own, so the files its
/// Markdown names are still there: a golden pins its assets as well as its
/// words, and every asset is discovered from the golden's own references rather
/// than listed here. A writer that starts naming a new file is pinned by the
/// same test, and a pinned file no golden names any more fails it.
#[test]
fn markdown_matches_the_golden_files() {
    let update = std::env::var_os("UPDATE_GOLDEN_MD").is_some();
    let regenerate = "UPDATE_GOLDEN_MD=1 cargo test -p pdfextractor-cli --test md_golden_test";
    let mut goldens: Vec<String> = Vec::new();

    for name in CASES {
        let dir = scratch(&stem_of(name));
        let actual = convert_to_markdown_in(name, &dir);
        let golden_path = golden_of(name);
        let pins = Pins {
            label: name,
            produced: &dir,
            pinned: golden_path.parent().expect("golden has a directory"),
            update,
            regenerate,
        };

        if pins.update {
            std::fs::create_dir_all(pins.pinned).expect("create the golden directory");
            std::fs::write(&golden_path, &actual).expect("write the golden file");
            // The assets are written from the same run, so a regenerating run
            // pins what the new golden names.
            pins.check(&actual);
            goldens.push(actual);
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
                "{}'s Markdown changed:\n{}",
                name,
                difference(&golden, &actual)
            );
            pins.check(&golden);
            goldens.push(golden);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // One sweep of the whole set: a pin no Markdown golden names any more is a
    // leftover, not a golden.
    Sweep {
        pinned: &repository_root().join("test-data/golden/markdown"),
        update,
        regenerate,
    }
    .check(&goldens);
}

/// The part of a document a round trip must not change: every item, in reading
/// order, with what it says. Origin and names describe the file the words came
/// from; a picture's `uri` names the copy beside this run's output; neither is
/// the document's content.
fn stable_view(document: &serde_json::Value) -> serde_json::Value {
    let items = |name: &str| -> Vec<serde_json::Value> {
        document[name].as_array().cloned().unwrap_or_default()
    };
    let texts: Vec<serde_json::Value> = items("texts")
        .iter()
        .map(|item| {
            serde_json::json!({
                "label": item["label"],
                "text": item["text"],
                "level": item["level"],
                "marker": item["marker"],
                "hyperlink": item["hyperlink"],
                // A remark that came back as slide content holds the same
                // words; only this says it is still on the notes layer.
                "content_layer": item["content_layer"],
            })
        })
        .collect();
    let tables: Vec<serde_json::Value> = items("tables")
        .iter()
        .map(|table| {
            let data = &table["data"];
            let cells: Vec<serde_json::Value> = data["table_cells"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|cell| {
                    serde_json::json!({
                        "text": cell["text"],
                        "start_row": cell["start_row_offset_idx"],
                        "end_row": cell["end_row_offset_idx"],
                        "start_col": cell["start_col_offset_idx"],
                        "end_col": cell["end_col_offset_idx"],
                        "row_span": cell["row_span"],
                        "col_span": cell["col_span"],
                        "column_header": cell["column_header"],
                        "row_header": cell["row_header"],
                        "row_section": cell["row_section"],
                    })
                })
                .collect();
            serde_json::json!({
                "num_rows": data["num_rows"],
                "num_cols": data["num_cols"],
                "cells": cells,
            })
        })
        .collect();
    let pictures: Vec<serde_json::Value> = items("pictures")
        .iter()
        .map(|picture| {
            serde_json::json!({
                "label": picture["label"],
                "mimetype": picture["image"]["mimetype"],
                "width": picture["image"]["size"]["width"],
                "height": picture["image"]["size"]["height"],
            })
        })
        .collect();

    serde_json::json!({"texts": texts, "tables": tables, "pictures": pictures})
}

/// The first place two values part, as a path a reader can follow —
/// `texts[0].level: 2 became 3` says what a full dump of two documents does
/// not.
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
                let here = format!("{path}.{key}");
                if let Some(difference) = first_difference(x, y, &here) {
                    return Some(difference);
                }
            }
            None
        }
        _ => (first != second).then(|| format!("{path}: {first} became {second}")),
    }
}

/// Every committed Markdown golden is re-read and rendered again, and the
/// result must be the golden **byte for byte**: the reader is the writer's
/// inverse for every construct the goldens carry — headings, lists, fences,
/// tables, links, `$$` formulas, a deck's notes and comment sections, and the
/// pictures a page names. The files those references name are copied beside the
/// copied golden first, where its own path resolves, so a picture is read as
/// the picture it is rather than as the words a broken reference falls back to;
/// a name with a space is written in `<…>` and has to come back in `<…>`.
/// `--include-notes` is passed on both passes, so a remark has to come back on
/// the notes layer instead of becoming a paragraph. Without this gate an
/// asymmetry — say, a heading gaining a `#` per round trip — renders happily and
/// still changes the document every time it is saved. A second pass then proves
/// the re-render is itself stable, which catches a construct that only agreed by
/// accident on the first read. The file-backed picture case is
/// `a_picture_survives_the_markdown_round_trip`.
#[test]
fn every_markdown_golden_is_a_fixed_point() {
    let golden_dir = repository_root().join("test-data/golden/markdown");
    let mut goldens: Vec<PathBuf> = std::fs::read_dir(&golden_dir)
        .expect("list the Markdown goldens")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    goldens.sort();
    assert!(
        !goldens.is_empty(),
        "no Markdown goldens in {}",
        golden_dir.display()
    );

    for golden in goldens {
        let stem = golden
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a golden file name");
        let golden_text = std::fs::read_to_string(&golden).expect("read the golden");
        let dir = scratch(&format!("fixed-{stem}"));
        let input = dir.join("doc.md");
        std::fs::copy(&golden, &input).expect("copy the golden");
        // The pictures the golden names are part of it: a reference whose file
        // is not beside the page is read as the words it is written with, and
        // the fixed point would be about a missing file rather than the page.
        for reference in common::referenced_assets(&golden_text) {
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

        let first = convert_flow_with_notes(&input, &dir.join("first.json"), &["json", "md"], true);
        let first_markdown =
            std::fs::read_to_string(dir.join("first.md")).expect("the first Markdown");
        assert!(
            first_markdown == golden_text,
            "{stem}: re-reading the golden changed its Markdown:\n{}",
            difference(&golden_text, &first_markdown)
        );
        let second = convert_flow_with_notes(
            &dir.join("first.md"),
            &dir.join("second.json"),
            &["json", "md"],
            true,
        );
        let second_markdown =
            std::fs::read_to_string(dir.join("second.md")).expect("the second Markdown");

        assert!(
            !first_markdown.trim().is_empty(),
            "{stem}: the golden rendered to nothing"
        );
        let (first_view, second_view) = (stable_view(&first), stable_view(&second));
        if first_view != second_view {
            let where_it_parts = first_difference(&first_view, &second_view, "document")
                .unwrap_or_else(|| "the views differ".to_string());
            panic!("{stem}: re-reading its own Markdown changed the document:\n  {where_it_parts}");
        }
        assert!(
            first_markdown == second_markdown,
            "{stem}: its Markdown is not a fixed point:\n{}",
            difference(&first_markdown, &second_markdown)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The same goldens, through the other reader: each document is written as
/// HTML with its notes, that HTML is read back, and the Markdown it renders
/// must be the Markdown written directly from the document. The comparison
/// ignores the two things HTML cannot keep — the file names this run gave its
/// pictures, and the runs of spaces an HTML cell collapses — and nothing else,
/// so headings, groups, notes and comments, code, formulas and flattened
/// tables all have to survive.
///
/// A second pass then reads the Markdown that came from the HTML and renders
/// it again: reading a page and then reading that page's Markdown must settle,
/// which is what proves the two readers agree with the writers and with each
/// other.
#[test]
fn every_golden_round_trips_through_html_with_notes() {
    for name in CASES {
        let stem = stem_of(name);
        let direct = convert_to_markdown(name);
        let dir = scratch(&format!("html-{stem}"));
        let html = dir.join("doc.html");

        let converted = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
            .args([
                "-i",
                input_of(name).to_str().expect("utf-8"),
                "-o",
                html.to_str().expect("utf-8"),
                "--to",
                "html",
                "--ocr",
                "never",
                "--include-notes",
            ])
            .output()
            .expect("run the CLI binary");
        assert!(
            converted.status.success(),
            "writing {name} as HTML failed\nstderr={}",
            String::from_utf8_lossy(&converted.stderr)
        );

        let back = dir.join("back");
        std::fs::create_dir_all(&back).expect("create the HTML round-trip directory");
        let document = back.join("doc.json");
        let read = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
            .args([
                "-i",
                html.to_str().expect("utf-8"),
                "-o",
                document.to_str().expect("utf-8"),
                "--to",
                "json",
                "--to",
                "md",
                "--ocr",
                "never",
                "--include-notes",
            ])
            .output()
            .expect("run the CLI binary");
        assert!(
            read.status.success(),
            "reading {name}'s HTML back failed\nstderr={}",
            String::from_utf8_lossy(&read.stderr)
        );

        let through_html =
            std::fs::read_to_string(back.join("doc.md")).expect("the Markdown from HTML");
        assert!(
            !through_html.trim().is_empty(),
            "{stem}: its HTML rendered to nothing"
        );
        let (want, got) = (
            comparable_markdown(&direct),
            comparable_markdown(&through_html),
        );
        assert!(
            want == got,
            "{stem}: reading its HTML changed its Markdown:\n{}",
            difference(&want, &got)
        );

        // The remarks are still remarks: the same lines, and every one of them
        // on the notes layer of the document the reader built — not turned
        // into a paragraph of the page.
        let remarks = remark_lines(&direct);
        assert_eq!(
            remarks,
            remark_lines(&through_html),
            "{stem}: its HTML lost or gained a remark"
        );
        let parsed: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&document).expect("the JSON the HTML read wrote"),
        )
        .expect("valid JSON");
        let texts = parsed["texts"].as_array().expect("a text list");
        let on_notes = texts
            .iter()
            .filter(|item| item["content_layer"] == "notes")
            .count();
        if remarks > 0 {
            assert!(on_notes > 0, "{stem}: the remarks came back as body text");
        }
        let body: Vec<&str> = texts
            .iter()
            .filter(|item| item["content_layer"] != "notes")
            .filter_map(|item| item["text"].as_str())
            .collect();
        for item in texts.iter().filter(|item| item["content_layer"] == "notes") {
            let words = item["text"].as_str().unwrap_or_default();
            assert!(
                !body.contains(&words),
                "{stem}: the remark {words:?} is also body text"
            );
        }

        // Reading the Markdown that came from the HTML settles: the two
        // readers agree with each other, not only with the writers.
        let again = back.join("again");
        std::fs::create_dir_all(&again).expect("create the second-pass directory");
        let settled = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
            .args([
                "-i",
                back.join("doc.md").to_str().expect("utf-8"),
                "-o",
                again.join("doc.json").to_str().expect("utf-8"),
                "--to",
                "json",
                "--to",
                "md",
                "--ocr",
                "never",
                "--include-notes",
            ])
            .output()
            .expect("run the CLI binary");
        assert!(
            settled.status.success(),
            "reading {name}'s HTML-derived Markdown failed\nstderr={}",
            String::from_utf8_lossy(&settled.stderr)
        );
        let second = std::fs::read_to_string(again.join("doc.md")).expect("the second Markdown");
        assert!(
            comparable_markdown(&through_html) == comparable_markdown(&second),
            "{stem}: the Markdown from its HTML is not a fixed point:\n{}",
            difference(&through_html, &second)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// What an HTML round trip must preserve, for comparison: picture destinations
/// name the copies this run wrote and the HTML cell layout the file shares;
/// neither is the document's content. Table rows are compared by their cells'
/// words, not by the padding that aligns them.
fn comparable_markdown(markdown: &str) -> String {
    markdown
        .lines()
        .map(|line| {
            let line = line.trim_end();
            if line.starts_with('|') {
                let cells: Vec<String> = line
                    .split('|')
                    .map(|cell| cell.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect();
                // The separator row's dashes are as wide as the column was
                // padded; its cells say how many columns there are, which is
                // the table's shape.
                let separator = cells
                    .iter()
                    .skip(1)
                    .take(cells.len().saturating_sub(2))
                    .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':'));
                if separator {
                    format!("|{}|", vec!["---"; cells.len().saturating_sub(2)].join("|"))
                } else {
                    cells.join("|")
                }
            } else if line.starts_with("![") {
                match (line.find("]("), line.rfind(')')) {
                    (Some(open), Some(close)) if open < close => {
                        format!("{}](image){}", &line[..open], &line[close + 1..])
                    }
                    _ => line.to_string(),
                }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How many remark lines a Markdown rendering carries — speaker notes and
/// comments alike — so a round trip that quietly turned one into prose is
/// visible even if the words move.
fn remark_lines(markdown: &str) -> usize {
    markdown
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("> **Notes:**") || line.starts_with("> **Comment:**")
        })
        .count()
}

/// Markdown out and Markdown back in: the writer renders a hyperlink as
/// `[text](url)`, and the reader has to recover both the label and the
/// address, or a round trip through Markdown quietly drops every link.
#[test]
fn a_link_survives_the_markdown_round_trip() {
    let markdown = convert_to_markdown("link_annotations.pdf");
    assert!(
        markdown.contains("[Visit the spec page](https://example.com/spec)"),
        "the writer did not render the link:\n{markdown}"
    );

    let dir = scratch("round-trip");
    let input = dir.join("round_trip.md");
    let output = dir.join("round_trip.json");
    std::fs::write(&input, &markdown).expect("write the Markdown");

    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "-o",
            output.to_str().expect("utf-8"),
            "--ocr",
            "never",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "reading the Markdown back failed\nstderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    let document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&output).expect("the round-tripped JSON"))
            .expect("valid JSON");
    assert_eq!(document["origin"]["mimetype"], "text/markdown");

    let texts = document["texts"].as_array().expect("a text list");
    let linked: Vec<&serde_json::Value> = texts
        .iter()
        .filter(|item| item["hyperlink"].is_string())
        .collect();
    assert_eq!(linked.len(), 1, "one link in, one link out: {linked:#?}");
    assert_eq!(linked[0]["text"], "Visit the spec page");
    assert_eq!(linked[0]["hyperlink"], "https://example.com/spec");

    // The round trip kept the rest of the page as words, not only the link.
    let words = texts
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(words.contains("Second page"), "{words}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A 2×2 red PNG, written by the test itself rather than committed as a
/// binary: a Markdown picture needs a real image file beside it, and this is
/// the smallest one whose header the image reader accepts.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xfd, 0xd4, 0x9a,
    0x73, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x44, 0x0c, 0x10, 0x0a, 0x00, 0x1f, 0xee, 0x03, 0xfd, 0x8b, 0x5f, 0x14, 0xd4, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Run the CLI on `input` and write `output`'s format beside it, then read the
/// JSON it wrote.
fn convert_flow(
    input: &std::path::Path,
    output: &std::path::Path,
    formats: &[&str],
) -> serde_json::Value {
    convert_flow_with_notes(input, output, formats, false)
}

/// [`convert_flow`] with the reader's choice of scope: the notes layer joins
/// when `notes`, which is how a deck's remarks are read back as remarks.
fn convert_flow_with_notes(
    input: &std::path::Path,
    output: &std::path::Path,
    formats: &[&str],
    notes: bool,
) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"));
    command.args([
        "-i",
        input.to_str().expect("utf-8"),
        "-o",
        output.to_str().expect("utf-8"),
    ]);
    for format in formats {
        command.args(["--to", format]);
    }
    if notes {
        command.arg("--include-notes");
    }
    let result = command.output().expect("run the CLI binary");
    assert!(
        result.status.success(),
        "converting {} failed\nstderr={}",
        input.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_str(&std::fs::read_to_string(output).expect("the JSON written"))
        .expect("valid JSON")
}

/// Markdown in, Markdown out: a picture the file points at beside itself has
/// to be read as a picture, not left as the `![alt](source)` characters, or a
/// round trip through Markdown loses the image and keeps its syntax.
#[test]
fn a_picture_survives_the_markdown_round_trip() {
    let dir = scratch("picture");
    // The source is written as a URL path with its space escaped; it still
    // names a file on disk, so it has to be decoded before it is read.
    std::fs::write(dir.join("a shot.png"), TINY_PNG).expect("write the image");
    let input = dir.join("source.md");
    std::fs::write(
        &input,
        "# Illustrated\n\nBefore ![a red square](a%20shot.png) after.\n",
    )
    .expect("write the Markdown");

    let json = dir.join("page.json");
    let first = convert_flow(&input, &json, &["json", "md"]);

    let pictures = first["pictures"].as_array().expect("a pictures list");
    assert_eq!(
        pictures.len(),
        1,
        "one image in, one picture out: {pictures:#?}"
    );
    assert_eq!(pictures[0]["image"]["mimetype"], "image/png");
    assert_eq!(pictures[0]["image"]["uri"], "page_images/img1.png");
    assert_eq!(pictures[0]["image"]["size"]["width"], 2.0);
    assert_eq!(pictures[0]["image"]["size"]["height"], 2.0);
    assert_eq!(
        std::fs::read(dir.join("page_images/img1.png")).expect("the image written beside the JSON"),
        TINY_PNG,
        "the picture was not copied byte for byte"
    );

    // The words around the image are still words, and the writer points at the
    // copy it made rather than at the source file.
    let markdown = std::fs::read_to_string(dir.join("page.md")).expect("the Markdown");
    assert!(
        markdown.contains("![Image](page_images/img1.png)"),
        "the picture was not rendered:\n{markdown}"
    );
    assert!(markdown.contains("Before"), "{markdown}");
    assert!(markdown.contains("after."), "{markdown}");

    // Read that Markdown back: the picture has to survive the second pass too,
    // and it keeps the name the page gave it — `page_images/img1.png` is a
    // plain path beside the document, so the copy goes back where it came from
    // rather than under a name this run invented.
    let again_json = dir.join("again.json");
    let second = convert_flow(&dir.join("page.md"), &again_json, &["json"]);
    let pictures = second["pictures"].as_array().expect("a pictures list");
    assert_eq!(
        pictures.len(),
        1,
        "the round trip lost the picture: {pictures:#?}"
    );
    assert_eq!(pictures[0]["image"]["uri"], "page_images/img1.png");
    assert_eq!(
        std::fs::read(dir.join("page_images/img1.png")).expect("the round-tripped image"),
        TINY_PNG
    );
    let words = second["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        words.contains("Before") && words.contains("after."),
        "{words}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A page rendered embedded carries the picture's bytes in the page itself:
/// `![Image](data:image/png;base64,…)` in Markdown, `<img src="data:…">` in
/// HTML. Reading either back has to decode them into the same picture — the
/// same media type, the same size, the same bytes — or the second pass keeps
/// the literal and the image is gone. Both spellings make one `Picture` block,
/// so the two are held to the same reading.
#[test]
fn an_embedded_picture_reads_back_as_the_picture() {
    let dir = scratch("embedded-picture");
    let uri = format!("data:image/png;base64,{}", STANDARD.encode(TINY_PNG));

    for (name, page) in [
        (
            "embedded.md",
            format!("# Illustrated\n\nBefore ![a red square]({uri}) after.\n"),
        ),
        (
            "embedded.html",
            format!(
                "<!DOCTYPE html><html><body><h1>Illustrated</h1><p>Before \
                 <img src=\"{uri}\" alt=\"a red square\"> after.</p></body></html>"
            ),
        ),
    ] {
        let input = dir.join(name);
        std::fs::write(&input, page).expect("write the page");
        let stem = name.replace('.', "-");
        let json = dir.join(format!("{stem}.json"));
        let doc = convert_flow(&input, &json, &["json"]);

        let pictures = doc["pictures"].as_array().expect("a pictures list");
        assert_eq!(
            pictures.len(),
            1,
            "{name} lost its embedded picture: {pictures:#?}"
        );
        assert_eq!(pictures[0]["image"]["mimetype"], "image/png", "{name}");
        assert_eq!(pictures[0]["image"]["size"]["width"], 2.0, "{name}");
        assert_eq!(pictures[0]["image"]["size"]["height"], 2.0, "{name}");
        let written = pictures[0]["image"]["uri"]
            .as_str()
            .expect("the picture's uri");
        assert_eq!(
            std::fs::read(dir.join(written)).expect("the image written beside the JSON"),
            TINY_PNG,
            "{name}: the decoded bytes are not the picture the URI carried"
        );

        // The page said the picture, not its syntax: no literal survives.
        let words: String = doc["texts"]
            .as_array()
            .expect("a text list")
            .iter()
            .filter_map(|item| item["text"].as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !words.contains("data:image"),
            "{name} kept the URI as words: {words}"
        );
        assert!(
            words.contains("Before") && words.contains("after."),
            "{name}: the words around the picture did not survive: {words}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// RFC 2397 lets the payload be percent-encoded instead of base64. A page that
/// writes a picture that way is read the same: the escapes are the URI's
/// syntax, so they are decoded before the bytes are measured or copied.
#[test]
fn a_percent_encoded_picture_is_decoded_too() {
    let dir = scratch("percent-picture");
    // The spaces are escapes too: a Markdown destination ends at whitespace.
    let svg = "%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='10'%20height='20'%3E%3C/svg%3E";
    let input = dir.join("page.md");
    std::fs::write(
        &input,
        format!("Before ![a box](data:image/svg+xml,{svg}) after.\n"),
    )
    .expect("write the Markdown");

    let json = dir.join("page.json");
    let doc = convert_flow(&input, &json, &["json"]);
    let pictures = doc["pictures"].as_array().expect("a pictures list");
    assert_eq!(pictures.len(), 1, "the picture was lost: {pictures:#?}");
    assert_eq!(pictures[0]["image"]["mimetype"], "image/svg+xml");
    assert_eq!(pictures[0]["image"]["size"]["width"], 10.0);
    assert_eq!(pictures[0]["image"]["size"]["height"], 20.0);
    assert_eq!(
        std::fs::read(dir.join("page_images/img1.svg")).expect("the copied picture"),
        b"<svg xmlns='http://www.w3.org/2000/svg' width='10' height='20'></svg>",
        "the percent escapes were not decoded into the picture's bytes"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A source this cannot read names no picture: a converter must not fetch the
/// network, and a missing file has nothing to copy. The literal stays as the
/// words the file contains rather than becoming a picture with nothing behind
/// it.
#[test]
fn a_picture_that_cannot_be_read_stays_words() {
    let dir = scratch("picture-missing");
    let input = dir.join("source.md");
    std::fs::write(
        &input,
        "![missing](gone.png)\n\n![remote](https://example.com/x.png)\n",
    )
    .expect("write the Markdown");

    let json = dir.join("page.json");
    let document = convert_flow(&input, &json, &["json", "md"]);

    assert_eq!(
        document["pictures"]
            .as_array()
            .expect("a pictures list")
            .len(),
        0,
        "a source this cannot read became a picture: {}",
        document["pictures"]
    );
    let texts: Vec<&str> = document["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        texts,
        vec![
            "![missing](gone.png)",
            "![remote](https://example.com/x.png)",
        ]
    );

    // Which means the Markdown writer puts them back exactly as they came.
    let markdown = std::fs::read_to_string(dir.join("page.md")).expect("the Markdown");
    for literal in texts {
        assert!(
            markdown.contains(literal),
            "{literal} is missing:\n{markdown}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn several_formats_write_sibling_files() {
    let dir = scratch("siblings");
    let input = input_of("hello.pdf");
    let json = dir.join("hello.json");
    let md = dir.join("hello.md");

    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "-o",
            json.to_str().expect("utf-8"),
            "--to",
            "json",
            "--to",
            "md",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    let document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).expect("the JSON"))
            .expect("valid JSON");
    assert_eq!(document["schema_name"], "DoclingDocument");
    // The Markdown is the golden one: the same document, written twice.
    assert_eq!(
        std::fs::read_to_string(&md).expect("the Markdown"),
        std::fs::read_to_string(golden_of("hello.pdf")).expect("the golden")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_repeated_format_is_written_once() {
    let dir = scratch("dedup");
    let input = input_of("hello.pdf");
    let md = dir.join("hello.md");

    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "-o",
            md.to_str().expect("utf-8"),
            "--to",
            "md",
            "--to",
            "md",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    let written: Vec<String> = std::fs::read_dir(&dir)
        .expect("list the output directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(written, vec!["hello.md".to_string()], "wrote {written:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stdout_carries_only_the_document() {
    let input = input_of("hello.pdf");
    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "--to",
            "md",
            "-o",
            "-",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        std::fs::read_to_string(golden_of("hello.pdf")).expect("the golden")
    );
}

#[test]
fn stdout_refuses_what_it_cannot_hold_alone() {
    let input = input_of("hello.pdf");
    let directory = repository_root().join("test-data/real");

    // Two formats cannot both be the stream.
    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "-o",
            "-",
            "--to",
            "md",
            "--to",
            "json",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        !result.status.success(),
        "two formats on stdout was accepted"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("one format"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    // A directory is many documents, and the stream holds one.
    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            directory.to_str().expect("utf-8"),
            "--to",
            "md",
            "-o",
            "-",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        !result.status.success(),
        "a directory on stdout was accepted"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("one document"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    // The progress stream and the document would be interleaved.
    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "--to",
            "md",
            "-o",
            "-",
            "--progress",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        !result.status.success(),
        "--progress on stdout was accepted"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("stdout"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
}

/// A run that writes Markdown writes no JSON beside it: `--to` says what the
/// run writes, and the default is a default, not a floor.
#[test]
fn markdown_only_writes_no_json() {
    let dir = scratch("mdonly");
    let input = input_of("hello.pdf");
    let md = dir.join("hello.md");

    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "-o",
            md.to_str().expect("utf-8"),
            "--to",
            "md",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    let written: Vec<String> = std::fs::read_dir(&dir)
        .expect("list the output directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(written, vec!["hello.md".to_string()], "wrote {written:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The progress record names the run's primary output — the first format
/// asked for — whatever the others are.
#[test]
fn progress_names_the_primary_format() {
    let dir = scratch("primary");
    let input = input_of("hello.pdf");
    let md = dir.join("hello.md");

    let result = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("utf-8"),
            "-o",
            md.to_str().expect("utf-8"),
            "--to",
            "md",
            "--to",
            "json",
            "--progress",
        ])
        .output()
        .expect("run the CLI binary");
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    let stdout = String::from_utf8_lossy(&result.stdout);
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("every stdout line is a record"))
        .collect();
    let finished = records
        .iter()
        .find(|record| record["kind"] == "progress")
        .expect("a progress record");
    assert_eq!(finished["output"], md.display().to_string());
    assert!(
        dir.join("hello.json").exists(),
        "the sibling JSON is missing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A workbook arrives as sheets, and its Markdown says so: `inventory.xlsx`
/// pins a `sheet` group around each worksheet's tables, named as the workbook
/// names it, and the grouping has to survive being read again — the tables
/// come back under the sheets they belong to, not as one flat list.
///
/// The golden is byte-exact, so this test states what those bytes are for
/// rather than replacing them: a regeneration that accepted a writer which
/// stopped marking sheets would still match itself, and this fails.
#[test]
fn a_workbook_pins_a_sheet_group_per_worksheet() {
    let golden_text =
        std::fs::read_to_string(golden_of("inventory.xlsx")).expect("the workbook's golden");

    let mut starts = Vec::new();
    for name in ["Inventory", "Notes"] {
        let open = format!("<!-- pdfextractor:group sheet body {name} -->");
        let start = golden_text
            .find(&open)
            .unwrap_or_else(|| panic!("no `{name}` sheet group in the golden:\n{golden_text}"));
        let close = golden_text[start..]
            .find("<!-- pdfextractor:/group -->")
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("the `{name}` group is never closed:\n{golden_text}"));
        assert!(
            golden_text[start..close].contains("|---"),
            "the `{name}` group holds no table:\n{golden_text}"
        );
        starts.push(start);
    }
    assert!(
        starts[0] < starts[1],
        "the sheet groups are not in the workbook's sheet order:\n{golden_text}"
    );
    // The hidden worksheet is on the `invisible` layer, and Markdown renders the
    // body: its sheet is not in the golden at all.
    assert!(
        !golden_text.contains("Hidden"),
        "the hidden worksheet leaked into the Markdown:\n{golden_text}"
    );

    // The golden read back rebuilds the sheets, with their tables as the
    // groups' children — the JSON's `origin` is the Markdown now, so a group
    // that survived only as an HTML comment would not be here at all.
    let dir = scratch("workbook-groups");
    let input = dir.join("doc.md");
    std::fs::copy(golden_of("inventory.xlsx"), &input).expect("copy the golden");
    let document = convert_flow(&input, &dir.join("doc.json"), &["json"]);
    assert_eq!(
        document["origin"]["mimetype"], "text/markdown",
        "the round trip did not read the golden as Markdown"
    );

    let groups = document["groups"].as_array().expect("a group list");
    let sheets: Vec<(&str, &str, &str)> = groups
        .iter()
        .map(|group| {
            (
                group["label"].as_str().unwrap_or_default(),
                group["name"].as_str().unwrap_or_default(),
                group["content_layer"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        sheets,
        vec![("sheet", "Inventory", "body"), ("sheet", "Notes", "body")],
        "the sheet groups did not come back: {groups:#?}"
    );

    // Every table is inside a sheet, and each sheet holds the tables that sat
    // under it: three for `Inventory` (its header block, the merged note, the
    // `Total` cell) and one for `Notes`.
    let owned: Vec<usize> = groups
        .iter()
        .map(|group| group["children"].as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(owned, vec![3, 1], "a table left its sheet: {groups:#?}");
    assert_eq!(
        document["tables"].as_array().map_or(0, Vec::len),
        4,
        "the tables themselves did not come back"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
