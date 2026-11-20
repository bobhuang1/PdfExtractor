#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfextractor_fuzz::{limits, note};

fuzz_target!(|data: &[u8]| {
    // The parser is the first thing a hostile PDF reaches. `parse_with_limits`
    // and `extract_checked` are the paths the command line takes, so the fuzzer
    // sees the page count, decoded-stream ceiling and deadline as well as the
    // object parser. `FUZZ_*` variables lower those budgets for a local run.
    let parsed = pdfextractor_pdf_parser::PdfDocument::parse_with_limits(data, limits());
    note("pdf", &parsed);
    if let Ok(document) = parsed {
        let pages = document.extract_checked();
        note("pdf pages", &pages);
    }
});
