mod compare;
mod redline;

// The range grammar is the window's too: a typo is refused there before a run
// starts, with the message a process would have printed.
use pdfextractor_core::page_range;

use anyhow::Context as _;
use clap::{CommandFactory as _, FromArgMatches as _, Parser, ValueEnum, ValueHint};
// The format question — what a file actually is, and the one family with no
// reader here — lives in a crate of its own, because the corpus report asks it
// of the same documents and the two must not answer it differently.
use pdfextractor_formats::{detect, is_bridged, Format};
use pdfextractor_json_exporter::export_to_file;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
    /// match is told apart as what it is (see `compare::Session`).
    fn as_str(self) -> &'static str {
        match self {
            OcrMode::Auto => "auto",
            OcrMode::Always => "always",
            OcrMode::Never => "never",
        }
    }
}

/// What a finished document can be written as.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// The Docling-compatible JSON document (the default).
    Json,
    /// A Markdown rendering of the same document.
    Md,
    /// A standalone HTML page, tables with rowspan/colspan.
    Html,
    /// Plain text in reading order.
    Text,
    /// The whole document model as YAML, reading order first.
    Yaml,
    /// Retrieval chunks as JSON Lines, in Docling's `DocChunk` shape.
    Chunks,
}

/// How `--to chunks` splits a document.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ChunkType {
    /// One chunk per body item, with consecutive list items together and the
    /// heading path carried on each chunk.
    Hierarchical,
    /// Sentences packed to `--chunks-max-tokens`, with overlap.
    Hybrid,
}

impl ChunkType {
    fn kind(self) -> pdfextractor_md_exporter::chunks::ChunkKind {
        match self {
            ChunkType::Hierarchical => pdfextractor_md_exporter::chunks::ChunkKind::Hierarchical,
            ChunkType::Hybrid => pdfextractor_md_exporter::chunks::ChunkKind::Hybrid,
        }
    }
}

/// The tiktoken encoding family a `.tiktoken` vocabulary belongs to. The
/// variants are tiktoken's own names (`cl100k_base`, …), suffix and all.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[allow(clippy::enum_variant_names)]
enum TokenizerEncoding {
    #[value(name = "cl100k_base", alias = "cl100k")]
    Cl100k,
    #[value(name = "o200k_base", alias = "o200k")]
    O200k,
    #[value(name = "p50k_base", alias = "p50k")]
    P50k,
    #[value(name = "r50k_base", alias = "r50k")]
    R50k,
}

impl TokenizerEncoding {
    fn encoding(self) -> pdfextractor_md_exporter::chunks::TiktokenEncoding {
        use pdfextractor_md_exporter::chunks::TiktokenEncoding;
        match self {
            TokenizerEncoding::Cl100k => TiktokenEncoding::Cl100k,
            TokenizerEncoding::O200k => TiktokenEncoding::O200k,
            TokenizerEncoding::P50k => TiktokenEncoding::P50k,
            TokenizerEncoding::R50k => TiktokenEncoding::R50k,
        }
    }
}

/// How a Markdown or HTML reading refers to its pictures.
///
/// The names are Docling's `--image-export-mode` values, so the flag means the
/// same thing in both tools.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ImageExportMode {
    /// Refer to the image files written beside the document (the default).
    Referenced,
    /// Carry each picture's bytes in the document as a base64 `data:` URI.
    Embedded,
    /// Mark each picture's place with `<!-- image -->` and take no reference.
    Placeholder,
}

impl ImageExportMode {
    fn mode(self) -> pdfextractor_md_exporter::ImageMode {
        match self {
            ImageExportMode::Referenced => pdfextractor_md_exporter::ImageMode::Referenced,
            ImageExportMode::Embedded => pdfextractor_md_exporter::ImageMode::Embedded,
            ImageExportMode::Placeholder => pdfextractor_md_exporter::ImageMode::Placeholder,
        }
    }
}

impl OutputFormat {
    /// The extension this format is written under.
    fn ext(self) -> &'static str {
        match self {
            OutputFormat::Json => "json",
            OutputFormat::Md => "md",
            OutputFormat::Html => "html",
            OutputFormat::Text => "txt",
            OutputFormat::Yaml => "yaml",
            OutputFormat::Chunks => "jsonl",
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "PDF, Office and image document extractor",
    long_about = None,
    after_help = accepted_formats_help()
)]
struct Args {
    /// Input file or directory path (required unless `--list-input-formats` is
    /// given)
    #[arg(short, long, value_hint = ValueHint::AnyPath)]
    input: Option<PathBuf>,

    /// Print the input formats this run accepts as JSON and exit: the set
    /// `--help` names, for a machine rather than a reader
    #[arg(long)]
    list_input_formats: bool,

    /// Output path (optional). If not provided, outputs in same folder as input.
    /// An existing folder, or a path ending in a separator, is the folder to
    /// write into. The name `-` writes the document to stdout — one input, one
    /// format. An output that resolves to the input document is refused rather
    /// than written, so a conversion cannot replace the file it read.
    #[arg(short, long, value_hint = ValueHint::AnyPath)]
    output: Option<PathBuf>,

    /// Output format, repeatable (`--to json --to md --to html --to text
    /// --to yaml --to chunks`); absent means `json`,
    /// which is what the compare mode and the window read. Each format is
    /// written beside the same stem, so `--to json --to md` writes siblings
    #[arg(long, value_enum)]
    to: Vec<OutputFormat>,

    /// Join words a line break split when writing `--to text` (the default;
    /// pdftotext does the same. `--text-dehyphenate false` keeps every source
    /// character, at the cost of `exam- ple` style breaks)
    #[arg(
        long,
        default_value_t = true,
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set
    )]
    text_dehyphenate: bool,

    /// The largest input a run will read, in bytes (default 268435456). A file
    /// over it is refused by the shared budgets, not read.
    #[arg(long, value_name = "N")]
    max_input_bytes: Option<u64>,

    /// The most pages, sheets, slides or frames one document may have (default
    /// 10000). A document over it is refused by the shared budgets.
    #[arg(long, value_name = "N")]
    max_pages: Option<usize>,

    /// The most indirect objects a PDF may hold (default 250000)
    #[arg(long, value_name = "N")]
    max_pdf_objects: Option<usize>,

    /// The most lexical tokens a PDF may present (default 4000000); bounds the
    /// parser's token vector
    #[arg(long, value_name = "N")]
    max_pdf_tokens: Option<usize>,

    /// The deepest PDF array, dictionary or page tree the parser will descend
    /// (default 64)
    #[arg(long, value_name = "N")]
    max_pdf_nesting: Option<usize>,

    /// The largest image grid a picture may have, in pixels (default 100000000)
    #[arg(long, value_name = "N")]
    max_image_pixels: Option<u64>,

    /// The largest one side of an image grid may be, in pixels (default
    /// 100000)
    #[arg(long, value_name = "N")]
    max_image_dimension: Option<u32>,

    /// The most bytes one image decode may allocate (default 536870912)
    #[arg(long, value_name = "N")]
    max_image_alloc_bytes: Option<u64>,

    /// The most frames a TIFF, GIF or animated WebP may hold when its frames
    /// are read as pages (default 1000)
    #[arg(long, value_name = "N")]
    max_image_frames: Option<usize>,

    /// The most blocks a flow reader — text, Markdown, CSV, RTF, HTML, DOCX or
    /// legacy `.doc` — may produce (default 1000000). A table is one block per
    /// cell
    #[arg(long, value_name = "N")]
    max_flow_blocks: Option<usize>,

    /// The largest byte count a single decoded PDF stream may produce (default
    /// 134217728)
    #[arg(long, value_name = "N")]
    max_decoded_stream_bytes: Option<u64>,

    /// The most entries a ZIP package may hold (default 10000)
    #[arg(long, value_name = "N")]
    max_zip_entries: Option<usize>,

    /// The largest uncompressed size one ZIP entry may reach, in bytes
    /// (default 67108864)
    #[arg(long, value_name = "N")]
    max_zip_entry_bytes: Option<u64>,

    /// The largest total uncompressed size a ZIP package may reach, in bytes
    /// (default 536870912)
    #[arg(long, value_name = "N")]
    max_zip_total_bytes: Option<u64>,

    /// The largest expansion ratio an entry above one mebibyte may declare
    /// (default 200)
    #[arg(long, value_name = "N")]
    max_zip_ratio: Option<u64>,

    /// The largest XML part a package may carry, in bytes (default 67108864)
    #[arg(long, value_name = "N")]
    max_xml_bytes: Option<u64>,

    /// The deepest XML element nesting accepted (default 256)
    #[arg(long, value_name = "N")]
    max_xml_depth: Option<usize>,

    /// The largest total one document may expand to, across every PDF stream
    /// and image frame its reader decodes, in bytes (default 1073741824). The
    /// per-item ceilings bound one item each; this bounds what a file of many
    /// of them adds up to
    #[arg(long, value_name = "N")]
    max_document_bytes: Option<u64>,

    /// The largest file one LibreOffice bridge conversion may hand back, in
    /// bytes (default 536870912): the bridge reads a small file and can write a
    /// huge one, which no check on the original would catch
    #[arg(long, value_name = "N")]
    max_bridge_output_bytes: Option<u64>,

    /// The wall-clock budget for one LibreOffice bridge conversion, in seconds
    /// (default 120). A conversion that outlives it is stopped and refused
    #[arg(long, value_name = "SECONDS")]
    bridge_timeout: Option<f64>,

    /// The wall-clock budget for one document's reading, in seconds (default
    /// 30). A read that outlives it is refused rather than finished
    #[arg(long, value_name = "SECONDS")]
    timeout: Option<f64>,

    /// The wall-clock budget for one document's OCR pages, in seconds (default
    /// 600). Recognition is seconds a page, so it has a budget of its own
    #[arg(long, value_name = "SECONDS")]
    ocr_timeout: Option<f64>,

    /// Include the notes layer — a deck's speaker notes and comments — in the
    /// Markdown, HTML and text outputs. Off, those three are a reading of the
    /// slides; JSON and YAML always carry the whole document
    #[arg(long)]
    include_notes: bool,

    /// How the Markdown and HTML outputs refer to pictures: `referenced`
    /// points at the images written beside the document, `embedded` carries
    /// their bytes as base64 `data:` URIs in the document itself, and
    /// `placeholder` writes `<!-- image -->` where each picture stood
    #[arg(long, value_enum, default_value_t = ImageExportMode::Referenced)]
    image_export_mode: ImageExportMode,

    /// How `--to chunks` splits a document: `hierarchical` makes one chunk per
    /// body item (a run of list items is one) carrying the heading path,
    /// `hybrid` packs sentences up to `--chunks-max-tokens`, carrying the tail
    /// of one chunk into the next
    #[arg(long, value_enum, default_value_t = ChunkType::Hierarchical)]
    chunks_type: ChunkType,

    /// The most tokens a `--to chunks --chunks-type hybrid` chunk may hold
    #[arg(long, default_value_t = 512, value_name = "N")]
    chunks_max_tokens: usize,

    /// Tokens of the previous hybrid chunk repeated at the start of the next,
    /// so a passage across a boundary is in both
    #[arg(long, default_value_t = 0, value_name = "N")]
    chunks_overlap_tokens: usize,

    /// Count hybrid chunk budgets with a real tokenizer file instead of the
    /// built-in approximation: a Hugging Face `tokenizer.json`, a SentencePiece
    /// `.model`, or a tiktoken `.tiktoken` vocabulary. The file is read and
    /// validated before conversion; it is not a model name fetched over the
    /// network
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath)]
    chunks_tokenizer: Option<PathBuf>,

    /// The tiktoken encoding family a `.tiktoken` vocabulary belongs to, when
    /// its filename does not name cl100k, o200k, p50k or r50k. Overrides a hint
    /// in the name; only used with `--chunks-tokenizer`
    #[arg(long, value_enum, value_name = "FAMILY")]
    chunks_tokenizer_encoding: Option<TokenizerEncoding>,

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

    /// Read only these pages: `1-4,9`, and `7-` for page 7 to the end (1-based,
    /// inclusive). A page is a PDF's page, a workbook's sheet (XLSX or ODS), a
    /// presentation's slide (PPTX or ODP), or an image's frame (a GIF's or an
    /// animated WebP's frame, a TIFF's directory).
    /// Applied before layout analysis, so the furniture and footnotes are found
    /// among the pages kept, and each page keeps the number it has in the file. A
    /// format with no pages is read whole, and the log says so
    #[arg(long, value_name = "RANGE", value_parser = page_range::PageRange::parse)]
    page_range: Option<page_range::PageRange>,

    /// The password of an encrypted PDF (user or owner) — refused here, because
    /// `ps` shows every argument to every local user. Put it in
    /// `PDFE_PDF_PASSWORD`, the supported route, or in a file named by
    /// `--pdf-password-file`. Without a password an encrypted PDF is opened with
    /// the empty user password, as every reader opens one that asks nothing; a
    /// file that needs a password is refused by name
    #[arg(
        long,
        value_name = "PASSWORD",
        env = "PDFE_PDF_PASSWORD",
        hide_env_values = true
    )]
    pdf_password: Option<String>,

    /// Read the PDF password from a file instead of the argument vector: the
    /// file's first line, without its line ending, is the password. This is the
    /// route that keeps the password out of `ps` and the shell history, where a
    /// `--pdf-password` value would be visible; it takes precedence over
    /// `PDFE_PDF_PASSWORD`
    #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
    pdf_password_file: Option<PathBuf>,

    /// When to run OCR on page images
    #[arg(long, value_enum, default_value_t = OcrMode::Auto)]
    ocr: OcrMode,

    /// OCR languages: comma-separated Tesseract codes (`eng,deu`), default `eng`.
    /// The recognised text only comes back if the language's data is installed
    /// (and, in a released package, shipped beside the binary)
    #[arg(long, default_value = "eng", value_name = "LANGS")]
    ocr_lang: String,

    /// Tesseract page segmentation mode, 0–13 (the engine's own default when
    /// absent; 6 is one block, 7 one line, 11 sparse text)
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=13), value_name = "N")]
    psm: Option<u8>,

    /// Remove the isolated pixels a fax leaves, before recognising each page.
    /// Off by default, because a born-digital page has none and every page
    /// pays for the pass
    #[arg(long)]
    ocr_despeckle: bool,

    /// Render PDF pages whose text is only vector art — no text layer and no
    /// picture — with `pdftoppm` and recognise the render. Off by default,
    /// because it is the one reading that needs a renderer installed; needs
    /// OCR on, so `--ocr auto` or `always`
    #[arg(long)]
    ocr_vector_pages: bool,

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

    /// Compare two versions of the same PDF or DOCX and write a page-aware
    /// HTML redline report with page and picture placement previews. `--input`
    /// is the newer version and this is the older one; `--output` names the
    /// report
    #[arg(long, value_name = "OLD", value_hint = ValueHint::AnyPath)]
    redline: Option<PathBuf>,
}

const MAX_PARALLEL: usize = 256;

/// Exit code for a document the shared budgets refused, as opposed to one that
/// could not be read: a caller that retries with a larger budget needs to tell
/// the two apart without parsing the message.
const EXIT_LIMIT_REFUSED: i32 = 3;

/// The budgets this run is held to: the defaults, with every `--max-*` and
/// `--timeout` flag applied before anything is read, so the PDF reader, the
/// packages, the flow formats, `.doc`, the image readers and the OCR pages are
/// all held to one value.
///
/// The value belongs to the run, not to the process: it is passed to each
/// conversion rather than installed in a global, so an embedder reading two
/// documents can hold each to its own ceiling.
///
/// The flags are named one per budget, so a refusal can point at the flag that
/// would raise it (`raise_flag`).
fn install_budgets(args: &Args) -> pdfextractor_limits::Limits {
    let mut limits = pdfextractor_limits::Limits::default();
    if let Some(bytes) = args.max_input_bytes {
        limits.max_input_bytes = bytes;
    }
    if let Some(pages) = args.max_pages {
        limits.max_pages = pages;
    }
    if let Some(objects) = args.max_pdf_objects {
        limits.max_pdf_objects = objects;
    }
    if let Some(tokens) = args.max_pdf_tokens {
        limits.max_pdf_tokens = tokens;
    }
    if let Some(depth) = args.max_pdf_nesting {
        limits.max_pdf_nesting = depth;
    }
    if let Some(pixels) = args.max_image_pixels {
        limits.max_image_pixels = pixels;
    }
    if let Some(dimension) = args.max_image_dimension {
        limits.max_image_dimension = dimension;
    }
    if let Some(bytes) = args.max_image_alloc_bytes {
        limits.max_image_alloc_bytes = bytes;
    }
    if let Some(frames) = args.max_image_frames {
        limits.max_image_frames = frames;
    }
    if let Some(blocks) = args.max_flow_blocks {
        limits.max_flow_blocks = blocks;
    }
    if let Some(bytes) = args.max_decoded_stream_bytes {
        limits.max_decoded_stream_bytes = bytes;
    }
    if let Some(entries) = args.max_zip_entries {
        limits.max_zip_entries = entries;
    }
    if let Some(bytes) = args.max_zip_entry_bytes {
        limits.max_zip_entry_bytes = bytes;
    }
    if let Some(bytes) = args.max_zip_total_bytes {
        limits.max_zip_total_bytes = bytes;
    }
    if let Some(ratio) = args.max_zip_ratio {
        limits.max_zip_ratio = ratio;
    }
    if let Some(bytes) = args.max_xml_bytes {
        limits.max_xml_bytes = bytes;
    }
    if let Some(depth) = args.max_xml_depth {
        limits.max_xml_depth = depth;
    }
    if let Some(bytes) = args.max_document_bytes {
        limits.max_document_bytes = bytes;
    }
    if let Some(bytes) = args.max_bridge_output_bytes {
        limits.max_bridge_output_bytes = bytes;
    }
    if let Some(seconds) = args.bridge_timeout {
        limits.bridge_timeout = std::time::Duration::from_secs_f64(seconds.max(0.0));
    }
    if let Some(seconds) = args.timeout {
        limits.timeout = std::time::Duration::from_secs_f64(seconds.max(0.0));
    }
    if let Some(seconds) = args.ocr_timeout {
        limits.ocr_timeout = std::time::Duration::from_secs_f64(seconds.max(0.0));
    }
    limits
}

/// Whether `error` is a budget refusal, at any depth of its context.
///
/// A refusal reaches the run one of two ways: as the pipeline's typed
/// [`pdfextractor_pipeline::PipelineError::Limit`], or as a [`LimitError`] from
/// a check the command line makes itself — the input size it holds a file to
/// before handing it over, the image turn a preview performs. Both are the
/// budgets saying no.
fn is_limit_refusal(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<pdfextractor_pipeline::PipelineError>(),
            Some(pdfextractor_pipeline::PipelineError::Limit(_))
        ) || cause
            .downcast_ref::<pdfextractor_limits::LimitError>()
            .is_some()
    })
}

/// The `error_kind` a `--progress` record carries for `error`.
///
/// A refusal is its own kind: the budgets said no, which is not the same failure
/// as a file that cannot be read, and a consumer deciding whether to retry needs
/// the two apart.
fn error_kind(error: &anyhow::Error) -> &'static str {
    if is_limit_refusal(error) {
        "limit"
    } else {
        "error"
    }
}

/// The flag that would raise the budget a refusal ran out of.
///
/// `None` for a refusal no flag here raises: a malformed part, a `DOCTYPE`, an
/// empty image, and a corrupt package.
fn raise_flag(error: &anyhow::Error) -> Option<&'static str> {
    use pdfextractor_limits::LimitError;
    let refusal = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<LimitError>())?;
    Some(match refusal {
        LimitError::InputTooLarge { .. } => "--max-input-bytes",
        LimitError::TooManyPages { .. } => "--max-pages",
        LimitError::TooManyPdfObjects { .. } => "--max-pdf-objects",
        LimitError::TooManyPdfTokens { .. } => "--max-pdf-tokens",
        LimitError::PdfTooDeep { .. } => "--max-pdf-nesting",
        LimitError::DecodedStreamTooLarge { .. } => "--max-decoded-stream-bytes",
        LimitError::TooManyZipEntries { .. } => "--max-zip-entries",
        LimitError::ZipEntryTooLarge { .. } => "--max-zip-entry-bytes",
        LimitError::ZipRatioTooHigh { .. } => "--max-zip-ratio",
        LimitError::ZipTotalTooLarge { .. } => "--max-zip-total-bytes",
        LimitError::XmlTooLarge { .. } => "--max-xml-bytes",
        LimitError::XmlTooDeep { .. } => "--max-xml-depth",
        LimitError::ImageTooWide { .. } => "--max-image-dimension",
        LimitError::ImageTooManyPixels { .. } => "--max-image-pixels",
        LimitError::ImageAllocTooLarge { .. } => "--max-image-alloc-bytes",
        LimitError::TooManyImageFrames { .. } => "--max-image-frames",
        LimitError::TooManyFlowBlocks { .. } => "--max-flow-blocks",
        LimitError::DocumentTooLarge { .. } => "--max-document-bytes",
        LimitError::BridgeOutputTooLarge { .. } => "--max-bridge-output-bytes",
        // Three deadlines share the `Timeout` variant, and the stage is what
        // tells them apart: recognition between pages, the bridge's conversion,
        // and everything else, which is the parser's own `--timeout`.
        LimitError::Timeout { stage, .. } if stage == "recognising a page" => "--ocr-timeout",
        LimitError::Timeout { stage, .. } if stage == "the LibreOffice bridge" => {
            "--bridge-timeout"
        }
        LimitError::Timeout { .. } => "--timeout",
        _ => return None,
    })
}

/// The suffix a refusal's message carries: the flag that would raise it.
fn flag_hint(error: &anyhow::Error) -> String {
    match raise_flag(error) {
        Some(flag) => format!(" (raise {flag} to allow it)"),
        None => String::new(),
    }
}

/// The password written in `path`, for `--pdf-password-file`.
///
/// A file is the route that keeps the secret out of the argument vector and the
/// shell history, where a `--pdf-password` value is visible to anyone who runs
/// `ps`. The file's first line is the password and its line ending is not: a
/// trailing `LF` or `CRLF` is how a text file ends a line, not part of what was
/// typed. Nothing after that line is read, so a password may hold spaces but
/// not a newline, and an empty file is the empty user password rather than an
/// error — an unreadable file is the error, so a missing secret is never
/// mistaken for no secret.
fn read_password_file(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("cannot read the password file {}", path.display()))?;
    let text = String::from_utf8(bytes)
        .with_context(|| format!("the password file {} is not UTF-8", path.display()))?;
    let line = match text.split_once('\n') {
        Some((line, _)) => line,
        None => text.as_str(),
    };
    Ok(line.strip_suffix('\r').unwrap_or(line).to_string())
}

fn main() -> anyhow::Result<()> {
    match run() {
        Ok(()) => Ok(()),
        Err(error) => {
            if is_limit_refusal(&error) {
                eprintln!("error: {error:#}{}", flag_hint(&error));
                std::process::exit(EXIT_LIMIT_REFUSED);
            }
            Err(error)
        }
    }
}

fn run() -> anyhow::Result<()> {
    // Log records go to stderr, not the default stdout: stdout carries the
    // `--progress` records, where a stray line of prose would be read as a
    // malformed record by whatever is streaming the run.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let matches = Args::command().get_matches();
    let mut args = Args::from_arg_matches(&matches)?;
    // `ps` shows every argument to every local user, so a password typed on the
    // command line is not accepted at all — not even with a warning, which would
    // still have put it there. The environment is the supported route, and a
    // named file is the other one.
    if matches.value_source("pdf_password") == Some(clap::parser::ValueSource::CommandLine) {
        anyhow::bail!(
            "--pdf-password puts the password in the process argument list, where any local \
             user can read it with `ps`; put it in PDFE_PDF_PASSWORD instead, or name a file \
             with --pdf-password-file"
        );
    }
    // A password file is read before the password is used and before the
    // variable is removed: it is the more specific of the two sources.
    if let Some(path) = args.pdf_password_file.as_deref() {
        args.pdf_password = Some(read_password_file(path)?);
    }
    // The password is in the arguments now; the variable is not needed for the
    // rest of this process, and any child spawned below (LibreOffice, pdftoppm)
    // would otherwise inherit a secret about a document it may know nothing
    // about. Removing it here is the single point that covers every child.
    std::env::remove_var(pdfextractor_formats::PDF_PASSWORD_ENV);
    let limits = install_budgets(&args);

    // A consumer asking what this build can take should not have to hold an
    // input path, run a conversion, or parse help text or a refusal message:
    // the set is printed as JSON and the run ends here.
    if args.list_input_formats {
        let formats = serde_json::to_string_pretty(&input_formats())?;
        write_stdout(&format!("{formats}\n"))?;
        return Ok(());
    }

    // Without the flag the input is what the run is for. Clap cannot require it
    // itself, because the flag above has to work with no input at all.
    let Some(input) = args.input.as_deref() else {
        anyhow::bail!("-i, --input <PATH> is required unless --list-input-formats is given");
    };

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

    if !input.exists() {
        anyhow::bail!("input path does not exist: {}", input.display());
    }

    // Redline is a two-document mode of its own: it converts both versions and
    // writes one report, rather than running the directory/format/compare
    // pipeline. Handle it before the input is expanded into a queue.
    if let Some(old) = args.redline.as_deref() {
        return run_redline(old, input, &args, &limits);
    }

    let found = collect_inputs(input, args.recursive)?;
    let targets = found.documents;
    if targets.is_empty() {
        anyhow::bail!(nothing_to_read(input, &found.carried));
    }

    let formats = output_formats(&args.to);
    // A chunk budget that cannot be met is a broken configuration, refused
    // before a document is converted rather than half a run later.
    let chunks = chunk_options(&args)?;

    // `-o -` is stdout. The stream is the document, so nothing else may be
    // printed to it, and the compare mode needs files on disk to hold the run
    // to — each of those is a broken combination to refuse up front, before a
    // single document is converted.
    if args.output.as_deref().is_some_and(|o| o.as_os_str() == "-") {
        if args.progress {
            anyhow::bail!("--progress and `-o -` both write stdout; use one or the other");
        }
        if args.preview {
            anyhow::bail!("--preview needs an output path; it cannot be written to stdout");
        }
        if args.compare.is_some() {
            anyhow::bail!("--compare holds the run to files on disk; `-o -` writes stdout");
        }
        if targets.len() != 1 {
            anyhow::bail!(
                "`-o -` writes one document to stdout; {} inputs were given",
                targets.len()
            );
        }
        if formats.len() != 1 {
            anyhow::bail!(
                "`-o -` writes one format to stdout; {} were asked for",
                formats.len()
            );
        }
        // Images an extraction still leaves beside its output land beside the
        // input, which is where the default output would have gone.
        let target = &targets[0];
        let base = target.with_extension(formats[0].ext());
        let doc = build_document(target, &base, &args, &limits)?;
        // A reader of stdout has no other sign that picture files appeared next
        // to the source, so say where they went — but only when this conversion
        // wrote one: no pictures leaves no folder, a vector figure has no
        // pixels to write, and `embedded` keeps the pictures in the stream.
        let wrote_pictures = args.image_export_mode == ImageExportMode::Referenced
            && doc.pictures.iter().any(|picture| {
                picture
                    .image
                    .as_ref()
                    .is_some_and(|image| image.uri.is_some())
            });
        if wrote_pictures {
            // A note rather than a log record: it is news for the person who
            // typed the command, and the default log filter would swallow it.
            eprintln!(
                "note: `-o -` wrote extracted pictures into {}; `--image-export-mode \
                 embedded` keeps the pictures in the stream",
                image_dir_for(&base).display()
            );
        }
        let text = match formats[0] {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&doc)?),
            OutputFormat::Md => pdfextractor_md_exporter::render_with_images(
                &doc,
                args.include_notes,
                args.image_export_mode.mode(),
                base.parent(),
            ),
            OutputFormat::Html => pdfextractor_md_exporter::html::render_with_images(
                &doc,
                args.include_notes,
                args.image_export_mode.mode(),
                base.parent(),
            ),
            OutputFormat::Text => pdfextractor_md_exporter::text::render_with_notes(
                &doc,
                args.text_dehyphenate,
                args.include_notes,
            ),
            OutputFormat::Yaml => pdfextractor_md_exporter::yaml::render(&doc)?,
            OutputFormat::Chunks => pdfextractor_md_exporter::chunks::to_jsonl(&doc, &chunks)?,
        };
        // The stream is the document, so it is written once and a consumer
        // that stops reading ends the run quietly (see `write_stdout`).
        write_stdout(&text)?;
        return Ok(());
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

    // A comparison run leaves out what this machine cannot read the way the
    // memory's numbers were read, and decides **before** converting anything: a
    // family the bridge is the only reader for has no reading here to leave out
    // afterwards, so a rule that waited for a finished document would have the run
    // fail on the missing bridge instead of on what the gate is about. A run that
    // is only converting keeps every document — there the missing reader *is* the
    // failure, and it says so with the re-save route.
    let (left_out, targets): (Vec<(PathBuf, &'static str)>, Vec<PathBuf>) = match &comparing {
        Some(_) => {
            let mut left_out = Vec::new();
            let mut held = Vec::with_capacity(targets.len());
            for path in targets {
                match compare::not_held_here(&path) {
                    Some(why) => left_out.push((path, why)),
                    None => held.push(path),
                }
            }
            (left_out, held)
        }
        None => (Vec::new(), targets),
    };

    tracing::info!(
        "{} document(s) to convert, parallelism {}",
        targets.len(),
        parallel
    );

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(parallel)
        .build()?;

    let outputs = resolve_outputs(&targets, &args, input);
    let total = targets.len();

    // A plan precedes the first record so a parent process can show the whole
    // queue before any of it has finished.
    if args.progress {
        // A plan precedes the first record so a parent process can show the
        // whole queue before any of it has finished.
        write_progress_record(json!({
            "kind": "plan",
            "total": total,
            "inputs": targets.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        }));
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
                let result = convert_one(path, output, &args, &formats, &chunks, &limits);
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
        Some(session) => compare::check(session, &results, &left_out),
        None => Vec::new(),
    };

    if failures > 0 {
        // A refusal is the budgets saying no, not a file that cannot be read:
        // when that is the only thing that went wrong, the run says so in its
        // exit code as well as in the message.
        let all_refused = results
            .iter()
            .filter_map(|(_, result)| result.as_ref().err())
            .all(is_limit_refusal);
        if all_refused {
            // When every refusal is the same budget, the summary can name the
            // one flag that would read them all.
            let flags: Vec<&str> = results
                .iter()
                .filter_map(|(_, result)| result.as_ref().err())
                .filter_map(raise_flag)
                .collect();
            let hint = match flags.first() {
                Some(flag) if flags.iter().all(|other| other == flag) => {
                    format!(" (raise {flag} to allow them)")
                }
                _ => String::new(),
            };
            eprintln!(
                "{failures} of {} document(s) were refused by the shared budgets{hint}",
                results.len()
            );
            std::process::exit(EXIT_LIMIT_REFUSED);
        }
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

/// Convert two versions of one PDF or DOCX, compare them through the same
/// `diff` engine the Compare panel uses, and write the redline HTML report.
///
/// The older version is `old` and the newer one is `new` (`--input`). Only the
/// two document families whose readers publish enough structure for the
/// comparison are accepted: a PDF's page provenance makes the report page-aware
/// and gives it placement-box previews, and a DOCX is compared as the flow
/// document it is.
fn run_redline(
    old: &Path,
    new: &Path,
    args: &Args,
    limits: &pdfextractor_limits::Limits,
) -> anyhow::Result<()> {
    if args.compare.is_some() {
        anyhow::bail!("--redline and --compare are separate modes; use one or the other");
    }
    if args.preview {
        anyhow::bail!("--redline writes its own HTML report; drop --preview");
    }
    if args.progress {
        anyhow::bail!("--redline writes one report; it has no progress stream");
    }
    if !args.to.is_empty() {
        anyhow::bail!("--redline writes its own HTML report; drop --to");
    }
    if args
        .output
        .as_deref()
        .is_some_and(|path| path.as_os_str() == "-")
    {
        anyhow::bail!("--redline writes an HTML file; `-o -` is not a report path");
    }
    for (role, path) in [("old", old), ("new", new)] {
        if !path.exists() {
            anyhow::bail!("redline {role} version does not exist: {}", path.display());
        }
        if !path.is_file() {
            anyhow::bail!(
                "redline {role} version must be a file, not a directory: {}",
                path.display()
            );
        }
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("read the size of {}", path.display()))?;
        limits.check_input(usize::try_from(metadata.len()).unwrap_or(usize::MAX))?;
    }

    let family = match (detect(old)?, detect(new)?) {
        (Format::Pdf, Format::Pdf) => "PDF",
        (Format::Docx, Format::Docx) => "DOCX",
        (old_format, new_format) => anyhow::bail!(
            "--redline compares two versions of the same PDF or DOCX; the old file is {old_format:?} and the new file is {new_format:?}"
        ),
    };

    let output = args
        .output
        .clone()
        .unwrap_or_else(|| new.with_extension("redline.html"));
    let output = if output.extension().is_none() {
        output.with_extension("html")
    } else {
        output
    };
    if same_path(&output, old) || same_path(&output, new) {
        anyhow::bail!(
            "the redline report would overwrite one of its inputs: {}",
            output.display()
        );
    }

    let scratch = RedlineScratch::new()?;
    let old_doc = build_document(old, &scratch.path("old.json"), args, limits)?;
    let new_doc = build_document(new, &scratch.path("new.json"), args, limits)?;
    let old_name = file_name(old);
    let new_name = file_name(new);
    let html = redline::render(&old_doc, &new_doc, &old_name, &new_name, family)?;
    redline::write(&output, &html)?;
    eprintln!(
        "redline: {} against {} -> {}",
        new.display(),
        old.display(),
        output.display()
    );
    Ok(())
}

/// Whether two paths name the same file, including when only one of them can
/// be canonicalized because the report does not exist yet.
fn same_path(one: &Path, other: &Path) -> bool {
    if one == other {
        return true;
    }
    match (resolved_path(one), resolved_path(other)) {
        (Some(one), Some(other)) => one == other,
        _ => false,
    }
}

/// A path with its parent resolved even when the file itself does not exist
/// yet, so a report path cannot overwrite an input through a symlinked folder.
fn resolved_path(path: &Path) -> Option<PathBuf> {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return Some(resolved);
    }
    let parent = path.parent()?;
    let parent = std::fs::canonicalize(parent).ok()?;
    Some(parent.join(path.file_name()?))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A scratch directory for the two conversions' extracted images, removed when
/// the report is finished.
///
/// The report is text, so the pictures the converters write are not referenced;
/// keeping them out of the user's folder is the point of the directory.
struct RedlineScratch(PathBuf);

impl RedlineScratch {
    fn new() -> anyhow::Result<Self> {
        // Private and freshly created: the extracted pictures are the
        // document's content, and another local user has no business reading
        // them while the comparison runs.
        let path = pdfextractor_formats::private_scratch_dir("redline")
            .with_context(|| "create the redline scratch directory")?;
        Ok(Self(path))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for RedlineScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
                write_progress_record(json!({
                    "kind": "progress",
                    "done": seen,
                    "total": total,
                    "input": input.display().to_string(),
                    "ok": true,
                    "output": output.display().to_string(),
                }));
            } else {
                tracing::info!("{} -> {}", input.display(), output.display());
            }
        }
        Err(err) => {
            if args.progress {
                write_progress_record(json!({
                    "kind": "progress",
                    "done": seen,
                    "total": total,
                    "input": input.display().to_string(),
                    "ok": false,
                    "error_kind": error_kind(err),
                    "limit_flag": raise_flag(err),
                    "error": format!("{err:#}"),
                }));
            }
            eprintln!("error: {}: {err:#}{}", input.display(), flag_hint(err));
        }
    }
}

/// Write text to stdout, treating a closed pipe as the end of the stream.
///
/// `print!`/`println!` panic when the reader goes away, so
/// `pdfextractor -i paper.pdf -o - | head` ended in a Rust panic (exit 101)
/// once the first screenful had been read. A consumer that stops reading is not
/// a failure of the conversion: the write stops, quietly, with the run's own
/// status.
fn write_stdout(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
            Err(error).context("write to stdout")
        }
        // Written, or written as far as a reader that has gone away wanted.
        _ => Ok(()),
    }
}

/// Write one `--progress` record, dropping a reader that has gone away: the
/// stream is the parent's view of the run, not the run's result, so a closed
/// pipe stops the reports rather than the conversions.
fn write_progress_record(record: serde_json::Value) {
    let _ = write_stdout(&format!("{record}\n"));
}

/// What a walk of the input found.
///
/// The second list is pictures this tool carries but cannot read as pages — an
/// SVG, a metafile. They are not inputs, but a folder holding nothing else is
/// not *empty*, and the refusal has to say so: "no supported documents" over a
/// folder of drawings reads as if the walk had found nothing at all.
struct Found {
    documents: Vec<PathBuf>,
    carried: Vec<PathBuf>,
}

/// The refusal of a folder with nothing in it this run can convert.
///
/// The accepted set is printed in full — it is the one place a run says what it
/// takes — and the carried pictures the walk passed over are named with it,
/// counted, with the reason they are not pages, and with the first few shown.
fn nothing_to_read(root: &Path, carried: &[PathBuf]) -> String {
    // The list is the accepted set itself, so it cannot name a picture the
    // reader has stopped reading or miss one the shared table has added.
    let mut message = format!(
        "no supported documents ({}) found under {}",
        accepted_extensions().join(", "),
        root.display()
    );
    if carried.is_empty() {
        return message;
    }

    // Enough names to show what is there, not a listing: a folder of a thousand
    // drawings is one fact, and its count says the rest.
    const NAMED: usize = 6;
    let names = carried
        .iter()
        .take(NAMED)
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(", ");
    let more = carried.len().saturating_sub(NAMED);
    let rest = if more == 0 {
        String::new()
    } else {
        format!(" and {more} more")
    };
    let (what, page) = if carried.len() == 1 {
        ("one picture".to_string(), "a page")
    } else {
        (format!("{} pictures", carried.len()), "pages")
    };
    message.push_str(&format!(
        " — it does hold {what} this tool carries but does not read as {page} on its own: \
         {names}{rest}; a picture inside a document is extracted, while a standalone \
         picture has to be one of the formats above"
    ));
    message
}

fn collect_inputs(input: &Path, recursive: bool) -> anyhow::Result<Found> {
    if input.is_file() {
        return Ok(Found {
            documents: vec![input.to_path_buf()],
            carried: Vec::new(),
        });
    }
    let mut found = Found {
        documents: Vec::new(),
        carried: Vec::new(),
    };
    // `None` means unlimited depth (recursive); `Some(0)` means top level only.
    let max_depth = if recursive { None } else { Some(0usize) };
    let mut visited = HashSet::new();
    walk_dir(input, max_depth, &mut found, &mut visited)?;
    found.documents.sort();
    found.carried.sort();
    Ok(found)
}

fn walk_dir(
    dir: &Path,
    depth_left: Option<usize>,
    found: &mut Found,
    visited: &mut HashSet<PathBuf>,
) -> anyhow::Result<()> {
    // A folder that leads back to one already walked is not new work: a symlink
    // to its own parent used to be followed until the kernel stopped resolving
    // it, converting the same document forty times. Identity is the resolved
    // path, so an alias and its target are one folder.
    let resolved = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(resolved) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            if is_supported(&path) {
                found.documents.push(path);
            } else if is_carried_picture(&path) {
                found.carried.push(path);
            }
            continue;
        }
        let child_depth = match depth_left {
            None => None,
            Some(0) => continue,
            Some(n) => Some(n - 1),
        };
        walk_dir(&path, child_depth, found, visited)?;
    }
    Ok(())
}

/// The extensions this tool accepts that are not pictures: the document and
/// flow readers, and the legacy formats the bridge promises.
///
/// An extension is a promise the run makes about the file; what is *in* the
/// file decides which reader it goes to (see `formats`). So an extension must
/// be listed here even when the bytes are what matter — a DOCX called
/// `report.wps` needs `wps` to be accepted before its content can name the
/// reader. Pictures are not here: their extensions come from the shared
/// picture table, so a format it carries as a readable page is one the CLI
/// accepts without another edit.
const DOCUMENT_EXTENSIONS: &[&str] = &[
    "pdf", "docx", "odt", "ods", "odp", "xlsx", "pptx", "ppt", "doc", "wps", "wp", "wks", "wdb",
    "wpd", "xls", "rtf", "html", "htm", "txt", "md", "markdown", "csv", "tsv",
];

/// The picture extensions an input can be read under: every format the shared
/// table carries whose media type the image reader reads as a page.
///
/// An SVG or a metafile is carried *inside* a document but is not a document on
/// its own, so it is accepted here only when the reader that turns an image
/// into a page can read it — as a GIF, a PNG or a JPEG is, and a GIF is a page
/// by its logical screen, with a page for each frame of an animation.
fn picture_input_extensions() -> impl Iterator<Item = &'static str> {
    pdfextractor_picture_formats::PICTURE_FORMATS
        .iter()
        .filter(|format| pdfextractor_image_parser::is_document_media_type(format.mime))
        .flat_map(|format| std::iter::once(format.extension).chain(format.read_as.iter().copied()))
}

/// Every extension this tool accepts, in the order it names them to a user.
fn supported_extensions() -> Vec<&'static str> {
    DOCUMENT_EXTENSIONS
        .iter()
        .copied()
        .chain(picture_input_extensions())
        .collect()
}

/// The accepted extensions written the way a user names a file: `.pdf`, `.jpg`,
/// … One spelling of the list, shared by `--help` and the empty-directory
/// message, so the two cannot come to disagree about what this run takes.
fn accepted_extensions() -> Vec<String> {
    supported_extensions()
        .into_iter()
        .map(|extension| format!(".{extension}"))
        .collect()
}

/// One family of input formats: the label a reader looks a format up by, and the
/// encodings under it.
///
/// It is the same table `--help` prints and `--list-input-formats` publishes, so
/// a picker built from the JSON's groups offers the names under the headings the
/// help uses rather than a second arrangement of the same set.
struct InputFamily {
    name: &'static str,
    formats: Vec<InputFormat>,
}

impl InputFamily {
    /// The family's names as a user writes a file's: `.pdf`, `.docx`, … with a
    /// picture's aliases as names of their own (`.jpg` and `.jpeg`), because a
    /// caller filtering by them needs the spelling on the file.
    fn names(&self) -> Vec<String> {
        self.formats
            .iter()
            .flat_map(|format| {
                std::iter::once(format.extension).chain(format.aliases.iter().copied())
            })
            .map(|extension| format!(".{extension}"))
            .collect()
    }
}

/// One encoding inside a family.
struct InputFormat {
    /// The extension the format is written under.
    extension: &'static str,
    /// Other extensions that name the same encoding: a `.jpeg` file is a `.jpg`.
    aliases: &'static [&'static str],
    /// Read through the LibreOffice bridge, which must be installed. `--help`
    /// marks such a name with `*`; the JSON leaves the mark out and keeps the
    /// name plain, because a machine reading it wants the extension and not a
    /// piece of typography.
    bridged: bool,
}

/// The document families `--help` groups the accepted extensions into, in the
/// order it prints them: a reader asking "can it take a deck?" looks under
/// PowerPoint rather than through a line of twenty-eight names.
///
/// These are the non-picture names of [`DOCUMENT_EXTENSIONS`], in families a
/// user recognises; the picture encodings come from the shared table. The
/// grouped list is checked against the gate, so a format added to either source
/// fails a test rather than going unmentioned.
const DOCUMENT_FAMILIES: &[(&str, &[&str])] = &[
    ("PDF", &["pdf"]),
    ("Word", &["docx", "doc"]),
    ("Excel", &["xlsx", "xls"]),
    ("PowerPoint", &["pptx", "ppt"]),
    ("OpenDocument", &["odt", "ods", "odp"]),
    ("Works/WordPerfect", &["wps", "wp", "wks", "wdb", "wpd"]),
    (
        "Text and markup",
        &["rtf", "html", "htm", "txt", "md", "markdown", "csv", "tsv"],
    ),
];

/// The accepted formats grouped the way the help lists them: the document
/// families above, then the pictures from the shared table in that table's own
/// order.
///
/// One grouping for both printings — the tail of `--help` and the JSON of
/// `--list-input-formats` — so a grouped picker built from the JSON cannot offer
/// a shape of the set the help does not describe.
fn input_families() -> Vec<InputFamily> {
    let documents = DOCUMENT_FAMILIES
        .iter()
        .map(|(name, extensions)| InputFamily {
            name,
            formats: extensions
                .iter()
                .map(|extension| InputFormat {
                    extension,
                    aliases: &[],
                    bridged: is_bridged(extension),
                })
                .collect(),
        });
    let pictures = InputFamily {
        name: "Pictures",
        formats: pdfextractor_picture_formats::PICTURE_FORMATS
            .iter()
            .filter(|format| pdfextractor_image_parser::is_document_media_type(format.mime))
            .map(|format| InputFormat {
                extension: format.extension,
                aliases: format.read_as,
                bridged: false,
            })
            .collect(),
    };
    documents.chain(std::iter::once(pictures)).collect()
}

/// The tail of `--help`: the input formats this run accepts, grouped by the
/// family a reader looks them up by, and generated from the gate's own lists
/// rather than written out beside them.
///
/// A picture format's aliases sit beside the name it is written under
/// (`.jpg/.jpeg`), so the line is one entry per encoding. A `*` marks a name the
/// LibreOffice bridge reads, with the footnote saying so; the empty-folder
/// message keeps the flat list, and `--list-input-formats` publishes the same
/// groups as plain names, because a machine parsing either should not have to
/// know this layout.
fn accepted_formats_help() -> String {
    let rows: Vec<(&str, String)> = input_families()
        .iter()
        .map(|family| {
            let names = family
                .formats
                .iter()
                .map(|format| {
                    let mut name = std::iter::once(format.extension)
                        .chain(format.aliases.iter().copied())
                        .map(|extension| format!(".{extension}"))
                        .collect::<Vec<_>>()
                        .join("/");
                    if format.bridged {
                        name.push('*');
                    }
                    name
                })
                .collect::<Vec<_>>()
                .join(", ");
            (family.name, names)
        })
        .collect();

    // One label width for the whole block, so the names line up whichever
    // family is longest.
    let width = rows
        .iter()
        .map(|(family, _)| family.len() + 1)
        .max()
        .unwrap_or(0);
    let mut lines = vec!["Accepted input formats (a picture is one page):".to_string()];
    lines.extend(rows.iter().map(|(family, names)| {
        let label = format!("{family}:");
        format!("  {label:<width$}  {names}")
    }));
    lines.push("  * read through LibreOffice, which must be installed".to_string());
    lines.join("\n")
}

/// The accepted input formats as a machine reads them: the same set `--help`
/// and the empty-folder message print, under a key a consumer can name.
///
/// The extensions are the run's own [`supported_extensions`] — the list the gate
/// itself reads — so this cannot drift from what a run actually takes, and a
/// caller never has to parse help text or a refusal message, both of which are
/// written for a person and free to be reworded.
///
/// `families` is the same set under the headings the help uses, one family per
/// heading in the help's order, so a picker can be built with groups without
/// knowing what a family is called or which names belong to one: every name is
/// in exactly one family, and the families together are `input_formats`. The
/// flat list keeps its own order — documents first, then pictures — for a caller
/// that only filters.
fn input_formats() -> serde_json::Value {
    let families: Vec<serde_json::Value> = input_families()
        .iter()
        .map(|family| json!({ "name": family.name, "input_formats": family.names() }))
        .collect();
    json!({ "input_formats": accepted_extensions(), "families": families })
}

/// Whether an already lower-cased `ext` is one this tool accepts.
fn is_supported_extension(ext: &str) -> bool {
    DOCUMENT_EXTENSIONS.contains(&ext)
        || pdfextractor_picture_formats::format_for_extension(ext)
            .is_some_and(|format| pdfextractor_image_parser::is_document_media_type(format.mime))
}

/// Why a file this tool will not read is still a picture it knows.
///
/// An SVG or a Windows metafile is a format the extractor carries: one
/// embedded in a document is written out beside the JSON. Neither is a page on
/// its own — an SVG has no raster this reader lays out, and a metafile's pixels
/// are a drawing played back — so a run on such a file is refused, and
/// "unsupported extension: svg" reads as if the format were unknown when it is
/// one this tool names. `None` for a name this knows nothing about, which keeps
/// the plain message for a plain stranger.
fn carried_picture_not_a_page(ext: &str) -> Option<String> {
    let format = pdfextractor_picture_formats::format_for_extension(ext)?;
    if pdfextractor_image_parser::is_document_media_type(format.mime) {
        return None;
    }
    let pages = pdfextractor_picture_formats::PICTURE_FORMATS
        .iter()
        .filter(|format| pdfextractor_image_parser::is_document_media_type(format.mime))
        .map(|format| format!(".{}", format.extension))
        .collect::<Vec<_>>();
    Some(format!(
        "unsupported extension: {ext} — this is a picture format this tool carries ({}), but a \
         .{ext} is not read as a page on its own: a picture embedded in a document is extracted, \
         while a standalone image has to be one this reader can turn into a page ({}). Convert \
         it to one of those, or put it in a document",
        format.mime,
        pages.join(", ")
    ))
}

/// What a refused file's bytes say it is, and the name that would read them.
///
/// The extension gate refuses before any reader looks at the file, so a PDF
/// saved as `report.xyz` was told only that `.xyz` is not accepted — true, and
/// useless when the bytes are a format this tool reads well under its own name.
/// [`detect`] already reads the magic for the reader choice; this turns its
/// answer into the sentence a refusal needs: what the file is, called by a
/// name a user recognises, and what to do about it.
///
/// `None` when the bytes say nothing more than the name did — a stranger stays
/// a plain stranger — or when the file cannot be read at all, in which case the
/// name is the only thing there was to go on.
fn bytes_say_what_it_is(path: &Path, ext: &str) -> Option<String> {
    let (description, advice) = match detect(path).ok()? {
        Format::Pdf => ("a PDF", Some("rename the file to .pdf".to_string())),
        Format::Docx => (
            "a Word document (DOCX)",
            Some("rename the file to .docx".to_string()),
        ),
        Format::Odt => (
            "an OpenDocument text document (ODT)",
            Some("rename the file to .odt".to_string()),
        ),
        Format::Ods => (
            "an OpenDocument spreadsheet (ODS)",
            Some("rename the file to .ods".to_string()),
        ),
        Format::Odp => (
            "an OpenDocument presentation (ODP)",
            Some("rename the file to .odp".to_string()),
        ),
        Format::Xlsx => (
            "an Excel workbook (XLSX)",
            Some("rename the file to .xlsx".to_string()),
        ),
        Format::Pptx => (
            "a PowerPoint deck (PPTX)",
            Some("rename the file to .pptx".to_string()),
        ),
        Format::Doc => (
            "a Word 97-2003 document",
            Some("rename the file to .doc".to_string()),
        ),
        // An image is named by the encoding its signature declares, with the
        // extension it is read under — the same table the extractor writes a
        // picture out with, so the two cannot disagree.
        Format::Image { mimetype } => {
            let name = match mimetype {
                "image/png" => "a PNG image",
                "image/jpeg" => "a JPEG image",
                "image/tiff" => "a TIFF image",
                "image/bmp" => "a BMP image",
                "image/webp" => "a WebP image",
                "image/gif" => "a GIF image",
                "image/jp2" => "a JPEG 2000 image",
                _ => "an image",
            };
            let extension = pdfextractor_picture_formats::extension_for_mime(mimetype)?;
            (name, Some(format!("rename the file to .{extension}")))
        }
        Format::Flow(reader) => {
            let (name, extension) = match reader {
                "rtf" => ("an RTF document", "rtf"),
                "html" => ("an HTML page", "html"),
                "md" => ("a Markdown document", "md"),
                "csv" => ("a CSV table", "csv"),
                "tsv" => ("a TSV table", "tsv"),
                _ => ("plain text", "txt"),
            };
            (name, Some(format!("rename the file to .{extension}")))
        }
        // A format LibreOffice reads is named by the phrase its own detector
        // already uses; renaming cannot be promised, because the bridge is
        // keyed by the extension the file had, so the way out is the re-save.
        Format::Bridge { target, label, .. } => (
            label,
            Some(format!("re-save it as .{}", target.extension())),
        ),
        Format::Unsupported(label) => (label, None),
    };
    let name = if ext.is_empty() { "none" } else { ext };
    Some(match advice {
        Some(advice) => format!(
            "unsupported extension: {name} — the bytes are {description}, which this tool reads: \
             {advice}"
        ),
        None => format!("unsupported extension: {name} — the bytes are {description}"),
    })
}

fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| is_supported_extension(&ext))
}

/// Whether a path is a picture this tool carries but cannot read as a page —
/// an SVG, a metafile. The same test [`carried_picture_not_a_page`] makes,
/// asked of a path rather than a name.
fn is_carried_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| carried_picture_not_a_page(&ext).is_some())
}

/// Convert one input into a document, without writing it out.
///
/// The command line's gate lives here: the extension must be one this tool
/// promises to read, and a refusal says what the bytes actually are when they
/// name a format this reader knows under another name. The conversion itself is
/// the pipeline crate's, so an embedder can call it without the gate or the
/// argument parser.
fn build_document(
    path: &Path,
    output_path: &Path,
    args: &Args,
    limits: &pdfextractor_limits::Limits,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    if args.ocr_vector_pages && args.ocr == OcrMode::Never {
        anyhow::bail!(
            "--ocr-vector-pages reads a rendered page with OCR: pass --ocr auto or `always`"
        );
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let metadata =
        std::fs::metadata(path).with_context(|| format!("read the size of {}", path.display()))?;
    limits.check_input(usize::try_from(metadata.len()).unwrap_or(usize::MAX))?;
    if !is_supported_extension(&ext) {
        match carried_picture_not_a_page(&ext) {
            Some(reason) => anyhow::bail!(reason),
            None => anyhow::bail!(bytes_say_what_it_is(path, &ext)
                .unwrap_or_else(|| format!("unsupported extension: {ext}"))),
        }
    }

    // The naming is the command line's: pictures land in a folder beside the
    // output, named for the document, and `--preview` writes its overlay beside
    // the output as well. The reading is only told where those are.
    let image_base = image_dir_for(output_path);
    let options = pdfextractor_pipeline::ConvertOptions {
        ocr: match args.ocr {
            OcrMode::Auto => pdfextractor_pipeline::OcrMode::Auto,
            OcrMode::Always => pdfextractor_pipeline::OcrMode::Always,
            OcrMode::Never => pdfextractor_pipeline::OcrMode::Never,
        },
        ocr_lang: &args.ocr_lang,
        psm: args.psm,
        despeckle: args.ocr_despeckle,
        vector_pages: args.ocr_vector_pages,
        page_range: args.page_range.as_ref(),
        pdf_password: args.pdf_password.as_deref(),
        image_base: Some(&image_base),
        preview: args.preview.then_some(output_path),
        preview_dpi: args.preview_dpi,
        limits: limits.clone(),
    };
    pdfextractor_pipeline::build_document(path, &options).map_err(flag_named)
}

/// The pipeline's typed refusal as a user reads it.
///
/// The library says what happened; the sentence that names `--page-range` or
/// `--ocr-lang`, and the password sources this binary accepts, are the command
/// line's words, so they are written here rather than in a crate an embedder
/// also links.
fn flag_named(error: pdfextractor_pipeline::PipelineError) -> anyhow::Error {
    use pdfextractor_pipeline::{Encrypted, PipelineError};
    match error {
        PipelineError::OcrLangEmpty => {
            anyhow::anyhow!("--ocr-lang needs at least one language code, e.g. `eng`")
        }
        PipelineError::PageRange {
            path,
            range,
            total,
            unit,
        } => {
            anyhow::anyhow!(
                "--page-range {range} names no {unit} of {} ({})",
                path.display(),
                unit.counted(total)
            )
        }
        PipelineError::RendererMissing { page_no } => anyhow::anyhow!(
            "--ocr-vector-pages: page {page_no} has no text layer and no picture — its text is \
             drawn as vector art — and rendering it needs `pdftoppm` (install poppler, or name the \
             program with PDFE_PDFTOPPM)"
        ),
        PipelineError::Encrypted {
            path,
            why: Encrypted::NeedsPassword,
        } => anyhow::anyhow!(
            "{}: this PDF is encrypted and needs a password: pass it in PDFE_PDF_PASSWORD, or \
             name a file with --pdf-password-file",
            path.display()
        ),
        other => anyhow::Error::new(other),
    }
}

/// Directory holding a document's extracted images, sibling to its output.
///
/// This is the command line's convention — `notes.json` puts its pictures in
/// `notes_images/` — so an embedder names its own folder rather than inheriting
/// ours.
fn image_dir_for(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    output_path.with_file_name(format!("{stem}_images"))
}

fn convert_one(
    path: &Path,
    base: &Path,
    args: &Args,
    formats: &[OutputFormat],
    chunks: &pdfextractor_md_exporter::chunks::ChunkOptions,
    limits: &pdfextractor_limits::Limits,
) -> anyhow::Result<PathBuf> {
    // Every format is written beside the same stem: `-o out.json --to md`
    // writes `out.json` and `out.md`, and the first format asked for is the
    // run's primary output — the one reported to `--progress` and named in
    // the log line.
    let primary = base.with_extension(formats[0].ext());
    // A target may be the input itself: `-i notes.md --to md` normalizes the
    // Markdown in place, and the whole document is read before any of it is
    // written (each writer also replaces its file through a temporary, so a
    // failed write leaves the source whole).
    //
    // The output's folder is created here, once: a conversion writes pictures
    // beside the output and the writers put the document there, so the first
    // `-o out/nested/doc.json` has nothing to write into until it exists.
    if let Some(parent) = primary.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let doc = build_document(path, &primary, args, limits)?;
    for format in formats {
        let target = base.with_extension(format.ext());
        match format {
            OutputFormat::Json => export_to_file(&doc, &target)?,
            OutputFormat::Md => pdfextractor_md_exporter::export_to_file_with_images(
                &doc,
                &target,
                args.include_notes,
                args.image_export_mode.mode(),
            )?,
            OutputFormat::Html => pdfextractor_md_exporter::html::export_to_file_with_images(
                &doc,
                &target,
                args.include_notes,
                args.image_export_mode.mode(),
            )?,
            OutputFormat::Text => pdfextractor_md_exporter::text::export_to_file_with_notes(
                &doc,
                &target,
                args.text_dehyphenate,
                args.include_notes,
            )?,
            OutputFormat::Yaml => pdfextractor_md_exporter::yaml::export_to_file(&doc, &target)?,
            OutputFormat::Chunks => {
                pdfextractor_md_exporter::chunks::export_to_file(&doc, &target, chunks)?
            }
        }
    }
    Ok(primary)
}

/// The reader's chunking choices, checked.
///
/// A budget of zero would make every hybrid chunk empty, and an overlap that is
/// the whole budget would repeat a chunk forever, so both are refused up front
/// with the numbers named rather than producing a file nobody can use.
fn chunk_options(args: &Args) -> anyhow::Result<pdfextractor_md_exporter::chunks::ChunkOptions> {
    if args.chunks_max_tokens == 0 {
        anyhow::bail!("--chunks-max-tokens must be at least 1");
    }
    if args.chunks_overlap_tokens >= args.chunks_max_tokens {
        anyhow::bail!(
            "--chunks-overlap-tokens ({}) must be smaller than --chunks-max-tokens ({})",
            args.chunks_overlap_tokens,
            args.chunks_max_tokens
        );
    }
    if args.chunks_tokenizer_encoding.is_some() && args.chunks_tokenizer.is_none() {
        anyhow::bail!("--chunks-tokenizer-encoding requires --chunks-tokenizer");
    }
    // A tokenizer is a token budget, and only hybrid packing spends one:
    // hierarchical chunks are made per item. Passing one anyway used to be
    // silent, which reads as a budget that was applied when it was not.
    if args.chunks_tokenizer.is_some() && args.chunks_type != ChunkType::Hybrid {
        tracing::warn!(
            "--chunks-tokenizer only counts tokens for --chunks-type hybrid; \
             hierarchical chunks ignore it"
        );
    }
    let tokenizer = match args.chunks_tokenizer.as_deref() {
        Some(path) => {
            let encoding = args
                .chunks_tokenizer_encoding
                .map(TokenizerEncoding::encoding);
            let tokenizer =
                pdfextractor_md_exporter::chunks::Tokenizer::from_file_with_tiktoken_encoding(
                    path, encoding,
                )
                .with_context(|| match encoding {
                    Some(encoding) => format!(
                        "--chunks-tokenizer {} --chunks-tokenizer-encoding {encoding}",
                        path.display()
                    ),
                    None => format!("--chunks-tokenizer {}", path.display()),
                })?;
            Some(Arc::new(tokenizer))
        }
        None => None,
    };
    Ok(pdfextractor_md_exporter::chunks::ChunkOptions {
        kind: args.chunks_type.kind(),
        max_tokens: args.chunks_max_tokens,
        overlap_tokens: args.chunks_overlap_tokens,
        tokenizer,
        include_notes: args.include_notes,
    })
}

/// The formats a run writes: what was asked for, in the order asked, with a
/// repeated format written once; nothing asked for means JSON, which is the
/// document the compare mode and the window read.
fn output_formats(requested: &[OutputFormat]) -> Vec<OutputFormat> {
    if requested.is_empty() {
        return vec![OutputFormat::Json];
    }
    let mut formats: Vec<OutputFormat> = Vec::new();
    for format in requested {
        if !formats.contains(format) {
            formats.push(*format);
        }
    }
    formats
}

/// Resolve every output path up front.
///
/// In a directory run two inputs with different extensions can share a stem
/// (`report.pdf` and `report.txt`): replacing both extensions would write the
/// same file twice and silently drop one document. When that happens the full
/// input name is kept instead, so `report.txt` becomes `report.txt.json`.
fn resolve_outputs(targets: &[PathBuf], args: &Args, root: &Path) -> Vec<PathBuf> {
    let paths: Vec<PathBuf> = targets
        .iter()
        .map(|path| output_path_for(path, args, root))
        .collect();
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

/// Where the primary output for `path` should be written.
///
/// The stem is the base every requested format is written beside; the primary
/// format's own extension replaces it in `convert_one`.
fn output_path_for(path: &Path, args: &Args, root: &Path) -> PathBuf {
    match &args.output {
        Some(o) if root.is_dir() => {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .with_extension("json");
            o.join(rel)
        }
        // A folder is a folder for one document too. Without this, `-i
        // report.pdf -o out/` treats `out` as a stem and writes `out.json`
        // beside the folder instead of `out/report.json` inside it.
        Some(o) if is_folder_destination(o) => {
            let name = path.file_name().unwrap_or(path.as_os_str());
            o.join(name).with_extension("json")
        }
        Some(o) => o.clone(),
        None => path.with_extension("json"),
    }
}

/// Whether an output path is a folder to write into: one that exists, or one
/// spelled with a trailing separator the way a shell's completion writes it.
/// `is_separator` is the platform's own rule, so a backslash is not a folder
/// name on Unix.
fn is_folder_destination(path: &Path) -> bool {
    let spelled_as_folder = path
        .as_os_str()
        .to_string_lossy()
        .ends_with(std::path::is_separator);
    path.is_dir() || spelled_as_folder
}
#[cfg(test)]
mod tests {
    use super::*;
    use pdfextractor_pipeline::{orient_for_ocr, rotate_for_ocr, rotate_for_ocr_checked};

    /// The accepted list is one fact, not two: the message a user reads names
    /// the picture formats the shared table carries in an encoding the image
    /// reader can turn into a page, and the run accepts exactly those names and
    /// no others. A carried format that is only ever a picture inside a
    /// document (an SVG, a metafile) is not offered as an input, and a readable
    /// one cannot be listed without being accepted.
    #[test]
    fn the_accepted_list_is_the_shared_table_filtered_by_the_reader() {
        for format in pdfextractor_picture_formats::PICTURE_FORMATS {
            let readable = pdfextractor_image_parser::is_document_media_type(format.mime);
            for extension in std::iter::once(format.extension).chain(format.read_as.iter().copied())
            {
                assert_eq!(
                    is_supported_extension(extension),
                    readable,
                    "{extension} ({}): {}",
                    format.mime,
                    if readable {
                        "the run refuses a format it lists"
                    } else {
                        "the run accepts a picture that is not a page"
                    }
                );
            }
        }

        // The list a user is shown and the predicate that decides are the same
        // set, so the error cannot offer an extension the run then refuses.
        for extension in supported_extensions() {
            assert!(
                is_supported_extension(extension),
                "the message names .{extension}, which the run refuses"
            );
        }

        // The grouped `--help` block names every accepted extension exactly
        // once, marks only the bridged names, and names nothing the run refuses.
        let help = accepted_formats_help();
        let named = help
            .lines()
            .skip(1)
            .filter(|line| !line.trim_start().starts_with('*'))
            .filter_map(|line| line.split_once(':').map(|(_, names)| names.trim()))
            .flat_map(|names| names.split(", "))
            .flat_map(|names| names.split('/'))
            .map(|name| name.trim_end_matches('*').to_string())
            .collect::<Vec<_>>();
        let mut sorted = named.clone();
        sorted.sort();
        let mut accepted: Vec<String> = supported_extensions()
            .iter()
            .map(|extension| format!(".{extension}"))
            .collect();
        accepted.sort();
        assert_eq!(
            sorted, accepted,
            "the grouped help and the gate do not name the same set: {help}"
        );
        for (family, extensions) in DOCUMENT_FAMILIES {
            for extension in *extensions {
                let marked = help.contains(&format!(".{extension}*"));
                assert_eq!(
                    marked,
                    is_bridged(extension),
                    "{family}: .{extension} is marked {} the bridge reads it",
                    if marked { "as if" } else { "though" }
                );
            }
        }

        // The formats that are only pictures inside a document are not offered.
        for extension in ["svg", "emf", "wmf"] {
            assert!(
                !supported_extensions().contains(&extension),
                ".{extension} is not an input document"
            );
            assert!(!is_supported_extension(extension), ".{extension}");
        }

        // A GIF is: its screen states the page. (Its frames are the pages.)
        assert!(
            supported_extensions().contains(&"gif"),
            ".gif is a page now and is not offered"
        );
        assert!(is_supported_extension("gif"));

        // A JPEG 2000 is a page in both its spellings: the JP2 container, and
        // the bare codestream whose `SIZ` marker states the grid. Each spelling
        // has several names, and which header a file has is its bytes` answer.
        for extension in ["jp2", "j2c", "j2k", "jpc", "jpf"] {
            assert!(
                supported_extensions().contains(&extension),
                ".{extension} is a page now and is not offered"
            );
            assert!(is_supported_extension(extension), ".{extension}");
        }

        // Every readable picture encoding on the shared table is listed, and
        // the image extensions come from there rather than from a second list.
        for format in pdfextractor_picture_formats::PICTURE_FORMATS {
            if pdfextractor_image_parser::is_document_media_type(format.mime) {
                assert!(
                    supported_extensions().contains(&format.extension),
                    "{} is readable but not listed",
                    format.mime
                );
            }
        }
    }

    /// The refusal of a carried picture is one fact, not two: it names the
    /// format's media type, says the file is not a page on its own, and offers
    /// exactly the picture formats the run accepts — taken from the shared
    /// table rather than written out again. A readable format is never
    /// explained that way (the run takes it), and a stranger is not explained
    /// at all.
    #[test]
    fn a_carried_picture_that_is_not_a_page_says_why() {
        for format in pdfextractor_picture_formats::PICTURE_FORMATS {
            let message = carried_picture_not_a_page(format.extension);
            if pdfextractor_image_parser::is_document_media_type(format.mime) {
                assert_eq!(message, None, "{} is a page", format.mime);
                assert!(is_supported_extension(format.extension), "{}", format.mime);
                continue;
            }
            let message = message.unwrap_or_else(|| panic!("{} was not explained", format.mime));
            assert!(
                message.contains(&format!("unsupported extension: {}", format.extension)),
                "{message}"
            );
            assert!(message.contains(format.mime), "{message}");
            assert!(
                message.contains(&format!(
                    ".{} is not read as a page on its own",
                    format.extension
                )),
                "{message}"
            );
            assert!(
                !is_supported_extension(format.extension),
                "{} is both refused and offered",
                format.extension
            );
            // Every way out the message names is a picture the run reads as a
            // page, and every such picture is named.
            for page in pdfextractor_picture_formats::PICTURE_FORMATS {
                if !pdfextractor_image_parser::is_document_media_type(page.mime) {
                    continue;
                }
                let offered = message.contains(&format!(".{}", page.extension));
                assert!(
                    offered,
                    "{} is a page the refusal does not offer: {message}",
                    page.mime
                );
                assert!(is_supported_extension(page.extension));
            }
        }

        // A name the shared table does not carry is no carried picture.
        assert_eq!(carried_picture_not_a_page("xyz"), None);
        assert_eq!(carried_picture_not_a_page(""), None);
    }

    /// The empty-folder refusal names what the walk passed over, and nothing at
    /// all about pictures when there were none: an empty folder is empty.
    #[test]
    fn the_refusal_of_a_folder_names_the_pictures_it_passed_over() {
        let root = Path::new("scans");

        let empty = nothing_to_read(root, &[]);
        assert!(empty.contains("no supported documents"), "{empty}");
        assert!(empty.contains("scans"), "{empty}");
        assert!(!empty.contains("it does hold"), "{empty}");

        let one = nothing_to_read(root, &[PathBuf::from("scans/chart.svg")]);
        assert!(
            one.contains("it does hold one picture this tool carries but does not read as a page")
                && one.contains("chart.svg"),
            "{one}"
        );

        // Enough to show what is there: six names, then the count of the rest.
        let many: Vec<PathBuf> = (0..8)
            .map(|index| PathBuf::from(format!("scans/picture{index}.svg")))
            .collect();
        let all = nothing_to_read(root, &many);
        assert!(all.contains("it does hold 8 pictures"), "{all}");
        assert!(all.contains("picture5.svg"), "{all}");
        assert!(!all.contains("picture6.svg"), "{all}");
        assert!(all.contains("and 2 more"), "{all}");
    }

    /// One value of every `LimitError` variant.
    ///
    /// [`limit_error_variant`] matches the same enum with no wildcard arm, so a
    /// new variant fails to compile until it is named here and given an index;
    /// the test below then holds the count to `variant_count`, which is what
    /// catches a sample that was never added.
    fn limit_error_samples() -> Vec<pdfextractor_limits::LimitError> {
        use pdfextractor_limits::LimitError::*;
        let timeout = std::time::Duration::from_secs(1);
        vec![
            InputTooLarge {
                actual: 2,
                limit: 1,
            },
            TooManyPages {
                actual: 2,
                limit: 1,
            },
            TooManyPdfObjects {
                actual: 2,
                limit: 1,
            },
            TooManyPdfTokens {
                actual: 2,
                limit: 1,
            },
            PdfTooDeep {
                actual: 2,
                limit: 1,
            },
            DecodedStreamTooLarge {
                actual: 2,
                limit: 1,
            },
            TooManyZipEntries {
                actual: 2,
                limit: 1,
            },
            ZipEntryTooLarge {
                name: "word/document.xml".to_string(),
                actual: 2,
                limit: 1,
            },
            ZipRatioTooHigh {
                name: "word/document.xml".to_string(),
                ratio: 2,
                limit: 1,
            },
            ZipTotalTooLarge {
                actual: 2,
                limit: 1,
            },
            Zip {
                detail: "invalid Zip archive".to_string(),
            },
            XmlTooLarge {
                actual: 2,
                limit: 1,
            },
            XmlTooDeep {
                actual: 2,
                limit: 1,
            },
            XmlDoctype,
            XmlMalformed {
                detail: "mismatched close tag".to_string(),
            },
            ImageDecode {
                detail: "truncated PNG".to_string(),
            },
            ImageEmpty,
            ImageTooWide {
                width: 2,
                height: 2,
                limit: 1,
            },
            ImageTooManyPixels {
                width: 2,
                height: 2,
                pixels: 4,
                limit: 1,
            },
            ImageAllocTooLarge {
                actual: 2,
                limit: 1,
            },
            TooManyImageFrames {
                actual: 2,
                limit: 1,
            },
            TooManyFlowBlocks {
                actual: 2,
                limit: 1,
            },
            DocumentTooLarge {
                actual: 2,
                limit: 1,
            },
            BridgeOutputTooLarge {
                actual: 2,
                limit: 1,
            },
            Timeout {
                stage: "reading the PDF header".to_string(),
                elapsed: timeout,
                limit: timeout,
            },
        ]
    }

    /// Which variant `error` is, numbered in the order of the enum.
    fn limit_error_variant(error: &pdfextractor_limits::LimitError) -> usize {
        use pdfextractor_limits::LimitError::*;
        match error {
            InputTooLarge { .. } => 0,
            TooManyPages { .. } => 1,
            TooManyPdfObjects { .. } => 2,
            TooManyPdfTokens { .. } => 3,
            PdfTooDeep { .. } => 4,
            DecodedStreamTooLarge { .. } => 5,
            TooManyZipEntries { .. } => 6,
            ZipEntryTooLarge { .. } => 7,
            ZipRatioTooHigh { .. } => 8,
            ZipTotalTooLarge { .. } => 9,
            Zip { .. } => 10,
            XmlTooLarge { .. } => 11,
            XmlTooDeep { .. } => 12,
            XmlDoctype => 13,
            XmlMalformed { .. } => 14,
            ImageDecode { .. } => 15,
            ImageEmpty => 16,
            ImageTooWide { .. } => 17,
            ImageTooManyPixels { .. } => 18,
            ImageAllocTooLarge { .. } => 19,
            TooManyImageFrames { .. } => 20,
            TooManyFlowBlocks { .. } => 21,
            DocumentTooLarge { .. } => 22,
            BridgeOutputTooLarge { .. } => 23,
            Timeout { .. } => 24,
        }
    }

    /// Every `LimitError` ends a run the same way: the refusal exit code and the
    /// `limit` error kind, so a consumer can act on a refusal without parsing the
    /// sentence it carries.
    #[test]
    fn every_limit_error_variant_maps_to_the_refusal_exit_code_and_kind() {
        // The sample at position `i` must be variant `i`: the indices are dense
        // and in enum order, so a sample inserted out of order, or one of two
        // samples of the same variant, fails here. A variant with no sample is a
        // compile error first — `limit_error_variant` has no wildcard arm.
        // The flag each sample's refusal names, in variant order: every budget
        // the command line exposes has one, and the malformed-input variants
        // have none.
        let flags: [Option<&str>; 25] = [
            Some("--max-input-bytes"),
            Some("--max-pages"),
            Some("--max-pdf-objects"),
            Some("--max-pdf-tokens"),
            Some("--max-pdf-nesting"),
            Some("--max-decoded-stream-bytes"),
            Some("--max-zip-entries"),
            Some("--max-zip-entry-bytes"),
            Some("--max-zip-ratio"),
            Some("--max-zip-total-bytes"),
            None, // a corrupt package
            Some("--max-xml-bytes"),
            Some("--max-xml-depth"),
            None, // a DOCTYPE is refused outright
            None, // malformed XML
            None, // a picture that will not decode
            None, // an empty picture
            Some("--max-image-dimension"),
            Some("--max-image-pixels"),
            Some("--max-image-alloc-bytes"),
            Some("--max-image-frames"),
            Some("--max-flow-blocks"),
            Some("--max-document-bytes"),
            Some("--max-bridge-output-bytes"),
            Some("--timeout"),
        ];

        for (position, sample) in limit_error_samples().into_iter().enumerate() {
            assert_eq!(
                limit_error_variant(&sample),
                position,
                "sample {position} is not its variant: {sample:?}"
            );
            let error = anyhow::Error::new(sample);
            assert!(is_limit_refusal(&error), "{error:#} is not a refusal");
            assert_eq!(error_kind(&error), "limit");
            assert_eq!(
                raise_flag(&error),
                flags[position],
                "the refusal at {position} names the wrong flag"
            );
            // The mapping is what the run prints, not just what it returns: a
            // refusal's suffix has to name the same flag.
            if let Some(flag) = flags[position] {
                let hint = flag_hint(&error);
                assert!(
                    hint.contains(flag),
                    "the refusal at {position} does not offer {flag}: {hint}"
                );
            }
        }

        // The OCR deadline is a different budget from the parser's, and its
        // stage is what tells the two apart.
        let ocr = anyhow::Error::new(pdfextractor_limits::LimitError::Timeout {
            stage: "recognising a page".to_string(),
            elapsed: std::time::Duration::from_secs(1),
            limit: std::time::Duration::from_secs(1),
        });
        assert_eq!(raise_flag(&ocr), Some("--ocr-timeout"));
        assert!(flag_hint(&ocr).contains("--ocr-timeout"));

        // The bridge's deadline is a third budget behind the same variant.
        let bridge = anyhow::Error::new(pdfextractor_limits::LimitError::Timeout {
            stage: "the LibreOffice bridge".to_string(),
            elapsed: std::time::Duration::from_secs(1),
            limit: std::time::Duration::from_secs(1),
        });
        assert_eq!(raise_flag(&bridge), Some("--bridge-timeout"));
        assert!(flag_hint(&bridge).contains("--bridge-timeout"));

        // A refusal that reached the run as the pipeline's typed case is the
        // same refusal: the status and the kind are read from that case, not
        // from the sentence a reader happened to wrap around it.
        let typed = anyhow::Error::new(pdfextractor_pipeline::PipelineError::Limit(
            pdfextractor_limits::LimitError::TooManyPages {
                actual: 2,
                limit: 1,
            },
        ));
        assert!(is_limit_refusal(&typed), "the typed case is not a refusal");
        assert_eq!(error_kind(&typed), "limit");
        assert_eq!(raise_flag(&typed), Some("--max-pages"));

        assert_eq!(EXIT_LIMIT_REFUSED, 3, "the refusal exit code moved");
        // The other kind: a file that cannot be read is not a refusal, so it
        // exits through the ordinary error path.
        let ordinary = anyhow::anyhow!("not a DOCX (ZIP) package");
        assert_eq!(error_kind(&ordinary), "error");
        assert!(!is_limit_refusal(&ordinary));
    }

    /// The OCR page turn is a decode like any other, so the run's image budgets
    /// govern it. A ceiling of one pixel refuses a 2x2 page, and the same bytes
    /// turn at the defaults: the refusal is the budget's, not the picture's, and
    /// it names the flag that would allow the turn.
    #[test]
    fn a_one_pixel_ceiling_refuses_the_page_turn() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode the test picture");

        let limits = pdfextractor_limits::Limits {
            max_image_pixels: 1,
            ..pdfextractor_limits::Limits::default()
        };
        let refusal = rotate_for_ocr_checked(&png, 90, &limits)
            .expect_err("a one-pixel ceiling turned a 2x2 page");
        assert_eq!(
            raise_flag(&refusal.into()),
            Some("--max-image-pixels"),
            "the refusal does not name the budget that refused the turn"
        );

        let (turned, width, height) =
            rotate_for_ocr(&png, 90, &pdfextractor_limits::Limits::default())
                .expect("the defaults decode a 2x2 page");
        assert_eq!((width, height), (2, 2));
        assert!(!turned.is_empty(), "the turn wrote no PNG");
    }

    #[test]
    fn a_one_pixel_ceiling_refuses_the_exif_turn() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode the test picture");

        let limits = pdfextractor_limits::Limits {
            max_image_pixels: 1,
            ..pdfextractor_limits::Limits::default()
        };
        let refusal = orient_for_ocr(
            &png,
            pdfextractor_image_parser::Orientation::Rotate90,
            &limits,
        )
        .expect_err("a one-pixel ceiling turned a 2x2 page by its EXIF tag");
        assert_eq!(
            raise_flag(&refusal.into()),
            Some("--max-image-pixels"),
            "the refusal does not name the budget that refused the turn"
        );

        let turned = orient_for_ocr(
            &png,
            pdfextractor_image_parser::Orientation::Rotate90,
            &pdfextractor_limits::Limits::default(),
        )
        .expect("the defaults turn a 2x2 page");
        assert!(!turned.is_empty(), "the EXIF turn wrote no PNG");
    }
}
