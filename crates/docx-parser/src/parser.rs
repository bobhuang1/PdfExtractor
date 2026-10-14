//! DOCX parsing (ECMA-376 WordprocessingML, via the OPC package).
//!
//! A `.docx` is a ZIP of XML parts. We read `word/document.xml` in document
//! order and emit blocks as we encounter them:
//!
//! * paragraphs, labelled from their style (`Heading1` -> `section_header`,
//!   numbering -> `list_item`),
//! * tables, as a rectangular grid of cell text,
//! * embedded pictures from `word/media/`, placed in order via the drawing's
//!   relationship id — including the preview of an embedded object (`w:object`,
//!   a MathType equation or a pasted drawing), and including the vector
//!   drawings Word stores as metafiles, which are played into PNGs first (see
//!   [`crate::drawing`]),
//! * a chart's title, which exists only in the chart part, as a caption,
//! * Office Math (`m:oMath`), linearised to readable text — one formula item per
//!   equation.
//!
//! Headers and footers live in separate parts (`word/header*.xml`), so they are
//! extracted as furniture rather than silently dropped; footnotes and endnotes
//! live in `word/footnotes.xml` and `word/endnotes.xml` and are body text.
//!
//! DOCX is flow-based: there is no page rectangle to point at, so blocks carry
//! no provenance. Page size comes from `w:sectPr` and is reported on the
//! document, not as a page a block sits on: Word repaginates as the text is
//! edited, so no page can be claimed.

use crate::drawing;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::{Cursor, Read};

/// English Metric Units per point (914400 EMU per inch, 72 points per inch).
const EMU_PER_POINT: f64 = 12700.0;
/// Twentieths of a point per point (twips).
const TWIPS_PER_POINT: f64 = 20.0;

/// One block of document content, in document order.
#[derive(Debug, Clone)]
pub enum DocxItem {
    Text {
        text: String,
        label: String,
        /// Bullet or numbering the level prints, when the paragraph is a list
        /// item. It is *not* part of `text`.
        marker: Option<String>,
        /// Heading depth or list nesting depth, counted from one.
        level: Option<i32>,
    },
    Formula {
        text: String,
    },
    Table {
        rows: Vec<Vec<String>>,
    },
    Picture {
        mime: &'static str,
        data: Vec<u8>,
        /// Placement size in points: the drawing's `wp:extent`, or an embedded
        /// object's original size.
        width_pt: Option<f64>,
        height_pt: Option<f64>,
    },
}

#[derive(Debug)]
pub struct DocxDocument {
    /// Page size in points, from `w:sectPr`.
    pub page_width: f64,
    pub page_height: f64,
    pub items: Vec<DocxItem>,
    /// Content from `word/header*.xml` and `word/footer*.xml`.
    pub furniture: Vec<DocxItem>,
}

impl Default for DocxDocument {
    fn default() -> Self {
        Self {
            // Letter, the same fallback Word applies when `w:sectPr` is absent.
            page_width: 612.0,
            page_height: 792.0,
            items: Vec::new(),
            furniture: Vec::new(),
        }
    }
}

impl DocxDocument {
    pub fn parse_file(path: &str) -> anyhow::Result<Self> {
        let data = std::fs::read(path)?;
        Self::parse_bytes(&data)
    }

    pub fn parse_bytes(data: &[u8]) -> anyhow::Result<Self> {
        let mut archive = zip::ZipArchive::new(Cursor::new(data))
            .map_err(|e| anyhow::anyhow!("not a DOCX (ZIP) package: {e}"))?;

        // Snapshot every media part up front. Holding the archive open while the
        // extractor walks the XML would tangle the mutable borrows for no gain.
        let mut media: HashMap<String, Vec<u8>> = HashMap::new();
        for i in 0..archive.len() {
            let mut file = match archive.by_index(i) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let name = file.name().to_string();
            if !name.starts_with("word/media/") {
                continue;
            }
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_ok() {
                media.insert(name, buf);
            }
        }

        // A chart's title is page content that exists nowhere else: the chart
        // part is the only place the words are written down. Docling reports
        // them, and dropping them loses text a reader sees (bug 46).
        let mut charts: HashMap<String, Vec<u8>> = HashMap::new();
        for i in 0..archive.len() {
            let mut file = match archive.by_index(i) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let name = file.name().to_string();
            if !name.starts_with("word/charts/") || !name.ends_with(".xml") {
                continue;
            }
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_ok() {
                charts.insert(name, buf);
            }
        }

        let rels = read_relationships(&mut archive, "word/_rels/document.xml.rels")?;
        let main = read_part(&mut archive, "word/document.xml")
            .ok_or_else(|| anyhow::anyhow!("package has no word/document.xml"))?;

        let numbering = read_numbering(&mut archive);

        let mut doc = DocxDocument::default();
        let mut extractor = Extractor::new(&rels, &media, &charts, &numbering);
        extractor.run(&main);
        doc.page_width = extractor.page_width;
        doc.page_height = extractor.page_height;
        doc.items = extractor.items;
        // Headers and footers live in their own parts; report them as furniture
        // rather than dropping their text and images on the floor.
        let mut part_names: Vec<String> = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .filter(|n| {
                (n.starts_with("word/header") || n.starts_with("word/footer"))
                    && n.ends_with(".xml")
            })
            .collect();
        part_names.sort();
        let mut extra = Vec::new();
        for name in part_names {
            let bytes = match read_part(&mut archive, &name) {
                Some(b) => b,
                None => continue,
            };
            let sub_rels = read_relationships(
                &mut archive,
                &format!("word/_rels/{}.rels", name.trim_start_matches("word/")),
            )?;
            let mut sub = Extractor::new(&sub_rels, &media, &charts, &numbering);
            sub.run(&bytes);
            extra.extend(sub.items);
        }
        doc.furniture = extra;

        // Footnotes and endnotes are their own parts, and their text is body
        // content a reader sees. Docling reports both as `footnote` items at the
        // end of the document, which is where they are appended here.
        for part in ["word/footnotes.xml", "word/endnotes.xml"] {
            let bytes = match read_part(&mut archive, part) {
                Some(b) => b,
                None => continue,
            };
            let sub_rels = read_relationships(
                &mut archive,
                &format!("word/_rels/{}.rels", part.trim_start_matches("word/")),
            )?;
            let mut sub = Extractor::new(&sub_rels, &media, &charts, &numbering);
            sub.run(&bytes);
            for mut item in sub.items {
                if let DocxItem::Text { label, .. } = &mut item {
                    *label = "footnote".to_string();
                }
                doc.items.push(item);
            }
        }

        Ok(doc)
    }
}

fn read_part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut file = archive.by_name(name).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Map relationship ids to part names, e.g. `rId5` -> `word/media/image1.png`.
fn read_relationships(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    rels_path: &str,
) -> anyhow::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    let bytes = match read_part(archive, rels_path) {
        Some(b) => b,
        None => return Ok(out),
    };
    let mut reader = Reader::from_reader(Cursor::new(bytes));
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == b"Relationship" {
                    // Only package-internal targets resolve inside the ZIP.
                    let external = attr(&e, b"TargetMode")
                        .map(|m| m.eq_ignore_ascii_case("external"))
                        .unwrap_or(false);
                    if !external {
                        if let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                            out.insert(id, resolve_target(target));
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("malformed {rels_path}: {e}"),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// One numbering level, as `word/numbering.xml` defines it.
#[derive(Debug, Clone, Default, PartialEq)]
struct NumberingLevel {
    /// `w:numFmt` — `decimal`, `lowerLetter`, `upperRoman`, `bullet`, ...
    format: String,
    /// `w:lvlText`, with `%1`..`%9` standing in for the counter of that level.
    text: String,
    /// `w:start`, the value the first item at this level takes.
    start: i32,
}

/// One `w:num`: the levels it inherits from its abstract definition, plus the
/// per-list adjustments Word writes inside `w:num` itself.
///
/// Level indices are zero-based, matching `w:ilvl`; `%1` in a level text refers
/// to index 0.
#[derive(Debug, Clone, Default)]
struct NumberingDef {
    /// The `w:abstractNum` this `w:num` binds to.
    levels: HashMap<i32, NumberingLevel>,
    /// `w:lvlOverride/w:lvl`: a level redefined for this list alone, so a scheme
    /// can switch to letters - or to a different bullet - part way through.
    overrides: HashMap<i32, NumberingLevel>,
    /// `w:lvlOverride/@w:startOverride`: the value this list restarts a level at.
    starts: HashMap<i32, i32>,
}

impl NumberingDef {
    /// The definition in force for a level: an override wins over the abstract
    /// scheme it was inherited from.
    fn level(&self, index: i32) -> Option<&NumberingLevel> {
        self.overrides
            .get(&index)
            .or_else(|| self.levels.get(&index))
    }

    /// The value a level's counter starts at.
    fn start(&self, index: i32) -> i32 {
        self.starts
            .get(&index)
            .copied()
            .or_else(|| self.level(index).map(|l| l.start))
            .unwrap_or(1)
            .max(1)
    }
}

/// `numId` -> its numbering definition.
type Numbering = HashMap<i32, NumberingDef>;

/// File a level definition under whichever list is currently open: an
/// `w:abstractNum` when one is being read, otherwise the `w:lvlOverride`.
fn store_level(
    abstracts: &mut HashMap<i32, HashMap<i32, NumberingLevel>>,
    overrides: &mut HashMap<i32, HashMap<i32, NumberingLevel>>,
    abstract_id: Option<i32>,
    override_at: Option<(i32, i32)>,
    index: Option<i32>,
    level: Option<NumberingLevel>,
) {
    let (Some(index), Some(level)) = (index, level) else {
        return;
    };
    if let Some((num, ilvl)) = override_at {
        overrides.entry(num).or_default().insert(ilvl, level);
    } else if let Some(id) = abstract_id {
        abstracts.entry(id).or_default().insert(index, level);
    }
}

/// Read `word/numbering.xml`.
///
/// Word splits the definitions in two: `w:abstractNum` holds the levels and
/// `w:num` binds an `abstractNumId` to the `numId` a paragraph refers to. Both
/// halves are needed before a list item can be expanded. A `w:lvlOverride`
/// inside `w:num` redefines (or restarts) a level for that one list, and it
/// overrides the abstract scheme it came from.
fn read_numbering(archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Numbering {
    let Some(bytes) = read_part(archive, "word/numbering.xml") else {
        return Numbering::new();
    };
    let mut abstracts: HashMap<i32, HashMap<i32, NumberingLevel>> = HashMap::new();
    let mut num_to_abstract: HashMap<i32, i32> = HashMap::new();
    // Collected for a `w:num` as they are read, then attached once the abstract
    // schemes are known.
    let mut overrides: HashMap<i32, HashMap<i32, NumberingLevel>> = HashMap::new();
    let mut overridden_starts: HashMap<i32, HashMap<i32, i32>> = HashMap::new();

    let mut current_abstract: Option<i32> = None;
    let mut current_num: Option<i32> = None;
    // `(numId, ilvl)` while a `w:lvlOverride` is open.
    let mut current_override: Option<(i32, i32)> = None;
    let mut level_index: Option<i32> = None;
    let mut level: Option<NumberingLevel> = None;

    let mut reader = Reader::from_reader(Cursor::new(bytes));
    let mut buf = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buf);
        match event {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let close_level =
                    |abstracts: &mut HashMap<i32, HashMap<i32, NumberingLevel>>,
                     abstract_id: Option<i32>,
                     index: Option<i32>,
                     level: Option<NumberingLevel>| {
                        if let (Some(id), Some(index), Some(level)) = (abstract_id, index, level) {
                            abstracts.entry(id).or_default().insert(index, level);
                        }
                    };
                // The level in hand belongs to whichever list is open: an
                // abstract scheme, or the override inside a `w:num`.
                let close_current =
                    |overrides: &mut HashMap<i32, HashMap<i32, NumberingLevel>>,
                     abstracts: &mut HashMap<i32, HashMap<i32, NumberingLevel>>,
                     abstract_id: Option<i32>,
                     override_at: Option<(i32, i32)>,
                     index: Option<i32>,
                     level: Option<NumberingLevel>| {
                        store_level(abstracts, overrides, abstract_id, override_at, index, level);
                    };
                match local_name(e.name().as_ref()) {
                    b"abstractNum" => {
                        close_level(&mut abstracts, current_abstract, level_index, level.take());
                        level_index = None;
                        current_abstract =
                            attr_local(&e, b"abstractNumId").and_then(|v| v.parse().ok());
                        if let Some(id) = current_abstract {
                            abstracts.entry(id).or_default();
                        }
                    }
                    b"lvlOverride" => {
                        if let (Some(num), Some(ilvl)) = (
                            current_num,
                            attr_local(&e, b"ilvl").and_then(|v| v.parse::<i32>().ok()),
                        ) {
                            current_override = Some((num, ilvl));
                            // `w:startOverride` restarts the list here: the
                            // counter begins at this value instead of the
                            // abstract `w:start`.
                            if let Some(start) =
                                attr_local(&e, b"startOverride").and_then(|v| v.parse::<i32>().ok())
                            {
                                overridden_starts
                                    .entry(num)
                                    .or_default()
                                    .insert(ilvl, start);
                            }
                        }
                    }
                    b"lvl" => {
                        close_current(
                            &mut overrides,
                            &mut abstracts,
                            current_abstract,
                            current_override,
                            level_index,
                            level.take(),
                        );
                        // Inside an override the `w:lvlOverride/@w:ilvl` is the
                        // index; stand-alone levels carry their own.
                        level_index = current_override
                            .map(|(_, ilvl)| ilvl)
                            .or_else(|| attr_local(&e, b"ilvl").and_then(|v| v.parse().ok()));
                        level = Some(NumberingLevel {
                            start: 1,
                            ..Default::default()
                        });
                    }
                    b"start" => {
                        if let Some(l) = level.as_mut() {
                            l.start = attr_local(&e, b"val")
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(1);
                        }
                    }
                    b"numFmt" => {
                        if let Some(l) = level.as_mut() {
                            l.format = attr_local(&e, b"val").unwrap_or_default();
                        }
                    }
                    b"lvlText" => {
                        if let Some(l) = level.as_mut() {
                            l.text = attr_local(&e, b"val").unwrap_or_default();
                        }
                    }
                    b"num" => {
                        current_num = attr_local(&e, b"numId").and_then(|v| v.parse().ok());
                    }
                    b"abstractNumId" => {
                        if let (Some(num), Some(abstract_id)) = (
                            current_num,
                            attr_local(&e, b"val").and_then(|v| v.parse::<i32>().ok()),
                        ) {
                            num_to_abstract.insert(num, abstract_id);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => match local_name(e.name().as_ref()) {
                b"lvl" => {
                    store_level(
                        &mut abstracts,
                        &mut overrides,
                        current_abstract,
                        current_override,
                        level_index,
                        level.take(),
                    );
                    level_index = None;
                }
                b"lvlOverride" => current_override = None,
                b"abstractNum" => {
                    if let (Some(id), Some(index), Some(level)) =
                        (current_abstract, level_index, level.take())
                    {
                        abstracts.entry(id).or_default().insert(index, level);
                    }
                    level_index = None;
                    current_abstract = None;
                }
                b"num" => {
                    current_num = None;
                    current_override = None;
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    num_to_abstract
        .into_iter()
        .filter_map(|(num, abstract_id)| {
            let levels = abstracts.get(&abstract_id).cloned().unwrap_or_default();
            // A `w:num` that redefines a level is usable even when its abstract
            // scheme is missing: the override carries the whole definition.
            let overrides = overrides.remove(&num).unwrap_or_default();
            if levels.is_empty() && overrides.is_empty() {
                return None;
            }
            let def = NumberingDef {
                levels,
                overrides,
                starts: overridden_starts.remove(&num).unwrap_or_default(),
            };
            Some((num, def))
        })
        .collect()
}

/// Symbol-font bullets Word writes into the private use area.
///
/// `Symbol` and `Wingdings` glyphs are not in any text encoding, so passing
/// them through would put an invisible character in the JSON.
fn visible_bullet(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{F0B7}' | '\u{F0A7}' => '•',
            '\u{F075}' => '◆',
            '\u{F06E}' => '■',
            '\u{F0D8}' => '➢',
            '\u{F0FC}' => '✓',
            other => other,
        })
        .collect()
}

/// Format one counter value the way its level's `numFmt` asks.
fn format_counter(value: i32, format: &str) -> String {
    match format {
        "lowerLetter" => letters(value, false),
        "upperLetter" => letters(value, true),
        "lowerRoman" => roman(value, false),
        "upperRoman" => roman(value, true),
        "decimalZero" => format!("{:02}", value.max(0)),
        _ => value.to_string(),
    }
}

/// `1 -> a`, `27 -> aa`.
fn letters(mut value: i32, upper: bool) -> String {
    if value <= 0 {
        return value.to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        let digit = ((value - 1) % 26) as u8;
        out.push((b'a' + digit) as char);
        value = (value - 1) / 26;
    }
    out.reverse();
    let text: String = out.into_iter().collect();
    if upper {
        text.to_uppercase()
    } else {
        text
    }
}

/// Roman numerals, falling back to digits outside the range they express.
fn roman(value: i32, upper: bool) -> String {
    if value <= 0 || value > 3999 {
        return value.to_string();
    }
    const TABLE: [(i32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut left = value;
    let mut out = String::new();
    for (amount, digits) in TABLE {
        while left >= amount {
            out.push_str(digits);
            left -= amount;
        }
    }
    if upper {
        out.to_uppercase()
    } else {
        out
    }
}

/// Resolve a relationship target against the `word/` part directory.
fn resolve_target(target: String) -> String {
    if target.starts_with('/') {
        return target.trim_start_matches('/').to_string();
    }
    format!("word/{}", target.trim_start_matches("./"))
}

/// Strip a namespace prefix: `w:pgSz` -> `pgSz`.
fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|b| *b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

fn attr_local(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| local_name(a.key.as_ref()) == key)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

/// A picture referenced by a drawing, and where it sits.
#[derive(Default)]
struct PendingPicture {
    rel_id: Option<String>,
    /// Relationship id of a chart this drawing paints, when it paints one.
    chart_rel_id: Option<String>,
    width_pt: Option<f64>,
    height_pt: Option<f64>,
}

/// Whether an element opens a `mc:Fallback` subtree (the duplicate copy).
fn is_alternate_fallback(e: &BytesStart) -> bool {
    let name = e.name();
    name.as_ref().starts_with(b"mc:") && local_name(name.as_ref()) == b"Fallback"
}

/// One piece of a paragraph, in document order.
#[derive(Debug)]
enum ParagraphPiece {
    /// Prose, exactly as it was written.
    Text(String),
    /// An `m:oMath`, linearised.
    Math(String),
}

/// The state machine that walks one XML part in document order.
struct Extractor<'a> {
    /// Image bytes keyed by package part name.
    media: &'a HashMap<String, Vec<u8>>,
    /// Chart parts keyed by package part name, for the titles they carry.
    charts: &'a HashMap<String, Vec<u8>>,
    rels: &'a HashMap<String, String>,

    items: Vec<DocxItem>,

    // Current paragraph.
    in_paragraph: bool,
    para_text: String,
    para_has_plain_text: bool,
    para_label: String,

    /// The current paragraph, in order: prose and equations as they were
    /// written.
    ///
    /// Keeping the pieces rather than one string is what lets a paragraph of
    /// nothing but math be one equation per `m:oMath` — three equations set one
    /// after another are three equations, not one long one — and what tells an
    /// equation at the end of a paragraph apart from one inside a sentence
    /// (bugs 46 and 47).
    para_pieces: Vec<ParagraphPiece>,

    // Numbering definitions and the counters they are expanded with.
    numbering: &'a Numbering,
    para_num_id: Option<i32>,
    para_level: Option<i32>,
    counters: HashMap<(i32, i32), i32>,

    // Pictures found inside the current paragraph.
    para_pictures: Vec<PendingPicture>,
    drawing: Option<PendingPicture>,

    // Table stack: table -> rows -> cells.
    tables: Vec<Vec<Vec<String>>>,

    // Text capture: we only collect text inside a text element.
    in_text: bool,

    /// Whether a `w:pPr` is open. Tab *stops* are declared with `w:tab` inside
    /// it, and reading one of those as text would inject a tab that is not on
    /// the page.
    in_paragraph_properties: usize,

    /// Depth of an `mc:Fallback` subtree being skipped. Word writes every
    /// shape twice: the real DrawingML in `mc:Choice` and a VML copy for old
    /// readers in `mc:Fallback`. Reading both extracts every shape's text twice
    /// (bug 46).
    skip_depth: Option<usize>,

    // Office Math: a subtree is built, then linearised when it closes.
    math: Option<MathBuilder>,

    page_width: f64,
    page_height: f64,
}

/// Accumulates the element tree of one `m:oMath` run.
struct MathBuilder {
    stack: Vec<MathNode>,
}

impl MathBuilder {
    fn new() -> Self {
        Self {
            stack: vec![MathNode::default()],
        }
    }
    /// Open a node, keeping its attributes: operators and accents live there
    /// (`<m:chr m:val="∑"/>`), not in child text.
    fn start(&mut self, e: &BytesStart) {
        let mut node = MathNode {
            name: String::from_utf8_lossy(local_name(e.name().as_ref())).into_owned(),
            ..Default::default()
        };
        for a in e.attributes().flatten() {
            if let Ok(v) = a.unescape_value() {
                node.attrs.push((
                    String::from_utf8_lossy(local_name(a.key.as_ref())).into_owned(),
                    v.into_owned(),
                ));
            }
        }
        self.stack.push(node);
    }
    fn empty(&mut self, e: &BytesStart) {
        self.start(e);
        self.close();
    }
    fn close(&mut self) {
        if self.stack.len() > 1 {
            let node = self.stack.pop().expect("len checked");
            if let Some(parent) = self.stack.last_mut() {
                parent.children.push(node);
            }
        }
    }
    fn text(&mut self, t: &str) {
        if let Some(cur) = self.stack.last_mut() {
            cur.text.push_str(t);
        }
    }
    /// Close the outermost node and render it.
    fn finish(mut self) -> String {
        while self.stack.len() > 1 {
            self.close();
        }
        let root = self.stack.pop().expect("root");
        render_math_children(&root)
    }
}

#[derive(Debug, Default)]
struct MathNode {
    /// Local (namespace-stripped) element name.
    name: String,
    /// Local-name keyed attributes.
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<MathNode>,
}

impl MathNode {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

impl<'a> Extractor<'a> {
    fn new(
        rels: &'a HashMap<String, String>,
        media: &'a HashMap<String, Vec<u8>>,
        charts: &'a HashMap<String, Vec<u8>>,
        numbering: &'a Numbering,
    ) -> Self {
        Self {
            media,
            charts,
            rels,
            items: Vec::new(),
            in_paragraph: false,
            para_text: String::new(),
            para_has_plain_text: false,
            para_label: "paragraph".to_string(),
            para_pieces: Vec::new(),
            numbering,
            para_num_id: None,
            para_level: None,
            counters: HashMap::new(),
            para_pictures: Vec::new(),
            drawing: None,
            tables: Vec::new(),
            in_text: false,
            in_paragraph_properties: 0,
            skip_depth: None,
            math: None,
            page_width: 612.0,
            page_height: 792.0,
        }
    }

    fn run(&mut self, xml: &[u8]) {
        let mut reader = Reader::from_reader(Cursor::new(xml));
        // Keep text exactly as written: `w:t` content is authoritative,
        // including leading and trailing spaces.
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();

        loop {
            let event = match reader.read_event_into(&mut buf) {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(
                        "stopping DOCX XML parse at byte {}: {err}",
                        reader.buffer_position()
                    );
                    break;
                }
            };
            match event {
                // Inside a fallback subtree nothing is read: it is a duplicate
                // of the DrawingML the same shape already provided.
                Event::Start(e) => match self.skip_depth {
                    Some(depth) => self.skip_depth = Some(depth + 1),
                    None if is_alternate_fallback(&e) => self.skip_depth = Some(1),
                    None => self.start(&e),
                },
                Event::Empty(e) => {
                    if self.skip_depth.is_none() {
                        self.empty(&e);
                    }
                }
                Event::End(e) => match self.skip_depth {
                    Some(1) => self.skip_depth = None,
                    Some(depth) => self.skip_depth = Some(depth - 1),
                    None => self.end(e.name().as_ref()),
                },
                Event::Text(e) => {
                    if self.skip_depth.is_none() {
                        if let Ok(text) = e.unescape() {
                            self.text(&text);
                        }
                    }
                }
                Event::CData(e) => {
                    if self.skip_depth.is_none() {
                        if let Ok(text) = std::str::from_utf8(&e) {
                            let owned = text.to_string();
                            self.text(&owned);
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        self.finish_paragraph();
    }

    fn start(&mut self, e: &BytesStart) {
        let name = local_name(e.name().as_ref()).to_vec();

        // Inside math, everything is Office Math: grow the subtree instead.
        if let Some(math) = self.math.as_mut() {
            math.start(e);
            return;
        }

        match name.as_slice() {
            b"oMath" | b"oMathPara" => {
                self.math = Some(MathBuilder::new());
            }
            b"p" => {
                if !self.in_paragraph {
                    self.in_paragraph = true;
                    self.para_text.clear();
                    self.para_has_plain_text = false;
                    self.para_label = "paragraph".to_string();
                    self.para_pieces.clear();
                    self.para_pictures.clear();
                    self.para_num_id = None;
                    self.para_level = None;
                }
            }
            b"pPr" => self.in_paragraph_properties += 1,
            b"pStyle" => {
                if let Some(v) = attr_local(e, b"val") {
                    self.para_label = label_for_style(&v);
                }
            }
            b"numPr" => {
                if self.para_label == "paragraph" {
                    self.para_label = "list_item".to_string();
                }
            }
            b"ilvl" => {
                if let Some(v) = attr_local(e, b"val").and_then(|v| v.parse::<i32>().ok()) {
                    self.para_level = Some(v);
                }
            }
            b"numId" => {
                if let Some(v) = attr_local(e, b"val").and_then(|v| v.parse::<i32>().ok()) {
                    // Zero means "no numbering", as Word writes it to cancel one.
                    self.para_num_id = (v != 0).then_some(v);
                }
            }
            b"tab" => self.tab(),
            b"t" => {
                self.in_text = true;
            }
            b"tbl" => {
                self.tables.push(Vec::new());
            }
            b"tr" => {
                if let Some(t) = self.tables.last_mut() {
                    t.push(Vec::new());
                }
            }
            b"tc" => {
                if let Some(t) = self.tables.last_mut() {
                    if let Some(row) = t.last_mut() {
                        row.push(String::new());
                    }
                }
            }
            b"drawing" | b"pict" | b"object" => {
                // A `w:object` (an embedded OLE object — a MathType equation, a
                // pasted drawing) draws its preview through a `v:imagedata`, or
                // through a `v:imagedata` beside an `o:OLEObject` that we cannot
                // read. Its size is stated on the element itself, in twips,
                // where a DrawingML drawing states it on `wp:extent`.
                let mut pending = PendingPicture::default();
                if name.as_slice() == b"object" {
                    pending.width_pt = attr_local(e, b"dxaOrig")
                        .and_then(|v| v.parse::<f64>().ok())
                        .map(|v| v / TWIPS_PER_POINT);
                    pending.height_pt = attr_local(e, b"dyaOrig")
                        .and_then(|v| v.parse::<f64>().ok())
                        .map(|v| v / TWIPS_PER_POINT);
                }
                self.drawing = Some(pending);
            }
            b"extent" => {
                if let Some(d) = self.drawing.as_mut() {
                    let cx = attr_local(e, b"cx").and_then(|v| v.parse::<f64>().ok());
                    let cy = attr_local(e, b"cy").and_then(|v| v.parse::<f64>().ok());
                    d.width_pt = cx.map(|v| v / EMU_PER_POINT);
                    d.height_pt = cy.map(|v| v / EMU_PER_POINT);
                }
            }
            b"blip" => {
                if let Some(d) = self.drawing.as_mut() {
                    d.rel_id = attr(e, b"r:embed").or_else(|| attr_local(e, b"embed"));
                }
            }
            b"imagedata" => {
                if let Some(d) = self.drawing.as_mut() {
                    d.rel_id = attr(e, b"r:id").or_else(|| attr_local(e, b"id"));
                }
            }
            b"chart" => {
                if let Some(d) = self.drawing.as_mut() {
                    d.chart_rel_id = attr(e, b"r:id").or_else(|| attr_local(e, b"id"));
                }
            }
            b"pgSz" => {
                if let Some(w) = attr_local(e, b"w").and_then(|v| v.parse::<f64>().ok()) {
                    self.page_width = w / TWIPS_PER_POINT;
                }
                if let Some(h) = attr_local(e, b"h").and_then(|v| v.parse::<f64>().ok()) {
                    self.page_height = h / TWIPS_PER_POINT;
                }
            }
            _ => {}
        }
    }

    fn empty(&mut self, e: &BytesStart) {
        let name = local_name(e.name().as_ref()).to_vec();

        if let Some(math) = self.math.as_mut() {
            math.empty(e);
            return;
        }

        match name.as_slice() {
            b"pPr" => self.in_paragraph_properties += 1,
            b"pStyle" => {
                if let Some(v) = attr_local(e, b"val") {
                    self.para_label = label_for_style(&v);
                }
            }
            b"numPr" => {
                if self.para_label == "paragraph" {
                    self.para_label = "list_item".to_string();
                }
            }
            b"ilvl" => {
                if let Some(v) = attr_local(e, b"val").and_then(|v| v.parse::<i32>().ok()) {
                    self.para_level = Some(v);
                }
            }
            b"numId" => {
                if let Some(v) = attr_local(e, b"val").and_then(|v| v.parse::<i32>().ok()) {
                    self.para_num_id = (v != 0).then_some(v);
                }
            }
            b"tab" => self.tab(),
            b"chart" => {
                if let Some(d) = self.drawing.as_mut() {
                    d.chart_rel_id = attr(e, b"r:id").or_else(|| attr_local(e, b"id"));
                }
            }
            b"extent" => {
                if let Some(d) = self.drawing.as_mut() {
                    let cx = attr_local(e, b"cx").and_then(|v| v.parse::<f64>().ok());
                    let cy = attr_local(e, b"cy").and_then(|v| v.parse::<f64>().ok());
                    d.width_pt = cx.map(|v| v / EMU_PER_POINT);
                    d.height_pt = cy.map(|v| v / EMU_PER_POINT);
                }
            }
            b"blip" => {
                if let Some(d) = self.drawing.as_mut() {
                    d.rel_id = attr(e, b"r:embed").or_else(|| attr_local(e, b"embed"));
                }
            }
            b"imagedata" => {
                if let Some(d) = self.drawing.as_mut() {
                    d.rel_id = attr(e, b"r:id").or_else(|| attr_local(e, b"id"));
                }
            }
            b"pgSz" => {
                if let Some(w) = attr_local(e, b"w").and_then(|v| v.parse::<f64>().ok()) {
                    self.page_width = w / TWIPS_PER_POINT;
                }
                if let Some(h) = attr_local(e, b"h").and_then(|v| v.parse::<f64>().ok()) {
                    self.page_height = h / TWIPS_PER_POINT;
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, qualified: &[u8]) {
        let name = local_name(qualified).to_vec();

        if let Some(math) = self.math.as_mut() {
            // The closing `oMath`/`oMathPara` ends the subtree.
            if name == b"oMath" || name == b"oMathPara" {
                let builder = self.math.take().expect("in math");
                let rendered = normalize_math(&builder.finish());
                if !rendered.is_empty() {
                    if !self.para_text.is_empty() && !self.para_text.ends_with(' ') {
                        self.para_text.push(' ');
                    }
                    self.para_text.push_str(&rendered);
                    self.para_pieces.push(ParagraphPiece::Math(rendered));
                }
                return;
            }
            math.close();
            return;
        }

        match name.as_slice() {
            b"t" => self.in_text = false,
            b"pPr" => self.in_paragraph_properties = self.in_paragraph_properties.saturating_sub(1),
            b"p" => self.finish_paragraph(),
            b"drawing" | b"pict" | b"object" => {
                if let Some(d) = self.drawing.take() {
                    self.para_pictures.push(d);
                }
            }
            b"tc" => {
                // Trim the accumulated cell text once the cell closes.
                if let Some(t) = self.tables.last_mut() {
                    if let Some(row) = t.last_mut() {
                        if let Some(cell) = row.last_mut() {
                            *cell = cell.trim().to_string();
                        }
                    }
                }
            }
            b"tbl" => {
                if let Some(table) = self.tables.pop() {
                    if !table.is_empty() {
                        self.items.push(DocxItem::Table { rows: table });
                    }
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, t: &str) {
        if let Some(math) = self.math.as_mut() {
            math.text(t);
            return;
        }
        if !self.in_text {
            return;
        }
        self.push_text(t);
    }

    /// Add literal text to the paragraph or cell being built.
    fn push_text(&mut self, t: &str) {
        if self.in_paragraph {
            self.para_text.push_str(t);
            if !t.trim().is_empty() {
                self.para_has_plain_text = true;
            }
            match self.para_pieces.last_mut() {
                Some(ParagraphPiece::Text(text)) => text.push_str(t),
                _ => self.para_pieces.push(ParagraphPiece::Text(t.to_string())),
            }
        } else if let Some(table) = self.tables.last_mut() {
            // Text between paragraphs inside a cell (rare) still counts.
            if let Some(row) = table.last_mut() {
                if let Some(cell) = row.last_mut() {
                    cell.push_str(t);
                }
            }
        }
    }

    /// A `w:tab` in a run is a real tab on the page — a table of contents
    /// separates its title from its page number with one. Dropping it glued the
    /// two together into `calibre1` (bug 46). Tab *stops*, which also use
    /// `w:tab`, are declared inside `w:pPr` and are not text.
    fn tab(&mut self) {
        if self.in_paragraph_properties == 0 {
            self.push_text("\t");
        }
    }

    /// The marker and depth for the paragraph that just closed, advancing the
    /// list counters as a reader would.
    fn numbering_for_paragraph(&mut self) -> (Option<String>, Option<i32>) {
        let Some(num_id) = self.para_num_id else {
            return (None, None);
        };
        let index = self.para_level.unwrap_or(0).max(0);
        let level = Some(index + 1);
        let Some(def) = self.numbering.get(&num_id) else {
            return (None, level);
        };
        let Some(level_def) = def.level(index) else {
            return (None, level);
        };
        // `numFmt="none"` prints nothing at all.
        if level_def.format == "none" {
            return (None, level);
        }

        // A shallower item restarts everything below it.
        self.counters
            .retain(|(id, depth), _| *id != num_id || *depth <= index);
        let start = def.start(index);
        let counter = self.counters.entry((num_id, index)).or_insert(start - 1);
        *counter += 1;
        let value = *counter;

        // Bullets print their literal glyph; numbers substitute the counters of
        // whatever levels their `lvlText` refers to.
        let marker = if level_def.format == "bullet" {
            visible_bullet(&level_def.text)
        } else {
            let mut out = String::new();
            let mut chars = level_def.text.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '%' {
                    match chars.peek().and_then(|d| d.to_digit(10)) {
                        Some(digit) if (1..=9).contains(&digit) => {
                            chars.next();
                            let target = digit as i32 - 1;
                            let format = def
                                .level(target)
                                .map(|l| l.format.as_str())
                                .unwrap_or("decimal");
                            let value = if target == index {
                                value
                            } else {
                                self.counters
                                    .get(&(num_id, target))
                                    .copied()
                                    .unwrap_or_else(|| def.start(target))
                            };
                            out.push_str(&format_counter(value, format));
                        }
                        // A bare `%` is printed as written.
                        _ => out.push('%'),
                    }
                } else {
                    out.push(c);
                }
            }
            if out.trim().is_empty() {
                // No `lvlText`: still better to report the number than nothing.
                format_counter(value, &level_def.format)
            } else {
                out
            }
        };
        (Some(marker), level)
    }

    fn finish_paragraph(&mut self) {
        if !self.in_paragraph {
            return;
        }
        self.in_paragraph = false;

        let text = self.para_text.trim().to_string();
        let pictures = std::mem::take(&mut self.para_pictures);

        // A paragraph inside a table cell contributes to the cell, not the body.
        if let Some(table) = self.tables.last_mut() {
            if !text.is_empty() {
                if let Some(row) = table.last_mut() {
                    if let Some(cell) = row.last_mut() {
                        if !cell.is_empty() {
                            cell.push(' ');
                        }
                        cell.push_str(&text);
                    }
                }
            }
            self.para_text.clear();
            self.para_has_plain_text = false;
            self.para_pieces.clear();
            return;
        }

        if !text.is_empty() {
            let pieces = std::mem::take(&mut self.para_pieces);
            if !self.para_has_plain_text {
                // Nothing but math: every equation in the paragraph is an
                // equation of its own.
                for piece in pieces {
                    if let ParagraphPiece::Math(equation) = piece {
                        self.items.push(DocxItem::Formula { text: equation });
                    }
                }
            } else {
                // An equation that opens or closes a paragraph stands on its own;
                // one written inside a sentence stays in the sentence, where a
                // reader reads it (bug 47). Docling splits both out and leaves
                // empty text items behind it, which is a faithful record of the
                // XML but not of the page.
                let (first, last) = prose_pieces(&pieces);
                let leading: Vec<String> = pieces[..first]
                    .iter()
                    .filter_map(|piece| match piece {
                        ParagraphPiece::Math(equation) => Some(equation.clone()),
                        ParagraphPiece::Text(_) => None,
                    })
                    .collect();
                let trailing: Vec<String> = pieces[last + 1..]
                    .iter()
                    .filter_map(|piece| match piece {
                        ParagraphPiece::Math(equation) => Some(equation.clone()),
                        ParagraphPiece::Text(_) => None,
                    })
                    .collect();
                let prose = join_pieces(&pieces[first..=last]);

                for equation in leading {
                    self.items.push(DocxItem::Formula { text: equation });
                }
                if !prose.trim().is_empty() {
                    let (marker, level) = self.numbering_for_paragraph();
                    self.items.push(DocxItem::Text {
                        text: prose.trim().to_string(),
                        label: self.para_label.clone(),
                        marker,
                        level,
                    });
                }
                for equation in trailing {
                    self.items.push(DocxItem::Formula { text: equation });
                }
            }
        }

        for pending in pictures {
            if let Some(item) = self.materialise(pending) {
                self.items.push(item);
            }
        }

        self.para_text.clear();
        self.para_has_plain_text = false;
        self.para_pieces.clear();
        self.para_label = "paragraph".to_string();
        self.para_num_id = None;
        self.para_level = None;
    }

    /// Load a drawing's image from the package.
    fn materialise(&mut self, pending: PendingPicture) -> Option<DocxItem> {
        // A chart is not an image part: it paints a chart part, and the title
        // written in that part is the only place the words exist. Docling
        // reports it as a caption at the chart's place in the flow.
        if let Some(rel_id) = pending.chart_rel_id.as_ref() {
            if let Some(title) = self
                .rels
                .get(rel_id)
                .and_then(|target| self.charts.get(target))
                .and_then(|xml| chart_title(xml))
            {
                return Some(DocxItem::Text {
                    text: title,
                    label: "caption".to_string(),
                    marker: None,
                    level: None,
                });
            }
        }

        let rel_id = pending.rel_id?;
        let target = self.rels.get(&rel_id)?.clone();
        let mime = match target.rsplit('.').next()?.to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            "tif" | "tiff" => "image/tiff",
            "emf" => "image/emf",
            "wmf" => "image/wmf",
            "svg" => "image/svg+xml",
            other => {
                tracing::debug!("skipping unsupported DOCX image type .{other}");
                return None;
            }
        };
        let data = self.media.get(&target)?.clone();
        // A drawing is played into a PNG here, so a picture Word stored as
        // vector calls is a picture of pixels, with a media type Docling can
        // name. One that will not play keeps its own payload and stays a box
        // with no image.
        if drawing::is_vector_drawing(mime) {
            if let Some(png) = drawing::rasterize_to_png(&data, pending.width_pt, pending.height_pt)
            {
                return Some(DocxItem::Picture {
                    mime: "image/png",
                    data: png,
                    width_pt: pending.width_pt,
                    height_pt: pending.height_pt,
                });
            }
        }
        Some(DocxItem::Picture {
            mime,
            data,
            width_pt: pending.width_pt,
            height_pt: pending.height_pt,
        })
    }
}

/// Map a paragraph style to a Docling-ish label.
/// The title a chart part carries, or `None` when it has none.
///
/// A chart title is a rich-text block (`c:title`) whose runs are `a:t`
/// elements, split wherever Word felt like splitting them — "my chart" and
/// " looks nice" are one title — so the runs are joined in document order.
///
/// Only the chart's own title is taken. Axis titles are `c:title` elements too,
/// nested inside the plot area, and they name an axis rather than the chart;
/// the first title to open is the chart's, so capture stops when it closes.
fn chart_title(xml: &[u8]) -> Option<String> {
    let mut reader = Reader::from_reader(Cursor::new(xml));
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut in_title = 0usize;
    let mut capturing = false;
    let mut in_run_text = 0usize;
    let mut title = String::new();

    loop {
        let event = reader.read_event_into(&mut buf).ok()?;
        match event {
            Event::Start(e) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"title" => {
                        if in_title == 0 {
                            capturing = true;
                        }
                        in_title += 1;
                    }
                    b"t" if capturing => in_run_text += 1,
                    _ => {}
                }
            }
            Event::End(e) => match local_name(e.name().as_ref()) {
                b"title" => {
                    in_title = in_title.saturating_sub(1);
                    if in_title == 0 {
                        break;
                    }
                }
                b"t" => in_run_text = in_run_text.saturating_sub(1),
                _ => {}
            },
            Event::Text(e) => {
                if capturing && in_run_text > 0 {
                    if let Ok(text) = e.unescape() {
                        title.push_str(&text);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    let title = title.trim().to_string();
    (!title.is_empty()).then_some(title)
}

fn label_for_style(style: &str) -> String {
    let s = style.to_ascii_lowercase().replace(' ', "");
    if s.starts_with("heading") || s == "title" || s == "subtitle" {
        "section_header".to_string()
    } else if s.contains("list") || s.starts_with("listparagraph") {
        "list_item".to_string()
    } else if s.contains("caption") {
        "caption".to_string()
    } else if s.contains("quote") {
        "text".to_string()
    } else {
        "paragraph".to_string()
    }
}

/// The first and last *prose* piece of a paragraph.
///
/// The caller only asks when the paragraph has prose at all, so there is always
/// a text piece to find.
fn prose_pieces(pieces: &[ParagraphPiece]) -> (usize, usize) {
    let first = pieces
        .iter()
        .position(|piece| matches!(piece, ParagraphPiece::Text(_)))
        .expect("a paragraph with prose has a text piece");
    let last = pieces
        .iter()
        .rposition(|piece| matches!(piece, ParagraphPiece::Text(_)))
        .expect("a paragraph with prose has a text piece");
    (first, last)
}

/// A paragraph's pieces as one string, with an equation held in the sentence at
/// the spacing the paragraph was written with.
fn join_pieces(pieces: &[ParagraphPiece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            ParagraphPiece::Text(text) => out.push_str(text),
            ParagraphPiece::Math(equation) => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(equation);
            }
        }
    }
    out
}

/// Collapse whitespace the XML introduces, keeping math readable.
fn normalize_math(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            // Keep a single space, but never break the tight `_` / `^` markers.
            if !last_space && !out.is_empty() {
                out.push(' ');
                last_space = true;
            }
        } else {
            if (ch == '_' || ch == '^') && out.ends_with(' ') {
                out.pop();
            }
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
}

fn math_child<'a>(n: &'a MathNode, name: &str) -> Option<&'a MathNode> {
    n.children.iter().find(|c| c.name == name)
}

fn render_math_children(n: &MathNode) -> String {
    n.children.iter().map(render_math_node).collect()
}

/// Whether an operand needs brackets when it becomes a fraction or a script.
fn compound(s: &str) -> bool {
    s.chars().count() > 2 || s.chars().any(|c| " +-*/^_=<>".contains(c))
}

/// Wrap only when the operand is not atomic, so `x^2` stays readable.
fn script(s: String) -> String {
    if s.chars().count() > 1 {
        format!("({s})")
    } else {
        s
    }
}

fn fraction_part(s: String) -> String {
    if compound(&s) {
        format!("({s})")
    } else {
        s
    }
}

/// Preferred character from a `*Pr` property node, e.g. `<m:chr m:val="∑"/>`.
fn property_char(n: &MathNode) -> Option<String> {
    n.children
        .iter()
        .find(|c| c.name.ends_with("Pr"))
        .and_then(|pr| math_child(pr, "chr"))
        .and_then(|chr| {
            chr.attr("val")
                .map(str::to_string)
                .or_else(|| (!chr.text.is_empty()).then(|| chr.text.clone()))
        })
}

/// Linearise one Office Math node into readable text.
///
/// This is deliberately not LaTeX: it keeps the structure visible
/// (`x^2`, `a/b`, `√(x)`) without pretending to be round-trippable.
fn render_math_node(n: &MathNode) -> String {
    // Property and control elements never surface on their own.
    if n.name.ends_with("Pr") || n.name == "chr" || n.name == "degHide" {
        return String::new();
    }
    match n.name.as_str() {
        "t" => n.text.clone(),
        "oMath" | "oMathPara" | "r" | "e" | "num" | "den" | "fName" | "lim" | "sub" | "sup"
        | "deg" | "box" | "borderBox" => render_math_children(n),
        "f" => {
            let num = math_child(n, "num")
                .map(render_math_children)
                .unwrap_or_default();
            let den = math_child(n, "den")
                .map(render_math_children)
                .unwrap_or_default();
            format!(
                "{}/{}",
                fraction_part(num.trim().to_string()),
                fraction_part(den.trim().to_string())
            )
        }
        "sSup" => {
            let base = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            let sup = math_child(n, "sup")
                .map(render_math_children)
                .unwrap_or_default();
            format!("{}^{}", base.trim(), script(sup.trim().to_string()))
        }
        "sSub" => {
            let base = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            let sub = math_child(n, "sub")
                .map(render_math_children)
                .unwrap_or_default();
            format!("{}_{}", base.trim(), script(sub.trim().to_string()))
        }
        "sSubSup" => {
            let base = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            let sub = math_child(n, "sub")
                .map(render_math_children)
                .unwrap_or_default();
            let sup = math_child(n, "sup")
                .map(render_math_children)
                .unwrap_or_default();
            format!(
                "{}_{}^{}",
                base.trim(),
                script(sub.trim().to_string()),
                script(sup.trim().to_string())
            )
        }
        "sPre" => {
            let base = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            let sub = math_child(n, "sub")
                .map(render_math_children)
                .unwrap_or_default();
            let sup = math_child(n, "sup")
                .map(render_math_children)
                .unwrap_or_default();
            format!(
                "{}_{}^{}",
                base.trim(),
                script(sub.trim().to_string()),
                script(sup.trim().to_string())
            )
        }
        "d" => {
            let inner = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            format!("({})", inner.trim())
        }
        "rad" => {
            let body = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            let deg = math_child(n, "deg")
                .map(render_math_children)
                .unwrap_or_default();
            let full = deg.trim().is_empty()
                || math_child(n, "radPr")
                    .map(|p| math_child(p, "degHide").is_some())
                    .unwrap_or(false);
            if full {
                format!("√({})", body.trim())
            } else {
                format!("√[{}]({})", deg.trim(), body.trim())
            }
        }
        "nary" => {
            let op = property_char(n).unwrap_or_else(|| "∫".to_string());
            let body = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            let sub = math_child(n, "sub")
                .map(render_math_children)
                .unwrap_or_default();
            let sup = math_child(n, "sup")
                .map(render_math_children)
                .unwrap_or_default();
            let mut limits = String::new();
            if !sub.trim().is_empty() {
                limits.push('_');
                limits.push_str(&script(sub.trim().to_string()));
            }
            if !sup.trim().is_empty() {
                limits.push('^');
                limits.push_str(&script(sup.trim().to_string()));
            }
            format!("{op}{limits} {}", body.trim())
        }
        "limLow" => {
            let base = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            let lim = math_child(n, "lim")
                .map(render_math_children)
                .unwrap_or_default();
            format!("{}_{}", base.trim(), script(lim.trim().to_string()))
        }
        "limUpp" => {
            let base = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            let lim = math_child(n, "lim")
                .map(render_math_children)
                .unwrap_or_default();
            format!("{}^{}", base.trim(), script(lim.trim().to_string()))
        }
        "acc" => {
            let base = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            let mark = property_char(n).unwrap_or_else(|| "^".to_string());
            format!("{}{}", base.trim(), mark)
        }
        "bar" => {
            let base = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            format!("{}(bar)", base.trim())
        }
        "groupChr" => math_child(n, "e")
            .map(render_math_children)
            .unwrap_or_default(),
        "func" => {
            let name = math_child(n, "fName")
                .map(render_math_children)
                .unwrap_or_default();
            let body = math_child(n, "e")
                .map(render_math_children)
                .unwrap_or_default();
            format!("{} {}", name.trim(), body.trim())
        }
        "phant" => math_child(n, "e")
            .map(render_math_children)
            .unwrap_or_default(),
        "m" => {
            // Matrix: rows of cells.
            let rows: Vec<String> = n
                .children
                .iter()
                .filter(|c| c.name == "mr")
                .map(|row| {
                    row.children
                        .iter()
                        .filter(|c| c.name == "e")
                        .map(|cell| render_math_children(cell).trim().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .collect();
            format!("[{}]", rows.join("; "))
        }
        "eqArr" => {
            let parts: Vec<String> = n
                .children
                .iter()
                .filter(|c| c.name == "e")
                .map(|e| render_math_children(e).trim().to_string())
                .collect();
            parts.join(" ; ")
        }
        _ => render_math_children(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal DOCX package in memory.
    fn package(document: &str, rels: &str, media: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("word/document.xml", opts).expect("start");
            zip.write_all(document.as_bytes()).expect("write");
            zip.start_file("word/_rels/document.xml.rels", opts)
                .expect("start");
            zip.write_all(rels.as_bytes()).expect("write");
            for (name, data) in media {
                zip.start_file(*name, opts).expect("start");
                zip.write_all(data).expect("write");
            }
            zip.finish().expect("finish");
        }
        out
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
    const M: &str = r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math""#;

    #[test]
    fn extracts_paragraphs_and_heading_labels() {
        let doc = format!(
            r#"<w:document {W} xmlns:r="x"><w:body>
                 <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>
                 <w:p><w:r><w:t>Body </w:t></w:r><w:r><w:t>text</w:t></w:r></w:p>
                 <w:p><w:pPr><w:numPr/></w:pPr><w:r><w:t>item</w:t></w:r></w:p>
               </w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        assert_eq!(parsed.items.len(), 3);
        match &parsed.items[0] {
            DocxItem::Text { text, label, .. } => {
                assert_eq!(text, "Title");
                assert_eq!(label, "section_header");
            }
            other => panic!("expected text, got {other:?}"),
        }
        match &parsed.items[1] {
            DocxItem::Text { text, .. } => assert_eq!(text, "Body text"),
            other => panic!("expected text, got {other:?}"),
        }
        match &parsed.items[2] {
            DocxItem::Text { label, .. } => assert_eq!(label, "list_item"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    /// A package that also carries `word/numbering.xml`.
    fn package_with_numbering(document: &str, numbering: &str) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("word/document.xml", opts).expect("start");
            zip.write_all(document.as_bytes()).expect("write");
            zip.start_file("word/numbering.xml", opts).expect("start");
            zip.write_all(numbering.as_bytes()).expect("write");
            zip.finish().expect("finish");
        }
        out
    }

    /// A decimal list with two nested levels, a roman list starting at four, and
    /// a bullet list whose glyph is a symbol-font character reference.
    const NUMBERING: &str = r#"<w:numbering xmlns:w="x">
      <w:abstractNum w:abstractNumId="0">
        <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
        <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1.%2"/></w:lvl>
        <w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%3)"/></w:lvl>
      </w:abstractNum>
      <w:abstractNum w:abstractNumId="1">
        <w:lvl w:ilvl="0"><w:start w:val="4"/><w:numFmt w:val="upperRoman"/><w:lvlText w:val="%1)"/></w:lvl>
      </w:abstractNum>
      <w:abstractNum w:abstractNumId="2">
        <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="&#xF0B7;"/></w:lvl>
      </w:abstractNum>
      <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
      <w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
      <w:num w:numId="3"><w:abstractNumId w:val="2"/></w:num>
    </w:numbering>"#;

    fn numbered(ilvl: i32, num_id: i32, text: &str) -> String {
        format!(
            "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"{ilvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr>\
             <w:r><w:t>{text}</w:t></w:r></w:p>"
        )
    }

    fn parts_of(item: &DocxItem) -> (String, Option<String>, Option<i32>) {
        match item {
            DocxItem::Text {
                text,
                marker,
                level,
                ..
            } => (text.clone(), marker.clone(), *level),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn resolves_list_markers_and_levels_from_numbering() {
        let body = format!(
            "<w:body>{}{}{}{}{}{}</w:body>",
            numbered(0, 1, "First"),
            numbered(1, 1, "Nested"),
            numbered(2, 1, "Sub"),
            numbered(0, 1, "Second"),
            numbered(0, 2, "Roman"),
            numbered(0, 3, "Bulleted"),
        );
        let doc = format!("<w:document {W}>{body}</w:document>");
        let parsed =
            DocxDocument::parse_bytes(&package_with_numbering(&doc, NUMBERING)).expect("parse");
        let got: Vec<(String, Option<String>, Option<i32>)> =
            parsed.items.iter().map(parts_of).collect();
        assert_eq!(got.len(), 6, "{got:?}");
        assert_eq!(
            got[0],
            ("First".into(), Some("1.".into()), Some(1)),
            "{got:?}"
        );
        assert_eq!(
            got[1],
            ("Nested".into(), Some("1.1".into()), Some(2)),
            "{got:?}"
        );
        assert_eq!(
            got[2],
            ("Sub".into(), Some("a)".into()), Some(3)),
            "{got:?}"
        );
        // The shallow item restarts everything nested inside it.
        assert_eq!(
            got[3],
            ("Second".into(), Some("2.".into()), Some(1)),
            "{got:?}"
        );
        // A list that starts at four prints its own first value.
        assert_eq!(
            got[4],
            ("Roman".into(), Some("IV)".into()), Some(1)),
            "{got:?}"
        );
        // Symbol-font bullets are not text, so they map to a real glyph.
        let bullet = got[5].1.clone().expect("a bullet marker");
        assert_eq!(
            bullet.chars().next().map(|c| c as u32),
            Some(0x2022),
            "{bullet:?}"
        );
        assert_eq!(got[5].2, Some(1));
    }

    /// `w:lvlOverride` is how one list changes its mind: it can relabel a level
    /// (decimal to letters) or restart it at a chosen value, without touching the
    /// abstract scheme every other list shares.
    #[test]
    fn applies_level_overrides_from_numbering() {
        let numbering = r#"<w:numbering xmlns:w="x">
          <w:abstractNum w:abstractNumId="0">
            <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
          </w:abstractNum>
          <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
          <w:num w:numId="4"><w:abstractNumId w:val="0"/>
            <w:lvlOverride w:ilvl="0">
              <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="upperLetter"/><w:lvlText w:val="%1)"/></w:lvl>
            </w:lvlOverride>
          </w:num>
          <w:num w:numId="5"><w:abstractNumId w:val="0">
            </w:abstractNumId><w:lvlOverride w:ilvl="0" w:startOverride="5"/></w:num>
        </w:numbering>"#;
        let body = format!(
            "<w:body>{}{}{}{}{}{}</w:body>",
            numbered(0, 1, "Plain"),
            numbered(0, 1, "Plain again"),
            numbered(0, 4, "Letters"),
            numbered(0, 4, "Letters again"),
            numbered(0, 5, "Restarted"),
            numbered(0, 5, "Restarted again"),
        );
        let doc = format!("<w:document {W}>{body}</w:document>");
        let parsed =
            DocxDocument::parse_bytes(&package_with_numbering(&doc, numbering)).expect("parse");
        let got: Vec<Option<String>> = parsed.items.iter().map(|i| parts_of(i).1).collect();
        assert_eq!(
            got,
            vec![
                Some("1.".into()),
                Some("2.".into()),
                // The override relabels this list only.
                Some("A)".into()),
                Some("B)".into()),
                // `w:startOverride` restarts the counter at five.
                Some("5.".into()),
                Some("6.".into()),
            ],
            "{got:?}"
        );
    }

    #[test]
    fn a_list_item_without_definitions_is_still_a_list_item() {
        // `numId` that no `w:num` defines: the label stands, the marker is absent
        // rather than invented.
        let doc = format!(
            "<w:document {W}><w:body>{}</w:body></w:document>",
            numbered(0, 9, "Orphan")
        );
        let parsed = DocxDocument::parse_bytes(&package(&doc, "", &[])).expect("parse");
        let (text, marker, level) = parts_of(&parsed.items[0]);
        assert_eq!(text, "Orphan");
        assert_eq!(marker, None);
        assert_eq!(level, Some(1), "the depth is still known");
    }

    #[test]
    fn extracts_a_table_grid() {
        let doc = format!(
            r#"<w:document {W}><w:body><w:tbl>
                 <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc>
                       <w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr>
                 <w:tr><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc>
                       <w:tc><w:p><w:r><w:t>d</w:t></w:r></w:p></w:tc></w:tr>
               </w:tbl></w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        match &parsed.items[0] {
            DocxItem::Table { rows } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0], vec!["a".to_string(), "b".to_string()]);
                assert_eq!(rows[1], vec!["c".to_string(), "d".to_string()]);
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn linearises_equations_and_keeps_them_out_of_the_text() {
        let doc = format!(
            r#"<w:document {W} {M}><w:body>
                 <w:p><m:oMath>
                   <m:f><m:num><m:r><m:t>a</m:t></m:r></m:num>
                        <m:den><m:r><m:t>b</m:t></m:r></m:den></m:f>
                 </m:oMath></w:p>
               </w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        match &parsed.items[0] {
            DocxItem::Formula { text } => assert_eq!(text, "a/b", "got {text:?}"),
            other => panic!("expected formula, got {other:?}"),
        }
    }

    /// An equation that opens or closes a paragraph is its own item, the way
    /// Docling reports it; an equation written inside a sentence stays in the
    /// sentence, where it is read (bug 47).
    #[test]
    fn an_equation_at_the_edge_of_a_paragraph_stands_alone() {
        let doc = format!(
            r#"<w:document {W} {M}><w:body>
                 <w:p><w:r><w:t>the line ends here </w:t></w:r>
                   <m:oMath><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e>
                     <m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath>
                 </w:p>
                 <w:p><w:r><w:t>where </w:t></w:r>
                   <m:oMath><m:sSup><m:e><m:r><m:t>y</m:t></m:r></m:e>
                     <m:sup><m:r><m:t>3</m:t></m:r></m:sup></m:sSup></m:oMath>
                   <w:r><w:t> is the ratio we use</w:t></w:r>
                 </w:p>
               </w:body></w:document>"#
        );
        let parsed = DocxDocument::parse_bytes(&package(&doc, "", &[])).expect("parse");
        let shape: Vec<String> = parsed
            .items
            .iter()
            .map(|item| match item {
                DocxItem::Formula { text } => format!("formula:{text}"),
                DocxItem::Text { text, .. } => format!("text:{text}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                "text:the line ends here",
                "formula:x^2",
                "text:where y^3 is the ratio we use",
            ]
        );
    }

    /// Three display equations in one paragraph are three equations: Docling
    /// reports one item per `m:oMath`, and the golden comparison of
    /// `math-nary.docx` found we were welding them into one string (bug 46).
    #[test]
    fn each_display_equation_in_a_paragraph_is_its_own_formula() {
        let doc = format!(
            r#"<w:document {W} {M}><w:body>
                 <w:p><m:oMath><m:r><m:t>a</m:t></m:r></m:oMath>
                      <m:oMath><m:r><m:t>b</m:t></m:r></m:oMath>
                      <m:oMath><m:r><m:t>c</m:t></m:r></m:oMath></w:p>
               </w:body></w:document>"#
        );
        let parsed = DocxDocument::parse_bytes(&package(&doc, "", &[])).expect("parse");
        let formulas: Vec<String> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Formula { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(formulas, vec!["a", "b", "c"]);
    }

    #[test]
    fn resolves_picture_relationships_and_reports_placement() {
        let doc = format!(
            r#"<w:document {W} xmlns:wp="x" xmlns:a="y"><w:body><w:p>
                 <w:r><w:drawing><wp:extent cx="914400" cy="457200"/>
                   <a:blip r:embed="rId7"/></w:drawing></w:r>
                 <w:r><w:t>caption</w:t></w:r>
               </w:p></w:body></w:document>"#
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
            <Relationship Id="rId8" Type="image" Target="https://x/i.png" TargetMode="External"/>
          </Relationships>"#;
        // 1x1 PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde,
        ];
        let pkg = package(&doc, rels, &[("word/media/image1.png", png)]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let picture = parsed
            .items
            .iter()
            .find_map(|i| match i {
                DocxItem::Picture {
                    mime,
                    data,
                    width_pt,
                    height_pt,
                } => Some((*mime, data.len(), *width_pt, *height_pt)),
                _ => None,
            })
            .expect("a picture");
        assert_eq!(picture.0, "image/png");
        assert_eq!(picture.1, png.len());
        // 914400 EMU / 12700 = 72pt; 457200 / 12700 = 36pt.
        assert_eq!(picture.2, Some(72.0));
        assert_eq!(picture.3, Some(36.0));
        // The caption text survives alongside the drawing.
        assert!(parsed.items.iter().any(|i| matches!(
            i,
            DocxItem::Text { text, .. } if text == "caption"
        )));
    }

    #[test]
    fn reports_the_section_page_size() {
        let doc = format!(
            r#"<w:document {W}><w:body><w:p><w:r><w:t>x</w:t></w:r></w:p>
               <w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr>
               </w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        // A4 in points.
        assert!(
            (parsed.page_width - 595.3).abs() < 0.1,
            "width {}",
            parsed.page_width
        );
        assert!(
            (parsed.page_height - 841.9).abs() < 0.1,
            "height {}",
            parsed.page_height
        );
    }

    const MC: &str = r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;
    const V: &str = r#"xmlns:v="urn:schemas-microsoft-com:vml""#;

    /// Word writes every shape twice: the real DrawingML in `mc:Choice` and a
    /// VML copy for old readers in `mc:Fallback`. Reading both gave every shape
    /// two entries in the document (bug 46).
    #[test]
    fn a_shape_written_twice_is_extracted_once() {
        let doc = format!(
            r#"<w:document {W} {MC} {V}><w:body><w:p>
                 <w:r><mc:AlternateContent>
                   <mc:Choice Requires="wps"><w:drawing><wps:txbx><w:txbxContent>
                     <w:p><w:r><w:t>Shape text</w:t></w:r></w:p></w:txbxContent></wps:txbx></w:drawing></mc:Choice>
                   <mc:Fallback><w:pict><v:shape><v:textbox><w:txbxContent>
                     <w:p><w:r><w:t>Shape text</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback>
                 </mc:AlternateContent></w:r>
               </w:p>
               <w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent>
                 <w:p><w:r><w:t>Standalone callout</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>
               </w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let texts: Vec<String> = parsed
            .items
            .iter()
            .filter_map(|i| match i {
                DocxItem::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Shape text", "Standalone callout"]);
    }

    /// A table of contents separates its title from its page number with a tab,
    /// and dropping the tab glued the two together (bug 46).
    #[test]
    fn a_tab_in_the_text_survives_but_a_tab_stop_does_not() {
        let doc = format!(
            r#"<w:document {W}><w:body>
                 <w:p><w:r><w:t>Chapter</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>3</w:t></w:r></w:p>
                 <w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="9000"/></w:tabs></w:pPr>
                   <w:r><w:t>Plain</w:t></w:r></w:p>
               </w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let texts: Vec<String> = parsed
            .items
            .iter()
            .filter_map(|i| match i {
                DocxItem::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Chapter\t3", "Plain"]);
    }

    /// A chart is a part, not an image: its title exists nowhere else, and
    /// Docling reports it as a caption (bug 46).
    #[test]
    fn a_chart_title_is_kept_as_a_caption() {
        let doc = format!(
            r#"<w:document {W} xmlns:r="x"><w:body><w:p><w:r><w:drawing>
                 <c:chart xmlns:c="c" r:id="rId9"/>
               </w:drawing></w:r></w:p></w:body></w:document>"#
        );
        let rels = r#"<Relationships><Relationship Id="rId9" Type="chart"
            Target="charts/chart1.xml"/></Relationships>"#;
        // The title is split across runs, exactly as Word writes it.
        let chart = r#"<c:chartSpace xmlns:c="c" xmlns:a="a"><c:chart>
            <c:title><c:tx><c:rich><a:p>
              <a:r><a:t>my chart</a:t></a:r><a:r><a:t> looks nice</a:t></a:r>
            </a:p></c:rich></c:tx></c:title>
            <c:plotArea><c:title><c:tx><c:rich><a:p><a:r><a:t>Axis</a:t></a:r></a:p></c:rich></c:tx></c:title></c:plotArea>
          </c:chart></c:chartSpace>"#;
        let pkg = package(&doc, rels, &[("word/charts/chart1.xml", chart.as_bytes())]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let captions: Vec<(String, String)> = parsed
            .items
            .iter()
            .filter_map(|i| match i {
                DocxItem::Text { text, label, .. } if label == "caption" => {
                    Some((text.clone(), label.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(captions.len(), 1, "{captions:?}");
        assert_eq!(captions[0].0, "my chart looks nice");
    }

    /// Footnote and endnote text is body content a reader sees; Docling reports
    /// both as `footnote` items at the end of the document (bug 46).
    #[test]
    fn footnotes_and_endnotes_become_footnote_items() {
        let doc = format!(
            r#"<w:document {W}><w:body><w:p><w:r><w:t>Body</w:t></w:r></w:p></w:body></w:document>"#
        );
        // The separator footnote Word writes first carries no text at all.
        let notes = format!(
            r#"<w:footnotes {W}><w:footnote w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>
                 <w:footnote w:id="1"><w:p><w:r><w:t>In paged media</w:t></w:r></w:p></w:footnote></w:footnotes>"#
        );
        let endnotes = format!(
            r#"<w:endnotes {W}><w:endnote w:id="2"><w:p><w:r><w:t>Endnote body</w:t></w:r></w:p></w:endnote></w:endnotes>"#
        );
        let pkg = package(
            &doc,
            "",
            &[
                ("word/footnotes.xml", notes.as_bytes()),
                ("word/endnotes.xml", endnotes.as_bytes()),
            ],
        );
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let labelled: Vec<(String, String)> = parsed
            .items
            .iter()
            .filter_map(|i| match i {
                DocxItem::Text { text, label, .. } => Some((label.clone(), text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            labelled,
            vec![
                ("paragraph".to_string(), "Body".to_string()),
                ("footnote".to_string(), "In paged media".to_string()),
                ("footnote".to_string(), "Endnote body".to_string()),
            ]
        );
    }

    #[test]
    fn external_relationship_targets_are_not_read_from_the_package() {
        let doc = format!(
            r#"<w:document {W} xmlns:wp="x" xmlns:a="y"><w:body><w:p>
                 <w:r><w:drawing><a:blip r:embed="rId8"/></w:drawing></w:r>
               </w:p></w:body></w:document>"#
        );
        let rels = r#"<Relationships><Relationship Id="rId8" Type="image"
            Target="https://example.com/i.png" TargetMode="External"/></Relationships>"#;
        let pkg = package(&doc, rels, &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        assert!(!parsed
            .items
            .iter()
            .any(|i| matches!(i, DocxItem::Picture { .. })));
    }
}
