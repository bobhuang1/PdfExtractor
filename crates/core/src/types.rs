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
}

/// Table item
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
    pub label: String,
    #[serde(default)]
    pub prov: Vec<Provenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageRef>,
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
