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

#[cfg(test)]
mod tests {
    use super::*;
    use pdfextractor_xlsx_parser::{SheetCell, SheetTable};

    const CHART: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:chart="urn:oasis:names:tc:opendocument:xmlns:chart:1.0"
    xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
  <office:body>
    <office:chart>
      <chart:chart chart:class="chart:bar">
        <chart:title><text:p>Cross-sheet revenue</text:p></chart:title>
        <chart:plot-area>
          <chart:axis chart:dimension="x">
            <chart:categories table:cell-range-address="Data.A2:Data.A4"/>
          </chart:axis>
          <chart:series
              chart:values-cell-range-address="Data.B2:Data.B4"
              chart:label-cell-address="Data.B1:Data.B1"/>
          <chart:series
              chart:values-cell-range-address="Data.C2:Data.C4"
              chart:label-cell-address="Data.C1:Data.C1"/>
        </chart:plot-area>
      </chart:chart>
    </office:chart>
  </office:body>
</office:document-content>"#;

    fn sheet(name: &str, rows: &[&[&str]]) -> Sheet {
        let num_rows = rows.len();
        let num_cols = rows.first().map_or(0, |row| row.len());
        let cells = rows
            .iter()
            .enumerate()
            .flat_map(|(row, values)| {
                values.iter().enumerate().map(move |(col, text)| SheetCell {
                    row,
                    col,
                    text: text.to_string(),
                    row_span: 1,
                    col_span: 1,
                })
            })
            .collect();
        Sheet {
            name: name.to_string(),
            hidden: false,
            tables: vec![SheetTable {
                anchor: (0, 0),
                num_rows,
                num_cols,
                cells,
            }],
            pictures: Vec::new(),
            charts: Vec::new(),
        }
    }

    #[test]
    fn chart_ranges_read_categories_and_values_from_the_sheet_they_name() {
        // The chart is anchored on Charts, whose cells are deliberately wrong:
        // every range below names Data, so only Data's cells can answer this.
        let sheets = vec![
            sheet(
                "Data",
                &[
                    &["Region", "Q1", "Q2"],
                    &["North", "120", "140"],
                    &["South", "90", "95"],
                    &["West", "60", "80"],
                ],
            ),
            sheet(
                "Charts",
                &[
                    &["Wrong", "Wrong", "Wrong"],
                    &["Wrong", "999", "999"],
                    &["Wrong", "999", "999"],
                    &["Wrong", "999", "999"],
                ],
            ),
        ];

        let chart = read_chart(CHART.as_bytes(), "Charts", &sheets).expect("a readable chart");
        assert_eq!(chart.title.as_deref(), Some("Cross-sheet revenue"));
        assert_eq!(chart.kind, ChartKind::Bar);
        assert_eq!(chart.categories, ["North", "South", "West"]);
        assert_eq!(chart.series.len(), 2);
        assert_eq!(chart.series[0].name, "Q1");
        assert_eq!(
            chart.series[0].values,
            [Some(120.0), Some(90.0), Some(60.0)]
        );
        assert_eq!(chart.series[1].name, "Q2");
        assert_eq!(
            chart.series[1].values,
            [Some(140.0), Some(95.0), Some(80.0)]
        );
    }

    /// A sheet whose name holds a space is written quoted, and ODF escapes an
    /// apostrophe inside the quotes by doubling it. Both names still reach the
    /// sheet they name rather than the sheet the chart is anchored on.
    #[test]
    fn chart_ranges_read_quoted_sheet_names() {
        const QUOTED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:chart="urn:oasis:names:tc:opendocument:xmlns:chart:1.0"
    xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
  <office:body>
    <office:chart>
      <chart:chart chart:class="chart:line">
        <chart:title><text:p>Quoted sheets</text:p></chart:title>
        <chart:plot-area>
          <chart:axis chart:dimension="x">
            <chart:categories table:cell-range-address="'Quarterly Totals'.A2:'Quarterly Totals'.A4"/>
          </chart:axis>
          <chart:series
              chart:values-cell-range-address="'Quarterly Totals'.B2:'Quarterly Totals'.B4"
              chart:label-cell-address="'Quarterly Totals'.B1:'Quarterly Totals'.B1"/>
          <chart:series
              chart:values-cell-range-address="'Bob''s Data'.B2:'Bob''s Data'.B4"
              chart:label-cell-address="'Bob''s Data'.B1:'Bob''s Data'.B1"/>
        </chart:plot-area>
      </chart:chart>
    </office:chart>
  </office:body>
</office:document-content>"#;

        // The chart's own sheet is deliberately wrong: only the quoted sheets can
        // answer the ranges.
        let sheets = vec![
            sheet(
                "Charts",
                &[
                    &["Wrong", "Wrong", "Wrong"],
                    &["Wrong", "999", "999"],
                    &["Wrong", "999", "999"],
                    &["Wrong", "999", "999"],
                ],
            ),
            sheet(
                "Quarterly Totals",
                &[
                    &["Region", "Q1"],
                    &["North", "120"],
                    &["South", "90"],
                    &["West", "60"],
                ],
            ),
            sheet(
                "Bob's Data",
                &[
                    &["Region", "Q2"],
                    &["North", "140"],
                    &["South", "95"],
                    &["West", "80"],
                ],
            ),
        ];

        let chart = read_chart(QUOTED.as_bytes(), "Charts", &sheets).expect("a readable chart");
        assert_eq!(chart.title.as_deref(), Some("Quoted sheets"));
        assert_eq!(chart.kind, ChartKind::Line);
        assert_eq!(chart.categories, ["North", "South", "West"]);
        assert_eq!(chart.series.len(), 2);
        assert_eq!(chart.series[0].name, "Q1");
        assert_eq!(
            chart.series[0].values,
            [Some(120.0), Some(90.0), Some(60.0)]
        );
        assert_eq!(chart.series[1].name, "Q2");
        assert_eq!(
            chart.series[1].values,
            [Some(140.0), Some(95.0), Some(80.0)]
        );
    }

    /// A range that names no sheet in the workbook answers no cells: it does not
    /// fall back to the sheet the chart is anchored on, and it does not stop the
    /// series beside it that does name a sheet from being read.
    #[test]
    fn chart_ranges_naming_a_missing_sheet_answer_no_cells() {
        const MISSING: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:chart="urn:oasis:names:tc:opendocument:xmlns:chart:1.0"
    xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
  <office:body>
    <office:chart>
      <chart:chart chart:class="chart:bar">
        <chart:title><text:p>Missing sheets</text:p></chart:title>
        <chart:plot-area>
          <chart:axis chart:dimension="x">
            <chart:categories table:cell-range-address="Nowhere.A2:Nowhere.A4"/>
          </chart:axis>
          <chart:series
              chart:values-cell-range-address="Gone.B2:Gone.B4"
              chart:label-cell-address="Gone.B1:Gone.B1"/>
          <chart:series
              chart:values-cell-range-address="Data.B2:Data.B4"
              chart:label-cell-address="Data.B1:Data.B1"/>
        </chart:plot-area>
      </chart:chart>
    </office:chart>
  </office:body>
</office:document-content>"#;

        let sheets = vec![
            sheet("Charts", &[&["Wrong"], &["999"], &["999"], &["999"]]),
            sheet(
                "Data",
                &[
                    &["Region", "Q1"],
                    &["North", "120"],
                    &["South", "90"],
                    &["West", "60"],
                ],
            ),
        ];

        let chart = read_chart(MISSING.as_bytes(), "Charts", &sheets).expect("a readable chart");
        assert_eq!(chart.title.as_deref(), Some("Missing sheets"));
        assert_eq!(chart.kind, ChartKind::Bar);
        assert!(
            chart.categories.is_empty(),
            "a missing sheet's categories were invented: {:?}",
            chart.categories
        );
        assert_eq!(chart.series.len(), 2);
        assert_eq!(chart.series[0].name, "");
        assert!(
            chart.series[0].values.is_empty(),
            "a missing sheet's values were invented: {:?}",
            chart.series[0].values
        );
        // The series that names a sheet in the workbook is still read.
        assert_eq!(chart.series[1].name, "Q1");
        assert_eq!(
            chart.series[1].values,
            [Some(120.0), Some(90.0), Some(60.0)]
        );
    }
}
