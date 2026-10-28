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
//!
//! The corpus's legacy `.doc` is the second fixture, for the one document whose
//! *reading* depends on the machine: Writer reads it where LibreOffice is
//! installed and the built-in reader reads it where it is not, and the two do not
//! say the same thing about the same file. The memory beside its reference was
//! measured with the bridge, so a run without one leaves the document to the
//! bridge rather than holding it to numbers another reader never produced — and
//! `PDFE_SOFFICE` is how these tests say which machine they are.
//!
//! The legacy `.xls` is the third, for the other half of that rule: a family the
//! bridge is the *only* reader for, where leaving it out cannot wait for a
//! finished document because there is no reading here to finish. Both are left
//! out before anything is converted, on the same sentence and in the same count.
//!
//! The last three ask the same question of a **misnamed** file, because the run
//! decides on the bytes and the extension is the only other thing there is to
//! decide on: a PDF called `report.doc` is read here with or without the bridge,
//! a Word binary called `notes.pdf` is Writer's to read all the same, and a legacy
//! workbook called `ledger.docx` is Calc's. A rule that looked at the name would
//! hold the first to Writer's numbers on a machine that never read it, and convert
//! the other two into a reader's failure.

use pdfextractor_ui::diff::{Measurement, Memory, Numbers};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Output};

/// The corpus's legacy `.doc`: read through LibreOffice where there is one, by
/// the built-in reader where there is not.
const LEGACY_DOC: &str = "test-data/real/two_images.doc";
/// Its committed reference, and the stem the memory remembers it under.
const LEGACY_REFERENCE: &str = "two_images.json";
const LEGACY_STEM: &str = "two_images";

/// A legacy Excel workbook: a BIFF compound file with no `WordDocument` stream,
/// so nothing here reads it and the bridge is not a better reading of it but the
/// only one. It has no reference, because a run that leaves it out never looks
/// for one.
const BRIDGE_ONLY: &str = "test-data/ledger.xls";
const BRIDGE_ONLY_STEM: &str = "ledger";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args(args)
        .output()
        .expect("run the CLI binary")
}

/// Run the CLI with `PDFE_SOFFICE` naming the LibreOffice this machine has.
///
/// The override is how a run is given a bridge, and a path that is not a file is
/// how it is told there is none — which is the only way to say "no LibreOffice"
/// on a machine whose `PATH` holds one.
fn run_with_soffice(args: &[&str], soffice: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args(args)
        .env("PDFE_SOFFICE", soffice)
        .output()
        .expect("run the CLI binary")
}

/// LibreOffice on this machine, by the lookup the CLI does when `PDFE_SOFFICE`
/// says nothing.
fn soffice_on_path() -> Option<PathBuf> {
    let search = std::env::var_os("PATH")?;
    std::env::split_paths(&search)
        .flat_map(|directory| ["soffice", "libreoffice"].map(|name| directory.join(name)))
        .find(|candidate| candidate.is_file())
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
        Scratch::holding(tag, "dummy.json")
    }

    /// A scratch folder with a reference folder of its own and **nothing** in it:
    /// a run that leaves its documents out never looks for a reference, and a run
    /// that converts one has none to hold it to — the two outcomes a test below
    /// needs to tell apart.
    fn empty(tag: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("pdfe-cli-compare-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("docling")).expect("create the scratch folder");
        Scratch(path)
    }

    /// A scratch folder holding one committed reference of `docling/`, under its
    /// own name — what the run compares a document of the same stem against.
    fn holding(tag: &str, reference: &str) -> Scratch {
        Scratch::holding_as(tag, reference, reference)
    }

    /// [`Scratch::holding`] under another name: the reference is found by the stem
    /// of the document it belongs to, so a document renamed in the scratch folder
    /// needs its reference renamed with it.
    fn holding_as(tag: &str, reference: &str, name: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("pdfe-cli-compare-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        let references = path.join("docling");
        std::fs::create_dir_all(&references).expect("create the scratch folder");
        std::fs::copy(
            repository_root()
                .join("test-data/golden/docling")
                .join(reference),
            references.join(name),
        )
        .expect("copy the reference");
        Scratch(path)
    }

    /// Copy a repository document into the scratch folder under `name`, and answer
    /// with where it landed: the same bytes carrying another extension, which is
    /// how a test hands the gate a file whose name lies about its format.
    fn copy_in(&self, document: &str, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::copy(repository_root().join(document), &path).expect("copy the document");
        path
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

    /// Convert `document` and compare it with the reference this scratch holds,
    /// on the machine whose LibreOffice is `soffice`.
    fn compare(&self, document: &str, soffice: &str) -> Output {
        self.compare_at(&repository_root().join(document), soffice)
    }

    /// Convert the document at `input` and compare it with the reference this
    /// scratch holds, on the machine whose LibreOffice is `soffice`.
    fn compare_at(&self, input: &std::path::Path, soffice: &str) -> Output {
        let output = self.0.join("out.json");
        run_with_soffice(
            &[
                "--input",
                input.to_str().expect("utf-8 input path"),
                "--output",
                output.to_str().expect("utf-8 output path"),
                "--ocr",
                "never",
                "--compare",
                self.reference().to_str().expect("utf-8 reference path"),
            ],
            soffice,
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A memory no reading of `stem` can stand under: every count remembered as
/// none, and the reference's characters as 200% held — a share above anything a
/// comparison can report. Whatever this machine reads of the document, a run that
/// holds it is refused, so the tests below cannot pass by the floor happening to
/// be out of reach.
fn a_memory_no_reading_can_meet(stem: &str) -> Memory {
    Memory {
        note: "a floor no reading reaches, written by the compare mode's test".to_string(),
        measured: Measurement::from_ocr("never"),
        documents: BTreeMap::from([(
            stem.to_string(),
            Numbers {
                rows: 0,
                same: 0,
                notation: 0,
                differs: 0,
                contained: 0,
                ours_only: 0,
                theirs_only: 0,
                placed: 0,
                placed_items: 0,
                placed_boxes: 0,
                placed_page_pictures: 0,
                joined: 0,
                agreement: 2.0,
            },
        )]),
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
    remembered("dummy", dummy)
}

/// A memory holding one document by its stem — the shape [`memory_of`] has, for a
/// test whose document is not called `dummy`.
fn remembered(stem: &str, numbers: Numbers) -> Memory {
    Memory {
        note: "written by the compare mode's test".to_string(),
        measured: Measurement::from_ocr("never"),
        documents: BTreeMap::from([(stem.to_string(), numbers)]),
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

/// A document the bridge reads is **not held** on a machine without one: the
/// numbers the memory remembers are Writer's reading of that file, and what this
/// run compared is the built-in reader's, so holding one to the other would refuse
/// a document for a fact about the machine. The run passes, says which document it
/// left out and why, and counts them — the scheduled bridge gate
/// (`.github/workflows/bridge.yml`) is where such a document is held.
#[test]
fn a_document_the_bridge_reads_is_not_held_on_a_machine_without_one() {
    let scratch = Scratch::holding("unbridged", LEGACY_REFERENCE);
    scratch.remember(&a_memory_no_reading_can_meet(LEGACY_STEM));

    let output = scratch.compare(LEGACY_DOC, "/nowhere/soffice");
    let logs = logs_of(&output);

    assert!(
        output.status.success(),
        "a document left to the bridge failed the run: {logs}"
    );
    assert!(
        logs.contains("not held · read through LibreOffice"),
        "the document was not said to be left out, nor why: {logs}"
    );
    assert!(
        logs.contains("1 document(s) not held"),
        "the run did not count what it left out: {logs}"
    );
    assert!(
        !logs.contains("below the floor"),
        "it was held to a floor another reader's numbers set: {logs}"
    );
}

/// The control for the test above: the same document, the same memory, on a
/// machine that has the bridge. Read through Writer, it *is* held — and refused,
/// because the memory is one no reading can reach. That is what makes the test
/// above about the bridge rather than about a floor that happened to be
/// unreachable. Returns without asserting where no LibreOffice is installed, as
/// the other bridge tests do.
#[test]
fn the_same_document_is_held_when_the_bridge_is_here() {
    let Some(soffice) = soffice_on_path() else {
        println!("skipped: no LibreOffice here, so there is no bridge to hold it to");
        return;
    };
    let scratch = Scratch::holding("bridged", LEGACY_REFERENCE);
    scratch.remember(&a_memory_no_reading_can_meet(LEGACY_STEM));

    let output = scratch.compare(LEGACY_DOC, soffice.to_str().expect("a UTF-8 path"));
    let logs = logs_of(&output);

    assert!(
        !output.status.success(),
        "a document the bridge read was not held: {logs}"
    );
    assert!(
        logs.contains("below the floor"),
        "the refusal was not the floor: {logs}"
    );
    assert!(
        !logs.contains("not held"),
        "a machine with the bridge left the document out: {logs}"
    );
}

/// A family the bridge is the **only** reader for is left out of a compare run on
/// a machine without it, on the same sentence and in the same count as the legacy
/// `.doc` — and before anything is converted, because there is no reading here to
/// finish and leave out afterwards. Without that, a workbook only Calc can open
/// would fail the gate on the missing tool rather than on anything the gate is
/// about, which is the failure the legacy `.doc` was left out to stop.
#[test]
fn a_format_only_the_bridge_can_read_is_not_held_on_a_machine_without_one() {
    let scratch = Scratch::empty("bridge-only-unbridged");
    scratch.remember(&a_memory_no_reading_can_meet(BRIDGE_ONLY_STEM));

    let output = scratch.compare(BRIDGE_ONLY, "/nowhere/soffice");
    let logs = logs_of(&output);

    assert!(
        output.status.success(),
        "a document left to the bridge failed the run: {logs}"
    );
    assert!(
        logs.contains("not held · read through LibreOffice"),
        "the document was not said to be left out, nor why: {logs}"
    );
    assert!(
        logs.contains("1 document(s) not held"),
        "the run did not count what it left out: {logs}"
    );
    assert!(
        !logs.contains("below the floor"),
        "it was held to a floor another reader's numbers set: {logs}"
    );
    // And not converted: leaving it out happens before the run spends a bridge it
    // does not have. A run that read the file first could not leave this family
    // out at all, because the read is what would fail.
    assert!(
        !scratch.0.join("out.json").exists(),
        "the document was converted by a run that left it out: {logs}"
    );
}

/// The control for the test above: the same workbook on a machine that has the
/// bridge. It is read, and then refused for having no reference in this scratch —
/// the ordinary rule for a document nothing remembers — which is what shows the
/// test above is about the bridge rather than about a file the run passes over.
/// Returns without asserting where no LibreOffice is installed, as the other
/// bridge tests do.
#[test]
fn the_bridge_only_format_is_read_when_the_bridge_is_here() {
    let Some(soffice) = soffice_on_path() else {
        println!("skipped: no LibreOffice here, so there is no bridge to read it through");
        return;
    };
    let scratch = Scratch::empty("bridge-only-bridged");
    scratch.remember(&a_memory_no_reading_can_meet(BRIDGE_ONLY_STEM));

    let output = scratch.compare(BRIDGE_ONLY, soffice.to_str().expect("a UTF-8 path"));
    let logs = logs_of(&output);

    assert!(
        !output.status.success(),
        "an unheld document passed where the bridge is: {logs}"
    );
    assert!(
        !logs.contains("not held"),
        "a machine with the bridge left the document out: {logs}"
    );
    assert!(
        logs.contains("no reference for this document"),
        "the refusal was not the missing reference: {logs}"
    );
    assert!(
        scratch.0.join("out.json").exists(),
        "the document was not converted where the bridge is: {logs}"
    );
}

/// A document whose **name** says the bridge reads it, but whose bytes say a PDF,
/// is read here and **held** on a machine without LibreOffice — because the run
/// asks the bytes, as the reader it chose did. `dummy.pdf` under the name
/// `report.doc` reads exactly as `dummy.pdf` does, so a name-decided rule would
/// have left it out and quietly held one document fewer than it claims to.
#[test]
fn a_document_whose_name_claims_the_bridge_is_held_when_the_bytes_do_not() {
    // The reference is found by the document's stem, so it is copied in under the
    // stem the renamed file has.
    let scratch = Scratch::holding_as("misnamed-held", "dummy.json", "report.json");
    scratch.remember(&remembered("report", dummy_as_read()));
    let input = scratch.copy_in("test-data/real/dummy.pdf", "report.doc");

    let output = scratch.compare_at(&input, "/nowhere/soffice");
    let logs = logs_of(&output);

    assert!(
        output.status.success(),
        "a PDF named `.doc` failed the run: {logs}"
    );
    assert!(
        !logs.contains("not held"),
        "a PDF named `.doc` was left to a bridge it does not need: {logs}"
    );
    assert!(
        logs.contains("at or above the whole ratchet"),
        "it was not held to the memory at all: {logs}"
    );
    assert!(
        scratch.0.join("out.json").exists(),
        "the document was not converted by a run that claimed to hold it: {logs}"
    );
}

/// The other direction, and the one that costs something if it is wrong: a Word
/// binary called `notes.pdf` is Writer's reading of that file, so a machine
/// without the bridge leaves it out — the name does not make the built-in reader
/// a reader of it. Converted instead, it would fail on the missing bridge and take
/// the gate down for a fact about the machine rather than about the document.
#[test]
fn a_document_whose_name_hides_the_bridge_is_left_out_without_one() {
    let scratch = Scratch::empty("misnamed-left-out");
    scratch.remember(&a_memory_no_reading_can_meet("notes"));
    let input = scratch.copy_in(LEGACY_DOC, "notes.pdf");

    let output = scratch.compare_at(&input, "/nowhere/soffice");
    let logs = logs_of(&output);

    assert!(
        output.status.success(),
        "a document left to the bridge failed the run: {logs}"
    );
    assert!(
        logs.contains("notes.pdf: not held · read through LibreOffice"),
        "the document was not said to be left out, nor why: {logs}"
    );
    assert!(
        logs.contains("1 document(s) not held"),
        "the run did not count what it left out: {logs}"
    );
    assert!(
        !scratch.0.join("out.json").exists(),
        "a Word binary named `.pdf` was converted by a run that left it out: {logs}"
    );
}

/// And the bridge-only families answer to their bytes the same way: the legacy
/// workbook is given a name the built-in readers own, where the gate must still
/// leave it out — its `Workbook` stream is what Calc opens, and the name promises
/// a package this file is not. Converted instead, it would fail on a name it
/// never had rather than on the missing bridge, and the gate would redden for the
/// same wrong reason the `.xls` was left out to avoid.
#[test]
fn a_workbook_named_as_a_package_is_left_out_without_the_bridge() {
    let scratch = Scratch::empty("bridge-only-misnamed");
    scratch.remember(&a_memory_no_reading_can_meet(BRIDGE_ONLY_STEM));
    let input = scratch.copy_in(BRIDGE_ONLY, "ledger.docx");

    let output = scratch.compare_at(&input, "/nowhere/soffice");
    let logs = logs_of(&output);

    assert!(
        output.status.success(),
        "a document left to the bridge failed the run: {logs}"
    );
    assert!(
        logs.contains("ledger.docx: not held · read through LibreOffice"),
        "the workbook was not said to be left out, nor why: {logs}"
    );
    assert!(
        logs.contains("1 document(s) not held"),
        "the run did not count what it left out: {logs}"
    );
    assert!(
        !scratch.0.join("out.json").exists(),
        "a workbook named `.docx` was converted by a run that left it out: {logs}"
    );
}
