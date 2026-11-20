//! Damaged files: what the parser reads when the cross-reference data lies.
//!
//! The parser never trusts `startxref` or the xref table: it reads every
//! `N G obj` declaration in the file, the later of two declarations of one
//! object winning (which is what an incremental update means). That is the
//! repair path MuPDF, PDFBox and qpdf fall back to, taken every time. These
//! tests damage real files in the ways that path exists for and hold the
//! result to the undamaged reading — measured, not assumed.

use pdfextractor_pdf_parser::PdfDocument;

fn fixture(path: &str) -> Vec<u8> {
    std::fs::read(format!("../../test-data/{path}")).expect("the fixture is committed")
}

/// Every page's text, page by page.
fn pages_text(data: &[u8]) -> Vec<String> {
    let doc = PdfDocument::parse(data).expect("parse");
    doc.extract()
        .iter()
        .map(|page| {
            page.spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn find(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).position(|w| w == needle)
}

fn rfind(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).rposition(|w| w == needle)
}

const FILES: [&str; 3] = ["hello.pdf", "real/dummy.pdf", "real/basicapi.pdf"];

#[test]
fn a_startxref_that_points_nowhere_changes_nothing() {
    for name in FILES {
        let original = fixture(name);
        let at = rfind(&original, b"startxref").expect("a startxref") + b"startxref".len();
        let mut damaged = original[..at].to_vec();
        damaged.extend_from_slice(b"\n999999999\n%%EOF\n");
        assert_eq!(pages_text(&damaged), pages_text(&original), "{name}");
    }
}

#[test]
fn a_file_with_no_xref_and_no_trailer_still_reads() {
    for name in FILES {
        let original = fixture(name);
        // Cut after the last object: the classic table, the trailer and the
        // startxref all go (a cross-reference *stream* is an object and stays,
        // which is what a writer that died mid-trailer leaves).
        let end = rfind(&original, b"endobj").expect("an object") + b"endobj".len();
        let damaged = original[..end].to_vec();
        let expected = pages_text(&original);
        assert!(!expected.concat().trim().is_empty(), "{name} has text to compare");
        assert_eq!(pages_text(&damaged), expected, "{name}");
    }
}

#[test]
fn objects_moved_away_from_their_xref_offsets_still_read() {
    for name in FILES {
        let original = fixture(name);
        // Junk after the header shifts every object, so every xref offset lies.
        let header_end = find(&original, b"\n").expect("a header line") + 1;
        let mut damaged = original[..header_end].to_vec();
        damaged.extend_from_slice(b"%junk inserted by a broken transfer: 0 0 0 0 0 0 0 0\n");
        damaged.extend_from_slice(&original[header_end..]);
        assert_eq!(pages_text(&damaged), pages_text(&original), "{name}");
    }
}

#[test]
fn a_truncated_file_reads_what_it_still_holds_without_panicking() {
    for name in ["real/basicapi.pdf", "real/tracemonkey.pdf", "real/irs_f1040.pdf"] {
        let original = fixture(name);
        for share in [10, 50, 90] {
            let cut = original.len() * share / 100;
            // Only that it returns: what survives depends on where the cut fell.
            let _ = pages_text(&original[..cut]);
        }
    }
}

/// An incremental update that adds a *new* catalog under a new object number:
/// the newest trailer's `/Root` decides, every time — not whichever catalog a
/// hash map happens to yield first.
#[test]
fn the_newest_trailers_root_decides_the_page_tree() {
    let original = fixture("hello.pdf");
    let mut updated = original.clone();
    let base = updated.len();
    let objects: [(usize, String); 4] = [
        (90, "<< /Type /Catalog /Pages 91 0 R >>".into()),
        (91, "<< /Type /Pages /Kids [92 0 R] /Count 1 >>".into()),
        (
            92,
            "<< /Type /Page /Parent 91 0 R /MediaBox [0 0 200 200] /Contents 93 0 R \
             /Resources << /Font << /F1 94 0 R >> >> >>"
                .into(),
        ),
        (93, {
            let content = "BT /F1 12 Tf 20 100 Td (Revised page) Tj ET";
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
        }),
    ];
    let mut offsets = Vec::new();
    for (num, body) in &objects {
        offsets.push((*num, updated.len()));
        updated.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    offsets.push((94, updated.len()));
    updated.extend_from_slice(
        b"94 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
    );
    let xref_at = updated.len();
    let mut xref = String::from("xref\n");
    for (num, offset) in &offsets {
        xref.push_str(&format!("{num} 1\n{offset:010} 00000 n \n"));
    }
    xref.push_str(&format!(
        "trailer\n<< /Size 95 /Root 90 0 R /Prev {base} >>\nstartxref\n{xref_at}\n%%EOF\n"
    ));
    updated.extend_from_slice(xref.as_bytes());

    for _ in 0..8 {
        let text = pages_text(&updated);
        assert_eq!(text.len(), 1, "{text:?}");
        assert!(text[0].contains("Revised page"), "the old catalog was read: {text:?}");
    }
}
