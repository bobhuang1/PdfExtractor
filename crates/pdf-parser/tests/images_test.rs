use pdfextractor_pdf_parser::PdfDocument;
use std::fs;

#[test]
fn extracts_embedded_jpeg_from_scanned_page() {
    let data = fs::read("../../test-data/scanned.pdf").expect("read scanned.pdf");
    let doc = PdfDocument::parse(&data).expect("parse");
    let pages = doc.extract();

    assert_eq!(pages.len(), 1, "one page");
    assert!(pages[0].spans.is_empty(), "a scan has no text layer");

    assert_eq!(
        pages[0].images.len(),
        1,
        "expected exactly one image XObject, got {}",
        pages[0].images.len()
    );
    let img = &pages[0].images[0];
    assert_eq!(img.mime, "image/jpeg");
    assert_eq!(&img.data[..2], &[0xFF, 0xD8], "JPEG SOI marker");
    assert_eq!(img.width, 1275);
    assert_eq!(img.height, 1650);
    assert!(
        img.data.len() > 10_000,
        "jpeg payload should be substantial"
    );
}
