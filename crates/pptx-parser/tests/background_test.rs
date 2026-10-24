//! A slide's picture background, read from `test-data/background_deck.pptx`.
//!
//! The fixture is authored by python-pptx and completed with the one thing it
//! cannot write: `p:cSld/p:bg/p:bgPr/a:blipFill` on the first slide, naming
//! `images/scan.jpg` through that slide's own relationships. The second slide
//! has no `p:bg` at all, so the reading is held to a per-slide fact rather than
//! to a deck-wide one.

use pdfextractor_pptx_parser::{PptxDocument, Shape};
use std::path::PathBuf;

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data")
        .join(name)
}

fn deck() -> PptxDocument {
    let path = path("background_deck.pptx");
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    PptxDocument::parse_bytes(&data).expect("parse")
}

#[test]
fn a_picture_background_is_the_slides_own_image() {
    let deck = deck();
    assert_eq!(deck.slides.len(), 2);

    let background = deck.slides[0]
        .background
        .as_ref()
        .expect("the first slide's picture background");
    assert_eq!(background.mime, "image/jpeg");
    assert_eq!(
        background.data,
        std::fs::read(path("images/scan.jpg")).expect("the backdrop fixture"),
        "the background is the media part's own bytes"
    );
    assert_eq!(
        background.bbox,
        [0.0, 0.0, deck.width, deck.height],
        "a background covers the slide's page"
    );

    // It is not a shape and not a picture item: the slide's tree holds only the
    // sentence in front of the backdrop.
    assert_eq!(deck.slides[0].shapes.len(), 1);
    assert!(matches!(deck.slides[0].shapes[0], Shape::Text(_)));
    assert!(deck.slides[1].background.is_none());
}

#[test]
fn a_slide_without_a_background_has_no_page_image() {
    let deck = deck();
    assert!(deck.slides[1].background.is_none());
    assert!(
        deck.slides[1]
            .shapes
            .iter()
            .all(|shape| matches!(shape, Shape::Text(_))),
        "the second slide holds only its sentence"
    );
}
