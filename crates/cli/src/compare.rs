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
//! * A document that **cannot be compared** — no reference for it, an unreadable
//!   output — and a reference whose memory is there but cannot be read are failures
//!   rather than silences, and a reference path that is not there is refused before
//!   any conversion starts: a gate that held nothing while claiming to hold
//!   everything is worse than no gate at all.
//!
//! With no memory beside the reference — every comparison against a document of
//! your own — the comparisons are still printed and no floor can fire.

use pdfextractor_ui::diff;
use pdfextractor_ui::gate;
use std::path::{Path, PathBuf};

/// Open the gate for this run: the reference, the memory beside it, and the value
/// `--ocr` took as this run's measurement — which is what decides how much of the
/// ratchet applies.
pub fn open(reference: &Path, ocr: &str) -> anyhow::Result<gate::Session> {
    gate::Session::open(reference, diff::Measurement::from_ocr(ocr)).map_err(anyhow::Error::msg)
}

/// Compare every document the run finished, print one line per document, and
/// answer with every reason the run must fail: a document below its floor, or one
/// that could not be compared at all. A conversion that failed has no output to
/// compare and is already a failure of the run, so it is left to the run's own
/// tally rather than reported twice.
pub fn check(
    session: &gate::Session,
    results: &[(PathBuf, anyhow::Result<PathBuf>)],
) -> Vec<String> {
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
