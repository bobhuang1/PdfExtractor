//! Golden tests against Docling itself.
//!
//! Two oracles, both real Docling artefacts:
//!
//! * `test-data/golden/DoclingDocument.json` — the JSON Schema docling-core
//!   generates from its own model. Every document the CLI writes is held to it:
//!   required fields present, no field an item class does not allow, enum values
//!   legal, references resolvable, the document hierarchy consistent.
//! * `test-data/golden/docling/<stem>.json` — real Docling conversions of corpus
//!   documents, when they have been generated (see `test-data/golden/README.md`).
//!   They are compared semantically, never byte for byte.
//!
//! The first oracle is what found bugs 44 and 45: our documents were not
//! loadable by Docling at all. The schema check is written against the schema
//! file rather than against a copy of it, so bumping the schema re-checks the
//! output instead of silently drifting.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

// --- fixtures -------------------------------------------------------------

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golden_dir() -> PathBuf {
    repository_root().join("test-data/golden")
}

fn corpus_dir() -> PathBuf {
    repository_root().join("test-data/real")
}

/// The official schema, as commited to the repository.
fn schema() -> Value {
    let path = golden_dir().join("DoclingDocument.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
    serde_json::from_str(&text).expect("the Docling schema is JSON")
}

/// A scratch directory, removed when it goes out of scope.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pdfe-golden-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Convert every corpus document once, into `out`.
fn convert_corpus(out: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            corpus_dir().to_str().expect("a UTF-8 path"),
            "-o",
            out.to_str().expect("a UTF-8 path"),
            "-p",
            "4",
        ])
        .env("RUST_LOG", "error")
        .status()
        .expect("run the CLI binary");
    assert!(status.success(), "the corpus conversion failed");
}

/// The document a reference was generated from, found by file stem.
///
/// The corpus comes first; a hand-written fixture beside it (`inventory.xlsx`,
/// the XLSX parser's own workbook) can be held to a real Docling conversion the
/// same way, which is what makes a new format's output comparable before it
/// joins the corpus.
fn corpus_input(stem: &str) -> PathBuf {
    let fixtures = repository_root().join("test-data");
    for directory in [corpus_dir(), fixtures] {
        let found = std::fs::read_dir(&directory)
            .expect("read the document directories")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .find(|path| {
                path.file_stem()
                    .is_some_and(|name| name.to_string_lossy() == stem)
            });
        if let Some(path) = found {
            return path;
        }
    }
    panic!("no corpus document or fixture named {stem}.*")
}

/// Every `*.json` the conversion wrote, with its document name.
fn outputs(dir: &Path) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .expect("read the output tree")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = path
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned();
            let text = std::fs::read_to_string(&path).expect("read an output");
            let value: Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
            out.push((name, value));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(out.len() >= 30, "expected the corpus, found {}", out.len());
    out
}

// --- the schema-driven conformance check ----------------------------------

/// Checks derived from the official schema file.
///
/// Only the parts of JSON Schema that Docling's own output relies on are
/// implemented — required fields, `additionalProperties: false`, `const`, `enum`
/// and the nested records we emit — because those are the constraints that broke
/// us. A general validator would be a dependency and a bigger claim than this
/// file needs.
struct Conformance {
    root: Value,
    problems: Vec<String>,
}

impl Conformance {
    fn new(root: Value) -> Self {
        Self {
            root,
            problems: Vec::new(),
        }
    }

    fn def(&self, name: &str) -> &Value {
        self.root
            .get("$defs")
            .and_then(|defs| defs.get(name))
            .unwrap_or_else(|| panic!("the schema has no $defs/{name}"))
    }

    /// Follow `{"$ref": "#/$defs/X"}` to the definition it names.
    fn resolve<'a>(&'a self, value: &'a Value) -> &'a Value {
        match value.get("$ref").and_then(Value::as_str) {
            Some(reference) => self.def(reference.trim_start_matches("#/$defs/")),
            None => value,
        }
    }

    fn properties(&self, name: &str) -> Vec<String> {
        self.def(name)
            .get("properties")
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn required(&self, name: &str) -> Vec<String> {
        self.def(name)
            .get("required")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn enums_of(&self, name: &str, field: &str) -> Vec<String> {
        let value = self.resolve(&self.def(name)["properties"][field]);
        let mut allowed: Vec<String> = value
            .get("enum")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(constant) = value.get("const").and_then(Value::as_str) {
            allowed.push(constant.to_string());
        }
        allowed
    }

    /// The document class a label belongs to, read from the `texts` union.
    fn text_class(&self, label: &str) -> Option<String> {
        let unions = self.root["properties"]["texts"]["items"]["anyOf"].as_array()?;
        for entry in unions {
            let name = entry["$ref"].as_str()?.trim_start_matches("#/$defs/");
            if self
                .enums_of(name, "label")
                .iter()
                .any(|allowed| allowed == label)
            {
                return Some(name.to_string());
            }
        }
        None
    }

    /// Docling's own default for `version`, so the two cannot drift.
    fn version(&self) -> String {
        self.root["properties"]["version"]["default"]
            .as_str()
            .expect("the schema declares a version")
            .to_string()
    }

    fn fail(&mut self, path: impl std::fmt::Display, problem: impl std::fmt::Display) {
        self.problems.push(format!("{path}: {problem}"));
    }

    /// Required fields present and, where the schema forbids extras, nothing
    /// the class does not allow. Classes that permit extras (the unsealed ones,
    /// such as `PageItem` and `TableData`) are only required-checked.
    fn fields(&mut self, class: &str, value: &Value, path: &str) {
        let Some(object) = value.as_object() else {
            self.fail(path, format!("expected an object, found {value}"));
            return;
        };
        let allowed = self.properties(class);
        for name in self.required(class) {
            if !object.contains_key(&name) {
                self.fail(path, format!("{class} is missing required field {name}"));
            }
        }
        let sealed = self.def(class).get("additionalProperties") == Some(&Value::Bool(false));
        if sealed {
            for key in object.keys() {
                if !allowed.iter().any(|name| name == key) {
                    self.fail(
                        path,
                        format!("{class} has no field {key} (Docling rejects unknown fields)"),
                    );
                }
            }
        }
        // Every item and group carries these, and each has a closed set of
        // values, so they are worth checking wherever they appear.
        self.enum_field(class, "label", value, path);
        self.enum_field(class, "content_layer", value, path);
    }

    /// A field of `class` that must be one of the schema's values.
    fn enum_field(&mut self, class: &str, field: &str, value: &Value, path: &str) {
        let Some(actual) = value.get(field).and_then(Value::as_str) else {
            return; // absence is the caller's business
        };
        let allowed = self.enums_of(class, field);
        if !allowed.is_empty() && !allowed.iter().any(|value| value == actual) {
            self.fail(
                path,
                format!("{field} is {actual:?}, which {class} does not allow"),
            );
        }
    }

    fn provenance(&mut self, value: &Value, path: &str) {
        let Some(records) = value.get("prov") else {
            return;
        };
        let Some(records) = records.as_array() else {
            self.fail(path, "prov is not a list".to_string());
            return;
        };
        for (index, record) in records.iter().enumerate() {
            let path = format!("{path}/prov/{index}");
            self.fields("ProvenanceItem", record, &path);
            let Some(bbox) = record.get("bbox") else {
                continue;
            };
            self.fields("BoundingBox", bbox, &path);
            self.enum_field("BoundingBox", "coord_origin", bbox, &path);
            if let Some(page_no) = record.get("page_no") {
                if page_no.as_i64().is_none() {
                    self.fail(
                        path.clone(),
                        format!("page_no is not an integer: {page_no}"),
                    );
                }
            }
            if let Some(span) = record.get("charspan") {
                let ok = span.as_array().is_some_and(|list| {
                    list.len() == 2 && list.iter().all(|v| v.as_u64().is_some())
                });
                if !ok {
                    self.fail(path, format!("charspan is not a pair of numbers: {span}"));
                }
            }
        }
    }

    fn image(&mut self, value: &Value, path: &str) {
        let Some(image) = value.get("image") else {
            return;
        };
        if image.is_null() {
            return;
        }
        let path = format!("{path}/image");
        self.fields("ImageRef", image, &path);
        if let Some(dpi) = image.get("dpi") {
            // Docling declares an integer; a fraction is rejected outright.
            if dpi.as_u64().is_none() {
                self.fail(path.clone(), format!("dpi is not a whole number: {dpi}"));
            }
        }
        if let Some(size) = image.get("size") {
            self.fields("Size", size, &format!("{path}/size"));
        }
    }

    fn table(&mut self, value: &Value, path: &str) {
        let Some(cells) = value["data"].get("table_cells").and_then(Value::as_array) else {
            self.fail(
                format!("{path}/data"),
                "a table has no table_cells".to_string(),
            );
            return;
        };
        for (index, cell) in cells.iter().enumerate() {
            // A cell with a `ref` is Docling's `RichTableCell` — the cell that
            // holds an item of the document rather than only words — and a
            // cell without one is the plain `TableCell`. Its `ref` is required
            // on the first and forbidden on the second, so the two are not
            // interchangeable and the class is chosen by what the cell states.
            let class = if cell.get("ref").is_some() {
                "RichTableCell"
            } else {
                "TableCell"
            };
            self.fields(class, cell, &format!("{path}/data/table_cells/{index}"));
        }
    }

    /// Every reference resolves, and every child agrees with its parent.
    ///
    /// The schema cannot express this, and Docling enforces it at load time
    /// ("Document hierarchy is inconsistent"), which is exactly how bug 45 was
    /// found: our furniture listed children that claimed the body.
    fn hierarchy(&mut self, document: &Value) {
        let mut parents: Map<String, Value> = Map::new();
        for (index, item) in document["texts"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            parents.insert(format!("#/texts/{index}"), item["parent"].clone());
        }
        for (index, item) in document["tables"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            parents.insert(format!("#/tables/{index}"), item["parent"].clone());
        }
        for (index, item) in document["pictures"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            parents.insert(format!("#/pictures/{index}"), item["parent"].clone());
        }
        // A group is a parent too: an XLSX sheet holds the sheet's tables, and
        // a body listing the sheet needs the sheet's own parent to agree with
        // it the way an item's does.
        for (index, group) in document["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            parents.insert(format!("#/groups/{index}"), group["parent"].clone());
        }

        for group in ["body", "furniture"] {
            let Some(node) = document.get(group) else {
                continue;
            };
            if node.is_null() {
                continue;
            }
            self.children_agree(&format!("#/{group}"), node, &parents);
        }
        // And a group owes its own children the same agreement: a sheet
        // listing a table that names another sheet is a broken hierarchy just
        // as a body listing a furniture item is.
        for (index, node) in document["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            self.children_agree(&format!("#/groups/{index}"), node, &parents);
        }

        // A rich cell references one of its table's children, whose parent is
        // that table. Docling refuses a document where either disagrees — its
        // own `validate_tree` asks the same two questions — so a cell holding a
        // picture in a group is held to them here.
        for (index, table) in document["tables"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let owner = format!("#/tables/{index}");
            let children: Vec<&str> = table["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|child| child.get("$ref").and_then(Value::as_str))
                .collect();
            for (cell_index, cell) in table["data"]["table_cells"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let Some(target) = cell
                    .get("ref")
                    .and_then(|reference| reference.get("$ref"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let path = format!("{owner}/data/table_cells/{cell_index}/ref");
                let Some(parent) = parents.get(target) else {
                    self.fail(path, format!("{target} is not an item of this document"));
                    continue;
                };
                if parent.get("$ref").and_then(Value::as_str) != Some(owner.as_str()) {
                    self.fail(path.clone(), format!("{target}'s parent is {parent}"));
                }
                if !children.contains(&target) {
                    self.fail(path, format!("{target} is not one of {owner}'s children"));
                }
            }
        }
    }

    /// Every child `node` lists resolves to an item whose `parent` is `owner`.
    fn children_agree(&mut self, owner: &str, node: &Value, parents: &Map<String, Value>) {
        for (index, child) in node["children"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let path = format!("{owner}/children/{index}");
            let Some(target) = child.get("$ref").and_then(Value::as_str) else {
                self.fail(path, "a child is not a {\"$ref\": ...} object");
                continue;
            };
            let Some(parent) = parents.get(target) else {
                self.fail(path, format!("{target} is not an item of this document"));
                continue;
            };
            let parent = parent.get("$ref").and_then(Value::as_str);
            if parent != Some(owner) {
                self.fail(
                    path,
                    format!("{target} is listed under {owner} but names {parent:?} as its parent"),
                );
            }
        }
    }

    fn document(&mut self, name: &str, document: &Value) -> Vec<String> {
        if document["schema_name"].as_str() != Some("DoclingDocument") {
            self.fail(name, format!("schema_name is {}", document["schema_name"]));
        }
        if document["version"].as_str() != Some(self.version().as_str()) {
            self.fail(
                name,
                format!(
                    "version is {} but the schema is {}",
                    document["version"],
                    self.version()
                ),
            );
        }

        for (index, item) in document["texts"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let path = format!("{name}/texts/{index}");
            let label = item["label"].as_str().unwrap_or_default();
            match self.text_class(label) {
                Some(class) => {
                    self.fields(&class, item, &path);
                    self.enum_field(&class, "label", item, &path);
                }
                None => self.fail(path.clone(), format!("unknown label {label:?}")),
            }
            self.provenance(item, &path);
        }

        for (index, item) in document["tables"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let path = format!("{name}/tables/{index}");
            self.fields("TableItem", item, &path);
            self.enum_field("TableItem", "label", item, &path);
            self.provenance(item, &path);
            self.table(item, &path);
        }

        for (index, item) in document["pictures"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let path = format!("{name}/pictures/{index}");
            self.fields("PictureItem", item, &path);
            self.provenance(item, &path);
            self.image(item, &path);
        }

        // Empty in every document we write today, but a key-value item without
        // its `graph` is rejected by Docling, so the moment one appears it is
        // held to the same rule as everything else.
        for (index, item) in document["key_value_items"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let path = format!("{name}/key_value_items/{index}");
            self.fields("KeyValueItem", item, &path);
            self.provenance(item, &path);
        }

        for (page_no, page) in document["pages"].as_object().into_iter().flatten() {
            let path = format!("{name}/pages/{page_no}");
            self.fields("PageItem", page, &path);
            if let Some(size) = page.get("size") {
                self.fields("Size", size, &format!("{path}/size"));
            }
        }

        if let Some(origin) = document.get("origin") {
            self.fields("DocumentOrigin", origin, &format!("{name}/origin"));
            if origin["binary_hash"].as_u64().is_none() && !origin["binary_hash"].is_null() {
                self.fail(
                    format!("{name}/origin"),
                    format!(
                        "binary_hash must be a non-negative integer, found {}",
                        origin["binary_hash"]
                    ),
                );
            }
        }

        for group in ["body", "furniture"] {
            if let Some(node) = document.get(group) {
                if !node.is_null() {
                    self.fields("GroupItem", node, &format!("{name}/{group}"));
                }
            }
        }

        // Groups are how an XLSX worksheet holds its tables; they obey the
        // same field and enum rules as every other node.
        for (index, group) in document["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            self.fields("GroupItem", group, &format!("{name}/groups/{index}"));
        }

        self.hierarchy(document);
        self.problems.clone()
    }
}

// --- the tests ------------------------------------------------------------

/// Every document we write must be loadable by Docling.
#[test]
fn every_corpus_document_conforms_to_the_docling_schema() {
    let scratch = Scratch::new("schema");
    convert_corpus(&scratch.0);
    let documents = outputs(&scratch.0);

    let mut problems = Vec::new();
    for (name, document) in &documents {
        problems.extend(Conformance::new(schema()).document(name, document));
    }

    assert!(
        problems.is_empty(),
        "{} document(s) checked, {} problem(s):\n  {}",
        documents.len(),
        problems.len(),
        problems.join("\n  ")
    );
}

/// Convert a fixture beside the corpus on its own and hold it to the schema.
///
/// A format that has not joined the corpus must still produce a document
/// Docling can load, and a fixture is what lets it be tested before it does.
fn assert_fixture_conforms(file: &str, stem: &str, tag: &str) {
    let scratch = Scratch::new(tag);
    let input = repository_root().join("test-data").join(file);
    assert!(input.exists(), "missing {}", input.display());
    let output = scratch.0.join(format!("{stem}.json"));
    let status = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            input.to_str().expect("a UTF-8 path"),
            "-o",
            output.to_str().expect("a UTF-8 path"),
        ])
        .status()
        .expect("run the CLI binary");
    assert!(status.success(), "{file} did not convert");

    let document: Value =
        serde_json::from_str(&std::fs::read_to_string(&output).expect("read our output"))
            .expect("our output is JSON");
    let problems = Conformance::new(schema()).document(stem, &document);
    assert!(
        problems.is_empty(),
        "{} problem(s):\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
}

/// A workbook is held to the same check, including the sheet groups it adds,
/// which the corpus documents have never exercised.
#[test]
fn a_workbook_conforms_to_the_docling_schema() {
    assert_fixture_conforms("inventory.xlsx", "inventory", "xlsx-schema");
}

/// A deck adds things no corpus document has: `chapter` groups that hold
/// their slide's items, list items with a `marker` and `enumerated`, and a
/// table whose cell spans two columns.
#[test]
fn a_presentation_conforms_to_the_docling_schema() {
    assert_fixture_conforms("q3_review.pptx", "q3_review", "pptx-schema");
}

/// A plain image is a page whose origin names the image's own media type —
/// not `application/pdf`, which is what Docling writes for one.
#[test]
fn a_plain_image_conforms_to_the_docling_schema() {
    assert_fixture_conforms("invoice_scan.png", "invoice_scan", "image-schema");
}

/// An RTF is a flow document, but its table is a real grid: a cell can span
/// rows and columns, and the header is a flag on the cells rather than a
/// position. The fixture is LibreOffice's export of a brief whose region runs
/// down two rows and whose total runs across two columns.
#[test]
fn a_word_processor_rtf_conforms_to_the_docling_schema() {
    assert_fixture_conforms("merged_table.rtf", "merged_table", "rtf-schema");
}

/// The format revision we write is the one the committed schema describes.
#[test]
fn the_document_version_is_the_one_the_schema_declares() {
    let expected = schema()["properties"]["version"]["default"]
        .as_str()
        .expect("the schema declares a version")
        .to_string();
    assert_eq!(
        pdfextractor_core::builder::DOCLING_VERSION,
        expected,
        "the exporter and the golden schema disagree about the format revision"
    );
}

// --- comparison against real Docling conversions ---------------------------

/// The labels that mean "this is a heading", on either side.
const HEADING_LABELS: [&str; 2] = ["section_header", "title"];
/// Labels whose block is not body text, so one that holds body text merged
/// across a boundary it should have drawn.
const STRUCTURAL_LABELS: [&str; 3] = ["section_header", "title", "caption"];
/// The labels both tools use for ordinary body text.
const BODY_LABELS: [&str; 4] = ["text", "paragraph", "code", "reference"];

/// What a comparison of two documents found.
#[derive(Debug, PartialEq)]
struct Agreement {
    pages_ours: usize,
    pages_theirs: usize,
    /// Share of the reference's alphanumeric characters that are also ours,
    /// counted as multisets so a repeated character has to be present twice.
    /// Formula items are compared as notation instead; see [`compare`].
    text_agreement: f64,
    formulas_ours: usize,
    /// The reference's formula items carrying text whose LaTeX we reproduce.
    /// Whitespace is ignored, so a difference in spacing is not a difference in
    /// notation; a different command, operator or character is.
    formulas_reproduced: usize,
    /// Every formula item the reference has, whether or not it carries text.
    formulas_theirs: usize,
    /// The reference's formula items that carry text — the ones that say what an
    /// equation *is*.
    ///
    /// A PDF reference can hold empty formula items: its layout model's idea of
    /// where an equation sits, with no characters recovered for it (its font
    /// decoding recovered nothing). There is no equation in one to be found, so
    /// the count we have to meet is this one. See `test-data/golden/README.md`.
    formulas_with_text_theirs: usize,
    /// Text blocks that appear twice in a row in our output.
    duplicates: Vec<String>,
    /// Reference headings and titles we hold under a heading or title label.
    headings_kept: usize,
    /// Reference headings/titles we hold *only* as body text: a mislabelled
    /// heading, and the boundary between it and its body was not drawn.
    headings_demoted: Vec<String>,
    /// Our structural blocks that swallowed a reference body block of
    /// comparable size — a boundary that was not drawn.
    merged_blocks: Vec<String>,
    /// Reference table cells whose text is in one of our tables.
    table_cells_in_tables: usize,
    /// Reference table cells we hold as prose although we drew a table over
    /// them: the text was found, but on the wrong side of a table boundary.
    table_cells_in_prose: Vec<String>,
    /// Reference table cells we hold as prose with no table of ours covering
    /// them: a table read as prose, which is our conservative table detection's
    /// known limit. Counted and printed, not failed — see `test-data/golden/README.md`.
    table_cells_uncovered: usize,
    /// The reference's footnotes, in all: the notes the two tools would have to
    /// agree on if either found every one.
    footnotes_theirs: usize,
    /// Reference footnotes we hold under a `footnote` label.
    footnotes_kept: usize,
    /// Reference footnotes we found but read as something else — the note text
    /// is in our output, the note is not.
    footnotes_demoted: Vec<String>,
    /// Reference footnotes whose opening is not in our output at all.
    footnotes_missing: usize,
    /// Those notes, so a loss is legible and not just a number.
    footnotes_lost: Vec<String>,
    notes: Vec<String>,
}

/// Compare our document with a real Docling conversion of the same input.
///
/// The comparison is about *content*, because the two disagree about layout by
/// design: blocks are segmented, labelled and ordered differently. What must
/// hold is that we saw the same pages, the same characters, and the equations.
///
/// Formulas are compared separately, as notation: Docling writes an equation as
/// LaTeX (`\iiint_{V}^{}divFdV`) and so do we, so each reference formula with
/// text must have the same LaTeX here up to whitespace. Its command letters are
/// not folded into the character agreement, where every `\frac` would count as
/// text. See `test-data/golden/README.md`.
fn compare(ours: &Value, reference: &Value) -> Agreement {
    let pages_ours = ours["pages"].as_object().map_or(0, Map::len);
    let pages_theirs = reference["pages"].as_object().map_or(0, Map::len);

    let mut notes = Vec::new();
    for (page, size) in reference["pages"].as_object().into_iter().flatten() {
        let Some(theirs) = size.get("size") else {
            continue;
        };
        let Some(ours_size) = ours["pages"]
            .get(page)
            .and_then(|page| page.get("size"))
            .cloned()
        else {
            notes.push(format!("page {page} is missing from our output"));
            continue;
        };
        for side in ["width", "height"] {
            let (a, b) = (ours_size[side].as_f64(), theirs[side].as_f64());
            if let (Some(a), Some(b)) = (a, b) {
                if (a - b).abs() > 1.0 {
                    notes.push(format!(
                        "page {page} {side}: ours {a}, Docling {b} (more than a point apart)"
                    ));
                }
            }
        }
    }

    // A multiset intersection: a character the reference repeats twice has to
    // appear twice here to count twice.
    let mut available = characters(ours);
    let theirs_text = characters(reference);
    let mut matched = 0usize;
    for (character, count) in &theirs_text {
        let left = available.entry(*character).or_default();
        let used = (*count).min(*left);
        matched += used;
        *left -= used;
    }
    let total: usize = theirs_text.values().sum();
    let text_agreement = if total == 0 {
        1.0
    } else {
        matched as f64 / total as f64
    };

    let count = |document: &Value, want: &str| -> usize {
        document["texts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["label"].as_str() == Some(want))
            .count()
    };

    // A block that appears twice in a row is a defect whatever the reference
    // says about segmentation: the same words read twice is not a document.
    //
    // Text drawn inside a figure is the exception, and the reason the rule has
    // one: a chart repeats a legend under each subplot, and the attention
    // visualisation in `arxiv_attention.pdf` prints every word twice by design
    // (the source says so — `pdftotext` reads each one twice). `arxiv_attention`
    // is exactly the case that exposed it: our figure text repeats and the
    // reference has none of it to compare against, because Docling's layout
    // model dropped that page's figure text. A figure is a picture on the page,
    // so a repeat whose box lies inside one is the drawing, not our extraction.
    let pictures: Vec<(i64, [f64; 4])> = ours["pictures"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|picture| picture["prov"].as_array().into_iter().flatten())
        .filter_map(|prov| {
            let page = prov["page_no"].as_i64()?;
            let bbox = prov.get("bbox")?;
            Some((
                page,
                [
                    bbox["l"].as_f64()?,
                    bbox["t"].as_f64()?,
                    bbox["r"].as_f64()?,
                    bbox["b"].as_f64()?,
                ],
            ))
        })
        .collect();
    let in_a_figure = |item: &Value| -> bool {
        let Some(prov) = item["prov"].as_array().and_then(|p| p.first()) else {
            return false;
        };
        let (Some(page), Some(bbox)) = (prov["page_no"].as_i64(), prov.get("bbox")) else {
            return false;
        };
        let (Some(l), Some(t), Some(r), Some(b)) = (
            bbox["l"].as_f64(),
            bbox["t"].as_f64(),
            bbox["r"].as_f64(),
            bbox["b"].as_f64(),
        ) else {
            return false;
        };
        pictures.iter().any(|(picture_page, picture)| {
            *picture_page == page
                && picture[0] < r
                && l < picture[2]
                && picture[1] < b
                && t < picture[3]
        })
    };

    let mut duplicates = Vec::new();
    let mut previous: Option<String> = None;
    for item in ours["texts"].as_array().into_iter().flatten() {
        if item["label"].as_str() == Some("formula") {
            continue;
        }
        let text = item["text"].as_str().unwrap_or_default().trim().to_string();
        if text.is_empty() {
            continue;
        }
        if previous.as_deref() == Some(text.as_str()) && !in_a_figure(item) {
            duplicates.push(text.clone());
        }
        previous = Some(text);
    }

    // --- labels and boundaries ---------------------------------------------
    //
    // Content agreement asks whether the characters are there. These ask whether
    // they are there *as the right thing*: a heading is a heading and not a
    // paragraph that swallowed it, a table's cells are cells and not prose, and a
    // block that is not body text does not hold body text. The two tools segment
    // differently, so nothing here compares item counts or boundaries by index:
    // a reference item is anchored in ours by its characters alone.
    let ours_texts = labelled_texts(ours);
    let reference_texts = labelled_texts(reference);

    let mut headings_kept = 0usize;
    let mut headings_demoted = Vec::new();
    for (label, text) in &reference_texts {
        if !HEADING_LABELS.contains(&label.as_str()) || text.chars().count() < 3 {
            continue;
        }
        let holders: Vec<&str> = ours_texts
            .iter()
            .filter(|(_, ours_text)| ours_text.contains(text.as_str()))
            .map(|(label, _)| label.as_str())
            .collect();
        if holders.is_empty() {
            // Not found at all: text agreement measures that loss already.
            continue;
        }
        if holders.iter().any(|label| HEADING_LABELS.contains(label)) {
            headings_kept += 1;
        } else {
            headings_demoted.push(format!("[{label}] {text:.48} read as {}", holders[0]));
        }
    }

    let mut merged_blocks = Vec::new();
    for (label, ours_text) in &ours_texts {
        if !STRUCTURAL_LABELS.contains(&label.as_str()) || ours_text.chars().count() < 20 {
            continue;
        }
        for (body_label, body_text) in &reference_texts {
            if !BODY_LABELS.contains(&body_label.as_str()) {
                continue;
            }
            // A body block of comparable size inside a heading, title or caption
            // is body text that crossed a boundary: a fragment the reference
            // split out of the same line is too short to count.
            if body_text.chars().count() >= 40
                && body_text.chars().count() * 5 >= ours_text.chars().count() * 2
                && ours_text.contains(body_text.as_str())
            {
                merged_blocks.push(format!(
                    "[{label}] {ours_text:.48} holds [{body_label}] {body_text:.40}"
                ));
                break;
            }
        }
    }

    // --- footnotes -----------------------------------------------------------
    //
    // A note is a judgement the two tools can disagree about, so nothing here
    // fails on a disagreement. What is measured is whether a note the reference
    // calls one is a note here, or text, or gone. A line read as body text is
    // the interesting failure: the page drew its rule and its marker and we
    // read past both.
    let mut footnotes_theirs = 0usize;
    let mut footnotes_kept = 0usize;
    let mut footnotes_demoted = Vec::new();
    let mut footnotes_missing = 0usize;
    let mut footnotes_lost = Vec::new();
    for (label, text) in &reference_texts {
        if label != "footnote" || text.chars().count() < 8 {
            continue;
        }
        footnotes_theirs += 1;
        // The note's *opening*, not the whole of it: the two tools cut a note
        // differently — ours may hold a fragment of the reference's text or the
        // reference a fragment of ours — and a note is identified by what it
        // opens with anyway, which is the marker the pass looks for.
        let opening: String = text.chars().take(24).collect();
        let holders: Vec<&str> = ours_texts
            .iter()
            .filter(|(_, ours_text)| ours_text.contains(opening.as_str()))
            .map(|(label, _)| label.as_str())
            .collect();
        if holders.is_empty() {
            footnotes_missing += 1;
            footnotes_lost.push(format!("{text:.60}"));
        } else if holders.contains(&"footnote") {
            footnotes_kept += 1;
        } else {
            footnotes_demoted.push(format!("[{label}] {text:.60} read as {}", holders[0]));
        }
    }

    let our_cell_anchors: Vec<String> = ours["tables"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|table| {
            table["data"]["table_cells"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .map(|cell| anchor(cell["text"].as_str().unwrap_or_default()))
        .collect();
    let our_table_boxes: Vec<(i64, [f64; 4])> = ours["tables"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(item_box)
        .collect();

    let mut table_cells_in_tables = 0usize;
    let mut table_cells_in_prose = Vec::new();
    let mut table_cells_uncovered = 0usize;
    for table in reference["tables"].as_array().into_iter().flatten() {
        for cell in table["data"]["table_cells"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let text = cell["text"].as_str().unwrap_or_default();
            let cell_anchor = anchor(text);
            if cell_anchor.chars().count() < 12 {
                continue;
            }
            if our_cell_anchors
                .iter()
                .any(|ours_cell| ours_cell.contains(cell_anchor.as_str()))
            {
                table_cells_in_tables += 1;
                continue;
            }
            // Where we drew a table is where the reference's cells have to be:
            // prose under one is a cell on the wrong side of a boundary.
            let covered = item_box(cell).is_some_and(|cell_box| {
                our_table_boxes
                    .iter()
                    .any(|table| boxes_overlap(*table, cell_box))
            });
            if covered {
                table_cells_in_prose.push(format!("{text:.60}"));
            } else {
                table_cells_uncovered += 1;
            }
        }
    }
    notes.push(format!(
        "labels: {headings_kept} heading(s) kept, {} demoted; {} block(s) merged across a boundary; \
         table cells {table_cells_in_tables} in a table, {} read as prose, {table_cells_uncovered} uncovered",
        headings_demoted.len(),
        merged_blocks.len(),
        table_cells_in_prose.len()
    ));
    notes.push(format!(
        "footnotes: {footnotes_kept}/{footnotes_theirs} kept, {} read as text, {footnotes_missing} not found",
        footnotes_demoted.len()
    ));

    let labels = |document: &Value| -> Vec<(String, usize)> {
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for item in document["texts"].as_array().into_iter().flatten() {
            let label = item["label"].as_str().unwrap_or("?").to_string();
            *counts.entry(label).or_default() += 1;
        }
        counts.into_iter().collect()
    };
    notes.push(format!("labels ours {:?}", labels(ours)));
    notes.push(format!("labels Docling {:?}", labels(reference)));

    // Formula notation: each reference equation is looked for among ours by
    // its LaTeX with whitespace removed, so `- \infty  < x <  \infty` and
    // `- \infty <x< \infty` agree while a different command does not.
    let notation = |text: &str| -> String {
        text.chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    };
    let ours_formulas: std::collections::BTreeSet<String> = ours["texts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["label"].as_str() == Some("formula"))
        .filter_map(|item| item["text"].as_str())
        .map(notation)
        .collect();
    let formulas_reproduced = reference["texts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["label"].as_str() == Some("formula"))
        .filter_map(|item| item["text"].as_str())
        .filter(|text| text.chars().any(char::is_alphanumeric))
        .filter(|text| ours_formulas.contains(&notation(text)))
        .count();

    Agreement {
        pages_ours,
        pages_theirs,
        text_agreement,
        formulas_reproduced,
        headings_kept,
        headings_demoted,
        merged_blocks,
        table_cells_in_tables,
        table_cells_in_prose,
        table_cells_uncovered,
        footnotes_theirs,
        footnotes_kept,
        footnotes_demoted,
        footnotes_missing,
        footnotes_lost,
        formulas_ours: count(ours, "formula"),
        formulas_theirs: count(reference, "formula"),
        formulas_with_text_theirs: reference["texts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["label"].as_str() == Some("formula"))
            .filter(|item| {
                item["text"]
                    .as_str()
                    .is_some_and(|text| text.chars().any(char::is_alphanumeric))
            })
            .count(),
        duplicates,
        notes,
    }
}

/// A text reduced to what survives both tools' segmentation.
///
/// Docling splits a paragraph at every formatting run and we keep it whole, so
/// the two never agree on where an item begins and ends. What they do agree on
/// is the characters: an item of one is anchored in the other by its
/// alphanumerics alone, which also makes hyphenation and line breaks irrelevant.
fn anchor(text: &str) -> String {
    without_inline_math(text)
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

/// A document's text items as `(label, anchor)`, formulas excluded.
fn labelled_texts(document: &Value) -> Vec<(String, String)> {
    document["texts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["label"].as_str() != Some("formula"))
        .map(|item| {
            (
                item["label"].as_str().unwrap_or("?").to_string(),
                anchor(item["text"].as_str().unwrap_or_default()),
            )
        })
        .collect()
}

/// A text item's page and box, when it has one. Flow formats (DOCX, `.doc`,
/// Markdown) carry no geometry, so this is `None` for them.
fn item_box(value: &Value) -> Option<(i64, [f64; 4])> {
    let prov = value["prov"].as_array()?.first()?;
    let page = prov["page_no"].as_i64()?;
    let bbox = prov.get("bbox")?;
    Some((
        page,
        [
            bbox["l"].as_f64()?,
            bbox["t"].as_f64()?,
            bbox["r"].as_f64()?,
            bbox["b"].as_f64()?,
        ],
    ))
}

/// Whether two boxes on the same page overlap.
fn boxes_overlap(one: (i64, [f64; 4]), other: (i64, [f64; 4])) -> bool {
    one.0 == other.0
        && one.1[0] < other.1[2]
        && other.1[0] < one.1[2]
        && one.1[1] < other.1[3]
        && other.1[1] < one.1[3]
}

/// The alphanumeric characters of a document, as a multiset.
///
/// Formula items are skipped because they are compared as notation separately;
/// their LaTeX command letters are not words. Inline math between dollar signs
/// in a table cell is removed for the same reason. A calendar value is removed
/// too, for the reason `without_calendar_values` gives.
fn characters(document: &Value) -> std::collections::BTreeMap<char, usize> {
    let mut counts = std::collections::BTreeMap::new();
    let mut add = |text: &str| {
        let text = without_calendar_values(&without_inline_math(text));
        for character in text.chars().filter(|c| c.is_alphanumeric()) {
            *counts.entry(character).or_insert(0usize) += 1;
        }
    };
    for item in document["texts"].as_array().into_iter().flatten() {
        if item["label"].as_str() == Some("formula") {
            continue;
        }
        add(item["text"].as_str().unwrap_or_default());
    }
    for table in document["tables"].as_array().into_iter().flatten() {
        for cell in table["data"]["table_cells"]
            .as_array()
            .into_iter()
            .flatten()
        {
            add(cell["text"].as_str().unwrap_or_default());
        }
    }
    counts
}

/// `text` with Docling's `$...$` inline math removed.
///
/// The dollars have to *pair up*. A document that prints a price (`$15,750`) or
/// a fund election (`...want $3 to go to this fund`) has a single `$` in an
/// item, and reading it as a delimiter swallowed the rest of that item — which
/// made the IRS form look like a 29% loss when our output held every character
/// of it, and did the same damage to the reference (bug 48). An unpaired dollar
/// is therefore literal text.
fn without_inline_math(text: &str) -> String {
    let total = text.matches('$').count();
    let unpaired_last = total % 2 == 1;
    let mut seen = 0usize;
    let mut in_math = false;
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if character == '$' {
            seen += 1;
            if unpaired_last && seen == total {
                out.push(character);
                continue;
            }
            in_math = !in_math;
            continue;
        }
        if !in_math {
            out.push(character);
        }
    }
    out
}

/// `text` with every calendar value written with numeric separators removed.
///
/// The same rule as the panel's (`without_calendar_values` in
/// `crates/ui/src/diff.rs`), carried here because this comparison has its own copy of
/// the character metric and the two figures must not drift apart. A `DATE` field has
/// no text of its own: a legacy `.doc` read through LibreOffice holds the day *that*
/// conversion ran, the reference holds the day Docling's did, and counting the digits
/// of the stamp made the figure a reading of the calendar rather than of the document.
fn without_calendar_values(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < characters.len() {
        match calendar_value(&characters, index) {
            Some(end) => index = end,
            None => {
                out.push(characters[index]);
                index += 1;
            }
        }
    }
    out
}

/// Where the calendar value that starts at `start` ends, or `None` when what starts
/// there is not one. A run is `a/b/c` with both separators the same, a four-digit year
/// at one end and a month or day in the middle (`10/12/2026`, `4-26-2006`,
/// `2026.10.12`); a two-digit year is text, because `1.2.3` is a version as often as
/// it is a date.
fn calendar_value(characters: &[char], start: usize) -> Option<usize> {
    let digit = |index: usize| characters.get(index).is_some_and(char::is_ascii_digit);
    let separator = |index: usize| matches!(characters.get(index), Some('/' | '-' | '.'));
    if start > 0 && digit(start - 1) {
        return None;
    }
    let group = |index: usize| {
        let mut end = index;
        while digit(end) {
            end += 1;
        }
        (index..end, end)
    };
    let (first, after_first) = group(start);
    if first.is_empty() || first.len() > 4 || !separator(after_first) {
        return None;
    }
    let (second, after_second) = group(after_first + 1);
    if second.is_empty()
        || second.len() > 2
        || characters.get(after_first) != characters.get(after_second)
        || !separator(after_second)
    {
        return None;
    }
    let (third, end) = group(after_second + 1);
    if third.is_empty() || third.len() > 4 || digit(end) {
        return None;
    }
    if first.len() != 4 && third.len() != 4 {
        return None;
    }
    Some(end)
}

/// How many mislabelled headings and boundary-merged blocks each document is
/// allowed, from the run that added the label comparison.
///
/// Everything not listed is allowed none. These are measurements of defects we
/// know about, not targets: the comparison prints them on every run and
/// `test-data/golden/README.md` records why each one is there. They may only
/// shrink.
const LABEL_BUDGETS: [(&str, usize, usize); 9] = [
    ("drawing", 15, 0),
    ("math_seq2seq", 10, 0),
    ("math_layernorm", 4, 0),
    ("irs_f1040", 1, 4),
    ("FloatingPictures", 3, 0),
    ("math_gan", 3, 0),
    ("arxiv_attention", 0, 1),
    ("dummy", 1, 0),
    ("tracemonkey", 1, 0),
];

/// How many of each document's footnotes must be *held* — labelled `footnote`
/// or, failing that, still in the body — and how many must be **labelled**, by
/// stem: `(stem, held, labelled)`.
///
/// Floors, not targets: what this comparison measured, so a regression is a failure
/// and an improvement is free. Every one is the whole of Docling's count held, and
/// nothing is missing. The second floor is on the notes we *label*, which is the
/// one the symbol markers earned: a note whose mark the page drew and whose text we
/// hold as a paragraph still counts as held, so without it a font reading that
/// stopped resolving `\u{2217}`/`\u{2020}`/`\u{2021}` would pass. `tracemonkey`'s
/// two stay at zero because their pages draw no rule above them; both are recorded
/// in `test-data/golden/README.md`.
const FOOTNOTE_BUDGETS: [(&str, usize, usize); 5] = [
    ("arxiv_attention", 5, 5),
    ("math_layernorm", 5, 5),
    ("math_seq2seq", 9, 9),
    ("math_gan", 4, 4),
    ("tracemonkey", 2, 0),
];

/// Compare against every real Docling conversion that has been generated.
///
/// None are committed yet — see `test-data/golden/README.md` for why and for the
/// command that generates them. The comparison itself is exercised by the tests
/// below, so this one is a matter of the files existing, not of the code working.
#[test]
fn our_output_agrees_with_real_docling_conversions() {
    let directory = golden_dir().join("docling");
    let references: Vec<PathBuf> = std::fs::read_dir(&directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
                .collect()
        })
        .unwrap_or_default();

    if references.is_empty() {
        println!(
            "no real Docling conversions in {} — see test-data/golden/README.md",
            directory.display()
        );
        return;
    }

    let scratch = Scratch::new("references");
    let mut mismatches = Vec::new();
    for reference in &references {
        let stem = reference
            .file_stem()
            .expect("a name")
            .to_string_lossy()
            .into_owned();
        let input = corpus_input(&stem);

        let out = scratch.0.join(&stem);
        std::fs::create_dir_all(&out).expect("create the output dir");
        let status = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
            .args([
                "-i",
                input.to_str().expect("a UTF-8 path"),
                "-o",
                out.join("out.json").to_str().expect("a UTF-8 path"),
            ])
            .status()
            .expect("run the CLI binary");
        assert!(status.success(), "{stem} did not convert");

        let ours: Value = serde_json::from_str(
            &std::fs::read_to_string(out.join("out.json")).expect("read our output"),
        )
        .expect("our output is JSON");
        let theirs: Value =
            serde_json::from_str(&std::fs::read_to_string(reference).expect("read the reference"))
                .expect("the reference is JSON");

        let agreement = compare(&ours, &theirs);
        println!(
            "{stem}: {}/{} pages, text agreement {:.3}, formulas {}/{} ({} in Docling's notation)",
            agreement.pages_ours,
            agreement.pages_theirs,
            agreement.text_agreement,
            agreement.formulas_ours,
            agreement.formulas_theirs,
            agreement.formulas_reproduced
        );
        println!(
            "    labels: {} heading(s) kept, {} read as body text, {} block(s) merged, \
             table cells {} in a table / {} prose / {} uncovered",
            agreement.headings_kept,
            agreement.headings_demoted.len(),
            agreement.merged_blocks.len(),
            agreement.table_cells_in_tables,
            agreement.table_cells_in_prose.len(),
            agreement.table_cells_uncovered
        );
        if agreement.pages_ours != agreement.pages_theirs {
            mismatches.push(format!(
                "{stem}: {} page(s), Docling saw {}",
                agreement.pages_ours, agreement.pages_theirs
            ));
        }
        if agreement.text_agreement < 0.85 {
            mismatches.push(format!(
                "{stem}: text agreement {:.3} (below 0.85)\n     {}",
                agreement.text_agreement,
                agreement.notes.join("\n     ")
            ));
        }
        if !agreement.duplicates.is_empty() {
            mismatches.push(format!(
                "{stem}: {} block(s) appear twice in a row, e.g. {:?}",
                agreement.duplicates.len(),
                &agreement.duplicates[..agreement.duplicates.len().min(3)]
            ));
        }

        // Labels and boundaries. Every character can be present and the
        // document still be wrong: a heading read as body text, a block that
        // merged across a boundary, a table's cells read as prose. The budgets
        // are what this comparison measured when it was written — see
        // `test-data/golden/README.md`, where each one is explained — and they
        // may only shrink, so a new mislabelled heading or a lost boundary fails.
        let budget = LABEL_BUDGETS
            .iter()
            .find(|(name, _, _)| *name == stem)
            .map_or((0, 0), |(_, headings, merged)| (*headings, *merged));
        if agreement.headings_demoted.len() > budget.0 {
            mismatches.push(format!(
                "{stem}: {} heading(s) read as body text, {} allowed\n     {}",
                agreement.headings_demoted.len(),
                budget.0,
                agreement.headings_demoted.join("\n     ")
            ));
        }
        if agreement.merged_blocks.len() > budget.1 {
            mismatches.push(format!(
                "{stem}: {} block(s) merged across a boundary, {} allowed\n     {}",
                agreement.merged_blocks.len(),
                budget.1,
                agreement.merged_blocks.join("\n     ")
            ));
        }
        if !agreement.table_cells_in_prose.is_empty() {
            mismatches.push(format!(
                "{stem}: {} table cell(s) read as prose although we drew a table there\n     {}",
                agreement.table_cells_in_prose.len(),
                agreement.table_cells_in_prose.join("\n     ")
            ));
        }
        // Footnotes. Printed for every document, failed only against the floor
        // this comparison measured: a note read as body text is the failure the
        // pass exists to prevent, and one we never found is a known limit.
        if agreement.footnotes_theirs > 0 {
            println!(
                "    footnotes: {}/{} kept, {} read as text, {} not found",
                agreement.footnotes_kept,
                agreement.footnotes_theirs,
                agreement.footnotes_demoted.len(),
                agreement.footnotes_missing
            );
            for note in &agreement.footnotes_demoted {
                println!("      read as text: {note}");
            }
            for note in &agreement.footnotes_lost {
                println!("      not found: {note}");
            }
        }
        // Two floors. The first is on the notes we *hold* — under a `footnote`
        // label or in the body — because those failures are different, and a note
        // whose characters never left the page is not the pass's doing; the second
        // is on the notes we *label*, so a marker that came back cannot quietly go
        // back to being a paragraph.
        let (held_floor, labelled_floor) = FOOTNOTE_BUDGETS
            .iter()
            .find(|(name, _, _)| *name == stem)
            .map_or((0, 0), |(_, held, labelled)| (*held, *labelled));
        let held = agreement.footnotes_kept + agreement.footnotes_demoted.len();
        if held < held_floor {
            mismatches.push(format!(
                "{stem}: {} of {} footnote(s) held ({held} with their text, {} labelled), {held_floor} required",
                held,
                agreement.footnotes_theirs,
                agreement.footnotes_kept
            ));
        }
        if agreement.footnotes_kept < labelled_floor {
            mismatches.push(format!(
                "{stem}: {} of {} footnote(s) labelled, {labelled_floor} required",
                agreement.footnotes_kept, agreement.footnotes_theirs
            ));
        }
        // A document that contains n equations must report n of them, and
        // each one in Docling's notation. (Docling itemises one equation per
        // `m:oMath`; so do we, since the comparison found us welding a
        // paragraph's equations into one item.) Empty formula boxes are not
        // equations and are not counted — see `formulas_with_text_theirs`.
        if agreement.formulas_ours < agreement.formulas_with_text_theirs {
            mismatches.push(format!(
                "{stem}: Docling found {} equation(s) with text, we found {} (of {} formula item(s) in all)",
                agreement.formulas_with_text_theirs,
                agreement.formulas_ours,
                agreement.formulas_theirs
            ));
        }
        if agreement.formulas_reproduced < agreement.formulas_with_text_theirs {
            mismatches.push(format!(
                "{stem}: {} of Docling's {} equation(s) are in its notation ({} formula item(s) in all)",
                agreement.formulas_reproduced,
                agreement.formulas_with_text_theirs,
                agreement.formulas_theirs
            ));
        }
    }

    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// The `docling-core` model itself, when a Python with it installed is pointed
/// at. This is the authoritative check — it is the library that defines the
/// format — but it is not required to build the project.
#[test]
fn docling_core_accepts_our_documents() {
    let Ok(python) = std::env::var("DOCLING_PY") else {
        println!(
            "skipped: set DOCLING_PY to a python with docling-core installed, e.g.\n  \
             DOCLING_PY=~/.venvs/pdfextractor-docling/bin/python cargo test -p pdfextractor-cli \
             --test docling_golden_test"
        );
        return;
    };

    let script = golden_dir().join("validate_with_docling.py");
    assert!(script.exists(), "missing {}", script.display());

    let scratch = Scratch::new("oracle");
    convert_corpus(&scratch.0);
    // The fixtures carry shapes the corpus has not: a workbook's sheet groups
    // and a deck's slide groups. Docling's own model decides whether those are
    // a document too.
    let fixtures = scratch.0.join("fixtures");
    std::fs::create_dir_all(&fixtures).expect("create the fixture output dir");
    for (file, stem) in [
        ("inventory.xlsx", "inventory"),
        ("q3_review.pptx", "q3_review"),
        ("invoice_scan.png", "invoice_scan"),
    ] {
        let input = repository_root().join("test-data").join(file);
        let status = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
            .args([
                "-i",
                input.to_str().expect("a UTF-8 path"),
                "-o",
                fixtures
                    .join(format!("{stem}.json"))
                    .to_str()
                    .expect("a UTF-8 path"),
            ])
            .status()
            .expect("run the CLI binary");
        assert!(status.success(), "{file} did not convert");
    }
    let output = Command::new(&python)
        .arg(&script)
        .arg(&scratch.0)
        .output()
        .unwrap_or_else(|e| panic!("could not run {python}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("{stdout}");
    assert!(
        output.status.success(),
        "docling-core rejected our documents:\n{}{}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}

// --- the comparison, exercised on documents that do and do not agree -------

mod comparison {
    use super::*;
    use serde_json::json;

    fn document(texts: Vec<Value>, pages: Vec<(&str, f64, f64)>) -> Value {
        let pages = pages
            .iter()
            .map(|(no, width, height)| {
                (
                    no.to_string(),
                    json!({"page_no": no.parse::<i64>().unwrap(), "size": {"width": width, "height": height}}),
                )
            })
            .collect::<Map<String, Value>>();
        json!({
            "schema_name": "DoclingDocument",
            "version": "1.10.0",
            "name": "x",
            "pages": pages,
            "texts": texts,
            "tables": [],
            "pictures": [],
        })
    }

    fn paragraph(text: &str) -> Value {
        json!({"self_ref": "#/texts/0", "label": "paragraph", "text": text, "orig": text, "prov": []})
    }

    #[test]
    fn a_matching_document_agrees() {
        let ours = document(
            vec![paragraph("Hello World"), paragraph("Second line")],
            vec![("1", 612.0, 792.0)],
        );
        let theirs = document(
            vec![paragraph("Hello World Second line")],
            vec![("1", 612.0, 792.0)],
        );
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.pages_ours, agreement.pages_theirs);
        assert!(agreement.text_agreement > 0.99, "{agreement:?}");
    }

    #[test]
    fn a_missing_page_is_caught() {
        let ours = document(vec![paragraph("Hello")], vec![]);
        let theirs = document(vec![paragraph("Hello")], vec![("1", 612.0, 792.0)]);
        let agreement = compare(&ours, &theirs);
        assert_ne!(agreement.pages_ours, agreement.pages_theirs);
    }

    #[test]
    fn a_page_of_a_different_size_is_noted() {
        let ours = document(vec![paragraph("Hello")], vec![("1", 595.0, 842.0)]);
        let theirs = document(vec![paragraph("Hello")], vec![("1", 612.0, 792.0)]);
        let agreement = compare(&ours, &theirs);
        assert!(
            agreement
                .notes
                .iter()
                .any(|note| note.contains("more than a point apart")),
            "{agreement:?}"
        );
    }

    #[test]
    fn lost_text_is_measured() {
        let ours = document(vec![paragraph("Hello")], vec![("1", 612.0, 792.0)]);
        let theirs = document(
            vec![paragraph("Hello World and everything else")],
            vec![("1", 612.0, 792.0)],
        );
        let agreement = compare(&ours, &theirs);
        assert!(agreement.text_agreement < 0.6, "{agreement:?}");
    }

    fn formula(text: &str) -> Value {
        json!({"self_ref": "#/texts/0", "label": "formula", "text": text, "orig": text, "prov": []})
    }

    /// Docling writes LaTeX for an equation and so do we; the same equation in
    /// the same notation agrees even when spacing differs.
    #[test]
    fn formula_notation_is_compared_ignoring_whitespace() {
        let ours = document(vec![formula("\\iiint_{V}^{}divFdV")], vec![]);
        let theirs = document(vec![formula("\\iiint_{V}^{} divFdV")], vec![]);
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.text_agreement, 1.0, "{agreement:?}");
        assert_eq!(
            (
                agreement.formulas_ours,
                agreement.formulas_theirs,
                agreement.formulas_reproduced
            ),
            (1, 1, 1)
        );
    }

    /// A formula written in another notation is still an equation, but it is
    /// not in Docling's notation and the count says so.
    #[test]
    fn formula_written_another_way_is_not_reproduced() {
        let ours = document(vec![formula("∭_V divFdV")], vec![]);
        let theirs = document(vec![formula("\\iiint_{V}^{}divFdV")], vec![]);
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.formulas_reproduced, 0, "{agreement:?}");
    }

    /// A PDF reference can hold formula boxes with no text at all — its layout
    /// model's idea of where an equation is, with nothing recovered for it.
    /// Those are regions, not equations, and the count we have to meet is the
    /// one that carries text.
    #[test]
    fn an_empty_formula_box_is_not_an_equation_to_find() {
        let ours = document(vec![paragraph("Body")], vec![]);
        let theirs = document(vec![paragraph("Body"), formula(""), formula("   ")], vec![]);
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.formulas_theirs, 2, "{agreement:?}");
        assert_eq!(agreement.formulas_with_text_theirs, 0, "{agreement:?}");
        assert_eq!(agreement.formulas_ours, 0, "{agreement:?}");
    }

    /// Dropping an equation is a real loss, and the count catches it.
    #[test]
    fn a_dropped_equation_is_visible_in_the_counts() {
        let ours = document(vec![paragraph("Body")], vec![]);
        let theirs = document(vec![paragraph("Body"), formula("x")], vec![]);
        let agreement = compare(&ours, &theirs);
        assert_eq!((agreement.formulas_ours, agreement.formulas_theirs), (0, 1));
        assert_eq!(agreement.text_agreement, 1.0);
    }

    /// Word writes every shape twice, and reading both copies produced each
    /// shape's text twice. The check is what found it (bug 46).
    #[test]
    fn a_block_that_appears_twice_in_a_row_is_reported() {
        let ours = document(
            vec![
                paragraph("A square shape with text inside"),
                paragraph("A square shape with text inside"),
            ],
            vec![],
        );
        let theirs = document(vec![paragraph("A square shape with text inside")], vec![]);
        let agreement = compare(&ours, &theirs);
        assert_eq!(
            agreement.duplicates,
            vec!["A square shape with text inside"]
        );
    }

    /// The same words twice is a defect; the same words twice *inside a figure*
    /// is the figure drawing them twice, which is what a chart legend or the
    /// attention visualisation does by design. Both copies here sit in the
    /// picture's box, so neither is reported.
    #[test]
    fn a_figure_may_label_itself_twice() {
        let mut ours = document(
            vec![paragraph("mean Recall"), paragraph("mean Recall")],
            vec![("1", 612.0, 792.0)],
        );
        ours["texts"][0]["prov"] = json!([{
            "page_no": 1,
            "bbox": {"l": 100.0, "t": 100.0, "r": 160.0, "b": 110.0, "coord_origin": "TOPLEFT"},
        }]);
        ours["texts"][1]["prov"] = json!([{
            "page_no": 1,
            "bbox": {"l": 100.0, "t": 130.0, "r": 160.0, "b": 140.0, "coord_origin": "TOPLEFT"},
        }]);
        ours["pictures"] = json!([{
            "self_ref": "#/pictures/0",
            "label": "picture",
            "prov": [{
                "page_no": 1,
                "bbox": {"l": 90.0, "t": 90.0, "r": 200.0, "b": 200.0, "coord_origin": "TOPLEFT"},
            }],
        }]);
        let theirs = document(vec![paragraph("mean Recall")], vec![("1", 612.0, 792.0)]);
        let agreement = compare(&ours, &theirs);
        assert!(agreement.duplicates.is_empty(), "{agreement:?}");
    }

    /// ...and the same text repeated *outside* a figure is still reported, so the
    /// exemption cannot be used by a block that merely sits on a page with a
    /// picture.
    #[test]
    fn a_repeat_outside_the_figure_is_still_reported() {
        let mut ours = document(
            vec![
                paragraph("A square shape with text inside"),
                paragraph("A square shape with text inside"),
            ],
            vec![("1", 612.0, 792.0)],
        );
        for index in [0, 1] {
            ours["texts"][index]["prov"] = json!([{
                "page_no": 1,
                "bbox": {"l": 300.0, "t": 300.0, "r": 400.0, "b": 310.0, "coord_origin": "TOPLEFT"},
            }]);
        }
        ours["pictures"] = json!([{
            "self_ref": "#/pictures/0",
            "label": "picture",
            "prov": [{
                "page_no": 1,
                "bbox": {"l": 90.0, "t": 90.0, "r": 200.0, "b": 200.0, "coord_origin": "TOPLEFT"},
            }],
        }]);
        let theirs = document(
            vec![paragraph("A square shape with text inside")],
            vec![("1", 612.0, 792.0)],
        );
        let agreement = compare(&ours, &theirs);
        assert_eq!(
            agreement.duplicates,
            vec!["A square shape with text inside"]
        );
    }

    /// The page and `[l, t, r, b]` box an item was read from, when the case has
    /// one.
    type Box_ = (i64, [f64; 4]);

    fn labelled(label: &str, text: &str, box_: Option<Box_>) -> Value {
        let prov = match box_ {
            Some((page, [l, t, r, b])) => json!([{
                "page_no": page,
                "bbox": {"l": l, "t": t, "r": r, "b": b, "coord_origin": "TOPLEFT"},
            }]),
            None => json!([]),
        };
        json!({"self_ref": "#/texts/0", "label": label, "text": text, "orig": text, "prov": prov})
    }

    fn table(cells: &[(&str, Option<Box_>)], box_: Option<Box_>) -> Value {
        let prov = match box_ {
            Some((page, [l, t, r, b])) => json!([{
                "page_no": page,
                "bbox": {"l": l, "t": t, "r": r, "b": b, "coord_origin": "TOPLEFT"},
            }]),
            None => json!([]),
        };
        let cells: Vec<Value> = cells
            .iter()
            .enumerate()
            .map(|(index, (text, cell_box))| {
                let cell_prov = match cell_box {
                    Some((page, [l, t, r, b])) => json!([{
                        "page_no": page,
                        "bbox": {"l": l, "t": t, "r": r, "b": b, "coord_origin": "TOPLEFT"},
                    }]),
                    None => json!([]),
                };
                json!({
                    "row_span": 1,
                    "col_span": 1,
                    "start_row_offset_idx": index,
                    "end_row_offset_idx": index + 1,
                    "start_col_offset_idx": 0,
                    "end_col_offset_idx": 1,
                    "text": text,
                    "prov": cell_prov,
                })
            })
            .collect();
        json!({
            "self_ref": "#/tables/0",
            "label": "table",
            "prov": prov,
            "data": {"num_rows": 1, "num_cols": cells.len(), "table_cells": cells},
        })
    }

    const BODY: &str = "The encoder is composed of a stack of identical layers.";

    /// A heading the reference saw and we kept as a heading is not reported.
    #[test]
    fn a_heading_kept_as_a_heading_is_not_reported() {
        let mut ours = document(
            vec![
                labelled("section_header", "3.1 Encoder and Decoder Stacks", None),
                labelled("paragraph", BODY, None),
            ],
            vec![],
        );
        ours["pictures"] = json!([]);
        let theirs = document(
            vec![
                labelled("section_header", "3.1 Encoder and Decoder Stacks", None),
                labelled("text", BODY, None),
            ],
            vec![],
        );
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.headings_kept, 1, "{agreement:?}");
        assert!(agreement.headings_demoted.is_empty(), "{agreement:?}");
    }

    /// The characters can all be present and the heading still be wrong: read as
    /// body text, and the boundary between it and its body was not drawn.
    #[test]
    fn a_heading_read_as_body_text_is_reported() {
        let ours = document(
            vec![labelled(
                "paragraph",
                "3.1 Encoder and Decoder Stacks The encoder is composed of a stack of identical layers.",
                None,
            )],
            vec![],
        );
        let theirs = document(
            vec![
                labelled("section_header", "3.1 Encoder and Decoder Stacks", None),
                labelled("text", BODY, None),
            ],
            vec![],
        );
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.headings_kept, 0, "{agreement:?}");
        assert_eq!(agreement.headings_demoted.len(), 1, "{agreement:?}");
    }

    /// A heading that holds its own paragraph merged across a boundary. The
    /// characters are all there; only the boundary is missing.
    #[test]
    fn a_heading_that_swallows_its_body_lost_a_boundary() {
        let mut ours = document(
            vec![labelled(
                "section_header",
                "3.1 Encoder and Decoder Stacks The encoder is composed of a stack of identical layers.",
                None,
            )],
            vec![],
        );
        ours["pictures"] = json!([]);
        let theirs = document(
            vec![
                labelled("section_header", "3.1 Encoder and Decoder Stacks", None),
                labelled("text", BODY, None),
            ],
            vec![],
        );
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.merged_blocks.len(), 1, "{agreement:?}");
    }

    /// A reference fragment too short to be a block is not a lost boundary:
    /// Docling splits a paragraph at every formatting run, and each fragment
    /// sits inside our paragraph by construction.
    #[test]
    fn a_short_fragment_is_not_a_lost_boundary() {
        let mut ours = document(
            vec![labelled(
                "caption",
                "Figure 3: a stack of identical layers, drawn here",
                None,
            )],
            vec![],
        );
        ours["pictures"] = json!([]);
        let theirs = document(vec![labelled("text", "a stack", None)], vec![]);
        let agreement = compare(&ours, &theirs);
        assert!(agreement.merged_blocks.is_empty(), "{agreement:?}");
    }

    /// The text of a table cell can be found and still be wrong: we drew a table
    /// over it and read the cell as prose. That is a cell on the wrong side of a
    /// boundary, and it is reported.
    #[test]
    fn a_table_cell_read_as_prose_is_reported() {
        let cell = "First quarter revenue was twelve million dollars";
        let table_box = Some((1, [100.0, 100.0, 400.0, 200.0]));
        let mut ours = document(
            vec![labelled(
                "paragraph",
                cell,
                Some((1, [110.0, 110.0, 390.0, 130.0])),
            )],
            vec![("1", 612.0, 792.0)],
        );
        ours["tables"] = json!([table(
            &[("Second quarter revenue was eleven million dollars", None)],
            table_box,
        )]);
        let theirs = document(vec![], vec![("1", 612.0, 792.0)]);
        let mut theirs = theirs;
        theirs["tables"] = json!([table(
            &[(cell, Some((1, [110.0, 110.0, 390.0, 130.0])))],
            None
        )]);
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.table_cells_in_prose.len(), 1, "{agreement:?}");
        assert_eq!(agreement.table_cells_uncovered, 0, "{agreement:?}");
    }

    /// A cell we hold inside a table of our own is not reported.
    #[test]
    fn a_table_cell_in_a_table_is_not_reported() {
        let cell = "First quarter revenue was twelve million dollars";
        let mut ours = document(vec![], vec![("1", 612.0, 792.0)]);
        ours["tables"] = json!([table(
            &[(cell, None)],
            Some((1, [100.0, 100.0, 400.0, 200.0]))
        )]);
        let mut theirs = document(vec![], vec![("1", 612.0, 792.0)]);
        theirs["tables"] = json!([table(
            &[(cell, Some((1, [110.0, 110.0, 390.0, 130.0])))],
            None
        )]);
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.table_cells_in_tables, 1, "{agreement:?}");
        assert!(agreement.table_cells_in_prose.is_empty(), "{agreement:?}");
    }

    /// A reference table we read as prose over no table of ours is counted, not
    /// failed: our table detection is deliberately conservative, and a check
    /// that failed on it would be asserting a fix nobody has made.
    #[test]
    fn an_uncovered_table_cell_is_counted_not_failed() {
        let cell = "First quarter revenue was twelve million dollars";
        let ours = document(
            vec![labelled(
                "paragraph",
                cell,
                Some((1, [110.0, 110.0, 390.0, 130.0])),
            )],
            vec![("1", 612.0, 792.0)],
        );
        let mut theirs = document(vec![], vec![("1", 612.0, 792.0)]);
        theirs["tables"] = json!([table(
            &[(cell, Some((1, [110.0, 110.0, 390.0, 130.0])))],
            None
        )]);
        let agreement = compare(&ours, &theirs);
        assert_eq!(agreement.table_cells_uncovered, 1, "{agreement:?}");
        assert!(agreement.table_cells_in_prose.is_empty(), "{agreement:?}");
    }

    /// A price on a form is not a math delimiter: the text after an unpaired
    /// `$` has to be counted, or a document that prints one looks lossy
    /// (bug 48).
    #[test]
    fn an_unpaired_dollar_is_literal_text() {
        assert_eq!(without_inline_math("want $3 to go"), "want $3 to go");
        assert_eq!(without_inline_math("$15,750"), "$15,750");
        // A matched pair is still Docling's notation, and comes out.
        assert_eq!(without_inline_math("$x^2$ and more"), " and more");
        assert_eq!(without_inline_math("$a$ plus $b$ end"), " plus  end");
    }

    #[test]
    fn repeated_characters_have_to_be_present_twice() {
        let ours = document(vec![paragraph("aaa")], vec![("1", 612.0, 792.0)]);
        let theirs = document(vec![paragraph("aaaaaa")], vec![("1", 612.0, 792.0)]);
        let agreement = compare(&ours, &theirs);
        assert!(
            (agreement.text_agreement - 0.5).abs() < 0.01,
            "{agreement:?}"
        );
    }
}
