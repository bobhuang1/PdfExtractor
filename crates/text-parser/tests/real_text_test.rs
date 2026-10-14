//! Real-document smoke tests for the flow parsers.
//!
//! The synthetic cases live in the crate's unit tests; these read documents
//! that actually ship in the repository, so a change that only works on
//! hand-written input is caught.

use pdfextractor_text_parser::{text, Block};
use std::path::PathBuf;

fn read(relative: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()))
}

fn labels(doc: &pdfextractor_text_parser::TextDocument, label: &str) -> usize {
    doc.items
        .iter()
        .filter(|b| matches!(b, Block::Text { label: l, .. } if l == label))
        .count()
}

/// The repository README is a real Markdown document: headings, tables and
/// prose. It must survive as structure, not one flat paragraph.
#[test]
fn readme_parses_as_structured_markdown() {
    let doc = text::parse_markdown(&read("../../README.md"));
    assert!(
        labels(&doc, "section_header") >= 3,
        "no headings found in README.md"
    );
    assert!(
        labels(&doc, "paragraph") >= 5,
        "no prose found in README.md"
    );
    let tables = doc
        .items
        .iter()
        .filter(|b| matches!(b, Block::Table { .. }))
        .count();
    assert!(tables >= 1, "README.md tables were not recognised");

    // The heading text must be the heading, not the raw `#`-prefixed line.
    let first_heading = doc.items.iter().find_map(|b| match b {
        Block::Text { text, label, .. } if label == "section_header" => Some(text.clone()),
        _ => None,
    });
    let heading = first_heading.expect("a heading");
    assert!(
        !heading.starts_with('#'),
        "heading kept its marker: {heading:?}"
    );
}

/// The corpus README is also Markdown, with a table of documents per format.
#[test]
fn corpus_readme_keeps_its_tables() {
    let doc = text::parse_markdown(&read("../../test-data/real/README.md"));
    let tables: Vec<_> = doc
        .items
        .iter()
        .filter_map(|b| match b {
            Block::Table { rows } => Some(rows),
            _ => None,
        })
        .collect();
    assert!(tables.len() >= 2, "expected the corpus tables");
    assert!(
        tables
            .iter()
            .any(|rows| rows.len() > 3 && rows.iter().any(|r| r.len() >= 3)),
        "tables are too small to be the real ones: {tables:?}"
    );
}
