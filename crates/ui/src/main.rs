//! The Tauri shell: a window around the logic in `lib.rs`.
//!
//! Exposed to the frontend: picking a path with the native chooser, starting a
//! conversion, stopping the one in flight, and a readiness ping the frontend
//! sends on load (which also lets `--selftest` prove the webview mounted and
//! the IPC round-trips).
//!
//! Which input formats the window offers is not decided here: [`input_formats`]
//! asks the CLI (`--list-input-formats`), and both the line and headings the page
//! draws them under and the chooser's filter are that answer. It is kept between
//! asks, and [`refresh_input_formats`] asks again — so a binary rebuilt or replaced while
//! the window is open, or a `PDFEXTRACTOR_CLI` pointed at a new path, is picked
//! up without reopening it. [`cli_program`] names the binary a refresh resolved,
//! for the line beside the formats, and the `conversion-started` event carries the
//! path a run resolved for itself, so the line follows the binary a run was given
//! when a start resolves a different one.
//!
//! The **Compare** mode is wired here too: when the form carries a reference,
//! every document that finishes is compared with it and the result is sent to
//! the page beside the document's own report.
//!
//! A conversion is never waited on. The CLI is started, its output is streamed
//! to the page as it arrives, and the process stays reachable so a stop takes
//! effect at once.

// Keep a console window from appearing alongside the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use pdfextractor_ui::{
    chooser_filters, diff, gate, locate_cli, outcome, pump, AcceptedFormats, ChunkType, DiffLine,
    Finished, Formats, Job, LogLine, OutputFormat, PlanLine, ProgressLine, RunEvent, Settings,
    Started,
};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdout};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// How often the waiter thread looks for the process exit.
const POLL: Duration = Duration::from_millis(40);

/// The events the page listens for.
const STARTED: &str = "conversion-started";
/// Sent to the page only by `--selftest`, which asks it to report what it drew,
/// to tick the chunking row for `--selftest-chunks`, and to show one tokenizer
/// file's badge for `--selftest-tokenizer`.
const SELFTEST_ON: &str = "selftest-enabled";
const PLAN: &str = "conversion-plan";
const PROGRESS: &str = "conversion-progress";
const LOG: &str = "conversion-log";
const FINISHED: &str = "conversion-finished";
/// One document beside its Docling reference, sent when Compare is on.
const DIFF: &str = "conversion-diff";

/// `--selftest [input]`: verify the webview mounted, and optionally convert the
/// given path through the real CLI before exiting. Used by the smoke test.
static SELFTEST: OnceLock<Option<String>> = OnceLock::new();

/// `--selftest-stop MILLIS`: with `--selftest`, stop the run that far in.
///
/// Stopping is the one part of the window that cannot be checked by letting a
/// conversion finish, so the smoke test can ask for a stop part way through and
/// see what the page makes of it.
static SELFTEST_STOP: OnceLock<Option<u64>> = OnceLock::new();

/// `--selftest-ref PATH`: with `--selftest`, turn the Compare mode on for the
/// run, so the smoke test can check that a comparison reaches the page.
static SELFTEST_REFERENCE: OnceLock<Option<String>> = OnceLock::new();

/// `--selftest-chunks`: with `--selftest`, tick Retrieval chunks and choose the
/// hybrid chunker, so the smoke test checks the row's shown state as well as the
/// run it describes.
static SELFTEST_CHUNKS: OnceLock<bool> = OnceLock::new();

/// `--selftest-tokenizer PATH`: with `--selftest`, put PATH in the tokenizer
/// field and let the page resolve its badge through the real format command.
/// The page reports the rendered label and exits; no conversion is started.
static SELFTEST_TOKENIZER: OnceLock<Option<String>> = OnceLock::new();

/// `--selftest-clear-tokenizer`: with `--selftest-tokenizer`, clear the field
/// after its badge is painted, so a smoke test checks that the label and its
/// tooltip are gone.
static SELFTEST_CLEAR_TOKENIZER: OnceLock<bool> = OnceLock::new();

/// `--selftest-tokenizer-encoding VALUE`: with `--selftest-tokenizer`, choose
/// the tiktoken family in the encoding selector. With an input and
/// `--selftest-chunks`, the page runs the conversion, so the smoke test checks
/// that the chosen family reaches the CLI's command line.
static SELFTEST_TOKENIZER_ENCODING: OnceLock<Option<String>> = OnceLock::new();

/// `--selftest-bad-range RANGE`: with `--selftest`, fill the page-range box with
/// a range the CLI refuses, press Convert, and report what the page drew. The
/// refusal must arrive through the same command the button uses, before a
/// conversion process exists.
static SELFTEST_BAD_RANGE: OnceLock<Option<String>> = OnceLock::new();

/// `--selftest-bad-budget VALUE`: with `--selftest`, tick Retrieval chunks, choose
/// hybrid, type VALUE into the token budget and press Convert, so a malformed
/// budget is refused in the window a person would see the refusal in.
static SELFTEST_BAD_BUDGET: OnceLock<Option<String>> = OnceLock::new();

/// `--selftest-bad-overlap VALUE`: the same for the overlap box.
static SELFTEST_BAD_OVERLAP: OnceLock<Option<String>> = OnceLock::new();

/// `--selftest-bad-encoding VALUE`: with `--selftest`, tick Retrieval chunks,
/// choose hybrid, choose VALUE in the encoding selector with no tokenizer file,
/// and press Convert. The form must refuse the orphaned encoding before a
/// conversion process exists.
static SELFTEST_BAD_ENCODING: OnceLock<Option<String>> = OnceLock::new();

/// `--selftest-reveal`: with `--selftest`, press the result pane's action once a
/// run finishes and print the path it asks to reveal. The command reports that
/// path instead of opening a file manager, so a test can check what the window
/// would reveal without a desktop window appearing over it.
static SELFTEST_REVEAL: OnceLock<bool> = OnceLock::new();

/// The conversion in flight, if any.
#[derive(Default)]
struct ActiveRun {
    /// Bumped per run so the page can ignore events from an older one.
    next: AtomicU64,
    current: Mutex<Option<Running>>,
}

struct Running {
    run: u64,
    job: Job,
}

/// Native file/folder chooser. `kind` is `"document"` (a file the extractor
/// reads), `"file"` (any file, such as a comparison's reference) or `"folder"`.
#[tauri::command]
fn pick_path(formats: State<'_, AcceptedFormats>, kind: String) -> Option<String> {
    let mut dialog = rfd::FileDialog::new();
    // A document is filtered by the formats the CLI named — the kept answer, so a
    // refresh on the page is the filter here too — so the chooser offers what a
    // run will take and nothing it would refuse. The filters are grouped the way
    // `--help` groups them: "All documents" first with every name, then one entry
    // per family under the heading the help prints, all of them the CLI's own.
    // A CLI that could not be asked leaves the chooser unfiltered — picking from no
    // list beats picking from an invented one — and the page says why on the same
    // line.
    if kind == "document" {
        if let Ok(formats) = formats.get() {
            for (label, extensions) in chooser_filters(&formats) {
                dialog = dialog.add_filter(label, &extensions);
            }
        }
    }
    let picked = if kind == "folder" {
        dialog.pick_folder()
    } else {
        dialog.pick_file()
    };
    picked.map(|path| path.to_string_lossy().into_owned())
}

/// The input formats the CLI accepts and their families, for the page to name
/// and group under the same headings as the chooser.
///
/// Asked rather than assumed: the window shows the set a run will take, so a
/// reader added to the command line reaches the window with no change here.
#[tauri::command]
async fn input_formats(formats: State<'_, AcceptedFormats>) -> Result<Formats, String> {
    // The page groups by the same `families` the native chooser's filters are
    // built from. Spawning the CLI is a blocking call, kept off the window's
    // thread so the page is never held while the answer is on its way.
    ask(formats, AcceptedFormats::get).await
}

/// The command line binary the window would run now, for the page to name
/// beside the formats. Resolved when asked, so a refresh shows a moved
/// `PDFEXTRACTOR_CLI` rather than the path the window opened with.
#[tauri::command]
fn cli_program(formats: State<'_, AcceptedFormats>) -> String {
    formats.program()
}

/// The format of the chosen `--chunks-tokenizer` file and its vocabulary size
/// or tiktoken family, for the page to name beside it. The chunker's own loader
/// answers, so an extensionless file is recognized by its bytes and the number
/// is the one a run will use.
#[tauri::command]
fn tokenizer_format(path: String) -> Result<String, String> {
    // A file whose name and bytes name no format gets its own label: it is not
    // a malformed tokenizer of a known kind, and the loader's detailed message
    // would imply a format the file never claimed.
    if pdfextractor_md_exporter::chunks::detect_tokenizer_format(&path).is_err() {
        return Err("Unknown format".to_string());
    }
    pdfextractor_md_exporter::chunks::Tokenizer::from_file(&path)
        .map(|tokenizer| tokenizer.description().to_string())
        .map_err(|error| error.to_string())
}

/// Ask the CLI again, replacing the kept answer.
///
/// The binary a run uses can be rebuilt, replaced or repaired while the window is
/// open, and `PDFEXTRACTOR_CLI` can be pointed at a different binary without
/// restarting the window; either way the old answer would go on naming the
/// formats of the one it asked when it opened, and a reader added to the new
/// binary would be missing from the line, the chooser and — worse — a run would
/// take formats the window never offered. The program is resolved again, then
/// asked; the fresh answer lets the page redraw both the line and its headings,
/// and the chooser filters by the same answer because it is kept in one place.
#[tauri::command]
async fn refresh_input_formats(formats: State<'_, AcceptedFormats>) -> Result<Formats, String> {
    ask(formats, AcceptedFormats::refresh).await
}

/// Run one ask off the window's thread: `spawn_blocking` starts a process, and the
/// page must stay live while the CLI answers.
async fn ask(
    formats: State<'_, AcceptedFormats>,
    which: impl FnOnce(&AcceptedFormats) -> Result<Formats, String> + Send + 'static,
) -> Result<Formats, String> {
    let formats = formats.inner().clone();
    tauri::async_runtime::spawn_blocking(move || which(&formats))
        .await
        .map_err(|error| format!("could not ask the CLI which formats it reads: {error}"))?
}

/// Show a path with the desktop: a folder opens, and a file is selected in the
/// folder it sits in.
///
/// The platform's own tools are run rather than linked to, so one action costs
/// no plugin, no capability and no dependency. The result pane names a single
/// converted document by its file rather than by the folder it was written
/// into, so the action has to reach that file: Finder and Explorer select an
/// item natively, the freedesktop file-manager interface does on Linux, and a
/// desktop without it still gets the file itself rather than nothing. A path
/// that is not there is refused rather than handed to an opener.
#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    if SELFTEST_REVEAL.get().copied().unwrap_or(false) {
        // The self-test checks what the action would reveal; opening the desktop's
        // file manager would put a window on top of the test.
        println!("selftest-reveal path={path}");
        return Ok(());
    }
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(format!("There is nothing at {path}."));
    }
    if target.is_file() && select_file(&target) {
        return Ok(());
    }
    opener(&target)
        .spawn()
        .map_err(|error| format!("Could not show {}: {error}", target.display()))?;
    Ok(())
}

/// Whether the desktop selected `target` in its own file manager.
fn select_file(target: &Path) -> bool {
    if cfg!(target_os = "macos") {
        return std::process::Command::new("open")
            .arg("-R")
            .arg(target)
            .spawn()
            .is_ok();
    }
    if cfg!(target_os = "windows") {
        // Explorer wants the selection in the same argument as the switch.
        return std::process::Command::new("explorer")
            .arg(format!("/select,{}", target.display()))
            .spawn()
            .is_ok();
    }
    // FileManager1 is the freedesktop interface a desktop's file manager
    // registers; `dbus-send` fails when there is no session bus or no file
    // manager, which is when the caller falls back to opening the file.
    let Some(uri) = file_uri(target) else {
        return false;
    };
    std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--type=method_call",
            "--dest=org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{uri}"))
        .arg("string:")
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// A path as the absolute, percent-encoded `file://` URI FileManager1 takes.
fn file_uri(target: &Path) -> Option<String> {
    let absolute = std::fs::canonicalize(target).ok()?;
    let mut uri = String::from("file://");
    for byte in absolute.to_string_lossy().bytes() {
        match byte {
            b'/' | b'-' | b'_' | b'.' | b'~' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    Some(uri)
}

/// The desktop's own opener for a path (`xdg-open`, `open`, `explorer`).
fn opener(target: &Path) -> std::process::Command {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let mut command = std::process::Command::new(program);
    command.arg(target);
    command
}

/// Validate the form and start the CLI in the background.
///
/// Returns as soon as the process is running; everything after that arrives as
/// events, so the window never blocks on a conversion.
#[tauri::command]
fn start_conversion(
    app: AppHandle,
    state: State<'_, ActiveRun>,
    settings: Settings,
) -> Result<Started, String> {
    if let Some(problem) = settings.problem() {
        return Err(problem);
    }

    let mut current = state
        .current
        .lock()
        .map_err(|_| "the conversion state is unusable".to_string())?;

    if let Some(running) = current.as_ref() {
        if !running.job.finished() {
            return Err("A conversion is already running.".to_string());
        }
    }

    let program = locate_cli();
    let args = settings.to_args();
    let (job, stdout, stderr) = Job::spawn(&program, &args)?;
    // A bad-form self-test must not get here. Say so on the process's own
    // output, so the test does not have to infer it from an event that may
    // still be in flight when the page reports back.
    if SELFTEST_BAD_RANGE
        .get()
        .is_some_and(|range| range.is_some())
        || SELFTEST_BAD_BUDGET
            .get()
            .is_some_and(|value| value.is_some())
        || SELFTEST_BAD_OVERLAP
            .get()
            .is_some_and(|value| value.is_some())
    {
        println!("selftest-conversion-started");
    }
    // The comparison is the window's own work on what the run produced, so the
    // reference travels beside the process rather than into its arguments.
    let reference = match settings.reference.trim() {
        "" => None,
        path => Some(PathBuf::from(path)),
    };

    let run = state.next.fetch_add(1, Ordering::SeqCst) + 1;
    let command = format!("{program} {}", args.join(" "));
    // The result pane opens the folder this run writes into. It is resolved now,
    // from the form this run was given, so a later edit cannot move the action;
    // a folder that does not exist yet is created by the run itself.
    let folder = settings.output_folder().display().to_string();
    let started = Started {
        run,
        folder,
        program,
        command,
    };

    // The tokenizer self-test is driven by the page, so this is where its run's
    // command line is printed; `frontend_ready` prints the Rust-driven one.
    if SELFTEST_TOKENIZER
        .get()
        .is_some_and(|tokenizer| tokenizer.is_some())
    {
        println!(
            "selftest-started run={} command={}",
            started.run, started.command
        );
    }

    // Announced before either reader thread exists, so the page has the run set
    // up before its first document can arrive. The id is in every event, which
    // is how the page tells this run from one it has already finished showing.
    let _ = app.emit(STARTED, started.clone());
    let current_measurement = diff::Measurement::from_ocr(settings.ocr.value());
    watch(
        app,
        run,
        job.clone(),
        stdout,
        stderr,
        reference,
        current_measurement,
    );
    // The previous run, if there was one, has already exited; its waiter thread
    // reaps it, so replacing it here only forgets it.
    *current = Some(Running { run, job });

    Ok(started)
}

/// Stop the conversion in flight.
///
/// Answers the run that was stopped, or `None` when there was nothing to stop,
/// so the page can tell whether the run it is showing is the one that ended. It
/// is safe to call when nothing is running.
#[tauri::command]
fn cancel_conversion(state: State<'_, ActiveRun>) -> Option<u64> {
    let current = state.current.lock().ok()?;
    let running = current.as_ref()?;
    running.job.cancel().then_some(running.run)
}

/// Follow a started run: stream both output streams to the page, then report
/// how it ended.
fn watch(
    app: AppHandle,
    run: u64,
    job: Job,
    stdout: ChildStdout,
    stderr: ChildStderr,
    reference: Option<PathBuf>,
    current_measurement: diff::Measurement,
) {
    // One reader thread per stream: a pipe that fills up would block the CLI, so
    // neither can be left until the other is done.
    let out_app = app.clone();
    let out_measurement = current_measurement.clone();
    let out_events = std::thread::spawn(move || {
        let mut sink = |event: RunEvent| {
            // Compared first, then reported: the document's own row is already
            // drawn when its comparison appears under it.
            if let (Some(reference), RunEvent::Progress(progress)) = (&reference, &event) {
                if let Some(report) = compare(progress, reference, &out_measurement) {
                    let _ = out_app.emit(DIFF, DiffLine { run, report });
                }
            }
            emit(&out_app, run, event);
        };
        pump(BufReader::new(stdout), &mut sink)
    });

    let err_app = app.clone();
    let err_events = std::thread::spawn(move || {
        let mut sink = |event: RunEvent| emit(&err_app, run, event);
        pump(BufReader::new(stderr), &mut sink)
    });

    std::thread::spawn(move || {
        let status = job.wait_polling(POLL);
        // Both streams end when the process does, so joining finishes promptly
        // with whatever was still buffered.
        let mut tally = out_events.join().unwrap_or_default();
        tally.add(err_events.join().unwrap_or_default());
        let _ = app.emit(
            FINISHED,
            Finished {
                run,
                outcome: outcome(job.cancelled(), status),
                tally,
            },
        );
    });
}

/// Compare one finished document with its reference.
///
/// `None` when there is nothing to compare: a document that failed wrote no
/// output, and its failure is already on the page.
fn compare(
    progress: &pdfextractor_ui::Progress,
    reference: &Path,
    current_measurement: &diff::Measurement,
) -> Option<diff::Report> {
    if !progress.ok {
        return None;
    }
    let output = progress.output.as_deref()?;
    let mut report = diff::compare_document(&progress.input, Path::new(output), reference);
    // The gate the corpus report and the command line's `--compare` mode run through,
    // with this window's own settings as the measurement: the panel refuses what they
    // would refuse, in the same words, and says which rules held the document. A memory
    // that is there and cannot be read is said on the panel instead of being passed over
    // in silence: a floor that quietly did nothing would be worse than no floor at all.
    match gate::Session::open(reference, current_measurement.clone()) {
        Ok(session) => {
            session.hold(&mut report);
        }
        // A missing reference is the form's own problem, refused before a run starts
        // (`Settings::problem`); what can reach here is a memory that cannot be read.
        Err(problem) => report
            .summary
            .notes
            .push(format!("the memory beside the reference: {problem}")),
    }
    Some(report)
}

/// Send one classified line to the page under the right event name.
fn emit(app: &AppHandle, run: u64, event: RunEvent) {
    // `conversion-started` is emitted by `start_conversion` itself, because it
    // has to precede every other event of a run.
    let _ = match event {
        RunEvent::Plan { total, inputs } => app.emit(PLAN, PlanLine { run, total, inputs }),
        RunEvent::Progress(progress) => app.emit(PROGRESS, ProgressLine { run, progress }),
        RunEvent::Log(line) => app.emit(LOG, LogLine { run, line }),
    };
}

/// Called by the frontend once it is alive. With `--selftest` the app proves
/// the window and IPC work, and optionally converts a path through the very
/// same path the window uses — events, page and all — so the whole feature can
/// be checked without a human looking at the screen.
#[tauri::command]
fn frontend_ready(app: tauri::AppHandle) {
    use std::io::Write;
    println!("webview-ready");
    let _ = std::io::stdout().flush();

    // `None` means the app was launched normally: the window stays open.
    // `Some("")` is `--selftest` on its own; `Some(path)` also converts.
    let Some(selftest_input) = SELFTEST.get().cloned().flatten() else {
        return;
    };
    let tokenizer = SELFTEST_TOKENIZER.get().cloned().flatten();
    let tokenizer_encoding = SELFTEST_TOKENIZER_ENCODING.get().cloned().flatten();
    let clear_tokenizer = SELFTEST_CLEAR_TOKENIZER.get().copied().unwrap_or(false);
    let reference = SELFTEST_REFERENCE
        .get()
        .cloned()
        .flatten()
        .unwrap_or_default();
    // The page drives a tokenizer run itself, so it needs the same scratch output
    // a Rust-driven self-test would use rather than writing beside the corpus.
    let output = if selftest_input.is_empty() {
        String::new()
    } else {
        Settings::selftest(&selftest_input, &reference).output
    };

    if selftest_input.is_empty() && tokenizer.is_none() {
        app.exit(0);
        return;
    }

    // Told before the run starts, so the page knows to report what it rendered;
    // `--selftest-chunks` also tells it which row to tick, and
    // `--selftest-tokenizer` which file's badge to show.
    let chunks = SELFTEST_CHUNKS.get().copied().unwrap_or(false);
    let bad_range = SELFTEST_BAD_RANGE.get().cloned().flatten();
    let bad_budget = SELFTEST_BAD_BUDGET.get().cloned().flatten();
    let bad_overlap = SELFTEST_BAD_OVERLAP.get().cloned().flatten();
    let bad_encoding = SELFTEST_BAD_ENCODING.get().cloned().flatten();
    let reveal = SELFTEST_REVEAL.get().copied().unwrap_or(false);
    let _ = app.emit(
        SELFTEST_ON,
        serde_json::json!({
            "chunks": chunks,
            "badRange": bad_range.clone(),
            "badBudget": bad_budget.clone(),
            "badOverlap": bad_overlap.clone(),
            "badEncoding": bad_encoding.clone(),
            "reveal": reveal,
            "input": selftest_input.clone(),
            "output": output,
            "tokenizer": tokenizer.clone(),
            "tokenizerEncoding": tokenizer_encoding.clone(),
            "clearTokenizer": clear_tokenizer,
        }),
    );

    // The badge check is driven by the page and needs no conversion: it fills
    // the tokenizer field, asks the real format command, reports the label and
    // exits. Nothing is spawned behind it.
    if tokenizer.is_some() {
        return;
    }

    // This case is driven by the page: it fills the box and presses the same
    // Convert action a person does, so the refusal comes back through
    // `start_conversion` and the button's own error path is what reports it.
    if bad_range.is_some() {
        return;
    }

    // A malformed count is refused either by the page (a fraction or a typo) or
    // by `start_conversion` (a zero budget, an overlap as large as the budget);
    // an encoding with no tokenizer is refused by the form's own `problem`. In
    // every case the page presses Convert and reports the refusal, so nothing is
    // started here.
    if bad_budget.is_some() || bad_overlap.is_some() || bad_encoding.is_some() {
        return;
    }

    // `Settings::selftest` rather than the form as typed: a smoke test is pointed at
    // the corpus itself, so it writes what it converts into a temporary directory of
    // its own instead of beside the documents and the committed references it read.
    // The chunking row is the page's, not the form's, so the one the page ticks and
    // the one the run uses are made here to be the same reading.
    let mut settings = Settings::selftest(&selftest_input, &reference);
    if chunks {
        settings.to.push(OutputFormat::Chunks);
        settings.chunks_type = ChunkType::Hybrid;
    }
    match start_conversion(app.clone(), app.state::<ActiveRun>(), settings) {
        Ok(started) => println!(
            "selftest-started run={} command={}",
            started.run, started.command
        ),
        Err(error) => {
            println!("selftest-convert success=false error={error}");
            app.exit(0);
            return;
        }
    }

    if let Some(delay) = SELFTEST_STOP.get().cloned().flatten() {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(delay));
            let stopped = cancel_conversion(app.state::<ActiveRun>());
            println!("selftest-stop after={delay}ms stopped={stopped:?}");
        });
    }
}

/// Used only by `--selftest`: the page hands back what it drew, which is the
/// only way to prove the events reached the window and were understood.
#[tauri::command]
fn selftest_rendered(app: AppHandle, summary: String) {
    use std::io::Write;
    println!("selftest-rendered {summary}");
    let _ = std::io::stdout().flush();
    app.exit(0);
}

/// Parse `--selftest [INPUT]`. `Some("")` means the window-only check.
fn selftest_mode(args: &[String]) -> Option<String> {
    let position = args.iter().position(|arg| arg == "--selftest")?;
    Some(
        args.get(position + 1)
            .filter(|value| !value.starts_with("--"))
            .cloned()
            .unwrap_or_default(),
    )
}

/// Parse `--selftest-stop MILLIS`.
fn selftest_stop_mode(args: &[String]) -> Option<u64> {
    let position = args.iter().position(|arg| arg == "--selftest-stop")?;
    args.get(position + 1)?.parse().ok()
}

/// Parse `--selftest-chunks`: tick the chunking row for a self-test run.
fn selftest_chunks(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--selftest-chunks")
}

/// Parse `--selftest-tokenizer PATH`, the file whose badge a smoke test checks.
fn selftest_tokenizer(args: &[String]) -> Option<String> {
    let position = args.iter().position(|arg| arg == "--selftest-tokenizer")?;
    args.get(position + 1)
        .filter(|value| !value.starts_with("--"))
        .cloned()
}

/// Parse `--selftest-tokenizer-encoding VALUE`, the tiktoken family a smoke test
/// chooses for that file.
fn selftest_tokenizer_encoding(args: &[String]) -> Option<String> {
    selftest_value(args, "--selftest-tokenizer-encoding")
}

/// Parse `--selftest-clear-tokenizer`: clear the badge check's field after its
/// label is painted.
fn selftest_clear_tokenizer(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--selftest-clear-tokenizer")
}

/// Parse `--selftest-bad-range RANGE`, the malformed range a smoke test types.
fn selftest_bad_range(args: &[String]) -> Option<String> {
    selftest_value(args, "--selftest-bad-range")
}

/// Parse `--selftest-bad-budget VALUE`, the malformed budget a smoke test types.
fn selftest_bad_budget(args: &[String]) -> Option<String> {
    selftest_value(args, "--selftest-bad-budget")
}

/// Parse `--selftest-bad-overlap VALUE`, the malformed overlap a smoke test types.
fn selftest_bad_overlap(args: &[String]) -> Option<String> {
    selftest_value(args, "--selftest-bad-overlap")
}

/// Parse `--selftest-bad-encoding VALUE`, the encoding a smoke test chooses
/// without a tokenizer file.
fn selftest_bad_encoding(args: &[String]) -> Option<String> {
    selftest_value(args, "--selftest-bad-encoding")
}

/// Parse `--selftest-reveal`: press the result pane's action after a run.
fn selftest_reveal(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--selftest-reveal")
}

/// The value after `flag`, when the flag is present and a value follows it.
fn selftest_value(args: &[String], flag: &str) -> Option<String> {
    let position = args.iter().position(|arg| arg == flag)?;
    args.get(position + 1)
        .filter(|value| !value.starts_with("--"))
        .cloned()
}

/// Parse `--selftest-ref PATH`, the Compare mode's reference for a smoke test.
fn selftest_reference(args: &[String]) -> Option<String> {
    let position = args.iter().position(|arg| arg == "--selftest-ref")?;
    args.get(position + 1)
        .filter(|value| !value.starts_with("--"))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "pdfextractor-ui - graphical front end for pdfextractor-cli\n\n\
             Usage: pdfextractor-ui [--selftest [INPUT]] [--selftest-stop MILLIS]\n\
                                   [--selftest-ref PATH] [--selftest-chunks]\n\
                                   [--selftest-tokenizer PATH]\n\
                                   [--selftest-tokenizer-encoding VALUE]\n\
                                   [--selftest-clear-tokenizer]\n\
                                   [--selftest-bad-range RANGE]\n\
                                   [--selftest-bad-budget VALUE]\n\
                                   [--selftest-bad-overlap VALUE]\n\
                                   [--selftest-bad-encoding VALUE]\n\
                                   [--selftest-reveal] [-h]\n\n\
             With no options the window opens and waits for you. A conversion\n\
             runs in the background and can be stopped at any time, and the\n\
             Compare mode shows each document beside a Docling reference.\n\
             --selftest mounts the window, and if INPUT is given converts it\n\
             through the CLI, then exits; --selftest-ref turns Compare on for\n\
             that run; --selftest-chunks ticks Retrieval chunks and the hybrid\n\
             chunker before it, so that row is checked too;\n\
             --selftest-tokenizer names a tokenizer file and reports the badge\n\
             the window renders for it, without starting a run;\n\
             --selftest-tokenizer-encoding chooses the tiktoken family for that\n\
             file and, with an input and --selftest-chunks, runs the conversion;\n\
             --selftest-clear-tokenizer clears the field after painting the badge,\n\
             so the test checks the label and tooltip are removed;\n\
             --selftest-bad-range fills an unreadable page range and presses\n\
             Convert, so the page's refusal is checked instead of a run;\n\
             --selftest-bad-budget VALUE and --selftest-bad-overlap VALUE check a\n\
             malformed chunk count the same way; --selftest-bad-encoding VALUE\n\
             chooses an encoding with no tokenizer and checks that refusal;\n\
             --selftest-reveal presses the\n\
             result pane's action after a run. What such a run\n\
             converts is written into a temporary\n\
             directory of its own — named on the command line it prints — so\n\
             smoke-testing the corpus never writes beside it. Set\n\
             PDFEXTRACTOR_CLI to choose the command line binary; otherwise it\n\
             is looked for beside this one."
        );
        return;
    }
    let _ = SELFTEST.set(selftest_mode(&args));
    let _ = SELFTEST_STOP.set(selftest_stop_mode(&args));
    let _ = SELFTEST_REFERENCE.set(selftest_reference(&args));
    let _ = SELFTEST_CHUNKS.set(selftest_chunks(&args));
    let _ = SELFTEST_TOKENIZER.set(selftest_tokenizer(&args));
    let _ = SELFTEST_TOKENIZER_ENCODING.set(selftest_tokenizer_encoding(&args));
    let _ = SELFTEST_CLEAR_TOKENIZER.set(selftest_clear_tokenizer(&args));
    let _ = SELFTEST_BAD_RANGE.set(selftest_bad_range(&args));
    let _ = SELFTEST_BAD_BUDGET.set(selftest_bad_budget(&args));
    let _ = SELFTEST_BAD_OVERLAP.set(selftest_bad_overlap(&args));
    let _ = SELFTEST_BAD_ENCODING.set(selftest_bad_encoding(&args));
    let _ = SELFTEST_REVEAL.set(selftest_reveal(&args));

    tauri::Builder::default()
        .manage(ActiveRun::default())
        .setup(|app| {
            // An installed window must convert without a separate install: a
            // bundle carries the command line in its resource directory, and
            // naming it here means every conversion runs the CLI the bundle was
            // built with. Set by hand, `PDFEXTRACTOR_CLI` still wins —
            // `locate_cli` reads it first — and without a bundle this finds
            // nothing and changes nothing.
            if std::env::var_os("PDFEXTRACTOR_CLI").is_none() {
                if let Ok(resources) = app.path().resource_dir() {
                    let name = if cfg!(windows) {
                        "pdfextractor-cli.exe"
                    } else {
                        "pdfextractor-cli"
                    };
                    let bundled = resources.join(name);
                    if bundled.exists() {
                        std::env::set_var("PDFEXTRACTOR_CLI", &bundled);
                    }
                }
            }
            // Asked for when the page loads, and re-askable from the page afterwards.
            // Constructed here, after the bundled binary has been named, so the
            // process it looks for is the one a run would start. It follows the
            // environment on every refresh, so repointing `PDFEXTRACTOR_CLI` takes
            // effect without reopening the window.
            app.manage(AcceptedFormats::from_environment());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pick_path,
            input_formats,
            cli_program,
            tokenizer_format,
            refresh_input_formats,
            start_conversion,
            reveal_path,
            cancel_conversion,
            frontend_ready,
            selftest_rendered
        ])
        .on_window_event(|window, event| {
            // A conversion would otherwise outlive the window that started it.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if let Some(state) = window.app_handle().try_state::<ActiveRun>() {
                    if let Ok(current) = state.current.lock() {
                        if let Some(running) = current.as_ref() {
                            running.job.cancel();
                        }
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running the PDF Extractor UI");
}
