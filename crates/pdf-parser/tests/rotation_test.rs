//! `/Rotate`: a viewer turns the page clockwise before showing it, so every
//! box the page produced is turned with it — once, at extraction — and
//! everything downstream sees the page as displayed.
//!
//! The fixtures are written here rather than committed: one page, a line of
//! text and a thin rule, with and without the page's own rotation. Each
//! rotated document's expected boxes come from its own unrotated twin, so the
//! relation asserted is the one a reader sees — turn the page, the boxes turn
//! with it — and no number is copied from the implementation.

use pdfextractor_pdf_parser::{ExtractedPage, PdfDocument};

/// One `200 × 100` page: a line of text and a full-width rule under it.
fn page_pdf(rotation: Option<i32>, on_parent: bool) -> Vec<u8> {
    let rotate = rotation
        .map(|r| format!("/Rotate {r} "))
        .unwrap_or_default();
    let (page_rotate, parent_rotate) = if on_parent {
        (String::new(), rotate)
    } else {
        (rotate, String::new())
    };
    let stream = "BT /F1 12 Tf 10 40 Td (Hello) Tj ET\n0 5 m 200 5 l S\n";
    format!(
        "%PDF-1.4\n\
         1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages {parent_rotate}/Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R {page_rotate}/MediaBox [0 0 200 100] \
         /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >> endobj\n\
         4 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n\
         5 0 obj << /Length {} >>\nstream\n{stream}endstream\nendobj\n",
        stream.len()
    )
    .into_bytes()
}

fn extract(rotation: Option<i32>) -> ExtractedPage {
    extract_from(page_pdf(rotation, false))
}

fn extract_from(bytes: Vec<u8>) -> ExtractedPage {
    let doc = PdfDocument::parse(&bytes).expect("parse the page");
    let mut pages = doc.extract();
    assert_eq!(pages.len(), 1);
    pages.remove(0)
}

/// The same turn the spec describes, written out here so the test pins the
/// behaviour rather than echoing the implementation.
fn turned(bbox: [f64; 4], degrees: i32, width: f64, height: f64) -> [f64; 4] {
    let [l, t, r, b] = bbox;
    match degrees {
        90 => [height - b, l, height - t, r],
        180 => [width - r, height - b, width - l, height - t],
        270 => [b, width - r, t, width - l],
        _ => unreachable!("the twin is unrotated"),
    }
}

fn assert_same_box(a: [f64; 4], b: [f64; 4]) {
    for i in 0..4 {
        assert!(
            (a[i] - b[i]).abs() < 1e-9,
            "box {a:?} does not match {b:?} (component {i})"
        );
    }
}

#[test]
fn an_unrotated_page_is_untouched() {
    let page = extract(None);
    assert_eq!(page.rotation, 0);
    assert_eq!(page.width, 200.0);
    assert_eq!(page.height, 100.0);
    assert_eq!(page.spans.len(), 1);
    assert!(!page.spans[0].rotated);
}

#[test]
fn ninety_degrees_turns_the_page_and_every_box() {
    let plain = extract(None);
    let page = extract(Some(90));

    assert_eq!(page.rotation, 90);
    assert_eq!(page.width, 100.0, "the page is turned, so its sides swap");
    assert_eq!(page.height, 200.0);

    assert_eq!(page.spans.len(), 1);
    assert_eq!(page.spans[0].text, "Hello", "the words are unchanged");
    assert!(
        page.spans[0].rotated,
        "a run that read left-to-right now reads down the page"
    );
    assert_same_box(
        page.spans[0].bbox,
        turned(plain.spans[0].bbox, 90, 200.0, 100.0),
    );

    assert_eq!(page.rules.len(), 1, "the page draws one rule");
    assert_same_box(page.rules[0], turned(plain.rules[0], 90, 200.0, 100.0));
}

#[test]
fn one_hundred_eighty_turns_the_page_and_its_text() {
    let plain = extract(None);
    let page = extract(Some(180));

    assert_eq!(page.rotation, 180);
    assert_eq!(page.width, 200.0, "a half turn keeps the sides");
    assert_eq!(page.height, 100.0);
    assert!(
        page.spans[0].rotated,
        "upside-down text is not horizontal text"
    );
    assert_same_box(
        page.spans[0].bbox,
        turned(plain.spans[0].bbox, 180, 200.0, 100.0),
    );
}

#[test]
fn two_hundred_seventy_turns_the_other_way() {
    let plain = extract(None);
    let page = extract(Some(270));

    assert_eq!(page.rotation, 270);
    assert_eq!(page.width, 100.0);
    assert_eq!(page.height, 200.0);
    assert!(page.spans[0].rotated);
    assert_same_box(
        page.spans[0].bbox,
        turned(plain.spans[0].bbox, 270, 200.0, 100.0),
    );
}

#[test]
fn the_rotation_is_inherited_from_the_parent_tree() {
    let page = extract_from(page_pdf(Some(90), true));
    assert_eq!(page.rotation, 90, "/Rotate on the Pages node counts too");
    assert_eq!(page.width, 100.0);
}

#[test]
fn a_turn_that_is_not_a_quadrant_is_ignored() {
    for (given, expected) in [(45, 0), (-90, 270), (360 + 90, 90)] {
        let page = extract(Some(given));
        assert_eq!(page.rotation, expected, "rotation {given}");
    }
}
