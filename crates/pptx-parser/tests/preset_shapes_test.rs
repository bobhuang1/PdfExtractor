//! The preset shapes a deck paints itself, read from `test-data/preset_deck.pptx`.
//!
//! `shapes_test` pins the handful of presets a hand-written deck holds; this one
//! reads the library a real writer produced (`python-pptx`, see
//! `test-data/make_preset_deck.py`): one shape for every preset the painter
//! covers, so a preset that is left out of the table — or drawn as something the
//! file did not state — shows up as a missing or wrong picture.

use image::GenericImageView as _;
use pdfextractor_pptx_parser::{PptxDocument, Shape};
use std::path::PathBuf;

fn deck() -> PptxDocument {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/preset_deck.pptx");
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    PptxDocument::parse_bytes(&data).expect("parse")
}

/// Every preset shape of the deck, then the elbow and the curve it holds.
const PICTURES: usize = 68;

/// The pictures of the one slide, in reading order.
fn pictures(deck: &PptxDocument) -> Vec<image::RgbaImage> {
    deck.slides[0]
        .shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Picture(picture) => Some(
                image::load_from_memory(&picture.data)
                    .unwrap_or_else(|e| panic!("a painted shape does not decode: {e}"))
                    .to_rgba8(),
            ),
            _ => None,
        })
        .collect()
}

fn white(pixel: [u8; 4]) -> bool {
    pixel[..3] == [255, 255, 255]
}

#[test]
fn every_preset_in_the_deck_is_a_picture() {
    let deck = deck();
    assert_eq!(deck.slides.len(), 1);
    let shapes = &deck.slides[0].shapes;
    let painted: Vec<&Shape> = shapes
        .iter()
        .filter(|shape| matches!(shape, Shape::Picture(_)))
        .collect();
    assert_eq!(
        painted.len(),
        PICTURES,
        "a preset was left out of the picture list: {shapes:#?}"
    );

    // Each autoshape is a 90 × 50 pt cell, drawn at two device pixels per
    // point; the two connectors span two cells and are as tall as one.
    for (index, shape) in painted.iter().enumerate() {
        let Shape::Picture(picture) = shape else {
            unreachable!()
        };
        assert_eq!(picture.mime, "image/png", "picture {index}");
        // The elbow and the curve span two cells; every preset shape one.
        let expected = if matches!(index, 66 | 67) {
            (360, 100)
        } else {
            (180, 100)
        };
        let image = image::load_from_memory(&picture.data)
            .unwrap_or_else(|e| panic!("picture {index} does not decode: {e}"));
        assert_eq!(
            image.dimensions(),
            expected,
            "picture {index} is not the size of its frame"
        );
    }
}

#[test]
fn the_presets_are_drawn_as_their_own_outlines() {
    let deck = deck();
    let pictures = pictures(&deck);
    assert_eq!(pictures.len(), PICTURES);

    // The theme's accent fills each shape; its `lnRef` draws the line.
    let accent = [0x4F, 0x81, 0xBD, 255];

    // `round1Rect` rounds one corner, the top right: the box is fill out to
    // its other three corners.
    let round1 = &pictures[0];
    assert_eq!(round1.get_pixel(4, 4).0, accent, "the square corner");
    assert!(
        white(round1.get_pixel(176, 4).0),
        "the rounded corner is filled"
    );

    // A donut is a ring: its middle is a hole rather than more fill.
    let donut = &pictures[8];
    assert!(white(donut.get_pixel(90, 50).0), "the hole");
    assert_eq!(donut.get_pixel(90, 8).0, accent, "the ring");

    // A no-smoking sign is that ring with a rule across it, so the hole is
    // still empty beside the rule.
    let sign = &pictures[9];
    assert!(white(sign.get_pixel(120, 45).0), "the hole beside the rule");

    // A frame is a band around a window.
    let frame = &pictures[10];
    assert!(white(frame.get_pixel(90, 50).0), "the window");
    assert_eq!(frame.get_pixel(90, 4).0, accent, "the band");

    // A brace is a line rather than a region: almost none of its cell is
    // inked, where a filled brace would cover half of it.
    let brace = &pictures[22];
    let inked = brace.pixels().filter(|pixel| !white(pixel.0)).count();
    let cell = (brace.width() * brace.height()) as usize;
    assert!(
        inked * 10 < cell,
        "a brace was filled: {inked} of {cell} pixels"
    );
    assert!(inked > 0, "a brace put no ink down at all");

    // A cube states its depth: the cell above its top face is empty, and the
    // top face and front face are fill.
    let cube = &pictures[27];
    assert!(white(cube.get_pixel(2, 2).0), "above the top face");
    assert_eq!(cube.get_pixel(90, 10).0, accent, "the top face");
    assert_eq!(cube.get_pixel(90, 60).0, accent, "the front face");

    // A cylinder's rim closes its cap below the top edge; the body is fill.
    let can = &pictures[30];
    assert!(
        !white(can.get_pixel(90, 25).0),
        "the cap's rim is not drawn"
    );
    assert_eq!(can.get_pixel(90, 80).0, accent, "the body");

    // The flowchart's document dips along its bottom edge: its lowest ink
    // near the left of the cell stands higher than near the right.
    let document = &pictures[32];
    let lowest = |x: u32| {
        (0..document.height())
            .rev()
            .find(|y| !white(document.get_pixel(x, *y).0))
            .unwrap_or(0)
    };
    assert!(
        lowest(20) + 6 < lowest(140),
        "the bottom edge is not a wave: {} against {}",
        lowest(20),
        lowest(140)
    );

    // The moon's bite leaves its right side empty and its left side full.
    let moon = &pictures[58];
    assert!(white(moon.get_pixel(170, 50).0), "the bite");
    assert_eq!(moon.get_pixel(20, 50).0, accent, "the crescent");

    // A block arc is a piece of a ring the shape is not: the band along the
    // top is fill, and the half it encloses is not.
    let block = &pictures[65];
    assert_eq!(block.get_pixel(90, 8).0, accent, "the band");
    assert!(white(block.get_pixel(90, 50).0), "the middle");

    // The arrows that bend round are bands rather than boxes: the curved right
    // arrow leaves the left edge and reaches the right one with its head,
    // while the bottom middle stays empty.
    let curved_right = &pictures[48];
    assert_eq!(
        curved_right.get_pixel(4, 40).0,
        accent,
        "the band at the left edge"
    );
    assert!(
        (0..100).any(|y| !white(curved_right.get_pixel(179, y).0)),
        "the head does not reach the right edge"
    );
    assert!(
        white(curved_right.get_pixel(90, 90).0),
        "the band became a box"
    );

    // Its left arrow is the same band reaching the other way.
    let curved_left = &pictures[49];
    assert_eq!(
        curved_left.get_pixel(175, 40).0,
        accent,
        "the band at the right edge"
    );
    assert!(
        (0..100).any(|y| !white(curved_left.get_pixel(0, y).0)),
        "the head does not reach the left edge"
    );

    // The up arrow is a bowl: both arms are inked at the top and the mouth
    // between them is not, while the down arrow is that bowl turned over.
    let curved_up = &pictures[50];
    assert_eq!(curved_up.get_pixel(6, 4).0, accent, "the left arm");
    assert_eq!(curved_up.get_pixel(157, 4).0, accent, "the head");
    assert!(white(curved_up.get_pixel(90, 30).0), "the bowl's mouth");

    let curved_down = &pictures[51];
    assert_eq!(curved_down.get_pixel(6, 95).0, accent, "the left arm");
    assert_eq!(curved_down.get_pixel(157, 95).0, accent, "the head");
    assert_eq!(curved_down.get_pixel(145, 40).0, accent, "the right arm");
    assert!(white(curved_down.get_pixel(90, 70).0), "the bowl's mouth");

    // A U-turn is two arms joined over an empty middle.
    let uturn = &pictures[52];
    assert_eq!(uturn.get_pixel(10, 50).0, accent, "the left arm");
    assert_eq!(uturn.get_pixel(150, 50).0, accent, "the right arm");
    assert!(white(uturn.get_pixel(90, 40).0), "the middle");

    // A circular arrow is a band with a gap rather than a disc.
    let circular = &pictures[53];
    assert_eq!(circular.get_pixel(90, 6).0, accent, "the band");
    assert!(white(circular.get_pixel(90, 50).0), "the hole");

    // A swoosh leaves its tail at the bottom left and sweeps up into the head
    // at the top right, with the space below the band empty.
    let swoosh = &pictures[54];
    assert_eq!(swoosh.get_pixel(2, 95).0, accent, "the tail");
    assert_eq!(swoosh.get_pixel(176, 30).0, accent, "the head");
    assert!(white(swoosh.get_pixel(90, 90).0), "below the band");

    // The elbow is a line, not a region: the two segments are drawn and the
    // cell between them is not.
    let elbow = &pictures[66];
    assert!(
        !white(elbow.get_pixel(180, 10).0),
        "the first segment of the elbow"
    );
    assert!(white(elbow.get_pixel(60, 40).0), "the inside of the elbow");
}
