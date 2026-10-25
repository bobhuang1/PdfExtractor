//! Decoding of embedded image XObjects (PDF 1.7, clause 8.9).
//!
//! Scanned PDFs carry their page as a single image XObject rather than as
//! text-drawing operators, so OCR needs these pixels. We normalise every image
//! to PNG or JPEG bytes, which is what the OCR backend can consume directly.

use std::io::Cursor;

/// An image ready to hand to the OCR backend.
#[derive(Debug, Clone)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    /// Encoded image data (PNG or JPEG).
    pub data: Vec<u8>,
    pub mime: &'static str,
}

impl PageImage {
    pub fn is_usable(&self) -> bool {
        // Skip degenerate or decorative images; OCR needs something substantial.
        self.width >= 32 && self.height >= 32
    }
}

/// Encode 8-bit grayscale samples as PNG.
pub fn encode_gray(width: u32, height: u32, samples: &[u8]) -> Option<Vec<u8>> {
    let img = image::GrayImage::from_raw(width, height, samples.to_vec())?;
    encode(image::DynamicImage::ImageLuma8(img))
}

/// Encode 8-bit RGB samples as PNG.
pub fn encode_rgb(width: u32, height: u32, samples: &[u8]) -> Option<Vec<u8>> {
    let img = image::RgbImage::from_raw(width, height, samples.to_vec())?;
    encode(image::DynamicImage::ImageRgb8(img))
}

/// Encode 8-bit CMYK samples as PNG, converting to RGB.
pub fn encode_cmyk(width: u32, height: u32, samples: &[u8]) -> Option<Vec<u8>> {
    let pixel_count = (width as usize).checked_mul(height as usize)?;
    if samples.len() < pixel_count * 4 {
        return None;
    }
    let mut rgb = Vec::with_capacity(pixel_count * 3);
    for chunk in samples.as_chunks::<4>().0.iter().take(pixel_count) {
        let (c, m, y, k) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        // Naive CMYK -> RGB; PDF CMYK is device-dependent, so exactness is impossible.
        rgb.push(((255 - c) as u16 * (255 - k) as u16 / 255) as u8);
        rgb.push(((255 - m) as u16 * (255 - k) as u16 / 255) as u8);
        rgb.push(((255 - y) as u16 * (255 - k) as u16 / 255) as u8);
    }
    encode_rgb(width, height, &rgb)
}

/// Expand 1-bit-per-pixel grayscale data (as PDF packs it, MSB first, rows
/// padded to byte boundaries) into 8-bit samples.
pub fn expand_1bit_gray(width: u32, height: u32, packed: &[u8]) -> Option<Vec<u8>> {
    let row_bytes = (width as usize).div_ceil(8);
    let needed = row_bytes.checked_mul(height as usize)?;
    if packed.len() < needed {
        return None;
    }
    let mut out = Vec::with_capacity((width as usize) * (height as usize));
    for row in 0..height as usize {
        let row_start = row * row_bytes;
        for col in 0..width as usize {
            let byte = packed[row_start + col / 8];
            let bit = (byte >> (7 - (col % 8))) & 1;
            // 0 is black, 1 is white in PDF DeviceGray.
            out.push(if bit == 0 { 0 } else { 255 });
        }
    }
    Some(out)
}

/// Undo a `/DecodeParms /Predictor` transform applied before compression.
///
/// Ignoring a predictor silently corrupts image data, so the common cases are
/// handled explicitly rather than assumed away.
pub fn apply_predictor(
    data: &[u8],
    predictor: i64,
    colors: usize,
    bpc: usize,
    columns: usize,
) -> Option<Vec<u8>> {
    match predictor {
        1 => Some(data.to_vec()),
        2 => tiff_predictor(data, colors, bpc, columns),
        10..=15 => png_predictor(data, colors, bpc, columns),
        _ => None,
    }
}

fn row_len(colors: usize, bpc: usize, columns: usize) -> usize {
    (columns * colors * bpc).div_ceil(8)
}

/// PNG predictors (10-15): every row is prefixed by its filter type byte.
fn png_predictor(data: &[u8], colors: usize, bpc: usize, columns: usize) -> Option<Vec<u8>> {
    let len = row_len(colors, bpc, columns);
    if len == 0 {
        return None;
    }
    // Bytes per pixel, rounded up; at least 1 so `Sub` can make progress.
    let bpp = (colors * bpc).div_ceil(8).max(1);

    let mut out = Vec::new();
    let mut prev = vec![0u8; len];
    let mut i = 0usize;
    while i < data.len() {
        let filter = data[i];
        i += 1;
        if i + len > data.len() {
            break; // truncated final row
        }
        let mut row = data[i..i + len].to_vec();
        i += len;

        for x in 0..len {
            let left = if x >= bpp { row[x - bpp] } else { 0 };
            let up = prev[x];
            let up_left = if x >= bpp { prev[x - bpp] } else { 0 };
            row[x] = match filter {
                0 => row[x],
                1 => row[x].wrapping_add(left),
                2 => row[x].wrapping_add(up),
                3 => row[x].wrapping_add(((left as u16 + up as u16) / 2) as u8),
                4 => row[x].wrapping_add(paeth(left, up, up_left)),
                _ => return None,
            };
        }
        out.extend_from_slice(&row);
        prev = row;
    }
    Some(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// TIFF predictor 2, implemented for 8-bit samples.
fn tiff_predictor(data: &[u8], colors: usize, bpc: usize, columns: usize) -> Option<Vec<u8>> {
    if bpc != 8 {
        return None;
    }
    let len = row_len(colors, bpc, columns);
    if len == 0 {
        return None;
    }
    let bpp = colors.max(1);
    let mut out = data.to_vec();
    for row_start in (0..out.len()).step_by(len) {
        let row_end = (row_start + len).min(out.len());
        for x in (row_start + bpp)..row_end {
            out[x] = out[x].wrapping_add(out[x - bpp]);
        }
    }
    Some(out)
}

fn encode(img: image::DynamicImage) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_one_bit_rows_with_padding() {
        // 3 pixels wide -> 1 byte per row, most significant bits used first.
        // In DeviceGray, 0 is black and 1 is white.
        // Row 0: bits 101 -> 0b1010_0000 = 0xA0  => white, black, white
        // Row 1: bits 010 -> 0b0100_0000 = 0x40  => black, white, black
        let packed = [0xA0u8, 0x40u8];
        let out = expand_1bit_gray(3, 2, &packed).unwrap();
        assert_eq!(out, vec![255, 0, 255, 0, 255, 0]);
    }

    #[test]
    fn rejects_short_input() {
        assert!(expand_1bit_gray(64, 64, &[0u8; 4]).is_none());
    }

    #[test]
    fn predictor_none_is_identity_minus_filter_bytes() {
        // One row, filter 0 (None): [ft, 10, 20, 30]
        let out = apply_predictor(&[0, 10, 20, 30], 12, 1, 8, 3).unwrap();
        assert_eq!(out, vec![10, 20, 30]);
    }

    #[test]
    fn predictor_up_accumulates_previous_row() {
        // Row 0 filter None, row 1 filter Up.
        let data = [0u8, 10, 20, 30, 2u8, 5, 5, 5];
        let out = apply_predictor(&data, 12, 1, 8, 3).unwrap();
        assert_eq!(out, vec![10, 20, 30, 15, 25, 35]);
    }

    #[test]
    fn predictor_sub_uses_left_neighbour() {
        // filter 1 (Sub): each byte adds the reconstructed byte bpp to the left.
        let out = apply_predictor(&[1u8, 10, 5, 5], 12, 1, 8, 3).unwrap();
        assert_eq!(out, vec![10, 15, 20]);
    }

    #[test]
    fn predictor_one_is_passthrough() {
        let data = [1u8, 2, 3];
        assert_eq!(apply_predictor(&data, 1, 1, 8, 3).unwrap(), data.to_vec());
    }

    #[test]
    fn gray_encodes_to_png() {
        let png = encode_gray(2, 2, &[0, 128, 200, 255]).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }
}
