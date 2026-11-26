//! The conversion pipeline: a file in, a `DoclingDocument` out.
//!
//! This is the command line's reading, without the command line: the same
//! readers, layout, OCR, budgets and picture extraction, callable from a
//! library that embeds the extractor rather than spawning the binary. The
//! command line keeps its argument parsing, input walk, output writers,
//! compare/redline modes, and the extension gate that tells a user why a name
//! is refused. The conversion itself — `build_document` and every reader it
//! dispatches to — lives here.

use anyhow::{Context, Result};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use pdfextractor_core::page_range;
use pdfextractor_core::{
    DoclingDocument, DocumentBuilder, GridCell, GridTable, Origin, PictureData, Ref, TextLabel,
};
use pdfextractor_formats::{bridge_with_limits, detect, BridgeTarget, Format};
use pdfextractor_layout::Span as LayoutSpan;
use pdfextractor_limits::{LimitError, Limits};
use pdfextractor_ocr_engine::{OcrEngine, QuarterTurn};
use pdfextractor_pdf_parser::PdfDocument;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

mod preview;

/// When to run OCR on page images.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OcrMode {
    /// OCR only pages that carry no extractable text layer (scanned pages).
    Auto,
    /// Always OCR page images, even when a text layer exists.
    Always,
    /// Never run OCR.
    Never,
}

/// The reader's choices for one conversion.
pub struct ConvertOptions<'a> {
    pub ocr: OcrMode,
    /// Tesseract language codes, already joined with `+` (`eng+deu`).
    pub ocr_lang: &'a str,
    /// Tesseract page segmentation mode, 0–13.
    pub psm: Option<u8>,
    /// Replace the isolated pixels a fax leaves before recognising a page.
    ///
    /// Off by default: a born-digital page has no specks, and the pass costs a
    /// decode per page. A caller reading faxed or noisy scans turns it on;
    /// `--ocr-despeckle` is the command line's word for the same decision.
    pub despeckle: bool,
    /// The budgets this conversion is held to.
    ///
    /// The conversion's own, not the process's: an embedder that reads two
    /// documents may hold each to a different ceiling, and a run's flags are
    /// gathered into one value here rather than installed in a global.
    pub limits: Limits,
    /// The pages to read, when fewer than all are wanted.
    pub page_range: Option<&'a page_range::PageRange>,
    /// The password of an encrypted PDF.
    pub pdf_password: Option<&'a str>,
    /// The directory this conversion writes its extracted pictures into, or
    /// `None` to report a picture's place and facts without pixels on disk.
    ///
    /// The path is the folder itself, not the stem beside it: the caller names
    /// the folder — the command line puts it beside the output, `<stem>_images`
    /// — and a document's picture URIs name it from there.
    pub image_base: Option<&'a Path>,
    /// The output a preview is written beside, as `<stem>.preview.html`, or
    /// `None` for no preview. The pages drawn behind its boxes go into a
    /// `<stem>_pages` sibling.
    pub preview: Option<&'a Path>,
    /// Resolution for the preview's page backdrop.
    pub preview_dpi: u32,
}

impl Default for ConvertOptions<'_> {
    fn default() -> Self {
        Self {
            ocr: OcrMode::Auto,
            ocr_lang: "eng",
            psm: None,
            despeckle: false,
            limits: Limits::default(),
            page_range: None,
            pdf_password: None,
            image_base: None,
            preview: None,
            preview_dpi: 110,
        }
    }
}

/// Why a conversion stopped, in the cases a caller can act on without reading
/// the message.
///
/// The readers keep their `anyhow` errors: a parser explains itself in context,
/// and that context is worth keeping. What this boundary adds is the handful of
/// outcomes a caller decides differently — a budget refusal to retry with a
/// larger ceiling, a password to ask for, a name or a range to fix — and
/// `Other` for everything that is only an explanation.
#[derive(Debug)]
pub enum PipelineError {
    /// A budget refused the document, a stream, a picture or a deadline.
    Limit(LimitError),
    /// The file is encrypted and cannot be read as it stands.
    Encrypted { path: PathBuf, why: Encrypted },
    /// The bytes are a format no reader here takes.
    UnsupportedFormat(String),
    /// The page range named none of the document's pages, sheets or slides.
    PageRange {
        path: PathBuf,
        range: page_range::PageRange,
        total: usize,
        unit: RangeUnit,
    },
    /// The OCR language list was empty: recognition needs one code at least.
    OcrLangEmpty,
    /// The file could not be opened, or its size read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Anything else a reader said, with its own context intact.
    Other(anyhow::Error),
}

/// Why an encrypted PDF was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Encrypted {
    /// No password was given and the file needs one.
    NeedsPassword,
    /// The password given does not open it.
    WrongPassword,
    /// The cipher is one this build cannot read.
    Unsupported(String),
}

/// What a page range counts in the document it was applied to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RangeUnit {
    /// A paginated document's pages.
    Page,
    /// An image file's own pages: its frames, or a TIFF's directories. The
    /// refusal names the file rather than the document, because that is all an
    /// image is.
    ImagePage,
    /// A workbook's sheets.
    Sheet,
    /// A presentation's slides.
    Slide,
}

impl std::fmt::Display for RangeUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RangeUnit::Page | RangeUnit::ImagePage => "page",
            RangeUnit::Sheet => "sheet",
            RangeUnit::Slide => "slide",
        })
    }
}

impl RangeUnit {
    /// The parenthetical a range refusal ends with: how many the range could
    /// have named, and how many of that unit the document has.
    ///
    /// Here rather than in the caller because the words are about the format —
    /// a workbook counts sheets, a deck slides — and a caller that says them
    /// again would drift from this one.
    pub fn counted(&self, total: usize) -> String {
        match self {
            RangeUnit::Page => format!("{total} page(s): 1-{total}"),
            RangeUnit::ImagePage => format!("the file has {total} page(s); 1-{total}"),
            RangeUnit::Sheet => format!("the workbook has {total} sheet(s); 1-{total}"),
            RangeUnit::Slide => format!("the deck has {total} slide(s); 1-{total}"),
        }
    }
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::Limit(error) => write!(f, "{error}"),
            PipelineError::Encrypted { path, why } => match why {
                Encrypted::NeedsPassword => write!(
                    f,
                    "{}: this PDF is encrypted and needs a password",
                    path.display()
                ),
                Encrypted::WrongPassword => write!(
                    f,
                    "{}: this PDF is encrypted and the password given does not open it",
                    path.display()
                ),
                Encrypted::Unsupported(cipher) => write!(
                    f,
                    "{}: this PDF is encrypted with {cipher}, which this build cannot decrypt — \
                     re-save it without encryption, for example `qpdf --decrypt in.pdf out.pdf`, \
                     and read that file",
                    path.display()
                ),
            },
            PipelineError::UnsupportedFormat(what) => write!(f, "unsupported format: {what}"),
            PipelineError::PageRange {
                path,
                range,
                total,
                unit,
            } => write!(
                f,
                "{range} names no {unit} of {} ({})",
                path.display(),
                unit.counted(*total)
            ),
            PipelineError::OcrLangEmpty => f.write_str(
                "the OCR language list is empty; at least one language code is needed, e.g. `eng`",
            ),
            PipelineError::Io { path, .. } => write!(f, "read {}", path.display()),
            PipelineError::Other(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PipelineError::Limit(error) => Some(error),
            PipelineError::Io { source, .. } => Some(source),
            PipelineError::Other(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl PipelineError {
    /// This error's case, from the `anyhow` error a reader handed back.
    ///
    /// The specific cases a conversion raises itself travel through `?`
    /// unchanged, so the outermost error is already one of them. A budget
    /// refusal is recognised wherever it sits in the chain, because the reader
    /// that hit it may have added context as it passed it on.
    fn classify(error: anyhow::Error) -> Self {
        match error.downcast::<PipelineError>() {
            Ok(typed) => typed,
            Err(error) => match budget_refusal(&error) {
                Some(limit) => PipelineError::Limit(limit),
                None => PipelineError::Other(error),
            },
        }
    }
}

/// The budget refusal `error` carries, at any depth of its context.
///
/// The command line has the public predicate — whether a run exits with the
/// refusal status is its decision, not the library's — and this is the
/// pipeline's own reading of the same chain, for the one place that acts on a
/// refusal here: a legacy `.doc` falls back to the built-in reader unless the
/// budget was what refused it.
fn budget_refusal(error: &anyhow::Error) -> Option<LimitError> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<LimitError>().cloned())
}

/// The language list Tesseract wants: `eng,deu` and `eng+deu` say the same
/// thing, and Tesseract spells it with `+`.
///
/// `None` when the list held no code at all; what to call that is the caller's
/// word, because the flag that carried the list is the caller's too.
fn tesseract_langs(raw: &str) -> Option<String> {
    let langs: Vec<&str> = raw
        .split([',', '+'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if langs.is_empty() {
        return None;
    }
    Some(langs.join("+"))
}

/// Convert one file, deciding its format from its bytes.
///
/// Where the conversion may write — the pictures' folder and the preview's
/// output — is the options', so an embedder that wants none of it passes
/// neither and still gets the document.
pub fn build_document(
    path: &Path,
    options: &ConvertOptions<'_>,
) -> Result<DoclingDocument, PipelineError> {
    check_input_size(path, &options.limits)?;
    let detected = detect(path).map_err(PipelineError::Other)?;
    build_document_with_format(path, &detected, options)
}

/// Convert one file whose format the caller has already decided.
///
/// The command line sniffs once, refuses a name its gate does not accept, and
/// then calls this; an embedder normally calls [`build_document`].
///
/// The output file itself is the caller's: this writes only the pictures and
/// the preview its options name, so an output directory is created by whoever
/// writes the document, not here.
pub fn build_document_with_format(
    path: &Path,
    detected: &Format,
    options: &ConvertOptions<'_>,
) -> Result<DoclingDocument, PipelineError> {
    check_input_size(path, &options.limits)?;
    let paginated = matches!(
        detected,
        Format::Pdf
            | Format::Ods
            | Format::Odp
            | Format::Xlsx
            | Format::Pptx
            | Format::Image { .. }
    ) || matches!(
        detected,
        Format::Bridge {
            target: BridgeTarget::Xlsx | BridgeTarget::Pptx,
            ..
        }
    );
    if options.page_range.is_some() && !paginated {
        tracing::info!(
            "{}: a page range applies to pages — a PDF's, a workbook's sheets, a \
             presentation's slides, an image's frames; this format is read whole",
            path.display()
        );
    }

    let converted = if matches!(detected, Format::Pdf | Format::Image { .. }) {
        let Some(langs) = tesseract_langs(options.ocr_lang) else {
            return Err(PipelineError::OcrLangEmpty);
        };
        with_worker_ocr_engine(options.ocr, &langs, options.psm, |engine| {
            build_with_engine(path, detected, options, engine)
        })
    } else {
        build_with_engine(path, detected, options, None)
    };
    converted.map_err(PipelineError::classify)
}

fn check_input_size(path: &Path, limits: &Limits) -> Result<(), PipelineError> {
    let metadata = std::fs::metadata(path).map_err(|source| PipelineError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    limits
        .check_input(usize::try_from(metadata.len()).unwrap_or(usize::MAX))
        .map_err(PipelineError::Limit)
}

fn build_with_engine(
    path: &Path,
    detected: &Format,
    options: &ConvertOptions<'_>,
    engine: Option<&mut OcrEngine>,
) -> Result<DoclingDocument> {
    let image_base = options.image_base;
    let doc = match detected {
        Format::Pdf => convert_pdf(
            path,
            &PdfReading {
                ocr: options.ocr,
                range: options.page_range,
                password: options.pdf_password,
                despeckle: options.despeckle,
            },
            image_base,
            options.preview,
            options.preview_dpi,
            &options.limits,
            engine,
        )?,
        Format::Docx => {
            note_flow_format(options.preview, path);
            convert_docx(path, image_base, &options.limits)?
        }
        Format::Odt => {
            note_flow_format(options.preview, path);
            convert_odt(path, image_base, &options.limits)?
        }
        Format::Ods => {
            if options.preview.is_some() {
                tracing::info!(
                    "{}: the preview draws page images; a workbook has none, skipping",
                    path.display()
                );
            }
            convert_ods(path, image_base, options.page_range, &options.limits)?
        }
        Format::Odp => {
            if options.preview.is_some() {
                tracing::info!(
                    "{}: the preview draws PDF page images; a presentation has none, skipping",
                    path.display()
                );
            }
            convert_odp(path, image_base, options.page_range, &options.limits)?
        }
        Format::Xlsx => {
            if options.preview.is_some() {
                tracing::info!(
                    "{}: the preview draws page images; a workbook has none, skipping",
                    path.display()
                );
            }
            convert_xlsx(path, image_base, options.page_range, &options.limits)?
        }
        Format::Pptx => {
            if options.preview.is_some() {
                tracing::info!(
                    "{}: the preview draws PDF page images; a presentation has none, skipping",
                    path.display()
                );
            }
            convert_pptx(path, image_base, options.page_range, &options.limits)?
        }
        Format::Image { .. } => {
            if options.preview.is_some() {
                tracing::info!(
                    "{}: the preview draws PDF page images; an image is its own page, skipping",
                    path.display()
                );
            }
            convert_image(
                path,
                options.page_range,
                options.despeckle,
                &options.limits,
                engine,
            )?
        }
        Format::Doc => {
            note_flow_format(options.preview, path);
            match bridge_with_limits(
                path,
                BridgeTarget::Docx,
                "a legacy Word document",
                &options.limits,
            ) {
                Ok(bridged) => {
                    tracing::info!(
                        "{}: a legacy Word document, read through LibreOffice as docx",
                        path.display()
                    );
                    let doc = convert_docx(bridged.path(), image_base, &options.limits)?;
                    relabel(doc, path, "application/msword", &options.limits)?
                }
                Err(error) => {
                    if budget_refusal(&error).is_some() {
                        return Err(error);
                    }
                    tracing::debug!("{}: {error:#}", path.display());
                    tracing::info!(
                        "{}: reading the legacy Word document with the built-in reader",
                        path.display()
                    );
                    convert_doc(path, image_base, &options.limits)?
                }
            }
        }
        Format::Flow(flow) => {
            note_flow_format(options.preview, path);
            convert_text(flow, path, image_base, &options.limits)?
        }
        Format::Bridge {
            target,
            mimetype,
            label,
        } => {
            match target {
                BridgeTarget::Pptx => {
                    if options.preview.is_some() {
                        tracing::info!(
                            "{}: the preview draws PDF page images; a presentation has none, skipping",
                            path.display()
                        );
                    }
                }
                _ => note_flow_format(options.preview, path),
            }
            let bridged = bridge_with_limits(path, *target, label, &options.limits)?;
            tracing::info!(
                "{}: {label}, read through LibreOffice as {}",
                path.display(),
                target.extension()
            );
            let doc = match target {
                BridgeTarget::Docx => convert_docx(bridged.path(), image_base, &options.limits)?,
                BridgeTarget::Xlsx => convert_xlsx(
                    bridged.path(),
                    image_base,
                    options.page_range,
                    &options.limits,
                )?,
                BridgeTarget::Csv => {
                    convert_text("csv", bridged.path(), image_base, &options.limits)?
                }
                BridgeTarget::Pptx => convert_pptx(
                    bridged.path(),
                    image_base,
                    options.page_range,
                    &options.limits,
                )?,
            };
            relabel(doc, path, mimetype, &options.limits)?
        }
        Format::Unsupported(what) => {
            return Err(PipelineError::UnsupportedFormat((*what).to_string()).into());
        }
    };
    Ok(doc)
}

/// Read an input only after its size is known to fit the budget.
///
/// The readers also check their slice, but the command line has to read the
/// file first; checking the metadata here keeps a giant file from becoming an
/// allocation before any reader sees it.
fn read_input(path: &Path, limits: &Limits) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::File::open(path).with_context(|| format!("read {}", path.display()))?;
    read_open_input(&file, path, limits)
}

/// The bytes of a file already open, held to the input ceiling.
///
/// The handle is taken rather than the name so the size that is checked and the
/// bytes that are read are the same file: a name can be swapped between the two.
fn read_open_input(
    mut file: &std::fs::File,
    path: &Path,
    limits: &Limits,
) -> anyhow::Result<Vec<u8>> {
    let metadata = file
        .metadata()
        .with_context(|| format!("read the size of {}", path.display()))?;
    limits.check_input(usize::try_from(metadata.len()).unwrap_or(usize::MAX))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(bytes)
}

/// Give a document read through the bridge the identity of the file the user
/// handed over: the name, MIME type and hash are the original's, because that is
/// the document; LibreOffice was only how its words were reached. Here rather
/// than beside the bridge itself, because it is about the document the run
/// produces, not about what a file is.
fn relabel(
    mut doc: pdfextractor_core::DoclingDocument,
    original: &Path,
    mimetype: &str,
    limits: &Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(original, limits)?;
    let name = original
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_string());
    doc.name = name.clone();
    doc.origin = Some(Origin {
        mimetype: Some(mimetype.to_string()),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name),
        uri: None,
    });
    Ok(doc)
}

/// Note once, when a preview was asked for, why a flow-based format has none:
/// its blocks carry no page rectangle to draw a box on.
fn note_flow_format(preview: Option<&Path>, path: &Path) {
    if preview.is_some() {
        tracing::info!(
            "{}: preview needs page geometry; this format is flow-based, skipping",
            path.display()
        );
    }
}

/// Extension matching an image MIME type.
///
/// This must never guess: writing an EMF payload under a `.png` name makes the
/// extracted file lie about its own format. The formats are
/// [`pdfextractor_picture_formats::PICTURE_FORMATS`] — the same list the flow
/// reader accepts a `<source type="…">` by — so a format cannot be added on one
/// side only; a type outside it is written as `.bin` rather than given an
/// extension that would be wrong.
fn ext_for_mime(mime: &str) -> &'static str {
    pdfextractor_picture_formats::extension_for_mime(mime).unwrap_or("bin")
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

/// Start an OCR engine for one document, or `None` when OCR was turned off
/// or is unavailable.
///
/// A mode Tesseract will not take leaves the engine on its own default; it is
/// not a reason to lose the text.
fn new_ocr_engine(lang: &str, psm: Option<u8>) -> Option<OcrEngine> {
    match OcrEngine::new(lang) {
        Ok(mut engine) => {
            if let Some(psm) = psm {
                if let Err(err) = engine.set_psm(psm) {
                    tracing::warn!(
                        "page segmentation mode {psm} refused ({err}); keeping the engine's default"
                    );
                }
            }
            Some(engine)
        }
        Err(err) => {
            tracing::warn!("OCR unavailable ({err}); continuing without OCR");
            None
        }
    }
}

/// One worker's engine, kept for the next document that worker reads.
///
/// `LepTess` is neither `Sync` nor cheap to build — measured at roughly 176 ms
/// against 485 ms for one page on the reference machine — so a batch that built
/// one per document paid that cost N times over. A Rayon worker runs one task at
/// a time, and this thread-local slot lives as long as the worker, so the engine
/// outlives a single conversion and is lent to the next one. The language and
/// mode are part of the slot: a run has one of each, but a changed setting must
/// rebuild rather than silently keep the old engine.
struct CachedOcrEngine {
    lang: String,
    psm: Option<u8>,
    engine: OcrEngine,
}

thread_local! {
    static WORKER_OCR_ENGINE: RefCell<Option<CachedOcrEngine>> = const { RefCell::new(None) };
}

/// Lend this worker's engine to `convert`, building it on first use.
fn with_worker_ocr_engine<R>(
    ocr: OcrMode,
    lang: &str,
    psm: Option<u8>,
    convert: impl FnOnce(Option<&mut OcrEngine>) -> R,
) -> R {
    if ocr == OcrMode::Never {
        return convert(None);
    }
    WORKER_OCR_ENGINE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let reusable = slot
            .as_ref()
            .is_some_and(|cached| cached.lang == lang && cached.psm == psm);
        if !reusable {
            *slot = new_ocr_engine(lang, psm).map(|engine| CachedOcrEngine {
                lang: lang.to_string(),
                psm,
                engine,
            });
        }
        convert(slot.as_mut().map(|cached| &mut cached.engine))
    })
}

/// Build a `DoclingDocument` from a PDF file path.
/// The reader's choices for one PDF, gathered so the conversion takes the
/// document and the choices rather than a row of loose flags.
struct PdfReading<'a> {
    ocr: OcrMode,
    /// The pages to read, when the reader asked for fewer than all of them.
    range: Option<&'a page_range::PageRange>,
    /// The password (from `PDFE_PDF_PASSWORD` or a named file), for an
    /// encrypted file.
    password: Option<&'a str>,
    /// Replace the isolated pixels a fax leaves before recognising a page.
    despeckle: bool,
}

fn convert_pdf(
    path: &Path,
    reading: &PdfReading<'_>,
    image_base: Option<&Path>,
    preview: Option<&Path>,
    preview_dpi: u32,
    limits: &Limits,
    mut engine: Option<&mut OcrEngine>,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    // Recognition runs after parsing and takes seconds a page where parsing
    // takes milliseconds, so it is measured against its own budget. The
    // deadline is checked before each picture is recognised.
    let ocr_deadline = limits.ocr_deadline();
    let parsed =
        PdfDocument::parse_with_limits_and_password(&data, limits.clone(), reading.password)?;
    // A ciphered file the password does not open is refused here, by name,
    // rather than read as though its streams were plain: what came out unread
    // would be noise in a document that looks successful.
    match &parsed.locked {
        None => {
            if let Some(opened_with) = parsed.opened_with {
                tracing::info!(
                    "{}: encrypted PDF opened with the {opened_with}",
                    path.display()
                );
            }
        }
        Some(pdfextractor_pdf_parser::crypt::CryptError::WrongPassword) => {
            let why = if reading.password.is_some() {
                Encrypted::WrongPassword
            } else {
                Encrypted::NeedsPassword
            };
            return Err(PipelineError::Encrypted {
                path: path.to_path_buf(),
                why,
            }
            .into());
        }
        Some(pdfextractor_pdf_parser::crypt::CryptError::Unsupported(what)) => {
            return Err(PipelineError::Encrypted {
                path: path.to_path_buf(),
                why: Encrypted::Unsupported(what.clone()),
            }
            .into());
        }
    }
    // The reader's page range is applied to the pages the file has, before any of
    // them is analysed: the furniture pass must see exactly the pages that were
    // kept, or a running head on the only page left would be judged against
    // neighbours that are not in the document any more.
    let mut pages = parsed.extract_checked()?;
    if let Some(range) = reading.range {
        let total = pages.len();
        pages.retain(|page| range.contains(page.page_no));
        if pages.is_empty() {
            return Err(PipelineError::PageRange {
                path: path.to_path_buf(),
                range: range.clone(),
                total,
                unit: RangeUnit::Page,
            }
            .into());
        }
        tracing::info!(
            "{}: reading {} of {} page(s) ({range})",
            path.display(),
            pages.len(),
            total
        );
    }

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

    // The engine is the worker's, lent by `build_document`: `LepTess` is not
    // `Sync`, so it cannot be shared, but it can outlive the document that used
    // it without being rebuilt for the next one.

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
        /// The page's `/Link` annotations: the box each covers and where it
        /// opens, so text a link box covers carries the link.
        links: Vec<pdfextractor_pdf_parser::PageLink>,
        /// The rows this page puts in the overlay for its text layer and its OCR
        /// lines, held here so they stay in front of its blocks in the sidebar.
        overlay: Vec<preview::OverlayItem>,
    }

    // The document's own table of contents. Its titles and nesting are the one
    // heading signal a PDF carries that is not guessed from the page: layout
    // uses them to level the headings it found and to recover a heading line the
    // content stream merged into the paragraph beneath it.
    let outline = parsed.outline();
    if !outline.is_empty() {
        let on_a_page = outline
            .iter()
            .filter(|entry| entry.page_no.is_some())
            .count();
        tracing::debug!(
            "{}: {} outline entr(ies), {on_a_page} with a direct page destination",
            path.display(),
            outline.len()
        );
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
        let needs_ocr = page.spans.is_empty() || reading.ocr == OcrMode::Always;
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

            // A scan can arrive on its side with nothing saying so: no
            // `/Rotate` on the page, no EXIF tag in the picture, just pixels a
            // scanner laid down the wrong way. The page's whole text is in
            // that one picture, so Tesseract's own reading of the page's turns
            // can decide which way up it is written.
            let detect_sideways = page.rotation == 0 && page.spans.is_empty();

            for (data, width, height, box_) in images {
                ocr_deadline.check("recognising a page")?;
                let turn = if detect_sideways && picture_is_page(box_, page.width, page.height) {
                    match engine.detect_upright(data, limits) {
                        Ok(QuarterTurn::AsStored) => QuarterTurn::AsStored,
                        Ok(turn) => {
                            tracing::debug!(
                                "page {} is stored {turn:?}; turning it upright for OCR",
                                page.page_no
                            );
                            turn
                        }
                        Err(err) => {
                            tracing::debug!(
                                "could not find the orientation of page {}: {err}",
                                page.page_no
                            );
                            QuarterTurn::AsStored
                        }
                    }
                } else {
                    QuarterTurn::AsStored
                };
                // A page the document turns before display is a scan the OCR
                // must see the same way round: the bitmap is turned with the
                // page before recognition, so its lines come back upright, in
                // the displayed space the target box is measured in.
                let rotated = if page.rotation != 0 {
                    rotate_for_ocr(data, page.rotation, limits)
                } else if turn != QuarterTurn::AsStored {
                    rotate_for_ocr(data, turn.degrees(), limits)
                } else {
                    None
                };
                let (bytes, pixels_w, pixels_h) = match &rotated {
                    Some((png, w, h)) => (png.as_slice(), *w, *h),
                    None => (data.as_slice(), width, height),
                };
                // The page is upright now; a faxed one also loses its isolated
                // pixels. The pass keeps the grid, so the boxes scale onto the
                // target exactly as they do without it.
                let prepared = if reading.despeckle {
                    match pdfextractor_ocr_engine::preprocess::despeckle_page(bytes, limits) {
                        Ok(png) => Some(png),
                        Err(err) => {
                            tracing::debug!("could not preprocess page {}: {err}", page.page_no);
                            None
                        }
                    }
                } else {
                    None
                };
                let bytes = prepared.as_deref().unwrap_or(bytes);
                match engine.recognize(bytes) {
                    Ok(result) => {
                        if result.lines.is_empty() {
                            tracing::debug!("OCR found no text on page {}", page.page_no);
                        }
                        for line in &result.lines {
                            let bbox = scale_ocr_bbox(line.bbox, pixels_w, pixels_h, box_);
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
        let headings: Vec<pdfextractor_layout::HeadingHint> = outline
            .iter()
            .filter(|entry| entry.page_no == Some(page.page_no))
            .map(|entry| pdfextractor_layout::HeadingHint {
                title: entry.title.clone(),
                level: entry.depth + 1,
            })
            .collect();
        let elements = pdfextractor_layout::analyze_page_with_headings(
            &spans,
            [page.width, page.height],
            &picture_boxes,
            &page.rules,
            page.page_no,
            &headings,
        );
        analysed.push(Analysed {
            page_no: page.page_no,
            width: page.width,
            height: page.height,
            pictures,
            elements,
            rules: page.rules.clone(),
            links: page.links.clone(),
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
                                hyperlink: link_covering(&page.links, bbox).map(str::to_string),
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
                                image_base,
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

    if let Some(preview) = preview {
        if !overlay.is_empty() {
            write_preview(&mut overlay, path, preview, preview_dpi);
        }
    }

    Ok(builder.build())
}

/// Render page backdrops where possible, then write the overlay HTML.
///
/// `output` is the document's own output, the path the caller handed in; the
/// overlay lands beside it as `<stem>.preview.html`, as it always has.
fn write_preview(overlay: &mut preview::Preview, pdf: &Path, output: &Path, dpi: u32) {
    let dir = preview_dir_for(output);
    let backdrops = preview::render_pages(pdf, &dir, &overlay.pages, dpi);
    for (page, backdrop) in overlay.pages.iter_mut().zip(backdrops) {
        page.backdrop = backdrop;
    }

    let html_path = output.with_extension("preview.html");
    let name = output
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document");
    match overlay.write(&html_path, name) {
        Ok(()) => tracing::info!("preview -> {}", html_path.display()),
        Err(err) => tracing::warn!("cannot write preview {}: {err}", html_path.display()),
    }
}

/// Directory holding a document's rendered page images, sibling to its JSON.
fn preview_dir_for(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    output.with_file_name(format!("{stem}_pages"))
}

/// Write an extracted image into the folder the caller named and return its
/// relative URI, or `None` when the caller named no folder.
///
/// The folder is created on first use. A write failure is logged and the
/// picture is still reported, just without pixels.
fn write_image(
    dir: &mut Option<PathBuf>,
    image_base: Option<&Path>,
    name: &str,
    mime: &str,
    data: &[u8],
) -> Option<String> {
    let base = image_base?;
    if dir.is_none() {
        if let Err(err) = std::fs::create_dir_all(base) {
            tracing::warn!("cannot create {}: {err}", base.display());
            return None;
        }
        *dir = Some(base.to_path_buf());
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

/// The relative path a flow page's own picture keeps when it is copied beside
/// the output, or `None` when the page's spelling is not one this can write
/// back.
///
/// The page named the file, so the page keeps the name: a picture read from
/// `images/scan.jpg` is copied to `images/scan.jpg`, and the page written from
/// that document names the file it came with — which is what makes an HTML page
/// its own bytes when it is read and written again. The path has to be one this
/// can spell and one a file can live at: relative, without `..`, and with the
/// extension its own bytes are written under — a page calling a JPEG `.png` is
/// not describing the file this would write.
///
/// A name may hold a space or a character HTML has to escape (`&`, a quote):
/// both writers spell those, and both readers give them back unchanged, so the
/// page keeps its picture's name. What is refused is a character that would
/// come back as a different name or cannot be part of a path — a backslash or a
/// colon (a Windows separator and a URI scheme), a percent sign (a reader
/// decodes `%20` into a space, so the page's spelling and the file's name would
/// part company) and a control character (which would end the attribute rather
/// than name a file).
fn kept_picture_path(source: &str, mimetype: &str) -> Option<PathBuf> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }
    let path = Path::new(source);
    if path.is_absolute() || source.starts_with("//") {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in source.split('/') {
        if part == "." {
            continue;
        }
        // A path that climbs out of the output directory is not a name beside
        // the page, and an empty part is a doubled slash.
        if part.is_empty() || part == ".." {
            return None;
        }
        parts.push(part);
    }
    // A space needs no escaping in a quoted attribute or a `<…>` destination,
    // and `&`, a quote and the rest of the punctuation are escaped by the
    // writer and decoded by the reader. A source that names a URI scheme is
    // refused by its colon, as it is by [`local_image_path`].
    if source
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '%'))
    {
        return None;
    }
    let extension = path.extension()?.to_str()?;
    if !extension.eq_ignore_ascii_case(ext_for_mime(mimetype)) {
        return None;
    }
    Some(parts.iter().collect())
}

/// Write a picture to the relative path its page named, beside the folder the
/// extraction writes its pictures into — which sits beside the document.
///
/// The path is the page's own, so the page written from the document names the
/// same file again; the answer is the reference in the form both writers use
/// (forward slashes, relative to the document). A name with a space or a
/// character needing an escape travels through this unchanged: the writers
/// escape it where their own syntax asks them to.
fn write_kept_image(image_base: Option<&Path>, relative: &Path, data: &[u8]) -> Option<String> {
    // The document's own directory: the picture folder's parent.
    let directory = image_base?.parent()?;
    let target = directory.join(relative);
    if let Some(parent) = target.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            tracing::warn!("cannot create {}: {err}", parent.display());
            return None;
        }
    }
    match std::fs::write(&target, data) {
        Ok(()) => Some(
            relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        ),
        Err(err) => {
            tracing::warn!("cannot write {}: {err}", target.display());
            None
        }
    }
}

/// Build a `DoclingDocument` from a DOCX file path.
fn convert_docx(
    path: &Path,
    image_base: Option<&Path>,
    limits: &Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    let parsed =
        pdfextractor_docx_parser::DocxDocument::parse_bytes_with_limits(&data, limits.clone())?;

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
            image_base,
            &mut image_dir,
            &mut image_seq,
            limits,
        );
    }

    // Header and footer content, kept as furniture rather than discarded.
    let mut furniture = Vec::new();
    for item in &parsed.furniture {
        if let Some(reference) = emit_furniture_item(
            &mut builder,
            item,
            image_base,
            &mut image_dir,
            &mut image_seq,
            limits,
        ) {
            furniture.push(reference);
        }
    }
    builder.set_furniture(furniture);

    Ok(builder.build())
}

/// Build a `DoclingDocument` from an OpenDocument text document.
///
/// ODF is flow-based like DOCX: a page style states a rectangle, but no block
/// sits on a page a reader could point at, so the document publishes none.
fn convert_odt(
    path: &Path,
    image_base: Option<&Path>,
    limits: &Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    let parsed =
        pdfextractor_odt_parser::OdtDocument::parse_bytes_with_limits(&data, limits.clone())?;

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document");

    let mut builder = DocumentBuilder::new(name);
    builder.set_origin(Origin {
        mimetype: Some("application/vnd.oasis.opendocument.text".to_string()),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name.to_string()),
        uri: None,
    });

    let mut image_dir: Option<PathBuf> = None;
    let mut image_seq = 0usize;
    for item in &parsed.items {
        emit_odt_item(
            &mut builder,
            item,
            image_base,
            &mut image_dir,
            &mut image_seq,
            limits,
        );
    }

    Ok(builder.build())
}

/// Emit one ODT block into the document body.
fn emit_odt_item(
    builder: &mut DocumentBuilder,
    item: &pdfextractor_odt_parser::OdtItem,
    image_base: Option<&Path>,
    image_dir: &mut Option<PathBuf>,
    image_seq: &mut usize,
    limits: &Limits,
) {
    use pdfextractor_odt_parser::OdtItem;
    match item {
        OdtItem::Text {
            text,
            label,
            marker,
            level,
            hyperlink,
        } => {
            builder.add_labeled_item_with_link(
                text,
                label,
                *level,
                marker.clone(),
                enumerated(marker),
                hyperlink.clone(),
            );
        }
        OdtItem::Table(table) => {
            let grid = odt_grid(table, image_base, image_dir, image_seq, limits);
            builder.add_table_grid(&grid);
        }
        OdtItem::Picture {
            mime,
            data,
            width_pt,
            height_pt,
        } => {
            *image_seq += 1;
            let name = format!("pg1_{}", image_seq);
            let (mime, data) = office_media(mime, data.clone(), *width_pt, *height_pt, limits);
            let image = write_image(image_dir, image_base, &name, mime, &data)
                .map(|uri| describe_image(mime, &data, *width_pt, *height_pt, uri));
            builder.add_picture_plain("picture", image);
        }
    }
}

/// An ODT table as the builder's grid: every cell a merge did not cover,
/// placed at its row and column with the spans the document declares.
fn odt_grid(
    table: &pdfextractor_odt_parser::OdtTable,
    image_base: Option<&Path>,
    image_dir: &mut Option<PathBuf>,
    image_seq: &mut usize,
    limits: &Limits,
) -> GridTable {
    let mut cells = Vec::new();
    for (r, row) in table.rows.iter().enumerate() {
        for cell in row {
            if cell.covered {
                continue;
            }
            let pictures = cell
                .pictures
                .iter()
                .filter_map(|picture| {
                    *image_seq += 1;
                    let name = format!("pg1_{}", image_seq);
                    let (mime, data) = office_media(
                        picture.mime,
                        picture.data.clone(),
                        picture.width_pt,
                        picture.height_pt,
                        limits,
                    );
                    write_image(image_dir, image_base, &name, mime, &data).map(|uri| {
                        describe_image(mime, &data, picture.width_pt, picture.height_pt, uri)
                    })
                })
                .collect();
            let tables = cell
                .tables
                .iter()
                .map(|nested| odt_grid(nested, image_base, image_dir, image_seq, limits))
                .collect();
            cells.push(GridCell {
                text: cell.text.clone(),
                start_row: r,
                start_col: cell.start_col,
                row_span: cell.row_span.max(1),
                col_span: cell.col_span.max(1),
                column_header: cell.header,
                row_header: !cell.header && cell.start_col == 0 && r > 0,
                pictures,
                tables,
            });
        }
    }
    GridTable {
        num_rows: table.rows.len(),
        num_cols: table.num_cols,
        cells,
    }
}

/// Build a `DoclingDocument` from an XLSX workbook.
fn convert_xlsx(
    path: &Path,
    image_base: Option<&Path>,
    range: Option<&page_range::PageRange>,
    limits: &Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    let parsed =
        pdfextractor_xlsx_parser::XlsxDocument::parse_bytes_with_limits(&data, limits.clone())?;
    convert_workbook(
        &parsed.sheets,
        &parsed.annotations,
        &WorkbookReading {
            data: &data,
            path,
            image_base,
            range,
            limits,
            mimetype: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        },
    )
}

/// Build a `DoclingDocument` from an OpenDocument spreadsheet.
///
/// An ODS workbook is read into the same sheets the XLSX reader produces, so
/// the two packages take one path: a sheet is a page, a hidden sheet stays on
/// the `invisible` layer, and `--page-range` names sheets the same way.
fn convert_ods(
    path: &Path,
    image_base: Option<&Path>,
    range: Option<&page_range::PageRange>,
    limits: &Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    let parsed =
        pdfextractor_ods_parser::OdsDocument::parse_bytes_with_limits(&data, limits.clone())?;
    convert_workbook(
        &parsed.sheets,
        &parsed.annotations,
        &WorkbookReading {
            data: &data,
            path,
            image_base,
            range,
            limits,
            mimetype: "application/vnd.oasis.opendocument.spreadsheet",
        },
    )
}

/// What a workbook's two readers hand [`convert_workbook`]: the file the sheets
/// were read from, where its pictures go, the sheets to keep and the budgets to
/// keep to. Gathered so the conversion takes the reading rather than six loose
/// arguments beside it.
#[derive(Copy, Clone)]
struct WorkbookReading<'a> {
    data: &'a [u8],
    path: &'a Path,
    image_base: Option<&'a Path>,
    range: Option<&'a page_range::PageRange>,
    limits: &'a Limits,
    mimetype: &'a str,
}

/// Build a `DoclingDocument` from a workbook's sheets.
///
/// Each worksheet is a page whose units are cells rather than points — the
/// shape Docling's own sheet backend produces: a `sheet` group per worksheet
/// with the worksheet's tables beneath it, and a provenance box in cell
/// indices (0-based, top-left origin). A hidden worksheet stays in the
/// document as the `invisible` layer rather than being dropped. A cell's
/// comment is a remark about the sheet rather than part of its grid, so it is
/// placed on the notes layer — one `comment_section` group per cell, named for
/// the cell — rather than becoming words in a table.
fn convert_workbook(
    workbook: &[pdfextractor_xlsx_parser::Sheet],
    annotations: &[pdfextractor_xlsx_parser::SheetAnnotation],
    reading: &WorkbookReading<'_>,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let WorkbookReading {
        data,
        path,
        image_base,
        range,
        limits,
        mimetype,
    } = *reading;
    // A sheet is a page, so the range is applied before the first one is read: a
    // workbook read as one sheet costs one sheet rather than all of them.
    let total = workbook.len();
    let sheets: Vec<(usize, &pdfextractor_xlsx_parser::Sheet)> = workbook
        .iter()
        .enumerate()
        .filter(|(index, _)| range.is_none_or(|range| range.contains((index + 1) as i32)))
        .collect();
    if let Some(range) = range {
        if sheets.is_empty() {
            return Err(PipelineError::PageRange {
                path: path.to_path_buf(),
                range: range.clone(),
                total,
                unit: RangeUnit::Sheet,
            }
            .into());
        }
        tracing::info!(
            "{}: reading {} of {} sheet(s) ({range})",
            path.display(),
            sheets.len(),
            total
        );
    }

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workbook.xlsx");
    // Docling names a workbook by its stem and keeps the file name in the
    // origin; a page is a sheet, not a printing of one.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("workbook");

    let mut builder = DocumentBuilder::new(stem);
    builder.set_origin(Origin {
        mimetype: Some(mimetype.to_string()),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(data)),
        filename: Some(name.to_string()),
        uri: None,
    });

    // A workbook's pictures are written beside the JSON like a Word package's,
    // under names that say which sheet they are on.
    let mut image_dir: Option<PathBuf> = None;
    let mut image_seq = 0usize;

    for (index, sheet) in sheets {
        let page_no = (index + 1) as i32;
        let layer = if sheet.hidden { "invisible" } else { "body" };
        let group = builder.add_group("sheet", &sheet.name, layer);
        let mut width = 0.0f64;
        let mut height = 0.0f64;

        // What a sheet holds is read in the order a reader meets it: top to
        // bottom, then left to right, which is the order Docling sorts a
        // sheet's items into. A picture above a table is therefore read before
        // it, rather than after every table of the sheet.
        enum Placed<'a> {
            Table(&'a pdfextractor_xlsx_parser::SheetTable),
            Picture(&'a pdfextractor_xlsx_parser::SheetPicture),
            Chart(&'a pdfextractor_chart::ChartShape),
        }
        let mut placed: Vec<(f64, f64, Placed)> = sheet
            .tables
            .iter()
            .map(|table| {
                (
                    table.anchor.1 as f64,
                    table.anchor.0 as f64,
                    Placed::Table(table),
                )
            })
            .chain(
                sheet
                    .pictures
                    .iter()
                    .map(|picture| (picture.bbox[1], picture.bbox[0], Placed::Picture(picture))),
            )
            .chain(
                sheet
                    .charts
                    .iter()
                    .map(|chart| (chart.bbox[1], chart.bbox[0], Placed::Chart(chart))),
            )
            .collect();
        placed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));

        for (_, _, item) in placed {
            match item {
                Placed::Table(table) => {
                    let (col, row) = table.anchor;
                    let bbox = [
                        col as f64,
                        row as f64,
                        (col + table.num_cols) as f64,
                        (row + table.num_rows) as f64,
                    ];
                    width = width.max(bbox[2]);
                    height = height.max(bbox[3]);
                    let grid = GridTable {
                        num_rows: table.num_rows,
                        num_cols: table.num_cols,
                        cells: table
                            .cells
                            .iter()
                            .map(|cell| GridCell {
                                text: cell.text.clone(),
                                start_row: cell.row,
                                start_col: cell.col,
                                row_span: cell.row_span,
                                col_span: cell.col_span,
                                // A worksheet's first row is the header a
                                // spreadsheet reader takes it for, and
                                // `w:tblHeader` has no spreadsheet equivalent to
                                // disagree with.
                                column_header: cell.row == 0,
                                // A cell never labels its own row on a
                                // worksheet.
                                row_header: false,
                                // A worksheet's cell holds text; its pictures
                                // are anchored on the sheet, not drawn in it.
                                pictures: Vec::new(),
                                tables: Vec::new(),
                            })
                            .collect(),
                    };
                    builder.add_table_grid_in(&grid, page_no, bbox, &group);
                }
                Placed::Picture(picture) => {
                    image_seq += 1;
                    // A sheet's picture is handed on as PNG unless it is a PNG
                    // or a JPEG, exactly as a Word or PowerPoint package's is:
                    // Docling's own readers leave a picture in one of those two
                    // encodings, and a BMP, a GIF or a TIFF has to be decoded
                    // and re-encoded to reach the model at all. A drawing has
                    // no sheet box to be played at — a worksheet's page is a
                    // grid of cells — so it is played at its own viewport.
                    let (mime, data) =
                        office_media(picture.mime, picture.data.clone(), None, None, limits);
                    let image = image_dimensions(&data).and_then(|(width, height)| {
                        let uri = write_image(
                            &mut image_dir,
                            image_base,
                            &format!("sheet{page_no}_{image_seq}"),
                            mime,
                            &data,
                        )?;
                        Some(PictureData {
                            mimetype: mime.to_string(),
                            width: Some(width),
                            height: Some(height),
                            // A sheet's page is a grid of cells, which states no
                            // physical size a resolution could be measured
                            // from; 72 — one pixel per point — is the reading
                            // Docling's own sheet backend gives every picture it
                            // finds.
                            dpi: Some(72),
                            uri,
                        })
                    });
                    builder.add_picture_in(page_no, picture.bbox, "picture", image, &group);
                    width = width.max(picture.bbox[2]);
                    height = height.max(picture.bbox[3]);
                }
                Placed::Chart(chart) => {
                    // A chart is a picture of its data, and its own title is
                    // the picture's caption — the same shape a slide's chart
                    // takes, so a workbook's chart reads the way a deck's does.
                    // No image is written for it: what a chart says is the
                    // numbers it plots, not pixels of them.
                    let caption = chart.title.as_deref().map(|title| {
                        builder.add_text_item_in(
                            title,
                            page_no,
                            chart.bbox,
                            TextLabel {
                                label: "caption".to_string(),
                                ..Default::default()
                            },
                            &group,
                        )
                    });
                    let table = chart_table(chart);
                    builder.add_chart_picture_in(
                        page_no,
                        chart.bbox,
                        chart.kind.class_name(),
                        table.as_ref(),
                        caption.as_ref(),
                        &group,
                    );
                    width = width.max(chart.bbox[2]);
                    height = height.max(chart.bbox[3]);
                }
            }
        }
        // A cell's comment is about the cell, not a place in it: each is one
        // item on the notes layer, in a `comment_section` group of its own,
        // named for the cell it was written against — the shape a deck's
        // comments take. A page range leaves out the comments of the sheets it
        // leaves out.
        for annotation in annotations
            .iter()
            .filter(|annotation| annotation.sheet == page_no as usize)
        {
            let comment_group = builder.add_comment_group(&format!(
                "comment-sheet{}-{}-{}",
                page_no, annotation.row, annotation.col
            ));
            builder.add_comment_in(
                &comment_text_parts(
                    annotation.author.as_deref(),
                    None,
                    annotation.date.as_deref(),
                    &annotation.text,
                ),
                &comment_group,
            );
        }

        // The page is the sheet's own extent in cells: the largest right and
        // bottom edge of anything on it, or 0×0 for an empty worksheet.
        builder.add_page(page_no, width, height);
    }

    Ok(builder.build())
}

/// English Metric Units per point: a slide's geometry and the pixel resolution
/// a picture was drawn at are both measured in EMU, and a document's boxes are
/// not.
const EMU_PER_POINT: f64 = 12_700.0;

/// The largest chart data table we will write: a chart can declare a category
/// range of a whole spreadsheet column, and a million rows of mostly empty cells
/// is not a reading of it, it is a transcription of the range. Docling caps a
/// chart's table the same way, at the same number.
const MAX_CHART_TABLE_CELLS: usize = 100_000;

/// The numbers a chart plots, as the table Docling reconstructs them into: a
/// header row naming the series over one column each, the categories down the
/// first column, and one row per category.
///
/// The rows are the ones the chart actually plots: a chart that references a
/// whole column declares far more rows than it has data for, and the empty tail
/// is trimmed — while a gap *between* plotted points stays, because the chart
/// draws it. `None` is a chart with no series, or one whose every row is empty:
/// a chart that plots nothing has no data to report.
fn chart_table(chart: &pdfextractor_chart::ChartShape) -> Option<GridTable> {
    if chart.series.is_empty() {
        return None;
    }
    let declared = chart.categories.len().max(
        chart
            .series
            .iter()
            .map(|series| series.values.len())
            .max()?,
    );
    let populated = (0..declared).rev().find(|row| {
        let category = chart
            .categories
            .get(*row)
            .map(String::as_str)
            .unwrap_or_default();
        !category.is_empty()
            || chart
                .series
                .iter()
                .any(|series| series.values.get(*row).is_some_and(Option::is_some))
    })? + 1;

    let num_cols = chart.series.len() + 1;
    let data_rows = if populated * num_cols > MAX_CHART_TABLE_CELLS {
        (MAX_CHART_TABLE_CELLS / num_cols).max(1)
    } else {
        populated
    };

    let mut cells = Vec::with_capacity((data_rows + 1) * num_cols);
    for col in 0..num_cols {
        let name = if col == 0 {
            String::new()
        } else {
            chart.series[col - 1].name.clone()
        };
        cells.push(GridCell {
            text: name,
            start_row: 0,
            start_col: col,
            row_span: 1,
            col_span: 1,
            // The series names are the columns' headings, and the categories
            // are what each row is about.
            column_header: true,
            row_header: false,
            pictures: Vec::new(),
            tables: Vec::new(),
        });
    }
    for row in 1..=data_rows {
        let category = chart.categories.get(row - 1).cloned().unwrap_or_default();
        for col in 0..num_cols {
            cells.push(GridCell {
                text: if col == 0 {
                    category.clone()
                } else {
                    pdfextractor_chart::chart_cell_text(
                        chart.series[col - 1].values.get(row - 1).copied().flatten(),
                    )
                },
                start_row: row,
                start_col: col,
                row_span: 1,
                col_span: 1,
                column_header: false,
                row_header: col == 0,
                pictures: Vec::new(),
                tables: Vec::new(),
            });
        }
    }

    Some(GridTable {
        num_rows: data_rows + 1,
        num_cols,
        cells,
    })
}

/// Build a `DoclingDocument` from a PPTX presentation.
///
/// A slide is a page, and the page is the deck's own size (`p:sldSz`) in EMU —
/// the shape Docling's own PPTX backend produces: a `chapter` group per slide
/// with the slide's items beneath it, each carrying the box its shape occupies.
/// A paragraph of a text body is its own item (a title, a list item, or body
/// text), a `graphicFrame` table is one grid with its spans, a picture is the
/// media part its relationship names, and a chart is a picture of its data —
/// its family, the reconstructed grid and its title as the caption. A slide's
/// picture background is the page's `image`, written beside the JSON as the
/// backdrop it is, not a picture item in the slide's group. A hidden slide
/// stays in the document on the `invisible` layer rather than being dropped.
/// `--page-range` selects slides before any item is emitted, so a deck's page
/// number is still the slide's number in the document.
fn convert_pptx(
    path: &Path,
    image_base: Option<&Path>,
    range: Option<&page_range::PageRange>,
    limits: &Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    let parsed =
        pdfextractor_pptx_parser::PptxDocument::parse_bytes_with_limits(&data, limits.clone())?;

    let total = parsed.slides.len();
    let slides: Vec<_> = parsed
        .slides
        .iter()
        .filter(|slide| range.is_none_or(|range| range.contains(slide.number)))
        .collect();
    if let Some(range) = range {
        if slides.is_empty() {
            return Err(PipelineError::PageRange {
                path: path.to_path_buf(),
                range: range.clone(),
                total,
                unit: RangeUnit::Slide,
            }
            .into());
        }
        tracing::info!(
            "{}: reading {} of {} slide(s) ({range})",
            path.display(),
            slides.len(),
            total
        );
    }

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("presentation.pptx");
    // Docling names a deck by its stem and keeps the file name in the origin.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("presentation");

    let mut builder = DocumentBuilder::new(stem);
    builder.set_origin(Origin {
        mimetype: Some(
            "application/vnd.openxmlformats-officedocument.presentationml.presentation".to_string(),
        ),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name.to_string()),
        uri: None,
    });

    let mut image_dir: Option<PathBuf> = None;
    let mut image_seq = 0usize;
    for slide in slides {
        let page_no = slide.number;
        let layer = if slide.hidden { "invisible" } else { "body" };
        // Docling names a slide's group `slide-<index>`, 0-based.
        let group = builder.add_group("chapter", &format!("slide-{}", page_no - 1), layer);
        // The page exists before its image is named, so the backdrop below has
        // a `Page` to belong to.
        builder.add_page(page_no, parsed.width, parsed.height);

        // A picture background is the page's image, not a shape: the slide
        // states it outside its shape tree, and a reader that only walks the
        // tree loses a photographic backdrop whole. It is written beside the
        // JSON like any other picture, then named as this page's image.
        if let Some(background) = &slide.background {
            let width_pt = parsed.width / EMU_PER_POINT;
            let height_pt = parsed.height / EMU_PER_POINT;
            let (mime, data) = office_media(
                background.mime,
                background.data.clone(),
                Some(width_pt),
                Some(height_pt),
                limits,
            );
            let image = image_dimensions(&data).and_then(|(width, height)| {
                let uri = write_image(
                    &mut image_dir,
                    image_base,
                    &format!("slide{page_no}_background"),
                    mime,
                    &data,
                )?;
                Some(PictureData {
                    mimetype: mime.to_string(),
                    width: Some(width),
                    height: Some(height),
                    // The backdrop is stretched across the slide; its effective
                    // resolution is the pixel width across the page's width.
                    dpi: effective_dpi(width, width_pt),
                    uri,
                })
            });
            if let Some(image) = image {
                builder.set_page_image(page_no, image);
            }
        }

        for shape in &slide.shapes {
            match shape {
                pdfextractor_pptx_parser::Shape::Text(text) => {
                    for paragraph in &text.paragraphs {
                        // An empty paragraph is a blank line, not content.
                        if paragraph.text.trim().is_empty() {
                            continue;
                        }
                        let label = match &paragraph.list {
                            Some(list) => TextLabel {
                                label: "list_item".to_string(),
                                marker: Some(list.marker.clone()),
                                enumerated: Some(list.enumerated),
                                ..Default::default()
                            },
                            None if text.title => TextLabel {
                                label: "title".to_string(),
                                ..Default::default()
                            },
                            None => TextLabel {
                                label: "paragraph".to_string(),
                                ..Default::default()
                            },
                        };
                        builder.add_text_item_in(
                            &paragraph.text,
                            page_no,
                            text.bbox,
                            label,
                            &group,
                        );
                    }
                }
                pdfextractor_pptx_parser::Shape::Table(table) => {
                    let grid = GridTable {
                        num_rows: table.table.num_rows,
                        num_cols: table.table.num_cols,
                        cells: table
                            .table
                            .cells
                            .iter()
                            .map(|cell| GridCell {
                                text: cell.text.clone(),
                                start_row: cell.row,
                                start_col: cell.col,
                                row_span: cell.row_span,
                                col_span: cell.col_span,
                                // A table's first row is the header a reader
                                // takes it for; a slide has no row headers.
                                column_header: cell.row == 0,
                                row_header: false,
                                // A slide's table cell holds text; a picture on
                                // the slide is a shape of its own.
                                pictures: Vec::new(),
                                tables: Vec::new(),
                            })
                            .collect(),
                    };
                    builder.add_table_grid_in(&grid, page_no, table.bbox, &group);
                }
                pdfextractor_pptx_parser::Shape::Chart(chart) => {
                    // A chart is a picture, and the chart's own title is the
                    // picture's caption: a `caption` item on the slide, which
                    // the picture points back at.
                    let caption = chart.title.as_deref().map(|title| {
                        builder.add_text_item_in(
                            title,
                            page_no,
                            chart.bbox,
                            TextLabel {
                                label: "caption".to_string(),
                                ..Default::default()
                            },
                            &group,
                        )
                    });
                    let table = chart_table(chart);
                    builder.add_chart_picture_in(
                        page_no,
                        chart.bbox,
                        chart.kind.class_name(),
                        table.as_ref(),
                        caption.as_ref(),
                        &group,
                    );
                }
                pdfextractor_pptx_parser::Shape::Picture(picture) => {
                    image_seq += 1;
                    // A slide's picture is handed on as PNG unless it is a PNG
                    // or a JPEG, which is the shape Docling's own deck reader
                    // leaves it in; a format whose header this does not read is
                    // still placed on the slide, with its box and no image
                    // rather than invented numbers.
                    let points = (picture.bbox[2] - picture.bbox[0]) / EMU_PER_POINT;
                    let height = (picture.bbox[3] - picture.bbox[1]) / EMU_PER_POINT;
                    let (mime, data) = office_media(
                        picture.mime,
                        picture.data.clone(),
                        Some(points),
                        Some(height),
                        limits,
                    );
                    let image = image_dimensions(&data).and_then(|(width, height)| {
                        let uri = write_image(
                            &mut image_dir,
                            image_base,
                            &format!("slide{page_no}_{image_seq}"),
                            mime,
                            &data,
                        )?;
                        Some(PictureData {
                            mimetype: mime.to_string(),
                            width: Some(width),
                            height: Some(height),
                            dpi: effective_dpi(width, points),
                            uri,
                        })
                    });
                    builder.add_picture_in(page_no, picture.bbox, "picture", image, &group);
                }
            }
        }

        // The deck's own notes about the slide: speaker notes first, then its
        // comments, each comment in a `comment_section` group of its own under
        // the body — the order and the shape Docling's own reading uses.
        if let Some(notes) = &slide.notes {
            builder.add_notes_item_in(notes, page_no, [0.0, 0.0, 0.0, 0.0], &group);
        }
        for comment in &slide.comments {
            let comment_group =
                builder.add_comment_group(&format!("comment-slide{}-{}", page_no, comment.index));
            builder.add_comment_in(&comment_text(comment), &comment_group);
        }
    }

    Ok(builder.build())
}

/// Build a `DoclingDocument` from an ODP presentation.
///
/// A slide is a page, as it is for PPTX, and the page is the slide size in EMU.
/// Each slide is a `chapter` group holding its text paragraphs, table and
/// pictures; a slide's speaker notes are one item on the notes layer inside the
/// same group. The regular shapes a slide inherits from its master page are
/// emitted with the slide's own, in master-page order. A slide's transition is
/// one notes-layer item, and each animation effect is another, naming its
/// target shape, property and timing. A hidden slide stays on the `invisible`
/// layer rather than being dropped.
/// `--page-range` selects slides before any item is emitted, so a deck's page
/// number is still the slide's number in the document.
fn convert_odp(
    path: &Path,
    image_base: Option<&Path>,
    range: Option<&page_range::PageRange>,
    limits: &Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    let parsed =
        pdfextractor_odp_parser::OdpDocument::parse_bytes_with_limits(&data, limits.clone())?;

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("presentation.odp");
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("presentation");

    let total = parsed.slides.len();
    let slides: Vec<&pdfextractor_odp_parser::Slide> = parsed
        .slides
        .iter()
        .filter(|slide| range.is_none_or(|range| range.contains(slide.number)))
        .collect();
    if let Some(range) = range {
        if slides.is_empty() {
            return Err(PipelineError::PageRange {
                path: path.to_path_buf(),
                range: range.clone(),
                total,
                unit: RangeUnit::Slide,
            }
            .into());
        }
        tracing::info!(
            "{}: reading {} of {} slide(s) ({range})",
            path.display(),
            slides.len(),
            total
        );
    }

    let mut builder = DocumentBuilder::new(stem);
    builder.set_origin(Origin {
        mimetype: Some("application/vnd.oasis.opendocument.presentation".to_string()),
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name.to_string()),
        uri: None,
    });

    let mut image_dir: Option<PathBuf> = None;
    let mut image_seq = 0usize;
    for slide in slides {
        let page_no = slide.number;
        let layer = if slide.hidden { "invisible" } else { "body" };
        let group = builder.add_group("chapter", &format!("slide-{}", page_no - 1), layer);
        builder.add_page(page_no, parsed.width, parsed.height);

        for shape in slide.background.iter().chain(slide.shapes.iter()) {
            match shape {
                pdfextractor_odp_parser::Shape::Text(text) => {
                    for paragraph in &text.paragraphs {
                        if paragraph.text.trim().is_empty() {
                            continue;
                        }
                        let label = if paragraph.list {
                            TextLabel {
                                label: "list_item".to_string(),
                                level: paragraph.level,
                                ..Default::default()
                            }
                        } else if text.title {
                            TextLabel {
                                label: "title".to_string(),
                                ..Default::default()
                            }
                        } else {
                            TextLabel {
                                label: "paragraph".to_string(),
                                ..Default::default()
                            }
                        };
                        builder.add_text_item_in(
                            &paragraph.text,
                            page_no,
                            text.bbox,
                            label,
                            &group,
                        );
                    }
                }
                pdfextractor_odp_parser::Shape::Table(table) => {
                    let grid = GridTable {
                        num_rows: table.table.num_rows,
                        num_cols: table.table.num_cols,
                        cells: table
                            .table
                            .cells
                            .iter()
                            .map(|cell| GridCell {
                                text: cell.text.clone(),
                                start_row: cell.row,
                                start_col: cell.col,
                                row_span: cell.row_span,
                                col_span: cell.col_span,
                                column_header: cell.row == 0,
                                row_header: false,
                                pictures: Vec::new(),
                                tables: Vec::new(),
                            })
                            .collect(),
                    };
                    builder.add_table_grid_in(&grid, page_no, table.bbox, &group);
                }
                pdfextractor_odp_parser::Shape::Chart(chart) => {
                    // A chart is a picture of its data, and its own title is
                    // the picture's caption — the same shape a PPTX slide's
                    // chart takes.
                    let caption = chart.title.as_deref().map(|title| {
                        builder.add_text_item_in(
                            title,
                            page_no,
                            chart.bbox,
                            TextLabel {
                                label: "caption".to_string(),
                                ..Default::default()
                            },
                            &group,
                        )
                    });
                    let table = chart_table(chart);
                    builder.add_chart_picture_in(
                        page_no,
                        chart.bbox,
                        chart.kind.class_name(),
                        table.as_ref(),
                        caption.as_ref(),
                        &group,
                    );
                }
                pdfextractor_odp_parser::Shape::Picture(picture) => {
                    image_seq += 1;
                    let points = (picture.bbox[2] - picture.bbox[0]) / EMU_PER_POINT;
                    let height = (picture.bbox[3] - picture.bbox[1]) / EMU_PER_POINT;
                    let (mime, data) = office_media(
                        picture.mime,
                        picture.data.clone(),
                        Some(points),
                        Some(height),
                        limits,
                    );
                    let image = image_dimensions(&data).and_then(|(width, height)| {
                        let uri = write_image(
                            &mut image_dir,
                            image_base,
                            &format!("slide{page_no}_{image_seq}"),
                            mime,
                            &data,
                        )?;
                        Some(PictureData {
                            mimetype: mime.to_string(),
                            width: Some(width),
                            height: Some(height),
                            dpi: effective_dpi(width, points),
                            uri,
                        })
                    });
                    builder.add_picture_in(page_no, picture.bbox, "picture", image, &group);
                }
            }
        }

        if let Some(notes) = &slide.notes {
            builder.add_notes_item_in(notes, page_no, [0.0, 0.0, 0.0, 0.0], &group);
        }
        // A transition changes the slide before its shape effects run. Docling
        // has no transition item either, so it is the first notes-layer item.
        if let Some(transition) = &slide.transition {
            builder.add_notes_item_in(
                &transition_text(transition),
                page_no,
                [0.0, 0.0, 0.0, 0.0],
                &group,
            );
        }
        // An animation has no Docling item of its own, so each effect is one
        // notes-layer item: the slide's timing stays in the document without
        // being mistaken for words the slide shows.
        for animation in &slide.animations {
            builder.add_notes_item_in(
                &animation_text(animation),
                page_no,
                [0.0, 0.0, 0.0, 0.0],
                &group,
            );
        }
        // Comments are the slide's own remarks, not places on it: each is one
        // item on the notes layer, in a `comment_section` group of its own.
        for comment in &slide.comments {
            let comment_group =
                builder.add_comment_group(&format!("comment-slide{}-{}", page_no, comment.index));
            builder.add_comment_in(
                &comment_text_parts(
                    comment.author.as_deref(),
                    None,
                    comment.date.as_deref(),
                    &comment.text,
                ),
                &comment_group,
            );
        }
    }

    Ok(builder.build())
}

/// One slide transition as the document records it: Docling has no transition
/// item either, so the effect and its settings become one notes-layer item.
fn transition_text(transition: &pdfextractor_odp_parser::Transition) -> String {
    let mut text = format!(
        "Transition: {}",
        transition.effect.as_deref().unwrap_or("unspecified")
    );
    if let Some(subtype) = &transition.subtype {
        text.push(' ');
        text.push_str(subtype);
    }
    let mut details = Vec::new();
    if let Some(direction) = &transition.direction {
        details.push(format!("direction {direction}"));
    }
    if let Some(speed) = &transition.speed {
        details.push(format!("speed {speed}"));
    }
    if let Some(duration) = &transition.duration {
        details.push(format!("duration {duration}"));
    }
    if !details.is_empty() {
        text.push_str(&format!(" ({})", details.join(", ")));
    }
    text
}

/// One animation effect as the document records it: Docling has no animation
/// item, so each effect is a notes-layer item naming the shape, the property
/// and the timing.
fn animation_text(animation: &pdfextractor_odp_parser::Animation) -> String {
    let mut text = format!("Animation: {}", animation.effect);
    if let Some(attribute) = &animation.attribute {
        text.push(' ');
        text.push_str(attribute);
    }
    if let Some(from) = &animation.from {
        text.push_str(&format!(" from {from}"));
    }
    if let Some(to) = &animation.to {
        text.push_str(&format!(" to {to}"));
    }
    if let Some(target) = &animation.target {
        text.push_str(&format!(" on \"{target}\""));
    }
    match (&animation.preset_class, &animation.preset_id) {
        (Some(class), Some(id)) => text.push_str(&format!(" [preset {class}:{id}]")),
        (Some(class), None) => text.push_str(&format!(" [preset {class}]")),
        (None, Some(id)) => text.push_str(&format!(" [preset {id}]")),
        (None, None) => {}
    }
    let mut timing = Vec::new();
    if let Some(begin) = &animation.begin {
        timing.push(format!("begin {begin}"));
    }
    if let Some(duration) = &animation.duration {
        timing.push(format!("duration {duration}"));
    }
    if !timing.is_empty() {
        text.push_str(&format!(" ({})", timing.join(", ")));
    }
    text
}

/// A comment as the document records it: Docling's own reading of a PPTX
/// comment, which is the comment's words prefixed with what the deck says about
/// it — `[author: Ada Lovelace (AL), time: 2026-10-18T09:15:00.000]: …` — or
/// just the words when the file names no author and no time. Both readings have
/// to record the same string, so the composition follows its own: an author's
/// initials are part of the name's parentheses, a blank one is not written at
/// all, and the prefix is left off entirely when there is nothing to say. The
/// time is written at the millisecond precision PowerPoint itself uses (see
/// [`comment_timestamp`]), so the same deck bridged through LibreOffice says
/// what the native one does.
fn comment_text(comment: &pdfextractor_pptx_parser::Comment) -> String {
    comment_text_parts(
        comment.author.as_deref(),
        comment.initials.as_deref(),
        comment.date.as_deref(),
        &comment.text,
    )
}

/// The composition both deck formats use. An ODP comment has no initials, so
/// its author is written without them.
fn comment_text_parts(
    author: Option<&str>,
    initials: Option<&str>,
    date: Option<&str>,
    text: &str,
) -> String {
    let mut parts = Vec::new();
    // A name the file left blank is no name: the author's whole field is left
    // out, initials and all, which is where Docling draws the same line.
    if let Some(author) = author.filter(|name| !name.is_empty()) {
        let mut named = format!("author: {author}");
        if let Some(initials) = initials.filter(|initials| !initials.is_empty()) {
            named.push_str(&format!(" ({initials})"));
        }
        parts.push(named);
    }
    if let Some(date) = date.filter(|date| !date.is_empty()) {
        parts.push(format!("time: {}", comment_timestamp(date)));
    }
    if parts.is_empty() {
        text.to_string()
    } else {
        format!("[{}]: {text}", parts.join(", "))
    }
}

/// A comment's `p:cm/@dt`, cut to the precision the notes text writes.
///
/// PowerPoint spells fractional seconds with three digits (`.000`), and
/// LibreOffice's bridge writes the same instant with nine (`.000000000`). The
/// two files name the same moment, so the note must say the same thing: a
/// legacy deck and its native original would otherwise differ in the notes
/// layer for no reason but which program saved the package. The fraction is
/// therefore padded or cut to three digits; a timestamp written without one is
/// left exactly as the file wrote it, which is also how Docling records such a
/// file, and anything that is not a time (no seconds field before the dot) is
/// not touched at all.
fn comment_timestamp(date: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;

    let Some(dot) = date.find('.') else {
        return Cow::Borrowed(date);
    };
    // The fraction follows a two-digit seconds field: `…T09:15:00.000`.
    let Some(colon) = date[..dot].rfind(':') else {
        return Cow::Borrowed(date);
    };
    let seconds = &date[colon + 1..dot];
    if seconds.len() != 2 || !seconds.bytes().all(|byte| byte.is_ascii_digit()) {
        return Cow::Borrowed(date);
    }
    let fraction = &date[dot + 1..];
    let digits = fraction
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return Cow::Borrowed(date);
    }

    // Pad a short fraction (`.5` is half a second) and cut a long one; a zone
    // after it (`Z`, `+00:00`) is the file's and stays as written.
    let milliseconds = digits.min(3);
    let mut text = String::with_capacity(date.len());
    text.push_str(&date[..dot + 1]);
    text.push_str(&fraction[..milliseconds]);
    text.extend(std::iter::repeat_n('0', 3 - milliseconds));
    text.push_str(&fraction[digits..]);
    if text == date {
        Cow::Borrowed(date)
    } else {
        Cow::Owned(text)
    }
}

/// Build a `DoclingDocument` from one plain image.
///
/// The shape Docling's own image backend produces: one page, sized by the
/// pixel grid at the resolution the file declares (72 dpi when it declares
/// none). An image has no text layer, so every text item comes from OCR — the
/// lines are mapped from pixels into the page's points, grouped by the same
/// layout pass a scanned PDF goes through, and emitted as the blocks they
/// form. `--ocr never` still writes the page, with no text on it, because
/// that is what the flag asks for.
///
/// A GIF or a WebP may hold an animation, and every frame of it is a page a
/// reader turns: the file is the whole strip, not its first picture, so the
/// document has one page per frame. Each page is the logical screen or `ANIM`
/// canvas the header states — the canvas every frame is drawn on — and each
/// frame is the pixels of its page.
///
/// A TIFF holds a chain of image file directories, which is how a scanner
/// writes a batch, and each of those is a page too. Unlike an animation's
/// frames, the sheets need not share a grid: a page is measured from its own
/// pixels at the resolution the file declares, and recognised on its own.
fn convert_image(
    path: &Path,
    page_range: Option<&page_range::PageRange>,
    despeckle: bool,
    limits: &Limits,
    mut engine: Option<&mut OcrEngine>,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    let image = pdfextractor_image_parser::ImageDocument::parse_bytes(&data)?;

    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("image");
    // Docling names an image by its stem and keeps the file name in the
    // origin, as it does for a workbook or a deck.
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image");

    let mut builder = DocumentBuilder::new(stem);
    builder.set_origin(Origin {
        mimetype: Some(image.mimetype.to_string()),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name.to_string()),
        uri: None,
    });

    // Every page the file holds. A GIF's or a WebP's frames are all drawn on
    // the canvas the header describes, so they take the page it states; a
    // TIFF's directories are separate sheets that need not be the same size, so
    // each page is measured from its own grid. One frame is a still image, so
    // only a file that really holds several is read as several pages.
    let gif_frames = pdfextractor_image_parser::gif_frames_checked(&data, limits)?;
    let webp_frames = pdfextractor_image_parser::webp_frames_checked(&data, limits)?;
    let tiff_pages = pdfextractor_image_parser::tiff_frames_checked(&data, limits)?;
    let animation: &[pdfextractor_image_parser::PngPicture] = if gif_frames.len() > 1 {
        &gif_frames
    } else if webp_frames.len() > 1 {
        &webp_frames
    } else {
        &[]
    };
    let animated = !animation.is_empty();
    let tiff_stack = tiff_pages.len() > 1;
    // The pages after the first, whichever kind of stack the file is.
    let later: &[pdfextractor_image_parser::PngPicture] = if tiff_stack {
        &tiff_pages[1..]
    } else {
        animation.get(1..).unwrap_or_default()
    };

    // The engine is the worker's, lent by `build_document`; a run without
    // recognition keeps it `None` and still has pages to write.

    // A JPEG 2000 or a GIF is decoded here rather than by Tesseract's loader.
    // The pixels are the reader's own decode — the one a picture in either
    // encoding already goes through when a document carries it — so recognition
    // does not depend on whether the installed leptonica was built with OpenJPEG
    // or giflib, and the warning that loader prints is not raised for every
    // scan. The page grid stays the header's (`jp2`'s `ihdr`, the GIF's logical
    // screen). An animation's frames are decoded already, on that screen, so its
    // first page is the first of them.
    let decoded = if animated {
        Some(animation[0].data.clone())
    } else if matches!(image.mimetype, "image/jp2" | "image/gif") {
        let decoded = pdfextractor_image_parser::transcode_to_png_checked(&data, limits)
            .ok()
            .map(|picture| picture.data);
        if decoded.is_none() {
            tracing::warn!(
                "could not decode {}; OCR sees the stored bytes",
                path.display()
            );
        }
        decoded
    } else {
        None
    };
    let source = decoded.as_deref().unwrap_or(&data);

    // A file with no EXIF tag says nothing about which way up its pixels are: a
    // scanner, or an editor that stripped the metadata, leaves a sideways page
    // with no declaration at all. Tesseract reads the pixels as stored, so its
    // own reading of the page's turns decides which one it is written in.
    let declared = image.orientation;
    let detected = if declared == pdfextractor_image_parser::Orientation::Normal {
        match engine
            .as_mut()
            .map(|engine| engine.detect_upright(source, limits))
        {
            Some(Ok(turn)) => turn,
            Some(Err(err)) => {
                tracing::debug!(
                    "could not find the orientation of {}: {err}",
                    path.display()
                );
                QuarterTurn::AsStored
            }
            // Nothing will be recognised, so which way up the page is does not
            // matter; it keeps the pixels it stores.
            None => QuarterTurn::AsStored,
        }
    } else {
        QuarterTurn::AsStored
    };

    // The page is the upright grid; Tesseract reads the stored one. A camera
    // that was held sideways is turned upright here, once, before recognition —
    // by EXIF when there is a tag, by the reading when there is not.
    let mut upright = None;
    if declared != pdfextractor_image_parser::Orientation::Normal {
        match orient_for_ocr(source, declared, limits) {
            Ok(png) => upright = Some(png),
            Err(refusal) => tracing::warn!(
                "could not apply EXIF orientation {:?} to {}; OCR sees the stored pixels: {refusal}",
                declared,
                path.display(),
            ),
        }
    }
    let mut applied = QuarterTurn::AsStored;
    if declared == pdfextractor_image_parser::Orientation::Normal
        && detected != QuarterTurn::AsStored
    {
        match rotate_for_ocr_checked(source, detected.degrees(), limits) {
            Ok((png, _, _)) => {
                upright = Some(png);
                applied = detected;
            }
            Err(refusal) => tracing::warn!(
                "could not turn {} upright; OCR sees the stored pixels: {refusal}",
                path.display(),
            ),
        }
    }

    // A quarter turn makes the stored height the page's width, at the
    // resolution measured along that axis — the same swap `Rotate90` makes.
    let (width, height) = if applied.swaps_axes() {
        let turned = pdfextractor_image_parser::ImageDocument {
            orientation: pdfextractor_image_parser::Orientation::Rotate90,
            ..image
        };
        (turned.width_pt(), turned.height_pt())
    } else {
        (image.width_pt(), image.height_pt())
    };

    // The pages the file is: the still, or the first frame of an animation, and
    // then every later frame. An animated WebP's EXIF tag states how the whole
    // strip was stored, and a GIF states no orientation of its own; either way a
    // file that had to be turned to be read is turned throughout, because its
    // frames are one strip of the same picture. A TIFF's directories are not one
    // strip, though: each sheet was already turned by its own directory's tag in
    // `tiff_frames`, so the first sheet's turn must not be applied to the rest
    // again.
    //
    // Each page carries the grid the engine will see, because a TIFF's sheets
    // need not share one: the first page's is the header's, a later frame's its
    // own — swapped when the frame had to be turned.
    let first_grid = if applied.swaps_axes() {
        (image.height, image.width)
    } else {
        image.upright_size()
    };
    let mut page_pixels: Vec<(i32, Vec<u8>, u32, u32, f64, f64)> =
        Vec::with_capacity(later.len() + 1);
    page_pixels.push((
        1,
        upright.as_deref().unwrap_or(source).to_vec(),
        first_grid.0,
        first_grid.1,
        width,
        height,
    ));
    for (index, frame) in later.iter().enumerate() {
        let (bytes, grid) = if tiff_stack {
            // Each directory's own tag was already applied to its pixels.
            (frame.data.clone(), (frame.width, frame.height))
        } else {
            // An EXIF tag is a statement about the file, not about its first
            // page: every frame of an animation was stored the same way, so
            // every frame is turned by it. Only a WebP carries such a tag and
            // an animation together; a GIF has none, and when the first page
            // had to be turned to be read, the strip is turned with it.
            let (turned, swaps) = if declared != pdfextractor_image_parser::Orientation::Normal {
                (
                    orient_for_ocr(&frame.data, declared, limits).ok(),
                    declared.swaps_axes(),
                )
            } else if applied == QuarterTurn::AsStored {
                (None, false)
            } else {
                (
                    rotate_for_ocr(&frame.data, applied.degrees(), limits).map(|(png, _, _)| png),
                    applied.swaps_axes(),
                )
            };
            match turned {
                Some(png) if swaps => (png, (frame.height, frame.width)),
                Some(png) => (png, (frame.width, frame.height)),
                None => (frame.data.clone(), (frame.width, frame.height)),
            }
        };
        let (width, height) = page_size(&image, grid, false);
        page_pixels.push(((index + 2) as i32, bytes, grid.0, grid.1, width, height));
    }

    // The frames are the file's pages, so the range selects among them before
    // one is recognised: page 2 of an animation is its second frame, and a
    // TIFF's second directory its second sheet.
    if let Some(range) = page_range {
        let total = page_pixels.len();
        page_pixels.retain(|page| range.contains(page.0));
        if page_pixels.is_empty() {
            return Err(PipelineError::PageRange {
                path: path.to_path_buf(),
                range: range.clone(),
                total,
                unit: RangeUnit::ImagePage,
            }
            .into());
        }
        // A GIF's or a WebP's pages are frames, and a TIFF's are the directories
        // its reader counted; the log names the unit the file holds rather than
        // calling every one of them a page.
        let (unit, units) = if tiff_stack {
            ("directory", "directories")
        } else if animated {
            ("frame", "frames")
        } else {
            ("page", "pages")
        };
        tracing::info!(
            "{}: reading {} of {} {} ({range})",
            path.display(),
            page_pixels.len(),
            total,
            if total == 1 { unit } else { units }
        );
    }

    for page in &page_pixels {
        builder.add_page(page.0, page.4, page.5);
    }

    let Some(engine) = engine else {
        // Without recognition the pages are still the file's: one per frame or
        // directory, each at its own size — and only the ones asked for.
        return Ok(builder.build());
    };

    // The engine measures in pixels; the page is measured in points. A line's
    // box is scaled onto the page, and its height comes back as a font size —
    // the same estimate the PDF path uses for an OCR line. Each page is scaled
    // against its own grid: a GIF's frames share its logical screen, a TIFF's
    // sheets are each their own size.
    // Frames are recognised one at a time; the deadline is checked before each,
    // so a stack of them cannot run past the OCR budget.
    let ocr_deadline = limits.ocr_deadline();
    for (page_no, pixels, pixel_width, pixel_height, width, height) in &page_pixels {
        ocr_deadline.check("recognising a page")?;
        let page = *page_no;
        let (pixel_width, pixel_height) = (*pixel_width, *pixel_height);
        let (width, height) = (*width, *height);
        // A faxed scan loses its isolated pixels before recognition. The pass
        // keeps the grid, so the line boxes scale onto the page exactly as they
        // would have without it.
        let prepared = if despeckle {
            match pdfextractor_ocr_engine::preprocess::despeckle_page(pixels.as_slice(), limits) {
                Ok(png) => Some(png),
                Err(err) => {
                    tracing::debug!(
                        "could not preprocess {} (page {page}): {err}",
                        path.display()
                    );
                    None
                }
            }
        } else {
            None
        };
        let pixels = prepared.as_deref().unwrap_or(pixels.as_slice());
        let result = match engine.recognize(pixels) {
            Ok(result) => result,
            Err(err) => {
                // A page that cannot be read is a page with no text, not a
                // document that stops at it: the frames after it are still
                // pages a reader turns.
                tracing::warn!("OCR failed for {} (page {page}): {err}", path.display());
                continue;
            }
        };
        if result.lines.is_empty() {
            tracing::debug!("OCR found no text in {} (page {page})", path.display());
            continue;
        }

        let spans: Vec<LayoutSpan> = result
            .lines
            .iter()
            .map(|line| {
                let bbox = scale_ocr_bbox(
                    line.bbox,
                    pixel_width,
                    pixel_height,
                    [0.0, 0.0, width, height],
                );
                LayoutSpan {
                    text: line.text.clone(),
                    bbox,
                    rotated: false,
                    font_size: ((bbox[3] - bbox[1]) * 0.8).clamp(4.0, 400.0),
                }
            })
            .collect();
        for element in pdfextractor_layout::analyze_page(&spans, [width, height], &[], &[], page) {
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
                    builder.add_text_item(
                        &text,
                        page,
                        bbox,
                        TextLabel {
                            label,
                            level,
                            enumerated: enumerated(&marker),
                            marker,
                            hyperlink: None,
                        },
                    );
                }
                // Layout only places a table where the OCR lines line up as
                // one; there are no picture boxes to place, so no picture is
                // emitted.
                pdfextractor_layout::Element::Table { rows, bbox } => {
                    builder.add_table_item(&rows, page, bbox);
                }
                pdfextractor_layout::Element::Picture { .. } => {}
            }
        }
    }

    Ok(builder.build())
}

/// Build a `DoclingDocument` from a legacy `.doc` file path.
fn convert_doc(
    path: &Path,
    _image_base: Option<&Path>,
    limits: &Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    let parsed =
        pdfextractor_doc_parser::DocDocument::parse_bytes_with_limits(&data, limits.clone())?;

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
    image_base: Option<&Path>,
    limits: &Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = read_input(path, limits)?;
    // A flow file is untrusted input like any other: the shared budgets bound
    // the blocks it may expand into and the time the walk through it may take.
    let parsed = match ext {
        "rtf" => pdfextractor_text_parser::rtf::parse_with_limits(&data, limits)?,
        "html" | "htm" => pdfextractor_text_parser::html::parse_with_limits(&data, limits)?,
        "md" | "markdown" => {
            pdfextractor_text_parser::text::parse_markdown_with_limits(&data, limits)?
        }
        "csv" => pdfextractor_text_parser::csv::parse_csv_with_limits(&data, limits)?,
        "tsv" => pdfextractor_text_parser::csv::parse_tsv_with_limits(&data, limits)?,
        _ => pdfextractor_text_parser::text::parse_plain_with_limits(&data, limits)?,
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
    // What a page says about itself — its description, the URL it calls
    // canonical, and the Open Graph and Twitter card statements behind its link
    // preview — travels beside the model, because Docling has no field for any
    // of it: the JSON and YAML therefore do not carry it. The HTML writer puts
    // it back in the head of the page it writes, and a Markdown page states it
    // as YAML front matter, so both round trips keep it. A format with neither
    // a head nor front matter (plain text, CSV, RTF) states none of it.
    builder.set_html_head(pdfextractor_core::HtmlHead {
        description: parsed.description.clone(),
        canonical: parsed.canonical.clone(),
        social: parsed.social.clone(),
    });
    // A flow-based format has no page rectangle, so the document reports no
    // pages, exactly as Docling's own output for one does.

    let mut image_dir: Option<PathBuf> = None;
    let mut image_seq = 0usize;
    // The groups the Markdown marked: a slide's `chapter`, a comment's
    // `comment_section`. An item belongs to the innermost one, which is what
    // keeps a slide's words and its notes together — and a comment's words in
    // the comment's own section — instead of all of it at the top of the body.
    let mut groups: Vec<(Ref, String)> = Vec::new();
    // A `**Comment:**` quote outside any marked group still says what it is.
    // The model holds a comment in a `comment_section` group, so one is made
    // for it; its name only has to be stable enough to read back.
    let mut loose_comments = 0usize;
    for block in &parsed.items {
        match block {
            pdfextractor_text_parser::Block::GroupStart {
                label,
                name,
                content_layer,
            } => {
                let group = builder.add_group(label, name, content_layer);
                groups.push((group, label.clone()));
            }
            // A closing marker with no open group is ignored: the writer never
            // writes one, and a hand-edited file must not fail the conversion.
            pdfextractor_text_parser::Block::GroupEnd => {
                groups.pop();
            }
            // A quotation is a paragraph with the one extra fact that the
            // document set it apart: no Docling label says so, so it is placed
            // as the paragraph it is and the flag beside it is what the Markdown
            // and HTML writers put the `>` or the `<blockquote>` back from.
            pdfextractor_text_parser::Block::Text {
                quote: true,
                text,
                hyperlink,
                ..
            } => match groups.last() {
                Some((group, _)) => {
                    builder.add_quotation_in(text, hyperlink.clone(), group);
                }
                None => {
                    builder.add_quotation(text, hyperlink.clone());
                }
            },
            pdfextractor_text_parser::Block::Text {
                quote: false,
                text,
                label,
                level,
                marker,
                hyperlink,
            } => match groups.last() {
                Some((group, _)) => {
                    builder.add_labeled_item_in(
                        text,
                        TextLabel {
                            label: label.clone(),
                            level: *level,
                            marker: marker.clone(),
                            enumerated: enumerated(marker),
                            hyperlink: hyperlink.clone(),
                        },
                        group,
                    );
                }
                None => {
                    builder.add_labeled_item_with_link(
                        text,
                        label,
                        *level,
                        marker.clone(),
                        enumerated(marker),
                        hyperlink.clone(),
                    );
                }
            },
            pdfextractor_text_parser::Block::Note {
                text,
                hyperlink,
                comment,
            } => match groups.last() {
                // A comment is a section of its own on the notes layer, added
                // the way the PPTX reader adds one.
                Some((group, label)) if label == "comment_section" => {
                    builder.add_comment_in_with_link(text, hyperlink.clone(), group);
                }
                // A comment marker inside a slide's group cannot be kept: the
                // slide is not a comment section, and the model has nowhere
                // else to put it. It stays that slide's note.
                Some((group, _)) => {
                    builder.add_notes_text_in_with_link(text, hyperlink.clone(), group);
                }
                None if *comment => {
                    let group = builder.add_comment_group(&format!("comment-{loose_comments}"));
                    loose_comments += 1;
                    builder.add_comment_in_with_link(text, hyperlink.clone(), &group);
                }
                None => {
                    builder.add_notes_text_with_link(text, hyperlink.clone());
                }
            },
            pdfextractor_text_parser::Block::Table { grid } => {
                // The grid arrives with its merges placed, so a `colspan` a page
                // declared is still a spanning cell here instead of becoming an
                // empty slot beside a cell that only looks wider.
                match groups.last() {
                    Some((group, _)) => {
                        builder.add_table_grid_flow_in(&flow_grid(grid), group);
                    }
                    None => {
                        builder.add_table_grid(&flow_grid(grid));
                    }
                }
            }
            pdfextractor_text_parser::Block::PicturePlaceholder => {
                // The page marked a spot where a picture stood and carried
                // nothing of it, so the picture is the item and no image is
                // behind it — the picture a chart or a vector drawing is, one
                // the model holds and no file answers for. That is what makes
                // a page written in `placeholder` mode come back as a picture:
                // read as words, `<!-- image -->` would be prose, and the next
                // render would have prose where a picture was.
                match groups.last() {
                    Some((group, _)) => {
                        builder.add_picture_plain_in("picture", None, group);
                    }
                    None => {
                        builder.add_picture_plain("picture", None);
                    }
                }
            }
            pdfextractor_text_parser::Block::Picture { alt, source } => {
                // A flow picture names either a file beside the document or,
                // when the page was rendered embedded, the bytes themselves in
                // a `data:` URI. Both are read and copied beside the output, so
                // an embedded render round-trips instead of becoming the words
                // it was written as. A source this cannot read — a remote URL,
                // a `data:` URI that is not a carried picture, a path that is
                // not there — is left as the words the file contains rather
                // than becoming a picture with nothing behind it.
                let picture = load_flow_picture(path, source, limits).and_then(|picture| {
                    // A picture the page named beside itself keeps that name, so
                    // the page written from this document names the same file
                    // and the page is its own bytes when it is read back. One
                    // the page carried (a `data:` URI) or named in a way this
                    // cannot spell is written under a name of this run's, the
                    // way every other format's pictures are.
                    let uri = match kept_picture_path(source, picture.mimetype) {
                        Some(relative) => write_kept_image(image_base, &relative, &picture.data),
                        None => {
                            image_seq += 1;
                            write_image(
                                &mut image_dir,
                                image_base,
                                &format!("img{image_seq}"),
                                picture.mimetype,
                                &picture.data,
                            )
                        }
                    }?;
                    Some(PictureData {
                        mimetype: picture.mimetype.to_string(),
                        width: picture.size.map(|size| size.0),
                        height: picture.size.map(|size| size.1),
                        // A flow format has no placement box to derive an
                        // effective resolution from.
                        dpi: None,
                        uri,
                    })
                });
                match picture {
                    Some(image) => match groups.last() {
                        Some((group, _)) => {
                            builder.add_picture_plain_in("picture", Some(image), group);
                        }
                        None => {
                            builder.add_picture_plain("picture", Some(image));
                        }
                    },
                    None => {
                        tracing::debug!("{}: leaving image {} as written", path.display(), source);
                        let words = markdown_image(alt, source);
                        match groups.last() {
                            Some((group, _)) => {
                                builder.add_labeled_item_in(
                                    &words,
                                    TextLabel {
                                        label: "paragraph".to_string(),
                                        ..Default::default()
                                    },
                                    group,
                                );
                            }
                            None => {
                                builder.add_labeled_text(&words, "paragraph");
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(builder.build())
}

/// A picture a flow document points at or carries, ready to be written out.
struct FlowPicture {
    mimetype: &'static str,
    data: Vec<u8>,
    /// The upright pixel grid, when the format declares one.
    size: Option<(u32, u32)>,
}

/// Load the picture a flow document's `Picture` block names — the same block
/// whether a Markdown `![alt](source)` or an HTML `<img src>` produced it.
///
/// `source` is a `data:` URI, whose bytes are already in the document, or a path
/// resolved against the document's own directory, because that is what a
/// relative destination means in both formats. Only a local file is read: a
/// converter must not fetch the network on its own.
fn load_flow_picture(document: &Path, source: &str, limits: &Limits) -> Option<FlowPicture> {
    let (declared, data) = match data_uri_picture(source) {
        Some(carried) => carried,
        None => {
            let (path, file) = open_local_image(document, source)?;
            let data = read_open_input(&file, &path, limits).ok()?;
            (image_mimetype_for_path(&path), data)
        }
    };
    picture_from_bytes(declared, data, limits)
}

/// The bytes a `data:` URI carries and the media type it declares, when that
/// type is one this carries.
///
/// `data:image/png;base64,…` is the form both writers put a picture's own bytes
/// in for `--image-export-mode embedded`, so reading one back is what makes a
/// render round-trip. The URI is read, never fetched; RFC 2397 also allows the
/// payload to be written with percent escapes instead of base64, so both forms
/// are taken. A declared type outside the shared picture list — `image/avif`, or
/// something that is not an image at all — is a picture this cannot carry, so it
/// is no picture here either and the source stays as written.
fn data_uri_picture(source: &str) -> Option<(Option<&'static str>, Vec<u8>)> {
    let source = source.trim();
    // A URI scheme is case-insensitive, and `get` leaves a non-ASCII boundary
    // alone rather than slicing through it.
    let rest = source
        .get(..5)
        .filter(|scheme| scheme.eq_ignore_ascii_case("data:"))
        .map(|_| &source[5..])?;
    let (header, payload) = rest.split_once(',')?;
    let mut parameters = header.split(';');
    // The essence is case-insensitive and a parameter (`;base64`, a charset)
    // is not part of it: the list is matched against the type alone.
    let essence = parameters.next()?.trim().to_ascii_lowercase();
    let mimetype = pdfextractor_picture_formats::format_for_mime(&essence)?.mime;
    let base64 = parameters.any(|parameter| parameter.trim().eq_ignore_ascii_case("base64"));
    let data = if base64 {
        // Base64 allows no whitespace, but a page may have wrapped a long URI:
        // the newlines are the page's layout, not the picture's bytes.
        let payload: String = payload
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        STANDARD
            .decode(&payload)
            .or_else(|_| STANDARD_NO_PAD.decode(&payload))
            .ok()?
    } else {
        percent_decode_bytes(payload)
    };
    // An empty payload is a picture that is not there, not a picture.
    (!data.is_empty()).then_some((Some(mimetype), data))
}

/// The picture those bytes are: the format its own header states when the pixels
/// can be read, otherwise the media type the source declared.
///
/// A picture whose format this cannot decode — a GIF, an SVG, a Windows
/// metafile — is still a picture the document named, so it is copied as it is
/// and its size is read from its own bytes ([`image_dimensions`]) rather than
/// left unknown: without a size the model has no `image` to hold, and the page
/// would lose a picture it was given. A file whose extension names no carried
/// format, and whose bytes are not a picture either, is nothing to read.
fn picture_from_bytes(
    declared: Option<&'static str>,
    data: Vec<u8>,
    limits: &Limits,
) -> Option<FlowPicture> {
    let picture = match pdfextractor_image_parser::ImageDocument::parse_bytes(&data) {
        Ok(image) => match pdfextractor_image_parser::turn_upright(&data) {
            // A camera may have stored the picture sideways: the file a reader
            // sees is the upright one, so that is the one to keep.
            Some(upright) => FlowPicture {
                mimetype: upright.mimetype,
                data: upright.data,
                size: Some((upright.width, upright.height)),
            },
            None => FlowPicture {
                mimetype: image.mimetype,
                data,
                size: Some(image.upright_size()),
            },
        },
        Err(_) => FlowPicture {
            mimetype: declared?,
            size: image_dimensions(&data),
            data,
        },
    };
    // A WebP or a JPEG 2000 is written out as PNG, as a metafile already is:
    // Docling cannot name either type, so the picture would otherwise be a box
    // with no image. The PNG's own grid is the size to report.
    let (mimetype, data) = docling_media(picture.mimetype, picture.data, limits);
    let size = if mimetype == picture.mimetype {
        picture.size
    } else {
        image_dimensions(&data)
    };
    Some(FlowPicture {
        mimetype,
        data,
        size,
    })
}

/// The file a picture's source names, opened, if it names one at all.
///
/// A `data:` URI never reaches this: its bytes are the source, and
/// [`data_uri_picture`] has already answered. What is left here is a path, and
/// it is opened only when it names a file inside the document's own directory: a
/// page is untrusted input, so an absolute path or one that climbs out with
/// `..` (or a symlink that leads out) names no picture here rather than
/// whatever file it points at. A relative path is resolved against the
/// document, because that is what a relative destination means in both
/// formats. The caller reads the handle, so the file that was checked is the
/// file that is read — see [`pdfextractor_md_exporter::open_under`].
fn open_local_image(document: &Path, source: &str) -> Option<(PathBuf, std::fs::File)> {
    let source = source.trim();
    if source.is_empty() || source.starts_with("//") {
        return None;
    }
    // `https:`, `mailto:` — anything shaped like a URI scheme names no local
    // file. A Windows drive (`C:\…`) is one letter and is not one.
    if source.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    }) {
        return None;
    }
    let decoded = percent_decode(source);
    let path = Path::new(&decoded);
    if path.is_absolute() {
        return None;
    }
    // A document named without a directory has an empty parent, which is the
    // working directory the document itself was resolved against.
    let directory = match document.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    // The containment lives with the Markdown writer's own picture reader:
    // one check, held to the opened handle, serves both formats.
    pdfextractor_md_exporter::open_under(directory, path)
}

/// Decode `%20`-style escapes in a Markdown destination: a path written for a
/// URL is still a path on disk.
fn percent_decode(text: &str) -> String {
    String::from_utf8_lossy(&percent_decode_bytes(text)).into_owned()
}

/// The bytes a percent-encoded text stands for, which a `data:` URI's payload
/// may be instead of base64.
fn percent_decode_bytes(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push(high << 4 | low);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The media type an image file's extension names, for a format whose header
/// this does not read.
///
/// The formats are the shared list the flow reader accepts a `<source>` by, so
/// an extension this can read is one the reader would take and vice versa.
fn image_mimetype_for_path(path: &Path) -> Option<&'static str> {
    pdfextractor_picture_formats::mime_for_extension(path.extension()?.to_str()?)
}

/// The Markdown for an image this could not read, so the literal survives.
fn markdown_image(alt: &str, source: &str) -> String {
    format!("![{alt}]({source})")
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
    image_base: Option<&Path>,
    image_dir: &mut Option<PathBuf>,
    image_seq: &mut usize,
    limits: &Limits,
) {
    use pdfextractor_docx_parser::DocxItem;
    match item {
        DocxItem::Text {
            text,
            label,
            marker,
            level,
            hyperlink,
        } => {
            builder.add_labeled_item_with_link(
                text,
                label,
                *level,
                marker.clone(),
                enumerated(marker),
                hyperlink.clone(),
            );
        }
        DocxItem::Formula { text } => {
            builder.add_labeled_text(text, "formula");
        }
        DocxItem::Table(table) => {
            let grid = grid_table(
                &table.rows,
                table.num_cols,
                image_base,
                image_dir,
                image_seq,
                "pg1_",
                limits,
            );
            builder.add_table_grid(&grid);
        }
        DocxItem::Picture {
            mime,
            data,
            width_pt,
            height_pt,
        } => {
            *image_seq += 1;
            let name = format!("pg1_{}", image_seq);
            // A Word picture that is not a PNG or a JPEG is written out as one,
            // which is the encoding Docling's own Word reader hands every
            // picture on under. A drawing is played at the size the drawing's
            // frame asks for, so the picture is the size the page shows.
            let (mime, data) = office_media(mime, data.clone(), *width_pt, *height_pt, limits);
            let image = write_image(image_dir, image_base, &name, mime, &data)
                .map(|uri| describe_image(mime, &data, *width_pt, *height_pt, uri));
            builder.add_picture_plain("picture", image);
        }
    }
}

/// Emit one furniture block and return the reference to attach to the group.
fn emit_furniture_item(
    builder: &mut DocumentBuilder,
    item: &pdfextractor_docx_parser::DocxItem,
    image_base: Option<&Path>,
    image_dir: &mut Option<PathBuf>,
    image_seq: &mut usize,
    limits: &Limits,
) -> Option<Ref> {
    use pdfextractor_docx_parser::DocxItem;
    match item {
        DocxItem::Text {
            text,
            label,
            marker,
            level,
            // A header or footer's link goes nowhere a reader of the document
            // body would follow: the furniture layer carries no `hyperlink`,
            // as it does not for a PDF's own running heads.
            hyperlink: _,
        } => Some(builder.add_furniture_item(text, label, *level, marker.clone())),
        DocxItem::Formula { text } => Some(builder.add_furniture_text(text, "formula")),
        DocxItem::Table(table) => {
            let grid = grid_table(
                &table.rows,
                table.num_cols,
                image_base,
                image_dir,
                image_seq,
                "furniture_",
                limits,
            );
            Some(builder.add_furniture_table_grid(&grid))
        }
        DocxItem::Picture {
            mime,
            data,
            width_pt,
            height_pt,
        } => {
            *image_seq += 1;
            let name = format!("furniture_{image_seq}");
            let (mime, data) = office_media(mime, data.clone(), *width_pt, *height_pt, limits);
            let image = write_image(image_dir, image_base, &name, mime, &data)
                .map(|uri| describe_image(mime, &data, *width_pt, *height_pt, uri));
            Some(builder.add_furniture_picture("picture", image))
        }
    }
}

/// A parsed DOCX table as the builder's grid: every cell a merge did not
/// cover, placed at its row and column with its spans. The cells a `w:vMerge`
/// covers are left out — the cell that opened the merge holds its rows, which
/// is what makes it one cell instead of several (the reading the rectangular
/// rows of `add_table_plain` cannot express).
///
/// A cell that holds pictures has them written out as they are met, and gives
/// them to the cell — the builder places them in a group under the table, so a
/// picture inside a table cell stays the cell's rather than becoming an item of
/// the body beside the table. A table written inside a cell is read the same
/// way, as the cell's own content, and so is the cell it holds in turn.
/// `prefix` is what those files are named from, the body's `pg1_` or the
/// furniture's `furniture_`, so a header's cell picture and one in the document
/// do not collide in the image directory.
fn grid_table(
    rows: &[Vec<pdfextractor_docx_parser::DocxCell>],
    num_cols: usize,
    image_base: Option<&Path>,
    image_dir: &mut Option<PathBuf>,
    image_seq: &mut usize,
    prefix: &str,
    limits: &Limits,
) -> GridTable {
    let mut cells = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for cell in row {
            if cell.covered {
                continue;
            }
            let pictures = cell
                .pictures
                .iter()
                .filter_map(|picture| {
                    *image_seq += 1;
                    let name = format!("{prefix}{image_seq}");
                    let (mime, data) = office_media(
                        picture.mime,
                        picture.data.clone(),
                        picture.width_pt,
                        picture.height_pt,
                        limits,
                    );
                    write_image(image_dir, image_base, &name, mime, &data).map(|uri| {
                        describe_image(mime, &data, picture.width_pt, picture.height_pt, uri)
                    })
                })
                .collect();
            let tables = cell
                .tables
                .iter()
                .map(|nested| {
                    grid_table(
                        &nested.rows,
                        nested.num_cols,
                        image_base,
                        image_dir,
                        image_seq,
                        prefix,
                        limits,
                    )
                })
                .collect();
            cells.push(GridCell {
                text: cell.text.clone(),
                start_row: r,
                start_col: cell.start_col,
                row_span: cell.row_span,
                col_span: cell.col_span,
                column_header: cell.header,
                // The reading a Word table has always had here: the first
                // column labels its row. It is stated rather than inferred so
                // an XLSX table — which has no row headers — can say otherwise.
                row_header: !cell.header && cell.start_col == 0 && r > 0,
                pictures,
                tables,
            });
        }
    }
    GridTable {
        num_rows: rows.len(),
        num_cols,
        cells,
    }
}

/// A flow format's table as the builder's grid: a cell where its span starts,
/// and no cell where a merge covers the slot.
///
/// The reader placed the cells against the grid their `rowspan`/`colspan`
/// describe, so the only thing left to decide is which cells label. A table
/// that states its own header row does — RTF's `\trhdr` marks the row that
/// heads the columns wherever it sits, and an HTML row of `<th>`s names the
/// columns it sits over — and the reading carries those rows through. A table
/// that states nothing keeps the positional default: the first row heads the
/// columns and the first column labels its row. That is also why the label is
/// `column_header` only on a row the document marked — a table with a group
/// title above its column names heads the names, not the title.
fn flow_grid(grid: &pdfextractor_text_parser::TableGrid) -> GridTable {
    GridTable {
        num_rows: grid.num_rows,
        num_cols: grid.num_cols,
        cells: grid
            .cells
            .iter()
            .map(|cell| GridCell {
                text: cell.text.clone(),
                start_row: cell.row,
                start_col: cell.col,
                row_span: cell.row_span,
                col_span: cell.col_span,
                column_header: grid.header_row(cell.row),
                row_header: cell.col == 0 && cell.row > 0,
                pictures: Vec::new(),
                tables: Vec::new(),
            })
            .collect(),
    }
}

/// Whether a list marker counts (`1.`, `a)`) rather than marks (`•`).
fn enumerated(marker: &Option<String>) -> Option<bool> {
    marker
        .as_deref()
        .map(|m| m.chars().next().is_some_and(|c| c.is_alphanumeric()))
}

/// The link rectangle that covers most of `bbox`, if one does.
///
/// The join is at the item's box rather than per run: layout has already
/// merged the runs into the paragraph a reader sees, and a rectangle that
/// covers most of that box describes the paragraph — which is where Docling's
/// own PDF references put their links (a footnote whose lines the box wraps).
/// A rectangle touching only part of a paragraph claims none of it: an
/// over-claim would put a URL on prose that never points there.
fn link_covering(links: &[pdfextractor_pdf_parser::PageLink], bbox: [f64; 4]) -> Option<&str> {
    let area = ((bbox[2] - bbox[0]) * (bbox[3] - bbox[1])).max(0.0);
    if area <= 0.0 {
        return None;
    }
    let mut best: Option<(f64, &str)> = None;
    for link in links {
        let width = (bbox[2].min(link.rect[2]) - bbox[0].max(link.rect[0])).max(0.0);
        let height = (bbox[3].min(link.rect[3]) - bbox[1].max(link.rect[1])).max(0.0);
        let covered = width * height;
        if covered > 0.5 * area && best.is_none_or(|(top, _)| covered > top) {
            best = Some((covered, link.uri.as_str()));
        }
    }
    best.map(|(_, uri)| uri)
}

/// The picture under a media type Docling can name: a WebP or a JPEG 2000 is
/// decoded and re-written as PNG, the way a metafile already is.
///
/// Docling rejects an `ImageRef` whose type is not in its registry, and the
/// rejection invalidates the whole document, not just the picture. These two
/// formats hold pixels that can be read, so they are handed on under the one
/// type every reader knows rather than dropped. A picture that cannot be
/// decoded keeps its own type and bytes: a payload is never replaced by a guess.
///
/// A picture past `max_image_pixels`, `max_image_dimension` or the allocation
/// ceiling keeps its own type and bytes rather than being decoded anyway.
fn docling_media(mime: &'static str, data: Vec<u8>, limits: &Limits) -> (&'static str, Vec<u8>) {
    if pdfextractor_core::is_representable_media_type(mime) {
        return (mime, data);
    }
    match pdfextractor_image_parser::transcode_to_png_checked(&data, limits).ok() {
        Some(png) => ("image/png", png.data),
        None => (mime, data),
    }
}

/// The picture an office package carries, under the encoding that package's
/// reader leaves behind: a PNG.
///
/// Docling's own Word and PowerPoint readers open every picture they find with
/// Pillow and write the frames back as PNG, so `image/png` is the only type
/// their model ever holds for one; a BMP, a TIFF, a GIF, a WebP or a JPEG 2000
/// in the package is never handed on as itself. The same is done here: a PNG or
/// a JPEG — the two encodings a scan or a photograph is already stored in —
/// travels untouched, and anything else is decoded and re-encoded losslessly.
///
/// An SVG is not an encoding of pixels at all but a drawing, so it is *played*:
/// at the size the package places the picture at, 144 dpi, which is the size and
/// resolution a metafile in the same package is played at. A drawing left as
/// markup is not a picture any consumer downstream can show, and an office
/// document that names one expects pixels. Where the package states no box, the
/// drawing's own viewport is the size there is, one pixel per CSS pixel.
///
/// A payload that cannot be drawn keeps its own bytes and type, so a drawing
/// whose markup this cannot parse is still placed and named; a type Docling
/// cannot name and this cannot decode is left a box with no image, as
/// [`docling_media`] leaves it. That is why there is no second attempt there:
/// the transcode has already been tried.
///
/// A picture past the pixel, dimension or allocation ceiling keeps its own type
/// and bytes rather than being decoded anyway — the same answer an undecodable
/// picture gets.
fn office_media(
    mime: &'static str,
    data: Vec<u8>,
    width_pt: Option<f64>,
    height_pt: Option<f64>,
    limits: &Limits,
) -> (&'static str, Vec<u8>) {
    if mime == "image/png" || mime == "image/jpeg" {
        return (mime, data);
    }
    if mime == "image/svg+xml" {
        let mut options = pdfextractor_svg::Options::default();
        if let (Some(width), Some(height)) = (width_pt, height_pt) {
            options = options.at_size(width, height);
        }
        match pdfextractor_svg::rasterize(&data, options) {
            Ok(raster) if raster.inked_pixels() == 0 => {
                tracing::debug!("a drawing rasterised to an empty picture");
            }
            Ok(raster) => match raster.to_png() {
                Ok(png) => return ("image/png", png),
                Err(err) => tracing::debug!("cannot encode a rasterised drawing: {err}"),
            },
            Err(err) => tracing::debug!("leaving a drawing unrasterised: {err}"),
        }
        return (mime, data);
    }
    match pdfextractor_image_parser::transcode_to_png_checked(&data, limits).ok() {
        Some(png) => ("image/png", png.data),
        None => (mime, data),
    }
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

/// The page a later sheet of a stack fills, in points.
///
/// A frame's own pixel grid at the resolution the file declares — or one pixel
/// per point when it declares none — measured the way
/// [`pdfextractor_image_parser::ImageDocument`] measures the first page. A
/// quarter turn puts each grid axis on the other axis's resolution, exactly as
/// it does there.
fn page_size(
    image: &pdfextractor_image_parser::ImageDocument,
    pixels: (u32, u32),
    swaps: bool,
) -> (f64, f64) {
    let (x, y) = match image.dpi {
        Some((x, y)) if x.is_finite() && y.is_finite() && x > 0.0 && y > 0.0 => (x, y),
        _ => (
            pdfextractor_image_parser::DEFAULT_DPI,
            pdfextractor_image_parser::DEFAULT_DPI,
        ),
    };
    let (width, height, x, y) = if swaps {
        (pixels.1, pixels.0, y, x)
    } else {
        (pixels.0, pixels.1, x, y)
    };
    (f64::from(width) * 72.0 / x, f64::from(height) * 72.0 / y)
}

/// The viewport an SVG declares, from the root `width`/`height` or its `viewBox`.
///
/// The number is taken as written and its unit ignored — `10`, `10px` and
/// `10pt` all answer 10 — because an SVG has no resolution until something
/// places it. A document that states neither answers `None` rather than
/// inventing a size.
fn svg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(data).ok()?;
    let start = text.find("<svg")? + "<svg".len();
    let tag = &text[start..start + text[start..].find('>')?];
    let extent = |width: f64, height: f64| {
        (width > 0.0 && height > 0.0).then_some((width.round() as u32, height.round() as u32))
    };
    if let (Some(width), Some(height)) = (svg_length(tag, "width"), svg_length(tag, "height")) {
        if let Some(size) = extent(width, height) {
            return Some(size);
        }
    }
    // `viewBox="min-x min-y width height"`: the drawing's own coordinate
    // system, whose extent is the only size left when the tag states none.
    let boxed = svg_attribute(tag, "viewBox")?;
    let extents: Vec<f64> = boxed.split_whitespace().filter_map(svg_number).collect();
    extent(*extents.get(2)?, *extents.get(3)?)
}

/// A length attribute of the root `<svg>` tag, as a number.
fn svg_length(tag: &str, name: &str) -> Option<f64> {
    svg_attribute(tag, name).and_then(svg_number)
}

/// The text an attribute of the root `<svg>` tag holds.
///
/// The name has to stand on its own, so `width` is not read out of a
/// `stroke-width`: a drawing's stroke is not the picture's size. An attribute
/// without quotes is not well-formed XML, and answers nothing.
fn svg_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut search = 0;
    while let Some(offset) = tag[search..].find(name) {
        let at = search + offset;
        search = at + name.len();
        let starts_word = match tag[..at].chars().next_back() {
            None => true,
            Some(c) => c.is_whitespace() || c == '<',
        };
        if !starts_word {
            continue;
        }
        let Some(rest) = tag[search..].trim_start().strip_prefix('=') else {
            continue;
        };
        let value = rest.trim_start();
        let Some(quote) = value.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
            continue;
        };
        return value[1..].split(quote).next();
    }
    None
}

/// The number an SVG length opens with, its unit ignored.
fn svg_number(value: &str) -> Option<f64> {
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    value[..end].parse().ok()
}

/// Pixel dimensions a picture's own bytes state, read from its header.
///
/// PNG, JPEG, GIF, SVG, BMP and TIFF declare them; a format that does not (or
/// one whose declaration is malformed) answers `None`, and the size is left out
/// rather than invented. An SVG's viewport is what its `width`/`height` say, or
/// its `viewBox` when they say nothing — user units, which is the only size an
/// SVG has without a page to sit on.
///
/// A BMP or a TIFF is sized by the same reader that makes a `.bmp` or a `.tif`
/// an input page: a flow page keeps both encodings as they were, and a header
/// this project already reads is what such a picture needs to be named in an
/// `image` reference rather than left a box with no image. An office package
/// does not reach this branch for either: its pictures are re-encoded as PNG
/// before they are measured ([`office_media`]).
fn image_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() > 24 && data.starts_with(&[0x89, b'P', b'N', b'G']) {
        let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if data.len() > 10 && (data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return (w > 0 && h > 0).then_some((w, h));
    }
    if data.starts_with(b"<svg") || data.starts_with(b"<?xml") {
        return svg_dimensions(data);
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
    // Everything left is a format the image reader sizes from its header — a
    // BMP, a TIFF, or a truncated PNG/JPEG whose signature the branches above
    // did not reach. The upright grid is what a reader sees, the same grid an
    // image input reports as its page.
    pdfextractor_image_parser::ImageDocument::parse_bytes(data)
        .ok()
        .map(|image| image.upright_size())
}

/// The image turned upright the way its EXIF orientation says the camera was
/// held — a phone held sideways stores a portrait page as a landscape grid,
/// and Tesseract's loader reads the stored grid without consulting the tag.
///
/// PNG out, lossless: recognition sees exactly the pixels a viewer would, and
/// the boxes come back in the upright pixel space the page is measured in.
/// `None` for an image that is already upright, or one that cannot be decoded,
/// which includes the run's image budgets refusing it: the turn is one more
/// decode, so `--max-image-pixels` governs it as it governs every other. The
/// original bytes are what Tesseract then gets.
pub fn orient_for_ocr(
    data: &[u8],
    orientation: pdfextractor_image_parser::Orientation,
    limits: &pdfextractor_limits::Limits,
) -> Result<Vec<u8>, pdfextractor_limits::LimitError> {
    pdfextractor_image_parser::upright_png_checked(data, orientation, limits)
}

/// Whether a picture *is* the page, rather than a figure printed on it.
///
/// A scan is one picture covering the page, and only that can be turned as a
/// whole without also turning text beside it: a diagram on a page with nothing
/// else on it must not drag the page round with it.
fn picture_is_page(bbox: [f64; 4], page_width: f64, page_height: f64) -> bool {
    let area = (bbox[2] - bbox[0]).max(0.0) * (bbox[3] - bbox[1]).max(0.0);
    area >= 0.5 * page_width * page_height
}

/// The same bitmap, turned clockwise by `degrees` — the turn the viewer gives
/// the page. PNG out, lossless: recognition sees exactly the pixels a reader
/// would, upright, and the boxes it reports are in the displayed space the
/// target box is measured in. `None` when the image cannot be decoded, which
/// includes the run's image budgets refusing it: the turn is one more decode,
/// so `--max-image-pixels` governs it as it governs every other. Tesseract's
/// own decoder then gets the original bytes and its error policy applies. A
/// caller that has to say which budget refused the turn wants
/// [`rotate_for_ocr_checked`], whose error carries it.
pub fn rotate_for_ocr(
    data: &[u8],
    degrees: i32,
    limits: &pdfextractor_limits::Limits,
) -> Option<(Vec<u8>, u32, u32)> {
    rotate_for_ocr_checked(data, degrees, limits).ok()
}

/// [`rotate_for_ocr`] with the refusal that stopped it.
///
/// The turn is one more decode, so it is the run's image budgets that refuse
/// it, and the budget's own [`pdfextractor_limits::LimitError`] is passed
/// through rather than wrapped: that is the form that says which flag would
/// raise the ceiling. A `degrees` that is not a quarter turn is a caller's bug
/// rather than a budget's refusal, and comes back as
/// [`pdfextractor_limits::LimitError::ImageDecode`].
pub fn rotate_for_ocr_checked(
    data: &[u8],
    degrees: i32,
    limits: &pdfextractor_limits::Limits,
) -> Result<(Vec<u8>, u32, u32), pdfextractor_limits::LimitError> {
    use pdfextractor_limits::LimitError;

    let bitmap = pdfextractor_image_parser::decode_checked(data, limits)?;
    let turned = match degrees {
        90 => bitmap.rotate90(),
        180 => bitmap.rotate180(),
        270 => bitmap.rotate270(),
        _ => {
            return Err(LimitError::ImageDecode {
                detail: format!("{degrees} is not a quarter turn"),
            })
        }
    };
    let (width, height) = (turned.width(), turned.height());
    let mut png = Vec::new();
    turned
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| LimitError::ImageDecode {
            detail: error.to_string(),
        })?;
    Ok((png, width, height))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture the page names beside itself is opened only inside the
    /// document's own directory, and the handle is held to the path that was
    /// checked: a name that climbs out, an absolute one, or a symlink that
    /// leads out names no picture.
    #[test]
    fn a_flow_picture_is_opened_only_from_the_documents_own_directory() {
        let root = std::env::temp_dir().join(format!("pdfe-flow-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("docs")).expect("the document's directory");
        std::fs::write(root.join("docs/picture.png"), b"inside").expect("the picture");
        std::fs::write(root.join("outside.png"), b"outside").expect("the file outside");

        let document = root.join("docs/report.md");
        let (path, _file) =
            open_local_image(&document, "picture.png").expect("a picture beside it opens");
        assert_eq!(
            path,
            root.join("docs/picture.png")
                .canonicalize()
                .expect("canonical")
        );
        assert!(
            open_local_image(&document, "../outside.png").is_none(),
            "`..` climbs out"
        );
        assert!(
            open_local_image(&document, root.join("outside.png").to_str().expect("utf-8"))
                .is_none(),
            "an absolute name is refused"
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("outside.png"), root.join("docs/link.png"))
                .expect("the symlink");
            assert!(
                open_local_image(&document, "link.png").is_none(),
                "a symlink leads out"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The limits are the conversion's, not the process's: two documents read in
    /// one process each get the ceiling their own options name. The first is
    /// held to four bytes and is refused; the second, with the defaults, reads
    /// the very same bytes.
    #[test]
    fn two_conversions_in_one_process_each_get_their_own_ceiling() {
        let root = std::env::temp_dir().join(format!("pdfe-two-ceilings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("the scratch directory");
        let path = root.join("notes.md");
        std::fs::write(
            &path,
            "# Notes\n\nA paragraph, long enough to pass a ceiling of four bytes.\n",
        )
        .expect("the document");

        let refused = build_document_with_format(
            &path,
            &Format::Flow("md"),
            &ConvertOptions {
                limits: Limits {
                    max_input_bytes: 4,
                    ..Limits::default()
                },
                ..ConvertOptions::default()
            },
        );
        match refused {
            Err(PipelineError::Limit(LimitError::InputTooLarge { limit, .. })) => {
                assert_eq!(limit, 4, "the refusal names another ceiling")
            }
            other => panic!("a four-byte ceiling did not refuse the document: {other:?}"),
        }

        // The same file, in the same process, under the ceiling the second
        // conversion names: a global would still hold the first one's.
        let read =
            build_document_with_format(&path, &Format::Flow("md"), &ConvertOptions::default())
                .expect("the defaults read the same file");
        assert_eq!(read.name, "notes.md", "the second conversion read nothing");

        let _ = std::fs::remove_dir_all(&root);
    }

    fn comment(
        author: Option<&str>,
        initials: Option<&str>,
        date: Option<&str>,
        text: &str,
    ) -> pdfextractor_pptx_parser::Comment {
        pdfextractor_pptx_parser::Comment {
            index: "1".to_string(),
            date: date.map(str::to_string),
            author: author.map(str::to_string),
            initials: initials.map(str::to_string),
            text: text.to_string(),
        }
    }

    /// A comment is recorded the way Docling records one: what the deck says
    /// about it, then the words. Each part is left out when the file has
    /// nothing to write there, which is what keeps the two readings equal.
    #[test]
    fn a_comment_carries_what_the_deck_says_about_it() {
        assert_eq!(
            comment_text(&comment(
                Some("Ada Lovelace"),
                Some("AL"),
                Some("2026-10-18T09:15:00.000"),
                "Check the numbers."
            )),
            "[author: Ada Lovelace (AL), time: 2026-10-18T09:15:00.000]: Check the numbers."
        );
        // An author without initials, and a file that names neither.
        assert_eq!(
            comment_text(&comment(Some("Ada"), None, None, "Note.")),
            "[author: Ada]: Note."
        );
        assert_eq!(comment_text(&comment(None, None, None, "Note.")), "Note.");
        // A blank name, initial or time is nothing to write.
        assert_eq!(
            comment_text(&comment(Some(""), Some("AL"), Some(""), "Note.")),
            "Note."
        );
        assert_eq!(
            comment_text(&comment(Some("Ada"), Some(""), Some(""), "Note.")),
            "[author: Ada]: Note."
        );
    }

    /// Impress writes a comment's time with nanosecond digits where PowerPoint
    /// writes milliseconds; the note is about the remark, not the writer, so
    /// the time is the same and the text has to be too.
    #[test]
    fn a_bridged_comment_time_reads_at_powerpoints_precision() {
        assert_eq!(
            comment_text(&comment(
                Some("Ada Lovelace"),
                Some("AL"),
                Some("2026-10-18T09:15:00.000000000"),
                "Check the numbers."
            )),
            "[author: Ada Lovelace (AL), time: 2026-10-18T09:15:00.000]: Check the numbers."
        );
        // A shorter fraction is padded to the same three digits, and a zone
        // after it is the file's.
        assert_eq!(
            comment_timestamp("2026-01-04T05:48:07.5+00:00"),
            "2026-01-04T05:48:07.500+00:00"
        );
        assert_eq!(
            comment_timestamp("2026-01-04T05:48:07.123456Z"),
            "2026-01-04T05:48:07.123Z"
        );
        // Already millisecond-precise, fractional-second-free, or not a time:
        // left exactly as written.
        for timestamp in [
            "2026-10-18T09:15:00.000",
            "2026-01-04T05:48:07",
            "2026-01-04T05:48:07Z",
            "v1.2.3",
        ] {
            assert_eq!(comment_timestamp(timestamp), timestamp);
        }
    }

    /// Writing a picture and reading its file back are the two directions of
    /// one list — the shared picture formats — so a format added there is named
    /// by this side without another edit, and a type outside it is never given
    /// an extension that would be wrong.
    #[test]
    fn every_carried_picture_format_is_written_and_read_the_same_way() {
        for format in pdfextractor_picture_formats::PICTURE_FORMATS {
            assert_eq!(
                ext_for_mime(format.mime),
                format.extension,
                "{}",
                format.mime
            );
            let path = Path::new("picture").with_extension(format.extension);
            assert_eq!(
                image_mimetype_for_path(&path),
                Some(format.mime),
                "{}",
                format.extension
            );
            for alias in format.read_as {
                let path = Path::new("picture").with_extension(alias);
                assert_eq!(
                    image_mimetype_for_path(&path),
                    Some(format.mime),
                    "{alias} names {}",
                    format.mime
                );
            }
        }

        // A type the list does not hold never claims an extension: `.bin` says
        // nothing rather than something false.
        assert_eq!(ext_for_mime("image/avif"), "bin");
        assert_eq!(ext_for_mime("application/octet-stream"), "bin");
        assert_eq!(image_mimetype_for_path(Path::new("picture.avif")), None);
        assert_eq!(image_mimetype_for_path(Path::new("picture")), None);
    }

    /// The office-media transcode takes the run's image budgets. A picture past
    /// the pixel ceiling keeps its own type and bytes instead of being decoded
    /// anyway, so the budget reaches this decode and not only the readers.
    #[test]
    fn a_picture_over_the_budget_keeps_its_own_bytes_for_an_office_package() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/images/scan.gif");
        let gif =
            std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));

        let (mime, png) = office_media(
            "image/gif",
            gif.clone(),
            None,
            None,
            &pdfextractor_limits::Limits::default(),
        );
        assert_eq!(mime, "image/png", "the defaults must decode the picture");
        assert!(!png.is_empty(), "the transcode produced no bytes");

        let tight = pdfextractor_limits::Limits {
            max_image_pixels: 1,
            ..Default::default()
        };
        assert_eq!(
            office_media("image/gif", gif.clone(), None, None, &tight),
            ("image/gif", gif),
            "a one-pixel budget must leave the picture as it was"
        );
    }

    /// The same rule where a picture is transcoded for Docling: a WebP past the
    /// budget is left as itself, which the builder then leaves out rather than
    /// naming a type Docling cannot read.
    #[test]
    fn a_picture_over_the_budget_is_not_transcoded_for_docling() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/images/chart.webp");
        let webp =
            std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));

        assert_eq!(
            docling_media(
                "image/webp",
                webp.clone(),
                &pdfextractor_limits::Limits::default()
            )
            .0,
            "image/png",
            "the defaults must transcode the picture"
        );

        let tight = pdfextractor_limits::Limits {
            max_image_pixels: 1,
            ..Default::default()
        };
        assert_eq!(
            docling_media("image/webp", webp.clone(), &tight),
            ("image/webp", webp),
            "a one-pixel budget must leave the picture as it was"
        );
    }

    /// An office SVG is played at the frame the package gives it, and a frame of
    /// different proportions is not permission to squash the drawing: 36 × 36
    /// points is 72 × 72 pixels at 144 dpi, and a 40 × 24 drawing inside it is
    /// scaled by one factor (1.8, the smaller of the two the frame allows) and
    /// centred — 72 × 43 pixels of drawing with clear margin above and below.
    /// The picture's box is still the frame's, so its placement is unchanged.
    #[test]
    fn an_office_svg_is_letterboxed_into_a_frame_of_another_shape() {
        let drawing = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24" viewBox="0 0 40 24"><rect width="40" height="24" fill="#e8eef7"/></svg>"##;
        let (mime, png) = office_media(
            "image/svg+xml",
            drawing.to_vec(),
            Some(36.0),
            Some(36.0),
            &pdfextractor_limits::Limits::default(),
        );
        assert_eq!(mime, "image/png", "the drawing was left as markup");
        let raster = image::load_from_memory(&png)
            .expect("the painted picture decodes")
            .to_rgba8();
        assert_eq!(raster.dimensions(), (72, 72), "the frame's own grid");
        // The margin is the frame's and is not painted: the drawing spans y from
        // 14 to 58, so the first and last rows are the picture's transparent
        // letterbox and the middle row is the panel.
        for y in [0, 71] {
            assert_eq!(
                raster.get_pixel(36, y).0[3],
                0,
                "row {y} was painted: the drawing was stretched into the frame"
            );
        }
        assert_eq!(raster.get_pixel(36, 36).0, [0xe8, 0xee, 0xf7, 0xff]);
    }

    /// A drawing placed in an office frame is fitted the way the drawing's own
    /// `preserveAspectRatio` asks, not always the default. `none` stretches it
    /// into the square frame, `slice` covers the frame and crops the overflow,
    /// and `xMaxYMax` pins it to the corner instead of centring it.
    #[test]
    fn an_office_svg_is_placed_by_its_own_preserve_aspect_ratio() {
        let picture = |aspect: &str, width_pt: f64, height_pt: f64| {
            let drawing = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24" viewBox="0 0 40 24" preserveAspectRatio="{aspect}"><rect width="40" height="24" fill="#e8eef7"/></svg>"##
            );
            let (mime, png) = office_media(
                "image/svg+xml",
                drawing.into_bytes(),
                Some(width_pt),
                Some(height_pt),
                &pdfextractor_limits::Limits::default(),
            );
            assert_eq!(mime, "image/png", "the drawing was left as markup");
            image::load_from_memory(&png)
                .expect("the painted picture decodes")
                .to_rgba8()
        };

        let stretched = picture("none", 36.0, 36.0);
        assert_eq!(stretched.dimensions(), (72, 72), "the frame's own grid");
        for y in [0, 71] {
            assert_eq!(
                stretched.get_pixel(36, y).0[3],
                255,
                "row {y} was clear: the drawing was not stretched into the frame"
            );
        }

        let cropped = picture("xMidYMid slice", 36.0, 36.0);
        assert_eq!(cropped.dimensions(), (72, 72));
        for (x, y) in [(36, 0), (0, 36), (71, 36), (36, 71)] {
            assert_eq!(
                cropped.get_pixel(x, y).0[3],
                255,
                "({x}, {y}) was clear: the drawing did not cover the frame"
            );
        }

        // 60 × 24 points is a 120 × 48 grid, wider than the drawing's shape:
        // `xMax` puts the 80 × 48 drawing at its right edge, not in the middle.
        let cornered = picture("xMaxYMax meet", 60.0, 24.0);
        assert_eq!(cornered.dimensions(), (120, 48));
        assert_eq!(
            cornered.get_pixel(4, 24).0[3],
            0,
            "the left margin was painted instead of leaving the drawing at the right"
        );
        assert_eq!(cornered.get_pixel(115, 24).0, [0xe8, 0xee, 0xf7, 0xff]);
    }

    /// An SVG states its size in its root tag: `width`/`height` when they are
    /// there, `viewBox` when they are not, and nothing when it says neither. A
    /// length is taken as written with its unit ignored — there is no page here
    /// to resolve one against — and a lookalike name is not the size.
    #[test]
    fn an_svg_states_its_size_in_its_root_tag() {
        let svg = |tag: &str| svg_dimensions(format!("<svg {tag}/>").as_bytes());
        assert_eq!(svg(r#"width="10" height="12""#), Some((10, 12)));
        assert_eq!(svg(r#"width='10.4px' height='12pt'"#), Some((10, 12)));
        assert_eq!(svg(r#"viewBox="0 0 24 18""#), Some((24, 18)));
        // A negative origin is where the drawing starts, not how big it is.
        assert_eq!(svg(r#"viewBox="-4 -5 30 40""#), Some((30, 40)));
        // `width`/`height` win over the `viewBox` when both are stated.
        assert_eq!(
            svg(r#"viewBox="0 0 24 18" width="6" height="7""#),
            Some((6, 7))
        );
        // A stroke's width is a line's, not the picture's.
        assert_eq!(svg(r#"stroke-width="9" stroke-height="9""#), None);
        // A size of zero is no size at all, and an unquoted value says nothing.
        assert_eq!(svg(r#"width="0" height="12""#), None);
        assert_eq!(svg(r#"width=10 height=12"#), None);
        assert_eq!(svg(""), None);
        // An XML declaration ahead of the root tag is part of the file.
        assert_eq!(
            svg_dimensions(br#"<?xml version="1.0"?><svg width="3" height="4"/></svg>"#),
            Some((3, 4))
        );
        // Not a drawing.
        assert_eq!(svg_dimensions(b"not markup"), None);
    }

    /// A GIF states its size in the ten bytes after its signature, little-endian
    /// where a PNG is big-endian. A zero extent is no size rather than a size
    /// of zero.
    #[test]
    fn a_gif_states_its_size_in_its_header() {
        let gif = |signature: &[u8; 6], width: u16, height: u16| {
            let mut bytes = [0u8; 13];
            bytes[..6].copy_from_slice(signature);
            bytes[6..8].copy_from_slice(&width.to_le_bytes());
            bytes[8..10].copy_from_slice(&height.to_le_bytes());
            bytes
        };
        assert_eq!(image_dimensions(&gif(b"GIF89a", 3, 5)), Some((3, 5)));
        assert_eq!(
            image_dimensions(&gif(b"GIF87a", 620, 350)),
            Some((620, 350))
        );
        assert_eq!(image_dimensions(&gif(b"GIF89a", 0, 5)), None);
        // A signature alone is not a GIF's header.
        assert_eq!(image_dimensions(b"GIF89a"), None);
    }

    /// A `data:` URI is read as RFC 2397 spells it: the scheme is
    /// case-insensitive, the media type is matched with its parameters dropped,
    /// and a base64 payload decodes whether it is padded or not. What is not a
    /// picture this can carry — another type, an undecodable or empty payload —
    /// is no picture, so the source stays as it was written.
    #[test]
    fn a_data_uri_names_a_picture_only_when_this_can_carry_it() {
        // A PNG's eight-byte signature, in the four spellings a page may use.
        for source in [
            "data:image/png;base64,iVBORw0KGgo=",
            "DATA:IMAGE/PNG;BASE64,iVBORw0KGgo=",
            "Data:Image/Png;Base64,iVBORw0KGgo=",
            "data:image/png;charset=binary;base64,iVBORw0KGgo=",
            "data:image/png;base64,iVBORw0KGgo",
        ] {
            let (mime, data) = data_uri_picture(source).unwrap_or_else(|| panic!("{source}"));
            assert_eq!(mime, Some("image/png"), "{source}");
            assert_eq!(
                data,
                vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
                "{source}"
            );
        }

        // Percent escapes are the payload's other spelling.
        assert_eq!(
            data_uri_picture("data:image/svg+xml,%3Csvg/%3E"),
            Some((Some("image/svg+xml"), b"<svg/>".to_vec()))
        );

        for source in [
            "https://example.com/x.png",
            "data:image/avif;base64,AAAA",
            "data:text/plain;base64,aGk=",
            "data:image/png;base64,",
            "data:image/png;base64,!!!!",
            "data:image/png",
        ] {
            assert_eq!(data_uri_picture(source), None, "{source}");
        }
    }
}
