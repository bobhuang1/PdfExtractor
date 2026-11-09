//! A process test for the window's own refusal path.
//!
//! This starts the real UI binary, has its page fill a malformed page range and
//! press Convert, and checks what the page reports back. `/bin/false` is the CLI
//! so the boot-time format question cannot convert anything, and the page's
//! `started=0` is the conversion boundary: a run that reached `Job::spawn` would
//! have announced itself and set it to 1.
//!
//! A window needs a display. On a machine without one the test uses `xvfb-run`
//! when it is installed, and otherwise skips loudly, as the other suites do when
//! their fixture tool is absent.

#![cfg(all(target_os = "linux", feature = "desktop"))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// An existing executable that exits immediately: the CLI the page asks for
/// formats at boot, and the one a conversion must never reach.
fn false_program() -> &'static str {
    ["/bin/false", "/usr/bin/false"]
        .into_iter()
        .find(|path| Path::new(path).exists())
        .expect("a false executable")
}

/// `xvfb-run` from `PATH`, for a headless machine.
fn xvfb_run() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join("xvfb-run"))
        .find(|path| path.is_file())
}

/// The UI binary, wrapped in `xvfb-run` when this machine has no display.
///
/// `None` means the test cannot run here; the caller skips after the message.
fn ui_command() -> Option<Command> {
    let ui = env!("CARGO_BIN_EXE_pdfextractor-ui");
    if std::env::var_os("DISPLAY").is_some() {
        return Some(Command::new(ui));
    }
    let Some(xvfb) = xvfb_run() else {
        eprintln!("skipped: no DISPLAY and no xvfb-run");
        return None;
    };
    let mut command = Command::new(xvfb);
    command.arg("-a").arg(ui);
    Some(command)
}

fn run_with_timeout(mut command: Command, timeout: Duration) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("start the UI process");
    let started = Instant::now();
    loop {
        if child.try_wait().expect("poll the UI process").is_some() {
            return child.wait_with_output().expect("collect the UI's output");
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child.wait_with_output().expect("collect the UI's output");
            panic!(
                "the UI did not exit within {timeout:?}\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_bad_page_range_shows_its_message_and_leaves_convert_enabled() {
    let root = repository_root();
    let input = root.join("test-data/brief.docx");
    assert!(input.exists(), "missing fixture {}", input.display());

    let Some(mut command) = ui_command() else {
        return;
    };
    command
        .current_dir(&root)
        .arg("--selftest")
        .arg(&input)
        .arg("--selftest-bad-range")
        .arg("3-1")
        .env("PDFEXTRACTOR_CLI", false_program());

    let output = run_with_timeout(command, Duration::from_secs(60));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "the UI exited with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("webview-ready"),
        "the window never loaded:\n{stdout}"
    );
    assert!(
        !stdout.contains("selftest-started"),
        "a conversion was started instead of refusing the form:\n{stdout}"
    );
    assert!(
        !stdout.contains("selftest-conversion-started"),
        "a conversion process was started instead of refusing the form:\n{stdout}"
    );

    let report = stdout
        .lines()
        .find(|line| line.starts_with("selftest-rendered "))
        .unwrap_or_else(|| {
            panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    assert!(
        report.contains("run=none"),
        "a run was on screen:\n{report}"
    );
    assert!(
        report.contains("title=\"Cannot convert\""),
        "the result pane did not show the refusal:\n{report}"
    );
    assert!(
        report.contains("verdict=\"Page range `3-1` runs backwards"),
        "the page-range message was not shown:\n{report}"
    );
    assert!(
        report.contains("convert=enabled"),
        "Convert was left disabled after the refusal:\n{report}"
    );
    assert!(
        report.contains("run-visible=hidden"),
        "the run pane was left on screen after the refusal:\n{report}"
    );
    assert!(
        report.contains("started=0"),
        "a conversion process was started:\n{report}"
    );
}

#[test]
fn the_chunking_row_is_hidden_before_retrieval_chunks_is_ticked() {
    let root = repository_root();
    let input = root.join("test-data/brief.docx");
    assert!(input.exists(), "missing fixture {}", input.display());

    let Some(mut command) = ui_command() else {
        return;
    };
    command
        .current_dir(&root)
        .arg("--selftest")
        .arg(&input)
        .arg("--selftest-chunks")
        .env("PDFEXTRACTOR_CLI", false_program());

    let output = run_with_timeout(command, Duration::from_secs(60));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "the UI exited with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("webview-ready"),
        "the window never loaded:\n{stdout}"
    );

    let report = stdout
        .lines()
        .find(|line| line.starts_with("selftest-rendered "))
        .unwrap_or_else(|| {
            panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    assert!(
        report.contains("chunk-controls-before=hidden"),
        "the chunking row was not hidden before Retrieval chunks was ticked:\n{report}"
    );
    assert!(
        report.contains("chunk-controls=shown"),
        "the chunking row did not appear after Retrieval chunks was ticked:\n{report}"
    );
    assert!(
        report.contains("chunk-controls-before=hidden chunk-controls=shown"),
        "the chunking row's before/after states are not the expected transition:\n{report}"
    );
    assert!(
        report.contains("chunk-type=hybrid"),
        "the self-test did not choose the hybrid chunker:\n{report}"
    );
    assert!(
        report.contains("chunk-fields=on|on|on"),
        "the hybrid chunk fields were not enabled:\n{report}"
    );
}

#[test]
fn the_tokenizer_badge_names_each_fixture_and_its_size() {
    let root = repository_root();

    for (fixture, expected) in [
        ("tokenizer-mini.json", "Hugging Face · 10 tokens"),
        ("tokenizer-mini.model", "SentencePiece · 13 tokens"),
        ("tokenizer-mini.tiktoken", "tiktoken · cl100k_base"),
    ] {
        let tokenizer = root.join("test-data").join(fixture);
        assert!(
            tokenizer.exists(),
            "missing fixture {}",
            tokenizer.display()
        );

        let Some(mut command) = ui_command() else {
            return;
        };
        command
            .current_dir(&root)
            .arg("--selftest")
            .arg("--selftest-chunks")
            .arg("--selftest-tokenizer")
            .arg(&tokenizer)
            .env("PDFEXTRACTOR_CLI", false_program());

        let output = run_with_timeout(command, Duration::from_secs(60));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "the UI exited with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status
        );
        assert!(
            stdout.contains("webview-ready"),
            "the window never loaded:\n{stdout}"
        );
        assert!(
            !stdout.contains("selftest-started"),
            "the badge check started a conversion:\n{stdout}"
        );

        let report = stdout
            .lines()
            .find(|line| line.starts_with("selftest-rendered "))
            .unwrap_or_else(|| {
                panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
            });
        assert!(
            report.contains(&format!("tokenizer=\"{}\"", tokenizer.display())),
            "the report did not name the chosen tokenizer:\n{report}"
        );
        assert!(
            report.contains(&format!("tokenizer-format=\"{expected}\"")),
            "the badge did not name {fixture}'s format and size:\n{report}"
        );
    }
}
