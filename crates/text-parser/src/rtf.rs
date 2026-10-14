//! Rich Text Format (`.rtf`) text extraction.
//!
//! RTF is a text format, but a hostile one: `{` opens a group, `\` introduces a
//! control word or symbol, and whole groups hold data rather than prose (font
//! and colour tables, embedded pictures, headers). This reader walks the groups
//! and drops everything that never reaches the page, honouring `\ucN` so the
//! `?` fallback after a `\uN` character does not double up, and decoding `\'hh`
//! bytes in CP1252.
//!
//! Tables and headings carry their structure in control words (`\trowd`,
//! `\sN`) whose definitions live in the skipped `\stylesheet`; they are not
//! rebuilt here, so a table flattens to tab-separated text and a heading is a
//! paragraph. Inline emphasis is dropped, leaving the words.

use crate::{cp1252, Block, TextDocument};

/// Parse an RTF document into blocks.
pub fn parse(bytes: &[u8]) -> TextDocument {
    let mut state = Reader::default();
    state.run(bytes);
    TextDocument {
        items: state.items,
        ..Default::default()
    }
}

/// One open `{` group.
struct Group {
    /// Contents never reach the page (a font table, a picture, ...).
    skip: bool,
    /// Nothing has been read yet, so the next control word may name the
    /// group's destination.
    at_start: bool,
    /// `\ucN`: how many fallback characters follow a `\uN`.
    uc: usize,
}

impl Default for Group {
    fn default() -> Self {
        Self {
            skip: false,
            at_start: true,
            uc: 1,
        }
    }
}

#[derive(Default)]
struct Reader {
    items: Vec<Block>,
    buf: String,
    groups: Vec<Group>,
    /// Fallback characters still to drop after a `\uN`.
    pending_skip: usize,
}

impl Reader {
    fn run(&mut self, bytes: &[u8]) {
        self.groups.push(Group::default());
        let mut i = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => {
                    let uc = self.groups.last().map(|g| g.uc).unwrap_or(1);
                    self.groups.push(Group {
                        uc,
                        ..Group::default()
                    });
                    i += 1;
                }
                b'}' => {
                    if self.groups.len() > 1 {
                        self.groups.pop();
                    }
                    i += 1;
                }
                b'\\' => {
                    i += 1;
                    i = self.control(bytes, i);
                }
                b'\r' | b'\n' => i += 1,
                byte => {
                    self.emit_char(cp1252(byte));
                    i += 1;
                }
            }
        }
        self.flush();
    }

    /// Handle the control sequence starting at `i` (just past the `\`).
    fn control(&mut self, bytes: &[u8], mut i: usize) -> usize {
        let Some(&first) = bytes.get(i) else {
            return i;
        };
        if first.is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            let word = &bytes[start..i];
            let mut negative = false;
            if bytes.get(i) == Some(&b'-') {
                negative = true;
                i += 1;
            }
            let dstart = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let param = if i > dstart {
                std::str::from_utf8(&bytes[dstart..i])
                    .ok()
                    .and_then(|s| s.parse::<i32>().ok())
                    .map(|v| if negative { -v } else { v })
            } else {
                None
            };
            // A single space delimits the control word and is not text.
            if bytes.get(i) == Some(&b' ') {
                i += 1;
            }
            self.word(word, param);
            return i;
        }

        // Control symbol.
        match first {
            b'\'' => {
                let hex = bytes.get(i + 1..i + 3).and_then(|h| {
                    std::str::from_utf8(h)
                        .ok()
                        .and_then(|s| u8::from_str_radix(s, 16).ok())
                });
                match hex {
                    Some(byte) => self.emit_char(cp1252(byte)),
                    None => self.emit_char('\u{FFFD}'),
                }
                i + 3
            }
            b'*' => {
                if let Some(group) = self.groups.last_mut() {
                    group.skip = true;
                }
                i + 1
            }
            b'\\' | b'{' | b'}' => {
                self.emit_char(first as char);
                i + 1
            }
            // A backslash before a newline is a line continuation.
            b'\r' | b'\n' => i + 1,
            other => {
                self.emit_char(other as char);
                i + 1
            }
        }
    }

    fn word(&mut self, word: &[u8], param: Option<i32>) {
        // Decide, before anything else, whether this control word opens a
        // destination whose text must be dropped.
        let opens_destination = {
            let group = self.groups.last_mut().expect("root group");
            let at_start = group.at_start;
            group.at_start = false;
            at_start && is_destination(word)
        };
        if opens_destination {
            if let Some(group) = self.groups.last_mut() {
                group.skip = true;
            }
            return;
        }

        match word {
            b"par" | b"sect" | b"row" | b"nestrow" => self.flush(),
            b"line" => self.emit_char('\n'),
            b"tab" | b"cell" | b"nestcell" => self.emit_char('\t'),
            b"u" => {
                if let Some(value) = param {
                    let code = (value as u32) & 0xFFFF;
                    if let Some(c) = char::from_u32(code) {
                        self.emit_char(c);
                    }
                }
                self.pending_skip = self.groups.last().map(|g| g.uc).unwrap_or(1);
            }
            b"uc" => {
                if let Some(group) = self.groups.last_mut() {
                    group.uc = param.unwrap_or(1).max(0) as usize;
                }
            }
            _ => {}
        }
    }

    fn skipping(&self) -> bool {
        self.groups.iter().any(|g| g.skip)
    }

    fn emit_char(&mut self, c: char) {
        if self.skipping() {
            return;
        }
        if self.pending_skip > 0 {
            self.pending_skip -= 1;
            return;
        }
        self.buf.push(c);
    }

    fn flush(&mut self) {
        if self.skipping() {
            return;
        }
        let text = self.buf.trim().to_string();
        self.buf.clear();
        if !text.is_empty() {
            self.items.push(Block::paragraph(text));
        }
    }
}

/// A control word that introduces a group of non-document data.
fn is_destination(word: &[u8]) -> bool {
    matches!(
        word,
        b"fonttbl"
            | b"colortbl"
            | b"stylesheet"
            | b"listtable"
            | b"listoverridetable"
            | b"rsidtbl"
            | b"generator"
            | b"info"
            | b"pict"
            | b"object"
            | b"objdata"
            | b"objclass"
            | b"footnote"
            | b"header"
            | b"headerl"
            | b"headerr"
            | b"headerf"
            | b"footer"
            | b"footerl"
            | b"footerr"
            | b"footerf"
            | b"annotation"
            | b"atnid"
            | b"atnauthor"
            | b"fldinst"
            | b"xe"
            | b"tc"
            | b"tcn"
            | b"themedata"
            | b"colorschememapping"
            | b"datastore"
            | b"latentstyles"
            | b"xmlnstbl"
            | b"filetbl"
            | b"revtbl"
            | b"pnseclvl"
            | b"listtext"
            | b"bkmkstart"
            | b"bkmkend"
            | b"shp"
            | b"shpinst"
            | b"shprslt"
            | b"nonshppict"
            | b"mmath"
            | b"mm"
            | b"do"
            | b"upr"
            | b"pgptbl"
            | b"protusertbl"
            | b"olddoc"
    )
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
    fn plain_words_become_paragraphs_at_par() {
        let doc = parse(b"{\\rtf1\\ansi Hello\\par World\\par}");
        assert_eq!(texts(&doc), vec!["Hello", "World"]);
    }

    #[test]
    fn font_and_colour_tables_are_dropped() {
        let rtf = br"{\rtf1\ansi{\fonttbl{\f0 Times;}}{\colortbl;\red0\green0\blue0;}Body\par}";
        assert_eq!(texts(&parse(rtf)), vec!["Body"]);
    }

    #[test]
    fn a_picture_group_is_not_text() {
        let rtf = br"{\rtf1\ansi Before {\pict\pngblip 0123456789abcdef} after\par}";
        assert_eq!(texts(&parse(rtf)), vec!["Before  after"]);
    }

    #[test]
    fn unicode_escapes_replace_their_fallback() {
        // `\'e9` is the fallback for `\u233` (e-acute); only one should survive.
        let doc = parse(b"{\\rtf1\\ansi caf\\u233\\'e9\\par}");
        assert_eq!(texts(&doc), vec!["caf\u{00E9}"]);
    }

    #[test]
    fn uc0_keeps_no_fallback() {
        let doc = parse(b"{\\rtf1\\ansi\\uc0 caf\\u233\\par}");
        assert_eq!(texts(&doc), vec!["caf\u{00E9}"]);
    }

    #[test]
    fn hex_escapes_use_cp1252() {
        // 0x93 is a left curly quote in CP1252, not a C1 control character.
        let doc = parse(b"{\\rtf1\\ansi \\'93quoted\\'94\\par}");
        assert_eq!(texts(&doc), vec!["\u{201C}quoted\u{201D}"]);
    }

    #[test]
    fn an_ignorable_destination_is_dropped() {
        let doc = parse(b"{\\rtf1\\ansi{\\*\\fldinst PAGE}7\\par}");
        assert_eq!(texts(&doc), vec!["7"]);
    }

    #[test]
    fn escaped_braces_are_literal() {
        let doc = parse(b"{\\rtf1\\ansi a \\{ b \\} c\\par}");
        assert_eq!(texts(&doc), vec!["a { b } c"]);
    }
}
