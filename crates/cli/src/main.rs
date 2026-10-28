mod compare;
mod preview;

// `relabel` reads the original file back to hash it, and says which file it
// could not read when it cannot.
use anyhow::Context as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use clap::{Parser, ValueEnum, ValueHint};
// The format question — what a file actually is, and the one family with no
// reader here — lives in a crate of its own, because the corpus report asks it
// of the same documents and the two must not answer it differently.
use pdfextractor_core::{
    DocumentBuilder, GridCell, GridTable, Origin, PictureData, Ref, TextLabel,
};
use pdfextractor_formats::{bridge, detect, is_bridged, BridgeTarget, Format};
use pdfextractor_json_exporter::export_to_file;
use pdfextractor_layout::Span as LayoutSpan;
use pdfextractor_ocr_engine::QuarterTurn;
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

/// Language for OCR when none is asked for.
const DEFAULT_OCR_LANG: &str = "eng";

/// The language list Tesseract wants: `eng,deu` and `eng+deu` say the same
/// thing, and Tesseract spells it with `+`.
fn tesseract_langs(raw: &str) -> anyhow::Result<String> {
    let langs: Vec<&str> = raw
        .split([',', '+'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if langs.is_empty() {
        anyhow::bail!("--ocr-lang needs at least one language code, e.g. `{DEFAULT_OCR_LANG}`");
    }
    Ok(langs.join("+"))
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
    /// The name `-` writes the document to stdout — one input, one format.
    #[arg(short, long, value_hint = ValueHint::AnyPath)]
    output: Option<PathBuf>,

    /// Output format, repeatable (`--to json --to md --to html --to text
    /// --to yaml`); absent means `json`,
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

    /// OCR languages: comma-separated Tesseract codes (`eng,deu`), default `eng`.
    /// The recognised text only comes back if the language's data is installed
    /// (and, in a released package, shipped beside the binary)
    #[arg(long, default_value = "eng", value_name = "LANGS")]
    ocr_lang: String,

    /// Tesseract page segmentation mode, 0–13 (the engine's own default when
    /// absent; 6 is one block, 7 one line, 11 sparse text)
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=13), value_name = "N")]
    psm: Option<u8>,

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

    // A consumer asking what this build can take should not have to hold an
    // input path, run a conversion, or parse help text or a refusal message:
    // the set is printed as JSON and the run ends here.
    if args.list_input_formats {
        println!("{}", serde_json::to_string_pretty(&input_formats())?);
        flush_stdout();
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

    let found = collect_inputs(input, args.recursive)?;
    let targets = found.documents;
    if targets.is_empty() {
        anyhow::bail!(nothing_to_read(input, &found.carried));
    }

    let formats = output_formats(&args.to);

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
        let doc = build_document(target, &base, &args)?;
        match formats[0] {
            OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&doc)?),
            OutputFormat::Md => print!(
                "{}",
                pdfextractor_md_exporter::render_with_images(
                    &doc,
                    args.include_notes,
                    args.image_export_mode.mode(),
                    base.parent(),
                )
            ),
            OutputFormat::Html => {
                print!(
                    "{}",
                    pdfextractor_md_exporter::html::render_with_images(
                        &doc,
                        args.include_notes,
                        args.image_export_mode.mode(),
                        base.parent(),
                    )
                )
            }
            OutputFormat::Text => print!(
                "{}",
                pdfextractor_md_exporter::text::render_with_notes(
                    &doc,
                    args.text_dehyphenate,
                    args.include_notes,
                )
            ),
            OutputFormat::Yaml => {
                print!("{}", pdfextractor_md_exporter::yaml::render(&doc)?)
            }
        }
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
                let result = convert_one(path, output, &args, &formats);
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
    walk_dir(input, max_depth, &mut found)?;
    found.documents.sort();
    found.carried.sort();
    Ok(found)
}

fn walk_dir(dir: &Path, depth_left: Option<usize>, found: &mut Found) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            match depth_left {
                None => walk_dir(&path, None, found)?,
                Some(n) if n > 0 => walk_dir(&path, Some(n - 1), found)?,
                Some(_) => {}
            }
        } else if is_supported(&path) {
            found.documents.push(path);
        } else if is_carried_picture(&path) {
            found.carried.push(path);
        }
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
    "pdf", "docx", "xlsx", "pptx", "ppt", "doc", "wps", "wp", "wks", "wdb", "wpd", "xls", "rtf",
    "html", "htm", "txt", "md", "markdown", "csv", "tsv",
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

fn convert_one(
    path: &Path,
    base: &Path,
    args: &Args,
    formats: &[OutputFormat],
) -> anyhow::Result<PathBuf> {
    // Every format is written beside the same stem: `-o out.json --to md`
    // writes `out.json` and `out.md`, and the first format asked for is the
    // run's primary output — the one reported to `--progress` and named in
    // the log line.
    let primary = base.with_extension(formats[0].ext());
    let doc = build_document(path, &primary, args)?;
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
        }
    }
    Ok(primary)
}

/// Convert one input into a document, without writing it out.
///
/// `output_path` is the primary output's path: extracted images and the
/// preview are named beside it, whatever format the document itself ends up
/// being written as.
fn build_document(
    path: &Path,
    output_path: &Path,
    args: &Args,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    // The extension has to be one this tool promises to read; which reader it
    // gets is the bytes' business, not the name's.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !is_supported_extension(&ext) {
        // A picture the shared table carries but this reader cannot page is not
        // an unknown format, and the message says so rather than leaving the
        // user to guess why a format the tool writes out is refused as input.
        match carried_picture_not_a_page(&ext) {
            Some(reason) => anyhow::bail!(reason),
            // The name is not accepted, but the bytes may name a format this
            // tool reads under another one: say so rather than sending the user
            // away with a fact they cannot act on.
            None => anyhow::bail!(bytes_say_what_it_is(path, &ext)
                .unwrap_or_else(|| format!("unsupported extension: {ext}"))),
        }
    }

    // Extracted images and previews are written beside the output.
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let doc = match detect(path)? {
        Format::Pdf => {
            let langs = tesseract_langs(&args.ocr_lang)?;
            convert_pdf(
                path,
                args.ocr,
                &langs,
                args.psm,
                output_path,
                args.preview,
                args.preview_dpi,
            )?
        }
        Format::Docx => {
            note_flow_format(args.preview, path);
            convert_docx(path, output_path)?
        }
        Format::Xlsx => {
            if args.preview {
                tracing::info!(
                    "{}: the preview draws page images; a workbook has none, skipping",
                    path.display()
                );
            }
            convert_xlsx(path, output_path)?
        }
        Format::Pptx => {
            if args.preview {
                tracing::info!(
                    "{}: the preview draws PDF page images; a presentation has none, skipping",
                    path.display()
                );
            }
            convert_pptx(path, output_path)?
        }
        Format::Image { .. } => {
            if args.preview {
                tracing::info!(
                    "{}: the preview draws PDF page images; an image is its own page, skipping",
                    path.display()
                );
            }
            convert_image(path, args)?
        }
        // `.doc` is the legacy OLE compound format, not an OPC package: the
        // bytes are a binary Word stream, not ZIP + XML. The reader here reads
        // the container and the piece table, and nothing of the paragraph or
        // character properties — so a heading is a paragraph, a list item keeps
        // no marker, and a table is tabs. LibreOffice's Writer reads the same
        // file and writes the package the DOCX reader reads whole, so where it
        // is installed it is the better reader; where it is not, the built-in
        // one still opens the file. Either way the document keeps the identity
        // of the file the user handed over.
        Format::Doc => {
            note_flow_format(args.preview, path);
            match bridge(path, BridgeTarget::Docx, "a legacy Word document") {
                Ok(bridged) => {
                    tracing::info!(
                        "{}: a legacy Word document, read through LibreOffice as docx",
                        path.display()
                    );
                    let doc = convert_docx(bridged.path(), output_path)?;
                    relabel(doc, path, "application/msword")?
                }
                Err(error) => {
                    // Not a failure: the file has a reader of its own. Why the
                    // bridge could not be used is said once, at debug, so a run
                    // on a machine without LibreOffice does not look wrong.
                    tracing::debug!("{}: {error:#}", path.display());
                    tracing::info!(
                        "{}: reading the legacy Word document with the built-in reader",
                        path.display()
                    );
                    convert_doc(path, output_path)?
                }
            }
        }
        Format::Flow(flow) => {
            note_flow_format(args.preview, path);
            convert_text(flow, path, output_path)?
        }
        // No reader here: LibreOffice reads it, and one of our own readers
        // reads what it wrote. The document is then re-labelled with the
        // original file's identity, because that is the document the user
        // handed over — the conversion is how its words were reached, not what
        // they are.
        Format::Bridge {
            target,
            mimetype,
            label,
        } => {
            // A deck keeps its slides, but the preview draws PDF page images
            // and a presentation has none; the other bridged families are
            // flow-based and have no page box to draw on at all.
            match target {
                BridgeTarget::Pptx => {
                    if args.preview {
                        tracing::info!(
                            "{}: the preview draws PDF page images; a presentation has none, skipping",
                            path.display()
                        );
                    }
                }
                _ => note_flow_format(args.preview, path),
            }
            let bridged = bridge(path, target, label)?;
            tracing::info!(
                "{}: {label}, read through LibreOffice as {}",
                path.display(),
                target.extension()
            );
            let doc = match target {
                BridgeTarget::Docx => convert_docx(bridged.path(), output_path)?,
                BridgeTarget::Xlsx => convert_xlsx(bridged.path(), output_path)?,
                BridgeTarget::Csv => convert_text("csv", bridged.path(), output_path)?,
                BridgeTarget::Pptx => convert_pptx(bridged.path(), output_path)?,
            };
            relabel(doc, path, mimetype)?
        }
        Format::Unsupported(what) => anyhow::bail!("unsupported format: {what}"),
    };

    Ok(doc)
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
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = std::fs::read(original).with_context(|| format!("read {}", original.display()))?;
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
/// `LepTess` is not `Sync` and construction is expensive, so a document makes
/// one engine and reuses it for every page — or, for a plain image, for its
/// only page. A mode Tesseract will not take leaves the engine on its own
/// default; it is not a reason to lose the text.
fn start_ocr_engine(
    ocr: OcrMode,
    lang: &str,
    psm: Option<u8>,
) -> Option<pdfextractor_ocr_engine::OcrEngine> {
    if ocr == OcrMode::Never {
        return None;
    }
    match pdfextractor_ocr_engine::OcrEngine::new(lang) {
        Ok(mut engine) => {
            if let Some(psm) = psm {
                if let Err(err) = engine.set_psm(psm) {
                    tracing::warn!("--psm {psm} refused ({err}); keeping the engine's default");
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

/// Build a `DoclingDocument` from a PDF file path.
fn convert_pdf(
    path: &Path,
    ocr: OcrMode,
    ocr_lang: &str,
    psm: Option<u8>,
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
    let mut engine = start_ocr_engine(ocr, ocr_lang, psm);

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

            // A scan can arrive on its side with nothing saying so: no
            // `/Rotate` on the page, no EXIF tag in the picture, just pixels a
            // scanner laid down the wrong way. The page's whole text is in
            // that one picture, so Tesseract's own reading of the page's turns
            // can decide which way up it is written.
            let detect_sideways = page.rotation == 0 && page.spans.is_empty();

            for (data, width, height, box_) in images {
                let turn = if detect_sideways && picture_is_page(box_, page.width, page.height) {
                    match engine.detect_upright(data) {
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
                    rotate_for_ocr(data, page.rotation)
                } else if turn != QuarterTurn::AsStored {
                    rotate_for_ocr(data, turn.degrees())
                } else {
                    None
                };
                let (bytes, pixels_w, pixels_h) = match &rotated {
                    Some((png, w, h)) => (png.as_slice(), *w, *h),
                    None => (data.as_slice(), width, height),
                };
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

/// Write a picture to the relative path its page named, beside `output_path`.
///
/// The path is the page's own, so the page written from the document names the
/// same file again; the answer is the reference in the form both writers use
/// (forward slashes, relative to the document). A name with a space or a
/// character needing an escape travels through this unchanged: the writers
/// escape it where their own syntax asks them to.
fn write_kept_image(output_path: &Path, relative: &Path, data: &[u8]) -> Option<String> {
    let directory = output_path.parent()?;
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
        if let Some(reference) = emit_furniture_item(
            &mut builder,
            item,
            output_path,
            &mut image_dir,
            &mut image_seq,
        ) {
            furniture.push(reference);
        }
    }
    builder.set_furniture(furniture);

    Ok(builder.build())
}

/// Build a `DoclingDocument` from an XLSX workbook.
///
/// Each worksheet is a page whose units are cells rather than points — the
/// shape Docling's own XLSX backend produces: a `sheet` group per worksheet
/// with the worksheet's tables beneath it, and a provenance box in cell
/// indices (0-based, top-left origin). A hidden worksheet stays in the
/// document as the `invisible` layer rather than being dropped.
fn convert_xlsx(
    path: &Path,
    output_path: &Path,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = std::fs::read(path)?;
    let parsed = pdfextractor_xlsx_parser::XlsxDocument::parse_bytes(&data)?;

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
        mimetype: Some(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string(),
        ),
        // Docling requires a non-negative integer, so the hash stays unsigned.
        binary_hash: Some(fnv1a(&data)),
        filename: Some(name.to_string()),
        uri: None,
    });

    // A workbook's pictures are written beside the JSON like a Word package's,
    // under names that say which sheet they are on.
    let mut image_dir: Option<PathBuf> = None;
    let mut image_seq = 0usize;

    for (index, sheet) in parsed.sheets.iter().enumerate() {
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
                    let (mime, data) = office_media(picture.mime, picture.data.clone(), None, None);
                    let image = image_dimensions(&data).and_then(|(width, height)| {
                        let uri = write_image(
                            &mut image_dir,
                            output_path,
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
fn convert_pptx(
    path: &Path,
    output_path: &Path,
) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = std::fs::read(path)?;
    let parsed = pdfextractor_pptx_parser::PptxDocument::parse_bytes(&data)?;

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
    for slide in &parsed.slides {
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
            );
            let image = image_dimensions(&data).and_then(|(width, height)| {
                let uri = write_image(
                    &mut image_dir,
                    output_path,
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
                    );
                    let image = image_dimensions(&data).and_then(|(width, height)| {
                        let uri = write_image(
                            &mut image_dir,
                            output_path,
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
    let mut parts = Vec::new();
    // A name the file left blank is no name: the author's whole field is left
    // out, initials and all, which is where Docling draws the same line.
    if let Some(author) = comment.author.as_deref().filter(|name| !name.is_empty()) {
        let mut named = format!("author: {author}");
        if let Some(initials) = comment
            .initials
            .as_deref()
            .filter(|initials| !initials.is_empty())
        {
            named.push_str(&format!(" ({initials})"));
        }
        parts.push(named);
    }
    if let Some(date) = comment.date.as_deref().filter(|date| !date.is_empty()) {
        parts.push(format!("time: {}", comment_timestamp(date)));
    }
    if parts.is_empty() {
        comment.text.clone()
    } else {
        format!("[{}]: {}", parts.join(", "), comment.text)
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
fn convert_image(path: &Path, args: &Args) -> anyhow::Result<pdfextractor_core::DoclingDocument> {
    let data = std::fs::read(path)?;
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
    let gif_frames = pdfextractor_image_parser::gif_frames(&data);
    let webp_frames = pdfextractor_image_parser::webp_frames(&data);
    let tiff_pages = pdfextractor_image_parser::tiff_frames(&data);
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

    let langs = tesseract_langs(&args.ocr_lang)?;
    let Some(mut engine) = start_ocr_engine(args.ocr, &langs, args.psm) else {
        // Without recognition the pages are still the file's: one per frame or
        // directory, each at its own size.
        builder.add_page(1, image.width_pt(), image.height_pt());
        for (index, page) in later.iter().enumerate() {
            // A TIFF sheet comes back already turned by its own tag; a GIF's
            // or a WebP's animation frames share the screen, and the file
            // states no tag for them.
            let swaps = if tiff_stack {
                false
            } else {
                image.orientation.swaps_axes()
            };
            let (width, height) = page_size(&image, (page.width, page.height), swaps);
            builder.add_page((index + 2) as i32, width, height);
        }
        return Ok(builder.build());
    };

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
        let decoded =
            pdfextractor_image_parser::transcode_to_png(&data).map(|picture| picture.data);
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
        match engine.detect_upright(source) {
            Ok(turn) => turn,
            Err(err) => {
                tracing::debug!(
                    "could not find the orientation of {}: {err}",
                    path.display()
                );
                QuarterTurn::AsStored
            }
        }
    } else {
        QuarterTurn::AsStored
    };

    // The page is the upright grid; Tesseract reads the stored one. A camera
    // that was held sideways is turned upright here, once, before recognition —
    // by EXIF when there is a tag, by the reading when there is not.
    let mut upright = orient_for_ocr(source, declared);
    if declared != pdfextractor_image_parser::Orientation::Normal && upright.is_none() {
        tracing::warn!(
            "could not apply EXIF orientation {:?} to {}; OCR sees the stored pixels",
            declared,
            path.display()
        );
    }
    let mut applied = QuarterTurn::AsStored;
    if declared == pdfextractor_image_parser::Orientation::Normal
        && detected != QuarterTurn::AsStored
    {
        match rotate_for_ocr(source, detected.degrees()) {
            Some((png, _, _)) => {
                upright = Some(png);
                applied = detected;
            }
            None => tracing::warn!(
                "could not turn {} upright; OCR sees the stored pixels",
                path.display()
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
    let mut page_pixels: Vec<(Vec<u8>, u32, u32, f64, f64)> = Vec::with_capacity(later.len() + 1);
    page_pixels.push((
        upright.as_deref().unwrap_or(source).to_vec(),
        first_grid.0,
        first_grid.1,
        width,
        height,
    ));
    for frame in later {
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
                (orient_for_ocr(&frame.data, declared), declared.swaps_axes())
            } else if applied == QuarterTurn::AsStored {
                (None, false)
            } else {
                (
                    rotate_for_ocr(&frame.data, applied.degrees()).map(|(png, _, _)| png),
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
        page_pixels.push((bytes, grid.0, grid.1, width, height));
    }
    for (index, page) in page_pixels.iter().enumerate() {
        builder.add_page((index + 1) as i32, page.3, page.4);
    }

    // The engine measures in pixels; the page is measured in points. A line's
    // box is scaled onto the page, and its height comes back as a font size —
    // the same estimate the PDF path uses for an OCR line. Each page is scaled
    // against its own grid: a GIF's frames share its logical screen, a TIFF's
    // sheets are each their own size.
    for (index, (pixels, pixel_width, pixel_height, width, height)) in
        page_pixels.iter().enumerate()
    {
        let (pixel_width, pixel_height) = (*pixel_width, *pixel_height);
        let (width, height) = (*width, *height);
        let page = (index + 1) as i32;
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
    output_path: &Path,
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
                let picture = load_flow_picture(path, source).and_then(|picture| {
                    // A picture the page named beside itself keeps that name, so
                    // the page written from this document names the same file
                    // and the page is its own bytes when it is read back. One
                    // the page carried (a `data:` URI) or named in a way this
                    // cannot spell is written under a name of this run's, the
                    // way every other format's pictures are.
                    let uri = match kept_picture_path(source, picture.mimetype) {
                        Some(relative) => write_kept_image(output_path, &relative, &picture.data),
                        None => {
                            image_seq += 1;
                            write_image(
                                &mut image_dir,
                                output_path,
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
fn load_flow_picture(document: &Path, source: &str) -> Option<FlowPicture> {
    let (declared, data) = match data_uri_picture(source) {
        Some(carried) => carried,
        None => {
            let file = local_image_path(document, source)?;
            let data = std::fs::read(&file).ok()?;
            (image_mimetype_for_path(&file), data)
        }
    };
    picture_from_bytes(declared, data)
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
fn picture_from_bytes(declared: Option<&'static str>, data: Vec<u8>) -> Option<FlowPicture> {
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
    let (mimetype, data) = docling_media(picture.mimetype, picture.data);
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

/// The file a picture's source names, if it names one at all.
///
/// A `data:` URI never reaches this: its bytes are the source, and
/// [`data_uri_picture`] has already answered. What is left here is a path.
fn local_image_path(document: &Path, source: &str) -> Option<PathBuf> {
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
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        document
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    })
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
                output_path,
                image_dir,
                image_seq,
                "pg1_",
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
            let (mime, data) = office_media(mime, data.clone(), *width_pt, *height_pt);
            let image = write_image(image_dir, output_path, &name, mime, &data)
                .map(|uri| describe_image(mime, &data, *width_pt, *height_pt, uri));
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
    image_seq: &mut usize,
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
                output_path,
                image_dir,
                image_seq,
                "furniture_",
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
            let (mime, data) = office_media(mime, data.clone(), *width_pt, *height_pt);
            let image = write_image(image_dir, output_path, &name, mime, &data)
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
    output_path: &Path,
    image_dir: &mut Option<PathBuf>,
    image_seq: &mut usize,
    prefix: &str,
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
                    );
                    write_image(image_dir, output_path, &name, mime, &data).map(|uri| {
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
                        output_path,
                        image_dir,
                        image_seq,
                        prefix,
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
fn docling_media(mime: &'static str, data: Vec<u8>) -> (&'static str, Vec<u8>) {
    if pdfextractor_core::is_representable_media_type(mime) {
        return (mime, data);
    }
    match pdfextractor_image_parser::transcode_to_png(&data) {
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
fn office_media(
    mime: &'static str,
    data: Vec<u8>,
    width_pt: Option<f64>,
    height_pt: Option<f64>,
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
    match pdfextractor_image_parser::transcode_to_png(&data) {
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
/// `None` for an image that is already upright, or one that cannot be
/// decoded — in which case the original bytes are what Tesseract gets.
fn orient_for_ocr(
    data: &[u8],
    orientation: pdfextractor_image_parser::Orientation,
) -> Option<Vec<u8>> {
    pdfextractor_image_parser::upright_png(data, orientation)
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
/// target box is measured in. `None` when the image cannot be decoded at all,
/// in which case Tesseract's own decoder gets the original bytes and its
/// error policy applies.
fn rotate_for_ocr(data: &[u8], degrees: i32) -> Option<(Vec<u8>, u32, u32)> {
    let bitmap = image::load_from_memory(data).ok()?;
    let turned = match degrees {
        90 => bitmap.rotate90(),
        180 => bitmap.rotate180(),
        270 => bitmap.rotate270(),
        _ => return None,
    };
    let (width, height) = (turned.width(), turned.height());
    let mut png = Vec::new();
    turned
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some((png, width, height))
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

    /// An office SVG is played at the frame the package gives it, and a frame of
    /// different proportions is not permission to squash the drawing: 36 × 36
    /// points is 72 × 72 pixels at 144 dpi, and a 40 × 24 drawing inside it is
    /// scaled by one factor (1.8, the smaller of the two the frame allows) and
    /// centred — 72 × 43 pixels of drawing with clear margin above and below.
    /// The picture's box is still the frame's, so its placement is unchanged.
    #[test]
    fn an_office_svg_is_letterboxed_into_a_frame_of_another_shape() {
        let drawing = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24" viewBox="0 0 40 24"><rect width="40" height="24" fill="#e8eef7"/></svg>"##;
        let (mime, png) = office_media("image/svg+xml", drawing.to_vec(), Some(36.0), Some(36.0));
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
