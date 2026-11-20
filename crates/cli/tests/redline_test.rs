//! `--redline` against the real binary.
//!
//! The comparison itself is covered by `pdfextractor_ui::diff`; these tests are
//! about the two-document mode around it: both versions are converted, the
//! report is grouped by the pages the PDFs published, a DOCX is reported as the
//! flow document it is, changed words are marked, and the mode refuses inputs it
//! cannot honestly compare.

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args(args)
        .output()
        .expect("run the CLI binary")
}

fn run_in(cwd: &Path, args: &[String]) -> Output {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run the CLI binary")
}

static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let unique = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pdfe-redline-{}-{unique}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch folder");
        Scratch(path)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("write the fixture");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One or more pages of a small, valid PDF.
///
/// Written here so a test can make two versions whose text differs without
/// depending on a binary fixture that would have to be regenerated. The objects
/// are laid out by hand and the cross-reference table is built from the byte
/// offsets, which is how a PDF states where each object begins.
fn pdf(pages: &[&str]) -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_string());
    let kids: Vec<String> = (0..pages.len())
        .map(|index| format!("{} 0 R", 4 + index * 2))
        .collect();
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        pages.len()
    ));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
    for (index, text) in pages.iter().enumerate() {
        let content_id = 5 + index * 2;
        let stream = format!("BT /F1 18 Tf 72 720 Td ({}) Tj ET", pdf_literal(text));
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>"
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            stream.len(),
            stream
        ));
    }

    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, object));
    }
    let xref = out.len();
    out.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
    out.push_str("0000000000 65535 f \n");
    for offset in offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

/// A copy of `brief.docx` with the given strings replaced in `word/document.xml`.
fn edited_docx_with(source_name: &str, replacements: &[(&str, &str)]) -> Vec<u8> {
    let source = repository_root().join("test-data").join(source_name);
    let mut archive = zip::ZipArchive::new(
        std::fs::File::open(&source)
            .unwrap_or_else(|error| panic!("open {}: {error}", source.display())),
    )
    .unwrap_or_else(|error| panic!("read {} as a ZIP: {error}", source.display()));
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("read a DOCX entry");
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read the entry");
        if name == "word/document.xml" {
            let mut text = String::from_utf8(data).expect("the document part is UTF-8");
            for (from, to) in replacements {
                text = text.replace(from, to);
            }
            data = text.into_bytes();
        }
        let options =
            zip::write::SimpleFileOptions::default().compression_method(entry.compression());
        writer
            .start_file(name, options)
            .expect("start the DOCX entry");
        writer.write_all(&data).expect("write the DOCX entry");
    }
    writer.finish().expect("finish the DOCX").into_inner()
}

fn edited_brief_with(replacements: &[(&str, &str)]) -> Vec<u8> {
    edited_docx_with("brief.docx", replacements)
}

/// A copy of `brief.docx` with a few words changed in `word/document.xml`.
fn edited_brief() -> Vec<u8> {
    edited_brief_with(&[
        (
            "The quarter closed ahead of plan.",
            "The quarter closed behind plan.",
        ),
        (
            "Supply is tight.",
            "Supply is tight, and the next quarter looks harder.",
        ),
    ])
}

/// A PDF literal string: the three characters a literal cannot carry raw are
/// backslash-escaped, so a test may put punctuation in a page.
fn pdf_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' | '(' | ')' => {
                out.push('\\');
                out.push(character);
            }
            _ => out.push(character),
        }
    }
    out
}

fn logs(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn run_redline(old: &Path, new: &Path, report: &Path) -> Output {
    run(&[
        "--input",
        new.to_str().expect("utf-8 path"),
        "--redline",
        old.to_str().expect("utf-8 path"),
        "--output",
        report.to_str().expect("utf-8 path"),
        "--ocr",
        "never",
    ])
}

fn redline_report(old: &Path, new: &Path, report: &Path) -> String {
    let output = run_redline(old, new, report);
    let text = logs(&output);
    assert!(output.status.success(), "redline failed: {text}");
    std::fs::read_to_string(report).expect("the report")
}

fn assert_summary_card(html: &str, class: &str, value: usize, label: &str) {
    let card =
        format!("<div class=\"metric {class}\"><strong>{value}</strong><span>{label}</span></div>");
    assert!(html.contains(&card), "missing summary card {card}: {html}");
}

fn assert_exact_summary_counts(
    html: &str,
    changed: usize,
    added: usize,
    removed: usize,
    placed: usize,
    unchanged: usize,
) {
    assert_summary_card(html, "changed", changed, "changed");
    assert_summary_card(html, "added", added, "added");
    assert_summary_card(html, "removed", removed, "removed");
    assert_summary_card(html, "placed", placed, "placed");
    assert_summary_card(html, "same", unchanged, "unchanged");
}

struct Pair {
    label: &'static str,
    old: &'static [&'static str],
    new: &'static [&'static str],
}

const METAMORPHIC_PAIRS: &[Pair] = &[
    Pair {
        label: "replacement",
        old: &["alpha bravo charlie"],
        new: &["alpha delta charlie"],
    },
    Pair {
        label: "multi-page-addition",
        old: &["one two three", "four five six"],
        new: &["one two three", "four seven six", "eight nine"],
    },
    Pair {
        label: "removal-and-reorder",
        old: &["red green blue"],
        new: &["blue red"],
    },
];

fn pair_words(pages: &[&str]) -> Vec<String> {
    pages
        .iter()
        .flat_map(|page| page.split_whitespace())
        .map(str::to_string)
        .collect()
}

fn sorted_words<'a>(words: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut words: Vec<_> = words.collect();
    words.sort_unstable();
    words
}

/// The words the DOCX package itself carries, in document order.
///
/// The fixture is a real package, so reading `word/document.xml` keeps the
/// expected side independent of the redline report. Only `w:t` text nodes are
/// content; table properties and other elements merely happen to share the
/// `w:t` prefix.
fn docx_words(bytes: &[u8]) -> Vec<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("read the DOCX as a ZIP");
    let mut document = archive
        .by_name("word/document.xml")
        .expect("read the document part");
    let mut xml = String::new();
    document
        .read_to_string(&mut xml)
        .expect("read the document XML");

    let mut words = Vec::new();
    let mut rest = xml.as_str();
    while let Some(start) = rest.find("<w:t") {
        let after_name = &rest[start + "<w:t".len()..];
        if !(after_name.starts_with('>')
            || after_name.starts_with(' ')
            || after_name.starts_with('\t')
            || after_name.starts_with('\r')
            || after_name.starts_with('\n'))
        {
            rest = after_name;
            continue;
        }
        let open = after_name.find('>').expect("a closed w:t tag");
        let body = &after_name[open + 1..];
        let close = body.find("</w:t>").expect("a closed w:t element");
        words.extend(
            unescape_entities(&body[..close])
                .split_whitespace()
                .map(str::to_string),
        );
        rest = &body[close + "</w:t>".len()..];
    }
    words
}

fn row_fragments(html: &str) -> Vec<&str> {
    let mut rows = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<article class=\"row ") {
        let row = &rest[start..];
        let end = row
            .find("</article>")
            .map(|end| end + "</article>".len())
            .unwrap_or(row.len());
        rows.push(&row[..end]);
        rest = &row[end..];
    }
    rows
}

fn unescape_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let entity = &rest[..=end];
        match entity {
            "&amp;" => out.push('&'),
            "&lt;" => out.push('<'),
            "&gt;" => out.push('>'),
            "&quot;" => out.push('"'),
            "&apos;" | "&#39;" | "&#x27;" => out.push('\''),
            _ => out.push_str(entity),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn visible_text(markup: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for character in markup.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(character),
            _ => {}
        }
    }
    unescape_entities(&out)
}

fn side_text(row: &str, side: &str) -> String {
    let open = format!("<div class=\"side {side}\">");
    let start = row.find(&open).expect("a row side") + open.len();
    let text_open = "<div class=\"text\">";
    let text_start =
        row[start..].find(text_open).expect("a row side text") + start + text_open.len();
    let text_end = row[text_start..]
        .find("</div>")
        .expect("a closed row side text");
    let text = &row[text_start..text_start + text_end];
    // Table cells have no literal spaces between their tags, so a cell boundary
    // must become one for the extracted words to match the source document.
    let text = text
        .replace("<span class=\"empty\">—</span>", "")
        .replace("</td>", " </td>")
        .replace("</th>", " </th>");
    visible_text(&text)
}

fn summary_count(html: &str, class: &str) -> usize {
    let marker = format!("<div class=\"metric {class}\"><strong>");
    let start = html.find(&marker).expect("a summary card") + marker.len();
    let end = html[start..].find('<').expect("a summary number") + start;
    html[start..end].parse().expect("a numeric summary value")
}

fn row_count(html: &str, class: &str) -> usize {
    html.matches(&format!("<article class=\"row {class}"))
        .count()
}

fn assert_summary_matches_rows(html: &str, label: &str) {
    let changed = row_count(html, "differs") + row_count(html, "notation");
    let added = row_count(html, "added");
    let removed = row_count(html, "removed");
    let placed = row_count(html, "placed");
    let unchanged = row_count(html, "same") + row_count(html, "contained");
    for (class, expected) in [
        ("changed", changed),
        ("added", added),
        ("removed", removed),
        ("placed", placed),
        ("same", unchanged),
    ] {
        assert_eq!(
            summary_count(html, class),
            expected,
            "{label} {class} count"
        );
    }
    assert_eq!(
        row_fragments(html).len(),
        changed + added + removed + placed + unchanged,
        "{label} summary classes do not account for every row"
    );
}

fn assert_metamorphic_report(
    label: &str,
    old: &Path,
    new: &Path,
    scratch: &Scratch,
    expected_before: &[String],
    expected_after: &[String],
) {
    let first_report = scratch.0.join(format!("{label}-first.html"));
    let second_report = scratch.0.join(format!("{label}-second.html"));

    for report in [&first_report, &second_report] {
        let output = run_redline(old, new, report);
        let text = logs(&output);
        assert!(output.status.success(), "{label} redline failed: {text}");
    }

    let first = std::fs::read(&first_report).expect("the first report");
    let second = std::fs::read(&second_report).expect("the second report");
    assert_eq!(first, second, "{label} rendered non-deterministically");
    let html = String::from_utf8(first).expect("the report is UTF-8");

    let rows = row_fragments(&html);
    assert!(!rows.is_empty(), "{label} produced no rows");
    let before = rows
        .iter()
        .map(|row| side_text(row, "before"))
        .collect::<Vec<_>>()
        .join(" ");
    let after = rows
        .iter()
        .map(|row| side_text(row, "after"))
        .collect::<Vec<_>>()
        .join(" ");
    // The report puts changed rows before the folded unchanged rows, so the
    // words are compared as a multiset: every source word must survive exactly
    // as many times as it appeared, without requiring report order.
    assert_eq!(
        sorted_words(before.split_whitespace()),
        sorted_words(expected_before.iter().map(String::as_str)),
        "{label} changed the words on the Before side"
    );
    assert_eq!(
        sorted_words(after.split_whitespace()),
        sorted_words(expected_after.iter().map(String::as_str)),
        "{label} changed the words on the After side"
    );

    assert_summary_matches_rows(&html, label);
}

#[test]
fn redline_metamorphic_words_summary_counts_and_deterministic_rendering() {
    let scratch = Scratch::new("metamorphic");

    for pair in METAMORPHIC_PAIRS {
        let old = scratch.write(&format!("{}-old.pdf", pair.label), &pdf(pair.old));
        let new = scratch.write(&format!("{}-new.pdf", pair.label), &pdf(pair.new));
        assert_metamorphic_report(
            pair.label,
            &old,
            &new,
            &scratch,
            &pair_words(pair.old),
            &pair_words(pair.new),
        );
    }

    // A DOCX is flow-based and publishes no pages. `merged_cells.docx` is
    // text-only from the report's point of view: its table has no picture or
    // drawing metadata that could add words absent from the source document.
    let old_bytes = std::fs::read(repository_root().join("test-data/merged_cells.docx"))
        .expect("the tracked flow DOCX fixture");
    let new_bytes = edited_docx_with("merged_cells.docx", &[("Alice", "Alicia")]);
    let expected_before = docx_words(&old_bytes);
    let expected_after = docx_words(&new_bytes);
    let old = scratch.write("flow-old.docx", &old_bytes);
    let new = scratch.write("flow-new.docx", &new_bytes);
    assert_metamorphic_report(
        "docx-flow",
        &old,
        &new,
        &scratch,
        &expected_before,
        &expected_after,
    );
}

fn redline_args(
    old: &std::path::Path,
    new: &std::path::Path,
    output: &std::path::Path,
) -> Vec<String> {
    vec![
        "--input".to_string(),
        new.display().to_string(),
        "--redline".to_string(),
        old.display().to_string(),
        "--output".to_string(),
        output.display().to_string(),
        "--ocr".to_string(),
        "never".to_string(),
    ]
}

#[derive(Debug, PartialEq, Eq)]
enum InputState {
    Missing,
    Directory,
    File(Vec<u8>),
}

fn input_state(path: &std::path::Path) -> InputState {
    if path.is_dir() {
        InputState::Directory
    } else if let Ok(bytes) = std::fs::read(path) {
        InputState::File(bytes)
    } else {
        InputState::Missing
    }
}

struct RefusalCase {
    label: &'static str,
    args: Vec<String>,
    old: PathBuf,
    new: PathBuf,
    expected: &'static str,
}

#[test]
fn redline_refusals_and_input_aliases_are_table_driven() {
    let scratch = Scratch::new("refusal-table");
    let old = scratch.write("old.pdf", &pdf(&["Old text."]));
    let new = scratch.write("new.pdf", &pdf(&["New text."]));
    let docx = scratch.write("new.docx", &edited_brief());
    let missing_old = scratch.0.join("missing-old.pdf");
    let missing_new = scratch.0.join("missing-new.pdf");
    let old_directory = scratch.0.join("old-directory");
    let new_directory = scratch.0.join("new-directory");
    std::fs::create_dir(&old_directory).expect("create the old directory");
    std::fs::create_dir(&new_directory).expect("create the new directory");

    let mut cases = Vec::new();
    let mut add = |label, args: Vec<String>, old: PathBuf, new: PathBuf, expected| {
        cases.push(RefusalCase {
            label,
            args,
            old,
            new,
            expected,
        });
    };

    let base = redline_args(&old, &new, &scratch.0.join("report.html"));
    let with = |flag: &str| {
        let mut args = base.clone();
        args.push(flag.to_string());
        args
    };
    add(
        "compare",
        {
            let mut args = with("--compare");
            args.push(scratch.0.display().to_string());
            args
        },
        old.clone(),
        new.clone(),
        "separate modes",
    );
    add(
        "preview",
        with("--preview"),
        old.clone(),
        new.clone(),
        "drop --preview",
    );
    add(
        "progress",
        with("--progress"),
        old.clone(),
        new.clone(),
        "no progress stream",
    );
    add(
        "to",
        {
            let mut args = with("--to");
            args.push("json".to_string());
            args
        },
        old.clone(),
        new.clone(),
        "drop --to",
    );
    add(
        "stdout",
        redline_args(&old, &new, std::path::Path::new("-")),
        old.clone(),
        new.clone(),
        "is not a report path",
    );
    add(
        "missing-old",
        redline_args(&missing_old, &new, &scratch.0.join("report.html")),
        missing_old.clone(),
        new.clone(),
        "redline old version does not exist",
    );
    add(
        "old-directory",
        redline_args(&old_directory, &new, &scratch.0.join("report.html")),
        old_directory.clone(),
        new.clone(),
        "redline old version must be a file",
    );
    add(
        "missing-new",
        redline_args(&old, &missing_new, &scratch.0.join("report.html")),
        old.clone(),
        missing_new.clone(),
        "input path does not exist",
    );
    add(
        "new-directory",
        redline_args(&old, &new_directory, &scratch.0.join("report.html")),
        old.clone(),
        new_directory.clone(),
        "redline new version must be a file",
    );
    add(
        "different-family",
        redline_args(&old, &docx, &scratch.0.join("report.html")),
        old.clone(),
        docx.clone(),
        "same PDF or DOCX",
    );
    add(
        "exact-old-alias",
        redline_args(&old, &new, &old),
        old.clone(),
        new.clone(),
        "would overwrite one of its inputs",
    );
    add(
        "exact-new-alias",
        redline_args(&old, &new, &new),
        old.clone(),
        new.clone(),
        "would overwrite one of its inputs",
    );
    add(
        "relative-old-alias",
        redline_args(
            std::path::Path::new("old.pdf"),
            std::path::Path::new("new.pdf"),
            std::path::Path::new("./old.pdf"),
        ),
        old.clone(),
        new.clone(),
        "would overwrite one of its inputs",
    );
    add(
        "relative-new-alias",
        redline_args(
            std::path::Path::new("old.pdf"),
            std::path::Path::new("new.pdf"),
            std::path::Path::new("./new.pdf"),
        ),
        old.clone(),
        new.clone(),
        "would overwrite one of its inputs",
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let old_link = scratch.0.join("old-link.pdf");
        let new_link = scratch.0.join("new-link.pdf");
        symlink(&old, &old_link).expect("create the old input symlink");
        symlink(&new, &new_link).expect("create the new input symlink");
        add(
            "symlink-old-alias",
            redline_args(&old, &new, &old_link),
            old.clone(),
            new.clone(),
            "would overwrite one of its inputs",
        );
        add(
            "symlink-new-alias",
            redline_args(&old, &new, &new_link),
            old.clone(),
            new.clone(),
            "would overwrite one of its inputs",
        );
    }

    for case in cases {
        let old_before = input_state(&case.old);
        let new_before = input_state(&case.new);
        let output = run_in(&scratch.0, &case.args);
        let text = logs(&output);
        let reports: Vec<PathBuf> = std::fs::read_dir(&scratch.0)
            .expect("read the refusal scratch folder")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "html")
            })
            .collect();
        assert!(
            !output.status.success(),
            "{} should have been refused: {text}",
            case.label
        );
        assert!(
            text.contains(case.expected),
            "{} did not name the refusal {:#?}: {text}",
            case.label,
            case.expected
        );
        assert!(
            reports.is_empty(),
            "{} wrote a report despite refusing: {reports:?}",
            case.label
        );
        assert_eq!(
            input_state(&case.old),
            old_before,
            "{} changed the old input",
            case.label
        );
        assert_eq!(
            input_state(&case.new),
            new_before,
            "{} changed the new input",
            case.label
        );
    }
}

#[test]
fn redline_summary_counts_changed_and_unchanged_blocks_exactly() {
    let scratch = Scratch::new("summary-counts");
    let old = scratch.write(
        "old.pdf",
        &pdf(&["Alpha page stays the same.", "Beta page will change."]),
    );
    let new = scratch.write(
        "new.pdf",
        &pdf(&["Alpha page stays the same.", "Gamma page is different."]),
    );
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    assert_exact_summary_counts(&html, 1, 0, 0, 0, 1);
    assert!(
        html.contains("<span>1 change · 0 unchanged</span>")
            && html.contains("<span>0 changes · 1 unchanged</span>"),
        "the per-page summary did not keep the exact changed/unchanged split: {html}"
    );
}

#[test]
fn redline_no_change_report_is_a_clean_baseline() {
    let scratch = Scratch::new("no-change");
    let old = scratch.write(
        "old.pdf",
        &pdf(&["First page is unchanged.", "Second page is unchanged."]),
    );
    let new = scratch.write(
        "new.pdf",
        &pdf(&["First page is unchanged.", "Second page is unchanged."]),
    );
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    assert_exact_summary_counts(&html, 0, 0, 0, 0, 2);
    assert!(
        html.contains("<div class=\"metric agreement\"><strong>100.0%</strong><span>new text held by old</span></div>"),
        "an unchanged document did not report complete text agreement: {html}"
    );
    assert!(
        !html.contains("<del>"),
        "an unchanged document had deletions: {html}"
    );
    assert!(
        !html.contains("<ins>"),
        "an unchanged document had insertions: {html}"
    );
    assert!(
        !html.contains("<article class=\"row differs"),
        "an unchanged document had a changed row: {html}"
    );
    assert_eq!(
        html.matches("<p class=\"quiet\">No changes on this page.</p>")
            .count(),
        2,
        "both unchanged pages should be labelled as unchanged: {html}"
    );
}

#[test]
fn redline_escapes_document_text_and_filenames_in_html() {
    let scratch = Scratch::new("html-escaping");
    let old = scratch.write(
        "old & draft's.pdf",
        &pdf(&["Old <script>alert & 'x'</script> words."]),
    );
    let new = scratch.write(
        "new & final's.pdf",
        &pdf(&["New <script>alert & 'y'</script> words."]),
    );
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    assert!(
        html.contains("old &amp; draft&#39;s.pdf"),
        "the old filename was not escaped in the report header: {html}"
    );
    assert!(
        html.contains("new &amp; final&#39;s.pdf"),
        "the new filename was not escaped in the report header: {html}"
    );
    assert!(
        html.contains("&lt;script&gt;alert &amp; &#39;x&#39;&lt;/script&gt;"),
        "the old document text was not escaped: {html}"
    );
    assert!(
        html.contains("&lt;script&gt;alert &amp; &#39;y&#39;&lt;/script&gt;"),
        "the new document text was not escaped: {html}"
    );
    assert!(
        !html.contains("<script>alert"),
        "document text was emitted as raw markup: {html}"
    );
}

#[test]
fn redline_report_is_self_contained_and_has_no_scratch_paths() {
    let scratch = Scratch::new("self-contained");
    let old = scratch.write("old.pdf", &pdf(&["A sentence."]));
    let new = scratch.write("new.pdf", &pdf(&["A changed sentence."]));
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    let scratch_path = scratch.0.to_string_lossy();
    assert!(
        !html.contains(scratch_path.as_ref()),
        "the report leaked the absolute scratch path: {html}"
    );
    for marker in [
        "http://", "https://", "file://", "src=\"", "src='", "<link", "<script", "url(",
    ] {
        assert!(
            !html.contains(marker),
            "the report referenced an external asset via {marker}: {html}"
        );
    }
    for (index, _) in html.match_indices("href=\"") {
        let value = &html[index + "href=\"".len()..];
        assert!(
            value.starts_with('#'),
            "the report has a non-anchor link: {html}"
        );
    }
    assert!(
        html.contains("<style>") && html.contains("</style>"),
        "the report lost its inline stylesheet: {html}"
    );
}

#[test]
fn pdf_redline_is_grouped_by_page_and_marks_changed_words() {
    let scratch = Scratch::new("pdf");
    let old = scratch.write(
        "old.pdf",
        &pdf(&["Quarterly revenue grew.", "The second page is unchanged."]),
    );
    let new = scratch.write(
        "new.pdf",
        &pdf(&["Quarterly revenue fell.", "The second page is unchanged."]),
    );
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);
    assert!(html.contains("id=\"page-1\""), "{html}");
    assert!(html.contains("id=\"page-2\""), "{html}");
    assert!(html.contains("href=\"#page-1\""), "{html}");
    assert!(html.contains("<h2>Page 1</h2>"), "{html}");
    assert!(html.contains("<h2>Page 2</h2>"), "{html}");
    assert!(html.contains("<del>grew.</del>"), "{html}");
    assert!(html.contains("<ins>fell.</ins>"), "{html}");
    assert!(
        html.contains("No changes on this page."),
        "the unchanged page was not kept as context: {html}"
    );
    assert!(
        html.contains("<div class=\"page-previews\">"),
        "the PDF report lost its visual placement previews: {html}"
    );
    assert!(
        html.contains("class=\"page-preview\" viewBox=\"0 0 612.00 792.00\""),
        "the page preview did not use the PDF's own page shape: {html}"
    );
    assert!(
        html.contains("class=\"preview-box differs\""),
        "the changed block was not drawn as a changed placement box: {html}"
    );
}

#[test]
fn pdf_redline_shows_cross_page_moves_and_removed_or_added_pages() {
    let scratch = Scratch::new("page-moves");
    let old = scratch.write(
        "old.pdf",
        &pdf(&[
            "Quarterly migration retains alpha status.",
            "Legacy instructions retired.",
            "Closing remarks persist.",
        ]),
    );
    let new = scratch.write(
        "new.pdf",
        &pdf(&[
            "Launch material introduces the edition.",
            "Quarterly migration retains beta status.",
            "Closing remarks persist.",
        ]),
    );
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    // The changed block moved from the old page 1 to the new page 2. The
    // section is keyed by both pages, the heading names both, and the row says
    // which way it moved.
    assert!(
        html.contains("id=\"page-1-to-2\" data-old-page=\"1\" data-new-page=\"2\""),
        "the cross-page section did not keep both page numbers: {html}"
    );
    assert!(html.contains("<h2>Page 1 → 2</h2>"), "{html}");
    assert!(html.contains("href=\"#page-1-to-2\""), "{html}");
    assert!(
        html.contains("<span class=\"moved\">moved: page 1 → page 2</span>"),
        "the moved block carried no move badge: {html}"
    );

    // The old page 2 has no counterpart, while the new page 1 has no old page.
    assert!(
        html.contains("id=\"page-2-removed\" data-old-page=\"2\" data-new-page=\"none\""),
        "the removed page lost its old page number: {html}"
    );
    assert!(html.contains("<h2>Page 2 (removed)</h2>"), "{html}");
    assert!(html.contains("href=\"#page-2-removed\""), "{html}");
    assert!(
        html.contains("id=\"page-1-added\" data-old-page=\"none\" data-new-page=\"1\""),
        "the added page lost its new page number: {html}"
    );
    assert!(html.contains("<h2>Page 1 (added)</h2>"), "{html}");
    assert!(html.contains("href=\"#page-1-added\""), "{html}");

    // The unchanged page is still present as context, with its ordinary page
    // anchor rather than a move or a removed/added suffix.
    assert!(
        html.contains("id=\"page-3\" data-old-page=\"3\" data-new-page=\"3\""),
        "{html}"
    );
    assert!(html.contains("<h2>Page 3</h2>"), "{html}");
}

#[test]
fn pdf_redline_shows_an_added_page_and_picture_from_tracked_fixtures() {
    let scratch = Scratch::new("tracked-added-page");
    let old = repository_root().join("test-data/hello.pdf");
    // Page 1 carries the same text as hello.pdf; page 2 adds a 32x32 RGB
    // picture, the smallest image the readers accept. Both sides are tracked
    // fixtures, so the report is reproducible from a fresh checkout.
    let new = repository_root().join("test-data/hello_added_picture.pdf");
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    assert!(
        html.contains("id=\"page-2-added\" data-old-page=\"none\" data-new-page=\"2\""),
        "the tracked fixture's second page was not reported as added: {html}"
    );
    assert!(html.contains("<h2>Page 2 (added)</h2>"), "{html}");
    assert!(
        html.contains("<div class=\"metric changed\"><strong>0</strong><span>changed</span></div>")
            && html
                .contains("<div class=\"metric added\"><strong>1</strong><span>added</span></div>")
            && html.contains(
                "<div class=\"metric removed\"><strong>0</strong><span>removed</span></div>"
            )
            && html.contains(
                "<div class=\"metric same\"><strong>1</strong><span>unchanged</span></div>"
            ),
        "the fixture did not compare as one unchanged page plus one added page: {html}"
    );
    assert!(
        html.contains("class=\"preview-box added picture\""),
        "the added picture lost its placement box: {html}"
    );
    assert!(
        html.contains("class=\"picture-previews\""),
        "the added picture lost its before/after preview: {html}"
    );
    assert!(
        html.contains("<figcaption>After · page 2</figcaption>"),
        "the added picture preview did not point at the added page: {html}"
    );
    assert!(
        html.contains(
            "<span class=\"kind\">picture</span><span class=\"label\">picture</span><span class=\"verdict\">added</span>"
        ),
        "the added picture was not a picture row: {html}"
    );
}

#[test]
fn pdf_redline_previews_picture_placement_boxes() {
    let scratch = Scratch::new("pdf-picture-preview");
    let fixture = std::fs::read(repository_root().join("test-data/scanned.pdf"))
        .expect("the tracked picture PDF fixture");
    let old = scratch.write("old.pdf", &fixture);
    let new = scratch.write("new.pdf", &fixture);
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);
    assert!(
        html.contains("class=\"preview-box same picture\""),
        "the report did not draw the picture's placement box: {html}"
    );
    assert!(
        html.contains("class=\"picture-previews\""),
        "the picture row lost its before/after preview: {html}"
    );
    assert!(
        html.contains("<figcaption>Before · page 1</figcaption>")
            && html.contains("<figcaption>After · page 1</figcaption>"),
        "the picture preview did not show both sides: {html}"
    );
}

#[test]
fn a_deletion_at_the_end_of_a_block_is_not_folded_away_as_containment() {
    let scratch = Scratch::new("contained-deletion");
    let old = scratch.write("old.pdf", &pdf(&["The revenue grew"]));
    let new = scratch.write("new.pdf", &pdf(&["The revenue"]));
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);
    assert!(html.contains("class=\"row differs\""), "{html}");
    assert!(html.contains("<del>grew</del>"), "{html}");
    assert!(
        !html.contains("same words, different blocks"),
        "a deletion was described as the same words: {html}"
    );
}

#[test]
fn docx_redline_is_one_flow_section_and_marks_changes() {
    let scratch = Scratch::new("docx");
    let old = scratch.write(
        "brief_old.docx",
        &std::fs::read(repository_root().join("test-data/brief.docx")).expect("the fixture"),
    );
    let new = scratch.write("brief_new.docx", &edited_brief());
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);
    assert!(html.contains("id=\"flow\""), "{html}");
    assert!(html.contains("Document flow"), "{html}");
    assert!(html.contains("<del>ahead of</del>"), "{html}");
    assert!(html.contains("<ins>behind</ins>"), "{html}");
    assert!(!html.contains("id=\"page-1\""), "{html}");
    assert!(
        !html.contains("class=\"page-previews\""),
        "a flow document must not invent page geometry: {html}"
    );
}

#[test]
fn docx_redline_renders_a_changed_table_cell_as_a_structured_table() {
    let scratch = Scratch::new("docx-table");
    let old = scratch.write(
        "brief_table_old.docx",
        &std::fs::read(repository_root().join("test-data/brief.docx")).expect("the fixture"),
    );
    let new = scratch.write(
        "brief_table_new.docx",
        &edited_brief_with(&[("<w:t>42</w:t>", "<w:t>99</w:t>")]),
    );
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    assert!(
        html.contains("<article class=\"row differs table\">"),
        "the table row was not identified as a table: {html}"
    );
    assert!(
        html.matches("<table class=\"diff-table\">").count() == 2,
        "the report did not render one table per side: {html}"
    );
    assert!(
        html.matches("<tr>").count() == 6,
        "the three-row table did not keep exactly three rows per side: {html}"
    );
    assert!(
        html.matches("<td").count() == 14,
        "the merged table did not keep seven cells per side after its covered slots were skipped: {html}"
    );
    assert!(
        html.contains("<td colspan=\"2\">Revenue</td>"),
        "the merged header lost its column span: {html}"
    );
    assert!(
        html.contains("<td rowspan=\"2\">North South</td>"),
        "the vertical merge lost its row span: {html}"
    );
    assert!(
        html.contains("<td><del>42</del></td>"),
        "the deleted cell value was not marked in its cell: {html}"
    );
    assert!(
        html.contains("<td><ins>99</ins></td>"),
        "the inserted cell value was not marked in its cell: {html}"
    );
    assert!(
        !html.contains("North South | 42 | 7"),
        "the table still fell back to pipe-delimited text: {html}"
    );
}

#[test]
fn docx_redline_preserves_the_merged_header_in_brief() {
    let scratch = Scratch::new("docx-merged-header");
    let old = scratch.write(
        "brief_old.docx",
        &std::fs::read(repository_root().join("test-data/brief.docx")).expect("the fixture"),
    );
    let new = scratch.write(
        "brief_new.docx",
        &edited_brief_with(&[("<w:t>Revenue</w:t>", "<w:t>Turnover</w:t>")]),
    );
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    assert!(
        html.contains("<td colspan=\"2\"><del>Revenue</del></td>"),
        "the old merged header was padded into separate cells: {html}"
    );
    assert!(
        html.contains("<td colspan=\"2\"><ins>Turnover</ins></td>"),
        "the new merged header was padded into separate cells: {html}"
    );
    assert!(
        !html.contains("<td>Revenue</td><td></td>"),
        "the header still used the old padded grid: {html}"
    );
}

#[test]
fn docx_redline_shows_a_merge_change_with_unchanged_cell_text() {
    let scratch = Scratch::new("docx-merge-only");
    let old = scratch.write(
        "brief_old.docx",
        &std::fs::read(repository_root().join("test-data/brief.docx")).expect("the fixture"),
    );
    // Revenue keeps every word it had; only the two-column merge becomes one
    // column. The row is a layout change, not an unchanged row to fold away.
    let new = scratch.write(
        "brief_new.docx",
        &edited_brief_with(&[("<w:gridSpan w:val=\"2\"/>", "<w:gridSpan w:val=\"1\"/>")]),
    );
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    assert!(
        html.contains("<article class=\"row differs table\">"),
        "the merge-only change was hidden as an unchanged row: {html}"
    );
    assert!(
        html.contains("<td colspan=\"2\">Revenue</td>"),
        "the old two-column merge was lost: {html}"
    );
    assert!(
        html.contains("<td>Revenue</td>"),
        "the new single-column layout was lost: {html}"
    );
    assert!(
        html.contains("table layout differs"),
        "the row did not name the layout change: {html}"
    );
    assert!(
        !html.contains("<del>Revenue</del>") && !html.contains("<ins>Revenue</ins>"),
        "the unchanged cell text was marked as changed: {html}"
    );
}

#[test]
fn docx_redline_keeps_cell_markers_on_values_after_a_row_is_inserted() {
    let scratch = Scratch::new("docx-inserted-row");
    let old = scratch.write(
        "brief_old.docx",
        &std::fs::read(repository_root().join("test-data/brief.docx")).expect("the fixture"),
    );
    let north_row = r#"<w:tr><w:tc><w:tcPr><w:tcW w:type="dxa" w:w="2880"/><w:vMerge w:val="restart"/></w:tcPr>"#;
    let inserted_row = r#"<w:tr><w:tc><w:p><w:r><w:t>Europe</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>55</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>9</w:t></w:r></w:p></w:tc></w:tr>"#;
    let replacement = format!("{inserted_row}{north_row}");
    let new = scratch.write(
        "brief_new.docx",
        &edited_brief_with(&[(north_row, replacement.as_str())]),
    );
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    assert!(
        html.contains("<article class=\"row differs table\">"),
        "the inserted row did not make the table a changed row: {html}"
    );
    assert!(
        html.contains("<td><ins>Europe</ins></td>")
            && html.contains("<td><ins>55</ins></td>")
            && html.contains("<td><ins>9</ins></td>"),
        "the inserted row's cells were not marked as inserted: {html}"
    );
    for value in ["42", "18", "3"] {
        assert!(
            html.contains(&format!("<td>{value}</td>")),
            "the unchanged value {value} was not left in its cell: {html}"
        );
        assert!(
            !html.contains(&format!("<del>{value}</del>"))
                && !html.contains(&format!("<ins>{value}</ins>")),
            "the unchanged value {value} was marked as changed after the row shifted: {html}"
        );
    }
    assert!(
        html.contains("<td rowspan=\"2\">North South</td>"),
        "the unchanged merged row was not kept with its row span: {html}"
    );
}

#[test]
fn docx_redline_keeps_cell_markers_on_values_after_a_row_is_removed() {
    let scratch = Scratch::new("docx-removed-row");
    let old = scratch.write(
        "brief_old.docx",
        &std::fs::read(repository_root().join("test-data/brief.docx")).expect("the fixture"),
    );
    let removed_row = r#"<w:tr><w:tc><w:tcPr><w:tcW w:type="dxa" w:w="2880"/><w:vMerge/></w:tcPr><w:p/></w:tc><w:tc><w:tcPr><w:tcW w:type="dxa" w:w="2880"/></w:tcPr><w:p><w:r><w:t>18</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:type="dxa" w:w="2880"/></w:tcPr><w:p><w:r><w:t>3</w:t></w:r></w:p></w:tc></w:tr>"#;
    let new = scratch.write("brief_new.docx", &edited_brief_with(&[(removed_row, "")]));
    let report = scratch.0.join("report.html");

    let html = redline_report(&old, &new, &report);

    assert!(
        html.contains("<article class=\"row differs table\">"),
        "the removed row did not make the table a changed row: {html}"
    );
    assert!(
        html.contains("<td><del>18</del></td>") && html.contains("<td><del>3</del></td>"),
        "the removed row's cells were not marked as removed: {html}"
    );
    for value in ["42", "7"] {
        assert!(
            html.contains(&format!("<td>{value}</td>")),
            "the unchanged value {value} was not left in its cell: {html}"
        );
        assert!(
            !html.contains(&format!("<del>{value}</del>"))
                && !html.contains(&format!("<ins>{value}</ins>")),
            "the unchanged value {value} was marked as changed after the row was removed: {html}"
        );
    }
    assert!(
        html.contains("<td rowspan=\"2\">North South</td>"),
        "the old merged row lost its row span: {html}"
    );
}

#[test]
fn the_default_report_lands_beside_the_new_version() {
    let scratch = Scratch::new("default-output");
    let old = scratch.write("old.pdf", &pdf(&["Old words."]));
    let new = scratch.write("new.pdf", &pdf(&["New words."]));

    let output = run(&[
        "--input",
        new.to_str().expect("utf-8 path"),
        "--redline",
        old.to_str().expect("utf-8 path"),
        "--ocr",
        "never",
    ]);
    let text = logs(&output);
    assert!(output.status.success(), "redline failed: {text}");
    assert!(
        new.with_extension("redline.html").exists(),
        "the default report is not beside the new version: {text}"
    );
}

/// Keep the helper honest: a PDF this test writes has to be one the CLI can
/// actually read, independent of the redline rendering.
#[test]
fn the_test_pdf_is_readable() {
    let scratch = Scratch::new("pdf-readable");
    let input = scratch.write("one.pdf", &pdf(&["Readable page."]));
    let output = scratch.0.join("one.json");
    let result = run(&[
        "--input",
        input.to_str().expect("utf-8 path"),
        "--output",
        output.to_str().expect("utf-8 path"),
        "--ocr",
        "never",
    ]);
    let text = logs(&result);
    assert!(
        result.status.success(),
        "the test PDF is not readable: {text}"
    );
    assert!(output.exists(), "the readable PDF produced no output");
}
