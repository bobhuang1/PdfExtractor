//! A slide's picture background, read from two fixtures.
//!
//! `test-data/background_deck.pptx` is authored by python-pptx and completed
//! with the one thing it cannot write: `p:cSld/p:bg/p:bgPr/a:blipFill` on the
//! first slide, naming `images/scan.jpg` through that slide's own
//! relationships. The second slide has no `p:bg` at all, so the reading is held
//! to a per-slide fact rather than to a deck-wide one.
//!
//! `test-data/master_background_deck.pptx` keeps the photograph in the slide
//! *master* instead, and none of its slides states a picture background: the
//! backdrop is inherited, from the layout and then the master, which is where a
//! deck usually keeps one picture behind every slide. Its third slide states a
//! colour background of its own, which is where that inheritance stops.
//!
//! `test-data/layout_background_deck.pptx` covers the middle of that chain: the
//! master states one picture and the blank layout another, so a slide that
//! states neither must take the layout's — the nearer level — and not the
//! master's that lies behind it.
//!
//! `test-data/layout_colour_background_deck.pptx` asks the other half of that
//! level: its layout states a colour rather than a picture, so the master's
//! photograph behind it must not reach the slides under it — a colour is a
//! background, and the chain stops at the part that states one — while the
//! third slide, which states a picture of its own, still gets one.

use pdfextractor_pptx_parser::{PptxDocument, Shape};
use std::path::PathBuf;

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data")
        .join(name)
}

fn deck(name: &str) -> PptxDocument {
    let path = path(name);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    PptxDocument::parse_bytes(&data).expect("parse")
}

#[test]
fn a_picture_background_is_the_slides_own_image() {
    let deck = deck("background_deck.pptx");
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
fn a_picture_background_the_layout_states_is_the_slides_before_the_masters() {
    let deck = deck("layout_background_deck.pptx");
    assert_eq!(deck.slides.len(), 3);
    let layout_picture = std::fs::read(path("images/scan.jpg")).expect("the layout's backdrop");
    let master_picture = std::fs::read(path("images/scan.webp")).expect("the master's backdrop");
    assert_ne!(
        layout_picture, master_picture,
        "the two levels state different pictures, or the reading could not tell them apart"
    );

    // The first two slides state no background of their own, and the layout
    // between them and the master states one: it is the level that answers.
    for (index, slide) in deck.slides.iter().take(2).enumerate() {
        let background = slide
            .background
            .as_ref()
            .unwrap_or_else(|| panic!("slide {} inherits the layout's picture", index + 1));
        assert_eq!(
            background.mime,
            "image/jpeg",
            "slide {}: the layout's media type, not the master's",
            index + 1
        );
        assert_eq!(
            background.data,
            layout_picture,
            "slide {}: the layout is the nearer level, so its picture is the one behind the page",
            index + 1
        );
        assert_eq!(
            background.bbox,
            [0.0, 0.0, deck.width, deck.height],
            "a background covers the slide's page"
        );
        assert_eq!(
            slide.shapes.len(),
            1,
            "slide {}: an inherited backdrop is the page's image, not a shape",
            index + 1
        );
        assert!(matches!(slide.shapes[0], Shape::Text(_)));
    }

    // The third slide states what is behind it, and a colour is not either
    // picture: the inheritance stops at the slide, before the layout.
    assert!(
        deck.slides[2].background.is_none(),
        "a slide that states its own colour is not given the layout's picture"
    );
    assert_eq!(deck.slides[2].shapes.len(), 1);
    assert!(matches!(deck.slides[2].shapes[0], Shape::Text(_)));
}

#[test]
fn a_layouts_colour_stops_the_chain_before_the_masters_picture() {
    let deck = deck("layout_colour_background_deck.pptx");
    assert_eq!(deck.slides.len(), 3);
    let slide_picture = std::fs::read(path("images/scan.jpg")).expect("the slide's backdrop");
    let master_picture = std::fs::read(path("images/scan.webp")).expect("the master's backdrop");
    assert_ne!(
        slide_picture, master_picture,
        "the two pictures differ, or the reading could not tell them apart"
    );

    // The first two slides state no background of their own, and the layout
    // between them and the master states a colour: a background that is not a
    // picture stops the chain, so the master's photograph never arrives.
    for (index, slide) in deck.slides.iter().take(2).enumerate() {
        assert!(
            slide.background.is_none(),
            "slide {}: the layout's colour is a background, so the master's picture behind it is not the page's image",
            index + 1
        );
        assert_eq!(
            slide.shapes.len(),
            1,
            "slide {}: a coloured backdrop is the page's, not a shape",
            index + 1
        );
        assert!(matches!(slide.shapes[0], Shape::Text(_)));
    }

    // The third slide states a picture of its own, the nearest level of all, so
    // the reading still finds a backdrop where one is stated: the bare pages
    // above are the layout's colour rather than a reading that lost pictures.
    let background = deck.slides[2]
        .background
        .as_ref()
        .expect("the third slide's own picture");
    assert_eq!(background.mime, "image/jpeg");
    assert_eq!(
        background.data, slide_picture,
        "the third slide's picture is its own, not the master's"
    );
    assert_eq!(
        background.bbox,
        [0.0, 0.0, deck.width, deck.height],
        "a background covers the slide's page"
    );
    assert_eq!(deck.slides[2].shapes.len(), 1);
    assert!(matches!(deck.slides[2].shapes[0], Shape::Text(_)));
}

#[test]
fn a_slide_without_a_background_has_no_page_image() {
    let deck = deck("background_deck.pptx");
    assert!(deck.slides[1].background.is_none());
    assert!(
        deck.slides[1]
            .shapes
            .iter()
            .all(|shape| matches!(shape, Shape::Text(_))),
        "the second slide holds only its sentence"
    );
}

#[test]
fn a_picture_background_the_master_states_is_every_slides() {
    let deck = deck("master_background_deck.pptx");
    assert_eq!(deck.slides.len(), 3);
    let scan = std::fs::read(path("images/scan.jpg")).expect("the backdrop fixture");

    // The first two slides state no background of their own; the photograph is
    // the master's, reached through their layout and then its master.
    for (index, slide) in deck.slides.iter().take(2).enumerate() {
        let background = slide
            .background
            .as_ref()
            .unwrap_or_else(|| panic!("slide {} inherits the master's picture", index + 1));
        assert_eq!(background.mime, "image/jpeg");
        assert_eq!(
            background.data,
            scan,
            "slide {}: the background is the master's media part",
            index + 1
        );
        assert_eq!(
            background.bbox,
            [0.0, 0.0, deck.width, deck.height],
            "a background covers the slide's page"
        );
        assert_eq!(
            slide.shapes.len(),
            1,
            "slide {}: an inherited backdrop is the page's image, not a shape",
            index + 1
        );
        assert!(matches!(slide.shapes[0], Shape::Text(_)));
    }

    // The third slide says what is behind it, and a colour is not the master's
    // photograph: the inheritance stops at the slide's own background.
    assert!(
        deck.slides[2].background.is_none(),
        "a slide that states its own colour is not given the master's picture"
    );
    assert_eq!(deck.slides[2].shapes.len(), 1);
    assert!(matches!(deck.slides[2].shapes[0], Shape::Text(_)));
}
