//! Reading an OpenDocument presentation into its slides and shapes.

use anyhow::{Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::{Cursor, Read};

use pdfextractor_picture_formats::{format_for_extension, format_for_mime};

/// Slide size when no page layout states one: 10 × 7.5 inches in EMU,
/// PowerPoint's 4:3 default, which is also LibreOffice's common 25.4 × 19.05 cm
/// presentation size.
const DEFAULT_WIDTH: f64 = 9_144_000.0;
const DEFAULT_HEIGHT: f64 = 6_858_000.0;
const EMU_PER_POINT: f64 = 12_700.0;

/// A repeated row or cell above this is filler, not content.
const MAX_REPEAT: usize = 1024;

/// A parsed presentation.
#[derive(Debug, Clone)]
pub struct OdpDocument {
    /// Slide width and height in EMU, from the page layout.
    pub width: f64,
    pub height: f64,
    /// The slides in presentation order; a slide is a page.
    pub slides: Vec<Slide>,
}

/// One slide: its page number, whether it is hidden, its shapes in document
/// order, and its speaker notes.
#[derive(Debug, Clone)]
pub struct Slide {
    /// 1-based, the page number a reader gives the slide.
    pub number: i32,
    /// `draw:page/@show="false"` — Impress' hidden-slide flag. The slide is
    /// kept and handed to the `invisible` layer.
    pub hidden: bool,
    pub shapes: Vec<Shape>,
    /// The text of the slide's `presentation:notes` frame, paragraphs joined by
    /// a newline and trimmed. `None` when the slide has no notes, or they are
    /// empty.
    pub notes: Option<String>,
}

/// One shape of a slide.
#[derive(Debug, Clone)]
pub enum Shape {
    Text(TextShape),
    Table(TableShape),
    Picture(PictureShape),
}

/// A text box, title, outline or custom shape with paragraphs.
#[derive(Debug, Clone)]
pub struct TextShape {
    /// `[l, t, r, b]` in EMU from the slide's top-left.
    pub bbox: [f64; 4],
    /// Whether the shape is a title placeholder.
    pub title: bool,
    pub paragraphs: Vec<Paragraph>,
}

/// One paragraph of a text shape.
#[derive(Debug, Clone)]
pub struct Paragraph {
    pub text: String,
    /// `Some(level)` for a list item, `None` for body text.
    pub level: Option<i32>,
    /// Whether the paragraph came from a `text:list-item`.
    pub list: bool,
}

/// A table on a slide.
#[derive(Debug, Clone)]
pub struct TableShape {
    pub bbox: [f64; 4],
    pub table: Table,
}

/// A table's grid and its cells that are not covered by a merge.
#[derive(Debug, Clone)]
pub struct Table {
    pub num_rows: usize,
    pub num_cols: usize,
    pub cells: Vec<TableCell>,
}

/// One cell of a slide table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub text: String,
}

/// A `draw:image` and the media part its `xlink:href` names.
#[derive(Debug, Clone)]
pub struct PictureShape {
    pub bbox: [f64; 4],
    pub mime: &'static str,
    pub data: Vec<u8>,
}

impl OdpDocument {
    pub fn parse_file(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::parse_bytes(&data)
    }

    pub fn parse_bytes(data: &[u8]) -> Result<Self> {
        let mut archive = zip::ZipArchive::new(Cursor::new(data))
            .map_err(|e| anyhow::anyhow!("not an OpenDocument (ZIP) package: {e}"))?;

        let content = read_part(&mut archive, "content.xml")
            .ok_or_else(|| anyhow::anyhow!("package has no content.xml"))?;
        let styles = read_part(&mut archive, "styles.xml");
        let layouts = styles
            .as_deref()
            .map(parse_page_layouts)
            .transpose()?
            .unwrap_or_default();
        let media = read_parts(&mut archive, "Pictures/");

        let root = Node::parse(&content).context("malformed content.xml")?;
        let presentation = root
            .find("presentation")
            .context("content.xml has no office:presentation")?;

        let mut slides = Vec::new();
        let mut page_size = None;
        for (index, page) in presentation.children_named("page").enumerate() {
            let size = page_size_for(page, &layouts);
            page_size.get_or_insert(size);

            let mut shapes = Vec::new();
            for child in &page.children {
                match child.name.as_str() {
                    "frame" | "custom-shape" | "caption" | "g" => {
                        collect_shape(child, &media, &mut shapes)
                    }
                    "table" => shapes.push(Shape::Table(TableShape {
                        bbox: bbox(page),
                        table: parse_table(child),
                    })),
                    "image" => {
                        if let Some(picture) = picture(child, bbox(page), &media) {
                            shapes.push(Shape::Picture(picture));
                        }
                    }
                    _ => {}
                }
            }

            slides.push(Slide {
                number: (index + 1) as i32,
                hidden: is_hidden(page),
                shapes,
                notes: read_notes(page),
            });
        }

        let (width, height) = page_size.unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT));
        Ok(Self {
            width,
            height,
            slides,
        })
    }
}

fn read_part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut file = archive.by_name(name).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

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
        if file.is_dir() || !name.starts_with(prefix) {
            continue;
        }
        let mut buffer = Vec::new();
        if file.read_to_end(&mut buffer).is_ok() {
            parts.insert(name, buffer);
        }
    }
    parts
}

/// The page size a slide's master page names, in EMU.
fn page_size_for(page: &Node, layouts: &HashMap<String, (f64, f64)>) -> (f64, f64) {
    let layout = page
        .attr("master-page-name")
        .and_then(|name| layouts.get(name))
        .or_else(|| {
            page.attr("page-layout-name")
                .and_then(|name| layouts.get(name))
        })
        .or_else(|| layouts.get("__default__"));
    layout.copied().unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT))
}

/// Read `style:page-layout` rectangles and `style:master-page` references from
/// `styles.xml`.
fn parse_page_layouts(xml: &[u8]) -> Result<HashMap<String, (f64, f64)>> {
    let root = Node::parse(xml).context("malformed styles.xml")?;
    let mut layouts = HashMap::new();
    let mut first = None;
    for layout in root.descendants_named("page-layout") {
        let Some(name) = layout.attr("name") else {
            continue;
        };
        let Some(properties) = layout.child("page-layout-properties") else {
            continue;
        };
        let Some(width) = properties.attr("page-width").and_then(length_to_emu) else {
            continue;
        };
        let Some(height) = properties.attr("page-height").and_then(length_to_emu) else {
            continue;
        };
        if width > 0.0 && height > 0.0 {
            first.get_or_insert((width, height));
            layouts.insert(name.to_string(), (width, height));
        }
    }

    for master in root.descendants_named("master-page") {
        let (Some(name), Some(layout)) = (master.attr("name"), master.attr("page-layout-name"))
        else {
            continue;
        };
        if let Some(size) = layouts.get(layout).copied() {
            layouts.insert(name.to_string(), size);
        }
    }

    // A page that names only a `draw:style-name` has no master reference to
    // follow. The first real page layout is the best statement left.
    if let Some(size) = first {
        layouts.insert("__default__".to_string(), size);
    }
    Ok(layouts)
}

fn length_to_emu(value: &str) -> Option<f64> {
    let value = value.trim();
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.trim().parse().ok()?;
    let points = match unit.trim().to_ascii_lowercase().as_str() {
        "cm" => number * 28.346_456_692_913_4,
        "mm" => number * 2.834_645_669_291_34,
        "in" => number * 72.0,
        "pc" => number * 12.0,
        "px" => number * 0.75,
        "" | "pt" => number,
        _ => return None,
    };
    // An EMU is the smallest unit the model records, so a length in cm is
    // rounded to it rather than carried as a repeating fraction of one.
    Some((points * EMU_PER_POINT).round())
}

fn bbox(node: &Node) -> [f64; 4] {
    let value = |name: &str| node.attr(name).and_then(length_to_emu).unwrap_or(0.0);
    let x = value("x");
    let y = value("y");
    [x, y, x + value("width"), y + value("height")]
}

fn is_hidden(page: &Node) -> bool {
    page.attr("show")
        .is_some_and(|show| show.eq_ignore_ascii_case("false") || show == "0")
}

fn collect_shape(node: &Node, media: &HashMap<String, Vec<u8>>, shapes: &mut Vec<Shape>) {
    let bbox = bbox(node);
    let title = matches!(node.attr("class"), Some("title" | "ctrTitle"));

    let mut paragraphs = Vec::new();
    if let Some(text_box) = node.child("text-box") {
        collect_paragraphs(text_box, &mut paragraphs);
    } else {
        collect_paragraphs(node, &mut paragraphs);
    }
    if !paragraphs.is_empty() {
        shapes.push(Shape::Text(TextShape {
            bbox,
            title,
            paragraphs,
        }));
    }

    for child in &node.children {
        match child.name.as_str() {
            "table" => shapes.push(Shape::Table(TableShape {
                bbox,
                table: parse_table(child),
            })),
            "image" => {
                if let Some(picture) = picture(child, bbox, media) {
                    shapes.push(Shape::Picture(picture));
                }
            }
            "frame" | "custom-shape" | "caption" => collect_shape(child, media, shapes),
            _ => {}
        }
    }
}

fn collect_paragraphs(node: &Node, out: &mut Vec<Paragraph>) {
    for child in &node.children {
        match child.name.as_str() {
            "p" | "h" => {
                let text = paragraph_text(child).trim().to_string();
                if !text.is_empty() {
                    out.push(Paragraph {
                        text,
                        level: None,
                        list: false,
                    });
                }
            }
            "list" => collect_list(child, 1, out),
            "text-box" => collect_paragraphs(child, out),
            _ => {}
        }
    }
}

fn collect_list(node: &Node, level: i32, out: &mut Vec<Paragraph>) {
    for item in node.children_named("list-item") {
        for child in &item.children {
            match child.name.as_str() {
                "p" | "h" => {
                    let text = paragraph_text(child).trim().to_string();
                    if !text.is_empty() {
                        out.push(Paragraph {
                            text,
                            level: Some(level),
                            list: true,
                        });
                    }
                }
                "list" => collect_list(child, level + 1, out),
                _ => {}
            }
        }
    }
}

fn paragraph_text(node: &Node) -> String {
    let mut text = String::new();
    inline_text(node, &mut text);
    text
}

fn inline_text(node: &Node, out: &mut String) {
    // Text and child elements are read in the order they occurred: a span in
    // the middle of a sentence must not move its words to one end.
    for content in &node.content {
        match content {
            Content::Text(text) => out.push_str(text),
            Content::Child(index) => {
                let Some(child) = node.children.get(*index) else {
                    continue;
                };
                match child.name.as_str() {
                    "s" => out.push_str(&" ".repeat(count(child, "c"))),
                    "tab" => out.push('\t'),
                    "line-break" => out.push('\n'),
                    "image" | "frame" | "note" | "annotation" => {}
                    _ => inline_text(child, out),
                }
            }
        }
    }
}

fn read_notes(page: &Node) -> Option<String> {
    let notes = page.child("notes")?;
    let mut paragraphs = Vec::new();
    for frame in notes.children_named("frame") {
        if frame.attr("class") != Some("notes") {
            continue;
        }
        collect_paragraphs(frame, &mut paragraphs);
    }
    let text = paragraphs
        .iter()
        .map(|paragraph| paragraph.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn picture(node: &Node, bbox: [f64; 4], media: &HashMap<String, Vec<u8>>) -> Option<PictureShape> {
    let href = node.attr("href")?.trim_start_matches("./");
    let data = media.get(href)?.clone();
    let mime = node
        .attr("mime-type")
        .and_then(format_for_mime)
        .map(|format| format.mime)
        .or_else(|| pdfextractor_image_parser::mimetype(&data))
        .or_else(|| {
            std::path::Path::new(href)
                .extension()
                .and_then(|extension| extension.to_str())
                .and_then(format_for_extension)
                .map(|format| format.mime)
        })?;
    Some(PictureShape { bbox, mime, data })
}

fn parse_table(node: &Node) -> Table {
    let mut rows = Vec::new();
    let mut declared_cols = 0usize;
    collect_rows(node, &mut rows, &mut declared_cols);

    let placed = rows
        .iter()
        .flatten()
        .map(|cell| cell.col + cell.col_span.max(1))
        .max()
        .unwrap_or(0);

    Table {
        num_rows: rows.len(),
        num_cols: declared_cols.max(placed),
        cells: rows.into_iter().flatten().collect(),
    }
}

fn collect_rows(node: &Node, rows: &mut Vec<Vec<TableCell>>, declared_cols: &mut usize) {
    for child in &node.children {
        match child.name.as_str() {
            "table-header-rows" => collect_rows(child, rows, declared_cols),
            "table-row" => {
                let repeat = count(child, "number-rows-repeated").min(MAX_REPEAT);
                for _ in 0..repeat {
                    // Each copy is a row of its own, so its cells carry the
                    // index a reader gives that row.
                    rows.push(parse_row(child, rows.len()));
                }
            }
            "table-column" => {
                *declared_cols += count(child, "number-columns-repeated").min(MAX_REPEAT);
            }
            _ => {}
        }
    }
}

fn parse_row(node: &Node, row: usize) -> Vec<TableCell> {
    let mut cells = Vec::new();
    let mut col = 0usize;
    let mut covered_by_span = 0usize;
    for child in &node.children {
        match child.name.as_str() {
            "table-cell" => {
                let col_span = count(child, "number-columns-spanned");
                let row_span = count(child, "number-rows-spanned");
                let repeat = count(child, "number-columns-repeated").min(MAX_REPEAT);
                let text = cell_text(child);
                for _ in 0..repeat {
                    cells.push(TableCell {
                        row,
                        col,
                        row_span,
                        col_span,
                        text: text.clone(),
                    });
                    col += col_span;
                    covered_by_span += col_span.saturating_sub(1);
                }
            }
            "covered-table-cell" => {
                let repeat = count(child, "number-columns-repeated").min(MAX_REPEAT);
                for _ in 0..repeat {
                    if covered_by_span > 0 {
                        covered_by_span -= 1;
                    } else {
                        col += 1;
                    }
                }
            }
            _ => {}
        }
    }
    cells
}

fn cell_text(node: &Node) -> String {
    let mut paragraphs = Vec::new();
    collect_paragraphs(node, &mut paragraphs);
    paragraphs
        .iter()
        .map(|paragraph| paragraph.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A count an element states, at least one.
fn count(node: &Node, key: &str) -> usize {
    node.attr(key)
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(1)
        .max(1)
}

// --- XML tree -------------------------------------------------------------

#[derive(Debug, Clone)]
enum Content {
    Text(String),
    Child(usize),
}

#[derive(Debug, Clone, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    /// Text and child positions in document order. A child's index is its
    /// position in `children`; the child is filled in when its end tag is read.
    content: Vec<Content>,
}

impl Node {
    fn parse(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(Cursor::new(xml));
        reader.config_mut().trim_text(false);
        let mut stack = vec![Node {
            name: "#document".to_string(),
            ..Node::default()
        }];
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(start)) => {
                    let index = stack.last().expect("a root").children.len();
                    stack
                        .last_mut()
                        .expect("a root")
                        .content
                        .push(Content::Child(index));
                    stack.push(Node::from_start(&start));
                }
                Ok(Event::Empty(start)) => {
                    let node = Node::from_start(&start);
                    let parent = stack.last_mut().expect("a root");
                    let index = parent.children.len();
                    parent.children.push(node);
                    parent.content.push(Content::Child(index));
                }
                Ok(Event::End(_)) => {
                    if stack.len() > 1 {
                        let node = stack.pop().expect("a node");
                        stack.last_mut().expect("a parent").children.push(node);
                    }
                }
                Ok(Event::Text(text)) => {
                    let text = text
                        .unescape()
                        .context("malformed text in an OpenDocument part")?;
                    stack
                        .last_mut()
                        .expect("a root")
                        .content
                        .push(Content::Text(text.into_owned()));
                }
                Ok(Event::CData(text)) => {
                    stack
                        .last_mut()
                        .expect("a root")
                        .content
                        .push(Content::Text(String::from_utf8_lossy(&text).into_owned()));
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(error) => anyhow::bail!("malformed OpenDocument XML: {error}"),
            }
            buffer.clear();
        }
        Ok(stack.pop().expect("a root"))
    }

    fn from_start(start: &BytesStart<'_>) -> Self {
        Self {
            name: local(start),
            attrs: attributes(start),
            ..Self::default()
        }
    }

    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |child| child.name == name)
    }

    fn find(&self, name: &str) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(name))
    }

    fn descendants_named<'a>(&'a self, name: &'a str) -> Vec<&'a Node> {
        let mut found = Vec::new();
        self.collect_descendants(name, &mut found);
        found
    }

    fn collect_descendants<'a>(&'a self, name: &str, found: &mut Vec<&'a Node>) {
        for child in &self.children {
            if child.name == name {
                found.push(child);
            }
            child.collect_descendants(name, found);
        }
    }
}

fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|byte| *byte == b':') {
        Some(index) => &name[index + 1..],
        None => name,
    }
}

fn local(element: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(local_name(element.name().as_ref())).into_owned()
}

fn attributes(element: &BytesStart<'_>) -> Vec<(String, String)> {
    element
        .attributes()
        .flatten()
        .filter_map(|attribute| {
            attribute.unescape_value().ok().map(|value| {
                (
                    String::from_utf8_lossy(local_name(attribute.key.as_ref())).into_owned(),
                    value.into_owned(),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            for (name, data) in parts {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    const CONTENT: &str = r#"<document-content><body><presentation>
        <page name="One" show="false" master-page-name="Master">
          <frame class="title" x="1cm" y="1cm" width="5cm" height="1cm">
            <text-box><p>Deck <span>title</span>!</p></text-box>
          </frame>
          <frame x="1cm" y="2cm" width="5cm" height="2cm">
            <text-box><list>
              <list-item><p>First</p><list><list-item><p>Nested</p></list-item></list></list-item>
            </list></text-box>
          </frame>
          <frame x="1cm" y="4cm" width="5cm" height="2cm">
            <table><table-row number-rows-repeated="2"><table-cell><p>A</p></table-cell><table-cell><p>B</p></table-cell></table-row></table>
          </frame>
          <frame x="1cm" y="6cm" width="2cm" height="1cm">
            <image href="Pictures/p.png" mime-type="image/png"/>
          </frame>
          <notes>
            <frame class="notes"><text-box><p>Speak first.</p><p>Then pause.</p></text-box></frame>
            <p>A comment, not a speaker note.</p>
          </notes>
        </page>
      </presentation></body></document-content>"#;

    const STYLES: &str = r#"<document-styles>
        <automatic-styles><page-layout name="PL"><page-layout-properties page-width="20cm" page-height="10cm"/></page-layout></automatic-styles>
        <master-styles><master-page name="Master" page-layout-name="PL"/></master-styles>
      </document-styles>"#;

    #[test]
    fn a_slide_reads_text_lists_tables_pictures_and_notes() {
        let document = OdpDocument::parse_bytes(&package(&[
            ("content.xml", CONTENT.as_bytes()),
            ("styles.xml", STYLES.as_bytes()),
            ("Pictures/p.png", b"not decoded by the parser"),
        ]))
        .expect("the slide package parses");

        assert_eq!(document.width, 7_200_000.0);
        assert_eq!(document.height, 3_600_000.0);
        assert_eq!(document.slides.len(), 1);
        let slide = &document.slides[0];
        assert_eq!(slide.number, 1);
        assert!(slide.hidden);
        assert_eq!(
            slide.notes.as_deref(),
            Some("Speak first.\nThen pause."),
            "only the notes frame is a speaker note"
        );

        let text: Vec<_> = slide
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(text.len(), 2);
        assert!(text[0].title);
        assert_eq!(
            text[0].paragraphs[0].text, "Deck title!",
            "a span in the middle of a sentence stays there"
        );
        assert_eq!(text[0].bbox, [360_000.0, 360_000.0, 2_160_000.0, 720_000.0]);
        assert_eq!(text[1].paragraphs[0].level, Some(1));
        assert!(text[1].paragraphs[0].list);
        assert_eq!(text[1].paragraphs[1].text, "Nested");
        assert_eq!(text[1].paragraphs[1].level, Some(2));

        let table = slide
            .shapes
            .iter()
            .find_map(|shape| match shape {
                Shape::Table(table) => Some(table),
                _ => None,
            })
            .expect("the table");
        assert_eq!(table.table.num_rows, 2);
        assert_eq!(table.table.num_cols, 2);
        assert_eq!(table.table.cells[1].text, "B");
        assert_eq!((table.table.cells[2].row, table.table.cells[2].col), (1, 0));
        assert_eq!(table.table.cells[2].text, "A");

        let picture = slide
            .shapes
            .iter()
            .find_map(|shape| match shape {
                Shape::Picture(picture) => Some(picture),
                _ => None,
            })
            .expect("the picture");
        assert_eq!(picture.mime, "image/png");
        assert_eq!(picture.data, b"not decoded by the parser");
    }

    #[test]
    fn a_package_without_styles_uses_the_slide_default() {
        let document = OdpDocument::parse_bytes(&package(&[("content.xml", CONTENT.as_bytes())]))
            .expect("the slide package parses");
        assert_eq!(document.width, DEFAULT_WIDTH);
        assert_eq!(document.height, DEFAULT_HEIGHT);
    }
}
