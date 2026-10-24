//! `/Link` annotations: where each one points and the box it covers.
//!
//! `test-data/link_annotations.pdf` is built for this suite: two pages, a URI
//! link, an internal destination, a hidden link (flag bit 2) and a non-link
//! annotation, so every branch has its own fixture rather than a hopeful
//! reading of whichever corpus file happens to be open.

use pdfextractor_pdf_parser::PdfDocument;
use std::fs;

#[test]
fn link_annotations_are_read_with_their_boxes() {
    let data = fs::read("../../test-data/link_annotations.pdf").expect("read the fixture");
    let doc = PdfDocument::parse(&data).expect("parse pdf");
    let pages = doc.extract();
    assert_eq!(pages.len(), 2);

    let first = &pages[0];
    assert_eq!(
        first.links.len(),
        2,
        "the hidden link and the /Text annotation are not links: {:?}",
        first.links
    );

    let uri = first
        .links
        .iter()
        .find(|l| l.uri.starts_with("https://"))
        .expect("the URI link");
    assert_eq!(uri.uri, "https://example.com/spec");
    // `/Rect [72 296 220 318]` on a 792 pt page: 792 - 318 = 474, 792 - 296 =
    // 496 — the flip out of PDF's bottom-left origin.
    assert_eq!(uri.rect, [72.0, 474.0, 220.0, 496.0]);

    let dest = first
        .links
        .iter()
        .find(|l| l.uri.starts_with('#'))
        .expect("the internal destination link");
    assert_eq!(dest.uri, "#page=1");
    assert_eq!(dest.rect, [10.0, 762.0, 100.0, 782.0]);

    // The hidden link's rectangle would cover text on the other half of the
    // page, and a link nobody can see does not open.
    assert!(
        !first.links.iter().any(|l| l.uri.contains("hidden")),
        "{:?}",
        first.links
    );

    // Page 2 holds only a `/Text` annotation: not a link, not read.
    assert!(pages[1].links.is_empty(), "{:?}", pages[1].links);
}
