//! The shapes a deck paints itself, read from `test-data/shape_deck.pptx`.
//!
//! The unit tests build slides by hand to pin one rule each; this one reads a
//! deck a real writer (`python-pptx`, see `test-data/make_shape_deck.py`)
//! produced, so the markup — the `p:style` a library writes on every autoshape,
//! the group's child frame, the zero height of a straight connector — is read as
//! it is written rather than as the tests imagine it.

use image::GenericImageView as _;
use pdfextractor_pptx_parser::{PptxDocument, Shape};
use std::path::PathBuf;

fn deck() -> PptxDocument {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/shape_deck.pptx");
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    PptxDocument::parse_bytes(&data).expect("parse")
}

/// A picture's pixels, decoded, with the name of the shape it came from.
fn pixels(shape: &Shape) -> image::RgbaImage {
    let Shape::Picture(picture) = shape else {
        panic!("not a picture: {shape:?}");
    };
    image::load_from_memory(&picture.data)
        .unwrap_or_else(|e| panic!("the painted shape does not decode: {e}"))
        .to_rgba8()
}

/// The slide's pictures in reading order: the box, the star, the connector, the
/// group and the embedded PNG.
#[test]
fn the_shapes_powerpoint_paints_come_out_as_pictures() {
    let deck = deck();
    assert_eq!(deck.slides.len(), 1);
    let shapes = &deck.slides[0].shapes;
    let pictures: Vec<&Shape> = shapes
        .iter()
        .filter(|shape| matches!(shape, Shape::Picture(_)))
        .collect();
    assert_eq!(
        pictures.len(),
        5,
        "a shape the slide paints was left out: {shapes:#?}"
    );

    let sizes: Vec<(u32, u32)> = pictures
        .iter()
        .map(|shape| {
            let Shape::Picture(picture) = shape else {
                unreachable!()
            };
            assert_eq!(picture.mime, "image/png", "a shape is painted as a PNG");
            let image = image::load_from_memory(&picture.data)
                .unwrap_or_else(|e| panic!("a shape does not decode: {e}"));
            image.dimensions()
        })
        .collect();
    // 120 × 60, 80 × 80 and 216 × 0 points at two device pixels per point — the
    // connector's zero height is its stroke's, so its raster is 2 pt tall — then
    // the group's 72 × 36 and the scan's own 620 × 350 grid.
    assert_eq!(
        sizes,
        vec![(240, 120), (160, 160), (436, 4), (144, 72), (620, 350)]
    );

    // The box: filled `#156082` with its rounded corner left white, and placed
    // in the cells... in the EMU the slide states.
    let Shape::Picture(box_) = pictures[0] else {
        unreachable!()
    };
    assert_eq!(
        box_.bbox,
        [
            914_400.0,
            1_371_600.0,
            914_400.0 + 1_524_000.0,
            1_371_600.0 + 762_000.0
        ]
    );
    let box_pixels = pixels(pictures[0]);
    assert_eq!(box_pixels.get_pixel(120, 60).0, [0x15, 0x60, 0x82, 255]);
    assert_eq!(
        box_pixels.get_pixel(0, 0).0,
        [255, 255, 255, 255],
        "the rounded corner"
    );

    // The star states no colour of its own: `fillRef idx="1"` names the theme's
    // `accent1`, so a theme that was never read would leave this white.
    let star = pixels(pictures[1]);
    assert_eq!(
        star.get_pixel(80, 80).0,
        [0x4F, 0x81, 0xBD, 255],
        "the star takes the theme's accent"
    );

    // The connector is one line, drawn in the colour it states.
    let line = pixels(pictures[2]);
    assert_eq!(line.get_pixel(218, 2).0, [0x1F, 0x4E, 0x79, 255]);

    // The group places its oval on the right of its frame, inside the rectangle
    // the other child outlines.
    let group = pixels(pictures[3]);
    assert_eq!(
        group.get_pixel(110, 36).0,
        [0x6D, 0xA3, 0x3A, 255],
        "the oval"
    );
    assert_eq!(
        group.get_pixel(14, 36).0,
        [255, 255, 255, 255],
        "left of it"
    );

    // The picture the deck embeds is still the media part's own bytes: a shape
    // that has an image behind it is not painted again.
    let Shape::Picture(scan) = pictures[4] else {
        unreachable!()
    };
    let source = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/invoice_scan.png"),
    )
    .expect("the scan fixture");
    assert_eq!(scan.data, source);
}

/// A shape's words are the slide's text, and a text box with no fill and no line
/// puts no ink down — so the slide holds three sentences and five pictures, not
/// a picture of every text box.
#[test]
fn the_words_of_a_shape_are_text_and_only_ink_is_a_picture() {
    let deck = deck();
    let words: Vec<String> = deck.slides[0]
        .shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Text(text) => text
                .paragraphs
                .first()
                .map(|paragraph| paragraph.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        words,
        vec![
            "Before the shapes.",
            "A box with words.",
            "Words with no ink behind them.",
        ]
    );

    // The rounded box is a picture *and* its words are its own text item: the
    // slide shows both, so the document holds both.
    let ink = deck.slides[0]
        .shapes
        .iter()
        .filter(|shape| matches!(shape, Shape::Picture(_)))
        .count();
    assert_eq!(
        ink, 5,
        "a text box with no ink was painted: {:#?}",
        deck.slides[0].shapes
    );
}
