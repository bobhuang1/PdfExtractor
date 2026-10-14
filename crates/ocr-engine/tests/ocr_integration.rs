//! End-to-end OCR: take the embedded image out of a scanned-style PDF fixture
//! and recognise its text with the real Tesseract backend.
//!
//! Gated on the `tesseract` feature, which is on by default because OCR is a
//! required capability of this project.
#![cfg(feature = "tesseract")]

use pdfextractor_ocr_engine::OcrEngine;
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
