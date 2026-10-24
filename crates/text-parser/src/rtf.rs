//! Rich Text Format (`.rtf`) text extraction.
//!
//! RTF is a text format, but a hostile one: `{` opens a group, `\` introduces a
//! control word or symbol, and whole groups hold data rather than prose (font
//! and colour tables, headers, embedded pictures). This reader walks the groups
//! and drops everything that never reaches the page, honouring `\ucN` so the
//! `?` fallback after a `\uN` character does not double up, and decoding `\'hh`
//! bytes in CP1252.
//!
//! A `\pict` group is not prose either, but it is the page: its blip control
//! word says what the picture is (`\pngblip`, `\jpegblip`, `\emfblip`,
//! `\wmetafile`, `\wbitmap`, `\dibitmap`) and the hex payload after it is the
//! picture's own bytes. Those become a [`Block::Picture`] carrying a `data:` URI
//! — the same spelling an embedded Markdown or HTML page uses — so a conversion
//! writes the image beside the document instead of losing it. A metafile is
//! played into a PNG at the size the document asks for, exactly as the DOCX
//! reader treats a drawing. A `\wbitmap` or `\dibitmap` payload is a device-
//! independent bitmap — the DIB header and pixels, with no `BM` file header in
//! front of them, because the file header was the container's job and RTF is the
//! container — so it is wrapped in one before it is carried, or old Word
//! pictures would spell `image/bmp` over bytes no BMP reader can open. Where the
//! pixels begin is read from that header by the metafile crate's DIB parser, the
//! one that draws the same bytes out of WMF and EMF records, so the wrapper and
//! the renderer cannot disagree about a palette or a colour mask.
//! Word writes its pictures twice, once under `\shppict` and once as a
//! `\nonshppict` fallback for readers that do not know the first; the fallback
//! copy is not read, so one picture stays one picture.
//!
//! Tables and headings carry their structure in control words, and both are
//! rebuilt: `\trowd` states a row's cell boundaries (`\cellxN`), `\cell` ends a
//! cell and `\row` ends the row, so a table becomes a [`Block::Table`] with its
//! rows, columns and merges instead of a line of tabs. `\trhdr` says which row
//! is the table's header, so a table that labels a group above its column names
//! is not read as if the group were the names. A heading is a paragraph
//! under a style the `\stylesheet` defines as one — the stylesheet is read first
//! for the styles whose `\outlinelevel` or name says heading, and a paragraph's
//! `\sN` (or its own `\outlinelevel`) then labels it `section_header` at that
//! depth, or `title` for Word's Title style. Inline emphasis is dropped, leaving
//! the words.

use crate::{cp1252, Block, TableCell, TableGrid, TextDocument};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use pdfextractor_metafile as metafile;
use std::collections::HashMap;

/// Parse an RTF document into blocks.
pub fn parse(bytes: &[u8]) -> TextDocument {
    let mut state = Reader {
        styles: heading_styles(bytes),
        ..Reader::default()
    };
    state.run(bytes);
    TextDocument {
        items: state.items,
        ..Default::default()
    }
}

/// One open `{` group.
struct Group {
    /// Contents never reach the page (a font table, a header, ...).
    skip: bool,
    /// Nothing has been read yet, so the next control word may name the
    /// group's destination.
    at_start: bool,
    /// `\ucN`: how many fallback characters follow a `\uN`.
    uc: usize,
    /// The picture this group is, when its destination is `\pict`: what the
    /// blip control word says it is and the hex payload that follows.
    picture: Option<Picture>,
    /// Whether this group sits inside Word's `\nonshppict` fallback, whose
    /// picture an enclosing `\shppict` has already carried.
    legacy_picture: bool,
}

impl Default for Group {
    fn default() -> Self {
        Self {
            skip: false,
            at_start: true,
            uc: 1,
            picture: None,
            legacy_picture: false,
        }
    }
}

/// The picture a `\pict` group holds, gathered as the group is walked.
#[derive(Default)]
struct Picture {
    /// The media type the blip control word names; none means the payload
    /// cannot be labelled honestly and is not a picture.
    mimetype: Option<&'static str>,
    /// Whether the payload is a device-independent bitmap (`\wbitmap`,
    /// `\dibitmap`): a DIB header and its pixels with no `BM` file header in
    /// front, which has to be wrapped around it before anything can read it as
    /// a BMP.
    dib: bool,
    /// The payload's hex digits, in the order the file writes them.
    hex: String,
    /// `\picwgoal` and `\pichgoal`: the size the document asks the picture to
    /// be drawn at, in twips — the size a metafile is rasterised at.
    width_twips: Option<i32>,
    height_twips: Option<i32>,
}

impl Picture {
    /// The `data:` URI that carries this picture into the document model.
    ///
    /// A metafile is a drawing, not a pixel grid Docling can name: it is played
    /// into a PNG here, exactly as a DOCX drawing is, so the picture reaches a
    /// reader instead of becoming a box with no image. A payload that is not a
    /// metafile is carried byte for byte under the type its blip named.
    fn into_source(self) -> Option<String> {
        let mut mimetype = self.mimetype?;
        let mut bytes = decode_hex(&self.hex)?;
        if bytes.is_empty() {
            return None;
        }
        if self.dib {
            // A writer that put a whole BMP file in the payload already said
            // what its bytes are; one that wrote the DIB RTF asks for gets the
            // file header wrapped around it. A DIB this cannot measure has no
            // offset to state, and a guessed header would be a broken picture,
            // so it is not carried at all.
            if !bytes.starts_with(b"BM") {
                bytes = dib_to_bmp(&bytes)?;
            }
        }
        if metafile::is_metafile(&bytes) {
            let mut options = metafile::Options::default();
            if let (Some(width), Some(height)) = (self.width_twips, self.height_twips) {
                if width > 0 && height > 0 {
                    options = options.at_size(width as f64 / 20.0, height as f64 / 20.0);
                }
            }
            if let Ok(raster) = metafile::rasterize(&bytes, options) {
                if let Ok(png) = raster.to_png() {
                    bytes = png;
                    mimetype = "image/png";
                }
            }
        }
        Some(format!("data:{mimetype};base64,{}", STANDARD.encode(bytes)))
    }
}

/// The bytes a run of RTF hex digits stands for.
///
/// An odd count is a truncated payload, not a picture: its last half-byte would
/// be invented, so the picture is left out rather than corrupted.
fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = hex
        .bytes()
        .filter(|byte| byte.is_ascii_hexdigit())
        .collect();
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.as_chunks::<2>().0 {
        bytes.push((hex_value(pair[0])? << 4) | hex_value(pair[1])?);
    }
    Some(bytes)
}

/// Wrap a device-independent bitmap in the `BITMAPFILEHEADER` a `.bmp` file
/// starts with.
///
/// RTF stores a `\wbitmap`/`\dibitmap` payload as the DIB itself: the header
/// that describes the pixels (`BITMAPINFOHEADER`, or the older
/// `BITMAPCOREHEADER`) and then the palette and the bits. Every BMP reader
/// starts at byte 14 instead, after a file header that names the pixel data's
/// offset — so the payload is a picture that no reader opens until one is put in
/// front of it.
///
/// The layout is read by the metafile crate, which draws DIBs out of WMF and EMF
/// records and therefore owns the one reading of a DIB header: sharing it is
/// what keeps this offset and that renderer from disagreeing about where a
/// palette or a colour mask outside the header ends. `None` when the header is
/// not one whose layout is known, or runs past the payload: a wrapped file with
/// an invented offset would be worse than no picture.
fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let layout = metafile::dib::header(dib).ok()?;
    // The pixel data starts after the header, its masks and its palette; a
    // header that reaches past the payload describes pixels this does not have.
    if layout.bits_offset > dib.len() {
        return None;
    }
    let offset = u32::try_from(14usize.checked_add(layout.bits_offset)?).ok()?;
    let size = u32::try_from(14usize.checked_add(dib.len())?).ok()?;
    let mut out = Vec::with_capacity(14 + dib.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// A heading a paragraph can be, as Docling labels one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Heading {
    /// Word's `Title` style: Docling's `title`, the page's own name.
    Title,
    /// An outline level, counted from one as Docling counts section headers.
    Section(i32),
}

/// The merge a cell's own control words declare.
#[derive(Clone, Copy, Default)]
struct Merge {
    /// `\clmgf`: this cell starts a horizontal merge.
    horizontal_first: bool,
    /// `\clmrg`: this cell is covered by the one to its left.
    horizontal_continue: bool,
    /// `\clvmgf`: this cell starts a vertical merge.
    vertical_first: bool,
    /// `\clvmrg`: this cell is covered by the one above it.
    vertical_continue: bool,
}

/// A cell's right edge, as `\cellxN` states it, and the merge that edge's
/// cell declares. The position is what makes a row's columns line up with its
/// neighbours': LibreOffice writes a cell over two columns as one cell ending
/// at the far boundary, with no `\clmgf` at all.
#[derive(Clone, Copy, Default)]
struct Edge {
    right: i32,
    merge: Merge,
}

/// A cell as the table is read: its words, its right edge and the merge it
/// declares.
#[derive(Default)]
struct CellDraft {
    text: String,
    right: i32,
    merge: Merge,
}

/// The table a run of `\trowd` groups is building.
#[derive(Default)]
struct Table {
    /// The rows already finished.
    rows: Vec<Vec<CellDraft>>,
    /// Whether each finished row was marked `\trhdr`, in the same order as
    /// [`Table::rows`] — the flag belongs to the row, not to its cells.
    headers: Vec<bool>,
    /// `\trhdr` on the row being read: the document states that this row is a
    /// header, which outranks the first-row fallback when the grid is built.
    header: bool,
    /// The cells of the row being read.
    row: Vec<CellDraft>,
    /// The right edge each cell of this row has, in `\cellx` order, with the
    /// merge it declares.
    edges: Vec<Edge>,
    /// Flags since the last `\cellx`, which belong to the next boundary.
    pending: Merge,
    /// The words of the cell being read, its paragraphs joined by newlines.
    text: String,
}

impl Table {
    /// Forget the previous row's cells, keeping the finished rows.
    fn begin_row(&mut self) {
        self.row.clear();
        self.edges.clear();
        self.pending = Merge::default();
        self.text.clear();
        // A row's own words: `\trhdr` states the header of the row it is in.
        self.header = false;
    }

    fn finish_cell(&mut self) {
        let text = std::mem::take(&mut self.text).trim().to_string();
        let edge = self.edges.get(self.row.len()).copied().unwrap_or_default();
        self.row.push(CellDraft {
            text,
            right: edge.right,
            merge: edge.merge,
        });
    }

    fn finish_row(&mut self) {
        // A writer may leave the last cell's words without a `\cell` before
        // the `\row`; the row still has that cell, so it is closed here.
        if !self.text.trim().is_empty() || self.row.len() < self.edges.len() {
            self.finish_cell();
        }
        if !self.row.is_empty() {
            let row = std::mem::take(&mut self.row);
            self.rows.push(row);
            let header = self.header;
            self.headers.push(header);
        }
        self.begin_row();
    }

    /// The grid the rows make: horizontal merges become one cell with a column
    /// span, vertical merges one cell with a row span, and a slot no cell
    /// covers becomes an empty cell so every row is as wide as the table.
    ///
    /// The columns are the distinct `\cellx` boundaries across the whole table,
    /// not a count of cells in one row: a cell whose right edge is three
    /// boundaries along covers three columns, which is how LibreOffice writes a
    /// merged cell — no `\clmgf`, just a wide cell — and how a row of four
    /// cells stays four columns when the row above merged two of them.
    fn grid(self) -> Option<TableGrid> {
        if self.rows.is_empty() {
            return None;
        }
        let mut boundaries: Vec<i32> = vec![0];
        boundaries.extend(self.rows.iter().flatten().map(|cell| cell.right));
        boundaries.sort_unstable();
        boundaries.dedup();
        if boundaries.len() < 2 {
            return None;
        }
        let boundary = |right: i32| boundaries.iter().position(|edge| *edge == right);
        let num_cols = boundaries.len() - 1;
        let num_rows = self.rows.len();

        struct Placed {
            text: String,
            row: usize,
            col: usize,
            row_span: usize,
            col_span: usize,
        }
        let mut placed: Vec<Placed> = Vec::new();
        // The cell a vertical merge started with, by column.
        let mut vertical: HashMap<usize, usize> = HashMap::new();

        for (row, cells) in self.rows.iter().enumerate() {
            let mut col = 0usize;
            let mut index = 0usize;
            while index < cells.len() {
                let cell = &cells[index];
                // A continuation cell is covered by the cell to its left: its
                // boundary is the merged cell's far edge.
                if cell.merge.horizontal_continue {
                    index += 1;
                    continue;
                }
                let mut right = cell.right;
                let mut next = index + 1;
                while next < cells.len() && cells[next].merge.horizontal_continue {
                    right = cells[next].right;
                    next += 1;
                }
                let end = boundary(right).map_or(col + 1, |end| end.max(col + 1));
                let col_span = end.saturating_sub(col).max(1);
                if cell.merge.vertical_continue {
                    if let Some(&above) = vertical.get(&col) {
                        placed[above].row_span += 1;
                    } else {
                        // A continuation with nothing above it is its own cell
                        // rather than a slot with no owner.
                        placed.push(Placed {
                            text: cell.text.clone(),
                            row,
                            col,
                            row_span: 1,
                            col_span,
                        });
                    }
                } else {
                    let at = placed.len();
                    placed.push(Placed {
                        text: cell.text.clone(),
                        row,
                        col,
                        row_span: 1,
                        col_span,
                    });
                    if cell.merge.vertical_first {
                        vertical.insert(col, at);
                    } else {
                        vertical.remove(&col);
                    }
                }
                col = end;
                index = next;
            }
        }

        let mut covered = vec![vec![false; num_cols]; num_rows];
        let mut cells: Vec<TableCell> = Vec::new();
        for cell in placed {
            let row_span = cell.row_span.min(num_rows - cell.row);
            let col_span = cell.col_span.min(num_cols.saturating_sub(cell.col));
            if col_span == 0 {
                continue;
            }
            for row in covered.iter_mut().skip(cell.row).take(row_span) {
                for taken in row.iter_mut().skip(cell.col).take(col_span) {
                    *taken = true;
                }
            }
            cells.push(TableCell {
                text: cell.text,
                row: cell.row,
                col: cell.col,
                row_span,
                col_span,
            });
        }
        for (row, slots) in covered.iter().enumerate() {
            for (col, taken) in slots.iter().enumerate() {
                if !taken {
                    cells.push(TableCell {
                        text: String::new(),
                        row,
                        col,
                        row_span: 1,
                        col_span: 1,
                    });
                }
            }
        }
        cells.sort_by_key(|cell| (cell.row, cell.col));
        // The rows the document marked `\trhdr`, in the order it marked them.
        // None marked leaves the list empty, which says so rather than
        // claiming the table has no header row: the unmarked table keeps the
        // reading it always had.
        let header_rows = self
            .headers
            .iter()
            .enumerate()
            .filter(|(_, header)| **header)
            .map(|(row, _)| row)
            .collect();
        Some(TableGrid {
            num_rows,
            num_cols,
            cells,
            header_rows,
        })
    }
}

#[derive(Default)]
struct Reader {
    items: Vec<Block>,
    buf: String,
    groups: Vec<Group>,
    /// Fallback characters still to drop after a `\uN`.
    pending_skip: usize,
    /// The styles the document's `\stylesheet` defines as headings, by style
    /// number — read before the walk so a `\sN` can label its paragraph.
    styles: HashMap<i32, Heading>,
    /// The paragraph's style (`\sN`), cleared by `\pard`.
    style: Option<i32>,
    /// The paragraph's own `\outlinelevel`, which outranks its style.
    outline: Option<i32>,
    /// `\intbl`: the paragraph belongs to a table cell.
    intbl: bool,
    /// The table being read, from its first `\trowd` to the paragraph after
    /// its last `\row`.
    table: Option<Table>,
}

impl Reader {
    fn run(&mut self, bytes: &[u8]) {
        self.groups.push(Group::default());
        let mut i = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => {
                    let uc = self.groups.last().map(|g| g.uc).unwrap_or(1);
                    let legacy_picture =
                        self.groups.last().is_some_and(|group| group.legacy_picture);
                    self.groups.push(Group {
                        uc,
                        legacy_picture,
                        ..Group::default()
                    });
                    i += 1;
                }
                b'}' => {
                    if self.groups.len() > 1 {
                        let closed = self.groups.pop().expect("a group to close");
                        // The words before a picture are their own block: a
                        // picture is an item of the page, not a character in
                        // a sentence. The group is popped first so the flush
                        // is not held back by the picture's own `skip`.
                        if let Some(picture) = closed.picture {
                            // The words before the picture were read outside
                            // it, so they are flushed even when the picture
                            // sat in a skipped destination of its own (Word's
                            // `\shppict`); the buffer never holds words read
                            // inside a skipped group.
                            self.flush_buffered();
                            if let Some(source) = picture.into_source() {
                                self.items.push(Block::Picture {
                                    alt: String::new(),
                                    source,
                                });
                            }
                        }
                    }
                    i += 1;
                }
                b'\\' => {
                    i += 1;
                    i = self.control(bytes, i);
                }
                b'\r' | b'\n' => i += 1,
                byte => {
                    // Inside a `\pict` group every byte is payload: the hex
                    // digits are the picture and the whitespace between them
                    // is layout, so neither is text.
                    if self.collecting_picture() {
                        if byte.is_ascii_hexdigit() {
                            if let Some(picture) = self.picture_mut() {
                                picture.hex.push(byte as char);
                            }
                        }
                    } else {
                        self.emit_char(cp1252(byte));
                    }
                    i += 1;
                }
            }
        }
        self.flush();
        // A document that ends inside a table still has one: its last row is
        // closed and the grid emitted like any other.
        self.close_table();
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

        // A `\pict` group is a destination, but its payload is the page: the
        // group gathers the picture instead of dropping its hex as text.
        if opens_destination && word == b"pict" {
            if let Some(group) = self.groups.last_mut() {
                group.skip = true;
                // Word writes the same picture twice: the `\shppict` copy is
                // the one to read, and the `\nonshppict` fallback that follows
                // would make one picture into two.
                group.picture = (!group.legacy_picture).then(Picture::default);
            }
            return;
        }

        if opens_destination {
            let legacy_picture = word == b"nonshppict";
            if let Some(group) = self.groups.last_mut() {
                group.skip = true;
                group.legacy_picture |= legacy_picture;
            }
            return;
        }

        // The blip and size control words are the picture's own header; they
        // say nothing the page reads as words.
        // Paragraph properties and table structure. A skipped destination
        // carries none of them, and the stylesheet's own `\s` must not label
        // the body that follows it.
        if !self.skipping() && self.structure_word(word, param) {
            return;
        }

        if self.collecting_picture() {
            let picture = self.picture_mut().expect("a picture is being read");
            match word {
                b"pngblip" => picture.mimetype = Some("image/png"),
                b"jpegblip" => picture.mimetype = Some("image/jpeg"),
                b"emfblip" => picture.mimetype = Some("image/emf"),
                b"wmetafile" => picture.mimetype = Some("image/wmf"),
                // The two spellings Word uses for a bitmap: `\wbitmap` for a
                // Windows DIB and `\dibitmap` for the same bytes, both without
                // the file header that would make them a `.bmp`.
                b"wbitmap" | b"dibitmap" => {
                    picture.mimetype = Some("image/bmp");
                    picture.dib = true;
                }
                b"picwgoal" => picture.width_twips = param,
                b"pichgoal" => picture.height_twips = param,
                _ => {}
            }
            return;
        }

        match word {
            b"par" | b"sect" => self.flush(),
            b"line" => self.emit_char('\n'),
            b"tab" | b"nestcell" => self.emit_char('\t'),
            b"nestrow" => self.flush(),
            b"u" => {
                if let Some(value) = param {
                    let code = (value as u32) & 0xFFFF;
                    if let Some(c) = char::from_u32(code) {
                        self.emit_char(c);
                    }
                }
                // Only a character that reaches the page has a fallback to
                // drop with it. A `\uN` inside a destination being skipped
                // (a list definition's bullet, say) would otherwise leave a
                // skip behind that ate the next visible character.
                if !self.skipping() {
                    self.pending_skip = self.groups.last().map(|g| g.uc).unwrap_or(1);
                }
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

    /// The paragraph and table control words, which say how the words are laid
    /// out rather than being words: what style a paragraph carries, and where a
    /// table's cells and rows begin and end. `true` when `word` was one.
    fn structure_word(&mut self, word: &[u8], param: Option<i32>) -> bool {
        match word {
            // `\pard` resets the paragraph's own properties, which is how a
            // paragraph after a table leaves it.
            b"pard" => {
                self.style = None;
                self.outline = None;
                self.intbl = false;
            }
            b"intbl" => self.intbl = true,
            b"s" => self.style = param,
            b"outlinelevel" => self.outline = param,
            b"trowd" => self.start_row(),
            b"cellx" => {
                if let Some(table) = self.table.as_mut() {
                    // The parameter is the cell's right edge; one that names no
                    // position still gets a boundary so the cell is not lost.
                    let right = param.unwrap_or_else(|| {
                        table
                            .edges
                            .last()
                            .map_or(1, |edge| edge.right.saturating_add(1))
                    });
                    table.edges.push(Edge {
                        right,
                        merge: std::mem::take(&mut table.pending),
                    });
                }
            }
            b"clmgf" => self.pending_merge(|merge| merge.horizontal_first = true),
            b"clmrg" => self.pending_merge(|merge| merge.horizontal_continue = true),
            b"clvmgf" => self.pending_merge(|merge| merge.vertical_first = true),
            b"clvmrg" => self.pending_merge(|merge| merge.vertical_continue = true),
            // `\trhdr`: the row being read is a header row — the table's own
            // statement of which row heads its columns, instead of the
            // positional guess that row 0 does.
            b"trhdr" => {
                if let Some(table) = self.table.as_mut() {
                    table.header = true;
                }
            }
            b"cell" => self.close_cell(),
            b"row" => self.close_row(),
            _ => return false,
        }
        true
    }

    fn pending_merge(&mut self, set: impl FnOnce(&mut Merge)) {
        if let Some(table) = self.table.as_mut() {
            set(&mut table.pending);
        }
    }

    /// `\trowd`: the row's `\cellx` boundaries follow. A `\trowd` while a row
    /// is still open means the previous row ended without its `\row`, so that
    /// row is closed first rather than losing its cells.
    fn start_row(&mut self) {
        if let Some(table) = self.table.as_mut() {
            if !table.row.is_empty() || !table.text.trim().is_empty() {
                table.finish_row();
            }
            table.begin_row();
            return;
        }
        // The words before a table are a paragraph of their own.
        self.flush_buffered();
        self.table = Some(Table::default());
    }

    /// `\cell`: the cell's words are finished. Outside a table the boundary is
    /// the tab it always was.
    fn close_cell(&mut self) {
        if self.table.is_none() {
            self.emit_char('\t');
            return;
        }
        self.flush_cell();
        if let Some(table) = self.table.as_mut() {
            table.finish_cell();
        }
    }

    /// `\row`: the last cell closes and the row joins the table. Outside a
    /// table the control word is the paragraph boundary it always was.
    fn close_row(&mut self) {
        if self.table.is_none() {
            self.flush();
            return;
        }
        self.flush_cell();
        if let Some(table) = self.table.as_mut() {
            table.finish_row();
        }
    }

    /// The words read so far are one paragraph of the cell being read.
    fn flush_cell(&mut self) {
        let text = self.buf.trim().to_string();
        self.buf.clear();
        if text.is_empty() {
            return;
        }
        if let Some(table) = self.table.as_mut() {
            if !table.text.is_empty() {
                table.text.push('\n');
            }
            table.text.push_str(&text);
        }
    }

    /// Emit the table being read, if one is open.
    fn close_table(&mut self) {
        let Some(mut table) = self.table.take() else {
            return;
        };
        table.finish_row();
        if let Some(grid) = table.grid() {
            self.items.push(Block::Table { grid });
        }
    }

    /// The heading this paragraph's style and outline level make it, if any.
    fn heading(&self) -> Option<Heading> {
        match self.outline {
            // `\outlinelevelN` counts from zero, Docling's headings from one.
            Some(level) => Some(Heading::Section((level + 1).clamp(1, 5))),
            None => self
                .style
                .and_then(|style| self.styles.get(&style).copied()),
        }
    }

    /// Whether the innermost group's bytes are a picture's hex payload.
    ///
    /// Only the picture's own group counts: a nested group inside it (Word's
    /// `\*\blipuid`) holds hex digits of its own, and none of them are pixels.
    fn collecting_picture(&self) -> bool {
        self.groups
            .last()
            .is_some_and(|group| group.picture.is_some())
    }

    /// The picture the innermost group holds, when that group is a `\pict`.
    fn picture_mut(&mut self) -> Option<&mut Picture> {
        self.groups
            .last_mut()
            .and_then(|group| group.picture.as_mut())
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
        // A paragraph inside a table is a paragraph of its cell, not an item
        // of the page; one outside it ends the table first.
        if self.table.is_some() && (self.intbl || self.cell_open()) {
            self.flush_cell();
            return;
        }
        self.close_table();
        self.flush_buffered();
    }

    /// Whether a cell of the row being read has words in it yet.
    fn cell_open(&self) -> bool {
        self.table
            .as_ref()
            .is_some_and(|table| !table.edges.is_empty() || !table.text.is_empty())
    }

    /// Flush the words read so far, whatever the enclosing group is.
    ///
    /// `emit_char` drops characters read inside a skipped group, so the buffer
    /// only ever holds words from the document; a boundary — a picture, say —
    /// is a boundary even when the group that closed around it was skipped.
    ///
    /// The paragraph's style decides its label: the stylesheet's heading styles
    /// make a `section_header` (or a `title`), and everything else is a
    /// paragraph.
    fn flush_buffered(&mut self) {
        let text = self.buf.trim().to_string();
        self.buf.clear();
        if text.is_empty() {
            return;
        }
        match self.heading() {
            Some(Heading::Title) => self.items.push(Block::labelled(text, "title", None, None)),
            Some(Heading::Section(level)) => {
                self.items
                    .push(Block::labelled(text, "section_header", Some(level), None));
            }
            None => self.items.push(Block::paragraph(text)),
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

/// The styles a document's `\stylesheet` defines as headings, keyed by the
/// number a paragraph names with `\sN`.
///
/// The stylesheet is a destination the walk drops — its words are definitions,
/// not the page — so it is read first, on its own. A style is a heading when its
/// `\outlinelevel` says so (Word writes one into each of its heading styles),
/// when its name is `Title`, or when the name says `Heading N` and no outline
/// level was written.
fn heading_styles(bytes: &[u8]) -> HashMap<i32, Heading> {
    let Some(body) = stylesheet_body(bytes) else {
        return HashMap::new();
    };
    let mut styles = HashMap::new();
    for group in style_groups(body) {
        if let Some((style, heading)) = style_heading(group) {
            styles.insert(style, heading);
        }
    }
    styles
}

/// The bytes after the `\stylesheet` control word, or `None` when the document
/// has none.
fn stylesheet_body(bytes: &[u8]) -> Option<&[u8]> {
    let needle = b"\\stylesheet";
    let mut from = 0usize;
    while from + needle.len() <= bytes.len() {
        let at = from
            + bytes[from..]
                .windows(needle.len())
                .position(|w| w == needle)?;
        let after = at + needle.len();
        // `\stylesheetx` would be another control word.
        if !bytes
            .get(after)
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        {
            // A single space delimits the control word and is not text.
            let start = after + usize::from(bytes.get(after) == Some(&b' '));
            return Some(&bytes[start..]);
        }
        from = after;
    }
    None
}

/// The child groups of a stylesheet, each one style's own definition.
///
/// The body starts inside the stylesheet's own group; a `}` at depth zero is
/// that group closing, which ends the scan.
fn style_groups(body: &[u8]) -> Vec<&[u8]> {
    let mut groups = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut at = 0usize;
    while at < body.len() {
        match body[at] {
            b'\\' => at = skip_control(body, at + 1),
            b'{' => {
                depth += 1;
                if depth == 1 {
                    start = at + 1;
                }
                at += 1;
            }
            b'}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    groups.push(&body[start..at]);
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
    groups
}

/// One style's definition: its number and the heading it makes a paragraph.
fn style_heading(group: &[u8]) -> Option<(i32, Heading)> {
    let mut style = None;
    let mut outline = None;
    let mut name = String::new();
    let mut at = 0usize;
    while at < group.len() {
        match group[at] {
            b'\\' => {
                let after = at + 1;
                if !group
                    .get(after)
                    .is_some_and(|byte| byte.is_ascii_alphabetic())
                {
                    at = skip_control(group, after);
                    continue;
                }
                let word_start = after;
                let mut word_end = after;
                while group
                    .get(word_end)
                    .is_some_and(|byte| byte.is_ascii_alphabetic())
                {
                    word_end += 1;
                }
                let word = &group[word_start..word_end];
                let mut next = word_end;
                let mut negative = false;
                if group.get(next) == Some(&b'-') {
                    negative = true;
                    next += 1;
                }
                let digits_start = next;
                while group.get(next).is_some_and(|byte| byte.is_ascii_digit()) {
                    next += 1;
                }
                let param = if next > digits_start {
                    std::str::from_utf8(&group[digits_start..next])
                        .ok()
                        .and_then(|digits| digits.parse::<i32>().ok())
                        .map(|value| if negative { -value } else { value })
                } else {
                    None
                };
                if group.get(next) == Some(&b' ') {
                    next += 1;
                }
                match word {
                    b"s" => style = param,
                    b"outlinelevel" => outline = param,
                    _ => {}
                }
                at = next;
            }
            // A nested group holds formatting, not the style's name.
            b'{' => at = matching_brace(group, at),
            b';' => break,
            byte => {
                name.push(byte as char);
                at += 1;
            }
        }
    }
    let style = style?;
    let name = name.trim().trim_matches('"').trim().to_ascii_lowercase();
    let heading = if matches!(name.as_str(), "title" | "document title") {
        Heading::Title
    } else if let Some(level) = outline {
        Heading::Section((level + 1).clamp(1, 5))
    } else {
        Heading::Section(heading_name_level(&name)?)
    };
    Some((style, heading))
}

/// The level a style named `Heading N` stands for, when the name says.
fn heading_name_level(name: &str) -> Option<i32> {
    let digits = name.strip_prefix("heading")?.trim();
    let level = digits.parse::<i32>().ok()?;
    Some(level.clamp(1, 5))
}

/// Skip the control sequence whose backslash was at `at - 1`; `at` is the byte
/// after the backslash.
fn skip_control(bytes: &[u8], mut at: usize) -> usize {
    match bytes.get(at) {
        Some(byte) if byte.is_ascii_alphabetic() => {
            while bytes.get(at).is_some_and(|byte| byte.is_ascii_alphabetic()) {
                at += 1;
            }
            if bytes.get(at) == Some(&b'-') {
                at += 1;
            }
            while bytes.get(at).is_some_and(|byte| byte.is_ascii_digit()) {
                at += 1;
            }
            if bytes.get(at) == Some(&b' ') {
                at += 1;
            }
            at
        }
        // `\'hh` is one byte written as two hex digits.
        Some(b'\'') => at + 3,
        Some(_) => at + 1,
        None => at,
    }
}

/// The index after the `}` matching the `{` at `open`.
fn matching_brace(bytes: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    let mut at = open;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at = skip_control(bytes, at + 1),
            b'{' => {
                depth += 1;
                at += 1;
            }
            b'}' => {
                depth -= 1;
                at += 1;
                if depth == 0 {
                    return at;
                }
            }
            _ => at += 1,
        }
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian word of the wrapped file, for reading back what the
    /// wrapper stated.
    fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    }

    /// A 2×2 red PNG, the smallest picture whose header the image reader
    /// accepts.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xfd,
        0xd4, 0x9a, 0x73, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x44, 0x0c, 0x10, 0x0a, 0x00, 0x1f, 0xee, 0x03, 0xfd, 0x8b, 0x5f, 0x14,
        0xd4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn texts(doc: &TextDocument) -> Vec<String> {
        doc.items
            .iter()
            .filter_map(|b| match b {
                Block::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn pictures(doc: &TextDocument) -> Vec<&str> {
        doc.items
            .iter()
            .filter_map(|block| match block {
                Block::Picture { source, .. } => Some(source.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Bytes as RTF writes a picture's payload: two hex digits each.
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The payload of a `data:` URI, decoded.
    fn payload(source: &str) -> Vec<u8> {
        let payload = source
            .split_once(";base64,")
            .map(|(_, payload)| payload)
            .unwrap_or_else(|| panic!("not a base64 data URI: {source}"));
        STANDARD.decode(payload).expect("a base64 payload")
    }

    /// RTF carrying `bytes` as its picture, with `PAYLOAD` standing for the
    /// hex: a document is mostly braces, and doubling every one of them in a
    /// `format!` hides the markup the test is about.
    fn rtf_with(template: &str, bytes: &[u8]) -> Vec<u8> {
        template.replace("PAYLOAD", &hex(bytes)).into_bytes()
    }

    fn record(function: u16, params: &[u8]) -> Vec<u8> {
        let bytes = 6 + params.len();
        assert_eq!(bytes % 2, 0, "a record is a whole number of words");
        let mut out = Vec::new();
        out.extend_from_slice(&((bytes / 2) as u32).to_le_bytes());
        out.extend_from_slice(&function.to_le_bytes());
        out.extend_from_slice(params);
        out
    }

    fn window_record(function: u16, y: i16, x: i16) -> Vec<u8> {
        let mut params = Vec::new();
        params.extend_from_slice(&y.to_le_bytes());
        params.extend_from_slice(&x.to_le_bytes());
        record(function, &params)
    }

    /// A placeable WMF that fills a wide bar in its top fifth: the same drawing
    /// the metafile crate's own tests build, written here because those
    /// fixtures are private to its test binary.
    fn bar_metafile() -> Vec<u8> {
        let mut out = Vec::new();
        // The placeable header: key, unused handle, frame, units per inch.
        out.extend_from_slice(&0x9ac6_cdd7u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for value in [0i16, 0, 1000, 1000] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&1000u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        // The 18-byte METAHEADER that follows it.
        out.extend_from_slice(&1u16.to_le_bytes()); // memory metafile
        out.extend_from_slice(&9u16.to_le_bytes()); // nine words
        out.extend_from_slice(&0x0300u16.to_le_bytes()); // version
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        // Window, a filled polygon, end of file.
        out.extend_from_slice(&window_record(0x020b, 0, 0));
        out.extend_from_slice(&window_record(0x020c, 10, 100));
        let mut polygon = Vec::new();
        polygon.extend_from_slice(&4u16.to_le_bytes());
        for (x, y) in [(10i16, 0i16), (90, 0), (90, 2), (10, 2)] {
            polygon.extend_from_slice(&x.to_le_bytes());
            polygon.extend_from_slice(&y.to_le_bytes());
        }
        out.extend_from_slice(&record(0x0324, &polygon));
        out.extend_from_slice(&record(0x0000, &[0, 0]));
        out
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

    /// A `\pict` group is a picture, not words: the payload comes back as the
    /// picture's own bytes in a `data:` URI, and the sentence around it splits
    /// rather than swallowing the hex.
    #[test]
    fn a_picture_group_becomes_a_picture_item() {
        let rtf = rtf_with(
            "{\\rtf1\\ansi Before {\\pict\\pngblip PAYLOAD} after\\par}",
            TINY_PNG,
        );
        let doc = parse(&rtf);
        assert_eq!(texts(&doc), vec!["Before", "after"]);
        let pictures = pictures(&doc);
        assert_eq!(pictures.len(), 1, "one picture in, one out: {pictures:?}");
        assert!(
            pictures[0].starts_with("data:image/png;base64,"),
            "{}",
            pictures[0]
        );
        assert_eq!(
            payload(pictures[0]),
            TINY_PNG,
            "the payload is not the picture's own bytes"
        );
    }

    /// The JPEG spelling names its own type, and the bytes are carried under
    /// it rather than guessed from the header.
    #[test]
    fn a_jpeg_payload_keeps_its_own_media_type() {
        let jpeg = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00";
        let rtf = rtf_with("{\\rtf1\\ansi{\\pict\\jpegblip PAYLOAD}}", jpeg);
        let doc = parse(&rtf);
        let pictures = pictures(&doc);
        assert_eq!(pictures.len(), 1);
        assert!(
            pictures[0].starts_with("data:image/jpeg;base64,"),
            "{}",
            pictures[0]
        );
        assert_eq!(payload(pictures[0]), jpeg);
    }

    /// A 3 × 2 24-bit DIB: a `BITMAPINFOHEADER`, then two rows of three
    /// pixels padded to four bytes — the bytes `\wbitmap` carries, with no
    /// `BM` file header in front of them.
    fn dib_24() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&40u32.to_le_bytes()); // header size
        out.extend_from_slice(&3i32.to_le_bytes()); // width
        out.extend_from_slice(&2i32.to_le_bytes()); // height
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        out.extend_from_slice(&12u32.to_le_bytes()); // pixel data size
        out.extend_from_slice(&2835i32.to_le_bytes()); // x pixels per metre
        out.extend_from_slice(&2835i32.to_le_bytes()); // y pixels per metre
        out.extend_from_slice(&0u32.to_le_bytes()); // palette entries used
        out.extend_from_slice(&0u32.to_le_bytes()); // important entries
        for _ in 0..2 {
            out.extend_from_slice(&[0x20, 0x60, 0xa0, 0x20, 0x60, 0xa0, 0x20, 0x60, 0xa0]);
            out.extend_from_slice(&[0, 0, 0]);
        }
        out
    }

    /// An 8-bit DIB carrying a palette of `colors` four-byte entries, and
    /// declaring `declared` of them in `biClrUsed` (zero means "as many as the
    /// bit depth implies"). The shape whose pixel offset is not just the
    /// header's size.
    fn dib_8(declared: u32, colors: usize) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&2i32.to_le_bytes()); // width
        out.extend_from_slice(&1i32.to_le_bytes()); // height
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&8u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        out.extend_from_slice(&4u32.to_le_bytes()); // pixel data size
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&declared.to_le_bytes()); // palette entries used
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&vec![0u8; colors * 4]);
        out.extend_from_slice(&[1, 2, 0, 0]); // two indices, padded
        out
    }

    /// Old Word pictures are `\wbitmap` (or `\dibitmap`) groups whose payload
    /// is a DIB, and a DIB is not a file: every BMP reader starts at the pixel
    /// offset a file header gives. The wrapper has to state that offset and
    /// leave the DIB's own bytes alone, or the picture it names cannot be
    /// opened.
    #[test]
    fn a_wbitmap_dib_is_wrapped_in_a_bmp_file_header() {
        let dib = dib_24();
        let rtf = rtf_with("{\\rtf1\\ansi{\\pict\\wbitmap0 PAYLOAD}}", &dib);
        let doc = parse(&rtf);
        let pictures = pictures(&doc);
        assert_eq!(pictures.len(), 1, "the bitmap was dropped");
        assert!(
            pictures[0].starts_with("data:image/bmp;base64,"),
            "{}",
            pictures[0]
        );
        let bmp = payload(pictures[0]);
        assert_eq!(&bmp[..2], b"BM", "the payload is not a BMP file");
        assert_eq!(
            u32_at(&bmp, 2),
            Some(14 + dib.len() as u32),
            "the file does not say how long it is"
        );
        // 24-bit pixels are the header and nothing else: no palette to step
        // over, so the data starts right after the 40-byte DIB header.
        assert_eq!(u32_at(&bmp, 10), Some(14 + 40), "wrong pixel offset");
        assert_eq!(&bmp[14..], &dib[..], "the DIB's bytes were rewritten");
        // The header inside is the DIB's own, read where a BMP reader looks.
        assert_eq!(u32_at(&bmp, 14), Some(40));
        assert_eq!(u32_at(&bmp, 18), Some(3), "wrong width");
        assert_eq!(u32_at(&bmp, 22), Some(2), "wrong height");
    }

    /// A palette sits between the header and the pixels, and the offset has to
    /// count it: with `biClrUsed` zero an 8-bit DIB carries a 256-colour table,
    /// and the declared count is what a writer that saved a smaller one means.
    /// Both spellings of the blip are the same picture.
    #[test]
    fn a_dibitmap_palette_is_counted_in_the_pixel_offset() {
        for (declared, colors) in [(0u32, 256usize), (2, 2)] {
            let dib = dib_8(declared, colors);
            let blip = if declared == 0 { "wbitmap" } else { "dibitmap" };
            let rtf = rtf_with(
                &format!("{{\\rtf1\\ansi{{\\pict\\{blip}0 PAYLOAD}}}}"),
                &dib,
            );
            let doc = parse(&rtf);
            let pictures = pictures(&doc);
            assert_eq!(pictures.len(), 1, "{blip}: the bitmap was dropped");
            assert!(
                pictures[0].starts_with("data:image/bmp;base64,"),
                "{blip}: {}",
                pictures[0]
            );
            let bmp = payload(pictures[0]);
            assert_eq!(
                u32_at(&bmp, 10),
                Some(14 + 40 + colors as u32 * 4),
                "{blip}: the pixels are not past the palette"
            );
            assert_eq!(&bmp[14..], &dib[..], "{blip}: the DIB's bytes moved");
            assert_eq!(
                u32_at(&bmp, 14 + 40 + colors * 4),
                Some(u32::from_le_bytes([1, 2, 0, 0])),
                "{blip}: the first pixel is not where the offset says"
            );
        }
    }

    /// A payload that is already a BMP file is carried as it is: the wrapper is
    /// for the DIB RTF stores, not a second header on a file that has one.
    #[test]
    fn a_bitmap_payload_that_is_already_a_file_is_not_wrapped() {
        let dib = dib_24();
        let mut file = b"BM".to_vec();
        file.extend_from_slice(&(14 + dib.len() as u32).to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&(14u32 + 40).to_le_bytes());
        file.extend_from_slice(&dib);

        let rtf = rtf_with("{\\rtf1\\ansi{\\pict\\wbitmap0 PAYLOAD}}", &file);
        let doc = parse(&rtf);
        let pictures = pictures(&doc);
        assert_eq!(pictures.len(), 1);
        assert_eq!(payload(pictures[0]), file, "the file was wrapped twice");
    }

    /// A DIB whose header cannot be measured is not a picture: a wrapper with
    /// an invented offset would name a BMP no reader could open, which is worse
    /// than the picture being absent.
    #[test]
    fn a_bitmap_whose_dib_header_is_unreadable_is_not_a_picture() {
        for payload in [
            // A header that claims more bytes than the payload holds.
            [40u32.to_le_bytes().as_slice(), &[0u8; 20]].concat(),
            // A header size no DIB layout is known for.
            [9999u32.to_le_bytes().as_slice(), &[0u8; 64]].concat(),
            // Shorter than even the smallest header.
            vec![1u8, 2, 3, 4],
        ] {
            let rtf = rtf_with("{\\rtf1\\ansi{\\pict\\wbitmap0 PAYLOAD}}", &payload);
            let doc = parse(&rtf);
            assert!(
                pictures(&doc).is_empty(),
                "a broken DIB became a picture: {:?}",
                pictures(&doc)
            );
        }
    }

    /// `BI_ALPHABITFIELDS` keeps four colour masks between its 40-byte header
    /// and its pixels, and the offset has to count all sixteen bytes. The
    /// layout is the metafile crate's reading — the one that draws the same
    /// header out of a WMF record — so the wrapper and the renderer cannot
    /// disagree about how many masks a header leaves outside itself.
    #[test]
    fn an_alpha_bitfields_dib_counts_its_four_masks() {
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes()); // header size
        dib.extend_from_slice(&2i32.to_le_bytes()); // width
        dib.extend_from_slice(&1i32.to_le_bytes()); // height
        dib.extend_from_slice(&1u16.to_le_bytes()); // planes
        dib.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        dib.extend_from_slice(&6u32.to_le_bytes()); // BI_ALPHABITFIELDS
        dib.extend_from_slice(&[0u8; 20]); // sizes, resolution, counts
        dib.extend_from_slice(&[0u8; 16]); // four masks: red, green, blue, alpha
        dib.extend_from_slice(&[1, 2, 3, 255, 0, 0, 0, 0]); // two BGRA pixels

        let rtf = rtf_with("{\\rtf1\\ansi{\\pict\\wbitmap0 PAYLOAD}}", &dib);
        let doc = parse(&rtf);
        let pictures = pictures(&doc);
        assert_eq!(pictures.len(), 1, "the bitmap was dropped");
        let bmp = payload(pictures[0]);
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(
            u32_at(&bmp, 10),
            Some(14 + 40 + 16),
            "the pixel offset does not count the four masks"
        );
        assert_eq!(&bmp[14..], &dib[..], "the DIB's bytes moved");
    }

    /// Word writes a picture twice — `\shppict` for readers that know it, a
    /// `\nonshppict` fallback for those that do not — and reading both would
    /// put every Word image on the page twice.
    #[test]
    fn the_nonshppict_fallback_is_not_a_second_picture() {
        let rtf = rtf_with(
            "{\\rtf1\\ansi Before {\\*\\shppict{\\pict\\pngblip PAYLOAD}}{\\nonshppict{\\pict\\pngblip PAYLOAD}} after\\par}",
            TINY_PNG,
        );
        let doc = parse(&rtf);
        assert_eq!(texts(&doc), vec!["Before", "after"]);
        assert_eq!(pictures(&doc).len(), 1, "the fallback became a picture");
    }

    /// A metafile is a drawing Docling cannot name, so it is played into a PNG
    /// at the size the document asks for — the same render a DOCX drawing gets
    /// — instead of being dropped or carried as a picture nothing can show.
    #[test]
    fn a_metafile_picture_is_played_into_a_png() {
        let rtf = rtf_with(
            "{\\rtf1\\ansi{\\pict\\wmetafile8\\picwgoal200\\pichgoal200 PAYLOAD}\\par}",
            &bar_metafile(),
        );
        let doc = parse(&rtf);
        let pictures = pictures(&doc);
        assert_eq!(pictures.len(), 1, "the drawing was dropped");
        assert!(
            pictures[0].starts_with("data:image/png;base64,"),
            "{}",
            pictures[0]
        );
        let png = payload(pictures[0]);
        assert_eq!(
            &png[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "the raster is not a PNG"
        );
        // 200 twips is 10pt, and the raster is played at 144 dpi.
        assert_eq!(
            u32::from_be_bytes(png[16..20].try_into().expect("a PNG width")),
            20
        );
        assert_eq!(
            u32::from_be_bytes(png[20..24].try_into().expect("a PNG height")),
            20
        );
    }

    /// A payload whose blip named no type this can carry is not a picture: an
    /// unlabelled `data:` URI would claim an image nobody can read.
    #[test]
    fn a_payload_with_no_blip_is_not_a_picture() {
        let doc = parse(b"{\\rtf1\\ansi{\\pict 0123456789abcdef}}");
        assert!(pictures(&doc).is_empty(), "{:?}", pictures(&doc));
        assert!(texts(&doc).is_empty(), "{:?}", texts(&doc));
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

    /// A `\uN` inside a destination the walk drops leaves no fallback behind:
    /// the skip belongs to a character that never reached the page, and must
    /// not eat the next one that did. A list definition's bullet is written
    /// this way in real documents.
    #[test]
    fn a_unicode_escape_in_a_dropped_destination_eats_nothing() {
        let rtf =
            br"{\rtf1\ansi{\*\listtable{\listlevel\leveltext \u61623 ?;}}\pard Quarterly\par}";
        assert_eq!(texts(&parse(rtf)), vec!["Quarterly"]);
    }

    /// One table of a document, which every table test asserts on.
    fn table_of(doc: &TextDocument) -> &TableGrid {
        doc.items
            .iter()
            .find_map(|block| match block {
                Block::Table { grid } => Some(grid),
                _ => None,
            })
            .expect("a table")
    }

    /// A `\trowd` row is a table, not a line of tabs: `\cellx` states the cell
    /// boundaries, `\cell` ends a cell and `\row` ends the row, so the grid the
    /// document printed comes back as a grid — with the words before and after
    /// it as their own paragraphs.
    #[test]
    fn a_trowd_table_becomes_a_grid() {
        let rtf = br"{\rtf1\ansi\pard Before\par
\trowd\cellx2000\cellx4000\cellx6000
\pard\intbl Year\cell Revenue\cell Growth\cell\row
\trowd\cellx2000\cellx4000\cellx6000
\pard\intbl 2024\cell 120\cell 8%\cell\row
\pard After\par}";
        let doc = parse(rtf);
        assert_eq!(texts(&doc), vec!["Before", "After"]);
        let grid = table_of(&doc);
        assert_eq!((grid.num_rows, grid.num_cols), (2, 3));
        let cells: Vec<(usize, usize, &str)> = grid
            .cells
            .iter()
            .map(|cell| (cell.row, cell.col, cell.text.as_str()))
            .collect();
        assert_eq!(
            cells,
            vec![
                (0, 0, "Year"),
                (0, 1, "Revenue"),
                (0, 2, "Growth"),
                (1, 0, "2024"),
                (1, 1, "120"),
                (1, 2, "8%"),
            ]
        );
        // Every cell is a single slot: a table with no merges is rectangular.
        assert!(grid
            .cells
            .iter()
            .all(|cell| cell.row_span == 1 && cell.col_span == 1));
    }

    /// `\trhdr` says which row is the table's header, and it outranks the
    /// first-row fallback: a table that labels a group above its column names
    /// heads the names. Word writes the marker on every row of a header that
    /// repeats across pages, so a table can have more than one.
    #[test]
    fn a_trhdr_row_is_the_tables_header() {
        let rtf = br"{\rtf1\ansi
\trowd\cellx2000\cellx4000
\pard\intbl Group\cell Result\cell\row
\trowd\trhdr\cellx2000\cellx4000
\pard\intbl Year\cell Revenue\cell\row
\trowd\cellx2000\cellx4000
\pard\intbl 2024\cell 120\cell\row}";
        let doc = parse(rtf);
        let grid = table_of(&doc);
        assert_eq!(grid.header_rows, vec![1]);
        assert!(!grid.header_row(0), "the fallback spoke over the marker");
        assert!(grid.header_row(1), "the marked row is not a header");
        assert!(!grid.header_row(2));

        // Two rows written under `\trhdr` are one header over two rows.
        let repeated = br"{\rtf1\ansi
\trowd\trhdr\cellx2000\cellx4000
\pard\intbl Year\cell Revenue\cell\row
\trowd\trhdr\cellx2000\cellx4000
\pard\intbl Q1\cell Q2\cell\row
\trowd\cellx2000\cellx4000
\pard\intbl 2024\cell 120\cell\row}";
        let doc = parse(repeated);
        let grid = table_of(&doc);
        assert_eq!(grid.header_rows, vec![0, 1]);
    }

    /// A table that marks no row keeps the reading it always had: its first
    /// row heads the columns, and nothing claims to have been declared.
    #[test]
    fn an_unmarked_table_keeps_its_first_row_as_the_header() {
        let rtf = br"{\rtf1\ansi
\trowd\cellx2000\cellx4000
\pard\intbl Year\cell Revenue\cell\row
\trowd\cellx2000\cellx4000
\pard\intbl 2024\cell 120\cell\row}";
        let doc = parse(rtf);
        let grid = table_of(&doc);
        assert!(grid.header_rows.is_empty(), "{grid:#?}");
        assert!(grid.header_row(0));
        assert!(!grid.header_row(1));
    }

    /// The words of a cell are its own, even when the cell holds more than one
    /// paragraph: the break is kept inside the cell instead of becoming a
    /// paragraph of the page.
    #[test]
    fn a_cells_paragraphs_stay_in_the_cell() {
        let rtf = br"{\rtf1\ansi
\trowd\cellx3000\cellx6000
\pard\intbl First line\par Second line\cell Other\cell\row}";
        let doc = parse(rtf);
        assert!(
            texts(&doc).is_empty(),
            "the cell's words escaped the table: {:?}",
            texts(&doc)
        );
        let grid = table_of(&doc);
        assert_eq!(grid.num_rows, 1);
        assert_eq!(
            grid.cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>(),
            vec!["First line\nSecond line", "Other"]
        );
    }

    /// A merged cell does not need `\clmgf`: a writer can say it by ending the
    /// cell at a further boundary, which is how LibreOffice saves a merge it
    /// read from Word. The grid's columns are the boundaries, so that cell is
    /// as wide as the boundaries it crosses.
    #[test]
    fn a_cell_ending_at_a_further_boundary_spans_its_columns() {
        let rtf = br"{\rtf1\ansi
\trowd\cellx2000\cellx4000\cellx8000
\pard\intbl Class\cell Student\cell Results\cell\row
\trowd\cellx2000\cellx4000\cellx6000\cellx8000
\pard\intbl A\cell Alice\cell 71\cell 84\cell\row}";
        let doc = parse(rtf);
        let grid = table_of(&doc);
        assert_eq!((grid.num_rows, grid.num_cols), (2, 4));
        let results = grid
            .cells
            .iter()
            .find(|cell| cell.text == "Results")
            .expect("the wide cell");
        assert_eq!((results.row, results.col, results.col_span), (0, 2, 2));
        // Nothing was invented to fill the columns it covers.
        assert!(
            grid.cells.iter().all(|cell| cell.row != 0 || cell.col < 3),
            "{:#?}",
            grid.cells
        );
    }

    /// `\clmgf`/`\clmrg` are one cell across several columns, and the slots it
    /// covers are not cells of their own.
    #[test]
    fn a_horizontal_merge_is_one_cell_across_its_columns() {
        let rtf = br"{\rtf1\ansi
\trowd\clmgf\cellx2000\clmrg\cellx4000\clmrg\cellx6000
\pard\intbl Wide\cell\cell\cell\row
\trowd\cellx2000\cellx4000\cellx6000
\pard\intbl Left\cell Middle\cell Right\cell\row}";
        let doc = parse(rtf);
        let grid = table_of(&doc);
        assert_eq!((grid.num_rows, grid.num_cols), (2, 3));
        let wide = grid
            .cells
            .iter()
            .find(|cell| cell.text == "Wide")
            .expect("the merged cell");
        assert_eq!((wide.row, wide.col, wide.col_span), (0, 0, 3));
        assert!(
            grid.cells.iter().all(|cell| cell.row != 0 || cell.col == 0),
            "a slot under the merge became a cell: {:#?}",
            grid.cells
        );
    }

    /// `\clvmgf`/`\clvmrg` are one cell over several rows: the continuation row
    /// has its other cells, but the merged column is covered by the cell above.
    #[test]
    fn a_vertical_merge_is_one_cell_over_its_rows() {
        let rtf = br"{\rtf1\ansi
\trowd\clvmgf\cellx2000\cellx4000
\pard\intbl Tall\cell Beside\cell\row
\trowd\clvmrg\cellx2000\cellx4000
\pard\intbl\cell Under\cell\row}";
        let doc = parse(rtf);
        let grid = table_of(&doc);
        assert_eq!((grid.num_rows, grid.num_cols), (2, 2));
        let tall = grid
            .cells
            .iter()
            .find(|cell| cell.text == "Tall")
            .expect("the merged cell");
        assert_eq!((tall.row, tall.col, tall.row_span), (0, 0, 2));
        let under = grid
            .cells
            .iter()
            .find(|cell| cell.text == "Under")
            .expect("the cell beside the merge");
        assert_eq!((under.row, under.col, under.row_span), (1, 1, 1));
        assert!(
            grid.cells.iter().all(|cell| cell.col != 0 || cell.row == 0),
            "the merged column got a second cell: {:#?}",
            grid.cells
        );
    }

    /// The stylesheet says which styles are headings, `\sN` says which style a
    /// paragraph uses, and the paragraph comes out labelled at the depth the
    /// style's outline level gives — `Title` as the document's title, a style
    /// with no outline level as an ordinary paragraph.
    #[test]
    fn the_stylesheet_labels_headings_and_their_depth() {
        let rtf = br"{\rtf1\ansi{\stylesheet{\s0 Normal;}{\s1\outlinelevel0 heading 1;}{\s2\outlinelevel1 heading 2;}{\s15 Title;}{\s3 Body Text;}}
\pard\s15 Quarterly Review\par
\pard\s1 Introduction\par
\pard\s2 Details\par
\pard\s3 Just words\par}";
        let doc = parse(rtf);
        let labelled: Vec<(&str, &str, Option<i32>)> = doc
            .items
            .iter()
            .filter_map(|block| match block {
                Block::Text {
                    text, label, level, ..
                } => Some((text.as_str(), label.as_str(), *level)),
                _ => None,
            })
            .collect();
        assert_eq!(
            labelled,
            vec![
                ("Quarterly Review", "title", None),
                ("Introduction", "section_header", Some(1)),
                ("Details", "section_header", Some(2)),
                ("Just words", "paragraph", None),
            ]
        );
    }

    /// A paragraph can state its own outline level, and a style whose name is
    /// `Heading N` is a heading even when it carries no outline level.
    #[test]
    fn a_paragraph_outline_level_and_a_heading_name_are_headings() {
        let rtf = br"{\rtf1\ansi{\stylesheet{\s4 heading 3;}}
\pard\outlinelevel2 Overview\par
\pard\s4 Deep\par
\pard Body\par}";
        let doc = parse(rtf);
        let labelled: Vec<(&str, &str, Option<i32>)> = doc
            .items
            .iter()
            .filter_map(|block| match block {
                Block::Text {
                    text, label, level, ..
                } => Some((text.as_str(), label.as_str(), *level)),
                _ => None,
            })
            .collect();
        assert_eq!(
            labelled,
            vec![
                ("Overview", "section_header", Some(3)),
                ("Deep", "section_header", Some(3)),
                ("Body", "paragraph", None),
            ]
        );
    }

    /// A heading style names the paragraph it is applied to even when the
    /// document writes no stylesheet at all: reading a style's sentence as the
    /// page's words would put `heading 1;` on the page.
    #[test]
    fn a_stylesheet_is_not_page_text() {
        let rtf = br"{\rtf1\ansi{\stylesheet{\s1\outlinelevel0 heading 1;}}\pard\s1 Title\par}";
        let doc = parse(rtf);
        assert_eq!(texts(&doc), vec!["Title"]);
    }
}
