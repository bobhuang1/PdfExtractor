//! The `--compare` mode against the real binary.
//!
//! The unit tests of `pdfextractor_ui::diff` hold documents to a memory by hand;
//! these run the command a build would run — convert `test-data/real/dummy.pdf`,
//! point `--compare` at a reference folder — and read the verdict from the
//! process: a document below its floor must fail the run, a document at or above
//! it must pass, and a run measured differently from the memory must be held only
//! to the floor rather than to every ratchet rule. That is the whole reason the
//! mode exists: the window refuses such a document *on its panel*, and a panel
//! nobody looks at does not fail a build.
//!
//! `dummy.pdf` is the fixture because it converts in a fraction of a second (one
//! page, no OCR) and its committed reference in `test-data/golden/docling/` is
//! its one row: `differs: 1`, `agreement: 1.0`, nothing placed. Each test writes
//! its own memory into a scratch folder beside a copy of that reference, so the
//! corpus's real baseline is never touched.

use pdfextractor_ui::diff::{Measurement, Memory, Numbers};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Output};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args(args)
        .output()
        .expect("run the CLI binary")
}

/// Everything the process said, wherever it said it.
fn logs_of(output: &Output) -> String {
    let mut logs = String::from_utf8_lossy(&output.stdout).into_owned();
    logs.push_str(&String::from_utf8_lossy(&output.stderr));
    logs
}

/// A scratch folder that removes itself, holding a reference folder with the
/// committed `dummy.json` copied into it as `docling/dummy.json`.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("pdfe-cli-compare-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        let references = path.join("docling");
        std::fs::create_dir_all(&references).expect("create the scratch folder");
        std::fs::copy(
            repository_root().join("test-data/golden/docling/dummy.json"),
            references.join("dummy.json"),
        )
        .expect("copy the reference");
        Scratch(path)
    }

    /// The reference folder to point `--compare` at.
    fn reference(&self) -> PathBuf {
        self.0.join("docling")
    }

    /// The memory beside that reference, where `Memory::beside` looks after the
    /// folder itself: `<scratch>/compare-baseline.json`.
    fn remember(&self, memory: &Memory) {
        let json = serde_json::to_string_pretty(memory).expect("serialize the memory");
        std::fs::write(self.0.join(Memory::FILE), json).expect("write the memory");
    }

    /// Convert `dummy.pdf` and compare it with the scratch reference, measured
    /// with `ocr` as the run's `--ocr` value.
    fn compare_dummy(&self, ocr: &str) -> Output {
        let input = repository_root().join("test-data/real/dummy.pdf");
        run(&[
            "--input",
            input.to_str().expect("utf-8 input path"),
            "--output",
            self.0.join("out.json").to_str().expect("utf-8 output path"),
            "--ocr",
            ocr,
            "--compare",
            self.reference().to_str().expect("utf-8 reference path"),
        ])
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What `dummy.pdf` reads as today, in the form the ratchet remembers it: one
/// row, that row disagreeing, the whole reference's text held, nothing placed.
fn dummy_as_read() -> Numbers {
    Numbers {
        rows: 1,
        same: 0,
        notation: 0,
        differs: 1,
        contained: 0,
        ours_only: 0,
        theirs_only: 0,
        placed: 0,
        placed_items: 0,
        placed_boxes: 0,
        placed_page_pictures: 0,
        joined: 0,
        agreement: 1.0,
    }
}

/// The memory as the corpus report would write it: `dummy` remembered by its
/// stem, measured with `--ocr never`.
fn memory_of(dummy: Numbers) -> Memory {
    Memory {
        note: "written by the compare mode's test".to_string(),
        measured: Measurement::from_ocr("never"),
        documents: BTreeMap::from([("dummy".to_string(), dummy)]),
    }
}

/// A document whose form lost its items fails the run: this is the panel the
/// window refuses, and the refusal has to reach the process's exit code, not
/// only the log line.
#[test]
fn a_document_below_the_floor_fails_the_run() {
    let scratch = Scratch::new("floor");
    let mut remembered = dummy_as_read();
    // The reference's form items were placed as five; today none of them are.
    remembered.placed_items = 5;
    scratch.remember(&memory_of(remembered));

    let output = scratch.compare_dummy("never");
    let logs = logs_of(&output);

    assert!(!output.status.success(), "a fallen panel passed: {logs}");
    assert!(
        logs.contains("form item(s) placed: 5 → 0 item(s) in 0 row(s)"),
        "the floor was not named: {logs}"
    );
    assert!(
        logs.contains("refused"),
        "the failure does not say the compare mode refused it: {logs}"
    );
}

/// When the run is measured the way the memory says it was, every rule of the
/// ratchet applies — not only the floor. Here a rule with nothing to do with
/// placed items (a row both readings have and disagree about) is what fails the
/// run, which only happens when the two measurements match.
#[test]
fn every_ratchet_rule_applies_when_the_run_matches_the_remembered_measurement() {
    let scratch = Scratch::new("match");
    let mut remembered = dummy_as_read();
    // The baseline pretends we used to have no disagreeing rows at all.
    remembered.differs = 0;
    scratch.remember(&memory_of(remembered));

    let output = scratch.compare_dummy("never");
    let logs = logs_of(&output);

    assert!(
        !output.status.success(),
        "a ratchet rule did not fire: {logs}"
    );
    assert!(
        logs.contains("rows that differ 0 → 1"),
        "the rule was not named: {logs}"
    );
}

/// The same memory, a run measured differently (`--ocr auto` against a baseline
/// measured with `--ocr never`): OCR is the setting that moves agreement counts,
/// so only the floor under a form's placed items applies — and that floor holds
/// nothing here (nothing was ever placed), so the run passes. The rule above
/// must not leak into a run the memory cannot speak for.
#[test]
fn a_rule_needing_matching_settings_is_held_back_when_they_differ() {
    let scratch = Scratch::new("mismatch");
    let mut remembered = dummy_as_read();
    remembered.differs = 0;
    scratch.remember(&memory_of(remembered));

    let output = scratch.compare_dummy("auto");
    let logs = logs_of(&output);

    assert!(
        output.status.success(),
        "a rule from another measurement's settings fired: {logs}"
    );
    assert!(
        logs.contains("memory: 1 document(s)"),
        "the header does not say what is remembered: {logs}"
    );
    // Held to the floor alone, and the line says *that* rather than claiming the
    // whole ratchet stood over a document it cannot vouch for.
    assert!(
        logs.contains("at or above the floor"),
        "the floor-only standing was not reported: {logs}"
    );
}

/// At or above the rule that held it: the run is measured the way the memory says
/// it was, every rule applies, and the line says so — a reader of a build log can
/// tell a whole-ratchet pass from a floor-only one, which is what the panel says
/// too, and should see that the document was actually held rather than skipped.
#[test]
fn a_document_at_or_above_the_whole_ratchet_passes_and_says_so() {
    let scratch = Scratch::new("above");
    scratch.remember(&memory_of(dummy_as_read()));

    let output = scratch.compare_dummy("never");
    let logs = logs_of(&output);

    assert!(output.status.success(), "a held document failed: {logs}");
    assert!(
        logs.contains("at or above the whole ratchet"),
        "the standing was not reported: {logs}"
    );
    assert!(
        logs.contains("1 row(s)"),
        "the rows were not reported: {logs}"
    );
}

/// No memory beside the reference is how every comparison against a document of
/// your own looks: the comparison still runs and prints, and there is nothing
/// for a floor to be measured against, so nothing fails.
#[test]
fn no_memory_beside_the_reference_holds_nothing_and_fails_nothing() {
    let scratch = Scratch::new("memoryless");

    let output = scratch.compare_dummy("never");
    let logs = logs_of(&output);

    assert!(
        output.status.success(),
        "a comparison with no memory failed: {logs}"
    );
    assert!(
        logs.contains("no memory beside the reference"),
        "the absence of a memory was not said: {logs}"
    );
}

/// A memory that is there and cannot be read fails the run before any
/// conversion: a floor that quietly did nothing would be worse than no floor at
/// all, and a build should not spend the build to discover it.
#[test]
fn a_memory_that_cannot_be_read_fails_before_the_run_starts() {
    let scratch = Scratch::new("broken");
    std::fs::write(scratch.0.join(Memory::FILE), "{not a memory at all")
        .expect("write the broken memory");

    let output = scratch.compare_dummy("never");
    let logs = logs_of(&output);

    assert!(!output.status.success(), "a broken memory passed: {logs}");
    assert!(
        logs.contains("is not a compare baseline"),
        "the broken memory was not named: {logs}"
    );
    assert!(
        !scratch.0.join("out.json").exists(),
        "the run converted anything before refusing the memory: {logs}"
    );
}

/// A reference path that is not there is a broken configuration — a typo in a
/// build's command line must not convert a corpus to prove nothing.
#[test]
fn a_compare_reference_that_is_not_there_is_refused() {
    let input = repository_root().join("test-data/real/dummy.pdf");
    let output = run(&[
        "--input",
        input.to_str().expect("utf-8 input path"),
        "--output",
        std::env::temp_dir()
            .join("pdfe-cli-compare-no-ref.json")
            .to_str()
            .expect("utf-8 output path"),
        "--compare",
        "/nowhere/any/reference",
    ]);
    let logs = logs_of(&output);

    assert!(
        !output.status.success(),
        "a missing reference passed: {logs}"
    );
    assert!(
        logs.contains("compare reference does not exist"),
        "the missing reference was not named: {logs}"
    );
}

/// A document with no reference beside it cannot be held to anything, so the
/// mode fails it rather than passing over it: a gate that held nothing while
/// claiming to hold everything is worse than no gate.
#[test]
fn a_document_with_no_reference_fails_the_comparison() {
    let scratch = Scratch::new("unreferenced");
    // The reference the fixture was copied in does not belong to this test.
    std::fs::remove_file(scratch.reference().join("dummy.json")).expect("remove the reference");

    let output = scratch.compare_dummy("never");
    let logs = logs_of(&output);

    assert!(
        !output.status.success(),
        "an unheld document passed: {logs}"
    );
    assert!(
        logs.contains("not compared"),
        "the skipped comparison was not said: {logs}"
    );
    assert!(
        logs.contains("no reference for this document"),
        "the reason was not named: {logs}"
    );
}
