//! The budgets derived from the input are the ones the target runs under.
//!
//! `limits_from` is the whole hop between a fuzz input and a reader's budget, and
//! a bug in it looks like an ordinary run: every derived ceiling is either too
//! wide to refuse anything or so low that the first byte refuses every input.
//! These tests hold the shape of the derivation — the header split, the ranges,
//! and the two ends of the scale — without running libFuzzer.

use pdfextractor_fuzz::{limits_from, BUDGET_HEADER_BYTES};
use pdfextractor_limits::Limits;

/// A full header of `byte`, followed by a document.
fn headered(byte: u8, document: &[u8]) -> Vec<u8> {
    let mut data = vec![byte; BUDGET_HEADER_BYTES];
    data.extend_from_slice(document);
    data
}

#[test]
fn the_header_is_taken_off_the_document() {
    let data = headered(0xFF, b"%PDF-1.4\n");
    let (_, payload) = limits_from(&data);
    assert_eq!(payload, b"%PDF-1.4\n", "the header stayed in the document");
}

#[test]
fn an_input_shorter_than_the_header_keeps_the_product_defaults() {
    let data = b"<a/>";
    let (limits, payload) = limits_from(data);
    let defaults = Limits::default();
    assert_eq!(payload, data, "a short input was not read whole");
    assert_eq!(limits.max_input_bytes, defaults.max_input_bytes);
    assert_eq!(limits.max_xml_bytes, defaults.max_xml_bytes);
    assert_eq!(limits.timeout, defaults.timeout);
}

/// The widest header still gives ceilings under a real document — that is what
/// makes the target fuzz refusals rather than only the happy path.
#[test]
fn the_widest_header_sits_under_the_product_defaults() {
    let data = headered(0xFF, b"%PDF-1.4\n");
    let (limits, _) = limits_from(&data);
    let defaults = Limits::default();

    assert_eq!(limits.max_input_bytes, 64 << 10);
    assert_eq!(limits.max_pdf_objects, 64);
    assert_eq!(limits.max_pdf_nesting, 8);
    assert_eq!(limits.max_xml_depth, 16);
    assert_eq!(limits.max_image_pixels, 1 << 16);
    assert_eq!(limits.timeout, std::time::Duration::from_millis(100));

    assert!(limits.max_input_bytes < defaults.max_input_bytes);
    assert!(limits.max_pdf_objects < defaults.max_pdf_objects);
    assert!(limits.max_pdf_tokens < defaults.max_pdf_tokens);
    assert!(limits.max_decoded_stream_bytes < defaults.max_decoded_stream_bytes);
    assert!(limits.max_zip_total_bytes < defaults.max_zip_total_bytes);
    assert!(limits.max_xml_bytes < defaults.max_xml_bytes);
    assert!(limits.max_document_bytes < defaults.max_document_bytes);
    assert!(limits.max_image_alloc_bytes < defaults.max_image_alloc_bytes);
    assert!(limits.timeout < defaults.timeout);
    assert!(limits.ocr_timeout < defaults.ocr_timeout);
}

/// Both ends of the scale are reachable, and a low byte is not silently raised
/// to the product default: a zero budget is a refusal of everything, which is a
/// boundary the fuzzer should be able to stand on.
#[test]
fn a_zero_header_reaches_the_lowest_refusal() {
    let data = headered(0x00, b"%PDF-1.4\n");
    let (limits, _) = limits_from(&data);
    assert_eq!(limits.max_input_bytes, 0);
    assert_eq!(limits.max_pdf_objects, 0);
    assert_eq!(limits.max_image_pixels, 0);
    assert_eq!(limits.timeout, std::time::Duration::ZERO);
}

/// A byte between the ends moves its budget without crossing to either, so a
/// mutation explores the range instead of only its two ends.
#[test]
fn a_middle_byte_gives_a_middle_budget() {
    let data = headered(0x80, b"%PDF-1.4\n");
    let (limits, _) = limits_from(&data);
    assert!(limits.max_pdf_objects > 0 && limits.max_pdf_objects < 64);
    assert!(limits.max_input_bytes > 0 && limits.max_input_bytes < 64 << 10);
}
