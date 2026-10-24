//! Reading an XLSX workbook into its sheets and their tables.
//!
//! The parser follows the package rather than the file name: the workbook part
//! names the sheets and their order, the relationship part says which worksheet
//! part each name points at, and every worksheet part is read for its cells,
//! its merged ranges and the styles that make a number a date. A sheet's drawing
//! part is followed as well: the pictures it anchors are media parts, and the
//! charts it frames are chart parts — drawings of data — read as the numbers
//! they plot rather than as pictures of them. What a sheet's cells then form is
//! decided the way Docling's own XLSX backend decides it: a
//! table is a region of cells connected to each other (a merged range is
//! connected to itself), and an empty row or column between two blocks of
//! content leaves them two tables rather than one.

use anyhow::{Context, Result};
use chrono::{NaiveDate, TimeDelta, Timelike};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Cursor, Read};

/// One worksheet: its name, whether it is hidden, and the tables it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    pub name: String,
    /// A sheet that is not `visible` — `hidden` or `veryHidden` — which
    /// Docling keeps as an `invisible` content layer rather than dropping.
    pub hidden: bool,
    /// The sheet's data regions in reading order: top to bottom, then left to
    /// right. A sheet with no cells has none.
    pub tables: Vec<SheetTable>,
    /// The pictures the sheet draws, in the order its drawing part anchors
    /// them. A sheet with no drawing has none.
    pub pictures: Vec<SheetPicture>,
    /// The charts the sheet draws, in the order its drawing part anchors them
    /// — the data each one plots, its family and its own title. A chart is not
    /// a picture file in the package: it is a drawing of data, held as a chart
    /// part, so it reaches the document as a picture with no image behind it.
    pub charts: Vec<pdfextractor_chart::ChartShape>,
}

/// A picture a worksheet draws, and where on its grid it was placed.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetPicture {
    /// The media part's bytes exactly as the package stores them. A BMP is a
    /// BMP here: which encoding a picture is *handed on* as is the writer's
    /// question, not the package's.
    pub data: Vec<u8>,
    /// The media type the part name states — `image/bmp` for `image1.bmp`.
    pub mime: &'static str,
    /// The rectangle the picture occupies, `[left, top, right, bottom]` in
    /// cells: the same units the sheet's tables are placed in.
    pub bbox: [f64; 4],
}

/// A connected region of a worksheet, laid out on its own grid.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetTable {
    /// The top-left cell of the region, `(column, row)`, both 0-based — the
    /// coordinate Docling names in the table's provenance box.
    pub anchor: (usize, usize),
    pub num_rows: usize,
    pub num_cols: usize,
    /// The cells that are the table, with `row`/`col` relative to the anchor.
    /// A cell a merge covers is not here: the merge's own cell carries its span.
    pub cells: Vec<SheetCell>,
}

/// One cell of a [`SheetTable`]: its text, its place in the table, and how far
/// a merged range carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetCell {
    pub row: usize,
    pub col: usize,
    pub text: String,
    pub row_span: usize,
    pub col_span: usize,
}

/// A parsed workbook.
#[derive(Debug, Clone)]
pub struct XlsxDocument {
    pub sheets: Vec<Sheet>,
}

impl XlsxDocument {
    /// Read a whole `.xlsx` package from memory.
    pub fn parse_bytes(data: &[u8]) -> Result<Self> {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(data)).context("the XLSX is not a ZIP package")?;

        let workbook = read_part(&mut archive, "xl/workbook.xml")
            .context("the package has no xl/workbook.xml")?;
        let book = read_workbook(&workbook)?;
        let rels = read_part(&mut archive, "xl/_rels/workbook.xml.rels")
            .map(|bytes| read_relationships(&bytes))
            .transpose()?
            .unwrap_or_default();
        let strings = read_part(&mut archive, "xl/sharedStrings.xml")
            .map(|bytes| read_shared_strings(&bytes))
            .transpose()?
            .unwrap_or_default();
        let styles = read_part(&mut archive, "xl/styles.xml")
            .map(|bytes| read_styles(&bytes))
            .transpose()?
            .unwrap_or_default();

        // Every sheet's cells are read before any drawing is, because a chart
        // part may carry its numbers as references (`'Quarterly'!$B$2:$B$4`)
        // rather than as caches, and what they name is a sheet's cells — its
        // own, or another sheet's.
        let mut parts = Vec::with_capacity(book.sheets.len());
        let mut raw_sheets = Vec::with_capacity(book.sheets.len());
        for (index, sheet) in book.sheets.iter().enumerate() {
            // A sheet names its part through a relationship; a package with
            // no relationship for it (a hand-edited file) falls back to the
            // part name the format conventionally uses.
            let part = sheet
                .relationship
                .as_ref()
                .and_then(|id| rels.get(id))
                .cloned()
                .unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", index + 1));
            let bytes = read_part(&mut archive, &part)
                .with_context(|| format!("worksheet {:?} has no part at {part}", sheet.name))?;
            let reader = SheetReader::new(&strings, &styles, book.date1904);
            raw_sheets.push(reader.read(&bytes)?);
            parts.push((sheet.name.clone(), part, bytes));
        }

        let cells: Vec<(&str, &RawSheet)> = parts
            .iter()
            .zip(&raw_sheets)
            .map(|((name, _, _), raw)| (name.as_str(), raw))
            .collect();

        let mut sheets = Vec::with_capacity(book.sheets.len());
        for (index, sheet) in book.sheets.iter().enumerate() {
            let (name, part, bytes) = &parts[index];
            // A reference with no sheet name of its own is the drawing's sheet.
            let resolve = |reference: &str| resolve_chart_reference(reference, name, &cells);
            let drawing = read_sheet_drawing(&mut archive, part, bytes, &resolve)?;
            sheets.push(Sheet {
                name: name.clone(),
                hidden: !sheet.state.eq_ignore_ascii_case("visible"),
                tables: raw_sheets[index].tables(),
                pictures: drawing.pictures,
                charts: drawing.charts,
            });
        }

        Ok(Self { sheets })
    }
}

// --- the package ----------------------------------------------------------

fn read_part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut file = archive.by_name(name).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

/// One `<sheet>` of `xl/workbook.xml`.
struct SheetRef {
    name: String,
    state: String,
    /// The `r:id` naming this sheet's worksheet part.
    relationship: Option<String>,
}

struct Book {
    sheets: Vec<SheetRef>,
    /// The workbook's date system: 1900 (the default) or 1904 (the Mac one),
    /// which shifts every serial by four years and a day.
    date1904: bool,
}

fn read_workbook(bytes: &[u8]) -> Result<Book> {
    let mut reader = Reader::from_reader(Cursor::new(bytes));
    let mut buffer = Vec::new();
    let mut sheets = Vec::new();
    let mut date1904 = false;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match local_name(e.name().as_ref()) {
                b"workbookPr" => {
                    date1904 = attr(&e, b"date1904")
                        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                        .unwrap_or(false);
                }
                b"sheet" => {
                    let name = attr(&e, b"name").unwrap_or_else(|| "Sheet".to_string());
                    let state = attr(&e, b"state").unwrap_or_else(|| "visible".to_string());
                    sheets.push(SheetRef {
                        name,
                        state,
                        relationship: attr_local(&e, b"id"),
                    });
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("malformed xl/workbook.xml: {e}"),
            _ => {}
        }
        buffer.clear();
    }
    if sheets.is_empty() {
        anyhow::bail!("xl/workbook.xml names no sheets");
    }
    Ok(Book { sheets, date1904 })
}

/// Map relationship ids to worksheet part names.
fn read_relationships(bytes: &[u8]) -> Result<HashMap<String, String>> {
    // Only worksheet parts carry cells; a chartsheet's part has no `sheetData`
    // and is not a sheet here.
    Ok(read_relationships_in(bytes, "xl", Some("/worksheet"))?
        .into_iter()
        .map(|(id, (_, part))| (id, part))
        .collect())
}

/// Every relationship a part's relationship part declares, keyed by id.
///
/// `base` is the directory of the part the relationships belong to: a target
/// is written relative to it, and resolved here into the package-wide name the
/// part has. A relationship whose target cannot name a part — one that climbs
/// above the package root — is not one this can follow, so it is left out
/// rather than pointed at somewhere imaginary.
fn read_relationships_in(
    bytes: &[u8],
    base: &str,
    only_kind: Option<&str>,
) -> Result<HashMap<String, (String, String)>> {
    let mut reader = Reader::from_reader(Cursor::new(bytes));
    let mut buffer = Vec::new();
    let mut out = HashMap::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == b"Relationship" {
                    let kind = attr(&e, b"Type").unwrap_or_default();
                    if only_kind.is_some_and(|wanted| !kind.ends_with(wanted)) {
                        buffer.clear();
                        continue;
                    }
                    if let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                        if let Some(part) = resolve_target(&target, base) {
                            out.insert(id, (kind, part));
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("malformed relationships: {e}"),
            _ => {}
        }
        buffer.clear();
    }
    Ok(out)
}

/// The part a relationship points at, resolved against `base` — the directory
/// of the part that names it.
///
/// A target is package-absolute when it starts with `/`, and otherwise
/// relative to `base`, where `../` steps out of a directory.
fn resolve_target(target: &str, base: &str) -> Option<String> {
    let joined = match target.strip_prefix('/') {
        Some(rest) => rest.to_string(),
        None => format!("{base}/{target}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for piece in joined.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    let part = parts.join("/");
    (!part.is_empty()).then_some(part)
}

/// The directory a part lives in: `xl/worksheets/sheet1.xml` is in
/// `xl/worksheets`.
fn part_dir(part: &str) -> &str {
    match part.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    }
}

/// The relationship part belonging to `part`: `xl/workbook.xml` declares its
/// relationships in `xl/_rels/workbook.xml.rels`.
fn rels_part(part: &str) -> String {
    let file = part.rsplit_once('/').map_or(part, |(_, file)| file);
    match part_dir(part) {
        "" => format!("_rels/{file}.rels"),
        dir => format!("{dir}/_rels/{file}.rels"),
    }
}

// --- the pictures a sheet draws -------------------------------------------

/// What one worksheet's drawing part holds.
struct SheetDrawing {
    pictures: Vec<SheetPicture>,
    /// The charts the sheet draws, in the order the drawing anchors them.
    charts: Vec<pdfextractor_chart::ChartShape>,
}

/// The pictures and charts one worksheet draws.
///
/// The worksheet part says nothing about them itself: it names a drawing part
/// (`<drawing r:id="…"/>`), that drawing's relationship part names the parts
/// its anchors point at, and each anchor says where the thing it holds sits. All
/// three are followed, and a link any of them is missing leaves that anchor out
/// rather than inventing one — a sheet with no `<drawing>`, an anchor whose part
/// is not in the package, a media part whose name is no picture this project
/// carries.
///
/// A picture and a chart are the same kind of anchor and two different things.
/// A picture is a media part and its bytes; a chart is a chart part — a drawing
/// of data — whose categories, values and title are what a reader can use. The
/// picture reaches the document with an image behind it, the chart as a picture
/// with none: what it said is the data, not a picture of it.
fn read_sheet_drawing(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    sheet_part: &str,
    sheet: &[u8],
    resolve: &dyn Fn(&str) -> Vec<String>,
) -> Result<SheetDrawing> {
    let mut drawing = SheetDrawing {
        pictures: Vec::new(),
        charts: Vec::new(),
    };
    let Some(drawing_part) =
        worksheet_drawing(sheet).and_then(|id| relationship_part(archive, sheet_part, &id))
    else {
        return Ok(drawing);
    };
    let Some(bytes) = read_part(archive, &drawing_part) else {
        return Ok(drawing);
    };
    let media = match read_part(archive, &rels_part(&drawing_part)) {
        Some(bytes) => read_relationships_in(&bytes, part_dir(&drawing_part), None)?,
        None => HashMap::new(),
    };

    for anchor in read_drawing(&bytes)? {
        let Some((_, part)) = media.get(anchor.target.id()) else {
            continue;
        };
        match anchor.target {
            DrawingTarget::Picture(_) => {
                // The part name's extension is the only statement the package
                // makes about what a media part is. A name this project has no
                // reading for — an OLE object — is not a picture.
                let Some(mime) = part.rsplit_once('.').and_then(|(_, extension)| {
                    pdfextractor_picture_formats::mime_for_extension(extension)
                }) else {
                    continue;
                };
                let Some(data) = read_part(archive, part) else {
                    continue;
                };
                drawing.pictures.push(SheetPicture {
                    data,
                    mime,
                    bbox: anchor.bbox,
                });
            }
            DrawingTarget::Chart(_) => {
                // A chart part nothing can be read from is nothing on the
                // sheet, not an empty frame: a chart that plots no data has no
                // picture to report.
                let Some(bytes) = read_part(archive, part) else {
                    continue;
                };
                let Some(mut chart) = pdfextractor_chart::read_chart_with(&bytes, resolve) else {
                    continue;
                };
                chart.bbox = anchor.bbox;
                drawing.charts.push(chart);
            }
        }
    }
    Ok(drawing)
}

/// The largest number of cells one chart reference is followed for: a chart can
/// name a whole column (`$A:$A`), and a million rows of mostly empty cells is
/// not a reading of a chart, it is a transcription of the range.
const MAX_CHART_CELLS: usize = 100_000;

/// The cells a chart's reference names, read from the workbook's own sheets.
///
/// A chart part may carry its numbers in caches (`c:numCache`) — Excel writes
/// them — or only the reference, which is what a scripted writer leaves behind:
/// the numbers are in the cells and nowhere else. This is the second case, and
/// it is resolved the way a spreadsheet resolves one: `'Sheet Name'!$B$2:$B$4` is
/// a sheet name (quoted when it holds a space) and a range between two cells,
/// each column letter and row number absolute or not. The cells come back the
/// way a spreadsheet fills them — down a column, across a row — with an empty
/// string for a cell that holds nothing.
///
/// A reference this cannot make sense of — a defined name, another workbook's
/// cells, a range past the end of the sheet — answers nothing, so the chart keeps
/// the data it did carry and nothing is invented for it.
fn resolve_chart_reference(
    reference: &str,
    default_sheet: &str,
    sheets: &[(&str, &RawSheet)],
) -> Vec<String> {
    // A reference may name the workbook first (`[1]Sheet!A1`); a chart part in
    // its own package names its own cells, so the bracket is dropped.
    let reference = match reference.rfind(']') {
        Some(end) => &reference[end + 1..],
        None => reference,
    };
    let (sheet, cells) = match reference.split_once('!') {
        Some((sheet, cells)) => (unquote_sheet(sheet), cells),
        None => (default_sheet, reference),
    };
    let Some((_, raw)) = sheets
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(sheet))
    else {
        return Vec::new();
    };
    let (start, end) = match cells.split_once(':') {
        Some((start, end)) => (start, end),
        None => (cells, cells),
    };
    let (Some((start_row, start_col)), Some((end_row, end_col))) =
        (cell_address(start), cell_address(end))
    else {
        return Vec::new();
    };
    let (min_row, max_row) = (start_row.min(end_row), start_row.max(end_row));
    let (min_col, max_col) = (start_col.min(end_col), start_col.max(end_col));

    let mut values = Vec::new();
    for row in min_row..=max_row {
        for col in min_col..=max_col {
            if values.len() == MAX_CHART_CELLS {
                return values;
            }
            values.push(raw.cells.get(&(row, col)).cloned().unwrap_or_default());
        }
    }
    values
}

/// A sheet name as a reference quotes it: `'Q1 report'` is `Q1 report`.
fn unquote_sheet(sheet: &str) -> &str {
    let sheet = sheet.trim();
    sheet
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or(sheet)
}

/// A cell as a reference writes it — `$B$2`, `B2` — as 0-based `(row, column)`.
fn cell_address(text: &str) -> Option<(usize, usize)> {
    let text: String = text.chars().filter(|c| *c != '$').collect();
    let split = text.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = text.split_at(split);
    if letters.is_empty() || digits.is_empty() {
        return None;
    }
    let mut col = 0usize;
    for letter in letters.chars() {
        if !letter.is_ascii_alphabetic() {
            return None;
        }
        let digit = letter.to_ascii_uppercase() as usize - 'A' as usize + 1;
        col = col.checked_mul(26)?.checked_add(digit)?;
    }
    let row: usize = digits.parse().ok()?;
    // Spreadsheet rows and columns are counted from one.
    Some((row.checked_sub(1)?, col.checked_sub(1)?))
}

/// The relationship id of the drawing part a worksheet names, if it names one.
fn worksheet_drawing(sheet: &[u8]) -> Option<String> {
    let mut reader = Reader::from_reader(Cursor::new(sheet));
    let mut buffer = Vec::new();
    let mut id = None;
    loop {
        match reader.read_event_into(&mut buffer) {
            // A worksheet names its drawing after its cells, but the element is
            // taken wherever it is written rather than in one place.
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == b"drawing" {
                    id = attr_local(&e, b"id");
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buffer.clear();
    }
    id
}

/// The part a relationship id names, read out of `part`'s own relationship
/// part — the reference a sheet's `<drawing>` is, and the ones a drawing's
/// anchors are.
fn relationship_part(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    part: &str,
    id: &str,
) -> Option<String> {
    let bytes = read_part(archive, &rels_part(part))?;
    let relationships = read_relationships_in(&bytes, part_dir(part), None).ok()?;
    relationships.get(id).map(|(_, target)| target.to_string())
}

/// The rules the cell grid is measured in, in the units an anchor is written
/// in: a pixel is 9525 English Metric Units, Excel's default column is 64
/// pixels wide and its default row 20 pixels tall.
///
/// A sheet that declares column widths of its own is not consulted: the anchor
/// names cells, and the grid those cells are on is the default one — the same
/// grid `xlsxwriter` and Excel compute an anchor's offsets against.
const EMU_PER_COLUMN: f64 = 609_600.0;
const EMU_PER_ROW: f64 = 190_500.0;

/// What one anchor of a drawing part points at.
enum DrawingTarget {
    /// A picture: the relationship id of the media part it draws.
    Picture(String),
    /// A chart: the relationship id of the chart part it frames, whose data is
    /// what reaches the document.
    Chart(String),
}

impl DrawingTarget {
    /// The relationship id, whatever kind of part it names.
    fn id(&self) -> &str {
        match self {
            DrawingTarget::Picture(id) | DrawingTarget::Chart(id) => id,
        }
    }
}

/// One thing a drawing part anchors, and where it sits.
struct DrawingAnchor {
    target: DrawingTarget,
    /// `[left, top, right, bottom]`, in cells.
    bbox: [f64; 4],
}

/// A corner of an anchor, in cells and in the offset inside the cell.
#[derive(Debug, Default, Clone, Copy)]
struct Marker {
    col: f64,
    row: f64,
    col_off: f64,
    row_off: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchorKind {
    /// Two corners named: the picture spans from the first cell to the second.
    TwoCell,
    /// One corner named and a size: what a writer that stores a picture's
    /// rectangle rather than its cells emits.
    OneCell,
    /// A position and a size in EMU, with no cell at all.
    Absolute,
}

/// A number being read, and the corner it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    None,
    From,
    To,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    None,
    Column,
    Row,
    ColumnOffset,
    RowOffset,
}

/// The anchor the drawing parse is inside of.
struct AnchorReader {
    kind: AnchorKind,
    from: Marker,
    to: Marker,
    /// The size a one-cell or absolute anchor states, in EMU.
    extent: (f64, f64),
    /// Where an absolute anchor puts its corner, in EMU.
    position: (f64, f64),
    /// The relationship id of the media part a picture anchor draws.
    embed: Option<String>,
    /// The relationship id of the chart part a frame anchors.
    chart: Option<String>,
    place: Place,
    field: Field,
    /// A picture's own `a:xfrm` repeats its size inside the anchor. It is not
    /// the anchor's `ext`, and its `a:ext` is read as nothing.
    in_picture: bool,
}

impl AnchorReader {
    fn new(kind: AnchorKind) -> Self {
        Self {
            kind,
            from: Marker::default(),
            to: Marker::default(),
            extent: (0.0, 0.0),
            position: (0.0, 0.0),
            embed: None,
            chart: None,
            place: Place::None,
            field: Field::None,
            in_picture: false,
        }
    }

    /// Take a number the parser read as text — a marker's cell or offset.
    fn take(&mut self, value: f64) {
        let marker = match self.place {
            Place::None => return,
            Place::From => &mut self.from,
            Place::To => &mut self.to,
        };
        match self.field {
            Field::Column => marker.col = value,
            Field::Row => marker.row = value,
            Field::ColumnOffset => marker.col_off = value,
            Field::RowOffset => marker.row_off = value,
            Field::None => {}
        }
    }

    /// The anchor as a rectangle in cells, when it holds something this reads:
    /// a picture's media part, or the chart part a frame draws.
    fn finish(self) -> Option<DrawingAnchor> {
        // A chart anchor names its part the same way a picture anchor does, in
        // a `c:chart`'s `r:id` rather than an `a:blip`'s `r:embed`.
        let target = match (self.chart, self.embed) {
            (Some(id), _) => DrawingTarget::Chart(id),
            (None, Some(id)) => DrawingTarget::Picture(id),
            (None, None) => return None,
        };
        let (left, top, right, bottom) = match self.kind {
            // Both corners: the offsets inside them say how much of the cell
            // the picture covers, so a picture smaller than one cell is the
            // fraction it is rather than the whole cell.
            AnchorKind::TwoCell => (
                self.from.col + self.from.col_off / EMU_PER_COLUMN,
                self.from.row + self.from.row_off / EMU_PER_ROW,
                self.to.col + self.to.col_off / EMU_PER_COLUMN,
                self.to.row + self.to.row_off / EMU_PER_ROW,
            ),
            // One corner and a size: the size, converted to the cells it spans.
            AnchorKind::OneCell => {
                let left = self.from.col + self.from.col_off / EMU_PER_COLUMN;
                let top = self.from.row + self.from.row_off / EMU_PER_ROW;
                (
                    left,
                    top,
                    left + self.extent.0 / EMU_PER_COLUMN,
                    top + self.extent.1 / EMU_PER_ROW,
                )
            }
            // No cell at all: the position is the corner, in the same units.
            AnchorKind::Absolute => {
                let left = self.position.0 / EMU_PER_COLUMN;
                let top = self.position.1 / EMU_PER_ROW;
                (
                    left,
                    top,
                    left + self.extent.0 / EMU_PER_COLUMN,
                    top + self.extent.1 / EMU_PER_ROW,
                )
            }
        };
        Some(DrawingAnchor {
            target,
            // An anchor can name no width or no height — a picture shrunk to
            // nothing — and a box that runs backwards is no box at all.
            bbox: [left, top, right.max(left), bottom.max(top)],
        })
    }
}

/// The anchors of a drawing part that name a part of the package.
///
/// A drawing part is a list of anchors, each holding one thing: a `pic` (a
/// raster or a metafile, found through its `a:blip`'s `r:embed`), a
/// `graphicFrame` (a chart, or a table — a frame whose relationship names a part
/// that is not an image), or a shape. The two the package holds a part for are
/// answered — a picture's media part and a chart's chart part — and a shape,
/// which is markup rather than a part, is not: it has nothing here to place.
fn read_drawing(bytes: &[u8]) -> Result<Vec<DrawingAnchor>> {
    let mut reader = Reader::from_reader(Cursor::new(bytes));
    let mut buffer = Vec::new();
    let mut anchors = Vec::new();
    let mut current: Option<AnchorReader> = None;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                if let Some(anchor) = current.as_mut() {
                    match name.as_slice() {
                        b"from" => anchor.place = Place::From,
                        b"to" => anchor.place = Place::To,
                        b"col" => anchor.field = Field::Column,
                        b"row" => anchor.field = Field::Row,
                        b"colOff" => anchor.field = Field::ColumnOffset,
                        b"rowOff" => anchor.field = Field::RowOffset,
                        // A size and a position are attributes of their own
                        // element, not text runs inside it.
                        b"ext" if !anchor.in_picture => {
                            anchor.extent = emu_extent(&e);
                        }
                        b"pos" => anchor.position = emu_origin(&e),
                        b"pic" => anchor.in_picture = true,
                        b"blip" => anchor.embed = attr_local(&e, b"embed"),
                        // A frame's payload is `a:graphic/a:graphicData`,
                        // which for a chart holds a `c:chart` naming the part.
                        b"chart" => anchor.chart = attr_local(&e, b"id"),
                        _ => {}
                    }
                } else {
                    match name.as_slice() {
                        b"twoCellAnchor" => current = Some(AnchorReader::new(AnchorKind::TwoCell)),
                        b"oneCellAnchor" => current = Some(AnchorReader::new(AnchorKind::OneCell)),
                        b"absoluteAnchor" => {
                            current = Some(AnchorReader::new(AnchorKind::Absolute))
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(anchor) = current.as_mut() {
                    if let Ok(value) = e.unescape().unwrap_or_default().trim().parse::<f64>() {
                        anchor.take(value);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"from" | b"to" => {
                        if let Some(anchor) = current.as_mut() {
                            anchor.place = Place::None;
                        }
                    }
                    b"pic" => {
                        if let Some(anchor) = current.as_mut() {
                            anchor.in_picture = false;
                        }
                    }
                    b"twoCellAnchor" | b"oneCellAnchor" | b"absoluteAnchor" => {
                        if let Some(anchor) = current.take() {
                            if let Some(anchor) = anchor.finish() {
                                anchors.push(anchor);
                            }
                        }
                    }
                    _ => {}
                }
                if let Some(anchor) = current.as_mut() {
                    anchor.field = Field::None;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("malformed drawing part: {e}"),
            _ => {}
        }
        buffer.clear();
    }
    Ok(anchors)
}

/// The `cx`/`cy` an `<ext>` element states, in EMU.
fn emu_extent(element: &BytesStart) -> (f64, f64) {
    let read = |key: &[u8]| {
        attr(element, key)
            .and_then(|value| value.trim().parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    (read(b"cx"), read(b"cy"))
}

/// The `x`/`y` a `<pos>` element states, in EMU.
fn emu_origin(element: &BytesStart) -> (f64, f64) {
    let read = |key: &[u8]| {
        attr(element, key)
            .and_then(|value| value.trim().parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    (read(b"x"), read(b"y"))
}

/// The strings cells refer to by index (`t="s"`).
fn read_shared_strings(bytes: &[u8]) -> Result<Vec<String>> {
    let mut reader = Reader::from_reader(Cursor::new(bytes));
    let mut buffer = Vec::new();
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut capture = false;
    let mut phonetic = 0usize;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                match local_name(e.name().as_ref()) {
                    b"si" => current = Some(String::new()),
                    // A phonetic guide is the reading of the text above it,
                    // not the text: Docling's reader does not keep it twice.
                    b"rPh" => phonetic += 1,
                    b"t" => capture = phonetic == 0,
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if capture {
                    if let Some(current) = current.as_mut() {
                        current.push_str(&e.unescape().unwrap_or_default());
                    }
                }
            }
            Ok(Event::End(e)) => match local_name(e.name().as_ref()) {
                b"si" => out.push(current.take().unwrap_or_default()),
                b"rPh" => phonetic = phonetic.saturating_sub(1),
                b"t" => capture = false,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("malformed xl/sharedStrings.xml: {e}"),
            _ => {}
        }
        buffer.clear();
    }
    Ok(out)
}

/// The number formats a cell can wear, and which of them mean "this number is
/// a date".
#[derive(Debug, Default)]
struct Styles {
    /// The number format of each `cellXfs` entry, by the index a cell's `s`
    /// attribute names.
    formats: Vec<usize>,
    /// Custom formats (`numFmtId` ≥ 164) by id; the built-in ids are known.
    custom: HashMap<usize, String>,
}

impl Styles {
    /// Whether the style at `index` renders its number as a date.
    fn is_date(&self, index: usize) -> bool {
        let Some(&format) = self.formats.get(index) else {
            return false;
        };
        match format {
            // The built-in date and time formats: 14–22 the western ones,
            // 27–36 and 45–47 the East Asian date and era formats, 50–58 the
            // East Asian ones Excel adds to those.
            14..=22 | 27..=36 | 45..=47 | 50..=58 => true,
            _ => self
                .custom
                .get(&format)
                .is_some_and(|code| is_date_format(code)),
        }
    }
}

fn read_styles(bytes: &[u8]) -> Result<Styles> {
    let mut reader = Reader::from_reader(Cursor::new(bytes));
    let mut buffer = Vec::new();
    let mut styles = Styles::default();
    let mut in_cell_xfs = false;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match local_name(e.name().as_ref()) {
                b"numFmt" => {
                    if let (Some(id), Some(code)) = (attr(&e, b"numFmtId"), attr(&e, b"formatCode"))
                    {
                        if let Ok(id) = id.parse() {
                            styles.custom.insert(id, code);
                        }
                    }
                }
                b"cellXfs" => in_cell_xfs = true,
                // `cellStyleXfs` holds the same element for named styles; a
                // cell's `s` indexes `cellXfs`, so only that list is kept.
                b"xf" if in_cell_xfs => {
                    let format = attr(&e, b"numFmtId")
                        .and_then(|id| id.parse().ok())
                        .unwrap_or(0);
                    styles.formats.push(format);
                }
                _ => {}
            },
            Ok(Event::End(e)) => {
                if local_name(e.name().as_ref()) == b"cellXfs" {
                    in_cell_xfs = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("malformed xl/styles.xml: {e}"),
            _ => {}
        }
        buffer.clear();
    }
    Ok(styles)
}

/// Whether a number format says its number is a date.
///
/// Only the format's *tokens* count: text in quotes, characters escaped with a
/// backslash, bracketed sections (`[Red]`, `[$-409]`, `[h]`) and the padding
/// placeholders `_`/`*` say nothing about the value's type, so `0.00 "days"`
/// is a number and `yyyy-mm-dd` is a date.
fn is_date_format(code: &str) -> bool {
    let mut chars = code.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for quote in chars.by_ref() {
                    if quote == '"' {
                        break;
                    }
                }
            }
            '\\' => {
                chars.next();
            }
            '[' => {
                for bracket in chars.by_ref() {
                    if bracket == ']' {
                        break;
                    }
                }
            }
            '_' | '*' => {
                chars.next();
            }
            'y' | 'Y' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' => return true,
            _ => {}
        }
    }
    false
}

// --- one worksheet --------------------------------------------------------

/// A merged range, 0-based and inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Merge {
    min_row: usize,
    min_col: usize,
    max_row: usize,
    max_col: usize,
}

impl Merge {
    fn contains(&self, row: usize, col: usize) -> bool {
        (self.min_row..=self.max_row).contains(&row) && (self.min_col..=self.max_col).contains(&col)
    }
}

/// What a worksheet part held before it was cut into tables.
#[derive(Debug, Default)]
struct RawSheet {
    /// Decoded cell text by 0-based `(row, column)`. Only cells with a value
    /// are here: a formula with no cached result is empty to a reader, and an
    /// empty cell is not content.
    cells: HashMap<(usize, usize), String>,
    merges: Vec<Merge>,
}

impl RawSheet {
    /// Cut the sheet into its connected regions, each a table.
    ///
    /// The sheet is borrowed rather than consumed: a chart's reference is read
    /// from the same cells the tables are, so the cells outlive both.
    ///
    /// Content is a cell with a value *or* a cell inside a merged range: a
    /// merge over empty cells is still something the sheet drew. Connectivity
    /// is four-directional and has no tolerance for gaps — the reading Docling
    /// uses by default — so one empty row between two blocks leaves them two
    /// tables.
    fn tables(&self) -> Vec<SheetTable> {
        let mut content: HashSet<(usize, usize)> = self.cells.keys().copied().collect();
        for merge in &self.merges {
            for row in merge.min_row..=merge.max_row {
                for col in merge.min_col..=merge.max_col {
                    content.insert((row, col));
                }
            }
        }

        let mut starts: Vec<(usize, usize)> = content.iter().copied().collect();
        starts.sort_unstable();
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        let mut tables = Vec::new();
        for start in starts {
            if seen.contains(&start) {
                continue;
            }
            let (min_row, min_col, max_row, max_col) = self.region(start, &content, &mut seen);
            tables.push(self.table(min_row, min_col, max_row, max_col));
        }
        tables
    }

    /// The bounding box of the content connected to `start`, marking every cell
    /// of it visited.
    fn region(
        &self,
        start: (usize, usize),
        content: &HashSet<(usize, usize)>,
        seen: &mut HashSet<(usize, usize)>,
    ) -> (usize, usize, usize, usize) {
        let (mut min_row, mut min_col) = start;
        let (mut max_row, mut max_col) = start;
        let mut queue = VecDeque::from([start]);
        seen.insert(start);
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

    /// Lay the region out on its grid: every slot of the bounding box is a
    /// cell except the slots a merge covers, which its spanning cell holds.
    fn table(&self, min_row: usize, min_col: usize, max_row: usize, max_col: usize) -> SheetTable {
        let mut anchors: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
        for merge in &self.merges {
            anchors.entry((merge.min_row, merge.min_col)).or_insert((
                merge.max_row - merge.min_row + 1,
                merge.max_col - merge.min_col + 1,
            ));
        }
        let mut cells = Vec::new();
        for row in min_row..=max_row {
            for col in min_col..=max_col {
                if self.merges.iter().any(|merge| {
                    merge.contains(row, col) && (row, col) != (merge.min_row, merge.min_col)
                }) {
                    continue;
                }
                let (row_span, col_span) = anchors.get(&(row, col)).copied().unwrap_or((1, 1));
                cells.push(SheetCell {
                    row: row - min_row,
                    col: col - min_col,
                    text: self.cells.get(&(row, col)).cloned().unwrap_or_default(),
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
}

/// What a `<c>` element's `t` attribute says its value is.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum CellKind {
    #[default]
    Number,
    SharedString,
    FormulaString,
    Boolean,
    Error,
    IsoDate,
    Inline,
}

/// A `<c>` while it is being read.
#[derive(Debug, Default)]
struct CellRead {
    row: usize,
    col: usize,
    kind: CellKind,
    /// The index into `cellXfs` that gives the cell its number format.
    style: usize,
    /// The `<v>` element's text: the value itself, or an index to a shared
    /// string, or a boolean's `0`/`1`.
    value: Option<String>,
    /// The inline string (`t="inlineStr"`), present when the package wrote one.
    inline: Option<String>,
}

/// The worksheet reader, and the parts' facts a cell's text is decoded from.
struct SheetReader<'a> {
    strings: &'a [String],
    styles: &'a Styles,
    date1904: bool,
    sheet: RawSheet,
    /// The row being read, 0-based; `None` before the first `<row>`.
    row: Option<usize>,
    /// The row a `<row>` without an `r` takes.
    next_row: usize,
    /// The column a `<c>` without an `r` takes.
    next_col: usize,
    cell: Option<CellRead>,
    /// The text of the `<v>`/`<t>` element currently open.
    capture: Option<String>,
    /// Inside a phonetic guide (`rPh`), whose text is a reading, not the value.
    phonetic: bool,
}

impl<'a> SheetReader<'a> {
    fn new(strings: &'a [String], styles: &'a Styles, date1904: bool) -> Self {
        Self {
            strings,
            styles,
            date1904,
            sheet: RawSheet::default(),
            row: None,
            next_row: 0,
            next_col: 0,
            cell: None,
            capture: None,
            phonetic: false,
        }
    }

    fn read(mut self, bytes: &[u8]) -> Result<RawSheet> {
        let mut reader = Reader::from_reader(Cursor::new(bytes));
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(e)) => {
                    let name = local_name(e.name().as_ref()).to_vec();
                    self.begin(&name, &e);
                }
                // An element written as `<x/>` is an open and a close at once;
                // `<v/>` and `<c/>` both occur in real packages.
                Ok(Event::Empty(e)) => {
                    let name = local_name(e.name().as_ref()).to_vec();
                    self.begin(&name, &e);
                    self.end(&name);
                }
                Ok(Event::Text(e)) => {
                    if self.capture.is_some() && self.cell.is_some() {
                        let text = e.unescape().unwrap_or_default();
                        self.capture.get_or_insert_with(String::new).push_str(&text);
                    }
                }
                Ok(Event::End(e)) => {
                    let name = local_name(e.name().as_ref()).to_vec();
                    self.end(&name);
                }
                Ok(Event::Eof) => break,
                Err(e) => anyhow::bail!("malformed worksheet: {e}"),
                _ => {}
            }
            buffer.clear();
        }
        Ok(self.sheet)
    }

    fn begin(&mut self, name: &[u8], element: &BytesStart) {
        match name {
            b"row" => {
                let row = attr(element, b"r")
                    .and_then(|value| value.parse::<usize>().ok())
                    .map(|row| row.saturating_sub(1))
                    .unwrap_or(self.next_row);
                self.row = Some(row);
                self.next_row = row + 1;
                self.next_col = 0;
            }
            b"c" => {
                let (row, col) = attr(element, b"r")
                    .and_then(|reference| parse_reference(&reference))
                    .unwrap_or((self.row.unwrap_or(self.next_row), self.next_col));
                let kind = match attr(element, b"t").as_deref() {
                    Some("s") => CellKind::SharedString,
                    Some("inlineStr") => CellKind::Inline,
                    Some("str") => CellKind::FormulaString,
                    Some("b") => CellKind::Boolean,
                    Some("e") => CellKind::Error,
                    Some("d") => CellKind::IsoDate,
                    _ => CellKind::Number,
                };
                let style = attr(element, b"s")
                    .and_then(|index| index.parse().ok())
                    .unwrap_or(0);
                self.cell = Some(CellRead {
                    row,
                    col,
                    kind,
                    style,
                    value: None,
                    inline: None,
                });
                self.next_col = col + 1;
            }
            b"v" => self.capture = Some(String::new()),
            b"is" => {
                if let Some(cell) = self.cell.as_mut() {
                    // Present even when empty: a cell written as an empty
                    // string is a value openpyxl reports as `""`, not as
                    // missing, so it counts as content.
                    cell.inline.get_or_insert_with(String::new);
                }
            }
            b"t" => {
                if self.cell.is_some() && !self.phonetic {
                    self.capture = Some(String::new());
                }
            }
            b"rPh" => self.phonetic = true,
            b"mergeCell" => {
                if let Some(reference) = attr(element, b"ref") {
                    if let Some(merge) = parse_range(&reference) {
                        self.sheet.merges.push(merge);
                    }
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &[u8]) {
        match name {
            b"v" => {
                if let (Some(cell), Some(value)) = (self.cell.as_mut(), self.capture.take()) {
                    cell.value = Some(value);
                }
            }
            b"t" => {
                if let (Some(cell), Some(text)) = (self.cell.as_mut(), self.capture.take()) {
                    cell.inline.get_or_insert_with(String::new).push_str(&text);
                }
            }
            b"rPh" => self.phonetic = false,
            b"c" => {
                if let Some(cell) = self.cell.take() {
                    self.next_col = cell.col + 1;
                    if let Some(text) = self.decode(&cell) {
                        self.sheet.cells.insert((cell.row, cell.col), text);
                    }
                }
            }
            b"row" => self.row = None,
            _ => {}
        }
    }

    /// The text a reader sees in the cell, or `None` when it holds nothing.
    fn decode(&self, cell: &CellRead) -> Option<String> {
        match cell.kind {
            CellKind::Inline => cell.inline.clone(),
            CellKind::SharedString => {
                let raw = cell.value.as_deref()?;
                let index: usize = raw.trim().parse().ok()?;
                self.strings.get(index).cloned()
            }
            CellKind::Boolean => {
                let raw = cell.value.as_deref()?.trim();
                Some(if raw == "1" || raw.eq_ignore_ascii_case("true") {
                    "True".to_string()
                } else {
                    "False".to_string()
                })
            }
            CellKind::Error | CellKind::FormulaString => cell.value.clone(),
            CellKind::IsoDate => cell.value.as_deref().map(iso_date_to_text),
            CellKind::Number => {
                let raw = cell.value.as_deref()?;
                if raw.trim().is_empty() {
                    return None;
                }
                if self.styles.is_date(cell.style) {
                    if let Ok(serial) = raw.trim().parse::<f64>() {
                        if let Some(text) = excel_datetime(serial, self.date1904) {
                            return Some(text);
                        }
                    }
                }
                Some(raw.to_string())
            }
        }
    }
}

// --- values ---------------------------------------------------------------

/// An Excel date serial as the text a reader sees — Python's `str(datetime)`,
/// which is what Docling writes: `2024-03-05 00:00:00`.
///
/// The 1900 system counts from 1899-12-30, except that it also counts the
/// 29th of February 1900, a day that never existed: serials below 60 are one
/// day later than their arithmetic would suggest, and serial 60 is that
/// phantom day, which every reader shows as February 28th.
fn excel_datetime(serial: f64, date1904: bool) -> Option<String> {
    if !serial.is_finite() || serial < 0.0 {
        return None;
    }
    let mut day = serial.floor();
    let fraction = serial - day;
    // Excel stores the time as a fraction of a day; a whole millisecond is the
    // precision every reader agrees on.
    let mut millis = (fraction * 86_400_000.0).round() as i64;
    let carried = millis >= 86_400_000;
    if carried {
        millis -= 86_400_000;
        day += 1.0;
    }
    // A serial below one with no time-of-day carry is a time, not a date:
    // Python's `str(time)` is `12:00:00`, and Docling keeps that reading.
    if serial < 1.0 && !carried {
        return Some(clock(millis));
    }
    let mut days = day as i64;
    if !date1904 && serial > 0.0 && serial < 60.0 {
        days += 1;
    }
    let base = if date1904 {
        NaiveDate::from_ymd_opt(1904, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(1899, 12, 30)?
    };
    let date = base.checked_add_signed(TimeDelta::days(days))?;
    Some(format!("{} {}", date.format("%Y-%m-%d"), clock(millis)))
}

/// A time of day as Python's `str` writes it: `HH:MM:SS`, plus microseconds
/// when the time has any.
fn clock(millis: i64) -> String {
    let seconds = millis / 1000;
    let rest = millis % 1000;
    let text = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    );
    if rest == 0 {
        text
    } else {
        format!("{text}.{:06}", rest * 1000)
    }
}

/// An ISO 8601 cell (`t="d"`) as the same `str(datetime)` text.
fn iso_date_to_text(raw: &str) -> String {
    let raw = raw.trim();
    if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f") {
        let micros = parsed.nanosecond() / 1000;
        let text = parsed.format("%Y-%m-%d %H:%M:%S").to_string();
        return if micros == 0 {
            text
        } else {
            format!("{text}.{micros:06}")
        };
    }
    raw.replace('T', " ")
        .trim_end_matches('Z')
        .trim_end_matches("+00:00")
        .to_string()
}

// --- references -----------------------------------------------------------

/// Parse an A1 reference: `B5` is row 4, column 1 (both 0-based).
fn parse_reference(text: &str) -> Option<(usize, usize)> {
    let mut col = 0usize;
    let mut row = 0usize;
    let mut letters = 0usize;
    let mut digits = false;
    for c in text.chars() {
        match c {
            'A'..='Z' | 'a'..='z' if !digits => {
                col = col * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
                letters += 1;
            }
            '0'..='9' => {
                digits = true;
                row = row * 10 + (c as usize - '0' as usize);
            }
            // A `$` (an absolute reference) names the same cell.
            '$' => {}
            _ => return None,
        }
    }
    if letters == 0 || row == 0 {
        return None;
    }
    Some((row - 1, col - 1))
}

/// Parse a range reference: `B5:C5` is one merged rectangle.
fn parse_range(text: &str) -> Option<Merge> {
    let (start, end) = text.split_once(':').unwrap_or((text, text));
    let (start_row, start_col) = parse_reference(start)?;
    let (end_row, end_col) = parse_reference(end)?;
    Some(Merge {
        min_row: start_row.min(end_row),
        min_col: start_col.min(end_col),
        max_row: start_row.max(end_row),
        max_col: start_col.max(end_col),
    })
}

// --- XML helpers ----------------------------------------------------------

/// Strip a namespace prefix: `x:row` -> `row`.
fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|b| *b == b':') {
        Some(index) => &name[index + 1..],
        None => name,
    }
}

fn attr(element: &BytesStart, key: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == key)
        .and_then(|attribute| {
            attribute
                .unescape_value()
                .ok()
                .map(|value| value.into_owned())
        })
}

fn attr_local(element: &BytesStart, key: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| local_name(attribute.key.as_ref()) == key)
        .and_then(|attribute| {
            attribute
                .unescape_value()
                .ok()
                .map(|value| value.into_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const NS: &str = r#"xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main""#;
    const R: &str =
        r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    const RELS: &str = r#"xmlns="http://schemas.openxmlformats.org/package/2006/relationships""#;

    /// Build a package from parts, in memory.
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

    fn workbook(sheets: &str) -> String {
        format!(r#"<workbook {NS} {R}><sheets>{sheets}</sheets></workbook>"#)
    }

    fn relationships(entries: &str) -> String {
        format!(r#"<Relationships {RELS}>{entries}</Relationships>"#)
    }

    fn worksheet(body: &str) -> String {
        format!(r#"<worksheet {NS}><sheetData>{body}</sheetData></worksheet>"#)
    }

    /// A one-sheet package whose worksheet part holds `body`.
    fn parse_sheet(body: &str) -> XlsxDocument {
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(r#"<sheet name="Sheet1" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            ("xl/worksheets/sheet1.xml", worksheet(body)),
        ]);
        XlsxDocument::parse_bytes(&pkg).expect("parse")
    }

    fn texts(table: &SheetTable) -> Vec<(usize, usize, String)> {
        table
            .cells
            .iter()
            .map(|cell| (cell.row, cell.col, cell.text.clone()))
            .collect()
    }

    #[test]
    fn reads_numbers_booleans_and_shared_strings() {
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(r#"<sheet name="Data" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            (
                "xl/sharedStrings.xml",
                format!(r#"<sst {NS}><si><t>Item</t></si><si><t>Widget</t></si></sst>"#),
            ),
            (
                "xl/worksheets/sheet1.xml",
                worksheet(
                    r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
                       <row r="2"><c r="A2"><v>12</v></c><c r="B2" t="b"><v>1</v></c></row>
                       <row r="3"><c r="A3"><v>1.5</v></c><c r="B3" t="b"><v>0</v></c></row>"#,
                ),
            ),
        ]);
        let document = XlsxDocument::parse_bytes(&pkg).expect("parse");
        assert_eq!(document.sheets.len(), 1);
        assert_eq!(document.sheets[0].name, "Data");
        assert!(!document.sheets[0].hidden);
        let table = &document.sheets[0].tables[0];
        assert_eq!(table.num_rows, 3);
        assert_eq!(table.num_cols, 2);
        assert_eq!(
            texts(table),
            vec![
                (0, 0, "Item".to_string()),
                (0, 1, "Widget".to_string()),
                (1, 0, "12".to_string()),
                (1, 1, "True".to_string()),
                (2, 0, "1.5".to_string()),
                (2, 1, "False".to_string()),
            ]
        );
    }

    #[test]
    fn a_merge_is_one_cell_that_covers_its_slots() {
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(r#"<sheet name="Sheet1" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!(
                    r#"<worksheet {NS}><sheetData>
                         <row r="1"><c r="A1" t="inlineStr"><is><t>Shipping</t></is></c><c r="C1"><v>1</v></c></row>
                       </sheetData><mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells></worksheet>"#
                ),
            ),
        ]);
        let document = XlsxDocument::parse_bytes(&pkg).expect("parse");
        let table = &document.sheets[0].tables[0];
        assert_eq!((table.num_rows, table.num_cols), (1, 3));
        assert_eq!(table.cells.len(), 2, "{:?}", table.cells);
        assert_eq!(table.cells[0].text, "Shipping");
        assert_eq!((table.cells[0].row_span, table.cells[0].col_span), (1, 2));
        // The covered slot is not a cell of the table; the cell after it is.
        assert_eq!(table.cells[1].col, 2);
    }

    #[test]
    fn a_vertical_merge_spans_rows() {
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(r#"<sheet name="Sheet1" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!(
                    r#"<worksheet {NS}><sheetData>
                         <row r="1"><c r="A1" t="inlineStr"><is><t>Books</t></is></c><c r="B1"><v>1</v></c></row>
                         <row r="2"><c r="A2" t="inlineStr"><is><t></t></is></c><c r="B2"><v>2</v></c></row>
                       </sheetData><mergeCells><mergeCell ref="A1:A2"/></mergeCells></worksheet>"#
                ),
            ),
        ]);
        let document = XlsxDocument::parse_bytes(&pkg).expect("parse");
        let table = &document.sheets[0].tables[0];
        let anchor = table
            .cells
            .iter()
            .find(|cell| cell.text == "Books")
            .expect("the merging cell");
        assert_eq!((anchor.row, anchor.col), (0, 0));
        assert_eq!((anchor.row_span, anchor.col_span), (2, 1));
    }

    #[test]
    fn an_empty_row_leaves_two_tables() {
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(r#"<sheet name="Sheet1" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                worksheet(
                    r#"<row r="1"><c r="A1"><v>1</v></c></row>
                       <row r="3"><c r="A3"><v>2</v></c></row>"#,
                ),
            ),
        ]);
        let document = XlsxDocument::parse_bytes(&pkg).expect("parse");
        let tables = &document.sheets[0].tables;
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].anchor, (0, 0));
        assert_eq!(tables[1].anchor, (0, 2));
        assert_eq!(tables[1].cells[0].text, "2");
    }

    #[test]
    fn a_formula_without_a_cached_value_is_empty() {
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(r#"<sheet name="Sheet1" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                worksheet(
                    r#"<row r="1"><c r="A1" t="inlineStr"><is><t>Total</t></is></c>
                       <c r="B1"><f>SUM(B2:B3)</f><v>15</v></c></row>
                       <row r="2"><c r="A2"><f>TODAY()</f></c></row>"#,
                ),
            ),
        ]);
        let document = XlsxDocument::parse_bytes(&pkg).expect("parse");
        let table = &document.sheets[0].tables[0];
        assert_eq!(
            texts(table),
            vec![
                (0, 0, "Total".to_string()),
                (0, 1, "15".to_string()),
                // The formula with no cached value is not content, so it is not
                // a cell; the region therefore stops at column B.
            ]
        );
        assert_eq!(table.num_cols, 2);
    }

    #[test]
    fn a_date_is_the_text_a_reader_sees() {
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(r#"<sheet name="Sheet1" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            (
                "xl/styles.xml",
                format!(
                    r#"<styleSheet {NS}><numFmts count="1"><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/></numFmts>
                       <cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="164"/><xf numFmtId="14"/></cellXfs></styleSheet>"#
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                worksheet(
                    r#"<row r="1"><c r="A1" s="1"><v>45356</v></c></row>
                       <row r="2"><c r="A2" s="2"><v>45356</v></c></row>
                       <row r="3"><c r="A3"><v>45356</v></c></row>"#,
                ),
            ),
        ]);
        let document = XlsxDocument::parse_bytes(&pkg).expect("parse");
        assert_eq!(
            texts(&document.sheets[0].tables[0]),
            vec![
                (0, 0, "2024-03-05 00:00:00".to_string()),
                (1, 0, "2024-03-05 00:00:00".to_string()),
                // Without a date format the serial is the number it is.
                (2, 0, "45356".to_string()),
            ]
        );
    }

    #[test]
    fn the_1900_leap_day_and_times_read_as_openpyxl_reads_them() {
        assert_eq!(excel_datetime(1.0, false).unwrap(), "1900-01-01 00:00:00");
        assert_eq!(excel_datetime(59.0, false).unwrap(), "1900-02-28 00:00:00");
        assert_eq!(excel_datetime(60.0, false).unwrap(), "1900-02-28 00:00:00");
        assert_eq!(excel_datetime(61.0, false).unwrap(), "1900-03-01 00:00:00");
        assert_eq!(excel_datetime(0.5, false).unwrap(), "12:00:00");
        assert_eq!(
            excel_datetime(45356.5, false).unwrap(),
            "2024-03-05 12:00:00"
        );
        assert_eq!(excel_datetime(1.0, true).unwrap(), "1904-01-02 00:00:00");
    }

    #[test]
    fn a_hidden_sheet_is_kept_and_marked() {
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(
                    r#"<sheet name="Visible" sheetId="1" r:id="rId1"/>
                       <sheet name="Hidden" sheetId="2" state="hidden" r:id="rId2"/>"#,
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>
                       <Relationship Id="rId2" Type="http://x/worksheet" Target="/xl/worksheets/sheet2.xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                worksheet(r#"<row r="1"><c r="A1"><v>1</v></c></row>"#),
            ),
            (
                "xl/worksheets/sheet2.xml",
                worksheet(r#"<row r="1"><c r="A1"><v>2</v></c></row>"#),
            ),
        ]);
        let document = XlsxDocument::parse_bytes(&pkg).expect("parse");
        assert_eq!(document.sheets.len(), 2);
        assert_eq!(document.sheets[0].name, "Visible");
        assert!(!document.sheets[0].hidden);
        assert_eq!(document.sheets[1].name, "Hidden");
        assert!(document.sheets[1].hidden);
    }

    #[test]
    fn cells_without_references_keep_their_places() {
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(r#"<sheet name="Sheet1" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                worksheet(r#"<row><c><v>1</v></c><c><v>2</v></c></row><row><c><v>3</v></c></row>"#),
            ),
        ]);
        let document = XlsxDocument::parse_bytes(&pkg).expect("parse");
        let table = &document.sheets[0].tables[0];
        assert_eq!((table.num_rows, table.num_cols), (2, 2));
        // The bounding box is the table: the slot the second row does not
        // reach is an empty cell, as it is in Docling's own reading.
        assert_eq!(
            texts(table),
            vec![
                (0, 0, "1".to_string()),
                (0, 1, "2".to_string()),
                (1, 0, "3".to_string()),
                (1, 1, String::new()),
            ]
        );
    }

    #[test]
    fn a_sheet_with_no_cells_has_no_tables() {
        let document = parse_sheet("");
        assert!(document.sheets[0].tables.is_empty());
    }

    #[test]
    fn a_number_format_is_a_date_only_when_its_tokens_say_so() {
        assert!(is_date_format("yyyy-mm-dd"));
        assert!(is_date_format("d/m/yyyy h:mm"));
        assert!(!is_date_format("0.00"));
        assert!(!is_date_format("#,##0"));
        assert!(!is_date_format(r#"0.00 "days""#));
        assert!(!is_date_format("[Red]0.0"));
        assert!(!is_date_format("General"));
        assert!(is_date_format("[h]:mm:ss"));
    }

    #[test]
    fn a_comment_or_a_chart_does_not_stop_the_cells_being_read() {
        // `sheetData` is not the only child of a worksheet; the parts after it
        // (`mergeCells`, data validations, the drawing) must not be mistaken
        // for cells nor wipe what was read.
        let pkg = package(&[
            (
                "xl/workbook.xml",
                workbook(r#"<sheet name="Sheet1" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!(
                    r#"<worksheet {NS}><sheetData><row r="1"><c r="A1"><v>7</v></c></row></sheetData>
                       <dataValidations count="1"><dataValidation type="list"><formula1>"yes,no"</formula1></dataValidation></dataValidations>
                       <drawing r:id="rId9"/></worksheet>"#
                ),
            ),
        ]);
        let document = XlsxDocument::parse_bytes(&pkg).expect("parse");
        assert_eq!(
            texts(&document.sheets[0].tables[0]),
            vec![(0, 0, "7".to_string())]
        );
    }

    /// The namespaces a drawing part declares, and the one its `r:embed`
    /// attributes are in — `local_name` reads the attribute whichever prefix
    /// it wears.
    const XDR: &str = r#"xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

    /// A one-sheet package whose worksheet draws `anchors`, with the media
    /// parts those anchors reach: `(relationship id, part under xl/, bytes)`.
    fn parse_drawing(anchors: &str, media: &[(&str, &str, &str)]) -> XlsxDocument {
        let entries: String = media
            .iter()
            .map(|(id, part, _)| {
                format!(r#"<Relationship Id="{id}" Type="http://x/image" Target="../{part}"/>"#)
            })
            .collect();
        let mut parts: Vec<(String, String)> = vec![
            (
                "xl/workbook.xml".to_string(),
                workbook(r#"<sheet name="Drawn" sheetId="1" r:id="rId1"/>"#),
            ),
            (
                "xl/_rels/workbook.xml.rels".to_string(),
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/worksheet" Target="worksheets/sheet1.xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml".to_string(),
                format!(r#"<worksheet {NS}><sheetData/><drawing r:id="rId1"/></worksheet>"#),
            ),
            (
                "xl/worksheets/_rels/sheet1.xml.rels".to_string(),
                relationships(
                    r#"<Relationship Id="rId1" Type="http://x/drawing" Target="../drawings/drawing1.xml"/>"#,
                ),
            ),
            (
                "xl/drawings/drawing1.xml".to_string(),
                format!(r#"<xdr:wsDr {XDR}>{anchors}</xdr:wsDr>"#),
            ),
            (
                "xl/drawings/_rels/drawing1.xml.rels".to_string(),
                relationships(&entries),
            ),
        ];
        parts.extend(
            media
                .iter()
                .map(|(_, part, bytes)| (format!("xl/{part}"), bytes.to_string())),
        );
        let borrowed: Vec<(&str, String)> = parts
            .iter()
            .map(|(name, body)| (name.as_str(), body.clone()))
            .collect();
        XlsxDocument::parse_bytes(&package(&borrowed)).expect("parse")
    }

    #[test]
    fn a_drawing_anchor_places_its_picture_on_the_cells_it_names() {
        // Three anchors, the three kinds a writer emits: the two-corner one
        // that names the cells a picture spans, the one-corner one that names a
        // corner and a size, and the absolute one that names neither cells nor
        // a page but its own position.
        let anchors = format!(
            r#"<xdr:twoCellAnchor>
                 <xdr:from><xdr:col>1</xdr:col><xdr:colOff>{col}</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>
                 <xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>5</xdr:row><xdr:rowOff>{row}</xdr:rowOff></xdr:to>
                 <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
               </xdr:twoCellAnchor>
               <xdr:oneCellAnchor>
                 <xdr:from><xdr:col>6</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>
                 <xdr:ext cx="1219200" cy="381000"/>
                 <xdr:pic><xdr:blipFill><a:blip r:embed="rId2"/></xdr:blipFill></xdr:pic>
               </xdr:oneCellAnchor>
               <xdr:absoluteAnchor>
                 <xdr:pos x="609600" y="190500"/>
                 <xdr:ext cx="609600" cy="190500"/>
                 <xdr:pic><xdr:blipFill><a:blip r:embed="rId3"/></xdr:blipFill></xdr:pic>
               </xdr:absoluteAnchor>"#,
            col = EMU_PER_COLUMN,
            row = EMU_PER_ROW,
        );
        let document = parse_drawing(
            &anchors,
            &[
                ("rId1", "media/image1.bmp", "a bitmap"),
                ("rId2", "media/image2.gif", "a gif"),
                ("rId3", "media/image3.png", "a png"),
            ],
        );

        let pictures = &document.sheets[0].pictures;
        assert_eq!(pictures.len(), 3, "{pictures:#?}");
        let described: Vec<(&str, Vec<u8>, [f64; 4])> = pictures
            .iter()
            .map(|picture| (picture.mime, picture.data.clone(), picture.bbox))
            .collect();
        assert_eq!(
            described,
            vec![
                // From B3, one column into the cell, to E6, one row into it.
                ("image/bmp", b"a bitmap".to_vec(), [2.0, 2.0, 4.0, 6.0]),
                // A corner at G2 and a rectangle two columns by two rows.
                ("image/gif", b"a gif".to_vec(), [6.0, 1.0, 8.0, 3.0]),
                // A position one column and one row in, one cell square.
                ("image/png", b"a png".to_vec(), [1.0, 1.0, 2.0, 2.0]),
            ]
        );
    }

    /// A drawing holds more than pictures, and a relationship names more than
    /// images: a chart's frame and a picture whose media part is not photo-
    /// graphic are anchors this must pass over rather than report as pictures.
    #[test]
    fn a_chart_frame_and_a_part_that_is_no_picture_are_not_pictures() {
        let anchors = r#"<xdr:twoCellAnchor>
                 <xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>0</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>
                 <xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>8</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>
                 <xdr:graphicFrame>
                   <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="rId1"/></a:graphicData></a:graphic>
                 </xdr:graphicFrame>
               </xdr:twoCellAnchor>
               <xdr:twoCellAnchor>
                 <xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>9</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>
                 <xdr:to><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>10</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>
                 <xdr:pic><xdr:blipFill><a:blip r:embed="rId2"/></xdr:blipFill></xdr:pic>
               </xdr:twoCellAnchor>"#;
        let document = parse_drawing(
            anchors,
            &[
                ("rId1", "charts/chart1.xml", "<chart/>"),
                ("rId2", "media/oleObject1.bin", "an OLE object"),
            ],
        );
        assert!(
            document.sheets[0].pictures.is_empty(),
            "a drawing was read as a picture: {:#?}",
            document.sheets[0].pictures
        );
    }

    #[test]
    fn a_relationship_target_is_resolved_against_its_own_part() {
        assert_eq!(
            resolve_target("../media/image1.bmp", "xl/drawings").as_deref(),
            Some("xl/media/image1.bmp")
        );
        assert_eq!(
            resolve_target("worksheets/sheet1.xml", "xl").as_deref(),
            Some("xl/worksheets/sheet1.xml")
        );
        // A target that starts with `/` is written from the package root, not
        // from the part that names it.
        assert_eq!(
            resolve_target("/xl/media/image1.bmp", "xl/drawings").as_deref(),
            Some("xl/media/image1.bmp")
        );
        assert_eq!(
            resolve_target("./media/image1.bmp", "xl").as_deref(),
            Some("xl/media/image1.bmp")
        );
        // A target that climbs out of the package names no part at all.
        assert_eq!(resolve_target("../../media/image1.bmp", "xl"), None);
    }

    /// A chart's reference is resolved the way a spreadsheet resolves one: a
    /// quoted sheet name, cells absolute or not, and a range that comes back in
    /// the order a spreadsheet fills it — down a column, across a row.
    #[test]
    fn a_chart_reference_reads_the_cells_it_names() {
        let mut sheet = RawSheet::default();
        for (row, label, value) in [(1usize, "North", "120"), (2, "South", "90")] {
            sheet.cells.insert((row, 0), label.to_string());
            sheet.cells.insert((row, 1), value.to_string());
        }
        let sheets: Vec<(&str, &RawSheet)> = vec![("Q1 report", &sheet)];

        // Quoted, because the name holds a space; the rows down the column.
        assert_eq!(
            resolve_chart_reference("'Q1 report'!$A$2:$A$3", "Other", &sheets),
            vec!["North", "South"]
        );
        // A row across, with no dollar signs and the range written backwards.
        assert_eq!(
            resolve_chart_reference("'Q1 report'!B3:B2", "Other", &sheets),
            vec!["120", "90"]
        );
        // No sheet name: it is the drawing's own sheet.
        assert_eq!(
            resolve_chart_reference("$B$2", "Q1 report", &sheets),
            vec!["120"]
        );
        // A cell with no value is the empty string it is, not a missing row.
        assert_eq!(
            resolve_chart_reference("'Q1 report'!B2:B4", "Other", &sheets),
            vec!["120", "90", ""]
        );
    }

    /// A reference this cannot follow answers nothing, so the chart keeps the
    /// data it carried rather than inventing numbers for it.
    #[test]
    fn a_chart_reference_that_names_nothing_is_nothing() {
        let sheet = RawSheet::default();
        let sheets: Vec<(&str, &RawSheet)> = vec![("Sheet1", &sheet)];
        for reference in ["Other!$A$1", "Sheet1!named_range", "[1]"] {
            assert!(
                resolve_chart_reference(reference, "Sheet1", &sheets).is_empty(),
                "{reference} answered cells"
            );
        }
        // A range that exists but holds nothing is two empty cells, not no
        // rows: the chart's own row count is the range's.
        assert_eq!(
            resolve_chart_reference("Sheet1!$A$1:$A$2", "Other", &sheets),
            vec!["", ""]
        );
        // `[1]Sheet1!A1` names this package's sheet, the bracket dropped.
        assert_eq!(
            resolve_chart_reference("[1]Sheet1!A1", "Other", &sheets),
            vec![""]
        );
    }
}
