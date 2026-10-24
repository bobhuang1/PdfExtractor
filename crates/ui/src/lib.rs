//! The UI's logic, kept separate from the window so it can be tested without one.
//!
//! The window is a thin Tauri shell (see `main.rs`); everything that decides
//! *what* to run and *how to read what it prints* lives here: the settings form,
//! the argument vector it produces for the CLI, the child process, and the
//! parser that turns the CLI's `--progress` stream into events for the page.
//!
//! The formats the window offers are not written down here either: [`ask_input_formats`]
//! runs the CLI's `--list-input-formats` and reads the answer, so the window names
//! what a run will take rather than a copy of it that can drift. [`AcceptedFormats`]
//! is that answer kept between asks — and re-asked on demand, so a binary rebuilt
//! while the window is open is the one the window names.
//!
//! The window also has a **Compare** mode, which shows each finished document
//! beside a Docling reference of the same file: [`diff`] does the comparing, and
//! [`DiffLine`] is what the page receives. [`gate`] is how a finished document is
//! held to the memory beside its reference — the one Session the panel, the
//! corpus report and the command line's `--compare` mode all run through.
//! A conversion is therefore never waited on: the CLI is started, its two
//! output streams are read as they fill, and the process can be stopped at any
//! moment — so the window stays responsive throughout.

pub mod diff;
pub mod gate;

use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// When OCR runs, mirroring the CLI's `--ocr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OcrMode {
    Auto,
    Always,
    Never,
}

impl OcrMode {
    pub const ALL: [OcrMode; 3] = [OcrMode::Auto, OcrMode::Always, OcrMode::Never];

    pub fn label(self) -> &'static str {
        match self {
            OcrMode::Auto => "Auto (scanned pages only)",
            OcrMode::Always => "Always",
            OcrMode::Never => "Never",
        }
    }

    /// The value the CLI expects.
    pub fn value(self) -> &'static str {
        match self {
            OcrMode::Auto => "auto",
            OcrMode::Always => "always",
            OcrMode::Never => "never",
        }
    }
}

/// The whole form: one input, one output, one settings panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// A file or a directory.
    pub input: String,
    /// Optional; empty means "beside each input".
    ///
    /// A `--selftest` run is the one caller that fills it in without a user having
    /// chosen it: see [`Settings::selftest`] and the scratch directory it names.
    pub output: String,
    pub parallel: u32,
    pub recursive: bool,
    pub ocr: OcrMode,
    pub preview: bool,
    /// The Compare mode's reference: a Docling document, or a folder of them
    /// named after the documents they belong to. Empty means "do not compare".
    pub reference: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input: String::new(),
            output: String::new(),
            parallel: 4,
            recursive: true,
            ocr: OcrMode::Auto,
            preview: false,
            reference: String::new(),
        }
    }
}

impl Settings {
    pub const MIN_PARALLEL: u32 = 1;
    pub const MAX_PARALLEL: u32 = 1000;

    /// The form a `--selftest` run uses: the window's own defaults around `input`
    /// and `reference`, except that what the run converts is written into a
    /// temporary directory of its own instead of beside the document it read.
    ///
    /// The smoke test is pointed at the corpus itself, and `test-data/real` sits
    /// among the committed references: a `*.json` left there is a document with an
    /// unsupported extension to the golden suite, and the `<stem>_images/`
    /// directory a PDF run writes beside its output is litter of the same kind. So a
    /// self-test never writes beside its input. Nothing is cleaned up afterwards
    /// either — a temporary directory is where a run's output belongs, and the
    /// command line the smoke test prints names the directory it wrote in, so what
    /// was converted can still be looked at.
    pub fn selftest(input: &str, reference: &str) -> Settings {
        Settings {
            input: input.to_string(),
            output: scratch_output(input),
            reference: reference.to_string(),
            ..Default::default()
        }
    }

    /// The argument vector the CLI is invoked with, beside the program name.
    ///
    /// `--progress` is always present: the window shows a run as it happens and
    /// offers to stop it, which a single blocking call could not do.
    ///
    /// The Compare mode is deliberately *not* here: a reference is what the
    /// window does with our output, not something the extractor is told about,
    /// and the CLI has no business knowing which Docling document it is up
    /// against.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec!["--input".to_string(), self.input.trim().to_string()];

        let output = self.output.trim();
        if !output.is_empty() {
            args.push("--output".to_string());
            args.push(output.to_string());
        }

        args.push("--parallel".to_string());
        args.push(
            self.parallel
                .clamp(Self::MIN_PARALLEL, Self::MAX_PARALLEL)
                .to_string(),
        );

        args.push("--recursive".to_string());
        args.push(self.recursive.to_string());

        args.push("--ocr".to_string());
        args.push(self.ocr.value().to_string());

        if self.preview {
            args.push("--preview".to_string());
        }

        args.push("--progress".to_string());

        args
    }

    /// Why the form cannot be run yet, or `None` when it is ready.
    pub fn problem(&self) -> Option<String> {
        let input = self.input.trim();
        if input.is_empty() {
            return Some("Choose an input file or folder.".to_string());
        }
        if !PathBuf::from(input).exists() {
            return Some(format!("Input does not exist: {input}"));
        }
        if !(Self::MIN_PARALLEL..=Self::MAX_PARALLEL).contains(&self.parallel) {
            return Some(format!(
                "Parallelism must be between {} and {}.",
                Self::MIN_PARALLEL,
                Self::MAX_PARALLEL
            ));
        }
        let reference = self.reference.trim();
        if !reference.is_empty() && !PathBuf::from(reference).exists() {
            return Some(format!("Reference does not exist: {reference}"));
        }
        None
    }
}

/// Where a `--selftest` run writes what it converts.
///
/// A directory is laid out *in* the scratch directory, which is what the CLI does
/// with `--output <dir>`: the documents keep the structure they had underneath it.
/// A single file gets a name of its own there, because the CLI takes `--output` to
/// be the file itself when the input is a file — the same name a run beside the
/// input would have produced, so what `--selftest` shows is what a real run writes.
fn scratch_output(input: &str) -> String {
    let input = PathBuf::from(input.trim());
    if input.is_dir() {
        return scratch_dir().to_string_lossy().into_owned();
    }
    let name = input
        .with_extension("json")
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.json".to_string());
    scratch_dir().join(name).to_string_lossy().into_owned()
}

/// The directory a `--selftest` run writes in: one of this process's own under the
/// system temporary directory.
///
/// Named after the process so two smoke tests cannot write over each other, and so
/// the path on the run's own command line says which run it belongs to.
fn scratch_dir() -> PathBuf {
    std::env::temp_dir().join(format!("pdfextractor-selftest-{}", std::process::id()))
}

/// One finished document, as the CLI reports it with `--progress`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// How many documents have finished, this one included.
    pub done: usize,
    pub total: usize,
    /// The input path, exactly as the CLI spells it.
    pub input: String,
    pub ok: bool,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// The document list the CLI announces before it converts anything.
#[derive(Debug, Clone, Deserialize)]
struct PlanRecord {
    total: usize,
    inputs: Vec<String>,
}

/// One line of the CLI's output, once classified.
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    /// The queue, sent before the first document finishes.
    Plan { total: usize, inputs: Vec<String> },
    /// A document that has finished, successfully or not.
    Progress(Progress),
    /// Anything else on either stream: log records, warnings, errors.
    Log(String),
}

/// Classify one line of the CLI's output.
///
/// A structured record is recognised by its `kind`; everything else — a
/// `tracing` record, a bare error — is text for the log pane.
pub fn classify(line: &str) -> RunEvent {
    if let Some(record) = record_of(line.trim()) {
        match record.get("kind").and_then(|kind| kind.as_str()) {
            Some("plan") => {
                if let Ok(plan) = serde_json::from_value::<PlanRecord>(record) {
                    return RunEvent::Plan {
                        total: plan.total,
                        inputs: plan.inputs,
                    };
                }
            }
            Some("progress") => {
                if let Ok(progress) = serde_json::from_value::<Progress>(record) {
                    return RunEvent::Progress(progress);
                }
            }
            _ => {}
        }
    }
    RunEvent::Log(strip_ansi(line))
}

/// The record on this line, when it looks like one.
fn record_of(line: &str) -> Option<serde_json::Value> {
    if !line.starts_with('{') {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// Drop terminal colour escapes.
///
/// The log pane is HTML, where `\u{1b}[32m` is not green: it is debris.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // A CSI sequence: ESC `[`, parameters, then a final byte in 0x40..=0x7E.
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// What a stream contained, counted as it was read.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    pub converted: usize,
    pub failed: usize,
    /// The queue size the CLI reported, or 0 before it has been announced.
    pub total: usize,
}

impl Tally {
    /// Fold another stream's count into this one.
    pub fn add(&mut self, other: Tally) {
        self.converted += other.converted;
        self.failed += other.failed;
        self.total = self.total.max(other.total);
    }
}

/// Read a stream to its end, handing every line to `sink` as it arrives.
///
/// This is the whole point of the streaming design: the callback fires while
/// the CLI is still working, so the window can show progress rather than a
/// blank wait.
pub fn pump(mut reader: impl BufRead, sink: &mut dyn FnMut(RunEvent)) -> Tally {
    let mut tally = Tally::default();
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            // The writer closed its end: the stream is over.
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        // Lossy on purpose: a path in some other encoding must not end the
        // stream, and a replacement character in a log line is harmless.
        let line = String::from_utf8_lossy(&buffer);
        let line = line.trim_end_matches(['\n', '\r']);
        let event = classify(line);
        match &event {
            RunEvent::Progress(progress) => {
                if progress.ok {
                    tally.converted += 1;
                } else {
                    tally.failed += 1;
                }
                tally.total = tally.total.max(progress.total);
            }
            RunEvent::Plan { total, .. } => tally.total = tally.total.max(*total),
            RunEvent::Log(text) => {
                // Blank lines are separators, not news.
                if text.trim().is_empty() {
                    continue;
                }
            }
        }
        sink(event);
    }
    tally
}

/// A running CLI process: something to watch, and something to stop.
#[derive(Clone)]
pub struct Job {
    child: Arc<Mutex<Child>>,
    cancelled: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

impl Job {
    /// Start the CLI and hand back its two output streams.
    pub fn spawn(
        program: &str,
        args: &[String],
    ) -> Result<(Job, ChildStdout, ChildStderr), String> {
        let mut command = Command::new(program);
        command
            .args(args)
            // No stdin: the CLI never reads it, and an inherited one could
            // swallow the terminal's keystrokes.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // The CLI's own notes (how many documents, a clamped parallelism) are
        // `tracing` records, which stay silent unless the level is raised. They
        // are worth showing; an explicit RUST_LOG still wins.
        if std::env::var_os("RUST_LOG").is_none() {
            command.env("RUST_LOG", "info");
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("could not run {program}: {e}"))?;

        // Asking for a piped stream is what creates it, so both are present.
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("{program} produced no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| format!("{program} produced no stderr"))?;

        Ok((
            Job {
                child: Arc::new(Mutex::new(child)),
                cancelled: Arc::new(AtomicBool::new(false)),
                finished: Arc::new(AtomicBool::new(false)),
            },
            stdout,
            stderr,
        ))
    }

    /// Stop the process; tells whether a signal was actually sent.
    ///
    /// The kill is a hard one, and it is safe here because the CLI does not
    /// need to tidy up: every document is written through a temporary file and
    /// renamed, so a stopped run leaves whole files or none, never half of one.
    pub fn cancel(&self) -> bool {
        if self.finished() {
            return false;
        }
        // Set before the kill so the waiter thread can tell a stop from an
        // ordinary failure even if the exit is observed first.
        self.cancelled.store(true, Ordering::SeqCst);
        match self.child.lock() {
            Ok(mut child) => child.kill().is_ok(),
            Err(_) => false,
        }
    }

    /// Whether a stop was requested for this run.
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Whether the exit has been observed.
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Wait for the process to exit, looking every `interval`.
    ///
    /// `Child::wait` borrows the child for the whole life of the run, which
    /// would leave `cancel` unable to reach it, so the exit is polled instead.
    /// The interval trades a negligible call every so often for a stop that
    /// takes effect immediately; forty milliseconds is below what anyone can
    /// perceive as a delay.
    pub fn wait_polling(&self, interval: Duration) -> Option<ExitStatus> {
        loop {
            {
                let mut child = self.child.lock().ok()?;
                match child.try_wait() {
                    Ok(Some(status)) => {
                        self.finished.store(true, Ordering::SeqCst);
                        return Some(status);
                    }
                    Ok(None) => {}
                    Err(_) => {
                        self.finished.store(true, Ordering::SeqCst);
                        return None;
                    }
                }
            }
            std::thread::sleep(interval);
        }
    }
}

/// How a run that has exited should be described to the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub success: bool,
    pub cancelled: bool,
    pub exit_code: Option<i32>,
}

/// Decide the verdict for a run that has exited.
///
/// A stop is only reported as a cancellation when the process really was
/// stopped: if the last document finished in the moment between the click and
/// the signal, the run's own result is the honest description.
pub fn outcome(cancelled: bool, status: Option<ExitStatus>) -> Outcome {
    let success = status.map(|status| status.success()).unwrap_or(false);
    Outcome {
        success,
        cancelled: cancelled && !success,
        exit_code: status.and_then(|status| status.code()),
    }
}

/// What the page is told when a run starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    pub run: u64,
    /// The exact command line, so it can be reproduced by hand.
    pub command: String,
}

/// The document list, announced before the first document finishes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanLine {
    pub run: u64,
    pub total: usize,
    pub inputs: Vec<String>,
}

/// One finished document, tagged with the run it belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressLine {
    pub run: u64,
    #[serde(flatten)]
    pub progress: Progress,
}

/// One line of ordinary output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub run: u64,
    pub line: String,
}

/// One document compared with its Docling reference, tagged with its run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    pub run: u64,
    #[serde(flatten)]
    pub report: diff::Report,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finished {
    pub run: u64,
    #[serde(flatten)]
    pub outcome: Outcome,
    pub tally: Tally,
}

/// One heading of the CLI's format list: a family's name as `--help` prints it,
/// and the formats under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Family {
    pub name: String,
    pub input_formats: Vec<String>,
}

/// The CLI's answer to `--list-input-formats`: every format a run takes, and
/// those formats under the headings `--help` prints them in.
///
/// `names` is the flat set — what a run accepts, which the page counts. `families`
/// is the same set arranged for the page and the chooser, in the help's own order,
/// so the window can offer *Pictures* beside *Word* without inventing a grouping
/// of its own that could drift from the one the help states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Formats {
    pub names: Vec<String>,
    pub families: Vec<Family>,
}

/// Ask the CLI which input formats this build accepts.
///
/// The window offers, names and filters by exactly the set a run reads, asked of
/// the binary that will do the reading: `--list-input-formats` prints it as JSON
/// and exits, so a reader added to the command line appears in the window without
/// a list being kept — and drifting — on this side. Nothing is converted and no
/// input path is needed: the CLI answers the question and stops.
///
/// The names come back dotted, the way a user writes a file's name. One printed
/// without a dot is given one, so the page and the native chooser read the same
/// spelling, and a name that is not an extension is refused rather than passed to
/// a chooser that would quietly offer less than the run reads.
pub fn ask_input_formats(program: &str) -> Result<Formats, String> {
    let output = Command::new(program)
        .arg("--list-input-formats")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run {program} --list-input-formats: {error}"))?;
    if !output.status.success() {
        // The CLI's own reason, when it gave one: `anyhow` writes `Error: …`.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("it gave no reason");
        return Err(format!(
            "{program} --list-input-formats exited with {}: {reason}",
            output.status
        ));
    }
    parse_formats(&String::from_utf8_lossy(&output.stdout))
        .map_err(|problem| format!("{program} --list-input-formats: {problem}"))
}

/// Read the flag's record: `{"input_formats": [".pdf", …], "families": […]}`.
///
/// The flat array is the run's set and arrives in the order the CLI prints it,
/// deduplicated because it ends up in a file chooser. `families` is that set under
/// the headings `--help` groups it by, kept in the CLI's order: it is only there
/// for the chooser's grouping, so a family that names nothing is left out rather
/// than offered as an empty filter. An answer with no `families` at all — an older
/// CLI — is still readable: everything goes under one heading, rather than the
/// window refusing a list the run would take.
fn parse_formats(answer: &str) -> Result<Formats, String> {
    let record: serde_json::Value = serde_json::from_str(answer.trim())
        .map_err(|error| format!("its answer is not JSON ({error})"))?;
    let listed = record
        .get("input_formats")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "its answer has no `input_formats` array".to_string())?;

    let names = parse_names(listed, "`input_formats`")?;
    if names.is_empty() {
        return Err("it named no input formats".to_string());
    }

    let families = match record.get("families") {
        None => one_group(&names),
        Some(value) => {
            let listed = value
                .as_array()
                .ok_or_else(|| "its `families` is not an array".to_string())?;
            let mut families = Vec::with_capacity(listed.len());
            for entry in listed {
                let family = entry
                    .as_object()
                    .ok_or_else(|| format!("`families` holds a {entry}, not a family"))?;
                let name = family
                    .get("name")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .unwrap_or_default();
                if name.is_empty() {
                    return Err("`families` holds a family with no name".to_string());
                }
                let names = family
                    .get("input_formats")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| format!("the {name:?} family has no `input_formats` array"))?;
                let input_formats = parse_names(names, &format!("the {name:?} family"))?;
                if !input_formats.is_empty() {
                    families.push(Family {
                        name: name.to_string(),
                        input_formats,
                    });
                }
            }
            if families.is_empty() {
                return Err("its `families` named no input formats".to_string());
            }
            families
        }
    };

    Ok(Formats { names, families })
}

/// The dotted, deduplicated names of one JSON array, refusing anything that is not
/// an extension: a chooser handed a name it cannot filter by would quietly offer
/// less than a run reads.
fn parse_names(listed: &[serde_json::Value], what: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::with_capacity(listed.len());
    for entry in listed {
        let Some(name) = entry.as_str() else {
            return Err(format!("{what} holds a {entry}, not a name"));
        };
        let extension = name.trim().trim_start_matches('.');
        if extension.is_empty()
            || !extension
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
        {
            return Err(format!("{what} holds {name:?}, which is not an extension"));
        }
        let dotted = format!(".{extension}");
        if !names.contains(&dotted) {
            names.push(dotted);
        }
    }
    Ok(names)
}

/// The one heading a CLI without families gets: everything, grouped as a whole.
fn one_group(names: &[String]) -> Vec<Family> {
    vec![Family {
        name: "Documents".to_string(),
        input_formats: names.to_vec(),
    }]
}

/// The filters a native chooser is given, grouped the way `--help` groups them.
///
/// "All documents" comes first with every name, because that is what most picks
/// want; then one filter per family in the CLI's order, so a person opening a
/// picture can choose *Pictures* and see only those, under the same heading the
/// help prints. Families are the CLI's own — this side never invents a name for a
/// group — and a family that offers no filterable extension is left out rather
/// than added as an empty entry.
pub fn chooser_filters(formats: &Formats) -> Vec<(String, Vec<String>)> {
    let mut filters = Vec::with_capacity(formats.families.len() + 1);
    let all = bare_extensions(&formats.names);
    if !all.is_empty() {
        filters.push(("All documents".to_string(), all));
    }
    for family in &formats.families {
        let extensions = bare_extensions(&family.input_formats);
        if !extensions.is_empty() {
            filters.push((family.name.clone(), extensions));
        }
    }
    filters
}

/// The CLI's answer to `--list-input-formats`, kept between asks.
///
/// The window asks on load and the native chooser asks on every click, so the
/// answer is kept rather than a process started per click. Keeping it forever is
/// the other half of the problem: a binary rebuilt, replaced or moved while the
/// window is open would never be seen, because the window would go on naming the
/// formats of the one it asked first. [`Self::refresh`] is the way back — it asks
/// the CLI again and keeps what *that* binary says.
///
/// A failure is kept too: a binary that cannot be found cannot be found twice
/// either, and the reason belongs on the page rather than in a silent nothing.
/// It never survives a refresh, which is exactly the ask a repaired binary
/// should be found by.
type Answer = Result<Formats, String>;

/// Which program an ask runs.
#[derive(Clone)]
enum ProgramSource {
    /// The caller named the binary, so it stays named.
    Fixed(String),
    /// Look the binary up at each ask, so a changed `PDFEXTRACTOR_CLI` is seen.
    Environment,
}

impl ProgramSource {
    fn resolve(&self) -> String {
        match self {
            ProgramSource::Fixed(program) => program.clone(),
            ProgramSource::Environment => locate_cli(),
        }
    }
}

#[derive(Clone)]
pub struct AcceptedFormats {
    source: ProgramSource,
    answer: Arc<Mutex<Option<Answer>>>,
}

impl AcceptedFormats {
    /// The formats `program` names, to be asked for when first needed.
    ///
    /// Use this when the caller owns the path. The window uses
    /// [`Self::from_environment`] instead, because its binary can be pointed
    /// somewhere else while it is open.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            source: ProgramSource::Fixed(program.into()),
            answer: Arc::new(Mutex::new(None)),
        }
    }

    /// The formats of whichever CLI `PDFEXTRACTOR_CLI` names now.
    ///
    /// The path is resolved on every ask, and every refresh asks again, so
    /// moving the variable to a new binary takes effect without restarting the
    /// window. Without the variable, [`locate_cli`]'s usual search — beside the
    /// UI binary, then `PATH` — is what is resolved.
    pub fn from_environment() -> Self {
        Self {
            source: ProgramSource::Environment,
            answer: Arc::new(Mutex::new(None)),
        }
    }

    /// The program the window would run now.
    ///
    /// For an environment-backed instance this resolves `PDFEXTRACTOR_CLI`
    /// again, so the page can name the binary a refresh has just moved to.
    pub fn program(&self) -> String {
        self.source.resolve()
    }

    /// The kept answer, asking the CLI the first time and again after a refresh.
    pub fn get(&self) -> Result<Formats, String> {
        match self.kept() {
            Some(answer) => answer,
            None => self.ask(),
        }
    }

    /// Ask the CLI again, replacing whatever was kept.
    ///
    /// What a window calls when the binary it runs may have been fixed or
    /// replaced underneath it, or when `PDFEXTRACTOR_CLI` has been pointed at a
    /// new path: the answer afterwards is the one a run would get now, not the
    /// one it got when the window opened.
    pub fn refresh(&self) -> Result<Formats, String> {
        self.ask()
    }

    /// What was answered last, and nothing when nothing has been asked.
    fn kept(&self) -> Option<Answer> {
        self.answer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn ask(&self) -> Result<Formats, String> {
        let answer = ask_input_formats(&self.source.resolve());
        *self
            .answer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(answer.clone());
        answer
    }
}

/// The same names without their dots, which is the form a native chooser filters
/// by: `add_filter` takes extensions, not file names.
pub fn bare_extensions(formats: &[String]) -> Vec<String> {
    formats
        .iter()
        .map(|format| format.trim().trim_start_matches('.').to_string())
        .filter(|extension| !extension.is_empty())
        .collect()
}

/// Where the UI expects to find the CLI.
///
/// An explicit `PDFEXTRACTOR_CLI` wins; otherwise the binary sitting next to
/// the UI executable (which is where a workspace build puts it), and finally
/// the name on `PATH`.
pub fn locate_cli() -> String {
    if let Some(path) = std::env::var_os("PDFEXTRACTOR_CLI") {
        return path.to_string_lossy().into_owned();
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let name = if cfg!(windows) {
                "pdfextractor-cli.exe"
            } else {
                "pdfextractor-cli"
            };
            let sibling = dir.join(name);
            if sibling.exists() {
                return sibling.to_string_lossy().into_owned();
            }
        }
    }
    "pdfextractor-cli".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn settings() -> Settings {
        Settings {
            input: env!("CARGO_MANIFEST_DIR").to_string(),
            ..Default::default()
        }
    }

    /// Drain a whole stream, collecting the events and the tally.
    fn drain(text: &str) -> (Vec<RunEvent>, Tally) {
        let mut events = Vec::new();
        let tally = pump(Cursor::new(text.as_bytes()), &mut |event| {
            events.push(event)
        });
        (events, tally)
    }

    #[test]
    fn the_argument_vector_matches_the_cli() {
        let args = settings().to_args();
        assert_eq!(
            args,
            vec![
                "--input",
                env!("CARGO_MANIFEST_DIR"),
                "--parallel",
                "4",
                "--recursive",
                "true",
                "--ocr",
                "auto",
                "--progress",
            ]
        );
    }

    /// The window's formats are the CLI's own answer, not a list kept here: the
    /// record is read as printed, and a name the chooser cannot filter by is
    /// refused rather than quietly dropped.
    #[test]
    fn the_accepted_formats_are_read_out_of_the_clis_answer() {
        let answer = r#"{"input_formats": [".pdf", ".docx", ".md"]}"#;
        assert_eq!(
            parse_formats(answer).unwrap().names,
            vec![".pdf", ".docx", ".md"]
        );

        // A name printed without its dot reads the same, and a repeat is one
        // entry once the list reaches a chooser.
        let undotted = r#"{"input_formats": ["pdf", ".PDF", "pdf"]}"#;
        assert_eq!(
            parse_formats(undotted).unwrap().names,
            vec![".pdf", ".PDF"],
            "names are kept as the CLI spells them"
        );
    }

    /// The families are not this side's idea of a grouping: they are read under the
    /// names the help prints, in the help's order, so the chooser's headings can be
    /// the same words a reader of `--help` already knows.
    #[test]
    fn the_families_are_read_in_the_order_the_help_prints_them() {
        let answer = r#"{
            "input_formats": [".pdf", ".docx", ".png", ".jpg"],
            "families": [
                {"name": "PDF", "input_formats": [".pdf"]},
                {"name": "Word", "input_formats": ["docx"]},
                {"name": "Pictures", "input_formats": [".png", ".jpg", "png"]},
                {"name": "Empty", "input_formats": []}
            ]
        }"#;
        let formats = parse_formats(answer).unwrap();
        assert_eq!(formats.names, vec![".pdf", ".docx", ".png", ".jpg"]);
        assert_eq!(
            formats.families,
            vec![
                Family {
                    name: "PDF".to_string(),
                    input_formats: vec![".pdf".to_string()],
                },
                Family {
                    name: "Word".to_string(),
                    input_formats: vec![".docx".to_string()],
                },
                Family {
                    name: "Pictures".to_string(),
                    input_formats: vec![".png".to_string(), ".jpg".to_string()],
                },
            ],
            "a family that names nothing is no filter, so it is left out"
        );
    }

    /// An older CLI prints the flat list alone. The window still has a chooser: one
    /// heading holding everything, rather than a refusal to offer what a run reads.
    #[test]
    fn a_cli_without_families_still_gives_one_heading() {
        let formats =
            parse_formats(r#"{"input_formats": [".pdf", ".md"]}"#).expect("a flat answer");
        assert_eq!(
            formats.families,
            vec![Family {
                name: "Documents".to_string(),
                input_formats: vec![".pdf".to_string(), ".md".to_string()],
            }]
        );
    }

    /// The page groups by the same answer the chooser does: `names` stays flat for
    /// the count, and each family reaches it as a heading with its own formats.
    #[test]
    fn the_format_answer_serializes_with_the_pages_family_headings() {
        let formats = Formats {
            names: vec![".pdf".to_string(), ".docx".to_string()],
            families: vec![
                Family {
                    name: "PDF".to_string(),
                    input_formats: vec![".pdf".to_string()],
                },
                Family {
                    name: "Word".to_string(),
                    input_formats: vec![".docx".to_string()],
                },
            ],
        };
        let value = json(&formats);
        assert_eq!(value["names"], serde_json::json!([".pdf", ".docx"]));
        assert_eq!(value["families"][0]["name"], "PDF");
        assert_eq!(
            value["families"][0]["inputFormats"],
            serde_json::json!([".pdf"]),
            "the page reads the camelCase field"
        );
        assert_eq!(
            value["families"][1]["inputFormats"],
            serde_json::json!([".docx"])
        );
    }

    /// What the native chooser is handed: every name under "All documents" first,
    /// then one filter per family in the help's order, without dots because a
    /// filter takes extensions rather than file names.
    #[test]
    fn the_chooser_gets_all_documents_then_one_filter_per_family() {
        let formats = Formats {
            names: vec![".pdf".to_string(), ".docx".to_string(), ".png".to_string()],
            families: vec![
                Family {
                    name: "PDF".to_string(),
                    input_formats: vec![".pdf".to_string()],
                },
                Family {
                    name: "Word".to_string(),
                    input_formats: vec![".docx".to_string()],
                },
                Family {
                    name: "Pictures".to_string(),
                    input_formats: vec![".png".to_string()],
                },
            ],
        };
        assert_eq!(
            chooser_filters(&formats),
            vec![
                (
                    "All documents".to_string(),
                    vec!["pdf".to_string(), "docx".to_string(), "png".to_string()]
                ),
                ("PDF".to_string(), vec!["pdf".to_string()]),
                ("Word".to_string(), vec!["docx".to_string()]),
                ("Pictures".to_string(), vec!["png".to_string()]),
            ]
        );
    }

    #[test]
    fn an_answer_that_is_not_a_format_list_is_refused_with_the_reason() {
        for (answer, expected) in [
            ("no formats here", "not JSON"),
            ("[]", "no `input_formats` array"),
            (r#"{"input_formats": []}"#, "named no input formats"),
            (r#"{"input_formats": [".pdf", 7]}"#, "not a name"),
            (r#"{"input_formats": [".p df"]}"#, "not an extension"),
            (r#"{"input_formats": [""]}"#, "not an extension"),
            (
                r#"{"input_formats": [".pdf"], "families": {}}"#,
                "not an array",
            ),
            (
                r#"{"input_formats": [".pdf"], "families": [7]}"#,
                "not a family",
            ),
            (
                r#"{"input_formats": [".pdf"], "families": [{"input_formats": [".pdf"]}]}"#,
                "no name",
            ),
            (
                r#"{"input_formats": [".pdf"], "families": [{"name": "PDF"}]}"#,
                "no `input_formats` array",
            ),
            (
                r#"{"input_formats": [".pdf"], "families": [{"name": "PDF", "input_formats": [7]}]}"#,
                "the \"PDF\" family holds a 7",
            ),
            (
                r#"{"input_formats": [".pdf"], "families": []}"#,
                "`families` named no input formats",
            ),
        ] {
            let problem =
                parse_formats(answer).expect_err(&format!("{answer} was read as a format list"));
            assert!(
                problem.contains(expected),
                "{answer}: expected {expected:?}, got {problem:?}"
            );
        }
    }

    /// A CLI that is not there is a reason on the page, not a panic in the
    /// window — the same call runs before a conversion would, so the message is
    /// the first sign the binary is missing.
    #[test]
    fn a_cli_that_cannot_be_asked_is_an_error_naming_it() {
        let problem =
            ask_input_formats("/definitely/not/a/program").expect_err("a missing program answered");
        assert!(
            problem.contains("/definitely/not/a/program"),
            "the program is not named: {problem}"
        );
    }

    /// An existing program that answers something else — `echo` prints the flag
    /// back — is an error about the answer, not a list of nonsense names.
    #[cfg(unix)]
    #[test]
    fn a_cli_that_answers_something_else_is_an_error_about_the_answer() {
        let problem =
            ask_input_formats("/bin/echo").expect_err("echo's answer was read as a format list");
        assert!(problem.contains("not JSON"), "{problem}");
    }

    /// The window asks once and asks again: the kept answer is what the chooser
    /// filters by, and a refresh replaces it with what the binary says *now* —
    /// which is how a CLI rebuilt or replaced under an open window is picked up.
    ///
    /// The stand-in binary answers from a counter of its own calls, so "asked
    /// again" is a fact about the process, not about the return value.
    #[cfg(unix)]
    #[test]
    fn a_refresh_asks_the_cli_again_so_a_replaced_binary_is_seen() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "pdfextractor-formats-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a temporary directory for the stand-in CLI");
        let script = dir.join("cli.sh");
        // First ask: one format. Every ask after: the fixed binary's two.
        let stand_in = r#"#!/bin/sh
            dir=$(dirname "$0")
            n=$(cat "$dir/calls" 2>/dev/null || echo 0)
            n=$((n + 1))
            echo $n > "$dir/calls"
            if [ "$n" -eq 1 ]; then
              echo '{"input_formats": [".pdf"]}'
            else
              echo '{"input_formats": [".pdf", ".rtf"]}'
            fi
            "#;
        std::fs::write(&script, stand_in).expect("the stand-in CLI");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("an executable stand-in CLI");

        let formats = AcceptedFormats::new(script.to_string_lossy().into_owned());
        assert_eq!(formats.get().unwrap().names, vec![".pdf"]);
        assert_eq!(
            formats.get().unwrap().names,
            vec![".pdf"],
            "the answer is kept, not asked for again on every chooser click"
        );
        assert_eq!(formats.refresh().unwrap().names, vec![".pdf", ".rtf"]);
        assert_eq!(
            formats.get().unwrap().names,
            vec![".pdf", ".rtf"],
            "the refresh replaced the kept answer"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The window's formats follow the environment, not the path it resolved
    /// when it opened: a refresh re-resolves `PDFEXTRACTOR_CLI`, so pointing it
    /// at a rebuilt binary in another directory takes effect without restarting.
    #[cfg(unix)]
    #[test]
    fn a_refresh_re_resolves_the_cli_named_by_the_environment() {
        use std::os::unix::fs::PermissionsExt;

        let previous = std::env::var_os("PDFEXTRACTOR_CLI");
        let dir = std::env::temp_dir().join(format!(
            "pdfextractor-relocate-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a temporary directory for the stand-in CLIs");
        let write = |name: &str, formats: &str| {
            let path = dir.join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\necho '{{\"input_formats\": [{formats}]}}'\n"),
            )
            .expect("a stand-in CLI");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("an executable stand-in CLI");
            path
        };
        let first = write("first.sh", "\".pdf\"");
        let second = write("second.sh", "\".pdf\", \".rtf\"");

        std::env::set_var("PDFEXTRACTOR_CLI", &first);
        let formats = AcceptedFormats::from_environment();
        assert_eq!(formats.get().unwrap().names, vec![".pdf"]);
        assert_eq!(formats.program(), first.to_string_lossy());

        std::env::set_var("PDFEXTRACTOR_CLI", &second);
        assert_eq!(
            formats.program(),
            second.to_string_lossy(),
            "the displayed program follows the environment"
        );
        assert_eq!(
            formats.get().unwrap().names,
            vec![".pdf"],
            "the kept answer is not re-resolved until a refresh"
        );
        assert_eq!(
            formats.refresh().unwrap().names,
            vec![".pdf", ".rtf"],
            "the refresh still asked the binary the environment named at setup"
        );
        assert_eq!(formats.get().unwrap().names, vec![".pdf", ".rtf"]);

        match previous {
            Some(value) => std::env::set_var("PDFEXTRACTOR_CLI", value),
            None => std::env::remove_var("PDFEXTRACTOR_CLI"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_chooser_is_given_the_names_without_their_dots() {
        assert_eq!(
            bare_extensions(&[".pdf".to_string(), ".jpg".to_string()]),
            vec!["pdf", "jpg"]
        );
    }

    #[test]
    fn output_preview_and_flags_are_forwarded() {
        let mut s = settings();
        s.output = "/tmp/out".to_string();
        s.parallel = 16;
        s.recursive = false;
        s.ocr = OcrMode::Always;
        s.preview = true;

        let args = s.to_args();
        let joined = args.join(" ");
        assert!(joined.contains("--output /tmp/out"), "{joined}");
        assert!(joined.contains("--parallel 16"), "{joined}");
        assert!(joined.contains("--recursive false"), "{joined}");
        assert!(joined.contains("--ocr always"), "{joined}");
        assert!(joined.contains("--preview"), "{joined}");
        assert!(joined.contains("--progress"), "{joined}");
    }

    /// The Compare mode is the window's own business: the reference is what we
    /// compare against afterwards, so the extractor is never told about it.
    #[test]
    fn a_reference_is_not_an_argument_to_the_cli() {
        let mut s = settings();
        s.reference = format!("{}/dist", env!("CARGO_MANIFEST_DIR"));

        let args = s.to_args();
        assert!(
            !args
                .iter()
                .any(|arg| *arg == s.reference || arg.contains("reference")),
            "the reference reached the command line: {args:?}"
        );
        // Nine arguments: the input, the parallelism, recursion, OCR and
        // `--progress`, with a value for each flag that takes one.
        assert_eq!(args.len(), 9, "{args:?}");
    }

    #[test]
    fn a_reference_that_is_not_there_is_refused() {
        let mut s = settings();
        s.reference = "/definitely/not/here".to_string();
        assert!(s.problem().unwrap().contains("Reference does not exist"));

        // An empty reference is the mode being off, which is not a problem.
        s.reference = "   ".to_string();
        assert_eq!(s.problem(), None);
    }

    /// The smoke test is pointed at the corpus to be useful, so it must not write
    /// beside it: a stray `*.json` among the documents is a document with an
    /// unsupported extension to the golden suite, and a `<stem>_images/` directory
    /// is litter of the same kind (one such run left `mathtype_images/` behind).
    /// A single-file self-test therefore converts into a temporary directory of its
    /// own — under the same name a run beside the input would have written, so what
    /// the smoke test shows is still what a real run produces.
    #[test]
    fn a_selftest_of_a_file_writes_into_a_temporary_directory_not_beside_it() {
        let input = format!("{}/dist/index.html", env!("CARGO_MANIFEST_DIR"));
        let settings = Settings::selftest(&input, "test-data/golden/docling");

        let output = PathBuf::from(&settings.output);
        assert!(
            output.starts_with(std::env::temp_dir()),
            "not a temporary directory: {output:?}"
        );
        assert_eq!(
            output.file_name().unwrap().to_string_lossy(),
            "index.json",
            "{output:?}"
        );
        assert!(
            output
                .parent()
                .is_some_and(|parent| parent
                    .ends_with(format!("pdfextractor-selftest-{}", std::process::id()))),
            "not a directory of this run's own: {output:?}"
        );
        assert_ne!(
            output,
            PathBuf::from(&input).with_extension("json"),
            "the run would still write beside its input"
        );

        // The CLI is really told, and nothing else about the form is special.
        let args = settings.to_args();
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--output" && pair[1] == settings.output),
            "no --output in {args:?}"
        );
        // The output path is the only thing about the form a self-test decides:
        // everything else is the window's own default, and the reference travels
        // beside the run rather than into the CLI's arguments.
        let defaults = Settings::default();
        assert_eq!(settings.parallel, defaults.parallel);
        assert_eq!(settings.recursive, defaults.recursive);
        assert_eq!(settings.ocr, defaults.ocr);
        assert_eq!(settings.preview, defaults.preview);
        assert_eq!(settings.input, input);
        assert_eq!(settings.reference, "test-data/golden/docling");
        assert!(
            !args.iter().any(|arg| arg.contains("golden/docling")),
            "the reference reached the command line: {args:?}"
        );
    }

    /// A directory input is laid out in the scratch directory, which is what the
    /// CLI does with `--output <dir>`: the whole point is that smoking-testing
    /// `test-data/real` leaves nothing in `test-data/real`.
    #[test]
    fn a_selftest_of_a_directory_lays_it_out_in_the_scratch_directory() {
        let settings = Settings::selftest(env!("CARGO_MANIFEST_DIR"), "");
        let output = PathBuf::from(&settings.output);
        assert!(output.starts_with(std::env::temp_dir()), "{output:?}");
        assert_eq!(
            output.file_name().unwrap().to_string_lossy(),
            format!("pdfextractor-selftest-{}", std::process::id())
        );
        // A directory, not a file: the CLI keeps each document's place under it.
        assert_eq!(
            output.components().count(),
            std::env::temp_dir().components().count() + 1
        );
    }

    #[test]
    fn every_run_streams_progress() {
        // The window shows a live run and a Stop button, which a blocking call
        // could not do, so `--progress` is not optional.
        assert!(settings().to_args().contains(&"--progress".to_string()));
    }

    #[test]
    fn an_empty_input_is_not_runnable() {
        let problem = Settings::default().problem();
        assert!(problem.is_some(), "an empty input must be refused");
        assert!(problem.unwrap().contains("Choose an input"));
    }

    #[test]
    fn a_missing_input_is_reported() {
        let s = Settings {
            input: "/definitely/not/here".to_string(),
            ..Default::default()
        };
        assert!(s.problem().unwrap().contains("does not exist"));
    }

    #[test]
    fn a_ready_form_has_no_problem() {
        assert_eq!(settings().problem(), None);
    }

    #[test]
    fn ocr_modes_map_to_the_cli_values() {
        assert_eq!(OcrMode::Auto.value(), "auto");
        assert_eq!(OcrMode::Always.value(), "always");
        assert_eq!(OcrMode::Never.value(), "never");
    }

    #[test]
    fn a_plan_line_announces_the_queue() {
        let line = r#"{"kind":"plan","total":2,"inputs":["/a.pdf","/b.txt"]}"#;
        match classify(line) {
            RunEvent::Plan { total, inputs } => {
                assert_eq!(total, 2);
                assert_eq!(inputs, vec!["/a.pdf", "/b.txt"]);
            }
            other => panic!("expected a plan, got {other:?}"),
        }
    }

    #[test]
    fn a_progress_line_names_the_document_and_its_result() {
        let ok = r#"{"kind":"progress","done":1,"total":2,"input":"/a b.pdf","ok":true,"output":"/a b.json"}"#;
        match classify(ok) {
            RunEvent::Progress(progress) => {
                assert_eq!(progress.done, 1);
                assert_eq!(progress.total, 2);
                assert_eq!(progress.input, "/a b.pdf");
                assert!(progress.ok);
                assert_eq!(progress.output.as_deref(), Some("/a b.json"));
                assert_eq!(progress.error, None);
            }
            other => panic!("expected progress, got {other:?}"),
        }

        let failed = r#"{"kind":"progress","done":2,"total":2,"input":"/bad.pdf","ok":false,"error":"broken xref"}"#;
        match classify(failed) {
            RunEvent::Progress(progress) => {
                assert!(!progress.ok);
                assert_eq!(progress.error.as_deref(), Some("broken xref"));
                assert_eq!(progress.output, None);
            }
            other => panic!("expected progress, got {other:?}"),
        }
    }

    #[test]
    fn a_log_record_is_text_not_a_progress_report() {
        let line = "\u{1b}[32m INFO\u{1b}[0m pdfextractor_cli: 3 document(s) to convert";
        match classify(line) {
            RunEvent::Log(text) => {
                assert_eq!(text, " INFO pdfextractor_cli: 3 document(s) to convert")
            }
            other => panic!("expected a log line, got {other:?}"),
        }
    }

    #[test]
    fn a_json_object_that_is_not_a_record_is_still_logged() {
        // A document could legitimately log a brace; and a record missing a
        // field must not be mistaken for a finished document.
        for line in [
            "{\"hello\":\"world\"}",
            "{\"kind\":\"progress\"}",
            "{\"kind\":\"plan\"}",
            "{\"kind\":\"something-else\"}",
            "not json at all",
        ] {
            match classify(line) {
                RunEvent::Log(text) => assert_eq!(text, line),
                other => panic!("{line} was read as {other:?}"),
            }
        }
    }

    #[test]
    fn colour_escapes_are_removed_from_log_text() {
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m done"), "ok done");
        assert_eq!(strip_ansi("plain text"), "plain text");
        // A stray escape without a sequence leaves the text alone.
        assert_eq!(strip_ansi("a\u{1b}b"), "ab");
    }

    #[test]
    fn a_whole_run_pumps_into_ordered_events_and_a_tally() {
        let stream = concat!(
            "{\"kind\":\"plan\",\"total\":3,\"inputs\":[\"/a.md\",\"/b.pdf\",\"/c.md\"]}\n",
            "\u{1b}[32m INFO\u{1b}[0m cli: 3 document(s) to convert\n",
            "{\"kind\":\"progress\",\"done\":1,\"total\":3,\"input\":\"/a.md\",\"ok\":true,\"output\":\"/a.json\"}\n",
            "\n",
            "{\"kind\":\"progress\",\"done\":2,\"total\":3,\"input\":\"/b.pdf\",\"ok\":false,\"error\":\"broken\"}\n",
            "error: /b.pdf: broken\n",
            "{\"kind\":\"progress\",\"done\":3,\"total\":3,\"input\":\"/c.md\",\"ok\":true,\"output\":\"/c.json\"}\n",
        );
        let (events, tally) = drain(stream);

        assert_eq!(
            tally,
            Tally {
                converted: 2,
                failed: 1,
                total: 3
            }
        );
        // Blank lines are dropped; everything else keeps its order.
        assert_eq!(events.len(), 6, "{events:#?}");
        assert!(matches!(events[0], RunEvent::Plan { .. }));
        assert!(matches!(events[1], RunEvent::Log(_)));
        assert!(matches!(&events[2], RunEvent::Progress(p) if p.done == 1 && p.ok));
        assert!(matches!(&events[3], RunEvent::Progress(p) if p.done == 2 && !p.ok));
        assert!(matches!(&events[4], RunEvent::Log(l) if l == "error: /b.pdf: broken"));
        assert!(matches!(&events[5], RunEvent::Progress(p) if p.done == 3));
    }

    #[test]
    fn a_stream_without_records_still_counts_nothing_and_logs_everything() {
        let (events, tally) = drain("warning: something\nanother line\n");
        assert_eq!(tally, Tally::default());
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn windows_line_endings_do_not_leak_into_the_paths() {
        let (events, _) = drain(
            "{\"kind\":\"progress\",\"done\":1,\"total\":1,\"input\":\"C:\\\\a.pdf\",\"ok\":true}\r\n",
        );
        match &events[0] {
            RunEvent::Progress(progress) => assert_eq!(progress.input, "C:\\a.pdf"),
            other => panic!("expected progress, got {other:?}"),
        }
    }

    /// The field names below are a contract with `dist/app.js`. Renaming one
    /// here would empty part of the window without failing anything else, so
    /// they are pinned to the exact keys the page reads.
    #[test]
    fn every_event_the_page_listens_for_has_the_shape_it_expects() {
        fn keys(value: &serde_json::Value) -> Vec<String> {
            let mut keys: Vec<String> = value
                .as_object()
                .expect("an event payload is an object")
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        }

        let started = Started {
            run: 1,
            command: "pdfextractor-cli --input a".to_string(),
        };
        assert_eq!(keys(&json(&started)), vec!["command", "run"]);

        let plan = PlanLine {
            run: 1,
            total: 3,
            inputs: vec!["a.md".to_string()],
        };
        assert_eq!(keys(&json(&plan)), vec!["inputs", "run", "total"]);

        let progress = ProgressLine {
            run: 1,
            progress: Progress {
                done: 1,
                total: 3,
                input: "a.md".to_string(),
                ok: true,
                output: Some("a.json".to_string()),
                error: None,
            },
        };
        assert_eq!(
            keys(&json(&progress)),
            vec!["done", "error", "input", "ok", "output", "run", "total"]
        );

        let log = LogLine {
            run: 1,
            line: "hi".to_string(),
        };
        assert_eq!(keys(&json(&log)), vec!["line", "run"]);

        let finished = Finished {
            run: 1,
            outcome: Outcome {
                success: true,
                cancelled: false,
                exit_code: Some(0),
            },
            tally: Tally {
                converted: 2,
                failed: 1,
                total: 3,
            },
        };
        let value = json(&finished);
        assert_eq!(
            keys(&value),
            vec!["cancelled", "exitCode", "run", "success", "tally"]
        );
        assert_eq!(keys(&value["tally"]), vec!["converted", "failed", "total"]);

        let diff = DiffLine {
            run: 1,
            report: diff::Report {
                input: "a.pdf".to_string(),
                reference: "a.json".to_string(),
                problem: None,
                summary: diff::Summary {
                    rows: 1,
                    same: 1,
                    notation: 0,
                    differs: 0,
                    contained: 0,
                    ours_only: 0,
                    theirs_only: 0,
                    placed: 0,
                    paired_by_position: 0,
                    text_agreement: 1.0,
                    pages_ours: 1,
                    pages_theirs: 1,
                    truncated: 0,
                    notes: Vec::new(),
                },
                placed_pages: Vec::new(),
                // Nothing about this comparison is below the floor its memory remembers,
                // and nothing held it to one: the fields are here so the page's contract
                // with the Rust side is pinned to the exact keys it reads (`floor` and
                // `held` among them). `held` carries a value so its key is pinned rather
                // than hidden behind the skip that leaves an unheld document's line
                // without it.
                floor: None,
                held: Some(diff::Held {
                    rules: diff::HeldRules::Ratchet,
                    why: "every rule of the ratchet applies: this run was measured the way \
                          the memory records (--ocr never)"
                        .to_string(),
                }),
                rows: vec![
                    diff::Row {
                        kind: diff::Kind::Text,
                        verdict: diff::Verdict::Same,
                        ours: vec![diff::Block {
                            kind: diff::Kind::Text,
                            label: "paragraph".to_string(),
                            text: "hello".to_string(),
                            page: Some(1),
                            place: None,
                        }],
                        theirs: vec![diff::Block {
                            kind: diff::Kind::Text,
                            label: "paragraph".to_string(),
                            text: "hello".to_string(),
                            page: None,
                            place: None,
                        }],
                        note: None,
                        placement: None,
                    },
                    // A block only one side has: the missing side is left out
                    // entirely rather than sent as an empty list.
                    diff::Row {
                        kind: diff::Kind::Picture,
                        verdict: diff::Verdict::TheirsOnly,
                        ours: Vec::new(),
                        theirs: vec![diff::Block {
                            kind: diff::Kind::Picture,
                            label: "picture".to_string(),
                            text: "image/png 10x10".to_string(),
                            page: None,
                            place: None,
                        }],
                        note: Some("only the reference: picture".to_string()),
                        placement: None,
                    },
                    // A row that stands for a *place*: the one kind of row the page
                    // draws rather than reads, so what it carries is a contract too.
                    diff::Row {
                        kind: diff::Kind::Text,
                        verdict: diff::Verdict::Placed,
                        ours: Vec::new(),
                        theirs: vec![diff::Block {
                            kind: diff::Kind::Text,
                            label: "text".to_string(),
                            text: ". . . . . .".to_string(),
                            page: Some(1),
                            place: None,
                        }],
                        note: Some(
                            "a dotted leader or a rule at page 1, box 50,560–300,580".to_string(),
                        ),
                        placement: Some(diff::Placement {
                            page: 1,
                            width: 600.0,
                            height: 800.0,
                            x: 50.0 / 600.0,
                            y: 560.0 / 800.0,
                            w: 250.0 / 600.0,
                            h: 20.0 / 800.0,
                        }),
                    },
                ],
            },
        };
        let value = json(&diff);
        assert_eq!(
            keys(&value),
            vec!["held", "input", "reference", "rows", "run", "summary"]
        );
        // Which rules held the document, as the page reads them.
        assert_eq!(value["held"]["rules"], "ratchet");
        assert_eq!(
            keys(&value["summary"]),
            vec![
                "contained",
                "differs",
                "notation",
                "notes",
                "oursOnly",
                "pagesOurs",
                "pagesTheirs",
                "pairedByPosition",
                "placed",
                "rows",
                "same",
                "textAgreement",
                "theirsOnly",
                "truncated",
            ]
        );
        assert_eq!(
            keys(&value["rows"][0]),
            vec!["kind", "ours", "theirs", "verdict"]
        );
        assert_eq!(
            keys(&value["rows"][1]),
            vec!["kind", "note", "theirs", "verdict"]
        );
        // A placed row is the one row that carries a box to draw: where it is as
        // fractions of the page, and the shape of the page it was found on.
        assert_eq!(
            keys(&value["rows"][2]),
            vec!["kind", "note", "placement", "theirs", "verdict"]
        );
        assert_eq!(
            keys(&value["rows"][2]["placement"]),
            vec!["h", "height", "page", "w", "width", "x", "y"]
        );
        assert_eq!(value["rows"][2]["placement"]["page"], 1);
        assert_eq!(value["rows"][2]["placement"]["width"], 600.0);
        assert_eq!(value["rows"][2]["placement"]["height"], 800.0);
        assert!((value["rows"][2]["placement"]["x"].as_f64().unwrap() - 50.0 / 600.0).abs() < 1e-9);
        assert_eq!(value["rows"][0]["verdict"], "same");
        assert_eq!(value["rows"][0]["kind"], "text");
        // The verdict spellings are what the page paints a row by.
        assert_eq!(value["rows"][1]["verdict"], "theirs-only");
        assert_eq!(value["rows"][2]["verdict"], "placed");
        // Every other row leaves the box out entirely rather than sending null.
        assert_eq!(value["rows"][1].get("placement"), None);
        // A block's page is left out when the document has no geometry (a flow
        // format) rather than sent as null.
        // A row's sides are *lists*: the page draws every block a side has, and a
        // reference item cut finer than our block is a run of them.
        assert_eq!(value["rows"][0]["ours"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            keys(&value["rows"][0]["ours"][0]),
            vec!["kind", "label", "page", "text"]
        );
        assert_eq!(value["rows"][0]["theirs"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            keys(&value["rows"][0]["theirs"][0]),
            vec!["kind", "label", "text"]
        );
    }

    fn json(value: &impl Serialize) -> serde_json::Value {
        serde_json::to_value(value).expect("a payload serialises")
    }
}
