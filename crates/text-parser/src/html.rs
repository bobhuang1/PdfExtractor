//! HTML text extraction.
//!
//! The markup is scanned tag by tag, not embedded in a browser: block elements
//! (`p`, `h1`..`h6`, `li`, block containers) become blocks, `<table>` becomes a
//! grid, and anything that is not rendered text (`head`, `script`, `style`,
//! comments) is dropped rather than leaked into the output. Character entities
//! are decoded, so `&amp;` reads as `&`.

use crate::{decode, Block, TextDocument};

/// Parse an HTML document into blocks.
pub fn parse(bytes: &[u8]) -> TextDocument {
    let chars: Vec<char> = decode(bytes).chars().collect();
    let mut state = State::default();
    state.run(&chars);
    TextDocument {
        items: state.items,
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
    table: Option<Vec<Vec<String>>>,
    row: Option<Vec<String>>,
    cell: Option<String>,
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
        }
    }
}

impl State {
    fn run(&mut self, chars: &[char]) {
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i] == '<' {
                // Comment.
                if chars[i..].starts_with(&['<', '!', '-', '-']) {
                    i = skip_until(chars, i, "-->");
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
                    i = find_ascii_case_insensitive(chars, i, &end);
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
                self.items.push(Block::Table { rows: table });
            }
        }
    }

    fn tag(&mut self, tag: &Tag) {
        match tag.name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if tag.closing {
                    self.flush();
                } else {
                    self.flush();
                    self.label = "section_header".to_string();
                    self.level = tag.name[1..].parse().ok();
                }
            }
            "p" | "div" | "section" | "article" | "blockquote" | "pre" | "body" | "main" => {
                self.flush();
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
                    self.marker = Some(if ordered {
                        format!("{}.", self.next_counter())
                    } else {
                        "•".to_string()
                    });
                }
            }
            "br" => {
                self.place("\n");
            }
            "hr" => self.flush(),
            "table" => {
                self.flush();
                if tag.closing {
                    self.close_row();
                    if let Some(table) = self.table.take() {
                        if !table.is_empty() {
                            self.items.push(Block::Table { rows: table });
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
        self.place(text);
    }

    /// Append text to the current block or table cell.
    fn place(&mut self, text: &str) {
        if let Some(cell) = self.cell.as_mut() {
            cell.push_str(text);
        } else {
            self.buf.push_str(text);
        }
    }

    fn close_cell(&mut self) {
        if let Some(cell) = self.cell.take() {
            if let Some(row) = self.row.as_mut() {
                row.push(collapse(&cell));
            }
        }
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
        let text = self.buf.trim().to_string();
        self.buf.clear();
        if !text.is_empty() {
            self.items.push(Block::labelled(
                text,
                &self.label,
                self.level,
                self.marker.clone(),
            ));
        }
        self.label = "paragraph".to_string();
        self.level = None;
        self.marker = None;
    }
}

/// Collapse runs of whitespace, as a browser renders them.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Tag {
    name: String,
    closing: bool,
}

/// Parse the inside of a `<...>`: name and whether it closes.
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
    Tag { name, closing }
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

    #[test]
    fn headings_and_paragraphs_are_separated() {
        let doc = parse(b"<h1>Title</h1><p>Hello <b>world</b>.</p>");
        assert_eq!(
            doc.items[0],
            Block::labelled("Title", "section_header", Some(1), None)
        );
        assert_eq!(doc.items[1], Block::paragraph("Hello world."));
    }

    #[test]
    fn script_and_head_content_is_not_text() {
        let doc = parse(
            b"<head><title>x</title></head><body><script>var a=1;</script><p>real</p></body>",
        );
        assert_eq!(texts(&doc), vec!["real"]);
    }

    #[test]
    fn entities_are_decoded() {
        let doc = parse(b"<p>a &amp; b &#8212; c &lt;d&gt;</p>");
        assert_eq!(texts(&doc), vec!["a & b \u{2014} c <d>"]);
    }

    #[test]
    fn tables_become_grids() {
        let doc =
            parse(b"<table><tr><td>a</td><td>b</td></tr><tr><td>1</td><td>2</td></tr></table>");
        assert_eq!(
            doc.items,
            vec![Block::Table {
                rows: vec![vec!["a".into(), "b".into()], vec!["1".into(), "2".into()]]
            }]
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

    #[test]
    fn attributes_containing_gt_do_not_end_the_tag() {
        let doc = parse(br#"<a title="a > b">link</a><p>after</p>"#);
        assert_eq!(texts(&doc), vec!["link", "after"]);
    }
}
