//! Reading an OpenDocument presentation's embedded chart into the data it plots.
//!
//! Impress stores a chart as an embedded object: a slide's `draw:object` names
//! a directory, and that directory's `content.xml` holds an `office:chart`.
//! Unlike a Calc chart, whose series name cells on a sheet, a presentation's
//! chart carries its data in a `table:table` beside the chart, named
//! `local-table`; the series' ranges point into that table. The chart is not a
//! picture part, so it is read as the numbers behind it — the same
//! [`ChartShape`] the XLSX and ODS readers produce — and placed by the frame
//! that anchored it. A chart object without that table has no data to
//! reconstruct and is not read.

use super::{parse_table, Content, Node};
use pdfextractor_chart::{ChartKind, ChartSeries, ChartShape};
use std::collections::HashMap;

/// Read one embedded chart part, placed at the frame's box.
///
/// A part with no chart, a chart with no plot area, or a chart with no
/// `local-table` is not a chart that can be read, and answers `None`.
pub(super) fn read_chart(bytes: &[u8], bbox: [f64; 4]) -> Option<ChartShape> {
    let root = Node::parse(bytes).ok()?;
    // The XML root is `office:chart`; the chart itself is its `chart:chart`
    // child, not the wrapper that carries it.
    let office_chart = root.find("chart")?;
    let chart = office_chart.child("chart").unwrap_or(office_chart);
    let plot_area = chart.child("plot-area")?;

    let table = root
        .descendants_named("table")
        .into_iter()
        .find(|table| table.attr("name") == Some("local-table"))?;
    let cells = parse_table(table)
        .cells
        .into_iter()
        .map(|cell| ((cell.row, cell.col), cell.text))
        .collect::<HashMap<_, _>>();

    let categories = plot_area
        .children_named("axis")
        .find(|axis| axis.attr("dimension") == Some("x"))
        .and_then(|axis| axis.child("categories"))
        .or_else(|| plot_area.child("categories"))
        .and_then(|categories| categories.attr("cell-range-address"))
        // A scatter chart has no category axis: LibreOffice writes each
        // series' x-values in its `chart:domain` instead. The chart model has
        // one categories column, so the first series' domain is its x-axis.
        .or_else(|| {
            plot_area
                .children_named("series")
                .find_map(|series| series.child("domain"))
                .and_then(|domain| domain.attr("cell-range-address"))
        })
        .map(|range| resolve(range, &cells))
        .unwrap_or_default();

    let series = plot_area
        .children_named("series")
        .map(|series| {
            let values = series
                .attr("values-cell-range-address")
                .map(|range| resolve(range, &cells))
                .unwrap_or_default();
            let name = series
                .attr("label-cell-address")
                .map(|range| resolve(range, &cells))
                .and_then(|cells| cells.into_iter().next())
                .unwrap_or_default();
            ChartSeries {
                name,
                values: values.iter().map(|text| number(text)).collect(),
            }
        })
        .collect();

    Some(ChartShape {
        bbox,
        title: chart
            .child("title")
            .map(text_content)
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty()),
        kind: chart_kind(chart, plot_area),
        categories,
        series,
    })
}

fn chart_kind(chart: &Node, plot_area: &Node) -> ChartKind {
    chart
        .attr("class")
        .into_iter()
        .chain(
            plot_area
                .children_named("series")
                .filter_map(|series| series.attr("class")),
        )
        .find_map(kind_of)
        .unwrap_or(ChartKind::Other)
}

fn kind_of(class: &str) -> Option<ChartKind> {
    Some(match class.rsplit(':').next().unwrap_or(class) {
        "bar" => ChartKind::Bar,
        "line" => ChartKind::Line,
        // LibreOffice writes a 3D pie as `chart:circle`; both are pies.
        "pie" | "circle" => ChartKind::Pie,
        "scatter" => ChartKind::Scatter,
        _ => return None,
    })
}

/// A plotted value, with the chart's missing-point marker read as the gap it
/// draws rather than as a number.
fn number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("nan") {
        return None;
    }
    text.parse().ok()
}

/// The cells a chart reference names, in table order.
///
/// A presentation chart's reference is `local-table.$B$2:.$B$4`: the chart's
/// own table and a range of cells. The sheet name is accepted whether or not it
/// is present, and a missing cell is an empty string rather than invented data.
fn resolve(reference: &str, cells: &HashMap<(usize, usize), String>) -> Vec<String> {
    let reference = reference.trim();
    let cells_part = reference
        .split_once('.')
        .map(|(_, cells)| cells)
        .unwrap_or(reference);
    let (start, end) = cells_part
        .split_once(':')
        .map_or((cells_part, cells_part), |(start, end)| (start, end));
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
            values.push(cells.get(&(row, col)).cloned().unwrap_or_default());
        }
    }
    values
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

fn text_content(node: &Node) -> String {
    fn walk(node: &Node, text: &mut String) {
        for content in &node.content {
            match content {
                Content::Text(part) => text.push_str(part),
                Content::Child(index) => {
                    if let Some(child) = node.children.get(*index) {
                        walk(child, text);
                    }
                }
            }
        }
    }
    let mut text = String::new();
    walk(node, &mut text);
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = r#"<table name="local-table">
        <table-row><table-cell/><table-cell><p>Q2</p></table-cell><table-cell><p>Q3</p></table-cell></table-row>
        <table-row><table-cell><p>North America</p></table-cell><table-cell><p>120</p></table-cell><table-cell><p>145</p></table-cell></table-row>
        <table-row><table-cell><p>Europe</p></table-cell><table-cell><p>98</p></table-cell><table-cell><p>121</p></table-cell></table-row>
        <table-row><table-cell><p>Asia Pacific</p></table-cell><table-cell><p>74</p></table-cell><table-cell><p>NaN</p></table-cell></table-row>
    </table>"#;

    const AXIS_CATEGORIES: &str =
        r#"<axis dimension="x"><categories cell-range-address="local-table.$A$2:.$A$4"/></axis>"#;
    const DIRECT_CATEGORIES: &str = r#"<categories cell-range-address="local-table.$A$2:.$A$4"/>"#;

    fn fixture(class: &str, categories: &str, series: &str) -> String {
        format!(
            r#"<document-content><body><chart>
                <chart class="chart:{class}">
                    <title><text:p>{class} by region</text:p></title>
                    <plot-area>{categories}{series}</plot-area>
                </chart>
                {TABLE}
            </chart></body></document-content>"#
        )
    }

    #[test]
    fn a_line_chart_reads_every_series_it_draws() {
        let xml = fixture(
            "line",
            AXIS_CATEGORIES,
            r#"<series values-cell-range-address="local-table.$B$2:.$B$4" label-cell-address="local-table.$B$1"/>
               <series values-cell-range-address="local-table.$C$2:.$C$4" label-cell-address="local-table.$C$1"/>"#,
        );
        let chart = read_chart(xml.as_bytes(), [10.0, 20.0, 30.0, 40.0]).expect("a line chart");

        assert_eq!(chart.title.as_deref(), Some("line by region"));
        assert_eq!(chart.kind, ChartKind::Line);
        assert_eq!(
            chart.categories,
            ["North America", "Europe", "Asia Pacific"]
        );
        assert_eq!(chart.series.len(), 2);
        assert_eq!(chart.series[0].name, "Q2");
        assert_eq!(
            chart.series[0].values,
            [Some(120.0), Some(98.0), Some(74.0)]
        );
        assert_eq!(chart.series[1].name, "Q3");
        assert_eq!(chart.series[1].values, [Some(145.0), Some(121.0), None]);
        assert_eq!(chart.bbox, [10.0, 20.0, 30.0, 40.0]);
    }

    #[test]
    fn pie_and_circle_charts_read_categories_without_an_axis() {
        for class in ["pie", "circle"] {
            let xml = fixture(
                class,
                DIRECT_CATEGORIES,
                r#"<series values-cell-range-address="local-table.$B$2:.$B$4" label-cell-address="local-table.$B$1"/>"#,
            );
            let chart = read_chart(xml.as_bytes(), [0.0; 4]).expect("a pie chart");

            let expected_title = format!("{class} by region");
            assert_eq!(chart.title.as_deref(), Some(expected_title.as_str()));
            assert_eq!(chart.kind, ChartKind::Pie, "{class} is a pie");
            assert_eq!(
                chart.categories,
                ["North America", "Europe", "Asia Pacific"]
            );
            assert_eq!(chart.series.len(), 1);
            assert_eq!(chart.series[0].name, "Q2");
            assert_eq!(
                chart.series[0].values,
                [Some(120.0), Some(98.0), Some(74.0)]
            );
        }
    }

    #[test]
    fn a_scatter_chart_reads_its_x_values_from_the_domain() {
        let xml = fixture(
            "scatter",
            "",
            r#"<series class="chart:scatter" values-cell-range-address="local-table.$B$2:.$B$4" label-cell-address="local-table.$B$1">
                   <domain cell-range-address="local-table.$A$2:.$A$4"/>
               </series>"#,
        );
        let chart = read_chart(xml.as_bytes(), [0.0; 4]).expect("a scatter chart");

        assert_eq!(chart.kind, ChartKind::Scatter);
        assert_eq!(
            chart.categories,
            ["North America", "Europe", "Asia Pacific"]
        );
        assert_eq!(chart.series.len(), 1);
        assert_eq!(chart.series[0].name, "Q2");
        assert_eq!(
            chart.series[0].values,
            [Some(120.0), Some(98.0), Some(74.0)]
        );
    }

    #[test]
    fn a_chart_without_a_local_table_is_not_read() {
        let xml = r#"<document-content><body><chart>
            <chart class="chart:bar">
                <title><text:p>No cached data</text:p></title>
                <plot-area>
                    <axis dimension="x"><categories cell-range-address="data.$A$2:.$A$3"/></axis>
                    <series values-cell-range-address="data.$B$2:.$B$3" label-cell-address="data.$B$1"/>
                </plot-area>
            </chart>
            <table name="data">
                <table-row><table-cell/><table-cell><p>Q2</p></table-cell></table-row>
                <table-row><table-cell><p>North America</p></table-cell><table-cell><p>120</p></table-cell></table-row>
            </table>
        </chart></body></document-content>"#;

        assert!(
            read_chart(xml.as_bytes(), [0.0; 4]).is_none(),
            "a chart with no local-table was read from another table"
        );
    }
}
