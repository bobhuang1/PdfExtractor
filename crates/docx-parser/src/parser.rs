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
use crate::vml;
// A shape Word paints itself is drawn by the painter the deck reader uses too:
// the shape markup is the same DrawingML under either prefix.
use pdfextractor_drawingml::{
    paints_a_picture_fill, render as render_drawing, render_with as render_drawing_with, Media,
    Theme,
};
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
        /// The relationship's external target when a `w:hyperlink` wraps the
        /// words — the URL they point at. `None` for internal jumps and for
        /// text that goes nowhere.
        hyperlink: Option<String>,
    },
    Formula {
        text: String,
    },
    Table {
        /// The table's rows; each cell knows its place in the layout grid and
        /// the merges its markup declares.
        rows: Vec<Vec<DocxCell>>,
        /// The grid's width in columns: `w:tblGrid` when the document declares
        /// one, else what the cells' positions and spans add up to.
        num_cols: usize,
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

/// A picture a cell holds: the media part's bytes, the type they are, and the
/// frame the document places them in.
#[derive(Debug, Clone)]
pub struct DocxPicture {
    pub mime: &'static str,
    pub data: Vec<u8>,
    /// Placement size in points: the drawing's `wp:extent`, or an embedded
    /// object's original size.
    pub width_pt: Option<f64>,
    pub height_pt: Option<f64>,
}

/// One cell of a parsed table: its text, its place in the layout grid, and
/// the merges its markup declares.
#[derive(Debug, Clone, Default)]
pub struct DocxCell {
    pub text: String,
    /// The pictures the cell states, in the order they were read: a Word cell
    /// is a container of its own, and a picture in it belongs to the cell
    /// rather than to the table's neighbourhood in the body.
    pub pictures: Vec<DocxPicture>,
    /// Grid column the cell starts at (0-based): `w:gridBefore` plus the
    /// spans of the cells before it in its row.
    pub start_col: usize,
    /// Grid columns the cell covers (`w:gridSpan`), at least one.
    pub col_span: usize,
    /// Grid rows the cell covers, at least one: more than one when the rows
    /// below continued its `w:vMerge`.
    pub row_span: usize,
    /// True when a `w:vMerge` started above covers this slot. The merge's own
    /// cell holds what the cell was written with; this one is a slot of the
    /// grid rather than a cell of the document, exactly as Docling's DOCX
    /// reading treats a `vMerge="continue"` cell.
    pub covered: bool,
    /// True when the cell's row is marked `w:tblHeader` — the document's own
    /// statement of what a header is. Where no row is marked, the first row
    /// carries it: the reading every unmarked table had before the marker was
    /// read at all.
    pub header: bool,
}

/// One `w:tbl` while it is being read.
#[derive(Default)]
struct TableState {
    rows: Vec<TableRow>,
    /// The width `w:tblGrid` declares, counting `w:gridCol` elements as they
    /// are seen; `None` until one is.
    grid_cols: Option<usize>,
}

/// One `w:tr` while it is being read.
#[derive(Default)]
struct TableRow {
    /// `w:tblHeader` printed on the row.
    header: bool,
    /// Grid columns the row skips at its start and end (`w:gridBefore` /
    /// `w:gridAfter`): cells do not occupy them, the grid does.
    grid_before: usize,
    grid_after: usize,
    cells: Vec<CellRead>,
}

/// One `w:tc` while its table is being read: the parse-time facts a
/// [`DocxCell`] is resolved from when the table closes.
#[derive(Default)]
struct CellRead {
    text: String,
    /// The pictures its paragraphs held, resolved to their media parts.
    pictures: Vec<DocxPicture>,
    /// `w:gridSpan`, one when unstated.
    col_span: usize,
    /// `w:vMerge`: a restart opens a vertical merge, anything else continues
    /// the one open above.
    v_merge: Option<VMerge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VMerge {
    Restart,
    Continue,
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

        // A DrawingML shape's colours are the package's theme — a shape's fill
        // is usually `a:fillRef`/`a:schemeClr`, not a literal colour — so the
        // theme part is read once and resolved by every drawing in the package.
        let theme = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .filter(|name| name.starts_with("word/theme/") && name.ends_with(".xml"))
            .min()
            .and_then(|name| read_part(&mut archive, &name))
            .map(|xml| Theme::parse(&xml))
            .unwrap_or_default();

        let rels = read_relationships(&mut archive, "word/_rels/document.xml.rels")?;
        let main = read_part(&mut archive, "word/document.xml")
            .ok_or_else(|| anyhow::anyhow!("package has no word/document.xml"))?;

        let numbering = read_numbering(&mut archive);

        let mut doc = DocxDocument::default();
        let mut extractor = Extractor::new(&rels, &media, &charts, &theme, &numbering);
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
            let mut sub = Extractor::new(&sub_rels, &media, &charts, &theme, &numbering);
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
            let mut sub = Extractor::new(&sub_rels, &media, &charts, &theme, &numbering);
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
                    // Only package-internal targets resolve inside the ZIP;
                    // an external one (`TargetMode="External"`) is a URL as
                    // written — exactly what a `w:hyperlink` points at — and
                    // no other lookup can match it, since `media` holds
                    // package part names.
                    let external = attr(&e, b"TargetMode")
                        .map(|m| m.eq_ignore_ascii_case("external"))
                        .unwrap_or(false);
                    if let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                        if external {
                            out.insert(id, target);
                        } else {
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
    /// `w:lvlPicBulletId`: a `w:numPicBullet` this level prints instead of a
    /// glyph. The bullet is a picture, not a character, so it leaves the
    /// document as one.
    picture: Option<i32>,
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

/// One `w:numPicBullet`: the picture a list level prints as its bullet.
///
/// Word writes the picture the way it writes any other: a `w:drawing` whose
/// `a:blip` names the media part — with the drawing itself in an
/// `asvg:svgBlip` extension when the bullet is an SVG and a PNG fallback beside
/// it — or, in the older spelling, a `w:pict` whose `v:imagedata` names it.
/// Its frame comes with it: `wp:extent` for the drawing, the VML shape's
/// `style` for the picture, and the picture's own viewport when neither says.
#[derive(Debug, Clone, Default)]
struct PictureBullet {
    /// Relationship id of the bullet's image part.
    rel_id: Option<String>,
    /// The drawing itself when the bullet is an SVG stated as Word states one:
    /// a raster on the `a:blip` and an `asvg:svgBlip` extension beside it.
    svg_rel_id: Option<String>,
    width_pt: Option<f64>,
    height_pt: Option<f64>,
}

/// `word/numbering.xml`: the lists a `numId` names, and the pictures a level
/// can print as its bullet.
#[derive(Debug, Default)]
struct Numbering {
    /// `numId` -> its numbering definition.
    lists: HashMap<i32, NumberingDef>,
    /// `numPicBulletId` -> the picture that bullet prints.
    bullets: HashMap<i32, PictureBullet>,
    /// `word/_rels/numbering.xml.rels`: a bullet's picture is a part of the
    /// numbering part, not of the document, so its relationships are read from
    /// there.
    rels: HashMap<String, String>,
}

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
        return Numbering::default();
    };
    let mut abstracts: HashMap<i32, HashMap<i32, NumberingLevel>> = HashMap::new();
    let mut num_to_abstract: HashMap<i32, i32> = HashMap::new();
    // Collected for a `w:num` as they are read, then attached once the abstract
    // schemes are known.
    let mut overrides: HashMap<i32, HashMap<i32, NumberingLevel>> = HashMap::new();
    let mut overridden_starts: HashMap<i32, HashMap<i32, i32>> = HashMap::new();

    // The pictures a level can print as its bullet, collected as they are read
    // and attached to the level that names them by id — the `w:numPicBullet`
    // elements precede the abstract schemes that refer to them.
    let mut bullets: HashMap<i32, PictureBullet> = HashMap::new();
    let mut current_bullet: Option<(i32, PictureBullet)> = None;

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
                    b"lvlPicBulletId" => {
                        if let Some(l) = level.as_mut() {
                            l.picture = attr_local(&e, b"val").and_then(|v| v.parse().ok());
                        }
                    }
                    b"numPicBullet" => {
                        current_bullet = attr_local(&e, b"numPicBulletId")
                            .and_then(|v| v.parse().ok())
                            .map(|id| (id, PictureBullet::default()));
                    }
                    b"extent" => {
                        // The frame the list draws the bullet in, in EMU.
                        if let Some((_, bullet)) = current_bullet.as_mut() {
                            bullet.width_pt = attr_local(&e, b"cx")
                                .and_then(|v| v.parse::<f64>().ok())
                                .map(|v| v / EMU_PER_POINT);
                            bullet.height_pt = attr_local(&e, b"cy")
                                .and_then(|v| v.parse::<f64>().ok())
                                .map(|v| v / EMU_PER_POINT);
                        }
                    }
                    b"blip" => {
                        if let Some((_, bullet)) = current_bullet.as_mut() {
                            bullet.rel_id =
                                attr(&e, b"r:embed").or_else(|| attr_local(&e, b"embed"));
                        }
                    }
                    b"svgBlip" => {
                        if let Some((_, bullet)) = current_bullet.as_mut() {
                            bullet.svg_rel_id =
                                attr(&e, b"r:embed").or_else(|| attr_local(&e, b"embed"));
                        }
                    }
                    b"imagedata" => {
                        if let Some((_, bullet)) = current_bullet.as_mut() {
                            bullet.rel_id = attr(&e, b"r:id").or_else(|| attr_local(&e, b"id"));
                        }
                    }
                    b"shape" => {
                        // A VML bullet states its frame in the `style` string
                        // (`width:9pt;height:9pt`), not in a box element.
                        if let Some((_, bullet)) = current_bullet.as_mut() {
                            if let Some(style) = attr_local(&e, b"style") {
                                bullet.width_pt = vml_length(&style, "width");
                                bullet.height_pt = vml_length(&style, "height");
                            }
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
                b"numPicBullet" => {
                    // A bullet with no image part prints nothing: there is no
                    // glyph to fall back to, so it is not a bullet at all.
                    if let Some((id, bullet)) = current_bullet.take() {
                        if bullet.rel_id.is_some() {
                            bullets.insert(id, bullet);
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    let lists = num_to_abstract
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
        .collect();
    let rels = read_relationships(archive, "word/_rels/numbering.xml.rels").unwrap_or_default();
    Numbering {
        lists,
        bullets,
        rels,
    }
}

/// A length a VML shape states in its `style` string, in points.
///
/// `style="width:9pt;height:9pt"` is how a VML shape declares its frame; the
/// number carries its unit and a bare one is a pixel, as CSS reads it. Anything
/// this cannot read is no size at all, which leaves the picture its own
/// viewport.
fn vml_length(style: &str, property: &str) -> Option<f64> {
    let value = style.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case(property)
            .then(|| value.trim())
    })?;
    let unit_at = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    let number: f64 = value[..unit_at].trim().parse().ok()?;
    let per_unit = match value[unit_at..].trim().to_ascii_lowercase().as_str() {
        "pt" | "" => 1.0,
        "in" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        // A CSS pixel is a ninety-sixth of an inch.
        "px" => 0.75,
        _ => return None,
    };
    (number > 0.0).then_some(number * per_unit)
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

/// Where the tag that ends at `after` starts.
///
/// quick-xml reports how many bytes it has consumed, not where an event began.
/// A tag's own text holds no `<`, so the last one before its end is its opening
/// bracket, and only a `w:pict` pays for the walk.
fn tag_start(xml: &[u8], after: usize) -> Option<usize> {
    let end = after.min(xml.len());
    xml[..end].iter().rposition(|byte| *byte == b'<')
}

/// A picture referenced by a drawing, and where it sits.
#[derive(Default)]
struct PendingPicture {
    rel_id: Option<String>,
    /// Relationship id of the drawing itself when the picture is an SVG, which
    /// Word states as an `asvg:svgBlip` extension beside the PNG fallback.
    svg_rel_id: Option<String>,
    /// Relationship id of a chart this drawing paints, when it paints one.
    chart_rel_id: Option<String>,
    width_pt: Option<f64>,
    height_pt: Option<f64>,
    /// The markup of a `w:pict`, when the drawing is one: a VML shape has no
    /// image part to load, so the bytes have to be drawn instead.
    vml: Option<Vec<u8>>,
    /// The markup of a `w:drawing`, for the same reason: a DrawingML shape
    /// paints itself from its `wps:spPr` and has no image part either.
    drawing: Option<Vec<u8>>,
}

/// Whether an element opens a `mc:Fallback` subtree (the duplicate copy).
fn is_alternate_fallback(e: &BytesStart) -> bool {
    is_markup_compatibility(e.name().as_ref(), b"Fallback")
}

/// Whether an element opens a `mc:AlternateContent` — the pair of a DrawingML
/// shape and its VML copy.
fn is_alternate_content(e: &BytesStart) -> bool {
    is_markup_compatibility(e.name().as_ref(), b"AlternateContent")
}

fn is_markup_compatibility(qualified: &[u8], name: &[u8]) -> bool {
    qualified.starts_with(b"mc:") && local_name(qualified) == name
}

/// One piece of a paragraph, in document order.
#[derive(Debug)]
enum ParagraphPiece {
    /// Prose, exactly as it was written.
    Text(String),
    /// An `m:oMath`, linearised.
    Math(String),
    /// Words a `w:hyperlink` wraps, and the relationship's external target
    /// they point at. A `w:hyperlink w:anchor="…"` jumps inside the document
    /// and names no URL: its words stay plain [`ParagraphPiece::Text`].
    Link { uri: String, text: String },
}

/// The state machine that walks one XML part in document order.
struct Extractor<'a> {
    /// Image bytes keyed by package part name.
    media: &'a HashMap<String, Vec<u8>>,
    /// Chart parts keyed by package part name, for the titles they carry.
    charts: &'a HashMap<String, Vec<u8>>,
    /// The package's theme, where a DrawingML shape's colours come from.
    theme: &'a Theme,
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

    /// Where the open `w:drawing` starts in the part being read, and how many
    /// are open: a DrawingML shape is rendered from its own markup.
    drawing_start: Option<usize>,
    drawing_depth: usize,

    /// Where the open `w:pict` starts in the part being read, and how many are
    /// open: a VML drawing is rendered from its own markup.
    pict_start: Option<usize>,
    pict_depth: usize,

    /// Depth of a `mc:Fallback` being read for its drawings, when its
    /// `mc:Choice` gave no picture. Only the `w:pict` groups inside it are read:
    /// their words were already read from the choice.
    fallback: Option<usize>,

    /// Pictures already materialised outside a paragraph, because a text box
    /// inside the drawing ended the paragraph first. They count as claimed, so
    /// the VML copy beside the drawing is not painted a second time.
    claimed_outside: usize,

    /// Pictures in the paragraph when each open `mc:AlternateContent` began:
    /// how the fallback knows whether the choice drew anything.
    alternate_pictures: Vec<usize>,

    // Table stack: the tables being read, each with its rows and grid.
    tables: Vec<TableState>,

    // Text capture: we only collect text inside a text element.
    in_text: bool,

    /// The external target of the `w:hyperlink` the runs are inside, when
    /// there is one: the URL to put on the words it wraps.
    hyperlink: Option<String>,

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
        theme: &'a Theme,
        numbering: &'a Numbering,
    ) -> Self {
        Self {
            media,
            charts,
            theme,
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
            drawing_start: None,
            drawing_depth: 0,
            claimed_outside: 0,
            pict_start: None,
            pict_depth: 0,
            fallback: None,
            alternate_pictures: Vec::new(),
            tables: Vec::new(),
            in_text: false,
            hyperlink: None,
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
                // of the DrawingML the same shape already provided — unless the
                // choice gave no picture. Then the VML copy is the only drawing
                // the document has, and its `w:pict` groups are read while the
                // rest of the copy stays unread (its words came from the choice).
                Event::Start(e) => {
                    let name = local_name(e.name().as_ref()).to_vec();
                    if let Some(depth) = self.fallback {
                        self.fallback = Some(depth + 1);
                        if name == b"pict" {
                            let at = tag_start(xml, reader.buffer_position() as usize);
                            self.start(&e, at);
                        }
                    } else {
                        match self.skip_depth {
                            Some(depth) => self.skip_depth = Some(depth + 1),
                            None if is_alternate_fallback(&e) => {
                                let drew_something = self
                                    .alternate_pictures
                                    .last()
                                    .is_some_and(|before| self.claimed_pictures() > *before);
                                if drew_something {
                                    self.skip_depth = Some(1);
                                } else {
                                    self.fallback = Some(1);
                                }
                            }
                            None => {
                                if is_alternate_content(&e) {
                                    self.alternate_pictures.push(self.claimed_pictures());
                                }
                                // A `w:pict` is drawn from its own markup, so
                                // where it starts is remembered: quick-xml hands
                                // out events, not the bytes they were read from.
                                let at = (name == b"pict" || name == b"drawing")
                                    .then(|| tag_start(xml, reader.buffer_position() as usize))
                                    .flatten();
                                self.start(&e, at);
                            }
                        }
                    }
                }
                Event::Empty(e) => {
                    // A fallback is read only for its `w:pict` groups, and VML
                    // states the parts of one as empty elements: the
                    // `v:imagedata` naming the image part, its `v:f` formulas.
                    // Those are read — without them the copy has no picture in
                    // it at all — while the copy's words are not.
                    let in_a_fallback_drawing = self.fallback.is_some() && self.pict_depth > 0;
                    if self.skip_depth.is_none()
                        && (self.fallback.is_none() || in_a_fallback_drawing)
                    {
                        self.empty(&e);
                    }
                }
                Event::End(e) => {
                    let name = local_name(e.name().as_ref()).to_vec();
                    if let Some(depth) = self.fallback {
                        if name == b"pict" && self.pict_depth > 0 {
                            self.end(e.name().as_ref(), xml, reader.buffer_position() as usize);
                        }
                        self.fallback = match depth {
                            1 => None,
                            _ => Some(depth - 1),
                        };
                    } else {
                        match self.skip_depth {
                            Some(1) => self.skip_depth = None,
                            Some(depth) => self.skip_depth = Some(depth - 1),
                            None => {
                                if is_markup_compatibility(e.name().as_ref(), b"AlternateContent") {
                                    self.alternate_pictures.pop();
                                }
                                self.end(e.name().as_ref(), xml, reader.buffer_position() as usize);
                            }
                        }
                    }
                }
                Event::Text(e) => {
                    if self.skip_depth.is_none() && self.fallback.is_none() {
                        if let Ok(text) = e.unescape() {
                            self.text(&text);
                        }
                    }
                }
                Event::CData(e) => {
                    if self.skip_depth.is_none() && self.fallback.is_none() {
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

    fn start(&mut self, e: &BytesStart, at: Option<usize>) {
        let name = local_name(e.name().as_ref()).to_vec();

        // Inside math, everything is Office Math: grow the subtree instead.
        if let Some(math) = self.math.as_mut() {
            math.start(e);
            return;
        }

        // A table's, row's or cell's property: routed before the element
        // handling below, which is about paragraphs, pictures and math.
        if self.table_property(&name, e) {
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
            b"hyperlink" => {
                // The relationship's target is the URL, stored verbatim for
                // external targets. A `w:hyperlink` with no `r:id` (one that
                // jumps inside the document) wraps its words as plain text,
                // which is where Docling's own DOCX reading leaves it too.
                self.hyperlink = attr(e, b"r:id").and_then(|id| self.rels.get(&id).cloned());
            }
            b"tbl" => {
                self.tables.push(TableState::default());
            }
            b"tr" => {
                if let Some(t) = self.tables.last_mut() {
                    t.rows.push(TableRow::default());
                }
            }
            b"tc" => {
                if let Some(t) = self.tables.last_mut() {
                    if let Some(row) = t.rows.last_mut() {
                        row.cells.push(CellRead {
                            col_span: 1,
                            ..Default::default()
                        });
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
                if name.as_slice() == b"pict" {
                    self.pict_depth += 1;
                    if self.pict_depth == 1 {
                        self.pict_start = at;
                    }
                }
                if name.as_slice() == b"drawing" {
                    self.drawing_depth += 1;
                    if self.drawing_depth == 1 {
                        self.drawing_start = at;
                    }
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
            b"svgBlip" => {
                // Word writes an SVG picture as two parts: the PNG fallback on
                // the `a:blip` itself, and the drawing named here, in an
                // `asvg:svgBlip` extension beside it. The prefix is the file's
                // to choose, so the element is matched by its local name.
                if let Some(d) = self.drawing.as_mut() {
                    d.svg_rel_id = attr(e, b"r:embed").or_else(|| attr_local(e, b"embed"));
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

        // Word writes each of these as an empty element; some writers use a
        // start/end pair. Both routes take them through one handler so the two
        // forms cannot drift apart.
        if self.table_property(&name, e) {
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
            b"svgBlip" => {
                if let Some(d) = self.drawing.as_mut() {
                    d.svg_rel_id = attr(e, b"r:embed").or_else(|| attr_local(e, b"embed"));
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

    fn end(&mut self, qualified: &[u8], xml: &[u8], after: usize) {
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
            b"hyperlink" => self.hyperlink = None,
            b"pPr" => self.in_paragraph_properties = self.in_paragraph_properties.saturating_sub(1),
            b"p" => self.finish_paragraph(),
            b"drawing" => {
                // The whole drawing is kept for the shapes it paints: a
                // DrawingML shape has no image part, so its own `wps:spPr` is
                // the only thing that can become a picture. A nested drawing
                // is the outer one's markup, so only the outermost is sliced.
                let markup = (self.drawing_depth == 1)
                    .then_some(self.drawing_start)
                    .flatten()
                    .and_then(|start| {
                        let end = after.min(xml.len());
                        (start < end).then(|| xml[start..end].to_vec())
                    });
                self.drawing_depth = self.drawing_depth.saturating_sub(1);
                if self.drawing_depth == 0 {
                    self.drawing_start = None;
                }
                if let Some(mut d) = self.drawing.take() {
                    d.drawing = markup;
                    if self.in_paragraph {
                        self.para_pictures.push(d);
                    } else if let Some(item) = self.materialise(d) {
                        // A text box inside the run ends the paragraph before
                        // the drawing does, and the shape still stands where
                        // the file wrote it.
                        self.claimed_outside += 1;
                        self.items.push(item);
                    }
                }
            }
            b"object" => {
                if let Some(d) = self.drawing.take() {
                    self.para_pictures.push(d);
                }
            }
            b"pict" => {
                // The whole group is kept for the shapes it draws. A nested
                // `w:pict` is the outer one's markup, so only the outermost is
                // sliced.
                let vml = (self.pict_depth == 1)
                    .then_some(self.pict_start)
                    .flatten()
                    .and_then(|start| {
                        let end = after.min(xml.len());
                        (start < end).then(|| xml[start..end].to_vec())
                    });
                self.pict_depth = self.pict_depth.saturating_sub(1);
                if self.pict_depth == 0 {
                    self.pict_start = None;
                }
                if let Some(mut d) = self.drawing.take() {
                    d.vml = vml;
                    if self.in_paragraph {
                        self.para_pictures.push(d);
                    } else if let Some(item) = self.materialise(d) {
                        // A shape can close after its paragraph did — a textbox
                        // inside the run ends the paragraph early — and it still
                        // stands where the file wrote it.
                        self.items.push(item);
                    }
                }
            }
            b"tc" => {
                // Trim the accumulated cell text once the cell closes.
                if let Some(t) = self.tables.last_mut() {
                    if let Some(row) = t.rows.last_mut() {
                        if let Some(cell) = row.cells.last_mut() {
                            cell.text = cell.text.trim().to_string();
                        }
                    }
                }
            }
            b"tbl" => {
                if let Some(table) = self.tables.pop() {
                    if !table.rows.is_empty() {
                        let (num_cols, rows) = resolve_table(table);
                        self.items.push(DocxItem::Table { rows, num_cols });
                    }
                }
            }
            _ => {}
        }
    }

    /// A property of the table being read: the grid's declared width, a row's
    /// header marker or column offset, a cell's span. Word writes each as an
    /// empty element, some writers as a start/end pair — both events route
    /// here, so the two forms cannot drift apart.
    fn table_property(&mut self, name: &[u8], e: &BytesStart) -> bool {
        match name {
            b"gridCol" => {
                if let Some(table) = self.tables.last_mut() {
                    *table.grid_cols.get_or_insert(0) += 1;
                }
            }
            b"tblHeader" => {
                if let Some(row) = self.current_row_mut() {
                    row.header = true;
                }
            }
            b"gridBefore" | b"gridAfter" => {
                // Columns, from `w:val`; its absence means one, as everywhere
                // in WordprocessingML.
                let columns = attr_local(e, b"val")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1);
                if let Some(row) = self.current_row_mut() {
                    if name == b"gridBefore" {
                        row.grid_before = columns;
                    } else {
                        row.grid_after = columns;
                    }
                }
            }
            b"gridSpan" => {
                if let Some(cell) = self.current_cell_mut() {
                    if let Some(columns) =
                        attr_local(e, b"val").and_then(|v| v.parse::<usize>().ok())
                    {
                        cell.col_span = columns.max(1);
                    }
                }
            }
            b"vMerge" => {
                if let Some(cell) = self.current_cell_mut() {
                    // A restart opens a merge; `w:vMerge` with no value
                    // continues the one open above, and so does `val="continue"`.
                    cell.v_merge = Some(if attr_local(e, b"val").as_deref() == Some("restart") {
                        VMerge::Restart
                    } else {
                        VMerge::Continue
                    });
                }
            }
            _ => return false,
        }
        true
    }

    /// Pictures claimed so far: one the part holds with image bytes, a chart or
    /// VML markup, or a DrawingML drawing that really paints — plus the ones
    /// materialised as they closed, outside any paragraph.
    ///
    /// A DrawingML drawing counts only when it renders: the shape it states is
    /// then a picture the choice did produce, so the VML copy beside it would be
    /// the same shape a second time. A drawing that paints nothing (a text box
    /// with no fill, or a preset this reader does not know) claims nothing, and
    /// the copy is still read — it may be the only drawing the file has.
    fn claimed_pictures(&self) -> usize {
        self.para_pictures
            .iter()
            .filter(|picture| self.picture_is_produced(picture))
            .count()
            + self.claimed_outside
    }

    /// Whether this reader can turn a pending picture into a picture of the
    /// document: a relationship that resolves to a part the package holds, a
    /// chart part, or markup that draws.
    ///
    /// A choice that names a part the package does not hold produced nothing —
    /// [`Self::materialise`] answers `None` for it too — so the `mc:Fallback`
    /// beside it is not a duplicate: it is the picture an older reader shows, and
    /// the only drawing the file has.
    fn picture_is_produced(&self, picture: &PendingPicture) -> bool {
        if let Some(target) = self
            .picture_rel(picture)
            .and_then(|rel_id| self.rels.get(&rel_id))
        {
            if mime_for_part(target).is_some() && self.media.contains_key(target) {
                return true;
            }
        }
        if picture
            .chart_rel_id
            .as_ref()
            .and_then(|rel_id| self.rels.get(rel_id))
            .is_some_and(|target| self.charts.contains_key(target))
        {
            return true;
        }
        picture
            .vml
            .as_deref()
            .is_some_and(|markup| vml::render(markup).is_some())
            || picture
                .drawing
                .as_deref()
                .is_some_and(|markup| render_drawing(markup, self.theme).is_some())
    }

    /// The `w:tr` being read, when there is one.
    fn current_row_mut(&mut self) -> Option<&mut TableRow> {
        self.tables.last_mut()?.rows.last_mut()
    }

    /// The `w:tc` being read, when there is one.
    fn current_cell_mut(&mut self) -> Option<&mut CellRead> {
        self.tables.last_mut()?.rows.last_mut()?.cells.last_mut()
    }

    fn text(&mut self, t: &str) {
        // A fallback read for its drawings has no words of its own to add: they
        // were read from the choice beside it.
        if self.fallback.is_some() {
            return;
        }
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
            match (&self.hyperlink, self.para_pieces.last_mut()) {
                // The words of one hyperlink join into one piece — the piece
                // an item of its own is built from.
                (Some(uri), last) => {
                    let uri = uri.clone();
                    match last {
                        Some(ParagraphPiece::Link { uri: open, text }) if *open == uri => {
                            text.push_str(t);
                        }
                        _ => self.para_pieces.push(ParagraphPiece::Link {
                            uri,
                            text: t.to_string(),
                        }),
                    }
                }
                (None, Some(ParagraphPiece::Text(text))) => text.push_str(t),
                (None, _) => self.para_pieces.push(ParagraphPiece::Text(t.to_string())),
            }
        } else if let Some(table) = self.tables.last_mut() {
            // Text between paragraphs inside a cell (rare) still counts.
            if let Some(row) = table.rows.last_mut() {
                if let Some(cell) = row.cells.last_mut() {
                    cell.text.push_str(t);
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
        // The definition is copied out: a picture bullet is materialised while
        // the level is still being read, and loading a picture needs the
        // extractor rather than the numbering it came from.
        let Some(def) = self.numbering.lists.get(&num_id).cloned() else {
            return (None, level);
        };
        let Some(level_def) = def.level(index).cloned() else {
            return (None, level);
        };
        // `numFmt="none"` prints nothing at all.
        if level_def.format == "none" {
            return (None, level);
        }

        // A level whose bullet is a picture prints that picture. It is the one
        // thing about a list item that is a drawing rather than words, so it
        // leaves as the picture it is, ahead of the words it introduces — and a
        // drawing stated as an SVG is played into pixels there, like any other
        // picture in the package.
        if let Some(bullet) = level_def
            .picture
            .and_then(|id| self.numbering.bullets.get(&id))
            .cloned()
        {
            if let Some(target) = self.bullet_target(&bullet) {
                if let Some(picture) = self.picture_item(&target, bullet.width_pt, bullet.height_pt)
                {
                    self.items.push(picture);
                }
            }
            // The picture *is* the marker: the words carry no glyph of their
            // own, and a counter this level never prints is not advanced.
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
        if !self.tables.is_empty() {
            if let Some(table) = self.tables.last_mut() {
                if !text.is_empty() {
                    if let Some(row) = table.rows.last_mut() {
                        if let Some(cell) = row.cells.last_mut() {
                            if !cell.text.is_empty() {
                                cell.text.push(' ');
                            }
                            cell.text.push_str(&text);
                        }
                    }
                }
            }
            self.para_text.clear();
            self.para_has_plain_text = false;
            self.para_pieces.clear();
            // The picture is the cell's: it is resolved to its media part and
            // kept on the cell it was read in, rather than left in the body
            // beside the table. A cell that holds nothing else is still a cell
            // of the grid, and its picture travels with it.
            for pending in pictures {
                if let Some(DocxItem::Picture {
                    mime,
                    data,
                    width_pt,
                    height_pt,
                }) = self.materialise(pending)
                {
                    if let Some(cell) = self
                        .tables
                        .last_mut()
                        .and_then(|table| table.rows.last_mut())
                        .and_then(|row| row.cells.last_mut())
                    {
                        cell.pictures.push(DocxPicture {
                            mime,
                            data,
                            width_pt,
                            height_pt,
                        });
                    }
                }
            }
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
                        _ => None,
                    })
                    .collect();
                let trailing: Vec<String> = pieces[last + 1..]
                    .iter()
                    .filter_map(|piece| match piece {
                        ParagraphPiece::Math(equation) => Some(equation.clone()),
                        _ => None,
                    })
                    .collect();

                for equation in leading {
                    self.items.push(DocxItem::Formula { text: equation });
                }

                // Prose, with every `w:hyperlink` as an item of its own: the
                // words before the link, the link's words holding the URL, the
                // words after — the split Docling's own DOCX reading makes of
                // a linked sentence. The bullet the paragraph printed belongs
                // to the item as a whole, so the first piece carries it and
                // the rest keep only the label they all shared.
                let (marker, level) = self.numbering_for_paragraph();
                let label = self.para_label.clone();
                let mut marker = marker;
                let mut level = level;
                let mut prose = String::new();
                for piece in &pieces[first..=last] {
                    match piece {
                        ParagraphPiece::Text(text) => prose.push_str(text),
                        ParagraphPiece::Math(equation) => {
                            // The equation sits at the spacing the paragraph
                            // was written with: one space before it, when
                            // the run before did not already end in one.
                            if !prose.is_empty() && !prose.ends_with(' ') {
                                prose.push(' ');
                            }
                            prose.push_str(equation);
                        }
                        ParagraphPiece::Link { uri, text } => {
                            let carried = std::mem::take(&mut prose);
                            emit_prose(&mut self.items, &carried, &label, &mut marker, &mut level);
                            let text = text.trim().to_string();
                            if !text.is_empty() {
                                self.items.push(DocxItem::Text {
                                    text,
                                    label: label.clone(),
                                    marker: marker.take(),
                                    level: level.take(),
                                    hyperlink: Some(uri.clone()),
                                });
                            }
                        }
                    }
                }
                emit_prose(&mut self.items, &prose, &label, &mut marker, &mut level);

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
    /// The relationship a picture's bytes come from, preferring the drawing.
    ///
    /// Word 2016 and later stores an SVG picture twice: a PNG fallback on the
    /// `a:blip`'s own `r:embed`, for readers that do not draw SVG, and the
    /// drawing itself in an `asvg:svgBlip` extension beside it. The fallback is
    /// only what an old reader shows — the SVG is the picture the file asks for,
    /// and the one whose resolution the file leaves open — so it is taken
    /// whenever its part is in the package. A `svgBlip` that points at anything
    /// else, or at a part this package does not hold, leaves the fallback as the
    /// picture, which is what the file itself says it is for.
    fn picture_rel(&self, pending: &PendingPicture) -> Option<String> {
        let drawing = pending.svg_rel_id.as_ref().and_then(|id| {
            let target = self.rels.get(id)?;
            let mime = mime_for_part(target)?;
            (mime == "image/svg+xml" && self.media.contains_key(target)).then(|| id.clone())
        });
        drawing.or_else(|| pending.rel_id.clone())
    }

    /// The part a picture bullet's image is.
    ///
    /// A bullet's media is named from `word/numbering.xml`, so its relationship
    /// lives in that part's own `.rels` — `rId1` there is not the document's
    /// `rId1`. The document's relationships are consulted second, for a writer
    /// that names the bullet's image beside the body's pictures. A drawing
    /// stated twice is taken from its `asvg:svgBlip`, the same way a body
    /// picture's is.
    fn bullet_target(&self, bullet: &PictureBullet) -> Option<String> {
        let lookup = |id: &str| {
            self.numbering
                .rels
                .get(id)
                .or_else(|| self.rels.get(id))
                .cloned()
        };
        let drawing = bullet.svg_rel_id.as_ref().and_then(|id| {
            let target = lookup(id)?;
            (mime_for_part(&target) == Some("image/svg+xml")).then_some(target)
        });
        drawing.or_else(|| bullet.rel_id.as_ref().and_then(|id| lookup(id)))
    }

    /// The picture item one media part makes, at the size it is placed at.
    ///
    /// A drawing is played into pixels first, because a metafile is not an
    /// encoding Docling can name; a picture whose EXIF tag turns it is turned,
    /// because that is the picture the document displays. `None` when the part
    /// is not in the package or its extension names no picture this knows.
    fn picture_item(
        &self,
        target: &str,
        width_pt: Option<f64>,
        height_pt: Option<f64>,
    ) -> Option<DocxItem> {
        let mime = match mime_for_part(target) {
            Some(mime) => mime,
            None => {
                tracing::debug!("skipping unsupported DOCX image part {target}");
                return None;
            }
        };
        let data = self.media.get(target)?.clone();
        // A drawing is played into a PNG here, so a picture Word stored as
        // vector calls is a picture of pixels, with a media type Docling can
        // name. One that will not play keeps its own payload and stays a box
        // with no image.
        if drawing::is_vector_drawing(mime) {
            if let Some(png) = drawing::rasterize_to_png(&data, width_pt, height_pt) {
                return Some(DocxItem::Picture {
                    mime: "image/png",
                    data: png,
                    width_pt,
                    height_pt,
                });
            }
        }
        // Word shows a picture the way its EXIF orientation says, but the
        // package keeps the pixels the camera stored. Turn them here, so the
        // picture that leaves the parser is the one the document displays.
        let (mime, data) = match pdfextractor_image_parser::turn_upright(&data) {
            Some(upright) => {
                tracing::debug!(
                    "turned an embedded picture upright ({}x{})",
                    upright.width,
                    upright.height
                );
                (upright.mimetype, upright.data)
            }
            None => (mime, data),
        };
        Some(DocxItem::Picture {
            mime,
            data,
            width_pt,
            height_pt,
        })
    }

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
                    hyperlink: None,
                });
            }
        }

        // A drawing that fills a shape with a picture is painted whole. A fill
        // is part of the shape that holds it, so the picture is drawn through
        // the shape's own geometry, under its line; the shape is not the fill's
        // bare picture standing where the shape is.
        if let Some(markup) = pending.drawing.as_deref() {
            if paints_a_picture_fill(markup) {
                let media = DocxMedia {
                    rels: self.rels,
                    media: self.media,
                };
                if let Some(painted) = render_drawing_with(markup, self.theme, &media) {
                    return Some(DocxItem::Picture {
                        mime: "image/png",
                        data: painted.png,
                        width_pt: Some(painted.width_pt),
                        height_pt: Some(painted.height_pt),
                    });
                }
            }
        }

        let rel_id = match self.picture_rel(&pending) {
            Some(rel_id) => rel_id,
            None => {
                // A drawing with no image part is one Word paints itself — a
                // DrawingML `wps:wsp`, or a VML autoshape, group or WordArt — so
                // there are no bytes to load and the markup is drawn instead.
                // A `w:object` has no markup worth drawing without its preview.
                if let Some(drawing) = pending
                    .drawing
                    .as_deref()
                    .and_then(|markup| render_drawing(markup, self.theme))
                {
                    return Some(DocxItem::Picture {
                        mime: "image/png",
                        data: drawing.png,
                        width_pt: Some(drawing.width_pt),
                        height_pt: Some(drawing.height_pt),
                    });
                }
                let drawing = vml::render(pending.vml.as_deref()?)?;
                return Some(DocxItem::Picture {
                    mime: "image/png",
                    data: drawing.png,
                    width_pt: Some(drawing.width_pt),
                    height_pt: Some(drawing.height_pt),
                });
            }
        };
        let target = self.rels.get(&rel_id)?.clone();
        self.picture_item(&target, pending.width_pt, pending.height_pt)
    }
}

/// The document's pictures, by the relationship id a shape's fill names.
///
/// The painter draws a picture fill through the shape that holds it, and only a
/// reader that has the package can resolve an id to bytes: this is that lookup,
/// over the document's own relationships. A part the package does not hold is
/// no picture, and a fill that names one is a fill this cannot paint.
struct DocxMedia<'a> {
    rels: &'a HashMap<String, String>,
    media: &'a HashMap<String, Vec<u8>>,
}

impl Media for DocxMedia<'_> {
    fn picture(&self, id: &str) -> Option<(String, Vec<u8>)> {
        let target = self.rels.get(id)?;
        let mime = mime_for_part(target)?;
        let data = self.media.get(target)?;
        Some((mime.to_string(), data.clone()))
    }
}

/// The media type an embedded picture part is named by, from its extension.
///
/// The list is [`pdfextractor_picture_formats::PICTURE_FORMATS`] — the same one
/// the CLI writes a picture under and the flow reader accepts a `<source>`
/// for — so a Word package's media type and the file it becomes are the same
/// fact, and a format added there is read out of a package without another
/// edit here. An extension the table does not carry (`image/avif`, or a part
/// with no extension at all) is not a picture this names.
fn mime_for_part(target: &str) -> Option<&'static str> {
    pdfextractor_picture_formats::mime_for_extension(target.rsplit_once('.')?.1)
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
    // A hyperlink's words are prose too: a paragraph that *opens* with a link
    // has its first prose piece there, and refusing it would drop the link on
    // the floor.
    let prose = |piece: &ParagraphPiece| {
        matches!(piece, ParagraphPiece::Text(_) | ParagraphPiece::Link { .. })
    };
    let first = pieces
        .iter()
        .position(prose)
        .expect("a paragraph with prose has words");
    let last = pieces
        .iter()
        .rposition(prose)
        .expect("a paragraph with prose has words");
    (first, last)
}

/// Emit prose accumulated so far as one text item, when it holds words.
fn emit_prose(
    items: &mut Vec<DocxItem>,
    prose: &str,
    label: &str,
    marker: &mut Option<String>,
    level: &mut Option<i32>,
) {
    let text = prose.trim().to_string();
    if text.is_empty() {
        return;
    }
    items.push(DocxItem::Text {
        text,
        label: label.to_string(),
        marker: marker.take(),
        level: level.take(),
        hyperlink: None,
    });
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

/// The value of a property such as `fPr/type` or `dPr/begChr`, from the
/// element's `val` attribute or, in its absence, its text.
fn math_prop<'a>(n: &'a MathNode, property: &str, tag: &str) -> Option<&'a str> {
    let pr = math_child(n, property)?;
    let child = math_child(pr, tag)?;
    child
        .attr("val")
        .or_else(|| (!child.text.is_empty()).then_some(child.text.as_str()))
}

fn render_math_children(n: &MathNode) -> String {
    n.children.iter().map(render_math_node).collect()
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

/// Render one Office Math node as LaTeX, in the notation Docling uses.
///
/// The mapping is Docling's own OMML converter's: a fraction is `\frac` (or the
/// variant its properties name), scripts are `_{…}`/`^{…}` over their base, a
/// delimiter is `\left…\right`, a radical is `\sqrt` with an optional degree,
/// and an n-ary operator is its command (`\sum`, `\iiint`, …) followed by its
/// limits and body. Function names map to their commands (`\sin`, `\lim`, …),
/// matrices become a `matrix` environment, and a character outside ASCII maps
/// to the LaTeX spelling of the same symbol — so an equation is the notation a
/// LaTeX reader, and Docling, can read back.
fn render_math_node(n: &MathNode) -> String {
    // Property and control elements never surface on their own.
    if n.name.ends_with("Pr") || n.name == "chr" {
        return String::new();
    }
    match n.name.as_str() {
        "t" => latex_text(&n.text),
        "oMath" | "oMathPara" | "e" | "num" | "den" | "deg" | "box" | "borderBox" => {
            render_math_children(n)
        }
        // A run owns its text; the `w:rPr` inside it is formatting, not words.
        "r" => n
            .children
            .iter()
            .filter(|c| c.name == "t")
            .map(|c| latex_text(&c.text))
            .collect(),
        "f" => render_fraction(n),
        "sSup" => {
            let base = script_base(math_child(n, "e"));
            format!("{base}^{{{}}}", render_child(n, "sup"))
        }
        "sSub" => {
            let base = script_base(math_child(n, "e"));
            format!("{base}_{{{}}}", render_child(n, "sub"))
        }
        "sSubSup" => {
            let base = script_base(math_child(n, "e"));
            format!(
                "{base}_{{{}}}^{{{}}}",
                render_child(n, "sub"),
                render_child(n, "sup")
            )
        }
        // Docling writes a pre-script as the leading scripts alone; the base
        // they decorate is not repeated after them.
        "sPre" => format!(
            "_{{{}}}^{{{}}}",
            render_child(n, "sub"),
            render_child(n, "sup")
        ),
        "sub" => format!("_{{{}}}", render_math_children(n)),
        "sup" => format!("^{{{}}}", render_math_children(n)),
        "d" => render_delimiter(n),
        "rad" => render_radical(n),
        "nary" => render_nary(n),
        "limLow" => {
            let base = render_child(n, "e");
            let lim = math_child(n, "lim").map(render_limit).unwrap_or_default();
            let base = base.trim();
            match base {
                "lim" => format!("\\lim_{{{lim}}}"),
                "max" => format!("\\max_{{{lim}}}"),
                "min" => format!("\\min_{{{lim}}}"),
                "argmax" => format!("\\operatorname{{argmax}}_{{{lim}}}"),
                "argmin" => format!("\\operatorname{{argmin}}_{{{lim}}}"),
                _ => format!("{base}_{{{lim}}}"),
            }
        }
        "limUpp" => {
            let base = render_child(n, "e");
            let lim = math_child(n, "lim").map(render_limit).unwrap_or_default();
            format!("\\overset{{{lim}}}{{{base}}}")
        }
        "lim" => render_limit(n),
        "acc" => {
            let body = render_child(n, "e");
            let mark = math_prop(n, "accPr", "chr").unwrap_or("\u{302}");
            accent_latex(mark, &body).unwrap_or_else(|| mark.to_string())
        }
        "bar" => {
            let body = render_child(n, "e");
            match math_prop(n, "barPr", "pos") {
                Some("bot") => format!("\\underline{{{body}}}"),
                _ => format!("\\overline{{{body}}}"),
            }
        }
        "groupChr" => {
            let body = render_child(n, "e");
            let mark = math_prop(n, "groupChrPr", "chr").unwrap_or("\u{23df}");
            accent_latex(mark, &body).unwrap_or(mark.to_string())
        }
        "func" => {
            let name = math_child(n, "fName")
                .map(render_function_name)
                .unwrap_or_default();
            name.replace("{fe}", &render_child(n, "e"))
        }
        "fName" => render_function_name(n),
        "m" => {
            let rows: Vec<String> = n
                .children
                .iter()
                .filter(|c| c.name == "mr")
                .map(render_matrix_row)
                .collect();
            format!("\\begin{{matrix}}{}\\end{{matrix}}", rows.join("\\\\"))
        }
        "mr" => render_matrix_row(n),
        "eqArr" => n
            .children
            .iter()
            .filter(|c| c.name == "e")
            .map(render_math_children)
            .collect::<Vec<_>>()
            .join("\\\\"),
        // Anything left is a wrapper whose children are the equation.
        _ => render_math_children(n),
    }
}

fn render_child(n: &MathNode, name: &str) -> String {
    math_child(n, name)
        .map(render_math_children)
        .unwrap_or_default()
}

/// A script's base, braced when it is a construct that must keep its own
/// scope (`\frac`, `\sqrt`).
fn script_base(base: Option<&MathNode>) -> String {
    let base = base.map(render_math_children).unwrap_or_default();
    let base = base.trim_end().to_string();
    if base.contains("\\frac") || base.contains("\\sqrt") {
        format!("{{{base}}}")
    } else {
        base
    }
}

/// A fraction, in the style its `fPr/type` names — Docling's four styles.
fn render_fraction(n: &MathNode) -> String {
    let num = render_child(n, "num");
    let den = render_child(n, "den");
    match math_prop(n, "fPr", "type").unwrap_or("bar") {
        "noBar" => format!("\\genfrac{{}}{{}}{{0pt}}{{}}{{{num}}}{{{den}}}"),
        "lin" => format!("{{{num}}}/{{{den}}}"),
        "skw" => format!("^{{{num}}}/_{{{den}}}"),
        _ => format!("\\frac{{{num}}}{{{den}}}"),
    }
}

/// A delimiter pair: the characters its properties name, or `(` and `)`, with
/// an empty side written as `.` (LaTeX's “no delimiter”).
fn render_delimiter(n: &MathNode) -> String {
    let left = math_prop(n, "dPr", "begChr").unwrap_or("(");
    let right = math_prop(n, "dPr", "endChr").unwrap_or(")");
    let delimiter = |value: &str| {
        if value.is_empty() {
            ".".to_string()
        } else {
            escape_delimiter(value)
        }
    };
    format!(
        "\\left{}{}\\right{}",
        delimiter(left),
        render_child(n, "e"),
        delimiter(right)
    )
}

fn render_radical(n: &MathNode) -> String {
    let body = render_child(n, "e");
    let degree = render_child(n, "deg");
    if degree.trim().is_empty() {
        format!("\\sqrt{{{body}}}")
    } else {
        format!("\\sqrt[{}]{{{body}}}", degree.trim())
    }
}

/// An n-ary operator: its command from the `chr` attribute (`\sum`, `\iiint`,
/// …), then the limits, then the body — empty limits as `_{}`/`^{}`, because
/// the element was there and the equation printed nothing over or under it.
fn render_nary(n: &MathNode) -> String {
    let operator = property_char(n)
        .map(|c| nary_command(&c).to_string())
        .unwrap_or_else(|| "\\int".to_string());
    let mut out = operator;
    if let Some(sub) = math_child(n, "sub") {
        out.push_str(&format!("_{{{}}}", render_math_children(sub)));
    }
    if let Some(sup) = math_child(n, "sup") {
        out.push_str(&format!("^{{{}}}", render_math_children(sup)));
    }
    out.push_str(&render_child(n, "e"));
    out
}

/// A limit label: arrows become `\to`, and a label of plain words has its
/// spaces escaped so LaTeX does not swallow them.
fn render_limit(n: &MathNode) -> String {
    let rendered = render_math_children(n).replace("\\rightarrow", "\\to");
    let mut text = rendered.trim().to_string();
    while text.ends_with("\\\\") {
        text.truncate(text.len() - 2);
        text = text.trim_end().to_string();
    }
    if !text.is_empty() && !text.chars().any(|c| "\\<>=+*/^_{}".contains(c)) {
        text = text.replace(' ', "\\ ");
    }
    text
}

/// A function's name: each run is a known command (`\sin`, `\cos`, …) or the
/// words it printed, followed by the placeholder the body replaces.
fn render_function_name(n: &MathNode) -> String {
    let mut out = String::new();
    for child in &n.children {
        if child.name == "r" {
            let words: String = child
                .children
                .iter()
                .filter(|c| c.name == "t")
                .map(|c| c.text.as_str())
                .collect();
            match function_template(words.trim()) {
                Some(template) => out.push_str(template),
                None => out.push_str(&latex_text(&words)),
            }
        } else {
            out.push_str(&render_math_node(child));
        }
    }
    if !out.contains("{fe}") {
        out.push_str("{fe}");
    }
    out
}

fn render_matrix_row(n: &MathNode) -> String {
    n.children
        .iter()
        .filter(|c| c.name == "e")
        .map(render_math_children)
        .collect::<Vec<_>>()
        .join("&")
}

/// The LaTeX command for an n-ary character. A missing `chr` means an integral.
fn nary_command(c: &str) -> &'static str {
    match c {
        "\u{2140}" => "\\Bbbsum",
        "\u{220f}" => "\\prod",
        "\u{2210}" => "\\coprod",
        "\u{2211}" => "\\sum",
        "\u{222b}" => "\\int",
        "\u{222c}" => "\\iint",
        "\u{222d}" => "\\iiint",
        "\u{222e}" => "\\oint",
        "\u{222f}" => "\\oiint",
        "\u{2230}" => "\\oiiint",
        "\u{22c0}" => "\\bigwedge",
        "\u{22c1}" => "\\bigvee",
        "\u{22c2}" => "\\bigcap",
        "\u{22c3}" => "\\bigcup",
        "\u{2a00}" => "\\bigodot",
        "\u{2a01}" => "\\bigoplus",
        "\u{2a02}" => "\\bigotimes",
        _ => "\\int",
    }
}

/// The commands a function name maps to, with `{fe}` where the argument goes.
fn function_template(name: &str) -> Option<&'static str> {
    Some(match name {
        "sin" => "\\sin({fe})",
        "cos" => "\\cos({fe})",
        "tan" => "\\tan({fe})",
        "arcsin" => "\\arcsin({fe})",
        "arccos" => "\\arccos({fe})",
        "arctan" => "\\arctan({fe})",
        "arccot" => "\\arccot({fe})",
        "sinh" => "\\sinh({fe})",
        "cosh" => "\\cosh({fe})",
        "tanh" => "\\tanh({fe})",
        "coth" => "\\coth({fe})",
        "sec" => "\\sec({fe})",
        "csc" => "\\csc({fe})",
        "mod" => "\\mod {fe}",
        "max" => "\\max({fe})",
        "min" => "\\min({fe})",
        "log" => "\\log({fe})",
        "ln" => "\\ln({fe})",
        "exp" => "\\exp({fe})",
        "det" => "\\det({fe})",
        "gcd" => "\\gcd({fe})",
        "deg" => "\\deg({fe})",
        "hom" => "\\hom({fe})",
        "ker" => "\\ker({fe})",
        "dim" => "\\dim({fe})",
        "arg" => "\\arg({fe})",
        "inf" => "\\inf({fe})",
        "sup" => "\\sup({fe})",
        "lim" => "\\lim({fe})",
        "Pr" => "\\Pr({fe})",
        _ => return None,
    })
}

/// The accent commands for the combining marks OMML names.
fn accent_latex(mark: &str, body: &str) -> Option<String> {
    Some(match mark {
        "\u{300}" => format!("\\grave{{{body}}}"),
        "\u{301}" => format!("\\acute{{{body}}}"),
        "\u{302}" => format!("\\hat{{{body}}}"),
        "\u{303}" => format!("\\tilde{{{body}}}"),
        "\u{304}" => format!("\\bar{{{body}}}"),
        "\u{305}" => format!("\\overbar{{{body}}}"),
        "\u{306}" => format!("\\breve{{{body}}}"),
        "\u{307}" => format!("\\dot{{{body}}}"),
        "\u{308}" => format!("\\ddot{{{body}}}"),
        "\u{309}" => format!("\\ovhook{{{body}}}"),
        "\u{30a}" => format!("\\ocirc{{{body}}}"),
        "\u{30c}" => format!("\\check{{{body}}}"),
        "\u{310}" => format!("\\candra{{{body}}}"),
        "\u{312}" => format!("\\oturnedcomma{{{body}}}"),
        "\u{315}" => format!("\\ocommatopright{{{body}}}"),
        "\u{31a}" => format!("\\droang{{{body}}}"),
        "\u{338}" => format!("\\not{{{body}}}"),
        "\u{20d0}" => format!("\\leftharpoonaccent{{{body}}}"),
        "\u{20d1}" => format!("\\rightharpoonaccent{{{body}}}"),
        "\u{20d2}" => format!("\\vertoverlay{{{body}}}"),
        "\u{20d6}" => format!("\\overleftarrow{{{body}}}"),
        "\u{20d7}" => format!("\\vec{{{body}}}"),
        "\u{20db}" => format!("\\dddot{{{body}}}"),
        "\u{20dc}" => format!("\\ddddot{{{body}}}"),
        "\u{20e1}" => format!("\\overleftrightarrow{{{body}}}"),
        "\u{20e7}" => format!("\\annuity{{{body}}}"),
        "\u{20e9}" => format!("\\widebridgeabove{{{body}}}"),
        "\u{20f0}" => format!("\\asteraccent{{{body}}}"),
        "\u{330}" => format!("\\wideutilde{{{body}}}"),
        "\u{331}" => format!("\\underbar{{{body}}}"),
        "\u{20e8}" => format!("\\threeunderdot{{{body}}}"),
        "\u{20ec}" => format!("\\underrightharpoondown{{{body}}}"),
        "\u{20ed}" => format!("\\underleftharpoondown{{{body}}}"),
        "\u{20ee}" => format!("\\underledtarrow{{{body}}}"),
        "\u{20ef}" => format!("\\underrightarrow{{{body}}}"),
        "\u{23b4}" => format!("\\overbracket{{{body}}}"),
        "\u{23b5}" => format!("\\underbracket{{{body}}}"),
        "\u{23dc}" => format!("\\overparen{{{body}}}"),
        "\u{23dd}" => format!("\\underparen{{{body}}}"),
        "\u{23de}" => format!("\\overbrace{{{body}}}"),
        "\u{23df}" => format!("\\underbrace{{{body}}}"),
        _ => return None,
    })
}

/// Escape the characters LaTeX treats as syntax in a delimiter.
fn escape_delimiter(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '{' | '}' | '_' | '^' | '#' | '&' | '$' | '%' | '~' => format!("\\{c}"),
            _ => c.to_string(),
        })
        .collect()
}

/// Text of a math run as LaTeX: symbols get their commands, and the characters
/// LaTeX reserves are escaped. Spacing around a symbol mirrors Docling's: a
/// `\pi` or `\infty` is surrounded by spaces, which the renderer collapses.
fn latex_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if let Some(letter) = italic_letter(c) {
            out.push(letter);
        } else if let Some(symbol) = latex_symbol(c) {
            out.push_str(symbol);
        } else if matches!(c, '{' | '}' | '_' | '^' | '#' | '&' | '$' | '%' | '~') {
            out.push('\\');
            out.push(c);
        } else {
            out.push(c);
        }
    }
    out
}

fn latex_symbol(c: char) -> Option<&'static str> {
    Some(match c {
        // A dash between terms is a minus, whatever dash the run uses.
        '\u{2212}' | '\u{2013}' | '\u{2014}' => "-",
        // Greek letters.
        '\u{3b1}' => " \\alpha ",
        '\u{3b2}' => " \\beta ",
        '\u{3b3}' => " \\gamma ",
        '\u{3b4}' => " \\delta ",
        '\u{3b5}' => " \\epsilon ",
        '\u{3b6}' => " \\zeta ",
        '\u{3b7}' => " \\eta ",
        '\u{3b8}' => " \\theta ",
        '\u{3b9}' => " \\iota ",
        '\u{3ba}' => " \\kappa ",
        '\u{3bb}' => " \\lambda ",
        '\u{3bc}' => " \\mu ",
        '\u{3bd}' => " \\nu ",
        '\u{3be}' => " \\xi ",
        '\u{3bf}' => " \\omicron ",
        '\u{3c0}' => " \\pi ",
        '\u{3c1}' => " \\rho ",
        '\u{3c2}' => " \\varsigma ",
        '\u{3c3}' => " \\sigma ",
        '\u{3c4}' => " \\tau ",
        '\u{3c5}' => " \\upsilon ",
        '\u{3c6}' => " \\phi ",
        '\u{3c7}' => " \\chi ",
        '\u{3c8}' => " \\psi ",
        '\u{3c9}' => " \\omega ",
        '\u{393}' => " \\Gamma ",
        '\u{394}' => " \\Delta ",
        '\u{398}' => " \\Theta ",
        '\u{39b}' => " \\Lambda ",
        '\u{39e}' => " \\Xi ",
        '\u{3a0}' => " \\Pi ",
        '\u{3a3}' => " \\Sigma ",
        '\u{3a6}' => " \\Phi ",
        '\u{3a8}' => " \\Psi ",
        '\u{3a9}' => " \\Omega ",
        '\u{2202}' => " \\partial ",
        '\u{2207}' => " \\nabla ",
        // Symbols and relations.
        '\u{221e}' => " \\infty ",
        '\u{b1}' => " \\pm ",
        '\u{2213}' => " \\mp ",
        '\u{d7}' => " \\times ",
        '\u{f7}' => " \\div ",
        '\u{2219}' | '\u{22c5}' => " \\bullet ",
        '\u{2216}' => " \\setminus ",
        '\u{2264}' => " \\leq ",
        '\u{2265}' => " \\geq ",
        '\u{2260}' => " \\ne ",
        '\u{2248}' => " \\approx ",
        '\u{2208}' => " \\in ",
        '\u{2209}' => " \\notin ",
        '\u{2282}' => " \\subset ",
        '\u{2286}' => " \\subseteq ",
        '\u{222a}' => " \\cup ",
        '\u{2229}' => " \\cap ",
        '\u{2192}' => " \\text{ \\textrightarrow } ",
        '\u{2190}' => " \\leftarrow ",
        '\u{2194}' => " \\leftrightarrow ",
        '\u{22ef}' => " \\cdots ",
        '\u{2026}' => " \\text{ \\textellipsis } ",
        '\u{2211}' => " \\sum ",
        '\u{220f}' => " \\prod ",
        '\u{222b}' => " \\int ",
        '\u{222c}' => " \\iint ",
        '\u{222d}' => " \\iiint ",
        '\u{222e}' => " \\oint ",
        '\u{222f}' => " \\oiint ",
        '\u{2230}' => " \\oiiint ",
        // Quotes become their plain ASCII counterpart.
        '\u{2019}' | '\u{2018}' => "'",
        '\u{201c}' | '\u{201d}' => "\"",
        _ => return None,
    })
}

/// Mathematical italic letters (U+1D434…, and the italic h at U+210E) are
/// ordinary letters to LaTeX; map them back to ASCII.
fn italic_letter(c: char) -> Option<char> {
    let code = c as u32;
    if (0x1D434..=0x1D44D).contains(&code) {
        char::from_u32('A' as u32 + code - 0x1D434)
    } else if (0x1D44E..=0x1D467).contains(&code) {
        char::from_u32('a' as u32 + code - 0x1D44E)
    } else if code == 0x210E {
        Some('h')
    } else {
        None
    }
}

/// Resolve a table as Word wrote it into the grid the document model holds.
///
/// Three readings happen here, all of them markup semantics rather than
/// guesses:
///
/// * **Columns**: a cell starts after `w:gridBefore` and the spans of the
///   cells before it in its row (`w:gridSpan` widens a cell, and the cells
///   after it sit at the columns it pushed them to, not at their own
///   positions in the row).
/// * **Rows**: a `w:vMerge` restart opens a merge that each continuation
///   below extends — the opening cell gains the rows, and the continuing
///   cell is marked covered rather than standing as a cell of its own. The
///   chain is carried only by continuations: a row whose cell at that column
///   says nothing closes it. A continuation with no merge open at its column
///   is malformed markup, and its cell simply stands (Docling's DOCX backend
///   falls the same way).
/// * **Headers**: the rows `w:tblHeader` marks; where no row is marked, the
///   first — the reading every unmarked table had before the marker was read
///   at all.
///
/// The width is `w:tblGrid` when the document declares one, else what the
/// rows add up to — never less than what the cells reach, so no cell can sit
/// outside the grid it belongs to.
fn resolve_table(table: TableState) -> (usize, Vec<Vec<DocxCell>>) {
    // Every cell placed on its row's grid line.
    let mut rows: Vec<Vec<DocxCell>> = Vec::with_capacity(table.rows.len());
    let mut merges: Vec<Vec<Option<VMerge>>> = Vec::with_capacity(table.rows.len());
    let mut widest = 0;
    for row in &table.rows {
        let mut column = row.grid_before;
        let mut cells = Vec::with_capacity(row.cells.len());
        for cell in &row.cells {
            let span = cell.col_span.max(1);
            cells.push(DocxCell {
                text: cell.text.clone(),
                pictures: cell.pictures.clone(),
                start_col: column,
                col_span: span,
                row_span: 1,
                covered: false,
                header: false,
            });
            column += span;
        }
        widest = widest.max(column + row.grid_after);
        rows.push(cells);
        merges.push(row.cells.iter().map(|cell| cell.v_merge).collect());
    }

    // Vertical merges, row by row: what is open after one row depends only on
    // that row's continuations, so the chains are rebuilt each time. A chain
    // is keyed by the column it opened at — two merges cannot share a column
    // of the same row — and extending writes the opening cell's row span.
    let mut open: Vec<(usize, usize, usize)> = Vec::new(); // (row, cell, column)
    for r in 0..rows.len() {
        // What each cell of the row does, read once: the row itself must stay
        // free for the writes the chain makes into it (and rows above it).
        let placed: Vec<(usize, Option<VMerge>)> = rows[r]
            .iter()
            .zip(merges[r].iter())
            .map(|(cell, merge)| (cell.start_col, *merge))
            .collect();
        let mut chained: Vec<(usize, usize, usize)> = Vec::new();
        for (c, (column, merge)) in placed.iter().enumerate() {
            match merge {
                Some(VMerge::Restart) => chained.push((r, c, *column)),
                Some(VMerge::Continue) => {
                    if let Some(&(o_row, o_cell, o_col)) =
                        open.iter().find(|(_, _, open_col)| *open_col == *column)
                    {
                        rows[o_row][o_cell].row_span = r - o_row + 1;
                        rows[r][c].covered = true;
                        // A continuation is not a cell of the document: its
                        // slots belong to the cell that opened the merge, and
                        // so does a picture it states. Leaving the picture on a
                        // covered cell would drop it, since only the opening
                        // cell is written to the grid.
                        let pictures = std::mem::take(&mut rows[r][c].pictures);
                        rows[o_row][o_cell].pictures.extend(pictures);
                        chained.push((o_row, o_cell, o_col));
                    }
                }
                None => {}
            }
        }
        open = chained;
    }

    // Header rows: what the document marks, else the first.
    let marked: Vec<bool> = table.rows.iter().map(|row| row.header).collect();
    let any_marked = marked.iter().any(|flag| *flag);
    for (r, cells) in rows.iter_mut().enumerate() {
        let header = if any_marked { marked[r] } else { r == 0 };
        if header {
            for cell in cells {
                cell.header = true;
            }
        }
    }

    let num_cols = table.grid_cols.unwrap_or(0).max(widest);
    (num_cols, rows)
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

    /// A package carrying a numbering part, the document's relationships and
    /// its media: a picture bullet needs all three, since the bullet's picture
    /// is a media part named through a relationship.
    fn package_with_parts(
        document: &str,
        numbering: &str,
        rels: &str,
        media: &[(&str, &[u8])],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, text) in [
                ("word/document.xml", document),
                ("word/numbering.xml", numbering),
                ("word/_rels/document.xml.rels", rels),
            ] {
                zip.start_file(name, opts).expect("start");
                zip.write_all(text.as_bytes()).expect("write");
            }
            for (name, data) in media {
                zip.start_file(*name, opts).expect("start");
                zip.write_all(data).expect("write");
            }
            zip.finish().expect("finish");
        }
        out
    }

    /// A package whose numbering part has relationships of its own: Word names
    /// a picture bullet's image from `numbering.xml`, not from the document.
    fn package_with_numbering_rels(
        document: &str,
        numbering: &str,
        numbering_rels: &str,
        media: &[(&str, &[u8])],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, text) in [
                ("word/document.xml", document),
                ("word/numbering.xml", numbering),
                ("word/_rels/numbering.xml.rels", numbering_rels),
            ] {
                zip.start_file(name, opts).expect("start");
                zip.write_all(text.as_bytes()).expect("write");
            }
            for (name, data) in media {
                zip.start_file(*name, opts).expect("start");
                zip.write_all(data).expect("write");
            }
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

    /// A table row's cells as their plain texts, for grid assertions that
    /// should read as words rather than as structure.
    fn texts_of(cells: &[DocxCell]) -> Vec<&str> {
        cells.iter().map(|cell| cell.text.as_str()).collect()
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

    /// A list level can print a picture instead of a glyph: `w:lvlPicBulletId`
    /// names a `w:numPicBullet`, whose drawing names the media part the bullet is
    /// painted with. The bullet is the one part of a list item that is a drawing
    /// rather than words, so it is reported as the picture it is — ahead of the
    /// words it introduces, which carry no marker of their own — and when it is
    /// stated the way Word states an SVG (a raster fallback on the `a:blip`, the
    /// drawing in an `asvg:svgBlip` beside it) the drawing is what leaves the
    /// parser, for the CLI to play into pixels.
    #[test]
    fn a_picture_bullet_is_the_picture_it_prints() {
        let numbering = r##"<w:numbering xmlns:w="x" xmlns:r="z" xmlns:wp="p" xmlns:a="a"
                        xmlns:pic="q"
                        xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main">
          <w:numPicBullet w:numPicBulletId="0">
            <w:drawing><wp:inline><wp:extent cx="228600" cy="228600"/>
              <a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId7"><a:extLst>
                <a:ext uri="{96DAC541-7B7A-43D3-8B79-37D633B846F1}">
                  <asvg:svgBlip r:embed="rId8"/>
                </a:ext></a:extLst></a:blip></pic:blipFill></pic:pic></a:graphicData></a:graphic>
            </wp:inline></w:drawing>
          </w:numPicBullet>
          <w:abstractNum w:abstractNumId="0">
            <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/>
              <w:lvlPicBulletId w:val="0"/></w:lvl>
          </w:abstractNum>
          <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
        </w:numbering>"##;
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/bullet.png"/>
            <Relationship Id="rId8" Type="image" Target="media/bullet.svg"/>
          </Relationships>"#;
        let body = format!(
            "<w:body>{}{}</w:body>",
            numbered(0, 1, "First item"),
            numbered(0, 1, "Second item"),
        );
        let doc = format!("<w:document {W}>{body}</w:document>");
        let parsed = DocxDocument::parse_bytes(&package_with_parts(
            &doc,
            numbering,
            rels,
            &[
                ("word/media/bullet.png", ONE_PIXEL_PNG),
                ("word/media/bullet.svg", SVG),
            ],
        ))
        .expect("parse");

        // Each bullet sits before the words it introduces, and both are the
        // drawing rather than the raster fallback Word keeps beside it.
        assert_eq!(parsed.items.len(), 4, "{:?}", parsed.items);
        let picture = |item: &DocxItem| match item {
            DocxItem::Picture {
                mime,
                data,
                width_pt,
                height_pt,
            } => (*mime, data.clone(), *width_pt, *height_pt),
            other => panic!("expected a picture, got {other:?}"),
        };
        for index in [0, 2] {
            let (mime, data, width, height) = picture(&parsed.items[index]);
            assert_eq!(mime, "image/svg+xml", "item {index}");
            assert_eq!(
                data, SVG,
                "the raster fallback was taken instead of the drawing"
            );
            // 228600 EMU is 18 points: the frame the list draws it in.
            assert_eq!((width, height), (Some(18.0), Some(18.0)));
        }
        let words: Vec<(String, Option<String>)> = parsed.items[1..]
            .iter()
            .filter(|item| !matches!(item, DocxItem::Picture { .. }))
            .map(|item| {
                let (text, marker, _) = parts_of(item);
                (text, marker)
            })
            .collect();
        assert_eq!(
            words,
            vec![
                ("First item".to_string(), None),
                ("Second item".to_string(), None),
            ]
        );
    }

    /// The older spelling of a picture bullet: a `w:pict` whose `v:imagedata`
    /// names the image part, with the frame in the shape's `style` rather than in
    /// a box element. It is the same bullet, at the size the style states.
    #[test]
    fn a_vml_picture_bullet_keeps_the_frame_its_style_states() {
        let numbering = r#"<w:numbering xmlns:w="x" xmlns:r="z"
                        xmlns:v="urn:schemas-microsoft-com:vml">
          <w:numPicBullet w:numPicBulletId="3">
            <w:pict><v:shape style="width:10.5pt;height:7pt"><v:imagedata r:id="rId7"/></v:shape></w:pict>
          </w:numPicBullet>
          <w:abstractNum w:abstractNumId="0">
            <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/>
              <w:lvlPicBulletId w:val="3"/></w:lvl>
          </w:abstractNum>
          <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
        </w:numbering>"#;
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/bullet.png"/>
          </Relationships>"#;
        let doc = format!(
            "<w:document {W}><w:body>{}</w:body></w:document>",
            numbered(0, 1, "Only item")
        );
        let parsed = DocxDocument::parse_bytes(&package_with_parts(
            &doc,
            numbering,
            rels,
            &[("word/media/bullet.png", ONE_PIXEL_PNG)],
        ))
        .expect("parse");
        match &parsed.items[0] {
            DocxItem::Picture {
                mime,
                data,
                width_pt,
                height_pt,
            } => {
                assert_eq!(*mime, "image/png");
                assert_eq!(data.as_slice(), ONE_PIXEL_PNG);
                assert_eq!((*width_pt, *height_pt), (Some(10.5), Some(7.0)));
            }
            other => panic!("expected the bullet picture, got {other:?}"),
        }
        let (text, marker, level) = parts_of(&parsed.items[1]);
        assert_eq!((text.as_str(), marker, level), ("Only item", None, Some(1)));
    }

    /// Word names a picture bullet's image from `numbering.xml`, so the
    /// relationship is in that part's own `.rels` — the document's `rId1` is a
    /// different part or none at all, and a reader that looked there would lose
    /// the bullet. The drawing stated beside the raster is still taken.
    #[test]
    fn a_picture_bullet_reads_the_numbering_parts_relationships() {
        let numbering = r##"<w:numbering xmlns:w="x" xmlns:r="z" xmlns:wp="p" xmlns:a="a"
                        xmlns:pic="q"
                        xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main">
          <w:numPicBullet w:numPicBulletId="0">
            <w:drawing><wp:inline><wp:extent cx="114300" cy="114300"/>
              <a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId1"><a:extLst>
                <a:ext uri="{96DAC541-7B7A-43D3-8B79-37D633B846F1}">
                  <asvg:svgBlip r:embed="rId2"/>
                </a:ext></a:extLst></a:blip></pic:blipFill></pic:pic></a:graphicData></a:graphic>
            </wp:inline></w:drawing>
          </w:numPicBullet>
          <w:abstractNum w:abstractNumId="0">
            <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/>
              <w:lvlPicBulletId w:val="0"/></w:lvl>
          </w:abstractNum>
          <w:num w:numId="4"><w:abstractNumId w:val="0"/></w:num>
        </w:numbering>"##;
        let numbering_rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId1" Type="image" Target="media/bullet.png"/>
            <Relationship Id="rId2" Type="image" Target="media/bullet.svg"/>
          </Relationships>"#;
        let doc = format!(
            "<w:document {W}><w:body>{}</w:body></w:document>",
            numbered(0, 4, "This bullet uses an image")
        );
        let parsed = DocxDocument::parse_bytes(&package_with_numbering_rels(
            &doc,
            numbering,
            numbering_rels,
            &[
                ("word/media/bullet.png", ONE_PIXEL_PNG),
                ("word/media/bullet.svg", SVG),
            ],
        ))
        .expect("parse");
        assert_eq!(parsed.items.len(), 2, "{:?}", parsed.items);
        match &parsed.items[0] {
            DocxItem::Picture {
                mime,
                data,
                width_pt,
                height_pt,
            } => {
                assert_eq!(*mime, "image/svg+xml");
                assert_eq!(data.as_slice(), SVG);
                // 114300 EMU is 9 points: a bullet drawn in a 9-point square.
                assert_eq!((*width_pt, *height_pt), (Some(9.0), Some(9.0)));
            }
            other => panic!("expected the bullet's picture, got {other:?}"),
        }
        let (text, marker, level) = parts_of(&parsed.items[1]);
        assert_eq!(
            (text.as_str(), marker, level),
            ("This bullet uses an image", None, Some(1))
        );
    }

    /// A VML shape's frame is a CSS `style` string; each length carries its unit
    /// and a bare number is a pixel, as CSS reads it.
    #[test]
    fn a_vml_style_length_is_read_in_points() {
        let style = "position:absolute;width:9pt;height:1in;left:0";
        assert_eq!(vml_length(style, "width"), Some(9.0));
        assert_eq!(vml_length(style, "height"), Some(72.0));
        // 96 CSS pixels are an inch.
        assert_eq!(vml_length("width:96px", "width"), Some(72.0));
        assert_eq!(vml_length("width:12", "width"), Some(12.0));
        let cm = vml_length("width:2.54cm", "width").expect("a centimetre length");
        assert!((cm - 72.0).abs() < 1e-9, "{cm}");
        // A unit this does not know is no size at all, and so is a shape that
        // declares nothing: the picture keeps its own viewport.
        assert_eq!(vml_length("width:9em", "width"), None);
        assert_eq!(vml_length("width:-9pt", "width"), None);
        assert_eq!(vml_length("", "width"), None);
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
            DocxItem::Table { rows, num_cols } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(*num_cols, 2, "the grid is as wide as its widest row");
                assert_eq!(texts_of(&rows[0]), ["a", "b"]);
                assert_eq!(texts_of(&rows[1]), ["c", "d"]);
                // Nothing merges, so every cell is an ordinary 1×1 cell and
                // the first row is the header by fallback.
                assert!(rows[0]
                    .iter()
                    .all(|c| c.col_span == 1 && c.row_span == 1 && c.header));
                assert!(rows[1].iter().all(|c| !c.header && !c.covered));
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    /// `w:gridSpan` widens its cell and pushes the cells after it onto their
    /// real grid columns — a cell's index within its row is not its column.
    #[test]
    fn a_spanning_cell_keeps_the_cells_after_it_on_their_columns() {
        let doc = format!(
            r#"<w:document {W}><w:body><w:tbl><w:tblGrid>
                 <w:gridCol/><w:gridCol/><w:gridCol/></w:tblGrid>
                 <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc>
                       <w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr>
                             <w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr>
                 <w:tr><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc>
                       <w:tc><w:p><w:r><w:t>d</w:t></w:r></w:p></w:tc>
                       <w:tc><w:p><w:r><w:t>e</w:t></w:r></w:p></w:tc></w:tr>
               </w:tbl></w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        match &parsed.items[0] {
            DocxItem::Table { rows, num_cols } => {
                assert_eq!(*num_cols, 3, "the span counts as two columns");
                assert_eq!(rows[0][0].start_col, 0);
                assert_eq!((rows[0][1].start_col, rows[0][1].col_span), (1, 2));
                // The second row's cells sit under the grid, not under the
                // row above's cell count.
                let placed: Vec<(usize, &str)> = rows[1]
                    .iter()
                    .map(|cell| (cell.start_col, cell.text.as_str()))
                    .collect();
                assert_eq!(placed, [(0, "c"), (1, "d"), (2, "e")]);
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    /// `w:vMerge`'s restart opens a merge the cells below continue: the cell
    /// that opened it gains the rows, and a continuing cell is covered rather
    /// than a cell of its own.
    #[test]
    fn a_vertical_merge_extends_the_cell_that_opened_it() {
        let doc = format!(
            r#"<w:document {W}><w:body><w:tbl>
                 <w:tr><w:tc><w:p><w:r><w:t>top</w:t></w:r></w:p></w:tc>
                       <w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr>
                 <w:tr><w:tc><w:p><w:r><w:t>left</w:t></w:r></w:p></w:tc>
                       <w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr>
                             <w:p><w:r><w:t>open</w:t></w:r></w:p></w:tc>
                       <w:tc><w:p><w:r><w:t>y</w:t></w:r></w:p></w:tc></w:tr>
                 <w:tr><w:tc><w:p><w:r><w:t>bottom</w:t></w:r></w:p></w:tc>
                       <w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc>
                       <w:tc><w:p><w:r><w:t>z</w:t></w:r></w:p></w:tc></w:tr>
               </w:tbl></w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        match &parsed.items[0] {
            DocxItem::Table { rows, .. } => {
                let opener = &rows[1][1];
                assert_eq!(opener.text, "open");
                assert_eq!(opener.row_span, 2, "the continuation did not extend it");
                assert!(!opener.covered);
                // The continuing cell is a slot of the grid, not a cell.
                assert!(rows[2][1].covered, "{rows:?}");
                // Its neighbours are untouched by the merge.
                assert!(!rows[2][0].covered && !rows[2][2].covered);
                assert_eq!(rows[2][0].text, "bottom");
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    /// `w:tblHeader` marks its own row, and outranks the first-row fallback
    /// where it is stated; a table without the marker keeps the first row,
    /// which is the only reading it ever had.
    #[test]
    fn the_header_is_the_row_word_marks_or_else_the_first() {
        let marked = format!(
            r#"<w:document {W}><w:body><w:tbl>
                 <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc></w:tr>
                 <w:tr><w:trPr><w:tblHeader/></w:trPr>
                       <w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr>
               </w:tbl></w:body></w:document>"#
        );
        let parsed = DocxDocument::parse_bytes(&package(&marked, "", &[])).expect("parse");
        match &parsed.items[0] {
            DocxItem::Table { rows, .. } => {
                assert!(!rows[0][0].header, "the fallback spoke over the marker");
                assert!(rows[1][0].header, "the marked row is not a header");
            }
            other => panic!("expected table, got {other:?}"),
        }

        let unmarked = format!(
            r#"<w:document {W}><w:body><w:tbl>
                 <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc></w:tr>
                 <w:tr><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr>
               </w:tbl></w:body></w:document>"#
        );
        let parsed = DocxDocument::parse_bytes(&package(&unmarked, "", &[])).expect("parse");
        match &parsed.items[0] {
            DocxItem::Table { rows, .. } => {
                assert!(rows[0][0].header);
                assert!(!rows[1][0].header);
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    /// `w:gridBefore`/`w:gridAfter` are grid columns a row skips: cells do
    /// not occupy them, the grid does.
    #[test]
    fn offsets_a_row_skips_still_count_in_the_grid() {
        let doc = format!(
            r#"<w:document {W}><w:body><w:tbl>
                 <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc></w:tr>
                 <w:tr><w:trPr><w:gridBefore w:val="1"/><w:gridAfter w:val="1"/></w:trPr>
                       <w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr>
               </w:tbl></w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        match &parsed.items[0] {
            DocxItem::Table { rows, num_cols } => {
                assert_eq!(rows[1][0].start_col, 1, "the row starts one column in");
                assert_eq!(*num_cols, 3, "one before, one cell, one after");
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn renders_equations_as_latex_and_keeps_them_out_of_the_text() {
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
            DocxItem::Formula { text } => assert_eq!(text, "\\frac{a}{b}", "got {text:?}"),
            other => panic!("expected formula, got {other:?}"),
        }
    }

    /// The accents OMML names and Docling spells are all written as the command
    /// that draws them — including the ones it names `\annuity`, `\threeunderdot`
    /// and the harpoons and arrows under a letter. A mark left as a bare
    /// character took the letter under it with it, and the HTML writer could not
    /// set the equation from it.
    #[test]
    fn names_the_accents_only_docling_names() {
        let accents = [
            ("20e7", "\\annuity"),
            ("20e8", "\\threeunderdot"),
            ("20ec", "\\underrightharpoondown"),
            ("20ed", "\\underleftharpoondown"),
            ("20ee", "\\underledtarrow"),
            ("20ef", "\\underrightarrow"),
        ];
        let math: String = accents
            .iter()
            .map(|(mark, _)| {
                format!(
                    "<m:acc><m:accPr><m:chr m:val=\"&#x{mark};\"/></m:accPr>\
                     <m:e><m:r><m:t>a</m:t></m:r></m:e></m:acc>"
                )
            })
            .collect();
        let doc = format!(
            r#"<w:document {W} {M}><w:body><w:p><m:oMath>{math}</m:oMath></w:p></w:body></w:document>"#
        );
        let parsed = DocxDocument::parse_bytes(&package(&doc, "", &[])).expect("parse");
        let wanted: String = accents
            .iter()
            .map(|(_, name)| format!("{name}{{a}}"))
            .collect();
        match &parsed.items[0] {
            DocxItem::Formula { text } => assert_eq!(text, &wanted, "got {text:?}"),
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
                "formula:x^{2}",
                "text:where y^{3} is the ratio we use",
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

    /// A 1 × 1 PNG, for a package that needs a raster part and not a picture
    /// of anything in particular.
    const ONE_PIXEL_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde,
    ];

    /// Word 2016 and later states an SVG picture twice: the `a:blip`'s own
    /// `r:embed` is a PNG fallback for readers that cannot draw SVG, and the
    /// drawing itself is an `asvg:svgBlip` extension beside it. The fallback is
    /// what an old reader shows, not what the file says the picture is, so the
    /// drawing is the one that has to leave the parser.
    #[test]
    fn an_svg_picture_is_read_from_its_svg_blip_not_the_png_beside_it() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24"/></svg>"##;
        // The extension's `uri` is Word's own fixed GUID for the SVG part.
        let doc = format!(
            r##"<w:document {W} xmlns:wp="x" xmlns:a="y" xmlns:r="z"
                          xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main"><w:body>
                 <w:p><w:r><w:drawing><wp:extent cx="914400" cy="457200"/>
                   <a:blip r:embed="rId7"><a:extLst>
                     <a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}">
                       <asvg:svgBlip r:embed="rId8"/>
                     </a:ext></a:extLst></a:blip>
                 </w:drawing></w:r></w:p>
               </w:body></w:document>"##
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
            <Relationship Id="rId8" Type="image" Target="media/image1.svg"/>
          </Relationships>"#;
        let pkg = package(
            &doc,
            rels,
            &[
                ("word/media/image1.png", ONE_PIXEL_PNG),
                ("word/media/image1.svg", svg),
            ],
        );
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let pictures: Vec<(&str, &[u8])> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Picture { mime, data, .. } => Some((*mime, data.as_slice())),
                _ => None,
            })
            .collect();
        assert_eq!(
            pictures.len(),
            1,
            "the drawing is one picture: {pictures:?}"
        );
        assert_eq!(pictures[0].0, "image/svg+xml");
        assert_eq!(pictures[0].1, svg, "the fallback PNG was taken instead");
    }

    /// A `svgBlip` that does not name a drawing is not a drawing. Word's
    /// fallback exists for exactly this reader — one that cannot show the SVG —
    /// so a broken or misplaced reference leaves the PNG as the picture rather
    /// than leaving the picture out.
    #[test]
    fn an_svg_blip_naming_a_raster_leaves_the_fallback_as_the_picture() {
        let doc = format!(
            r##"<w:document {W} xmlns:wp="x" xmlns:a="y" xmlns:r="z"
                          xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main"><w:body>
                 <w:p><w:r><w:drawing><wp:extent cx="914400" cy="457200"/>
                   <a:blip r:embed="rId7">
                     <a:extLst><a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}">
                       <asvg:svgBlip r:embed="rId9"/>
                     </a:ext></a:extLst>
                   </a:blip>
                 </w:drawing></w:r></w:p>
               </w:body></w:document>"##
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
            <Relationship Id="rId9" Type="image" Target="media/image2.png"/>
          </Relationships>"#;
        let pkg = package(
            &doc,
            rels,
            &[
                ("word/media/image1.png", ONE_PIXEL_PNG),
                ("word/media/image2.png", b"not the picture"),
            ],
        );
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let (mime, data) = parsed
            .items
            .iter()
            .find_map(|item| match item {
                DocxItem::Picture { mime, data, .. } => Some((*mime, data.as_slice())),
                _ => None,
            })
            .expect("a picture");
        assert_eq!(mime, "image/png");
        assert_eq!(data, ONE_PIXEL_PNG);
    }

    /// A shape can be filled with a picture rather than a colour, and the fill
    /// states that picture the way a picture states itself: the raster on the
    /// `a:blip`, the drawing in an `asvg:svgBlip` extension beside it.
    ///
    /// The fill is part of the shape, so the shape is painted whole: the
    /// drawing is drawn through the shape's geometry — inside the rounded
    /// rectangle, not as the rectangle's bounding picture — and the shape's
    /// words are still the document's text.
    #[test]
    fn a_shape_filled_with_a_picture_is_painted_through_the_shape() {
        let doc = format!(
            r##"<w:document {W} xmlns:wp="x" xmlns:a="y" xmlns:r="z" xmlns:wps="x"
                          xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main"><w:body>
                 <w:p><w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="457200"/>
                   <a:graphic><a:graphicData><wps:wsp><wps:spPr>
                     <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
                     <a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom>
                     <a:blipFill><a:blip r:embed="rId7"><a:extLst>
                       <a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}">
                         <asvg:svgBlip r:embed="rId8"/>
                       </a:ext></a:extLst></a:blip><a:stretch><a:fillRect/></a:stretch></a:blipFill>
                   </wps:spPr>
                     <wps:txbx><w:txbxContent><w:p><w:r><w:t>Inside the fill.</w:t></w:r></w:p></w:txbxContent></wps:txbx>
                     <wps:bodyPr/>
                   </wps:wsp></a:graphicData></a:graphic>
                 </wp:inline></w:drawing></w:r></w:p>
               </w:body></w:document>"##
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
            <Relationship Id="rId8" Type="image" Target="media/diagram.svg"/>
          </Relationships>"#;
        // A drawing that paints a box: the fill's picture, and not the raster
        // fallback beside it, which is a single grey pixel.
        let drawing = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24"><rect width="40" height="24" fill="#ff0000"/></svg>"##;
        let parsed = DocxDocument::parse_bytes(&package(
            &doc,
            rels,
            &[
                ("word/media/image1.png", ONE_PIXEL_PNG),
                ("word/media/diagram.svg", drawing),
            ],
        ))
        .expect("parse");
        let pictures: Vec<&DocxItem> = parsed
            .items
            .iter()
            .filter(|item| matches!(item, DocxItem::Picture { .. }))
            .collect();
        assert_eq!(pictures.len(), 1, "{pictures:?}");
        let DocxItem::Picture {
            mime,
            data,
            width_pt,
            height_pt,
        } = pictures[0]
        else {
            unreachable!("filtered to pictures")
        };
        // A painted shape has no source encoding: it is the PNG it was drawn
        // into, at the shape's own 72 × 36 points.
        assert_eq!(*mime, "image/png");
        assert_eq!((*width_pt, *height_pt), (Some(72.0), Some(36.0)));
        let painted = image::load_from_memory(data)
            .expect("the painted shape decodes")
            .to_rgba8();
        assert_eq!(painted.dimensions(), (144, 72));
        assert_eq!(
            painted.get_pixel(72, 36).0,
            [255, 0, 0, 255],
            "the fill's drawing was not painted into the shape"
        );
        // The shape is a rounded rectangle, and the fill does not reach its
        // corner: the picture was drawn through the geometry, not as a box.
        assert_eq!(
            painted.get_pixel(1, 1).0,
            [255, 255, 255, 255],
            "the fill was laid down as a box instead of through the shape"
        );

        let texts: Vec<&str> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Inside the fill."]);
    }

    /// The fill a shape states with no drawing is the raster it names, painted
    /// into the shape at the size the document places it: not every filled shape
    /// is an SVG, and the raster path is the one most files take.
    #[test]
    fn a_shape_filled_with_a_raster_is_painted_at_the_shapes_size() {
        let doc = format!(
            r#"<w:document {W} xmlns:wp="x" xmlns:a="y" xmlns:r="z" xmlns:wps="x"><w:body>
                 <w:p><w:r><w:drawing><wp:inline><wp:extent cx="762000" cy="457200"/>
                   <a:graphic><a:graphicData><wps:wsp><wps:spPr>
                     <a:xfrm><a:off x="0" y="0"/><a:ext cx="762000" cy="457200"/></a:xfrm>
                     <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                     <a:blipFill><a:blip r:embed="rId7"/><a:stretch><a:fillRect/></a:stretch></a:blipFill>
                   </wps:spPr><wps:bodyPr/></wps:wsp>
                 </a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
               </w:body></w:document>"#
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
          </Relationships>"#;
        let orange = image::RgbaImage::from_pixel(30, 18, image::Rgba([224, 140, 40, 255]));
        let mut png = Vec::new();
        orange
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("the fill encodes");
        let parsed =
            DocxDocument::parse_bytes(&package(&doc, rels, &[("word/media/image1.png", &png)]))
                .expect("parse");
        let (mime, data, width, height) = parsed
            .items
            .iter()
            .find_map(|item| match item {
                DocxItem::Picture {
                    mime,
                    data,
                    width_pt,
                    height_pt,
                } => Some((*mime, data.as_slice(), *width_pt, *height_pt)),
                _ => None,
            })
            .expect("a picture");
        assert_eq!(mime, "image/png");
        assert_eq!((width, height), (Some(60.0), Some(36.0)));
        let painted = image::load_from_memory(data)
            .expect("the painted shape decodes")
            .to_rgba8();
        // 60 × 36 points at 144 dpi, whatever the raster's own grid was.
        assert_eq!(painted.dimensions(), (120, 72));
        assert_eq!(
            painted.get_pixel(60, 36).0,
            [224, 140, 40, 255],
            "the fill's own pixels were not painted into the shape"
        );
    }

    /// A picture the camera stored sideways, with nothing but its EXIF tag
    /// saying so. Word displays it upright; the parser has to hand on the
    /// picture Word showed, not the pixels the package holds.
    #[test]
    fn an_exif_tagged_picture_is_turned_upright() {
        let sideways = std::fs::read("../../test-data/photos/06_rotated.jpg")
            .expect("the photographed corpus is generated");
        assert!(
            sideways.windows(6).any(|window| window == b"Exif\0\0"),
            "the fixture must carry the tag, or the turn is not what is tested"
        );
        let doc = format!(
            r#"<w:document {W} xmlns:wp="x" xmlns:a="y"><w:body><w:p>
                 <w:r><w:drawing><wp:extent cx="3200400" cy="4526280"/>
                   <a:blip r:embed="rId7"/></w:drawing></w:r>
               </w:p></w:body></w:document>"#
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.jpg"/>
          </Relationships>"#;
        let pkg = package(&doc, rels, &[("word/media/image1.jpg", &sideways)]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let (mime, data) = parsed
            .items
            .iter()
            .find_map(|i| match i {
                DocxItem::Picture { mime, data, .. } => Some((*mime, data)),
                _ => None,
            })
            .expect("a picture");

        assert_eq!(mime, "image/jpeg", "a JPEG stays a JPEG");
        assert!(!data.windows(6).any(|window| window == b"Exif\0\0"));
        let decoded = image::load_from_memory(data).expect("the turned picture decodes");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (1240, 1754),
            "the stored 1754 x 1240 grid is turned"
        );
    }

    /// Every picture the extractor carries is named out of a Word package by
    /// the shared list, in both of its names and in any case: the table is the
    /// one place a media type is decided, so a format added there is read out
    /// of a package without another edit here.
    #[test]
    fn every_carried_picture_format_is_named_from_a_word_part() {
        for format in pdfextractor_picture_formats::PICTURE_FORMATS {
            let names = std::iter::once(format.extension).chain(format.read_as.iter().copied());
            for extension in names {
                let part = format!("word/media/image1.{extension}");
                assert_eq!(mime_for_part(&part), Some(format.mime), "{part}");
                let shouted = format!("word/media/image1.{}", extension.to_ascii_uppercase());
                assert_eq!(mime_for_part(&shouted), Some(format.mime), "{shouted}");
            }
        }
        // A part the list does not carry names nothing, and neither does one
        // with no extension to name it by.
        assert_eq!(mime_for_part("word/media/image1.avif"), None);
        assert_eq!(mime_for_part("word/media/image1"), None);
        assert_eq!(mime_for_part("image1.png"), Some("image/png"));
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

    /// An older Word file states its shapes in VML, with no image part behind
    /// them: Word paints the shape, so the reader has to as well or the drawing
    /// is nothing in the document. A WordArt is the same, its string living
    /// only in `v:textpath`.
    #[test]
    fn a_vml_shape_and_a_wordart_become_pictures() {
        let doc = format!(
            r##"<w:document {W} {V} xmlns:o="urn:schemas-microsoft-com:office:office"><w:body>
                 <w:p><w:r><w:t>Before </w:t></w:r>
                   <w:r><w:pict><v:rect style="width:40pt;height:20pt"
                                        fillcolor="#ff0000" stroked="f"/></w:pict></w:r>
                   <w:r><w:pict><v:shape style="width:120pt;height:30pt"
                                          fillcolor="#c00000" stroked="f">
                     <v:textpath string="Caption" style="font-size:12pt"/>
                   </v:shape></w:pict></w:r>
                   <w:r><w:t>After</w:t></w:r>
                 </w:p></w:body></w:document>"##
        );
        let parsed = DocxDocument::parse_bytes(&package(&doc, "", &[])).expect("parse");
        let pictures: Vec<(&str, Option<f64>, Option<f64>, usize)> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Picture {
                    mime,
                    data,
                    width_pt,
                    height_pt,
                } => Some((*mime, *width_pt, *height_pt, data.len())),
                _ => None,
            })
            .collect();
        assert_eq!(pictures.len(), 2, "{pictures:?}");
        assert_eq!(pictures[0].0, "image/png");
        assert_eq!((pictures[0].1, pictures[0].2), (Some(40.0), Some(20.0)));
        assert_eq!(pictures[1].0, "image/png");
        assert_eq!((pictures[1].1, pictures[1].2), (Some(120.0), Some(30.0)));
        for (_, _, _, length) in &pictures {
            assert!(*length > 100, "a drawn picture has bytes: {pictures:?}");
        }

        let texts: Vec<&str> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // The words around the drawings are the paragraph's, and a picture does
        // not split a DOCX paragraph the way it splits a flow-format page.
        assert_eq!(texts, vec!["Before After"]);
    }

    /// A VML picture is still a picture when the image is an image part: the
    /// `v:imagedata` relationship is the bytes, and nothing is drawn over it.
    #[test]
    fn a_vml_picture_with_an_image_part_is_that_image() {
        let doc = format!(
            r#"<w:document {W} {V} xmlns:r="x"><w:body><w:p><w:r><w:pict>
                 <v:shape style="width:40pt;height:20pt"><v:imagedata r:id="rId7"/></v:shape>
               </w:pict></w:r></w:p></w:body></w:document>"#
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
          </Relationships>"#;
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde,
        ];
        let parsed =
            DocxDocument::parse_bytes(&package(&doc, rels, &[("word/media/image1.png", png)]))
                .expect("parse");
        let (mime, data) = parsed
            .items
            .iter()
            .find_map(|item| match item {
                DocxItem::Picture { mime, data, .. } => Some((*mime, data)),
                _ => None,
            })
            .expect("a picture");
        assert_eq!(mime, "image/png");
        assert_eq!(data, png, "the part's own bytes, not a drawing of them");
    }

    /// The bytes of a 1 × 1 PNG, for the pictures these tests resolve
    /// relationships to.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde,
    ];
    const SVG: &[u8] =
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24"/></svg>"##;

    /// A picture inside a table cell is the cell's picture. It is kept on the
    /// cell it was read in rather than left in the body beside the table, so a
    /// reader that needs to know which cell showed a picture can ask the cell,
    /// and the body's order is the document's own: the words, the table, the
    /// words. A cell that held nothing but the picture is still that cell.
    #[test]
    fn a_picture_in_a_table_cell_stays_in_its_cell() {
        let doc = format!(
            r#"<w:document {W} {V} xmlns:r="x"><w:body>
                 <w:p><w:r><w:t>Before.</w:t></w:r></w:p>
                 <w:tbl><w:tr><w:tc><w:p><w:r><w:pict>
                   <v:shape style="width:40pt;height:24pt"><v:imagedata r:id="rId7"/></v:shape>
                 </w:pict></w:r></w:p></w:tc></w:tr></w:tbl>
                 <w:p><w:r><w:t>After.</w:t></w:r></w:p>
               </w:body></w:document>"#
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
          </Relationships>"#;
        let parsed =
            DocxDocument::parse_bytes(&package(&doc, rels, &[("word/media/image1.png", PNG)]))
                .expect("parse");
        let items: Vec<&str> = parsed
            .items
            .iter()
            .map(|item| match item {
                DocxItem::Text { text, .. } => text.as_str(),
                DocxItem::Table { .. } => "table",
                DocxItem::Picture { mime, .. } => mime,
                DocxItem::Formula { .. } => "formula",
            })
            .collect();
        assert_eq!(items, vec!["Before.", "table", "After."]);

        let DocxItem::Table { rows, num_cols } = &parsed.items[1] else {
            panic!("the table is not second: {items:?}");
        };
        assert_eq!(*num_cols, 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 1);
        let cell = &rows[0][0];
        assert_eq!(cell.text, "", "a picture is not words");
        assert_eq!(cell.pictures.len(), 1, "the cell's picture");
        assert_eq!(cell.pictures[0].mime, "image/png");
        assert_eq!(
            cell.pictures[0].data, PNG,
            "the part's own bytes, not a drawing of them"
        );
    }

    /// A merged cell is one cell, however many rows state it: the row that
    /// opened the merge holds the rows, and a picture a continuation states is
    /// inside that cell. It travels to the cell that is written to the grid —
    /// the continuation itself is covered, not a cell of the document — so a
    /// picture the file states is not lost by being stated in the second row of
    /// a merge.
    #[test]
    fn a_picture_in_a_merged_cell_belongs_to_the_cell_that_opens_it() {
        let doc = format!(
            r#"<w:document {W} {V} xmlns:r="x"><w:body>
                 <w:tbl>
                   <w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p/></w:tc></w:tr>
                   <w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p><w:r><w:pict>
                     <v:shape style="width:40pt;height:24pt"><v:imagedata r:id="rId7"/></v:shape>
                   </w:pict></w:r></w:p></w:tc></w:tr>
                 </w:tbl>
               </w:body></w:document>"#
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
          </Relationships>"#;
        let parsed =
            DocxDocument::parse_bytes(&package(&doc, rels, &[("word/media/image1.png", PNG)]))
                .expect("parse");
        let DocxItem::Table { rows, .. } = &parsed.items[0] else {
            panic!("the picture is not in a table: {:?}", parsed.items);
        };
        assert_eq!(rows.len(), 2);
        let opening = &rows[0][0];
        assert_eq!(opening.row_span, 2, "the merge is one cell");
        assert!(!opening.covered, "the cell that opened the merge stands");
        assert_eq!(opening.pictures.len(), 1, "the merged cell's picture");
        assert_eq!(opening.pictures[0].data, PNG);
        assert!(rows[1][0].covered, "the second row continues the merge");
        assert!(
            rows[1][0].pictures.is_empty(),
            "the picture is not also left on a cell that is not written"
        );
    }

    /// A picture stated twice the way a converter keeps an older reader happy:
    /// the drawing in `mc:Choice`, and the raster an old reader shows in
    /// `mc:Fallback` as a VML `v:imagedata`. One of them is the document's
    /// picture, not two, and the drawing is the one the file asks for.
    #[test]
    fn a_picture_in_an_alternate_content_choice_is_read_once() {
        let doc = format!(
            r#"<w:document {W} {MC} {V} xmlns:r="x"><w:body><w:p><w:r>
                 <mc:AlternateContent>
                   <mc:Choice Requires="wps"><w:drawing><wp:extent cx="508000" cy="304800"/>
                     <a:blip r:embed="rId8"/></w:drawing></mc:Choice>
                   <mc:Fallback><w:pict><v:shape style="width:40pt;height:24pt">
                     <v:imagedata r:id="rId7"/></v:shape></w:pict></mc:Fallback>
                 </mc:AlternateContent>
               </w:r></w:p></w:body></w:document>"#
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
            <Relationship Id="rId8" Type="image" Target="media/diagram.svg"/>
          </Relationships>"#;
        let parsed = DocxDocument::parse_bytes(&package(
            &doc,
            rels,
            &[
                ("word/media/image1.png", PNG),
                ("word/media/diagram.svg", SVG),
            ],
        ))
        .expect("parse");
        let pictures: Vec<&DocxItem> = parsed
            .items
            .iter()
            .filter(|item| matches!(item, DocxItem::Picture { .. }))
            .collect();
        assert_eq!(
            pictures.len(),
            1,
            "the fallback beside the choice is not a second picture: {pictures:?}"
        );
        let DocxItem::Picture { mime, data, .. } = pictures[0] else {
            unreachable!("filtered to pictures")
        };
        assert_eq!(*mime, "image/svg+xml");
        assert_eq!(
            data.as_slice(),
            SVG,
            "the raster fallback was taken instead of the drawing"
        );
    }

    /// The fallback is what it says it is: when the choice names a part the
    /// package does not hold, the copy an older reader shows is the picture the
    /// document means — and it is not lost to the choice's empty promise.
    #[test]
    fn a_picture_falls_back_when_its_choice_names_no_part() {
        let doc = format!(
            r#"<w:document {W} {MC} {V} xmlns:r="x"><w:body><w:p><w:r>
                 <mc:AlternateContent>
                   <mc:Choice Requires="wps"><w:drawing><wp:extent cx="508000" cy="304800"/>
                     <a:blip r:embed="rId8"/></w:drawing></mc:Choice>
                   <mc:Fallback><w:pict><v:shape style="width:40pt;height:24pt">
                     <v:imagedata r:id="rId7"/></v:shape></w:pict></mc:Fallback>
                 </mc:AlternateContent>
               </w:r></w:p></w:body></w:document>"#
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
            <Relationship Id="rId8" Type="image" Target="media/gone.svg"/>
          </Relationships>"#;
        let parsed =
            DocxDocument::parse_bytes(&package(&doc, rels, &[("word/media/image1.png", PNG)]))
                .expect("parse");
        let pictures: Vec<&DocxItem> = parsed
            .items
            .iter()
            .filter(|item| matches!(item, DocxItem::Picture { .. }))
            .collect();
        assert_eq!(pictures.len(), 1, "{pictures:?}");
        let DocxItem::Picture { mime, data, .. } = pictures[0] else {
            unreachable!("filtered to pictures")
        };
        assert_eq!(*mime, "image/png");
        assert_eq!(data.as_slice(), PNG, "the fallback is the picture here");
    }

    /// The VML copy beside a DrawingML shape is read only when the DrawingML
    /// gave no picture: reading it always would draw a second copy of every
    /// shape, and never reading it leaves an older shape as nothing at all.
    #[test]
    fn a_fallback_vml_copy_is_drawn_when_the_choice_has_no_picture() {
        let doc = format!(
            r##"<w:document {W} {MC} {V} xmlns:wp="x" xmlns:a="y" xmlns:wps="x"
                          xmlns:r="x" xmlns:o="urn:schemas-microsoft-com:office:office"><w:body>
                 <w:p><w:r><mc:AlternateContent>
                   <mc:Choice Requires="wps"><w:drawing><wps:txbx><w:txbxContent>
                     <w:p><w:r><w:t>Shape text</w:t></w:r></w:p></w:txbxContent></wps:txbx></w:drawing></mc:Choice>
                   <mc:Fallback><w:pict><v:rect style="width:30pt;height:15pt"
                                               fillcolor="#00aa00" stroked="f"/></w:pict></mc:Fallback>
                 </mc:AlternateContent></w:r></w:p>
                 <w:p><w:r><mc:AlternateContent>
                   <mc:Choice Requires="wps"><w:drawing><wp:extent cx="914400" cy="457200"/>
                     <a:blip r:embed="rId7"/></w:drawing></mc:Choice>
                   <mc:Fallback><w:pict><v:rect style="width:50pt;height:25pt"
                                               fillcolor="#0000ff" stroked="f"/></w:pict></mc:Fallback>
                 </mc:AlternateContent></w:r></w:p>
               </w:body></w:document>"##
        );
        let rels = r#"<Relationships xmlns="z">
            <Relationship Id="rId7" Type="image" Target="media/image1.png"/>
          </Relationships>"#;
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde,
        ];
        let parsed =
            DocxDocument::parse_bytes(&package(&doc, rels, &[("word/media/image1.png", png)]))
                .expect("parse");
        let pictures: Vec<(Option<f64>, Option<f64>, usize)> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Picture {
                    data,
                    width_pt,
                    height_pt,
                    ..
                } => Some((*width_pt, *height_pt, data.len())),
                _ => None,
            })
            .collect();
        assert_eq!(pictures.len(), 2, "{pictures:?}");
        // The shape the choice could not draw is painted from its VML copy.
        assert_eq!((pictures[0].0, pictures[0].1), (Some(30.0), Some(15.0)));
        // The picture the choice did give is its own bytes, at its own size: the
        // VML copy beside it adds nothing.
        assert_eq!((pictures[1].0, pictures[1].1), (Some(72.0), Some(36.0)));
        assert_eq!(pictures[1].2, png.len());

        // And the fallback's words are not read a second time.
        let texts: Vec<&str> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Shape text"]);
    }

    /// A modern Word file states a shape in DrawingML only: a `wps:wsp` with a
    /// preset outline and its own `wps:spPr`, and no image part behind it. Word
    /// paints it from the markup, so a reader that only looked for an image
    /// relationship left the shape out of the document altogether.
    #[test]
    fn a_drawingml_shape_becomes_a_picture() {
        let doc = format!(
            r##"<w:document {W} xmlns:wp="x" xmlns:a="y" xmlns:wps="x"><w:body>
                 <w:p><w:r><w:t>Before </w:t></w:r>
                   <w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="457200"/>
                     <a:graphic><a:graphicData><wps:wsp><wps:spPr>
                       <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
                       <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                       <a:solidFill><a:srgbClr val="ff0000"/></a:solidFill>
                       <a:ln><a:noFill/></a:ln>
                     </wps:spPr><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic>
                   </wp:inline></w:drawing></w:r>
                   <w:r><w:t>After</w:t></w:r>
                 </w:p></w:body></w:document>"##
        );
        let parsed = DocxDocument::parse_bytes(&package(&doc, "", &[])).expect("parse");
        let pictures: Vec<(&str, Option<f64>, Option<f64>, usize)> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Picture {
                    mime,
                    data,
                    width_pt,
                    height_pt,
                } => Some((*mime, *width_pt, *height_pt, data.len())),
                _ => None,
            })
            .collect();
        assert_eq!(pictures.len(), 1, "{pictures:?}");
        assert_eq!(pictures[0].0, "image/png");
        assert_eq!((pictures[0].1, pictures[0].2), (Some(72.0), Some(36.0)));
        assert!(pictures[0].3 > 100, "a drawn shape has bytes");

        let texts: Vec<&str> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Before After"]);
    }

    /// A shape Word writes twice is still one picture: the DrawingML copy
    /// paints, so the VML copy in `mc:Fallback` must not be drawn beside it.
    #[test]
    fn a_painted_drawingml_shape_is_not_drawn_again_from_its_vml_copy() {
        let doc = format!(
            r##"<w:document {W} {MC} {V} xmlns:wp="x" xmlns:a="y" xmlns:wps="x"
                          xmlns:o="urn:schemas-microsoft-com:office:office"><w:body>
                 <w:p><w:r><mc:AlternateContent>
                   <mc:Choice Requires="wps"><w:drawing><wp:inline><wp:extent cx="914400" cy="457200"/>
                     <a:graphic><a:graphicData><wps:wsp><wps:spPr>
                       <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
                       <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                       <a:solidFill><a:srgbClr val="ff0000"/></a:solidFill>
                       <a:ln><a:noFill/></a:ln>
                     </wps:spPr><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic>
                   </wp:inline></w:drawing></mc:Choice>
                   <mc:Fallback><w:pict><v:rect style="width:30pt;height:15pt"
                                               fillcolor="#00aa00" stroked="f"/></w:pict></mc:Fallback>
                 </mc:AlternateContent></w:r></w:p>
               </w:body></w:document>"##
        );
        let parsed = DocxDocument::parse_bytes(&package(&doc, "", &[])).expect("parse");
        let pictures: Vec<(Option<f64>, Option<f64>)> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Picture {
                    width_pt,
                    height_pt,
                    ..
                } => Some((*width_pt, *height_pt)),
                _ => None,
            })
            .collect();
        assert_eq!(pictures, vec![(Some(72.0), Some(36.0))]);
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

    #[test]
    fn a_hyperlink_splits_its_sentence_and_carries_the_url() {
        // The split is the reference reading: Docling's own DOCX conversion
        // of calibre_demo.docx emits "Visit the …", the link's own words with
        // the URL, and ". Then we have …" as three separate text items.
        let doc = format!(
            r#"<w:document {W}><w:body><w:p>
                 <w:r><w:t>Visit the </w:t></w:r>
                 <w:hyperlink r:id="rId3"><w:r><w:t>calibre download page</w:t></w:r></w:hyperlink>
                 <w:r><w:t> today.</w:t></w:r>
               </w:p></w:body></w:document>"#
        );
        let rels = r#"<Relationships>
            <Relationship Id="rId3"
                Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink"
                Target="http://calibre-ebook.com/download" TargetMode="External"/>
          </Relationships>"#;
        let pkg = package(&doc, rels, &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let texts: Vec<(&str, Option<&str>)> = parsed
            .items
            .iter()
            .filter_map(|item| match item {
                DocxItem::Text {
                    text, hyperlink, ..
                } => Some((text.as_str(), hyperlink.as_deref())),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                ("Visit the", None),
                (
                    "calibre download page",
                    Some("http://calibre-ebook.com/download"),
                ),
                ("today.", None),
            ]
        );
    }

    #[test]
    fn an_internal_hyperlink_names_no_url() {
        // `w:hyperlink w:anchor` jumps inside the document and names no URL:
        // its words stay in the sentence around them — where Docling's own
        // DOCX reading leaves them too (calibre's back-links split nothing).
        let doc = format!(
            r#"<w:document {W}><w:body><w:p>
                 <w:r><w:t>See </w:t></w:r>
                 <w:hyperlink w:anchor="sec1"><w:r><w:t>the section on formatting</w:t></w:r></w:hyperlink>
                 <w:r><w:t>.</w:t></w:r>
               </w:p></w:body></w:document>"#
        );
        let pkg = package(&doc, "", &[]);
        let parsed = DocxDocument::parse_bytes(&pkg).expect("parse");
        let texts: Vec<&DocxItem> = parsed
            .items
            .iter()
            .filter(|i| matches!(i, DocxItem::Text { .. }))
            .collect();
        assert_eq!(texts.len(), 1, "{texts:?}");
        match texts[0] {
            DocxItem::Text {
                text, hyperlink, ..
            } => {
                assert_eq!(text, "See the section on formatting.");
                assert!(hyperlink.is_none(), "{hyperlink:?}");
            }
            other => panic!("expected text, got {other:?}"),
        }
    }
}
