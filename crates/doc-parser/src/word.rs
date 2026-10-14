//! Legacy Word `.doc` (Word 6/95/97-2003 binary format) text extraction.
//!
//! Unlike `.docx`, a `.doc` is not a package of XML parts: it is an OLE compound
//! file (see [`crate::cfb`]) holding a `WordDocument` stream with a binary
//! `Fib`, plus a `1Table`/`0Table` stream carrying the piece table. The text is
//! read through the *piece table* (`Clx`), which says where each run of
//! characters lives and how it is encoded - 8-bit bytes in the document code
//! page, or UTF-16 - so a document written in either form reads correctly.
//!
//! The character stream is not plain prose. It carries control characters that
//! stand for structure: a paragraph mark (`0x0D`), a table cell mark (`0x07`), a
//! field (`0x13`..`0x15`), an inline picture (`0x01`) and a page break
//! (`0x0C`). Those are resolved into blocks rather than left as raw bytes.
//!
//! What is deliberately *not* done here: character formatting (`PAPX`/`CHPX`)
//! is not read, so paragraph styles are unknown and tables are flattened to
//! tab-separated text rather than rebuilt as grids. See the crate docs.

use anyhow::{bail, Context, Result};

use crate::cfb::CompoundFile;

/// One block of document content, in document order.
#[derive(Debug, Clone, PartialEq)]
pub enum DocItem {
    Text {
        text: String,
        label: String,
    },
    /// An inline picture. Its Art bytes live in OfficeArt records whose
    /// placement requires character formatting, so the placeholder is reported
    /// without pixels rather than guessed.
    Picture,
}

/// A parsed legacy Word document.
#[derive(Debug, Clone)]
pub struct DocDocument {
    /// Page size in points. `.doc` keeps page geometry in its section table,
    /// which this parser does not read, so the Word default is reported.
    pub page_width: f64,
    pub page_height: f64,
    /// Body content, in document order.
    pub items: Vec<DocItem>,
    /// Header and footer content.
    ///
    /// Headers and footers are a separate subdocument in the same character
    /// stream, so nothing in the body points at them and they used to be
    /// dropped entirely (bug 47).
    pub furniture: Vec<DocItem>,
}

impl Default for DocDocument {
    fn default() -> Self {
        Self {
            page_width: 612.0,
            page_height: 792.0,
            items: Vec::new(),
            furniture: Vec::new(),
        }
    }
}

impl DocDocument {
    pub fn parse_file(path: &str) -> anyhow::Result<Self> {
        let data = std::fs::read(path)?;
        Self::parse_bytes(&data)
    }

    pub fn parse_bytes(data: &[u8]) -> Result<Self> {
        let compound =
            CompoundFile::open(data).context("not a legacy Word (.doc) OLE compound file")?;
        let word = compound
            .stream("WordDocument")
            .context("compound file has no WordDocument stream")?;

        let fib = Fib::parse(&word)?;
        let table = compound
            .stream(fib.table_stream())
            .with_context(|| format!("compound file has no {} stream", fib.table_stream()))?;

        let pieces = piece_table(&table, &fib)?;
        let chars = text_in_range(&word, &pieces, 0, fib.ccp_text);

        // The subdocuments that follow the body share its character stream:
        // footnotes (`ccpFtn`), then headers and footers (`ccpHdd`), then the
        // story that holds comments. Their lengths are in the Fib.
        let after_footnotes = fib.ccp_text.saturating_add(fib.ccp_ftn);
        let header_start = after_footnotes;
        let header_end = header_start.saturating_add(fib.ccp_hdd);
        let furniture = structure(&text_in_range(&word, &pieces, header_start, header_end));

        let mut items = structure(&chars);
        if fib.comments > 0 {
            let stories = text_in_range(
                &word,
                &pieces,
                header_end,
                header_end
                    .saturating_add(fib.ccp_mcr)
                    .saturating_add(fib.ccp_atn)
                    .saturating_add(fib.ccp_edn),
            );
            for comment in comment_stories(&stories, fib.comments as usize) {
                items.push(DocItem::Text {
                    text: comment,
                    label: "footnote".to_string(),
                });
            }
        }

        Ok(DocDocument {
            items,
            furniture,
            ..Default::default()
        })
    }
}

// --- control characters in the Word character stream ----------------------

const PARAGRAPH: char = '\u{000D}';
const CELL_MARK: char = '\u{0007}';
const LINE_BREAK: char = '\u{000B}';
const PAGE_BREAK: char = '\u{000C}';
const COLUMN_BREAK: char = '\u{000E}';
const FIELD_BEGIN: char = '\u{0013}';
const FIELD_SEPARATOR: char = '\u{0014}';
const FIELD_END: char = '\u{0015}';
const PICTURE: char = '\u{0001}';
const DRAWN_OBJECT: char = '\u{0008}';
const FOOTNOTE_REF: char = '\u{0002}';
const OPTIONAL_HYPHEN: char = '\u{001F}';
const NON_BREAKING_HYPHEN: char = '\u{001E}';
const NON_BREAKING_SPACE: char = '\u{00A0}';
const TAB: char = '\u{0009}';

/// Resolve the control characters into ordered blocks.
///
/// This is a pure function of the character stream so it can be tested against
/// synthetic sequences for structures the corpus does not contain.
fn structure(chars: &[char]) -> Vec<DocItem> {
    let mut items = Vec::new();
    let mut text = String::new();
    let mut picture = false;
    // One flag per open field: whether its *instruction* is being read. Text in
    // an instruction ("PAGE", "MERGEFORMAT", ...) is not part of the document.
    let mut fields: Vec<bool> = Vec::new();

    let mut commit = |text: &mut String, picture: &mut bool| {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            items.push(DocItem::Text {
                text: trimmed.to_string(),
                label: "paragraph".to_string(),
            });
        }
        text.clear();
        if *picture {
            items.push(DocItem::Picture);
        }
        *picture = false;
    };

    for &c in chars {
        match c {
            FIELD_BEGIN => fields.push(true),
            FIELD_SEPARATOR => {
                if let Some(flag) = fields.last_mut() {
                    *flag = false;
                }
            }
            FIELD_END => {
                fields.pop();
            }
            _ if fields.iter().any(|&in_instruction| in_instruction) => {
                // Inside a field's instruction: drop it.
            }
            PARAGRAPH | PAGE_BREAK => commit(&mut text, &mut picture),
            // A cell mark also ends a cell/row in a table; flattened to a tab,
            // the table's text survives even though its grid does not.
            CELL_MARK => text.push('\t'),
            LINE_BREAK => text.push('\n'),
            COLUMN_BREAK => text.push(' '),
            PICTURE | DRAWN_OBJECT => picture = true,
            FOOTNOTE_REF => {}
            OPTIONAL_HYPHEN => {}
            TAB => text.push('\t'),
            NON_BREAKING_SPACE => text.push(NON_BREAKING_SPACE),
            NON_BREAKING_HYPHEN => text.push('\u{2011}'),
            c if (c as u32) < 0x20 => {}
            c => text.push(c),
        }
    }
    commit(&mut text, &mut picture);
    items
}

// --- Fib and the piece table ----------------------------------------------

/// Word's identifier for a `.doc` stream; anything else is not a Word file.
const WORD_SIGNATURE: u16 = 0xA5EC;

struct Fib {
    /// Whole main-document text length, in character positions.
    ccp_text: u32,
    /// Length of the footnotes, headers/footers, macro, comment and endnote
    /// subdocuments that follow the body in the same character stream.
    ccp_ftn: u32,
    ccp_hdd: u32,
    ccp_mcr: u32,
    ccp_atn: u32,
    ccp_edn: u32,
    /// How many comments the document has, from `PlcfandRef`: one 30-byte
    /// annotation record each, after the leading character position.
    comments: u32,
    /// `fWhichTblStm`: which of the two table streams is in use.
    which_table: bool,
    fc_clx: u32,
    lcb_clx: u32,
}

impl Fib {
    fn parse(word: &[u8]) -> Result<Self> {
        if word.len() < 34 || le16(word, 0) != Some(WORD_SIGNATURE) {
            bail!("WordDocument stream is not a Word binary document");
        }
        let flags = le16(word, 10).unwrap_or(0);
        let which_table = (flags >> 9) & 1 == 1;

        // FIB: FibBase (32 bytes) then length-prefixed arrays. Their sizes are
        // version dependent, so each one is stepped over rather than assumed.
        let mut offset = 32usize;
        let csw = le16(word, offset).unwrap_or(0) as usize;
        offset = offset
            .checked_add(2 + csw * 2)
            .context("truncated FibRgW")?;
        let cslw = le16(word, offset).unwrap_or(0) as usize;
        offset += 2;
        let rglw = offset;
        offset = offset.checked_add(cslw * 4).context("truncated FibRgLw")?;
        let cb_rg_fc_lcb = le16(word, offset).unwrap_or(0) as usize;
        offset += 2;

        if word.len() < offset + cb_rg_fc_lcb * 8 {
            bail!("WordDocument stream is truncated mid-Fib");
        }

        // FibRgLw97: the character counts of the body and of each subdocument
        // that follows it in the same stream.
        let ccp = |index: usize| -> u32 {
            if cslw > index {
                le32(word, rglw + 4 * index).unwrap_or(0)
            } else {
                0
            }
        };
        let ccp_text = ccp(3);

        // `fcPlcfandRef` is pair 4 of FibRgFcLcb97, `fcClx` pair 33.
        const FC_PLCFANDREF_INDEX: usize = 4;
        const FC_CLX_INDEX: usize = 33;
        if cb_rg_fc_lcb <= FC_CLX_INDEX {
            bail!("document has no piece table (Clx)");
        }
        let at = offset + FC_CLX_INDEX * 8;
        let annotations = offset + FC_PLCFANDREF_INDEX * 8;
        // `PlcfandRef` is a character position per comment followed by a 30-byte
        // annotation record, so the count follows from its length.
        let lcb_annotations = le32(word, annotations + 4).unwrap_or(0);
        let comments = lcb_annotations.saturating_sub(4) / 34;

        Ok(Self {
            ccp_text,
            ccp_ftn: ccp(4),
            ccp_hdd: ccp(5),
            ccp_mcr: ccp(7),
            ccp_atn: ccp(8),
            ccp_edn: ccp(9),
            comments,
            which_table,
            fc_clx: le32(word, at).unwrap_or(0),
            lcb_clx: le32(word, at + 4).unwrap_or(0),
        })
    }

    fn table_stream(&self) -> &'static str {
        if self.which_table {
            "1Table"
        } else {
            "0Table"
        }
    }
}

/// One run of characters: where it starts in `WordDocument`, and whether it is
/// stored as 8-bit bytes or UTF-16.
struct Piece {
    /// Character position the run starts at, counted over the whole character
    /// stream — the body and every subdocument that follows it.
    cp: u32,
    /// Byte offset of the run in the `WordDocument` stream.
    fc: usize,
    /// Number of character positions the run covers.
    ccp: u32,
    compressed: bool,
}

/// The character positions of `from`..`to` in the stream.
///
/// The piece table covers the body *and* the subdocuments after it (footnotes,
/// headers and footers, comments, endnotes) as one run of character positions,
/// so any of them can be read from its position alone.
fn text_in_range(word: &[u8], pieces: &[Piece], from: u32, to: u32) -> Vec<char> {
    let mut chars = Vec::with_capacity(to.saturating_sub(from) as usize);
    for piece in pieces {
        let piece_end = piece.cp + piece.ccp;
        if piece_end <= from {
            continue;
        }
        if piece.cp >= to {
            break;
        }
        let start = from.saturating_sub(piece.cp);
        let end = to.min(piece_end) - piece.cp;
        decode_piece(word, piece, start, end, &mut chars);
    }
    chars
}

/// Parse the piece table (`Clx`) into the runs of the character stream.
fn piece_table(table: &[u8], fib: &Fib) -> Result<Vec<Piece>> {
    let clx = table
        .get(fib.fc_clx as usize..fib.fc_clx as usize + fib.lcb_clx as usize)
        .context("piece table lies outside the table stream")?;

    // A `Clx` is a sequence of `Prc` entries (0x01) followed by one `Pcdt`
    // (0x02). The formatting blocks are skipped; only the piece descriptors
    // matter here.
    let mut i = 0usize;
    while clx.get(i) == Some(&0x01) {
        let cb = le16(clx, i + 1).unwrap_or(0) as usize;
        i = i
            .checked_add(3 + cb)
            .context("truncated Prc in piece table")?;
    }
    if clx.get(i) != Some(&0x02) {
        bail!("piece table has no Pcdt entry");
    }
    let lcb = le32(clx, i + 1).unwrap_or(0) as usize;
    let plc = clx
        .get(i + 5..i + 5 + lcb)
        .context("truncated Pcdt in piece table")?;

    // `PlcPcd`: n+1 character positions (CPs), then n 8-byte descriptors.
    if plc.len() < 4 {
        bail!("piece table is too small");
    }
    let n = (plc.len() - 4) / 12;
    if n == 0 {
        return Ok(Vec::new());
    }
    let cps: Vec<u32> = (0..=n).map(|k| le32(plc, 4 * k).unwrap_or(0)).collect();
    let pcd_base = 4 * (n + 1);

    let mut pieces = Vec::with_capacity(n);
    for k in 0..n {
        let pcd = &plc[pcd_base + 8 * k..pcd_base + 8 * (k + 1)];
        let fc = le32(pcd, 2).unwrap_or(0);
        let compressed = fc & 0x4000_0000 != 0;
        let fc = (fc & 0x3FFF_FFFF) as usize;
        pieces.push(Piece {
            cp: cps[k],
            // In the compressed form the stored byte offset is doubled.
            fc: if compressed { fc / 2 } else { fc },
            ccp: cps[k + 1].saturating_sub(cps[k]),
            compressed,
        });
    }
    Ok(pieces)
}

/// Append character positions `start`..`end` of a piece (relative to the piece)
/// to `out`.
fn decode_piece(word: &[u8], piece: &Piece, start: u32, end: u32, out: &mut Vec<char>) {
    if end <= start {
        return;
    }
    if piece.compressed {
        let slice = word
            .get(piece.fc + start as usize..piece.fc + end as usize)
            .unwrap_or(&[]);
        for &b in slice {
            out.push(cp1252(b));
        }
    } else {
        let Some(slice) = word.get(piece.fc + start as usize * 2..piece.fc + end as usize * 2)
        else {
            return;
        };
        let units: Vec<u16> = slice
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        out.extend(String::from_utf16_lossy(&units).chars());
    }
}

/// The first `count` comment texts in the annotation stories of `chars`.
///
/// A comment is a story of the character stream, and a story opens with a
/// control character (`0x05` for an annotation). Word stores as many stories as
/// the document has notes of each kind, so the count from `PlcfandRef` is what
/// says whether a `0x05` story is a comment at all — without it, macro text
/// could be mistaken for one.
fn comment_stories(chars: &[char], count: usize) -> Vec<String> {
    const STORY_START: char = '\u{0005}';
    let mut stories = Vec::new();
    let mut current: Option<String> = None;
    for &c in chars {
        if c == STORY_START {
            if let Some(story) = current.take() {
                stories.push(story);
            }
            current = Some(String::new());
            continue;
        }
        if let Some(story) = current.as_mut() {
            story.push(c);
        }
    }
    if let Some(story) = current {
        stories.push(story);
    }
    stories
        .into_iter()
        .take(count)
        .filter_map(|story| {
            let text = structure(&story.chars().collect::<Vec<_>>())
                .iter()
                .filter_map(|item| match item {
                    DocItem::Text { text, .. } => Some(text.clone()),
                    DocItem::Picture => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        })
        .collect()
}

/// Decode one byte of a compressed piece.
///
/// Word stores 8-bit text in the document's code page. CP1252 is the common case
/// and the only one that differs from Latin-1 in the 0x80..0x9F range, where the
/// punctuation that matters (curly quotes, dashes) lives.
fn cp1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}',
        '\u{017D}', '\u{008F}', '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
    ];
    match byte {
        0x80..=0x9F => HIGH[(byte - 0x80) as usize],
        _ => byte as char,
    }
}

fn le16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn le32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(items: &[DocItem]) -> String {
        items
            .iter()
            .filter_map(|i| match i {
                DocItem::Text { text, .. } => Some(text.as_str()),
                DocItem::Picture => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn paragraphs_split_on_the_paragraph_mark() {
        let chars: Vec<char> = "one\r\r two \rthree".chars().collect();
        let items = structure(&chars);
        assert_eq!(
            items,
            vec![
                DocItem::Text {
                    text: "one".into(),
                    label: "paragraph".into()
                },
                DocItem::Text {
                    text: "two".into(),
                    label: "paragraph".into()
                },
                DocItem::Text {
                    text: "three".into(),
                    label: "paragraph".into()
                },
            ]
        );
    }

    #[test]
    fn a_field_instruction_is_not_document_text() {
        // `PAGE` is the instruction, `7` the printed result.
        let chars: Vec<char> = "\u{13} PAGE \\* MERGEFORMAT \u{14}7\u{15}\r"
            .chars()
            .collect();
        let items = structure(&chars);
        assert_eq!(text_of(&items), "7");
        assert!(!text_of(&items).contains("MERGEFORMAT"));
    }

    #[test]
    fn a_nested_field_instruction_is_dropped_and_its_result_kept() {
        let chars: Vec<char> = "\u{13}IF\u{14}\u{13}REF x\u{14}42\u{15} is the answer\u{15}\r"
            .chars()
            .collect();
        let items = structure(&chars);
        assert_eq!(text_of(&items), "42 is the answer");
    }

    #[test]
    fn a_field_with_only_a_picture_yields_a_picture_not_text() {
        let chars: Vec<char> = "\u{13} EMBED opendocument.MathDocument.1 \u{14}\u{01}\u{15}\r"
            .chars()
            .collect();
        let items = structure(&chars);
        assert_eq!(items, vec![DocItem::Picture]);
    }

    #[test]
    fn cell_marks_become_tabs_and_control_bytes_are_dropped() {
        let chars: Vec<char> = "a\u{07}b\u{0c}c\u{1f}d\u{00a0}e".chars().collect();
        let items = structure(&chars);
        // The cell mark is a tab, the page break ends the paragraph, the
        // optional hyphen vanishes and the non-breaking space survives.
        assert_eq!(text_of(&items), "a\tb\ncd\u{00a0}e");
    }

    #[test]
    fn a_drawn_object_is_reported_as_a_picture() {
        let chars: Vec<char> = "text\r\u{08}\u{01}\r".chars().collect();
        let items = structure(&chars);
        assert_eq!(
            items[0],
            DocItem::Text {
                text: "text".into(),
                label: "paragraph".into()
            }
        );
        assert_eq!(items[1], DocItem::Picture);
    }

    /// A comment is a story of the character stream; the count from the Fib is
    /// what says whether a `0x05` story is a comment at all.
    #[test]
    fn comment_stories_are_split_on_the_story_marker() {
        let chars: Vec<char> = "\u{5}First comment.\r\r\u{5}Second comment.\r\u{5}Third.\r"
            .chars()
            .collect();
        assert_eq!(
            comment_stories(&chars, 2),
            vec!["First comment.".to_string(), "Second comment.".to_string()]
        );
        // A document with no comments reports none, whatever the stream holds.
        assert!(comment_stories(&chars, 0).is_empty());
    }

    #[test]
    fn cp1252_punctuation_is_not_the_raw_byte() {
        // 0x93/0x94 are the curly double quotes; passing them through as bytes
        // would put C1 control characters in the text.
        assert_eq!(cp1252(0x93), '\u{201C}');
        assert_eq!(cp1252(0x94), '\u{201D}');
        assert_eq!(cp1252(0x41), 'A');
    }
}
