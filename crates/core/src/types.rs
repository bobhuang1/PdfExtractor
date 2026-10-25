use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level DoclingDocument structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DoclingDocument {
    pub schema_name: String,
    pub version: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    #[serde(default)]
    pub pages: HashMap<String, Page>,
    #[serde(default)]
    pub texts: Vec<TextItem>,
    #[serde(default)]
    pub tables: Vec<TableItem>,
    #[serde(default)]
    pub pictures: Vec<PictureItem>,
    #[serde(default)]
    pub key_value_items: Vec<KeyValueItem>,
    #[serde(default)]
    pub groups: Vec<Group>,
    pub body: Body,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub furniture: Option<Group>,
    /// What an HTML page's head said about the page itself, when it was read
    /// from HTML: its description, the URL it says it lives at, and the Open
    /// Graph and Twitter card statements a link preview is built from.
    ///
    /// Kept beside the model rather than in it, the way [`TextItem::quote`] is:
    /// Docling has nowhere to put either fact and rejects a field it does not
    /// know (`extra_forbidden`), so the field is skipped when a document is
    /// written. A page read as HTML and written as HTML keeps both in its head,
    /// Markdown writes them as YAML front matter, and either reads to a Docling
    /// consumer as the document Docling itself would make.
    #[serde(skip)]
    pub html_head: HtmlHead,
}

/// What an HTML page's head says about the page, rather than on it.
///
/// `<meta name="description">` is the summary a search engine shows, and
/// `<link rel="canonical">` is the URL the page says it lives at. Neither is
/// rendered, so neither is an item, and Docling's schema has no field for
/// either — see [`DoclingDocument::html_head`] for where they wait instead.
///
/// The Open Graph and Twitter card statements are here for the same reason: a
/// social embed reads them to build a link preview, a browser renders none of
/// them, and Docling has nowhere to keep them. They are carried so a page read
/// as HTML and written as HTML is the page it was, head and all; a Markdown
/// rendering states the same facts as YAML front matter and reads them back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HtmlHead {
    pub description: Option<String>,
    pub canonical: Option<String>,
    /// The social cards' `<meta>` statements, as the key the page wrote and the
    /// content it gave, in the order the head stated them: `og:title`, then
    /// `og:image`, then the `og:image:width` that belongs to that image.
    ///
    /// One list holds both vocabularies because they are one kind of statement
    /// and each key names its own — `og:` for Open Graph, `twitter:` for the
    /// card Twitter reads — and the order is part of what a card says, since a
    /// width can only belong to the image before it.
    pub social: Vec<(String, String)>,
}

/// Document origin metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Origin {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mimetype: Option<String>,
    /// Docling declares this a non-negative integer, so it is an unsigned hash
    /// rather than a signed one that can come out negative (bug 44).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_hash: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// Page information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Page {
    pub page_no: i32,
    pub size: Size,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageRef>,
}

/// Page size
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// Image reference
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ImageRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mimetype: Option<String>,
    /// An integer in Docling, so a computed resolution is rounded rather than
    /// written as a fraction (bug 44).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<Size>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// Bounding box
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BoundingBox {
    pub l: f64,
    pub t: f64,
    pub r: f64,
    pub b: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coord_origin: Option<CoordOrigin>,
}

/// The media types Docling can name in an `ImageRef`.
///
/// Docling validates the field against its own registry, and the registry has no
/// entry for some of the formats we can extract — `image/wmf`, `image/emf` and
/// `image/jp2` are all rejected, which invalidates the whole document, not just
/// the picture. Those formats are therefore reported as a picture with a box and
/// no `image`: a drawing that cannot be named is better than a document that
/// cannot be read. (Real Docling never meets this case because it rasterises
/// such drawings to PNG first, which is what `crates/metafile` now does for a
/// DOCX drawing — see `progress.md`, bug 50.)
///
/// A GIF, a BMP and a TIFF are accepted here, but no office reader hands one on:
/// Docling's own Word and PowerPoint readers open every picture with Pillow and
/// write it back as PNG, so the CLI normalises an office picture the same way —
/// `office_media` in `crates/cli` — and these three are met only in a flow page,
/// where the bytes travel as the file stores them.
const REPRESENTABLE_MEDIA_TYPES: [&str; 6] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/bmp",
    "image/tiff",
    "image/svg+xml",
];

/// Whether `mimetype` can be written in an image reference (see
/// [`REPRESENTABLE_MEDIA_TYPES`]).
pub fn is_representable_media_type(mimetype: &str) -> bool {
    REPRESENTABLE_MEDIA_TYPES.contains(&mimetype)
}

/// Which corner a box's coordinates are measured from.
///
/// The spellings are Docling's own (`CoordOrigin.TOPLEFT`); writing them in
/// lower case produced documents Docling refuses to load (bug 44).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoordOrigin {
    #[serde(rename = "TOPLEFT")]
    Topleft,
    #[serde(rename = "BOTTOMLEFT")]
    Bottomleft,
}

/// Provenance record
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Provenance {
    pub page_no: i32,
    pub bbox: BoundingBox,
    /// Docling has this on every provenance record, not only on text: a table or
    /// a picture carries `[0, 0]` because it has no character range (bug 44).
    pub charspan: (usize, usize),
}

/// Reference to another node/item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ref {
    #[serde(rename = "$ref")]
    pub ref_path: String,
}

/// Base content item
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BaseItem {
    pub self_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Ref>,
    #[serde(default)]
    pub children: Vec<Ref>,
}

/// Group/node (body, furniture)
///
/// `name` and `label` are Docling's own defaults for a root group; writing them
/// means our root groups are byte-identical to the ones Docling itself emits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Group {
    pub self_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Ref>,
    #[serde(default)]
    pub children: Vec<Ref>,
    pub content_layer: String,
    pub name: String,
    pub label: String,
}

/// Text item
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TextItem {
    pub self_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Ref>,
    #[serde(default)]
    pub children: Vec<Ref>,
    /// `body` or `furniture`: a page footer is not body content, and leaving the
    /// field out would claim it is (the Docling default).
    pub content_layer: String,
    pub label: String,
    pub orig: String,
    pub text: String,
    #[serde(default)]
    pub prov: Vec<Provenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enumerated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<String>,
    /// Where this text points, when it points anywhere: a PDF `/Link`
    /// annotation over it, a DOCX `w:hyperlink` around it, or an HTML
    /// `a` with an `href`. Left out entirely when there is no link, which
    /// is how Docling writes it — the field never appears for text that
    /// goes nowhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hyperlink: Option<String>,
    /// Whether the document set these words apart as a quotation.
    ///
    /// Docling has no block quote: its own readers flatten a `<blockquote>` or
    /// a `>` line into a paragraph, and no label in its schema says otherwise.
    /// The fact is therefore kept here, beside the model rather than in it —
    /// the field is skipped when the document is written, because Docling
    /// rejects a field it does not know (`extra_forbidden`) — so a quotation
    /// survives a Markdown or an HTML round trip as the quotation it was, and
    /// reads to a Docling consumer as the paragraph Docling itself would make.
    #[serde(skip)]
    pub quote: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TableItem {
    pub self_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Ref>,
    #[serde(default)]
    pub children: Vec<Ref>,
    pub content_layer: String,
    pub label: String,
    #[serde(default)]
    pub prov: Vec<Provenance>,
    pub data: TableData,
}

/// Table data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TableData {
    pub num_rows: usize,
    pub num_cols: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    pub table_cells: Vec<TableCell>,
}

/// Table cell
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TableCell {
    pub text: String,
    pub start_row_offset_idx: usize,
    pub end_row_offset_idx: usize,
    pub start_col_offset_idx: usize,
    pub end_col_offset_idx: usize,
    pub row_span: i32,
    pub col_span: i32,
    #[serde(default)]
    pub column_header: bool,
    #[serde(default)]
    pub row_header: bool,
    #[serde(default)]
    pub row_section: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BoundingBox>,
    /// The item this cell holds when it holds more than words.
    ///
    /// Docling's `RichTableCell` is a cell that references a document item — a
    /// group of pictures, a paragraph — instead of only carrying text, and the
    /// field is how it says so. A Word cell with a picture in it is such a
    /// cell: the picture is the cell's, held in a group under the table, rather
    /// than an item of the body standing beside the table. A cell without one
    /// writes no `ref` and reads back as the plain `TableCell` it is.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<Ref>,
}

/// Picture item
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PictureItem {
    pub self_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Ref>,
    #[serde(default)]
    pub children: Vec<Ref>,
    pub content_layer: String,
    /// What the picture *is*, when the source said more than "a picture": a
    /// chart's type, and the numbers it plots. A photograph or a drawing has
    /// nothing to add, and says so by leaving the field out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<PictureMeta>,
    pub label: String,
    #[serde(default)]
    pub prov: Vec<Provenance>,
    /// The text items that caption this picture. Only a source that states the
    /// link can fill it — a chart's title is its own caption — so a picture
    /// with no stated caption leaves the field out rather than claiming none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captions: Vec<Ref>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageRef>,
}

/// Docling's `PictureMeta`: the extras a picture can carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PictureMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<PictureClassificationMetaField>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tabular_chart: Option<TabularChartMetaField>,
}

/// A picture's classification: what kind of picture a reader thinks it is.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PictureClassificationMetaField {
    /// Docling requires at least one prediction, so a picture that is
    /// classified at all names its class.
    pub predictions: Vec<PictureClassificationPrediction>,
}

/// One classification, by Docling's own class names — `bar_chart`,
/// `line_chart`, `pie_chart`, `scatter_chart`, `other_chart`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PictureClassificationPrediction {
    pub class_name: String,
}

/// A chart's plotted numbers, held as a table beside the picture rather than in
/// the document's table list: the chart is one picture, and the table is what
/// the picture says.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TabularChartMetaField {
    pub chart_data: TableData,
}

/// Key-value item
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct KeyValueItem {
    pub self_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Ref>,
    #[serde(default)]
    pub children: Vec<Ref>,
    pub content_layer: String,
    pub label: String,
    #[serde(default)]
    pub prov: Vec<Provenance>,
}

/// Body (root group)
pub type Body = Group;
