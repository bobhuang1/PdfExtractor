//! Assemble a [`DoclingDocument`] from extracted content.

use crate::{
    is_representable_media_type, BoundingBox, CoordOrigin, DoclingDocument, Group, HtmlHead,
    ImageRef, Origin, Page, PictureClassificationMetaField, PictureClassificationPrediction,
    PictureItem, PictureMeta, Provenance, Ref, Size, TableCell, TableData, TableItem,
    TabularChartMetaField, TextItem,
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
    /// Where the item points, when its source said so: a PDF `/Link`
    /// annotation, a DOCX `w:hyperlink`, an HTML `a[href]`.
    pub hyperlink: Option<String>,
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
/// A slide's speaker notes and its comments: what the deck says about itself
/// rather than what it shows. Docling keeps them out of the default export but
/// in the document, which is what this layer is for.
const NOTES: &str = "notes";

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

/// A table laid out on an explicit grid, with its cells placed and spanned.
///
/// [`DocumentBuilder::add_table_plain`] takes rows of cell text, which is
/// every table a rectangular reading produces. A table whose markup declares
/// merges cannot be expressed that way: a cell that spans two columns sits at
/// one grid position while covering two, and the cells a merge covers are not
/// cells of the document at all. That is what DOCX `w:gridSpan` and `w:vMerge`
/// describe, and the shape the `DoclingDocument` table model holds.
#[derive(Debug, Clone)]
pub struct GridTable {
    /// The grid's height in rows and width in columns — the sizes the cells
    /// are placed against, which a trailing empty row or column can make
    /// larger than the cells themselves imply.
    pub num_rows: usize,
    pub num_cols: usize,
    /// The cells that *are* the table: a cell a merge covers is left out, its
    /// slots held by the spanning cell's row and column spans.
    pub cells: Vec<GridCell>,
}

/// One cell of a [`GridTable`]: its text, where it starts, and how far it
/// merges across the grid.
#[derive(Debug, Clone)]
pub struct GridCell {
    pub text: String,
    /// Grid row and column of the cell's top-left slot (0-based).
    pub start_row: usize,
    pub start_col: usize,
    /// Rows and columns the cell covers, at least 1 each — more than one is a
    /// merged cell.
    pub row_span: usize,
    pub col_span: usize,
    /// Whether the document marks this row a header: the cell renders as `<th>`
    /// in HTML and opens the pipe table's header row in Markdown.
    pub column_header: bool,
    /// Whether the cell labels its own row. What "labels" means is the
    /// source's business: an XLSX cell never does (a worksheet has no row
    /// headers to speak of), a grid read from a printed table gives the first
    /// column that meaning. The builder writes this through rather than
    /// deciding it, because the two sources disagree and both are right about
    /// themselves.
    pub row_header: bool,
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
                html_head: HtmlHead::default(),
            },
        }
    }

    pub fn set_origin(&mut self, origin: Origin) -> &mut Self {
        self.doc.origin = Some(origin);
        self
    }

    /// Keep what an HTML page's head said about the page — its description, the
    /// URL it calls canonical, and the Open Graph and Twitter card statements
    /// behind its link preview — beside the model, where the HTML writer finds
    /// them. Docling has no field for any of it, so none of it reaches the JSON;
    /// see [`DoclingDocument::html_head`].
    pub fn set_html_head(&mut self, head: HtmlHead) -> &mut Self {
        self.doc.html_head = head;
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

    /// Make an extracted picture the page's own image.
    ///
    /// This is the image behind a page rather than an item on it: a slide's
    /// photographic background is the one such picture in a deck. The page
    /// image is recorded only when Docling can describe it — a representable
    /// media type, a pixel grid, and a file beside the JSON — because its
    /// `ImageRef` has no partial form; otherwise the page keeps no image rather
    /// than an invalid one.
    pub fn set_page_image(&mut self, page_no: i32, image: PictureData) -> &mut Self {
        if let Some(page) = self.doc.pages.get_mut(&page_no.to_string()) {
            page.image = image_ref(image);
        }
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
            hyperlink: label.hyperlink,
            quote: false,
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

    /// Add a flow-based text item that carries a link.
    ///
    /// The same item as [`Self::add_labeled_item`], plus the URL its source
    /// held: a DOCX `w:hyperlink` around it, an HTML `a[href]` inside it.
    pub fn add_labeled_item_with_link(
        &mut self,
        text: &str,
        label: &str,
        level: Option<i32>,
        marker: Option<String>,
        enumerated: Option<bool>,
        hyperlink: Option<String>,
    ) -> &mut Self {
        let index = self.doc.texts.len();
        self.add_labeled_item(text, label, level, marker, enumerated);
        if let Some(url) = hyperlink {
            if let Some(item) = self.doc.texts.get_mut(index) {
                item.hyperlink = Some(url);
            }
        }
        self
    }

    /// Add a flow-based text item inside a group, on the group's own layer.
    ///
    /// The grouped form of [`Self::add_labeled_item_with_link`]: a Markdown
    /// reader that rebuilt a slide's `chapter` group (or a comment's
    /// `comment_section` group) puts the item back inside that group, rather
    /// than under the body where the grouping would be lost. A flow format has
    /// no page geometry, so the item claims none.
    pub fn add_labeled_item_in(&mut self, text: &str, structure: TextLabel, group: &Ref) -> Ref {
        let layer = self.layer_of(group);
        let reference =
            self.push_text_with_parent(text, &structure.label, Vec::new(), &layer, &group.ref_path);
        if let Some(item) = self.doc.texts.last_mut() {
            apply_structure(
                item,
                structure.level,
                structure.marker,
                structure.enumerated,
            );
            item.hyperlink = structure.hyperlink;
        }
        self.add_to_group(group, &reference);
        reference
    }

    /// Add a flow-based quotation: a paragraph the document set apart from its
    /// own words, with the link it opened with.
    ///
    /// Docling's model has no block quote — its own readers flatten one into a
    /// paragraph — so the item is a `paragraph`, the label Docling itself gives
    /// these words, and the fact that they are quoted lives on
    /// [`TextItem::quote`] where the Markdown and HTML writers find it. Nothing
    /// of it reaches the JSON: a Docling consumer reads the paragraph Docling
    /// would have made.
    pub fn add_quotation(&mut self, text: &str, hyperlink: Option<String>) -> &mut Self {
        let reference = self.push_quotation(text, hyperlink, BODY, "#/body");
        self.doc.body.children.push(reference);
        self
    }

    /// Add a quotation inside a group, on the group's own layer: a quoted
    /// paragraph of a slide, when a Markdown writer marked the slide it belongs
    /// to.
    pub fn add_quotation_in(&mut self, text: &str, hyperlink: Option<String>, group: &Ref) -> Ref {
        let layer = self.layer_of(group);
        let reference = self.push_quotation(text, hyperlink, &layer, &group.ref_path);
        self.add_to_group(group, &reference);
        reference
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

    /// Add a flow-based table inside a group, on the group's own layer.
    ///
    /// The grouped form of [`Self::add_table_plain`]: a slide's table read back
    /// from Markdown belongs to the slide's group, not to the body.
    pub fn add_table_plain_in(&mut self, rows: &[Vec<String>], group: &Ref) -> Ref {
        let layer = self.layer_of(group);
        let reference =
            self.push_grid_table_at(&grid_from_rows(rows), Vec::new(), &layer, &group.ref_path);
        self.add_to_group(group, &reference);
        reference
    }

    /// Add a table whose cells are already placed on the grid, with no page
    /// geometry: the merged-cell reading of [`Self::add_table_plain`], where a
    /// spanning cell covers the slots it merges instead of losing them.
    pub fn add_table_grid(&mut self, table: &GridTable) -> &mut Self {
        let reference = self.push_grid_table(table, Vec::new(), BODY);
        self.doc.body.children.push(reference);
        self
    }

    /// Add a placed table inside a group, on the group's own layer.
    ///
    /// The grouped form of [`Self::add_table_grid`], as
    /// [`Self::add_table_plain_in`] is of [`Self::add_table_plain`]: a slide's
    /// table read back from a flow format belongs to the slide's group, not to
    /// the body.
    pub fn add_table_grid_flow_in(&mut self, table: &GridTable, group: &Ref) -> Ref {
        let layer = self.layer_of(group);
        let reference = self.push_grid_table_at(table, Vec::new(), &layer, &group.ref_path);
        self.add_to_group(group, &reference);
        reference
    }

    /// Add a slide's speaker notes: one item on the notes layer, inside the
    /// slide's own group.
    ///
    /// The layer is stated here rather than taken from the group, because a
    /// hidden slide's group is `invisible` and its notes are still notes:
    /// Docling moves only the slide's body content when a slide is hidden, and
    /// the two readings have to agree about that.
    pub fn add_notes_item_in(
        &mut self,
        text: &str,
        page_no: i32,
        bbox: [f64; 4],
        group: &Ref,
    ) -> Ref {
        let prov = vec![self.prov(page_no, bbox, text)];
        let reference = self.push_text_with_parent(text, "text", prov, NOTES, &group.ref_path);
        self.add_to_group(group, &reference);
        reference
    }

    /// Add a flow-based notes-layer item: a remark a reader recognised by the
    /// writer's own `**Notes:**` label.
    ///
    /// It is listed under the body — Markdown carries no slide to group it with
    /// — but the layer says what it is, so a reader who did not ask for notes
    /// leaves it out again and the remark never becomes body content.
    pub fn add_notes_text(&mut self, text: &str) -> &mut Self {
        self.add_notes_text_with_link(text, None)
    }

    /// Add a flow-based remark that carried a link.
    ///
    /// A note's address belongs to the item, and a reader of Markdown or HTML
    /// recovers it when the writer's mark says the whole first paragraph was a
    /// link. Keeping it here is what makes the next render the same link
    /// instead of its brackets.
    pub fn add_notes_text_with_link(&mut self, text: &str, hyperlink: Option<String>) -> &mut Self {
        let reference = self.push_text(text, "text", Vec::new(), NOTES);
        self.link_last_text(hyperlink);
        self.doc.body.children.push(reference);
        self
    }

    /// Add a flow-based notes-layer item inside a group.
    ///
    /// A deck read back from Markdown keeps its speaker notes inside the
    /// slide's group, where the PPTX reader put them; [`Self::add_notes_text`]
    /// is the ungrouped form a document with no markers uses.
    pub fn add_notes_text_in(&mut self, text: &str, group: &Ref) -> Ref {
        self.add_notes_text_in_with_link(text, None, group)
    }

    /// The grouped form of [`Self::add_notes_text_with_link`]: a linked remark
    /// read back from a grouped Markdown or HTML file.
    pub fn add_notes_text_in_with_link(
        &mut self,
        text: &str,
        hyperlink: Option<String>,
        group: &Ref,
    ) -> Ref {
        let reference =
            self.push_text_with_parent(text, "text", Vec::new(), NOTES, &group.ref_path);
        self.link_last_text(hyperlink);
        self.add_to_group(group, &reference);
        reference
    }

    /// Add a comment's group: `comment_section`, on the notes layer, listed
    /// under the body rather than under the slide the comment is about — which
    /// is exactly where Docling's own PPTX backend puts it.
    pub fn add_comment_group(&mut self, name: &str) -> Ref {
        self.add_group("comment_section", name, NOTES)
    }

    /// Add a comment inside its group: one item on the notes layer, with no
    /// page geometry.
    ///
    /// A comment is about the slide, not a place on it: PowerPoint stores only
    /// a position, nothing the document model can point at, and Docling writes
    /// it with an empty provenance list.
    pub fn add_comment_in(&mut self, text: &str, group: &Ref) -> Ref {
        self.add_comment_in_with_link(text, None, group)
    }

    /// Add a comment that carried a link, inside its group: the same item as
    /// [`Self::add_comment_in`], with the address the remark opened with.
    pub fn add_comment_in_with_link(
        &mut self,
        text: &str,
        hyperlink: Option<String>,
        group: &Ref,
    ) -> Ref {
        let reference =
            self.push_text_with_parent(text, "text", Vec::new(), NOTES, &group.ref_path);
        self.link_last_text(hyperlink);
        self.add_to_group(group, &reference);
        reference
    }

    /// Add a group that holds other items — an XLSX sheet, a PPTX slide.
    ///
    /// The group is listed under the body and names it as its parent; the
    /// items inside it are added with [`Self::add_table_grid_in`], which takes
    /// the group's reference. `content_layer` is the layer those items belong
    /// to as well: Docling keeps a hidden worksheet's content as `invisible`
    /// rather than dropping it, and the group is where that fact is stated.
    pub fn add_group(&mut self, label: &str, name: &str, content_layer: &str) -> Ref {
        let idx = self.doc.groups.len();
        let reference = Ref {
            ref_path: format!("#/groups/{idx}"),
        };
        self.doc.groups.push(Group {
            self_ref: reference.ref_path.clone(),
            parent: Some(Ref {
                ref_path: "#/body".to_string(),
            }),
            children: Vec::new(),
            content_layer: content_layer.to_string(),
            name: name.to_string(),
            label: label.to_string(),
        });
        self.doc.body.children.push(reference.clone());
        reference
    }

    /// Add a grid table on a page, inside an existing group.
    ///
    /// The table takes the group's content layer, so a hidden worksheet's
    /// table and the group that holds it cannot disagree; the group lists the
    /// table as its child, which is what makes the sheet one node of the
    /// document rather than a bare sequence of tables.
    pub fn add_table_grid_in(
        &mut self,
        table: &GridTable,
        page_no: i32,
        bbox: [f64; 4],
        group: &Ref,
    ) -> Ref {
        let layer = self.layer_of(group);
        let prov = vec![self.prov(page_no, bbox, "")];
        let reference = self.push_grid_table_at(table, prov, &layer, &group.ref_path);
        self.add_to_group(group, &reference);
        reference
    }

    /// Add a text item on a page, inside an existing group: a paragraph of a
    /// slide, which knows the box of the shape it was drawn in.
    ///
    /// The item takes the group's content layer (a hidden slide's words are
    /// `invisible` along with the group), and the group lists it as a child.
    pub fn add_text_item_in(
        &mut self,
        text: &str,
        page_no: i32,
        bbox: [f64; 4],
        label: TextLabel,
        group: &Ref,
    ) -> Ref {
        let layer = self.layer_of(group);
        let prov = vec![self.prov(page_no, bbox, text)];
        let reference =
            self.push_text_with_parent(text, &label.label, prov, &layer, &group.ref_path);
        if let Some(item) = self.doc.texts.last_mut() {
            apply_structure(item, label.level, label.marker, label.enumerated);
            item.hyperlink = label.hyperlink;
        }
        self.add_to_group(group, &reference);
        reference
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

    /// Add a flow-based picture inside a group, on the group's own layer.
    ///
    /// The grouped form of [`Self::add_picture_plain`]: a picture a slide
    /// showed stays in the slide's group when the document is read back.
    pub fn add_picture_plain_in(
        &mut self,
        label: &str,
        image: Option<PictureData>,
        group: &Ref,
    ) -> Ref {
        let layer = self.layer_of(group);
        let reference = self.push_picture_at(label, Vec::new(), image, &layer, &group.ref_path);
        self.add_to_group(group, &reference);
        reference
    }

    /// Add a picture with a box, inside an existing group: the media a slide
    /// shows, placed where the slide places it.
    pub fn add_picture_in(
        &mut self,
        page_no: i32,
        bbox: [f64; 4],
        label: &str,
        image: Option<PictureData>,
        group: &Ref,
    ) -> Ref {
        let layer = self.layer_of(group);
        let prov = vec![self.prov(page_no, bbox, "")];
        let reference = self.push_picture_at(label, prov, image, &layer, &group.ref_path);
        self.add_to_group(group, &reference);
        reference
    }

    /// Add a chart, inside an existing group: the frame the slide draws, which
    /// is a picture, saying what kind of chart it is and the numbers it plots.
    ///
    /// There is no image: a chart is a drawing of data, not a picture file, and
    /// the document keeps the data instead of a rendering of it. `caption` is
    /// the text item that captions the picture — the chart's own title — and the
    /// picture points at it, which is how Docling records the two together.
    pub fn add_chart_picture_in(
        &mut self,
        page_no: i32,
        bbox: [f64; 4],
        classification: &str,
        chart: Option<&GridTable>,
        caption: Option<&Ref>,
        group: &Ref,
    ) -> Ref {
        let layer = self.layer_of(group);
        let prov = vec![self.prov(page_no, bbox, "")];
        let reference = self.push_picture_at("picture", prov, None, &layer, &group.ref_path);
        if let Some(item) = self.doc.pictures.last_mut() {
            item.meta = Some(PictureMeta {
                classification: Some(PictureClassificationMetaField {
                    predictions: vec![PictureClassificationPrediction {
                        class_name: classification.to_string(),
                    }],
                }),
                tabular_chart: chart.map(|chart| TabularChartMetaField {
                    chart_data: table_data(chart),
                }),
            });
            item.captions = caption.cloned().into_iter().collect();
        }
        self.add_to_group(group, &reference);
        reference
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

    /// A header/footer table whose cells are already placed on the grid.
    pub fn add_furniture_table_grid(&mut self, table: &GridTable) -> Ref {
        self.push_grid_table(table, Vec::new(), FURNITURE)
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

    /// Place a quotation: a paragraph with the fact that it is one set on it.
    fn push_quotation(
        &mut self,
        text: &str,
        hyperlink: Option<String>,
        layer: &str,
        parent: &str,
    ) -> Ref {
        let reference = self.push_text_with_parent(text, "paragraph", Vec::new(), layer, parent);
        if let Some(item) = self.doc.texts.last_mut() {
            item.quote = true;
            item.hyperlink = hyperlink;
        }
        reference
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
            // The reference path has no link to point at; a flow item sets one
            // after it is placed ([`Self::add_labeled_item_with_link`]).
            hyperlink: None,
            // Only a flow reader can find a quotation; every other one sets it
            // on the item it placed ([`Self::add_quotation`]).
            quote: false,
        });
        Ref {
            ref_path: format!("#/texts/{idx}"),
        }
    }

    fn push_table(&mut self, rows: &[Vec<String>], prov: Vec<Provenance>, layer: &str) -> Ref {
        self.push_grid_table(&grid_from_rows(rows), prov, layer)
    }

    /// Emit an already-placed table: cells by their grid position and spans.
    fn push_grid_table(&mut self, table: &GridTable, prov: Vec<Provenance>, layer: &str) -> Ref {
        let parent = parent_of(layer);
        self.push_grid_table_at(table, prov, layer, &parent)
    }

    /// Emit a placed table under a named parent — the body, or a group such as
    /// an XLSX sheet.
    fn push_grid_table_at(
        &mut self,
        table: &GridTable,
        prov: Vec<Provenance>,
        layer: &str,
        parent: &str,
    ) -> Ref {
        let data = table_data(table);
        let idx = self.doc.tables.len();
        self.doc.tables.push(TableItem {
            self_ref: format!("#/tables/{idx}"),
            parent: Some(Ref {
                ref_path: parent.to_string(),
            }),
            children: Vec::new(),
            content_layer: layer.to_string(),
            label: "table".to_string(),
            prov,
            data,
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
        let parent = parent_of(layer);
        self.push_picture_at(label, prov, image, layer, &parent)
    }

    fn push_picture_at(
        &mut self,
        label: &str,
        prov: Vec<Provenance>,
        image: Option<PictureData>,
        layer: &str,
        parent: &str,
    ) -> Ref {
        let idx = self.doc.pictures.len();
        self.doc.pictures.push(PictureItem {
            self_ref: format!("#/pictures/{idx}"),
            parent: Some(Ref {
                ref_path: parent.to_string(),
            }),
            children: Vec::new(),
            content_layer: layer.to_string(),
            meta: None,
            label: label.to_string(),
            prov,
            captions: Vec::new(),
            image: image.and_then(image_ref),
        });
        Ref {
            ref_path: format!("#/pictures/{idx}"),
        }
    }

    /// The content layer of a group, for the items placed inside it.
    ///
    /// The group and its children have to agree: Docling rejects a document
    /// whose furniture lists a child that claims the body, and an item dropped
    /// into an invisible group with the body layer would be the same kind of
    /// disagreement.
    fn layer_of(&self, group: &Ref) -> String {
        self.doc
            .groups
            .iter()
            .find(|candidate| candidate.self_ref == group.ref_path)
            .map(|candidate| candidate.content_layer.clone())
            .unwrap_or_else(|| BODY.to_string())
    }

    /// Put a link on the text item just pushed, when the source named one.
    ///
    /// The notes adders push first and link after, so the helper is shared by
    /// the grouped and ungrouped forms; a blank address is no address.
    fn link_last_text(&mut self, hyperlink: Option<String>) {
        if let Some(url) = hyperlink.filter(|url| !url.trim().is_empty()) {
            if let Some(item) = self.doc.texts.last_mut() {
                item.hyperlink = Some(url);
            }
        }
    }

    /// List an item under the group it was placed in.
    fn add_to_group(&mut self, group: &Ref, item: &Ref) {
        if let Some(candidate) = self
            .doc
            .groups
            .iter_mut()
            .find(|candidate| candidate.self_ref == group.ref_path)
        {
            candidate.children.push(item.clone());
        }
    }

    pub fn build(self) -> DoclingDocument {
        self.doc
    }
}

/// A table's cells placed on its grid, ready for the document.
///
/// Shared by the table list and by a chart's metadata: a chart's plotted
/// A rectangular table's rows placed on its grid: one cell per slot, the first
/// row a header and the first column a row label, unless a reader placed spans
/// explicitly.
fn grid_from_rows(rows: &[Vec<String>]) -> GridTable {
    let num_rows = rows.len();
    let num_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut cells = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for (c, text) in row.iter().enumerate() {
            cells.push(GridCell {
                text: text.clone(),
                start_row: r,
                start_col: c,
                row_span: 1,
                col_span: 1,
                column_header: r == 0,
                row_header: c == 0 && r > 0,
            });
        }
    }
    GridTable {
        num_rows,
        num_cols,
        cells,
    }
}

/// numbers are a table too, and the two must place their cells the same way.
fn table_data(table: &GridTable) -> TableData {
    let cells = table
        .cells
        .iter()
        .map(|cell| {
            let row_span = cell.row_span.max(1);
            let col_span = cell.col_span.max(1);
            TableCell {
                text: cell.text.clone(),
                start_row_offset_idx: cell.start_row,
                end_row_offset_idx: cell.start_row + row_span,
                start_col_offset_idx: cell.start_col,
                end_col_offset_idx: cell.start_col + col_span,
                row_span: row_span as i32,
                col_span: col_span as i32,
                column_header: cell.column_header,
                row_header: cell.row_header,
                row_section: false,
                bbox: None,
            }
        })
        .collect();
    TableData {
        num_rows: table.num_rows,
        num_cols: table.num_cols,
        orientation: None,
        table_cells: cells,
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
