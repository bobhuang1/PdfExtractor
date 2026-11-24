#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfextractor_fuzz::{limits, note};
use pdfextractor_pdf_parser::PdfDocument;

fuzz_target!(|data: &[u8]| {
    // The parser is the first thing a hostile PDF reaches. `parse_with_limits`
    // and `extract_checked` are the paths the command line takes, so the fuzzer
    // sees the page count, decoded-stream ceiling and deadline as well as the
    // object parser. `FUZZ_*` variables lower those budgets for a local run.
    let budgets = limits();
    let parsed = PdfDocument::parse_with_limits(data, budgets.clone());
    note("pdf", &parsed);
    if let Ok(document) = parsed {
        let pages = document.extract_checked();
        note("pdf pages", &pages);
    }

    // The same input again with a password derived from it, so an encrypted
    // document is opened through the security handler rather than only refused
    // for having no password: the handler reads `/Encrypt`, derives a key from
    // this text and, when it authenticates, decrypts the streams before the
    // page walk. Mutating an input's `/O`/`/U` is what makes the derived
    // password the one that opens it; the crypt target covers real ones.
    let password = String::from_utf8_lossy(data);
    let parsed =
        PdfDocument::parse_with_limits_and_password(data, budgets, Some(password.as_ref()));
    note("pdf password", &parsed);
    if let Ok(document) = parsed {
        let pages = document.extract_checked();
        note("pdf password pages", &pages);
    }
});
