//! Assemble a [`DoclingDocument`] from extracted content.

use crate::{
    is_representable_media_type, BoundingBox, CoordOrigin, DoclingDocument, Group, ImageRef,
    Origin, Page, PictureItem, Provenance, Ref, Size, TableCell, TableData, TableItem, TextItem,
};
use std::collections::HashMap;

/// How a text block should be described in the document.
///
/// `marker` is the bullet or number the page printed and it has already been
/// removed from the text; `enumerated` separates a numbered item from a bulleted
/// one, because the JSON consumer needs both facts.
#[derive(Debug, Clone, Default)]
pub struct TextLabel {
    pub label: String,
    /// Heading depth, when a numbering made it discoverable.
    pub level: Option<i32>,
    pub marker: Option<String>,
    pub enumerated: Option<bool>,
}

/// Put the structure a Docling text item class actually has on the item.
///
/// Each class allows its own fields and rejects anything else, so copying the
/// same three options onto every item made documents Docling refuses to load
/// (`extra_forbidden`): a section header has `level` but no `marker`, a list
/// item has `marker` and `enumerated` but no `level`, and a title has neither.
///
/// A heading's number is not thrown away to satisfy that. Docling keeps the
/// number inside the heading text (`"3.1 Encoder and Decoder Stacks"`), so it
/// is folded back in — dropping it would lose content to win a schema argument.
fn apply_structure(
    item: &mut TextItem,
    level: Option<i32>,
    marker: Option<String>,
    enumerated: Option<bool>,
) {
    match item.label.as_str() {
        "section_header" => {
            item.level = level;
            if let Some(marker) = marker {
                let numbered = if item.text.is_empty() {
                    marker
                } else {
                    format!("{marker} {}", item.text)
                };
                item.text = numbered.clone();
                item.orig = numbered;
            }
        }
        "list_item" => {
            item.marker = marker;
            item.enumerated = enumerated;
        }
        _ => {}
    }
}

/// The content layer an item added through a given path belongs to.
const BODY: &str = "body";
const FURNITURE: &str = "furniture";

/// The Docling format revision this exporter writes.
///
/// It is the default of the `DoclingDocument.version` field in the schema the
/// golden tests validate against, so the two cannot drift apart unnoticed.
pub const DOCLING_VERSION: &str = "1.10.0";

/// Where an encoded picture went and what it is.
///
/// Size and DPI are optional because they are genuinely unknown for some
/// formats (a vector EMF has no pixel grid), and reporting a made-up `0 x 0`
/// would be worse than reporting nothing.
#[derive(Debug, Clone)]
pub struct PictureData {
    pub mimetype: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Effective resolution, derived from the placement box when known. An
    /// integer because Docling declares it one.
    pub dpi: Option<u32>,
    /// Path to the extracted image, relative to the JSON file.
    pub uri: String,
}

pub struct DocumentBuilder {
    doc: DoclingDocument,
}

impl DocumentBuilder {
    pub fn new(name: &str) -> Self {
        let body = Group {
            self_ref: "#/body".to_string(),
            parent: None,
            children: Vec::new(),
            content_layer: BODY.to_string(),
            name: "_root_".to_string(),
            label: "unspecified".to_string(),
        };
        Self {
            doc: DoclingDocument {
                schema_name: "DoclingDocument".to_string(),
                // The revision of the Docling format this output targets; the
                // golden tests hold it to the schema they check against.
                version: DOCLING_VERSION.to_string(),
                name: name.to_string(),
                origin: None,
                pages: HashMap::new(),
                texts: Vec::new(),
                tables: Vec::new(),
                pictures: Vec::new(),
                key_value_items: Vec::new(),
                groups: Vec::new(),
                body,
                furniture: None,
            },
        }
    }

    pub fn set_origin(&mut self, origin: Origin) -> &mut Self {
        self.doc.origin = Some(origin);
        self
    }

    pub fn add_page(&mut self, page_no: i32, width: f64, height: f64) -> &mut Self {
        self.doc.pages.insert(
            page_no.to_string(),
            Page {
                page_no,
                size: Size { width, height },
                image: None,
            },
        );
        self
    }

    // --- body items -------------------------------------------------------

    /// Add a text block with an explicit `[l, t, r, b]` box (top-left origin).
    pub fn add_text_block(
        &mut self,
        text: &str,
        page_no: i32,
        bbox: [f64; 4],
        label: &str,
    ) -> &mut Self {
        let prov = vec![self.prov(page_no, bbox, text)];
        let reference = self.push_text(text, label, prov, BODY);
        self.doc.body.children.push(reference);
        self
    }

    /// Add a text block that is more than a paragraph: a heading at some depth,
    /// or a list item with the bullet or number it was written with.
    ///
    /// `marker` is removed from `text` already; keeping it here records what the
    /// page actually printed. `enumerated` distinguishes a numbered item from a
    /// bulleted one.
    pub fn add_text_item(
        &mut self,
        text: &str,
        page_no: i32,
        bbox: [f64; 4],
        label: TextLabel,
    ) -> &mut Self {
        let prov = vec![self.prov(page_no, bbox, text)];
        let idx = self.doc.texts.len();
        self.doc.texts.push(TextItem {
            self_ref: format!("#/texts/{idx}"),
            parent: Some(Ref {
                ref_path: "#/body".to_string(),
            }),
            children: Vec::new(),
            content_layer: BODY.to_string(),
            label: label.label,
            orig: text.to_string(),
            text: text.to_string(),
            prov,
            level: None,
            enumerated: None,
            marker: None,
        });
        if let Some(item) = self.doc.texts.get_mut(idx) {
            apply_structure(item, label.level, label.marker, label.enumerated);
        }
        let reference = Ref {
            ref_path: format!("#/texts/{idx}"),
        };
        self.doc.body.children.push(reference);
        self
    }

    /// Add a table found on a page, with the region it occupies.
    pub fn add_table_item(
        &mut self,
        rows: &[Vec<String>],
        page_no: i32,
        bbox: [f64; 4],
    ) -> &mut Self {
        let prov = vec![self.prov(page_no, bbox, "")];
        let reference = self.push_table(rows, prov, BODY);
        self.doc.body.children.push(reference);
        self
    }

    /// Add a text item that carries no page geometry.
    ///
    /// Formats such as DOCX are flow-based: there is no fixed page rectangle to
    /// point at, so claiming one would be a fabrication.
    pub fn add_labeled_text(&mut self, text: &str, label: &str) -> &mut Self {
        let reference = self.push_text(text, label, Vec::new(), BODY);
        self.doc.body.children.push(reference);
        self
    }

    /// Add a flow-based text item that is a heading or a list item.
    ///
    /// DOCX has no page geometry, so this is [`Self::add_labeled_text`] with the
    /// structure a reader sees: how deep, and with which bullet or number.
    pub fn add_labeled_item(
        &mut self,
        text: &str,
        label: &str,
        level: Option<i32>,
        marker: Option<String>,
        enumerated: Option<bool>,
    ) -> &mut Self {
        let index = self.doc.texts.len();
        let reference = self.push_text(text, label, Vec::new(), BODY);
        if let Some(item) = self.doc.texts.get_mut(index) {
            apply_structure(item, level, marker, enumerated);
        }
        self.doc.body.children.push(reference);
        self
    }

    /// Add a paragraph with a default box (used when no geometry is known).
    pub fn add_text(&mut self, text: &str, page_no: i32) -> &mut Self {
        self.add_text_block(text, page_no, [72.0, 72.0, 72.0, 72.0], "paragraph")
    }

    /// Add a page's own furniture: a running head, a footer or a page number,
    /// with the box the page printed it in.
    ///
    /// It stays in the body order — it is where the page put it, and a reader
    /// looking for the page numbers should find them there — but it is marked as
    /// the furniture layer, so a consumer that wants the *document* can leave it
    /// out. That is why the parent is the body while the layer is not: Docling
    /// writes its own page numbers exactly that way, and the two disagreeing the
    /// other way round is what bug 45 was.
    pub fn add_furniture_on_page(
        &mut self,
        text: &str,
        page_no: i32,
        bbox: [f64; 4],
        label: &str,
    ) -> &mut Self {
        let prov = vec![self.prov(page_no, bbox, text)];
        let reference = self.push_text_with_parent(text, label, prov, FURNITURE, "#/body");
        self.doc.body.children.push(reference);
        self
    }

    /// Add a table from rows of cell text, with no page geometry.
    pub fn add_table_plain(&mut self, rows: &[Vec<String>]) -> &mut Self {
        let reference = self.push_table(rows, Vec::new(), BODY);
        self.doc.body.children.push(reference);
        self
    }

    /// Add a picture with an explicit `[l, t, r, b]` box (top-left origin).
    pub fn add_picture(
        &mut self,
        page_no: i32,
        bbox: [f64; 4],
        label: &str,
        image: Option<PictureData>,
    ) -> &mut Self {
        let prov = vec![self.prov(page_no, bbox, "")];
        let reference = self.push_picture(label, prov, image, BODY);
        self.doc.body.children.push(reference);
        self
    }

    /// Add a picture that carries no page geometry.
    pub fn add_picture_plain(&mut self, label: &str, image: Option<PictureData>) -> &mut Self {
        let reference = self.push_picture(label, Vec::new(), image, BODY);
        self.doc.body.children.push(reference);
        self
    }

    // --- furniture --------------------------------------------------------

    /// Attach header/footer items as the document's furniture group.
    pub fn set_furniture(&mut self, children: Vec<Ref>) -> &mut Self {
        self.doc.furniture = Some(Group {
            self_ref: "#/furniture".to_string(),
            parent: None,
            children,
            content_layer: FURNITURE.to_string(),
            name: "_root_".to_string(),
            label: "unspecified".to_string(),
        });
        self
    }

    pub fn add_furniture_text(&mut self, text: &str, label: &str) -> Ref {
        self.push_text(text, label, Vec::new(), FURNITURE)
    }

    /// Header/footer text that carries list structure, kept as furniture.
    pub fn add_furniture_item(
        &mut self,
        text: &str,
        label: &str,
        level: Option<i32>,
        marker: Option<String>,
    ) -> Ref {
        let index = self.doc.texts.len();
        let reference = self.push_text(text, label, Vec::new(), FURNITURE);
        if let Some(item) = self.doc.texts.get_mut(index) {
            apply_structure(item, level, marker, None);
        }
        reference
    }

    pub fn add_furniture_table(&mut self, rows: &[Vec<String>]) -> Ref {
        self.push_table(rows, Vec::new(), FURNITURE)
    }

    pub fn add_furniture_picture(&mut self, label: &str, image: Option<PictureData>) -> Ref {
        self.push_picture(label, Vec::new(), image, FURNITURE)
    }

    // --- primitives -------------------------------------------------------

    /// A provenance record for `text` in `bbox` (top-left origin).
    ///
    /// A table or a picture has no character range to point at, so its span is
    /// empty rather than absent: Docling requires the field.
    fn prov(&self, page_no: i32, bbox: [f64; 4], text: &str) -> Provenance {
        Provenance {
            page_no,
            bbox: BoundingBox {
                l: bbox[0],
                t: bbox[1],
                r: bbox[2],
                b: bbox[3],
                coord_origin: Some(CoordOrigin::Topleft),
            },
            charspan: (0, text.chars().count()),
        }
    }

    fn push_text(&mut self, text: &str, label: &str, prov: Vec<Provenance>, layer: &str) -> Ref {
        let parent = parent_of(layer);
        self.push_text_with_parent(text, label, prov, layer, &parent)
    }

    fn push_text_with_parent(
        &mut self,
        text: &str,
        label: &str,
        prov: Vec<Provenance>,
        layer: &str,
        parent: &str,
    ) -> Ref {
        let idx = self.doc.texts.len();
        self.doc.texts.push(TextItem {
            self_ref: format!("#/texts/{idx}"),
            parent: Some(Ref {
                ref_path: parent.to_string(),
            }),
            children: Vec::new(),
            content_layer: layer.to_string(),
            label: label.to_string(),
            orig: text.to_string(),
            text: text.to_string(),
            prov,
            level: None,
            enumerated: None,
            marker: None,
        });
        Ref {
            ref_path: format!("#/texts/{idx}"),
        }
    }

    fn push_table(&mut self, rows: &[Vec<String>], prov: Vec<Provenance>, layer: &str) -> Ref {
        let num_rows = rows.len();
        let num_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut cells = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, text) in row.iter().enumerate() {
                cells.push(TableCell {
                    text: text.clone(),
                    start_row_offset_idx: r,
                    end_row_offset_idx: r + 1,
                    start_col_offset_idx: c,
                    end_col_offset_idx: c + 1,
                    row_span: 1,
                    col_span: 1,
                    column_header: r == 0,
                    row_header: c == 0 && r > 0,
                    row_section: false,
                    bbox: None,
                });
            }
        }
        let idx = self.doc.tables.len();
        self.doc.tables.push(TableItem {
            self_ref: format!("#/tables/{idx}"),
            parent: Some(Ref {
                ref_path: parent_of(layer),
            }),
            children: Vec::new(),
            content_layer: layer.to_string(),
            label: "table".to_string(),
            prov,
            data: TableData {
                num_rows,
                num_cols,
                orientation: None,
                table_cells: cells,
            },
        });
        Ref {
            ref_path: format!("#/tables/{idx}"),
        }
    }

    fn push_picture(
        &mut self,
        label: &str,
        prov: Vec<Provenance>,
        image: Option<PictureData>,
        layer: &str,
    ) -> Ref {
        let idx = self.doc.pictures.len();
        self.doc.pictures.push(PictureItem {
            self_ref: format!("#/pictures/{idx}"),
            parent: Some(Ref {
                ref_path: parent_of(layer),
            }),
            children: Vec::new(),
            content_layer: layer.to_string(),
            label: label.to_string(),
            prov,
            image: image.and_then(image_ref),
        });
        Ref {
            ref_path: format!("#/pictures/{idx}"),
        }
    }

    pub fn build(self) -> DoclingDocument {
        self.doc
    }
}

/// The group an item in `layer` belongs to.
///
/// The two have to agree: Docling rejects a document whose furniture lists a
/// child that claims the body as its parent. Ours did exactly that, because the
/// parent was written once for everything (bug 45).
fn parent_of(layer: &str) -> String {
    match layer {
        FURNITURE => "#/furniture".to_string(),
        _ => "#/body".to_string(),
    }
}

/// An image reference, when Docling can hold one.
///
/// All four fields are required, so a picture whose media type it cannot name,
/// or whose pixel size or resolution is unknown (a vector drawing has neither),
/// is left without one rather than with a partial, invalid record.
fn image_ref(data: PictureData) -> Option<ImageRef> {
    let (width, height) = match (data.width, data.height) {
        (Some(width), Some(height)) => (width, height),
        _ => return None,
    };
    if !is_representable_media_type(&data.mimetype) || data.uri.is_empty() {
        return None;
    }
    Some(ImageRef {
        mimetype: Some(data.mimetype),
        dpi: data.dpi,
        size: Some(Size {
            width: width as f64,
            height: height as f64,
        }),
        uri: Some(data.uri),
    })
}
