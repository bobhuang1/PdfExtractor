//! The Compare mode over the whole corpus, worst first.
//!
//! `compare_test.rs` looks a few documents over closely; this looks at every
//! document the corpus has a reference for, **in one pass**: a single CLI run
//! over `test-data/real`, then `diff::compare_document` — the function the window
//! itself calls — for each document, one after another. The point is the order of
//! the output: worst first, so the documents worth a look are at the top of the
//! table rather than buried in 37 rows.
//!
//! It ranks rather than judges: the golden suite is the authority on whether a
//! reading is good enough, and this shows what each document's panel would hold.
//! What it asserts is that the summary is *complete* — every reference in
//! `test-data/golden/docling` was compared, every tally adds up to the rows sent,
//! and no document quietly fell out of the run, because a report missing a
//! document is worse than no report — and nothing got **worse**.
//!
//! The second of those is a ratchet. The numbers a run measured are kept in the
//! repository (`test-data/golden/compare-baseline.json`, one entry per document),
//! and a document whose panel now says more `differs`, more one-sided rows, fewer
//! of the reference's characters, or fewer of its items the panel answered as
//! places than that file remembers fails this test and names itself and the count:
//! a change that helps one document and quietly harms another therefore cannot pass. Accepting a trade is a deliberate act —
//! the update below — and the diff of the baseline file is then the argument for
//! it, in front of the reviewer who reads the baseline beside the change.
//!
//! Run it with `--nocapture` to see the table:
//!
//! ```text
//! cargo test -p pdfextractor-ui --test corpus_report_test -- --nocapture
//! ```
//!
//! and with `CORPUS_BASELINE=update` to rewrite the baseline from a run — which
//! says "the panel is knowingly this much worse (or better)" and nothing else:
//!
//! ```text
//! CORPUS_BASELINE=update cargo test -p pdfextractor-ui --test corpus_report_test
//! ```
//!
//! The rule, the numbers it reads, and the *gate* that applies them are not written
//! here: they are [`diff::Numbers`], `Report::hold_to_floor` and [`gate::Session`], in
//! the library — the same Session the window's panel and the command line's
//! `--compare` mode hold their documents through, so the corpus gate, the panel and the
//! build gate are one gate rather than three that can drift. This run is measured the
//! way the baseline says it was measured (`--ocr never`), so the session holds every
//! document to every rule; a run measured otherwise would be held to the floor alone,
//! which is why the baseline's own recorded measurement is checked below before any
//! document is held.

use pdfextractor_ui::diff;
use pdfextractor_ui::gate;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Rows a document needs before its share is worth ranking.
///
/// A document of two blocks is either 0% or 100% disagreement, which says
/// nothing about the reading and everything about the document's size. Those are
/// listed below the table instead of above it.
const MIN_ROWS_TO_RANK: usize = 10;

/// The rows a panel lists (`diff::MAX_ROWS`), past which a document's rows were cut and
/// its per-page picture may hold items its rows no longer name.
const MAX_LISTED_ROWS: usize = 2000;

/// The environment variable that rewrites the baseline instead of checking it.
const BASELINE_UPDATE: &str = "CORPUS_BASELINE";

/// What a reader who opens the baseline file should know before changing it.
const BASELINE_NOTE: &str = "The Compare mode's numbers per document, as measured by crates/ui/tests/\
     corpus_report_test.rs over test-data/real with --ocr never. No document may get worse than this file: \
     more `differs`, more `oursOnly`/`theirsOnly`, a lower `agreement`, fewer of the reference's placed \
     items, or a form line placed as more rows (or drawn as more page pictures) for the same items fails \
     the test. Regenerate deliberately with CORPUS_BASELINE=update and let the diff of this file be the \
     argument. See test-data/golden/README.md.";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The command line binary, or `None` when it has not been built.
fn locate_cli() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PDFEXTRACTOR_CLI") {
        return Some(PathBuf::from(path));
    }
    let candidate = repository_root().join("target/debug/pdfextractor-cli");
    candidate.exists().then_some(candidate)
}

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("pdfe-corpus-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One document, reduced to what the table shows.
struct Line {
    name: String,
    rows: usize,
    same: usize,
    notation: usize,
    differs: usize,
    /// Rows where the reference's item (or the run of them) lies wholly inside one
    /// of our blocks: Docling cuts a paragraph into lines where we keep it whole.
    contained: usize,
    ours_only: usize,
    theirs_only: usize,
    /// Of those, the rows whose reference item has no letters or digits in it at
    /// all (Docling writes an item per rule and per dotted line on a form) *and* was
    /// left one-sided: there is nothing in an item like that to be inside one of our
    /// blocks, so it can only stand alone — and after the placement rule below, only a
    /// document whose reading published no box at all, which is every flow format.
    theirs_only_without_letters: usize,
    /// Rows that stand for a *place* rather than a comparison: the reference's items have
    /// no letters in them at all, and our reading has no block they could be compared
    /// with, so the row says where they lie. Counted apart from `theirs_only`, which means
    /// text we did not read.
    placed: usize,
    /// The items of the reference those rows stand for: the items of one line of a form
    /// are one row, so this is the number a reader would otherwise have to add up from the
    /// rows, and the number that must not look like text we lost when `placed` falls. It
    /// is ratcheted as a floor of its own, because a proportional fall keeps the ratio of
    /// items to rows and a document whose form is answered no more at all is `0` for `0`.
    placed_items: usize,
    /// The boxes the per-page view draws: every placed item on the page it lies on. At
    /// least the items the rows name, and more where a long document's rows were cut to
    /// what a panel lists — the picture is built before the cut.
    placed_boxes: usize,
    /// The pages those boxes are drawn on, one picture each.
    placed_page_pictures: usize,
    joined: usize,
    /// Share of the reference's alphanumeric characters our reading also holds —
    /// the golden suite's own figure, so the two cannot drift apart.
    agreement: f64,
    /// The gate's verdict on this document, in the words the panel refuses with and the
    /// command line fails with: the floor it fell through, when it fell. `None` on every
    /// document at or above the floor — and on one the session held to nothing, which
    /// the ratchet below names separately.
    floor: Option<String>,
}

impl Line {
    fn of(name: &str, report: &diff::Report) -> Line {
        // The document's numbers are the ratchet's own, from the same reduction the window
        // makes ([`diff::Numbers::of`]), so the table, the baseline and the panel cannot
        // come to disagree about what a document says. What is added here is the one
        // column the ratchet does not remember: how many of the reference's letterless
        // items are left one-sided, which is a reading of *this* run rather than a number
        // to hold the next one to.
        let numbers = diff::Numbers::of(report);
        Line {
            name: name.to_string(),
            rows: numbers.rows,
            same: numbers.same,
            notation: numbers.notation,
            differs: numbers.differs,
            contained: numbers.contained,
            ours_only: numbers.ours_only,
            theirs_only: numbers.theirs_only,
            theirs_only_without_letters: report
                .rows
                .iter()
                .filter(|row| row.verdict == diff::Verdict::TheirsOnly)
                .filter(|row| {
                    row.theirs
                        .iter()
                        .all(|block| !block.text.chars().any(char::is_alphanumeric))
                })
                .count(),
            placed: numbers.placed,
            placed_items: numbers.placed_items,
            placed_boxes: numbers.placed_boxes,
            placed_page_pictures: numbers.placed_page_pictures,
            joined: numbers.joined,
            agreement: numbers.agreement,
            // The gate has already held this report (`gate::Session::compare`), so the
            // complaint it earned — the same sentence the panel refuses with and the
            // command line fails with — rides the line into the ratchet below rather
            // than being recomputed there.
            floor: report.floor.clone(),
        }
    }

    /// The share of rows where the reference does not simply agree with us: a
    /// row the two readings disagree about, or one only we have.
    ///
    /// This is what orders the table, and it leaves two things out on purpose.
    /// Rows only the *reference* has are not counted, because Docling also writes
    /// an empty item where it read a picture area and cuts a line differently from
    /// us — what is left as "only the reference" is mostly its own reading rather
    /// than text we missed, and whether text was missed is a question about
    /// characters, which is what the agreement column answers. (What the reference
    /// cut finer *inside* our blocks is counted on its own, as `cont`, and is not
    /// a one-sided row at all.) Notation rows are not counted either: both readings
    /// found the block and the panel is explaining that an equation's spelling is
    /// LaTeX on one side and Unicode on the other, which is not a defect to rank.
    fn ours_plus_differing(&self) -> f64 {
        fraction(self.differs + self.ours_only, self.rows)
    }

    /// Rows that exist because we read something the reference did not.
    fn ours_alone(&self) -> f64 {
        fraction(self.ours_only, self.rows)
    }

    /// Rows where both readings have the block and disagree about it: the label
    /// or the words.
    fn differing(&self) -> f64 {
        fraction(self.differs, self.rows)
    }

    /// Rows that exist because the reference read something we did not.
    fn theirs_alone(&self) -> f64 {
        fraction(self.theirs_only, self.rows)
    }

    /// Rows where the reference cut one of our blocks into finer items: the same
    /// words, and the two readings disagreeing about where an item ends.
    fn cut_finer(&self) -> f64 {
        fraction(self.contained, self.rows)
    }
}

fn fraction(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 / total as f64
    }
}

impl Line {
    /// The numbers the ratchet remembers about this document.
    fn numbers(&self) -> diff::Numbers {
        diff::Numbers {
            rows: self.rows,
            same: self.same,
            notation: self.notation,
            differs: self.differs,
            contained: self.contained,
            ours_only: self.ours_only,
            theirs_only: self.theirs_only,
            placed: self.placed,
            placed_items: self.placed_items,
            placed_boxes: self.placed_boxes,
            placed_page_pictures: self.placed_page_pictures,
            joined: self.joined,
            agreement: self.agreement,
        }
    }

    // `worse_than` — every rule the ratchet applies to a document, the floor among them —
    // and the Session that applies it live in `crates/ui/src/` (`diff.rs` and `gate.rs`),
    // because the panel, the command line's gate and this test all hold a document
    // through the same code, and a rule written twice is a rule that drifts.
}

/// How this test measures the numbers it writes into (and expects from) the baseline.
///
/// Recorded in the baseline itself (`diff::Memory::measured`) so a reader — and the
/// window, which compares its own run's settings against it — can tell what produced the
/// numbers: the corpus and the tool that measured it, and the OCR mode the conversion
/// above ran with. A baseline that says how it was measured can be held against a run
/// made the same way, and one whose recorded settings differ from the run in front of it
/// is a comparison of different readings.
fn measurement() -> diff::Measurement {
    diff::Measurement {
        by: "crates/ui/tests/corpus_report_test.rs".to_string(),
        corpus: "test-data/real".to_string(),
        // `convert_corpus` passes `--ocr never`; this is that flag, named as the
        // memory file names it.
        ocr: "never".to_string(),
        settings: BTreeMap::new(),
    }
}

/// Where the ratchet's memory lives: in the repository, beside the references.
///
/// The same file the window reads, by the same name (`diff::Memory::FILE`) — the window
/// finds it beside the reference folder without being told where it is.
fn baseline_path() -> PathBuf {
    repository_root()
        .join("test-data/golden")
        .join(diff::Memory::FILE)
}

/// The numbers the repository remembers, or a failure that says how to write them.
fn read_baseline(path: &Path) -> diff::Memory {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "no baseline at {} ({error}).\n\
             Create it from a run you have looked at: {BASELINE_UPDATE}=update cargo test -p \
             pdfextractor-ui --test corpus_report_test",
            path.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "{} is not the baseline this test writes: {error}",
            path.display()
        )
    })
}

/// Hold this run against the numbers the repository remembers, or rewrite them.
///
/// Returns every complaint — a document that got worse, one the baseline does not
/// have yet, an entry no document produced — so the caller can fail once with all
/// of them rather than one per run. What "worse" means is not decided here either:
/// the lines arrive already held by the gate ([`gate::Session`]), and the floor a
/// document carries is the sentence the panel refuses with.
fn ratchet(
    lines: &[Line],
    baseline: Option<&diff::Memory>,
    update: bool,
    unbridged: &[String],
) -> Vec<String> {
    let path = baseline_path();
    let now: BTreeMap<String, diff::Numbers> = lines
        .iter()
        .map(|line| (line.name.clone(), line.numbers()))
        .collect();

    if update {
        let mut documents = now;
        // A document this machine could not measure keeps what the file already
        // remembers: rewriting the baseline without LibreOffice must not forget
        // the readings the bridge produced somewhere else.
        if !unbridged.is_empty() {
            if let Some(previous) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<diff::Memory>(&text).ok())
            {
                for name in unbridged {
                    if let Some(numbers) = previous.documents.get(name) {
                        documents
                            .entry(name.clone())
                            .or_insert_with(|| numbers.clone());
                    }
                }
            }
        }
        let baseline = diff::Memory {
            note: BASELINE_NOTE.to_string(),
            measured: measurement(),
            documents,
        };
        let json = serde_json::to_string_pretty(&baseline).expect("the baseline serialises");
        std::fs::write(&path, format!("{json}\n")).expect("write the baseline");
        println!();
        println!(
            "The baseline at {} was rewritten from this run: {} document(s).\n\
             It is now what \"no worse\" means — read its diff, and justify it.",
            path.display(),
            lines.len()
        );
        return Vec::new();
    }

    let baseline = baseline.expect("the baseline was read before the run");
    let mut complaints = Vec::new();
    let mut worse = Vec::new();

    // The baseline has to say it was measured the way this run was measured: a file
    // whose recorded settings are not these settings is a record of a different run,
    // and holding this run to its numbers would be holding two readings to one another.
    if baseline.measured != measurement() {
        complaints.push(format!(
            "{}: measured as {:?}, but this run measures {:?} — accept the file's own \
             measurement or regenerate it with {BASELINE_UPDATE}=update",
            diff::Memory::FILE,
            baseline.measured,
            measurement()
        ));
    }
    for line in lines {
        // Held already, by the same Session the panel and the command line's gate use:
        // what is collected here is that gate's own sentence for a document that fell,
        // not a second opinion from the rules. A document the baseline does not have is
        // not a fall — there is nothing to fall from — but it is a gap in the file, and
        // the corpus fails on gaps as well as on falls.
        if baseline.remember(&line.name).is_some() {
            if let Some(floor) = &line.floor {
                worse.push(format!("{}: {floor}", line.name));
            }
        } else {
            complaints.push(format!(
                "{}: not in the baseline (a new document: {BASELINE_UPDATE}=update to add it)",
                line.name
            ));
        }
    }
    for name in baseline.documents.keys() {
        // A document this machine could not measure — a legacy `.doc` with no
        // LibreOffice to read it through — is not a gap in the file: its
        // numbers are remembered from the run that had the bridge, and this run
        // has no reading of the same kind to hold them to.
        if !now.contains_key(name) && !unbridged.contains(name) {
            complaints.push(format!(
                "{name}: in the baseline, but this run compared no such document"
            ));
        }
    }
    if !unbridged.is_empty() {
        println!(
            "{} document(s) left out for lack of LibreOffice, their baseline entries kept: {}",
            unbridged.len(),
            unbridged.join(", ")
        );
    }

    println!();
    if worse.is_empty() && complaints.is_empty() {
        println!(
            "Every document is at or better than the baseline it remembers ({}, {} document(s)).",
            path.display(),
            lines.len()
        );
        return Vec::new();
    }
    if !worse.is_empty() {
        println!(
            "{} document(s) now read *worse* than the baseline at {}:",
            worse.len(),
            path.display()
        );
        for line in &worse {
            println!("    {line}");
        }
        println!(
            "Fix the reading, or accept the trade deliberately: {BASELINE_UPDATE}=update cargo \
             test -p pdfextractor-ui --test corpus_report_test"
        );
    }
    if !complaints.is_empty() {
        println!("{} problem(s) with the baseline itself:", complaints.len());
        for line in &complaints {
            println!("    {line}");
        }
    }
    worse.extend(complaints);
    worse
}

/// The documents a reference can be held against, by file stem: the corpus, and
/// the fixtures beside it. A format is held to a real Docling conversion before
/// it joins the corpus (`inventory.xlsx`, the XLSX parser's workbook, lives in
/// `test-data/` with its reference in the same `docling/` folder as the rest),
/// and this ratchet covers every reference the folder holds. The images written
/// beside a document are directories, not documents.
fn corpus_inputs() -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    for directory in ["test-data/real", "test-data"] {
        let directory = repository_root().join(directory);
        for entry in std::fs::read_dir(&directory)
            .expect("read the document directories")
            .flatten()
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Some(stem) = path.file_stem() {
                found
                    .entry(stem.to_string_lossy().into_owned())
                    .or_insert(path);
            }
        }
    }
    found
}

/// Every reference the corpus can be compared against, by stem, in a stable
/// order.
fn references() -> Vec<(String, PathBuf)> {
    let directory = repository_root().join("test-data/golden/docling");
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(&directory)
        .unwrap_or_else(|e| panic!("read {}: {e}", directory.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| {
            let stem = path.file_stem()?.to_string_lossy().into_owned();
            Some((stem, path))
        })
        .collect();
    found.sort();
    found
}

/// Whether a document is a plain image, whose only text comes from OCR.
///
/// This report is measured with `--ocr never` so its numbers do not depend on
/// the machine's tessdata. An image has no text layer at all, so converting one
/// that way would measure an empty page and remember it as such — a baseline
/// entry that says the document has no content. The image's reference is held
/// by the golden suite, which runs it through OCR and compares the text; the
/// ratchet has nothing to measure here.
fn is_plain_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp" | "webp")
    )
}

/// Whether a document is read through LibreOffice, one way or another.
///
/// A legacy `.doc` is the same file read two ways: through Writer when
/// LibreOffice is installed — headings, list markers, merged tables and pictures
/// — and through the built-in piece-table reader when it is not, which reads the
/// words alone. The rest of these extensions have no reader here at all and are
/// the bridge's alone. Their numbers therefore depend on an external converter,
/// so a run without one cannot compare them — the same reason a plain image is
/// skipped, stated for the same kind of fact.
fn is_bridged(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("doc" | "wps" | "wp" | "wpd" | "wks" | "wdb" | "xls" | "ppt")
    )
}

/// Whether the CLI on this machine will reach LibreOffice, by the same rule the
/// CLI itself uses: `PDFE_SOFFICE` naming a file, or `soffice`/`libreoffice` on
/// `PATH`.
fn libreoffice_available() -> bool {
    if let Some(path) = std::env::var_os("PDFE_SOFFICE") {
        return PathBuf::from(path).is_file();
    }
    let search = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&search).any(|directory| {
        ["soffice", "libreoffice"]
            .iter()
            .any(|name| directory.join(name).is_file())
    })
}

/// Convert the whole corpus in one run of the CLI.
///
/// One process rather than 37: the comparison is what this test is about, and a
/// directory conversion is the same code path the window drives when it is
/// pointed at a folder.
fn convert_corpus(cli: &Path, out: &Path) {
    let corpus = repository_root().join("test-data/real");
    let status = Command::new(cli)
        .args(["--input", corpus.to_str().expect("a UTF-8 path")])
        .args(["--output", out.to_str().expect("a UTF-8 path")])
        .args(["--parallel", "4"])
        // Deterministic and fast: with OCR off a document's reading is its text
        // layer and its drawings, and nothing depends on a tessdata build.
        .args(["--ocr", "never"])
        .status()
        .expect("run the CLI over the corpus");
    assert!(status.success(), "the corpus conversion failed: {status}");

    // A reference whose document is a fixture beside the corpus rather than in
    // it (`inventory.xlsx`, the XLSX parser's workbook) is converted on its
    // own, to the same name a directory run would give it — the stem, `.json`
    // — so the comparison finds its output the same way. Only documents that
    // have a reference are converted: the fixture folder holds scripts and
    // sources too.
    let inputs = corpus_inputs();
    for (stem, _) in references() {
        let Some(path) = inputs.get(&stem) else {
            continue;
        };
        if path.starts_with(&corpus) || is_plain_image(path) {
            continue;
        }
        let path = path.clone();
        let output = out.join(format!("{stem}.json"));
        let status = Command::new(cli)
            .args(["-i", path.to_str().expect("a UTF-8 path")])
            .args(["-o", output.to_str().expect("a UTF-8 path")])
            .args(["--ocr", "never"])
            .status()
            .expect("run the CLI over a fixture");
        assert!(
            status.success(),
            "{} did not convert: {status}",
            path.display()
        );
    }
}

/// Compare every corpus document with its reference and print them worst first.
#[test]
fn the_whole_corpus_compared_worst_first() {
    let Some(cli) = locate_cli() else {
        println!("skipped: build the CLI (`cargo build -p pdfextractor-cli`) to run this");
        return;
    };

    let scratch = Scratch::new("report");
    convert_corpus(&cli, &scratch.0);

    let golden = repository_root().join("test-data/golden/docling");
    // The gate, once, for the whole run: the memory beside those references and this
    // run's measurement (`--ocr never`, the same flag the conversion ran with). The
    // baseline is read here rather than inside the ratchet below because the session
    // needs it to hold each document as it arrives — and in update mode there is
    // deliberately no memory to hold against: the run is what the file becomes.
    let update = matches!(std::env::var(BASELINE_UPDATE).as_deref(), Ok("update"));
    let baseline = (!update).then(|| read_baseline(&baseline_path()));
    let session = gate::Session::with_memory(&golden, baseline.clone(), measurement());
    let inputs = corpus_inputs();
    let mut lines = Vec::new();
    let mut missing = Vec::new();
    let mut skipped = Vec::new();
    // The documents this machine could not measure because the bridge is not
    // here: kept apart from the other skips, because the baseline remembers
    // them and this run has nothing to hold them to.
    let mut unbridged = Vec::new();
    for (stem, reference) in references() {
        let Some(input) = inputs.get(&stem) else {
            missing.push(format!("{stem}: the corpus has no such document"));
            continue;
        };
        if is_plain_image(input) {
            println!(
                "skipping {stem}: an image's text needs OCR, and this report is measured with --ocr never"
            );
            skipped.push(stem.clone());
            continue;
        }
        if is_bridged(input) && !libreoffice_available() {
            println!(
                "skipping {stem}: it is read through LibreOffice, and this run has none \
                 (its numbers would be the built-in reader's, not the bridge's)"
            );
            skipped.push(stem.clone());
            unbridged.push(stem.clone());
            continue;
        }
        let ours = scratch.0.join(format!("{stem}.json"));
        if !ours.exists() {
            missing.push(format!("{stem}: the conversion wrote no output"));
            continue;
        }
        let report = session.compare(&input.to_string_lossy(), &ours);
        assert_eq!(
            report.reference,
            reference.display().to_string(),
            "{stem} was compared with the wrong reference"
        );
        assert!(
            report.problem.is_none(),
            "{stem} could not be compared: {:?}",
            report.problem
        );
        lines.push(Line::of(&stem, &report));
    }

    // Worst first. A tie is broken by the reading itself: two documents with the
    // same share of untouched rows are not equally good if one of them holds
    // less of the reference's text.
    let mut ranked: Vec<&Line> = lines
        .iter()
        .filter(|line| line.rows >= MIN_ROWS_TO_RANK)
        .collect();
    ranked.sort_by(|a, b| {
        b.ours_plus_differing()
            .partial_cmp(&a.ours_plus_differing())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.agreement
                    .partial_cmp(&b.agreement)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.name.cmp(&b.name))
    });

    println!();
    println!("The Compare mode over the whole corpus — worst first");
    println!();
    println!("Ordered by the share of rows the reference does not simply agree with");
    println!("(rows that differ + rows only we have). `cont` counts the rows where the");
    println!("reference read an item per line inside one block of ours — the same words,");
    println!("cut differently — so `refs` is text we did not read rather than Docling's");
    println!("finer side; `agree` is the share of the reference's characters we also hold,");
    println!("the golden suite's own figure. `plcd` counts the rows that are not a comparison");
    println!("at all: the reference wrote items with no letters in them — a dotted leader,");
    println!("the rule under it — where our reading read no line, so the row says where they");
    println!("lie rather than standing among the text we did not read. Rows, not items: the");
    println!("items of one line of a form are one row, which names how many it stands for and");
    println!("the extent they cover.");
    println!();
    println!(
        "{:<34} {:>5} {:>5} {:>5} {:>6} {:>5} {:>5} {:>5} {:>5} {:>6} {:>7} {:>9}",
        "document",
        "rows",
        "same",
        "notn",
        "diff",
        "cont",
        "ours",
        "refs",
        "plcd",
        "joined",
        "agree",
        "ours+diff"
    );
    for line in &ranked {
        println!(
            "{:<34} {:>5} {:>5} {:>5} {:>6} {:>6} {:>5} {:>5} {:>5} {:>6} {:>6.1}% {:>8.0}%",
            line.name,
            line.rows,
            line.same,
            line.notation,
            line.differs,
            line.contained,
            line.ours_only,
            line.theirs_only,
            line.placed,
            line.joined,
            line.agreement * 100.0,
            line.ours_plus_differing() * 100.0,
        );
    }

    // Small documents: a share of two rows says nothing.
    let mut small: Vec<&Line> = lines
        .iter()
        .filter(|line| line.rows < MIN_ROWS_TO_RANK)
        .collect();
    small.sort_by(|a, b| a.name.cmp(&b.name));
    if !small.is_empty() {
        println!();
        println!("Not ranked — fewer than {MIN_ROWS_TO_RANK} rows, where a share means nothing");
        for line in &small {
            println!(
                "    {:<32} {} row(s): {} same, {} notation, {} differ, {} contained, {} only ours, {} only the reference, {} placed",
                line.name, line.rows, line.same, line.notation, line.differs, line.contained, line.ours_only, line.theirs_only, line.placed
            );
        }
    }

    // What the order above deliberately leaves out, shown rather than hidden.
    // What the order above deliberately leaves out, shown rather than hidden:
    // the text of the reference's we do not hold, and each half of "not simply
    // agree" on its own. Small documents are left out of these too.
    let block = |title: &str, shape: fn(&Line) -> f64, detail: fn(&Line) -> String| {
        let mut worst: Vec<&Line> = ranked
            .iter()
            .copied()
            .filter(|line| shape(line) > 0.0)
            .collect();
        if worst.is_empty() {
            return;
        }
        worst.sort_by(|a, b| {
            shape(b)
                .partial_cmp(&shape(a))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.name.cmp(&b.name))
        });
        println!();
        println!("{title}");
        for line in worst.iter().take(5) {
            println!(
                "    {:<32} {:<16} {:>5.1}%",
                line.name,
                detail(line),
                shape(line) * 100.0
            );
        }
    };
    block(
        "Lowest character agreement — the reference's text we do not hold",
        |line| 1.0 - line.agreement,
        |line| format!("{:.1}% held", line.agreement * 100.0),
    );
    block(
        "Most rows only the reference has (text we did not read)",
        Line::theirs_alone,
        |line| {
            format!(
                "{} row(s), {} with no letters, {} placed where we read no line \
                 ({} item(s) of the reference)",
                line.theirs_only, line.theirs_only_without_letters, line.placed, line.placed_items
            )
        },
    );
    block(
        "Most rows where the reference cut one of our blocks finer than we did",
        Line::cut_finer,
        |line| format!("{} row(s)", line.contained),
    );
    block(
        "Most rows only we have (a block the reference read and we did not)",
        Line::ours_alone,
        |line| format!("{} row(s)", line.ours_only),
    );
    block(
        "Most differing rows (both readings have the block and disagree about it)",
        Line::differing,
        |line| format!("{} row(s)", line.differs),
    );

    println!();
    let joined: usize = lines.iter().map(|line| line.joined).sum();
    let contained: usize = lines.iter().map(|line| line.contained).sum();
    let theirs_only: usize = lines.iter().map(|line| line.theirs_only).sum();
    let placed: usize = lines.iter().map(|line| line.placed).sum();
    let placed_items: usize = lines.iter().map(|line| line.placed_items).sum();
    let placed_boxes: usize = lines.iter().map(|line| line.placed_boxes).sum();
    let page_pictures: usize = lines.iter().map(|line| line.placed_page_pictures).sum();
    println!(
        "{} document(s) compared, {} without a reading or a reference, {joined} row(s) paired by position,\n\
         {contained} row(s) where the reference cut one of our blocks finer, {theirs_only} row(s) only it has,\n\
         {placed} row(s) placed where our reading read no line, standing for {placed_items} item(s) of\n\
         the reference, drawn as {page_pictures} page(s) holding {placed_boxes} box(es)",
        lines.len(),
        missing.len()
    );
    // The per-page view: a picture per page, each carrying at least one box, and — for
    // every document whose rows were not cut to what a panel lists — no more boxes than
    // the placed items the rows stand for. A row can name a place the reference gave no
    // size for, and those are named and drawn nowhere, which is why the comparison
    // allows the pictures to hold *fewer* boxes than the rows name rather than requiring
    // equality; it may never hold more.
    assert!(
        page_pictures > 0,
        "no page was drawn for the corpus's placed items"
    );
    for line in &lines {
        assert_eq!(
            line.placed_page_pictures == 0,
            line.placed_boxes == 0,
            "{}: a page picture always carries a box",
            line.name
        );
        if line.rows < MAX_LISTED_ROWS {
            assert!(
                line.placed_boxes <= line.placed_items,
                "{}: {} box(es) drawn for {} placed item(s) the rows name",
                line.name,
                line.placed_boxes,
                line.placed_items
            );
        }
    }
    for note in &missing {
        println!("    {note}");
    }

    // The summary is the point of this test, so it must not be a partial one:
    // every reference is either compared or deliberately skipped, never lost.
    assert!(missing.is_empty(), "{missing:?}");
    assert_eq!(
        lines.len() + skipped.len(),
        references().len(),
        "{} compared, {} skipped, {} reference(s) in all",
        lines.len(),
        skipped.len(),
        references().len()
    );
    assert!(lines.len() >= 30, "the corpus is bigger than this");
    for line in &lines {
        assert_eq!(
            line.rows,
            line.same
                + line.notation
                + line.differs
                + line.contained
                + line.ours_only
                + line.theirs_only
                + line.placed,
            "{}: the verdicts do not add up to the rows sent",
            line.name
        );
    }

    // And the ratchet: what this run measured may not be worse, document for
    // document, than what the repository remembers. This is the only judgement this
    // test makes about the readings, and it is a relative one — the golden suite is
    // still the authority on whether they are good enough.
    let regressions = ratchet(&lines, baseline.as_ref(), update, &unbridged);
    assert!(
        regressions.is_empty(),
        "the corpus report is worse than the baseline in {} place(s), listed above.\n\
         The baseline is what the panel said when it was last accepted: a document that now \
         says more `differs`, more one-sided rows, or holds fewer of the reference's \
         characters is a regression to fix — or a trade to accept on purpose, by running \
         `{BASELINE_UPDATE}=update cargo test -p pdfextractor-ui --test corpus_report_test` \
         and letting the diff of test-data/golden/compare-baseline.json be the argument.",
        regressions.len()
    );
}

/// The ratchet's own rules, on documents small enough to hold in the hand.
///
/// The corpus run above can only say *that* nothing got worse; these say what
/// "worse" means for the placed rows a form is answered with, which is the one
/// number in the baseline that is not a count of rows but a *shape* — a line of a
/// form is one row, and un-collapsing it multiplies the rows and leaves the items
/// where they were.
mod placed_rows {
    use super::*;

    /// One document's numbers, with the fields this rule reads set and the rest left at
    /// what a document with no comparisons at all would say.
    ///
    /// [`diff::Numbers`] rather than the table's `Line`: the rule is the library's, and
    /// these are the numbers it is written about — the same ones the window reduces its
    /// own report to when it holds a document to the floor.
    fn line(rows: usize, placed: usize, placed_items: usize) -> diff::Numbers {
        diff::Numbers {
            rows,
            same: 0,
            notation: 0,
            differs: 0,
            contained: 0,
            ours_only: 0,
            theirs_only: 0,
            placed,
            placed_items,
            placed_boxes: placed_items,
            placed_page_pictures: if placed == 0 { 0 } else { 2 },
            joined: 0,
            agreement: 1.0,
        }
    }

    /// Whether a run of `rows` for `items` is worse than one that placed the same
    /// items in `was_rows` rows, by the ratchet's own reading of the pair.
    fn comes_apart(now: (usize, usize), was: (usize, usize)) -> Vec<String> {
        line(now.0 + 100, now.0, now.1).worse_than(&line(was.0 + 100, was.0, was.1))
    }

    #[test]
    fn a_form_line_read_dot_by_dot_again_is_a_regression() {
        // `irs_f1040.pdf` as the placement rule left it: 25 rows for 327 items. A change
        // that stopped collapsing them would say one row per item, with every item still
        // there — no other count in the file moves, which is why this one is watched.
        let worse = comes_apart((327, 327), (25, 327));
        assert_eq!(worse.len(), 1, "{worse:?}");
        assert!(
            worse[0].starts_with("form line(s) placed: 25 row(s) for 327 item(s) → 327 row(s)"),
            "{}",
            worse[0]
        );
    }

    #[test]
    fn a_form_line_collapsed_further_is_an_improvement() {
        // The same items in fewer rows, and the same shape as before: neither is worse.
        assert!(comes_apart((25, 327), (327, 327)).is_empty());
        assert!(comes_apart((25, 327), (25, 327)).is_empty());
        assert!(comes_apart((25, 327), (40, 327)).is_empty());
        // ...and the same items in *more* rows is the regression, however few: forty
        // rows for the IRS form's 327 items is a line that has begun to come apart.
        assert_eq!(comes_apart((40, 327), (25, 327)).len(), 1);
    }

    #[test]
    fn placing_more_items_is_not_a_regression() {
        // The direction the rule must not forbid: a document with nothing placed has
        // nothing to come apart, and a reading that finds *less* places more items — as
        // long as the lines it places them on are lines and not dots.
        assert!(comes_apart((69, 380), (0, 0)).is_empty());
        assert!(comes_apart((120, 800), (60, 369)).is_empty());
        // ...and one that places more items *and* comes apart is still refused.
        assert_eq!(comes_apart((800, 800), (60, 369)).len(), 1);
    }

    #[test]
    fn a_form_whose_placed_items_leave_the_panel_is_a_regression() {
        // The ratio is kept and the panel is halved: half the items in half the rows.
        // The cross-multiplication cannot see it — `160 * 25` is not less than
        // `327 * 12` — so the count itself is the rule, and it names the count.
        let worse = comes_apart((12, 160), (25, 327));
        assert_eq!(worse.len(), 1, "{worse:?}");
        assert!(
            worse[0].starts_with("form item(s) placed: 327 → 160 item(s) in 12 row(s)"),
            "{}",
            worse[0]
        );
    }

    #[test]
    fn a_form_answered_no_more_at_all_is_a_regression_and_not_a_rate() {
        // The limit the rate rule cannot reach: no rows and no items is not a ratio, and
        // `0 * 327 < 25 * 0` is false. The panel is empty where the baseline remembered a
        // form, which is the failure this floor exists for.
        let worse = comes_apart((0, 0), (25, 327));
        assert_eq!(worse.len(), 1, "{worse:?}");
        assert!(
            worse[0].starts_with("form item(s) placed: 327 → 0 item(s) in 0 row(s)"),
            "{}",
            worse[0]
        );
    }

    #[test]
    fn the_item_floor_never_stands_on_a_document_that_placed_nothing() {
        // Both directions of the guard: a document with nothing placed has nothing to
        // lose, and items that only ever rose are the improvement the count must allow.
        assert!(comes_apart((25, 327), (0, 0)).is_empty());
        assert!(comes_apart((25, 400), (25, 327)).is_empty());
        assert!(comes_apart((25, 330), (25, 327)).is_empty());
    }

    #[test]
    fn the_floor_the_window_holds_a_document_to_is_the_rule_here() {
        // The window refuses a document that has fallen through one rule of this ratchet
        // (`Report::hold_to_floor`) rather than through all of them, and that rule is this
        // one: half the items in half the rows, the ratio kept. The sentence the panel
        // carries is the sentence this test fails on, word for word, because both come from
        // `diff::Numbers` — a change to one that leaves the other behind fails here.
        let now = line(112, 12, 160);
        let was = line(125, 25, 327);
        let floor = now
            .below_placed_floor(&was)
            .expect("the items left the panel");
        assert!(
            floor.starts_with("form item(s) placed: 327 → 160 item(s) in 12 row(s)"),
            "{floor}"
        );
        // ...and it is the *only* complaint the halved panel earns: the rate is kept.
        assert_eq!(now.worse_than(&was), vec![floor]);
    }

    #[test]
    fn a_form_pattern_drawn_again_one_thumbnail_per_row_is_a_regression() {
        // The per-page view is the same rule one level up: the same boxes in more
        // pictures means the form's leader pattern is being answered row by row again.
        let mut now = line(500, 25, 327);
        now.placed_page_pictures = 25;
        let was = line(500, 25, 327);
        let worse = now.worse_than(&was);
        assert_eq!(worse.len(), 1, "{worse:?}");
        assert!(
            worse[0]
                .starts_with("form pattern drawn: 2 picture(s) for 327 box(es) → 25 picture(s)"),
            "{}",
            worse[0]
        );
        // Fewer pictures for the same boxes is the improvement the view exists for.
        let mut fewer = line(500, 25, 327);
        fewer.placed_page_pictures = 1;
        assert!(fewer.worse_than(&was).is_empty());
    }
}
