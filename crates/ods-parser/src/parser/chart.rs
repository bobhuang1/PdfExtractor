//! Reading an OpenDocument chart into the data it plots.
//!
//! Calc stores a chart as an embedded object: the sheet's `draw:object` names a
//! directory, and that directory's `content.xml` holds an `office:chart` whose
//! `chart:series` state the cells they plot. The chart is not a picture part, so
//! it is read as the numbers behind it — the same [`ChartShape`] the XLSX
//! reader produces — and placed by the frame that anchored it.

use pdfextractor_chart::{ChartKind, ChartSeries, ChartShape};
use pdfextractor_xlsx_parser::Sheet;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::io::Cursor;

/// Read one embedded chart part.
///
/// `sheet_name` is the sheet the chart is anchored on; a range with no sheet of
/// its own names that sheet. A chart with no `chart` or no `plot-area` is not a
/// chart that can be read, and answers `None`.
pub(crate) fn read_chart(bytes: &[u8], sheet_name: &str, sheets: &[Sheet]) -> Option<ChartShape> {
    let root = Node::parse(bytes)?;
    // The XML root is `office:chart`; the chart itself is its `chart:chart`
    // child, not the wrapper that carries it.
    let office_chart = root.find("chart")?;
    let chart = office_chart.child("chart").unwrap_or(office_chart);
    let plot_area = chart.child("plot-area")?;

    let categories = plot_area
        .children_named("axis")
        .find(|axis| axis.attr("dimension") == Some("x"))
        .and_then(|axis| axis.child("categories"))
        .and_then(|categories| categories.attr("cell-range-address"))
        .map(|range| resolve(range, sheet_name, sheets))
        .unwrap_or_default();

    let series = plot_area
        .children_named("series")
        .map(|series| {
            let values = series
                .attr("values-cell-range-address")
                .map(|range| resolve(range, sheet_name, sheets))
                .unwrap_or_default();
            let name = series
                .attr("label-cell-address")
                .map(|range| resolve(range, sheet_name, sheets))
                .and_then(|cells| cells.into_iter().next())
                .unwrap_or_default();
            ChartSeries {
                name,
                values: values.iter().map(|text| number(text)).collect(),
            }
        })
        .collect();

    Some(ChartShape {
        // The frame that anchors the object supplies the box.
        bbox: [0.0; 4],
        title: chart
            .child("title")
            .map(text_content)
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty()),
        kind: chart_kind(chart.attr("class").unwrap_or_default()),
        categories,
        series,
    })
}

fn chart_kind(class: &str) -> ChartKind {
    match class.rsplit(':').next().unwrap_or(class) {
        "bar" => ChartKind::Bar,
        "line" => ChartKind::Line,
        "pie" => ChartKind::Pie,
        "scatter" => ChartKind::Scatter,
        _ => ChartKind::Other,
    }
}

fn number(text: &str) -> Option<f64> {
    text.trim().parse().ok()
}

/// The cells an ODF chart reference names, in spreadsheet order.
///
/// A reference is `Sheet.A1:B2` — the sheet name and a range of cells — or a
/// range alone, which names the chart's own sheet. A range with a quoted sheet
/// name (`'Quarterly Totals'.A1:B2`) is read too. A reference this cannot parse,
/// or a sheet it does not find, answers no cells rather than invented ones.
fn resolve(reference: &str, default_sheet: &str, sheets: &[Sheet]) -> Vec<String> {
    let reference = reference.trim();
    let (sheet, cells) = match reference.split_once('.') {
        Some((sheet, cells)) => (unquote_sheet(sheet), cells),
        None => (default_sheet.to_string(), reference),
    };
    let Some(sheet) = sheets
        .iter()
        .find(|candidate| candidate.name.eq_ignore_ascii_case(&sheet))
    else {
        return Vec::new();
    };
    let (start, end) = match cells.split_once(':') {
        Some((start, end)) => (start, end),
        None => (cells, cells),
    };
    // ODF repeats the sheet on the end of a range (`Quarterly.B2:Quarterly.B4`),
    // so an end that still names a sheet keeps only its cell.
    let start = start
        .rsplit_once('.')
        .map(|(_, cell)| cell)
        .unwrap_or(start);
    let end = end.rsplit_once('.').map(|(_, cell)| cell).unwrap_or(end);
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
            values.push(cell_text(sheet, row, col));
        }
    }
    values
}

fn unquote_sheet(sheet: &str) -> String {
    sheet.trim().trim_matches('\'').replace("''", "'")
}

fn cell_address(address: &str) -> Option<(usize, usize)> {
    let address = address.trim().trim_start_matches('$');
    let split = address
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(address.len());
    let (letters, row) = address.split_at(split);
    if letters.is_empty() {
        return None;
    }
    let col = letters.chars().try_fold(0usize, |col, letter| {
        let digit = letter.to_ascii_uppercase() as u8;
        if !digit.is_ascii_uppercase() {
            return None;
        }
        col.checked_mul(26)?
            .checked_add((digit - b'A' + 1) as usize)
    })?;
    let row = row.trim_start_matches('$').parse::<usize>().ok()?;
    Some((row.checked_sub(1)?, col.checked_sub(1)?))
}

/// The text at one cell of a sheet, or an empty string when no table holds it.
fn cell_text(sheet: &Sheet, row: usize, col: usize) -> String {
    for table in &sheet.tables {
        let (left, top) = table.anchor;
        if row >= top && row < top + table.num_rows && col >= left && col < left + table.num_cols {
            return table
                .cells
                .iter()
                .find(|cell| cell.row == row - top && cell.col == col - left)
                .map(|cell| cell.text.clone())
                .unwrap_or_default();
        }
    }
    String::new()
}

fn text_content(node: &Node) -> String {
    let mut text = node.text.clone();
    for child in &node.children {
        text.push_str(&text_content(child));
    }
    text
}

// --- XML tree -------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    fn parse(xml: &[u8]) -> Option<Self> {
        let mut reader = Reader::from_reader(Cursor::new(xml));
        reader.config_mut().trim_text(false);
        let mut stack = vec![Node {
            name: "#document".to_string(),
            ..Node::default()
        }];
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(start)) => stack.push(Node::from_start(&start)),
                Ok(Event::Empty(start)) => {
                    let node = Node::from_start(&start);
                    stack.last_mut()?.children.push(node);
                }
                Ok(Event::End(_)) => {
                    if stack.len() > 1 {
                        let node = stack.pop()?;
                        stack.last_mut()?.children.push(node);
                    }
                }
                Ok(Event::Text(text)) => {
                    stack
                        .last_mut()?
                        .text
                        .push_str(text.unescape().ok()?.as_ref());
                }
                Ok(Event::CData(text)) => {
                    stack
                        .last_mut()?
                        .text
                        .push_str(&String::from_utf8_lossy(&text));
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(_) => return None,
            }
            buffer.clear();
        }
        stack.pop()
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

    fn find(&self, name: &str) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(name))
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |child| child.name == name)
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
