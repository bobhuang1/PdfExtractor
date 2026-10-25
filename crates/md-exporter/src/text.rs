//! Render a [`DoclingDocument`] as plain text.
//!
//! The same walk as every other writer decides what is read and in what
//! order; this module strips the document down to its words:
//!
//! * **Headings are their words.** No `#`, no underlining — the level is
//!   already carried by whatever places the text afterwards, and a `#` in a
//!   plain text file is a `#`.
//! * **List items keep the marker the document printed** (`1. First`,
//!   `- i. Roman`), the same lines the Markdown writer writes, because that
//!   marker *is* the reading.
//! * **Tables are the same padded grid** Markdown uses: a pipe table reads as
//!   aligned columns in any terminal, and sharing the writer means the two
//!   formats can never disagree about what is in a cell.
//! * **Code and formulas** are written verbatim, without fences or `$$`;
//!   **pictures** leave nothing — a file path is not the picture's reading,
//!   and plain text has no way to show it.
//! * **Furniture is omitted**, exactly as in Markdown and HTML. The notes
//!   layer is omitted too unless the reader asks for it with
//!   `--include-notes`, which puts a deck's speaker notes after their slide
//!   and its comments after the slides.
//!
//! **Dehyphenation** follows pdftotext's default: a word broken by a line end
//! is joined back up — `inter-` / `national` is one word `international`.
//! It is on by default and turned off with `--text-dehyphenate false`, which
//! is documented in the README beside the flag itself: the join is a
//! heuristic (a genuine `twenty-` / `four` fuses like any other split), so a
//! pipeline that must see the source characters switches it off.

use crate::walk::{walk_body_with, Visitor};
use crate::{cell_tables, is_bullet_glyph, pipe_table_with};
use pdfextractor_core::{DoclingDocument, PictureItem, TableItem, TextItem};
use std::path::Path;

/// Render `doc` as plain text, dehyphenating broken words (the default a
/// reader of plain text expects, pdftotext's own).
pub fn render(doc: &DoclingDocument) -> String {
    render_with(doc, true)
}

/// Render `doc` as plain text, joining line-end hyphens only when
/// `dehyphenate` asks for it.
pub fn render_with(doc: &DoclingDocument, dehyphenate: bool) -> String {
    render_with_notes(doc, dehyphenate, false)
}

/// Render `doc` as plain text, including the notes layer when `include_notes`:
/// a deck's speaker notes after their slide, its comments after the slides.
pub fn render_with_notes(doc: &DoclingDocument, dehyphenate: bool, include_notes: bool) -> String {
    let mut renderer = Renderer {
        doc,
        dehyphenate,
        blocks: Vec::new(),
    };
    walk_body_with(doc, include_notes, &mut renderer);
    let mut out = renderer.blocks.join("\n\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Write `doc` as plain text to `path`, atomically, as every other writer.
pub fn export_to_file(doc: &DoclingDocument, path: &Path, dehyphenate: bool) -> anyhow::Result<()> {
    export_to_file_with_notes(doc, path, dehyphenate, false)
}

/// Write `doc` as plain text to `path`, including the notes layer when
/// `include_notes`.
pub fn export_to_file_with_notes(
    doc: &DoclingDocument,
    path: &Path,
    dehyphenate: bool,
    include_notes: bool,
) -> anyhow::Result<()> {
    let text = render_with_notes(doc, dehyphenate, include_notes);

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.txt".to_string());
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));

    std::fs::write(&temp, text)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

struct Renderer<'a> {
    doc: &'a DoclingDocument,
    /// Whether line-end hyphens join the lines they broke.
    dehyphenate: bool,
    /// One entry per rendered block, joined by a blank line.
    blocks: Vec<String>,
}

impl Renderer<'_> {
    fn block(&mut self, text: String) {
        if !text.trim().is_empty() {
            self.blocks.push(text);
        }
    }

    /// The item's text as it reads: its lines folded onto one, with any
    /// word broken by a line end put back together when asked.
    fn words(&self, text: &str) -> String {
        let mut out = String::new();
        for line in text.lines().map(str::trim) {
            if out.is_empty() {
                out.push_str(line);
                continue;
            }
            if self.dehyphenate && broke_a_word(&out) && starts_a_word(line) {
                out.pop(); // the hyphen that broke it
                out.push_str(line);
            } else {
                out.push(' ');
                out.push_str(line);
            }
        }
        out
    }
}

impl Visitor for Renderer<'_> {
    fn text(&mut self, _index: usize, item: &TextItem) {
        match item.label.as_str() {
            "title" | "section_header" => {
                let text = self.words(&item.text);
                self.block(text);
            }
            "list_item" => {
                let text = self.words(&item.text);
                let line = match item.marker.as_deref() {
                    Some(marker) if marker.starts_with(|c: char| c.is_ascii_digit()) => {
                        format!("{marker} {text}")
                    }
                    Some(marker) if is_bullet_glyph(marker) => format!("- {text}"),
                    // A roman or lettered marker is the item's own first
                    // word, exactly as Markdown writes it.
                    Some(marker) => format!("- {marker} {text}"),
                    None => format!("- {text}"),
                };
                self.block(line);
            }
            // Code and formulas are written exactly as they are — there is no
            // markup to remove and nothing to dehyphenate inside.
            "code" => self.block(item.text.clone()),
            "formula" => self.block(item.text.trim().to_string()),
            // Paragraphs, captions, footnotes, references.
            _ => {
                let text = self.words(&item.text);
                self.block(text);
            }
        }
    }
    fn table(&mut self, _index: usize, table: &TableItem) {
        if let Some(block) = pipe_table_with(table, |cell| self.nested_cell_words(cell)) {
            self.block(block);
        }
    }
    fn picture(&mut self, _index: usize, _picture: &PictureItem) {
        // A plain text file cannot show a picture, and its file path is not
        // something to read: the same walk reaches it, this writes nothing.
    }
}

impl Renderer<'_> {
    /// The words of the tables a cell holds, after the cell's own: plain text
    /// cannot draw a table inside a cell, so a nested table's cells read where
    /// the outer cell is — the same flattening Markdown writes, and no words
    /// dropped for the sake of the shape.
    fn nested_cell_words(&self, cell: &pdfextractor_core::TableCell) -> String {
        let mut held: Vec<String> = Vec::new();
        for table in cell_tables(self.doc, cell) {
            held.extend(self.nested_table_words(table));
        }
        held.join(" ")
    }

    /// A nested table's cells in grid order, each as its words and then the
    /// words of whatever it holds in turn.
    fn nested_table_words(&self, table: &TableItem) -> Vec<String> {
        let mut cells: Vec<&pdfextractor_core::TableCell> = table.data.table_cells.iter().collect();
        cells.sort_by_key(|cell| (cell.start_row_offset_idx, cell.start_col_offset_idx));
        let mut held = Vec::new();
        for cell in cells {
            let text = self.words(&cell.text);
            if !text.is_empty() {
                held.push(text);
            }
            let rest = self.nested_cell_words(cell);
            if !rest.is_empty() {
                held.push(rest);
            }
        }
        held
    }
}

/// Whether the text so far ends at the hyphen that broke a word across the
/// line — one hyphen, not the double one of a real `--` break, and not a
/// soft hyphen, which is invisible but still a break.
fn broke_a_word(so_far: &str) -> bool {
    if let Some(base) = so_far.strip_suffix('\u{00ad}') {
        return !base.ends_with('-');
    }
    so_far.ends_with('-') && !so_far.ends_with("--")
}

/// Whether the next line continues that word: a word break never re-starts
/// with punctuation a word cannot begin with.
fn starts_a_word(line: &str) -> bool {
    line.starts_with(|c: char| c.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfextractor_core::{DocumentBuilder, PictureData, TextLabel};

    /// One slide with speaker notes and a comment, in the tree the PPTX reader
    /// builds: notes inside the slide's group, the comment in its own group.
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
        assert_eq!(render(&doc), "Q3 Review\n");
        assert_eq!(
            render_with_notes(&doc, true, true),
            "Q3 Review\n\nSay the picture is the growth.\n\n[author: Ada]: Check the numbers.\n"
        );
    }

    #[test]
    fn headings_are_their_words() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("Title", "title", None, None, None);
        b.add_labeled_item("One", "section_header", Some(1), None, None);
        assert_eq!(render(&b.build()), "Title\n\nOne\n");
    }

    #[test]
    fn a_word_broken_by_a_line_end_is_joined_by_default() {
        let mut b = DocumentBuilder::new("demo");
        b.add_text_block(
            "inter-\nnational word",
            1,
            [0.0, 0.0, 1.0, 1.0],
            "paragraph",
        );
        assert_eq!(render(&b.build()), "international word\n");
    }

    #[test]
    fn the_off_switch_leaves_the_source_characters_alone() {
        let mut b = DocumentBuilder::new("demo");
        b.add_text_block(
            "inter-\nnational word",
            1,
            [0.0, 0.0, 1.0, 1.0],
            "paragraph",
        );
        // `--text-dehyphenate false`: every line the PDF printed, folded but
        // otherwise untouched, hyphen and all.
        assert_eq!(render_with(&b.build(), false), "inter- national word\n");
    }

    #[test]
    fn a_double_hyphen_is_punctuation_not_a_break() {
        let mut b = DocumentBuilder::new("demo");
        b.add_text_block("a --\ntrue thing", 1, [0.0, 0.0, 1.0, 1.0], "paragraph");
        assert_eq!(render(&b.build()), "a -- true thing\n");
    }

    #[test]
    fn a_break_before_a_capital_still_joins() {
        // A proper noun split by the line is still one word.
        let mut b = DocumentBuilder::new("demo");
        b.add_text_block("Mc-\nDonald", 1, [0.0, 0.0, 1.0, 1.0], "paragraph");
        assert_eq!(render(&b.build()), "McDonald\n");
    }

    #[test]
    fn list_markers_keep_what_the_document_printed_and_html_marks_stay_out() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("First", "list_item", None, Some("1.".into()), Some(true));
        b.add_labeled_item("roman", "list_item", None, Some("i.".into()), Some(true));
        b.add_labeled_item("bullet", "list_item", None, Some("•".into()), Some(false));
        assert_eq!(render(&b.build()), "1. First\n\n- i. roman\n\n- bullet\n");
    }

    #[test]
    fn tables_are_the_same_grid_as_markdown() {
        let mut b = DocumentBuilder::new("demo");
        b.add_table_plain(&[vec!["A".into(), "B".into()], vec!["x".into(), "y".into()]]);
        assert_eq!(render(&b.build()), "| A | B |\n|---|---|\n| x | y |\n");
    }

    #[test]
    fn code_is_verbatim_and_formulas_without_delimiters() {
        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_text("let x = 1;", "code");
        b.add_labeled_text("E = mc^2", "formula");
        assert_eq!(render(&b.build()), "let x = 1;\n\nE = mc^2\n");
    }

    #[test]
    fn furniture_and_pictures_leave_nothing() {
        let mut b = DocumentBuilder::new("demo");
        b.add_text_block("Body", 1, [0.0, 0.0, 1.0, 1.0], "paragraph");
        b.add_furniture_on_page("Running head", 1, [0.0, 0.0, 1.0, 1.0], "page_header");
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
        assert_eq!(render(&b.build()), "Body\n");
    }

    #[test]
    fn an_empty_document_renders_empty() {
        let b = DocumentBuilder::new("demo");
        assert_eq!(render(&b.build()), "");
    }

    #[test]
    fn export_writes_where_asked_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("pdfe-text-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("out.txt");

        let mut b = DocumentBuilder::new("demo");
        b.add_labeled_item("Title", "title", None, None, None);
        export_to_file(&b.build(), &path, true).expect("export");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Title\n");
        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["out.txt".to_string()], "left {entries:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
