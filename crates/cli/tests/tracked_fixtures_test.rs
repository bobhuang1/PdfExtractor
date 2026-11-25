//! Every fixture under `test-data/` is in the git index.
//!
//! The suites read files rather than name them in code: a reference is anything
//! in `test-data/golden/docling/`, a render golden is anything under the four
//! format directories, and the document behind a golden is the one beside the
//! corpus with the same stem. That is what makes the tree work — and it is also
//! why a fixture can be *present* on the machine that wrote it and absent from
//! the repository: a rule in `.gitignore` hides it, `git status` never mentions
//! it, and a fresh checkout runs the suites with a document or a reference
//! missing. This test is the check that cannot happen silently: every file in
//! `test-data/` has to be in the index — not only the golden directories — and
//! one that is not fails here naming the rule that hid it, or saying it was
//! never added.
//!
//! It holds the other half of the same promise. A golden is named after the
//! document it pins, so the document is part of the fixture: a golden whose
//! document is nowhere in the tree fails here as well, rather than being left to
//! whichever suite happens to look for it first.
//!
//! It knows nothing about *why* a file is not tracked; that is the point. A file
//! in `test-data/` belongs in the repository, whether it is hand-written or
//! generated — the fixtures written by `test-data/make_*.py` are committed on
//! purpose. The exceptions are the files the repository deliberately ignores as
//! litter rather than source: interpreter bytecode, and the JSON and `*_images/`
//! directories a hand-run conversion leaves beside a document. Ignored files are
//! still fixtures inside the directories a suite reads whole, so a rule that
//! hides one cannot pass as litter.

use std::collections::{BTreeMap, BTreeSet};
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

/// Every golden's stem, and the goldens that carry it.
///
/// A golden is named after the document it pins — `<document>.<extension>` in
/// the golden directory — which is what lets a fixture be found as a file rather
/// than named in code. One document therefore has several goldens (its Markdown,
/// its HTML, its text, its YAML and its reference), and every one of them agrees
/// on the stem it is looking for.
fn goldens_by_stem(root: &Path) -> BTreeMap<String, Vec<PathBuf>> {
    let mut goldens: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for directory in RENDER_GOLDENS {
        for path in files_in(&root.join(directory)) {
            if let Some(stem) = path.file_stem().and_then(|name| name.to_str()) {
                goldens.entry(stem.to_string()).or_default().push(path);
            }
        }
    }
    for reference in files_in(&root.join("test-data/golden/docling")) {
        if let Some(stem) = reference.file_stem().and_then(|name| name.to_str()) {
            goldens.entry(stem.to_string()).or_default().push(reference);
        }
    }
    goldens
}

/// The goldens whose document is nowhere in the tree, as the stem they are named
/// after and the goldens that carry it.
///
/// The document is the other half of the fixture, so a golden whose document is
/// absent — deleted, renamed, or never committed — is a fixture missing from the
/// tree whether or not a suite trips over it first.
fn goldens_without_a_document(root: &Path) -> Vec<(String, Vec<PathBuf>)> {
    goldens_by_stem(root)
        .into_iter()
        .filter(|(stem, _)| document_named(root, stem).is_none())
        .collect()
}

/// Every file under `test-data/`, in a stable order: a suite may read any of
/// them, so any of them can be the fixture that is missing from a checkout.
fn all_files(root: &Path) -> Vec<PathBuf> {
    files_below(&root.join("test-data"))
}

/// Whether `path` is inside a directory a suite reads whole.
///
/// An ignored file there is a fixture a rule hid, not the litter a conversion
/// leaves; outside those directories the repository's ignore rules decide what
/// is output.
fn in_a_read_directory(root: &Path, path: &Path) -> bool {
    READ_WHOLE
        .iter()
        .chain(RENDER_GOLDENS.iter())
        .map(|directory| root.join(directory))
        .chain(std::iter::once(root.join("test-data/golden")))
        .any(|directory| path.starts_with(directory))
}

/// The files under `test-data/` that are present but not in the index, with
/// the rule that hid each one where an ignore rule is responsible.
///
/// Ignored files outside the directories a suite reads whole are the litter a
/// hand-run conversion leaves, which `.gitignore` excludes on purpose; every
/// other file is a fixture that has to be in the repository. The check is by
/// index membership rather than by name, so a fixture a new test starts reading
/// is covered the moment it is written beside the others.
fn untracked_fixtures(root: &Path) -> Vec<String> {
    let tracked = tracked(root);
    let mut untracked = Vec::new();
    for fixture in all_files(root) {
        let relative = fixture
            .strip_prefix(root)
            .unwrap_or_else(|_| panic!("{} is not under the repository", fixture.display()))
            .to_path_buf();
        if tracked.contains(&relative) {
            continue;
        }
        let hiding = hiding_rule(root, &relative);
        if hiding.is_some() && !in_a_read_directory(root, &relative) {
            continue;
        }
        untracked.push(match hiding {
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
    untracked
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
fn every_fixture_under_test_data_is_tracked() {
    let root = repository_root();
    if !is_a_checkout(&root) {
        println!(
            "skipped: {} is not a git checkout, so there is no index to ask",
            root.display()
        );
        return;
    }

    let fixtures = all_files(&root);
    // The walk has to have found the tree, not a mistyped path: a test that
    // checks nothing passes for the wrong reason.
    assert!(
        fixtures.len() >= 400,
        "the walk found {} file(s) under test-data, where the tree holds more than 400 — \
         `test-data/` has moved or the walk stopped early",
        fixtures.len()
    );

    // The other half of a fixture: a golden names its document by stem, so a
    // golden the tree cannot answer for is a fixture missing — the suite that
    // converts it would be the thing that noticed, and this is the test whose
    // subject is the tree rather than the conversion.
    let missing = goldens_without_a_document(&root);
    assert!(
        missing.is_empty(),
        "{} golden(s) name a document the tree does not hold:\n  {}\n\
         A golden is named after the document it pins, so the document belongs in the \
         repository beside the golden: restore it, or take the golden away with it.",
        missing.len(),
        missing
            .iter()
            .map(|(stem, goldens)| {
                let named_by: Vec<String> = goldens
                    .iter()
                    .map(|golden| {
                        golden
                            .strip_prefix(&root)
                            .unwrap_or(golden)
                            .display()
                            .to_string()
                    })
                    .collect();
                format!(
                    "`{stem}`: named by {} — no document with that stem in `test-data/real` \
                     or `test-data`",
                    named_by.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    let untracked = untracked_fixtures(&root);
    assert!(
        untracked.is_empty(),
        "{} file(s) under `test-data/` are not tracked by git:\n  {}\n\
         Commit them, or fix the rule that hides them: a fixture in `test-data/` belongs \
         in the repository, whether it is hand-written or written by a `test-data/make_*.py`.",
        untracked.len(),
        untracked.join("\n  ")
    );
}

/// A fixture written beside the others but never added is what this test is
/// for; the temporary repository holds one tracked and one untracked file, so
/// the walk has to name the untracked one and only it.
#[test]
fn a_fixture_in_the_tree_but_not_in_the_index_is_named() {
    if Command::new("git").arg("--version").output().is_err() {
        println!("skipped: git is not installed, so there is no index to ask");
        return;
    }
    let root = std::env::temp_dir().join(format!("pdfe-tracked-fixtures-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("test-data")).expect("the fixture directory");
    std::fs::write(root.join("test-data/tracked.pdf"), b"tracked").expect("a tracked fixture");
    std::fs::write(root.join("test-data/untracked.pdf"), b"untracked")
        .expect("an untracked fixture");

    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("run git")
    };
    let init = git(&["init", "-q"]);
    assert!(
        init.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let add = git(&["add", "test-data/tracked.pdf"]);
    assert!(
        add.status.success(),
        "git add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    let untracked = untracked_fixtures(&root);
    assert_eq!(untracked.len(), 1, "{untracked:#?}");
    assert!(
        untracked[0].starts_with("test-data/untracked.pdf: not in the index"),
        "the untracked fixture is the one named: {}",
        untracked[0]
    );

    let _ = std::fs::remove_dir_all(&root);
}
