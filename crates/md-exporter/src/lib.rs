//! Render a [`DoclingDocument`] as Markdown.
//!
//! The document's body tree is walked in reading order and each item becomes
//! the Markdown that says the same thing. The rules follow Docling's own
//! Markdown export where the two can be compared (its CLI writes Markdown by
//! default, which is how the style below was checked):
//!
//! * **Furniture is omitted.** A running head or page number repeated on every
//!   page would interrupt the prose, and a reader of the Markdown does not need
//!   it; it stays in the JSON for whoever does. The **notes layer** goes the
//!   same way by default: a deck's speaker notes and comments are the deck
//!   talking about itself, and this is a reading of what it shows. A reader who
//!   wants those too asks for them ([`render_with_notes`], the CLI's
//!   `--include-notes`); they then appear where the body tree holds them, after
//!   the slide they belong to, and each paragraph is a blockquote line headed
//!   `**Notes:**` — a remark reads as a remark, never as the slide it is about,
//!   and the speaker's paragraph breaks are kept. A comment is headed
//!   `**Comment:**` instead, so a reviewer's remark is not mistaken for the
//!   presenter's even when the group markers are gone; the reader keeps that
//!   distinction.
//!   The label also keeps a comment that opens `[author: …]:` from being read
//!   as a Markdown link reference definition, which a line beginning `[` is —
//!   inside a blockquote as much as outside one. A remark's `hyperlink` is
//!   written around the paragraph the item opens with, which is the shape the
//!   reader recovers the address from.
//! * **Headings**: `title` becomes `#`; a `section_header` becomes `#` at its
//!   own depth plus one, so a level-1 section under a title is `##` — the same
//!   mapping Docling uses.
//! * **List items** keep the marker the document printed: a numbered marker
//!   (`1.`, `2)`) starts a Markdown list of its own kind, a roman or lettered
//!   marker (`i.`, `a)`) is kept as text after `- `, and a plain bullet glyph
//!   (`•`) is dropped in favour of Markdown's own `-`.
//! * **Quotations keep their `>`.** Docling has no block quote item — its own
//!   readers flatten one — so a quoted paragraph is a `paragraph` with the fact
//!   kept beside the model (see `TextItem::quote`), and it is written back as a
//!   blockquote line rather than as the document's own prose. Consecutive quoted
//!   lines are one quotation when read back, and a blank line between two of
//!   them keeps two quotations apart, so a render is a fixed point.
//! * **Tables** become GitHub-flavoured pipe tables, padded to their column
//!   widths like Docling's. Markdown has no merged cells, so a spanning cell's
//!   text is written once at its top-left and the cells it covers render
//!   empty — the flattening Docling's export performs. The format needs a header
//!   row before the separator and has one place to put it, so the row the
//!   document marked heads the table wherever it sat: names written below a
//!   caption row become the header, and the rows above them follow in the body.
//!   A table that marks no row still gets an empty header row, so it parses as
//!   a table.
//! * **Formulas** become `$$` display blocks. The content is the Unicode
//!   linearisation the DOCX parser produces until the LaTeX lineariser lands
//!   (Phase 11 W5); Docling writes LaTeX here, and `test-data/golden/README.md`
//!   already records that the two notations are deliberately not compared.
//! * **Pictures** become `![Image](uri)` when the document holds an image for
//!   them; a drawing with no image leaves nothing rather than a broken link.
//!   [`ImageMode`] chooses what the reference is: the file beside the document
//!   (the default, Docling's `referenced`), the picture's own bytes as a base64
//!   `data:` URI (`embedded`), or a `<!-- image -->` comment where the picture
//!   stood (`placeholder`). The two mode names and the placeholder text are
//!   Docling's own.
//! * **Head facts become front matter.** A page read from HTML keeps its
//!   description, canonical URL and Open Graph/Twitter cards beside the model,
//!   where Docling cannot hold them. A Markdown rendering of such a page starts
//!   with YAML front matter holding them (`description`, `canonical`, and a
//!   `social` list of `key`/`content`), so writing the page, reading it back and
//!   writing it again keeps the head it came with. A document with none gains no
//!   front matter.
//! * **Groups** are marked, not flattened. Markdown has no groups, so the
//!   writer surrounds each one with HTML comments that render as nothing:
//!   `<!-- pdfextractor:group <label> <layer> <name> -->` and
//!   `<!-- pdfextractor:/group -->`. A deck's slide groups and its comments'
//!   `comment_section` groups are what the reader rebuilds from them, so a
//!   round trip keeps a slide's words and notes together — and each comment in
//!   its own section — instead of leaving one flat list. A group that renders
//!   nothing (a hidden slide, a comment read without `--include-notes`) is not
//!   marked at all, and a document with no groups is unchanged.
//! * **Links** become `[text](url)`. The document keeps a link on the whole
//!   item, not on a span of it — a PDF `/Link` annotation covers the words, a
//!   DOCX `w:hyperlink` wraps them, an HTML `a` holds them — so the whole text
//!   is the label. A URL that Markdown would otherwise read as the end of the
//!   address or as a title is written in `<…>` form.
//!
//! Everything else — paragraphs, captions, footnotes, references — is a plain
//! paragraph. The output ends with exactly one newline, and no run of blank
//! lines can appear, so two renders of the same document are byte-identical.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use pdfextractor_core::{DoclingDocument, Group, ImageRef, PictureItem, TableItem, TextItem};
use std::path::{Path, PathBuf};

/// The tree walk every non-JSON writer shares.
pub mod walk;

/// The same walk, written as an HTML page.
pub mod html;

/// An equation drawn as an SVG picture, for the HTML writer's fallback when
/// the LaTeX cannot be typeset.
mod formula_image;

/// The same walk, reduced to plain text.
pub mod text;

/// The whole document model, reading order first, as YAML.
pub mod yaml;

pub use walk::{walk_body, walk_body_with, Order, Visitor};

/// How a rendered document refers to its pictures.
///
/// The names are Docling's `--image-export-mode` values, so the flag means the
/// same thing in both tools; the default here is `referenced`, the behaviour
/// every existing render had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageMode {
    /// Point at the file written beside the document.
    #[default]
    Referenced,
    /// Carry the picture's bytes in the document as a base64 `data:` URI, so
    /// one file is the whole document.
    Embedded,
    /// Put `<!-- image -->` where the picture stood: the spot is marked, and
    /// nothing claims to be an image that is not there.
    Placeholder,
}

/// What a picture becomes when it cannot be embedded or referenced.
///
/// Docling writes the same comment in Markdown placeholder mode, so a reader
/// who sees one is seeing a mark both tools agree on. It is valid HTML too,
/// which is what the HTML writer uses it for.
pub const IMAGE_PLACEHOLDER: &str = "<!-- image -->";

/// The comment that opens a group, in the form the Markdown reader looks for:
/// `<!-- pdfextractor:group <label> <layer> <name> -->`. The name comes last so
/// a document's own names may hold spaces.
pub(crate) const GROUP_START: &str = "<!-- pdfextractor:group ";
/// The comment that closes the innermost open group.
pub(crate) const GROUP_END: &str = "<!-- pdfextractor:/group -->";

/// The label a speaker note's quote opens with.
pub(crate) const NOTE_MARKER: &str = "**Notes:**";
/// The label a comment's quote opens with. The two labels are what separate a
/// deck's own remarks from a reviewer's, even without the group markers.
pub(crate) const COMMENT_MARKER: &str = "**Comment:**";

/// Render `doc` as a Markdown document.
pub fn render(doc: &DoclingDocument) -> String {
    render_with_notes(doc, false)
}

/// Render `doc` as Markdown, including the notes layer when `include_notes`.
///
/// A deck's speaker notes follow the slide they belong to; its comments come
/// after the slides, where their `comment_section` groups sit in the body.
/// Everything else is the same reading [`render`] gives.
pub fn render_with_notes(doc: &DoclingDocument, include_notes: bool) -> String {
    render_with_images(doc, include_notes, ImageMode::Referenced, None)
}

/// Render `doc` as Markdown with the reader's choices: whether the notes layer
/// joins the reading, how pictures are referred to, and — for
/// [`ImageMode::Embedded`] — the directory a picture's `uri` is relative to.
///
/// `image_base` is the directory the export writes its images beside; without
/// it a relative URI is resolved from the current directory. An absolute URI,
/// or one already written as a `data:` URI, ignores it.
pub fn render_with_images(
    doc: &DoclingDocument,
    include_notes: bool,
    image_mode: ImageMode,
    image_base: Option<&Path>,
) -> String {
    let mut renderer = Renderer {
        blocks: Vec::new(),
        image_mode,
        image_base: image_base.map(Path::to_path_buf),
        groups: Vec::new(),
        doc,
    };
    // The body tree is the document. Furniture lives beside it (and, for a
    // page-labelled document, mixed into its order) and is skipped per item by
    // the shared walk.
    walk_body_with(doc, include_notes, &mut renderer);
    let body = renderer.blocks.join("\n\n");
    let mut out = String::new();
    if let Some(front) = front_matter(doc) {
        out.push_str("---\n");
        out.push_str(&front);
        out.push_str("\n---\n");
    }
    if !body.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&body);
        out.push('\n');
    }
    out
}

/// The page's head facts as YAML front matter, when it has any.
///
/// `social` is a list of `key`/`content` records rather than a mapping because
/// a card is an ordered statement: `og:image:width` belongs to the image before
/// it, and a page may state the same key twice. A mapping would sort the keys
/// and keep only the last, which changes what the page said.
fn front_matter(doc: &DoclingDocument) -> Option<String> {
    let head = &doc.html_head;
    let mut fields = serde_yaml::Mapping::new();
    if let Some(description) = &head.description {
        fields.insert(
            serde_yaml::Value::String("description".to_string()),
            serde_yaml::Value::String(description.clone()),
        );
    }
    if let Some(canonical) = &head.canonical {
        fields.insert(
            serde_yaml::Value::String("canonical".to_string()),
            serde_yaml::Value::String(canonical.clone()),
        );
    }
    if !head.social.is_empty() {
        let cards: Vec<serde_yaml::Value> = head
            .social
            .iter()
            .map(|(key, content)| {
                let mut card = serde_yaml::Mapping::new();
                card.insert(
                    serde_yaml::Value::String("key".to_string()),
                    serde_yaml::Value::String(key.clone()),
                );
                card.insert(
                    serde_yaml::Value::String("content".to_string()),
                    serde_yaml::Value::String(content.clone()),
                );
                serde_yaml::Value::Mapping(card)
            })
            .collect();
        fields.insert(
            serde_yaml::Value::String("social".to_string()),
            serde_yaml::Value::Sequence(cards),
        );
    }
    if fields.is_empty() {
        return None;
    }
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(fields))
        .expect("a page's head is strings and a list of them");
    Some(yaml.trim_end_matches('\n').to_string())
}

/// Write `doc` as Markdown to `path`.
///
/// The bytes land in a sibling temporary file that is renamed into place, so a
/// reader never sees a half-written document — the same contract the JSON
/// exporter keeps, and for the same reason: a stopped run leaves whole files
/// or none. The rename is atomic because the temporary file sits in the same
/// directory, and therefore on the same filesystem, as the destination.
pub fn export_to_file(doc: &DoclingDocument, path: &Path) -> anyhow::Result<()> {
    export_to_file_with_notes(doc, path, false)
}

/// Write `doc` as Markdown to `path`, including the notes layer when
/// `include_notes` — [`export_to_file`] with the reader's choice of scope.
pub fn export_to_file_with_notes(
    doc: &DoclingDocument,
    path: &Path,
    include_notes: bool,
) -> anyhow::Result<()> {
    export_to_file_with_images(doc, path, include_notes, ImageMode::Referenced)
}

/// Write `doc` as Markdown to `path` with an [`ImageMode`].
///
/// An embedded picture is read from the directory the file is written in,
/// which is where the extraction wrote its images.
pub fn export_to_file_with_images(
    doc: &DoclingDocument,
    path: &Path,
    include_notes: bool,
    image_mode: ImageMode,
) -> anyhow::Result<()> {
    let base = path.parent();
    let text = render_with_images(doc, include_notes, image_mode, base);

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.md".to_string());
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));

    std::fs::write(&temp, text)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        // Do not leave the temporary file behind for a failed export.
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

struct Renderer<'a> {
    /// One entry per rendered block, joined by a blank line at the end.
    blocks: Vec<String>,
    /// How a picture is referred to.
    image_mode: ImageMode,
    /// Where a picture's relative `uri` is read from when embedding.
    image_base: Option<PathBuf>,
    /// One entry per group whose marker has been written: the number of
    /// blocks before the marker, so a group that renders nothing can take its
    /// marker back — and whether the group is a comment's section, which the
    /// quotes inside it are labelled for.
    groups: Vec<(usize, bool)>,
    /// The document being written, so a table cell can look up the pictures its
    /// own `ref` names.
    doc: &'a DoclingDocument,
}

impl<'a> Renderer<'a> {
    /// Append one block, unless it would be empty: a blank line in the output
    /// is a paragraph break, and an empty paragraph is noise.
    fn block(&mut self, text: String) {
        if !text.trim().is_empty() {
            self.blocks.push(text);
        }
    }
}

/// The Markdown writer as a [`Visitor`]: the shared walk decides what is
/// visited and in what order, this decides what each item becomes.
impl Visitor for Renderer<'_> {
    fn text(&mut self, _index: usize, item: &TextItem) {
        self.text_item(item);
    }
    fn table(&mut self, _index: usize, table: &TableItem) {
        Renderer::table(self, table);
    }
    fn picture(&mut self, _index: usize, picture: &PictureItem) {
        Renderer::picture(self, picture);
    }
    fn group_start(&mut self, group: &Group) {
        Renderer::open_group(self, group);
    }
    fn group_end(&mut self) {
        Renderer::close_group(self);
    }
}

impl Renderer<'_> {
    fn text_item(&mut self, item: &TextItem) {
        // A deck's remarks are not slide content, and a reader who asked for
        // them still has to be able to tell which is which: a quoted block
        // labelled once at its head says “the deck talking”, not “the slide”.
        if item.content_layer == "notes" {
            self.notes_item(item);
            return;
        }
        match item.label.as_str() {
            "title" => self.heading(1, item),
            "section_header" => {
                // A section header's level is its depth; without one it is the
                // document's top section, which sits at `##` under a title.
                let depth = item.level.unwrap_or(1).clamp(1, 5);
                self.heading(depth + 1, item);
            }
            "list_item" => {
                let text = inline_linked(&item.text, item.hyperlink.as_deref());
                let line = match item.marker.as_deref() {
                    // A marker the document wrote as nothing is Markdown's own
                    // bullet: writing it after the `-` would leave a second
                    // space, which reads back as a different bullet and makes
                    // the file change every time it is saved.
                    Some(marker) if marker.trim().is_empty() => format!("- {text}"),
                    Some(marker) if marker.starts_with(|c: char| c.is_ascii_digit()) => {
                        // The number the document printed starts an ordered
                        // list, exactly as written (`1.`, `2)`).
                        format!("{marker} {text}")
                    }
                    Some(marker) if is_bullet_glyph(marker) => format!("- {text}"),
                    // A roman or lettered marker is Markdown's `-` with the
                    // marker kept as text — what Docling writes for a list
                    // that continued with `i.` after an interruption.
                    Some(marker) => format!("- {marker} {text}"),
                    None => format!("- {text}"),
                };
                self.block(line);
            }
            // Code and formulas are literal: a link inside one would be an
            // address the document printed, not a link it made.
            "code" => self.block(fence(&item.text)),
            "formula" => self.block(format!("$$\n{}\n$$", item.text.trim())),
            // A quotation keeps its `>`: the model has no block quote — Docling
            // itself reads one as a paragraph — so the fact lives beside the
            // item, and a reader of the Markdown has to see that these words are
            // quoted rather than the document's own.
            _ if item.quote => self.block(format!(
                "> {}",
                inline_linked(&item.text, item.hyperlink.as_deref())
            )),
            // Paragraphs, captions, footnotes, references: a paragraph each.
            _ => self.block(inline_linked(&item.text, item.hyperlink.as_deref())),
        }
    }

    /// A notes-layer item as a quote: one labelled line per paragraph, so the
    /// speaker's paragraph breaks survive instead of being flattened.
    ///
    /// The label on *every* line is load-bearing. A comment's text begins with
    /// its `[author: …, time: …]:` prefix, and `[label]: destination` at the
    /// start of a line is exactly how Markdown writes a *link reference
    /// definition* — a line that is not shown at all. Quoting alone does not save
    /// it: CommonMark reads definitions inside blockquotes too, so
    /// `> [author: …]:` is still swallowed. The bold label before the note is
    /// what makes the line start with words instead of `[`, and every paragraph
    /// carries it because a later paragraph could open with `[` just as well.
    /// Consecutive quoted lines are one note when read back; a blank line
    /// between remarks keeps two notes apart. Comments arrive here too — the
    /// model keeps them on the same layer, and their own author/time prefix still
    /// says what they are after the label.
    fn notes_item(&mut self, item: &TextItem) {
        // A comment is not a speaker's aside: it is somebody talking about the
        // slide. Its own label keeps the two apart in the quote itself, so a
        // reader who loses the group markers still knows which is which.
        let comment = self.groups.last().is_some_and(|(_, comment)| *comment);
        let marker = if comment { COMMENT_MARKER } else { NOTE_MARKER };
        let paragraphs = note_paragraphs(&item.text);
        let mut lines = Vec::with_capacity(paragraphs.len());
        for (index, paragraph) in paragraphs.iter().enumerate() {
            // The hyperlink, when there is one, belongs to the item, not to a
            // paragraph: it is written once, on the first.
            let text = if index == 0 {
                inline_linked(paragraph, item.hyperlink.as_deref())
            } else {
                paragraph.clone()
            };
            lines.push(format!("> {marker} {text}"));
        }
        if !lines.is_empty() {
            self.block(lines.join("\n"));
        }
    }

    /// Mark a group's start with the comment the reader rebuilds it from.
    ///
    /// The marker is an HTML comment, so it renders as nothing and a reader of
    /// the Markdown never sees it. Markdown has no groups of its own, so this
    /// is the only way to say which items belong to one slide — or to one
    /// comment's section — and it is what the reader in `text-parser` looks
    /// for.
    fn open_group(&mut self, group: &Group) {
        self.groups
            .push((self.blocks.len(), group.label == "comment_section"));
        self.block(format!(
            "{GROUP_START}{} {} {} -->",
            group.label, group.content_layer, group.name
        ));
    }

    /// Close the innermost group, or drop its marker when nothing was
    /// rendered inside it: a hidden slide's content is on the invisible layer,
    /// and a comment read without `--include-notes` has no words here, so an
    /// empty pair would claim a group the file does not show.
    fn close_group(&mut self) {
        let Some((at, _)) = self.groups.pop() else {
            return;
        };
        if self.blocks.len() == at + 1 {
            self.blocks.pop();
        } else {
            self.block(GROUP_END.to_string());
        }
    }

    fn heading(&mut self, depth: i32, item: &TextItem) {
        let marks = "#".repeat(depth.clamp(1, 6) as usize);
        self.block(format!(
            "{marks} {}",
            inline_linked(&item.text, item.hyperlink.as_deref())
        ));
    }

    /// One picture as inline Markdown: `None` when there is no picture to
    /// point at (vector art, or a media type Docling cannot name), because a
    /// broken image link is worse than none.
    fn picture_markup(&self, item: &PictureItem) -> Option<String> {
        match self.image_mode {
            ImageMode::Referenced => {
                let uri = item.image.as_ref().and_then(|image| image.uri.as_deref())?;
                // A file the page named may hold a space, a bracket or a `&`:
                // `destination` is what keeps `(…)` from ending at the first
                // `)` and the space from reading as a title.
                Some(format!("![Image]({})", destination(uri)))
            }
            ImageMode::Embedded => {
                let data = item
                    .image
                    .as_ref()
                    .and_then(|image| image_data_uri(image, self.image_base.as_deref()));
                match data {
                    // The bytes are here, so the whole picture travels in the
                    // document. The media type is the image's own, never PNG by
                    // assumption: a JPEG relabelled PNG would not decode.
                    Some(uri) => Some(format!("![Image]({})", destination(&uri))),
                    // Nothing to embed (no image, or bytes not on disk): mark
                    // the spot rather than point at a file that may not be
                    // shipped with the page.
                    None => Some(IMAGE_PLACEHOLDER.to_string()),
                }
            }
            ImageMode::Placeholder => Some(IMAGE_PLACEHOLDER.to_string()),
        }
    }

    fn picture(&mut self, item: &PictureItem) {
        if let Some(markup) = self.picture_markup(item) {
            self.block(markup);
        }
    }

    /// The pictures a table cell holds, as inline Markdown, in the cell's own
    /// order: a Word cell's picture is the cell's, and reads inside its pipe
    /// cell rather than as a block beside the table.
    fn cell_picture_markup(&self, cell: &pdfextractor_core::TableCell) -> String {
        cell_pictures(self.doc, cell)
            .iter()
            .filter_map(|picture| self.picture_markup(picture))
            .map(|markup| markup.replace('|', "\\|"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn table(&mut self, table: &TableItem) {
        let rendered = pipe_table_with(table, |cell| self.cell_picture_markup(cell));
        if let Some(block) = rendered {
            self.block(block);
        }
    }
}

/// The pictures a table cell holds, resolved through the item its `ref` names.
///
/// A rich cell references one item — the group a builder places a Word cell's
/// content in — and its pictures are that group's children. A `ref` that names
/// a picture directly is answered too, and one that names something else (or
/// nothing the document holds) gives no pictures rather than a broken render.
pub(crate) fn cell_pictures<'a>(
    doc: &'a DoclingDocument,
    cell: &pdfextractor_core::TableCell,
) -> Vec<&'a PictureItem> {
    let Some(path) = cell
        .reference
        .as_ref()
        .map(|reference| reference.ref_path.as_str())
    else {
        return Vec::new();
    };
    let picture = |path: &str| {
        path.strip_prefix("#/pictures/")
            .and_then(|index| index.parse::<usize>().ok())
    };
    if let Some(index) = picture(path) {
        return doc.pictures.get(index).into_iter().collect();
    }
    let Some(group) = path
        .strip_prefix("#/groups/")
        .and_then(|index| index.parse::<usize>().ok())
        .and_then(|index| doc.groups.get(index))
    else {
        return Vec::new();
    };
    group
        .children
        .iter()
        .filter_map(|child| doc.pictures.get(picture(&child.ref_path)?))
        .collect()
}

/// The `data:` URI carrying `image`'s bytes, when they can be reached.
///
/// A `uri` that is already a `data:` URI is kept exactly as written; a file is
/// read from `base` (or as given, when there is no base). A remote address is
/// not fetched — a renderer that reached over the network would turn a local
/// conversion into an HTTP client — and an image with no media type cannot be
/// labelled honestly, so both answer `None` and the caller writes a
/// placeholder instead.
pub(crate) fn image_data_uri(image: &ImageRef, base: Option<&Path>) -> Option<String> {
    let uri = image.uri.as_deref()?;
    if uri.starts_with("data:") {
        return Some(uri.to_string());
    }
    if uri.contains("://") {
        return None;
    }
    let mimetype = image.mimetype.as_deref()?;
    let path = match base {
        Some(base) => base.join(uri),
        None => PathBuf::from(uri),
    };
    let bytes = std::fs::read(path).ok()?;
    Some(format!("data:{mimetype};base64,{}", STANDARD.encode(bytes)))
}

/// The GitHub-flavoured pipe table for `table`, shared with the text writer:
/// the two formats differ in every markup *around* a table but never in which
/// cell holds what. A table with no rows or no columns has no table to write.
pub(crate) fn pipe_table(table: &TableItem) -> Option<String> {
    pipe_table_with(table, |_| String::new())
}

/// The same table, with whatever each cell holds besides its words written into
/// the cell after them: a cell with pictures shows them inline, where a reader
/// of the table meets them, rather than as blocks beside it.
pub(crate) fn pipe_table_with(
    table: &TableItem,
    extra: impl Fn(&pdfextractor_core::TableCell) -> String,
) -> Option<String> {
    let rows = table.data.num_rows;
    let cols = table.data.num_cols;
    if rows == 0 || cols == 0 {
        return None;
    }

    // The grid, by cell start position; a spanning cell's text sits at its
    // top-left and the cells it covers stay empty (flattened, as Markdown
    // has no merged cells).
    let mut grid = vec![vec![String::new(); cols]; rows];
    let mut header_row = None;
    for cell in &table.data.table_cells {
        let (r, c) = (cell.start_row_offset_idx, cell.start_col_offset_idx);
        if r >= rows || c >= cols {
            continue;
        }
        let mut content = cell
            .text
            .replace('|', "\\|")
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        let held = extra(cell);
        if !held.is_empty() {
            if !content.is_empty() {
                content.push(' ');
            }
            content.push_str(&held);
        }
        grid[r][c] = content;
        if cell.column_header && header_row.is_none() {
            header_row = Some(r);
        }
    }

    // GitHub-flavoured Markdown needs a header row before the separator, and
    // has one place to put it. The row the document marked heads the pipe
    // table wherever in the grid it sits, so a table that names its columns
    // below a caption row writes the names first and the rows above them
    // after — no empty header row, and no column left unnamed. A table that
    // marks none still gets an empty header row, so it parses as a table.
    let header = header_row;

    let width: Vec<usize> = (0..cols)
        .map(|c| {
            // Every row takes part in the width, header included: a wide
            // heading must not push its column's text off the padding.
            (0..rows)
                .map(|r| grid[r][c].chars().count())
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();

    let mut out = String::new();
    let push_row = |out: &mut String, cells: &[String]| {
        out.push('|');
        for (c, cell) in cells.iter().enumerate() {
            out.push_str(&format!(" {:width$} |", cell, width = width[c]));
        }
        out.push('\n');
    };
    match header {
        Some(row) => push_row(&mut out, &grid[row]),
        None => push_row(&mut out, &vec![String::new(); cols]),
    }
    out.push('|');
    for w in &width {
        out.push_str(&"-".repeat(w + 2));
        out.push('|');
    }
    out.push('\n');
    for row in (0..rows).filter(|row| Some(*row) != header) {
        push_row(&mut out, &grid[row]);
    }
    Some(out.trim_end().to_string())
}

/// A text as a paragraph of Markdown: one line, so a stray newline in the
/// source cannot open an accidental code block or list.
pub(crate) fn inline(text: &str) -> String {
    text.lines().map(str::trim).collect::<Vec<_>>().join(" ")
}

/// A note's paragraphs, each on one line.
///
/// The document joins speaker-note paragraphs with a newline and a soft line
/// break with a vertical tab. The first is the speaker's structure and is kept;
/// the second is a break inside one paragraph, and a paragraph is one line, so
/// it becomes a space. Blank paragraphs are not paragraphs.
pub(crate) fn note_paragraphs(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|paragraph| inline(&paragraph.replace('\u{b}', " ")))
        .filter(|paragraph| !paragraph.is_empty())
        .collect()
}

/// A text item as inline Markdown, wrapped in `[text](url)` when it carries a
/// hyperlink.
///
/// Splitting a link into a span would need offsets the model does not keep
/// (`hyperlink` is the whole item), so the item's whole text is the label. A
/// hyperlink that is empty or blank is no link at all: `[text]()` is a broken
/// address, and leaving the text plain says what the document does.
pub(crate) fn inline_linked(text: &str, hyperlink: Option<&str>) -> String {
    let text = inline(text);
    match hyperlink.map(str::trim).filter(|url| !url.is_empty()) {
        Some(url) => format!("[{text}]({})", destination(url)),
        None => text,
    }
}

/// A link or picture destination Markdown reads as one address.
///
/// `(...)` ends at the first `)`, and whitespace ends it in favour of a
/// title, so a URL or a picture's file name holding either is written in `<…>`
/// form — Markdown's way of saying the whole thing is the address. `<`, `>`
/// and a backslash cannot appear there unescaped, so each is written with the
/// backslash Markdown reads as itself; a percent-encoding would give the reader
/// another name than the one the page holds.
fn destination(url: &str) -> String {
    if url.contains(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>')) {
        format!(
            "<{}>",
            url.replace('\\', "\\\\")
                .replace('<', "\\<")
                .replace('>', "\\>")
        )
    } else {
        url.to_string()
    }
}

/// A fenced code block, with a fence long enough to hold whatever the code
/// itself contains.
pub(crate) fn fence(text: &str) -> String {
    let mut fence = "```".to_string();
    while text.contains(fence.as_str()) {
        fence.push('`');
    }
    let body = text
        .strip_prefix("```")
        .and_then(|t| t.strip_suffix("```"))
        .unwrap_or(text);
    format!("{fence}\n{body}\n{fence}")
}

/// Whether a marker is one of the plain bullet glyphs, which Markdown's own
/// `-` replaces.
pub(crate) fn is_bullet_glyph(marker: &str) -> bool {
    matches!(marker.trim(), "•" | "◦" | "▪" | "‣" | "-" | "*" | "+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfextractor_core::{
        DocumentBuilder, GridCell, GridTable, HtmlHead, PictureData, TextLabel,
    };

    /// One slide, its speaker notes inside the slide's group and a comment in
    /// its own `comment_section` group — the shape the PPTX reader builds.
    fn noted_deck() -> DoclingDocument {
        let mut b = DocumentBuilder::new("demo");
        let slide = b.add_group("chapter", "slide-0", "body");
        b.add_text_item_in(
            "Q3 Review",
            1,
            [0.0, 0.0, 10.0, 10.0],
            TextLabel {
                label: "title".to_string(),
                ..Default::default()
            },
            &slide,
        );
        b.add_notes_item_in("Say the picture is the growth.", 1, [0.0; 4], &slide);
        let comment = b.add_comment_group("comment-slide1-1");
        b.add_comment_in("[author: Ada]: Check the numbers.", &comment);
        b.build()
    }

    #[test]
    fn the_notes_layer_is_omitted_unless_asked_for() {
        let doc = noted_deck();
        let slide = "<!-- pdfextractor:group chapter body slide-0 -->";
        let comment = "<!-- pdfextractor:group comment_section notes comment-slide1-1 -->";
        let end = "<!-- pdfextractor:/group -->";
        assert_eq!(render(&doc), format!("{slide}\n\n# Q3 Review\n\n{end}\n"));
        assert_eq!(
            render_with_notes(&doc, true),
            format!(
                "{slide}\n\n# Q3 Review\n\n\
                 > **Notes:** Say the picture is the growth.\n\n\
                 {end}\n\n\
                 {comment}\n\n\
                 > **Comment:** [author: Ada]: Check the numbers.\n\n\
                 {end}\n"
            )
        );
    }

    /// A quotation is a paragraph the document set apart, and Markdown says so
    /// with `>`: Docling's model has no block quote, so the fact travels on the
    /// item and this is what puts it back on the page.
    #[test]
    fn a_quotation_is_written_as_a_quote() {
        let mut b = DocumentBuilder::new("demo");
        b.add_quotation("The medium is the message.", None);
        b.add_text("After.", 1);
        assert_eq!(
            render(&b.build()),
            "> The medium is the message.\n\nAfter.\n"
        );

        // Two quotations are two of them: the blank line between them is what
        // keeps the second from reading back as the first one's next line.
        let mut b = DocumentBuilder::new("demo");
        b.add_quotation("One.", None);
        b.add_quotation("Two.", None);
        assert_eq!(render(&b.build()), "> One.\n\n> Two.\n");

        // The address a quotation opened with is still a link in the quote.
        let mut b = DocumentBuilder::new("demo");
        b.add_quotation("the ledger", Some("https://example.com/ledger".into()));
        assert_eq!(
            render(&b.build()),
            "> [the ledger](https://example.com/ledger)\n"
        );
    }

    /// A group nothing is rendered from leaves no marker: a hidden slide's
    /// words are on the invisible layer, and a file that says nothing about
    /// them would still be claiming a section for them.
    #[test]
    fn a_group_with_no_rendered_items_is_not_marked() {
        let mut b = DocumentBuilder::new("demo");
        let slide = b.add_group("chapter", "slide-0", "invisible");
        b.add_text_item_in(
            "Hidden",
            1,
            [0.0; 4],
            TextLabel {
                label: "paragraph".to_string(),
                ..Default::default()
            },
            &slide,
        );
        let doc = b.build();
        assert_eq!(render(&doc), "");
        assert_eq!(render_with_notes(&doc, true), "");
    }

    /// A note's words can be shaped like a link reference definition, which
    /// Markdown does not render at all — a comment's own `[author: …]:` prefix
    /// is the near miss. The label on the line keeps it words.
    #[test]
    fn a_note_that_looks_like_a_link_definition_stays_words() {
        let mut b = DocumentBuilder::new("demo");
        let group = b.add_comment_group("comment-slide1-1");
        b.add_comment_in("[spec]: https://example.com/spec", &group);
        assert_eq!(
            render_with_notes(&b.build(), true),
            "<!-- pdfextractor:group comment_section notes comment-slide1-1 -->\n\n\
             > **Comment:** [spec]: https://example.com/spec\n\n\
             <!-- pdfextractor:/group -->\n"
        );
    }

    /// A note's paragraphs are its structure: each is a quoted line of its own,
    /// labelled like the first, so the speaker's breaks survive instead of
    /// being flattened into one line. Every line carries the label because any
    /// paragraph could open with `[`, which would make it a link reference
    /// definition rather than words.
    #[test]
    fn a_note_keeps_its_paragraphs_as_labelled_quote_lines() {
        let mut b = DocumentBuilder::new("demo");
        let slide = b.add_group("chapter", "slide-0", "body");
        b.add_text_item_in(
            "Q3 Review",
            1,
            [0.0, 0.0, 10.0, 10.0],
            TextLabel {
                label: "title".to_string(),
                ..Default::default()
            },
            &slide,
        );
        b.add_notes_item_in(
            "Open with the revenue line.\nThen the regions.",
            1,
            [0.0; 4],
            &slide,
        );
        assert_eq!(
            render_with_notes(&b.build(), true),
            "<!-- pdfextractor:group chapter body slide-0 -->\n\n\
             # Q3 Review\n\n\
             > **Notes:** Open with the revenue line.\n> **Notes:** Then the regions.\n\n\
             <!-- pdfextractor:/group -->\n"
        );
    }

    /// A remark's link is the item's, written once around the paragraph the
    /// item opens with — the mark the Markdown reader recovers the address
    /// from.
    #[test]
    fn a_linked_remark_writes_its_link_around_the_first_paragraph() {
        let mut b = DocumentBuilder::new("demo");
        let group = b.add_comment_group("comment-slide1-1");
        b.add_comment_in_with_link(
            "the ledger",
            Some("https://example.com/ledger".to_string()),
            &group,
        );
        assert_eq!(
            render_with_notes(&b.build(), true),
            "<!-- pdfextractor:group comment_section notes comment-slide1-1 -->\n\n\
             > **Comment:** [the ledger](https://example.com/ledger)\n\n\
             <!-- pdfextractor:/group -->\n"
        );
    }

    /// A vertical tab is a soft line break inside one paragraph, not a new
    /// paragraph: it becomes a space, and the paragraph stays one line.
    #[test]
    fn a_soft_break_stays_inside_its_paragraph() {
        let mut b = DocumentBuilder::new("demo");
        let slide = b.add_group("chapter", "slide-0", "body");
        b.add_notes_item_in(
            "One line\u{b}split here.\nSecond paragraph.",
            1,
            [0.0; 4],
            &slide,
        );
        assert_eq!(
            render_with_notes(&b.build(), true),
            "<!-- pdfextractor:group chapter body slide-0 -->\n\n\
             > **Notes:** One line split here.\n> **Notes:** Second paragraph.\n\n\
             <!-- pdfextractor:/group -->\n"
        );
    }

    /// A page read from HTML keeps its head beside the model; Markdown has no
    /// head, so it states the same facts as YAML front matter. The cards are a
    /// list, not a mapping: a repeated key and the order of an image and its
    /// width are part of what the page said.
    #[test]
    fn a_pages_head_facts_become_yaml_front_matter() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("Q3 review", "title", None, None, None);
        b.set_html_head(HtmlHead {
            description: Some("Quarterly results & outlook".to_string()),
            canonical: Some("https://example.com/reports/q3?quarter=3&region=eu".to_string()),
            social: vec![
                ("og:title".to_string(), "Q3 & costs".to_string()),
                (
                    "og:image".to_string(),
                    "https://example.com/q3.png".to_string(),
                ),
                ("og:image:width".to_string(), "1200".to_string()),
                (
                    "og:image".to_string(),
                    "https://example.com/q3@2x.png".to_string(),
                ),
                (
                    "twitter:card".to_string(),
                    "summary_large_image".to_string(),
                ),
            ],
        });
        let markdown = render(&b.build());
        let (yaml, body) = markdown
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n\n"))
            .expect("a front matter and a blank line before the body");
        let value: serde_json::Value = serde_yaml::from_str(yaml).expect("front matter is YAML");
        assert_eq!(value["description"], "Quarterly results & outlook");
        assert_eq!(
            value["canonical"],
            "https://example.com/reports/q3?quarter=3&region=eu"
        );
        let cards = value["social"].as_array().expect("a card list");
        let keys: Vec<&str> = cards
            .iter()
            .map(|card| card["key"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            keys,
            vec![
                "og:title",
                "og:image",
                "og:image:width",
                "og:image",
                "twitter:card"
            ],
            "the cards are listed in the order the page stated them"
        );
        assert_eq!(cards[2]["content"], "1200");
        assert_eq!(cards[3]["content"], "https://example.com/q3@2x.png");
        assert_eq!(body, "# Q3 review\n");
    }

    #[test]
    fn headings_take_their_depth() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("Title", "title", None, None, None);
        b.add_labeled_item("One", "section_header", Some(1), None, None);
        b.add_labeled_item("Deep", "section_header", Some(3), None, None);
        assert_eq!(render(&b.build()), "# Title\n\n## One\n\n#### Deep\n");
    }

    #[test]
    fn list_markers_keep_what_the_document_printed() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("First", "list_item", None, Some("1.".into()), Some(true));
        b.add_labeled_item("Second", "list_item", None, Some("2.".into()), Some(true));
        b.add_labeled_item("Bullet", "list_item", None, Some("•".into()), Some(false));
        b.add_labeled_item("Roman", "list_item", None, Some("i.".into()), Some(true));
        b.add_labeled_item("Bare", "list_item", None, None, None);
        assert_eq!(
            render(&b.build()),
            "1. First\n\n2. Second\n\n- Bullet\n\n- i. Roman\n\n- Bare\n"
        );
    }

    /// A list item whose printed marker is the empty string — a deck's bullet
    /// is one — must not write that empty marker after the `-`: the extra space
    /// reads back as a different bullet, so the file changes on every save.
    #[test]
    fn an_empty_marker_leaves_one_space() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item(
            "Bullet",
            "list_item",
            None,
            Some(String::new()),
            Some(false),
        );
        assert_eq!(render(&b.build()), "- Bullet\n");
    }

    #[test]
    fn a_link_wraps_the_whole_item() {
        let url = Some("https://example.com/spec".to_string());
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item_with_link("Spec", "title", None, None, None, url.clone());
        b.add_labeled_item_with_link(
            "Details",
            "section_header",
            Some(1),
            None,
            None,
            url.clone(),
        );
        b.add_labeled_item_with_link("Read the spec", "paragraph", None, None, None, url.clone());
        b.add_labeled_item_with_link(
            "First",
            "list_item",
            None,
            Some("1.".into()),
            Some(true),
            url,
        );
        assert_eq!(
            render(&b.build()),
            "# [Spec](https://example.com/spec)\n\n\
             ## [Details](https://example.com/spec)\n\n\
             [Read the spec](https://example.com/spec)\n\n\
             1. [First](https://example.com/spec)\n"
        );
    }

    #[test]
    fn a_link_markdown_cannot_read_is_angle_bracketed() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item_with_link(
            "A spaced address",
            "paragraph",
            None,
            None,
            None,
            Some("https://example.com/a b".into()),
        );
        b.add_labeled_item_with_link(
            "A bracketed address",
            "paragraph",
            None,
            None,
            None,
            Some("https://example.com/x_(y)".into()),
        );
        b.add_labeled_item_with_link(
            "An angled address",
            "paragraph",
            None,
            None,
            None,
            Some("https://example.com/a<b>".into()),
        );
        assert_eq!(
            render(&b.build()),
            "[A spaced address](<https://example.com/a b>)\n\n\
             [A bracketed address](<https://example.com/x_(y)>)\n\n\
             [An angled address](<https://example.com/a\\<b\\>>)\n"
        );
    }

    #[test]
    fn a_blank_link_is_no_link() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item_with_link(
            "Goes nowhere",
            "paragraph",
            None,
            None,
            None,
            Some("  ".into()),
        );
        assert_eq!(render(&b.build()), "Goes nowhere\n");
    }

    #[test]
    fn a_link_inside_code_stays_text() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item_with_link(
            "see https://example.com",
            "code",
            None,
            None,
            None,
            Some("https://example.com".into()),
        );
        assert_eq!(render(&b.build()), "```\nsee https://example.com\n```\n");
    }

    #[test]
    fn tables_are_padded_and_have_a_header() {
        let mut b = DocumentBuilder::new("demo");
        b.add_table_plain(&[
            vec!["A".into(), "B".into()],
            vec!["long cell".into(), "x".into()],
        ]);
        assert_eq!(
            render(&b.build()),
            "| A         | B |\n|-----------|---|\n| long cell | x |\n"
        );
    }

    #[test]
    fn a_table_claiming_no_header_still_gets_one() {
        // The builder always marks row 0 a header, so this document is written
        // by hand: the renderer must not assume the flag is there.
        let doc: DoclingDocument = serde_json::from_str(
            r##"{
                "schema_name": "DoclingDocument", "version": "1.10.0", "name": "demo",
                "body": {"self_ref": "#/body", "children": [{"$ref": "#/tables/0"}],
                         "content_layer": "body", "name": "_root_", "label": "unspecified"},
                "tables": [{
                    "self_ref": "#/tables/0", "children": [], "content_layer": "body",
                    "label": "table", "prov": [],
                    "data": {"num_rows": 2, "num_cols": 1, "table_cells": [
                        {"text": "a", "start_row_offset_idx": 0, "end_row_offset_idx": 1,
                         "start_col_offset_idx": 0, "end_col_offset_idx": 1,
                         "row_span": 1, "col_span": 1, "column_header": false,
                         "row_header": false, "row_section": false},
                        {"text": "b", "start_row_offset_idx": 1, "end_row_offset_idx": 2,
                         "start_col_offset_idx": 0, "end_col_offset_idx": 1,
                         "row_span": 1, "col_span": 1, "column_header": false,
                         "row_header": false, "row_section": false}
                    ]}
                }]
            }"##,
        )
        .expect("a legal document");
        assert_eq!(render(&doc), "|   |\n|---|\n| a |\n| b |\n");
    }

    /// A table can name its columns below a first row that does not — a caption
    /// row, a unit row. The pipe table has one place for a header, so the row the
    /// document marked is written there and the row above it follows in the body:
    /// the names label the columns and no empty header row is invented.
    #[test]
    fn a_header_below_the_first_row_heads_the_pipe_table() {
        let mut b = DocumentBuilder::new("demo");
        let cell = |text: &str, row: usize, col: usize, span: usize, header: bool| GridCell {
            text: text.to_string(),
            start_row: row,
            start_col: col,
            row_span: 1,
            col_span: span,
            column_header: header,
            row_header: false,
            pictures: Vec::new(),
        };
        b.add_table_grid(&GridTable {
            num_rows: 3,
            num_cols: 2,
            cells: vec![
                cell("Monthly totals", 0, 0, 2, false),
                cell("Month", 1, 0, 1, true),
                cell("Total", 1, 1, 1, true),
                cell("January", 2, 0, 1, false),
                cell("10", 2, 1, 1, false),
            ],
        });
        assert_eq!(
            render(&b.build()),
            "| Month          | Total |\n\
             |----------------|-------|\n\
             | Monthly totals |       |\n\
             | January        | 10    |\n"
        );
    }

    #[test]
    fn a_spanning_cell_writes_once_and_covers_its_neighbours() {
        let doc: DoclingDocument = serde_json::from_str(
            r##"{
                "schema_name": "DoclingDocument", "version": "1.10.0", "name": "demo",
                "body": {"self_ref": "#/body", "children": [{"$ref": "#/tables/0"}],
                         "content_layer": "body", "name": "_root_", "label": "unspecified"},
                "tables": [{
                    "self_ref": "#/tables/0", "children": [], "content_layer": "body",
                    "label": "table", "prov": [],
                    "data": {"num_rows": 2, "num_cols": 2, "table_cells": [
                        {"text": "wide", "start_row_offset_idx": 0, "end_row_offset_idx": 1,
                         "start_col_offset_idx": 0, "end_col_offset_idx": 2,
                         "row_span": 1, "col_span": 2, "column_header": true,
                         "row_header": false, "row_section": false},
                        {"text": "a", "start_row_offset_idx": 1, "end_row_offset_idx": 2,
                         "start_col_offset_idx": 0, "end_col_offset_idx": 1,
                         "row_span": 1, "col_span": 1, "column_header": false,
                         "row_header": false, "row_section": false},
                        {"text": "b", "start_row_offset_idx": 1, "end_row_offset_idx": 2,
                         "start_col_offset_idx": 1, "end_col_offset_idx": 2,
                         "row_span": 1, "col_span": 1, "column_header": false,
                         "row_header": false, "row_section": false}
                    ]}
                }]
            }"##,
        )
        .expect("a legal document");
        assert_eq!(render(&doc), "| wide |   |\n|------|---|\n| a    | b |\n");
    }

    #[test]
    fn a_pipe_in_a_cell_does_not_break_the_row() {
        let mut b = DocumentBuilder::new("demo");
        b.add_table_plain(&[vec!["a|b".into()], vec!["c".into()]]);
        let md = render(&b.build());
        assert!(md.contains("a\\|b"), "{md}");
    }

    /// A cell a Word document fills with a picture holds it in the model, so the
    /// Markdown writer has to write it where the cell is: inside the pipe row,
    /// after the cell's words. A writer that carried the picture as a block of
    /// its own would put an image beside the table, where the document has none.
    #[test]
    fn a_picture_a_cell_holds_is_written_in_the_cell() {
        let mut b = DocumentBuilder::new("demo");
        b.add_table_grid(&GridTable {
            num_rows: 1,
            num_cols: 2,
            cells: vec![
                GridCell {
                    text: "Before".into(),
                    start_row: 0,
                    start_col: 0,
                    row_span: 1,
                    col_span: 1,
                    column_header: true,
                    row_header: false,
                    pictures: Vec::new(),
                },
                GridCell {
                    text: String::new(),
                    start_row: 0,
                    start_col: 1,
                    row_span: 1,
                    col_span: 1,
                    column_header: true,
                    row_header: false,
                    pictures: vec![PictureData {
                        mimetype: "image/png".into(),
                        width: Some(60),
                        height: Some(36),
                        dpi: Some(96),
                        uri: "demo_images/pg1_1.png".into(),
                    }],
                },
            ],
        });
        let md = render(&b.build());
        assert_eq!(md.matches("![Image]").count(), 1, "{md}");
        let line = md
            .lines()
            .find(|line| line.contains("![Image]"))
            .expect("the picture is rendered");
        assert!(
            line.starts_with("| Before") && line.contains("![Image](demo_images/pg1_1.png)"),
            "the picture is not in its cell: {md}"
        );
    }

    #[test]
    fn code_fences_survive_code() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_text("fn main() {}", "code");
        assert_eq!(render(&b.build()), "```\nfn main() {}\n```\n");

        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_text("``` already fenced ```", "code");
        let md = render(&b.build());
        assert!(md.starts_with("````\n"), "{md}");
    }

    #[test]
    fn formulas_become_display_blocks() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_text("E = mc^2", "formula");
        assert_eq!(render(&b.build()), "$$\nE = mc^2\n$$\n");
    }

    #[test]
    fn furniture_and_empty_blocks_leave_no_trace() {
        let mut b = DocumentBuilder::new("demo");
        b.add_text_block("Body", 1, [0.0, 0.0, 1.0, 1.0], "paragraph");
        b.add_furniture_on_page("Running head", 1, [0.0, 0.0, 1.0, 1.0], "page_header");
        b.add_text_block("   ", 1, [0.0, 0.0, 1.0, 1.0], "paragraph");
        assert_eq!(render(&b.build()), "Body\n");
    }

    #[test]
    fn a_picture_points_at_its_image_or_says_nothing() {
        let mut b = DocumentBuilder::new("demo");
        b.add_picture(
            1,
            [0.0, 0.0, 10.0, 10.0],
            "picture",
            Some(PictureData {
                mimetype: "image/png".into(),
                width: Some(2),
                height: Some(2),
                dpi: Some(72),
                uri: "demo_images/pg1_1.png".into(),
            }),
        );
        b.add_picture(1, [0.0, 0.0, 10.0, 10.0], "picture", None);
        assert_eq!(render(&b.build()), "![Image](demo_images/pg1_1.png)\n");
    }

    /// A picture's file name is the page's own, spaces and all, so the
    /// destination is written in the `<…>` form when a bare one would end at
    /// the space or at a parenthesis. `&` needs no bracket and stays as it is.
    #[test]
    fn a_picture_with_a_spaced_name_is_angle_bracketed() {
        let mut b = DocumentBuilder::new("demo");
        for uri in [
            "images/scan & notes.jpg",
            "images/scan (1).jpg",
            "images/a<b>c.jpg",
            "a&b.png",
        ] {
            b.add_picture(
                1,
                [0.0, 0.0, 10.0, 10.0],
                "picture",
                Some(PictureData {
                    mimetype: "image/jpeg".into(),
                    width: Some(2),
                    height: Some(2),
                    dpi: None,
                    uri: uri.into(),
                }),
            );
        }
        assert_eq!(
            render(&b.build()),
            "![Image](<images/scan & notes.jpg>)\n\n\
             ![Image](<images/scan (1).jpg>)\n\n\
             ![Image](<images/a\\<b\\>c.jpg>)\n\n\
             ![Image](a&b.png)\n"
        );
    }

    /// A name can need both things at once: the `<…>` form, because a space
    /// would otherwise end the address and start a title, and the backslash
    /// escapes Markdown requires inside it for `<`, `>` and a backslash itself.
    /// `a_picture_with_a_spaced_name_is_angle_bracketed` holds each on its own —
    /// `images/scan & notes.jpg` for the wrapper, `images/a<b>c.jpg` for the
    /// escapes — and the `picture_names.html` golden holds a real file name with
    /// a space. The pair together is held here, because a writer that escaped
    /// only when no space was present would pass all of those.
    #[test]
    fn a_picture_name_is_wrapped_and_escaped_at_once() {
        let doc = picture_doc("images/Q3 chart <final>\\graph.gif", "image/gif");
        assert_eq!(
            render(&doc),
            "![Image](<images/Q3 chart \\<final\\>\\\\graph.gif>)\n"
        );
    }

    #[test]
    fn paragraphs_of_many_lines_stay_one_line() {
        let mut b = DocumentBuilder::new("demo");
        b.add_text_block("one\ntwo", 1, [0.0, 0.0, 1.0, 1.0], "paragraph");
        assert_eq!(render(&b.build()), "one two\n");
    }

    #[test]
    fn an_empty_document_renders_empty() {
        let b = DocumentBuilder::new("demo");
        assert_eq!(render(&b.build()), "");
    }

    #[test]
    fn export_writes_where_asked_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("pdfe-md-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("out.md");

        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("Title", "title", None, None, None);
        export_to_file(&b.build(), &path).expect("export");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Title\n");
        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["out.md".to_string()], "left {entries:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A picture with bytes on disk beside the document.
    fn picture_doc(uri: &str, mimetype: &str) -> DoclingDocument {
        let mut b = DocumentBuilder::new("demo");
        b.add_picture(
            1,
            [0.0, 0.0, 10.0, 10.0],
            "picture",
            Some(PictureData {
                mimetype: mimetype.into(),
                width: Some(2),
                height: Some(2),
                dpi: Some(72),
                uri: uri.into(),
            }),
        );
        b.build()
    }

    /// The three ways a Markdown reading can treat a picture: the file beside
    /// the document, its own bytes inside the document, or a mark where it
    /// stood. The embedded bytes are the file's, under the file's own media
    /// type — not a re-encoding.
    #[test]
    fn pictures_are_referenced_embedded_or_marked() {
        let dir = std::env::temp_dir().join(format!("pdfe-md-images-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("demo_images")).expect("scratch dir");
        let bytes = b"\x89PNG\r\n\x1a\nnot quite a png";
        std::fs::write(dir.join("demo_images/pg1_1.png"), bytes).expect("write image");

        let doc = picture_doc("demo_images/pg1_1.png", "image/png");
        assert_eq!(
            render_with_images(&doc, false, ImageMode::Referenced, Some(&dir)),
            "![Image](demo_images/pg1_1.png)\n"
        );

        let embedded = render_with_images(&doc, false, ImageMode::Embedded, Some(&dir));
        assert_eq!(
            embedded,
            format!(
                "![Image](data:image/png;base64,{})\n",
                STANDARD.encode(bytes)
            )
        );
        assert!(
            !embedded.contains("demo_images"),
            "the embedded page still points at a file: {embedded}"
        );

        assert_eq!(
            render_with_images(&doc, false, ImageMode::Placeholder, Some(&dir)),
            "<!-- image -->\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `uri` that is already a `data:` URI is carried as written: reading an
    /// embedded page back and rendering it embedded again must not double-encode
    /// it, and dropping the base directory is not a reason to lose the picture.
    #[test]
    fn an_already_embedded_picture_is_kept_as_written() {
        let uri = "data:image/png;base64,iVBORw0KGgo=";
        let doc = picture_doc(uri, "image/png");
        assert_eq!(
            render_with_images(&doc, false, ImageMode::Embedded, None),
            format!("![Image]({uri})\n")
        );
    }

    /// A picture whose bytes are not there cannot be embedded, and a drawing
    /// with no image at all has nothing to carry: both mark the spot rather
    /// than emit a link a reader could not follow. Referenced keeps the old
    /// behaviour, where a drawing with no image leaves nothing.
    #[test]
    fn a_picture_with_no_bytes_is_a_placeholder_not_a_broken_link() {
        let doc = picture_doc("demo_images/gone.png", "image/png");
        assert_eq!(
            render_with_images(&doc, false, ImageMode::Embedded, None),
            "<!-- image -->\n"
        );

        let mut b = DocumentBuilder::new("demo");
        b.add_picture(1, [0.0; 4], "picture", None);
        let drawing = b.build();
        assert_eq!(
            render_with_images(&drawing, false, ImageMode::Embedded, None),
            "<!-- image -->\n"
        );
        assert_eq!(
            render_with_images(&drawing, false, ImageMode::Placeholder, None),
            "<!-- image -->\n"
        );
        assert_eq!(render(&drawing), "", "a referenced drawing writes nothing");
    }
}
