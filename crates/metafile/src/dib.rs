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

use crate::paint::{Bitmap, Color};

/// Where the pixels start and how they are laid out.
pub(crate) struct Info {
    pub width: u32,
    pub height: u32,
    /// Bits per pixel.
    pub bpp: u16,
    /// `BI_BITFIELDS`: three channel masks follow the header.
    bitfields: bool,
    /// Offset of the pixel data from the start of the `BITMAPINFO`.
    pub bits_offset: usize,
    /// Length of the pixel data in bytes.
    pub bits_len: usize,
}

/// Read the header of a `BITMAPINFO` and work out where its pixels are.
pub(crate) fn info(data: &[u8]) -> Result<Info, &'static str> {
    if data.len() < 12 {
        return Err("a bitmap header shorter than 12 bytes");
    }
    let header_size = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    if header_size == 12 {
        return core_info(data);
    }
    if header_size < 40 {
        return Err("an unknown bitmap header");
    }
    if data.len() < header_size {
        return Err("a bitmap header that runs past the record");
    }
    let width = i32::from_le_bytes(data[4..8].try_into().unwrap());
    let height = i32::from_le_bytes(data[8..12].try_into().unwrap());
    let bpp = u16::from_le_bytes(data[14..16].try_into().unwrap());
    let compression = u32::from_le_bytes(data[16..20].try_into().unwrap());
    if width <= 0 {
        return Err("a bitmap with no width");
    }
    if height == 0 {
        return Err("a bitmap with no height");
    }
    let height = height.unsigned_abs();

    // The palette is written after the header, which for a V4 or V5 header
    // means after its extra fields — `header_size` covers that.
    let palette_entries = if bpp <= 8 {
        let used = u32::from_le_bytes(data[32..36].try_into().unwrap());
        if used == 0 {
            1usize << bpp
        } else {
            used as usize
        }
    } else {
        0
    };
    if !matches!(compression, 0 | 3) {
        return Err("a compressed bitmap");
    }
    let mut bits_offset = header_size + palette_entries * 4;
    if compression == 3 {
        // BI_BITFIELDS: the masks sit between the header and the palette.
        if bpp != 16 && bpp != 32 {
            return Err("colour masks on a bitmap that cannot use them");
        }
        bits_offset += 12;
    }
    let row_bytes = row_bytes(bpp, width as u32).ok_or("an unsupported colour depth")?;
    let bits_len = row_bytes
        .checked_mul(height as usize)
        .ok_or("a bitmap larger than the record")?;
    if bits_offset + bits_len > data.len() {
        return Err("a bitmap whose pixels run past the record");
    }
    Ok(Info {
        width: width as u32,
        height,
        bpp,
        bitfields: compression == 3,
        bits_offset,
        bits_len,
    })
}

/// The `BITMAPCOREHEADER`, which predates the palette's fourth byte.
fn core_info(data: &[u8]) -> Result<Info, &'static str> {
    let width = u16::from_le_bytes(data[4..6].try_into().unwrap()) as u32;
    let height = u16::from_le_bytes(data[6..8].try_into().unwrap()) as u32;
    let bpp = u16::from_le_bytes(data[10..12].try_into().unwrap());
    if width == 0 || height == 0 {
        return Err("a bitmap with no extent");
    }
    let palette_entries = if bpp <= 8 { 1usize << bpp } else { 0 };
    let bits_offset = 12 + palette_entries * 3;
    let row_bytes = row_bytes(bpp, width).ok_or("an unsupported colour depth")?;
    let bits_len = row_bytes
        .checked_mul(height as usize)
        .ok_or("a bitmap larger than the record")?;
    if bits_offset + bits_len > data.len() {
        return Err("a bitmap whose pixels run past the record");
    }
    Ok(Info {
        width,
        height,
        bpp,
        bitfields: false,
        bits_offset,
        bits_len,
    })
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
    let header_size = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let core = header_size == 12;
    // A negative height means the rows are already stored top-down.
    let top_down = if core {
        false
    } else {
        i32::from_le_bytes(data[8..12].try_into().unwrap()) < 0
    };
    let palette_entries = if info.bpp <= 8 {
        if core {
            1usize << info.bpp
        } else {
            let used = u32::from_le_bytes(data[32..36].try_into().unwrap());
            if used == 0 {
                1usize << info.bpp
            } else {
                used as usize
            }
        }
    } else {
        0
    };
    let entry_size = if core { 3 } else { 4 };
    let palette_start = if core { 12 } else { header_size };
    let mut palette: Vec<Color> = Vec::with_capacity(palette_entries);
    for i in 0..palette_entries {
        let at = palette_start + i * entry_size;
        if at + entry_size > data.len() {
            return Err("a bitmap palette that runs past the record");
        }
        palette.push(Color {
            r: data[at + 2],
            g: data[at + 1],
            b: data[at],
        });
    }
    let (red_mask, green_mask, blue_mask) = if info.bitfields {
        let at = header_size;
        (
            u32::from_le_bytes(data[at..at + 4].try_into().unwrap()),
            u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()),
            u32::from_le_bytes(data[at + 8..at + 12].try_into().unwrap()),
        )
    } else {
        (0, 0, 0)
    };

    let stride = row_bytes(info.bpp, info.width).ok_or("an unsupported colour depth")?;
    let pixels = &data[info.bits_offset..info.bits_offset + info.bits_len];
    let mut out = vec![255u8; (info.width as usize) * (info.height as usize) * 4];
    for row in 0..info.height as usize {
        // The row as it appears: the last stored row of a bottom-up bitmap is
        // its first visible one.
        let stored = if top_down {
            row
        } else {
            info.height as usize - 1 - row
        };
        let line = &pixels[stored * stride..(stored + 1) * stride];
        for column in 0..info.width as usize {
            let color = sample(
                line, column, info.bpp, &palette, red_mask, green_mask, blue_mask,
            );
            let at = (row * info.width as usize + column) * 4;
            out[at] = color.r;
            out[at + 1] = color.g;
            out[at + 2] = color.b;
            out[at + 3] = 255;
        }
    }
    Bitmap::new(info.width, info.height, out).ok_or("a bitmap that decoded to the wrong size")
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
                    if column % 2 == 0 {
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
    fn header(width: i32, height: i32, bpp: u16) -> Vec<u8> {
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
        let mut dib = header(2, 2, 24);
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
        let mut dib = header(8, 1, 1);
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
        let mut dib = header(2, 1, 8);
        dib[32..36].copy_from_slice(&2u32.to_le_bytes()); // biClrUsed
        dib.extend_from_slice(&[0, 0, 255, 0]); // index 0: red
        dib.extend_from_slice(&[0, 255, 0, 0]); // index 1: green
        dib.extend_from_slice(&[0, 1, 0, 0]);
        let bitmap = decode(&dib).expect("decoded");
        assert_eq!(bitmap.color_at(0, 0), Some(Color { r: 255, g: 0, b: 0 }));
        assert_eq!(bitmap.color_at(1, 0), Some(Color { r: 0, g: 255, b: 0 }));
    }

    /// A run-length encoded bitmap is refused by name: drawing it as raw rows
    /// would put garbage on the page.
    #[test]
    fn a_compressed_bitmap_is_refused() {
        let mut dib = header(4, 4, 4);
        dib[16..20].copy_from_slice(&2u32.to_le_bytes()); // BI_RLE4
        let error = decode(&dib).expect_err("refused");
        assert!(error.contains("compressed"), "{error}");
    }
}
