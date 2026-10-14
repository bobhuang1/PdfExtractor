//! Plain text (`.txt`) and Markdown (`.md`) parsing.
//!
//! Plain text has no structure at all, so every non-empty line becomes a
//! paragraph; nothing is merged or re-wrapped. Markdown does carry structure,
//! so headings, lists, fenced code and pipe tables are recognised and the rest
//! stays prose. Inline emphasis is left as written rather than stripped, so the
//! text is exactly what the file contains.

use crate::{decode, Block, TextDocument};

/// Split into lines, treating CRLF, CR and LF alike.
fn lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// Parse a plain-text file: one paragraph per non-empty line.
pub fn parse_plain(bytes: &[u8]) -> TextDocument {
    let mut doc = TextDocument::default();
    for line in lines(&decode(bytes)) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            doc.items.push(Block::paragraph(trimmed));
        }
    }
    doc
}

/// Parse a Markdown file, keeping the block structure it expresses.
pub fn parse_markdown(bytes: &[u8]) -> TextDocument {
    let raw = lines(&decode(bytes));
    let mut doc = TextDocument::default();
    let mut i = 0usize;

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

        // Setext heading: the text line, then a line of `=` or `-`.
        if !line.trim().is_empty() {
            if let Some(next) = raw.get(i + 1) {
                if let Some(level) = setext_level(next) {
                    doc.items.push(Block::labelled(
                        line.trim(),
                        "section_header",
                        Some(level),
                        None,
                    ));
                    i += 2;
                    continue;
                }
            }
        }

        // ATX heading.
        if let Some((level, text)) = atx_heading(line) {
            doc.items
                .push(Block::labelled(text, "section_header", Some(level), None));
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
            doc.items.push(Block::Table { rows });
            continue;
        }

        // List item.
        if let Some((marker, level, text)) = list_item(line) {
            doc.items.push(Block::labelled(
                text,
                "list_item",
                Some(level),
                Some(marker),
            ));
            i += 1;
            continue;
        }

        // Block quote.
        if let Some(rest) = line.trim_start().strip_prefix('>') {
            let text = rest.trim();
            if !text.is_empty() {
                doc.items.push(Block::paragraph(text));
            }
            i += 1;
            continue;
        }

        let trimmed = line.trim();
        if !trimmed.is_empty() {
            doc.items.push(Block::paragraph(trimmed));
        }
        i += 1;
    }

    doc
}

/// The fence marker of a code-block line, e.g. `"```"`.
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

/// `### Title` -> `(3, "Title")`.
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

/// A line of `=` or `-` under text is a heading: level 1 or 2.
fn setext_level(line: &str) -> Option<i32> {
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

    #[test]
    fn markdown_headings_carry_their_depth() {
        let doc = parse_markdown(b"# Title\n\n## Section\n\nbody\n");
        assert_eq!(
            doc.items[0],
            Block::labelled("Title", "section_header", Some(1), None)
        );
        assert_eq!(
            doc.items[1],
            Block::labelled("Section", "section_header", Some(2), None)
        );
        assert_eq!(doc.items[2], Block::paragraph("body"));
    }

    #[test]
    fn markdown_setext_headings_are_recognised() {
        let doc = parse_markdown(b"Title\n=====\n\nbody\n");
        assert_eq!(
            doc.items[0],
            Block::labelled("Title", "section_header", Some(1), None)
        );
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
                rows: vec![
                    vec!["a".into(), "b".into()],
                    vec!["1".into(), "2".into()],
                    vec!["3".into(), "4".into()],
                ]
            }]
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
}
