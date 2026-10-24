//! Reading a PPTX presentation into its slides and the shapes on them.
//!
//! PresentationML is the most deeply nested part of OOXML: a slide holds a
//! shape tree, a shape holds a text body, a paragraph holds runs, a graphic
//! frame holds a table whose cells hold text bodies of their own. A streaming
//! reader would spend most of its state on which depth it is at, so the small
//! parts are read into a tree first ([`Node`]) and the tree is walked with the
//! structure's names in hand. Slides are a few kilobytes each; the size is not
//! a concern, and the shape of the code follows the shape of the format.
//!
//! Everything is reached the way the package describes it. The presentation's
//! relationships name the slides, a slide's relationships name its layout and
//! its pictures, the layout's name the master. A picture is the media part its
//! `r:embed` points at — not `ppt/media/image1.png` guessed from a name.

use anyhow::{bail, Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::{Cursor, Read};
// A shape the slide draws itself — an autoshape, a connector, a group, a text
// box with a fill — has no image part behind it. The painter that draws a Word
// shape draws it too: the markup is the same DrawingML under either prefix.
use pdfextractor_drawingml::{
    paint::Node as DrawingNode, render_node_with, Media, Picture as PaintedPicture, Theme,
};

/// Slide size when `p:sldSz` is absent: 10 × 7.5 inches, PowerPoint's 4:3
/// default, in English Metric Units.
const DEFAULT_WIDTH: f64 = 9_144_000.0;
const DEFAULT_HEIGHT: f64 = 6_858_000.0;

/// Two shapes whose top edges are this close are on one visual line: 0.05 inch,
/// the tolerance Docling's own PPTX backend uses. PowerPoint stores shapes in
/// creation order, which is not the order a reader meets them.
const ROW_TOLERANCE: i64 = 45_720;

/// A parsed presentation.
#[derive(Debug, Clone)]
pub struct PptxDocument {
    /// Slide width and height in EMU, from `p:sldSz`.
    pub width: f64,
    pub height: f64,
    /// The slides in presentation order; a slide is a page.
    pub slides: Vec<Slide>,
}

/// One slide: its page number, whether it is hidden, its shapes in reading
/// order, the picture it states as its background, and what its speaker notes
/// and comments say.
#[derive(Debug, Clone)]
pub struct Slide {
    /// 1-based, the page number Docling gives the slide.
    pub number: i32,
    /// `p:sld/@show="0"` — PowerPoint's “Hide Slide”. The slide is kept (it is
    /// still in the file) and handed to the `invisible` content layer.
    pub hidden: bool,
    pub shapes: Vec<Shape>,
    /// The slide's speaker notes, when its notes part holds any: the text of
    /// the notes body placeholder, read the way python-pptx reads it — the
    /// paragraphs joined by a newline, a soft line break a vertical tab, the
    /// whole thing trimmed. `None` is a slide with no notes part, or one whose
    /// notes say nothing.
    pub notes: Option<String>,
    /// The slide's comments, in the order its comments part lists them.
    pub comments: Vec<Comment>,
    /// The picture the slide states as its background, when it states one:
    /// `p:cSld/p:bg/p:bgPr/a:blipFill`. It is not a shape in the shape tree
    /// and not a picture item; it is the page's own image, at the full slide
    /// box.
    pub background: Option<PictureShape>,
}

/// One comment attached to a slide: what it says, and who said it when the
/// deck's authors part names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// `p:cm/@idx` — the id PowerPoint gives the comment. Docling falls back to
    /// the slide's 0-based index when the attribute is missing, and so does
    /// this, so the field is always the id to use.
    pub index: String,
    /// `p:cm/@dt`, as the file writes it: a timestamp, not a parsed date (the
    /// two readings agree because neither interprets it).
    pub date: Option<String>,
    /// The comment's author, when the deck's comment-authors part names the
    /// `authorId` and gives a name. Never empty: an empty name is no name.
    pub author: Option<String>,
    /// That author's initials, as written; empty is no initials.
    pub initials: Option<String>,
    /// The comment's words, trimmed. A comment with no words is not read at
    /// all — an empty note is not content, which is how Docling reads it too.
    pub text: String,
}

/// One shape of a slide.
#[derive(Debug, Clone)]
pub enum Shape {
    Text(TextShape),
    Table(TableShape),
    Picture(PictureShape),
    Chart(ChartShape),
}

/// A shape with a text body: a title, a body placeholder, a text box.
#[derive(Debug, Clone)]
pub struct TextShape {
    /// The shape's placement, `[l, t, r, b]` in EMU from the slide's top-left.
    pub bbox: [f64; 4],
    /// Whether the shape is a title placeholder (`title` or `ctrTitle`), which
    /// is the one thing that makes its paragraphs titles rather than text.
    pub title: bool,
    pub paragraphs: Vec<Paragraph>,
}

/// One `a:p` of a text body.
#[derive(Debug, Clone)]
pub struct Paragraph {
    /// The formatting runs the paragraph is made of, in order. A line break is
    /// not a run: it is a space in [`Paragraph::text`], the way a reader sees
    /// it and the way Docling joins the line.
    pub runs: Vec<Run>,
    /// The paragraph as it reads: every run's text, one space per `a:br`.
    pub text: String,
    /// `a:pPr/@lvl`, 0-based.
    pub level: usize,
    /// The bullet or number the paragraph is drawn with, when it is a list
    /// item. `None` is body text.
    pub list: Option<ListMarker>,
}

/// One `a:r` or `a:fld` of a paragraph.
#[derive(Debug, Clone)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// `a:rPr/@sz`, in points (the part stores hundredths of one).
    pub size_pt: Option<f64>,
}

/// What a list item prints before its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListMarker {
    /// `"1."` for a numbered item; empty for a bullet. The bullet character is
    /// the shape's style, and Docling keeps only the empty marker, so the same
    /// fact is written here rather than the glyph.
    pub marker: String,
    pub enumerated: bool,
}

/// A `p:graphicFrame` holding a table.
#[derive(Debug, Clone)]
pub struct TableShape {
    pub bbox: [f64; 4],
    pub table: Table,
}

/// A table as its grid: its span of rows and columns and the cells that are
/// not covered by a merge.
#[derive(Debug, Clone)]
pub struct Table {
    pub num_rows: usize,
    pub num_cols: usize,
    pub cells: Vec<TableCell>,
}

/// One cell of a table. A cell a horizontal or vertical merge covers is not
/// here: the cell that opened the merge holds its span instead.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub text: String,
}

/// A `p:pic` and the media part its relationship points at.
#[derive(Debug, Clone)]
pub struct PictureShape {
    pub bbox: [f64; 4],
    /// Named from the media part's extension, which is what the package knows.
    pub mime: &'static str,
    pub data: Vec<u8>,
}

/// A deck's chart is read by the reader a workbook's charts go through: the
/// part is the same DrawingML whatever anchored it — a slide's `p:graphicFrame`
/// or a worksheet's `xdr:graphicFrame` — so the data a chart plots is read once
/// and placed by the format that holds it. Re-exported here because this is
/// where a deck's callers look for it.
pub use pdfextractor_chart::{chart_cell_text, ChartKind, ChartSeries, ChartShape};

impl PptxDocument {
    /// Read a whole `.pptx` package from memory.
    pub fn parse_bytes(data: &[u8]) -> Result<Self> {
        let mut archive = zip::ZipArchive::new(Cursor::new(data))
            .map_err(|e| anyhow::anyhow!("not a PPTX (ZIP) package: {e}"))?;

        let presentation = read_part(&mut archive, "ppt/presentation.xml")
            .context("the package has no ppt/presentation.xml")?;
        let root = Node::parse(&presentation).context("malformed ppt/presentation.xml")?;

        let slide_size = root.find("sldSz");
        let width = slide_size
            .and_then(|size| size.attr("cx"))
            .and_then(number)
            .unwrap_or(DEFAULT_WIDTH);
        let height = slide_size
            .and_then(|size| size.attr("cy"))
            .and_then(number)
            .unwrap_or(DEFAULT_HEIGHT);

        // The slides' parts are named by the presentation's relationships. If
        // there are none (a hand-written package), the conventional part names
        // are still there to be counted.
        let rels = read_relationships(&mut archive, "ppt/presentation.xml")?;
        let mut slide_parts = Vec::new();
        if let Some(list) = root.find("sldIdLst") {
            for entry in list.children_named("sldId") {
                let Some(id) = entry.attr_ns("id") else {
                    continue;
                };
                let Some(rel) = rels.get(id) else {
                    continue;
                };
                if !rel.external {
                    slide_parts.push(rel.target.clone());
                }
            }
        }
        if slide_parts.is_empty() {
            slide_parts = enumerate_slides(&mut archive);
        }

        // Pictures and charts are read from the package once; a slide's
        // relationship says which part it shows. The comment authors are named
        // by the presentation's own relationships, so they are read once too.
        let media = read_parts(&mut archive, "ppt/media/");
        let charts = read_parts(&mut archive, "ppt/charts/");
        let authors = read_comment_authors(&mut archive, &rels);

        let mut slides = Vec::with_capacity(slide_parts.len());
        for (index, part) in slide_parts.iter().enumerate() {
            let bytes = read_part(&mut archive, part)
                .with_context(|| format!("slide {} has no part at {part}", index + 1))?;
            let slide = Node::parse(&bytes).with_context(|| format!("malformed {part}"))?;
            let hidden = matches!(slide.attr("show"), Some("0") | Some("false"));
            let slide_rels = read_relationships(&mut archive, part)?;
            let style = SlideStyle::load(&mut archive, &slide_rels)?;
            let context = ShapeContext {
                width,
                height,
                style: &style,
                rels: &slide_rels,
                media: &media,
                charts: &charts,
            };
            let background = slide_background(&slide, &context, width, height);
            let mut shapes = Vec::new();
            if let Some(tree) = slide.find("spTree") {
                collect_shapes(tree, &context, &mut shapes, true);
            }
            slides.push(Slide {
                number: (index + 1) as i32,
                hidden,
                shapes,
                notes: read_notes(&mut archive, &slide_rels),
                comments: read_comments(&mut archive, &slide_rels, &authors, index),
                background,
            });
        }

        Ok(Self {
            width,
            height,
            slides,
        })
    }
}

// --- the package ----------------------------------------------------------

fn read_part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut file = archive.by_name(name).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

/// Every `ppt/slides/slideN.xml` in the package, in numeric order — the
/// fallback for a package whose presentation part names no slides.
fn enumerate_slides(archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Vec<String> {
    let mut parts: Vec<(u32, String)> = Vec::new();
    for index in 0..archive.len() {
        let Ok(file) = archive.by_index(index) else {
            continue;
        };
        let name = file.name().to_string();
        if !name.starts_with("ppt/slides/") || !name.ends_with(".xml") {
            continue;
        }
        let number = name
            .trim_start_matches("ppt/slides/slide")
            .trim_end_matches(".xml")
            .parse()
            .unwrap_or(u32::MAX);
        parts.push((number, name));
    }
    parts.sort();
    parts.into_iter().map(|(_, name)| name).collect()
}

/// Snapshot every part under `prefix`, so walking a slide does not need the
/// archive open behind it: the media a slide shows and the charts it draws.
fn read_parts(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    prefix: &str,
) -> HashMap<String, Vec<u8>> {
    let mut parts = HashMap::new();
    for index in 0..archive.len() {
        let Ok(mut file) = archive.by_index(index) else {
            continue;
        };
        let name = file.name().to_string();
        if !name.starts_with(prefix) {
            continue;
        }
        let mut buffer = Vec::new();
        if file.read_to_end(&mut buffer).is_ok() {
            parts.insert(name, buffer);
        }
    }
    parts
}

/// One relationship of a part.
#[derive(Debug, Clone)]
struct Relationship {
    /// The target resolved to a package part (`../media/image1.png` from
    /// `ppt/slides/` is `ppt/media/image1.png`), or the URL as written for an
    /// external one.
    target: String,
    /// The relationship type's URI; its tail says what the target is.
    kind: String,
    external: bool,
}

impl Relationship {
    /// Whether the target is a part of `kind`, e.g. `/slideLayout`.
    fn is(&self, kind: &str) -> bool {
        self.kind.ends_with(kind)
    }
}

fn read_relationships(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    part: &str,
) -> Result<HashMap<String, Relationship>> {
    let path = rels_path(part);
    let Some(bytes) = read_part(archive, &path) else {
        return Ok(HashMap::new());
    };
    let root = Node::parse(&bytes).with_context(|| format!("malformed {path}"))?;
    let mut out = HashMap::new();
    for entry in root.children_named("Relationship") {
        let (Some(id), Some(target)) = (entry.attr("Id"), entry.attr("Target")) else {
            continue;
        };
        let external = entry
            .attr("TargetMode")
            .is_some_and(|mode| mode.eq_ignore_ascii_case("external"));
        let target = if external {
            target.to_string()
        } else {
            resolve_target(part, target)
        };
        out.insert(
            id.to_string(),
            Relationship {
                target,
                kind: entry.attr("Type").unwrap_or_default().to_string(),
                external,
            },
        );
    }
    Ok(out)
}

/// `ppt/slides/slide1.xml` → `ppt/slides/_rels/slide1.xml.rels`.
fn rels_path(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((directory, file)) => format!("{directory}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// Resolve a relationship target against the directory its part lives in.
fn resolve_target(part: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let directory = part.rsplit_once('/').map_or("", |(directory, _)| directory);
    let mut segments: Vec<&str> = directory.split('/').filter(|s| !s.is_empty()).collect();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

// --- a slide's notes and comments -----------------------------------------

/// The comment authors a deck names: `p:cmAuthor/@id` → the name and initials
/// as written, so a comment can say who left it.
///
/// The authors part is reached through the presentation's own relationships,
/// like every other part, and a deck with no such part simply has no names.
fn read_comment_authors(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    presentation_rels: &HashMap<String, Relationship>,
) -> HashMap<String, (Option<String>, Option<String>)> {
    let Some(part) = presentation_rels
        .values()
        .find(|rel| rel.is("/commentAuthors") && !rel.external)
        .map(|rel| rel.target.clone())
    else {
        return HashMap::new();
    };
    let Some(bytes) = read_part(archive, &part) else {
        return HashMap::new();
    };
    let Ok(root) = Node::parse(&bytes) else {
        return HashMap::new();
    };
    let mut authors = HashMap::new();
    for author in root.children_named("cmAuthor") {
        let Some(id) = author.attr("id") else {
            continue;
        };
        authors.insert(
            id.to_string(),
            (
                author.attr("name").map(str::to_string),
                author.attr("initials").map(str::to_string),
            ),
        );
    }
    authors
}

/// A slide's speaker notes.
///
/// The notes are the notes part's body placeholder — a notes slide also carries
/// the slide number and a picture of the slide, and neither is a speaker note —
/// read the way python-pptx reads a text frame: the runs and fields one after
/// another, a soft line break a vertical tab, a newline between paragraphs,
/// and the whole thing trimmed. Docling's own PPTX backend records exactly that
/// string, so this is what the two readings can be held to.
fn read_notes(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    slide_rels: &HashMap<String, Relationship>,
) -> Option<String> {
    let part = slide_rels
        .values()
        .find(|rel| rel.is("/notesSlide") && !rel.external)?
        .target
        .clone();
    let bytes = read_part(archive, &part)?;
    let root = Node::parse(&bytes).ok()?;
    let tree = root.find("spTree")?;
    let body = tree
        .children_named("sp")
        .find(|shape| placeholder(shape).is_some_and(|holder| holder.kind == "body"))?;
    let text = text_frame_text(body.find("txBody")?).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// A text body as python-pptx reads it: every paragraph's text, joined by a
/// newline.
fn text_frame_text(body: &Node) -> String {
    body.children_named("p")
        .map(paragraph_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// One paragraph as python-pptx reads it: runs and fields concatenated, a soft
/// line break a vertical tab (the character PowerPoint uses for one when text
/// is copied out).
fn paragraph_text(paragraph: &Node) -> String {
    let mut text = String::new();
    for child in &paragraph.children {
        match child.name.as_str() {
            "r" | "fld" => text.push_str(
                child
                    .find("t")
                    .map(|run| run.text.as_str())
                    .unwrap_or_default(),
            ),
            "br" => text.push('\u{b}'),
            _ => {}
        }
    }
    text
}

/// The comments a slide's own relationships name, in the order the comments
/// part lists them.
///
/// `slide_index` is the comment id a file that omits `p:cm/@idx` is read with,
/// which is the slide's 0-based index — Docling's own fallback, kept so the two
/// readings name a comment's group the same way.
fn read_comments(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    slide_rels: &HashMap<String, Relationship>,
    authors: &HashMap<String, (Option<String>, Option<String>)>,
    slide_index: usize,
) -> Vec<Comment> {
    let mut comments = Vec::new();
    for rel in slide_rels.values() {
        if !rel.is("/comments") || rel.external {
            continue;
        }
        let Some(bytes) = read_part(archive, &rel.target) else {
            continue;
        };
        let Ok(root) = Node::parse(&bytes) else {
            continue;
        };
        for comment in root.children_named("cm") {
            let text = comment
                .child("text")
                .map(|text| text.text.trim().to_string())
                .unwrap_or_default();
            // A comment with no words is not content; Docling skips it too.
            if text.is_empty() {
                continue;
            }
            let (author, initials) = comment
                .attr("authorId")
                .and_then(|id| authors.get(id))
                .cloned()
                .unwrap_or((None, None));
            // An author whose name the deck did not write is no author at all.
            let author = author.filter(|name| !name.is_empty());
            comments.push(Comment {
                index: comment
                    .attr("idx")
                    .map(str::to_string)
                    .unwrap_or_else(|| slide_index.to_string()),
                date: comment.attr("dt").map(str::to_string),
                initials: author.as_ref().and(initials),
                author,
                text,
            });
        }
    }
    comments
}

// --- the layout and master styles a slide inherits ------------------------

/// A placeholder on a slide or a layout.
#[derive(Debug, Clone)]
struct Placeholder {
    /// `a:ph/@type`, `obj` when unstated.
    kind: String,
    /// `a:ph/@idx`; python-pptx reads an absent `idx` as 0, and so does this.
    idx: usize,
}

/// The style information a slide inherits from its layout and master: where a
/// placeholder sits, and what bullet each paragraph level is drawn with.
#[derive(Default)]
struct SlideStyle {
    layout: HashMap<usize, LayoutPlaceholder>,
    master: MasterStyle,
    /// The colours the master's theme declares. A shape's fill is usually a
    /// theme slot rather than a literal colour, so a slide without this could
    /// not paint the shapes the deck draws itself.
    theme: Theme,
}

#[derive(Debug, Default)]
struct LayoutPlaceholder {
    bbox: Option<[f64; 4]>,
    /// The bullet of each level, `Some(BulletKind::None)` for an explicit
    /// `buNone`. `None` means the level said nothing, so the master decides.
    levels: Vec<Option<BulletKind>>,
}

#[derive(Debug, Default)]
struct MasterStyle {
    /// Placement of the master's placeholders by type, the last resort for a
    /// slide placeholder that neither it nor its layout places.
    by_type: HashMap<String, [f64; 4]>,
    title: Vec<Option<BulletKind>>,
    body: Vec<Option<BulletKind>>,
    other: Vec<Option<BulletKind>>,
}

impl SlideStyle {
    /// Load the layout and master behind one slide part.
    fn load(
        archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
        rels: &HashMap<String, Relationship>,
    ) -> Result<Self> {
        let mut style = Self::default();
        let Some(layout_part) = rels
            .values()
            .find(|rel| rel.is("/slideLayout") && !rel.external)
            .map(|rel| rel.target.clone())
        else {
            return Ok(style);
        };
        let Some(bytes) = read_part(archive, &layout_part) else {
            return Ok(style);
        };
        let layout = Node::parse(&bytes).with_context(|| format!("malformed {layout_part}"))?;

        if let Some(tree) = layout.find("spTree") {
            for shape in tree.children_named("sp") {
                let Some(placeholder) = placeholder(shape) else {
                    continue;
                };
                let levels =
                    levels_from(shape.find("txBody").and_then(|body| body.child("lstStyle")));
                style.layout.insert(
                    placeholder.idx,
                    LayoutPlaceholder {
                        bbox: own_bbox(shape),
                        levels,
                    },
                );
            }
        }

        let layout_rels = read_relationships(archive, &layout_part)?;
        let Some(master_part) = layout_rels
            .values()
            .find(|rel| rel.is("/slideMaster") && !rel.external)
            .map(|rel| rel.target.clone())
        else {
            return Ok(style);
        };
        let Some(bytes) = read_part(archive, &master_part) else {
            return Ok(style);
        };
        let master = Node::parse(&bytes).with_context(|| format!("malformed {master_part}"))?;

        if let Some(tree) = master.find("spTree") {
            for shape in tree.children_named("sp") {
                if let (Some(placeholder), Some(bbox)) = (placeholder(shape), own_bbox(shape)) {
                    style.master.by_type.entry(placeholder.kind).or_insert(bbox);
                }
            }
        }
        if let Some(styles) = master.find("txStyles") {
            style.master.title = levels_from(styles.child("titleStyle"));
            style.master.body = levels_from(styles.child("bodyStyle"));
            style.master.other = levels_from(styles.child("otherStyle"));
        }

        // The master's theme is the file's colour scheme: a shape stating
        // `a:schemeClr val="accent1"` takes it from here.
        let master_rels = read_relationships(archive, &master_part)?;
        if let Some(theme_part) = master_rels
            .values()
            .find(|rel| rel.is("/theme") && !rel.external)
            .map(|rel| rel.target.clone())
        {
            if let Some(bytes) = read_part(archive, &theme_part) {
                style.theme = Theme::parse(&bytes);
            }
        }

        Ok(style)
    }

    /// The bullet the master draws this placeholder's level with.
    fn master_level(&self, kind: &str, level: usize) -> Option<BulletKind> {
        let levels = match kind {
            "title" => &self.master.title,
            "body" | "obj" => &self.master.body,
            _ => &self.master.other,
        };
        levels.get(level).copied().flatten()
    }
}

/// What a paragraph's level is drawn with, as far as one style says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BulletKind {
    /// `buNone`: explicitly not a list item.
    None,
    Char(char),
    /// `buAutoNum`: numbered from `startAt`.
    Number(usize),
    /// `buBlip`: a picture bullet, which reads as a bullet.
    Picture,
}

/// The bullet each of the nine levels of a `lstStyle`-like element says.
fn levels_from(style: Option<&Node>) -> Vec<Option<BulletKind>> {
    let mut levels = vec![None; 9];
    let Some(style) = style else {
        return levels;
    };
    for (level, slot) in levels.iter_mut().enumerate() {
        if let Some(properties) = style.child(&format!("lvl{}pPr", level + 1)) {
            *slot = bullet_of(Some(properties));
        }
    }
    levels
}

/// The bullet a paragraph-properties element names, if it names one.
fn bullet_of(properties: Option<&Node>) -> Option<BulletKind> {
    let properties = properties?;
    if properties.child("buNone").is_some() {
        return Some(BulletKind::None);
    }
    if let Some(bullet) = properties.child("buChar") {
        return Some(BulletKind::Char(
            bullet
                .attr("char")
                .and_then(|text| text.chars().next())
                .unwrap_or('•'),
        ));
    }
    if let Some(numbering) = properties.child("buAutoNum") {
        return Some(BulletKind::Number(
            numbering
                .attr("startAt")
                .and_then(|value| value.parse().ok())
                .unwrap_or(1),
        ));
    }
    if properties.child("buBlip").is_some() {
        return Some(BulletKind::Picture);
    }
    None
}

/// Resolve a paragraph's list marker, in the order a reader's eye follows:
/// the paragraph itself, the shape's list style, the layout's placeholder, the
/// master's text style. Docling resolves the same five places in the same
/// order, which is why a body placeholder's paragraphs are list items without
/// any bullet being written into the slide.
fn resolve_bullet(
    paragraph: Option<&Node>,
    shape_style: Option<&Node>,
    placeholder: Option<&Placeholder>,
    style: &SlideStyle,
    level: usize,
) -> Option<BulletKind> {
    if let Some(kind) = bullet_of(paragraph) {
        return Some(kind);
    }
    if let Some(kind) = shape_style
        .and_then(|style| style.child(&format!("lvl{}pPr", level + 1)))
        .and_then(|properties| bullet_of(Some(properties)))
    {
        return Some(kind);
    }
    let placeholder = placeholder?;
    let layout = style.layout.get(&placeholder.idx)?;
    if let Some(Some(kind)) = layout.levels.get(level) {
        return Some(*kind);
    }
    style.master_level(&placeholder.kind, level)
}

fn placeholder(shape: &Node) -> Option<Placeholder> {
    let properties = shape
        .child("nvSpPr")
        .or_else(|| shape.child("nvPicPr"))
        .or_else(|| shape.child("nvGraphicFramePr"))?;
    let placeholder = properties.child("nvPr")?.child("ph")?;
    Some(Placeholder {
        kind: placeholder.attr("type").unwrap_or("obj").to_string(),
        idx: placeholder
            .attr("idx")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
    })
}

// --- shapes ---------------------------------------------------------------

/// What one slide's shapes are read against.
struct ShapeContext<'a> {
    width: f64,
    height: f64,
    style: &'a SlideStyle,
    rels: &'a HashMap<String, Relationship>,
    media: &'a HashMap<String, Vec<u8>>,
    charts: &'a HashMap<String, Vec<u8>>,
}

/// Walk a shape tree, in the order a reader meets its shapes.
///
/// `paint` says whether a shape found here is drawn by its own markup. It is
/// true for the slide's own shapes and false inside a group, which is painted
/// whole: a group is one picture, so drawing its children again would put the
/// same ink on the slide twice.
///
/// An `mc:AlternateContent` is one shape stated twice: the modern form in its
/// `mc:Choice`, and the copy an older reader shows in `mc:Fallback` — the way
/// Word writes a picture it keeps compatible with an older Word. They are not
/// two shapes, so the choice is read and the fallback only when the choice gave
/// this reader nothing: a picture naming a part the package does not hold, which
/// is what the fallback is for.
fn collect_shapes(container: &Node, context: &ShapeContext, out: &mut Vec<Shape>, paint: bool) {
    let mut run: Vec<&Node> = Vec::new();
    for child in &container.children {
        if is_shape(child) {
            run.push(child);
            continue;
        }
        if child.name != "AlternateContent" {
            continue;
        }
        if !run.is_empty() {
            collect_shape_nodes(&run, context, out, paint);
            run.clear();
        }
        let before = out.len();
        collect_shape_nodes(&branch(child, "Choice"), context, out, paint);
        // A choice this reader cannot use — a picture naming a part the package
        // does not hold — leaves the fallback, which is what an older reader
        // shows.
        if out.len() == before {
            collect_shape_nodes(&branch(child, "Fallback"), context, out, paint);
        }
    }
    if !run.is_empty() {
        collect_shape_nodes(&run, context, out, paint);
    }
}

/// The elements of a shape tree a reader takes as shapes.
const SHAPES: [&str; 5] = ["sp", "pic", "graphicFrame", "grpSp", "cxnSp"];

fn is_shape(node: &Node) -> bool {
    SHAPES.contains(&node.name.as_str())
}

/// The shape elements one branch of an `mc:AlternateContent` holds.
fn branch<'a>(node: &'a Node, which: &str) -> Vec<&'a Node> {
    node.children
        .iter()
        .filter(|child| child.name == which)
        .flat_map(|chosen| chosen.children.iter())
        .filter(|child| is_shape(child))
        .collect()
}

/// Read a run of sibling shapes as one block: sorted into rows by where they
/// sit, each row left to right.
fn collect_shape_nodes(nodes: &[&Node], context: &ShapeContext, out: &mut Vec<Shape>, paint: bool) {
    let mut entries: Vec<(usize, &Node, i64, i64)> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            // A shape's place decides when it is read; one that cannot be placed
            // goes last, where Docling puts it.
            let (top, left) = resolved_bbox(node, context).map_or((i64::MAX, i64::MAX), |bbox| {
                (bbox[1] as i64, bbox[0] as i64)
            });
            (index, *node, top, left)
        })
        .collect();

    // Sort by top edge, keeping creation order for a tie, then cut the sorted
    // list into rows: shapes within the tolerance of their predecessor share a
    // row even when the row as a whole is taller than one tolerance step.
    entries.sort_by_key(|(index, _, top, _)| (*top, *index));
    let mut boundaries = Vec::new();
    let mut previous: Option<i64> = None;
    for (position, entry) in entries.iter().enumerate() {
        if let Some(previous) = previous {
            if entry.2.saturating_sub(previous) > ROW_TOLERANCE {
                boundaries.push(position);
            }
        }
        previous = Some(entry.2);
    }
    boundaries.push(entries.len());

    let mut start = 0;
    for end in boundaries {
        let row = &mut entries[start..end];
        row.sort_by_key(|(index, _, _, left)| (*left, *index));
        for (_, node, _, _) in row.iter() {
            match node.name.as_str() {
                "grpSp" => {
                    if paint {
                        if let Some(shape) = painted_shape(node, context) {
                            out.push(Shape::Picture(shape));
                        }
                    }
                    // The group's children are placed in the group's own
                    // coordinate space, which a reader that has only the slide
                    // cannot map; their text is still the slide's text.
                    collect_shapes(node, context, out, false);
                }
                "sp" => {
                    if paint {
                        if let Some(shape) = painted_shape(node, context) {
                            out.push(Shape::Picture(shape));
                        }
                    }
                    // A paragraph's marker can be a picture (`a:buBlip`): the
                    // shape paints no ink for it and no run carries it, so it
                    // is reported as the picture it is, ahead of the item it
                    // opens.
                    if paint {
                        for bullet in bullet_pictures(node, context) {
                            out.push(Shape::Picture(bullet));
                        }
                    }
                    if let Some(shape) = text_shape(node, context) {
                        out.push(Shape::Text(shape));
                    }
                }
                // A connector draws a line and carries no words, but the line
                // is ink on the slide like any other shape's.
                "cxnSp" => {
                    if paint {
                        if let Some(shape) = painted_shape(node, context) {
                            out.push(Shape::Picture(shape));
                        }
                    }
                }
                // A graphic frame holds a table or a chart, and says which.
                "graphicFrame" => {
                    if let Some(shape) = table_shape(node, context) {
                        out.push(Shape::Table(shape));
                    } else if let Some(shape) = chart_shape(node, context) {
                        out.push(Shape::Chart(shape));
                    }
                }
                "pic" => {
                    if let Some(shape) = picture_shape(node, context) {
                        out.push(Shape::Picture(shape));
                    }
                }
                _ => {}
            }
        }
        start = end;
    }
}

/// A shape the slide paints itself, drawn into a PNG.
///
/// A `p:sp`, a `p:cxnSp` or a `p:grpSp` has no image part behind it: its
/// geometry, fill and line are the markup, and PowerPoint paints them. A reader
/// that only looked for a `p:blipFill` left such a shape out of the document
/// altogether, so it is drawn here at the size the shape states. A shape that
/// puts no ink down — a text box with no fill and no line, a placeholder that
/// states nothing — is no picture, which is what the painter answers with
/// `None`.
///
/// A fill can be a picture, and then the fill belongs to the shape: the painter
/// is given the slide's media, so the image is drawn through the shape's own
/// geometry — inside an ellipse, inside a rounded corner — with the shape's
/// line over it, rather than the picture being reported as a box where the
/// shape is.
fn painted_shape(shape: &Node, context: &ShapeContext) -> Option<PictureShape> {
    // Nothing to paint without a body: a `p:sp` always has one, a `p:grpSp`
    // keeps its transform in `p:grpSpPr`.
    shape.child("spPr").or_else(|| shape.child("grpSpPr"))?;
    let bbox = resolved_bbox(shape, context)?;
    let media = FillMedia {
        rels: context.rels,
        media: context.media,
    };
    let PaintedPicture { png, .. } =
        render_node_with(&drawing_node(shape), &context.style.theme, &media)?;
    Some(PictureShape {
        bbox,
        // The painter produces a PNG; Docling's model names a picture it cannot
        // decode no other way, and a shape has no source encoding.
        mime: "image/png",
        data: png,
    })
}

/// The slide's pictures, by the relationship id a fill names them with.
///
/// The painter draws a picture fill through the shape that holds it, and only
/// the reader that has the package can resolve the id to bytes: this is that
/// lookup, over the slide's own relationships. A linked picture (`r:link`) has
/// no bytes in the package, so a fill that names one is a fill this cannot
/// paint.
struct FillMedia<'a> {
    rels: &'a HashMap<String, Relationship>,
    media: &'a HashMap<String, Vec<u8>>,
}

impl Media for FillMedia<'_> {
    fn picture(&self, id: &str) -> Option<(String, Vec<u8>)> {
        let relationship = self.rels.get(id)?;
        if relationship.external {
            return None;
        }
        let mime = mime_for(&relationship.target)?;
        let data = self.media.get(&relationship.target)?;
        Some((mime.to_string(), data.clone()))
    }
}

/// The slide's tree as the painter reads it: local element names, every
/// attribute under its local name, and no text (the words of a shape are the
/// slide's text, and the painter never draws them).
fn drawing_node(node: &Node) -> DrawingNode {
    DrawingNode {
        name: node.name.clone(),
        attrs: node
            .attributes
            .iter()
            .map(|attribute| (attribute.local.clone(), attribute.value.clone()))
            .collect(),
        children: node.children.iter().map(drawing_node).collect(),
    }
}

fn text_shape(shape: &Node, context: &ShapeContext) -> Option<TextShape> {
    let body = shape.find("txBody")?;
    let placeholder = placeholder(shape);
    let title = placeholder
        .as_ref()
        .is_some_and(|placeholder| matches!(placeholder.kind.as_str(), "title" | "ctrTitle"));
    let paragraphs = read_paragraphs(
        body,
        body.child("lstStyle"),
        placeholder.as_ref(),
        context.style,
    );
    // A shape whose text is empty is not content; Docling skips it too.
    if !paragraphs
        .iter()
        .any(|paragraph| !paragraph.text.is_empty())
    {
        return None;
    }
    Some(TextShape {
        bbox: bbox_of(shape, context),
        title,
        paragraphs,
    })
}

fn read_paragraphs(
    body: &Node,
    shape_style: Option<&Node>,
    placeholder: Option<&Placeholder>,
    style: &SlideStyle,
) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    // Docling counts a numbered list per open list group. The counter of each
    // level is what that comes to: a numbered item at a level continues that
    // level's count, and a change of level or a non-list paragraph closes the
    // deeper lists.
    let mut counters = [0usize; 9];
    for paragraph in body.children_named("p") {
        let properties = paragraph.child("pPr");
        let level = properties
            .and_then(|properties| properties.attr("lvl"))
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0)
            .min(counters.len() - 1);

        let mut runs = Vec::new();
        let mut text = String::new();
        for child in &paragraph.children {
            match child.name.as_str() {
                "r" | "fld" => {
                    let run = read_run(child);
                    text.push_str(&run.text);
                    runs.push(run);
                }
                // A break is a space in the line, as it is to a reader and to
                // Docling's own PPTX reader.
                "br" => text.push(' '),
                _ => {}
            }
        }

        let list = match resolve_bullet(properties, shape_style, placeholder, style, level) {
            Some(BulletKind::None) => {
                counters.fill(0);
                None
            }
            Some(BulletKind::Number(start)) => {
                if counters[level] == 0 {
                    counters[level] = start.max(1) - 1;
                }
                counters[level] += 1;
                for deeper in counters.iter_mut().skip(level + 1) {
                    *deeper = 0;
                }
                Some(ListMarker {
                    marker: format!("{}.", counters[level]),
                    enumerated: true,
                })
            }
            Some(BulletKind::Char(_)) | Some(BulletKind::Picture) => {
                for deeper in counters.iter_mut().skip(level + 1) {
                    *deeper = 0;
                }
                Some(ListMarker {
                    marker: String::new(),
                    enumerated: false,
                })
            }
            // A paragraph below the first level is a nested list even when no
            // style says so; Docling reads it that way, and so does a reader.
            None if level > 0 => Some(ListMarker {
                marker: String::new(),
                enumerated: false,
            }),
            None => {
                counters.fill(0);
                None
            }
        };

        paragraphs.push(Paragraph {
            runs,
            text,
            level,
            list,
        });
    }
    paragraphs
}

fn read_run(run: &Node) -> Run {
    let properties = run.child("rPr");
    let flag = |name: &str| {
        properties
            .and_then(|properties| properties.attr(name))
            .is_some_and(truthy)
    };
    Run {
        text: run
            .find("t")
            .map(|text| text.text.clone())
            .unwrap_or_default(),
        bold: flag("b"),
        italic: flag("i"),
        underline: properties
            .and_then(|properties| properties.attr("u"))
            .is_some_and(|value| value != "none"),
        size_pt: properties
            .and_then(|properties| properties.attr("sz"))
            .and_then(|value| value.parse::<f64>().ok())
            .map(|hundredths| hundredths / 100.0),
    }
}

fn table_shape(shape: &Node, context: &ShapeContext) -> Option<TableShape> {
    let table = shape.find("tbl")?;
    let rows: Vec<&Node> = table.children_named("tr").collect();
    if rows.is_empty() {
        return None;
    }
    let declared = table
        .child("tblGrid")
        .map(|grid| grid.children_named("gridCol").count())
        .unwrap_or(0);

    let mut cells = Vec::new();
    let mut widest = 0;
    for (row, line) in rows.iter().enumerate() {
        let mut column = 0;
        for cell in line.children_named("tc") {
            let col_span = span(cell.attr("gridSpan"));
            // A merge cannot reach past the table's last row.
            let row_span = span(cell.attr("rowSpan")).min(rows.len() - row).max(1);
            // A horizontal continuation is the slot its anchor already
            // spans, so it adds no column of its own; a vertical one is a slot
            // of *this* row, so it does.
            let horizontal = truthy_attr(cell, "hMerge");
            let vertical = truthy_attr(cell, "vMerge");
            if !horizontal && !vertical {
                cells.push(TableCell {
                    row,
                    col: column,
                    row_span,
                    col_span,
                    text: cell_text(cell),
                });
            }
            if !horizontal {
                column += col_span;
                widest = widest.max(column);
            }
        }
    }

    Some(TableShape {
        bbox: bbox_of(shape, context),
        table: Table {
            num_rows: rows.len(),
            num_cols: declared.max(widest),
            cells,
        },
    })
}

/// The text a reader sees in a table cell: its paragraphs, one per line.
fn cell_text(cell: &Node) -> String {
    let Some(body) = cell.find("txBody") else {
        return String::new();
    };
    let mut lines = Vec::new();
    for paragraph in body.children_named("p") {
        let mut text = String::new();
        for child in &paragraph.children {
            match child.name.as_str() {
                "r" | "fld" => {
                    if let Some(part) = child.find("t") {
                        text.push_str(&part.text);
                    }
                }
                "br" => text.push(' '),
                _ => {}
            }
        }
        lines.push(text);
    }
    lines.join("\n").trim().to_string()
}

fn picture_shape(shape: &Node, context: &ShapeContext) -> Option<PictureShape> {
    let blip = shape.find("blip")?;
    let (mime, data) = blip_bytes(blip, context)?;
    Some(PictureShape {
        bbox: bbox_of(shape, context),
        mime,
        data,
    })
}

/// The picture a slide states as its background, if it states one.
///
/// A background is not a shape: PowerPoint writes it in
/// `p:cSld/p:bg/p:bgPr`, outside the shape tree, and a picture fill there is an
/// `a:blipFill` naming a media part through the slide's own relationships — the
/// same `a:blip` a picture uses. Reading only the shape tree loses a
/// photographic backdrop whole, so it is taken here as the page's image at the
/// full slide box. A background stated as a theme fill (`p:bgRef`) or as a
/// colour is not a picture and is left to the slide's shapes and page size.
fn slide_background(
    slide: &Node,
    context: &ShapeContext,
    width: f64,
    height: f64,
) -> Option<PictureShape> {
    let blip = slide
        .child("cSld")?
        .child("bg")?
        .child("bgPr")?
        .child("blipFill")?
        .child("blip")?;
    let (mime, data) = blip_bytes(blip, context)?;
    Some(PictureShape {
        bbox: [0.0, 0.0, width, height],
        mime,
        data,
    })
}

/// English Metric Units per point: the units a slide's placement is stated in
/// and the units a picture bullet's box is built in.
const EMU_PER_POINT: f64 = 12700.0;

/// The type size a paragraph falls back to when it states none: the size a
/// slide's body text is set in.
const DEFAULT_TEXT_PT: f64 = 18.0;

/// Every picture bullet a shape's paragraphs print, one per paragraph.
///
/// `a:buBlip` names the media the marker is drawn with, the way a picture names
/// its part — the raster on the `a:blip`, the drawing in the `asvg:svgBlip`
/// extension beside it. It is the one bullet that is a drawing rather than a
/// glyph: the painter draws geometry and no run carries its words, so a
/// paragraph marked with one used to be a list item with an empty marker and
/// nothing where the picture was.
///
/// A bullet is drawn at the size of the line it opens, so the picture is a
/// square of the item's type size. A slide states no line boxes, so where that
/// square sits on the line is not in the file: it is reported at the corner the
/// shape's text starts from, which is where an item's bullet is drawn.
fn bullet_pictures(shape: &Node, context: &ShapeContext) -> Vec<PictureShape> {
    let Some(frame) = resolved_bbox(shape, context) else {
        return Vec::new();
    };
    let Some(body) = shape.find("txBody") else {
        return Vec::new();
    };
    let mut pictures = Vec::new();
    for paragraph in body.children_named("p") {
        let Some(blip) = paragraph
            .child("pPr")
            .and_then(|properties| properties.child("buBlip"))
            .and_then(|bullet| bullet.child("blip"))
        else {
            continue;
        };
        let Some((mime, data)) = blip_bytes(blip, context) else {
            continue;
        };
        let size_pt = paragraph
            .children_named("r")
            .next()
            .and_then(|run| run.child("rPr"))
            .and_then(|properties| properties.attr("sz"))
            .and_then(|value| value.parse::<f64>().ok())
            .or_else(|| {
                paragraph
                    .child("endParaRPr")
                    .and_then(|properties| properties.attr("sz"))
                    .and_then(|value| value.parse::<f64>().ok())
            })
            .map(|hundredths| hundredths / 100.0)
            .unwrap_or(DEFAULT_TEXT_PT);
        let side = size_pt * EMU_PER_POINT;
        pictures.push(PictureShape {
            bbox: [frame[0], frame[1], frame[0] + side, frame[1] + side],
            mime,
            data,
        });
    }
    pictures
}

/// The bytes a blip names, turned the way PowerPoint displays them.
///
/// A picture and a shape's fill carry the same `a:blip`, so both resolve it
/// here: the drawing when the blip states one, the raster otherwise, and pixels
/// turned upright when their EXIF tag says PowerPoint shows them that way.
fn blip_bytes(blip: &Node, context: &ShapeContext) -> Option<(&'static str, Vec<u8>)> {
    let (mime, data) = picture_bytes(blip, context)?;
    Some(match pdfextractor_image_parser::turn_upright(&data) {
        Some(upright) => {
            tracing::debug!(
                "turned an embedded picture upright ({}x{})",
                upright.width,
                upright.height
            );
            (upright.mimetype, upright.data)
        }
        None => (mime, data),
    })
}

/// The bytes a `a:blip` names, and their media type — the drawing first.
///
/// Word and PowerPoint 2016 and later store an SVG picture twice: the `a:blip`'s
/// own `r:embed` names a PNG fallback, for readers that do not draw SVG, and an
/// `asvg:svgBlip` extension beside it names the drawing itself. The fallback is
/// only what an old reader shows; the SVG is the picture the file asks for, and
/// the one whose resolution the file leaves open, so it is taken whenever its
/// part is in the package. A `svgBlip` naming anything else, or a part that is
/// not here, leaves the fallback as the picture — which is what it is for.
fn picture_bytes(blip: &Node, context: &ShapeContext) -> Option<(&'static str, Vec<u8>)> {
    let part = |id: &str| -> Option<(&'static str, Vec<u8>)> {
        // `r:embed` is the embedded copy; a linked picture (`r:link`) points
        // outside the package and has no bytes here.
        let relationship = context.rels.get(id)?;
        if relationship.external {
            return None;
        }
        let mime = mime_for(&relationship.target)?;
        let data = context.media.get(&relationship.target)?;
        Some((mime, data.clone()))
    };
    if let Some(drawing) = blip
        .find("svgBlip")
        .and_then(|svg| svg.attr_ns("embed"))
        .and_then(part)
        .filter(|(mime, _)| *mime == "image/svg+xml")
    {
        return Some(drawing);
    }
    part(blip.attr_ns("embed")?)
}

/// The media type an embedded picture part is named by, from its extension.
///
/// The list is [`pdfextractor_picture_formats::PICTURE_FORMATS`] — the same one
/// the CLI writes a picture under and the flow reader accepts a `<source>`
/// for — so a deck's media type and the file it becomes are the same fact, and
/// a format added there is read out of a package without another edit here. An
/// extension the table does not carry is not a picture this names.
fn mime_for(part: &str) -> Option<&'static str> {
    pdfextractor_picture_formats::mime_for_extension(part.rsplit_once('.')?.1)
}

// --- charts ---------------------------------------------------------------

/// A chart shape: the frame's box and the chart part its relationship names.
fn chart_shape(shape: &Node, context: &ShapeContext) -> Option<ChartShape> {
    // A graphic frame's payload is `a:graphic/a:graphicData`; a chart is the
    // `c:chart` inside it, pointing at the part holding the data.
    let reference = shape.find("chart")?;
    let id = reference.attr_ns("id")?;
    let relationship = context.rels.get(id)?;
    if relationship.external {
        return None;
    }
    let mut chart = pdfextractor_chart::read_chart(context.charts.get(&relationship.target)?)?;
    chart.bbox = bbox_of(shape, context);
    Some(chart)
}

// --- geometry -------------------------------------------------------------

/// The bbox a shape will be reported with.
fn bbox_of(shape: &Node, context: &ShapeContext) -> [f64; 4] {
    resolved_bbox(shape, context).unwrap_or([0.0, 0.0, context.width, context.height])
}

/// The shape's own placement, or the one its placeholder inherits from the
/// layout and then the master.
fn resolved_bbox(shape: &Node, context: &ShapeContext) -> Option<[f64; 4]> {
    if let Some(bbox) = own_bbox(shape) {
        return Some(bbox);
    }
    if shape.name != "sp" {
        return None;
    }
    let placeholder = placeholder(shape)?;
    if let Some(bbox) = context
        .style
        .layout
        .get(&placeholder.idx)
        .and_then(|layout| layout.bbox)
    {
        return Some(bbox);
    }
    context
        .style
        .master
        .by_type
        .get(master_type(&placeholder.kind))
        .copied()
}

/// The master placeholder a layout placeholder inherits from. This is
/// python-pptx's own mapping: a content placeholder on a layout is written
/// without a `type`, and the master calls the one it inherits from `body`.
fn master_type(kind: &str) -> &str {
    match kind {
        "body" | "chart" | "bitmap" | "orgChart" | "media" | "obj" | "pic" | "subTitle" | "tbl" => {
            "body"
        }
        "ctrTitle" => "title",
        other => other,
    }
}

/// Where the shape itself is placed: `a:xfrm`'s offset and extent.
fn own_bbox(shape: &Node) -> Option<[f64; 4]> {
    let transform = match shape.name.as_str() {
        // A graphic frame's transform is `p:xfrm`, not `a:xfrm`.
        "graphicFrame" => shape.child("xfrm"),
        "grpSp" => shape
            .child("grpSpPr")
            .and_then(|properties| properties.child("xfrm")),
        _ => shape
            .child("spPr")
            .and_then(|properties| properties.child("xfrm")),
    }?;
    let offset = transform.child("off")?;
    let extent = transform.child("ext")?;
    let left = number(offset.attr("x")?)?;
    let top = number(offset.attr("y")?)?;
    let width = number(extent.attr("cx").unwrap_or("0")).unwrap_or(0.0);
    let height = number(extent.attr("cy").unwrap_or("0")).unwrap_or(0.0);
    Some([left, top, left + width, top + height])
}

// --- XML ------------------------------------------------------------------

/// One element of a part, with its attributes and children.
///
/// Attributes keep their qualified name (`r:id`) as well as their local one,
/// because `p:sldId` carries both `id` and `r:id` and they mean different
/// things.
#[derive(Debug, Default)]
struct Node {
    /// Local name: `sp`, `txBody`, `p`.
    name: String,
    attributes: Vec<XmlAttribute>,
    children: Vec<Node>,
    text: String,
}

#[derive(Debug)]
struct XmlAttribute {
    qualified: String,
    local: String,
    value: String,
}

impl Node {
    fn parse(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut root: Option<Node> = None;
        let mut open: Vec<Node> = Vec::new();
        loop {
            match reader.read_event() {
                Ok(Event::Start(start)) => open.push(element(&start)),
                Ok(Event::Empty(start)) => {
                    let node = element(&start);
                    attach(&mut root, &mut open, node);
                }
                Ok(Event::End(_)) => {
                    if let Some(node) = open.pop() {
                        attach(&mut root, &mut open, node);
                    }
                }
                Ok(Event::Text(text)) => {
                    if let Some(top) = open.last_mut() {
                        if let Ok(text) = text.unescape() {
                            top.text.push_str(&text);
                        }
                    }
                }
                Ok(Event::Eof) => break,
                // Declarations, comments, processing instructions and doctypes
                // carry nothing a slide needs.
                Ok(_) => {}
                Err(error) => bail!("malformed XML: {error}"),
            }
        }
        root.context("the part holds no element")
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The first descendant with this local name, this element included.
    fn find(&self, name: &str) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find(name) {
                return Some(found);
            }
        }
        None
    }

    /// An attribute by its qualified name, `r:id` and the like.
    fn attr(&self, qualified: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.qualified == qualified)
            .map(|attribute| attribute.value.as_str())
    }

    /// An attribute in a namespace: `r:id` rather than `id`, whatever prefix
    /// the package actually bound to that namespace.
    fn attr_ns(&self, local: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.local == local && attribute.qualified != local)
            .map(|attribute| attribute.value.as_str())
    }
}

fn attach(root: &mut Option<Node>, open: &mut [Node], node: Node) {
    match open.last_mut() {
        Some(parent) => parent.children.push(node),
        None => *root = Some(node),
    }
}

fn element(start: &BytesStart) -> Node {
    let name = local_name(start.name().as_ref());
    let mut attributes = Vec::new();
    for attribute in start.attributes().flatten() {
        let qualified = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        let local = local_name(attribute.key.as_ref());
        let value = attribute
            .unescape_value()
            .map(|value| value.into_owned())
            .unwrap_or_default();
        attributes.push(XmlAttribute {
            qualified,
            local,
            value,
        });
    }
    Node {
        name,
        attributes,
        children: Vec::new(),
        text: String::new(),
    }
}

fn local_name(name: &[u8]) -> String {
    let text = String::from_utf8_lossy(name);
    match text.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => text.into_owned(),
    }
}

fn number(text: &str) -> Option<f64> {
    text.trim().parse().ok()
}

fn span(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1)
}

fn truthy(value: &str) -> bool {
    matches!(value, "1") || value.eq_ignore_ascii_case("true")
}

fn truthy_attr(node: &Node, name: &str) -> bool {
    node.attr(name).is_some_and(truthy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const P: &str = r#"xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
    const A: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;
    const R: &str =
        r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    const RELS: &str = r#"xmlns="http://schemas.openxmlformats.org/package/2006/relationships""#;
    const TABLE_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/table";
    const LAYOUT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout";
    const MASTER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";
    const IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
    const CHART: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";

    /// A package from parts, in memory.
    fn package(parts: &[(&str, String)]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, body) in parts {
                zip.start_file(*name, options).expect("start part");
                zip.write_all(body.as_bytes()).expect("write part");
            }
            zip.finish().expect("finish package");
        }
        out
    }

    /// The same, with media parts whose bytes are not text.
    fn package_with_media(parts: &[(&str, String)], media: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, body) in parts {
                zip.start_file(*name, options).expect("start part");
                zip.write_all(body.as_bytes()).expect("write part");
            }
            for (name, data) in media {
                zip.start_file(*name, options).expect("start part");
                zip.write_all(data).expect("write part");
            }
            zip.finish().expect("finish package");
        }
        out
    }

    fn presentation(entries: &str, size: &str) -> String {
        format!(
            r#"<p:presentation {P} {R}><p:sldIdLst>{entries}</p:sldIdLst><p:sldSz cx="{size}" cy="{size}"/></p:presentation>"#
        )
    }

    fn relationships(entries: &str) -> String {
        format!(r#"<Relationships {RELS}>{entries}</Relationships>"#)
    }

    /// A one-slide deck whose slide part is `body`.
    fn deck(body: &str) -> PptxDocument {
        let pkg = package(&[
            (
                "ppt/presentation.xml",
                presentation(r#"<p:sldId id="256" r:id="rId1"/>"#, "9144000"),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>"#
                )),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(r#"<p:sld {P} {A} {R}>{body}</p:sld>"#),
            ),
        ]);
        PptxDocument::parse_bytes(&pkg).expect("parse")
    }

    /// A shape the slide paints itself becomes a picture at the shape's own
    /// box, and a text box that states no fill and no line is still only text.
    #[test]
    fn a_shape_the_slide_paints_becomes_a_picture() {
        let painted = format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
               <p:spPr>{}
                 <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                 <a:solidFill><a:srgbClr val="156082"/></a:solidFill>
                 <a:ln><a:noFill/></a:ln>
               </p:spPr>
               <p:txBody><a:bodyPr/><a:lstStyle/>
                 <a:p><a:r><a:t>A box with words.</a:t></a:r></a:p></p:txBody></p:sp>"#,
            transform(914400, 1371600, 1524000, 762000)
        );
        let blank = text_shape(
            &format!(
                r#"{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/>"#,
                transform(0, 2743200, 914400, 457200)
            ),
            r#"<a:p><a:r><a:t>Words with no ink.</a:t></a:r></a:p>"#,
        );
        let deck = deck(&format!(
            "<p:cSld><p:spTree>{painted}{blank}</p:spTree></p:cSld>"
        ));

        let pictures: Vec<&PictureShape> = deck.slides[0]
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Picture(picture) => Some(picture),
                _ => None,
            })
            .collect();
        assert_eq!(
            pictures.len(),
            1,
            "a text box with no ink was painted: {:#?}",
            deck.slides[0].shapes
        );
        assert_eq!(pictures[0].mime, "image/png");
        assert_eq!(
            pictures[0].bbox,
            [
                914_400.0,
                1_371_600.0,
                914_400.0 + 1_524_000.0,
                1_371_600.0 + 762_000.0
            ]
        );

        // The words of the painted shape are still the slide's text: a box
        // with ink behind it is a picture and its own paragraph, both.
        let texts: Vec<&str> = deck.slides[0]
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Text(text) => text.paragraphs.first().map(|p| p.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["A box with words.", "Words with no ink."]);
    }

    /// A slide shape: a text body with `paragraphs` inside `spPr`.
    fn text_shape(properties: &str, paragraphs: &str) -> String {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="text"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
               <p:spPr>{properties}</p:spPr>
               <p:txBody><a:bodyPr/><a:lstStyle/>{paragraphs}</p:txBody></p:sp>"#
        )
    }

    fn transform(x: i64, y: i64, cx: i64, cy: i64) -> String {
        format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>"#)
    }

    #[test]
    fn a_slide_is_a_page_in_presentation_order_and_its_own_size() {
        let pkg = package(&[
            (
                "ppt/presentation.xml",
                presentation(
                    r#"<p:sldId id="256" r:id="rId1"/><p:sldId id="257" r:id="rId2"/>"#,
                    "12192000",
                ),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>
                       <Relationship Id="rId2" Type="{LAYOUT}" Target="slides/slide2.xml"/>"#
                )),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(r#"<p:sld {P} {A} {R}></p:sld>"#),
            ),
            (
                "ppt/slides/slide2.xml",
                format!(r#"<p:sld {P} {A} {R}></p:sld>"#),
            ),
        ]);
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        assert_eq!((deck.width, deck.height), (12192000.0, 12192000.0));
        assert_eq!(deck.slides.len(), 2);
        assert_eq!(deck.slides[0].number, 1);
        assert_eq!(deck.slides[1].number, 2);
        assert!(!deck.slides[0].hidden);
    }

    #[test]
    fn a_text_body_keeps_its_paragraphs_and_runs() {
        let deck = deck(&format!(
            "<p:cSld><p:spTree>{}</p:spTree></p:cSld>",
            text_shape(
                &transform(0, 0, 914400, 914400),
                r#"<a:p><a:pPr/><a:r><a:rPr b="1" sz="2000"/><a:t>Revenue </a:t></a:r>
                     <a:r><a:rPr i="1"/><a:t>grew</a:t></a:r><a:br/>
                     <a:r><a:rPr u="sng"/><a:t>12%</a:t></a:r></a:p>
                   <a:p><a:endParaRPr/></a:p>"#
            )
        ));
        let Shape::Text(text) = &deck.slides[0].shapes[0] else {
            panic!("expected text, got {:?}", deck.slides[0].shapes);
        };
        assert!(!text.title);
        assert_eq!(text.bbox, [0.0, 0.0, 914400.0, 914400.0]);
        assert_eq!(text.paragraphs.len(), 2);
        let paragraph = &text.paragraphs[0];
        assert_eq!(paragraph.text, "Revenue grew 12%");
        assert_eq!(paragraph.runs.len(), 3);
        assert!(paragraph.runs[0].bold);
        assert_eq!(paragraph.runs[0].size_pt, Some(20.0));
        assert!(!paragraph.runs[0].italic);
        assert!(paragraph.runs[1].italic);
        assert!(paragraph.runs[2].underline);
        assert_eq!(paragraph.runs[2].text, "12%");
        assert!(paragraph.list.is_none());
        // The empty paragraph is kept, and carries no text.
        assert_eq!(text.paragraphs[1].text, "");
    }

    #[test]
    fn a_title_placeholder_is_a_title_and_inherits_its_place() {
        // The slide's title has no `a:xfrm`; the layout's placeholder does,
        // which is where a real deck keeps it.
        let slide = r#"<p:cSld><p:spTree>
                 <p:sp><p:nvSpPr><p:cNvPr id="2" name="title"/><p:cNvSpPr/><p:nvPr>
                   <p:ph type="title"/></p:nvPr></p:nvSpPr>
                   <p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>
                   <a:p><a:r><a:t>Q3 Review</a:t></a:r></a:p></p:txBody></p:sp>
               </p:spTree></p:cSld>"#;
        let layout = format!(
            r#"<p:sldLayout {P} {A} {R}><p:cSld><p:spTree>
                 <p:sp><p:nvSpPr><p:cNvPr id="2" name="title"/><p:cNvSpPr/><p:nvPr>
                   <p:ph type="title"/></p:nvPr></p:nvSpPr>
                   <p:spPr>{}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/></p:txBody></p:sp>
               </p:spTree></p:cSld></p:sldLayout>"#,
            transform(457200, 274638, 8229600, 1143000)
        );
        let pkg = package(&[
            (
                "ppt/presentation.xml",
                presentation(r#"<p:sldId id="256" r:id="rId1"/>"#, "9144000"),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>"#
                )),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(r#"<p:sld {P} {A} {R}>{slide}</p:sld>"#),
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="../slideLayouts/slideLayout1.xml"/>"#
                )),
            ),
            ("ppt/slideLayouts/slideLayout1.xml", layout),
        ]);
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let Shape::Text(text) = &deck.slides[0].shapes[0] else {
            panic!("expected text");
        };
        assert!(text.title);
        assert_eq!(
            text.bbox,
            [457200.0, 274638.0, 8686800.0, 1417638.0],
            "the layout's place is the title's place"
        );
        assert_eq!(text.paragraphs[0].text, "Q3 Review");
    }

    #[test]
    fn bullets_come_from_the_master_and_numbers_count_themselves() {
        // The body placeholder writes no bullet anywhere on the slide; the
        // master's `bodyStyle` is what makes these list items, which is how a
        // real deck is put together.
        let slide = r#"<p:cSld><p:spTree>
                 <p:sp><p:nvSpPr><p:cNvPr id="2" name="body"/><p:cNvSpPr/><p:nvPr>
                   <p:ph idx="1"/></p:nvPr></p:nvSpPr>
                   <p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>
                     <a:p><a:r><a:t>First bullet</a:t></a:r></a:p>
                     <a:p><a:pPr lvl="1"/><a:r><a:t>Nested bullet</a:t></a:r></a:p>
                     <a:p><a:r><a:t>Second bullet</a:t></a:r></a:p>
                     <a:p><a:pPr><a:buAutoNum type="arabicPeriod"/></a:pPr><a:r><a:t>Step one</a:t></a:r></a:p>
                     <a:p><a:pPr><a:buNone/></a:pPr><a:r><a:t>A paragraph between the lists.</a:t></a:r></a:p>
                     <a:p><a:pPr><a:buAutoNum type="arabicPeriod" startAt="7"/></a:pPr><a:r><a:t>Step seven</a:t></a:r></a:p>
                   </p:txBody></p:sp>
               </p:spTree></p:cSld>"#;
        let layout = format!(
            r#"<p:sldLayout {P} {A} {R}><p:cSld><p:spTree>
                 <p:sp><p:nvSpPr><p:cNvPr id="2" name="body"/><p:cNvSpPr/><p:nvPr>
                   <p:ph idx="1"/></p:nvPr></p:nvSpPr>
                   <p:spPr>{}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/></p:txBody></p:sp>
               </p:spTree></p:cSld></p:sldLayout>"#,
            transform(457200, 1600200, 8229600, 4525963)
        );
        let master = format!(
            r#"<p:sldMaster {P} {A} {R}><p:cSld><p:spTree>
                 <p:sp><p:nvSpPr><p:cNvPr id="2" name="body"/><p:cNvSpPr/><p:nvPr>
                   <p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>
                   <p:spPr>{}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/></p:txBody></p:sp>
               </p:spTree></p:cSld>
               <p:txStyles><p:titleStyle><a:lvl1pPr><a:buNone/></a:lvl1pPr></p:titleStyle>
                 <p:bodyStyle>
                   <a:lvl1pPr><a:buChar char="•"/></a:lvl1pPr>
                   <a:lvl2pPr><a:buChar char="–"/></a:lvl2pPr>
                 </p:bodyStyle>
                 <p:otherStyle><a:lvl1pPr><a:buNone/></a:lvl1pPr></p:otherStyle>
               </p:txStyles></p:sldMaster>"#,
            transform(457200, 1600200, 8229600, 4525963)
        );
        let pkg = package(&[
            (
                "ppt/presentation.xml",
                presentation(r#"<p:sldId id="256" r:id="rId1"/>"#, "9144000"),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>"#
                )),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(r#"<p:sld {P} {A} {R}>{slide}</p:sld>"#),
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="../slideLayouts/slideLayout1.xml"/>"#
                )),
            ),
            ("ppt/slideLayouts/slideLayout1.xml", layout),
            (
                "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{MASTER}" Target="../slideMasters/slideMaster1.xml"/>"#
                )),
            ),
            ("ppt/slideMasters/slideMaster1.xml", master),
        ]);
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let Shape::Text(text) = &deck.slides[0].shapes[0] else {
            panic!("expected text");
        };
        let listed: Vec<(bool, String, usize)> = text
            .paragraphs
            .iter()
            .map(|paragraph| match &paragraph.list {
                Some(list) => (list.enumerated, list.marker.clone(), paragraph.level),
                None => (false, paragraph.text.clone(), paragraph.level),
            })
            .collect();
        assert_eq!(
            listed,
            vec![
                (false, String::new(), 0),
                (false, String::new(), 1),
                (false, String::new(), 0),
                // A numbered list starts at 1, and continues at its level.
                (true, "1.".to_string(), 0),
                (false, "A paragraph between the lists.".to_string(), 0),
                // An unbulleted paragraph closes the list; the next one starts
                // afresh, and `startAt` is where its first item begins.
                (true, "7.".to_string(), 0),
            ]
        );
    }

    #[test]
    fn a_graphic_frame_table_keeps_its_spans() {
        let table = format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="3" name="table"/>
                 <p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
               <p:xfrm><a:off x="731520" y="1645920"/><a:ext cx="6400800" cy="2194560"/></p:xfrm>
               <a:graphic><a:graphicData uri="{TABLE_URI}"><a:tbl>
                 <a:tblGrid><a:gridCol w="2133600"/><a:gridCol w="2133600"/><a:gridCol w="2133600"/></a:tblGrid>
                 <a:tr><a:tc><a:txBody><a:p><a:r><a:t>Region</a:t></a:r></a:p></a:txBody></a:tc>
                       <a:tc><a:txBody><a:p><a:r><a:t>Q2</a:t></a:r></a:p></a:txBody></a:tc>
                       <a:tc><a:txBody><a:p><a:r><a:t>Q3</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
                 <a:tr><a:tc gridSpan="2"><a:txBody><a:p><a:r><a:t>Total</a:t></a:r></a:p>
                         <a:p><a:r><a:t>218</a:t></a:r></a:p></a:txBody></a:tc>
                       <a:tc hMerge="1"><a:txBody><a:p/></a:txBody></a:tc>
                       <a:tc><a:txBody><a:p><a:r><a:t>266</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
                 <a:tr><a:tc rowSpan="2"><a:txBody><a:p><a:r><a:t>Books</a:t></a:r></a:p></a:txBody></a:tc>
                       <a:tc><a:txBody><a:p><a:r><a:t>1</a:t></a:r></a:p></a:txBody></a:tc>
                       <a:tc><a:txBody><a:p><a:r><a:t>2</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
                 <a:tr><a:tc vMerge="1"><a:txBody><a:p/></a:txBody></a:tc>
                       <a:tc><a:txBody><a:p><a:r><a:t>3</a:t></a:r></a:p></a:txBody></a:tc>
                       <a:tc><a:txBody><a:p><a:r><a:t>4</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
               </a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
        );
        let deck = deck(&format!("<p:cSld><p:spTree>{table}</p:spTree></p:cSld>"));
        let Shape::Table(table) = &deck.slides[0].shapes[0] else {
            panic!("expected a table, got {:?}", deck.slides[0].shapes);
        };
        assert_eq!(table.table.num_rows, 4);
        assert_eq!(table.table.num_cols, 3);
        assert_eq!(table.bbox, [731520.0, 1645920.0, 7132320.0, 3840480.0]);

        let cell = |row: usize, col: usize| {
            table
                .table
                .cells
                .iter()
                .find(|cell| cell.row == row && cell.col == col)
                .unwrap_or_else(|| panic!("no cell at {row},{col}"))
        };
        // The merged cell covers two columns and holds both its lines; the
        // covered slot is not a cell of its own.
        let merged = cell(1, 0);
        assert_eq!(merged.col_span, 2);
        assert_eq!(merged.text, "Total\n218");
        assert!(table
            .table
            .cells
            .iter()
            .all(|cell| !(cell.row == 1 && cell.col == 1)));
        assert_eq!(cell(3, 2).text, "4");
        // A vertical merge reaches down two rows from its opening cell.
        assert_eq!(cell(2, 0).row_span, 2);
        assert_eq!(cell(1, 2).text, "266");
    }

    #[test]
    fn a_picture_is_the_media_part_its_relationship_names() {
        let picture = format!(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="4" name="image"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>
                 <p:blipFill><a:blip r:embed="rId9"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>
                 <p:spPr>{}</p:spPr></p:pic>"#,
            transform(5669280, 1463040, 2194560, 1097280)
        );
        let pkg = package(&[
            (
                "ppt/presentation.xml",
                presentation(r#"<p:sldId id="256" r:id="rId1"/>"#, "9144000"),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>"#
                )),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(
                    r#"<p:sld {P} {A} {R}><p:cSld><p:spTree>{picture}</p:spTree></p:cSld></p:sld>"#
                ),
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId9" Type="{IMAGE}" Target="../media/image7.png"/>"#
                )),
            ),
            ("ppt/media/image7.png", "pixels".to_string()),
            // A picture under a name no relationship names is not the one the
            // slide shows.
            ("ppt/media/image1.png", "another".to_string()),
        ]);
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let Shape::Picture(picture) = &deck.slides[0].shapes[0] else {
            panic!("expected a picture, got {:?}", deck.slides[0].shapes);
        };
        assert_eq!(picture.mime, "image/png");
        assert_eq!(picture.data, b"pixels");
        assert_eq!(picture.bbox, [5669280.0, 1463040.0, 7863840.0, 2560320.0]);
    }

    /// The parts of a one-slide deck whose slide holds `body`, with media parts
    /// of their own.
    fn deck_parts(body: &str, slide_rels: &str) -> Vec<(&'static str, String)> {
        vec![
            (
                "ppt/presentation.xml",
                presentation(r#"<p:sldId id="256" r:id="rId1"/>"#, "9144000"),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>"#
                )),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(
                    r#"<p:sld {P} {A} {R}><p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sld>"#
                ),
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                relationships(slide_rels),
            ),
        ]
    }

    /// A `p:pic` around whatever `a:blip` markup a test needs: the picture's
    /// own `r:embed`, with or without an `asvg:svgBlip` extension beside it.
    fn picture_with(blip: &str) -> String {
        format!(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="4" name="image"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>
                 <p:blipFill>{blip}<a:stretch><a:fillRect/></a:stretch></p:blipFill>
                 <p:spPr>{}</p:spPr></p:pic>"#,
            transform(914400, 914400, 914400, 457200)
        )
    }

    const SVG: &[u8] =
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24"/></svg>"##;

    /// PowerPoint 2016 and later states an SVG picture twice: the `a:blip`'s own
    /// `r:embed` is a PNG fallback for readers that cannot draw SVG, and the
    /// drawing itself is an `asvg:svgBlip` extension beside it. The fallback is
    /// what an old reader shows, not what the file says the picture is, so the
    /// drawing is the one that has to leave the parser.
    #[test]
    fn a_picture_is_read_from_its_svg_blip_not_the_png_beside_it() {
        let blip = r##"<a:blip r:embed="rId9"><a:extLst>
            <a:ext uri="{96DAC541-7B7A-43D3-8B79-37D633B846F1}">
              <asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main"
                            r:embed="rId10"/>
            </a:ext></a:extLst></a:blip>"##;
        let parts = deck_parts(
            &picture_with(blip),
            &format!(
                r#"<Relationship Id="rId9" Type="{IMAGE}" Target="../media/image7.png"/>
                   <Relationship Id="rId10" Type="{IMAGE}" Target="../media/image7.svg"/>"#
            ),
        );
        let pkg = package_with_media(
            &parts,
            &[
                ("ppt/media/image7.png", b"the fallback"),
                ("ppt/media/image7.svg", SVG),
            ],
        );
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let Shape::Picture(picture) = &deck.slides[0].shapes[0] else {
            panic!("expected a picture, got {:?}", deck.slides[0].shapes);
        };
        assert_eq!(picture.mime, "image/svg+xml");
        assert_eq!(picture.data, SVG, "the fallback PNG was taken instead");
    }

    /// A `svgBlip` that does not name a drawing is not a drawing: the fallback
    /// exists for exactly this reader, so it stays the picture.
    #[test]
    fn an_svg_blip_naming_a_raster_leaves_the_fallback_as_the_picture() {
        let blip = r##"<a:blip r:embed="rId9"><a:extLst>
            <a:ext uri="{96DAC541-7B7A-43D3-8B79-37D633B846F1}">
              <asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main"
                            r:embed="rId10"/>
            </a:ext></a:extLst></a:blip>"##;
        let parts = deck_parts(
            &picture_with(blip),
            &format!(
                r#"<Relationship Id="rId9" Type="{IMAGE}" Target="../media/image7.png"/>
                   <Relationship Id="rId10" Type="{IMAGE}" Target="../media/image8.png"/>"#
            ),
        );
        let pkg = package_with_media(
            &parts,
            &[
                ("ppt/media/image7.png", b"the fallback"),
                ("ppt/media/image8.png", b"not the picture"),
            ],
        );
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let Shape::Picture(picture) = &deck.slides[0].shapes[0] else {
            panic!("expected a picture, got {:?}", deck.slides[0].shapes);
        };
        assert_eq!(picture.mime, "image/png");
        assert_eq!(picture.data, b"the fallback");
    }

    /// A picture stated the way Word states one it keeps compatible: the modern
    /// form in `mc:Choice` and a copy an older reader shows in `mc:Fallback`.
    /// Only one of them is a picture of the slide, so the drawing in the choice
    /// is taken and the fallback beside it is not.
    #[test]
    fn an_alternate_content_picture_is_read_once_from_its_choice() {
        let body = format!(
            r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
                 <mc:Choice Requires="p14">{}</mc:Choice>
                 <mc:Fallback>{}</mc:Fallback>
               </mc:AlternateContent>"#,
            picture_with(r#"<a:blip r:embed="rId10"/>"#),
            picture_with(r#"<a:blip r:embed="rId9"/>"#),
        );
        let parts = deck_parts(
            &body,
            &format!(
                r#"<Relationship Id="rId9" Type="{IMAGE}" Target="../media/image7.png"/>
                   <Relationship Id="rId10" Type="{IMAGE}" Target="../media/image7.svg"/>"#
            ),
        );
        let pkg = package_with_media(
            &parts,
            &[
                ("ppt/media/image7.png", b"the fallback"),
                ("ppt/media/image7.svg", SVG),
            ],
        );
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        assert_eq!(
            deck.slides[0].shapes.len(),
            1,
            "the choice and its fallback are not two pictures: {:#?}",
            deck.slides[0].shapes
        );
        let Shape::Picture(picture) = &deck.slides[0].shapes[0] else {
            panic!("expected a picture, got {:?}", deck.slides[0].shapes);
        };
        assert_eq!(picture.mime, "image/svg+xml");
        assert_eq!(picture.data, SVG, "the fallback was taken instead");
    }

    /// The fallback is not decoration: when the choice names a part the package
    /// does not hold, the copy an older reader would show is the slide's picture.
    #[test]
    fn an_alternate_content_picture_falls_back_when_its_choice_has_no_part() {
        let body = format!(
            r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
                 <mc:Choice Requires="p14">{}</mc:Choice>
                 <mc:Fallback>{}</mc:Fallback>
               </mc:AlternateContent>"#,
            picture_with(r#"<a:blip r:embed="rId11"/>"#),
            picture_with(r#"<a:blip r:embed="rId9"/>"#),
        );
        let parts = deck_parts(
            &body,
            &format!(
                r#"<Relationship Id="rId9" Type="{IMAGE}" Target="../media/image7.png"/>
                   <Relationship Id="rId11" Type="{IMAGE}" Target="../media/missing.svg"/>"#
            ),
        );
        let pkg = package_with_media(&parts, &[("ppt/media/image7.png", b"the fallback")]);
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let Shape::Picture(picture) = &deck.slides[0].shapes[0] else {
            panic!("expected a picture, got {:?}", deck.slides[0].shapes);
        };
        assert_eq!(picture.mime, "image/png");
        assert_eq!(picture.data, b"the fallback");
    }

    /// One colour, `size` pixels, as the PNG a fill's media is.
    fn solid_png(size: (u32, u32), color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(size.0, size.1, image::Rgba(color));
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("the fill encodes");
        png
    }

    /// A shape can be filled with a picture rather than a colour, and the fill
    /// states that picture the way a picture states itself: the raster on the
    /// `a:blip`, the drawing in an `asvg:svgBlip` extension beside it. The fill
    /// is the shape's, so it is painted through the shape's geometry — the
    /// drawing is shown inside a rounded rectangle, not as the rectangle's
    /// bounding picture — with the shape's words still the slide's text.
    #[test]
    fn a_shape_filled_with_a_picture_is_painted_through_the_shape() {
        let filled = text_shape(
            &format!(
                r##"{}<a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom>
                   <a:blipFill><a:blip r:embed="rId9"><a:extLst>
                     <a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}">
                       <asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main"
                                     r:embed="rId10"/>
                     </a:ext></a:extLst></a:blip><a:stretch><a:fillRect/></a:stretch></a:blipFill>
                   <a:ln><a:noFill/></a:ln>"##,
                transform(914400, 914400, 914400, 457200)
            ),
            r#"<a:p><a:r><a:t>Inside the fill.</a:t></a:r></a:p>"#,
        );
        let drawing = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24"><rect width="40" height="24" fill="#ff0000"/></svg>"##;
        let parts = deck_parts(
            &filled,
            &format!(
                r#"<Relationship Id="rId9" Type="{IMAGE}" Target="../media/image7.png"/>
                   <Relationship Id="rId10" Type="{IMAGE}" Target="../media/image7.svg"/>"#
            ),
        );
        let pkg = package_with_media(
            &parts,
            &[
                ("ppt/media/image7.png", b"the fill's raster"),
                ("ppt/media/image7.svg", drawing),
            ],
        );
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let pictures: Vec<&PictureShape> = deck.slides[0]
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Picture(picture) => Some(picture),
                _ => None,
            })
            .collect();
        assert_eq!(
            pictures.len(),
            1,
            "the fill is one picture, and the shape's own outline is not a second: {:#?}",
            deck.slides[0].shapes
        );
        // A painted shape has no source encoding: it is the PNG the shape was
        // drawn into.
        assert_eq!(pictures[0].mime, "image/png");
        assert_eq!(
            pictures[0].bbox,
            [
                914_400.0,
                914_400.0,
                914_400.0 + 914_400.0,
                914_400.0 + 457_200.0
            ]
        );
        let painted = image::load_from_memory(&pictures[0].data)
            .expect("the painted shape decodes")
            .to_rgba8();
        // 72 × 36 points at 144 dpi.
        assert_eq!(painted.dimensions(), (144, 72));
        // The middle is the drawing the fill names, not the raster beside it.
        assert_eq!(
            painted.get_pixel(72, 36).0,
            [255, 0, 0, 255],
            "the fill is not the drawing its svgBlip names"
        );
        // The shape is a rounded rectangle, and the fill does not reach its
        // corner: the picture was drawn through the geometry, not as a box.
        assert_eq!(
            painted.get_pixel(1, 1).0,
            [255, 255, 255, 255],
            "the fill was laid down as a box instead of through the shape"
        );

        // The shape's words are still the slide's text.
        let texts: Vec<&str> = deck.slides[0]
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Text(text) => text.paragraphs.first().map(|p| p.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Inside the fill."]);
    }

    /// A paragraph's marker can be a picture: `a:buBlip` names the media the
    /// bullet is drawn with, the way a picture names its part, and an SVG in it
    /// is stated as Word and PowerPoint state one — the raster on the `a:blip`,
    /// the drawing in an `asvg:svgBlip` extension beside it. No run carries the
    /// marker and the painter draws no media, so each such paragraph's bullet
    /// leaves as the picture it is: a square of the item's type size at the
    /// corner the text starts from, with the words still the slide's text.
    #[test]
    fn a_picture_bullet_is_the_picture_it_prints() {
        let paragraphs = r##"<a:p><a:pPr><a:buBlip><a:blip r:embed="rId9"><a:extLst>
              <a:ext uri="{96DAC541-7B7A-43D3-8B79-37D633B846F1}">
                <asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main"
                              r:embed="rId10"/>
              </a:ext></a:extLst></a:blip></a:buBlip></a:pPr>
            <a:r><a:rPr sz="2400"/><a:t>Drawn bullet item</a:t></a:r></a:p>
            <a:p><a:pPr><a:buBlip><a:blip r:embed="rId11"/></a:buBlip></a:pPr>
            <a:r><a:t>Painted bullet item</a:t></a:r></a:p>"##;
        let shape = text_shape(&transform(457200, 228600, 3657600, 914400), paragraphs);
        let parts = deck_parts(
            &shape,
            &format!(
                r#"<Relationship Id="rId9" Type="{IMAGE}" Target="../media/bullet.png"/>
                   <Relationship Id="rId10" Type="{IMAGE}" Target="../media/bullet.svg"/>
                   <Relationship Id="rId11" Type="{IMAGE}" Target="../media/star.png"/>"#
            ),
        );
        let pkg = package_with_media(
            &parts,
            &[
                ("ppt/media/bullet.png", b"the fallback"),
                ("ppt/media/bullet.svg", SVG),
                ("ppt/media/star.png", b"the raster"),
            ],
        );
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let pictures: Vec<&PictureShape> = deck.slides[0]
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Picture(picture) => Some(picture),
                _ => None,
            })
            .collect();
        assert_eq!(pictures.len(), 2, "{:#?}", deck.slides[0].shapes);

        // The drawing wins over the raster fallback beside it, at the 24-point
        // type size the item is set in.
        assert_eq!(pictures[0].mime, "image/svg+xml");
        assert_eq!(
            pictures[0].data, SVG,
            "the bullet's raster fallback was taken instead of its drawing"
        );
        let side = 24.0 * EMU_PER_POINT;
        assert_eq!(
            pictures[0].bbox,
            [457_200.0, 228_600.0, 457_200.0 + side, 228_600.0 + side]
        );

        // A bullet that states no type size is drawn at the body default, and
        // its raster is kept as stored.
        assert_eq!(pictures[1].mime, "image/png");
        assert_eq!(pictures[1].data, b"the raster");
        let side = DEFAULT_TEXT_PT * EMU_PER_POINT;
        assert_eq!(
            pictures[1].bbox,
            [457_200.0, 228_600.0, 457_200.0 + side, 228_600.0 + side]
        );

        // The words are still the slide's text, and both items read as list
        // items whose marker is the picture rather than a glyph.
        let items: Vec<(&str, Option<&str>)> = deck.slides[0]
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Text(text) => Some(
                    text.paragraphs
                        .iter()
                        .map(|paragraph| {
                            (
                                paragraph.text.as_str(),
                                paragraph.list.as_ref().map(|list| list.marker.as_str()),
                            )
                        })
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            items,
            vec![
                ("Drawn bullet item", Some("")),
                ("Painted bullet item", Some(""))
            ]
        );
    }

    /// The fill a shape states with no drawing is the raster it names, painted
    /// into the shape at the size the slide places it: not every filled shape is
    /// an SVG, and the raster path is the one most decks take.
    #[test]
    fn a_shape_filled_with_a_raster_is_painted_at_the_shapes_size() {
        let filled = text_shape(
            &format!(
                r#"{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                   <a:blipFill><a:blip r:embed="rId9"/><a:stretch><a:fillRect/></a:stretch></a:blipFill>
                   <a:ln><a:noFill/></a:ln>"#,
                transform(0, 0, 762000, 457200)
            ),
            "",
        );
        let parts = deck_parts(
            &filled,
            &format!(r#"<Relationship Id="rId9" Type="{IMAGE}" Target="../media/image7.png"/>"#),
        );
        let orange = solid_png((30, 18), [224, 140, 40, 255]);
        let pkg = package_with_media(&parts, &[("ppt/media/image7.png", &orange)]);
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let Shape::Picture(picture) = &deck.slides[0].shapes[0] else {
            panic!("expected a picture, got {:?}", deck.slides[0].shapes);
        };
        assert_eq!(picture.mime, "image/png");
        assert_eq!(
            picture.bbox,
            [0.0, 0.0, 762_000.0, 457_200.0],
            "the picture is the shape's own frame"
        );
        let painted = image::load_from_memory(&picture.data)
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

    /// A picture the camera stored sideways, with only its EXIF tag saying so.
    /// PowerPoint displays it upright; the parser has to hand on the picture the
    /// slide showed, not the pixels the package holds.
    #[test]
    fn an_exif_tagged_picture_is_turned_upright() {
        let sideways = std::fs::read("../../test-data/photos/06_rotated.jpg")
            .expect("the photographed corpus is generated");
        assert!(
            sideways.windows(6).any(|window| window == b"Exif\0\0"),
            "the fixture must carry the tag, or the turn is not what is tested"
        );
        let picture = format!(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="4" name="image"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>
                 <p:blipFill><a:blip r:embed="rId9"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>
                 <p:spPr>{}</p:spPr></p:pic>"#,
            transform(914400, 914400, 3200400, 4526280)
        );
        let pkg = package_with_media(
            &[
                (
                    "ppt/presentation.xml",
                    presentation(r#"<p:sldId id="256" r:id="rId1"/>"#, "9144000"),
                ),
                (
                    "ppt/_rels/presentation.xml.rels",
                    relationships(&format!(
                        r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>"#
                    )),
                ),
                (
                    "ppt/slides/slide1.xml",
                    format!(
                        r#"<p:sld {P} {A} {R}><p:cSld><p:spTree>{picture}</p:spTree></p:cSld></p:sld>"#
                    ),
                ),
                (
                    "ppt/slides/_rels/slide1.xml.rels",
                    relationships(&format!(
                        r#"<Relationship Id="rId9" Type="{IMAGE}" Target="../media/image7.jpg"/>"#
                    )),
                ),
            ],
            &[("ppt/media/image7.jpg", &sideways)],
        );
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let Shape::Picture(picture) = &deck.slides[0].shapes[0] else {
            panic!("expected a picture, got {:?}", deck.slides[0].shapes);
        };

        assert_eq!(picture.mime, "image/jpeg", "a JPEG stays a JPEG");
        assert!(!picture.data.windows(6).any(|window| window == b"Exif\0\0"));
        let decoded = image::load_from_memory(&picture.data).expect("the turned picture decodes");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (1240, 1754),
            "the stored 1754 x 1240 grid is turned"
        );
    }

    /// Every picture the extractor carries is named out of a deck's package by
    /// the shared list, in both of its names and in any case: the table is the
    /// one place a media type is decided, so a format added there is read out
    /// of a package without another edit here.
    #[test]
    fn every_carried_picture_format_is_named_from_a_slide_part() {
        for format in pdfextractor_picture_formats::PICTURE_FORMATS {
            let names = std::iter::once(format.extension).chain(format.read_as.iter().copied());
            for extension in names {
                let part = format!("ppt/media/image7.{extension}");
                assert_eq!(mime_for(&part), Some(format.mime), "{part}");
                let shouted = format!("ppt/media/image7.{}", extension.to_ascii_uppercase());
                assert_eq!(mime_for(&shouted), Some(format.mime), "{shouted}");
            }
        }
        // A part the list does not carry names nothing, and neither does one
        // with no extension to name it by.
        assert_eq!(mime_for("ppt/media/image7.avif"), None);
        assert_eq!(mime_for("ppt/media/image7"), None);
    }

    #[test]
    fn a_chart_is_its_data_rather_than_a_picture() {
        // A graphic frame says its payload is a chart and names the part; the
        // part holds the title, the categories and the series — nothing is
        // drawn on the slide to read.
        let chart = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="chart"/>
                 <p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
               <p:xfrm><a:off x="914400" y="914400"/><a:ext cx="7315200" cy="3657600"/></p:xfrm>
               <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">
                 <c:chart r:id="rId5"/></a:graphicData></a:graphic></p:graphicFrame>"#;
        // The series are written back to front: the plot draws by `c:order`,
        // not by where the element sits. The category cache declares four rows
        // and writes three; the values declare four points each and leave the
        // tail out — those are gaps, not zeros.
        let part = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"
                                  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
             <c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>Units by region</a:t></a:r></a:p>
             </c:rich></c:tx></c:title>
             <c:plotArea><c:layout/><c:barChart><c:barDir val="col"/>
               <c:ser><c:idx val="1"/><c:order val="1"/>
                 <c:tx><c:strRef><c:f>Sheet1!$C$1</c:f><c:strCache><c:ptCount val="1"/>
                   <c:pt idx="0"><c:v>Q3</c:v></c:pt></c:strCache></c:strRef></c:tx>
                 <c:cat><c:strRef><c:f>Sheet1!$A$2:$A$5</c:f><c:strCache><c:ptCount val="4"/>
                   <c:pt idx="0"><c:v>North America</c:v></c:pt>
                   <c:pt idx="1"><c:v>Europe</c:v></c:pt>
                   <c:pt idx="2"><c:v>Asia Pacific</c:v></c:pt></c:strCache></c:strRef></c:cat>
                 <c:val><c:numRef><c:f>Sheet1!$C$2:$C$5</c:f><c:numCache><c:formatCode>General</c:formatCode>
                   <c:ptCount val="4"/><c:pt idx="0"><c:v>145</c:v></c:pt>
                   <c:pt idx="1"><c:v>121</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser>
               <c:ser><c:idx val="0"/><c:order val="0"/>
                 <c:tx><c:strRef><c:f>Sheet1!$B$1</c:f><c:strCache><c:ptCount val="1"/>
                   <c:pt idx="0"><c:v>Q2</c:v></c:pt></c:strCache></c:strRef></c:tx>
                 <c:cat><c:strRef><c:f>Sheet1!$A$2:$A$5</c:f><c:strCache><c:ptCount val="4"/></c:strCache></c:strRef></c:cat>
                 <c:val><c:numRef><c:f>Sheet1!$B$2:$B$5</c:f><c:numCache><c:formatCode>General</c:formatCode>
                   <c:ptCount val="4"/><c:pt idx="0"><c:v>120</c:v></c:pt>
                   <c:pt idx="1"><c:v>98</c:v></c:pt>
                   <c:pt idx="2"><c:v>74</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser>
               <c:axId val="1"/><c:axId val="2"/></c:barChart>
               <c:catAx><c:axId val="1"/></c:catAx><c:valAx><c:axId val="2"/></c:valAx>
             </c:plotArea></c:chart></c:chartSpace>"#;
        let pkg = package(&[
            (
                "ppt/presentation.xml",
                presentation(r#"<p:sldId id="256" r:id="rId1"/>"#, "9144000"),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>"#
                )),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(
                    r#"<p:sld {P} {A} {R}><p:cSld><p:spTree>{chart}</p:spTree></p:cSld></p:sld>"#
                ),
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId5" Type="{CHART}" Target="../charts/chart1.xml"/>"#
                )),
            ),
            ("ppt/charts/chart1.xml", part.to_string()),
        ]);
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        let Shape::Chart(chart) = &deck.slides[0].shapes[0] else {
            panic!("expected a chart, got {:?}", deck.slides[0].shapes);
        };
        assert_eq!(chart.bbox, [914400.0, 914400.0, 8229600.0, 4572000.0]);
        assert_eq!(chart.title.as_deref(), Some("Units by region"));
        assert_eq!(chart.kind.class_name(), "bar_chart");
        assert_eq!(
            chart.categories,
            vec!["North America", "Europe", "Asia Pacific", ""]
        );
        let names: Vec<&str> = chart.series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Q2", "Q3"]);
        assert_eq!(
            chart.series[0].values,
            vec![Some(120.0), Some(98.0), Some(74.0), None]
        );
        // The gap between the plotted points and the absent tail are both
        // gaps: not a zero.
        assert_eq!(
            chart.series[1].values,
            vec![Some(145.0), Some(121.0), None, None]
        );
    }

    #[test]
    fn a_chart_is_named_by_its_family() {
        // Docling classifies the type python-pptx reports for the first plot;
        // every family it has no name of its own for is an `other_chart`.
        let cases = [
            ("barChart", "bar_chart"),
            ("bar3DChart", "bar_chart"),
            ("lineChart", "line_chart"),
            ("line3DChart", "line_chart"),
            ("pieChart", "pie_chart"),
            ("doughnutChart", "pie_chart"),
            ("scatterChart", "scatter_chart"),
            ("areaChart", "other_chart"),
        ];
        for (plot, name) in cases {
            let part = format!(
                r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
                     <c:chart><c:plotArea><c:{plot}/></c:plotArea></c:chart></c:chartSpace>"#
            );
            let chart = pdfextractor_chart::read_chart(part.as_bytes()).expect(plot);
            assert_eq!(chart.kind.class_name(), name, "{plot}");
        }
    }

    #[test]
    fn shapes_are_read_top_to_bottom_then_left_to_right() {
        // Creation order is bottom shape first; reading order is not.
        let bottom = text_shape(
            &transform(0, 4000000, 914400, 457200),
            r#"<a:p><a:r><a:t>bottom</a:t></a:r></a:p>"#,
        );
        let right = text_shape(
            &transform(3000000, 1000000, 914400, 457200),
            r#"<a:p><a:r><a:t>right</a:t></a:r></a:p>"#,
        );
        let left = text_shape(
            &transform(457200, 1000000, 914400, 457200),
            r#"<a:p><a:r><a:t>left</a:t></a:r></a:p>"#,
        );
        // 20,000 EMU below `left`: within the row tolerance, so one line.
        let nudged = text_shape(
            &transform(2000000, 1020000, 914400, 457200),
            r#"<a:p><a:r><a:t>middle</a:t></a:r></a:p>"#,
        );
        let deck = deck(&format!(
            "<p:cSld><p:spTree>{bottom}{right}{nudged}{left}</p:spTree></p:cSld>"
        ));
        let texts: Vec<&str> = deck.slides[0]
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Text(text) => Some(text.paragraphs[0].text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["left", "middle", "right", "bottom"]);
    }

    #[test]
    fn a_hidden_slide_is_kept_and_marked() {
        let pkg = package(&[
            (
                "ppt/presentation.xml",
                presentation(r#"<p:sldId id="256" r:id="rId1"/>"#, "9144000"),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>"#
                )),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(
                    r#"<p:sld {P} {A} {R} show="0"><p:cSld><p:spTree></p:spTree></p:cSld></p:sld>"#
                ),
            ),
        ]);
        let deck = PptxDocument::parse_bytes(&pkg).expect("parse");
        assert!(deck.slides[0].hidden);
    }

    #[test]
    fn a_group_is_walked_and_a_connector_with_no_ink_is_nothing() {
        let line = format!(
            r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="9" name="line"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr>
                 <p:spPr>{}</p:spPr></p:cxnSp>"#,
            transform(0, 0, 100, 100)
        );
        let grouped = format!(
            r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="3" name="group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
                 <p:grpSpPr>{}</p:grpSpPr>
                 {}</p:grpSp>"#,
            transform(0, 0, 5000000, 5000000),
            text_shape(
                &transform(1000000, 1000000, 914400, 457200),
                r#"<a:p><a:r><a:t>in the group</a:t></a:r></a:p>"#
            )
        );
        let deck = deck(&format!(
            "<p:cSld><p:spTree>{line}{grouped}</p:spTree></p:cSld>"
        ));
        assert_eq!(deck.slides[0].shapes.len(), 1);
        let Shape::Text(text) = &deck.slides[0].shapes[0] else {
            panic!("expected text");
        };
        assert_eq!(text.paragraphs[0].text, "in the group");
    }

    #[test]
    fn a_package_without_a_presentation_is_an_error() {
        let pkg = package(&[("word/document.xml", "<w:document/>".to_string())]);
        let error = PptxDocument::parse_bytes(&pkg).expect_err("no presentation part");
        assert!(
            error.to_string().contains("ppt/presentation.xml"),
            "unclear error: {error}"
        );
    }

    const NOTES_SLIDE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide";
    const COMMENTS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
    const COMMENT_AUTHORS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/commentAuthors";

    /// The notes slide of a one-slide deck, with `body` as its note text.
    fn notes_part(body: &str) -> String {
        format!(
            r#"<p:notes {P} {A}><p:cSld><p:spTree>
                 <p:sp><p:nvSpPr><p:cNvPr id="1" name="Slide Number Placeholder"/>
                   <p:cNvSpPr/><p:nvPr><p:ph type="sldNum" sz="quarter" idx="10"/></p:nvPr>
                   </p:nvSpPr><p:txBody><a:bodyPr/><a:lstStyle/>
                   <a:fld id="1" type="slidenum"><a:t>1</a:t></a:fld></p:txBody></p:sp>
                 <p:sp><p:nvSpPr><p:cNvPr id="2" name="Notes Placeholder"/>
                   <p:cNvSpPr/><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>
                   <p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>
               </p:spTree></p:cSld></p:notes>"#
        )
    }

    /// A one-slide deck with extra slide parts (a notes slide, comments) and
    /// extra presentation parts (the comment authors).
    fn deck_with(
        slide_parts: &[(&str, String)],
        slide_rels: &str,
        presentation_parts: &[(&str, String)],
        presentation_rels: &str,
    ) -> PptxDocument {
        let mut parts: Vec<(&str, String)> = vec![
            (
                "ppt/presentation.xml",
                presentation(r#"<p:sldId id="256" r:id="rId1"/>"#, "9144000"),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                relationships(&format!(
                    r#"<Relationship Id="rId1" Type="{LAYOUT}" Target="slides/slide1.xml"/>{presentation_rels}"#
                )),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(r#"<p:sld {P} {A} {R}></p:sld>"#),
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                relationships(slide_rels),
            ),
        ];
        parts.extend(slide_parts.iter().map(|(n, b)| (*n, b.clone())));
        parts.extend(presentation_parts.iter().map(|(n, b)| (*n, b.clone())));
        PptxDocument::parse_bytes(&package(&parts)).expect("parse")
    }

    #[test]
    fn speaker_notes_read_the_body_placeholder_like_a_text_frame() {
        let deck = deck_with(
            &[(
                "ppt/notesSlides/notesSlide1.xml",
                notes_part(
                    r#"<a:p><a:r><a:t>Ask about </a:t></a:r><a:r><a:t>the numbers</a:t></a:r>
                         <a:br/><a:r><a:t>and the split</a:t></a:r></a:p>
                       <a:p><a:r><a:t>Second paragraph</a:t></a:r></a:p>"#,
                ),
            )],
            &format!(
                r#"<Relationship Id="rIdNotes" Type="{NOTES_SLIDE}" Target="../notesSlides/notesSlide1.xml"/>"#
            ),
            &[],
            "",
        );
        // The slide-number field is not a note: only the body placeholder is.
        // A soft break is the vertical tab python-pptx reads, and the two
        // paragraphs are one newline apart.
        assert_eq!(
            deck.slides[0].notes.as_deref(),
            Some("Ask about the numbers\u{b}and the split\nSecond paragraph")
        );
        assert!(deck.slides[0].comments.is_empty());
    }

    #[test]
    fn notes_that_say_nothing_are_no_notes() {
        let deck = deck_with(
            &[(
                "ppt/notesSlides/notesSlide1.xml",
                notes_part(r#"<a:p><a:endParaRPr/></a:p>"#),
            )],
            &format!(
                r#"<Relationship Id="rIdNotes" Type="{NOTES_SLIDE}" Target="../notesSlides/notesSlide1.xml"/>"#
            ),
            &[],
            "",
        );
        assert_eq!(deck.slides[0].notes, None);
    }

    #[test]
    fn comments_keep_their_author_time_and_their_part_order() {
        let deck = deck_with(
            &[(
                "ppt/comments/comment1.xml",
                format!(
                    r#"<p:cmLst {P}>
                         <p:cm authorId="0" dt="2026-10-18T09:15:00.000" idx="1">
                           <p:pos x="10" y="10"/><p:text>Check the numbers. </p:text></p:cm>
                         <p:cm authorId="7" dt="2026-10-18T10:00:00.000">
                           <p:pos x="10" y="10"/><p:text>No author in the map.</p:text></p:cm>
                         <p:cm authorId="1" idx="4">
                           <p:pos x="10" y="10"/><p:text>   </p:text></p:cm>
                       </p:cmLst>"#
                ),
            )],
            &format!(
                r#"<Relationship Id="rIdComments" Type="{COMMENTS}" Target="../comments/comment1.xml"/>"#
            ),
            &[(
                "ppt/commentAuthors.xml",
                format!(
                    r#"<p:cmAuthorLst {P}>
                         <p:cmAuthor id="0" name="Ada Lovelace" initials="AL" lastIdx="2" clrIdx="0"/>
                         <p:cmAuthor id="1" name="Grace Hopper" initials="GH" lastIdx="1" clrIdx="1"/>
                       </p:cmAuthorLst>"#
                ),
            )],
            &format!(
                r#"<Relationship Id="rIdAuthors" Type="{COMMENT_AUTHORS}" Target="commentAuthors.xml"/>"#
            ),
        );
        let comments = &deck.slides[0].comments;
        // The empty comment is not content; the other two keep their order.
        assert_eq!(comments.len(), 2, "{comments:?}");
        assert_eq!(comments[0].index, "1");
        assert_eq!(comments[0].author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(comments[0].initials.as_deref(), Some("AL"));
        assert_eq!(comments[0].date.as_deref(), Some("2026-10-18T09:15:00.000"));
        assert_eq!(comments[0].text, "Check the numbers.");
        // A comment whose id the authors part does not name has no author, and
        // one that writes no id is read with the slide's own index.
        assert_eq!(comments[1].author, None);
        assert!(comments[1].initials.is_none());
        assert_eq!(comments[1].index, "0");
        assert_eq!(comments[1].date.as_deref(), Some("2026-10-18T10:00:00.000"));
    }

    #[test]
    fn relationship_targets_resolve_against_their_own_part() {
        assert_eq!(
            resolve_target("ppt/slides/slide1.xml", "../media/image1.png"),
            "ppt/media/image1.png"
        );
        assert_eq!(
            resolve_target("ppt/slides/slide1.xml", "/ppt/media/image2.png"),
            "ppt/media/image2.png"
        );
        assert_eq!(
            rels_path("ppt/slides/slide1.xml"),
            "ppt/slides/_rels/slide1.xml.rels"
        );
        assert_eq!(
            resolve_target("ppt/presentation.xml", "slides/slide1.xml"),
            "ppt/slides/slide1.xml"
        );
    }
}
