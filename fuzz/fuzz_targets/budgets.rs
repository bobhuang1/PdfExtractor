#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use pdfextractor_fuzz::{limits_from, note};

fuzz_target!(|data: &[u8]| {
    // The budgets come off the input itself, so every reader below is held to a
    // ceiling small enough to sit under a real document. The refusal paths are
    // fuzzed alongside the parsers, without a rebuild or a `FUZZ_*` recipe.
    let (budget, payload) = limits_from(data);

    // PDF: the object, token and nesting ceilings, the decoded-stream and
    // whole-document totals, the page count and the parser deadline.
    let parsed = pdfextractor_pdf_parser::PdfDocument::parse_with_limits(payload, budget.clone());
    note("pdf", &parsed);
    if let Ok(document) = parsed {
        note("pdf pages", &document.extract_checked());
    }

    // ZIP: the preflight every package passes before a part is parsed.
    if let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(payload)) {
        note("zip", &budget.check_zip(&mut archive));
    }

    // XML: size, nesting and DOCTYPE, walked as a stream under the deadline.
    note("xml", &budget.check_xml(payload));

    // Image: pixels, dimensions, allocation and the frame stack. The checked
    // transcode is used so a refusal is reported instead of a silent `None`.
    note(
        "image",
        &pdfextractor_image_parser::ImageDocument::parse_bytes_with_limits(payload, &budget),
    );
    note(
        "transcode",
        &pdfextractor_image_parser::transcode_to_png_checked(payload, &budget),
    );
});
