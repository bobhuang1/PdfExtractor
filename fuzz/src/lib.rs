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
    if let Some(value) = number("FUZZ_TIMEOUT_MS") {
        limits.timeout = Duration::from_millis(value);
    }
    if let Some(value) = number("FUZZ_OCR_TIMEOUT_MS") {
        limits.ocr_timeout = Duration::from_millis(value);
    }
    limits
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
