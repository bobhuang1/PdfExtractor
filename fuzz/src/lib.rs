//! Budget overrides and refusal reporting for the fuzz targets.
//!
//! The targets take their budgets from the environment, so a local run can be
//! lowered into the refusal paths without a rebuild. An unset variable keeps the
//! product default, so an ordinary `cargo fuzz run` still fuzzes what ships.

use std::time::Duration;

use pdfextractor_limits::Limits;

/// A number from `name`, when the run set one.
fn number(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

/// The budgets this process fuzzes under.
///
/// Every `FUZZ_*` variable is optional: the unset ones keep their product
/// default, and an unparsable value is ignored rather than made fatal, so a
/// typo in a local run cannot silently widen a budget.
pub fn limits() -> Limits {
    let mut limits = Limits::default();
    if let Some(value) = number("FUZZ_MAX_INPUT_BYTES") {
        limits.max_input_bytes = value;
    }
    if let Some(value) = number("FUZZ_MAX_PAGES") {
        limits.max_pages = value as usize;
    }
    if let Some(value) = number("FUZZ_MAX_PDF_OBJECTS") {
        limits.max_pdf_objects = value as usize;
    }
    if let Some(value) = number("FUZZ_MAX_DECODED_STREAM_BYTES") {
        limits.max_decoded_stream_bytes = value;
    }
    if let Some(value) = number("FUZZ_MAX_ZIP_ENTRIES") {
        limits.max_zip_entries = value as usize;
    }
    if let Some(value) = number("FUZZ_MAX_ZIP_ENTRY_BYTES") {
        limits.max_zip_entry_bytes = value;
    }
    if let Some(value) = number("FUZZ_MAX_XML_BYTES") {
        limits.max_xml_bytes = value;
    }
    if let Some(value) = number("FUZZ_MAX_XML_DEPTH") {
        limits.max_xml_depth = value as usize;
    }
    if let Some(value) = number("FUZZ_MAX_IMAGE_PIXELS") {
        limits.max_image_pixels = value;
    }
    if let Some(value) = number("FUZZ_MAX_IMAGE_FRAMES") {
        limits.max_image_frames = value as usize;
    }
    if let Some(value) = number("FUZZ_MAX_FLOW_BLOCKS") {
        limits.max_flow_blocks = value as usize;
    }
    if let Some(value) = number("FUZZ_MAX_DOCUMENT_BYTES") {
        limits.max_document_bytes = value;
    }
    if let Some(value) = number("FUZZ_TIMEOUT_MS") {
        limits.timeout = Duration::from_millis(value);
    }
    if let Some(value) = number("FUZZ_OCR_TIMEOUT_MS") {
        limits.ocr_timeout = Duration::from_millis(value);
    }
    limits
}

/// The input bytes the derived budgets are read from.
pub const BUDGET_HEADER_BYTES: usize = 16;

/// Budgets read from the input's own first bytes, and the document after them.
///
/// The `FUZZ_*` recipe above is fixed: it reaches the refusals someone already
/// thought of. Here each budget is a byte of the header scaled into a range that
/// sits *under* real documents — kilobytes rather than gigabytes, tens of
/// objects, a nesting in single digits — so the fuzzer walks every ceiling's
/// refusal boundary at once, including the zero and the ceiling itself.
///
/// An input shorter than the header keeps the product defaults and is read
/// whole: there is no room for a header, so there are no budgets to derive.
pub fn limits_from(data: &[u8]) -> (Limits, &[u8]) {
    let mut limits = Limits::default();
    if data.len() < BUDGET_HEADER_BYTES {
        return (limits, data);
    }
    let (header, payload) = data.split_at(BUDGET_HEADER_BYTES);
    // 0..=255 onto 0..=ceiling, so a one-bit mutation moves a budget by one
    // step and the lowest and highest values are both reachable.
    let scale = |index: usize, ceiling: u64| u64::from(header[index]) * ceiling / 255;
    let count =
        |index: usize, ceiling: u64| usize::try_from(scale(index, ceiling)).unwrap_or(usize::MAX);
    let millis = |index: usize| Duration::from_millis(scale(index, 100));

    limits.max_input_bytes = scale(0, 64 << 10);
    limits.max_pages = count(1, 8);
    limits.max_pdf_objects = count(2, 64);
    limits.max_pdf_tokens = count(3, 4 << 10);
    limits.max_pdf_nesting = count(4, 8);
    limits.max_decoded_stream_bytes = scale(5, 1 << 20);
    limits.max_zip_entries = count(6, 16);
    limits.max_zip_entry_bytes = scale(7, 1 << 20);
    limits.max_zip_total_bytes = scale(8, 2 << 20);
    limits.max_zip_ratio = scale(9, 8);
    limits.max_xml_bytes = scale(10, 64 << 10);
    limits.max_xml_depth = count(11, 16);
    limits.max_image_pixels = scale(12, 1 << 16);
    limits.max_image_dimension = u32::try_from(count(13, 512)).unwrap_or(u32::MAX);
    limits.max_image_frames = count(14, 16);
    limits.max_image_alloc_bytes = scale(15, 1 << 20);

    // The totals and the three clocks share bytes the header already spent on a
    // size: sixteen bytes cannot give each of twenty-two budgets its own, and a
    // total is fuzzed as well by the byte that already moves a size.
    limits.max_flow_blocks = count(0, 1 << 10);
    limits.max_document_bytes = scale(5, 4 << 20);
    limits.max_bridge_output_bytes = scale(7, 4 << 20);
    limits.bridge_timeout = millis(9);
    limits.timeout = millis(10);
    limits.ocr_timeout = millis(11);

    (limits, payload)
}

/// Report a refusal on stderr when `FUZZ_REPORT_REFUSALS` is set.
///
/// A budget refusal is not a finding: the readers return it instead of reading.
/// The corpus pass sets this variable to list which seeds were refused and why;
/// a timed run leaves it unset, because a message per input would drown the
/// fuzzer's own output.
pub fn note<T, E: std::fmt::Display>(what: &str, result: &Result<T, E>) {
    if std::env::var_os("FUZZ_REPORT_REFUSALS").is_none() {
        return;
    }
    if let Err(error) = result {
        eprintln!("{what}: {error}");
    }
}
