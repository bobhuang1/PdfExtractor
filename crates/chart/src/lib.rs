//! Reading a chart part into the data it plots.
//!
//! A chart in an office package is not a picture file: it is a drawing of data,
//! held as a chart part whose caches carry the categories and the plotted
//! numbers. Those numbers are what a reader can use, so they are read out here
//! and the frame that shows them is placed by the format that anchored it — a
//! slide's `p:graphicFrame`, a worksheet's `xdr:graphicFrame`.
//!
//! The same part is written by PowerPoint and by Excel: DrawingML's
//! `c:chartSpace` holds one `c:chart`, whose `c:plotArea` holds the plots and
//! the axes. Reading is shared rather than written twice because the part is
//! shared — a deck and a workbook that plot the same numbers state them the same
//! way, and a reading that differs between the two would be a reading of the
//! container, not of the chart.
//!
//! What is read is Docling's own reading of a chart: the family of its first
//! plot, its title, the categories its first series plots against, and every
//! series' legend name and values, in the order the chart draws them. What is
//! not read is how the chart looks — colours, a legend's position, an axis
//! scale — because the picture a chart becomes is its data, not its paint.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// A chart part, ready to be placed by whichever format anchored it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartShape {
    /// Where the frame holding the chart sits, in the anchoring format's own
    /// units. A chart read on its own is unplaced, and the caller fills this
    /// in.
    pub bbox: [f64; 4],
    /// The chart's own title, trimmed, when it has one.
    pub title: Option<String>,
    pub kind: ChartKind,
    /// The category labels the chart plots against, one per data row. A
    /// category the cache declares but carries no label for is an empty string,
    /// because the row it belongs to still exists.
    pub categories: Vec<String>,
    /// The plotted series, in the order the chart draws them.
    pub series: Vec<ChartSeries>,
}

/// The family a chart belongs to, which is all Docling's classification names.
/// It is read from the first plot, the way python-pptx reports a chart's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    Bar,
    Line,
    Pie,
    Scatter,
    Other,
}

impl ChartKind {
    /// The class name Docling gives this family.
    pub fn class_name(self) -> &'static str {
        match self {
            ChartKind::Bar => "bar_chart",
            ChartKind::Line => "line_chart",
            ChartKind::Pie => "pie_chart",
            ChartKind::Scatter => "scatter_chart",
            ChartKind::Other => "other_chart",
        }
    }
}

/// One plotted series: its legend name and its values, one per category.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartSeries {
    /// The name the chart gives the series; empty when it gives none.
    pub name: String,
    /// The values, in category order. A point the chart leaves out is `None` —
    /// the gap the chart draws — and is not the same as a plotted zero.
    pub values: Vec<Option<f64>>,
}

/// A chart value as a cell of the table it is reconstructed into.
///
/// This is Docling's own `_cell_text`: an integral value loses its `.0` so the
/// table reads like the data it came from (`145`, not `145.0`), and a gap is an
/// empty cell rather than the word for one.
pub fn chart_cell_text(value: Option<f64>) -> String {
    match value {
        None => String::new(),
        // 2^53 is the largest magnitude at which every integer is exactly
        // representable, so above it the conversion to `i64` would invent
        // digits; such a value is written as the float it is.
        Some(number) if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 => {
            format!("{}", number as i64)
        }
        Some(number) => format!("{number}"),
    }
}

/// Read a chart part into its title, family, categories and series, taking the
/// numbers from the part's own caches.
///
/// `None` is a chart nothing can be read from — a malformed part, or one with
/// no plot at all. Nothing is drawn in either case, so there is nothing to
/// report where the chart stood.
pub fn read_chart(bytes: &[u8]) -> Option<ChartShape> {
    read_chart_with(bytes, &|_| Vec::new())
}

/// The same reading, for a chart part that carries its data as references
/// rather than as caches.
///
/// Excel and PowerPoint write a cache beside every reference — `c:numCache`
/// holding the numbers as of the last save — and a reader can use those. A
/// scripted writer often leaves the reference alone (`'Quarterly'!$B$2:$B$4`):
/// the numbers are in the cells and nowhere else, so the caller resolves the
/// reference against the workbook that holds them. `resolve` answers the cells a
/// reference names, in the order a spreadsheet fills them — across a row, down a
/// column — with an empty string for a cell that has no value. A cache, when
/// there is one, is still what is read: it is what the chart was drawn from.
///
/// A chart nothing can be read from is `None` either way.
pub fn read_chart_with(bytes: &[u8], resolve: &dyn Fn(&str) -> Vec<String>) -> Option<ChartShape> {
    let root = Node::parse(bytes)?;
    let chart = root.find("chart")?;
    let plot_area = chart.child("plotArea")?;
    // A plot area holds the plots and the axes; every plot element's name ends
    // in `Chart`, and no axis' does.
    let plots: Vec<&Node> = plot_area
        .children
        .iter()
        .filter(|node| node.name.ends_with("Chart"))
        .collect();
    if plots.is_empty() {
        return None;
    }
    Some(ChartShape {
        // Placed by the caller, which knows the frame's box.
        bbox: [0.0; 4],
        title: chart_title(chart),
        kind: chart_kind(plots[0]),
        categories: chart_categories(plots[0], resolve),
        series: plots
            .iter()
            .flat_map(|plot| ordered_series(plot, resolve))
            .collect(),
    })
}

/// The chart's own title, trimmed, when it has one.
fn chart_title(chart: &Node) -> Option<String> {
    let rich = chart.child("title")?.child("tx")?.child("rich")?;
    let text = text_frame_text(rich).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// A rich text body as python-pptx reads it: every paragraph's text, joined by
/// a newline.
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

/// The family a chart belongs to, from its first plot.
///
/// Docling classifies the type python-pptx reports for the first plot, and
/// python-pptx has no type of its own for a chart whose first plot is an area,
/// radar, bubble, stock or surface plot — so those are the other charts, along
/// with every family this reader does not know.
fn chart_kind(plot: &Node) -> ChartKind {
    match plot.name.as_str() {
        // A bar chart's `barDir` chooses between the horizontal and the
        // vertical variants — BAR and COLUMN — and both are bar charts.
        "barChart" | "bar3DChart" => ChartKind::Bar,
        "lineChart" | "line3DChart" => ChartKind::Line,
        "pieChart" | "pie3DChart" | "doughnutChart" => ChartKind::Pie,
        "scatterChart" => ChartKind::Scatter,
        _ => ChartKind::Other,
    }
}

/// The categories a chart plots against, read the way python-pptx reads them:
/// the rows are the cache's declared point count, the labels come from the first
/// series' category cache (the first level of a multi-level one), and a category
/// the cache declares without a label is an empty string rather than a missing
/// row.
///
/// One deliberate divergence: python-pptx hands back a Python `None` for a
/// category whose `c:v` is present but empty, which Docling then writes into the
/// table as the four letters `None`. A language's null sentinel is not a
/// category name, so an empty label here stays empty.
fn chart_categories(plot: &Node, resolve: &dyn Fn(&str) -> Vec<String>) -> Vec<String> {
    let Some(cat) = plot.child("ser").and_then(|ser| ser.child("cat")) else {
        return Vec::new();
    };
    let count = cat
        .find("ptCount")
        .and_then(|count| count.attr("val"))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut points = Vec::new();
    if let Some(level) = cat.find("lvl") {
        points.extend(level.children_named("pt"));
    }
    if points.is_empty() {
        descendants(cat, "pt", &mut points);
    }
    let mut labels = vec![String::new(); count];
    for point in points {
        let Some(index) = point.attr("idx").and_then(|idx| idx.parse::<usize>().ok()) else {
            continue;
        };
        if let Some(label) = labels.get_mut(index) {
            *label = point
                .child("v")
                .map(|value| value.text.clone())
                .unwrap_or_default();
        }
    }
    // No cache: the categories are whatever the reference names, read from the
    // cells themselves.
    if labels.is_empty() {
        if let Some(reference) = cat.find("f") {
            return resolve(reference.text.trim());
        }
    }
    labels
}

/// One plot's series, in the order the plot draws them: `c:order`, which is not
/// necessarily the order the elements appear in.
fn ordered_series(plot: &Node, resolve: &dyn Fn(&str) -> Vec<String>) -> Vec<ChartSeries> {
    let mut series: Vec<(usize, usize, ChartSeries)> = plot
        .children_named("ser")
        .enumerate()
        .map(|(position, ser)| {
            let order = ser
                .child("order")
                .and_then(|order| order.attr("val"))
                .and_then(|order| order.parse().ok())
                .unwrap_or(position);
            (order, position, read_series(ser, resolve))
        })
        .collect();
    series.sort_by_key(|(order, position, _)| (*order, *position));
    series.into_iter().map(|(_, _, series)| series).collect()
}

/// One series: what the chart calls it, and the numbers it plots.
fn read_series(ser: &Node, resolve: &dyn Fn(&str) -> Vec<String>) -> ChartSeries {
    // python-pptx takes a series' name from the first cached point under
    // `c:tx`; a name written as a bare `c:v` is not read, and neither is it
    // here, so both readings name a series the same way.
    let tx = ser.child("tx");
    let mut name = tx
        .and_then(|tx| tx.find("pt"))
        .and_then(|point| point.child("v"))
        .map(|value| value.text.clone())
        .unwrap_or_default();
    // A name given only as a reference is the cell the name is in.
    if name.is_empty() {
        if let Some(reference) = tx.and_then(|tx| tx.find("f")) {
            name = resolve(reference.text.trim())
                .into_iter()
                .find(|cell| !cell.is_empty())
                .unwrap_or_default();
        }
    }
    // A category series' numbers are `c:val`; an XY or bubble series' are
    // `c:yVal`, which is the same shape under a different name — python-pptx
    // reads the plotted value from one or the other the same way.
    let values = ser
        .child("val")
        .or_else(|| ser.child("yVal"))
        .map(|val| chart_values(val, resolve))
        .unwrap_or_default();
    ChartSeries { name, values }
}

/// A series' values in category order: the declared point count says how many
/// rows there are, and a row with no point is the gap the chart draws — which is
/// not the same as a plotted zero.
fn chart_values(val: &Node, resolve: &dyn Fn(&str) -> Vec<String>) -> Vec<Option<f64>> {
    let count = val
        .find("ptCount")
        .and_then(|count| count.attr("val"))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut points = Vec::new();
    descendants(val, "pt", &mut points);
    // No cache: the numbers are the cells the reference names. A cell that is
    // not a number — an empty one, a label in the range — is the gap it is.
    if count == 0 && points.is_empty() {
        if let Some(reference) = val.find("f") {
            return resolve(reference.text.trim())
                .iter()
                .map(|text| text.trim().parse::<f64>().ok())
                .collect();
        }
    }
    let mut values = vec![None; count];
    for point in points {
        let Some(index) = point.attr("idx").and_then(|idx| idx.parse::<usize>().ok()) else {
            continue;
        };
        let Some(slot) = values.get_mut(index) else {
            continue;
        };
        *slot = point
            .child("v")
            .map(|value| value.text.trim())
            .and_then(|text| text.parse::<f64>().ok());
    }
    values
}

/// Every descendant with this local name, in document order.
fn descendants<'a>(node: &'a Node, name: &str, out: &mut Vec<&'a Node>) {
    for child in &node.children {
        if child.name == name {
            out.push(child);
        }
        descendants(child, name, out);
    }
}

// --- XML ------------------------------------------------------------------

/// One element of a chart part, with its attributes and children.
///
/// Only what a chart needs is kept: local names (a chart part's prefixes are
/// `c:` and `a:`, and the reader never has to tell them apart), every
/// attribute under its local name, the children in document order, and the text
/// an element holds.
#[derive(Debug, Default)]
struct Node {
    /// Local name: `chart`, `plotArea`, `ser`.
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    /// Read a part into its tree; `None` is a part that does not parse.
    fn parse(xml: &[u8]) -> Option<Self> {
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
                // carry nothing a chart's data needs.
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        root
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

    /// An attribute by its local name: `val`, `idx`.
    fn attr(&self, local: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == local)
            .map(|(_, value)| value.as_str())
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
        let key = local_name(attribute.key.as_ref());
        let value = attribute
            .unescape_value()
            .map(|value| value.into_owned())
            .unwrap_or_default();
        attributes.push((key, value));
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
