//! Decoding of a device-independent bitmap (`BITMAPINFO`) as the metafiles
//! carry one.
//!
//! Two records need this: a WMF's `DIBSTRETCHBLT`/`STRETCHDIB` and an EMF's
//! `STRETCHDIBITS`/`SETDIBITSTODEVICE` embed the bitmap they draw, header,
//! palette and all, inside the record. A drawing that is a picture pasted into
//! Word — an icon, a photograph, a screenshot — is usually one of these rather
//! than vector art, so a renderer that skipped them would produce an empty page
//! for exactly the files a reader cares most about.
//!
//! Only the uncompressed forms are decoded. The run-length encodings
//! (`BI_RLE8`, `BI_RLE4`) are refused by name rather than drawn wrongly, and the
//! caller reports the record as one it could not draw.
//!
//! The header's layout is read here and nowhere else, for a second reason: a
//! `\wbitmap`/`\dibitmap` payload in an RTF file is this same DIB with no file
//! header in front of it, and the RTF reader has to wrap a `BITMAPFILEHEADER`
//! around it. The wrapper needs the pixel offset, which is the same sum of
//! header, palette and mask bytes the renderer steps over — so both read
//! [`header`] instead of counting those bytes twice, where a `biClrUsed` above
//! eight bits per pixel, a mask stored outside a 40-byte header, or a core
//! header's three-byte palette entries could be counted one way by the wrapper
//! and another by the renderer.

use crate::paint::{Bitmap, Color};

/// What a DIB header states about where its pixels are.
///
/// Read by the renderer below and by the RTF reader's byte-for-byte BMP
/// wrapper, so the two cannot disagree about a header's layout. Only the
/// layout: whether the pixels can be *drawn* is [`info`]'s question, and
/// whether the payload is long enough to open is the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Width in pixels, as the header states it.
    pub width: u32,
    /// Height in pixels, without its sign.
    pub height: u32,
    /// Whether the rows are stored top-down: the header states a negative
    /// height.
    pub top_down: bool,
    /// Bits per pixel.
    pub bit_count: u16,
    /// `biCompression`: `BI_RGB` (0) and `BI_BITFIELDS` (3) are the ones this
    /// crate draws; the rest are named by [`info`] when it refuses them.
    pub compression: u32,
    /// The header's own length: 12 for a `BITMAPCOREHEADER`, the stated size
    /// for anything newer.
    pub header_size: usize,
    /// Where the palette starts, counted from the beginning of the DIB.
    pub palette_offset: usize,
    /// Bytes per palette entry: three for the core header, four otherwise.
    pub palette_entry_size: usize,
    /// Entries in the palette. A header that declares `biClrUsed` with more
    /// than eight bits per pixel still has a table of that many entries
    /// between itself and the pixels, even though the pixels are direct colour.
    pub palette_entries: usize,
    /// Where the colour masks start, counted from the beginning of the DIB: a
    /// 40-byte header leaves them just past itself, and a V4 or V5 header keeps
    /// them in fixed fields at byte 40 of its own body. `None` when the
    /// compression states no masks.
    pub masks_offset: Option<usize>,
    /// Where the pixel data starts, counted from the beginning of the DIB.
    pub bits_offset: usize,
}

/// Read the header of a `BITMAPINFO` and work out where its pixels are.
///
/// `Err` when the bytes are not a header whose layout is known, or when the
/// header runs past the data it is read from: a caller with an offset to
/// publish would otherwise state one no reader could open.
pub fn header(data: &[u8]) -> Result<Header, &'static str> {
    if data.len() < 12 {
        return Err("a bitmap header shorter than 12 bytes");
    }
    let header_size = u32_at(data, 0).unwrap() as usize;
    if header_size == 12 {
        return core_header(data);
    }
    if header_size < 40 {
        return Err("an unknown bitmap header");
    }
    if data.len() < header_size {
        return Err("a bitmap header that runs past the record");
    }
    let width = i32::from_le_bytes(data[4..8].try_into().unwrap());
    let height = i32::from_le_bytes(data[8..12].try_into().unwrap());
    let bit_count = u16::from_le_bytes(data[14..16].try_into().unwrap());
    let compression = u32::from_le_bytes(data[16..20].try_into().unwrap());
    if width <= 0 {
        return Err("a bitmap with no width");
    }
    if height == 0 {
        return Err("a bitmap with no height");
    }

    // The palette is written after the header, which for a V4 or V5 header
    // means after its extra fields — `header_size` covers that. `biClrUsed`
    // is authoritative when it says anything, at any depth.
    let palette_entries = match u32::from_le_bytes(data[32..36].try_into().unwrap()) {
        0 if bit_count <= 8 => 1usize << bit_count,
        0 => 0,
        declared => declared as usize,
    };
    // A `BITMAPINFOHEADER` of exactly 40 bytes leaves the colour masks outside
    // itself, between it and the palette; a V4 or V5 header states them in
    // fixed fields of its own body, so nothing has to be stepped over. Both
    // spellings put them at byte 40 either way.
    let mask_bytes = match (header_size, compression) {
        (40, 3) => 12, // BI_BITFIELDS: three channel masks
        (40, 6) => 16, // BI_ALPHABITFIELDS: three masks and an alpha mask
        _ => 0,
    };
    let masks_offset = matches!(compression, 3 | 6).then_some(40);
    let palette_offset = header_size
        .checked_add(mask_bytes)
        .ok_or("a bitmap header larger than the record")?;
    let bits_offset = after_palette(palette_offset, palette_entries, 4)?;
    Ok(Header {
        width: width as u32,
        height: height.unsigned_abs(),
        top_down: height < 0,
        bit_count,
        compression,
        header_size,
        palette_offset,
        palette_entry_size: 4,
        palette_entries,
        masks_offset,
        bits_offset,
    })
}

/// Where the palette ends — and so where the pixels begin. `Err` when the table
/// claims more bytes than an offset can hold (a truncated declaration read off
/// the wire), which also keeps every arithmetic step below a caller's bounds
/// check inside `usize`.
fn after_palette(offset: usize, entries: usize, entry_size: usize) -> Result<usize, &'static str> {
    let bytes = entries
        .checked_mul(entry_size)
        .ok_or("a bitmap palette larger than the record")?;
    offset
        .checked_add(bytes)
        .ok_or("a bitmap palette larger than the record")
}

/// The `BITMAPCOREHEADER`, which predates the palette's fourth byte.
fn core_header(data: &[u8]) -> Result<Header, &'static str> {
    let width = u16_at(data, 4).unwrap() as u32;
    let height = u16_at(data, 6).unwrap() as u32;
    let bit_count = u16_at(data, 10).unwrap();
    if width == 0 || height == 0 {
        return Err("a bitmap with no extent");
    }
    let palette_entries = if bit_count <= 8 {
        1usize << bit_count
    } else {
        0
    };
    Ok(Header {
        width,
        height,
        top_down: false,
        bit_count,
        compression: 0,
        header_size: 12,
        palette_offset: 12,
        palette_entry_size: 3,
        palette_entries,
        masks_offset: None,
        bits_offset: after_palette(12, palette_entries, 3)?,
    })
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Where the pixels start, how long they are, and whether they can be drawn.
pub(crate) struct Info {
    /// The layout, read once.
    pub header: Header,
    /// Length of the pixel data in bytes.
    pub bits_len: usize,
}

/// Refuse what cannot be drawn, and work out how many bytes the pixels take.
pub(crate) fn info(data: &[u8]) -> Result<Info, &'static str> {
    let header = header(data)?;
    if !matches!(header.compression, 0 | 3) {
        return Err("a compressed bitmap");
    }
    if header.compression == 3 && !matches!(header.bit_count, 16 | 32) {
        return Err("colour masks on a bitmap that cannot use them");
    }
    let row_bytes =
        row_bytes(header.bit_count, header.width).ok_or("an unsupported colour depth")?;
    let bits_len = row_bytes
        .checked_mul(header.height as usize)
        .ok_or("a bitmap larger than the record")?;
    if header.bits_offset + bits_len > data.len() {
        return Err("a bitmap whose pixels run past the record");
    }
    Ok(Info { header, bits_len })
}

/// Pixels per row, padded to a four-byte boundary as every DIB is.
fn row_bytes(bpp: u16, width: u32) -> Option<usize> {
    if !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) {
        return None;
    }
    let bits = (bpp as u64) * (width as u64);
    Some((bits.div_ceil(32) * 4) as usize)
}

/// Decode a `BITMAPINFO`, palette and all, into an opaque bitmap whose row 0 is
/// the top: a DIB with a positive height is stored bottom-up, and every record
/// that draws one places it as it is seen.
pub(crate) fn decode(data: &[u8]) -> Result<Bitmap, &'static str> {
    let info = info(data)?;
    let layout = info.header;
    // Grown by the loop rather than reserved from the declared count: a header
    // can state any `biClrUsed`, and the bound that matters is the data's.
    let mut palette: Vec<Color> = Vec::new();
    for i in 0..layout.palette_entries {
        let at = layout.palette_offset + i * layout.palette_entry_size;
        if at + layout.palette_entry_size > data.len() {
            return Err("a bitmap palette that runs past the record");
        }
        palette.push(Color {
            r: data[at + 2],
            g: data[at + 1],
            b: data[at],
        });
    }
    // `BI_BITFIELDS` on a 40-byte header keeps its masks outside it; a V4 or V5
    // header keeps them in the fields this crate does not step over, and the
    // layout says so by naming no offset.
    let (red_mask, green_mask, blue_mask) = match layout.masks_offset {
        Some(at) if layout.compression == 3 => (
            u32_at(data, at).ok_or("a bitmap whose colour masks run past the record")?,
            u32_at(data, at + 4).ok_or("a bitmap whose colour masks run past the record")?,
            u32_at(data, at + 8).ok_or("a bitmap whose colour masks run past the record")?,
        ),
        _ => (0, 0, 0),
    };

    let stride = row_bytes(layout.bit_count, layout.width).ok_or("an unsupported colour depth")?;
    let pixels = &data[layout.bits_offset..layout.bits_offset + info.bits_len];
    let mut out = vec![255u8; (layout.width as usize) * (layout.height as usize) * 4];
    for row in 0..layout.height as usize {
        // The row as it appears: the last stored row of a bottom-up bitmap is
        // its first visible one.
        let stored = if layout.top_down {
            row
        } else {
            layout.height as usize - 1 - row
        };
        let line = &pixels[stored * stride..(stored + 1) * stride];
        for column in 0..layout.width as usize {
            let color = sample(
                line,
                column,
                layout.bit_count,
                &palette,
                red_mask,
                green_mask,
                blue_mask,
            );
            let at = (row * layout.width as usize + column) * 4;
            out[at] = color.r;
            out[at + 1] = color.g;
            out[at + 2] = color.b;
            out[at + 3] = 255;
        }
    }
    Bitmap::new(layout.width, layout.height, out).ok_or("a bitmap that decoded to the wrong size")
}

fn sample(
    line: &[u8],
    column: usize,
    bpp: u16,
    palette: &[Color],
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
) -> Color {
    let palette_at =
        |index: usize| -> Color { palette.get(index).copied().unwrap_or(Color::BLACK) };
    match bpp {
        1 | 4 | 8 => {
            let index = match bpp {
                8 => line.get(column).copied().unwrap_or(0) as usize,
                4 => {
                    let byte = line.get(column / 2).copied().unwrap_or(0);
                    if column.is_multiple_of(2) {
                        (byte >> 4) as usize
                    } else {
                        (byte & 0x0f) as usize
                    }
                }
                _ => {
                    let byte = line.get(column / 8).copied().unwrap_or(0);
                    ((byte >> (7 - column % 8)) & 1) as usize
                }
            };
            palette_at(index)
        }
        16 => {
            let at = column * 2;
            let value = u16::from_le_bytes([
                line.get(at).copied().unwrap_or(0),
                line.get(at + 1).copied().unwrap_or(0),
            ]) as u32;
            if red_mask != 0 {
                masked(value, red_mask, green_mask, blue_mask)
            } else {
                // BI_RGB at 16 bits is 5-5-5.
                Color {
                    r: (((value >> 10) & 0x1f) * 255 / 31) as u8,
                    g: (((value >> 5) & 0x1f) * 255 / 31) as u8,
                    b: ((value & 0x1f) * 255 / 31) as u8,
                }
            }
        }
        24 => {
            let at = column * 3;
            Color {
                b: line.get(at).copied().unwrap_or(0),
                g: line.get(at + 1).copied().unwrap_or(0),
                r: line.get(at + 2).copied().unwrap_or(0),
            }
        }
        _ => {
            let at = column * 4;
            let value = u32::from_le_bytes([
                line.get(at).copied().unwrap_or(0),
                line.get(at + 1).copied().unwrap_or(0),
                line.get(at + 2).copied().unwrap_or(0),
                line.get(at + 3).copied().unwrap_or(0),
            ]);
            if red_mask != 0 {
                masked(value, red_mask, green_mask, blue_mask)
            } else {
                Color {
                    b: (value & 0xff) as u8,
                    g: ((value >> 8) & 0xff) as u8,
                    r: ((value >> 16) & 0xff) as u8,
                }
            }
        }
    }
}

/// Split a pixel through its channel masks, scaling each to 0–255.
fn masked(value: u32, red_mask: u32, green_mask: u32, blue_mask: u32) -> Color {
    let channel = |mask: u32| -> u8 {
        let shift = mask.trailing_zeros();
        if mask == 0 {
            return 0;
        }
        let maximum = (mask >> shift) as u64;
        let raw = ((value & mask) >> shift) as u64;
        ((raw * 255 + maximum / 2) / maximum) as u8
    };
    Color {
        r: channel(red_mask),
        g: channel(green_mask),
        b: channel(blue_mask),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bottom-up `BITMAPINFOHEADER` with no palette, for the tests below.
    fn dib_header(width: i32, height: i32, bpp: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 40];
        bytes[0..4].copy_from_slice(&40u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&width.to_le_bytes());
        bytes[8..12].copy_from_slice(&height.to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&bpp.to_le_bytes());
        bytes
    }

    /// A DIB stores rows bottom-up; every record that draws one means it to be
    /// seen the other way round, so decoding has to turn it over.
    #[test]
    fn a_bottom_up_bitmap_is_turned_over() {
        let mut dib = dib_header(2, 2, 24);
        // Bottom row: blue. Top row: red, then white. A 24-bit row of two
        // pixels is six bytes, padded to the four-byte boundary the format
        // requires, so each row below is eight bytes.
        dib.extend_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0]);
        dib.extend_from_slice(&[0, 0, 255, 255, 255, 255, 0, 0]);
        let bitmap = decode(&dib).expect("decoded");
        assert_eq!(bitmap.width, 2);
        assert_eq!(bitmap.height, 2);
        assert_eq!(bitmap.color_at(0, 1), Some(Color { r: 0, g: 0, b: 255 }));
        assert_eq!(bitmap.color_at(0, 0), Some(Color { r: 255, g: 0, b: 0 }));
        assert_eq!(bitmap.color_at(1, 0), Some(Color::WHITE));
    }

    /// A one-bit-per-pixel bitmap is a mask: its palette decides whether a set
    /// bit means black or white, and the transparent-icon idiom depends on it.
    #[test]
    fn a_one_bit_bitmap_uses_its_palette() {
        let mut dib = dib_header(8, 1, 1);
        // The two palette entries a 1-bit DIB has by default: black, white.
        dib.extend_from_slice(&[0, 0, 0, 0, 255, 255, 255, 0]);
        // 0b10100000: dots at columns 0 and 2.
        dib.extend_from_slice(&[0b1010_0000, 0, 0, 0]);
        let bitmap = decode(&dib).expect("decoded");
        assert_eq!(bitmap.color_at(0, 0), Some(Color::WHITE));
        assert_eq!(bitmap.color_at(1, 0), Some(Color::BLACK));
        assert_eq!(bitmap.color_at(2, 0), Some(Color::WHITE));
        assert_eq!(bitmap.color_at(3, 0), Some(Color::BLACK));
    }

    /// An eight-bit bitmap indexes a palette written after the header.
    #[test]
    fn an_eight_bit_bitmap_indexes_its_palette() {
        let mut dib = dib_header(2, 1, 8);
        dib[32..36].copy_from_slice(&2u32.to_le_bytes()); // biClrUsed
        dib.extend_from_slice(&[0, 0, 255, 0]); // index 0: red
        dib.extend_from_slice(&[0, 255, 0, 0]); // index 1: green
        dib.extend_from_slice(&[0, 1, 0, 0]);
        let bitmap = decode(&dib).expect("decoded");
        assert_eq!(bitmap.color_at(0, 0), Some(Color { r: 255, g: 0, b: 0 }));
        assert_eq!(bitmap.color_at(1, 0), Some(Color { r: 0, g: 255, b: 0 }));
    }

    /// The header's layout is what the RTF wrapper publishes as a BMP's pixel
    /// offset, so it is parsed on its own account here: a 40-byte header keeps
    /// `BI_BITFIELDS`' three masks outside itself, and its twelve bytes come
    /// before the palette and the pixels.
    #[test]
    fn a_bitfields_header_keeps_its_masks_outside() {
        let mut dib = dib_header(2, 2, 16);
        dib[16..20].copy_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
        let layout = header(&dib).expect("a known header");
        assert_eq!(layout.header_size, 40);
        assert_eq!(layout.masks_offset, Some(40));
        assert_eq!(layout.palette_entries, 0);
        assert_eq!(layout.bits_offset, 40 + 12);
    }

    /// `BI_ALPHABITFIELDS` has a fourth mask, and the offset has to count all
    /// four: the renderer refuses to draw it, but the bytes are still there and
    /// a wrapper that pointed past only three would open the wrong pixels.
    #[test]
    fn an_alpha_bitfields_header_steps_over_four_masks() {
        let mut dib = dib_header(2, 2, 32);
        dib[16..20].copy_from_slice(&6u32.to_le_bytes()); // BI_ALPHABITFIELDS
        let layout = header(&dib).expect("a known header");
        assert_eq!(layout.masks_offset, Some(40));
        assert_eq!(layout.bits_offset, 40 + 16);
        assert!(info(&dib).is_err(), "the renderer draws neither");
    }

    /// A V4 header states the masks in fixed fields at byte 40 of its own body,
    /// so nothing sits between it and the palette — the size it states is the
    /// whole header — but the masks are still read where they are, not at the
    /// header's end.
    #[test]
    fn a_v4_header_keeps_its_masks_inside() {
        let mut dib = vec![0u8; 108];
        dib[0..4].copy_from_slice(&108u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&2i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
        let layout = header(&dib).expect("a known header");
        assert_eq!(layout.header_size, 108);
        assert_eq!(layout.masks_offset, Some(40));
        assert_eq!(layout.bits_offset, 108);
    }

    /// A V4 header's masks are read at byte 40 of its body. Reading them where
    /// the header ends would take the palette's first bytes for channel masks
    /// and paint every pixel the wrong colour.
    #[test]
    fn a_v4_header_reads_its_masks_where_they_are() {
        let mut dib = vec![0u8; 108];
        dib[0..4].copy_from_slice(&108u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
        dib[40..44].copy_from_slice(&0x00ff_0000u32.to_le_bytes()); // red
        dib[44..48].copy_from_slice(&0x0000_ff00u32.to_le_bytes()); // green
        dib[48..52].copy_from_slice(&0x0000_00ffu32.to_le_bytes()); // blue
        dib.extend_from_slice(&[0x00, 0x00, 0xff, 0x00]); // one red BGRA pixel
        let bitmap = decode(&dib).expect("decoded");
        assert_eq!(bitmap.color_at(0, 0), Some(Color { r: 255, g: 0, b: 0 }));
    }

    /// `biClrUsed` above eight bits per pixel still puts a colour table between
    /// the header and the pixels. The table is not used to read the pixels —
    /// they are direct colour — but it is in the file, and a renderer that did
    /// not step over it would draw the table as pixels.
    #[test]
    fn a_declared_colour_table_is_stepped_over_at_any_depth() {
        let mut dib = dib_header(1, 1, 24);
        dib[32..36].copy_from_slice(&2u32.to_le_bytes()); // biClrUsed
        let layout = header(&dib).expect("a known header");
        assert_eq!(layout.palette_entries, 2);
        assert_eq!(layout.palette_entry_size, 4);
        assert_eq!(layout.bits_offset, 40 + 8);
    }

    /// The core header predates the palette's fourth byte, and a negative
    /// height says the rows are already the right way up.
    #[test]
    fn a_core_header_has_three_byte_palette_entries() {
        let mut dib = vec![0u8; 16];
        dib[0..4].copy_from_slice(&12u32.to_le_bytes());
        dib[4..6].copy_from_slice(&4u16.to_le_bytes());
        dib[6..8].copy_from_slice(&2u16.to_le_bytes());
        dib[10..12].copy_from_slice(&4u16.to_le_bytes());
        let layout = header(&dib).expect("a known header");
        assert_eq!(layout.header_size, 12);
        assert_eq!(layout.palette_entries, 16);
        assert_eq!(layout.palette_entry_size, 3);
        assert_eq!(layout.bits_offset, 12 + 16 * 3);

        let mut dib = dib_header(2, -2, 32);
        dib.extend_from_slice(&[0; 16]);
        let layout = header(&dib).expect("a known header");
        assert!(
            layout.top_down,
            "a negative height is not a top-down bitmap"
        );
        assert_eq!(layout.height, 2);
    }

    /// A run-length encoded bitmap is refused by name: drawing it as raw rows
    /// would put garbage on the page.
    #[test]
    fn a_compressed_bitmap_is_refused() {
        let mut dib = dib_header(4, 4, 4);
        dib[16..20].copy_from_slice(&2u32.to_le_bytes()); // BI_RLE4
        let error = decode(&dib).expect_err("refused");
        assert!(error.contains("compressed"), "{error}");
    }
}
