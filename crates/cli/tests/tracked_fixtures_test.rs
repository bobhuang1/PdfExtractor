//! Every fixture the golden suites read is in the git index.
//!
//! The suites read files rather than name them in code: a reference is anything
//! in `test-data/golden/docling/`, a render golden is anything under the four
//! format directories, and the document behind a golden is the one beside the
//! corpus with the same stem. That is what makes the tree work — and it is also
//! why a fixture can be *present* on the machine that wrote it and absent from
//! the repository: a rule in `.gitignore` hides it, `git status` never mentions
//! it, and a fresh checkout runs the suites with a document or a reference
//! missing. This test is the check that cannot happen silently: every file those
//! directories hand a suite has to be in the index, and one that is not fails
//! here naming the rule that hid it, or saying it was never added.
//!
//! It knows nothing about *why* a file is not tracked; that is the point. A file
//! the suites read belongs in the repository, whether it is hand-written or
//! generated — the fixtures written by `test-data/make_*.py` are committed on
//! purpose — and the generator scripts themselves are the one kind of file in
//! `test-data/` a suite does not read.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every file below `directory`, in a stable order.
fn files_below(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The files directly in `directory`, without its subdirectories.
fn files_in(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    found
}

/// The document a golden's stem names, from the corpus first and then the
/// fixtures beside it — the order `corpus_input` in the Docling test resolves
/// them in.
fn document_named(root: &Path, stem: &str) -> Option<PathBuf> {
    for directory in ["test-data/real", "test-data"] {
        for path in files_in(&root.join(directory)) {
            if path.file_stem().and_then(|name| name.to_str()) == Some(stem) {
                return Some(path);
            }
        }
    }
    None
}

/// The directories a golden suite reads whole, one file at a time: the
/// references, the corpus the schema test converts, the photographed corpus the
/// OCR floors are measured on, and the images the picture fixtures name.
const READ_WHOLE: [&str; 4] = [
    "test-data/golden/docling",
    "test-data/real",
    "test-data/photos",
    "test-data/images",
];

/// The render golden directories. Their files are read whole *and* name their
/// document by stem.
const RENDER_GOLDENS: [&str; 4] = [
    "test-data/golden/markdown",
    "test-data/golden/html",
    "test-data/golden/text",
    "test-data/golden/yaml",
];

/// The files the golden suites read.
///
/// Gathered the way the suites gather them, rather than from a list that would
/// have to be kept in step: the schema, the oracle script and the ratchet's
/// memory they open by name; every file in the directories they walk; and the
/// document behind every golden, found by its stem.
fn fixtures(root: &Path) -> Vec<PathBuf> {
    let golden = root.join("test-data/golden");
    let mut paths = vec![
        golden.join("DoclingDocument.json"),
        golden.join("validate_with_docling.py"),
        golden.join("compare-baseline.json"),
    ];
    for directory in READ_WHOLE.iter().chain(RENDER_GOLDENS.iter()) {
        paths.extend(files_below(&root.join(directory)));
    }

    let mut stems = BTreeSet::new();
    for directory in RENDER_GOLDENS {
        for path in files_in(&root.join(directory)) {
            if let Some(stem) = path.file_stem().and_then(|name| name.to_str()) {
                stems.insert(stem.to_string());
            }
        }
    }
    for reference in files_in(&golden.join("docling")) {
        if let Some(stem) = reference.file_stem().and_then(|name| name.to_str()) {
            stems.insert(stem.to_string());
        }
    }
    // A golden whose document cannot be found is the suites' own failure (the
    // corpus report names a reference with no document), not this test's.
    for stem in stems {
        if let Some(document) = document_named(root, &stem) {
            paths.push(document);
        }
    }

    paths.sort();
    paths.dedup();
    paths
}

/// The files in the index, as paths from the repository root.
///
/// `-z` splits on NUL, so a name with a space or a newline in it — the picture
/// fixtures have both — is one entry rather than two.
fn tracked(root: &Path) -> BTreeSet<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--"])
        .output()
        .expect("run git ls-files");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// The rule that hides `path`, as `git check-ignore -v` spells it, or `None`
/// when nothing does and the file is simply not added yet.
///
/// `check-ignore` reports the last rule that matched, negation included, so a
/// `!` pattern (the exception that *keeps* a file in) is not a hiding rule: the
/// file then matches nothing hidden, which is why it is reported as one that was
/// never added.
fn hiding_rule(root: &Path, path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("check-ignore")
        .args(["-v", "--"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // `<source>:<line>:<pattern>\t<path>`; the pattern is what decides.
    let rule = text.lines().next()?.split('\t').next()?.trim();
    if rule.rsplit(':').next()?.starts_with('!') {
        return None;
    }
    Some(rule.to_string())
}

/// Whether the repository root is a git checkout at all.
///
/// A source archive has no index to ask, and this test has nothing to say about
/// one; a checkout that is missing its fixtures is exactly what it is for.
fn is_a_checkout(root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true"
        })
        .unwrap_or(false)
}

#[test]
fn every_fixture_the_golden_suites_read_is_tracked() {
    let root = repository_root();
    if !is_a_checkout(&root) {
        println!(
            "skipped: {} is not a git checkout, so there is no index to ask",
            root.display()
        );
        return;
    }

    let fixtures = fixtures(&root);
    // The walk has to have found the tree, not a mistyped path: a test that
    // checks nothing passes for the wrong reason.
    assert!(
        fixtures.len() >= 150,
        "the walk found {} fixture(s), where the tree it reads holds more than 150 — a \
         directory it walks has moved",
        fixtures.len()
    );

    let tracked = tracked(&root);
    let mut untracked = Vec::new();
    for fixture in &fixtures {
        let relative = fixture
            .strip_prefix(&root)
            .unwrap_or_else(|_| panic!("{} is not under the repository", fixture.display()));
        if tracked.contains(relative) {
            continue;
        }
        untracked.push(match hiding_rule(&root, relative) {
            Some(rule) => format!(
                "{}: hidden by `{rule}` — a fresh checkout would not have it",
                relative.display()
            ),
            None => format!(
                "{}: not in the index — it has never been added",
                relative.display()
            ),
        });
    }

    assert!(
        untracked.is_empty(),
        "{} file(s) the golden suites read are not tracked by git:\n  {}\n\
         Commit them, or fix the rule that hides them: a fixture a suite reads belongs in \
         the repository, whether it is hand-written or written by a `test-data/make_*.py`.",
        untracked.len(),
        untracked.join("\n  ")
    );
}
