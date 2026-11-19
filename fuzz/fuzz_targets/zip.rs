#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use pdfextractor_formats::{detect_bytes, Format};
use pdfextractor_fuzz::{limits, note};

fuzz_target!(|data: &[u8]| {
    // Every office reader is a ZIP reader first. `detect_bytes` chooses the one
    // the package's own parts name, then the chosen parser runs under the shared
    // ZIP and XML gates, with whatever `FUZZ_*` lowered for this run.
    let budget = limits();
    match detect_bytes(Path::new("fuzz.docx"), data) {
        Format::Docx => note(
            "docx",
            &pdfextractor_docx_parser::DocxDocument::parse_bytes_with_limits(data, budget),
        ),
        Format::Xlsx => note(
            "xlsx",
            &pdfextractor_xlsx_parser::XlsxDocument::parse_bytes_with_limits(data, budget),
        ),
        Format::Pptx => note(
            "pptx",
            &pdfextractor_pptx_parser::PptxDocument::parse_bytes_with_limits(data, budget),
        ),
        Format::Odt => note(
            "odt",
            &pdfextractor_odt_parser::OdtDocument::parse_bytes_with_limits(data, budget),
        ),
        Format::Ods => note(
            "ods",
            &pdfextractor_ods_parser::OdsDocument::parse_bytes_with_limits(data, budget),
        ),
        Format::Odp => note(
            "odp",
            &pdfextractor_odp_parser::OdpDocument::parse_bytes_with_limits(data, budget),
        ),
        _ => {
            // A ZIP that names no office format still has to survive the shared
            // preflight, because that is the gate every package passes.
            if let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(data)) {
                let checked = budget.check_zip(&mut archive);
                note("zip", &checked);
            }
        }
    }
});
