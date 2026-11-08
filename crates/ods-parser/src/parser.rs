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
//! Not read yet: the pictures and charts a sheet draws, notes and annotations,
//! comments and tracked changes, and the formula itself — only the value it
//! displays is read.

use anyhow::{Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Cursor, Read};

use pdfextractor_xlsx_parser::{Sheet, SheetCell, SheetTable};

/// A `table:number-*-repeated` larger than this is filler rather than content:
/// Calc writes the rest of a sheet as one repeated empty cell or row.
const MAX_REPEAT: usize = 1024;

/// One OpenDocument spreadsheet.
#[derive(Debug, Clone)]
pub struct OdsDocument {
    pub sheets: Vec<Sheet>,
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

        // A hidden sheet is named by its style, and the style is stated in the
        // same part: read the whole part before any sheet is.
        let hidden = hidden_sheet_styles(&content)?;
        Ok(Self {
            sheets: read_sheets(&content, &hidden)?,
        })
    }
}

fn read_part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut file = archive.by_name(name).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    Some(buffer)
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
fn read_sheets(xml: &[u8], hidden_styles: &HashSet<String>) -> Result<Vec<Sheet>> {
    let mut pull = Pull::new(xml);
    let mut sheets = Vec::new();
    loop {
        match pull.next()? {
            Tok::Start { name, attrs } if name == "table" => {
                let index = sheets.len() + 1;
                sheets.push(read_sheet(&mut pull, &attrs, index, hidden_styles)?);
            }
            Tok::Eof => break,
            _ => {}
        }
    }
    Ok(sheets)
}

/// One `table:table`: its name, whether it is hidden, and the tables its cells
/// form.
fn read_sheet(
    pull: &mut Pull<'_>,
    attrs: &[(String, String)],
    index: usize,
    hidden_styles: &HashSet<String>,
) -> Result<Sheet> {
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

    loop {
        match pull.next()? {
            Tok::Start { name, attrs } if name == "table-row" => {
                let repeat = count(&attrs, "number-rows-repeated").min(MAX_REPEAT);
                let cells = read_row(pull)?;
                for copy in 0..repeat {
                    for cell in &cells {
                        place(&mut grid, &mut covered, row + copy, cell);
                    }
                }
                row = row.saturating_add(repeat);
            }
            Tok::End { name } if name == "table" => break,
            Tok::Eof => break,
            _ => {}
        }
    }

    Ok(Sheet {
        name,
        hidden,
        tables: tables(&grid, &covered),
        // A sheet's pictures and charts are not read yet.
        pictures: Vec::new(),
        charts: Vec::new(),
    })
}

/// One cell a row holds, at the column it starts at.
struct RowCell {
    col: usize,
    text: String,
    row_span: usize,
    col_span: usize,
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
fn read_row(pull: &mut Pull<'_>) -> Result<Vec<RowCell>> {
    let mut cells = Vec::new();
    let mut col = 0usize;
    loop {
        match pull.next()? {
            Tok::Start { name, attrs } => match name.as_str() {
                "table-cell" => {
                    let row_span = count(&attrs, "number-rows-spanned");
                    let col_span = count(&attrs, "number-columns-spanned");
                    let repeat = count(&attrs, "number-columns-repeated");
                    let text = read_cell(pull, &attrs)?;
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
/// it — so it is preferred. A typed value fills the cells a producer leaves
/// without paragraphs: a boolean is `TRUE`/`FALSE` as a spreadsheet shows it,
/// and a date, a time, a string or a number is the value the element states.
fn read_cell(pull: &mut Pull<'_>, attrs: &[(String, String)]) -> Result<String> {
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
                // A comment is not the cell's text, and a frame's words are the
                // frame's; both are left out rather than read as the value.
                "annotation" | "note" | "frame" | "object" => skip_element(pull)?,
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
    if text.is_empty() {
        Ok(typed_value(attrs))
    } else {
        Ok(text)
    }
}

/// The value a cell states when it displays no text.
fn typed_value(attrs: &[(String, String)]) -> String {
    match field(attrs, "value-type") {
        Some("string") => field(attrs, "string-value").unwrap_or_default().to_string(),
        Some("date") => field(attrs, "date-value").unwrap_or_default().to_string(),
        Some("time") => field(attrs, "time-value").unwrap_or_default().to_string(),
        Some("boolean") => match field(attrs, "boolean-value") {
            Some("true") => "TRUE".to_string(),
            Some("false") => "FALSE".to_string(),
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
        for col in min_col..=max_col {
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
              </table:table-row>
            </table:table>"#;
        let document = OdsDocument::parse_bytes(&package(&content(body))).unwrap();
        assert_eq!(
            texts(&document.sheets[0]),
            ["Widget", "1.5", "TRUE", "2024-03-05", "15"],
            "a formula reads the value it displays, not its markup"
        );
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
