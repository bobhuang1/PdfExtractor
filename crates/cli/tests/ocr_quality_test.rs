//! OCR quality over photographed pages: a score per page, and a floor under it.
//!
//! The plain image fixture is a clean scan; a photograph is not. `test-data/photos/`
//! holds five synthetic photographs of rendered pages — turned a few degrees, lit
//! unevenly, one soft enough to be at the edge of what Tesseract reads — and
//! `corpus.json` beside them holds the exact lines each page was drawn from. This
//! test converts every page through the real binary (`--ocr auto`), compares the
//! recognised text with those lines, and prints the report:
//!
//! ```text
//! cargo test -p pdfextractor-cli --test ocr_quality_test -- --nocapture
//! ```
//!
//! Two numbers are reported for each page. **Characters** is one minus the
//! character edit distance divided by the reference's length: the share of the
//! page's characters the reading got right, counting an insertion, a deletion or a
//! substitution as one error. **Words** is the same measure over whitespace-split
//! tokens, after both sides are lower-cased and their whitespace collapsed. A lost
//! space merges two tokens, which costs two word edits — one token disappears and
//! the other becomes the merged pair — while punctuation inside a token is compared.
//! The aggregate row is weighted by the reference's own length, not an average of
//! percentages, so a short page cannot outvote a long one.
//!
//! The floors are the regression. They are recorded from a run that was read and
//! accepted, with a margin: a change that costs a clean page a couple of characters,
//! or the hardest page more than a few points, fails here and prints what was read
//! beside what the page says. Regenerate the corpus with
//! `~/.venvs/pdfextractor-docling/bin/python test-data/make_photo_corpus.py`; its
//! ground truth is written with it, so the images and the reference cannot drift
//! apart.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

/// The floors, per page: `(file, characters, words)`.
///
/// The three sharp pages are read perfectly today; the slightly soft shadow page is
/// read perfectly too, and the softest page (smaller text, blur, lighting and JPEG)
/// sits at about 95.6% characters and 87% words. The margins below are the room a
/// different Tesseract build or a reordered line is allowed without being called a
/// regression.
const FLOORS: &[(&str, f64, f64)] = &[
    ("01_flat.png", 0.98, 0.98),
    ("02_skew.png", 0.98, 0.98),
    ("03_backlit.png", 0.98, 0.98),
    ("04_shadow.png", 0.98, 0.97),
    ("05_photo.jpg", 0.92, 0.82),
    ("06_rotated.jpg", 0.98, 0.98),
    ("07_sideways.jpg", 0.98, 0.98),
    ("08_upside_down.png", 0.98, 0.98),
];

/// The whole corpus has to hold at least this much of the reference, weighted by
/// the reference's length: a single hard page cannot be traded for a clean one.
const CORPUS_FLOOR: (f64, f64) = (0.96, 0.92);

struct Scratch {
    path: PathBuf,
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

impl Scratch {
    fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pdfe-photo-{}-{}-{tag}",
            std::process::id(),
            unique
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One photographed page: the ground-truth lines it was drawn from, and the
/// condition it was photographed under.
struct Document {
    file: String,
    condition: String,
    lines: Vec<String>,
}

/// The corpus manifest the generator wrote beside the images.
fn corpus() -> Vec<Document> {
    let path = repository_root().join("test-data/photos/corpus.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    let manifest: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
    manifest["documents"]
        .as_array()
        .unwrap_or_else(|| panic!("{} names no documents", path.display()))
        .iter()
        .map(|document| Document {
            file: document["file"].as_str().expect("a file name").to_string(),
            condition: document["condition"]
                .as_str()
                .expect("a condition")
                .to_string(),
            lines: document["lines"]
                .as_array()
                .expect("ground-truth lines")
                .iter()
                .map(|line| line.as_str().expect("a line").to_string())
                .collect(),
        })
        .collect()
}

/// Lower-cased, with every run of whitespace one space: what the two readings can
/// honestly be compared on.
fn normalized(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The Levenshtein distance between two sequences: the fewest insertions,
/// deletions and substitutions that turn one into the other.
fn edit_distance<T: PartialEq>(left: &[T], right: &[T]) -> usize {
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (i, one) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, other) in right.iter().enumerate() {
            let substitution = previous[j] + usize::from(one != other);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// The share of the reference the reading holds, by characters and by words, as
/// `(held / reference length, errors)` for each.
fn score(reference: &str, recognized: &str) -> ((f64, usize, usize), (f64, usize, usize)) {
    let reference = normalized(reference);
    let recognized = normalized(recognized);
    let reference_chars: Vec<char> = reference.chars().collect();
    let recognized_chars: Vec<char> = recognized.chars().collect();
    let reference_words: Vec<&str> = reference.split_whitespace().collect();
    let recognized_words: Vec<&str> = recognized.split_whitespace().collect();
    let char_errors = edit_distance(&reference_chars, &recognized_chars);
    let word_errors = edit_distance(&reference_words, &recognized_words);
    (
        (
            confidence(reference_chars.len(), char_errors),
            char_errors,
            reference_chars.len(),
        ),
        (
            confidence(reference_words.len(), word_errors),
            word_errors,
            reference_words.len(),
        ),
    )
}

/// One minus the errors over the reference's length, never below zero: a reading
/// that invented more than the page had still holds none of it.
fn confidence(length: usize, errors: usize) -> f64 {
    if length == 0 {
        return 1.0;
    }
    (1.0 - errors as f64 / length as f64).max(0.0)
}

fn floor_of(file: &str) -> (f64, f64) {
    FLOORS
        .iter()
        .find(|(name, _, _)| *name == file)
        .map(|(_, characters, words)| (*characters, *words))
        .unwrap_or_else(|| panic!("no floor recorded for {file}"))
}

#[test]
fn the_photographed_corpus_is_read_to_its_recorded_quality() {
    let documents = corpus();
    assert!(
        documents.len() >= 5,
        "the corpus is the point: {} page(s)",
        documents.len()
    );

    let scratch = Scratch::new("quality");
    let mut failures = Vec::new();
    let mut rows = Vec::new();
    let (mut reference_chars, mut char_errors) = (0usize, 0usize);
    let (mut reference_words, mut word_errors) = (0usize, 0usize);
    let mut total_words = 0usize;

    for document in &documents {
        let source = repository_root()
            .join("test-data/photos")
            .join(&document.file);
        assert!(
            source.is_file(),
            "missing photographed page {}",
            source.display()
        );
        let output = scratch.path.join(format!("{}.json", document.file));

        let run = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
            .args([
                "-i",
                source.to_str().expect("input path is utf-8"),
                "-o",
                output.to_str().expect("output path is utf-8"),
                "--ocr",
                "auto",
            ])
            .env("RUST_LOG", "warn")
            .output()
            .expect("spawn the CLI binary");
        assert!(
            run.status.success(),
            "{}: the CLI failed\nstdout={}\nstderr={}",
            document.file,
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );

        let text = std::fs::read_to_string(&output).unwrap_or_else(|e| {
            panic!(
                "{}: missing output {}: {e}",
                document.file,
                output.display()
            )
        });
        let doc: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{}: output is not JSON: {e}", document.file));

        // An image is one page, whatever its text says; a page count here would
        // make a text quality report silently accept a document cut in two.
        assert_eq!(
            doc["pages"].as_object().map(|pages| pages.len()),
            Some(1),
            "{}: one photograph is one page",
            document.file
        );
        assert!(
            doc["origin"]["mimetype"]
                .as_str()
                .is_some_and(|mime| mime.starts_with("image/")),
            "{}: not read as an image: {}",
            document.file,
            doc["origin"]["mimetype"]
        );

        let recognized = doc["texts"]
            .as_array()
            .map(|texts| {
                texts
                    .iter()
                    .map(|item| item["text"].as_str().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        assert!(
            !document.lines.is_empty(),
            "{}: a page with no ground truth cannot be scored",
            document.file
        );
        let reference = document.lines.join(" ");
        let ((characters, char_misses, reference_length), (words, word_misses, word_count)) =
            score(&reference, &recognized);
        let (char_floor, word_floor) = floor_of(&document.file);

        reference_chars += reference_length;
        char_errors += char_misses;
        reference_words += word_count;
        word_errors += word_misses;
        total_words += word_count;

        if characters < char_floor || words < word_floor {
            failures.push(format!(
                "{} ({}): {:.1}% characters, {:.1}% words — floor {:.1}% / {:.1}%\n  \
                 expected: {}\n  read:     {}",
                document.file,
                document.condition,
                characters * 100.0,
                words * 100.0,
                char_floor * 100.0,
                word_floor * 100.0,
                normalized(&reference),
                normalized(&recognized),
            ));
        }
        rows.push((
            document.file.clone(),
            document.condition.clone(),
            characters,
            words,
            char_floor,
            word_floor,
        ));
    }

    let corpus_characters = confidence(reference_chars, char_errors);
    let corpus_words = confidence(reference_words, word_errors);

    println!(
        "\nphotographed OCR quality — {} page(s), {} reference words",
        rows.len(),
        total_words
    );
    println!(
        "{:<16} {:>8} {:>8}  {:<46} {:>7}",
        "document", "chars", "words", "condition", "floor"
    );
    for (file, condition, characters, words, char_floor, word_floor) in &rows {
        println!(
            "{file:<16} {:>7.1}% {:>7.1}%  {condition:<46} {:>3.0}/{:<3.0}",
            characters * 100.0,
            words * 100.0,
            char_floor * 100.0,
            word_floor * 100.0,
        );
    }
    println!(
        "{:<16} {:>7.1}% {:>7.1}%  {:<46} {:>3.0}/{:<3.0}",
        "aggregate",
        corpus_characters * 100.0,
        corpus_words * 100.0,
        format!("weighted over {} pages", rows.len()),
        CORPUS_FLOOR.0 * 100.0,
        CORPUS_FLOOR.1 * 100.0,
    );

    if corpus_characters < CORPUS_FLOOR.0 || corpus_words < CORPUS_FLOOR.1 {
        failures.push(format!(
            "the corpus as a whole: {:.1}% characters, {:.1}% words — floor {:.1}% / {:.1}%",
            corpus_characters * 100.0,
            corpus_words * 100.0,
            CORPUS_FLOOR.0 * 100.0,
            CORPUS_FLOOR.1 * 100.0,
        ));
    }

    assert!(
        failures.is_empty(),
        "OCR quality fell below the recorded floors:\n\n{}",
        failures.join("\n\n")
    );
}

/// Convert one of the photographed pages through the real binary and return
/// its JSON.
fn convert_photo(scratch: &Scratch, file: &str) -> Value {
    let source = repository_root().join("test-data/photos").join(file);
    assert!(source.is_file(), "missing {}", source.display());
    let output = scratch.path.join(format!("{file}.json"));

    let run = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            source.to_str().expect("input path is utf-8"),
            "-o",
            output.to_str().expect("output path is utf-8"),
            "--ocr",
            "auto",
        ])
        .env("RUST_LOG", "warn")
        .output()
        .expect("spawn the CLI binary");
    assert!(
        run.status.success(),
        "{file}: the CLI failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    let text = std::fs::read_to_string(&output)
        .unwrap_or_else(|e| panic!("{file}: missing {}: {e}", output.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{file}: output is not JSON: {e}"))
}

/// The recognised text and the page size of a converted photograph.
fn reading(doc: &Value) -> (String, f64, f64) {
    let size = &doc["pages"]["1"]["size"];
    let width = size["width"].as_f64().expect("a width");
    let height = size["height"].as_f64().expect("a height");
    let text = doc["texts"]
        .as_array()
        .expect("a text list")
        .iter()
        .map(|item| item["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    (text, width, height)
}

/// Every text box is horizontal and inside the page: the only way for a scan
/// to satisfy both is for recognition to have seen upright pixels, measured on
/// the upright page.
fn assert_upright_boxes(doc: &Value, width: f64, height: f64) {
    for item in doc["texts"].as_array().expect("a text list") {
        let bbox = &item["prov"][0]["bbox"];
        let (l, t, r, b) = (
            bbox["l"].as_f64().unwrap(),
            bbox["t"].as_f64().unwrap(),
            bbox["r"].as_f64().unwrap(),
            bbox["b"].as_f64().unwrap(),
        );
        assert!(
            r - l > b - t,
            "a sideways line box was scaled with the stored grid: {bbox}"
        );
        assert!(
            l >= 0.0 && t >= 0.0 && r <= width + 0.5 && b <= height + 0.5,
            "box outside the page: {bbox}"
        );
    }
}

/// A camera held sideways stores the page on its side and says in EXIF how far
/// to turn it back — a tag Tesseract's own loader does not read. The reading
/// has to turn the pixels first, or the lines come back as noise; this pins
/// that turn and the portrait page the lines are measured on.
#[test]
fn a_sideways_camera_scan_is_read_upright() {
    let scratch = Scratch::new("rotated");
    let doc = convert_photo(&scratch, "06_rotated.jpg");

    // The stored grid is 1754 x 1240; EXIF orientation 6 makes the page A4
    // portrait, the same size as the other photographed pages.
    let (recognized, width, height) = reading(&doc);
    assert!(
        width < height,
        "the page is portrait, not the stored landscape grid: {width} x {height}"
    );
    assert!(
        (width - 595.2).abs() < 0.5 && (height - 841.92).abs() < 0.5,
        "the A4 page of a 150 dpi scan: {width} x {height}"
    );
    for line in [
        "ACME SUPPLIES",
        "Invoice 12345",
        "Total Due 42.50",
        "Thank you for your business.",
    ] {
        assert!(
            recognized.contains(line),
            "missing {line:?} in the reading: {recognized}"
        );
    }
    assert_upright_boxes(&doc, width, height);
}

/// A page with **no EXIF tag** says nothing about which way up it is, and
/// Tesseract reads the stored pixels whatever they show. The detector has to
/// find the quarter turn from the pixels alone, and the page, its boxes and the
/// pixels the engine saw all have to agree on the upright page.
#[test]
fn a_sideways_scan_with_no_exif_tag_is_read_upright() {
    let scratch = Scratch::new("untagged");
    let source = repository_root().join("test-data/photos/07_sideways.jpg");
    let bytes = std::fs::read(&source).expect("read the fixture");
    assert!(
        !bytes.windows(6).any(|window| window == b"Exif\0\0"),
        "{} carries an EXIF tag, so it does not test detection",
        source.display()
    );

    let doc = convert_photo(&scratch, "07_sideways.jpg");
    let (recognized, width, height) = reading(&doc);

    // The stored grid is 1754 x 1240 — landscape — and the page that comes
    // back is A4 portrait, the same as the tagged page beside it.
    assert!(
        width < height,
        "the stored landscape grid was not turned: {width} x {height}"
    );
    assert!(
        (width - 595.2).abs() < 0.5 && (height - 841.92).abs() < 0.5,
        "the A4 page of a 150 dpi scan: {width} x {height}"
    );
    for line in [
        "ACME SUPPLIES",
        "Invoice 12345",
        "Total Due 42.50",
        "Thank you for your business.",
    ] {
        assert!(
            recognized.contains(line),
            "missing {line:?} in the reading: {recognized}"
        );
    }
    assert_upright_boxes(&doc, width, height);
}

/// The half turn has no tag either, and it is the harder one: upside-down text
/// still has horizontal baselines, so Tesseract reads it as other words rather
/// than as a page that needs turning.
#[test]
fn an_upside_down_scan_with_no_exif_tag_is_read_upright() {
    let scratch = Scratch::new("upside-down");
    let source = repository_root().join("test-data/photos/08_upside_down.png");
    let bytes = std::fs::read(&source).expect("read the fixture");
    assert!(
        !bytes.windows(6).any(|window| window == b"Exif\0\0"),
        "{} carries an EXIF tag, so it does not test detection",
        source.display()
    );

    let doc = convert_photo(&scratch, "08_upside_down.png");
    let (recognized, width, height) = reading(&doc);
    assert!(width < height, "the page is portrait: {width} x {height}");
    for line in [
        "CITY LIBRARY",
        "Opening hours",
        "Closed on public holidays",
        "Please return books on time.",
    ] {
        assert!(
            recognized.contains(line),
            "missing {line:?} in the reading: {recognized}"
        );
    }
    assert_upright_boxes(&doc, width, height);
}

/// The metric itself, on strings whose scores are countable by hand: a clean copy
/// holds everything, one substituted character costs exactly one character (and its
/// token one word), and a lost space costs two word edits because two tokens became
/// one: a deletion and a substitution.
#[test]
fn the_score_counts_characters_and_words_as_documented() {
    let ((characters, char_errors, _), (words, word_errors, _)) =
        score("Total Due 42.50", "Total Due 42.50");
    assert_eq!(
        (characters, char_errors, words, word_errors),
        (1.0, 0, 1.0, 0)
    );

    let ((characters, char_errors, _), (words, word_errors, _)) =
        score("Total Due 42.50", "Total Due 42.5O");
    assert_eq!(char_errors, 1);
    assert!((characters - 14.0 / 15.0).abs() < 1e-9, "{characters}");
    // The changed character also makes its *word* wrong, though the other two
    // tokens still count.
    assert_eq!(word_errors, 1);
    assert!((words - 2.0 / 3.0).abs() < 1e-9, "{words}");

    let ((characters, _, _), (words, word_errors, _)) =
        score("Please return books", "Please returnbooks");
    assert!(characters <= 1.0);
    assert_eq!(
        word_errors, 2,
        "a lost space deletes one token and merges another"
    );
    assert!((words - 1.0 / 3.0).abs() < 1e-9, "{words}");

    let ((characters, _, _), (words, _, _)) = score("abc", "");
    assert_eq!((characters, words), (0.0, 0.0));
}
