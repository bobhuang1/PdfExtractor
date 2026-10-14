mod compare;
mod preview;

use clap::{Parser, ValueEnum, ValueHint};
use pdfextractor_core::{DocumentBuilder, Origin, PictureData, Ref};
use pdfextractor_json_exporter::export_to_file;
use pdfextractor_layout::Span as LayoutSpan;
use pdfextractor_pdf_parser::PdfDocument;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tracing_subscriber::EnvFilter;

/// When to run OCR.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum OcrMode {
    /// OCR only pages that carry no extractable text layer (scanned pages).
    Auto,
    /// Always OCR page images, even when a text layer exists.
    Always,
    /// Never run OCR.
    Never,
}

impl OcrMode {
    /// The name this mode has on the command line — the same name a compare
    /// baseline records its measurement's OCR mode under, so a run measured with
    /// `--ocr never` matches a memory that says `"never"` and one that does not
    /// match is told apart as what it is (see [`compare::Session`]).
    fn as_str(self) -> &'static str {
        match self {
            OcrMode::Auto => "auto",
            OcrMode::Always => "always",
            OcrMode::Never => "never",
        }
    }
}

/// Language for OCR, e.g. `eng`.
const OCR_LANG: &str = "eng";

#[derive(Parser, Debug)]
#[command(author, version, about = "PDF and DOCX/DOC document extractor", long_about = None)]
struct Args {
    /// Input file or directory path
    #[arg(short, long, value_hint = ValueHint::AnyPath)]
    input: PathBuf,

    /// Output path (optional). If not provided, outputs in same folder as input
    #[arg(short, long, value_hint = ValueHint::AnyPath)]
    output: Option<PathBuf>,

    /// Number of parallel conversions (1-1000)
    #[arg(short, long, default_value = "1")]
    parallel: usize,

    /// Recursive directory traversal (`--recursive false` disables it)
    #[arg(
        short,
        long,
        default_value_t = true,
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set
    )]
    recursive: bool,

    /// When to run OCR on page images
    #[arg(long, value_enum, default_value_t = OcrMode::Auto)]
    ocr: OcrMode,

    /// Also write an HTML page preview with every extracted box drawn on it
    #[arg(long)]
    preview: bool,

    /// Resolution for the preview's page backdrop (needs `pdftoppm`)
    #[arg(long, default_value_t = 110)]
    preview_dpi: u32,

    /// Stream one machine-readable JSON record per finished document to stdout
    #[arg(long)]
    progress: bool,

    /// Compare every finished document with a Docling reference (a file, or a
    /// folder of them named after each document) and fail the run if any falls
    /// below the floor the memory beside that reference remembers it by
    #[arg(long, value_hint = ValueHint::AnyPath)]
    compare: Option<PathBuf>,
}

const MAX_PARALLEL: usize = 256;

fn main() -> anyhow::Result<()> {
    // Log records go to stderr, not the default stdout: stdout carries the
    // `--progress` records, where a stray line of prose would be read as a
    // malformed record by whatever is streaming the run.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    // The spec allows 1..=1000, but that many OS threads is counter-productive;
    // clamp to a sane ceiling and tell the user.
    let parallel = if args.parallel == 0 {
        tracing::warn!("--parallel must be >= 1; using 1");
        1
    } else if args.parallel > MAX_PARALLEL {
        tracing::warn!(
            "--parallel {} exceeds the supported maximum of {}; clamping",
            args.parallel,
            MAX_PARALLEL
        );
        MAX_PARALLEL
    } else {
        args.parallel
    };

    if !args.input.exists() {
        anyhow::bail!("input path does not exist: {}", args.input.display());
    }

    let targets = collect_inputs(&args.input, args.recursive)?;
    if targets.is_empty() {
        anyhow::bail!(
            "no supported documents (.pdf, .docx, .doc, .rtf, .html, .txt, .md, .csv) found under {}",
            args.input.display()
        );
    }

    // The compare mode opens before anything is converted: a reference that is
    // not there, or a memory beside it that cannot be read, is a broken
    // configuration and must fail the run before it spends the run's time — and
    // the header then says from the start what the documents will be held to.
    let comparing = match &args.compare {
        Some(reference) => {
            let session = compare::open(reference, args.ocr.as_str())?;
            eprintln!("compare: {}", session.announce());
            Some(session)
        }
        None => None,
    };

    tracing::info!(
        "{} document(s) to convert, parallelism {}",
        targets.len(),
        parallel
    );

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(parallel)
        .build()?;

    let outputs = resolve_outputs(&targets, &args);
    let total = targets.len();

    // A plan precedes the first record so a parent process can show the whole
    // queue before any of it has finished.
    if args.progress {
        println!(
            "{}",
            json!({
                "kind": "plan",
                "total": total,
                "inputs": targets.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            })
        );
        flush_stdout();
    }

    let done = AtomicUsize::new(0);
    // Reporting is serialised so the stream stays ordered. A consumer reads
    // `done` as a count of finished documents, and a bar built from it must not
    // jump backwards because two workers reported at once.
    let printer = Mutex::new(());
    let results: Vec<(PathBuf, anyhow::Result<PathBuf>)> = pool.install(|| {
        use rayon::prelude::*;
        targets
            .par_iter()
            .zip(outputs.par_iter())
            .map(|(path, output)| {
                let result = convert_one(path, output, &args);
                // Each document is reported the moment it finishes rather than
                // being collected and printed at the end: that is what makes the
                // progress stream live.
                announce(&args, &done, &printer, total, path, &result);
                (path.clone(), result)
            })
            .collect()
    });

    let failures = results.iter().filter(|(_, result)| result.is_err()).count();

    // The compare mode's verdict is printed before the run's own so that its
    // per-document lines are on the log either way: a run that failed for some
    // other reason still says what each panel held.
    let refused = match &comparing {
        Some(session) => compare::check(session, &results),
        None => Vec::new(),
    };

    if failures > 0 {
        anyhow::bail!("{failures} of {} document(s) failed", results.len());
    }
    if !refused.is_empty() {
        anyhow::bail!(
            "the compare mode refused {} document(s):\n    {}",
            refused.len(),
            refused.join("\n    ")
        );
    }
    Ok(())
}

/// Report one finished document.
///
/// With `--progress` the report is a JSON record on stdout, flushed so a parent
/// process sees it immediately. Without it the same news goes to the log —
/// `tracing` for a success — where a human reading the terminal will see it.
/// A failure is always written to stderr as well.
///
/// `printer` is held for the whole report, which is what keeps the finished
/// count increasing down the stream.
fn announce(
    args: &Args,
    done: &AtomicUsize,
    printer: &Mutex<()>,
    total: usize,
    input: &Path,
    result: &anyhow::Result<PathBuf>,
) {
    // A worker panicking must not silence the reports that follow, so a
    // poisoned lock is still used.
    let _guard = printer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // The count comes from the atomic rather than from the caller so the number
    // is right however many threads finish at once.
    let seen = done.fetch_add(1, Ordering::Relaxed) + 1;
    match result {
        Ok(output) => {
            if args.progress {
                println!(
                    "{}",
                    json!({
                        "kind": "progress",
                        "done": seen,
                        "total": total,
                        "input": input.display().to_string(),
                        "ok": true,
                        "output": output.display().to_string(),
                    })
                );
                flush_stdout();
            } else {
                tracing::info!("{} -> {}", input.display(), output.display());
            }
        }
        Err(err) => {
            if args.progress {
                println!(
                    "{}",
                    json!({
                        "kind": "progress",
                        "done": seen,
                        "total": total,
                        "input": input.display().to_string(),
                        "ok": false,
                        "error": err.to_string(),
                    })
                );
                flush_stdout();
            }
            eprintln!("error: {}: {err}", input.display());
        }
    }
}

/// `println!` already flushes on a newline; this is belt and braces for a
/// consumer that reads the stream through a pipe.
fn flush_stdout() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

fn collect_inputs(input: &Path, recursive: bool) -> anyhow::Result<Vec<PathBuf>> {
    if input.is_file() {
        return Ok(vec![input.to_path_buf()]);
    }
    let mut out = Vec::new();
    // `None` means unlimited depth (recursive); `Some(0)` means top level only.
    let max_depth = if recursive { None } else { Some(0usize) };
    walk_dir(input, max_depth, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_dir(dir: &Path, depth_left: Option<usize>, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            match depth_left {
                None => walk_dir(&path, None, out)?,
                Some(n) if n > 0 => walk_dir(&path, Some(n - 1), out)?,
                Some(_) => {}
            }
        } else if is_supported(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_supported(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("pdf")
            | Some("docx")
            | Some("doc")
            | Some("rtf")
            | Some("html")
            | Some("htm")
            | Some("txt")
            | Some("md")
            | Some("markdown")
            | Some("csv")
            | Some("tsv")
    )
}

fn convert_one(path: &Path, output: &Path, args: &Args) -> anyhow::Result<PathBuf> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    // Extracted images and previews are written beside the output.
    let output_path = output.to_path_buf();
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let doc = match ext.as_str() {
        "pdf" => convert_pdf(path, args.ocr, &output_path, args.preview, args.preview_dpi)?,
        "docx" => {
            note_flow_format(args.preview, path);
            convert_docx(path, &output_path)?
        }
        // `.doc` is the legacy OLE compound format, not an OPC package: the
        // bytes are a binary Word stream, not ZIP + XML.
        "doc" => {
            note_flow_format(args.preview, path);
            convert_doc(path, &output_path)?
        }
        "rtf" | "html" | "htm" | "txt" | "md" | "markdown" | "csv" | "tsv" => {
            note_flow_format(args.preview, path);
            convert_text(&ext, path, &output_path)?
        }
        other => anyhow::bail!("unsupported extension: {other}"),
    };

    export_to_file(&doc, &output_path)?;
    Ok(output_path)
}

/// Note once, when a preview was asked for, why a flow-based format has none:
/// its blocks carry no page rectangle to draw a box on.
fn note_flow_format(preview: bool, path: &Path) {
    if preview {
        tracing::info!(
            "{}: preview needs page geometry; this format is flow-based, skipping",
            path.display()
        );
    }
}

/// Resolve every output path up front.
///
/// In a directory run two inputs with different extensions can share a stem
/// (`report.pdf` and `report.txt`): replacing both extensions would write the
/// same file twice and silently drop one document. When that happens the full
/// input name is kept instead, so `report.txt` becomes `report.txt.json`.
fn resolve_outputs(targets: &[PathBuf], args: &Args) -> Vec<PathBuf> {
    let paths: Vec<PathBuf> = targets.iter().map(|p| output_path_for(p, args)).collect();
    let mut counts: HashMap<PathBuf, usize> = HashMap::new();
    for path in &paths {
        *counts.entry(path.clone()).or_default() += 1;
    }
    targets
        .iter()
        .zip(paths)
        .map(|(input, output)| {
            if counts.get(&output).copied().unwrap_or(0) > 1 {
                let name = input
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "document".to_string());
                output.with_file_name(format!("{name}.json"))
            } else {
                output
            }
        })
        .collect()
}

/// Where the JSON for `path` should be written.
fn output_path_for(path: &Path, args: &Args) -> PathBuf {
    match &args.output {
        Some(o) if args.input.is_dir() => {
            let rel = path
                .strip_prefix(&args.input)
                .unwrap_or(path)
                .with_extension("json");
            o.join(rel)
        }
        Some(o) => o.clone(),
        None => path.with_extension("json"),
    }
}

/// Directory holding a document's extracted images, sibling to its JSON.
fn image_dir_for(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    output_path.with_file_name(format!("{stem}_images"))
}

/// Extension matching an image MIME type.
///
/// This must never guess: writing an EMF payload under a `.png` name makes the
/// extracted file lie about its own format.
fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/jp2" => "jp2",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tif",
        "image/emf" => "emf",
        "image/wmf" => "wmf",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}

/// A picture as the page carries it: pixels when it is a raster XObject,
/// geometry only when it is vector art.
#[derive(Clone, Copy)]
enum PagePicture<'a> {
    Raster(&'a pdfextractor_pdf_parser::PlacedImage),
    Vector([f64; 4]),
}

impl PagePicture<'_> {
    fn bbox(&self) -> [f64; 4] {
        match self {
            PagePicture::Raster(placed) => placed.bbox,
            PagePicture::Vector(bbox) => *bbox,
        }
    }
}

/// Build a `DoclingDocument` from a PDF file path.
fn convert_pdf(
    path: &Path,
    ocr: OcrMode,
    output_path: &Path,
    want_preview: bool,
    preview_dpi: u32,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = std::fs::read(path)?;
    let parsed = PdfDocument::parse(&data)?;
    let pages = parsed.extract();

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document");

    let mut builder = DocumentBuilder::new(name);
    builder.set_origin(Origin {
        mimetype: Some("application/pdf".to_string()),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name.to_string()),
        uri: None,
    });

    // One engine per document: `LepTess` is not `Sync` and construction is costly.
    let mut engine = if ocr == OcrMode::Never {
        None
    } else {
        match pdfextractor_ocr_engine::OcrEngine::new(OCR_LANG) {
            Ok(e) => Some(e),
            Err(err) => {
                tracing::warn!("OCR unavailable ({err}); continuing without OCR");
                None
            }
        }
    };

    // Raster images are written next to the JSON; create the directory lazily
    // so documents without pictures leave no empty folder behind.
    let mut image_dir: Option<PathBuf> = None;
    let mut image_seq = 0usize;
    let mut overlay = preview::Preview::default();

    // One page, analysed but not yet written out.
    //
    // The furniture of a document — its running heads, its footers and its page
    // numbers — is a fact about the *document*: a page number is one only because
    // the next page has one too. So every page is analysed first, and the pages
    // are given their labels together, before any of them is emitted.
    struct Analysed<'a> {
        page_no: i32,
        width: f64,
        height: f64,
        pictures: Vec<PagePicture<'a>>,
        elements: Vec<pdfextractor_layout::Element>,
        /// The page's horizontal rules, kept for the footnote pass: a rule above a
        /// line of small print is what makes a note of it.
        rules: Vec<[f64; 4]>,
        /// The rows this page puts in the overlay for its text layer and its OCR
        /// lines, held here so they stay in front of its blocks in the sidebar.
        overlay: Vec<preview::OverlayItem>,
    }

    let mut analysed: Vec<Analysed> = Vec::new();
    for page in &pages {
        let mut rows: Vec<preview::OverlayItem> = Vec::new();

        // Text-layer runs stay visible in the overlay as a reference, even when
        // OCR is what the document will be built from.
        for span in &page.spans {
            rows.push(preview::OverlayItem {
                page_no: page.page_no,
                bbox: span.bbox,
                kind: preview::OverlayKind::Text,
                label: "paragraph".to_string(),
                text: span.text.clone(),
                image_uri: None,
            });
        }

        // Every picture on the page, so layout analysis can place it in reading
        // order rather than appending it after all the text.
        let mut pictures: Vec<PagePicture<'_>> = Vec::new();
        for placed in &page.placed_images {
            pictures.push(PagePicture::Raster(placed));
        }
        for figure in &page.figures {
            pictures.push(PagePicture::Vector(*figure));
        }
        let picture_boxes: Vec<[f64; 4]> = pictures.iter().map(PagePicture::bbox).collect();

        // A page with no text layer is almost always a scan: fall back to OCR.
        // When OCR runs, it is the only source of text for the document, so a
        // forced re-OCR replaces the text layer instead of duplicating it.
        let needs_ocr = page.spans.is_empty() || ocr == OcrMode::Always;
        let mut spans: Vec<LayoutSpan> = Vec::new();
        if !needs_ocr {
            spans.extend(page.spans.iter().map(|span| LayoutSpan {
                text: span.text.clone(),
                bbox: span.bbox,
                font_size: span.font_size,
                rotated: span.rotated,
            }));
        } else if let Some(engine) = engine.as_mut() {
            // Prefer the actual placement box; fall back to the whole page for
            // images the content stream never referenced.
            let targets: Vec<(&pdfextractor_pdf_parser::PlacedImage, [f64; 4])> =
                if page.placed_images.is_empty() {
                    Vec::new()
                } else {
                    page.placed_images.iter().map(|p| (p, p.bbox)).collect()
                };
            let fallback: Vec<[f64; 4]> = if targets.is_empty() {
                page.images
                    .iter()
                    .map(|_| [0.0, 0.0, page.width, page.height])
                    .collect()
            } else {
                Vec::new()
            };

            let images: Vec<(&Vec<u8>, u32, u32, [f64; 4])> = if targets.is_empty() {
                page.images
                    .iter()
                    .zip(fallback.iter())
                    .map(|(img, box_)| (&img.data, img.width, img.height, *box_))
                    .collect()
            } else {
                targets
                    .iter()
                    .map(|(p, box_)| (&p.image.data, p.image.width, p.image.height, *box_))
                    .collect()
            };

            for (data, width, height, box_) in images {
                match engine.recognize(data) {
                    Ok(result) => {
                        if result.lines.is_empty() {
                            tracing::debug!("OCR found no text on page {}", page.page_no);
                        }
                        for line in &result.lines {
                            let bbox = scale_ocr_bbox(line.bbox, width, height, box_);
                            spans.push(LayoutSpan {
                                text: line.text.clone(),
                                bbox,
                                rotated: false,
                                // Tesseract's line box is a little taller than the
                                // em, so shrink it back to a font size. Only the
                                // ratios matter, and they survive this estimate.
                                font_size: ((bbox[3] - bbox[1]) * 0.8).clamp(4.0, 400.0),
                            });
                            rows.push(preview::OverlayItem {
                                page_no: page.page_no,
                                bbox,
                                kind: preview::OverlayKind::Ocr,
                                label: "paragraph".to_string(),
                                text: line.text.clone(),
                                image_uri: None,
                            });
                        }
                    }
                    Err(err) => {
                        tracing::warn!("OCR failed for page {}: {err}", page.page_no);
                    }
                }
            }
        }

        // Layout: group the runs into ordered, labelled blocks and place the
        // pictures among them. The page's own rules go with them: the rule a page
        // draws above its notes stands where a table's top border would, and it is
        // what tells a note block from the two-column table it looks like.
        let elements = pdfextractor_layout::analyze_page(
            &spans,
            [page.width, page.height],
            &picture_boxes,
            &page.rules,
            page.page_no,
        );
        analysed.push(Analysed {
            page_no: page.page_no,
            width: page.width,
            height: page.height,
            pictures,
            elements,
            rules: page.rules.clone(),
            overlay: rows,
        });
    }

    // Give the document's own furniture its shape: the running heads, the
    // footers and the page numbers of its pages are labelled apart from the text
    // they stand beside, so a consumer can read the document without them — and
    // so a page number stops being read as a section heading.
    let mut scenes: Vec<pdfextractor_layout::PageElements> = analysed
        .iter()
        .map(|page| pdfextractor_layout::PageElements {
            size: [page.width, page.height],
            elements: page.elements.clone(),
            rules: page.rules.clone(),
        })
        .collect();
    let furniture = pdfextractor_layout::mark_furniture(&mut scenes);
    if furniture > 0 {
        tracing::debug!("labelled {furniture} item(s) of page furniture");
    }
    // And the notes the pages set at their feet, found by the rule above them and the
    // marker they open with. A note is the document's text, not the page's mark, so it
    // runs after the furniture: a page number is never a note, and a note never a page
    // number.
    let footnotes = pdfextractor_layout::mark_footnotes(&mut scenes);
    if footnotes > 0 {
        tracing::debug!("labelled {footnotes} footnote(s)");
    }
    for (page, scene) in analysed.iter_mut().zip(scenes) {
        page.elements = scene.elements;
    }

    for page in analysed {
        builder.add_page(page.page_no, page.width, page.height);
        overlay.pages.push(preview::PreviewPage {
            page_no: page.page_no,
            width: page.width,
            height: page.height,
            backdrop: None,
        });
        overlay.items.extend(page.overlay);

        for element in page.elements {
            match element {
                pdfextractor_layout::Element::Text {
                    text,
                    label,
                    level,
                    marker,
                    bbox,
                } => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    if pdfextractor_layout::is_furniture_label(&label) {
                        builder.add_furniture_on_page(&text, page.page_no, bbox, &label);
                    } else {
                        let enumerated = enumerated(&marker);
                        builder.add_text_item(
                            &text,
                            page.page_no,
                            bbox,
                            pdfextractor_core::TextLabel {
                                // The overlay keeps the label too, for its sidebar.
                                label: label.clone(),
                                level,
                                marker,
                                enumerated,
                            },
                        );
                    }
                    overlay.items.push(preview::OverlayItem {
                        page_no: page.page_no,
                        bbox,
                        kind: preview::OverlayKind::Block,
                        label,
                        text,
                        image_uri: None,
                    });
                }
                pdfextractor_layout::Element::Table { rows, bbox } => {
                    builder.add_table_item(&rows, page.page_no, bbox);
                    overlay.items.push(preview::OverlayItem {
                        page_no: page.page_no,
                        bbox,
                        kind: preview::OverlayKind::Block,
                        label: "table".to_string(),
                        text: rows
                            .iter()
                            .map(|row| row.join(" | "))
                            .collect::<Vec<_>>()
                            .join("  ·  "),
                        image_uri: None,
                    });
                }
                pdfextractor_layout::Element::Picture { index, bbox } => {
                    match &page.pictures[index] {
                        PagePicture::Raster(placed) => {
                            image_seq += 1;
                            let name = format!("pg{}_{}", page.page_no, image_seq);
                            let uri = write_image(
                                &mut image_dir,
                                output_path,
                                &name,
                                placed.image.mime,
                                &placed.image.data,
                            );
                            let image = uri.as_ref().map(|uri| PictureData {
                                mimetype: placed.image.mime.to_string(),
                                width: Some(placed.image.width),
                                height: Some(placed.image.height),
                                // A PDF image's effective resolution is its pixel grid
                                // divided by the box it was drawn into.
                                dpi: effective_dpi(
                                    placed.image.width,
                                    placed.bbox[2] - placed.bbox[0],
                                ),
                                uri: uri.clone(),
                            });
                            builder.add_picture(page.page_no, bbox, "picture", image);
                            overlay.items.push(preview::OverlayItem {
                                page_no: page.page_no,
                                bbox,
                                kind: preview::OverlayKind::Image,
                                label: "picture".to_string(),
                                text: String::new(),
                                image_uri: uri,
                            });
                        }
                        // Vector figures have no pixels of their own: report the box.
                        PagePicture::Vector(_) => {
                            builder.add_picture(page.page_no, bbox, "picture", None);
                            overlay.items.push(preview::OverlayItem {
                                page_no: page.page_no,
                                bbox,
                                kind: preview::OverlayKind::Figure,
                                label: "picture".to_string(),
                                text: String::new(),
                                image_uri: None,
                            });
                        }
                    }
                }
            }
        }
    }

    if want_preview && !overlay.is_empty() {
        write_preview(&mut overlay, path, output_path, preview_dpi);
    }

    Ok(builder.build())
}

/// Render page backdrops where possible, then write the overlay HTML.
fn write_preview(overlay: &mut preview::Preview, pdf: &Path, output_path: &Path, dpi: u32) {
    let dir = preview_dir_for(output_path);
    let backdrops = preview::render_pages(pdf, &dir, &overlay.pages, dpi);
    for (page, backdrop) in overlay.pages.iter_mut().zip(backdrops) {
        page.backdrop = backdrop;
    }

    let html_path = output_path.with_extension("preview.html");
    let name = output_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document");
    match overlay.write(&html_path, name) {
        Ok(()) => tracing::info!("preview -> {}", html_path.display()),
        Err(err) => tracing::warn!("cannot write preview {}: {err}", html_path.display()),
    }
}

/// Directory holding a document's rendered page images, sibling to its JSON.
fn preview_dir_for(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    output_path.with_file_name(format!("{stem}_pages"))
}

/// Write an extracted image beside the JSON and return its relative URI.
///
/// The directory is created on first use. A write failure is logged and the
/// picture is still reported, just without pixels.
fn write_image(
    dir: &mut Option<PathBuf>,
    output_path: &Path,
    name: &str,
    mime: &str,
    data: &[u8],
) -> Option<String> {
    if dir.is_none() {
        let path = image_dir_for(output_path);
        if let Err(err) = std::fs::create_dir_all(&path) {
            tracing::warn!("cannot create {}: {err}", path.display());
            return None;
        }
        *dir = Some(path);
    }
    let directory = dir.as_ref()?;
    let file_name = format!("{name}.{}", ext_for_mime(mime));
    match std::fs::write(directory.join(&file_name), data) {
        Ok(()) => {
            let folder = directory
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("images");
            Some(format!("{folder}/{file_name}"))
        }
        Err(err) => {
            tracing::warn!("cannot write image {file_name}: {err}");
            None
        }
    }
}

/// Build a `DoclingDocument` from a DOCX file path.
fn convert_docx(
    path: &Path,
    output_path: &Path,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = std::fs::read(path)?;
    let parsed = pdfextractor_docx_parser::DocxDocument::parse_bytes(&data)?;

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document");

    let mut builder = DocumentBuilder::new(name);
    builder.set_origin(Origin {
        mimetype: Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        ),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name.to_string()),
        uri: None,
    });
    // DOCX is flow-based. `w:sectPr` declares a page size, but a Word document
    // has no fixed pagination — Word repaginates as the text is edited — and no
    // item sits on a page rectangle. Docling publishes no pages for one, and
    // publishing a page nothing is on was a claim we could not support (bug
    // 46). The declared size stays a parser-level fact (`DocxDocument`).

    let mut image_dir: Option<PathBuf> = None;
    let mut image_seq = 0usize;
    for item in &parsed.items {
        emit_docx_item(
            &mut builder,
            item,
            output_path,
            &mut image_dir,
            &mut image_seq,
        );
    }

    // Header and footer content, kept as furniture rather than discarded.
    let mut furniture = Vec::new();
    for item in &parsed.furniture {
        image_seq += 1;
        if let Some(reference) =
            emit_furniture_item(&mut builder, item, output_path, &mut image_dir, image_seq)
        {
            furniture.push(reference);
        }
    }
    builder.set_furniture(furniture);

    Ok(builder.build())
}

/// Build a `DoclingDocument` from a legacy `.doc` file path.
fn convert_doc(
    path: &Path,
    _output_path: &Path,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = std::fs::read(path)?;
    let parsed = pdfextractor_doc_parser::DocDocument::parse_bytes(&data)?;

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document");

    let mut builder = DocumentBuilder::new(name);
    builder.set_origin(Origin {
        mimetype: Some("application/msword".to_string()),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name.to_string()),
        uri: None,
    });
    // Legacy `.doc` is flow-based too, and its section table is not read, so
    // there is no page geometry to report at all.

    for item in &parsed.items {
        match item {
            pdfextractor_doc_parser::DocItem::Text { text, label } => {
                builder.add_labeled_text(text, label);
            }
            pdfextractor_doc_parser::DocItem::Picture => {
                // OfficeArt raster extraction is not implemented; the picture is
                // reported as a placeholder so its place in the flow survives.
                builder.add_picture_plain("picture", None);
            }
        }
    }

    // Headers and footers live in their own subdocument, so they are furniture
    // rather than body content (bug 47).
    let mut furniture = Vec::new();
    for item in &parsed.furniture {
        match item {
            pdfextractor_doc_parser::DocItem::Text { text, label } => {
                furniture.push(builder.add_furniture_text(text, label));
            }
            pdfextractor_doc_parser::DocItem::Picture => {
                furniture.push(builder.add_furniture_picture("picture", None));
            }
        }
    }
    builder.set_furniture(furniture);

    Ok(builder.build())
}

/// Build a `DoclingDocument` from a flow-based text format.
fn convert_text(
    ext: &str,
    path: &Path,
    _output_path: &Path,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = std::fs::read(path)?;
    let parsed = match ext {
        "rtf" => pdfextractor_text_parser::rtf::parse(&data),
        "html" | "htm" => pdfextractor_text_parser::html::parse(&data),
        "md" | "markdown" => pdfextractor_text_parser::text::parse_markdown(&data),
        "csv" => pdfextractor_text_parser::csv::parse_csv(&data),
        "tsv" => pdfextractor_text_parser::csv::parse_tsv(&data),
        _ => pdfextractor_text_parser::text::parse_plain(&data),
    };

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document");

    let mut builder = DocumentBuilder::new(name);
    builder.set_origin(Origin {
        mimetype: Some(mimetype_for_ext(ext).to_string()),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name.to_string()),
        uri: None,
    });
    // A flow-based format has no page rectangle, so the document reports no
    // pages, exactly as Docling's own output for one does.

    for block in &parsed.items {
        match block {
            pdfextractor_text_parser::Block::Text {
                text,
                label,
                level,
                marker,
            } => {
                builder.add_labeled_item(text, label, *level, marker.clone(), enumerated(marker));
            }
            pdfextractor_text_parser::Block::Table { rows } => {
                builder.add_table_plain(rows);
            }
        }
    }

    Ok(builder.build())
}

/// MIME type for a flow-based extension.
fn mimetype_for_ext(ext: &str) -> &'static str {
    match ext {
        "rtf" => "application/rtf",
        "html" | "htm" => "text/html",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        _ => "text/plain",
    }
}

/// Emit one DOCX block into the document body.
fn emit_docx_item(
    builder: &mut DocumentBuilder,
    item: &pdfextractor_docx_parser::DocxItem,
    output_path: &Path,
    image_dir: &mut Option<PathBuf>,
    image_seq: &mut usize,
) {
    use pdfextractor_docx_parser::DocxItem;
    match item {
        DocxItem::Text {
            text,
            label,
            marker,
            level,
        } => {
            builder.add_labeled_item(text, label, *level, marker.clone(), enumerated(marker));
        }
        DocxItem::Formula { text } => {
            builder.add_labeled_text(text, "formula");
        }
        DocxItem::Table { rows } => {
            builder.add_table_plain(rows);
        }
        DocxItem::Picture {
            mime,
            data,
            width_pt,
            height_pt,
        } => {
            *image_seq += 1;
            let name = format!("pg1_{}", image_seq);
            let image = write_image(image_dir, output_path, &name, mime, data)
                .map(|uri| describe_image(mime, data, *width_pt, *height_pt, uri));
            builder.add_picture_plain("picture", image);
        }
    }
}

/// Emit one furniture block and return the reference to attach to the group.
fn emit_furniture_item(
    builder: &mut DocumentBuilder,
    item: &pdfextractor_docx_parser::DocxItem,
    output_path: &Path,
    image_dir: &mut Option<PathBuf>,
    seq: usize,
) -> Option<Ref> {
    use pdfextractor_docx_parser::DocxItem;
    match item {
        DocxItem::Text {
            text,
            label,
            marker,
            level,
        } => Some(builder.add_furniture_item(text, label, *level, marker.clone())),
        DocxItem::Formula { text } => Some(builder.add_furniture_text(text, "formula")),
        DocxItem::Table { rows } => Some(builder.add_furniture_table(rows)),
        DocxItem::Picture {
            mime,
            data,
            width_pt,
            height_pt,
        } => {
            let name = format!("furniture_{seq}");
            let image = write_image(image_dir, output_path, &name, mime, data)
                .map(|uri| describe_image(mime, data, *width_pt, *height_pt, uri));
            Some(builder.add_furniture_picture("picture", image))
        }
    }
}

/// Whether a list marker counts (`1.`, `a)`) rather than marks (`•`).
fn enumerated(marker: &Option<String>) -> Option<bool> {
    marker
        .as_deref()
        .map(|m| m.chars().next().is_some_and(|c| c.is_alphanumeric()))
}

/// Describe an extracted DOCX image: pixel size when its header tells us, and
/// the resolution implied by the size the document asked for.
fn describe_image(
    mime: &str,
    data: &[u8],
    width_pt: Option<f64>,
    height_pt: Option<f64>,
    uri: String,
) -> PictureData {
    let dims = image_dimensions(data);
    let dpi = match (dims, width_pt) {
        (Some((w, _)), Some(pt)) => effective_dpi(w, pt),
        _ => None,
    };
    let _ = height_pt;
    PictureData {
        mimetype: mime.to_string(),
        width: dims.map(|d| d.0),
        height: dims.map(|d| d.1),
        dpi,
        uri,
    }
}

/// Resolution implied by `pixels` across `points`.
/// The resolution a picture's pixel count implies for the box it was drawn in.
///
/// Rounded to a whole number: Docling declares `image.dpi` an integer, so a
/// fraction is not a valid document.
fn effective_dpi(pixels: u32, points: f64) -> Option<u32> {
    if pixels == 0 || points <= 0.0 {
        return None;
    }
    Some((pixels as f64 * 72.0 / points).round() as u32)
}

/// Pixel dimensions of a PNG or JPEG, read from its header.
///
/// Reported as the image's `size`; returning `None` leaves it out rather than
/// inventing numbers.
fn image_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() > 24 && data.starts_with(&[0x89, b'P', b'N', b'G']) {
        let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if data.starts_with(&[0xff, 0xd8]) {
        let mut i = 2usize;
        while i + 9 < data.len() {
            if data[i] != 0xff {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            // SOF0..SOF15, excluding the non-frame markers DHT/JPG/DAC.
            if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((w, h));
            }
            let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            if len < 2 {
                return None;
            }
            i += 2 + len;
        }
    }
    None
}

/// Map an OCR box in image pixels into the box the image occupies on the page.
///
/// `target` is `[l, t, r, b]` in points, top-left origin. For a scan it is the
/// whole page; for a figure it is the tighter placement box.
fn scale_ocr_bbox(
    bbox: [i32; 4],
    image_width: u32,
    image_height: u32,
    target: [f64; 4],
) -> [f64; 4] {
    let sx = (target[2] - target[0]) / image_width.max(1) as f64;
    let sy = (target[3] - target[1]) / image_height.max(1) as f64;
    [
        target[0] + bbox[0] as f64 * sx,
        target[1] + bbox[1] as f64 * sy,
        target[0] + bbox[2] as f64 * sx,
        target[1] + bbox[3] as f64 * sy,
    ]
}

/// FNV-1a hash, matching the spirit of Docling's cheap binary fingerprint.
fn fnv1a(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
