//! Tests for the part of the UI that runs the CLI as a subprocess.
//!
//! System binaries stand in for the CLI so the tests cover the UI's process
//! handling — argument forwarding, output capture, streaming, stopping a run —
//! without depending on the CLI's own build.
//!
//! The stand-ins are existing executables rather than generated shell scripts
//! on purpose: writing a file and immediately executing it races with the forks
//! of other tests running in parallel, which intermittently fails with
//! `ETXTBSY` ("Text file busy"). A pre-existing binary has no such window.

#![cfg(unix)]

use pdfextractor_ui::{outcome, pump, Job, RunEvent};
use std::io::BufReader;
use std::time::{Duration, Instant};

/// How long the polling wait looks between checks; short so a test is not the
/// thing that slows the suite down.
const POLL: Duration = Duration::from_millis(5);

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|value| value.to_string()).collect()
}

#[test]
fn arguments_are_forwarded_unchanged() {
    // `echo` prints exactly the arguments it was given, separated by spaces.
    let (job, stdout, _stderr) = Job::spawn(
        "/bin/echo",
        &args(&["--input", "a b.pdf", "--ocr", "never"]),
    )
    .expect("run echo");

    let mut lines = Vec::new();
    let tally = pump(BufReader::new(stdout), &mut |event| {
        if let RunEvent::Log(line) = event {
            lines.push(line);
        }
    });
    let status = job.wait_polling(POLL).expect("an exit status");

    assert_eq!(lines, vec!["--input a b.pdf --ocr never"]);
    assert_eq!(tally, Default::default(), "echo reported documents");
    assert!(status.success());
}

#[test]
fn a_non_zero_exit_is_a_failure_that_keeps_its_stderr() {
    let (job, _stdout, stderr) = Job::spawn(
        "/bin/sh",
        &args(&["-c", "echo 'something went wrong' >&2; exit 3"]),
    )
    .expect("run sh");

    let mut lines = Vec::new();
    pump(BufReader::new(stderr), &mut |event| {
        if let RunEvent::Log(line) = event {
            lines.push(line);
        }
    });
    let status = job.wait_polling(POLL).expect("an exit status");
    let verdict = outcome(job.cancelled(), Some(status));

    assert!(!verdict.success, "exit 3 must not read as success");
    assert!(!verdict.cancelled, "nothing was stopped");
    assert_eq!(verdict.exit_code, Some(3));
    assert_eq!(lines, vec!["something went wrong"], "stderr was dropped");
}

#[test]
fn a_missing_program_is_an_error_not_a_panic() {
    let result = Job::spawn("/definitely/not/a/program", &[]);
    assert!(result.is_err(), "a missing program should be an error");
}

#[test]
fn output_is_streamed_as_it_arrives_not_after_the_run() {
    // The point of the whole design: a slow run must show its first line long
    // before it finishes, otherwise the window is still a blank wait.
    let (job, stdout, _stderr) = Job::spawn(
        "/bin/sh",
        &args(&["-c", "echo first; sleep 30; echo second"]),
    )
    .expect("run sh");

    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        pump(BufReader::new(stdout), &mut |event| {
            if let RunEvent::Log(line) = event {
                let _ = sender.send(line);
            }
        })
    });

    let started = Instant::now();
    let first = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the first line never arrived");
    let waited = started.elapsed();

    assert_eq!(first, "first");
    assert!(
        waited < Duration::from_secs(5),
        "the first line waited {waited:?}, so output is not being streamed"
    );

    // Stopping the run must also let the reader finish with the stream.
    assert!(job.cancel(), "the run could not be stopped");
    reader.join().expect("the reader finished");
}

#[test]
fn a_run_can_be_stopped_while_it_is_working() {
    let (job, _stdout, _stderr) = Job::spawn("/bin/sleep", &args(&["30"])).expect("run sleep");

    let started = Instant::now();
    assert!(job.cancel(), "the stop was not sent");
    assert!(job.cancelled(), "the stop was not recorded");
    let status = job.wait_polling(Duration::from_millis(10));
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "a stopped run took {elapsed:?} to end"
    );
    let verdict = outcome(job.cancelled(), status);
    assert!(!verdict.success, "a stopped run is not a success");
    assert!(verdict.cancelled, "a stop must read as a cancellation");
    assert_eq!(
        verdict.exit_code, None,
        "a stopped process has no exit code"
    );
}

#[test]
fn stopping_a_finished_run_does_nothing() {
    let (job, _stdout, _stderr) = Job::spawn("/bin/true", &[]).expect("run true");
    assert!(job.wait_polling(POLL).expect("an exit status").success());
    assert!(job.finished(), "the exit was not recorded");

    assert!(!job.cancel(), "there was nothing left to stop");
    assert!(!job.cancelled());
}

#[test]
fn a_run_that_finished_as_it_was_stopped_is_a_success() {
    // Clicking Stop as the last document lands must not misreport the result:
    // the run's own exit status is the honest description.
    let (job, _stdout, _stderr) = Job::spawn("/bin/true", &[]).expect("run true");
    let status = job.wait_polling(POLL);

    let verdict = outcome(true, status);
    assert!(verdict.success, "a completed run stayed successful");
    assert!(
        !verdict.cancelled,
        "a completed run was reported as stopped"
    );
}
