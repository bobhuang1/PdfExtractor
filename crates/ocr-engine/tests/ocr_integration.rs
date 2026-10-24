//! End-to-end OCR: take the embedded image out of a scanned-style PDF fixture
//! and recognise its text with the real Tesseract backend.
//!
//! Gated on the `tesseract` feature, which is on by default because OCR is a
//! required capability of this project.
#![cfg(feature = "tesseract")]

use pdfextractor_ocr_engine::{OcrEngine, QuarterTurn};
use pdfextractor_pdf_parser::PdfDocument;

fn scanned_page_image() -> (Vec<u8>, u32, u32) {
    let data = std::fs::read("../../test-data/scanned.pdf").expect("read scanned.pdf fixture");
    let doc = PdfDocument::parse(&data).expect("parse scanned.pdf");
    let pages = doc.extract();
    assert_eq!(pages.len(), 1);
    assert!(
        pages[0].spans.is_empty(),
        "fixture must not have a text layer, otherwise OCR is not exercised"
    );
    let img = pages[0].images.first().expect("one embedded image");
    (img.data.clone(), img.width, img.height)
}

#[test]
fn recognises_text_in_a_scanned_page() {
    let (jpeg, width, height) = scanned_page_image();

    let mut engine = OcrEngine::new("eng").expect("initialise Tesseract");
    let result = engine.recognize(&jpeg).expect("ocr");

    let joined = result
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join(" | ");

    // Assert on content, not just that something came back.
    assert!(
        joined.contains("ACME"),
        "missing header text; got: {joined}"
    );
    assert!(
        joined.contains("12345"),
        "missing invoice number; got: {joined}"
    );
    assert!(joined.contains("Total Due"), "missing total; got: {joined}");
    assert!(
        result.lines.len() >= 5,
        "expected at least 5 lines, got {}: {joined}",
        result.lines.len()
    );

    // Boxes must be non-degenerate and lie within the image.
    for line in &result.lines {
        let [l, t, r, b] = line.bbox;
        assert!(
            r > l && b > t,
            "degenerate box {:?} for {:?}",
            line.bbox,
            line.text
        );
        assert!(l >= 0 && t >= 0, "negative origin {:?}", line.bbox);
        assert!(
            r <= width as i32 + 1 && b <= height as i32 + 1,
            "box {:?} outside image {}x{}",
            line.bbox,
            width,
            height
        );
    }

    // Word-level output should be at least as granular as the lines.
    assert!(
        result.words.len() >= result.lines.len(),
        "expected words ({}) to be >= lines ({})",
        result.words.len(),
        result.lines.len()
    );
}

/// A photographed page with nothing in the file saying which way up it is:
/// the detector has to read the pixels in all four turns and find the one the
/// page is actually written in.
fn photo(name: &str) -> Vec<u8> {
    std::fs::read(format!("../../test-data/photos/{name}"))
        .unwrap_or_else(|e| panic!("missing {name}: {e}"))
}

#[test]
fn a_sideways_page_is_found_without_a_tag() {
    let mut engine = OcrEngine::new("eng").expect("initialise Tesseract");
    assert_eq!(
        engine
            .detect_upright(&photo("07_sideways.jpg"))
            .expect("find the orientation"),
        QuarterTurn::Clockwise90,
        "the invoice is stored a quarter turn anticlockwise"
    );
}

#[test]
fn an_upside_down_page_is_turned_round() {
    let mut engine = OcrEngine::new("eng").expect("initialise Tesseract");
    assert_eq!(
        engine
            .detect_upright(&photo("08_upside_down.png"))
            .expect("find the orientation"),
        QuarterTurn::Clockwise180
    );
}

#[test]
fn an_upright_page_is_left_as_stored() {
    let mut engine = OcrEngine::new("eng").expect("initialise Tesseract");
    for name in ["01_flat.png", "02_skew.png", "05_photo.jpg"] {
        assert_eq!(
            engine
                .detect_upright(&photo(name))
                .expect("find the orientation"),
            QuarterTurn::AsStored,
            "{name} is already upright"
        );
    }
}

/// A page segmentation mode can be chosen, and an impossible one is refused
/// rather than smuggled to Tesseract as a string.
#[test]
fn a_page_segmentation_mode_can_be_chosen() {
    let (jpeg, _, _) = scanned_page_image();

    let mut engine = OcrEngine::new("eng").expect("initialise Tesseract");

    // 3 is the engine's own default and 6 one block of text; both must be
    // accepted *and* still recognise the page — setting a variable is not
    // worth anything if it quietly breaks recognition.
    for psm in [3u8, 6] {
        engine.set_psm(psm).expect("the mode is accepted");
        let result = engine.recognize(&jpeg).expect("ocr");
        let joined = result
            .lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            joined.contains("ACME"),
            "--psm {psm} lost the page: {joined}"
        );
    }

    // Tesseract's own modes stop at 13.
    assert!(
        engine.set_psm(14).is_err(),
        "an impossible mode was accepted"
    );
}
