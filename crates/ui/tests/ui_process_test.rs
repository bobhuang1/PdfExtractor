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

/// Drive the window's refusal path with one malformed chunk count: tick
/// Retrieval chunks, choose hybrid, type the value and press Convert. Returns
/// the page's rendered report, or `None` when this machine cannot show a window.
fn chunk_refusal_report(root: &Path, flag: &str, value: &str) -> Option<String> {
    let input = root.join("test-data/brief.docx");
    assert!(input.exists(), "missing fixture {}", input.display());

    let mut command = ui_command()?;
    command
        .current_dir(root)
        .arg("--selftest")
        .arg(&input)
        .arg(flag)
        .arg(value)
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
    Some(
        stdout
            .lines()
            .find(|line| line.starts_with("selftest-rendered "))
            .unwrap_or_else(|| {
                panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
            })
            .to_string(),
    )
}

/// The refusal a chunk-count check draws: the typed value is in its box, the
/// message is in the result pane, and nothing was run.
fn assert_chunk_refusal(report: &str, field: &str, message: &str) {
    assert!(
        report.contains(field),
        "the report does not show the typed value {field}:\n{report}"
    );
    assert!(
        report.contains("run=none"),
        "a run was on screen:\n{report}"
    );
    assert!(
        report.contains("title=\"Cannot convert\""),
        "the result pane did not show the refusal:\n{report}"
    );
    assert!(
        report.contains(&format!("verdict=\"{message}\"")),
        "the refusal message was not {message:?}:\n{report}"
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

/// A chunk budget that is not a whole number is refused by the page before the
/// command is asked, and a budget of zero is refused by the command before a
/// process exists: either way the window shows the refusal and starts nothing.
#[test]
fn a_malformed_chunk_budget_is_refused_before_a_conversion() {
    let root = repository_root();
    for (value, message) in [
        ("1.5", "The chunk token budget must be a whole number."),
        ("0", "The chunk token budget must be at least 1."),
    ] {
        let Some(report) = chunk_refusal_report(&root, "--selftest-bad-budget", value) else {
            return;
        };
        assert_chunk_refusal(&report, &format!("chunk-budget={value}"), message);
    }
}

/// The same for the overlap: a fraction is refused by the page, and an overlap
/// as large as the budget is refused by the command. Neither starts a process.
#[test]
fn a_malformed_chunk_overlap_is_refused_before_a_conversion() {
    let root = repository_root();
    for (value, message) in [
        ("1.5", "The chunk overlap must be a whole number."),
        (
            "600",
            "The chunk overlap must be smaller than the token budget.",
        ),
    ] {
        let Some(report) = chunk_refusal_report(&root, "--selftest-bad-overlap", value) else {
            return;
        };
        assert_chunk_refusal(&report, &format!("chunk-overlap={value}"), message);
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

/// A single-file run writes one reading, and the result pane's action reaches
/// that file rather than only the folder it sits in. `--selftest-reveal` presses
/// the action after the run; the command prints the path it was given instead of
/// opening a file manager, so the test checks the window's choice without a
/// desktop window appearing over it.
#[test]
fn a_single_written_output_is_revealed_as_the_file() {
    let root = repository_root();
    let input = root.join("test-data/brief.docx");
    assert!(input.exists(), "missing fixture {}", input.display());
    let cli = root.join("target/debug/pdfextractor-cli");
    if !cli.exists() {
        eprintln!("skipped: pdfextractor-cli is not built");
        return;
    }

    let Some(mut command) = ui_command() else {
        return;
    };
    command
        .current_dir(&root)
        .arg("--selftest")
        .arg(&input)
        .arg("--selftest-reveal")
        .env("PDFEXTRACTOR_CLI", &cli);

    let output = run_with_timeout(command, Duration::from_secs(120));
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

    let revealed = stdout
        .lines()
        .find(|line| line.starts_with("selftest-reveal path="))
        .unwrap_or_else(|| {
            panic!(
                "the page never pressed the reveal action:\nstdout:\n{stdout}\nstderr:\n{stderr}"
            )
        });
    let path = revealed
        .strip_prefix("selftest-reveal path=")
        .expect("the reveal line names its path");
    let target = Path::new(path);
    assert_eq!(
        target.file_name().and_then(|name| name.to_str()),
        Some("brief.json"),
        "the action revealed {path}, not the file the run wrote"
    );
    assert!(
        target.is_file(),
        "the revealed path is not the written output file: {path}"
    );

    let report = stdout
        .lines()
        .find(|line| line.starts_with("selftest-rendered "))
        .unwrap_or_else(|| {
            panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    assert!(
        report.contains("title=\"Finished\""),
        "the run did not finish:\n{report}"
    );
    assert!(
        report.contains(&format!("folder=\"{path}\"")),
        "the result pane did not name the written file:\n{report}"
    );
    assert!(
        report.contains("folder-action=1"),
        "the result pane offered no action for the written file:\n{report}"
    );
}

/// A folder run writes one reading per document, so there is no single file for
/// the result pane's action to mean. `--selftest-reveal` presses it after the
/// run; the path it asks to reveal must be the output folder, holding both
/// readings, rather than either one of them.
#[test]
fn a_multi_file_run_is_revealed_as_its_output_folder() {
    let root = repository_root();
    let fixture = root.join("test-data/brief.docx");
    assert!(fixture.exists(), "missing fixture {}", fixture.display());
    let cli = root.join("target/debug/pdfextractor-cli");
    if !cli.exists() {
        eprintln!("skipped: pdfextractor-cli is not built");
        return;
    }

    let input_dir =
        std::env::temp_dir().join(format!("pdfextractor-multi-reveal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&input_dir);
    std::fs::create_dir_all(&input_dir).expect("create the multi-file input directory");
    for name in ["first.docx", "second.docx"] {
        std::fs::copy(&fixture, input_dir.join(name)).expect("copy the multi-file fixture");
    }

    let Some(mut command) = ui_command() else {
        return;
    };
    command
        .current_dir(&root)
        .arg("--selftest")
        .arg(&input_dir)
        .arg("--selftest-reveal")
        .env("PDFEXTRACTOR_CLI", &cli);

    let output = run_with_timeout(command, Duration::from_secs(120));
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

    let revealed = stdout
        .lines()
        .find(|line| line.starts_with("selftest-reveal path="))
        .unwrap_or_else(|| {
            panic!(
                "the page never pressed the reveal action:\nstdout:\n{stdout}\nstderr:\n{stderr}"
            )
        });
    let path = revealed
        .strip_prefix("selftest-reveal path=")
        .expect("the reveal line names its path");
    let target = Path::new(path);
    assert!(
        target.is_dir(),
        "the action revealed {path}, not the folder the folder run wrote into"
    );
    assert!(
        target
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| { name.starts_with("pdfextractor-selftest-") }),
        "the action revealed {path}, not the run's scratch output folder"
    );
    for name in ["first.json", "second.json"] {
        assert!(
            target.join(name).is_file(),
            "the revealed folder does not hold {name}: {path}"
        );
    }

    let report = stdout
        .lines()
        .find(|line| line.starts_with("selftest-rendered "))
        .unwrap_or_else(|| {
            panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    assert!(
        report.contains("title=\"Finished\""),
        "the run did not finish:\n{report}"
    );
    assert!(
        report.contains(&format!("folder=\"{path}\"")),
        "the result pane did not name the output folder:\n{report}"
    );
    assert!(
        report.contains("folder-action=1"),
        "the result pane offered no action for the output folder:\n{report}"
    );

    let _ = std::fs::remove_dir_all(&input_dir);
    let _ = std::fs::remove_dir_all(target);
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
        report.contains("chunk-fields=on|on|on|on"),
        "the hybrid chunk fields were not enabled:\n{report}"
    );
}

/// Run the real window's badge check for one tokenizer path and return the
/// report the page rendered. The caller has already established that a window
/// is available.
fn tokenizer_badge_report(root: &Path, tokenizer: &Path) -> String {
    let mut command = ui_command().expect("the window check was available above");
    command
        .current_dir(root)
        .arg("--selftest")
        .arg("--selftest-chunks")
        .arg("--selftest-tokenizer")
        .arg(tokenizer)
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

    stdout
        .lines()
        .find(|line| line.starts_with("selftest-rendered "))
        .unwrap_or_else(|| {
            panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        })
        .to_string()
}

#[test]
fn the_tokenizer_badge_names_each_fixture_and_its_size() {
    let root = repository_root();
    if ui_command().is_none() {
        return;
    }

    let scratch = std::env::temp_dir().join(format!(
        "pdfextractor-tokenizer-badge-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create the tokenizer badge scratch directory");
    let tiktoken = root.join("test-data/tokenizer-mini.tiktoken");
    assert!(tiktoken.exists(), "missing fixture {}", tiktoken.display());

    let mut cases: Vec<(PathBuf, &str)> = vec![
        (
            root.join("test-data/tokenizer-mini.json"),
            "Hugging Face · 10 tokens",
        ),
        (
            root.join("test-data/tokenizer-mini.model"),
            "SentencePiece · 13 tokens",
        ),
        (tiktoken.clone(), "tiktoken · cl100k_base · 257 tokens"),
    ];
    for (name, expected) in [
        (
            "tokenizer-mini-o200k.tiktoken",
            "tiktoken · o200k_base · 257 tokens",
        ),
        (
            "tokenizer-mini-p50k.tiktoken",
            "tiktoken · p50k_base · 257 tokens",
        ),
    ] {
        let path = scratch.join(name);
        std::fs::copy(&tiktoken, &path).unwrap_or_else(|error| {
            panic!("copy {} to {}: {error}", tiktoken.display(), path.display())
        });
        cases.push((path, expected));
    }

    for (tokenizer, expected) in cases {
        assert!(
            tokenizer.exists(),
            "missing fixture {}",
            tokenizer.display()
        );

        let report = tokenizer_badge_report(&root, &tokenizer);
        assert!(
            report.contains(&format!("tokenizer=\"{}\"", tokenizer.display())),
            "the report did not name the chosen tokenizer:\n{report}"
        );
        assert!(
            report.contains(&format!("tokenizer-format=\"{expected}\"")),
            "the badge did not name {}'s format and size:\n{report}",
            tokenizer.display()
        );
        assert!(
            report.contains(&format!(
                "tokenizer-format-title=\"Detected from {}\"",
                tokenizer.display()
            )),
            "the badge tooltip did not name {} as its source:\n{report}",
            tokenizer.display()
        );
        assert!(
            report.contains("tokenizer-format-bad=0"),
            "the badge marked a valid tokenizer as bad:\n{report}"
        );
    }

    let _ = std::fs::remove_dir_all(&scratch);
}

/// A malformed tokenizer file is rejected beside the field, with the loader's
/// message and the badge's error state, without starting a conversion.
#[test]
fn the_tokenizer_badge_shows_a_malformed_file_error() {
    let root = repository_root();
    if ui_command().is_none() {
        return;
    }

    let scratch = std::env::temp_dir().join(format!(
        "pdfextractor-tokenizer-badge-error-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create the tokenizer badge error directory");
    let bad = scratch.join("not-a-tokenizer.tiktoken");
    std::fs::write(&bad, b"not a rank\n").expect("write a malformed tokenizer");

    let report = tokenizer_badge_report(&root, &bad);
    assert!(
        report.contains("tokenizer-format=\"Cannot use this tokenizer:"),
        "the badge did not show the loader's error beside the field:\n{report}"
    );
    assert!(
        report.contains("expected one base64 token and one rank"),
        "the badge did not carry the loader's message:\n{report}"
    );
    assert!(
        report.contains("tokenizer-format-title=\"Tokenizer file cannot be loaded:")
            && report.contains("expected one base64 token and one rank"),
        "the badge tooltip did not carry the loader's error:\n{report}"
    );
    assert!(
        report.contains("tokenizer-format-bad=1"),
        "the badge did not mark the field's error state:\n{report}"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

/// Clearing the tokenizer field removes everything the badge said about the
/// previous file: no label, no tooltip, and no error styling. The check first
/// paints a valid badge, clears the box, and reports the field after the second
/// ask.
#[test]
fn clearing_the_tokenizer_field_removes_the_badge_label_and_tooltip() {
    let root = repository_root();
    if ui_command().is_none() {
        return;
    }
    let tokenizer = root.join("test-data/tokenizer-mini.tiktoken");
    assert!(
        tokenizer.exists(),
        "missing fixture {}",
        tokenizer.display()
    );

    let mut command = ui_command().expect("the window check was available above");
    command
        .current_dir(&root)
        .arg("--selftest")
        .arg("--selftest-tokenizer")
        .arg(&tokenizer)
        .arg("--selftest-clear-tokenizer")
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
        "clearing the tokenizer started a conversion:\n{stdout}"
    );

    let report = stdout
        .lines()
        .find(|line| line.starts_with("selftest-rendered "))
        .unwrap_or_else(|| {
            panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    assert!(
        report.contains("tokenizer=\"\""),
        "the tokenizer field was not cleared:\n{report}"
    );
    assert!(
        report.contains("tokenizer-format=\"\"") && report.contains("tokenizer-format-title=\"\""),
        "clearing the tokenizer left a badge label or tooltip:\n{report}"
    );
    assert!(
        report.contains("tokenizer-format-bad=0"),
        "clearing the tokenizer left the badge in its error state:\n{report}"
    );
}

/// A malformed tokenizer is refused in the result pane before a conversion
/// process exists. `--selftest-tokenizer` with an input and the chunk row drives
/// Convert, so the command's form validation loads the tokenizer and returns the
/// error the page draws as `Cannot convert`. The CLI is `/bin/false`, and the
/// absent started line proves it was never reached.
#[test]
fn a_malformed_tokenizer_is_refused_in_the_result_pane_before_conversion() {
    let root = repository_root();
    if ui_command().is_none() {
        return;
    }
    let input = root.join("test-data/brief.docx");
    assert!(input.exists(), "missing fixture {}", input.display());

    let scratch = std::env::temp_dir().join(format!(
        "pdfextractor-tokenizer-refusal-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create the tokenizer refusal directory");
    let bad = scratch.join("not-a-tokenizer.json");
    std::fs::write(&bad, b"this is not JSON").expect("write a malformed tokenizer");

    let mut command = ui_command().expect("the window check was available above");
    command
        .current_dir(&root)
        .arg("--selftest")
        .arg(&input)
        .arg("--selftest-chunks")
        .arg("--selftest-tokenizer")
        .arg(&bad)
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
        !stdout.contains("selftest-started") && !stdout.contains("selftest-conversion-started"),
        "a conversion process was started for a malformed tokenizer:\n{stdout}"
    );

    let report = stdout
        .lines()
        .find(|line| line.starts_with("selftest-rendered "))
        .unwrap_or_else(|| {
            panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    assert!(
        report.contains("title=\"Cannot convert\""),
        "the result pane did not show the refusal:\n{report}"
    );
    assert!(
        report.contains("Tokenizer file cannot be loaded:")
            && report.contains("not-a-tokenizer.json"),
        "the result pane did not carry the loader's error:\n{report}"
    );
    assert!(
        report.contains("started=0") && report.contains("run-visible=hidden"),
        "a run was drawn despite the refusal:\n{report}"
    );
    assert!(
        report.contains("convert=enabled"),
        "Convert was left disabled after the refusal:\n{report}"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

/// A file whose name and bytes name no tokenizer format is labelled for what it
/// is — `Unknown format` — and marked bad, rather than borrowing the loader's
/// error for a format the file never claimed. The badge check starts nothing.
#[test]
fn the_tokenizer_badge_calls_an_unrecognized_file_unknown_format() {
    let root = repository_root();
    if ui_command().is_none() {
        return;
    }

    let scratch = std::env::temp_dir().join(format!(
        "pdfextractor-tokenizer-badge-unknown-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create the tokenizer badge scratch directory");
    let unknown = scratch.join("not-a-tokenizer.bin");
    std::fs::write(&unknown, b"neither JSON nor a model nor a rank list\n")
        .expect("write an unrecognized tokenizer file");

    let report = tokenizer_badge_report(&root, &unknown);
    assert!(
        report.contains("tokenizer-format=\"Unknown format\""),
        "the badge did not call an unrecognized file unknown:\n{report}"
    );
    assert!(
        report.contains("tokenizer-format-bad=1"),
        "the badge did not mark the unknown format bad:\n{report}"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

/// With an input and the chunking row, `--selftest-tokenizer` plus an encoding
/// makes the page choose that family in the form and run. Both the run's printed
/// command line and the rendered command are read back, so the test proves the
/// page's choice reached the CLI rather than only that the selector changed.
#[test]
fn a_selftest_run_forwards_the_tokenizer_encoding_to_the_cli() {
    let root = repository_root();
    let input = root.join("test-data/brief.docx");
    let tokenizer = root.join("test-data/tokenizer-mini.tiktoken");
    assert!(input.exists(), "missing fixture {}", input.display());
    assert!(
        tokenizer.exists(),
        "missing fixture {}",
        tokenizer.display()
    );
    let cli = root.join("target/debug/pdfextractor-cli");
    if !cli.exists() {
        eprintln!("skipped: pdfextractor-cli is not built");
        return;
    }

    let Some(mut command) = ui_command() else {
        return;
    };
    command
        .current_dir(&root)
        .arg("--selftest")
        .arg(&input)
        .arg("--selftest-chunks")
        .arg("--selftest-tokenizer")
        .arg(&tokenizer)
        .arg("--selftest-tokenizer-encoding")
        .arg("o200k_base")
        .env("PDFEXTRACTOR_CLI", &cli);

    let output = run_with_timeout(command, Duration::from_secs(120));
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

    let printed = stdout
        .lines()
        .find(|line| line.starts_with("selftest-started "))
        .unwrap_or_else(|| {
            panic!("the run never printed its command line:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    assert!(
        printed.contains("--chunks-tokenizer-encoding o200k_base"),
        "the chosen encoding is not on the run's printed command line:\n{printed}"
    );
    assert!(
        printed.contains(&format!("--chunks-tokenizer {}", tokenizer.display())),
        "the chosen tokenizer is not on the run's printed command line:\n{printed}"
    );

    let report = stdout
        .lines()
        .find(|line| line.starts_with("selftest-rendered "))
        .unwrap_or_else(|| {
            panic!("the page reported nothing:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    assert!(
        report.contains("title=\"Finished\"") && report.contains("success=true"),
        "the encoded conversion did not finish successfully:\n{report}"
    );
    assert!(
        report.contains("tokenizer-encoding=o200k_base"),
        "the page did not choose the requested encoding:\n{report}"
    );
    assert!(
        report.contains("tokenizer tokenizer-mini.tiktoken (tiktoken · cl100k_base · 257 tokens)"),
        "the run summary omitted the detected tokenizer format and size:\n{report}"
    );
    assert!(
        report.contains("--to json --to chunks") && report.contains("--chunks-type hybrid"),
        "the run did not ask for hybrid chunks:\n{report}"
    );
    assert!(
        report.contains(&format!("--chunks-tokenizer {}", tokenizer.display())),
        "the chosen tokenizer did not reach the command line:\n{report}"
    );
    assert!(
        report.contains("--chunks-tokenizer-encoding o200k_base"),
        "the chosen encoding did not reach the command line:\n{report}"
    );
}
