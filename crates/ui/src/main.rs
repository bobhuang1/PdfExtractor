//! The Tauri shell: a window around the logic in `lib.rs`.
//!
//! Exposed to the frontend: picking a path with the native chooser, starting a
//! conversion, stopping the one in flight, and a readiness ping the frontend
//! sends on load (which also lets `--selftest` prove the webview mounted and
//! the IPC round-trips).
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
    diff, gate, locate_cli, outcome, pump, DiffLine, Finished, Job, LogLine, PlanLine,
    ProgressLine, RunEvent, Settings, Started,
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
/// Sent to the page only by `--selftest`, which asks it to report what it drew.
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

/// Native file/folder chooser. `kind` is `"file"` or `"folder"`.
#[tauri::command]
fn pick_path(kind: String) -> Option<String> {
    let dialog = rfd::FileDialog::new();
    let picked = if kind == "folder" {
        dialog.pick_folder()
    } else {
        dialog.pick_file()
    };
    picked.map(|path| path.to_string_lossy().into_owned())
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
    // The comparison is the window's own work on what the run produced, so the
    // reference travels beside the process rather than into its arguments.
    let reference = match settings.reference.trim() {
        "" => None,
        path => Some(PathBuf::from(path)),
    };

    let run = state.next.fetch_add(1, Ordering::SeqCst) + 1;
    let started = Started {
        run,
        command: format!("{program} {}", args.join(" ")),
    };

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

    if selftest_input.is_empty() {
        app.exit(0);
        return;
    }

    // Told before the run starts, so the page knows to report what it rendered.
    let _ = app.emit(SELFTEST_ON, ());

    // `Settings::selftest` rather than the form as typed: a smoke test is pointed at
    // the corpus itself, so it writes what it converts into a temporary directory of
    // its own instead of beside the documents and the committed references it read.
    let reference = SELFTEST_REFERENCE
        .get()
        .cloned()
        .flatten()
        .unwrap_or_default();
    let settings = Settings::selftest(&selftest_input, &reference);
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
                                   [--selftest-ref PATH] [-h]\n\n\
             With no options the window opens and waits for you. A conversion\n\
             runs in the background and can be stopped at any time, and the\n\
             Compare mode shows each document beside a Docling reference.\n\
             --selftest mounts the window, and if INPUT is given converts it\n\
             through the CLI, then exits; --selftest-ref turns Compare on for\n\
             that run. What such a run converts is written into a temporary\n\
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pick_path,
            start_conversion,
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
