//! End-to-end tests of `--to chunks`.
//!
//! The real `pdfextractor-cli` binary runs as a child process and its JSON Lines
//! output is read back line by line, the way a retrieval pipeline would read it:
//! every line has to be one whole record, and every record has to name the
//! document item it came from.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Workspace {
    path: PathBuf,
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

impl Workspace {
    fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pdfe-chunks-cli-{}-{unique}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self { path }
    }

    fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.path.join(relative);
        std::fs::write(&path, contents).expect("write fixture");
        path
    }

    fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args(args)
        .env("RUST_LOG", "info")
        .output()
        .expect("spawn the CLI binary")
}

fn run_ok(args: &[&str]) -> Output {
    let out = run(args);
    assert!(
        out.status.success(),
        "CLI failed: args={args:?}\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Every line of `text` as a chunk record, failing on a line that is not one.
fn records(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("not a JSON record ({e}): {line}"))
        })
        .collect()
}

fn records_of(path: &PathBuf) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("missing output {}: {e}", path.display()));
    records(&text)
}

const DOC: &[u8] = b"# Annual Report\n\nRevenue grew by 9%. Every region contributed.\n\n## Outlook\n\nThe next quarter is uncertain.\n";

const META_SCHEMA: &str = "docling_core.transforms.chunker.DocMeta";

/// One of the tiny real tokenizer files. The Hugging Face model splits
/// `abcdefgh` into more tokens than the approximation, so a budget measured by
/// it behaves differently; the SentencePiece and tiktoken files are the same
/// vocabulary in the other two accepted formats.
fn tokenizer_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data")
        .join(name)
}

/// `--to chunks` writes `<stem>.jsonl` beside the input, one whole record a
/// line, each naming the item and page it came from.
#[test]
fn the_chunks_are_written_beside_the_document_as_json_lines() {
    let ws = Workspace::new("beside");
    let input = ws.write("report.md", DOC);

    run_ok(&["-i", input.to_str().unwrap(), "--to", "chunks"]);

    let path = ws.join("report.jsonl");
    assert!(path.exists(), "no chunk file was written");
    let records = records_of(&path);
    assert!(records.len() >= 3, "only {} records", records.len());
    for record in &records {
        assert!(record["text"].as_str().is_some_and(|text| !text.is_empty()));
        let meta = &record["meta"];
        assert_eq!(meta["schema_name"], META_SCHEMA);
        assert_eq!(meta["version"], "1.0.0");
        let items = meta["doc_items"].as_array().expect("doc_items");
        assert!(
            !items.is_empty(),
            "a chunk with no item behind it: {record}"
        );
        assert!(items[0]["self_ref"]
            .as_str()
            .is_some_and(|r| r.starts_with("#/texts/")));
        // A flow document has no page geometry, so its items carry no
        // provenance — the page and box come with a paginated reading, which
        // the unit tests cover; the record's shape is the same either way.
        assert!(items[0]["label"].as_str().is_some());
    }

    // The section's chunk carries the path it sits under, outermost first.
    let outlook = records
        .iter()
        .find(|record| record["text"] == "The next quarter is uncertain.")
        .expect("the paragraph");
    assert_eq!(outlook["meta"]["headings"][0], "Annual Report");
    assert_eq!(outlook["meta"]["headings"][1], "Outlook");
}

/// The hybrid chunker packs the same items to the budget it was given, so a
/// smaller budget makes more chunks and no sentence is cut.
#[test]
fn a_smaller_budget_makes_more_chunks_without_cutting_a_sentence() {
    let ws = Workspace::new("budget");
    let input = ws.write("report.md", DOC);
    let input = input.to_str().unwrap();

    // The same document chunked hierarchically is the comparison: a budget
    // smaller than its sentences has to split them into more chunks.
    run_ok(&["-i", input, "--to", "chunks"]);
    let hierarchical = records_of(&ws.join("report.jsonl")).len();

    run_ok(&[
        "-i",
        input,
        "--to",
        "chunks",
        "--chunks-max-tokens",
        "4",
        "--chunks-type",
        "hybrid",
    ]);

    let records = records_of(&ws.join("report.jsonl"));
    assert!(
        records.len() > hierarchical,
        "hybrid made {} chunks against {hierarchical} items",
        records.len()
    );
    let joined: String = records
        .iter()
        .map(|record| record["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    for sentence in [
        "Revenue grew by 9%.",
        "Every region contributed.",
        "The next quarter is uncertain.",
    ] {
        assert!(joined.contains(sentence), "{sentence:?} was cut apart");
    }
}

/// `-o -` streams the records to stdout and writes no file: one input, one
/// format, nothing else on the stream.
#[test]
fn the_records_can_be_streamed_to_stdout() {
    let ws = Workspace::new("stdout");
    let input = ws.write("report.md", DOC);

    let out = run_ok(&["-i", input.to_str().unwrap(), "--to", "chunks", "-o", "-"]);

    let records = records(&String::from_utf8_lossy(&out.stdout));
    assert!(!records.is_empty());
    assert!(
        !ws.join("report.jsonl").exists(),
        "a file was written beside the stream"
    );
}

/// A real tokenizer file replaces the approximation for hybrid budgets, so the
/// same long-worded document needs more chunks than it did without one.
#[test]
fn a_real_tokenizer_file_counts_the_hybrid_budget() {
    let ws = Workspace::new("tokenizer");
    let input = ws.write(
        "report.md",
        b"# Report\n\nabcdefgh abcdefgh. abcdefgh abcdefgh. abcdefgh abcdefgh.\n",
    );
    let input = input.to_str().unwrap();

    let base = [
        "-i",
        input,
        "--to",
        "chunks",
        "--chunks-type",
        "hybrid",
        "--chunks-max-tokens",
        "12",
    ];
    run_ok(&base);
    let approximate = records_of(&ws.join("report.jsonl")).len();

    let tokenizer = tokenizer_fixture("tokenizer-mini.json");
    let tokenizer = tokenizer.to_str().unwrap();
    let mut with_file = base.to_vec();
    with_file.extend(["--chunks-tokenizer", tokenizer]);
    run_ok(&with_file);
    let real = records_of(&ws.join("report.jsonl")).len();
    assert!(
        real > approximate,
        "the tokenizer made {real} chunk(s), the approximation {approximate}"
    );
}

/// A tokenizer only sets a token budget, which hierarchical chunks — the
/// default — never spend. Passing one without `--chunks-type hybrid` warns and
/// still writes the chunks, and a hybrid run says nothing.
#[test]
fn a_tokenizer_without_hybrid_chunks_is_warned_about() {
    let ws = Workspace::new("tokenizer-hierarchical");
    let input = ws.write("report.md", b"# Report\n\nabcdefgh abcdefgh.\n");
    let tokenizer = tokenizer_fixture("tokenizer-mini.json");

    let out = run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "chunks",
        "--chunks-tokenizer",
        tokenizer.to_str().unwrap(),
    ]);
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        logs.contains("--chunks-tokenizer") && logs.contains("hierarchical chunks ignore it"),
        "the run did not warn that hierarchical chunks ignore the tokenizer: {logs}"
    );
    assert!(
        !records_of(&ws.join("report.jsonl")).is_empty(),
        "the warned run wrote no chunks"
    );

    let out = run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "chunks",
        "--chunks-type",
        "hybrid",
        "--chunks-max-tokens",
        "8",
        "--chunks-tokenizer",
        tokenizer.to_str().unwrap(),
    ]);
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !logs.contains("hierarchical chunks ignore it"),
        "a hybrid run warned about a tokenizer it uses: {logs}"
    );
}

/// Every accepted file format is loaded by the real binary and used for a
/// hybrid run, not just the Hugging Face JSON the other tests exercise.
#[test]
fn every_supported_tokenizer_file_is_accepted() {
    for (name, tag) in [
        ("tokenizer-mini.json", "hf"),
        ("tokenizer-mini.model", "sentencepiece"),
        ("tokenizer-mini.tiktoken", "tiktoken"),
    ] {
        let ws = Workspace::new(tag);
        let input = ws.write(
            "report.md",
            b"# Report\n\nabcdefgh abcdefgh abcdefgh abcdefgh.\n",
        );
        let tokenizer = tokenizer_fixture(name);
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "--to",
            "chunks",
            "--chunks-type",
            "hybrid",
            "--chunks-max-tokens",
            "8",
            "--chunks-tokenizer",
            tokenizer.to_str().unwrap(),
        ]);
        let records = records_of(&ws.join("report.jsonl"));
        assert!(!records.is_empty(), "{name} wrote no chunks");
    }
}

/// A `.tiktoken` file whose name names no family is loaded with the family the
/// caller states, rather than silently assuming cl100k_base.
#[test]
fn an_explicit_encoding_family_names_a_tiktoken_vocabulary() {
    let ws = Workspace::new("ambiguous-tiktoken");
    let input = ws.write("report.md", b"# Report\n\nabcdefgh abcdefgh abcdefgh.\n");
    let vocabulary = std::fs::read(tokenizer_fixture("tokenizer-mini.tiktoken"))
        .expect("read the tiktoken fixture");
    let tokenizer = ws.write("vocabulary.tiktoken", &vocabulary);

    for family in ["cl100k_base", "o200k_base", "p50k_base", "r50k_base"] {
        run_ok(&[
            "-i",
            input.to_str().unwrap(),
            "--to",
            "chunks",
            "--chunks-type",
            "hybrid",
            "--chunks-max-tokens",
            "8",
            "--chunks-tokenizer",
            tokenizer.to_str().unwrap(),
            "--chunks-tokenizer-encoding",
            family,
        ]);
        assert!(
            !records_of(&ws.join("report.jsonl")).is_empty(),
            "{family} wrote no chunks"
        );
    }
}

/// An encoding family is part of a tokenizer: naming one without a tokenizer,
/// or for a file that is not a tiktoken vocabulary, fails before conversion.
#[test]
fn an_encoding_family_needs_a_tiktoken_vocabulary() {
    let ws = Workspace::new("bad-encoding");
    let input = ws.write("report.md", DOC);

    let missing = run(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "chunks",
        "--chunks-type",
        "hybrid",
        "--chunks-tokenizer-encoding",
        "cl100k_base",
    ]);
    assert!(
        !missing.status.success(),
        "an encoding family without a tokenizer was accepted"
    );
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(
        stderr.contains("--chunks-tokenizer-encoding"),
        "the refusal does not name the flag: {stderr}"
    );
    assert!(
        !ws.join("report.jsonl").exists(),
        "a refused run wrote a chunk file"
    );

    let json = tokenizer_fixture("tokenizer-mini.json");
    let wrong = run(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "chunks",
        "--chunks-type",
        "hybrid",
        "--chunks-tokenizer",
        json.to_str().unwrap(),
        "--chunks-tokenizer-encoding",
        "o200k_base",
    ]);
    assert!(
        !wrong.status.success(),
        "an encoding family was accepted for a Hugging Face file"
    );
    let stderr = String::from_utf8_lossy(&wrong.stderr);
    assert!(
        stderr.contains("--chunks-tokenizer-encoding") && stderr.contains("tiktoken"),
        "the refusal does not say why: {stderr}"
    );
    assert!(
        !ws.join("report.jsonl").exists(),
        "a refused run wrote a chunk file"
    );
}

/// A tokenizer that cannot be loaded is a broken configuration: the run fails
/// before converting anything, names the flag, and writes no chunk file.
#[test]
fn an_unreadable_tokenizer_is_refused_before_conversion() {
    let ws = Workspace::new("bad-tokenizer");
    let input = ws.write("report.md", DOC);
    let bad = ws.write("not-a-tokenizer.json", b"this is not JSON");

    let out = run(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "chunks",
        "--chunks-type",
        "hybrid",
        "--chunks-tokenizer",
        bad.to_str().unwrap(),
    ]);

    assert!(!out.status.success(), "a broken tokenizer was accepted");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--chunks-tokenizer"),
        "the refusal does not name the flag: {stderr}"
    );
    assert!(
        !ws.join("report.jsonl").exists(),
        "a refused run wrote a chunk file"
    );
}

/// A tokenizer path that names no file is refused before conversion, with the
/// path carried in the error: a typo is not a run that reads the whole document
/// and only then fails at the budget.
#[test]
fn a_nonexistent_tokenizer_path_is_refused_before_conversion() {
    let ws = Workspace::new("missing-tokenizer");
    let input = ws.write("report.md", DOC);
    let missing = ws.join("no-such-tokenizer.json");

    let out = run(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "chunks",
        "--chunks-type",
        "hybrid",
        "--chunks-tokenizer",
        missing.to_str().unwrap(),
    ]);

    assert!(
        !out.status.success(),
        "a nonexistent tokenizer path was accepted"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--chunks-tokenizer"),
        "the refusal does not name the flag: {stderr}"
    );
    assert!(
        stderr.contains(missing.to_str().unwrap()),
        "the refusal does not name the path: {stderr}"
    );
    assert!(
        !ws.join("report.jsonl").exists(),
        "a refused run wrote a chunk file"
    );
}

/// Two formats asked for are written as siblings: the JSON document and the
/// chunks, under the same stem.
#[test]
fn json_and_chunks_are_written_as_siblings() {
    let ws = Workspace::new("siblings");
    let input = ws.write("report.md", DOC);

    run_ok(&[
        "-i",
        input.to_str().unwrap(),
        "--to",
        "json",
        "--to",
        "chunks",
    ]);

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("report.json")).expect("read json"))
            .expect("the JSON document");
    assert_eq!(json["schema_name"], "DoclingDocument");
    assert!(!records_of(&ws.join("report.jsonl")).is_empty());
}

/// A chunk budget that cannot be met is a broken configuration: the run refuses
/// before converting anything, and writes nothing.
#[test]
fn an_impossible_budget_is_refused_before_conversion() {
    let ws = Workspace::new("refused");
    let input = ws.write("report.md", DOC);

    for (args, expected) in [
        (&["--chunks-max-tokens", "0"][..], "--chunks-max-tokens"),
        (
            &["--chunks-max-tokens", "8", "--chunks-overlap-tokens", "8"][..],
            "--chunks-overlap-tokens",
        ),
    ] {
        let mut argv = vec!["-i", input.to_str().unwrap(), "--to", "chunks"];
        argv.extend(args);
        let out = run(&argv);
        assert!(!out.status.success(), "the run accepted {args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(expected),
            "stderr did not name {expected}: {stderr}"
        );
        assert!(
            !ws.join("report.jsonl").exists(),
            "a refused run wrote a file"
        );
    }
}
