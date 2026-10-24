//! HTML text extraction.
//!
//! The markup is scanned tag by tag, not embedded in a browser: block elements
//! (`p`, `h1`..`h6`, `li`, block containers) become blocks, `<table>` becomes a
//! grid, and anything that is not rendered text (`head`, `script`, `style`,
//! comments) is dropped rather than leaked into the output. Character entities
//! are decoded, so `&amp;` reads as `&`. Two kinds of fact are taken from the
//! head. Its `<title>` names the document: a page whose body has no `h1` has
//! said nowhere else what the document is called, so those words become the
//! document's `title` — the item the writers render as a heading beside the
//! `<title>` they write. A body that does state one keeps it, and the head's
//! title is dropped with the rest of the head. The head also says what the page
//! *is* rather than what it shows: a description
//! (`<meta name="description" content="…">`), the URL the page calls
//! canonical (`<link rel="canonical" href="…">`), and the Open Graph and
//! Twitter card statements a link preview is built from (`<meta
//! property="og:title">`, `<meta name="twitter:card">`, and every other `og:`
//! or `twitter:` key the page states, in the order it states them). None of
//! them is rendered, so none becomes a block; all of them are carried on the
//! [`TextDocument`] so the HTML writer can put them back in the head of the
//! page it writes.
//!
//! The reader is the writer's inverse where the model can say what the markup
//! says. Headings come back at Docling's depths — an `h1` is the document's
//! `title`, an `h2` a level-1 `section_header` — the same depths the Markdown
//! `#`s use, so a rendering neither gains nor loses a level. A
//! `<div class="formula">` is a formula item and a `<pre><code>` block is code,
//! kept with its line breaks. A cell's `rowspan`/`colspan` is the span it
//! covers: the cell is placed where its span starts and the slots it merges
//! hold no cell at all, so a merged table is read as the merged table its
//! markup declared rather than as a rectangle with holes in it.
//!
//! An `<aside class="notes">` is the writer's mark for a deck's remarks: its
//! words are one [`Block::Note`], not prose, so an HTML page written with
//! `--include-notes` reads back onto the notes layer instead of turning a
//! presenter's remark into a paragraph of the document. A reviewer's comment
//! also carries `comment` in its class and a `Comment:` label, which is what
//! keeps it a comment on the next render rather than one more speaker note.
//!
//! An `<img>` with a source is a [`Block::Picture`], not markup to skip: the
//! caller loads and copies the file exactly as it does for a Markdown
//! `![alt](source)`. A picture's alt text stands in where the model holds only
//! words — a table cell, a remark — and for an `img` with no source at all.
//!
//! Responsive markup names several files for one picture. A `<picture>`'s
//! `<source srcset>` candidates come first, then the `img`'s own `srcset`, then
//! its `src` as the fallback a browser would use when nothing better applies;
//! the first candidate in that order is taken. A `<source>` is only a candidate
//! when the page says it can be used: its `media` query has to hold, and its
//! `type` has to be a picture. A feature query — `(min-width: 800px)`,
//! `(prefers-color-scheme: dark)` — asks about a viewport, a colour scheme or a
//! pointer, and a document being read has none, so a source that asks for one
//! is passed over for the next candidate; a query that names only a media type
//! (`all`, `screen`, `print`) asks nothing of the reader and applies. A
//! declared type naming a format this cannot carry — `image/avif`, `image/heic`,
//! or something that is not an image at all — is passed over the same way. The
//! `1x`/`2x` and `480w`/`1200w` descriptors still choose nothing — there is no
//! viewport to resolve them against — so among the candidates that are left, the
//! document's own first choice is the one kept. A candidate written as a `data:`
//! URI is one URL, comma and all: the comma joining its payload does not start
//! the next candidate, so an embedded picture survives its own syntax.

use crate::{decode, Block, TableCell, TableGrid, TextDocument};

/// Parse an HTML document into blocks.
pub fn parse(bytes: &[u8]) -> TextDocument {
    let chars: Vec<char> = decode(bytes).chars().collect();
    let mut state = State::default();
    state.run(&chars);
    TextDocument {
        items: state.items,
        description: state.description,
        canonical: state.canonical,
        social: state.social,
        ..Default::default()
    }
}

struct State {
    items: Vec<Block>,
    buf: String,
    label: String,
    level: Option<i32>,
    marker: Option<String>,

    /// Nesting of `ul`/`ol`; `true` marks an ordered list.
    lists: Vec<bool>,
    counters: Vec<usize>,

    /// The table currently being assembled.
    table: Option<Vec<Vec<Cell>>>,
    row: Option<Vec<Cell>>,
    cell: Option<String>,
    /// The `rowspan`/`colspan` of the cell being read, from its opening tag.
    cell_span: (usize, usize),
    /// Whether the cell being read is a `<td>` or the kind of header its `<th>`
    /// says. The cell's closing tag carries none of this, so it is remembered
    /// from the opening one.
    cell_header: Header,

    /// The `href` of the anchor the words being buffered sit in.
    hyperlink: Option<String>,

    /// How many `<aside class="notes">` elements are open. Their text is
    /// collected in `notes` instead of `buf`, and becomes one notes item when
    /// the outermost closes — however many paragraphs the aside holds.
    notes_depth: usize,
    notes: String,
    /// The hrefs of the anchors inside the remark, in order, and whether each
    /// open anchor's text was marked. A remark's hyperlink belongs to the item,
    /// so only an anchor around the whole first paragraph becomes one.
    notes_hrefs: Vec<String>,
    notes_anchors: Vec<bool>,
    /// Every open `aside`, whether it is one of the remarks, and whether it
    /// is a comment's. A closing tag carries no class, so the stack is what
    /// pairs the two.
    asides: Vec<(bool, bool)>,

    /// Every open `<picture>`, holding the source chosen from its `<source>`
    /// children (`None` until one carries a usable candidate). Markup cannot
    /// nest a picture in a picture, but a stack keeps a malformed one from
    /// stealing the outer picture's choice.
    pictures: Vec<Option<String>>,

    /// Every open `<span>`, and whether it is an inline formula. A closing tag
    /// carries no class, so the stack is what pairs the dollars the HTML writer
    /// leaves out with the span that held them.
    spans: Vec<bool>,

    /// How many `<math>` elements are open. Their tokens are the drawing of an
    /// equation rather than the document's words, so what is read inside them is
    /// not placed as text.
    math_depth: usize,
    /// Whether the text being read is the `<annotation>` that carries the
    /// equation's TeX — the one part of the MathML that is the formula itself.
    in_annotation: bool,
    /// Whether the `<math>` now open carried an annotation at all.
    annotation_seen: bool,
    /// The tokens of the `<math>` now open, kept in case it has no annotation: a
    /// page from elsewhere may hold MathML with no TeX beside it, and then the
    /// tokens are the only words the equation has.
    math_tokens: String,

    /// How many `<blockquote>` elements are open. A paragraph inside one is a
    /// quotation — Docling has no item for that, so the words are read as the
    /// paragraph they are and the quoting is the one extra fact, which the
    /// Markdown writer puts back as `>`.
    quote_depth: usize,

    /// The words of the page's `<title>`, kept while the head is skipped. A
    /// body with no `h1` has said nothing else about what the document is
    /// called, so these become the document's title at the end of the read.
    head_title: Option<String>,

    /// What the head says the page *is*: the summary its `<meta
    /// name="description">` gives and the URL its `<link rel="canonical">`
    /// names. Neither is content, so neither is an item; both travel on the
    /// document so the HTML writer can put them back in the head. See
    /// [`head_facts`].
    description: Option<String>,
    canonical: Option<String>,
    /// The Open Graph and Twitter card statements the head made, in order.
    social: Vec<(String, String)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            buf: String::new(),
            label: "paragraph".to_string(),
            level: None,
            marker: None,
            lists: Vec::new(),
            counters: Vec::new(),
            table: None,
            row: None,
            cell: None,
            cell_span: (1, 1),
            cell_header: Header::Data,
            hyperlink: None,
            notes_depth: 0,
            notes: String::new(),
            notes_hrefs: Vec::new(),
            notes_anchors: Vec::new(),
            asides: Vec::new(),
            pictures: Vec::new(),
            math_depth: 0,
            in_annotation: false,
            annotation_seen: false,
            math_tokens: String::new(),
            spans: Vec::new(),
            quote_depth: 0,
            head_title: None,
            description: None,
            canonical: None,
            social: Vec::new(),
        }
    }
}

impl State {
    fn run(&mut self, chars: &[char]) {
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i] == '<' {
                // Comment. The writers mark group boundaries with one, and
                // those come back as the groups they stood for; every other
                // comment is invisible, as a browser renders it.
                if chars[i..].starts_with(&['<', '!', '-', '-']) {
                    let end = skip_until(chars, i, "-->");
                    let comment: String = chars[i..end].iter().collect();
                    i = end;
                    if self.notes_depth == 0 {
                        if let Some(block) = crate::text::group_marker(&comment) {
                            self.flush();
                            self.items.push(block);
                        } else if comment.trim() == crate::IMAGE_PLACEHOLDER {
                            // The writer's mark for a picture it did not carry:
                            // the picture is still there to be an item, with no
                            // image behind it, rather than becoming the words
                            // the comment is spelled with.
                            self.flush();
                            self.items.push(Block::PicturePlaceholder);
                        }
                    }
                    continue;
                }
                // Declaration (`<!DOCTYPE ...>`).
                if chars.get(i + 1) == Some(&'!') {
                    i = skip_past(chars, i);
                    continue;
                }
                let Some(close) = find_tag_end(chars, i) else {
                    break;
                };
                let tag = parse_tag(&chars[i + 1..close]);
                i = close + 1;

                // Elements whose content is not document text.
                if !tag.closing && matches!(tag.name.as_str(), "script" | "style" | "head") {
                    let end = format!("</{}", tag.name);
                    let close = find_ascii_case_insensitive(chars, i, &end);
                    // The head's title is the one thing in it that names the
                    // document: a browser shows it in the tab, and a page whose
                    // body has no `h1` has no other title. It is kept and
                    // placed only if the body never states one.
                    if tag.name == "head" {
                        self.head_title = head_title(chars, i, close);
                        let facts = head_facts(chars, i, close);
                        self.description = facts.description;
                        self.canonical = facts.canonical;
                        self.social = facts.social;
                    }
                    i = close;
                    continue;
                }
                self.tag(&tag);
            } else {
                let start = i;
                while i < chars.len() && chars[i] != '<' {
                    i += 1;
                }
                let raw: String = chars[start..i].iter().collect();
                self.text(&decode_entities(&raw));
            }
        }
        self.flush();
        self.close_row();
        if let Some(table) = self.table.take() {
            if !table.is_empty() {
                self.items.push(Block::Table {
                    grid: place(&table),
                });
            }
        }
        self.finish_head_title();
    }

    /// Put the head's title on the document when the body never stated one.
    ///
    /// An `h1` is the document's title, so a page with one has already said
    /// what it is called and the head adds nothing: every other part of the
    /// head stays dropped, and this is dropped with it. A body with no `h1`
    /// has not said it, so the head's words become a `title` item at the front
    /// — the position a title has in the model, and the one both writers
    /// render as a heading above the `<title>` they write.
    fn finish_head_title(&mut self) {
        let Some(title) = self.head_title.take() else {
            return;
        };
        let stated = self
            .items
            .iter()
            .any(|item| matches!(item, Block::Text { label, .. } if label == "title"));
        if !stated {
            self.items
                .insert(0, Block::labelled(title, "title", None, None));
        }
    }

    fn tag(&mut self, tag: &Tag) {
        match tag.name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if tag.closing {
                    self.flush();
                } else {
                    self.flush();
                    // Docling's depths: `h1` is the title, and an `h2` is a
                    // section header one level down. The Markdown writer uses
                    // the same mapping for its `#`s, so a heading keeps its
                    // depth when a page is read back.
                    let depth = tag.name[1..].parse::<i32>().unwrap_or(1);
                    if depth == 1 {
                        self.label = "title".to_string();
                        self.level = None;
                    } else {
                        self.label = "section_header".to_string();
                        self.level = Some(depth - 1);
                    }
                }
            }
            // MathML draws the equation; the `<annotation>` inside it is the
            // equation as it was written, and that is what a formula item is.
            // The tokens are collected only in case there is no annotation at
            // all — a `<math>` from somewhere else, with no TeX beside it — and
            // are placed when the element closes if so.
            "math" => {
                if tag.closing {
                    self.math_depth = self.math_depth.saturating_sub(1);
                    if self.math_depth == 0 {
                        self.in_annotation = false;
                        if !self.annotation_seen {
                            let tokens = std::mem::take(&mut self.math_tokens);
                            self.place(&tokens);
                        }
                        self.math_tokens.clear();
                    }
                } else {
                    if self.math_depth == 0 {
                        self.math_tokens.clear();
                        self.annotation_seen = false;
                        self.in_annotation = false;
                    }
                    self.math_depth += 1;
                }
            }
            "annotation" => {
                if tag.closing {
                    self.in_annotation = false;
                } else if self.math_depth > 0 {
                    self.in_annotation = true;
                    self.annotation_seen = true;
                }
            }
            "pre" => {
                self.flush();
                // `<pre>` preserves layout, so its words are a code item, not
                // a paragraph: the Markdown writer turns that back into a
                // fence, and a fence keeps its lines.
                if !tag.closing {
                    self.label = "code".to_string();
                }
            }
            "div" if tag.has_class("formula") => {
                self.flush();
                if !tag.closing {
                    self.label = "formula".to_string();
                }
            }
            "blockquote" => {
                // The words inside are the document's quotation, not its own:
                // the paragraph they make says so. The flush comes first even
                // when closing, so the words a quote ends with — markup with no
                // `<p>` of their own — are still read as quoted.
                self.flush();
                if tag.closing {
                    self.quote_depth = self.quote_depth.saturating_sub(1);
                } else {
                    self.quote_depth += 1;
                }
            }
            "p" | "div" | "section" | "article" | "body" | "main" => {
                self.flush();
            }
            "aside" => {
                self.flush();
                if tag.closing {
                    // A closing tag carries no class, so the stack is what
                    // says whether the aside it closes was a remark.
                    if let Some((notes, comment)) = self.asides.pop() {
                        if notes {
                            self.notes_depth = self.notes_depth.saturating_sub(1);
                            if self.notes_depth == 0 {
                                self.finish_notes(comment);
                            }
                        }
                    }
                } else {
                    let notes = tag.has_class("notes");
                    let comment = tag.has_class("comment");
                    self.asides.push((notes, comment));
                    if notes {
                        self.notes_depth += 1;
                        // One remark, one link at most: the queues describe the
                        // aside being opened, not one that ended before it.
                        if self.notes_depth == 1 {
                            self.notes_hrefs.clear();
                            self.notes_anchors.clear();
                        }
                    }
                }
            }
            "ul" => self.toggle_list(tag, false),
            "ol" => self.toggle_list(tag, true),
            "li" => {
                if tag.closing {
                    self.flush();
                } else {
                    self.flush();
                    self.label = "list_item".to_string();
                    self.level = Some(self.lists.len().max(1) as i32);
                    let ordered = self.lists.last().copied().unwrap_or(false);
                    // `<li value>` restates the number the writer printed when
                    // a list resumed after an interruption, so it wins over
                    // counting the element's position.
                    let counted = match tag.value.as_deref().and_then(|v| v.trim().parse().ok()) {
                        Some(value) => {
                            if let Some(counter) = self.counters.last_mut() {
                                *counter = value;
                            }
                            value
                        }
                        None => self.next_counter(),
                    };
                    // The writer keeps a marker `<ol>` cannot spell — the
                    // `1.1.` of a multi-level list — in `data-marker`; that is
                    // the marker the document printed, so it wins over the
                    // count the element would give the item.
                    self.marker = Some(match tag.marker.as_deref() {
                        Some(marker) if !marker.trim().is_empty() => {
                            decode_entities(marker).trim().to_string()
                        }
                        _ if ordered => format!("{counted}."),
                        _ => "•".to_string(),
                    });
                }
            }
            // An inline formula is a span in the page and `$…$` in the
            // document: the delimiters are restored around its content, so a
            // page read back is the Markdown sentence that made it.
            "span" => {
                if tag.closing {
                    if self.spans.pop().unwrap_or(false) {
                        self.place("$");
                    }
                } else {
                    let formula = tag.has_class("formula");
                    self.spans.push(formula);
                    if formula {
                        self.place("$");
                    }
                }
            }
            "picture" => {
                if tag.closing {
                    self.pictures.pop();
                } else {
                    self.pictures.push(None);
                }
            }
            "source" => {
                // A `<source>` outside a `<picture>` has no image of its own
                // to contribute: it belongs to a media element, not a document
                // picture.
                if !tag.closing {
                    if let Some(chosen) = self.pictures.last_mut() {
                        if chosen.is_none() {
                            *chosen = source_candidate(tag);
                        }
                    }
                }
            }
            "img" => {
                if !tag.closing {
                    self.picture(tag);
                }
            }
            "br" => {
                // A soft break inside a remark is not a paragraph break: the
                // document stores one as a vertical tab, and the writers render
                // it as a space.
                self.place(if self.notes_depth > 0 { "\u{b}" } else { "\n" });
            }
            "a" => {
                // A remark collects its words rather than splitting into items,
                // so an anchor is marked in the text and remembered: whether it
                // wraps the whole first paragraph decides if the note can carry
                // the address, which is how the writer wrote it.
                if self.notes_depth > 0 {
                    if tag.closing {
                        if self.notes_anchors.pop().unwrap_or(false) {
                            self.place(&REMARK_LINK_END.to_string());
                        }
                    } else {
                        let href = tag
                            .href
                            .as_deref()
                            .map(decode_entities)
                            .map(|url| url.trim().to_string())
                            .filter(|url| !url.is_empty());
                        self.notes_anchors.push(href.is_some());
                        if let Some(url) = href {
                            self.notes_hrefs.push(url);
                            self.place(&REMARK_LINK_START.to_string());
                        }
                    }
                    return;
                }
                // The words around a link are not the link: they are sent
                // ahead of it, so the URL ends up on the anchor's own text
                // alone. Neither cut closes the block's shape — an anchor
                // inside a list item splits the item's words, not the list.
                if tag.closing {
                    self.flush_inline();
                    self.hyperlink = None;
                } else {
                    self.flush_inline();
                    self.hyperlink = tag.href.clone();
                }
            }
            "hr" => self.flush(),
            "table" => {
                self.flush();
                if tag.closing {
                    self.close_row();
                    if let Some(table) = self.table.take() {
                        if !table.is_empty() {
                            self.items.push(Block::Table {
                                grid: place(&table),
                            });
                        }
                    }
                } else if self.table.is_none() {
                    self.table = Some(Vec::new());
                }
            }
            "tr" => {
                self.close_row();
                if !tag.closing && self.table.is_some() {
                    self.row = Some(Vec::new());
                }
            }
            "td" | "th" => {
                if tag.closing {
                    self.close_cell();
                } else {
                    self.cell = Some(String::new());
                    self.cell_span = (tag.row_span, tag.col_span);
                    self.cell_header = tag.header();
                }
            }
            "caption" => {
                if tag.closing {
                    self.flush();
                }
            }
            "title" => {}
            _ => {}
        }
    }

    fn toggle_list(&mut self, tag: &Tag, ordered: bool) {
        self.flush();
        if tag.closing {
            self.lists.pop();
            self.counters.pop();
        } else {
            self.lists.push(ordered);
            self.counters.push(0);
        }
    }

    fn next_counter(&mut self) -> usize {
        if let Some(c) = self.counters.last_mut() {
            *c += 1;
            *c
        } else {
            1
        }
    }

    fn text(&mut self, text: &str) {
        if self.math_depth > 0 {
            if self.in_annotation {
                self.place(text);
            } else {
                self.math_tokens.push_str(text);
            }
            return;
        }
        self.place(text);
    }

    /// An `<img>` as the document sees it: a picture of its own when the source
    /// is a real file or address, the way a Markdown `![alt](source)` is. The
    /// words around it are flushed first, so a picture in a sentence splits the
    /// sentence exactly as the Markdown reader splits one.
    ///
    /// A picture cannot live inside text, so where the model has only words — a
    /// table cell, a remark, or an `img` whose `src` is missing — the alt text
    /// stands in for it rather than the image being dropped without trace.
    fn picture(&mut self, tag: &Tag) {
        let alt = tag.alt.as_deref().map(decode_entities).unwrap_or_default();
        // An `<img>` inside a formula is the equation drawn where its TeX could
        // not be typeset: the alt text is that TeX, and the formula's words are
        // what the page is saying. It is not a picture of the document, so it
        // neither splits the sentence around it nor becomes an item.
        if self.label == "formula" || self.spans.last() == Some(&true) {
            if !alt.is_empty() {
                self.place(&alt);
            }
            return;
        }
        // A picture takes the first source its `<source>` children offered;
        // otherwise the `img`'s own responsive candidates, and its `src` last.
        let source = self
            .pictures
            .last_mut()
            .and_then(|chosen| chosen.take())
            .or_else(|| image_source(tag));
        let Some(source) = source else {
            if !alt.is_empty() {
                self.place(&alt);
            }
            return;
        };
        if self.notes_depth > 0 || self.cell.is_some() {
            self.place(&alt);
            return;
        }
        self.flush_inline();
        self.items.push(Block::Picture { alt, source });
    }

    /// Append text to the current block, table cell, or remark.
    fn place(&mut self, text: &str) {
        if self.notes_depth > 0 {
            self.notes.push_str(text);
        } else if let Some(cell) = self.cell.as_mut() {
            cell.push_str(text);
        } else {
            self.buf.push_str(text);
        }
    }

    /// Close the remark being collected: collapse the layout a browser would
    /// collapse, drop the writer's own `Notes:` or `Comment:` label, and place
    /// it on the notes layer. The label also says which kind it is — a comment
    /// keeps that fact, so the next rendering marks it as one again rather than
    /// demoting it to a speaker note. An aside with no words is no remark.
    fn finish_notes(&mut self, comment: bool) {
        let raw = std::mem::take(&mut self.notes);
        let hrefs = std::mem::take(&mut self.notes_hrefs);
        self.notes_anchors.clear();
        // Each block boundary in the aside was a newline, and so is a paragraph
        // break in the document's note; a soft break is a vertical tab, which
        // stays inside its paragraph as a space.
        let paragraphs: Vec<String> = raw
            .replace('\u{b}', " ")
            .split('\n')
            .map(collapse)
            .filter(|paragraph| !paragraph.is_empty())
            .collect();
        let joined = paragraphs.join("\n");
        let (comment, remark) = if let Some(rest) = joined.strip_prefix("Comment:") {
            (true, rest.trim_start())
        } else if let Some(rest) = joined.strip_prefix("Notes:") {
            (comment, rest.trim_start())
        } else {
            (comment, joined.as_str())
        };
        let (text, hyperlink) = remark_link(remark.trim(), &hrefs);
        if !text.is_empty() {
            self.items.push(Block::Note {
                text,
                hyperlink,
                comment,
            });
        }
    }

    fn close_cell(&mut self) {
        if let Some(cell) = self.cell.take() {
            if let Some(row) = self.row.as_mut() {
                let (row_span, col_span) = self.cell_span;
                row.push(Cell {
                    text: collapse(&cell),
                    row_span,
                    col_span,
                    header: self.cell_header,
                });
            }
        }
        self.cell_span = (1, 1);
        self.cell_header = Header::Data;
    }

    fn close_row(&mut self) {
        self.close_cell();
        if let Some(row) = self.row.take() {
            if let Some(table) = self.table.as_mut() {
                if !row.is_empty() {
                    table.push(row);
                }
            }
        }
    }

    fn flush(&mut self) {
        // Inside a remark a block boundary is a break between its words, not a
        // new item: the remark is one note. The shape is reset all the same, so
        // a list opened inside an aside cannot label the paragraph after it.
        if self.notes_depth > 0 {
            if self
                .notes
                .chars()
                .last()
                .is_some_and(|c| !c.is_whitespace())
            {
                self.notes.push('\n');
            }
            self.buf.clear();
            self.label = "paragraph".to_string();
            self.level = None;
            self.marker = None;
            return;
        }
        let text = self.buf.trim().to_string();
        self.buf.clear();
        if !text.is_empty() {
            self.items.push(Block::Text {
                text,
                label: self.label.clone(),
                level: self.level,
                marker: self.marker.clone(),
                // Taken here, so a flush mid-anchor closes the link cleanly.
                hyperlink: self.hyperlink.take(),
                // A quotation is prose: a heading or a list item inside one is
                // still that heading or item, and says so on its own.
                quote: self.quote_depth > 0 && self.label == "paragraph",
            });
        }
        self.label = "paragraph".to_string();
        self.level = None;
        self.marker = None;
    }

    /// Emit what's buffered as its own block, keeping the block's label for
    /// what follows: an anchor inside a list item cuts the item's words in
    /// two, not the list. The bullet and the depth go with the *first* piece
    /// — the one the paragraph printed them on — and the rest keep only the
    /// label they shared, which is the split the DOCX reader makes too.
    fn flush_inline(&mut self) {
        let text = self.buf.trim().to_string();
        self.buf.clear();
        if !text.is_empty() {
            self.items.push(Block::Text {
                text,
                label: self.label.clone(),
                level: self.level.take(),
                marker: self.marker.take(),
                hyperlink: self.hyperlink.take(),
                quote: self.quote_depth > 0 && self.label == "paragraph",
            });
        }
    }
}

/// Collapse runs of whitespace, as a browser renders them.
/// The marks this reader puts around an anchor's text inside a remark, so that
/// its extent is still visible after the words are collapsed into paragraphs.
const REMARK_LINK_START: char = '\u{1}';
const REMARK_LINK_END: char = '\u{2}';

/// A remark's words and the address it carried, if any.
///
/// The writer puts an item's hyperlink around the whole paragraph the item
/// opens with, which leaves the marks this reader wrote enclosing that
/// paragraph exactly. Anything else — a link inside a sentence, two anchors —
/// is not a link the model can hold: claiming the whole remark was linked would
/// widen it on the next render, so the words come back without it.
fn remark_link(text: &str, hrefs: &[String]) -> (String, Option<String>) {
    let without_marks = |words: &str| words.replace([REMARK_LINK_START, REMARK_LINK_END], "");
    if let [url] = hrefs {
        let (first, rest) = match text.split_once('\n') {
            Some((first, rest)) => (first, Some(rest)),
            None => (text, None),
        };
        if let Some(inner) = first
            .strip_prefix(REMARK_LINK_START)
            .and_then(|body| body.strip_suffix(REMARK_LINK_END))
        {
            if !inner.contains(REMARK_LINK_START) && !inner.contains(REMARK_LINK_END) {
                let mut words = inner.to_string();
                if let Some(rest) = rest {
                    words.push('\n');
                    words.push_str(&without_marks(rest));
                }
                return (words, Some(url.clone()));
            }
        }
    }
    (without_marks(text), None)
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a cell's own tag says it is: the two headers HTML names, the `<th>`
/// that leaves its scope unstated, and the data a `<td>` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Header {
    /// A `<td>`: data, which is what a cell that says nothing is.
    #[default]
    Data,
    /// A `<th scope="row">`: the label of its own row, not of a column.
    Row,
    /// A `<th scope="col">`: a column's name, said plainly.
    Column,
    /// A `<th>` with no `scope`: a header whose kind is left to its position,
    /// which is how the row heading a table's columns is usually written.
    Unstated,
}

/// One cell as its row holds it: its words, how many rows and columns it
/// covers, and what its tag said it is. A cell with no span covers one of each.
struct Cell {
    text: String,
    row_span: usize,
    col_span: usize,
    header: Header,
}

/// Lay the cells on the grid their spans describe, and read which rows the
/// markup declared a header.
///
/// A cell keeps the slot it starts in and the rows and columns it covers; the
/// slots a merge covers hold no cell at all, which is how the document model
/// says "this cell spans" rather than "this slot is empty". A row's cells are
/// placed at the first free column, so markup that leaves the covered slots out
/// (as the HTML writer does) and markup that repeats them — an empty `<td>`
/// where a merge already reaches — describe the same table.
fn place(rows: &[Vec<Cell>]) -> TableGrid {
    let num_cols = rows
        .iter()
        .map(|row| row.iter().map(|cell| cell.col_span).sum())
        .max()
        .unwrap_or(0);
    let num_rows = rows.len();
    let mut occupied = vec![vec![false; num_cols]; num_rows];
    let mut cells = Vec::new();
    // What each placed cell's tag said, by grid slot: the header rows are read
    // from where the cells landed, since a merge moves a cell's column.
    let mut said: Vec<(usize, usize, Header)> = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let mut c = 0usize;
        for cell in row {
            while c < num_cols && occupied[r][c] {
                c += 1;
            }
            if c >= num_cols {
                break;
            }
            // A span that runs off the grid covers only what is there: the
            // table's own width is the widest row, not a claim that the last
            // cell reaches beyond it.
            let row_span = cell.row_span.min(num_rows - r);
            let col_span = cell.col_span.min(num_cols - c);
            for dr in 0..row_span {
                for dc in 0..col_span {
                    occupied[r + dr][c + dc] = true;
                }
            }
            cells.push(TableCell {
                text: cell.text.clone(),
                row: r,
                col: c,
                row_span,
                col_span,
            });
            said.push((r, c, cell.header));
            c += col_span;
        }
    }
    TableGrid {
        num_rows,
        num_cols,
        header_rows: header_rows(num_rows, &said),
        cells,
    }
}

/// The grid rows a `<th>` declares a header, in grid order.
///
/// A `<th>` says "this cell heads something", and which something is a scope:
/// `scope="row"` labels the row it sits in, which leaves the columns unspoken
/// for, and `scope="col"` names the column it sits over. Those two are the
/// document's own statement and are taken as it says them, wherever in the
/// table they sit.
///
/// An unstated scope needs one position to be read from: a `<th>` that opens
/// its row beside `<td>`s labels that row, the way `scope="row"` says, because
/// a table with row labels and no scopes is the common spelling of that. The
/// same `<th>` opening a row of nothing but `<th>`s heads the columns. Row 0 is
/// where that distinction never matters — it is the header the unmarked table
/// has always had, and an empty list keeps it — so a row-label table gains no
/// declared header, and a table that names its columns below a caption row now
/// declares the row that does.
fn header_rows(num_rows: usize, said: &[(usize, usize, Header)]) -> Vec<usize> {
    (0..num_rows)
        .filter(|row| {
            let of_row = || said.iter().filter(|(r, _, _)| r == row);
            let data_beside = of_row().any(|(_, _, header)| *header == Header::Data);
            of_row().any(|(_, col, header)| match header {
                Header::Column => true,
                Header::Unstated => *col > 0 || !data_beside,
                Header::Row | Header::Data => false,
            })
        })
        .collect()
}

struct Tag {
    name: String,
    closing: bool,
    href: Option<String>,
    /// A `<meta>`'s `name` attribute, which says what the meta *is* — the
    /// element's own name says only that it is a meta.
    meta_name: Option<String>,
    /// A `<meta>`'s `property` attribute, the other way a meta says what it is:
    /// the Open Graph specification states its keys there rather than in `name`.
    meta_property: Option<String>,
    /// A `<meta>`'s `content`: the statement the element makes.
    content: Option<String>,
    /// A `<link>`'s `rel`, as written: a token list, so a canonical link may
    /// name its relation beside another.
    rel: Option<String>,
    class: Option<String>,
    /// An `img`'s source, exactly as the attribute wrote it.
    src: Option<String>,
    /// An `img`'s or `source`'s responsive candidates, as written.
    srcset: Option<String>,
    /// An `img`'s alt text.
    alt: Option<String>,
    /// A `source`'s media query, as written: the condition a browser would have
    /// to satisfy before it used this candidate.
    media: Option<String>,
    /// A `source`'s declared media type, as written. `type` is a keyword here,
    /// so the field is named for what the attribute holds.
    mime: Option<String>,
    /// How many rows this cell covers; one when it says nothing.
    row_span: usize,
    /// How many columns this cell covers; one when it says nothing.
    col_span: usize,
    /// A `th`'s `scope`, as written: the kind of header the element declares.
    scope: Option<String>,
    /// A list item's printed marker, when it is one `<ol>` cannot spell.
    marker: Option<String>,
    /// An `<li>`'s restated number, when the list resumed mid-count.
    value: Option<String>,
}

impl Tag {
    /// What this cell's element says it is. A `<td>` is data; a `<th>` is a
    /// header whose scope it may state — `row` labels its row, `col` names a
    /// column — and one that states nothing is whichever its position shows it
    /// to be, which [`header_rows`] decides.
    fn header(&self) -> Header {
        if self.name != "th" {
            return Header::Data;
        }
        match self
            .scope
            .as_deref()
            .map(|scope| scope.trim().to_ascii_lowercase())
            .as_deref()
        {
            // A row group's label labels rows, the way a single row's does.
            Some("row" | "rowgroup") => Header::Row,
            Some("col" | "colgroup") => Header::Column,
            _ => Header::Unstated,
        }
    }

    /// The Open Graph or Twitter card key this `<meta>` states, when it states
    /// one: `property="og:title"` or `name="twitter:card"`, as written.
    ///
    /// Open Graph requires `property` and Twitter documents `name`, but both
    /// vocabularies are read off either attribute by real pages and by the
    /// crawlers that consume them, so either is accepted here. The key is kept
    /// as the page spelled it: the writer decides from its prefix which
    /// attribute to put it back in, not from how this reader found it.
    fn social_key(&self) -> Option<String> {
        let keys = [self.meta_property.as_deref(), self.meta_name.as_deref()];
        keys.into_iter()
            .flatten()
            .map(str::trim)
            .find(|key| {
                let lower = key.to_ascii_lowercase();
                // A bare `og:` names no property, so it is not a card.
                ["og:", "twitter:"]
                    .iter()
                    .any(|prefix| lower.starts_with(prefix) && lower.len() > prefix.len())
            })
            .map(str::to_string)
    }

    /// Whether the element carries `name` among its `class` tokens, the way a
    /// browser matches a class selector: whole tokens, any case.
    fn has_class(&self, name: &str) -> bool {
        self.class.as_deref().is_some_and(|classes| {
            classes
                .split_whitespace()
                .any(|token| token.eq_ignore_ascii_case(name))
        })
    }
}

/// The source a `<source>` offers this reader, if it offers one at all: the
/// candidate has to be one the page says can be used — its `media` query has to
/// hold, its `type` has to name a format this can carry — and it has to name a
/// file.
///
/// A `<source>` is a conditional candidate in a browser's hands, and these are
/// the two conditions. Neither is a question a document can ask: it has no
/// viewport to satisfy a feature query with, and it is no particular kind of
/// viewer, so a bare media type holds for it; and it carries a picture file
/// rather than drawing the pixels, so the type it can use is a format it can
/// name. What is left is the page's own order.
fn source_candidate(tag: &Tag) -> Option<String> {
    if !media_applies(tag.media.as_deref()) || !is_picture_type(tag.mime.as_deref()) {
        return None;
    }
    image_source(tag)
}

/// Whether a `media` query holds for a reader that is a document rather than a
/// browser window.
///
/// A query is alternatives separated by commas, and an alternative asks about
/// its surroundings through a feature — written in parentheses:
/// `(min-width: 800px)`, `(prefers-color-scheme: dark)`, `(orientation: portrait)`.
/// Nothing here has a width, a resolution, a colour scheme, an orientation or a
/// pointer, so no feature can hold and a source that asks for one is passed
/// over. An alternative that names only a media type — `all`, `screen`,
/// `print`, `not print` — asks nothing of the reader: it says what kind of
/// place the page is shown in, and a document is shown in all of them. An
/// absent or empty query is the same as `all`.
fn media_applies(media: Option<&str>) -> bool {
    let Some(media) = media.map(str::trim) else {
        return true;
    };
    if media.is_empty() {
        return true;
    }
    media
        .split(',')
        .any(|alternative| !alternative.trim().is_empty() && !alternative.contains('('))
}

/// Whether a `<source>`'s declared `type` is a picture this can carry.
///
/// The carried formats are [`pdfextractor_picture_formats::PICTURE_FORMATS`] —
/// the same list the CLI writes an extracted picture under, so a format cannot be
/// added on one side only. Whether the pixels can be read is a different
/// question: a declared SVG is still a picture the page named, the way a
/// Markdown `![](chart.svg)` is.
///
/// An absent declaration says nothing and is taken at its word — the file itself
/// is then the only thing that says what it is. A declaration is the page's own
/// statement, and a browser uses it to skip a format it cannot show, which is
/// what happens here too: `image/avif`, `image/heic`, `text/html` and a bare
/// `image/` are all passed over for the next candidate, so a page that offers an
/// AVIF first and a JPEG beside it keeps its picture. Parameters
/// (`; charset=…`) are not part of the type, and case does not matter.
fn is_picture_type(mime: Option<&str>) -> bool {
    let Some(mime) = mime else {
        return true;
    };
    let essence = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    pdfextractor_picture_formats::is_carried(&essence)
}

/// The source an image or a `<source>` offers: the first `srcset` candidate,
/// then `src` as the fallback. `srcset` wins because it is the responsive
/// image the page actually displays where the browser understands it; `src`
/// is what the page shows when it does not.
fn image_source(tag: &Tag) -> Option<String> {
    tag.srcset
        .as_deref()
        .map(decode_entities)
        .and_then(|set| srcset_candidate(&set))
        .or_else(|| {
            tag.src
                .as_deref()
                .map(decode_entities)
                .map(|source| source.trim().to_string())
                .filter(|source| !source.is_empty())
        })
}

/// The first URL in a `srcset`, skipping each candidate's width or density
/// descriptor. A reader has no viewport or device pixel ratio with which to
/// choose between `1x`/`2x` or `480w`/`1200w`, so the document's own order is
/// the choice: the first candidate is the picture's source.
///
/// A URL runs to the next whitespace, not to the next comma. The comma before a
/// descriptor separates candidates, but a `data:` URI has one of its own — the
/// one between its media type and its payload — and splitting there would hand
/// back `data:image/png;base64` as a picture with no bytes behind it.
fn srcset_candidate(srcset: &str) -> Option<String> {
    let rest = srcset.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    let url = rest.split_whitespace().next()?.trim_end_matches(',');
    (!url.is_empty()).then(|| url.to_string())
}

/// Parse the inside of a `<...>`: name, whether it closes, and the `href` of
/// an `a` (the only attribute these parsers read).
fn parse_tag(inner: &[char]) -> Tag {
    let mut chars = inner.iter().copied().peekable();
    let mut closing = false;
    if chars.peek() == Some(&'/') {
        closing = true;
        chars.next();
    }
    let mut name = String::new();
    for c in chars.by_ref() {
        if c.is_whitespace() || c == '/' {
            break;
        }
        name.push(c.to_ascii_lowercase());
    }
    let mut href = None;
    let mut class = None;
    let mut meta_name = None;
    let mut meta_property = None;
    let mut content = None;
    let mut rel = None;
    let mut src = None;
    let mut srcset = None;
    let mut alt = None;
    let mut media = None;
    let mut mime = None;
    let mut row_span = 1usize;
    let mut col_span = 1usize;
    let mut scope = None;
    let mut marker = None;
    let mut ordinal = None;
    loop {
        // Skip to the next attribute name.
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == '/') {
            chars.next();
        }
        // The name runs to whitespace, to `=`, or to the tag's end. Breaking
        // on `=` *consumes* it, so remember that instead of peeking for it
        // again below — peeking would find the value's opening quote and
        // decide the attribute had none.
        let mut attr = String::new();
        let mut after_eq = false;
        for c in chars.by_ref() {
            if c == '=' {
                after_eq = true;
                break;
            }
            if c.is_whitespace() || c == '/' {
                break;
            }
            attr.push(c);
        }
        if attr.is_empty() {
            break;
        }
        if !after_eq {
            // `name = value`, the `=` still ahead.
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            if chars.peek() == Some(&'=') {
                chars.next();
                after_eq = true;
            }
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if after_eq {
            match chars.peek().copied() {
                Some(q @ ('\'' | '"')) => {
                    chars.next();
                    for c in chars.by_ref() {
                        if c == q {
                            break;
                        }
                        value.push(c);
                    }
                }
                // A bare value runs to whitespace; `href` inside one keeps
                // its slashes.
                Some(c) if !c.is_whitespace() => {
                    for c in chars.by_ref() {
                        if c.is_whitespace() {
                            break;
                        }
                        value.push(c);
                    }
                }
                _ => {}
            }
        }
        if attr.eq_ignore_ascii_case("href") && !value.is_empty() {
            href = Some(value);
        } else if attr.eq_ignore_ascii_case("name") && !value.is_empty() {
            meta_name = Some(value);
        } else if attr.eq_ignore_ascii_case("property") && !value.is_empty() {
            meta_property = Some(value);
        } else if attr.eq_ignore_ascii_case("content") {
            content = Some(value);
        } else if attr.eq_ignore_ascii_case("rel") && !value.is_empty() {
            rel = Some(value);
        } else if attr.eq_ignore_ascii_case("class") && !value.is_empty() {
            class = Some(value);
        } else if attr.eq_ignore_ascii_case("src") && !value.is_empty() {
            src = Some(value);
        } else if attr.eq_ignore_ascii_case("srcset") && !value.is_empty() {
            srcset = Some(value);
        } else if attr.eq_ignore_ascii_case("alt") {
            // An empty `alt` is a deliberate "no description"; only the absence
            // of the attribute is different, and both read the same here.
            alt = Some(value);
        } else if attr.eq_ignore_ascii_case("media") {
            media = Some(value);
        } else if attr.eq_ignore_ascii_case("type") {
            mime = Some(value);
        } else if attr.eq_ignore_ascii_case("rowspan") {
            row_span = span(&value);
        } else if attr.eq_ignore_ascii_case("colspan") {
            col_span = span(&value);
        } else if attr.eq_ignore_ascii_case("scope") && !value.is_empty() {
            scope = Some(value);
        } else if attr.eq_ignore_ascii_case("data-marker") && !value.is_empty() {
            marker = Some(value);
        } else if attr.eq_ignore_ascii_case("value") && !value.is_empty() {
            ordinal = Some(value);
        }
    }
    Tag {
        name,
        closing,
        href,
        meta_name,
        meta_property,
        content,
        rel,
        class,
        src,
        srcset,
        alt,
        media,
        mime,
        row_span,
        col_span,
        scope,
        marker,
        value: ordinal,
    }
}

/// A span attribute as a count of cells: a missing, unreadable or zero value
/// covers the one slot a cell always covers.
fn span(value: &str) -> usize {
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

/// Index just past the `>` of a tag, respecting quoted attribute values.
fn find_tag_end(chars: &[char], start: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (offset, c) in chars[start..].iter().enumerate() {
        match (quote, *c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"') | (None, '\'') => quote = Some(*c),
            (None, '>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

/// Advance to the end of a `-->` comment.
fn skip_until(chars: &[char], start: usize, marker: &str) -> usize {
    let needle: Vec<char> = marker.chars().collect();
    let mut i = start;
    while i + needle.len() <= chars.len() {
        if chars[i..i + needle.len()] == needle[..] {
            return i + needle.len();
        }
        i += 1;
    }
    chars.len()
}

/// Advance past the next `>`.
fn skip_past(chars: &[char], start: usize) -> usize {
    find_tag_end(chars, start)
        .map(|i| i + 1)
        .unwrap_or(chars.len())
}

/// Position of `needle` (ASCII-case-insensitive) at or after `start`.
/// The words of a page's `<title>`, from a head that is being skipped.
///
/// Nothing in a head is rendered, and none of it is document text — but the
/// title is a *name* for the document, and a page can carry it there while its
/// body has no `h1` to say the same thing. The words are returned with their
/// entities decoded; an empty title is no title, and an element cut off by the
/// end of the head is the words that are there.
fn head_title(chars: &[char], head_start: usize, head_end: usize) -> Option<String> {
    let open = find_ascii_case_insensitive(&chars[..head_end], head_start, "<title");
    let after = open + "<title".len();
    if after >= head_end {
        return None;
    }
    // `<titles>` is not a title: the name ends where the element's does.
    match chars.get(after) {
        Some(character) if *character == '>' || character.is_ascii_whitespace() => {}
        _ => return None,
    }
    let text_start = chars[after..head_end].iter().position(|c| *c == '>')? + after + 1;
    let close = find_ascii_case_insensitive(&chars[..head_end], text_start, "</title");
    let words: String = chars[text_start..close].iter().collect();
    // A title is one line on the tab however it is wrapped in the source, so
    // its whitespace is collapsed the way a browser renders it.
    let words = decode_entities(&words)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!words.is_empty()).then_some(words)
}

/// What a page's head says it *is*, from a head that is being skipped: the
/// description a `<meta name="description" content="…">` gives, the URL a
/// `<link rel="canonical" href="…">` names, and the Open Graph and Twitter
/// card statements a link preview reads.
///
/// A browser shows none of it, so none of it is document text — but all of it is
/// the page's own statement about itself, and the HTML writer puts it back in the
/// head of the page it writes. Only the first description and the first canonical
/// URL are kept: each is a single fact, so a second one is a mistake rather than
/// another statement. `rel` is a token list, so `rel="canonical alternate"`
/// still names the canonical URL. A value is entity-decoded, the way the title
/// is, and one that is only whitespace says nothing.
///
/// The cards are a list rather than one fact, because a card is: `og:image`
/// states each image, and `og:image:width` and `og:alt` state something *about*
/// the image before them, so the order the head wrote them in is part of what
/// they say and a repeat is another statement, not a mistake. Both vocabularies
/// travel together: a `twitter:` key and an `og:` key are the same kind of
/// `<meta>` about the same page, and the key says which reads it.
#[derive(Default)]
struct HeadFacts {
    description: Option<String>,
    canonical: Option<String>,
    social: Vec<(String, String)>,
}

fn head_facts(chars: &[char], head_start: usize, head_end: usize) -> HeadFacts {
    let mut facts = HeadFacts::default();
    let mut i = head_start;
    while i < head_end {
        let Some(offset) = chars[i..head_end].iter().position(|c| *c == '<') else {
            break;
        };
        let open = i + offset;
        // A tag is bounded by the head: an element cut off by it has no end.
        let Some(close) = find_tag_end(&chars[..head_end], open) else {
            break;
        };
        let tag = parse_tag(&chars[open + 1..close]);
        i = close + 1;
        if tag.closing {
            continue;
        }
        match tag.name.as_str() {
            "meta" => {
                let content = tag
                    .content
                    .as_deref()
                    .map(decode_entities)
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty());
                let about_the_page = tag
                    .meta_name
                    .as_deref()
                    .is_some_and(|name| name.trim().eq_ignore_ascii_case("description"));
                if about_the_page && facts.description.is_none() {
                    facts.description = content.clone();
                }
                // A card states itself in a `<meta>` too, under a key that names
                // its vocabulary. An empty `content` is a meta that says nothing,
                // and one the head left without a content attribute says nothing
                // at all — neither is a card.
                if let (Some(key), Some(content)) = (tag.social_key(), content) {
                    facts.social.push((key, content));
                }
            }
            "link" if facts.canonical.is_none() => {
                let names_the_page = tag.rel.as_deref().is_some_and(|rel| {
                    rel.split_whitespace()
                        .any(|token| token.eq_ignore_ascii_case("canonical"))
                });
                if names_the_page {
                    facts.canonical = tag
                        .href
                        .as_deref()
                        .map(decode_entities)
                        .map(|url| url.trim().to_string())
                        .filter(|url| !url.is_empty());
                }
            }
            _ => {}
        }
    }
    facts
}

fn find_ascii_case_insensitive(chars: &[char], start: usize, needle: &str) -> usize {
    let needle: Vec<char> = needle.chars().collect();
    let mut i = start;
    while i + needle.len() <= chars.len() {
        let matches = chars[i..i + needle.len()]
            .iter()
            .zip(&needle)
            .all(|(a, b)| a.to_ascii_lowercase() == *b);
        if matches {
            return i;
        }
        i += 1;
    }
    chars.len()
}

/// Decode HTML character entities, named and numeric.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let Some(semi) = chars[i..].iter().position(|c| *c == ';').map(|p| i + p) else {
            out.push('&');
            i += 1;
            continue;
        };
        if semi - i > 10 {
            out.push('&');
            i += 1;
            continue;
        }
        let entity: String = chars[i + 1..semi].iter().collect();
        match resolve_entity(&entity) {
            Some(c) => {
                out.push(c);
                i = semi + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

fn resolve_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => return Some('&'),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "quot" => return Some('"'),
        "apos" => return Some('\''),
        "nbsp" => return Some('\u{00A0}'),
        "ndash" => return Some('\u{2013}'),
        "mdash" => return Some('\u{2014}'),
        "hellip" => return Some('\u{2026}'),
        "copy" => return Some('\u{00A9}'),
        "reg" => return Some('\u{00AE}'),
        "trade" => return Some('\u{2122}'),
        _ => {}
    }
    if let Some(digits) = entity.strip_prefix('#') {
        let code = if let Some(hex) = digits.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            digits.parse::<u32>().ok()?
        };
        return char::from_u32(code);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(doc: &TextDocument) -> Vec<String> {
        doc.items
            .iter()
            .filter_map(|b| match b {
                Block::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// `<blockquote>` is the page saying these words are not the document's
    /// own: each paragraph inside it is read as a quotation, so the Markdown
    /// written from the page keeps the `>`. A heading inside one is still a
    /// heading — the quote is prose, not a label.
    #[test]
    fn a_blockquote_is_read_as_a_quotation() {
        let doc = parse(
            b"<p>Before.</p><blockquote><p>Quoted.</p><p>Also quoted.</p></blockquote>\
              <blockquote>Loose words.</blockquote><p>After.</p>",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::paragraph("Before."),
                Block::quoted("Quoted."),
                Block::quoted("Also quoted."),
                Block::quoted("Loose words."),
                Block::paragraph("After."),
            ]
        );

        let doc = parse(b"<blockquote><h2>Still a heading</h2></blockquote>");
        assert_eq!(
            doc.items,
            vec![Block::labelled(
                "Still a heading",
                "section_header",
                Some(1),
                None
            )]
        );
    }

    #[test]
    fn headings_and_paragraphs_are_separated() {
        let doc = parse(b"<h1>Title</h1><h2>Section</h2><p>Hello <b>world</b>.</p>");
        assert_eq!(doc.items[0], Block::labelled("Title", "title", None, None));
        assert_eq!(
            doc.items[1],
            Block::labelled("Section", "section_header", Some(1), None)
        );
        assert_eq!(doc.items[2], Block::paragraph("Hello world."));
    }

    /// The writer's own elements come back as the items they were: a formula
    /// is a `formula`, a code block is `code` with its lines intact, and a
    /// table's `rowspan`/`colspan` are the spans a cell covers — the cells the
    /// merge covers are not cells at all, the way the document model holds a
    /// merged table, so the table can be written back with them.
    #[test]
    fn formulas_code_and_merged_cells_are_read_back() {
        let doc = parse(
            b"<div class=\"formula\">E = mc^2</div><pre><code>fn main() {\n    run();\n}</code></pre>",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("E = mc^2", "formula", None, None),
                Block::labelled("fn main() {\n    run();\n}", "code", None, None),
            ]
        );

        let doc = parse(
            b"<table><tr><th>Class</th><th>Student</th><th colspan=\"2\">Results</th></tr><tr><td rowspan=\"2\">A</td><td>Alice</td><td>71</td><td>84</td></tr><tr><td>Bob</td><td>65</td><td>79</td></tr></table>",
        );
        assert_eq!(
            doc.items,
            vec![Block::Table {
                grid: TableGrid {
                    num_rows: 3,
                    num_cols: 4,
                    // The first row's cells are `<th>`s, so the markup heads
                    // the columns there — the statement the default only
                    // guessed at.
                    header_rows: vec![0],
                    cells: vec![
                        // `Results` reaches across the last two columns, and
                        // `A` down into the third row — so `Bob` starts the
                        // third row at column 1, not at the empty slot the
                        // merge covers.
                        cell("Class", 0, 0, 1, 1),
                        cell("Student", 0, 1, 1, 1),
                        cell("Results", 0, 2, 1, 2),
                        cell("A", 1, 0, 2, 1),
                        cell("Alice", 1, 1, 1, 1),
                        cell("71", 1, 2, 1, 1),
                        cell("84", 1, 3, 1, 1),
                        cell("Bob", 2, 1, 1, 1),
                        cell("65", 2, 2, 1, 1),
                        cell("79", 2, 3, 1, 1),
                    ],
                }
            }]
        );
    }

    /// One placed cell, for the table tests above.
    fn cell(text: &str, row: usize, col: usize, row_span: usize, col_span: usize) -> TableCell {
        TableCell {
            text: text.to_string(),
            row,
            col,
            row_span,
            col_span,
        }
    }

    /// The HTML writer typesets an equation as MathML with its TeX kept in the
    /// `<annotation>`; the formula item is that TeX, not the tokens drawn around
    /// it. A `<mtext>` in the drawing is the one place MathML carries words, so
    /// it is what a reader that took the text would get wrong.
    #[test]
    fn a_typeset_equation_reads_back_as_its_annotation() {
        let doc = parse(
            br#"<div class="formula"><math display="block" xmlns="http://www.w3.org/1998/Math/MathML"><semantics><mtext>if&nbsp;</mtext><mi>x</mi><mo>&gt;</mo><mn>0</mn><annotation encoding="application/x-tex">\text{if } x &gt; 0</annotation></semantics></math></div>"#,
        );
        assert_eq!(
            doc.items,
            vec![Block::labelled("\\text{if } x > 0", "formula", None, None)]
        );
    }

    /// MathML with no annotation at all still says something — the tokens spell
    /// the equation as well as anything can — so a page written elsewhere is
    /// not left with a hole where its formula stood.
    #[test]
    fn mathml_without_an_annotation_keeps_its_tokens() {
        let doc = parse(
            b"<div class=\"formula\"><math><mi>E</mi><mo>=</mo><mi>m</mi></math></div><p>After.</p>",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("E=m", "formula", None, None),
                Block::paragraph("After."),
            ]
        );
    }

    /// A formula the writer could not typeset is drawn as an `<img>` whose alt
    /// text is the TeX that drew it: the image is the equation, and the formula
    /// item is its alt text — not a picture of one.
    #[test]
    fn a_drawn_formula_reads_back_as_its_latex() {
        let doc = parse(
            b"<div class=\"formula\"><img src=\"data:image/svg+xml;base64,PHN2Zy8+\" alt=\"\\ocirc{a}\"></div><p>After.</p>",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("\\ocirc{a}", "formula", None, None),
                Block::paragraph("After."),
            ]
        );
        // The picture did not split the formula into an image item beside it.
        assert!(
            !doc.items
                .iter()
                .any(|item| matches!(item, Block::Picture { .. })),
            "{:#?}",
            doc.items
        );
    }

    /// The same drawing inside a sentence: the alt text is the formula, so the
    /// span reads back with its dollars around it.
    #[test]
    fn a_drawn_inline_formula_reads_back_as_dollars() {
        let doc = parse(
            b"<p>The value <span class=\"formula\"><img src=\"data:image/svg+xml;base64,PHN2Zy8+\" alt=\"\\ocirc{a}\"></span> stands.</p>",
        );
        assert_eq!(texts(&doc), vec!["The value $\\ocirc{a}$ stands."]);
    }

    /// An inline formula is a span in HTML and `$…$` in the document: the
    /// reader restores the delimiters around the span's content, so the
    /// sentence reads back as the one the Markdown writer wrote.
    #[test]
    fn an_inline_formula_span_reads_back_as_dollars() {
        let doc =
            parse(b"<p>The area is <span class=\"formula\">a^2</span>, not <span>a</span>.</p>");
        assert_eq!(texts(&doc), vec!["The area is $a^2$, not a."]);

        // The same span holding typeset MathML: the annotation is the formula,
        // and the drawing around it is not words.
        let doc = parse(
            br#"<p>The area is <span class="formula"><math display="inline" xmlns="http://www.w3.org/1998/Math/MathML"><semantics><msup><mi>a</mi><mn>2</mn></msup><annotation encoding="application/x-tex">a^2</annotation></semantics></math></span>.</p>"#,
        );
        assert_eq!(texts(&doc), vec!["The area is $a^2$."]);
    }

    /// The same span inside a list item or a quote: the restoration belongs to
    /// the span, not to the paragraph, so a container's sentence reads back
    /// with the dollars it had, its marker and its remark intact.
    #[test]
    fn an_inline_formula_span_reads_back_as_dollars_in_containers() {
        let doc = parse(
            b"<ul><li>The perimeter is <span class=\"formula\">2\\pi r</span>.</li></ul> \
              <aside class=\"notes\"><p><strong>Notes:</strong> the slope is \
              <span class=\"formula\">m x + b</span>.</p></aside>",
        );
        assert_eq!(texts(&doc), vec!["The perimeter is $2\\pi r$."]);
        assert!(
            doc.items.iter().any(|item| matches!(
                item,
                Block::Text {
                    marker: Some(_),
                    ..
                }
            )),
            "the list item lost its marker: {:#?}",
            doc.items
        );
        // The remark keeps its own block, its label already taken off, with
        // the formula's dollars restored like any other sentence's.
        let note = doc.items.iter().find_map(|item| match item {
            Block::Note {
                text,
                comment: false,
                ..
            } => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(
            note.map(|text| text.split_whitespace().collect::<Vec<_>>().join(" ")),
            Some("the slope is $m x + b$.".to_string()),
            "the quote's formula was not read back: {:#?}",
            doc.items
        );
    }

    #[test]
    fn script_and_head_content_is_not_text() {
        let doc = parse(
            b"<head><meta charset=\"utf-8\"><style>p { color: red }</style></head>\
              <body><script>var a=1;</script><p>real</p></body>",
        );
        assert_eq!(texts(&doc), vec!["real"]);
    }

    /// A page's title can live only in the head: the tab says what the document
    /// is called and the body has no `h1` to repeat it. Those words are the
    /// document's title, so a page read and written back does not lose it.
    #[test]
    fn a_head_title_is_the_documents_title_when_the_body_states_none() {
        let doc = parse(b"<head><title>Q3 &amp; Q4</title></head><body><p>real</p></body>");
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("Q3 & Q4", "title", None, None),
                Block::paragraph("real"),
            ]
        );

        // The element's own name is case-insensitive and may carry attributes.
        let doc = parse(b"<HEAD><TITLE lang=\"en\">Upper</TITLE></HEAD><body><p>x</p></body>");
        assert_eq!(doc.items[0], Block::labelled("Upper", "title", None, None));

        // One line on the tab however the source wraps it.
        let doc = parse(b"<head><title>Wrapped\n    title</title></head><body><p>x</p></body>");
        assert_eq!(
            doc.items[0],
            Block::labelled("Wrapped title", "title", None, None)
        );

        // An empty title is no title at all, and a longer element name is not it.
        for page in [
            b"<head><title>  </title></head><body><p>x</p></body>".as_slice(),
            b"<head><titles>not a title</titles></head><body><p>x</p></body>",
            b"<head></head><body><p>x</p></body>",
        ] {
            let doc = parse(page);
            assert!(
                !doc.items.iter().any(|item| matches!(
                    item,
                    Block::Text { label, .. } if label == "title"
                )),
                "{page:?} made a title"
            );
        }
    }

    /// A page's description and canonical URL are not rendered, so neither is a
    /// block — but both are what the page says it *is*, so both travel on the
    /// document and the HTML writer puts them back in the head.
    #[test]
    fn the_head_states_the_pages_description_and_canonical_url() {
        let doc = parse(
            b"<head><meta charset=\"utf-8\">\
              <meta name=\"description\" content=\"Q3 &amp; costs.\">\
              <link rel=\"canonical\" href=\"https://example.com/q3?a=1&amp;b=2\"></head>\
              <body><p>real</p></body>",
        );
        assert_eq!(doc.description.as_deref(), Some("Q3 & costs."));
        assert_eq!(
            doc.canonical.as_deref(),
            Some("https://example.com/q3?a=1&b=2")
        );
        // Neither is text: the page's body is all it says to a reader.
        assert_eq!(texts(&doc), vec!["real"]);

        // Attributes may come in either order, in either quote, in any case, and
        // `rel` is a token list rather than one word.
        let doc = parse(
            b"<HEAD><LINK HREF='https://example.com/a' REL='alternate CANONICAL'>\
              <META CONTENT=\"Trimmed  \" NAME=\"Description\"></HEAD><body><p>x</p></body>",
        );
        assert_eq!(doc.description.as_deref(), Some("Trimmed"));
        assert_eq!(doc.canonical.as_deref(), Some("https://example.com/a"));

        // A meta that is not a description, a link that is not canonical, and a
        // head that states neither all say nothing.
        for page in [
            b"<head><meta name=\"viewport\" content=\"width=device-width\"></head><body><p>x</p></body>".as_slice(),
            b"<head><link rel=\"stylesheet\" href=\"page.css\"></head><body><p>x</p></body>",
            b"<head><meta name=\"description\" content=\"  \"></head><body><p>x</p></body>",
            b"<head></head><body><p>x</p></body>",
            b"<p>no head at all</p>",
        ] {
            let doc = parse(page);
            assert_eq!(doc.description, None, "{page:?} made a description");
            assert_eq!(doc.canonical, None, "{page:?} named a canonical URL");
        }

        // The first statement of each is the page's: a second one contradicts it
        // rather than adding to it.
        let doc = parse(
            b"<head><meta name=\"description\" content=\"first\">\
              <meta name=\"description\" content=\"second\">\
              <link rel=\"canonical\" href=\"https://example.com/one\">\
              <link rel=\"canonical\" href=\"https://example.com/two\"></head><body><p>x</p></body>",
        );
        assert_eq!(doc.description.as_deref(), Some("first"));
        assert_eq!(doc.canonical.as_deref(), Some("https://example.com/one"));
    }

    /// A page's link preview is stated in its head as well: Open Graph's keys in
    /// `property`, the Twitter card's in `name`. Neither is rendered, so neither
    /// is a block — but both travel on the document, in the order the head wrote
    /// them, because an `og:image:width` says something about the image before
    /// it and a card is otherwise a collection with no shape.
    #[test]
    fn the_head_states_the_pages_social_cards() {
        let doc = parse(
            b"<head>\
              <meta property=\"og:title\" content=\"Q3 &amp; costs\">\
              <meta property=\"og:image\" content=\"https://example.com/q3.png\">\
              <meta property=\"og:image:width\" content=\"1200\">\
              <meta name=\"twitter:card\" content=\"summary_large_image\">\
              <meta property=\"twitter:site\" content=\"@example\">\
              </head><body><p>real</p></body>",
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
                (
                    "twitter:card".to_string(),
                    "summary_large_image".to_string()
                ),
                ("twitter:site".to_string(), "@example".to_string()),
            ]
        );
        // A card is not document text.
        assert_eq!(texts(&doc), vec!["real"]);

        // A meta that says nothing is no card, a key that is not a card's is no
        // card, and a meta outside the head is body markup rather than a
        // statement about the page.
        for page in [
            b"<head><meta property=\"og:title\" content=\"  \"></head><body><p>x</p></body>".as_slice(),
            b"<head><meta property=\"og:title\"></head><body><p>x</p></body>",
            b"<head><meta property=\"og:\" content=\"nameless\"></head><body><p>x</p></body>",
            b"<head><meta name=\"viewport\" content=\"width=device-width\"></head><body><p>x</p></body>",
            b"<head></head><body><p>x</p></body>",
            b"<body><p>x</p><meta property=\"og:title\" content=\"in the body\"></body>",
        ] {
            let doc = parse(page);
            assert!(doc.social.is_empty(), "{page:?} made a card: {:?}", doc.social);
        }

        // A description and a card live side by side, and a key is kept as the
        // page spelled it — which vocabulary it belongs to is the reader's
        // business, not a correction it makes to the page.
        let doc = parse(
            b"<head><meta name=\"description\" content=\"In short.\">\
              <meta PROPERTY=\"OG:Title\" CONTENT=\"Spelled as written\"></head>\
              <body><p>x</p></body>",
        );
        assert_eq!(doc.description.as_deref(), Some("In short."));
        assert_eq!(
            doc.social,
            vec![("OG:Title".to_string(), "Spelled as written".to_string())]
        );
    }

    /// An `h1` is the body saying what the document is called. When it is there
    /// the head's title adds nothing, and it stays dropped with the rest of the
    /// head — one title, not two.
    #[test]
    fn a_body_title_wins_over_the_heads() {
        let doc =
            parse(b"<head><title>Tab title</title></head><body><h1>Page title</h1><p>x</p></body>");
        let titles: Vec<&str> = doc
            .items
            .iter()
            .filter_map(|item| match item {
                Block::Text { text, label, .. } if label == "title" => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(titles, vec!["Page title"]);
    }

    #[test]
    fn entities_are_decoded() {
        let doc = parse(b"<p>a &amp; b &#8212; c &lt;d&gt;</p>");
        assert_eq!(texts(&doc), vec!["a & b \u{2014} c <d>"]);
    }

    /// A table's columns can be named below a first row that does not name
    /// them — a caption row, a unit row, a group title. The `<th>`s are the
    /// document saying where its column names are, so the grid declares that
    /// row a header and row 0 stops being one by default.
    #[test]
    fn a_table_that_heads_its_columns_below_the_first_row_keeps_that_header() {
        let doc = parse(
            b"<table><tr><td colspan=\"2\">Monthly totals</td></tr><tr><th>Month</th><th>Total</th></tr><tr><td>January</td><td>10</td></tr></table>",
        );
        let Block::Table { grid } = &doc.items[0] else {
            panic!("no table: {:#?}", doc.items);
        };
        assert_eq!(grid.header_rows, vec![1]);
        assert!(
            !grid.header_row(0),
            "the group title above the names is not the names"
        );
        assert!(grid.header_row(1));
        assert!(!grid.header_row(2));
    }

    /// A `<th>` that opens its row beside `<td>`s labels that row, not a
    /// column: a table whose first column is a header column has no row that
    /// heads columns, so the first-row default stands and the labels are not
    /// claimed as one.
    #[test]
    fn a_th_beside_data_labels_its_row_and_not_the_columns() {
        let doc = parse(
            b"<table><tr><th>Region</th><th>Sales</th></tr><tr><th>North</th><td>10</td></tr><tr><th>South</th><td>12</td></tr></table>",
        );
        let Block::Table { grid } = &doc.items[0] else {
            panic!("no table: {:#?}", doc.items);
        };
        assert_eq!(grid.header_rows, vec![0], "only the row of names");
        assert!(grid.header_row(0));
        assert!(!grid.header_row(1));
        assert!(!grid.header_row(2));
    }

    /// `scope="row"` states what position could only guess: the `<th>` labels
    /// its own row. A lone `<th>` row is the header when it says nothing and
    /// not one when it says it labels a row — the same markup, told apart by
    /// the document's own words.
    #[test]
    fn a_scoped_row_header_is_not_a_column_header() {
        let read = |markup: &[u8]| {
            let doc = parse(markup);
            let Block::Table { grid } = &doc.items[0] else {
                panic!("no table: {:#?}", doc.items);
            };
            grid.header_rows.clone()
        };
        let unstated =
            read(b"<table><tr><th>Month</th><th>Total</th></tr><tr><th>January</th></tr></table>");
        assert_eq!(unstated, vec![0, 1], "an unstated th heads the columns");
        let scoped = read(
            b"<table><tr><th>Month</th><th>Total</th></tr><tr><th scope=\"row\">January</th></tr></table>",
        );
        assert_eq!(scoped, vec![0], "a scoped th labels its row");
    }

    #[test]
    fn tables_become_grids() {
        let doc =
            parse(b"<table><tr><td>a</td><td>b</td></tr><tr><td>1</td><td>2</td></tr></table>");
        assert_eq!(
            doc.items,
            vec![Block::Table {
                grid: TableGrid::from_rows(&[
                    vec!["a".into(), "b".into()],
                    vec!["1".into(), "2".into()],
                ])
            }]
        );
    }

    /// The numbers a list printed are the document's, not the element's
    /// count: a resumed list's `value` and a multi-level marker the writer
    /// kept in `data-marker` both come back.
    #[test]
    fn printed_list_markers_and_resumed_numbers_come_back() {
        let doc = parse(
            b"<ol><li value=\"1\">One</li><li value=\"1\" data-marker=\"1.1.\">Two</li><li value=\"2\">Six</li></ol>",
        );
        assert_eq!(
            doc.items[0],
            Block::labelled("One", "list_item", Some(1), Some("1.".into()))
        );
        assert_eq!(
            doc.items[1],
            Block::labelled("Two", "list_item", Some(1), Some("1.1.".into()))
        );
        assert_eq!(
            doc.items[2],
            Block::labelled("Six", "list_item", Some(1), Some("2.".into()))
        );
    }

    #[test]
    fn list_items_are_labelled_with_depth() {
        let doc = parse(b"<ul><li>one<ul><li>two</li></ul></li></ul>");
        assert_eq!(
            doc.items[0],
            Block::labelled("one", "list_item", Some(1), Some("•".into()))
        );
        assert_eq!(
            doc.items[1],
            Block::labelled("two", "list_item", Some(2), Some("•".into()))
        );
    }

    /// An `<img>` with a local source is a picture of its own, and a picture
    /// inside a sentence splits it the way a Markdown image does: the words
    /// before, the picture, the words after.
    #[test]
    fn an_image_is_a_picture_and_splits_its_sentence() {
        let doc = parse(b"<p>Before <img src=\"shot.png\" alt=\"a shot\"> after.</p>");
        assert_eq!(
            doc.items,
            vec![
                Block::paragraph("Before"),
                Block::Picture {
                    alt: "a shot".into(),
                    source: "shot.png".into(),
                },
                Block::paragraph("after."),
            ]
        );
    }

    /// Responsive markup names several files for one picture. A `<picture>`'s
    /// `<source>` candidate comes first, then the `img`'s own `srcset`, then
    /// its `src`. The first usable candidate wins, because there is no viewport
    /// or device pixel ratio here to choose after that.
    #[test]
    fn a_responsive_image_takes_its_first_usable_source() {
        // The first `<source>` asks for a viewport at least 800px wide, which a
        // document has no way to be, so the unconditional source beside it is
        // the page's picture.
        let doc = parse(
            br#"<p><picture><source media="(min-width: 800px)" srcset="phone.webp 480w, desk.webp 1200w"><source srcset="print.png 2x"><img src="fallback.png" alt="hero"></picture></p>"#,
        );
        assert_eq!(
            doc.items,
            vec![Block::Picture {
                alt: "hero".into(),
                source: "print.png".into(),
            }]
        );

        let doc =
            parse(br#"<img srcset="small.png 1x, large.png 2x" src="fallback.png" alt="shot">"#);
        assert_eq!(
            doc.items,
            vec![Block::Picture {
                alt: "shot".into(),
                source: "small.png".into(),
            }]
        );
    }

    /// A `data:` URI is one URL: its own comma joins the payload to the type
    /// and does not start another candidate, in `src` or in a `srcset`. The
    /// whole URI is the picture's source, so an embedded render stays a picture
    /// the caller can decode rather than becoming `data:image/png;base64`.
    #[test]
    fn an_embedded_picture_keeps_its_payload_whole() {
        let uri = "data:image/png;base64,iVBORw0KGgo=";
        for markup in [
            format!(r#"<img src="{uri}" alt="inline">"#),
            format!(r#"<img srcset="{uri} 1x, fallback.png 2x" alt="inline">"#),
            format!(
                r#"<picture><source srcset="{uri} 2x"><img src="fallback.png" alt="inline"></picture>"#
            ),
        ] {
            assert_eq!(
                parse(markup.as_bytes()).items,
                vec![Block::Picture {
                    alt: "inline".into(),
                    source: uri.into(),
                }],
                "{markup}"
            );
        }
    }

    /// A `media` query is honoured: only an alternative that asks nothing about
    /// the reader's surroundings applies. A bare media type does — a document is
    /// shown on screens and printed — while a feature query cannot be satisfied
    /// without a viewport, a colour scheme, an orientation or a pointer, so the
    /// next source, or the `img`'s own fallback, is taken instead.
    #[test]
    fn a_media_query_has_to_ask_nothing_of_a_document() {
        let source = |html: &str| match parse(html.as_bytes()).items.into_iter().next() {
            Some(Block::Picture { source, .. }) => source,
            other => panic!("expected a picture, got {other:?}"),
        };

        // Media types apply: `all`, `screen`, `print`, an empty query, and the
        // writer's own bare `all` with spaces around the alternatives.
        for media in ["all", "screen", "print", "", "not print", "all, print"] {
            assert_eq!(
                source(&format!(
                    r#"<picture><source media="{media}" srcset="chosen.png"><img src="fallback.png" alt="x"></picture>"#
                )),
                "chosen.png",
                "media \"{media}\" should have applied"
            );
        }

        // Features do not, however they are spelled: a width, a colour scheme,
        // an orientation, a pointer, and a query that mixes a feature with a
        // type in the same alternative.
        for media in [
            "(min-width: 800px)",
            "(prefers-color-scheme: dark)",
            "(orientation: portrait)",
            "(pointer: coarse)",
            "screen and (max-width: 600px)",
        ] {
            assert_eq!(
                source(&format!(
                    r#"<picture><source media="{media}" srcset="ignored.png"><img src="fallback.png" alt="x"></picture>"#
                )),
                "fallback.png",
                "media \"{media}\" should not have applied"
            );
        }

        // One alternative that asks nothing is enough, and a skipped source
        // hands over to the next one rather than to the `img` at once.
        assert_eq!(
            source(
                r#"<picture><source media="(min-width: 800px), print" srcset="a.png"><img src="fallback.png" alt="x"></picture>"#
            ),
            "a.png"
        );
        assert_eq!(
            source(
                r#"<picture><source media="(min-width: 800px)" srcset="a.png"><source media="print" srcset="b.png"><img src="fallback.png" alt="x"></picture>"#
            ),
            "b.png"
        );
    }

    /// The carried formats are the shared list, not a copy of it: every format
    /// `pdfextractor_picture_formats` holds is a declaration this takes, and the
    /// list itself is the only place a format is added.
    #[test]
    fn every_carried_picture_format_is_a_picture_type() {
        for format in pdfextractor_picture_formats::PICTURE_FORMATS {
            assert!(
                is_picture_type(Some(format.mime)),
                "{} is carried but not a picture type",
                format.mime
            );
        }
        // And a format the list does not hold is still passed over.
        assert!(!is_picture_type(Some("image/avif")));
    }

    /// A `type` is honoured too: a `<source>` declaring a format this cannot
    /// carry — a newer codec, or something that is not an image — is passed over
    /// for the browser's next choice, and one declaring a format it can carry, or
    /// saying nothing, is used. The declaration's parameters are not part of it.
    #[test]
    fn a_source_has_to_declare_a_picture_type() {
        let source = |html: &str| match parse(html.as_bytes()).items.into_iter().next() {
            Some(Block::Picture { source, .. }) => source,
            other => panic!("expected a picture, got {other:?}"),
        };

        for declared in [
            "image/png",
            "image/webp",
            "IMAGE/JPEG",
            // Carried but not decodable: a declared SVG is still a picture the
            // page named, the way a Markdown `![](chart.svg)` is.
            "image/svg+xml",
        ] {
            assert_eq!(
                source(&format!(
                    r#"<picture><source type="{declared}" srcset="chosen.png"><img src="fallback.png" alt="x"></picture>"#
                )),
                "chosen.png",
                "type \"{declared}\" should have been usable"
            );
        }

        // A format this cannot carry — a newer codec, or something that is not
        // an image — leaves the browser's remaining choice, not a picture with
        // nothing behind it.
        for declared in [
            "image/avif",
            "image/heic",
            "text/html",
            "video/mp4",
            "application/octet-stream",
            "image/",
        ] {
            assert_eq!(
                source(&format!(
                    r#"<picture><source type="{declared}" srcset="ignored.png"><img src="fallback.png" alt="x"></picture>"#
                )),
                "fallback.png",
                "type \"{declared}\" should not have been usable"
            );
        }

        // A parameter after the essence does not change which type it is, and a
        // usable source further down the list takes over from an unusable one.
        assert_eq!(
            source(
                r#"<picture><source type="image/png; charset=binary" srcset="png.png"><img src="fallback.png" alt="x"></picture>"#
            ),
            "png.png"
        );
        assert_eq!(
            source(
                r#"<picture><source type="text/html" srcset="page.html"><source type="image/png" srcset="b.png"><img src="fallback.png" alt="x"></picture>"#
            ),
            "b.png"
        );
    }

    /// Both attributes are conditions on the same candidate: a source that fails
    /// either is skipped, and when every source is skipped the `img` is what a
    /// browser would have shown anyway — its `srcset` first, then its `src`.
    #[test]
    fn skipped_sources_leave_the_img_as_the_picture() {
        let doc = parse(
            br#"<picture><source media="(min-width: 800px)" srcset="wide.png"><source type="text/html" srcset="page.html"><img srcset="small.png 1x, large.png 2x" src="fallback.png" alt="hero"></picture>"#,
        );
        assert_eq!(
            doc.items,
            vec![Block::Picture {
                alt: "hero".into(),
                source: "small.png".into(),
            }]
        );
    }

    /// A `<picture>` whose `img` has no `src` — the form a browser uses when
    /// the `<source>` is the only real image — still yields its picture, and a
    /// source with no usable candidate leaves the `img`'s fallback in place.
    #[test]
    fn a_picture_without_an_img_source_uses_its_source_element() {
        let doc = parse(
            br#"<picture><source srcset="hero.webp"><img alt="hero"></picture><picture><source srcset="  ,  "><img src="fallback.png" alt="fallback"></picture>"#,
        );
        assert_eq!(
            doc.items,
            vec![
                Block::Picture {
                    alt: "hero".into(),
                    source: "hero.webp".into(),
                },
                Block::Picture {
                    alt: "fallback".into(),
                    source: "fallback.png".into(),
                },
            ]
        );
    }

    /// A `srcset` URL is entity-decoded like any other attribute, and a
    /// `<source>` outside a `<picture>` belongs to a media element rather than
    /// to the document's pictures.
    #[test]
    fn a_srcset_source_is_decoded_and_source_is_scoped_to_its_picture() {
        let doc = parse(
            br#"<source srcset="not-a-picture.png"><img srcset="a&amp;b.png 1x" alt="A &amp; B">"#,
        );
        assert_eq!(
            doc.items,
            vec![Block::Picture {
                alt: "A & B".into(),
                source: "a&b.png".into(),
            }]
        );
    }

    /// Markup cannot hold a picture where the model has only words: a cell and
    /// a remark keep the alt text, and an `img` with no source is the alt text
    /// a reader would see. Attributes are entity-decoded, so a path written
    /// `a&amp;b.png` names the file `a&b.png`.
    #[test]
    fn an_image_where_only_words_fit_keeps_its_alt_text() {
        let doc = parse(
            b"<table><tr><td><img src=\"i.png\" alt=\"icon\"></td></tr></table><aside class=\"notes\"><p><img src=\"j.png\" alt=\"chart\"></p></aside><p><img alt=\"missing\"> after</p>",
        );
        assert_eq!(
            doc.items,
            vec![
                Block::Table {
                    grid: TableGrid::from_rows(&[vec!["icon".into()]]),
                },
                Block::Note {
                    text: "chart".into(),
                    hyperlink: None,
                    comment: false,
                },
                Block::paragraph("missing after"),
            ]
        );

        let doc = parse(br#"<img src="a&amp;b.png" alt="A &amp; B">"#);
        assert_eq!(
            doc.items,
            vec![Block::Picture {
                alt: "A & B".into(),
                source: "a&b.png".into(),
            }]
        );
    }

    #[test]
    fn attributes_containing_gt_do_not_end_the_tag() {
        let doc = parse(br#"<a title="a > b">link</a><p>after</p>"#);
        assert_eq!(texts(&doc), vec!["link", "after"]);
    }

    #[test]
    fn parse_tag_reads_href() {
        let inner: Vec<char> = "a href=\"https://example.com\"".chars().collect();
        let tag = parse_tag(&inner);
        assert_eq!(tag.name, "a");
        assert!(!tag.closing);
        assert_eq!(tag.href.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn an_anchor_splits_its_sentence_and_carries_the_href() {
        // The words around a link are sent ahead of it, so the URL lands on
        // the anchor's own text alone — the same split the DOCX reader makes
        // of a linked sentence.
        let doc = parse(b"<p>See <a href=\"https://example.com\">the site</a> today.</p>");
        assert_eq!(
            doc.items,
            vec![
                Block::paragraph("See"),
                Block::Text {
                    text: "the site".to_string(),
                    label: "paragraph".to_string(),
                    marker: None,
                    level: None,
                    hyperlink: Some("https://example.com".to_string()),
                    quote: false,
                },
                Block::paragraph("today."),
            ]
        );
    }

    /// A remark's address belongs to the item; the writer puts it around the
    /// paragraph the remark opens with. An anchor around that whole paragraph
    /// is the note's link, while one inside a sentence — or a second anchor —
    /// is not, because the model would then claim more of the remark was
    /// linked than was.
    #[test]
    fn a_remark_that_is_one_link_keeps_its_address() {
        let doc = parse(
            br##"<aside class="notes"><p><strong>Notes:</strong> <a href="https://example.com/ledger">the ledger</a></p></aside><aside class="notes"><p><strong>Notes:</strong> See <a href="https://example.com/ledger">the ledger</a> for it.</p></aside><aside class="notes"><p><strong>Notes:</strong> <a href="https://a.example">one</a> <a href="https://b.example">two</a></p></aside>"##,
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
                    text: "See the ledger for it.".into(),
                    hyperlink: None,
                    comment: false,
                },
                Block::Note {
                    text: "one two".into(),
                    hyperlink: None,
                    comment: false,
                },
            ]
        );
    }

    /// The link covers the remark's first paragraph, and the paragraphs after
    /// it are still the same note.
    #[test]
    fn a_linked_remark_keeps_the_paragraphs_after_its_link() {
        let doc = parse(
            br#"<aside class="notes"><p><strong>Notes:</strong> <a href="https://example.com/ledger">the ledger</a></p><p>Then the regions.</p></aside>"#,
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

    /// The writer marks a deck's remarks as an `<aside class="notes">` whose
    /// paragraph opens with a bold `Notes:` label. Reading one back has to put
    /// it on the notes layer; reading it as prose would make the remark a
    /// paragraph of the document. An aside that is not marked stays prose.
    #[test]
    fn a_notes_aside_is_a_note_not_prose() {
        let doc = parse(
            br#"<h1>Q3 Review</h1><aside class="notes">
<p><strong>Notes:</strong> Check the numbers.</p>
</aside><aside class="sidebar"><p>A sidebar.</p></aside><p>Slide text.</p>"#,
        );
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("Q3 Review", "title", None, None),
                Block::Note {
                    text: "Check the numbers.".into(),
                    hyperlink: None,
                    comment: false,
                },
                Block::paragraph("A sidebar."),
                Block::paragraph("Slide text."),
            ]
        );
    }

    /// A page written with `--image-export-mode placeholder` marks a picture
    /// with a bare comment, which a browser does not render at all. Read back,
    /// the mark is the picture — one with no image behind it — so a page that
    /// carried no bytes still has a picture on the next render, and only the
    /// writer's own mark is read that way; any other comment stays invisible.
    #[test]
    fn a_picture_placeholder_reads_as_a_picture_with_no_image() {
        let doc = parse(b"<p>Before.</p>\n<!-- image -->\n<p>After.</p>\n<!-- two pictures -->\n");
        assert_eq!(
            doc.items,
            vec![
                Block::paragraph("Before."),
                Block::PicturePlaceholder,
                Block::paragraph("After."),
            ]
        );
    }

    /// A reviewer's comment keeps its own class and label, so the next render
    /// marks it as a comment rather than demoting it to a speaker note. The
    /// label alone is enough when the class is not there.
    #[test]
    fn a_comment_aside_is_told_apart_from_a_speaker_note() {
        let doc = parse(
            br#"<aside class="notes comment"><p><strong>Comment:</strong> [author: Ada]: Check the numbers.</p></aside><aside class="notes"><p><strong>Notes:</strong> Say the picture.</p></aside><aside class="notes"><p><strong>Comment:</strong> Only the label.</p></aside>"#,
        );
        assert_eq!(
            doc.items,
            vec![
                Block::Note {
                    text: "[author: Ada]: Check the numbers.".into(),
                    hyperlink: None,
                    comment: true,
                },
                Block::Note {
                    text: "Say the picture.".into(),
                    hyperlink: None,
                    comment: false,
                },
                Block::Note {
                    text: "Only the label.".into(),
                    hyperlink: None,
                    comment: true,
                },
            ]
        );
    }

    /// The class is the mark, so the label and the exact element are not
    /// required: an aside produced elsewhere keeps its words (with the label
    /// dropped when it is there), gets one item whose paragraphs stay apart,
    /// and keeps entities decoded and layout collapsed the way a browser
    /// renders it.
    #[test]
    fn a_notes_aside_is_one_item_however_it_is_written() {
        let doc = parse(
            br#"<aside class="aside notes">
<p>Notes: Check the <b>numbers</b>.</p>
<p>Then the regions.</p>
</aside>"#,
        );
        assert_eq!(
            doc.items,
            vec![Block::Note {
                text: "Check the numbers.\nThen the regions.".into(),
                hyperlink: None,
                comment: false,
            }]
        );

        let doc = parse(b"<aside class='NOTES'><p>Raw &amp; words.</p></aside>");
        assert_eq!(
            doc.items,
            vec![Block::Note {
                text: "Raw & words.".into(),
                hyperlink: None,
                comment: false,
            }]
        );
    }

    #[test]
    fn an_anchor_inside_a_list_item_keeps_the_item() {
        // The words are cut in three, but all three remain list items: an
        // anchor cuts the words, not the list. The bullet was printed once,
        // so the first piece carries it and the rest keep the label only.
        let doc = parse(b"<ul><li>grab <a href=\"https://x.io/y\">it</a> here</li></ul>");
        assert_eq!(
            doc.items,
            vec![
                Block::labelled("grab", "list_item", Some(1), Some("•".into())),
                Block::Text {
                    text: "it".to_string(),
                    label: "list_item".to_string(),
                    marker: None,
                    level: None,
                    hyperlink: Some("https://x.io/y".to_string()),
                    quote: false,
                },
                Block::labelled("here", "list_item", None, None),
            ]
        );
    }
}
