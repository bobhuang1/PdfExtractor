//! Flow-format parsers: RTF, HTML, Markdown, plain text and CSV/TSV.
//!
//! These formats carry no page geometry - they are streams of blocks - so the
//! output is the same shape the DOCX parser produces: ordered [`Block`]s with no
//! provenance. Every parser reads the bytes itself; no third-party format
//! library is used.

pub mod csv;
pub mod html;
pub mod rtf;
pub mod text;

/// One block of document content, in document order.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text {
        text: String,
        label: String,
        /// Bullet or numbering the line printed, when the block is a list item.
        marker: Option<String>,
        /// Heading depth or list nesting depth, counted from one.
        level: Option<i32>,
    },
    Table {
        rows: Vec<Vec<String>>,
    },
}

impl Block {
    /// A plain paragraph.
    pub fn paragraph(text: impl Into<String>) -> Self {
        Block::Text {
            text: text.into(),
            label: "paragraph".to_string(),
            marker: None,
            level: None,
        }
    }

    /// A heading at `level`, or a list item with `marker`.
    pub fn labelled(
        text: impl Into<String>,
        label: &str,
        level: Option<i32>,
        marker: Option<String>,
    ) -> Self {
        Block::Text {
            text: text.into(),
            label: label.to_string(),
            marker,
            level,
        }
    }
}

/// A parsed flow-based document.
#[derive(Debug, Clone)]
pub struct TextDocument {
    /// Page size in points for the single nominal page these formats occupy.
    pub page_width: f64,
    pub page_height: f64,
    pub items: Vec<Block>,
}

impl Default for TextDocument {
    fn default() -> Self {
        Self {
            // US Letter, the same fallback the DOCX parser uses when the
            // document states no page size.
            page_width: 612.0,
            page_height: 792.0,
            items: Vec::new(),
        }
    }
}

/// Decode bytes to text, honouring a byte-order mark and guessing UTF-16 when
/// the bytes are full of NULs. Anything that is not UTF-8 falls back to CP1252
/// rather than producing replacement characters for accented letters.
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, true);
    }
    if looks_like_utf16(bytes) {
        return utf16(bytes, false);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

/// UTF-16 with NULs in every other byte is common for Windows-generated files
/// that carry no BOM.
fn looks_like_utf16(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || bytes.len() % 2 != 0 {
        return false;
    }
    let sample = &bytes[..bytes.len().min(64)];
    let zeros = sample.iter().step_by(2).filter(|&&b| b == 0).count();
    zeros * 2 >= sample.len() / 2
}

fn utf16(bytes: &[u8], big_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if big_endian {
                u16::from_be_bytes([c[0], c[1]])
            } else {
                u16::from_le_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// Decode one CP1252 byte; the C1 range differs from Latin-1.
pub fn cp1252(byte: u8) -> char {
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
