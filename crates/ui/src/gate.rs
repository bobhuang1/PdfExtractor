//! Holding a finished document to the memory beside its reference: **one gate**.
//!
//! Three places hold a reading to the ratchet the memory remembers — the window's
//! Compare panel (`main.rs`), the corpus report (`tests/corpus_report_test.rs`)
//! and the command line's `--compare` mode (`crates/cli`) — and this is the one
//! Session all three run their documents through, so a rule, a sentence or a
//! refusal added here reaches all of them at once and none can drift from the
//! others. The rules themselves are [`diff`]'s ([`diff::Numbers`],
//! [`diff::Report::hold_to_floor`]); what this module adds is the *holding*: the
//! reference, the memory beside it, and how the run was measured — the last of
//! those deciding how much of the ratchet applies ([`diff::Held`]).
//!
//! A document the memory does not have stands under no floor, and a comparison
//! against a reference with no memory beside it — every run of a document of your
//! own — is simply not held to one.

use crate::diff::{self, Measurement, Memory, Report};
use std::path::{Path, PathBuf};

/// One run's gate: the reference, the memory beside it, and how the run was
/// measured.
#[derive(Debug)]
pub struct Session {
    reference: PathBuf,
    memory: Option<Memory>,
    measurement: Measurement,
}

impl Session {
    /// Open the reference and read the memory that lives beside it.
    ///
    /// Fails on a reference that is not there — a typo in a build's configuration
    /// must not convert a corpus to prove nothing — and on a memory that *is*
    /// there and cannot be read ([`Memory::beside`]'s own rule: a floor that
    /// quietly did nothing would be worse than no floor at all). No memory at all
    /// is not an error: comparing without one is how every run of a document of
    /// your own works, and nothing is held there.
    ///
    /// `measurement` is how this run was measured — the window records the
    /// settings the person chose, the command line the value `--ocr` took —
    /// recorded against the memory's own ([`Memory::measured`]) to decide whether
    /// every ratchet rule applies or only the floor.
    pub fn open(reference: &Path, measurement: Measurement) -> Result<Session, String> {
        if !reference.exists() {
            return Err(format!(
                "compare reference does not exist: {}",
                reference.display()
            ));
        }
        let memory = Memory::beside(reference)?;
        Ok(Session {
            reference: reference.to_path_buf(),
            memory,
            measurement,
        })
    }

    /// A session over a memory the caller already holds.
    ///
    /// The corpus report both reads its baseline and *writes* it
    /// (`CORPUS_BASELINE=update`), so it cannot be the one that discovers it
    /// beside the references; everything else — which rules apply, what a fall is
    /// called, which documents the memory has — is the gate's either way.
    pub fn with_memory(
        reference: &Path,
        memory: Option<Memory>,
        measurement: Measurement,
    ) -> Session {
        Session {
            reference: reference.to_path_buf(),
            memory,
            measurement,
        }
    }

    /// The line a run opens with: what documents are compared with, and what —
    /// if anything — holds them to their numbers.
    pub fn announce(&self) -> String {
        match &self.memory {
            Some(memory) => {
                let how = if memory.measured.ocr.is_empty() {
                    "measurement not recorded".to_string()
                } else {
                    format!("measured with --ocr {}", memory.measured.ocr)
                };
                format!(
                    "{} · memory: {} document(s), {how}",
                    self.reference.display(),
                    memory.documents.len()
                )
            }
            None => format!(
                "{} · no memory beside the reference: nothing is held to a floor",
                self.reference.display()
            ),
        }
    }

    /// The reference documents are compared with.
    pub fn reference(&self) -> &Path {
        &self.reference
    }

    /// The memory documents are held to, when there is one.
    pub fn memory(&self) -> Option<&Memory> {
        self.memory.as_ref()
    }

    /// Hold one comparison to the memory beside the reference, and answer whether
    /// it fell through.
    ///
    /// Nothing is held when there is no memory, or when the memory does not know
    /// this document; either way the report says which rules held it
    /// ([`Report::held`]) so a caller — panel, report or gate — can show what
    /// applied rather than only what fell.
    pub fn hold(&self, report: &mut Report) -> bool {
        match &self.memory {
            Some(memory) => report.hold_to_floor(memory, Some(&self.measurement)),
            None => false,
        }
    }

    /// Compare one finished document with its reference — wherever that reference
    /// is, by the same lookup the panel does ([`diff::compare_document`]) — and
    /// hold the comparison to the memory.
    pub fn compare(&self, input: &str, output: &Path) -> Report {
        let mut report = diff::compare_document(input, output, &self.reference);
        self.hold(&mut report);
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A scratch directory that removes itself.
    fn scratch(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pdfe-gate-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        path
    }

    #[test]
    fn a_reference_that_is_not_there_is_refused() {
        let error = Session::open(
            Path::new("/nowhere/any/reference"),
            Measurement::from_ocr("never"),
        )
        .expect_err("a missing reference is not a session");
        assert!(
            error.contains("compare reference does not exist"),
            "{error}"
        );
    }

    #[test]
    fn a_memory_that_cannot_be_read_is_reported_rather_than_passed_over() {
        let dir = scratch("broken");
        let references = dir.join("docling");
        std::fs::create_dir_all(&references).unwrap();
        std::fs::write(dir.join(Memory::FILE), "{not a memory}").unwrap();

        let error = Session::open(&references, Measurement::from_ocr("never"))
            .expect_err("a broken memory is not a session");
        assert!(error.contains("is not a compare baseline"), "{error}");
    }

    #[test]
    fn a_reference_with_no_memory_beside_it_holds_nothing_and_is_not_an_error() {
        let dir = scratch("memoryless");
        let references = dir.join("docling");
        std::fs::create_dir_all(&references).unwrap();

        let session =
            Session::open(&references, Measurement::from_ocr("never")).expect("open the gate");
        assert!(session.memory().is_none());
        assert_eq!(session.reference(), references);
        assert!(session
            .announce()
            .contains("no memory beside the reference"));
    }

    #[test]
    fn the_announce_line_says_what_the_memory_holds_documents_to() {
        // The memory beside the corpus's references, in the words every entry point
        // opens with — the CLI prints this line before a gate run, and it names the
        // measurement the whole run will be held against.
        let memory = Memory {
            note: "a memory, for the tests".to_string(),
            measured: Measurement::from_ocr("never"),
            documents: BTreeMap::new(),
        };
        let session = Session::with_memory(
            Path::new("test-data/golden/docling"),
            Some(memory),
            Measurement::from_ocr("never"),
        );
        assert_eq!(
            session.announce(),
            "test-data/golden/docling · memory: 0 document(s), measured with --ocr never"
        );
    }

    #[test]
    fn a_session_with_no_memory_holds_a_report_to_nothing() {
        // The ordinary case of a document of your own: the comparison is the
        // caller's, and the gate neither holds it nor claims to.
        let session = Session::with_memory(
            Path::new("test-data/golden/docling"),
            None,
            Measurement::from_ocr("never"),
        );
        let mut report = diff::compare_document(
            "test-data/real/mine.pdf",
            Path::new("/nowhere/out.json"),
            session.reference(),
        );
        assert!(!session.hold(&mut report));
        assert_eq!(report.held, None);
        assert_eq!(report.floor, None);
    }
}
