//! Plain text (`.txt`) and Markdown (`.md`) parsing.
//!
//! Plain text carries one kind of structure: a blank line. A run of non-empty
//! lines is one block, because that is the only boundary a `.txt` file states —
//! a one-line run is a paragraph, and a longer run is kept verbatim as a `code`
//! item, since the text writer folds a paragraph's own lines onto one, so words
//! whose line breaks were kept can only be content (a code block, a grid). A
//! run that opens with the writer's pipe grid is the table it is. That is what
//! makes the text writer's output a fixed point when it is read back, and the
//! committed goldens are held to it by `every_text_golden_is_a_fixed_point`.
//!
//! Markdown does carry structure, so headings, lists, fenced code, `$$` display
//! formulas — fenced or on one line, and lifted out of a block quote or list
//! item when they are written there — and pipe tables are recognised and the
//! rest stays prose. Inline emphasis is left as written rather than stripped,
//! so the text is exactly what the file contains. A leading YAML front matter
//! is read for the page's head facts — description, canonical URL and social
//! cards — rather than becoming paragraphs; a block that states none of them is
//! left as prose, so a Markdown file's own horizontal rule is not swallowed.
//!
//! Links and images are the exceptions: `[label](destination)` is the one
//! inline syntax the document model carries in a field of its own
//! (`hyperlink`), so it is read back into it, and `![alt](source)` is a
//! picture, not words, so it becomes an item of its own. Neither fits inside a
//! sentence, so a line splits around it — the words before, the link or
//! picture, and the words after — exactly as the HTML reader splits at an
//! anchor or an `img`.
//!
//! Headings are read the way Docling's own Markdown backend reads them, which
//! is the inverse of the writer: `#` is the document's title and carries no
//! depth, `##` is a level-1 section header, `###` level 2, and so on. The two
//! conventions being inverses is what makes Markdown in, Markdown out a fixed
//! point — the committed goldens are held to it by
//! `every_markdown_golden_is_a_fixed_point` in the CLI's tests.

use crate::{decode, Block, TableGrid, TextDocument};

/// Split into lines, treating CRLF, CR and LF alike.
fn lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// Parse a plain-text file: blank lines separate blocks.
///
/// A run of non-empty lines is one block. One line is a paragraph; more than
/// one is kept exactly as it stands, newlines and indentation included, as a
/// `code` item — the text writer writes a paragraph's lines folded onto one, so
/// a block that keeps its line breaks can only be words whose breaks are the
/// content, and writing them verbatim is what reproduces them. A run that
/// opens with a header row over a pipe-table alignment row is a table, the same
/// grid Markdown and the text writer share.
pub fn parse_plain(bytes: &[u8]) -> TextDocument {
    let raw = lines(&decode(bytes));
    let mut doc = TextDocument::default();
    let mut i = 0usize;
    while i < raw.len() {
        if raw[i].trim().is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        while i < raw.len() && !raw[i].trim().is_empty() {
            i += 1;
        }
        let run = &raw[start..i];

        // The writer's grid: a header row, then the `|---|` alignment row.
        let mut at = 0usize;
        if run.len() >= 2 && run[0].contains('|') && is_table_delimiter(&run[1]) {
            let mut rows = vec![split_row(&run[0])];
            at = 2;
            while at < run.len() && run[at].contains('|') {
                rows.push(split_row(&run[at]));
                at += 1;
            }
            doc.items.push(Block::Table {
                grid: TableGrid::from_rows(&rows),
            });
        }

        match &run[at..] {
            [] => {}
            [line] => doc.items.push(Block::paragraph(line.trim())),
            // Verbatim: the line breaks and the indentation are the content.
            block => doc
                .items
                .push(Block::labelled(block.join("\n"), "code", None, None)),
        }
    }
    doc
}

/// Parse a Markdown file, keeping the block structure it expresses.
pub fn parse_markdown(bytes: &[u8]) -> TextDocument {
    let (raw, front) = take_front_matter(lines(&decode(bytes)));
    let mut doc = TextDocument::default();
    if let Some(front) = front {
        doc.description = front.description;
        doc.canonical = front.canonical;
        doc.social = front.social;
    }
    let mut i = 0usize;
    // The line the current run of quoted note lines started on. Consecutive
    // lines are one note's paragraphs; a blank line between remarks ends the
    // run, because the writer separates two notes with one.
    let mut notes_at: Option<usize> = None;
    // The line the current run of quoted prose started on: consecutive `>`
    // lines are one quotation's lines, the way consecutive lines anywhere else
    // are one paragraph.
    let mut quoted_at: Option<usize> = None;

    while i < raw.len() {
        let line = raw[i].as_str();

        // Fenced code block: everything up to the closing fence is verbatim.
        if let Some(fence) = fence_marker(line) {
            let mut body = Vec::new();
            i += 1;
            while i < raw.len() && fence_marker(&raw[i]).as_deref() != Some(fence.as_str()) {
                body.push(raw[i].clone());
                i += 1;
            }
            i += 1; // consume the closing fence (or run off the end)
            if !body.is_empty() {
                doc.items
                    .push(Block::labelled(body.join("\n"), "code", None, None));
            }
            continue;
        }

        // Display formula: `$$` on its own line, the formula, `$$` again —
        // the writer's form. Reading it as three paragraphs would make the
        // equation change on every save.
        if line.trim() == "$$" {
            let mut body = Vec::new();
            i += 1;
            while i < raw.len() && raw[i].trim() != "$$" {
                body.push(raw[i].clone());
                i += 1;
            }
            i += 1; // consume the closing `$$` (or run off the end)
            doc.items.push(Block::labelled(
                body.join("\n").trim(),
                "formula",
                None,
                None,
            ));
            continue;
        }

        // A display formula inside a block quote or a list item is still a
        // formula, not the quote's or item's words. It is lifted out of the
        // container and becomes the same item the top-level forms make.
        if let Some((formula, next)) = lifted_formula(&raw, i) {
            doc.items
                .push(Block::labelled(formula, "formula", None, None));
            i = next;
            continue;
        }

        // The same display formula on one line, `$$E = mc^2$$`. It is a
        // block of its own like the fenced form; the delimiters are trimmed
        // and the formula is kept as written inside them.
        if let Some(formula) = one_line_formula(line) {
            doc.items
                .push(Block::labelled(formula, "formula", None, None));
            i += 1;
            continue;
        }

        // A group boundary the writer marked with an HTML comment. Markdown
        // has no groups, so this is the only thing that keeps a slide's items
        // together — and a comment's — when the file is read back. The markers
        // are checked after fences and formulas, so a marker quoted inside
        // either stays the literal line it is.
        if let Some(block) = group_marker(line) {
            notes_at = None;
            doc.items.push(block);
            i += 1;
            continue;
        }

        // A spot the writer marked where a picture stood, in the mode that
        // writes no link and no bytes. The mark is the picture — one with
        // nothing behind it, which is what the page said — so a page written
        // that way comes back with its picture instead of the words it was
        // spelled with. A comment that is not this one stays the paragraph it
        // always was, which is what keeps prose about HTML prose.
        if line.trim() == crate::IMAGE_PLACEHOLDER {
            notes_at = None;
            doc.items.push(Block::PicturePlaceholder);
            i += 1;
            continue;
        }

        // Setext heading: the text line, then a line of `=` or `-`.
        if !line.trim().is_empty() {
            if let Some(next) = raw.get(i + 1) {
                if let Some(marks) = setext_marks(next) {
                    push_heading(&mut doc, line.trim(), marks);
                    i += 2;
                    continue;
                }
            }
        }

        // ATX heading.
        if let Some((marks, text)) = atx_heading(line) {
            push_heading(&mut doc, &text, marks);
            i += 1;
            continue;
        }

        // Pipe table: a row line followed by a `---` alignment row.
        if line.contains('|') && raw.get(i + 1).is_some_and(|l| is_table_delimiter(l)) {
            let mut rows = Vec::new();
            rows.push(split_row(line));
            i += 2; // skip the delimiter row
            while i < raw.len() && raw[i].contains('|') && !raw[i].trim().is_empty() {
                rows.push(split_row(&raw[i]));
                i += 1;
            }
            // Markdown has no merged cells, so a pipe table is one cell per
            // slot — the shape a rectangle always has.
            doc.items.push(Block::Table {
                grid: TableGrid::from_rows(&rows),
            });
            continue;
        }

        // List item.
        if let Some((marker, level, text)) = list_item(line) {
            push_inline(
                &mut doc,
                &text,
                "list_item",
                Some(level),
                Some(marker),
                false,
            );
            i += 1;
            continue;
        }

        // Block quote. One labelled `Notes:` or `Comment:` is a deck's
        // remark, not prose: the writer quotes every notes-layer item behind
        // one of those labels, and reading the quote as a paragraph would make
        // the remark a slide. Anything else is an ordinary quotation — prose
        // the document set apart — and stays one, so the next render quotes it
        // again instead of letting it pass as the document's own words.
        if let Some(rest) = line.trim_start().strip_prefix('>') {
            let text = rest.trim();
            if let Some((comment, note)) = note_text(text) {
                if !note.is_empty() {
                    // A comment and a speaker note are different items even
                    // when they touch: only a line with the same label
                    // continues the paragraph before it.
                    let continued = notes_at == Some(i.saturating_sub(1))
                        && match doc.items.last_mut() {
                            Some(Block::Note {
                                text: existing,
                                comment: previous,
                                ..
                            }) if *previous == comment => {
                                existing.push('\n');
                                existing.push_str(note);
                                true
                            }
                            _ => false,
                        };
                    if !continued {
                        // The writer puts a note's hyperlink around the whole
                        // paragraph the item opens with, so a line that is one
                        // link is that link, not its Markdown spelling.
                        let (words, hyperlink) = match note_link(note) {
                            Some((label, url)) => (label, Some(url)),
                            None => (note.to_string(), None),
                        };
                        doc.items.push(Block::Note {
                            text: words,
                            hyperlink,
                            comment,
                        });
                    }
                    notes_at = Some(i);
                }
            } else {
                notes_at = None;
                if !text.is_empty() {
                    // A line break inside a quotation is a soft break, as it is
                    // inside a paragraph: the writer strips it, so reading it
                    // back as a break of its own would make the file change on
                    // every save. Only a blank line — no quote between the two
                    // — starts another quotation.
                    let continued = quoted_at == Some(i.saturating_sub(1))
                        && match doc.items.last_mut() {
                            Some(Block::Text {
                                text: existing,
                                quote: true,
                                // A linked passage is the link's own item: the
                                // words after it are not part of the address.
                                hyperlink: None,
                                ..
                            }) => {
                                existing.push(' ');
                                existing.push_str(text);
                                true
                            }
                            _ => false,
                        };
                    if !continued {
                        push_quoted(&mut doc, text);
                    }
                    quoted_at = Some(i);
                }
            }
            i += 1;
            continue;
        }

        let trimmed = line.trim();
        if !trimmed.is_empty() {
            push_inline(&mut doc, trimmed, "paragraph", None, None, false);
        }
        i += 1;
    }

    doc
}

/// The head facts a Markdown front matter states, in the shape the writer
/// emits: a description and canonical URL, and an ordered list of social-card
/// key/content records.
#[derive(Default)]
struct FrontMatter {
    description: Option<String>,
    canonical: Option<String>,
    social: Vec<(String, String)>,
}

impl FrontMatter {
    fn is_empty(&self) -> bool {
        self.description.is_none() && self.canonical.is_none() && self.social.is_empty()
    }
}

/// Split a leading YAML front matter off a Markdown file.
///
/// The writer puts a page's head facts there when it has any. The block is
/// recognised only when it starts on the first line, closes with `---` or `...`,
/// and actually names one of those facts: a file that opens with a horizontal
/// rule, or with front matter for another tool, is left as the prose it was.
/// A malformed block is likewise left alone rather than failing the conversion.
fn take_front_matter(raw: Vec<String>) -> (Vec<String>, Option<FrontMatter>) {
    let Some(close) = front_matter_end(&raw) else {
        return (raw, None);
    };
    let yaml = raw[1..close].join("\n");
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&yaml) else {
        return (raw, None);
    };
    let Some(fields) = value.as_mapping() else {
        return (raw, None);
    };
    let text = |key: &str| {
        fields
            .get(serde_yaml::Value::String(key.to_string()))
            .and_then(serde_yaml::Value::as_str)
            .map(str::to_string)
    };
    let social = fields
        .get(serde_yaml::Value::String("social".to_string()))
        .and_then(serde_yaml::Value::as_sequence)
        .map(|cards| {
            cards
                .iter()
                .filter_map(|card| {
                    let card = card.as_mapping()?;
                    let field = |key: &str| {
                        card.get(serde_yaml::Value::String(key.to_string()))
                            .and_then(serde_yaml::Value::as_str)
                    };
                    Some((field("key")?.to_string(), field("content")?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let front = FrontMatter {
        description: text("description"),
        canonical: text("canonical"),
        social,
    };
    if front.is_empty() {
        return (raw, None);
    }

    let mut rest = raw;
    rest.drain(..=close);
    (rest, Some(front))
}

/// The closing delimiter of a front matter that starts on line one.
fn front_matter_end(raw: &[String]) -> Option<usize> {
    if raw.first()?.trim() != "---" {
        return None;
    }
    raw.iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| matches!(line.trim(), "---" | "..."))
        .map(|(index, _)| index)
}

/// A quoted line's remark, when it is one: whether it is a comment or a
/// speaker note, and the words after the bold label the Markdown writer puts
/// on every notes-layer item.
///
/// The label has to match what the writer emits, because a quote without it is
/// an ordinary quotation and belongs to the document's prose. `Comment:` is a
/// reviewer's remark and `Notes:` the presenter's; keeping the distinction
/// here means the two stay apart when the file is rendered again. The remark
/// is kept exactly as written — no link or picture is read out of it — so the
/// words come back the way they went out.
fn note_text(text: &str) -> Option<(bool, &str)> {
    if let Some(rest) = text.strip_prefix("**Comment:**") {
        return Some((true, rest.trim_start()));
    }
    text.strip_prefix("**Notes:**")
        .map(|rest| (false, rest.trim_start()))
}

/// A remark whose first paragraph is one link and nothing else.
///
/// The Markdown writer puts an item's hyperlink around the whole paragraph it
/// opens with, so this is exactly what it wrote for a linked remark. Words
/// around the link mean the link was a part of the sentence, which the model
/// cannot record without claiming the rest of the remark was linked too — those
/// keep the words as written.
fn note_link(text: &str) -> Option<(String, String)> {
    let pieces = inline_pieces(text);
    match pieces.as_slice() {
        [Inline::Link { text, url }] => Some((text.clone(), url.clone())),
        _ => None,
    }
}

/// A group marker line, when the writer wrote one.
///
/// The writer's form is `<!-- pdfextractor:group <label> <layer> <name> -->`
/// and `<!-- pdfextractor:/group -->`; the name is last because a document's
/// own names may hold spaces. Any other HTML comment is not a marker and is
/// left to become the paragraph it always was, so a file that merely talks
/// about HTML is not restructured. A malformed marker (a missing field) is
/// likewise left alone rather than guessed at.
pub(crate) fn group_marker(line: &str) -> Option<Block> {
    let line = line.trim();
    if line == "<!-- pdfextractor:/group -->" {
        return Some(Block::GroupEnd);
    }
    let rest = line
        .strip_prefix("<!-- pdfextractor:group ")?
        .strip_suffix(" -->")?;
    let mut fields = rest.splitn(3, ' ');
    let label = fields.next()?.trim();
    let content_layer = fields.next()?.trim();
    let name = fields.next()?.trim();
    if label.is_empty() || content_layer.is_empty() || name.is_empty() {
        return None;
    }
    Some(Block::GroupStart {
        label: label.to_string(),
        name: name.to_string(),
        content_layer: content_layer.to_string(),
    })
}

/// One piece of a line's inline content: words, a link's label and address,
/// or an image's alt text and source.
enum Inline {
    Text(String),
    Link { text: String, url: String },
    Image { alt: String, source: String },
}

/// Push a line of Markdown, giving links their address.
///
/// A line with no link is the one block it always was. A line with a link is
/// split at each link, because the model holds one address per item: the
/// words before become an item of their own, the link's label becomes an item
/// carrying its `hyperlink`, and the words after become another. The label and
/// depth belong to the first piece — the bullet and heading depth were printed
/// once, on the line the link interrupted.
fn push_inline(
    doc: &mut TextDocument,
    line: &str,
    label: &str,
    level: Option<i32>,
    marker: Option<String>,
    quote: bool,
) {
    let pieces = inline_pieces(line);
    if pieces.iter().all(|piece| matches!(piece, Inline::Text(_))) {
        doc.items.push(Block::Text {
            text: line.to_string(),
            label: label.to_string(),
            level,
            marker,
            hyperlink: None,
            quote,
        });
        return;
    }

    // The label and depth belong to the first *text* piece: a line that opens
    // with an image still bulleted its words, and a picture carries neither.
    let mut first = true;
    for piece in pieces {
        let (text, hyperlink) = match piece {
            Inline::Image { alt, source } => {
                doc.items.push(Block::Picture { alt, source });
                continue;
            }
            Inline::Text(text) => (text.trim().to_string(), None),
            Inline::Link { text, url } => (text.trim().to_string(), Some(url)),
        };
        if text.is_empty() {
            continue;
        }
        doc.items.push(Block::Text {
            text,
            label: label.to_string(),
            level,
            marker: if first { marker.clone() } else { None },
            hyperlink,
            quote,
        });
        first = false;
    }
}

/// A quoted line: its words and its links read exactly as a paragraph's are,
/// with the fact that they are quoted. A quotation is prose — a link inside it
/// is a link, an inline formula stays the formula — so only the quoting is
/// extra.
fn push_quoted(doc: &mut TextDocument, line: &str) {
    push_inline(doc, line, "paragraph", None, None, true);
}

/// Push a heading written with `marks` hash marks or, for a setext heading,
/// the one or two underline marks — read the way Docling's Markdown backend
/// reads them: the first level is the document's title and the levels below it
/// are section headers numbered from one. The writer is the inverse (a title
/// gets one mark, a section header `level + 1`), so a rendered document parses
/// back to itself rather than gaining a level on every pass.
fn push_heading(doc: &mut TextDocument, text: &str, marks: i32) {
    if marks <= 1 {
        push_inline(doc, text, "title", None, None, false);
    } else {
        push_inline(doc, text, "section_header", Some(marks - 1), None, false);
    }
}

/// A line split at its inline links. No link means one text piece, so the
/// caller can tell the two cases apart by looking for a [`Inline::Link`].
fn inline_pieces(line: &str) -> Vec<Inline> {
    let chars: Vec<char> = line.chars().collect();
    let mut pieces = Vec::new();
    let mut words = String::new();
    let mut i = 0usize;

    while i < chars.len() {
        // A backslash escapes the next character, bracket or not.
        if chars[i] == '\\' {
            words.push(chars[i]);
            i += 1;
            if let Some(&escaped) = chars.get(i) {
                words.push(escaped);
                i += 1;
            }
            continue;
        }
        // `![alt](src)` is a picture; `[label](url)` is a link. The words
        // around either are their own pieces, so the model can hold them as
        // separate items.
        if chars[i] == '!' {
            if let Some(image) = image_at(&chars, i) {
                if !words.is_empty() {
                    pieces.push(Inline::Text(std::mem::take(&mut words)));
                }
                pieces.push(Inline::Image {
                    alt: image.alt,
                    source: image.source,
                });
                i = image.end;
                continue;
            }
        }
        if chars[i] == '[' {
            if let Some(link) = link_at(&chars, i) {
                if !words.is_empty() {
                    pieces.push(Inline::Text(std::mem::take(&mut words)));
                }
                pieces.push(Inline::Link {
                    text: link.label,
                    url: link.url,
                });
                i = link.end;
                continue;
            }
        }
        words.push(chars[i]);
        i += 1;
    }
    if !words.is_empty() {
        pieces.push(Inline::Text(words));
    }
    pieces
}

struct Link {
    label: String,
    url: String,
    /// The index just past the closing `)`.
    end: usize,
}

struct Image {
    alt: String,
    source: String,
    /// The index just past the closing `)`.
    end: usize,
}

/// The label inside the brackets starting at `start`, which holds `[`, and the
/// index of its closing `]`. Brackets nest, and a backslash escapes the next
/// character.
fn bracketed_label(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start) != Some(&'[') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                i += 2;
                continue;
            }
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    let label = chars[start + 1..i].iter().collect();
                    return Some((label, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The link starting at `start`, which holds `[`, when there is one: a label
/// with balanced brackets, then `(destination)` with an optional title.
fn link_at(chars: &[char], start: usize) -> Option<Link> {
    let (label, label_end) = bracketed_label(chars, start)?;
    // A link with no words is not a label; leaving it as written says what the
    // file contains rather than inventing an empty text item.
    if label.trim().is_empty() {
        return None;
    }
    if chars.get(label_end + 1) != Some(&'(') {
        return None;
    }
    let (url, end) = destination(chars, label_end + 2)?;
    Some(Link { label, url, end })
}

/// The image starting at `start`, which holds `!`: `![alt](source)`. An image
/// with no alt text is still a picture, so the empty label is accepted here
/// where a link's is not.
fn image_at(chars: &[char], start: usize) -> Option<Image> {
    if chars.get(start) != Some(&'!') {
        return None;
    }
    let (alt, label_end) = bracketed_label(chars, start + 1)?;
    if chars.get(label_end + 1) != Some(&'(') {
        return None;
    }
    let (source, end) = destination(chars, label_end + 2)?;
    Some(Image { alt, source, end })
}

/// A link destination beginning just after `(`, and the index just past the
/// link's closing `)`.
///
/// The address runs to the `)` unless it is written in `<…>` form, which the
/// Markdown writer uses when the address itself holds spaces or parentheses.
/// An optional title after the address is Markdown's, not the document's; it
/// belongs to no field and is dropped.
fn destination(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }

    let mut url = String::new();
    if chars.get(i) == Some(&'<') {
        i += 1;
        loop {
            match chars.get(i)? {
                '\\' => {
                    i += 1;
                    url.push(*chars.get(i)?);
                    i += 1;
                }
                '>' => {
                    i += 1;
                    break;
                }
                c => {
                    url.push(*c);
                    i += 1;
                }
            }
        }
        i = close_link(chars, i)?;
    } else {
        let mut depth = 0usize;
        loop {
            match chars.get(i)? {
                '\\' => {
                    i += 1;
                    url.push(*chars.get(i)?);
                    i += 1;
                }
                '(' => {
                    depth += 1;
                    url.push('(');
                    i += 1;
                }
                ')' if depth == 0 => {
                    i += 1;
                    break;
                }
                ')' => {
                    depth -= 1;
                    url.push(')');
                    i += 1;
                }
                c if c.is_whitespace() => {
                    i = close_link(chars, i)?;
                    break;
                }
                c => {
                    url.push(*c);
                    i += 1;
                }
            }
        }
    }

    // `[text]()` points at nothing the model can carry: the words are a label,
    // but a blank address is no hyperlink.
    if url.trim().is_empty() {
        return None;
    }
    Some((url, i))
}

/// Skip the whitespace and optional title after a destination, then its `)`.
fn close_link(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    if let Some(&quote @ ('"' | '\'' | '(')) = chars.get(i) {
        let close = if quote == '(' { ')' } else { quote };
        i += 1;
        while *chars.get(i)? != close {
            if chars[i] == '\\' {
                i += 1;
            }
            i += 1;
        }
        i += 1;
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
    }
    (chars.get(i) == Some(&')')).then_some(i + 1)
}

/// The fence marker of a code-block line, e.g. `"```"`.
/// A one-line `$$…$$` formula: the content between the delimiters, trimmed.
/// Requiring content keeps a line of four dollar signs from becoming an empty
/// formula; inline `$…$` is not a display block and is left as prose.
fn one_line_formula(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let body = trimmed.strip_prefix("$$")?.strip_suffix("$$")?.trim();
    (!body.is_empty()).then_some(body)
}

/// A display formula wrapped in block-quote and/or list-item markup, and the
/// index just past the line that closes it.
///
/// The markdown writer never puts a formula inside a list or a quote, but a
/// hand-written file may: `> $$…$$`, `- $$…$$`, or the fenced form with every
/// line carrying the same prefix. Lifting it is the inverse of nothing the
/// writer writes — it is what makes the reader's formula item independent of
/// how the file happened to arrange the block. The container prefix is checked
/// on every line so a list's formula cannot swallow the prose after the list.
fn lifted_formula(raw: &[String], start: usize) -> Option<(String, usize)> {
    let (quote_depth, after_quotes) = strip_quotes(&raw[start]);
    let (list_indent, content) = match strip_list_content(after_quotes) {
        Some((indent, content)) => (Some(indent), content),
        None => (None, after_quotes),
    };
    // The top-level forms are the existing paths; only a wrapped line is
    // this helper's business.
    if quote_depth == 0 && list_indent.is_none() {
        return None;
    }

    let content = content.trim();
    if let Some(formula) = one_line_formula(content) {
        return Some((formula.to_string(), start + 1));
    }
    if content != "$$" {
        return None;
    }

    let mut body = Vec::new();
    let mut i = start + 1;
    while i < raw.len() {
        let (depth, after_quotes) = strip_quotes(&raw[i]);
        if depth != quote_depth {
            return None;
        }
        if after_quotes.trim().is_empty() && list_indent.is_some() {
            body.push(String::new());
            i += 1;
            continue;
        }
        let line = match list_indent {
            Some(indent) => strip_indent(after_quotes, indent)?,
            None => after_quotes,
        };
        if line.trim() == "$$" {
            return Some((body.join("\n").trim().to_string(), i + 1));
        }
        body.push(line.to_string());
        i += 1;
    }
    None
}

/// Strip any number of leading `>` markers, each with at most one following
/// space, and answer how many there were along with what follows them.
fn strip_quotes(mut line: &str) -> (usize, &str) {
    let mut depth = 0usize;
    loop {
        let trimmed = line.trim_start_matches([' ', '\t']);
        let Some(rest) = trimmed.strip_prefix('>') else {
            break;
        };
        depth += 1;
        line = rest.strip_prefix(' ').unwrap_or(rest);
    }
    (depth, line)
}

/// A list item's content on this line, and the byte column it begins at, so a
/// continuation line can be measured against the same column.
fn strip_list_content(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    for bullet in ['-', '*', '+'] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            if let Some(rest) = rest.strip_prefix(' ') {
                let content = rest.trim_start();
                return Some((line.len() - content.len(), content));
            }
        }
    }

    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &trimmed[digits..];
        if rest.starts_with('.') || rest.starts_with(')') {
            if let Some(rest) = rest[1..].strip_prefix(' ') {
                let content = rest.trim_start();
                return Some((line.len() - content.len(), content));
            }
        }
    }
    None
}

/// Remove exactly `indent` bytes of leading whitespace, when the line has
/// them: the continuation of a list item starts at the item's content column.
fn strip_indent(line: &str, indent: usize) -> Option<&str> {
    if indent == 0 {
        return Some(line);
    }
    let bytes = line.as_bytes();
    if bytes.len() >= indent && bytes[..indent].iter().all(|b| *b == b' ' || *b == b'\t') {
        Some(&line[indent..])
    } else {
        None
    }
}

fn fence_marker(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    for marker in ["```", "~~~"] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            // The closing fence carries no info string; the opening one may.
            if rest.is_empty() || !rest.starts_with(marker) {
                return Some(marker.to_string());
            }
        }
    }
    None
}

/// `### Title` -> `(3, "Title")`, the number of hash marks written.
fn atx_heading(line: &str) -> Option<(i32, String)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim().to_string();
    (!text.is_empty()).then_some((hashes as i32, text))
}

/// A line of `=` or `-` under text is a heading: one or two marks, the same
/// number an ATX heading would have written.
fn setext_marks(line: &str) -> Option<i32> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    if t.chars().all(|c| c == '=') {
        Some(1)
    } else if t.chars().all(|c| c == '-') {
        Some(2)
    } else {
        None
    }
}

/// `| --- | :--: |` alignment row.
fn is_table_delimiter(line: &str) -> bool {
    let t = line.trim();
    if !t.contains('|') {
        return false;
    }
    t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) && t.contains('-')
}

/// Split a pipe-table row into trimmed cells, dropping the outer pipes.
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// A bullet or ordered-list item: `(marker, level, text)`.
fn list_item(line: &str) -> Option<(String, i32, String)> {
    let indent = line.len() - line.trim_start().len();
    let trimmed = line.trim_start();
    let level = (indent / 2) as i32 + 1;

    for bullet in ['-', '*', '+'] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            if rest.starts_with(' ') {
                let text = rest.trim().to_string();
                if !text.is_empty() {
                    return Some((bullet.to_string(), level, text));
                }
            }
        }
    }

    // Ordered: digits followed by `.` or `)`.
    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &trimmed[digits..];
        if let Some(delim) = rest.chars().next().filter(|c| *c == '.' || *c == ')') {
            let after = &rest[1..];
            if after.starts_with(' ') {
                let text = after.trim().to_string();
                if !text.is_empty() {
                    return Some((format!("{}{delim}", &trimmed[..digits]), level, text));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_keeps_every_non_empty_line() {
        let doc = parse_plain(b"first\r\n\r\nsecond\n");
        assert_eq!(
            doc.items,
            vec![Block::paragraph("first"), Block::paragraph("second")]
        );
    }

    /// A blank line ends a block; lines inside one are one item, because a
    /// `.txt` file says nothing else about where a block starts and ends. A
    /// block of several lines keeps its breaks and its indentation — the text
    /// writer writes those verbatim, which is what makes its own output a
    /// fixed point when it is read again.
    #[test]
    fn plain_text_takes_a_blank_line_as_the_end_of_a_block() {
        let doc = parse_plain(b"one\ntwo\n\nthree\n");
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("one\ntwo", "code", None, None),
                Block::paragraph("three"),
            ]
        );

        // Indentation is content, not layout to trim away.
        let doc = parse_plain(b"fn main() {\n    body();\n}\n");
        assert_eq!(
            doc.items,
            vec![Block::labelled(
                "fn main() {\n    body();\n}",
                "code",
                None,
                None
            )]
        );
    }

    /// The writer's pipe grid is read back as the table it is, not as a block
    /// of lines — and a grid followed by prose in the same run keeps the prose.
    #[test]
    fn plain_text_reads_the_writers_grid_as_a_table() {
        let doc = parse_plain(b"| A | B |\n|---|---|\n| x | y |\n");
        assert_eq!(
            doc.items,
            vec![Block::Table {
                grid: TableGrid::from_rows(&[
                    vec!["A".into(), "B".into()],
                    vec!["x".into(), "y".into()],
                ])
            }]
        );

        // A line after the grid is prose, not another row.
        let doc = parse_plain(b"| A |\n|---|\n| x |\nAfter the grid.\n");
        assert_eq!(doc.items.len(), 2, "{:#?}", doc.items);
        assert_eq!(doc.items[1], Block::paragraph("After the grid."));
    }

    /// The writer states a page's head facts as YAML front matter. Reading that
    /// back has to put them beside the model, not into the prose — and in the
    /// order the cards were written.
    #[test]
    fn markdown_front_matter_states_the_pages_head() {
        let source = concat!(
            "---\n",
            "description: Quarterly results & outlook\n",
            "canonical: https://example.com/reports/q3?quarter=3&region=eu\n",
            "social:\n",
            "- key: og:title\n",
            "  content: Q3 & costs\n",
            "- key: og:image\n",
            "  content: https://example.com/q3.png\n",
            "- key: og:image:width\n",
            "  content: '1200'\n",
            "unknown: ignored\n",
            "---\n\n# Q3 review\n",
        );
        let doc = parse_markdown(source.as_bytes());
        assert_eq!(
            doc.description.as_deref(),
            Some("Quarterly results & outlook")
        );
        assert_eq!(
            doc.canonical.as_deref(),
            Some("https://example.com/reports/q3?quarter=3&region=eu")
        );
        assert_eq!(
            doc.social,
            vec![
                ("og:title".to_string(), "Q3 & costs".to_string()),
                (
                    "og:image".to_string(),
                    "https://example.com/q3.png".to_string()
                ),
                ("og:image:width".to_string(), "1200".to_string()),
            ]
        );
        assert_eq!(
            doc.items,
            vec![Block::labelled("Q3 review", "title", None, None)]
        );
    }

    /// The writer gives a title one hash and a section header `level + 1`;
    /// reading them back the way Docling's Markdown backend does is what makes
    /// the two inverses, and a rendered document a fixed point.
    #[test]
    fn markdown_headings_follow_the_writers_depth() {
        let doc = parse_markdown(b"# Title\n\n## Section\n\n### Sub\n\nbody\n");
        assert_eq!(doc.items[0], Block::labelled("Title", "title", None, None));
        assert_eq!(
            doc.items[1],
            Block::labelled("Section", "section_header", Some(1), None)
        );
        assert_eq!(
            doc.items[2],
            Block::labelled("Sub", "section_header", Some(2), None)
        );
        assert_eq!(doc.items[3], Block::paragraph("body"));
    }

    #[test]
    fn markdown_setext_headings_are_recognised() {
        let doc = parse_markdown(b"Title\n=====\n\nSection\n-------\n\nbody\n");
        assert_eq!(doc.items[0], Block::labelled("Title", "title", None, None));
        assert_eq!(
            doc.items[1],
            Block::labelled("Section", "section_header", Some(1), None)
        );
        assert_eq!(doc.items[2], Block::paragraph("body"));
    }

    #[test]
    fn markdown_lists_keep_their_marker_and_depth() {
        let doc = parse_markdown(b"- one\n  - nested\n1. first\n");
        assert_eq!(
            doc.items[0],
            Block::labelled("one", "list_item", Some(1), Some("-".into()))
        );
        assert_eq!(
            doc.items[1],
            Block::labelled("nested", "list_item", Some(2), Some("-".into()))
        );
        assert_eq!(
            doc.items[2],
            Block::labelled("first", "list_item", Some(1), Some("1.".into()))
        );
    }

    #[test]
    fn markdown_pipe_tables_become_grids() {
        let md = b"| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
        let doc = parse_markdown(md);
        assert_eq!(
            doc.items,
            vec![Block::Table {
                grid: TableGrid::from_rows(&[
                    vec!["a".into(), "b".into()],
                    vec!["1".into(), "2".into()],
                    vec!["3".into(), "4".into()],
                ])
            }]
        );
    }

    /// A `$$` block is a formula item, not three paragraphs: the writer fences
    /// a formula that way, and a reader that missed it would change the
    /// equation's shape on every save. The content is trimmed like the writer
    /// trims it, so a rendering read back is the same rendering.
    #[test]
    fn display_math_is_a_formula_not_paragraphs() {
        let doc = parse_markdown(b"$$\nE = mc^2\n$$\n\nAfter.\n");
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("E = mc^2", "formula", None, None),
                Block::paragraph("After."),
            ]
        );

        let doc = parse_markdown(b"$$\n  a + b\n  = c  \n$$\n");
        assert_eq!(
            doc.items,
            vec![Block::labelled("a + b\n  = c", "formula", None, None)]
        );
    }

    /// A display formula written between `$$` on its own line is the same
    /// formula item the fenced form makes. The delimiters and the space just
    /// inside them are layout, not the equation.
    #[test]
    fn one_line_display_math_is_a_formula() {
        let doc = parse_markdown(b"$$E = mc^2$$\n\nAfter.\n");
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("E = mc^2", "formula", None, None),
                Block::paragraph("After."),
            ]
        );

        let doc = parse_markdown(b"  $$ a + b = c $$  \n");
        assert_eq!(
            doc.items,
            vec![Block::labelled("a + b = c", "formula", None, None)]
        );

        // Inline math and a line of nothing but delimiters are prose, not a
        // display block.
        let doc = parse_markdown(b"Inline $x$ math.\n$$$$\n");
        assert_eq!(
            doc.items,
            vec![
                Block::paragraph("Inline $x$ math."),
                Block::paragraph("$$$$"),
            ]
        );
    }

    /// A display formula wrapped in a quote or a list marker is still a formula:
    /// it becomes its own item instead of the container's words. The formula
    /// item carries no marker, because the equation is what the block said.
    #[test]
    fn one_line_display_math_inside_quotes_and_lists_is_lifted() {
        let doc = parse_markdown(b"> $$E = mc^2$$\n\n- $$a + b$$\n");
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("E = mc^2", "formula", None, None),
                Block::labelled("a + b", "formula", None, None),
            ]
        );

        // A wrapped line that is not a formula stays the container's words —
        // the quote's as a quotation, the list item's as an item.
        let doc = parse_markdown(b"> Not math.\n\n- Also not math.\n");
        assert_eq!(
            doc.items,
            vec![
                Block::quoted("Not math."),
                Block::labelled("Also not math.", "list_item", Some(1), Some("-".into())),
            ]
        );
    }

    /// The fenced form inside a container loses the container prefix from every
    /// line: what is left is the equation, indented relative to the list item's
    /// own content column rather than to the page.
    #[test]
    fn fenced_display_math_inside_quotes_and_lists_is_lifted() {
        let doc = parse_markdown(b"> $$\n> a + b\n> = c\n> $$\n\n- $$\n  d + e\n  = f\n  $$\n");
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("a + b\n= c", "formula", None, None),
                Block::labelled("d + e\n= f", "formula", None, None),
            ]
        );

        // Containers nest: the quote is stripped first, then the list item's
        // content column.
        let doc = parse_markdown(b"> - $$\n>   x + y\n>   $$\n");
        assert_eq!(
            doc.items,
            vec![Block::labelled("x + y", "formula", None, None)]
        );
    }

    #[test]
    fn markdown_code_fences_are_kept_verbatim() {
        let doc = parse_markdown(b"```rust\nfn main() {}\n```\n");
        assert_eq!(
            doc.items,
            vec![Block::labelled("fn main() {}", "code", None, None)]
        );
    }

    fn linked(text: &str, label: &str, url: &str) -> Block {
        Block::Text {
            text: text.to_string(),
            label: label.to_string(),
            level: None,
            marker: None,
            hyperlink: Some(url.to_string()),
            quote: false,
        }
    }

    #[test]
    fn a_line_that_is_one_link_becomes_its_label_and_address() {
        let doc = parse_markdown(b"[Visit the spec page](https://example.com/spec)\n");
        assert_eq!(
            doc.items,
            vec![linked(
                "Visit the spec page",
                "paragraph",
                "https://example.com/spec"
            )]
        );
    }

    #[test]
    fn a_link_inside_a_sentence_splits_it_the_way_an_anchor_does() {
        let doc =
            parse_markdown(b"Read the [API guide](https://example.com/api) before you start.\n");
        assert_eq!(
            doc.items,
            vec![
                Block::paragraph("Read the"),
                linked("API guide", "paragraph", "https://example.com/api"),
                Block::paragraph("before you start."),
            ]
        );
    }

    #[test]
    fn headings_and_list_items_keep_their_shape_around_a_link() {
        let doc = parse_markdown(b"## [Release notes](https://example.com/notes)\n\n- [Buy it](https://shop.example/item)\n");
        assert_eq!(
            doc.items,
            vec![
                Block::Text {
                    text: "Release notes".into(),
                    label: "section_header".into(),
                    level: Some(1),
                    marker: None,
                    hyperlink: Some("https://example.com/notes".into()),
                    quote: false,
                },
                Block::Text {
                    text: "Buy it".into(),
                    label: "list_item".into(),
                    level: Some(1),
                    marker: Some("-".into()),
                    hyperlink: Some("https://shop.example/item".into()),
                    quote: false,
                },
            ]
        );
    }

    #[test]
    fn angle_destinations_and_titles_are_read_as_the_address_alone() {
        let doc = parse_markdown(
            b"[map](<https://example.com/a page>)\n\n[home](https://example.com \"Home page\")\n",
        );
        assert_eq!(
            doc.items,
            vec![
                linked("map", "paragraph", "https://example.com/a page"),
                linked("home", "paragraph", "https://example.com"),
            ]
        );
    }

    #[test]
    fn a_label_may_hold_brackets_and_an_image_is_a_picture() {
        let doc = parse_markdown(
            b"[the [draft] spec](https://example.com/draft)\n\n![Image](shot.png)\n",
        );
        assert_eq!(
            doc.items,
            vec![
                linked("the [draft] spec", "paragraph", "https://example.com/draft"),
                Block::Picture {
                    alt: "Image".into(),
                    source: "shot.png".into(),
                },
            ]
        );
    }

    /// A picture is not words, so a sentence is split around it the way it is
    /// around a link: the words before, the picture, the words after.
    #[test]
    fn an_image_inside_a_sentence_splits_it() {
        let doc = parse_markdown(b"See ![a diagram](media/diagram.png) here.\n");
        assert_eq!(
            doc.items,
            vec![
                Block::paragraph("See"),
                Block::Picture {
                    alt: "a diagram".into(),
                    source: "media/diagram.png".into(),
                },
                Block::paragraph("here."),
            ]
        );
    }

    /// An image needs no alt text, and a title after the source is Markdown's
    /// dressing, not part of the address.
    #[test]
    fn an_image_may_have_no_alt_text_and_may_carry_a_title() {
        let doc = parse_markdown(b"![](empty.png)\n\n![alt](titled.png \"A title\")\n");
        assert_eq!(
            doc.items,
            vec![
                Block::Picture {
                    alt: String::new(),
                    source: "empty.png".into(),
                },
                Block::Picture {
                    alt: "alt".into(),
                    source: "titled.png".into(),
                },
            ]
        );
    }

    /// A `data:` URI is one destination, whatever its payload holds: the
    /// base64 alphabet's `/`, `+` and `=` are part of the address, not markup,
    /// so the whole URI is the picture's source and the caller can decode it.
    #[test]
    fn an_embedded_picture_is_a_picture_with_its_whole_uri() {
        let uri = "data:image/png;base64,iVBORw0KGgo+/8=";
        let doc = parse_markdown(format!("![inline]({uri})\n").as_bytes());
        assert_eq!(
            doc.items,
            vec![Block::Picture {
                alt: "inline".into(),
                source: uri.into(),
            }]
        );
    }

    /// A backslash before the `!` is Markdown's escape: the `!` is a literal
    /// bang, not an image marker. What follows is still a link — that is the
    /// syntax the backslash left untouched — and the sentence splits there.
    #[test]
    fn an_escaped_image_marker_is_not_a_picture() {
        let doc = parse_markdown(b"\\![not](a.png)\n");
        assert_eq!(
            doc.items,
            vec![Block::paragraph("\\!"), linked("not", "paragraph", "a.png"),]
        );
    }

    /// The bullet belongs to the line, and the picture carries none: the words
    /// either side keep the list item's label, and the first of them keeps the
    /// marker.
    #[test]
    fn an_image_in_a_list_item_splits_the_item() {
        let doc = parse_markdown(b"- before ![icon](i.png) after\n");
        assert_eq!(
            doc.items,
            vec![
                Block::Text {
                    text: "before".into(),
                    label: "list_item".into(),
                    level: Some(1),
                    marker: Some("-".into()),
                    hyperlink: None,
                    quote: false,
                },
                Block::Picture {
                    alt: "icon".into(),
                    source: "i.png".into(),
                },
                Block::Text {
                    text: "after".into(),
                    label: "list_item".into(),
                    level: Some(1),
                    marker: None,
                    hyperlink: None,
                    quote: false,
                },
            ]
        );
    }

    /// The writer marks a deck's remarks with a bold `Notes:` label inside a
    /// quote; reading one back has to keep it a remark rather than a
    /// paragraph, or the next render turns the notes into slides. An
    /// unlabelled quote is prose — prose the document quoted.
    #[test]
    fn a_labelled_quote_is_a_note_and_a_plain_quote_is_prose() {
        let doc = parse_markdown(
            b"> **Notes:** Check the numbers.\n\n> An ordinary quotation.\n\n> **Notes:** [spec]: https://example.com/spec\n",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::Note {
                    text: "Check the numbers.".into(),
                    hyperlink: None,
                    comment: false,
                },
                Block::quoted("An ordinary quotation."),
                // A link definition is not Markdown inside a note: the remark
                // keeps the words the writer put there.
                Block::Note {
                    text: "[spec]: https://example.com/spec".into(),
                    hyperlink: None,
                    comment: false,
                },
            ]
        );
    }

    /// A quotation's lines are one quotation, and a blank line between two runs
    /// starts another — the rule a paragraph follows anywhere else, so a page
    /// written from these words reads back as the same quotations.
    #[test]
    fn a_quotation_keeps_its_lines_and_its_paragraphs_apart() {
        let doc = parse_markdown(b"> The first line\n> and the second.\n\n> Another.\n");
        assert_eq!(
            doc.items,
            vec![
                Block::quoted("The first line and the second."),
                Block::quoted("Another."),
            ]
        );

        // A quoted blank line separates two paragraphs of the same quotation:
        // the writer never makes one, and it is not words either way.
        let doc = parse_markdown(b"> One.\n>\n> Two.\n");
        assert_eq!(
            doc.items,
            vec![Block::quoted("One."), Block::quoted("Two.")]
        );
    }

    /// A link inside a quotation is still a link: the quoting is the one extra
    /// fact, and it does not put the Markdown spelling of an address back into
    /// the words.
    #[test]
    fn a_quotation_keeps_the_links_inside_it() {
        let doc = parse_markdown(b"> See [the ledger](https://example.com/ledger).\n");
        assert_eq!(
            doc.items,
            vec![
                Block::quoted("See"),
                Block::Text {
                    text: "the ledger".into(),
                    label: "paragraph".into(),
                    level: None,
                    marker: None,
                    hyperlink: Some("https://example.com/ledger".into()),
                    quote: true,
                },
                Block::quoted("."),
            ]
        );
    }

    /// A remark's address belongs to the item, and the writer puts it around
    /// the item's whole first paragraph. A line that is one link is that link,
    /// not its brackets; a link inside a sentence is not a link the remark can
    /// carry, because the model would then claim the rest was linked too.
    #[test]
    fn a_remark_that_is_one_link_keeps_its_address() {
        let doc = parse_markdown(
            b"> **Notes:** [the ledger](https://example.com/ledger)\n\n\
              > **Notes:** See [the ledger](https://example.com/ledger) for the numbers.\n",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::Note {
                    text: "the ledger".into(),
                    hyperlink: Some("https://example.com/ledger".into()),
                    comment: false,
                },
                Block::Note {
                    text: "See [the ledger](https://example.com/ledger) for the numbers.".into(),
                    hyperlink: None,
                    comment: false,
                },
            ]
        );
    }

    /// The link is on the paragraph the remark opens with, so the paragraphs
    /// after it still belong to the same note without being part of the label.
    #[test]
    fn a_linked_remark_keeps_the_paragraphs_after_its_link() {
        let doc = parse_markdown(
            b"> **Notes:** [the ledger](https://example.com/ledger)\n\
              > **Notes:** Then the regions.\n",
        );
        assert_eq!(
            doc.items,
            vec![Block::Note {
                text: "the ledger\nThen the regions.".into(),
                hyperlink: Some("https://example.com/ledger".into()),
                comment: false,
            }]
        );
    }

    /// A comment and a speaker note are both remarks but not the same thing:
    /// the writer labels them differently, and the reader keeps the
    /// distinction so the next render labels each as it was.
    #[test]
    fn a_comment_quote_is_told_apart_from_a_speaker_note() {
        let doc = parse_markdown(
            b"> **Notes:** The presenter's aside.\n\n> **Comment:** [author: Ada]: A reviewer's.\n",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::Note {
                    text: "The presenter's aside.".into(),
                    hyperlink: None,
                    comment: false,
                },
                Block::Note {
                    text: "[author: Ada]: A reviewer's.".into(),
                    hyperlink: None,
                    comment: true,
                },
            ]
        );

        // Touching lines of different kinds are different items: only the same
        // label continues the paragraph before it.
        let doc = parse_markdown(b"> **Notes:** One.\n> **Comment:** Two.\n");
        assert_eq!(
            doc.items,
            vec![
                Block::Note {
                    text: "One.".into(),
                    hyperlink: None,
                    comment: false,
                },
                Block::Note {
                    text: "Two.".into(),
                    hyperlink: None,
                    comment: true,
                },
            ]
        );
    }

    /// The writer puts each of a note's paragraphs on its own labelled line,
    /// with no blank line between them: consecutive lines are one note's
    /// paragraphs. A blank line separates two remarks, so they stay two.
    #[test]
    fn consecutive_note_lines_are_one_note() {
        let doc = parse_markdown(
            b"> **Notes:** Open with the revenue line.\n> **Notes:** Then the regions.\n\n> **Notes:** A second remark.\n",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::Note {
                    text: "Open with the revenue line.\nThen the regions.".into(),
                    hyperlink: None,
                    comment: false,
                },
                Block::Note {
                    text: "A second remark.".into(),
                    hyperlink: None,
                    comment: false,
                },
            ]
        );
    }

    /// The writer marks a slide's and a comment's group with HTML comments that
    /// render as nothing. Reading them back restores the boundaries instead of
    /// flattening the document into one list; every other HTML comment is the
    /// prose it always was.
    #[test]
    fn group_markers_are_boundaries_and_other_comments_are_prose() {
        let doc = parse_markdown(
            b"<!-- pdfextractor:group chapter body slide-0 -->\n\n# Slide\n\n\
              <!-- pdfextractor:group comment_section notes comment-slide1-2 -->\n\n\
              > **Notes:** [author: Ada]: Check the numbers.\n\n\
              <!-- pdfextractor:/group -->\n\n<!-- pdfextractor:/group -->\n\n\
              <!-- just a comment -->\n\n<!-- pdfextractor:group chapter -->\n",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::GroupStart {
                    label: "chapter".into(),
                    name: "slide-0".into(),
                    content_layer: "body".into(),
                },
                Block::Text {
                    text: "Slide".into(),
                    label: "title".into(),
                    marker: None,
                    level: None,
                    hyperlink: None,
                    quote: false,
                },
                Block::GroupStart {
                    label: "comment_section".into(),
                    name: "comment-slide1-2".into(),
                    content_layer: "notes".into(),
                },
                Block::Note {
                    text: "[author: Ada]: Check the numbers.".into(),
                    hyperlink: None,
                    comment: false,
                },
                Block::GroupEnd,
                Block::GroupEnd,
                Block::paragraph("<!-- just a comment -->"),
                // A marker missing its layer and name is malformed, not a
                // group: guessing at it would move the items that follow.
                Block::paragraph("<!-- pdfextractor:group chapter -->"),
            ]
        );
    }

    /// A page written with `--image-export-mode placeholder` says a picture
    /// stood here and keeps no bytes of it. The mark is that picture, not the
    /// words it is spelled with: read as a paragraph, `<!-- image -->` would
    /// travel as prose and the next render would have no picture at all.
    #[test]
    fn a_picture_placeholder_reads_as_a_picture_with_no_image() {
        let doc = parse_markdown(
            b"Before.\n\n<!-- image -->\n\nAfter.\n\n\
              <!-- two pictures -->\n",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::paragraph("Before."),
                Block::PicturePlaceholder,
                Block::paragraph("After."),
                // Another comment is not the writer's mark and stays prose.
                Block::paragraph("<!-- two pictures -->"),
            ]
        );
    }

    /// A marker inside a fence is code, not structure: the fence is read as
    /// one block before any line of it is looked at.
    #[test]
    fn a_marker_inside_a_fence_stays_code() {
        let doc = parse_markdown(
            b"```\n<!-- pdfextractor:group chapter body slide-0 -->\n<!-- pdfextractor:/group -->\n```\n",
        );
        assert_eq!(
            doc.items,
            vec![Block::labelled(
                "<!-- pdfextractor:group chapter body slide-0 -->\n<!-- pdfextractor:/group -->",
                "code",
                None,
                None,
            )]
        );
    }

    #[test]
    fn a_link_with_no_address_is_left_as_written() {
        let doc = parse_markdown(b"[note]() and [unclosed](https://example.com\n");
        assert_eq!(
            doc.items,
            vec![Block::paragraph(
                "[note]() and [unclosed](https://example.com"
            )]
        );
    }
}
