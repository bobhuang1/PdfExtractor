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

/// Two lines drawn in the producer style that put each run in its own `Td`:
/// the first repositions the pen exactly where the glyph advances stopped, the
/// second jumps a further `0.3 em` before drawing. The first jump is not a word
/// break and must not become a space; the second is, and must.
///
/// `dummy.pdf` writes every run this way and used to come out as
/// `Dumm y  PDF  fi le` — each run's repositioning read as a gap even though
/// the text carried its own space characters.
fn positioned_runs_pdf() -> Vec<u8> {
    // /Widths for codes 97..=100 (`a`..`d`): 500 units each, so at size 10 the
    // pen advances 0.5 em per glyph and `abc` measures exactly 15 pt.
    let stream = "BT /F1 10 Tf 10 50 Td (abc) Tj 15 0 Td (d) Tj ET\n\
                  BT /F1 10 Tf 10 70 Td (abc) Tj 18 0 Td (d) Tj ET\n";
    format!(
        "%PDF-1.4\n\
         1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] \
         /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >> endobj\n\
         4 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
         /FirstChar 97 /LastChar 100 /Widths [500 500 500 500] >> endobj\n\
         5 0 obj << /Length {} >>\nstream\n{stream}endstream\nendobj\n",
        stream.len()
    )
    .into_bytes()
}

#[test]
fn a_repositioning_td_is_not_a_space_but_an_overshoot_is() {
    let doc = PdfDocument::parse(&positioned_runs_pdf()).expect("parse");
    let pages = doc.extract();
    let spans = &pages[0].spans;
    assert_eq!(spans.len(), 2, "each text object is one run");

    assert_eq!(
        spans[0].text, "abcd",
        "a Td landing on the drawn advance is repositioning, not a word gap"
    );
    assert_eq!(
        spans[1].text, "abc d",
        "a Td that overshoots the drawn advance is a word gap the producer \
         expressed by moving the pen"
    );
}
