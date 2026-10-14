//! Regression tests against real legacy `.doc` files (Apache POI test data).
//!
//! See `test-data/real/README.md` for provenance. These documents are OLE
//! compound files, so they exercise the container reader, the piece table and
//! the character-stream control codes at once.

use pdfextractor_doc_parser::{DocDocument, DocItem};
use std::path::PathBuf;

fn real(name: &str) -> Vec<u8> {
    let path = PathBuf::from("../../test-data/real").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("missing corpus file {}: {e}", path.display()))
}

fn parse(name: &str) -> DocDocument {
    DocDocument::parse_bytes(&real(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn body_text(doc: &DocDocument) -> String {
    doc.items
        .iter()
        .filter_map(|i| match i {
            DocItem::Text { text, .. } => Some(text.as_str()),
            DocItem::Picture => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn pictures(doc: &DocDocument) -> usize {
    doc.items.iter().filter(|i| **i == DocItem::Picture).count()
}

/// Every fixture must open: the container, the Fib and the piece table all have
/// to agree before a single character is read.
#[test]
fn every_fixture_opens() {
    for name in [
        "equation.doc",
        "two_images.doc",
        "testPictures.doc",
        "FloatingPictures.doc",
    ] {
        let doc = parse(name);
        assert!(
            !doc.items.is_empty(),
            "{name}: parsed but produced no blocks"
        );
    }
}

/// `equation.doc` holds exactly one thing: an embedded OLE object wrapped in a
/// field. The instruction text (`EMBED opendocument.MathDocument.1`) is not part
/// of the document, and only the picture placeholder should remain.
#[test]
fn an_embedded_object_leaves_no_field_instruction_text() {
    let doc = parse("equation.doc");
    let text = body_text(&doc);
    assert!(
        text.is_empty(),
        "field instruction leaked into the text: {text:?}"
    );
    assert!(
        !text.contains("EMBED") && !text.contains("MathDocument"),
        "field instruction leaked: {text:?}"
    );
    assert_eq!(
        pictures(&doc),
        1,
        "the embedded object should be one picture"
    );
}

/// Two inline images, each with its own caption.
#[test]
fn two_images_are_reported_alongside_their_captions() {
    let doc = parse("two_images.doc");
    let text = body_text(&doc);
    assert!(text.contains("the jpg file"), "caption lost: {text:?}");
    assert!(text.contains("the png file"), "caption lost: {text:?}");
    assert_eq!(pictures(&doc), 2, "expected two pictures: {text:?}");
}

/// The pictures fixture mixes captions, a `SHAPE` field and a page break. The
/// field's instruction must be dropped while every caption survives.
#[test]
fn pictures_doc_keeps_captions_and_drops_field_instructions() {
    let doc = parse("testPictures.doc");
    let text = body_text(&doc);
    for caption in ["Sunset:", "Winter:", "Auto shape picture:", "Dino.wmf:"] {
        assert!(text.contains(caption), "caption {caption:?} lost: {text:?}");
    }
    assert!(
        !text.contains("MERGEFORMAT") && !text.contains("SHAPE"),
        "a field instruction leaked: {text:?}"
    );
    assert!(
        pictures(&doc) >= 8,
        "expected the picture placeholders, got {}: {text:?}",
        pictures(&doc)
    );
}

/// A styles-and-fonts listing: plain prose in a single stream.
#[test]
fn floating_pictures_doc_reads_its_prose() {
    let doc = parse("FloatingPictures.doc");
    let text = body_text(&doc);
    assert!(text.contains("Heading 1"), "heading text lost: {text:?}");
    assert!(text.contains("Arial, 8pt"), "styled run lost: {text:?}");
    assert!(
        text.len() > 1000,
        "expected a long document, got {} chars",
        text.len()
    );
}

fn furniture_text(doc: &DocDocument) -> String {
    doc.furniture
        .iter()
        .filter_map(|i| match i {
            DocItem::Text { text, .. } => Some(text.as_str()),
            DocItem::Picture => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn footnote_text(doc: &DocDocument) -> String {
    doc.items
        .iter()
        .filter_map(|i| match i {
            DocItem::Text { text, label } if label == "footnote" => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Headers, footers and comments are separate subdocuments of the character
/// stream, so nothing in the body points at them and they used to vanish
/// silently. `FloatingPictures.doc` is the fixture that has them (bug 47).
#[test]
fn headers_footers_and_comments_come_out() {
    let doc = parse("FloatingPictures.doc");
    let furniture = furniture_text(&doc);
    assert!(
        furniture.contains("This is plain text in the header"),
        "header lost: {furniture:?}"
    );
    assert!(
        furniture.contains("This is plain text in the footer"),
        "footer lost: {furniture:?}"
    );
    // A field prints its result, which is what the document stores - not a
    // re-evaluation of it. `FILENAME` is stored as the name the file had.
    assert!(
        furniture.contains("Office Open XML sample.doc"),
        "the FILENAME field's result was lost: {furniture:?}"
    );
    assert!(
        !furniture.contains("FILENAME") && !furniture.contains("AUTHOR"),
        "a field instruction leaked into the header: {furniture:?}"
    );

    let comments = footnote_text(&doc);
    assert!(
        comments.contains("This is a comment."),
        "the comment was lost: {comments:?}"
    );
}

/// A document without headers or comments must report neither: the extraction
/// is driven by the Fib's own counts, not by a guess at the stream.
#[test]
fn a_document_without_headers_reports_no_furniture() {
    for name in ["equation.doc", "two_images.doc", "testPictures.doc"] {
        let doc = parse(name);
        assert!(
            doc.furniture.is_empty(),
            "{name} grew furniture: {:?}",
            furniture_text(&doc)
        );
        assert!(
            footnote_text(&doc).is_empty(),
            "{name} grew a comment: {:?}",
            footnote_text(&doc)
        );
    }
}

/// A file that is not a compound document must fail clearly, not panic.
#[test]
fn a_non_doc_file_is_rejected() {
    let bytes =
        std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/hello.pdf"))
            .expect("hello.pdf");
    let err = DocDocument::parse_bytes(&bytes).unwrap_err();
    assert!(
        err.to_string().contains("OLE compound file"),
        "unexpected error: {err}"
    );
}
