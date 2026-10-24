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

/// The mark a prose writer puts where a picture stood, in the one mode that
/// writes neither a link nor the bytes: `--image-export-mode placeholder`.
///
/// It is an HTML comment, so a browser renders it as nothing, and it is the
/// exact string `pdfextractor_md_exporter::IMAGE_PLACEHOLDER` writes — the two
/// are held equal by a test, since this crate cannot depend on that one.
pub const IMAGE_PLACEHOLDER: &str = "<!-- image -->";

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
        /// The `href` of the anchor these words sat in: an `a` with no `href`
        /// is not a link and leaves this `None`.
        hyperlink: Option<String>,
        /// Whether the document set these words apart as a quotation.
        ///
        /// A block quote is a paragraph to Docling — its own readers flatten
        /// one — so the words keep their `paragraph` label and the quoting is
        /// the one extra fact. A writer that can say it (`>` in Markdown,
        /// `<blockquote>` in HTML) says it again, which is what keeps a quoted
        /// passage quoted when a page is written back.
        quote: bool,
    },
    /// A table on its own grid, with the cells a merge covers left out.
    ///
    /// A rectangular reading of a table fills every slot, so [`TableGrid`]
    /// holds one single-slot cell per grid position; markup that declares a
    /// merge — HTML's `rowspan`/`colspan` — places one cell over several slots
    /// instead, and the slots it covers are not cells of the table. Keeping
    /// that distinction is what lets a merged table read from a page be written
    /// back as the merged table it was; the flow writers that cannot express a
    /// merge flatten it where they render, not here.
    Table { grid: TableGrid },
    /// One image a flow document pointed at: a Markdown `![alt](source)` or an
    /// HTML `<img src>`.
    ///
    /// A picture is an item of its own, not words: when an image stands inside
    /// a sentence the sentence is split around it, the way a link splits one.
    /// The source is kept exactly as written — usually a path relative to the
    /// file — because only the caller knows where the file lives. A source that
    /// is a `data:` URI carries its bytes itself, so the caller has nothing to
    /// find: the readers keep it whole and the caller decodes it into the same
    /// picture a file would become.
    Picture {
        /// The image's alt text, exactly as written; empty is allowed.
        alt: String,
        /// The destination inside `(...)`, as written.
        source: String,
    },
    /// A remark, not slide content: a quoted line the Markdown writer labelled
    /// `**Notes:**`.
    ///
    /// The writer puts every notes-layer item on exactly one such line, so a
    /// reader can tell a presentation's speaker notes and comments from the
    /// document's own prose. Reading the quote back as a paragraph would turn
    /// the remark into a slide on the next pass; this keeps it in the notes
    /// layer where it came from.
    Note {
        /// The remark, with the label removed and its words kept as written.
        text: String,
        /// The address the remark pointed at, when the writer's own mark says
        /// the whole first paragraph was a link — a note's hyperlink belongs to
        /// the item, so it is written once, around the paragraph it opens with.
        ///
        /// A remark with a link in the middle of a sentence cannot carry one:
        /// the model holds one URL per item, and claiming the whole remark was
        /// linked would widen the link on the next render.
        hyperlink: Option<String>,
        /// Whether this is a comment rather than a speaker note. The writer
        /// marks a comment `Comment:` and a note `Notes:`, so the two stay
        /// distinguishable in a quote even when its group markers are gone;
        /// the comment also belongs in a `comment_section` group.
        comment: bool,
    },
    /// The start of a group the Markdown writer marked.
    ///
    /// Markdown has no groups, so the writer surrounds a slide's `chapter`
    /// group and a comment's `comment_section` group with HTML comments that
    /// render as nothing:
    /// `<!-- pdfextractor:group chapter body slide-0 -->`. Reading one back is
    /// what keeps a deck's slides — and each comment's own section — grouped
    /// when the document is rendered again, instead of the remarks and the
    /// slides becoming one flat list. The group runs until the matching
    /// [`Block::GroupEnd`]; a document with no markers reads as it always did.
    GroupStart {
        /// The group's label: `chapter` for a slide, `comment_section` for a
        /// comment.
        label: String,
        /// The group's name, as the document names it (`slide-0`,
        /// `comment-slide1-1`).
        name: String,
        /// `body`, `notes`, `invisible` or `furniture` — the layer the group
        /// states, which its items inherit.
        content_layer: String,
    },
    /// A spot a picture stood at, with no picture behind it.
    ///
    /// A prose writer has three ways to treat a picture and the third —
    /// `--image-export-mode placeholder` — writes neither a link nor the
    /// bytes, only [`IMAGE_PLACEHOLDER`] where the picture stood. That mark is
    /// a picture, not the words it is spelled with: reading it as a paragraph
    /// would put `<!-- image -->` into the document's prose and leave the next
    /// render with prose where a picture was.
    ///
    /// The item this becomes has no image behind it, which is exactly what the
    /// page said — the same picture a chart or a vector drawing is, one the
    /// model holds and no file answers for.
    PicturePlaceholder,
    /// The end of the group the last [`Block::GroupStart`] began.
    GroupEnd,
}

impl Block {
    /// A plain paragraph.
    pub fn paragraph(text: impl Into<String>) -> Self {
        Block::Text {
            text: text.into(),
            label: "paragraph".to_string(),
            marker: None,
            level: None,
            hyperlink: None,
            quote: false,
        }
    }

    /// A quotation: a paragraph whose words are not the document's own.
    pub fn quoted(text: impl Into<String>) -> Self {
        Block::Text {
            text: text.into(),
            label: "paragraph".to_string(),
            marker: None,
            level: None,
            hyperlink: None,
            quote: true,
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
            hyperlink: None,
            quote: false,
        }
    }
}

/// A table laid out on an explicit grid, with its cells placed and spanned.
///
/// The same shape the document model holds: a cell says which grid slot it
/// starts in and how far it reaches, and the slots it covers hold no cell. A
/// reading that knows nothing about merges therefore produces one cell per
/// slot, which is what [`TableGrid::from_rows`] builds.
#[derive(Debug, Clone, PartialEq)]
pub struct TableGrid {
    /// The grid's height and width, which a cell's spans are measured against.
    pub num_rows: usize,
    pub num_cols: usize,
    /// The cells that *are* the table, in the order the source wrote them.
    pub cells: Vec<TableCell>,
    /// The rows the document itself declared a header, by grid row, in the
    /// order it declared them.
    ///
    /// Empty when the source said nothing about headers, which is not the same
    /// statement as "no row is a header": a format that cannot mark one still
    /// has the reading it always had, its first row. [`TableGrid::header_row`]
    /// is where those two readings are told apart.
    ///
    /// The marks that fill it are the ones a format states in its own words:
    /// RTF's `\trhdr`, and an HTML row holding `<th>`s instead of `<td>`s.
    pub header_rows: Vec<usize>,
}

/// One cell of a [`TableGrid`]: its words, the grid slot it starts in, and how
/// many rows and columns it covers — more than one of either is a merge.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub text: String,
    /// Grid row and column of the cell's top-left slot, counted from zero.
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
}

impl TableGrid {
    /// A rectangular table, one single-slot cell per position: every reading
    /// that knows nothing about merges (a CSV file, a pipe table) makes this.
    pub fn from_rows(rows: &[Vec<String>]) -> Self {
        let num_rows = rows.len();
        let num_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut cells = Vec::new();
        for (row, texts) in rows.iter().enumerate() {
            for (col, text) in texts.iter().enumerate() {
                cells.push(TableCell {
                    text: text.clone(),
                    row,
                    col,
                    row_span: 1,
                    col_span: 1,
                });
            }
        }
        TableGrid {
            num_rows,
            num_cols,
            cells,
            // Nothing in a rectangular reading marks a row a header.
            header_rows: Vec::new(),
        }
    }

    /// Whether `row` is the table's header row.
    ///
    /// The rows the document declared outrank the positional default: a table
    /// that marks none keeps its first row, the only reading it ever had. A
    /// marked row is a header wherever it sits, so a table that labels a group
    /// above its column names says so instead of being read as if row 0 were
    /// the names.
    pub fn header_row(&self, row: usize) -> bool {
        if self.header_rows.is_empty() {
            row == 0
        } else {
            self.header_rows.contains(&row)
        }
    }

    /// The words at one slot, when a cell starts there. A slot a merge covers
    /// answers `None`, which is what keeps it out of a rectangular rendering.
    #[cfg(test)]
    pub fn text_at(&self, row: usize, col: usize) -> Option<&str> {
        self.cells
            .iter()
            .find(|cell| cell.row == row && cell.col == col)
            .map(|cell| cell.text.as_str())
    }
}

/// A parsed flow-based document.
#[derive(Debug, Clone)]
pub struct TextDocument {
    /// Page size in points for the single nominal page these formats occupy.
    pub page_width: f64,
    pub page_height: f64,
    pub items: Vec<Block>,
    /// What an HTML page's head said about the page itself: its
    /// `<meta name="description">` and the URL a `<link rel="canonical">`
    /// named. Neither is rendered, so neither is a block; a format with no head
    /// of its own leaves both unset.
    pub description: Option<String>,
    pub canonical: Option<String>,
    /// The Open Graph and Twitter card `<meta>` statements the head made, as the
    /// key the page wrote and its content, in the order it wrote them: the
    /// `og:title`, `og:image` and `twitter:card` a link preview is built from.
    ///
    /// A list of `(key, content)` rather than two lists for the two
    /// vocabularies: the key names which vocabulary it belongs to, and the order
    /// matters — a card names an image and then that image's width in the meta
    /// after it. A format with no head of its own has no cards.
    pub social: Vec<(String, String)>,
}

impl Default for TextDocument {
    fn default() -> Self {
        Self {
            // US Letter, the same fallback the DOCX parser uses when the
            // document states no page size.
            page_width: 612.0,
            page_height: 792.0,
            items: Vec::new(),
            description: None,
            canonical: None,
            social: Vec::new(),
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
