//! Reading an OpenDocument spreadsheet into its sheets and their tables.
//!
//! The package is followed rather than the file name: `content.xml` holds the
//! whole workbook, its `office:spreadsheet` naming each worksheet as a
//! `table:table` in the order the workbook shows them. A sheet's cells are
//! `table:table-cell` elements, each stating its value with `office:value-type`
//! and the text a reader sees in its `text:p` paragraphs; a merged range is one
//! cell carrying `table:number-columns-spanned`/`number-rows-spanned` with
//! `table:covered-table-cell` placeholders over the slots it covers. What a
//! sheet's cells then form is decided the way Docling's own sheet backend
//! decides it, and the XLSX reader decides it the same way: a table is a region
//! of cells connected to each other (a merge is connected to itself), and an
//! empty row or column between two blocks of content leaves them two tables.
//!
//! A sheet is hidden by `table:display="false"`, which Calc writes in the table
//! style the sheet's `table:style-name` names rather than on the table itself,
//! so the automatic styles are read beside the sheets.
//!
//! The pictures and charts a sheet draws are read too. A cell's `draw:frame`
//! names either a `Pictures/` media part or an embedded `draw:object` chart;
//! its box is the anchoring cell on the same 64 × 20 pixel grid the XLSX reader
//! uses. A picture keeps its media type and bytes; a chart is read from its
//! object's `content.xml` as the same data table a worksheet's chart is.
//!
//! A cell's comment is read as well: the `office:annotation` Calc hangs on a
//! cell keeps its author in `dc:creator`, its time in `dc:date` and its words in
//! paragraphs, and it is a remark about the cell rather than the cell's text.
//!
//! Not read yet: tracked changes, and the formula itself — only the value it
//! displays is read.

use anyhow::{Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Cursor, Read};

use pdfextractor_picture_formats::{format_for_mime, mime_for_extension};
use pdfextractor_xlsx_parser::{Sheet, SheetCell, SheetPicture, SheetTable};

mod chart;

/// A `table:number-*-repeated` larger than this is filler rather than content:
/// Calc writes the rest of a sheet as one repeated empty cell or row.
const MAX_REPEAT: usize = 1024;

/// A picture or chart frame is placed in cells: one cell is Calc's default
/// 64-pixel column by 20-pixel row, the same grid the XLSX reader places a
/// drawing on. A frame states its size and offset in inches.
const PIXELS_PER_INCH: f64 = 96.0;
const COLUMN_PIXELS: f64 = 64.0;
const ROW_PIXELS: f64 = 20.0;

/// One OpenDocument spreadsheet.
#[derive(Debug, Clone)]
pub struct OdsDocument {
    pub sheets: Vec<Sheet>,
    /// The comments the workbook hangs on its cells, in the order they are read.
    pub annotations: Vec<SheetAnnotation>,
}

/// A comment on a cell.
///
/// It is about the cell, not a part of it: Calc writes it as an
/// `office:annotation` beside the cell's paragraphs, so the words are a remark
/// on the sheet rather than the cell's content.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetAnnotation {
    /// The sheet's number in the workbook, 1-based — the page it is read as.
    pub sheet: usize,
    /// The cell's row and column on the sheet, both 0-based.
    pub row: usize,
    pub col: usize,
    /// The name the comment's `dc:creator` states, when it states one.
    pub author: Option<String>,
    /// The time its `dc:date` states, when it states one.
    pub date: Option<String>,
    /// Its words, one paragraph per line.
    pub text: String,
}

impl OdsDocument {
    pub fn parse_file(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::parse_bytes(&data)
    }

    pub fn parse_bytes(data: &[u8]) -> Result<Self> {
        let mut archive = zip::ZipArchive::new(Cursor::new(data))
            .map_err(|e| anyhow::anyhow!("not an OpenDocument (ZIP) package: {e}"))?;
        let content = read_part(&mut archive, "content.xml")
            .ok_or_else(|| anyhow::anyhow!("package has no content.xml"))?;
        let media = read_parts(&mut archive, "Pictures/");
        let objects = read_chart_objects(&mut archive);

        // A hidden sheet is named by its style, and the style is stated in the
        // same part: read the whole part before any sheet is.
        let hidden = hidden_sheet_styles(&content)?;
        let (sheets, annotations) = read_sheets(&content, &hidden, &media, &objects)?;
        Ok(Self {
            sheets,
            annotations,
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

/// The embedded chart objects a package holds, keyed by the directory a
/// `draw:object` names (`Object 1`). Each is the chart's `content.xml`.
fn read_chart_objects(archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> HashMap<String, Vec<u8>> {
    let mut objects = HashMap::new();
    for index in 0..archive.len() {
        let Ok(mut file) = archive.by_index(index) else {
            continue;
        };
        let name = file.name().to_string();
        let Some(object) = name.strip_suffix("/content.xml") else {
            continue;
        };
        if !object.starts_with("Object ") {
            continue;
        }
        let mut buffer = Vec::new();
        if file.read_to_end(&mut buffer).is_ok() {
            objects.insert(object.to_string(), buffer);
        }
    }
    objects
}

/// The table styles that state `table:display="false"`: a sheet is hidden by
/// its style, not by an attribute on the table element.
fn hidden_sheet_styles(xml: &[u8]) -> Result<HashSet<String>> {
    let mut pull = Pull::new(xml);
    let mut hidden = HashSet::new();
    let mut current: Option<String> = None;
    loop {
        match pull.next()? {
            Tok::Start { name, attrs } if name == "style" => {
                current = if field(&attrs, "family") == Some("table") {
                    field(&attrs, "name").map(str::to_string)
                } else {
                    None
                };
            }
            Tok::Start { name, attrs } | Tok::Empty { name, attrs }
                if name == "table-properties" =>
            {
                let hidden_here = field(&attrs, "display")
                    .is_some_and(|value| value.eq_ignore_ascii_case("false"));
                if hidden_here {
                    hidden.extend(current.iter().cloned());
                }
            }
            Tok::End { name } if name == "style" => current = None,
            Tok::Eof => break,
            _ => {}
        }
    }
    Ok(hidden)
}

/// Every worksheet, in the workbook's order.
fn read_sheets(
    xml: &[u8],
    hidden_styles: &HashSet<String>,
    media: &HashMap<String, Vec<u8>>,
    objects: &HashMap<String, Vec<u8>>,
) -> Result<(Vec<Sheet>, Vec<SheetAnnotation>)> {
    let mut pull = Pull::new(xml);
    let mut sheets = Vec::new();
    let mut frames = Vec::new();
    let mut annotations = Vec::new();
    loop {
        match pull.next()? {
            Tok::Start { name, attrs } if name == "table" => {
                let index = sheets.len() + 1;
                let (sheet, sheet_frames, sheet_annotations) =
                    read_sheet(&mut pull, &attrs, index, hidden_styles)?;
                sheets.push(sheet);
                frames.push(sheet_frames);
                annotations.extend(sheet_annotations);
            }
            Tok::Eof => break,
            _ => {}
        }
    }

    let resolved: Vec<(Vec<SheetPicture>, Vec<pdfextractor_chart::ChartShape>)> = sheets
        .iter()
        .zip(&frames)
        .map(|(sheet, frames)| drawings(&sheets, &sheet.name, frames, media, objects))
        .collect();
    for (sheet, (pictures, charts)) in sheets.iter_mut().zip(resolved) {
        sheet.pictures = pictures;
        sheet.charts = charts;
    }
    Ok((sheets, annotations))
}

/// One `table:table`: its name, whether it is hidden, and the tables its cells
/// form.
fn read_sheet(
    pull: &mut Pull<'_>,
    attrs: &[(String, String)],
    index: usize,
    hidden_styles: &HashSet<String>,
) -> Result<(Sheet, Vec<Frame>, Vec<SheetAnnotation>)> {
    let name = field(attrs, "name")
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Sheet {index}"));
    let hidden = field(attrs, "display").is_some_and(|value| value.eq_ignore_ascii_case("false"))
        || field(attrs, "style-name").is_some_and(|style| hidden_styles.contains(style));

    let mut grid: HashMap<(usize, usize), Cell> = HashMap::new();
    // Slots a merge covers: the anchor holds the text, and these hold its place
    // in the grid so the region is connected and the slot is not written twice.
    let mut covered: HashSet<(usize, usize)> = HashSet::new();
    let mut row = 0usize;
    let mut frames = Vec::new();
    let mut annotations = Vec::new();

    loop {
        match pull.next()? {
            Tok::Start { name, attrs } if name == "table-row" => {
                let repeat = count(&attrs, "number-rows-repeated").min(MAX_REPEAT);
                let mut row_frames = Vec::new();
                let mut row_annotations = Vec::new();
                let cells = read_row(pull, row, &mut row_frames, &mut row_annotations)?;
                for copy in 0..repeat {
                    for cell in &cells {
                        place(&mut grid, &mut covered, row + copy, cell);
                    }
                    for frame in &row_frames {
                        let mut frame = frame.clone();
                        frame.row += copy;
                        frames.push(frame);
                    }
                    // A comment is repeated with the row it is on, so it stays
                    // on the cell it was written against.
                    for note in &row_annotations {
                        annotations.push(SheetAnnotation {
                            sheet: index,
                            row: row + copy,
                            col: note.col,
                            author: note.author.clone(),
                            date: note.date.clone(),
                            text: note.text.clone(),
                        });
                    }
                }
                row = row.saturating_add(repeat);
            }
            Tok::End { name } if name == "table" => break,
            Tok::Eof => break,
            _ => {}
        }
    }

    Ok((
        Sheet {
            name,
            hidden,
            tables: tables(&grid, &covered),
            pictures: Vec::new(),
            charts: Vec::new(),
        },
        frames,
        annotations,
    ))
}

/// One cell a row holds, at the column it starts at.
struct RowCell {
    col: usize,
    text: String,
    row_span: usize,
    col_span: usize,
}

/// A `draw:frame` found in a sheet cell: its anchor and the media or chart it
/// holds. The frame's own position is relative to the cell that contains it.
#[derive(Debug, Clone)]
struct Frame {
    row: usize,
    col: usize,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    image: Option<String>,
    image_mime: Option<String>,
    object: Option<String>,
}

impl Frame {
    /// The picture's box in cells, on the same 64 × 20 pixel grid the XLSX
    /// reader places a drawing on. A frame's own `svg:x`/`svg:y` is its offset
    /// inside the anchoring cell.
    fn bbox(&self) -> [f64; 4] {
        let left = self.col as f64 + self.x * PIXELS_PER_INCH / COLUMN_PIXELS;
        let top = self.row as f64 + self.y * PIXELS_PER_INCH / ROW_PIXELS;
        [
            left,
            top,
            left + self.width * PIXELS_PER_INCH / COLUMN_PIXELS,
            top + self.height * PIXELS_PER_INCH / ROW_PIXELS,
        ]
    }
}

/// A comment found in a cell, before the sheet and row it sits on are known.
#[derive(Clone)]
struct CellAnnotation {
    col: usize,
    author: Option<String>,
    date: Option<String>,
    text: String,
}

/// Repeat one cell's frames and comments for the copies
/// `table:number-columns-repeated` asks for, each in its own column.
fn repeat_cell(
    frames: &mut Vec<Frame>,
    annotations: &mut Vec<CellAnnotation>,
    cell_frames: &[Frame],
    cell_annotations: &[CellAnnotation],
    copy: usize,
    col_span: usize,
) {
    for frame in cell_frames {
        let mut frame = frame.clone();
        frame.col = frame.col.saturating_add(copy * col_span);
        frames.push(frame);
    }
    for annotation in cell_annotations {
        let mut annotation = annotation.clone();
        annotation.col = annotation.col.saturating_add(copy * col_span);
        annotations.push(annotation);
    }
}

/// Read one `office:annotation`: its author, its time and its words.
///
/// Calc writes a cell's comment as an `office:annotation` beside the cell's
/// paragraphs, with the name in `dc:creator`, the time in `dc:date` and the
/// words in one or more paragraphs joined by a newline. An annotation with no
/// words is no remark and is not one.
fn read_annotation(
    pull: &mut Pull<'_>,
) -> Result<Option<(Option<String>, Option<String>, String)>> {
    let mut author = None;
    let mut date = None;
    let mut paragraphs: Vec<String> = Vec::new();
    loop {
        match pull.next()? {
            Tok::Start { name, .. } => match name.as_str() {
                "creator" => author = Some(read_inline(pull)?.trim().to_string()),
                "date" => date = Some(read_inline(pull)?.trim().to_string()),
                "p" | "h" => paragraphs.push(read_inline(pull)?),
                _ => skip_element(pull)?,
            },
            Tok::End { .. } => break,
            Tok::Eof => break,
            _ => {}
        }
    }
    let text = paragraphs
        .iter()
        .map(|paragraph| paragraph.trim())
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some((
        author.filter(|author| !author.is_empty()),
        date.filter(|date| !date.is_empty()),
        text,
    )))
}

/// One cell of the grid a sheet is read onto.
struct Cell {
    text: String,
    row_span: usize,
    col_span: usize,
}

/// Put a cell on the grid, and record the slots a span covers.
fn place(
    grid: &mut HashMap<(usize, usize), Cell>,
    covered: &mut HashSet<(usize, usize)>,
    row: usize,
    cell: &RowCell,
) {
    if cell.text.trim().is_empty() && cell.row_span <= 1 && cell.col_span <= 1 {
        return;
    }
    grid.insert(
        (row, cell.col),
        Cell {
            text: cell.text.clone(),
            row_span: cell.row_span,
            col_span: cell.col_span,
        },
    );
    for offset_row in 0..cell.row_span {
        for offset_col in 0..cell.col_span {
            if offset_row == 0 && offset_col == 0 {
                continue;
            }
            covered.insert((row + offset_row, cell.col + offset_col));
        }
    }
}

/// One `table:table-row`, its cells placed left to right.
///
/// A cell repeated by `table:number-columns-repeated` is as many cells as it
/// says; a `table:covered-table-cell` is a slot a merge covers, so it advances
/// the column without becoming a cell of its own. An empty repeated cell is the
/// filler a producer writes for the rest of the sheet: it moves the column on
/// without costing a cell.
fn read_row(
    pull: &mut Pull<'_>,
    row: usize,
    frames: &mut Vec<Frame>,
    annotations: &mut Vec<CellAnnotation>,
) -> Result<Vec<RowCell>> {
    let mut cells = Vec::new();
    let mut col = 0usize;
    loop {
        match pull.next()? {
            Tok::Start { name, attrs } => match name.as_str() {
                "table-cell" => {
                    let row_span = count(&attrs, "number-rows-spanned");
                    let col_span = count(&attrs, "number-columns-spanned");
                    let repeat = count(&attrs, "number-columns-repeated");
                    let mut cell_frames = Vec::new();
                    let mut cell_annotations = Vec::new();
                    let text = read_cell(
                        pull,
                        &attrs,
                        row,
                        col,
                        &mut cell_frames,
                        &mut cell_annotations,
                    )?;
                    if text.trim().is_empty() && row_span == 1 && col_span == 1 {
                        for copy in 0..repeat.min(MAX_REPEAT) {
                            repeat_cell(
                                frames,
                                annotations,
                                &cell_frames,
                                &cell_annotations,
                                copy,
                                col_span,
                            );
                        }
                        col = col.saturating_add(repeat);
                        continue;
                    }
                    for copy in 0..repeat.min(MAX_REPEAT) {
                        cells.push(RowCell {
                            col,
                            text: text.clone(),
                            row_span,
                            col_span,
                        });
                        repeat_cell(
                            frames,
                            annotations,
                            &cell_frames,
                            &cell_annotations,
                            copy,
                            col_span,
                        );
                        col = col.saturating_add(col_span);
                    }
                }
                "covered-table-cell" => {
                    col = col.saturating_add(count(&attrs, "number-columns-repeated"));
                }
                _ => {}
            },
            Tok::Empty { name, attrs } => match name.as_str() {
                "table-cell" => {
                    let row_span = count(&attrs, "number-rows-spanned");
                    let col_span = count(&attrs, "number-columns-spanned");
                    let repeat = count(&attrs, "number-columns-repeated");
                    // A cell written with no paragraphs still states its value
                    // in its attributes; only a cell with neither is a slot
                    // with nothing in it. A merge over empty cells is still
                    // something the sheet drew, so a spanning empty cell is
                    // placed.
                    let text = typed_value(&attrs);
                    if text.trim().is_empty() && row_span == 1 && col_span == 1 {
                        col = col.saturating_add(repeat);
                        continue;
                    }
                    for _ in 0..repeat.min(MAX_REPEAT) {
                        cells.push(RowCell {
                            col,
                            text: text.clone(),
                            row_span,
                            col_span,
                        });
                        col = col.saturating_add(col_span);
                    }
                }
                "covered-table-cell" => {
                    col = col.saturating_add(count(&attrs, "number-columns-repeated"));
                }
                _ => {}
            },
            Tok::End { name } if name == "table-row" => break,
            Tok::Eof => break,
            _ => {}
        }
    }
    Ok(cells)
}

/// The text a `table:table-cell` holds: its paragraphs, in order, or the typed
/// value when the producer wrote no display text.
///
/// The displayed text is what the document says the cell says — a formula's
/// cached result, a number as its format prints it, a date as its format writes
/// it — so it is preferred, except for a boolean: that is `True`/`False` as
/// Docling writes it and the XLSX reader spells it, whatever words a producer
/// displays for it. A typed value fills the cells a producer leaves without
/// paragraphs, and a date, a time, a string or a number is the value the element
/// states.
fn read_cell(
    pull: &mut Pull<'_>,
    attrs: &[(String, String)],
    row: usize,
    col: usize,
    frames: &mut Vec<Frame>,
    annotations: &mut Vec<CellAnnotation>,
) -> Result<String> {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut loose = String::new();
    loop {
        match pull.next()? {
            Tok::Text(text) | Tok::CData(text) => loose.push_str(&text),
            Tok::Start { name, attrs } => match name.as_str() {
                "p" | "h" => {
                    let text = read_inline(pull)?;
                    paragraphs.push(text);
                }
                "s" => loose.push_str(&" ".repeat(count(&attrs, "c"))),
                "tab" => loose.push('\t'),
                "line-break" => loose.push('\n'),
                // A cell's comment is not its text: it is read as the remark
                // it is, with the author and time the file states, and the cell
                // keeps only what it displays. A frame's words are the frame's,
                // and the frame itself is a picture or a chart.
                "annotation" => {
                    if let Some((author, date, text)) = read_annotation(pull)? {
                        annotations.push(CellAnnotation {
                            col,
                            author,
                            date,
                            text,
                        });
                    }
                }
                "note" | "object" => skip_element(pull)?,
                "frame" => read_frame(pull, &attrs, row, col, frames)?,
                _ => {}
            },
            Tok::Empty { name, attrs } => match name.as_str() {
                "s" => loose.push_str(&" ".repeat(count(&attrs, "c"))),
                "tab" => loose.push('\t'),
                "line-break" => loose.push('\n'),
                _ => {}
            },
            Tok::End { name } if name == "table-cell" => break,
            Tok::Eof => break,
            _ => {}
        }
    }
    if !loose.trim().is_empty() {
        paragraphs.push(loose);
    }
    let text = paragraphs
        .iter()
        .map(|paragraph| paragraph.trim())
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    // A boolean is the value it states rather than the words it displays: Calc
    // writes `TRUE`, and the document Docling reads states `True`. The XLSX
    // reader gives a workbook's boolean the same spelling.
    if field(attrs, "value-type") == Some("boolean") {
        return Ok(typed_value(attrs));
    }
    if text.is_empty() {
        Ok(typed_value(attrs))
    } else {
        Ok(text)
    }
}

/// Read one `draw:frame` up to its closing tag and keep the media or chart it
/// holds. A chart frame also carries a preview `draw:image`; the object is the
/// chart, so the preview is not reported as a picture of it.
fn read_frame(
    pull: &mut Pull<'_>,
    attrs: &[(String, String)],
    row: usize,
    col: usize,
    frames: &mut Vec<Frame>,
) -> Result<()> {
    let mut frame = Frame {
        row,
        col,
        x: field(attrs, "x").and_then(length_in_inches).unwrap_or(0.0),
        y: field(attrs, "y").and_then(length_in_inches).unwrap_or(0.0),
        width: field(attrs, "width")
            .and_then(length_in_inches)
            .unwrap_or(0.0),
        height: field(attrs, "height")
            .and_then(length_in_inches)
            .unwrap_or(0.0),
        image: None,
        image_mime: None,
        object: None,
    };
    loop {
        match pull.next()? {
            Tok::Start { name, attrs } if name == "image" => {
                capture_image(&mut frame, &attrs);
                skip_element(pull)?;
            }
            Tok::Empty { name, attrs } if name == "image" => {
                capture_image(&mut frame, &attrs);
            }
            Tok::Start { name, attrs } if name == "object" => {
                if let Some(href) = field(&attrs, "href") {
                    frame.object = Some(href.to_string());
                }
                skip_element(pull)?;
            }
            Tok::Empty { name, attrs } if name == "object" => {
                if let Some(href) = field(&attrs, "href") {
                    frame.object = Some(href.to_string());
                }
            }
            Tok::End { name } if name == "frame" => break,
            Tok::Start { .. } => skip_element(pull)?,
            Tok::Eof => break,
            _ => {}
        }
    }
    if frame.image.is_some() || frame.object.is_some() {
        frames.push(frame);
    }
    Ok(())
}

fn capture_image(frame: &mut Frame, attrs: &[(String, String)]) {
    if let Some(href) = field(attrs, "href") {
        frame.image = Some(href.to_string());
    }
    if let Some(mime) = field(attrs, "mime-type") {
        frame.image_mime = Some(mime.to_string());
    }
}

fn length_in_inches(value: &str) -> Option<f64> {
    let value = value.trim();
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.trim().parse().ok()?;
    Some(match unit.trim().to_ascii_lowercase().as_str() {
        "in" => number,
        "cm" => number / 2.54,
        "mm" => number / 25.4,
        "pt" => number / 72.0,
        "pc" => number / 6.0,
        "px" => number / PIXELS_PER_INCH,
        "" => number,
        _ => return None,
    })
}

/// Resolve the frames a sheet anchored into the pictures and charts the shared
/// workbook model holds.
fn drawings(
    sheets: &[Sheet],
    sheet_name: &str,
    frames: &[Frame],
    media: &HashMap<String, Vec<u8>>,
    objects: &HashMap<String, Vec<u8>>,
) -> (Vec<SheetPicture>, Vec<pdfextractor_chart::ChartShape>) {
    let mut pictures = Vec::new();
    let mut charts = Vec::new();
    for frame in frames {
        if let Some(object) = &frame.object {
            let object = object.trim_start_matches("./");
            if let Some(bytes) = objects.get(object) {
                if let Some(mut chart) = chart::read_chart(bytes, sheet_name, sheets) {
                    chart.bbox = frame.bbox();
                    charts.push(chart);
                }
            }
            // A chart frame's `draw:image` is its replacement preview, not a
            // second picture of the chart.
            continue;
        }
        let Some(href) = &frame.image else {
            continue;
        };
        let href = href.trim_start_matches("./");
        let Some(data) = media.get(href) else {
            continue;
        };
        let mime = frame
            .image_mime
            .as_deref()
            .map(|mime| mime.split(';').next().unwrap_or(mime).trim())
            .and_then(format_for_mime)
            .map(|format| format.mime)
            .or_else(|| {
                href.rsplit_once('.')
                    .and_then(|(_, extension)| mime_for_extension(extension))
            });
        if let Some(mime) = mime {
            pictures.push(SheetPicture {
                data: data.clone(),
                mime,
                bbox: frame.bbox(),
            });
        }
    }
    (pictures, charts)
}

/// The value a cell states when it displays no text.
fn typed_value(attrs: &[(String, String)]) -> String {
    match field(attrs, "value-type") {
        Some("string") => field(attrs, "string-value").unwrap_or_default().to_string(),
        Some("date") => field(attrs, "date-value").unwrap_or_default().to_string(),
        Some("time") => field(attrs, "time-value").unwrap_or_default().to_string(),
        Some("boolean") => match field(attrs, "boolean-value") {
            Some("true") => "True".to_string(),
            Some("false") => "False".to_string(),
            other => other.unwrap_or_default().to_string(),
        },
        Some(_) => field(attrs, "value").unwrap_or_default().to_string(),
        None => String::new(),
    }
}

/// The inline content of one `text:p`/`text:h`, up to its closing tag.
fn read_inline(pull: &mut Pull<'_>) -> Result<String> {
    let mut text = String::new();
    let mut depth = 0usize;
    loop {
        match pull.next()? {
            Tok::Text(chunk) | Tok::CData(chunk) => text.push_str(&chunk),
            Tok::Start { name, attrs } => match name.as_str() {
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
                // Each consumes its own closing tag, so none adds to the depth.
                "frame" | "annotation" | "note" | "object" => skip_element(pull)?,
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
    Ok(text)
}

/// Cut a sheet into its connected regions, each a table.
///
/// Content is a cell with text *or* a span's covered slots: a merge over empty
/// cells is still something the sheet drew. Connectivity is four-directional
/// and has no tolerance for gaps, so one empty row between two blocks leaves
/// them two tables — the reading Docling's sheet backend uses, and the one the
/// XLSX reader produces.
fn tables(
    grid: &HashMap<(usize, usize), Cell>,
    covered: &HashSet<(usize, usize)>,
) -> Vec<SheetTable> {
    let mut content: HashSet<(usize, usize)> = grid.keys().copied().collect();
    content.extend(covered.iter().copied());

    let mut starts: Vec<(usize, usize)> = content.iter().copied().collect();
    starts.sort_unstable();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let mut tables = Vec::new();
    for start in starts {
        if !seen.insert(start) {
            continue;
        }
        let (min_row, min_col, max_row, max_col) = region(start, &content, &mut seen);
        tables.push(table_of(grid, covered, min_row, min_col, max_row, max_col));
    }
    tables
}

/// The bounding box of the content connected to `start`, marking every cell of
/// it visited.
fn region(
    start: (usize, usize),
    content: &HashSet<(usize, usize)>,
    seen: &mut HashSet<(usize, usize)>,
) -> (usize, usize, usize, usize) {
    let (mut min_row, mut min_col) = start;
    let (mut max_row, mut max_col) = start;
    let mut queue = VecDeque::from([start]);
    while let Some((row, col)) = queue.pop_front() {
        min_row = min_row.min(row);
        max_row = max_row.max(row);
        min_col = min_col.min(col);
        max_col = max_col.max(col);
        let neighbours = [
            (row.wrapping_sub(1), col),
            (row + 1, col),
            (row, col.wrapping_sub(1)),
            (row, col + 1),
        ];
        for neighbour in neighbours {
            if content.contains(&neighbour) && seen.insert(neighbour) {
                queue.push_back(neighbour);
            }
        }
    }
    (min_row, min_col, max_row, max_col)
}

/// Lay a region out on its grid: every slot of the bounding box is a cell
/// except the slots a merge covers, which its spanning cell holds.
fn table_of(
    grid: &HashMap<(usize, usize), Cell>,
    covered: &HashSet<(usize, usize)>,
    min_row: usize,
    min_col: usize,
    max_row: usize,
    max_col: usize,
) -> SheetTable {
    let mut cells = Vec::new();
    for row in min_row..=max_row {
        // A row ends at its last cell: the empty slots after it are the grid's
        // padding, which Docling does not report as cells. An empty slot with a
        // cell after it stays, because the sheet drew a gap there, and a slot a
        // merge covers is part of its spanning cell, so it reaches the row too.
        let last = (min_col..=max_col)
            .rev()
            .find(|col| covered.contains(&(row, *col)) || grid.contains_key(&(row, *col)));
        let Some(last) = last else {
            continue;
        };
        for col in min_col..=last {
            if covered.contains(&(row, col)) {
                continue;
            }
            let (text, row_span, col_span) = grid
                .get(&(row, col))
                .map(|cell| (cell.text.clone(), cell.row_span, cell.col_span))
                .unwrap_or_default();
            cells.push(SheetCell {
                row: row - min_row,
                col: col - min_col,
                text,
                row_span,
                col_span,
            });
        }
    }
    SheetTable {
        anchor: (min_col, min_row),
        num_rows: max_row - min_row + 1,
        num_cols: max_col - min_col + 1,
        cells,
    }
}

/// One XML token, with local names: `table:table-cell` is a `table-cell` and
/// `office:value-type` a `value-type`, whatever prefix the producer chose.
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
    buffer: Vec<u8>,
}

impl<'a> Pull<'a> {
    fn new(xml: &'a [u8]) -> Self {
        let mut reader = Reader::from_reader(Cursor::new(xml));
        reader.config_mut().trim_text(false);
        Self {
            reader,
            buffer: Vec::new(),
        }
    }

    fn next(&mut self) -> Result<Tok> {
        loop {
            self.buffer.clear();
            match self.reader.read_event_into(&mut self.buffer) {
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
                        name: String::from_utf8_lossy(local_name(end.name().as_ref())).into_owned(),
                    })
                }
                Ok(Event::Text(text)) => {
                    let text = text
                        .unescape()
                        .context("malformed text in an OpenDocument part")?;
                    return Ok(Tok::Text(text.into_owned()));
                }
                Ok(Event::CData(text)) => {
                    return Ok(Tok::CData(String::from_utf8_lossy(&text).into_owned()))
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
    match name.iter().rposition(|byte| *byte == b':') {
        Some(index) => &name[index + 1..],
        None => name,
    }
}

fn local(element: &BytesStart) -> String {
    String::from_utf8_lossy(local_name(element.name().as_ref())).into_owned()
}

fn attributes(element: &BytesStart) -> Vec<(String, String)> {
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

/// Throw away one element and everything under it.
fn skip_element(pull: &mut Pull<'_>) -> Result<()> {
    let mut depth = 0usize;
    loop {
        match pull.next()? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(content: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("mimetype", options).unwrap();
        writer
            .write_all(b"application/vnd.oasis.opendocument.spreadsheet")
            .unwrap();
        writer.start_file("content.xml", options).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
        writer.start_file("styles.xml", options).unwrap();
        writer
            .write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"/>"#,
            )
            .unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn content(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0">
 <office:automatic-styles>
  <style:style style:name="ta_hidden" style:family="table">
   <style:table-properties table:display="false"/>
  </style:style>
 </office:automatic-styles>
 <office:body><office:spreadsheet>{body}</office:spreadsheet></office:body>
</office:document-content>"#
        )
    }

    fn texts(sheet: &Sheet) -> Vec<String> {
        sheet
            .tables
            .iter()
            .flat_map(|table| table.cells.iter().map(|cell| cell.text.clone()))
            .collect()
    }

    #[test]
    fn sheets_keep_their_order_and_a_hidden_one_stays() {
        let body = r#"
            <table:table table:name="Visible">
              <table:table-row>
                <table:table-cell><text:p>Item</text:p></table:table-cell>
                <table:table-cell><text:p>Qty</text:p></table:table-cell>
              </table:table-row>
            </table:table>
            <table:table table:name="Hidden" table:style-name="ta_hidden">
              <table:table-row>
                <table:table-cell office:value-type="float" office:value="42"/>
              </table:table-row>
            </table:table>"#;
        let document = OdsDocument::parse_bytes(&package(&content(body))).unwrap();
        assert_eq!(
            document
                .sheets
                .iter()
                .map(|sheet| sheet.name.as_str())
                .collect::<Vec<_>>(),
            ["Visible", "Hidden"]
        );
        assert!(!document.sheets[0].hidden);
        assert!(
            document.sheets[1].hidden,
            "the style's table:display=false hides the sheet"
        );
        assert_eq!(texts(&document.sheets[0]), ["Item", "Qty"]);
        assert_eq!(
            texts(&document.sheets[1]),
            ["42"],
            "a cell with no display text reads its typed value"
        );
    }

    #[test]
    fn values_read_as_a_reader_sees_them() {
        let body = r#"
            <table:table table:name="Values">
              <table:table-row>
                <table:table-cell office:value-type="string"><text:p>Widget</text:p></table:table-cell>
                <table:table-cell office:value-type="float" office:value="1.5"><text:p>1.5</text:p></table:table-cell>
                <table:table-cell office:value-type="boolean" office:boolean-value="true"/>
                <table:table-cell office:value-type="date" office:date-value="2024-03-05"/>
                <table:table-cell office:value-type="float" office:value="15" table:formula="of:=SUM([.B2:.B3])"><text:p>15</text:p></table:table-cell>
                <table:table-cell office:value-type="boolean" office:boolean-value="false"><text:p>FALSE</text:p></table:table-cell>
              </table:table-row>
            </table:table>"#;
        let document = OdsDocument::parse_bytes(&package(&content(body))).unwrap();
        assert_eq!(
            texts(&document.sheets[0]),
            ["Widget", "1.5", "True", "2024-03-05", "15", "False"],
            "a formula reads the value it displays, not its markup, and a boolean \
             reads its value rather than the words a producer shows"
        );
    }

    #[test]
    fn a_row_ends_at_its_last_cell() {
        let body = r#"
            <table:table table:name="Region">
              <table:table-row>
                <table:table-cell/>
                <table:table-cell table:number-columns-spanned="2"><text:p>Shipping included</text:p></table:table-cell>
                <table:covered-table-cell/>
              </table:table-row>
              <table:table-row>
                <table:table-cell><text:p>Total</text:p></table:table-cell>
                <table:table-cell><text:p>15</text:p></table:table-cell>
                <table:table-cell/>
              </table:table-row>
            </table:table>"#;
        let document = OdsDocument::parse_bytes(&package(&content(body))).unwrap();
        let table = &document.sheets[0].tables[0];
        // The merge still gives the table its width, and the empty slot before
        // it stays: the sheet drew a gap with a cell after it.
        assert_eq!((table.num_rows, table.num_cols), (2, 3));
        assert_eq!(
            table
                .cells
                .iter()
                .map(|cell| (cell.row, cell.col, cell.text.as_str()))
                .collect::<Vec<_>>(),
            [
                (0, 0, ""),
                (0, 1, "Shipping included"),
                (1, 0, "Total"),
                (1, 1, "15"),
            ],
            "the empty slot after the row's last cell is padding, not a cell"
        );
    }

    #[test]
    fn a_cells_comment_is_a_remark_not_its_text() {
        let body = r#"
            <table:table table:name="Review">
              <table:table-row>
                <table:table-cell><text:p>Widget</text:p></table:table-cell>
                <table:table-cell office:value-type="float" office:value="2">
                  <office:annotation office:display="true">
                    <dc:creator>Ada Lovelace</dc:creator>
                    <dc:date>2024-03-05T09:15:00</dc:date>
                    <text:p>Checked against the invoice.</text:p>
                  </office:annotation>
                  <text:p>2</text:p>
                </table:table-cell>
              </table:table-row>
              <table:table-row>
                <table:table-cell/>
                <table:table-cell>
                  <office:annotation office:display="true">
                    <dc:creator>Grace Hopper</dc:creator>
                    <dc:date>2024-03-06T10:30:00</dc:date>
                    <text:p>An empty cell has a remark too.</text:p>
                  </office:annotation>
                </table:table-cell>
              </table:table-row>
            </table:table>"#;
        let document = OdsDocument::parse_bytes(&package(&content(body))).unwrap();
        assert_eq!(
            texts(&document.sheets[0]),
            ["Widget", "2"],
            "a comment is not the cell's text"
        );
        assert_eq!(document.annotations.len(), 2, "both comments were read");
        let first = &document.annotations[0];
        assert_eq!(first.sheet, 1);
        assert_eq!((first.row, first.col), (0, 1));
        assert_eq!(first.author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(first.date.as_deref(), Some("2024-03-05T09:15:00"));
        assert_eq!(first.text, "Checked against the invoice.");
        // A comment on a cell with no text is still a remark about it.
        let second = &document.annotations[1];
        assert_eq!((second.row, second.col), (1, 1));
        assert_eq!(second.text, "An empty cell has a remark too.");
    }

    #[test]
    fn a_merge_keeps_its_spans_and_its_covered_slots() {
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
        let document = OdsDocument::parse_bytes(&package(&content(body))).unwrap();
        let table = &document.sheets[0].tables[0];
        assert_eq!(table.num_cols, 3);
        assert_eq!(table.num_rows, 3);
        assert_eq!(table.anchor, (0, 0));
        let revenue = table
            .cells
            .iter()
            .find(|cell| cell.text == "Revenue")
            .expect("the spanning cell");
        assert_eq!(revenue.col_span, 2);
        assert_eq!((revenue.row, revenue.col), (0, 1));
        // The covered placeholder is part of the span, not a cell of its own.
        assert_eq!(table.cells.len(), 7);
        let north = table
            .cells
            .iter()
            .find(|cell| cell.text == "North\nSouth")
            .expect("the row-spanning cell");
        assert_eq!((north.row, north.col), (1, 0));
        assert_eq!(north.row_span, 2);
        assert!(table
            .cells
            .iter()
            .all(|cell| (cell.row, cell.col) != (2, 0)));
    }

    #[test]
    fn an_empty_row_splits_and_the_filler_does_not_inflate() {
        let body = r#"
            <table:table table:name="Regions">
              <table:table-column table:number-columns-repeated="16384"/>
              <table:table-row>
                <table:table-cell><text:p>a</text:p></table:table-cell>
                <table:table-cell><text:p>b</text:p></table:table-cell>
                <table:table-cell table:number-columns-repeated="16382"/>
              </table:table-row>
              <table:table-row>
                <table:table-cell table:number-columns-repeated="16384"/>
              </table:table-row>
              <table:table-row table:number-rows-repeated="2">
                <table:table-cell><text:p>c</text:p></table:table-cell>
                <table:table-cell><text:p>d</text:p></table:table-cell>
                <table:table-cell table:number-columns-repeated="16382"/>
              </table:table-row>
              <table:table-row table:number-rows-repeated="1048570">
                <table:table-cell table:number-columns-repeated="16384"/>
              </table:table-row>
            </table:table>"#;
        let document = OdsDocument::parse_bytes(&package(&content(body))).unwrap();
        let sheet = &document.sheets[0];
        assert_eq!(sheet.tables.len(), 2, "the empty row separates two tables");
        let first = &sheet.tables[0];
        assert_eq!(first.anchor, (0, 0));
        assert_eq!((first.num_rows, first.num_cols), (1, 2));
        assert_eq!(texts(sheet), ["a", "b", "c", "d", "c", "d"]);
        let second = &sheet.tables[1];
        assert_eq!(second.anchor, (0, 2));
        assert_eq!(
            (second.num_rows, second.num_cols),
            (2, 2),
            "a repeated row is repeated, and trailing filler is not a column"
        );
    }
}
