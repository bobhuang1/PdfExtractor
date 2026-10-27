//! The `--compare` mode: a fallen panel fails a build, not only a window.
//!
//! Pointed at a Docling reference — a file, or a folder of them named after each
//! document (`test-data/golden/docling/`) — every document the run finished is
//! compared with its reference and held to the **memory** that lives beside that
//! reference (`compare-baseline.json`), and the run exits non-zero when a document
//! falls below its floor.
//!
//! The holding itself is not written here: it is `pdfextractor_ui::gate::Session`,
//! the same Session the window's Compare panel and the corpus report
//! (`crates/ui/tests/corpus_report_test.rs`) run their documents through, so the
//! panel, the corpus gate and this build gate cannot drift apart — a document
//! whose panel the window would refuse (`Report::floor`) is a document this run
//! fails on, with the same sentence. What this module is is the command line's
//! face on that gate: it prints one line per document, and turns the gate's
//! refusals into a failing exit code.
//!
//! What the mode holds, and to what:
//!
//! * Everything `Report::hold_to_floor` holds: **every** rule of the ratchet when
//!   the run's measurement matches the one the memory records (`Memory::measured` —
//!   the file says how each of its baselines was measured), and only the floor under
//!   a form's placed items when it does not, because OCR is the setting that moves
//!   the counts measuring how two readings agree.
//! * A document the memory does not know stands under no floor, exactly as in the
//!   window: there is nothing for it to fall from.
//! * A document whose reading goes through LibreOffice — a legacy `.doc`, or a
//!   family the bridge is the only reader for — is **left out** of the run on a
//!   machine without one, before anything is converted. The memory's numbers are
//!   Writer's reading of that file, and what a run here would compare is either
//!   the built-in reader's reading or, for a family with no reader here at all, no
//!   reading — so neither can be held to the other's numbers, and a document only
//!   the bridge can read would otherwise fail the gate on the missing tool rather
//!   than on anything the gate is about. Such a document is said on a line of its
//!   own and counted at the end, because a gate that quietly held less than it
//!   claims to is worse than no gate at all.
//! * A document that **cannot be compared** — no reference for it, an unreadable
//!   output — and a reference whose memory is there but cannot be read are failures
//!   rather than silences, and a reference path that is not there is refused before
//!   any conversion starts: a gate that held nothing while claiming to hold
//!   everything is worse than no gate at all.
//!
//! With no memory beside the reference — every comparison against a document of
//! your own — the comparisons are still printed and no floor can fire.

use crate::formats;
use pdfextractor_ui::diff;
use pdfextractor_ui::gate;
use std::path::{Path, PathBuf};

/// Open the gate for this run: the reference, the memory beside it, and the value
/// `--ocr` took as this run's measurement — which is what decides how much of the
/// ratchet applies.
pub fn open(reference: &Path, ocr: &str) -> anyhow::Result<gate::Session> {
    gate::Session::open(reference, diff::Measurement::from_ocr(ocr)).map_err(anyhow::Error::msg)
}

/// Why this document is left out of the run, when it is: it is read through
/// LibreOffice and this machine has none, so what a run here would compare is
/// another reader's reading of it — or, where the bridge is the only reader, no
/// reading at all.
///
/// Asked **before** the conversion rather than after it, which is what makes the
/// answer the same for both families: a legacy Excel workbook has nothing here to
/// read it with, so a rule that waited for a finished document would have the run
/// fail on the missing bridge first and never be reached. A comparison run has no
/// business converting a document it is not going to hold; a run that is only
/// converting does, and asks nothing.
///
/// The format is detected rather than read off the file's name, because that is
/// the question the run itself asked when it chose a reader: a DOCX called
/// `report.doc`, or a `.wps` that is really a package, is read here exactly as it
/// would be with the bridge installed, and so is held as usual.
pub fn not_held_here(input: &Path) -> Option<&'static str> {
    if formats::libreoffice().is_some() {
        return None;
    }
    let format = formats::detect(input).ok()?;
    formats::reads_through_libreoffice(format).then_some(
        "read through LibreOffice, and this machine has none — a reading made \
         without it is not the reading the memory remembers",
    )
}

/// Compare every document the run finished, print one line per document, and
/// answer with every reason the run must fail: a document below its floor, or one
/// that could not be compared at all. A conversion that failed has no output to
/// compare and is already a failure of the run, so it is left to the run's own
/// tally rather than reported twice. The documents this machine could not read the
/// way the memory's numbers were read were taken out of `results` before the run
/// started ([`not_held_here`]); they arrive here as `left_out`, with the reason,
/// to be said and counted beside the ones that were held.
pub fn check(
    session: &gate::Session,
    results: &[(PathBuf, anyhow::Result<PathBuf>)],
    left_out: &[(PathBuf, &'static str)],
) -> Vec<String> {
    // Said first, because these are the documents the run did not convert at all:
    // a reader of the log meets the absence before the lines about what was read.
    for (input, why) in left_out {
        eprintln!("compare: {}: not held · {why}", input.display());
    }
    let mut refused = Vec::new();
    for (input, result) in results {
        let Ok(output) = result else {
            continue;
        };
        let report = session.compare(&input.to_string_lossy(), output);

        // A document the comparison could not make: the line names the reason,
        // and the run fails on it — a silently skipped document is a hole in
        // the gate shaped exactly like a passed one.
        if let Some(problem) = &report.problem {
            let line = format!("{}: not compared · {problem}", input.display());
            eprintln!("compare: {line}");
            refused.push(line);
            continue;
        }

        let status = match &report.floor {
            Some(floor) => {
                refused.push(format!("{}: below the floor · {floor}", input.display()));
                format!("below the floor · {floor}")
            }
            None => standing(session, &report),
        };
        eprintln!(
            "compare: {}: {} row(s), agreement {:.1}% · {status}",
            input.display(),
            report.summary.rows,
            report.summary.text_agreement * 100.0
        );
    }
    // Said once at the end as well as on the document's own line: a run whose
    // log is read by somebody scanning for what a build did should not have to
    // notice an absence.
    if !left_out.is_empty() {
        eprintln!(
            "compare: {} document(s) not held: this machine has no LibreOffice, and \
             they are read through it",
            left_out.len()
        );
    }
    refused
}

/// How a document that fell through nothing stands: at or above the rule that held
/// it — the whole ratchet or the floor alone, in the same words the panel's own
/// held line uses, so a build log and a window say the same thing about the same
/// document — outside that memory entirely, or with nothing to be held to because
/// no memory was beside the reference.
///
/// The gate recorded which of the four on the report itself (`Report::held`), so
/// this reads what the holding decided rather than looking the document's stem up
/// in the memory a second time.
fn standing(session: &gate::Session, report: &diff::Report) -> String {
    match report.held.as_ref().map(|held| held.rules) {
        Some(diff::HeldRules::Ratchet) => "at or above the whole ratchet".to_string(),
        Some(diff::HeldRules::Floor) => "at or above the floor".to_string(),
        None if session.memory().is_some() => "not in the memory · no floor".to_string(),
        None => "no memory beside the reference".to_string(),
    }
}
