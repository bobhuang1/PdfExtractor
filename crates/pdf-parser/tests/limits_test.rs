use std::io::Write;
use std::time::Duration;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use pdfextractor_limits::{LimitError, Limits};
use pdfextractor_pdf_parser::PdfDocument;

/// A three-page PDF with no content streams.
fn three_pages() -> Vec<u8> {
    let mut pdf = String::from("%PDF-1.4\n");
    pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>\nendobj\n");
    for page in 3..=5 {
        pdf.push_str(&format!(
            "{page} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>\nendobj\n"
        ));
    }
    pdf.push_str("trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    pdf.into_bytes()
}

#[test]
fn a_page_tree_over_the_budget_is_refused_not_truncated() {
    let pdf = three_pages();
    let limits = Limits {
        max_pages: 2,
        ..Limits::default()
    };
    let doc = PdfDocument::parse_with_limits(&pdf, limits).expect("parse");
    let error = doc.extract_checked().unwrap_err();
    assert!(error.to_string().contains("3 pages"), "{error}");
}

#[test]
fn a_document_over_the_token_budget_is_refused() {
    let pdf = three_pages();
    let limits = Limits {
        max_pdf_tokens: 4,
        ..Limits::default()
    };
    let error = PdfDocument::parse_with_limits(&pdf, limits).unwrap_err();
    assert!(error.to_string().contains("tokens"), "{error}");
}

#[test]
fn the_default_budget_reads_all_three_pages() {
    let pdf = three_pages();
    let doc = PdfDocument::parse(&pdf).expect("parse");
    assert_eq!(doc.extract_checked().expect("extract").len(), 3);
}

#[test]
fn a_zero_deadline_is_a_parser_timeout_with_a_stage_name() {
    let pdf = three_pages();
    let limits = Limits {
        timeout: Duration::ZERO,
        ..Limits::default()
    };
    let error = PdfDocument::parse_with_limits(&pdf, limits).unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(
        error.to_string().contains("reading the PDF header"),
        "{error}"
    );
}

#[test]
fn a_flate_stream_bomb_is_refused_when_it_is_decoded() {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&vec![b'A'; 4 * 1024 * 1024]).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut pdf = String::from("%PDF-1.4\n");
    pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    pdf.push_str("3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>\nendobj\n");
    pdf.push_str(&format!(
        "4 0 obj\n<< /Length {} /Filter /FlateDecode >>\nstream\n",
        compressed.len()
    ));
    let mut bytes = pdf.into_bytes();
    bytes.extend_from_slice(&compressed);
    bytes.extend_from_slice(b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");

    let limits = Limits {
        max_decoded_stream_bytes: 1024,
        ..Limits::default()
    };
    let doc = PdfDocument::parse_with_limits(&bytes, limits).expect("parse");
    let error = doc.extract_checked().unwrap_err();
    assert!(error.to_string().contains("decoded PDF stream"), "{error}");
}

/// A page whose content is two streams of 2,000 bytes each.
fn two_content_streams() -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&vec![b'A'; 2_000]).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut pdf = String::from("%PDF-1.4\n");
    pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    pdf.push_str(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] \
         /Contents [4 0 R 5 0 R] >>\nendobj\n",
    );
    let mut bytes = pdf.into_bytes();
    for number in [4, 5] {
        bytes.extend_from_slice(
            format!(
                "{number} 0 obj\n<< /Length {} /Filter /FlateDecode >>\nstream\n",
                compressed.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&compressed);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
    }
    bytes.extend_from_slice(b"trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    bytes
}

/// Each stream is inside the per-stream ceiling; their sum is what the
/// whole-document total refuses, so many streams cannot stack into terabytes.
#[test]
fn streams_inside_their_own_ceiling_are_refused_by_the_document_total() {
    let pdf = two_content_streams();
    let limits = Limits {
        max_document_bytes: 3_000,
        ..Limits::default()
    };
    let doc = PdfDocument::parse_with_limits(&pdf, limits).expect("parse");
    let error = doc.extract_checked().unwrap_err();
    let refusal = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<LimitError>())
        .unwrap_or_else(|| panic!("not a limit refusal: {error:#}"));
    assert!(
        matches!(refusal, LimitError::DocumentTooLarge { .. }),
        "unexpected refusal: {refusal}"
    );
}

/// A PDF with one page and one object that holds `depth` nested arrays.
fn nested_arrays(depth: usize) -> Vec<u8> {
    let mut pdf = String::from("%PDF-1.4\n");
    pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    pdf.push_str("3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>\nendobj\n");
    pdf.push_str("4 0 obj\n");
    pdf.push_str(&"[".repeat(depth));
    pdf.push('0');
    pdf.push_str(&"]".repeat(depth));
    pdf.push_str("\nendobj\n");
    pdf.push_str("trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    pdf.into_bytes()
}

/// The nesting ceiling is a boundary, not a shape only the fuzzer finds: a
/// document nested to the limit reads, one level deeper is refused by name.
///
/// The fuzz run met this at exactly 65 levels, so the edge had no deterministic
/// test. The depth is held at the tokens by `check_nesting`: `parse_value` drops
/// a value past the ceiling instead of failing, so without that check the
/// 65-deep document would be read as a shorter one rather than refused.
#[test]
fn a_pdf_at_the_nesting_limit_reads_and_one_level_deeper_is_refused() {
    let limits = Limits::default();
    assert_eq!(
        limits.max_pdf_nesting, 64,
        "the default nesting ceiling moved; this test pins the boundary"
    );

    let at_limit = nested_arrays(limits.max_pdf_nesting);
    let document = PdfDocument::parse_with_limits(&at_limit, limits.clone())
        .expect("a document nested to the limit must read");
    document.extract_checked().expect("the page reads");

    let over = nested_arrays(limits.max_pdf_nesting + 1);
    let error = match PdfDocument::parse_with_limits(&over, limits) {
        Ok(_) => panic!("a document one level past the limit must be refused"),
        Err(error) => error,
    };
    let refusal = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<LimitError>())
        .unwrap_or_else(|| panic!("not a limit refusal: {error:#}"));
    assert!(
        matches!(
            refusal,
            LimitError::PdfTooDeep {
                actual: 65,
                limit: 64
            }
        ),
        "unexpected refusal: {refusal}"
    );
    assert!(
        refusal.to_string().contains("nests 65 levels deep"),
        "the refusal does not name the depth: {refusal}"
    );
}

/// A one-page PDF whose content stream is `len` bytes of `A`.
fn one_stream_pdf(len: usize) -> Vec<u8> {
    let mut pdf = String::from("%PDF-1.4\n");
    pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    pdf.push_str(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] \
         /Contents 4 0 R >>\nendobj\n",
    );
    pdf.push_str(&format!("4 0 obj\n<< /Length {len} >>\nstream\n"));
    let mut bytes = pdf.into_bytes();
    bytes.extend(std::iter::repeat_n(b'A', len));
    bytes.extend_from_slice(b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    bytes
}

/// The payload is copied out of the file into the document's own buffer — the
/// ciphertext of an encrypted file included — so the copy is charged before it
/// is made, not after the reader decodes it.
#[test]
fn a_stream_payload_is_charged_when_the_parser_copies_it_out_of_the_file() {
    let pdf = one_stream_pdf(4_096);
    let limits = Limits {
        max_document_bytes: 1_024,
        ..Limits::default()
    };
    let error = PdfDocument::parse_with_limits(&pdf, limits).unwrap_err();
    let refusal = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<LimitError>())
        .unwrap_or_else(|| panic!("not a limit refusal: {error:#}"));
    assert!(
        matches!(refusal, LimitError::DocumentTooLarge { limit: 1_024, .. }),
        "unexpected refusal: {refusal}"
    );

    let limits = Limits {
        max_document_bytes: 8_192,
        ..Limits::default()
    };
    PdfDocument::parse_with_limits(&pdf, limits).expect("a ceiling that holds the payload reads");
}

/// A PDF whose object stream holds one object: `body` follows the `objnum
/// offset` header, so its payload is `body` plus those four bytes.
fn object_stream_pdf(body: &[u8]) -> Vec<u8> {
    let mut header = b"9 0 ".to_vec();
    let first = header.len();
    header.extend_from_slice(body);
    let mut pdf = String::from("%PDF-1.5\n");
    pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    pdf.push_str(&format!(
        "3 0 obj\n<< /Type /ObjStm /N 1 /First {first} /Length {} >>\nstream\n",
        header.len()
    ));
    let mut bytes = pdf.into_bytes();
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    bytes
}

/// Expanding an object stream decodes its payload into a buffer of its own, and
/// that buffer is charged before it is parsed. A ceiling that holds the payload
/// copied out of the file but not that decoded copy refuses the file.
#[test]
fn expanding_an_object_stream_is_charged_for_its_decoded_copy() {
    let body = format!("[{}]", "1 ".repeat(1_024));
    let pdf = object_stream_pdf(body.as_bytes());
    let payload = body.len() as u64 + 4;

    let limits = Limits {
        max_document_bytes: payload + 512,
        ..Limits::default()
    };
    let error = PdfDocument::parse_with_limits(&pdf, limits).unwrap_err();
    let refusal = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<LimitError>())
        .unwrap_or_else(|| panic!("not a limit refusal: {error:#}"));
    assert!(
        matches!(refusal, LimitError::DocumentTooLarge { .. }),
        "unexpected refusal: {refusal}"
    );

    let limits = Limits {
        max_document_bytes: 8 * payload,
        ..Limits::default()
    };
    PdfDocument::parse_with_limits(&pdf, limits)
        .expect("a ceiling that holds the payload and its decoded copy reads");
}
