use pdfextractor_pdf_parser::PdfDocument;
use std::fs;

fn hello_bytes() -> Vec<u8> {
    fs::read("../../test-data/hello.pdf").expect("read test-data/hello.pdf")
}

#[test]
fn extracts_text_from_content_stream() {
    let data = hello_bytes();
    let doc = PdfDocument::parse(&data).expect("parse pdf");
    let pages = doc.extract();

    assert_eq!(pages.len(), 1, "hello.pdf has exactly one page");

    let text = pages[0]
        .spans
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    assert!(
        text.contains("Hello World"),
        "expected to extract 'Hello World', got {text:?}"
    );
}

#[test]
fn page_size_comes_from_mediabox() {
    let data = hello_bytes();
    let doc = PdfDocument::parse(&data).expect("parse pdf");
    let pages = doc.extract();
    assert_eq!(pages[0].width, 612.0);
    assert_eq!(pages[0].height, 792.0);
}
