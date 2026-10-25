//! Render a [`DoclingDocument`] as a standalone HTML page.
//!
//! The same walk as the Markdown writer decides what is visited and in what
//! order — furniture stays out, `body` content comes in reading order, and the
//! notes layer joins when the reader asks for it — and this module decides what
//! each item becomes as HTML:
//!
//! * **Tables keep their merges.** This is the reason HTML exists alongside
//!   Markdown: a spanning cell is written once with `rowspan`/`colspan` and
//!   the cells it covers are not written at all, so the grid a browser lays
//!   out is the grid the document printed. The row that claims the header
//!   becomes `<thead>` with `<th>` wherever in the table it sits — the rows
//!   above it follow in `<tbody>`, since HTML has one place for a header —
//!   and a table that claims no header gets no `<thead>`, because an invented
//!   header is a claim about the document.
//! * **Headings** are `h1`–`h6`: `title` is `h1`, a `section_header` is its
//!   depth plus one — the same mapping the Markdown `#`s follow, clamped
//!   where Markdown clamps.
//! * **List items** run together into one `<ul>`/`<ol>`: a run whose markers
//!   the document printed as numbers becomes `<ol>` with each number kept in
//!   `value` (a list that started at `2)` starts at 2), everything else a
//!   `<ul>` whose text keeps a roman or lettered marker and drops a bullet
//!   glyph — exactly the distinction the Markdown writer makes.
//! * **Equations** are typeset: the LaTeX the document holds is written as
//!   MathML, the markup a browser draws without a script, inside a
//!   `<div class="formula">`; **code** is a `<pre><code>` block. The equation's
//!   own TeX travels with it in the MathML `<annotation>`, so the page says both
//!   what the equation reads and how it is written, and the reader takes it back
//!   from there unchanged. Docling's own OMML accent spellings (`\ocirc` for a
//!   ring, `\wideutilde` for a tilde below, `\dddot` for three dots) are
//!   re-spelled for the renderer — as the mark's shape where LaTeX has no name
//!   for it — so a Docling equation is typeset rather than drawn. A formula
//!   whose LaTeX this
//!   cannot read at all is drawn instead: the writer lays the equation out itself
//!   and carries it as an SVG `data:` URI, whose `alt` text is that LaTeX, so the
//!   reader sees the equation and the page still reads back as the formula item
//!   it came from. Losing an equation to a converter, or printing the TeX in its
//!   place, are both worse than drawing it. Inline math
//!   (`$…$` inside a sentence) is a `<span class="formula">` holding the same
//!   MathML — or the same drawing — where it stands, so the sentence stays one
//!   paragraph: the delimiters are markup, not words. A lone dollar, an escaped
//!   `\$`, and a price are left as text.
//!   **Pictures** with an image are a `<p><img …></p>` (a drawing with no file to
//!   point at writes nothing, rather than a broken image).
//! * A **quotation** is a `<blockquote>` around the `<p>` its words make — the
//!   element the reader needs to keep it a quotation — because Docling's model
//!   keeps the paragraph and the quoting travels beside it.
//! * **Links** become `<a href="url">text</a>`. The document keeps a link on
//!   the whole item — a PDF `/Link` annotation over the words, a DOCX
//!   `w:hyperlink` around them, an HTML `a` holding them — so the whole text
//!   is the anchor. The address and the text are escaped like any other text.
//! * **Groups** are marked with the same invisible HTML comments Markdown uses
//!   (`<!-- pdfextractor:group … -->`). HTML has `<section>`, but a section
//!   would claim structure the reader would have to guess back from nesting;
//!   the comment says which group it is, and the HTML reader rebuilds the same
//!   slides and comment sections the Markdown reader does.
//! * Everything else is `<p>`, with the text HTML-escaped.
//! * A **notes-layer** item (written only when the reader asks for a deck's
//!   remarks) is an `<aside class="notes">` holding one `<p>` per paragraph,
//!   the first labelled `Notes:` and — when the item has a link — carrying an
//!   `<a href>` around its words, the anchor the HTML reader recovers the
//!   note's address from. A comment is `<aside class="notes comment">` labelled
//!   `Comment:` instead, so a reviewer's remark is not read as the presenter's.
//!   Presenter remarks therefore sit visibly beside the slides rather than
//!   reading as more slide content, and their paragraph breaks are kept.
//!
//! The output is a whole page — doctype, `utf-8`, `<title>` from the
//! document's own title, so a file written with `--to html` opens on its own.
//! The title is content, not the name of the file that was read: the first
//! `title` item in the body, which is also the `<h1>` the page opens with. A
//! document with no title gets an empty `<title></title>` rather than the input
//! file's name, because a page has to re-render to the same bytes — a title
//! taken from the name would change the moment the page was read back under
//! another name.
//!
//! What the page says about *itself* in that head — its `<meta
//! name="description">`, its `<link rel="canonical">`, and the Open Graph and
//! Twitter card statements a link preview is built from — is written when the
//! document came from a page that stated them, and left out otherwise. Docling
//! has no field for any of it, so it travels beside the model rather than in its
//! JSON (see `DoclingDocument::html_head`); a page read as HTML and written as
//! HTML therefore keeps it, and a document converted from anything else gains no
//! claim it never made.

use crate::formula_image;
use crate::walk::{walk_body_with, Visitor};
use crate::{
    image_data_uri, inline, is_bullet_glyph, ImageMode, GROUP_END, GROUP_START, IMAGE_PLACEHOLDER,
};
use pdfextractor_core::{DoclingDocument, Group, PictureItem, TableItem, TextItem};
use pulldown_latex::config::DisplayMode;
use pulldown_latex::{Parser, RenderConfig, Storage};
use std::path::{Path, PathBuf};

/// Render `doc` as a standalone HTML page.
pub fn render(doc: &DoclingDocument) -> String {
    render_with_notes(doc, false)
}

/// Render `doc` as HTML, including the notes layer when `include_notes` — a
/// deck's speaker notes after their slide, its comments after the slides.
pub fn render_with_notes(doc: &DoclingDocument, include_notes: bool) -> String {
    render_with_images(doc, include_notes, ImageMode::Referenced, None)
}

/// Render `doc` as HTML with the reader's choices: whether the notes layer
/// joins the reading, how pictures are referred to, and — for
/// [`ImageMode::Embedded`] — the directory a picture's `uri` is relative to.
pub fn render_with_images(
    doc: &DoclingDocument,
    include_notes: bool,
    image_mode: ImageMode,
    image_base: Option<&Path>,
) -> String {
    let mut renderer = Renderer {
        blocks: Vec::new(),
        list: None,
        image_mode,
        image_base: image_base.map(Path::to_path_buf),
        groups: Vec::new(),
        title: None,
        description: doc.html_head.description.clone(),
        canonical: doc.html_head.canonical.clone(),
        social: doc.html_head.social.clone(),
        doc,
    };
    walk_body_with(doc, include_notes, &mut renderer);
    renderer.finish()
}

/// Write `doc` as an HTML page to `path`.
///
/// The bytes land in a sibling temporary file renamed into place, as every
/// other writer does: a stopped run leaves whole files or none.
pub fn export_to_file(doc: &DoclingDocument, path: &Path) -> anyhow::Result<()> {
    export_to_file_with_notes(doc, path, false)
}

/// Write `doc` as HTML to `path`, including the notes layer when
/// `include_notes`.
pub fn export_to_file_with_notes(
    doc: &DoclingDocument,
    path: &Path,
    include_notes: bool,
) -> anyhow::Result<()> {
    export_to_file_with_images(doc, path, include_notes, ImageMode::Referenced)
}

/// Write `doc` as HTML to `path` with an [`ImageMode`].
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
    let html = render_with_images(doc, include_notes, image_mode, base);

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.html".to_string());
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));

    std::fs::write(&temp, html)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

struct Renderer<'a> {
    /// One entry per rendered block, one per line of output.
    blocks: Vec<String>,
    /// The list being filled, if a list item is the last thing visited:
    /// whether it is `<ol>` (every marker printed a number) and the `<li>`
    /// lines so far.
    list: Option<(bool, Vec<String>)>,
    /// How a picture is referred to.
    image_mode: ImageMode,
    /// Where a picture's relative `uri` is read from when embedding.
    image_base: Option<PathBuf>,
    /// The block position each open group's marker was written at, so a group
    /// whose items were all left out (a comment read without `--include-notes`)
    /// does not leave a marker pair behind — and whether the group is a
    /// comment's section, whose asides are labelled as comments.
    groups: Vec<(usize, bool)>,
    /// The page's `<title>`: the words of the first `title` item in the body,
    /// kept as it is written in the `<h1>`. Taken from the document rather
    /// than the file's name so the page re-renders to the same bytes when it
    /// is read back under another name.
    title: Option<String>,
    /// The page's own description, when the document came from a page that
    /// stated one: `<meta name="description">` in the head, which is a fact
    /// about the page rather than a block on it.
    description: Option<String>,
    /// The canonical URL the page named, when it named one.
    canonical: Option<String>,
    /// The Open Graph and Twitter card statements the page made, in the order
    /// it made them: a key and its content, written back under the attribute
    /// the key's vocabulary uses.
    social: Vec<(String, String)>,
    /// The document being written, so a table cell can look up the pictures its
    /// own `ref` names.
    doc: &'a DoclingDocument,
}

impl Renderer<'_> {
    fn block(&mut self, text: String) {
        if !text.trim().is_empty() {
            self.blocks.push(text);
        }
    }

    /// Flush any list being filled — HTML lists are real elements, so they
    /// must close before the next block can open.
    fn close_list(&mut self) {
        if let Some((ordered, items)) = self.list.take() {
            let tag = if ordered { "ol" } else { "ul" };
            self.blocks
                .push(format!("<{tag}>\n{}\n</{tag}>", items.join("\n")));
        }
    }

    fn heading(&mut self, depth: i32, item: &TextItem) {
        self.close_list();
        // h1–h6 is the whole range HTML has.
        let level = depth.clamp(1, 6);
        self.block(format!("<h{level}>{}</h{level}>", inline_html(item)));
    }

    fn paragraph(&mut self, item: &TextItem) {
        self.close_list();
        self.block(format!("<p>{}</p>", inline_html(item)));
    }

    /// A quotation, as a `<blockquote>` around the paragraph its words make.
    ///
    /// The element is HTML's own way of saying these words are not the
    /// document's, and it is all the reader needs to put the quote back: the
    /// paragraph inside it is escaped and linked exactly as any other.
    fn quotation(&mut self, item: &TextItem) {
        self.close_list();
        self.block(format!(
            "<blockquote><p>{}</p></blockquote>",
            inline_html(item)
        ));
    }

    /// A notes-layer item, as an `<aside>` that carries its own label.
    ///
    /// The aside says, semantically, that this is beside the document rather
    /// than part of it; the bold `Notes:` says the same thing to a reader with
    /// no stylesheet. Comments are notes-layer items too and keep their own
    /// `[author: …, time: …]:` prefix inside the same aside.
    ///
    /// Each of the note's paragraphs is its own `<p>`, so the speaker's
    /// paragraph breaks survive; only the first carries the label, and the
    /// aside itself is what marks all of them as remarks.
    fn notes(&mut self, item: &TextItem) {
        self.close_list();
        let paragraphs = crate::note_paragraphs(&item.text);
        if paragraphs.is_empty() {
            return;
        }
        // A comment gets its own label and class: it is a reviewer talking
        // about the slide, and a reader has to be able to tell it from the
        // presenter's own note once the page is read back.
        let comment = self.groups.last().is_some_and(|(_, comment)| *comment);
        let (class, label) = if comment {
            ("notes comment", "Comment:")
        } else {
            ("notes", "Notes:")
        };
        let mut inner = String::new();
        for (index, paragraph) in paragraphs.iter().enumerate() {
            let body = match item.label.as_str() {
                "code" => format!("<pre><code>{}</code></pre>", escape(paragraph)),
                "formula" => formula_html(paragraph.trim()),
                _ => {
                    let text = inline_html_text(paragraph);
                    // The hyperlink belongs to the item, so it is written once,
                    // on the paragraph the item opens with.
                    match item
                        .hyperlink
                        .as_deref()
                        .map(str::trim)
                        .filter(|url| !url.is_empty())
                    {
                        Some(url) if index == 0 => {
                            format!("<a href=\"{}\">{text}</a>", escape(url))
                        }
                        _ => text,
                    }
                }
            };
            if index == 0 {
                inner.push_str(&format!("<p><strong>{label}</strong> {body}</p>\n"));
            } else {
                inner.push_str(&format!("<p>{body}</p>\n"));
            }
        }
        self.block(format!("<aside class=\"{class}\">\n{inner}</aside>"));
    }

    /// Mark the start of a group with the comment the Markdown writer uses.
    ///
    /// HTML comments render as nothing, and the reader already has to ignore
    /// them, so the marker the two writers share is what says which items
    /// belong to one slide — or to one comment's section.
    fn open_group(&mut self, group: &Group) {
        self.close_list();
        self.groups
            .push((self.blocks.len(), group.label == "comment_section"));
        self.block(format!(
            "{GROUP_START}{} {} {} -->",
            group.label, group.content_layer, group.name
        ));
    }

    /// Close the innermost group, or drop its marker when nothing was
    /// rendered inside it — the same rule the Markdown writer follows.
    fn close_group(&mut self) {
        self.close_list();
        let Some((at, _)) = self.groups.pop() else {
            return;
        };
        if self.blocks.len() == at + 1 {
            self.blocks.pop();
        } else {
            self.block(GROUP_END.to_string());
        }
    }

    fn code(&mut self, text: &str) {
        self.close_list();
        // The code is kept exactly, only escaped; `<pre>` preserves its
        // layout the way a fence does in Markdown.
        self.block(format!("<pre><code>{}</code></pre>", escape(text)));
    }

    fn formula(&mut self, text: &str) {
        self.close_list();
        self.block(formula_html(text.trim()));
    }

    /// One picture as an `<img>`, with no paragraph around it: a block wraps
    /// this in `<p>`, a table cell puts it where the cell's words are.
    fn picture_tag(&self, item: &PictureItem) -> Option<String> {
        match self.image_mode {
            ImageMode::Referenced => {
                let uri = item.image.as_ref().and_then(|image| image.uri.as_deref())?;
                Some(format!("<img src=\"{}\" alt=\"Image\">", escape(uri)))
            }
            ImageMode::Embedded => {
                let data = item
                    .image
                    .as_ref()
                    .and_then(|image| image_data_uri(image, self.image_base.as_deref()));
                match data {
                    Some(uri) => Some(format!("<img src=\"{}\" alt=\"Image\">", escape(&uri))),
                    // No image to carry: mark the spot, as the Markdown writer
                    // does, rather than emit an `<img>` with no source.
                    None => Some(IMAGE_PLACEHOLDER.to_string()),
                }
            }
            // An HTML comment is invisible, which is what a placeholder is for:
            // the spot stays in the source, and no broken image reaches a page.
            ImageMode::Placeholder => Some(IMAGE_PLACEHOLDER.to_string()),
        }
    }

    fn picture(&mut self, item: &PictureItem) {
        self.close_list();
        let Some(tag) = self.picture_tag(item) else {
            return;
        };
        // A placeholder is an HTML comment, not an element: wrapping it in a
        // paragraph would put a paragraph on the page that the page does not
        // have, and the page would no longer re-render to its own bytes.
        if tag.starts_with("<img") {
            self.block(format!("<p>{tag}</p>"));
        } else {
            self.block(tag);
        }
    }

    /// The pictures a table cell holds, as `<img>` elements in the cell's own
    /// order. A Word cell's picture is the cell's, so it reads inside its
    /// `<td>` rather than as a block beside the table.
    fn cell_pictures(&self, cell: &pdfextractor_core::TableCell) -> String {
        crate::cell_pictures(self.doc, cell)
            .iter()
            .filter_map(|picture| self.picture_tag(picture))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn list_item(&mut self, item: &TextItem) {
        let marker = item.marker.as_deref();
        let ordered = marker.is_some_and(|m| m.starts_with(|c: char| c.is_ascii_digit()));
        let text = inline_html(item);

        // A run of the same kind is one list; a change of kind closes the old
        // one, so a roman marker in the middle of a numbered list becomes its
        // own `<ul>` — the same information the Markdown writer keeps, just
        // in elements.
        let continues = matches!(&self.list, Some((open, _)) if *open == ordered);
        if !continues {
            self.close_list();
            self.list = Some((ordered, Vec::new()));
        }
        let line = match marker {
            // The number the document printed stays on the item, so a list
            // that started at `2)` still reads from 2 after an `<ol>` would
            // have renumbered it. A marker `value` cannot spell — the `1.1.` of
            // a multi-level list — is kept whole, or reading the page back
            // would renumber the list as 1, 2, 3.
            Some(m) if ordered => {
                let value = leading_number(m);
                if m.trim() == format!("{value}.") {
                    format!("<li value=\"{value}\">{text}</li>")
                } else {
                    format!(
                        "<li value=\"{value}\" data-marker=\"{}\">{text}</li>",
                        escape(m.trim())
                    )
                }
            }
            Some(m) if is_bullet_glyph(m) => format!("<li>{text}</li>"),
            // A roman or lettered marker is kept as the item's own first word.
            Some(m) => format!("<li>{} {text}</li>", escape(m)),
            None => format!("<li>{text}</li>"),
        };
        if let Some((_, items)) = &mut self.list {
            items.push(line);
        }
    }

    fn table(&mut self, table: &TableItem) {
        self.close_list();
        let rendered = table_html(table, |cell| self.cell_pictures(cell));
        if let Some(html) = rendered {
            self.block(html);
        }
    }

    /// Assemble the page: any open list closes last, then the whole document.
    fn finish(mut self) -> String {
        self.close_list();
        let body = self.blocks.join("\n");

        let mut out = String::new();
        out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
        out.push_str("<meta charset=\"utf-8\">\n");
        out.push_str(&format!(
            "<title>{}</title>\n",
            escape(self.title.as_deref().unwrap_or(""))
        ));
        // What the page said about itself, when it said anything: a document
        // read from HTML keeps the description and canonical URL it came with,
        // so a read-write round trip neither invents nor loses them. A document
        // from a format with no head writes neither line, which is the head a
        // conversion has always written.
        if let Some(description) = &self.description {
            out.push_str(&format!(
                "<meta name=\"description\" content=\"{}\">\n",
                escape(description)
            ));
        }
        if let Some(canonical) = &self.canonical {
            out.push_str(&format!(
                "<link rel=\"canonical\" href=\"{}\">\n",
                escape(canonical)
            ));
        }
        // The cards, in the order the page stated them: a `twitter:` key goes
        // back in `name`, the way Twitter's own documentation writes it, and
        // everything else in `property`, which is where Open Graph requires it.
        // A width statement therefore stays beside the image it describes.
        for (key, content) in &self.social {
            let attribute = if key.to_ascii_lowercase().starts_with("twitter:") {
                "name"
            } else {
                "property"
            };
            out.push_str(&format!(
                "<meta {attribute}=\"{}\" content=\"{}\">\n",
                escape(key),
                escape(content)
            ));
        }
        out.push_str("</head>\n<body>\n");
        if !body.is_empty() {
            out.push_str(&body);
            out.push('\n');
        }
        out.push_str("</body>\n</html>\n");
        out
    }
}

impl Visitor for Renderer<'_> {
    fn text(&mut self, _index: usize, item: &TextItem) {
        // A deck's remarks stay on the page when the reader asks for them, but
        // they are not the slide: an aside, labelled `Notes:`, is the document
        // talking about itself rather than what it shows.
        if item.content_layer == "notes" {
            self.notes(item);
            return;
        }
        // The first title the body carries is the page's title — the same
        // item that becomes the `<h1>`. A later one is just another heading.
        // The ends are trimmed as a reader trims the heading's words, so a
        // title with stray padding writes once and stays written.
        if item.label == "title" && self.title.is_none() {
            self.title = Some(item.text.trim().to_string());
        }
        match item.label.as_str() {
            "title" => self.heading(1, item),
            "section_header" => {
                let depth = item.level.unwrap_or(1).clamp(1, 5);
                self.heading(depth + 1, item);
            }
            "list_item" => self.list_item(item),
            // Code and formulas are literal: an address inside one is text the
            // document printed, not a link it made.
            "code" => self.code(&item.text),
            "formula" => self.formula(&item.text),
            // A quotation is a paragraph the document set apart: the element
            // says which, and the paragraph inside it is still a paragraph.
            _ if item.quote => self.quotation(item),
            _ => self.paragraph(item),
        }
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

/// The table as `<table>…</table>`, with the merges the Markdown format has
/// to flatten kept here as `rowspan`/`colspan`.
///
/// `None` when the table has no rows or no columns: there is no grid to draw
/// and an empty `<table>` on the page would be noise.
fn table_html(
    table: &TableItem,
    extra: impl Fn(&pdfextractor_core::TableCell) -> String,
) -> Option<String> {
    let rows = table.data.num_rows;
    let cols = table.data.num_cols;
    if rows == 0 || cols == 0 {
        return None;
    }

    // Cells by their starting row, left to right, and the slots already
    // covered by an earlier cell's span — a cell under a merge is not written
    // twice.
    let mut by_row: Vec<Vec<_>> = vec![Vec::new(); rows];
    for cell in &table.data.table_cells {
        if cell.start_row_offset_idx < rows && cell.start_col_offset_idx < cols {
            by_row[cell.start_row_offset_idx].push(cell);
        }
    }
    for cells in &mut by_row {
        cells.sort_by_key(|cell| cell.start_col_offset_idx);
    }
    let mut covered = vec![vec![false; cols]; rows];

    // A header only when the document claims one — the same rule the Markdown
    // writer obeys, because inventing a `<th>` would claim a header the
    // document never printed. HTML has one place for a header, so the marked
    // row is written in `<thead>` wherever in the grid it sat, and the rows
    // that came before it follow it in `<tbody>` in their own order.
    let header = by_row
        .iter()
        .position(|cells| cells.iter().any(|cell| cell.column_header));

    let render_row = |covered: &mut Vec<Vec<bool>>, cells: &[&pdfextractor_core::TableCell]| {
        let mut line = String::from("<tr>");
        for cell in cells {
            let (r, c) = (cell.start_row_offset_idx, cell.start_col_offset_idx);
            if covered[r][c] {
                continue;
            }
            let rowspan = cell.row_span.max(1) as usize;
            let colspan = cell.col_span.max(1) as usize;
            for row in covered.iter_mut().take(rows.min(r + rowspan)).skip(r) {
                for taken in row.iter_mut().take(cols.min(c + colspan)).skip(c) {
                    *taken = true;
                }
            }
            let tag = if cell.column_header { "th" } else { "td" };
            let span = (if rowspan > 1 {
                format!(" rowspan=\"{rowspan}\"")
            } else {
                String::new()
            }) + &(if colspan > 1 {
                format!(" colspan=\"{colspan}\"")
            } else {
                String::new()
            });
            let mut content = inline_html_text(&flatten(&cell.text));
            let held = extra(cell);
            if !held.is_empty() {
                if !content.is_empty() {
                    content.push(' ');
                }
                content.push_str(&held);
            }
            line.push_str(&format!("<{tag}{span}>{content}</{tag}>"));
        }
        line.push_str("</tr>");
        line
    };

    // Each row is rendered where it sits in the grid, so a merge is read in
    // grid order; the assembled page may then put the header row first.
    let rendered: Vec<String> = by_row
        .iter()
        .map(|cells| render_row(&mut covered, cells))
        .collect();

    let mut out = String::from("<table>\n");
    if let Some(header) = header {
        out.push_str("<thead>\n");
        out.push_str(&rendered[header]);
        out.push_str("\n</thead>\n");
    }
    let body: Vec<usize> = (0..rows).filter(|row| Some(*row) != header).collect();
    if !body.is_empty() {
        out.push_str("<tbody>\n");
        for row in body {
            out.push_str(&rendered[row]);
            out.push('\n');
        }
        out.push_str("</tbody>\n");
    }
    out.push_str("</table>");
    Some(out)
}

/// A cell's text as it reads in a cell: its lines kept on one line, trimmed,
/// since a cell is one value however the PDF wrapped it.
/// A cell's words as the page can say them: one line, and every run of
/// whitespace made one space, which is exactly what the HTML reader gives back
/// for a cell. A cell is the one piece of text a page cannot spell with two
/// spaces in it — the reader collapses them as a browser would — so writing the
/// double space would be writing a page that changes when it is read. Prose
/// outside a cell keeps its spacing: the reader keeps it too.
fn flatten(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The number a printed marker carries: `2)` starts at 2, `12.` at 12.
fn leading_number(marker: &str) -> u32 {
    marker
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(1)
}

/// A text item as inline HTML: escaped, and wrapped in an anchor when the
/// document holds a link for it.
///
/// The model keeps one link per item (`hyperlink`), so the item's whole text
/// is the anchor's label. A blank address is no link — `<a href="">` goes
/// nowhere the document did — so the text stays plain, as in the Markdown
/// writer.
fn inline_html(item: &TextItem) -> String {
    let text = inline_html_text(&inline(&item.text));
    match item
        .hyperlink
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        Some(url) => format!("<a href=\"{}\">{text}</a>", escape(url)),
        None => text,
    }
}

/// One piece of a line: words, or the TeX between a matched pair of dollars.
#[derive(Debug, PartialEq, Eq)]
enum InlinePiece<'a> {
    Text(&'a str),
    Formula(&'a str),
}

/// A line as escaped HTML, with every inline formula a span in place.
///
/// The model has no field for an inline span, and splitting a sentence into
/// items to carry one would change the sentence: its words would become
/// separate blocks on the next render. The spans are therefore recognised
/// where the line is written, and only the delimiters disappear.
fn inline_html_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in inline_pieces(text) {
        match piece {
            InlinePiece::Text(words) => out.push_str(&escape(words)),
            InlinePiece::Formula(formula) => {
                out.push_str("<span class=\"formula\">");
                match mathml(formula, DisplayMode::Inline) {
                    Some(math) => out.push_str(&math),
                    None => match formula_image::drawing(formula) {
                        Some(drawing) => out.push_str(&formula_picture(formula, &drawing, true)),
                        None => out.push_str(&escape(formula)),
                    },
                }
                out.push_str("</span>");
            }
        }
    }
    out
}

/// Split a line at its inline math: single dollars that pair up, with no
/// whitespace just inside the delimiters and no digit just after the close.
///
/// The rules are Pandoc's, and they matter as much for what they leave alone
/// as for what they mark: a price (`$15,750`), an election (`$3 to this fund`)
/// or a lone dollar is text, not an equation. Two dollars in a row are display
/// math's delimiter — the Markdown reader has already made a block of it — so
/// they never open an inline span. A backslash escapes the dollar after it.
fn inline_pieces(text: &str) -> Vec<InlinePiece<'_>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut pieces = Vec::new();
    let mut text_start = 0usize;
    let mut i = 0usize;

    while i < chars.len() {
        let (byte, character) = chars[i];
        if character == '\\' {
            i = (i + 2).min(chars.len());
            continue;
        }
        if character != '$' {
            i += 1;
            continue;
        }
        // `$$` belongs to a display formula, not to a span inside prose.
        if chars.get(i + 1).is_some_and(|(_, next)| *next == '$') {
            i += 2;
            continue;
        }
        let Some(close) = inline_math_close(&chars, i) else {
            i += 1;
            continue;
        };

        if text_start < byte {
            pieces.push(InlinePiece::Text(&text[text_start..byte]));
        }
        // A dollar is one byte, so the body begins just after it.
        let body_start = byte + 1;
        let body_end = chars[close].0;
        pieces.push(InlinePiece::Formula(&text[body_start..body_end]));
        text_start = body_end + 1;
        i = close + 1;
    }

    if text_start < text.len() {
        pieces.push(InlinePiece::Text(&text[text_start..]));
    }
    pieces
}

/// The closing dollar for an opening one at `open`, or `None` when the pair is
/// not inline math.
fn inline_math_close(chars: &[(usize, char)], open: usize) -> Option<usize> {
    let first = open + 1;
    if chars.get(first).is_none_or(|(_, c)| c.is_whitespace()) {
        return None;
    }

    let mut i = first;
    while i < chars.len() {
        match chars[i].1 {
            '\\' => i += 2,
            '\n' | '\r' => return None,
            '$' => {
                let empty = i == first;
                let before = chars[i - 1].1;
                let after = chars.get(i + 1).map(|(_, c)| *c);
                let closed = !empty
                    && !before.is_whitespace()
                    && !after.is_some_and(|c| c.is_ascii_digit() || c == '$');
                return closed.then_some(i);
            }
            _ => i += 1,
        }
    }
    None
}

/// A display formula as the page shows it: typeset MathML when the LaTeX can be
/// read, and a drawing of the equation when it cannot. The
/// `<div class="formula">` is the writer's own marker either way, so a page
/// reads back as a formula even where no converter was willing to set it.
fn formula_html(latex: &str) -> String {
    match mathml(latex, DisplayMode::Block) {
        Some(math) => format!("<div class=\"formula\">{math}</div>"),
        None => match formula_image::drawing(latex) {
            Some(drawing) => format!(
                "<div class=\"formula\">{}</div>",
                formula_picture(latex, &drawing, false)
            ),
            // Nothing could be drawn — an empty formula, or one with no glyph
            // this reads — so the TeX stands where the equation belongs rather
            // than the page losing it.
            None => format!("<div class=\"formula\">{}</div>", escape(latex)),
        },
    }
}

/// The picture a fallback equation travels as.
///
/// The image *is* the equation, and its alt text is the TeX that drew it: a
/// reader takes the formula back from the alt text, so the page round-trips as
/// the formula item it came from instead of as a picture of one.
///
/// An inline equation is dropped by the picture's own descent so that it stands
/// on the text baseline beside it: an `<img>` is otherwise aligned by its bottom
/// edge, which would lift the equation off the line it belongs to.
fn formula_picture(latex: &str, drawing: &formula_image::Drawing, inline: bool) -> String {
    let align = if inline {
        format!(" style=\"vertical-align: -{}px\"", drawing.descent)
    } else {
        String::new()
    };
    format!(
        "<img src=\"{}\" alt=\"{}\"{align}>",
        drawing.uri,
        escape(latex)
    )
}

/// Docling's OMML accents in the LaTeX this writer can set.
///
/// Docling names each accent with the `unicode-math` spelling of the combining
/// character it draws: `\ocirc` for a ring above (U+030A), `\wideutilde` for a
/// tilde below (U+0330), `\leftharpoonaccent` for a harpoon over (U+20D0). Some
/// of those names are not standard LaTeX and the renderer does not read them,
/// which would leave a whole equation drawn as a picture because one accent
/// inside it was spelled that way. Each entry is the same accent spelled so it
/// can be set, and the formula's own text is untouched: `\wideutilde` ends with
/// the brace its argument follows, because `\underset` takes the accent first.
///
/// The rest of Docling's accents have no LaTeX name at all — a triple or a
/// quadruple dot, a vertical stroke over the letter, the actuarial bend. Each is
/// set with the construction that draws the mark: `\overset` of the shape that
/// stands above the letter, `\underset` of the one below, and, for a mark that
/// is only a stroke LaTeX does not have (`\ovhook`'s hook, `\candra`'s ring with
/// a tail, `\annuity`'s bend), the nearest accent it does — a hook above is set
/// as a breve. The page then holds an equation a reader can read rather than a
/// picture of one, and the annotation is still Docling's exact name, so the
/// formula reads back as it was written.
///
/// The accents the renderer already reads as Docling writes them (`\grave`,
/// `\acute`, `\hat`, `\tilde`, `\bar`, `\breve`, `\dot`, `\ddot`, `\check`,
/// `\vec`, `\overleftarrow`, `\overleftrightarrow`, `\underbar`,
/// `\underrightarrow`, `\not`, and the braces, brackets and parentheses over
/// and under) need no entry.
const DOCLING_ACCENTS: [(&str, &str); 20] = [
    // The same accent, under a name the renderer knows.
    ("overbar", "overline"),
    ("ocirc", "mathring"),
    ("leftharpoonaccent", "overleftharpoon"),
    ("rightharpoonaccent", "overrightharpoon"),
    ("wideutilde", "underset{\\sim}"),
    // Marks over the letter, as the shape they are.
    ("dddot", "overset{\\cdots}"),
    ("ddddot", "overset{\\cdot\\cdot\\cdot\\cdot}"),
    ("vertoverlay", "overset{\\mid}"),
    ("asteraccent", "overset{\\ast}"),
    ("widebridgeabove", "overset{\\frown}"),
    ("annuity", "overset{\\wedge}"),
    // Marks over the letter with a standard accent near enough to read as
    // them: the hook above as a breve, the candrabindu as a ring, the turned
    // and right commas as the strokes they look like, the left angle as a hat.
    ("ovhook", "breve"),
    ("candra", "mathring"),
    ("oturnedcomma", "grave"),
    ("ocommatopright", "acute"),
    ("droang", "hat"),
    // Marks under the letter.
    ("threeunderdot", "underset{\\cdots}"),
    ("underrightharpoondown", "underset{\\rightharpoondown}"),
    ("underleftharpoondown", "underset{\\leftharpoondown}"),
    // Docling's own spelling of `\underleftarrow`, typo included.
    ("underledtarrow", "underleftarrow"),
];

/// A formula with Docling's accent names spelled so they can be typeset, or the
/// formula itself when it holds none of them.
///
/// Only a name followed by `{` is a macro taking the accented expression, so
/// that is the only shape rewritten; everything else — other commands, their
/// arguments, the words of a `\text{…}` — is copied through unchanged.
fn settable_spelling(latex: &str) -> std::borrow::Cow<'_, str> {
    let mut out = String::new();
    let mut rest = latex;
    let mut rewritten = false;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let end = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let name = &rest[..end];
        let accented = rest[end..].starts_with('{');
        match DOCLING_ACCENTS.iter().find(|(docling, _)| *docling == name) {
            Some((_, standard)) if accented => {
                out.push('\\');
                out.push_str(standard);
                rewritten = true;
            }
            _ => {
                out.push('\\');
                out.push_str(name);
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    if rewritten {
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(latex)
    }
}

/// LaTeX as MathML — the markup a browser draws as an equation with no script,
/// stylesheet or font of its own — or `None` when the formula cannot be set.
///
/// The equation's own TeX is kept in the `<annotation>` inside the MathML
/// `<semantics>`: the typesetting is what a reader sees, and the annotation is
/// what says how the equation was written, so a page written by this writer is
/// read back as the formula item it came from rather than as a redrawing of one.
/// `xmlns` is written because MathML in a foreign-agent context (Word, an
/// e-book reader) has nothing else to tell it what these elements are.
///
/// A formula this cannot read is not set at all. The renderer draws a parse
/// error as a red `<merror>` in the page rather than failing, so the parse is
/// run once on its own first: a command outside its vocabulary must leave the
/// LaTeX standing, not put an error box where an equation belongs. Docling's own
/// accent names are re-spelled first (see [`DOCLING_ACCENTS`]), so an equation
/// holding one of them is set rather than drawn.
fn mathml(latex: &str, display: DisplayMode) -> Option<String> {
    let source = latex.trim();
    if source.is_empty() {
        return None;
    }
    // The document's spelling is the annotation's — the reader takes the formula
    // back from there — while the renderer is handed the same accents spelled
    // the way it can read. An accent it still cannot spell leaves the formula
    // unset, as any other unknown command does.
    let spelled = settable_spelling(source);
    let latex = spelled.as_ref();
    let storage = Storage::new();
    let mut probe = Parser::new(latex, &storage);
    if probe.any(|event| event.is_err()) {
        return None;
    }

    let config = RenderConfig {
        display_mode: display,
        xml: true,
        ..RenderConfig::default()
    };
    let mut math = String::new();
    let mut parser = Parser::new(latex, &storage);
    pulldown_latex::push_mathml(&mut math, &mut parser, config).ok()?;

    // The TeX goes beside the drawing, in the `<annotation>` that `<semantics>`
    // exists for. The renderer can write that annotation itself, but it writes
    // the source raw — a `<` or an `&` in a formula would end the element it
    // sits in — so the body is wrapped here, in an `<mrow>` because
    // `<semantics>` shows only its first child, and the TeX is escaped like any
    // other text.
    let open = math.find('>')? + 1;
    let body = math.strip_suffix("</math>")?.get(open..)?;
    let math_open = if display == DisplayMode::Block {
        // `display="block"` is the equation's own claim, but MathML Core — what
        // browsers implement — has dropped the attribute: without this, a run of
        // display equations is laid out as one inline line, each on top of the
        // last.
        math[..open].replacen(
            "<math",
            "<math style=\"display: block; margin: 0.5em 0\"",
            1,
        )
    } else {
        math[..open].to_string()
    };
    Some(format!(
        "{math_open}<semantics><mrow>{body}</mrow><annotation encoding=\"application/x-tex\">{}</annotation></semantics></math>",
        escape(source)
    ))
}

/// The characters that would end an HTML document if written raw.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use pdfextractor_core::{
        DocumentBuilder, GridCell, GridTable, HtmlHead, PictureData, TextLabel,
    };

    /// One slide with speaker notes and a comment: the notes sit in the slide's
    /// group, the comment in its own `comment_section` group.
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
    fn the_notes_layer_joins_only_when_asked() {
        let doc = noted_deck();
        assert_eq!(
            render(&doc),
            titled_page(
                "Q3 Review",
                "<!-- pdfextractor:group chapter body slide-0 -->\n\
                 <h1>Q3 Review</h1>\n\
                 <!-- pdfextractor:/group -->"
            )
        );
        assert_eq!(
            render_with_notes(&doc, true),
            titled_page(
                "Q3 Review",
                "<!-- pdfextractor:group chapter body slide-0 -->\n\
                 <h1>Q3 Review</h1>\n\
                 <aside class=\"notes\">\n\
                 <p><strong>Notes:</strong> Say the picture is the growth.</p>\n\
                 </aside>\n\
                 <!-- pdfextractor:/group -->\n\
                 <!-- pdfextractor:group comment_section notes comment-slide1-1 -->\n\
                 <aside class=\"notes comment\">\n\
                 <p><strong>Comment:</strong> [author: Ada]: Check the numbers.</p>\n\
                 </aside>\n\
                 <!-- pdfextractor:/group -->"
            )
        );
    }

    /// A note's paragraphs are separate `<p>`s inside the aside — the break the
    /// speaker made — and only the first carries the label; the aside is what
    /// marks all of them as remarks.
    #[test]
    fn a_note_keeps_its_paragraphs_as_separate_paragraphs() {
        let mut b = DocumentBuilder::new("demo");
        let slide = b.add_group("chapter", "slide-0", "body");
        b.add_notes_item_in(
            "Open with the revenue line.\nThen the regions.",
            1,
            [0.0; 4],
            &slide,
        );
        assert_eq!(
            render_with_notes(&b.build(), true),
            page(
                "<!-- pdfextractor:group chapter body slide-0 -->\n\
                 <aside class=\"notes\">\n\
                 <p><strong>Notes:</strong> Open with the revenue line.</p>\n\
                 <p>Then the regions.</p>\n\
                 </aside>\n\
                 <!-- pdfextractor:/group -->"
            )
        );
    }

    /// A remark's link is written once, around the paragraph the item opens
    /// with — the anchor the HTML reader recognises as the note's address.
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
            page(
                "<!-- pdfextractor:group comment_section notes comment-slide1-1 -->\n\
                 <aside class=\"notes comment\">\n\
                 <p><strong>Comment:</strong> <a href=\"https://example.com/ledger\">the ledger</a></p>\n\
                 </aside>\n\
                 <!-- pdfextractor:/group -->"
            )
        );
    }

    /// The page with its scaffolding, so tests assert on what a file contains.
    /// These documents carry no title of their own, and a page does not borrow
    /// one from the file it was read from: `<title>` is empty.
    fn page(inner: &str) -> String {
        titled_page("", inner)
    }

    /// The page of a document whose own title is `title`.
    fn titled_page(title: &str, inner: &str) -> String {
        let body = if inner.is_empty() {
            String::new()
        } else {
            format!("{inner}\n")
        };
        format!(
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n</head>\n<body>\n{body}</body>\n</html>\n"
        )
    }

    /// A page's description and canonical URL are the page's own statements
    /// about itself: a document read from one writes them back into its head,
    /// escaped as attribute values, and a document that stated neither invents
    /// nothing.
    #[test]
    fn a_pages_description_and_canonical_url_go_back_into_its_head() {
        let mut b = DocumentBuilder::new("page");
        b.set_html_head(HtmlHead {
            description: Some("Q3 & costs — \"short\"".to_string()),
            canonical: Some("https://example.com/q3?a=1&b=2".to_string()),
            ..Default::default()
        });
        b.add_labeled_item("Title", "title", None, None, None);
        assert_eq!(
            render(&b.build()),
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <title>Title</title>\n\
             <meta name=\"description\" content=\"Q3 &amp; costs — &quot;short&quot;\">\n\
             <link rel=\"canonical\" href=\"https://example.com/q3?a=1&amp;b=2\">\n\
             </head>\n<body>\n<h1>Title</h1>\n</body>\n</html>\n"
        );

        // One statement is one line: a page that named no URL writes no link.
        let mut described = DocumentBuilder::new("page");
        described.set_html_head(HtmlHead {
            description: Some("Only a description.".to_string()),
            ..Default::default()
        });
        let page = render(&described.build());
        assert!(
            page.contains("<meta name=\"description\" content=\"Only a description.\">\n"),
            "{page}"
        );
        assert!(!page.contains("canonical"), "{page}");
    }

    /// The social cards go back the way their vocabularies spell them — `og:`
    /// in `property`, the Twitter card's keys in `name` — in the order the page
    /// stated them, escaped as attribute values, and a page that stated none
    /// gains no line.
    #[test]
    fn a_pages_social_cards_go_back_into_its_head() {
        let mut b = DocumentBuilder::new("page");
        b.set_html_head(HtmlHead {
            social: vec![
                ("og:title".to_string(), "Q3 & \"costs\"".to_string()),
                (
                    "og:image".to_string(),
                    "https://example.com/q3.png".to_string(),
                ),
                ("og:image:width".to_string(), "1200".to_string()),
                (
                    "twitter:card".to_string(),
                    "summary_large_image".to_string(),
                ),
            ],
            ..Default::default()
        });
        b.add_labeled_item("Title", "title", None, None, None);
        let page = render(&b.build());
        assert_eq!(
            page,
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <title>Title</title>\n\
             <meta property=\"og:title\" content=\"Q3 &amp; &quot;costs&quot;\">\n\
             <meta property=\"og:image\" content=\"https://example.com/q3.png\">\n\
             <meta property=\"og:image:width\" content=\"1200\">\n\
             <meta name=\"twitter:card\" content=\"summary_large_image\">\n\
             </head>\n<body>\n<h1>Title</h1>\n</body>\n</html>\n"
        );

        let plain = DocumentBuilder::new("page");
        let page = render(&plain.build());
        assert!(
            !page.contains("og:") && !page.contains("twitter:"),
            "{page}"
        );
    }

    #[test]
    fn headings_take_their_depth_and_the_page_is_whole() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("Title", "title", None, None, None);
        b.add_labeled_item("One", "section_header", Some(1), None, None);
        assert_eq!(
            render(&b.build()),
            titled_page("Title", "<h1>Title</h1>\n<h2>One</h2>")
        );
    }

    /// The page's `<title>` is the document's own title, not the name of the
    /// file it was read from: two documents whose words are the same but whose
    /// names differ write the same page, and a document with no title writes an
    /// empty one rather than a name that would change on the next reading.
    #[test]
    fn the_page_title_is_the_documents_own() {
        let mut first = DocumentBuilder::new("report.pdf");
        first.add_labeled_item("A study of things", "title", None, None, None);
        first.add_labeled_item("Body.", "paragraph", None, None, None);
        let from_pdf = render(&first.build());

        let mut second = DocumentBuilder::new("report.html");
        second.add_labeled_item("A study of things", "title", None, None, None);
        second.add_labeled_item("Body.", "paragraph", None, None, None);
        assert_eq!(from_pdf, render(&second.build()));
        assert!(
            from_pdf.contains("<title>A study of things</title>"),
            "{from_pdf}"
        );
        assert!(!from_pdf.contains("report.pdf"), "{from_pdf}");

        let mut untitled_named = DocumentBuilder::new("notes.pdf");
        untitled_named.add_labeled_item("Body.", "paragraph", None, None, None);
        let untitled = render(&untitled_named.build());
        assert!(untitled.contains("<title></title>"), "{untitled}");
        assert!(!untitled.contains("notes.pdf"), "{untitled}");

        // With two titles the page takes the first the body carries, which is
        // the `<h1>` it opens with; the later one is only a heading.
        let mut two_titles = DocumentBuilder::new("two.pdf");
        two_titles.add_labeled_item("First", "title", None, None, None);
        two_titles.add_labeled_item("Second", "title", None, None, None);
        let two = render(&two_titles.build());
        assert!(two.contains("<title>First</title>"), "{two}");
        assert!(two.contains("<h1>Second</h1>"), "{two}");
    }

    /// The title is escaped like any other text, so a document whose title holds
    /// markup cannot open a tag in the head.
    #[test]
    fn a_title_is_escaped() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("A & B <b>", "title", None, None, None);
        let html = render(&b.build());
        assert!(
            html.contains("<title>A &amp; B &lt;b&gt;</title>"),
            "{html}"
        );
        assert!(!html.contains("<b>"), "{html}");
    }

    #[test]
    fn a_spanning_cell_keeps_its_span_and_covers_nothing_twice() {
        // The Markdown writer must flatten this cell; HTML is where the grid
        // survives: `wide` spans two columns, `tall` two rows.
        let doc: DoclingDocument = serde_json::from_str(
            r##"{
                "schema_name": "DoclingDocument", "version": "1.10.0", "name": "demo",
                "body": {"self_ref": "#/body", "children": [{ "$ref": "#/tables/0" }],
                         "content_layer": "body", "name": "_root_", "label": "unspecified"},
                "tables": [{
                    "self_ref": "#/tables/0", "children": [], "content_layer": "body",
                    "label": "table", "prov": [],
                    "data": {"num_rows": 3, "num_cols": 3, "table_cells": [
                        {"text": "wide", "start_row_offset_idx": 0, "end_row_offset_idx": 1,
                         "start_col_offset_idx": 0, "end_col_offset_idx": 3,
                         "row_span": 1, "col_span": 3, "column_header": true,
                         "row_header": false, "row_section": false},
                        {"text": "tall", "start_row_offset_idx": 1, "end_row_offset_idx": 3,
                         "start_col_offset_idx": 0, "end_col_offset_idx": 1,
                         "row_span": 2, "col_span": 1, "column_header": false,
                         "row_header": false, "row_section": false},
                        {"text": "b", "start_row_offset_idx": 1, "end_row_offset_idx": 2,
                         "start_col_offset_idx": 1, "end_col_offset_idx": 2,
                         "row_span": 1, "col_span": 1, "column_header": false,
                         "row_header": false, "row_section": false},
                        {"text": "c", "start_row_offset_idx": 2, "end_row_offset_idx": 3,
                         "start_col_offset_idx": 1, "end_col_offset_idx": 2,
                         "row_span": 1, "col_span": 1, "column_header": false,
                         "row_header": false, "row_section": false}
                    ]}
                }]
            }"##,
        )
        .expect("a legal document");
        let html = render(&doc);
        assert!(html.contains("<th colspan=\"3\">wide</th>"), "{html}");
        assert!(html.contains("<td rowspan=\"2\">tall</td>"), "{html}");
        assert!(html.contains("<td>b</td>"), "{html}");
        assert!(html.contains("<td>c</td>"), "{html}");
        // The slots `wide` and `tall` cover hold no second copy of them.
        assert_eq!(
            html.matches("<th ").count() + html.matches("<th>").count(),
            1,
            "{html}"
        );
        assert_eq!(html.matches("wide").count(), 1, "{html}");
        assert_eq!(html.matches("tall").count(), 1, "{html}");
        // The header row is a head; the covered third column of row 0 and the
        // cells under the spans fill the grid out to three columns per row.
        assert!(html.contains("<thead>"), "{html}");
    }

    /// A cell a Word document fills with a picture holds it in the model, so the
    /// page writes the `<img>` inside the cell. A writer that carried the picture
    /// as a block of its own would put it beside the table, where the document
    /// has none.
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
        let html = render(&b.build());
        assert_eq!(html.matches("<img ").count(), 1, "{html}");
        assert!(
            html.contains("<th>Before</th>")
                && html.contains("<th><img src=\"demo_images/pg1_1.png\" alt=\"Image\"></th>"),
            "the picture is not in its cell: {html}"
        );
    }

    #[test]
    fn a_table_without_a_header_gets_no_thead() {
        let doc: DoclingDocument = serde_json::from_str(
            r##"{
                "schema_name": "DoclingDocument", "version": "1.10.0", "name": "demo",
                "body": {"self_ref": "#/body", "children": [{ "$ref": "#/tables/0" }],
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
        let html = render(&doc);
        assert!(!html.contains("thead"), "{html}");
        assert!(html.contains("<td>a</td>"), "{html}");
    }

    #[test]
    fn a_table_keeps_a_header_row_in_thead() {
        let mut b = DocumentBuilder::new("demo");
        b.add_table_plain(&[vec!["A".into(), "B".into()], vec!["1".into(), "2".into()]]);
        let html = render(&b.build());
        assert!(
            html.contains("<thead>\n<tr><th>A</th><th>B</th></tr>\n</thead>"),
            "{html}"
        );
        assert!(
            html.contains("<tbody>\n<tr><td>1</td><td>2</td></tr>\n</tbody>"),
            "{html}"
        );
    }

    /// A table can name its columns below a first row that does not — a caption
    /// row. HTML has one place for a header, so the marked row is written in
    /// `<thead>` and the row above it follows in `<tbody>`, in its own order.
    #[test]
    fn a_header_below_the_first_row_is_written_in_thead() {
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
        let html = render(&b.build());
        assert!(
            html.contains("<thead>\n<tr><th>Month</th><th>Total</th></tr>\n</thead>"),
            "{html}"
        );
        assert!(
            html.contains(
                "<tbody>\n\
                 <tr><td colspan=\"2\">Monthly totals</td></tr>\n\
                 <tr><td>January</td><td>10</td></tr>\n\
                 </tbody>"
            ),
            "{html}"
        );
    }

    #[test]
    fn a_run_of_list_items_is_one_list_and_printed_numbers_survive() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("First", "list_item", None, Some("1.".into()), Some(true));
        b.add_labeled_item("Second", "list_item", None, Some("2)".into()), Some(true));
        b.add_labeled_item("Third", "list_item", None, Some("•".into()), Some(false));
        b.add_labeled_item("Roman", "list_item", None, Some("i.".into()), Some(true));
        let html = render(&b.build());
        // The numbers are the document's own, not the browser's count, and a
        // marker `<ol>` cannot spell is kept whole in `data-marker`.
        assert!(html.contains("<ol>\n<li value=\"1\">First</li>"), "{html}");
        assert!(
            html.contains("<li value=\"2\" data-marker=\"2)\">Second</li>\n</ol>"),
            "{html}"
        );
        // The bullet becomes just the item; the roman marker is the text's own.
        assert!(html.contains("<ul>\n<li>Third</li>"), "{html}");
        assert!(html.contains("<li>i. Roman</li>\n</ul>"), "{html}");
    }

    #[test]
    fn markup_in_text_cannot_escape_the_page() {
        let mut b = DocumentBuilder::new("demo");
        b.add_text_block(
            "<script>alert('&')</script>",
            1,
            [0.0, 0.0, 1.0, 1.0],
            "paragraph",
        );
        let html = render(&b.build());
        assert!(
            html.contains("<p>&lt;script&gt;alert(&#39;&amp;&#39;)&lt;/script&gt;</p>"),
            "{html}"
        );
        assert!(!html.contains("<script>"), "{html}");
    }

    #[test]
    fn a_link_becomes_an_anchor_around_the_whole_item() {
        let url = Some("https://example.com/spec".to_string());
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item_with_link("Spec", "title", None, None, None, url.clone());
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
            titled_page(
                "Spec",
                "<h1><a href=\"https://example.com/spec\">Spec</a></h1>\n\
                  <p><a href=\"https://example.com/spec\">Read the spec</a></p>\n\
                  <ol>\n<li value=\"1\"><a href=\"https://example.com/spec\">First</a></li>\n</ol>"
            )
        );
    }

    #[test]
    fn a_link_escapes_its_address_and_its_text() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item_with_link(
            "A & B <b>",
            "paragraph",
            None,
            None,
            None,
            Some("https://example.com/?a=1&b=\"2\"".into()),
        );
        assert_eq!(
            render(&b.build()),
            page(
                "<p><a href=\"https://example.com/?a=1&amp;b=&quot;2&quot;\">A &amp; B &lt;b&gt;</a></p>"
            )
        );
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
        assert_eq!(
            render(&b.build()),
            page("<pre><code>see https://example.com</code></pre>")
        );
    }

    #[test]
    fn code_formulas_and_pictures_have_elements() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_text("fn main() {}", "code");
        b.add_labeled_text("E = mc^2", "formula");
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
        let html = render(&b.build());
        assert!(
            html.contains("<pre><code>fn main() {}</code></pre>"),
            "{html}"
        );
        // The equation is typeset MathML, and its own TeX travels with it in the
        // annotation: what a reader sees is the equation, what a reader of the
        // file gets back is the formula the document held.
        assert!(
            html.contains(
                "<div class=\"formula\"><math style=\"display: block; margin: 0.5em 0\" display=\"block\" xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow><mi>E</mi><mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow><annotation encoding=\"application/x-tex\">E = mc^2</annotation></semantics></math></div>"
            ),
            "{html}"
        );
        assert!(
            html.contains("<p><img src=\"demo_images/pg1_1.png\" alt=\"Image\"></p>"),
            "{html}"
        );
        // The image-less drawing leaves nothing rather than a broken <img>.
        assert_eq!(html.matches("<img").count(), 1, "{html}");
    }

    /// Inline math is a span where it stands, not a block of its own: the
    /// sentence keeps its words and its shape, and only the delimiters go.
    #[test]
    fn inline_math_is_a_span_inside_its_sentence() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item(
            "The area is $a^2$ and $2\\pi r$ is a circle.",
            "paragraph",
            None,
            None,
            None,
        );
        let html = render(&b.build());
        assert!(
            html.contains(
                "<p>The area is <span class=\"formula\"><math display=\"inline\" \
                 xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow><msup><mi>a</mi><mn>2</mn></msup></mrow><annotation encoding=\"application/x-tex\">a^2</annotation></semantics></math></span> and \
                 <span class=\"formula\"><math display=\"inline\" \
                 xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow><mn>2</mn><mi>π</mi><mi>r</mi></mrow><annotation encoding=\"application/x-tex\">2\\pi r</annotation></semantics></math></span> is a circle.</p>"
            ),
            "{html}"
        );
        assert_eq!(
            html.matches("<p>").count(),
            1,
            "the sentence was split: {html}"
        );
    }

    /// The dollars have to pair up *as math*. A price, an election and an
    /// escaped dollar are words; a lone dollar is not a delimiter.
    #[test]
    fn prices_and_escaped_dollars_stay_words() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item(
            "It costs $5 and $10, or \\$7, and only $x$ is math.",
            "paragraph",
            None,
            None,
            None,
        );
        let html = render(&b.build());
        assert!(
            html.contains(
                "<p>It costs $5 and $10, or \\$7, and only \
                 <span class=\"formula\"><math display=\"inline\" \
                 xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow><mi>x</mi></mrow><annotation encoding=\"application/x-tex\">x</annotation></semantics></math></span> is math.</p>"
            ),
            "{html}"
        );
    }

    /// A list item and a quote are prose too, so inline math in either is a
    /// span where it stands. The container keeps its words, the item keeps its
    /// bullet, and the note keeps its label — the dollars are Markdown's
    /// notation and never the page's own characters.
    #[test]
    fn inline_math_is_a_span_in_list_items_and_notes() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item(
            "The perimeter is $2\\pi r$ around.",
            "list_item",
            Some(1),
            Some("-".to_string()),
            Some(false),
        );
        b.add_notes_text("The slope is $m x + b$ here.");
        let html = render_with_notes(&b.build(), true);

        assert!(
            html.contains(
                "<li>The perimeter is <span class=\"formula\"><math display=\"inline\" \
                 xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow><mn>2</mn><mi>π</mi><mi>r</mi></mrow><annotation encoding=\"application/x-tex\">2\\pi r</annotation></semantics></math></span> around.</li>"
            ),
            "the list item's formula was not typeset in place:\n{html}"
        );
        assert!(
            html.contains(
                "<p><strong>Notes:</strong> The slope is <span class=\"formula\"><math display=\"inline\" \
                 xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow><mi>m</mi><mi>x</mi><mo>+</mo><mi>b</mi></mrow><annotation encoding=\"application/x-tex\">m x + b</annotation></semantics></math></span> here.</p>"
            ),
            "the note's formula was not typeset in place:\n{html}"
        );
        assert_eq!(
            html.matches('$').count(),
            0,
            "the delimiters reached the page:\n{html}"
        );
    }

    /// An equation this cannot typeset is drawn, not printed: never a red
    /// parse error where an equation belongs, and never the TeX standing in
    /// for the equation it describes. The alt text is that TeX, so the page
    /// still reads back as the formula it came from.
    #[test]
    fn a_formula_that_cannot_be_typeset_is_drawn() {
        let mut b = DocumentBuilder::new("demo");
        // `\macron` is a bar over the letter, spelled with a name the LaTeX
        // reader does not know: Docling's own accent names are re-spelled and
        // set (see `a_docling_accent_is_typeset`), so the drawing is reached by
        // an accent that has no spelling this writer knows either.
        b.add_labeled_text("\\macron{a}", "formula");
        let html = render(&b.build());
        assert!(!html.contains("<math"), "{html}");
        assert!(
            !html.contains("<div class=\"formula\">\\macron{a}</div>"),
            "the page shows the equation, not the command: {html}"
        );

        let start = html
            .find("<img src=\"data:image/svg+xml;base64,")
            .unwrap_or_else(|| panic!("a drawing: {html}"));
        let tag = &html[start..];
        let src = tag.split('"').nth(1).expect("a src attribute");
        let bytes = STANDARD
            .decode(src.trim_start_matches("data:image/svg+xml;base64,"))
            .expect("base64 payload");
        let svg = String::from_utf8(bytes).expect("UTF-8 SVG");
        // The letter with its bar drawn over it — the accent drawn instead of
        // typeset.
        assert!(
            svg.contains(">a</text>") && svg.contains("<path"),
            "the accent is drawn: {svg}"
        );
        // The TeX travels with the picture, and a reader takes it back from
        // there.
        assert!(html.contains("alt=\"\\macron{a}\""), "{html}");
    }

    /// Docling's OMML accent names are spelled so they can be set, and the
    /// formula's own TeX — the annotation — keeps Docling's spelling: the page
    /// typesets the equation, and reading it back gives the formula item that
    /// came in. A ring above (`\ocirc`) and a tilde below (`\wideutilde`) are
    /// two the renderer cannot read as Docling writes them.
    #[test]
    fn a_docling_accent_is_typeset() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_text("\\ocirc{a}", "formula");
        b.add_text("The value $\\wideutilde{x}$ stands here.", 1);
        let html = render(&b.build());

        assert_eq!(
            html.matches("<math").count(),
            2,
            "an accent was still drawn: {html}"
        );
        assert!(
            !html.contains("<img"),
            "the page still holds a drawing: {html}"
        );
        assert!(
            html.contains("<annotation encoding=\"application/x-tex\">\\ocirc{a}</annotation>"),
            "the annotation lost Docling's spelling: {html}"
        );
        assert!(
            html.contains(
                "<span class=\"formula\"><math display=\"inline\" \
                 xmlns=\"http://www.w3.org/1998/Math/MathML\"><semantics><mrow>"
            ),
            "the inline accent was not set in place: {html}"
        );
        assert!(
            html.contains(
                "<annotation encoding=\"application/x-tex\">\\wideutilde{x}</annotation>"
            ),
            "the inline annotation lost Docling's spelling: {html}"
        );
    }

    /// The same drawing inside a sentence: the span holds it where the formula
    /// stood, and the sentence stays one paragraph.
    #[test]
    fn an_inline_formula_that_cannot_be_typeset_is_drawn_in_place() {
        let mut b = DocumentBuilder::new("demo");
        b.add_text("The value $\\macron{a}$ stands here.", 1);
        let html = render(&b.build());
        assert!(
            html.contains("<span class=\"formula\"><img src=\"data:image/svg+xml;base64,"),
            "{html}"
        );
        assert!(html.contains("alt=\"\\macron{a}\""), "{html}");
        // The picture is stood on the sentence's baseline, not hung from it.
        assert!(html.contains("style=\"vertical-align: -"), "{html}");
        assert!(html.contains("</span> stands here.</p>"), "{html}");
        assert!(!html.contains("<math"), "{html}");
        assert!(html.contains("stands here.</p>"), "{html}");
    }

    /// The TeX inside the annotation is escaped like any other text: a formula
    /// holding `<` or `&` is still a page a browser can parse, and the annotation
    /// still reads back as the source it was.
    #[test]
    fn a_formula_annotation_is_escaped() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_text("a \\text{ < b ] > c} + b", "formula");
        let html = render(&b.build());
        assert!(
            html.contains(
                "<annotation encoding=\"application/x-tex\">a \\text{ &lt; b ] &gt; c} + b</annotation>"
            ),
            "{html}"
        );
    }

    #[test]
    fn furniture_never_reaches_the_page() {
        let mut b = DocumentBuilder::new("demo");
        b.add_text_block("Body", 1, [0.0, 0.0, 1.0, 1.0], "paragraph");
        b.add_furniture_on_page("Running head", 1, [0.0, 0.0, 1.0, 1.0], "page_header");
        let html = render(&b.build());
        assert!(html.contains("<p>Body</p>"), "{html}");
        assert!(!html.contains("Running head"), "{html}");
    }

    /// A quotation is a `<blockquote>` on the page, with the paragraph its words
    /// make inside it: the element is what the reader puts the quote back from,
    /// and the paragraph is still the paragraph.
    #[test]
    fn a_quotation_is_a_blockquote_on_the_page() {
        let mut b = DocumentBuilder::new("demo");
        b.add_quotation("The medium is the message.", None);
        b.add_text("After.", 1);
        assert_eq!(
            render(&b.build()),
            page("<blockquote><p>The medium is the message.</p></blockquote>\n<p>After.</p>")
        );
    }

    #[test]
    fn an_empty_document_is_an_empty_page() {
        let b = DocumentBuilder::new("demo");
        assert_eq!(render(&b.build()), page(""));
    }

    #[test]
    fn export_writes_where_asked_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("pdfe-html-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("out.html");

        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("Title", "title", None, None, None);
        export_to_file(&b.build(), &path).expect("export");

        let html = std::fs::read_to_string(&path).unwrap();
        assert_eq!(html, titled_page("Title", "<h1>Title</h1>"));
        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["out.html".to_string()], "left {entries:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The HTML side of the picture modes: a `data:` URI in the page, a mark
    /// where a picture stood, and the file reference it always wrote.
    #[test]
    fn pictures_are_referenced_embedded_or_marked() {
        let dir = std::env::temp_dir().join(format!("pdfe-html-images-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("demo_images")).expect("scratch dir");
        let bytes = b"\x89PNG\r\n\x1a\nnot quite a png";
        std::fs::write(dir.join("demo_images/pg1_1.png"), bytes).expect("write image");

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
        let doc = b.build();

        assert!(
            render_with_images(&doc, false, ImageMode::Referenced, Some(&dir))
                .contains("<p><img src=\"demo_images/pg1_1.png\" alt=\"Image\"></p>")
        );
        let embedded = render_with_images(&doc, false, ImageMode::Embedded, Some(&dir));
        assert!(
            embedded.contains(&format!(
                "<img src=\"data:image/png;base64,{}\"",
                STANDARD.encode(bytes)
            )),
            "{embedded}"
        );
        assert!(!embedded.contains("demo_images"), "{embedded}");

        let marked = render_with_images(&doc, false, ImageMode::Placeholder, Some(&dir));
        assert!(marked.contains("<!-- image -->"), "{marked}");
        assert!(!marked.contains("<img"), "{marked}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every accent Docling writes is set: each one in the table, and the ones
    /// the renderer already reads as Docling writes them. The formula keeps
    /// Docling's spelling in its annotation whether it was re-spelled or not.
    #[test]
    fn every_docling_accent_is_typeset() {
        let as_written = [
            "grave",
            "acute",
            "hat",
            "tilde",
            "bar",
            "breve",
            "dot",
            "ddot",
            "check",
            "vec",
            "overleftarrow",
            "overleftrightarrow",
            "underbar",
            "underrightarrow",
            "not",
            "overbrace",
            "underbrace",
            "overbracket",
            "underbracket",
            "overparen",
            "underparen",
        ];
        let mut names: Vec<&str> = DOCLING_ACCENTS
            .iter()
            .map(|(docling, _)| *docling)
            .collect();
        names.extend(as_written);

        for name in names {
            let latex = format!("\\{name}{{a}}");
            let math = mathml(&latex, DisplayMode::Inline)
                .unwrap_or_else(|| panic!("`{latex}` was not set"));
            assert!(
                math.contains(&format!(
                    "<annotation encoding=\"application/x-tex\">{latex}</annotation>"
                )),
                "`{latex}` lost Docling's spelling: {math}"
            );
        }
    }

    /// A mark LaTeX has no name for is set as the shape it is — three dots over
    /// the letter, three under it, an asterisk over it — not left as a command
    /// the renderer cannot read, and not drawn as a picture.
    #[test]
    fn an_accent_with_no_latex_name_is_set_as_its_shape() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_text("\\dddot{a}", "formula");
        b.add_labeled_text("\\threeunderdot{a}", "formula");
        b.add_labeled_text("\\asteraccent{a}", "formula");
        let html = render(&b.build());
        for expected in [
            "<mover><mrow><mi>a</mi></mrow><mrow><mi>⋯</mi></mrow></mover>",
            "<munder><mrow><mi>a</mi></mrow><mrow><mi>⋯</mi></mrow></munder>",
            "<mover><mrow><mi>a</mi></mrow><mrow><mi>*</mi></mrow></mover>",
        ] {
            assert!(html.contains(expected), "{expected} was not set: {html}");
        }
        assert!(!html.contains("<img"), "an accent was drawn: {html}");
    }

    /// A page of Docling's accents is a page of equations: no accent left the
    /// formula to a drawing.
    #[test]
    fn a_page_of_docling_accents_draws_nothing() {
        let mut b = DocumentBuilder::new("demo");
        for (docling, _) in DOCLING_ACCENTS {
            b.add_labeled_text(&format!("\\{docling}{{a}}"), "formula");
        }
        let html = render(&b.build());
        assert!(!html.contains("<img"), "an accent was drawn: {html}");
        assert_eq!(
            html.matches("<math").count(),
            DOCLING_ACCENTS.len(),
            "an accent was not set: {html}"
        );
    }
}
