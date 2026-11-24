//! ODT parsing (OpenDocument text, ODF 1.2/1.3).
//!
//! A `.odt` is a ZIP package. `content.xml` holds the body in document order —
//! `text:p` and `text:h` for paragraphs and headings, `text:list` for lists,
//! `table:table` for tables, `draw:frame` for pictures — and `styles.xml` holds
//! the named styles that say what a paragraph *is*: a style whose outline level
//! is set is a heading at that level, and Writer's `Title` style is the
//! document's title. A heading element states its own `text:outline-level`, so
//! the style lookup is for producers that leave it off the element.
//!
//! ODT is flow-based: there is no page a block sits on, so blocks carry no
//! provenance. The page size a page style declares is reported on the document
//! for the same reason DOCX reports its `w:sectPr` size — a fact of the layout,
//! not a page any item sits on.
//!
//! Not read yet: headers and footers (a master page's `style:header`), the
//! marker a numbered list prints (it lives in the list style with nested
//! counters), ODF comments and tracked changes, and formula objects.

use anyhow::Result;
use pdfextractor_limits as limits;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use quick_xml::XmlVersion;
use std::collections::HashMap;
use std::io::Cursor;

use pdfextractor_picture_formats::{format_for_extension, format_for_mime};

/// A page style that declares nothing: A4 at 72 points per inch.
const DEFAULT_PAGE_WIDTH: f64 = 595.276;
const DEFAULT_PAGE_HEIGHT: f64 = 841.89;

/// A `table:number-*-repeated` larger than this is filler, not content: Writer
/// writes a sheet's remaining rows and columns as one repeated cell.
const MAX_REPEAT: usize = 1024;

/// One block of document content, in document order.
#[derive(Debug, Clone)]
pub enum OdtItem {
    Text {
        text: String,
        /// `title`, `section_header`, `list_item` or `text`.
        label: String,
        /// The marker a list prints, when it is known. A numbered list's marker
        /// lives in the list style and its counters, which this does not read,
        /// so it is `None` rather than a guess.
        marker: Option<String>,
        /// Heading depth or list nesting depth, counted from one.
        level: Option<i32>,
        /// The target of a `text:a` wrapping the words, when they are a link.
        hyperlink: Option<String>,
    },
    Table(OdtTable),
    Picture {
        mime: &'static str,
        data: Vec<u8>,
        /// The frame's `svg:width`/`svg:height`, in points.
        width_pt: Option<f64>,
        height_pt: Option<f64>,
    },
}

/// One table: its rows, and the width of the grid they are placed on.
#[derive(Debug, Clone, Default)]
pub struct OdtTable {
    pub rows: Vec<Vec<OdtCell>>,
    pub num_cols: usize,
}

/// A picture a cell holds: the media part's bytes, their type, and the frame
/// the document places them in.
#[derive(Debug, Clone)]
pub struct OdtPicture {
    pub mime: &'static str,
    pub data: Vec<u8>,
    pub width_pt: Option<f64>,
    pub height_pt: Option<f64>,
}

/// One cell of a parsed table: its text, its place in the layout grid, and the
/// merges its markup declares.
#[derive(Debug, Clone, Default)]
pub struct OdtCell {
    pub text: String,
    pub pictures: Vec<OdtPicture>,
    pub tables: Vec<OdtTable>,
    /// Grid column the cell starts at (0-based).
    pub start_col: usize,
    /// Grid columns the cell covers (`table:number-columns-spanned`).
    pub col_span: usize,
    /// Grid rows the cell covers (`table:number-rows-spanned`).
    pub row_span: usize,
    /// True for a `table:covered-table-cell`: a slot a merge above or to the
    /// left covers, holding no content of its own.
    pub covered: bool,
    /// True when the cell's row is inside `table:table-header-rows` — the
    /// document's own statement of what a header is.
    pub header: bool,
}

/// One OpenDocument text document.
#[derive(Debug, Clone)]
pub struct OdtDocument {
    pub items: Vec<OdtItem>,
    /// The page rectangle a page style declares, in points.
    pub page_width: f64,
    pub page_height: f64,
}

impl OdtDocument {
    pub fn parse_file(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::parse_bytes(&data)
    }

    pub fn parse_bytes(data: &[u8]) -> Result<Self> {
        Self::parse_bytes_with_limits(data, limits::Limits::default())
    }

    /// Read an ODT under an explicit input, ZIP and XML budget.
    pub fn parse_bytes_with_limits(data: &[u8], budget: limits::Limits) -> Result<Self> {
        budget.check_input(data.len())?;
        let guard = budget.guard();
        guard.check("reading a package")?;
        let mut archive = limits::Package::new(data, &budget)
            .map_err(|e| anyhow::anyhow!("not an OpenDocument (ZIP) package: {e}"))?;
        budget.check_zip(archive.archive_mut())?;

        let content = read_part(&mut archive, "content.xml")
            .ok_or_else(|| anyhow::anyhow!("package has no content.xml"))?;
        let styles_xml = read_part(&mut archive, "styles.xml");

        // `styles.xml` holds the named styles; `content.xml` holds the automatic
        // ones a run of the document defined for itself (Writer writes `P1`,
        // `P2`, … there). Both name heading levels, so both are used.
        let mut styles = match &styles_xml {
            Some(xml) => Styles::parse(xml)?,
            None => Styles::default(),
        };
        styles.merge(Styles::parse(&content)?);

        let mut media: HashMap<String, Vec<u8>> = HashMap::new();
        for i in 0..archive.len() {
            let (name, is_dir) = match archive.by_index(i) {
                Ok(file) => match file.name() {
                    Ok(name) => (name.into_owned(), file.is_dir()),
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            if is_dir || !name.starts_with("Pictures/") {
                continue;
            }
            if let Some(buf) = archive.read_name(&name) {
                media.insert(name, buf);
            }
        }

        let mut walker = Walker {
            pull: Pull::new(&content),
            media: &media,
            styles: &styles,
            items: Vec::new(),
            notes: Vec::new(),
        };
        walker.walk(&guard)?;

        let mut items = walker.items;
        // A footnote's text is body content: Docling reports notes at the end of
        // the document, which is where they are appended.
        items.append(&mut walker.notes);

        let (page_width, page_height) = styles
            .page
            .unwrap_or((DEFAULT_PAGE_WIDTH, DEFAULT_PAGE_HEIGHT));
        Ok(OdtDocument {
            items,
            page_width,
            page_height,
        })
    }
}

fn read_part(archive: &mut limits::Package<'_>, name: &str) -> Option<Vec<u8>> {
    archive.read_name(name)
}

/// The named styles of a package: what a paragraph's `text:style-name` means.
#[derive(Debug, Clone, Default)]
struct Styles {
    map: HashMap<String, Style>,
    /// The first page rectangle a page layout declares, in points.
    page: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default)]
struct Style {
    display: Option<String>,
    parent: Option<String>,
    outline: Option<i32>,
}

impl Styles {
    fn parse(xml: &[u8]) -> Result<Self> {
        let mut styles = Styles::default();
        let mut pull = Pull::new(xml);
        loop {
            match pull.next()? {
                Tok::Start { name, attrs } | Tok::Empty { name, attrs } => match name.as_str() {
                    "style" => {
                        if let Some(name) = field(&attrs, "name") {
                            styles.map.insert(
                                name.to_string(),
                                Style {
                                    display: field(&attrs, "display-name").map(str::to_string),
                                    parent: field(&attrs, "parent-style-name").map(str::to_string),
                                    outline: field(&attrs, "outline-level")
                                        .and_then(|level| level.trim().parse().ok()),
                                },
                            );
                        }
                    }
                    "page-layout-properties" if styles.page.is_none() => {
                        let width = field(&attrs, "page-width").and_then(length_to_pt);
                        let height = field(&attrs, "page-height").and_then(length_to_pt);
                        if let (Some(width), Some(height)) = (width, height) {
                            styles.page = Some((width, height));
                        }
                    }
                    _ => {}
                },
                Tok::Eof => break,
                _ => {}
            }
        }
        Ok(styles)
    }

    /// Content.xml's automatic styles are added beside styles.xml's; a name the
    /// two share is the automatic one's, because that is the more specific
    /// statement about this document.
    fn merge(&mut self, other: Styles) {
        if self.page.is_none() {
            self.page = other.page;
        }
        for (name, style) in other.map {
            self.map.insert(name, style);
        }
    }

    /// The outline level a style means, following its parent chain.
    fn outline_level(&self, name: Option<&str>) -> Option<i32> {
        let mut current = name.map(str::to_string);
        for _ in 0..16 {
            let style_name = current?;
            match self.map.get(&style_name) {
                Some(style) => {
                    if let Some(level) = style.outline {
                        return Some(level);
                    }
                    let display = style.display.as_deref().unwrap_or(&style_name);
                    if let Some(level) = heading_number(display) {
                        return Some(level);
                    }
                    current = style.parent.clone();
                }
                // A style not named in this package is judged by its name alone:
                // a `Heading 3` is a level-3 heading wherever it was defined.
                None => return heading_number(&style_name),
            }
        }
        None
    }

    /// Whether a style is the document's title: Writer's `Title`, or a style
    /// that descends from it.
    fn is_title(&self, name: Option<&str>) -> bool {
        let mut current = name.map(str::to_string);
        for _ in 0..16 {
            let Some(style_name) = current else {
                return false;
            };
            let Some(style) = self.map.get(&style_name) else {
                return style_name == "Title";
            };
            let display = style.display.as_deref().unwrap_or(&style_name);
            if style_name == "Title" || display == "Title" {
                return true;
            }
            current = style.parent.clone();
        }
        false
    }
}

/// `Heading 1` (or `Heading_20_1`, the name Writer writes) as a level.
fn heading_number(name: &str) -> Option<i32> {
    let spaced = name.replace("_20_", " ");
    let rest = spaced.strip_prefix("Heading ")?;
    rest.trim().parse().ok()
}

/// A length as ODF writes it, in points. `px` is a CSS pixel — 3/4 of a point,
/// which is the 96 dpi a document without a resolution is read at.
fn length_to_pt(value: &str) -> Option<f64> {
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
    Some(points)
}

/// One XML token, with the local names this reader looks the document up by:
/// `text:p` is a `p` and `table:number-columns-spanned` is
/// `number-columns-spanned`, whatever prefix the producer chose.
enum Tok {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
    },
    Empty {
        name: String,
        attrs: Vec<(String, String)>,
    },
    End {
        name: String,
    },
    Text(String),
    CData(String),
    Eof,
}

/// A pull reader over one XML part.
struct Pull<'a> {
    reader: Reader<Cursor<&'a [u8]>>,
    buf: Vec<u8>,
}

impl<'a> Pull<'a> {
    fn new(xml: &'a [u8]) -> Self {
        let mut reader = Reader::from_reader(Cursor::new(xml));
        reader.config_mut().trim_text(false);
        Self {
            reader,
            buf: Vec::new(),
        }
    }

    fn next(&mut self) -> Result<Tok> {
        loop {
            self.buf.clear();
            match self.reader.read_event_into(&mut self.buf) {
                Ok(Event::Start(start)) => {
                    return Ok(Tok::Start {
                        name: local(&start),
                        attrs: attributes(&start),
                    })
                }
                Ok(Event::Empty(start)) => {
                    return Ok(Tok::Empty {
                        name: local(&start),
                        attrs: attributes(&start),
                    })
                }
                Ok(Event::End(end)) => {
                    return Ok(Tok::End {
                        name: String::from_utf8_lossy(local_name(end.name().as_ref().as_bytes()))
                            .into_owned(),
                    })
                }
                Ok(Event::Text(text)) => {
                    return Ok(Tok::Text(
                        text.xml_content(XmlVersion::Implicit1_0).into_owned(),
                    ))
                }
                // A reference between two runs of text is text too: `&amp;` in a
                // paragraph is one ampersand, and the reader reports it apart.
                Ok(Event::GeneralRef(reference)) => {
                    let mut text = String::new();
                    limits::push_reference(&mut text, &reference);
                    return Ok(Tok::Text(text));
                }
                Ok(Event::CData(text)) => {
                    return Ok(Tok::CData(
                        text.xml_content(XmlVersion::Implicit1_0).into_owned(),
                    ))
                }
                Ok(Event::Eof) => return Ok(Tok::Eof),
                // Declarations, processing instructions and comments carry no
                // document content.
                Ok(_) => continue,
                Err(err) => anyhow::bail!("malformed OpenDocument XML: {err}"),
            }
        }
    }
}

fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|b| *b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn local(element: &BytesStart) -> String {
    String::from_utf8_lossy(local_name(element.name().as_ref().as_bytes())).into_owned()
}

fn attributes(element: &BytesStart) -> Vec<(String, String)> {
    element
        .attributes()
        .flatten()
        .filter_map(|attribute| {
            attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|value| {
                    (
                        String::from_utf8_lossy(local_name(attribute.key.as_ref().as_bytes()))
                            .into_owned(),
                        value.into_owned(),
                    )
                })
        })
        .collect()
}

/// The attribute's value, when the element states it.
fn field<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// A count an element states, at least one.
fn count(attrs: &[(String, String)], key: &str) -> usize {
    field(attrs, key)
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(1)
        .max(1)
}

/// What a paragraph's inline content produced.
#[derive(Default)]
struct Inline {
    text: String,
    href: Option<String>,
    pictures: Vec<OdtPicture>,
    /// Text a frame's text box held, in document order.
    blocks: Vec<String>,
}

/// The body walker: reads `content.xml`'s flow in document order.
struct Walker<'a> {
    pull: Pull<'a>,
    media: &'a HashMap<String, Vec<u8>>,
    styles: &'a Styles,
    items: Vec<OdtItem>,
    notes: Vec<OdtItem>,
}

impl Walker<'_> {
    fn walk(&mut self, guard: &limits::Guard<'_>) -> Result<()> {
        // The walk is the reading, so the deadline is checked inside it: one
        // document's content part is the unit that can be large.
        let mut tokens = 0u64;
        loop {
            tokens = tokens.wrapping_add(1);
            if tokens.is_multiple_of(4096) {
                guard.check("reading an ODT part")?;
            }
            match self.pull.next()? {
                Tok::Start { name, attrs } => match name.as_str() {
                    "p" => self.paragraph(false, &attrs, None)?,
                    "h" => self.paragraph(true, &attrs, None)?,
                    "list" => self.list(1)?,
                    "table" => {
                        let table = self.table()?;
                        self.items.push(OdtItem::Table(table));
                    }
                    "frame" => self.frame(&attrs)?,
                    // `text:sequence-decls`, sections, an index's body and the
                    // rest: their children are still the document's blocks, and
                    // the loop reads them where they are written.
                    _ => {}
                },
                Tok::Eof => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// One paragraph or heading, emitted with the label it means.
    fn paragraph(
        &mut self,
        heading: bool,
        attrs: &[(String, String)],
        list_level: Option<i32>,
    ) -> Result<()> {
        let style = field(attrs, "style-name");
        let stated = field(attrs, "outline-level").and_then(|level| level.trim().parse().ok());
        let level = stated.or_else(|| self.styles.outline_level(style));
        let inline = self.inline()?;

        let label = if heading || (list_level.is_none() && level.is_some()) {
            "section_header"
        } else if list_level.is_some() {
            "list_item"
        } else if self.styles.is_title(style) {
            "title"
        } else {
            "text"
        };
        let level = if label == "section_header" || label == "list_item" {
            level.or(list_level).or(Some(1))
        } else {
            None
        };

        let text = inline.text.trim();
        if !text.is_empty() {
            self.items.push(OdtItem::Text {
                text: text.to_string(),
                label: label.to_string(),
                marker: None,
                level,
                hyperlink: inline.href,
            });
        }
        self.push_blocks(inline.blocks, None);
        self.push_pictures(inline.pictures);
        Ok(())
    }

    /// A paragraph inside a list: a list item at the list's depth.
    fn list_paragraph(&mut self, level: i32) -> Result<()> {
        let inline = self.inline()?;
        let text = inline.text.trim();
        if !text.is_empty() {
            self.items.push(OdtItem::Text {
                text: text.to_string(),
                label: "list_item".to_string(),
                marker: None,
                level: Some(level),
                hyperlink: inline.href,
            });
        }
        self.push_blocks(inline.blocks, None);
        self.push_pictures(inline.pictures);
        Ok(())
    }

    fn push_blocks(&mut self, blocks: Vec<String>, level: Option<i32>) {
        for block in blocks {
            let block = block.trim();
            if !block.is_empty() {
                self.items.push(OdtItem::Text {
                    text: block.to_string(),
                    label: "text".to_string(),
                    marker: None,
                    level,
                    hyperlink: None,
                });
            }
        }
    }

    fn push_pictures(&mut self, pictures: Vec<OdtPicture>) {
        for picture in pictures {
            self.items.push(OdtItem::Picture {
                mime: picture.mime,
                data: picture.data,
                width_pt: picture.width_pt,
                height_pt: picture.height_pt,
            });
        }
    }

    /// The inline content of a paragraph or heading, up to its closing tag.
    fn inline(&mut self) -> Result<Inline> {
        let mut inline = Inline::default();
        let mut depth = 0usize;
        loop {
            match self.pull.next()? {
                Tok::Text(text) | Tok::CData(text) => inline.text.push_str(&text),
                Tok::Start { name, attrs } => match name.as_str() {
                    "s" => {
                        inline.text.push_str(&" ".repeat(count(&attrs, "c")));
                        depth += 1;
                    }
                    "tab" => {
                        inline.text.push('\t');
                        depth += 1;
                    }
                    "line-break" => {
                        inline.text.push('\n');
                        depth += 1;
                    }
                    "a" => {
                        inline.href = inline
                            .href
                            .take()
                            .or_else(|| field(&attrs, "href").map(str::to_string));
                        depth += 1;
                    }
                    // A frame, a note and an annotation each consume their own
                    // closing tag, so neither adds to the depth here.
                    "frame" => self.frame_into(&attrs, &mut inline)?,
                    "note" => self.note()?,
                    // A comment is not read yet: its text is not the document's.
                    "annotation" => self.skip_element()?,
                    _ => depth += 1,
                },
                Tok::Empty { name, attrs } => match name.as_str() {
                    "s" => inline.text.push_str(&" ".repeat(count(&attrs, "c"))),
                    "tab" => inline.text.push('\t'),
                    "line-break" => inline.text.push('\n'),
                    "a" => {
                        inline.href = inline
                            .href
                            .take()
                            .or_else(|| field(&attrs, "href").map(str::to_string));
                    }
                    _ => {}
                },
                Tok::End { .. } => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Tok::Eof => break,
            }
        }
        Ok(inline)
    }

    /// A list at `level`, with its items and any list it nests.
    fn list(&mut self, level: i32) -> Result<()> {
        loop {
            match self.pull.next()? {
                Tok::Start { name, .. } => match name.as_str() {
                    "list-item" | "list-header" => self.list_item(level)?,
                    "p" => self.list_paragraph(level)?,
                    "h" => self.paragraph(true, &[], None)?,
                    "list" => self.list(level + 1)?,
                    _ => {}
                },
                Tok::End { name } if name == "list" => break,
                Tok::Eof => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// One `text:list-item`: its paragraphs, nested lists and pictures.
    fn list_item(&mut self, level: i32) -> Result<()> {
        loop {
            match self.pull.next()? {
                Tok::Start { name, .. } => match name.as_str() {
                    "p" => self.list_paragraph(level)?,
                    "h" => self.paragraph(true, &[], None)?,
                    "list" => self.list(level + 1)?,
                    "frame" => self.frame_from_reader()?,
                    _ => {}
                },
                Tok::End { name } if name == "list-item" || name == "list-header" => break,
                Tok::Eof => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// A frame met where its own start tag has already been consumed.
    fn frame_from_reader(&mut self) -> Result<()> {
        let mut inline = Inline::default();
        self.frame_into(&[], &mut inline)?;
        self.push_blocks(inline.blocks, None);
        self.push_pictures(inline.pictures);
        Ok(())
    }

    /// A frame: the pictures it places, and the text its text box holds.
    fn frame(&mut self, attrs: &[(String, String)]) -> Result<()> {
        let mut inline = Inline::default();
        self.frame_into(attrs, &mut inline)?;
        self.push_blocks(inline.blocks, None);
        self.push_pictures(inline.pictures);
        Ok(())
    }

    /// Read one `draw:frame` into `inline`: its image parts become pictures,
    /// and a `draw:text-box` becomes text blocks.
    fn frame_into(&mut self, attrs: &[(String, String)], inline: &mut Inline) -> Result<()> {
        let width = field(attrs, "width").and_then(length_to_pt);
        let height = field(attrs, "height").and_then(length_to_pt);
        let mut depth = 0usize;
        loop {
            match self.pull.next()? {
                Tok::Start { name, attrs } => match name.as_str() {
                    "image" => {
                        self.image(&attrs, width, height, inline);
                        self.skip_element()?;
                    }
                    "text-box" => self.text_box(inline)?,
                    "object" => self.skip_element()?,
                    "frame" => {
                        let mut nested = Inline::default();
                        self.frame_into(&attrs, &mut nested)?;
                        inline.pictures.extend(nested.pictures);
                        inline.blocks.extend(nested.blocks);
                    }
                    _ => depth += 1,
                },
                Tok::Empty { name, attrs } => match name.as_str() {
                    "image" => self.image(&attrs, width, height, inline),
                    "frame" => {
                        let mut nested = Inline::default();
                        self.frame_into(&attrs, &mut nested)?;
                        inline.pictures.extend(nested.pictures);
                        inline.blocks.extend(nested.blocks);
                    }
                    _ => {}
                },
                Tok::End { .. } => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Tok::Eof => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// The picture a `draw:image` names, placed at the frame's stated size.
    fn image(
        &self,
        attrs: &[(String, String)],
        width: Option<f64>,
        height: Option<f64>,
        inline: &mut Inline,
    ) {
        let Some(href) = field(attrs, "href") else {
            return;
        };
        if let Some(mut picture) = self.picture(href, field(attrs, "mime-type")) {
            picture.width_pt = width;
            picture.height_pt = height;
            inline.pictures.push(picture);
        }
    }

    /// The paragraphs a `draw:text-box` holds, as text blocks.
    fn text_box(&mut self, inline: &mut Inline) -> Result<()> {
        loop {
            match self.pull.next()? {
                Tok::Start { name, .. } => match name.as_str() {
                    "p" | "h" => {
                        let inner = self.inline()?;
                        inline.blocks.push(inner.text);
                        inline.pictures.extend(inner.pictures);
                    }
                    _ => {}
                },
                Tok::End { name } if name == "text-box" => break,
                Tok::Eof => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// One `text:note`: its text is the document's, reported as a footnote.
    fn note(&mut self) -> Result<()> {
        let mut text = String::new();
        let mut depth = 0usize;
        loop {
            match self.pull.next()? {
                Tok::Text(chunk) | Tok::CData(chunk) => text.push_str(&chunk),
                Tok::Start { name, attrs } => match name.as_str() {
                    // The citation is the marker the note prints, not its text.
                    "note-citation" => self.skip_element()?,
                    "p" => {
                        if !text.is_empty() && !text.ends_with('\n') {
                            text.push('\n');
                        }
                        depth += 1;
                    }
                    "s" => {
                        text.push_str(&" ".repeat(count(&attrs, "c")));
                        depth += 1;
                    }
                    "tab" => {
                        text.push('\t');
                        depth += 1;
                    }
                    "line-break" => {
                        text.push('\n');
                        depth += 1;
                    }
                    _ => depth += 1,
                },
                Tok::Empty { name, attrs } => match name.as_str() {
                    "s" => text.push_str(&" ".repeat(count(&attrs, "c"))),
                    "tab" => text.push('\t'),
                    "line-break" => text.push('\n'),
                    _ => {}
                },
                Tok::End { .. } => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Tok::Eof => break,
            }
        }
        let text = text.trim();
        if !text.is_empty() {
            self.notes.push(OdtItem::Text {
                text: text.to_string(),
                label: "footnote".to_string(),
                marker: None,
                level: None,
                hyperlink: None,
            });
        }
        Ok(())
    }

    /// A table: its rows, its spans and its header rows.
    fn table(&mut self) -> Result<OdtTable> {
        let mut rows: Vec<Vec<OdtCell>> = Vec::new();
        let mut declared_cols = 0usize;
        let mut header_row = false;
        let mut depth = 0usize;
        loop {
            match self.pull.next()? {
                Tok::Start { name, attrs } => match name.as_str() {
                    "table-header-rows" => {
                        header_row = true;
                        depth += 1;
                    }
                    "table-row" => {
                        let repeat = count(&attrs, "number-rows-repeated").min(MAX_REPEAT);
                        let row = self.row(header_row)?;
                        push_rows(&mut rows, row, repeat);
                    }
                    "table-column" => {
                        declared_cols += count(&attrs, "number-columns-repeated");
                    }
                    _ => depth += 1,
                },
                Tok::Empty { name, attrs } => {
                    if name == "table-column" {
                        declared_cols += count(&attrs, "number-columns-repeated");
                    }
                }
                Tok::End { name } => {
                    if name == "table-header-rows" {
                        header_row = false;
                    }
                    if depth == 0 && name == "table" {
                        break;
                    }
                    depth = depth.saturating_sub(1);
                }
                Tok::Eof => break,
                _ => {}
            }
        }

        // A table that marks no header row keeps the reading every unmarked
        // table has here: the first row heads the columns.
        if !rows.iter().flatten().any(|cell| cell.header) {
            if let Some(first) = rows.first_mut() {
                for cell in first {
                    cell.header = true;
                }
            }
        }
        let placed = rows
            .iter()
            .flatten()
            .map(|cell| cell.start_col + cell.col_span.max(1))
            .max()
            .unwrap_or(0);
        Ok(OdtTable {
            num_cols: declared_cols.max(placed),
            rows,
        })
    }

    /// One `table:table-row`, cells placed against the grid.
    fn row(&mut self, header: bool) -> Result<Vec<OdtCell>> {
        let mut cells = Vec::new();
        let mut col = 0usize;
        // ODF writes a `covered-table-cell` for every column a horizontal span
        // covers, *after* the spanning cell — but the span has already advanced
        // the column past them. Those placeholders are part of the span (the
        // builder places a span by its `col_span`), so they are taken here
        // rather than placed as cells of their own. A covered cell with no span
        // before it is a column a merge from an earlier row covers, and that one
        // is placed so the following cells keep their columns.
        let mut covered_by_span = 0usize;
        loop {
            match self.pull.next()? {
                Tok::Start { name, attrs } => match name.as_str() {
                    "table-cell" => {
                        let col_span = count(&attrs, "number-columns-spanned");
                        let row_span = count(&attrs, "number-rows-spanned");
                        let repeat = count(&attrs, "number-columns-repeated").min(MAX_REPEAT);
                        let cell = self.cell(col, col_span, row_span, false, header)?;
                        let empty = cell_is_empty(&cell);
                        cells.push(cell);
                        col += col_span;
                        covered_by_span = col_span.saturating_sub(1);
                        for _ in 1..repeat {
                            if !empty {
                                cells.push(OdtCell {
                                    start_col: col,
                                    col_span,
                                    row_span,
                                    covered: false,
                                    header,
                                    ..OdtCell::default()
                                });
                            }
                            col += col_span;
                            covered_by_span += col_span.saturating_sub(1);
                        }
                    }
                    "covered-table-cell" => {
                        let repeat = count(&attrs, "number-columns-repeated").min(MAX_REPEAT);
                        for _ in 0..repeat {
                            if covered_by_span > 0 {
                                covered_by_span -= 1;
                            } else {
                                cells.push(OdtCell {
                                    start_col: col,
                                    col_span: 1,
                                    row_span: 1,
                                    covered: true,
                                    header,
                                    ..OdtCell::default()
                                });
                                col += 1;
                            }
                        }
                    }
                    _ => self.skip_element()?,
                },
                Tok::Empty { name, attrs } => match name.as_str() {
                    "table-cell" => {
                        let col_span = count(&attrs, "number-columns-spanned");
                        let row_span = count(&attrs, "number-rows-spanned");
                        let repeat = count(&attrs, "number-columns-repeated").min(MAX_REPEAT);
                        for _ in 0..repeat {
                            cells.push(OdtCell {
                                start_col: col,
                                col_span,
                                row_span,
                                covered: false,
                                header,
                                ..OdtCell::default()
                            });
                            col += col_span;
                            covered_by_span += col_span.saturating_sub(1);
                        }
                    }
                    "covered-table-cell" => {
                        let repeat = count(&attrs, "number-columns-repeated").min(MAX_REPEAT);
                        for _ in 0..repeat {
                            if covered_by_span > 0 {
                                covered_by_span -= 1;
                            } else {
                                cells.push(OdtCell {
                                    start_col: col,
                                    col_span: 1,
                                    row_span: 1,
                                    covered: true,
                                    header,
                                    ..OdtCell::default()
                                });
                                col += 1;
                            }
                        }
                    }
                    _ => {}
                },
                Tok::End { .. } => break,
                Tok::Eof => break,
                _ => {}
            }
        }
        Ok(cells)
    }

    /// One `table:table-cell`: its paragraphs, tables and pictures.
    fn cell(
        &mut self,
        start_col: usize,
        col_span: usize,
        row_span: usize,
        covered: bool,
        header: bool,
    ) -> Result<OdtCell> {
        let mut cell = OdtCell {
            start_col,
            col_span,
            row_span,
            covered,
            header,
            ..OdtCell::default()
        };
        let mut paragraphs: Vec<String> = Vec::new();
        loop {
            match self.pull.next()? {
                Tok::Start { name, attrs } => match name.as_str() {
                    "p" | "h" => {
                        let inline = self.inline()?;
                        if !inline.text.trim().is_empty() {
                            paragraphs.push(inline.text.trim().to_string());
                        }
                        for block in inline.blocks {
                            if !block.trim().is_empty() {
                                paragraphs.push(block.trim().to_string());
                            }
                        }
                        cell.pictures.extend(inline.pictures);
                    }
                    "table" => {
                        let table = self.table()?;
                        // A cell holding a table shows that table's words: Docling
                        // flattens them into the cell's text, and a reading that
                        // left the cell empty would leave them only in the table
                        // beside it.
                        let flattened = flatten_table(&table);
                        if !flattened.is_empty() {
                            paragraphs.push(flattened);
                        }
                        cell.tables.push(table);
                    }
                    "frame" => {
                        let mut inline = Inline::default();
                        self.frame_into(&attrs, &mut inline)?;
                        cell.pictures.extend(inline.pictures);
                        for block in inline.blocks {
                            if !block.trim().is_empty() {
                                paragraphs.push(block.trim().to_string());
                            }
                        }
                    }
                    "list" => {
                        let before = self.items.len();
                        self.list(1)?;
                        for item in self.items.split_off(before) {
                            match item {
                                OdtItem::Text { text, .. } => paragraphs.push(text),
                                OdtItem::Picture {
                                    mime,
                                    data,
                                    width_pt,
                                    height_pt,
                                } => cell.pictures.push(OdtPicture {
                                    mime,
                                    data,
                                    width_pt,
                                    height_pt,
                                }),
                                OdtItem::Table(table) => {
                                    let flattened = flatten_table(&table);
                                    if !flattened.is_empty() {
                                        paragraphs.push(flattened);
                                    }
                                    cell.tables.push(table);
                                }
                            }
                        }
                    }
                    "note" => self.note()?,
                    _ => self.skip_element()?,
                },
                Tok::End { .. } => break,
                Tok::Eof => break,
                _ => {}
            }
        }
        cell.text = paragraphs.join("\n");
        Ok(cell)
    }

    /// Resolve a `draw:image`'s part name into the picture it holds.
    fn picture(&self, href: &str, hint: Option<&str>) -> Option<OdtPicture> {
        let key = href.trim_start_matches("./");
        let data = self.media.get(key)?.clone();
        let mime = hint
            .and_then(format_for_mime)
            .map(|format| format.mime)
            .or_else(|| pdfextractor_image_parser::mimetype(&data))
            .or_else(|| {
                std::path::Path::new(key)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(format_for_extension)
                    .map(|format| format.mime)
            })?;
        Some(OdtPicture {
            mime,
            data,
            width_pt: None,
            height_pt: None,
        })
    }

    /// Throw away one element and everything under it.
    fn skip_element(&mut self) -> Result<()> {
        let mut depth = 0usize;
        loop {
            match self.pull.next()? {
                Tok::Start { .. } => depth += 1,
                Tok::End { .. } => {
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                }
                Tok::Eof => return Ok(()),
                _ => {}
            }
        }
    }
}

/// The words a nested table shows, flattened the way Docling reports a cell
/// that holds one: each cell's text on its own line, rows and cells in order.
fn flatten_table(table: &OdtTable) -> String {
    table
        .rows
        .iter()
        .flatten()
        .map(|cell| cell.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn cell_is_empty(cell: &OdtCell) -> bool {
    cell.text.trim().is_empty() && cell.pictures.is_empty() && cell.tables.is_empty()
}

/// Append a row, honouring `table:number-rows-repeated`. A repeated row with
/// no content is filler — Writer writes the rest of a sheet as one — so it is
/// left out rather than multiplied.
fn push_rows(rows: &mut Vec<Vec<OdtCell>>, row: Vec<OdtCell>, repeat: usize) {
    let empty = row.iter().all(cell_is_empty);
    if empty {
        if repeat <= 1 {
            rows.push(row);
        }
        return;
    }
    for _ in 0..repeat {
        rows.push(row.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(content: &str, styles: &str, pictures: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("mimetype", options).unwrap();
        writer
            .write_all(b"application/vnd.oasis.opendocument.text")
            .unwrap();
        writer.start_file("content.xml", options).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
        writer.start_file("styles.xml", options).unwrap();
        writer.write_all(styles.as_bytes()).unwrap();
        for (name, data) in pictures {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
 xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
 <office:automatic-styles>
  <style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm"/></style:page-layout>
 </office:automatic-styles>
 <office:styles>
  <style:style style:name="Heading_20_2" style:display-name="Heading 2" style:family="paragraph" style:parent-style-name="Heading_20_1"/>
  <style:style style:name="Heading_20_1" style:family="paragraph" text:outline-level="1"/>
  <style:style style:name="Title" style:display-name="Title" style:family="paragraph"/>
 </office:styles>
</office:document-styles>"#;

    fn content(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
 xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"
 xmlns:xlink="http://www.w3.org/1999/xlink">
 <office:body><office:text>{body}</office:text></office:body>
</office:document-content>"#
        )
    }

    /// A spent deadline refuses the read by name rather than walking the part.
    #[test]
    fn a_spent_deadline_is_enforced() {
        let pkg = package(&content(""), STYLES, &[]);
        let budget = pdfextractor_limits::Limits {
            timeout: std::time::Duration::ZERO,
            ..pdfextractor_limits::Limits::default()
        };
        let error =
            OdtDocument::parse_bytes_with_limits(&pkg, budget).expect_err("the deadline is spent");
        assert!(error.to_string().contains("timed out"), "{error}");
    }

    #[test]
    fn a_custom_zip_budget_is_enforced() {
        let pkg = package(&content(""), STYLES, &[]);
        let budget = pdfextractor_limits::Limits {
            max_zip_entries: 2,
            ..pdfextractor_limits::Limits::default()
        };
        let error = OdtDocument::parse_bytes_with_limits(&pkg, budget)
            .expect_err("the package has three entries");
        assert!(error.to_string().contains("entries"), "{error}");
    }

    fn texts(items: &[OdtItem]) -> Vec<(String, String, Option<i32>)> {
        items
            .iter()
            .filter_map(|item| match item {
                OdtItem::Text {
                    text, label, level, ..
                } => Some((text.clone(), label.clone(), *level)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn headings_take_their_level_from_the_element_or_the_style() {
        let body = r#"
            <text:p text:style-name="Title">Brief</text:p>
            <text:h text:style-name="Heading_20_1" text:outline-level="1">Summary</text:h>
            <text:p text:style-name="Heading_20_2">Styled heading</text:p>
            <text:p text:style-name="Standard">Body text</text:p>"#;
        let doc = OdtDocument::parse_bytes(&package(&content(body), STYLES, &[])).unwrap();
        assert_eq!(
            texts(&doc.items),
            vec![
                ("Brief".to_string(), "title".to_string(), None),
                ("Summary".to_string(), "section_header".to_string(), Some(1)),
                (
                    "Styled heading".to_string(),
                    "section_header".to_string(),
                    Some(2)
                ),
                ("Body text".to_string(), "text".to_string(), None),
            ]
        );
        // The page style's rectangle is the document's.
        assert!((doc.page_width - 595.276).abs() < 0.5, "{}", doc.page_width);
        assert!(
            (doc.page_height - 841.89).abs() < 0.5,
            "{}",
            doc.page_height
        );
    }

    #[test]
    fn a_list_keeps_its_nesting_and_a_link_its_target() {
        let body = r#"
            <text:p text:style-name="Standard">See <text:a xlink:href="https://example.com/x">the filing</text:a>.</text:p>
            <text:list><text:list-item><text:p>One</text:p>
              <text:list><text:list-item><text:p>One point one</text:p></text:list-item></text:list>
            </text:list-item><text:list-item><text:p>Two</text:p></text:list-item></text:list>"#;
        let doc = OdtDocument::parse_bytes(&package(&content(body), STYLES, &[])).unwrap();
        let items = texts(&doc.items);
        assert!(items.contains(&("See the filing.".to_string(), "text".to_string(), None)));
        match &doc.items[0] {
            OdtItem::Text { hyperlink, .. } => {
                assert_eq!(hyperlink.as_deref(), Some("https://example.com/x"))
            }
            other => panic!("expected text, got {other:?}"),
        }
        assert!(items.contains(&("One".to_string(), "list_item".to_string(), Some(1))));
        assert!(items.contains(&(
            "One point one".to_string(),
            "list_item".to_string(),
            Some(2)
        )));
        assert!(items.contains(&("Two".to_string(), "list_item".to_string(), Some(1))));
    }

    #[test]
    fn a_table_keeps_its_spans_and_its_cover() {
        let body = r#"
            <table:table table:name="T">
              <table:table-column table:number-columns-repeated="3"/>
              <table:table-row>
                <table:table-cell><text:p>Region</text:p></table:table-cell>
                <table:table-cell table:number-columns-spanned="2"><text:p>Revenue</text:p></table:table-cell>
                <table:covered-table-cell/>
              </table:table-row>
              <table:table-row>
                <table:table-cell table:number-rows-spanned="2"><text:p>North</text:p><text:p>South</text:p></table:table-cell>
                <table:table-cell><text:p>42</text:p></table:table-cell>
                <table:table-cell><text:p>7</text:p></table:table-cell>
              </table:table-row>
              <table:table-row>
                <table:covered-table-cell/>
                <table:table-cell><text:p>18</text:p></table:table-cell>
                <table:table-cell><text:p>3</text:p></table:table-cell>
              </table:table-row>
            </table:table>"#;
        let doc = OdtDocument::parse_bytes(&package(&content(body), STYLES, &[])).unwrap();
        let table = match &doc.items[0] {
            OdtItem::Table(table) => table,
            other => panic!("expected a table, got {other:?}"),
        };
        assert_eq!(table.num_cols, 3);
        assert_eq!(table.rows.len(), 3);
        let revenue = &table.rows[0][1];
        assert_eq!(revenue.text, "Revenue");
        assert_eq!(revenue.col_span, 2);
        // The covered placeholder Writer writes after a horizontal span is part
        // of the span, not a cell of the row.
        assert_eq!(table.rows[0].len(), 2);
        assert!(table.rows[2][0].covered);
        let north = &table.rows[1][0];
        assert_eq!(north.text, "North\nSouth");
        assert_eq!(north.row_span, 2);
        assert!(
            table.rows[0][0].header,
            "the unmarked first row is the header"
        );
        assert_eq!(table.rows[2][1].text, "18");
    }

    /// A table inside a cell is read as a table of its own *and* its words are
    /// the cell's: a reader that saw only the nested table would say the cell
    /// was empty, which is not what Docling reports nor what a reader sees.
    #[test]
    fn a_nested_table_is_its_own_table_and_its_cells_words() {
        let body = r#"
            <table:table table:name="Outer">
              <table:table-column table:number-columns-repeated="2"/>
              <table:table-row>
                <table:table-cell><text:p>Outer left</text:p></table:table-cell>
                <table:table-cell>
                  <table:table table:name="Inner">
                    <table:table-column table:number-columns-repeated="2"/>
                    <table:table-row>
                      <table:table-cell><text:p>Inner A1</text:p></table:table-cell>
                      <table:table-cell><text:p>Inner B1</text:p></table:table-cell>
                    </table:table-row>
                    <table:table-row>
                      <table:table-cell><text:p>Inner A2</text:p></table:table-cell>
                      <table:table-cell><text:p>Inner B2</text:p></table:table-cell>
                    </table:table-row>
                  </table:table>
                </table:table-cell>
              </table:table-row>
            </table:table>"#;
        let doc = OdtDocument::parse_bytes(&package(&content(body), STYLES, &[])).unwrap();
        let outer = match &doc.items[0] {
            OdtItem::Table(table) => table,
            other => panic!("expected a table, got {other:?}"),
        };
        let holder = &outer.rows[0][1];
        assert_eq!(holder.text, "Inner A1\nInner B1\nInner A2\nInner B2");
        assert_eq!(holder.tables.len(), 1);
        let inner = &holder.tables[0];
        assert_eq!(inner.rows.len(), 2);
        assert_eq!(inner.rows[1][1].text, "Inner B2");
    }

    #[test]
    fn a_picture_takes_its_frame_size_and_its_bytes() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let body = r#"
            <text:p><draw:frame svg:width="2in" svg:height="1.5in">
              <draw:image xlink:href="Pictures/chart.png" draw:mime-type="image/png"/>
            </draw:frame></text:p>"#;
        let doc = OdtDocument::parse_bytes(&package(
            &content(body),
            STYLES,
            &[("Pictures/chart.png", &png)],
        ))
        .unwrap();
        match &doc.items[0] {
            OdtItem::Picture {
                mime,
                data,
                width_pt,
                height_pt,
            } => {
                assert_eq!(*mime, "image/png");
                assert_eq!(data, &png);
                assert_eq!(*width_pt, Some(144.0));
                assert_eq!(*height_pt, Some(108.0));
            }
            other => panic!("expected a picture, got {other:?}"),
        }
    }

    #[test]
    fn a_note_is_the_documents_and_comes_last() {
        let body = r#"<text:p>Claim<text:note text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>The evidence.</text:p></text:note-body></text:note></text:p>"#;
        let doc = OdtDocument::parse_bytes(&package(&content(body), STYLES, &[])).unwrap();
        let items = texts(&doc.items);
        assert_eq!(items[0].0, "Claim");
        assert_eq!(
            items.last().unwrap(),
            &("The evidence.".to_string(), "footnote".to_string(), None)
        );
    }
}
