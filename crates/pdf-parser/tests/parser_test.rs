use pdfextractor_pdf_parser::{objects::PdfValue, PdfDocument};
use std::fs;

#[test]
fn parses_indirect_objects_and_streams() {
    let data = fs::read("../../test-data/hello.pdf").expect("read test-data/hello.pdf");
    let doc = PdfDocument::parse(&data).expect("parse pdf");

    assert!(!doc.objects.is_empty(), "expected indirect objects");
    assert_eq!(doc.version, "1.4", "header version should be parsed");

    // Object 4 0 is the content stream in the generated fixture.
    let stream = doc.objects.get(&(4, 0)).expect("object 4 0 exists");
    assert!(
        matches!(stream, PdfValue::Stream(_, _)),
        "object 4 0 should be a stream, got {stream:?}"
    );

    // The catalog must be discoverable.
    let has_catalog = doc.objects.values().any(|v| {
        v.as_dict()
            .and_then(|d| d.get("Type"))
            .map(|t| matches!(t, PdfValue::Name(n) if n == "Catalog"))
            .unwrap_or(false)
    });
    assert!(has_catalog, "no /Type /Catalog object found");
}
