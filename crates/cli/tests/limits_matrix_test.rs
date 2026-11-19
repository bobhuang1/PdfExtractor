//! The refusal matrix: every budget, lowered to 0 or 1, against every reader.
//!
//! A budget that a reader forgets to consult does not fail loudly — the input is
//! simply read, which is the failure mode this table exists to catch. Each row
//! pairs one reader with one budget it is supposed to check, and is run twice:
//! the same bytes under the product defaults must be read, and under the lowered
//! budget must come back as a [`LimitError`]. Reading them twice is what makes a
//! row mean something: a fixture the reader never accepted would otherwise
//! "refuse" for a reason the budget had nothing to do with.
//!
//! A panic is a failure too — the readers return their refusals — so the lowered
//! run is caught and reported as a panic rather than as a refusal.

use std::io::{Cursor, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Duration;

use pdfextractor_formats::Format;
use pdfextractor_limits::{LimitError, Limits};

/// A reader's bounded entry point, as the matrix calls it.
type Reader<'a> = dyn Fn(&Limits) -> anyhow::Result<()> + 'a;

/// One reader, one budget, one input.
struct Case<'a> {
    reader: &'static str,
    budget: &'static str,
    limits: Limits,
    run: Box<Reader<'a>>,
}

fn case<'a>(
    reader: &'static str,
    budget: &'static str,
    limits: Limits,
    run: impl Fn(&Limits) -> anyhow::Result<()> + 'a,
) -> Case<'a> {
    Case {
        reader,
        budget,
        limits,
        run: Box::new(run),
    }
}

/// A committed fixture under `test-data/`.
fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("missing {}: {error}", path.display()))
}

/// A three-page PDF with no content streams: enough objects, tokens and nesting
/// for every parser-side ceiling, and a page tree the extraction budget sees.
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

/// A page tree two levels deep: the outer `/Pages` holds another `/Pages`, which
/// holds the single page. Only the nesting budget sees the extra level.
fn nested_page_tree() -> Vec<u8> {
    let mut pdf = String::from("%PDF-1.4\n");
    pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    pdf.push_str("3 0 obj\n<< /Type /Pages /Parent 2 0 R /Kids [4 0 R] /Count 1 >>\nendobj\n");
    pdf.push_str("4 0 obj\n<< /Type /Page /Parent 3 0 R /MediaBox [0 0 100 100] >>\nendobj\n");
    pdf.push_str("trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    pdf.into_bytes()
}

/// A PNG signature and IHDR: the header reader's facts, with no pixel data.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&[8, 6, 0, 0, 0]);
    out.extend_from_slice(&[0, 0, 0, 0]); // CRC: the header reader does not verify it
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&[0, 0, 0, 0]);
    out
}

/// A ZIP package of stored parts, so its entry sizes are what is written and its
/// expansion ratio is one: the defaults must read it, so a refusal is the
/// lowered budget's doing.
fn package(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in files {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer
            .start_file(*name, options)
            .expect("start a ZIP entry");
        writer.write_all(bytes).expect("write a ZIP entry");
    }
    writer.finish().expect("finish the ZIP").into_inner()
}

/// `<a>` nested `depth` deep, and closed again.
fn nested_xml(depth: usize) -> Vec<u8> {
    let mut xml = String::new();
    for _ in 0..depth {
        xml.push_str("<a>");
    }
    for _ in 0..depth {
        xml.push_str("</a>");
    }
    xml.into_bytes()
}

/// The panic message, for a reader that panicked instead of refusing.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a non-string panic payload".to_string()
    }
}

/// Hold one row to both halves: readable by default, a typed refusal when lowered.
fn check(case: &Case<'_>) {
    let label = format!("{} / {}", case.reader, case.budget);
    let defaults = (case.run)(&Limits::default());
    assert!(
        defaults.is_ok(),
        "{label}: the fixture is not readable under the product defaults, so a \
         refusal would say nothing about the budget: {}",
        defaults
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default()
    );

    let outcome = panic::catch_unwind(AssertUnwindSafe(|| (case.run)(&case.limits)));
    let result = match outcome {
        Ok(result) => result,
        Err(payload) => panic!(
            "{label}: the reader panicked instead of refusing: {}",
            panic_message(payload.as_ref())
        ),
    };
    let error = match result {
        Err(error) => error,
        Ok(()) => panic!("{label}: the lowered budget was silently accepted"),
    };

    let refusal = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<LimitError>());
    assert!(
        refusal.is_some(),
        "{label}: the failure is not a LimitError: {error:#}"
    );
}

type AnyResult = anyhow::Result<()>;

#[test]
fn every_lowered_budget_is_a_typed_refusal() {
    let pages = three_pages();
    let nested = nested_page_tree();
    let compressed = fixture("compressed.pdf");
    let docx = fixture("brief.docx");
    let xlsx = fixture("inventory.xlsx");
    let pptx = fixture("q3_review.pptx");
    let odt = fixture("brief_odt.odt");
    let ods = fixture("inventory_calc.ods");
    let odp = fixture("q3_review_impress.odp");
    let word = fixture("legacy_brief.doc");
    let tiff = fixture("images/pages.tiff");
    let image = png(2, 2);
    let bomb = package(&[("word/document.xml", &vec![b' '; 2 * 1024 * 1024])]);
    let boxed = package(&[("a.xml", b"<a/>"), ("b.xml", b"<b/>"), ("c.xml", b"<c/>")]);

    // --- PDF: the parser's own ceilings, and the deadline -------------------
    let pdf = |limits: &Limits| -> AnyResult {
        let document =
            pdfextractor_pdf_parser::PdfDocument::parse_with_limits(&pages, limits.clone())?;
        document.extract_checked()?;
        Ok(())
    };
    check(&case(
        "pdf",
        "input bytes 0",
        Limits {
            max_input_bytes: 0,
            ..Limits::default()
        },
        pdf,
    ));
    check(&case(
        "pdf",
        "tokens 1",
        Limits {
            max_pdf_tokens: 1,
            ..Limits::default()
        },
        pdf,
    ));
    check(&case(
        "pdf",
        "objects 1",
        Limits {
            max_pdf_objects: 1,
            ..Limits::default()
        },
        pdf,
    ));
    check(&case(
        "pdf",
        "nesting 1",
        Limits {
            max_pdf_nesting: 1,
            ..Limits::default()
        },
        |limits: &Limits| -> AnyResult {
            let document =
                pdfextractor_pdf_parser::PdfDocument::parse_with_limits(&nested, limits.clone())?;
            document.extract_checked()?;
            Ok(())
        },
    ));
    check(&case(
        "pdf",
        "pages 1",
        Limits {
            max_pages: 1,
            ..Limits::default()
        },
        pdf,
    ));
    check(&case(
        "pdf",
        "timeout 0",
        Limits {
            timeout: Duration::ZERO,
            ..Limits::default()
        },
        pdf,
    ));
    check(&case(
        "pdf",
        "decoded stream 1",
        Limits {
            max_decoded_stream_bytes: 1,
            ..Limits::default()
        },
        |limits| {
            let document = pdfextractor_pdf_parser::PdfDocument::parse_with_limits(
                &compressed,
                limits.clone(),
            )?;
            document.extract_checked()?;
            Ok(())
        },
    ));
    check(&case(
        "pdf",
        "document bytes 1",
        Limits {
            max_document_bytes: 1,
            ..Limits::default()
        },
        |limits| {
            let document = pdfextractor_pdf_parser::PdfDocument::parse_with_limits(
                &compressed,
                limits.clone(),
            )?;
            document.extract_checked()?;
            Ok(())
        },
    ));

    // --- DOCX: package, XML, blocks and the clock ---------------------------
    let docx_run = |limits: &Limits| -> AnyResult {
        pdfextractor_docx_parser::DocxDocument::parse_bytes_with_limits(&docx, limits.clone())?;
        Ok(())
    };
    check(&case(
        "docx",
        "input bytes 0",
        Limits {
            max_input_bytes: 0,
            ..Limits::default()
        },
        docx_run,
    ));
    check(&case(
        "docx",
        "zip entries 1",
        Limits {
            max_zip_entries: 1,
            ..Limits::default()
        },
        docx_run,
    ));
    check(&case(
        "docx",
        "zip entry bytes 1",
        Limits {
            max_zip_entry_bytes: 1,
            ..Limits::default()
        },
        docx_run,
    ));
    check(&case(
        "docx",
        "zip total bytes 1",
        Limits {
            max_zip_total_bytes: 1,
            ..Limits::default()
        },
        docx_run,
    ));
    check(&case(
        "docx",
        "xml bytes 1",
        Limits {
            max_xml_bytes: 1,
            ..Limits::default()
        },
        docx_run,
    ));
    check(&case(
        "docx",
        "xml depth 1",
        Limits {
            max_xml_depth: 1,
            ..Limits::default()
        },
        docx_run,
    ));
    check(&case(
        "docx",
        "flow blocks 0",
        Limits {
            max_flow_blocks: 0,
            ..Limits::default()
        },
        docx_run,
    ));
    check(&case(
        "docx",
        "timeout 0",
        Limits {
            timeout: Duration::ZERO,
            ..Limits::default()
        },
        docx_run,
    ));

    // --- The other packages: package and XML gates, and the clock -----------
    macro_rules! package_rows {
        ($name:literal, $reader:path, $data:expr) => {{
            let run = |limits: &Limits| -> AnyResult {
                $reader(&$data, limits.clone())?;
                Ok(())
            };
            check(&case(
                $name,
                "input bytes 0",
                Limits {
                    max_input_bytes: 0,
                    ..Limits::default()
                },
                run,
            ));
            check(&case(
                $name,
                "zip entries 1",
                Limits {
                    max_zip_entries: 1,
                    ..Limits::default()
                },
                run,
            ));
            check(&case(
                $name,
                "xml bytes 1",
                Limits {
                    max_xml_bytes: 1,
                    ..Limits::default()
                },
                run,
            ));
            check(&case(
                $name,
                "xml depth 1",
                Limits {
                    max_xml_depth: 1,
                    ..Limits::default()
                },
                run,
            ));
            check(&case(
                $name,
                "timeout 0",
                Limits {
                    timeout: Duration::ZERO,
                    ..Limits::default()
                },
                run,
            ));
        }};
    }
    package_rows!(
        "xlsx",
        pdfextractor_xlsx_parser::XlsxDocument::parse_bytes_with_limits,
        xlsx
    );
    package_rows!(
        "pptx",
        pdfextractor_pptx_parser::PptxDocument::parse_bytes_with_limits,
        pptx
    );
    package_rows!(
        "odt",
        pdfextractor_odt_parser::OdtDocument::parse_bytes_with_limits,
        odt
    );
    package_rows!(
        "ods",
        pdfextractor_ods_parser::OdsDocument::parse_bytes_with_limits,
        ods
    );
    package_rows!(
        "odp",
        pdfextractor_odp_parser::OdpDocument::parse_bytes_with_limits,
        odp
    );

    // --- Legacy `.doc`: its container, its blocks and the clock -------------
    let word_run = |limits: &Limits| -> AnyResult {
        pdfextractor_doc_parser::DocDocument::parse_bytes_with_limits(&word, limits.clone())?;
        Ok(())
    };
    check(&case(
        "doc",
        "input bytes 0",
        Limits {
            max_input_bytes: 0,
            ..Limits::default()
        },
        word_run,
    ));
    check(&case(
        "doc",
        "flow blocks 0",
        Limits {
            max_flow_blocks: 0,
            ..Limits::default()
        },
        word_run,
    ));
    check(&case(
        "doc",
        "timeout 0",
        Limits {
            timeout: Duration::ZERO,
            ..Limits::default()
        },
        word_run,
    ));

    // --- Images: the header budgets and the frame stack ---------------------
    let image_run = |limits: &Limits| -> AnyResult {
        pdfextractor_image_parser::ImageDocument::parse_bytes_with_limits(&image, limits)?;
        Ok(())
    };
    check(&case(
        "image",
        "input bytes 0",
        Limits {
            max_input_bytes: 0,
            ..Limits::default()
        },
        image_run,
    ));
    check(&case(
        "image",
        "pixels 1",
        Limits {
            max_image_pixels: 1,
            ..Limits::default()
        },
        image_run,
    ));
    check(&case(
        "image",
        "dimension 1",
        Limits {
            max_image_dimension: 1,
            ..Limits::default()
        },
        image_run,
    ));
    let frames_run = |limits: &Limits| -> AnyResult {
        pdfextractor_image_parser::tiff_frames_checked(&tiff, limits)?;
        Ok(())
    };
    check(&case(
        "image frames",
        "frames 1",
        Limits {
            max_image_frames: 1,
            ..Limits::default()
        },
        frames_run,
    ));
    check(&case(
        "image frames",
        "document bytes 1",
        Limits {
            max_document_bytes: 1,
            ..Limits::default()
        },
        frames_run,
    ));
    check(&case(
        "image frames",
        "timeout 0",
        Limits {
            timeout: Duration::ZERO,
            ..Limits::default()
        },
        frames_run,
    ));

    // --- Flow readers: the input and block ceilings, and the clock ----------
    let markdown = b"# one\n\ntwo\n";
    let flow = |limits: &Limits| -> AnyResult {
        pdfextractor_text_parser::text::parse_markdown_with_limits(markdown, limits)?;
        Ok(())
    };
    check(&case(
        "markdown",
        "input bytes 0",
        Limits {
            max_input_bytes: 0,
            ..Limits::default()
        },
        flow,
    ));
    check(&case(
        "markdown",
        "flow blocks 0",
        Limits {
            max_flow_blocks: 0,
            ..Limits::default()
        },
        flow,
    ));
    check(&case(
        "markdown",
        "timeout 0",
        Limits {
            timeout: Duration::ZERO,
            ..Limits::default()
        },
        flow,
    ));

    let plain = |limits: &Limits| -> AnyResult {
        pdfextractor_text_parser::text::parse_plain_with_limits(b"one\ntwo\n", limits)?;
        Ok(())
    };
    check(&case(
        "text",
        "flow blocks 0",
        Limits {
            max_flow_blocks: 0,
            ..Limits::default()
        },
        plain,
    ));

    let html = |limits: &Limits| -> AnyResult {
        pdfextractor_text_parser::html::parse_with_limits(b"<p>one</p>", limits)?;
        Ok(())
    };
    check(&case(
        "html",
        "input bytes 0",
        Limits {
            max_input_bytes: 0,
            ..Limits::default()
        },
        html,
    ));
    check(&case(
        "html",
        "flow blocks 0",
        Limits {
            max_flow_blocks: 0,
            ..Limits::default()
        },
        html,
    ));

    let rtf = |limits: &Limits| -> AnyResult {
        pdfextractor_text_parser::rtf::parse_with_limits(b"{\\rtf1 one}", limits)?;
        Ok(())
    };
    check(&case(
        "rtf",
        "flow blocks 0",
        Limits {
            max_flow_blocks: 0,
            ..Limits::default()
        },
        rtf,
    ));

    let csv = |limits: &Limits| -> AnyResult {
        pdfextractor_text_parser::csv::parse_with_limits(b"a,b\n1,2\n", b',', limits)?;
        Ok(())
    };
    check(&case(
        "csv",
        "flow blocks 0",
        Limits {
            max_flow_blocks: 0,
            ..Limits::default()
        },
        csv,
    ));

    // --- The two gates every package passes through -------------------------
    let zip_gate = |limits: &Limits| -> AnyResult {
        let mut archive = zip::ZipArchive::new(Cursor::new(&bomb)).expect("a ZIP");
        limits.check_zip(&mut archive)?;
        Ok(())
    };
    check(&case(
        "zip gate",
        "entry bytes 1",
        Limits {
            max_zip_entry_bytes: 1,
            ..Limits::default()
        },
        zip_gate,
    ));
    check(&case(
        "zip gate",
        "ratio 0",
        Limits {
            max_zip_ratio: 0,
            ..Limits::default()
        },
        zip_gate,
    ));
    let entries = |limits: &Limits| -> AnyResult {
        let mut archive = zip::ZipArchive::new(Cursor::new(&boxed)).expect("a ZIP");
        limits.check_zip(&mut archive)?;
        Ok(())
    };
    check(&case(
        "zip gate",
        "entries 1",
        Limits {
            max_zip_entries: 1,
            ..Limits::default()
        },
        entries,
    ));
    check(&case(
        "zip gate",
        "total bytes 1",
        Limits {
            max_zip_total_bytes: 1,
            ..Limits::default()
        },
        entries,
    ));

    let xml = nested_xml(4);
    let xml_gate = |limits: &Limits| -> AnyResult {
        limits.check_xml(&xml)?;
        Ok(())
    };
    check(&case(
        "xml gate",
        "bytes 1",
        Limits {
            max_xml_bytes: 1,
            ..Limits::default()
        },
        xml_gate,
    ));
    check(&case(
        "xml gate",
        "depth 1",
        Limits {
            max_xml_depth: 1,
            ..Limits::default()
        },
        xml_gate,
    ));
}

/// One fixture read under the product defaults, through the entry point the CLI
/// uses for its format.
fn parse_fixture(format: Format, data: &[u8]) -> anyhow::Result<()> {
    let limits = Limits::default();
    match format {
        Format::Pdf => {
            pdfextractor_pdf_parser::PdfDocument::parse_with_limits(data, limits)?;
        }
        Format::Docx => {
            pdfextractor_docx_parser::DocxDocument::parse_bytes_with_limits(data, limits)?;
        }
        Format::Xlsx => {
            pdfextractor_xlsx_parser::XlsxDocument::parse_bytes_with_limits(data, limits)?;
        }
        Format::Pptx => {
            pdfextractor_pptx_parser::PptxDocument::parse_bytes_with_limits(data, limits)?;
        }
        Format::Odt => {
            pdfextractor_odt_parser::OdtDocument::parse_bytes_with_limits(data, limits)?;
        }
        Format::Ods => {
            pdfextractor_ods_parser::OdsDocument::parse_bytes_with_limits(data, limits)?;
        }
        Format::Odp => {
            pdfextractor_odp_parser::OdpDocument::parse_bytes_with_limits(data, limits)?;
        }
        Format::Doc => {
            pdfextractor_doc_parser::DocDocument::parse_bytes_with_limits(data, limits)?;
        }
        Format::Image { .. } => {
            pdfextractor_image_parser::ImageDocument::parse_bytes_with_limits(data, &limits)?;
        }
        Format::Flow(kind) => match kind {
            "rtf" => {
                pdfextractor_text_parser::rtf::parse_with_limits(data, &limits)?;
            }
            "html" | "htm" => {
                pdfextractor_text_parser::html::parse_with_limits(data, &limits)?;
            }
            "md" => {
                pdfextractor_text_parser::text::parse_markdown_with_limits(data, &limits)?;
            }
            "csv" => {
                pdfextractor_text_parser::csv::parse_csv_with_limits(data, &limits)?;
            }
            "tsv" => {
                pdfextractor_text_parser::csv::parse_tsv_with_limits(data, &limits)?;
            }
            _ => {
                pdfextractor_text_parser::text::parse_plain_with_limits(data, &limits)?;
            }
        },
        Format::Bridge { .. } | Format::Unsupported(_) => {}
    }
    Ok(())
}

/// Every committed fixture under `test-data/`: the documents at the top of the
/// directory and the corpora beside them, without the goldens (expected
/// outputs, not inputs), the generated working directories and the generators
/// beside the fixtures, which no reader is meant to read.
fn fixtures() -> Vec<PathBuf> {
    fn is_document(path: &Path) -> bool {
        !matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("py" | "json" | "yaml" | "yml" | "sh" | "toml")
        )
    }

    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in
            std::fs::read_dir(dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                let name = path.file_name().and_then(|name| name.to_str());
                if matches!(
                    name,
                    Some("golden" | "__pycache__" | "worksheet_pictures_calc_images")
                ) {
                    continue;
                }
                walk(&path, out);
            } else if is_document(&path) {
                out.push(path);
            }
        }
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
    let mut out = Vec::new();
    walk(&root, &mut out);
    out.sort();
    out
}

/// The other half of the matrix: the product defaults refuse nothing the
/// repository ships.
///
/// The matrix proves a lowered budget is consulted; this proves the default is
/// not below real documents, so a refusal a user meets is about their file. The
/// sweep parses — extraction budgets are covered by the matrix and by the corpus
/// and golden tests, which extract these fixtures at the defaults.
#[test]
fn every_committed_fixture_parses_under_the_default_budgets() {
    let mut parsed = 0;
    let mut skipped = 0;
    let mut unreadable = Vec::new();
    for path in fixtures() {
        let data =
            std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let format = pdfextractor_formats::detect_bytes(&path, &data);
        if matches!(
            format,
            pdfextractor_formats::Format::Bridge { .. }
                | pdfextractor_formats::Format::Unsupported(_)
        ) {
            skipped += 1;
            continue;
        }
        if let Err(error) = parse_fixture(format, &data) {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            if let Some(refusal) = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<LimitError>())
            {
                panic!(
                    "{}: the default budgets refuse a committed fixture ({refusal}), \
                     so a default is below a document the repository ships: {error:#}",
                    path.display()
                );
            }
            unreadable.push(format!("{name}: {error:#}"));
            continue;
        }
        parsed += 1;
    }

    eprintln!(
        "fixture sweep: {parsed} parsed, {} unreadable ({unreadable:?}), {skipped} skipped",
        unreadable.len()
    );
    assert!(
        parsed > 0,
        "no fixture was found; the sweep is not reading test-data"
    );
    assert!(
        unreadable.is_empty(),
        "these fixtures do not parse at the defaults: {unreadable:#?}"
    );
}

/// The refusal contract, end to end: a budget refusal exits 3, states the budget
/// that ran out, names the flag that raises it, and reports the same flag and
/// kind in the progress JSON a caller reads.
#[test]
fn a_budget_refusal_names_its_budget_and_flag_end_to_end() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/brief.docx");
    let dir = std::env::temp_dir().join(format!("pdfextractor-refusal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the output directory");
    let output_path = dir.join("brief.md");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            fixture.to_str().expect("a UTF-8 fixture path"),
            "-o",
            output_path.to_str().expect("a UTF-8 output path"),
            "--max-flow-blocks",
            "1",
            "--ocr",
            "never",
            "--progress",
        ])
        .output()
        .expect("spawn the CLI binary");
    let _ = std::fs::remove_dir_all(&dir);

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(3), "stderr={stderr}");
    assert!(
        stderr.contains("the limit is 1"),
        "the refusal does not state the budget: {stderr}"
    );
    assert!(
        stderr.contains("(raise --max-flow-blocks to allow it)"),
        "the refusal does not name the flag that raises it: {stderr}"
    );
    assert!(
        stdout.contains("\"limit_flag\":\"--max-flow-blocks\""),
        "the progress report does not name the flag: {stdout}"
    );
    assert!(
        stdout.contains("\"error_kind\":\"limit\""),
        "the progress report does not say the failure was a refusal: {stdout}"
    );
}
