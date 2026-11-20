//! Decoding of embedded image XObjects (PDF 1.7, clause 8.9).
//!
//! Scanned PDFs carry their page as a single image XObject rather than as
//! text-drawing operators, so OCR needs these pixels. We normalise every image
//! to PNG or JPEG bytes, which is what the OCR backend can consume directly.

use std::io::Cursor;

use hayro_ccitt::{DecodeSettings as CcittSettings, Decoder as CcittDecoder, EncodingMode};
use hayro_jbig2::{DecodeSettings as Jbig2Settings, Decoder as Jbig2Decoder};
use pdfextractor_limits::MAX_IMAGE_PIXELS;

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
///
/// The buffer is cut to the grid it describes. The PNG encoder asserts an exact
/// length rather than returning an error, so an over-long buffer would abort the
/// process; the extra bytes lie past the declared `/Width` × `/Height` grid,
/// which is all a renderer shows, so cutting keeps the picture where refusing
/// would lose it. A buffer shorter than the grid has no last rows to encode, so
/// it is refused.
pub fn encode_gray(width: u32, height: u32, samples: &[u8]) -> Option<Vec<u8>> {
    let len = grid_len(width, height, 1)?;
    let img = image::GrayImage::from_raw(width, height, samples.get(..len)?.to_vec())?;
    encode(image::DynamicImage::ImageLuma8(img))
}

/// Encode 8-bit RGB samples as PNG.
///
/// Cut to the grid and refused when short, for the reason [`encode_gray`] gives.
pub fn encode_rgb(width: u32, height: u32, samples: &[u8]) -> Option<Vec<u8>> {
    let len = grid_len(width, height, 3)?;
    let img = image::RgbImage::from_raw(width, height, samples.get(..len)?.to_vec())?;
    encode(image::DynamicImage::ImageRgb8(img))
}

/// The samples a `width` × `height` grid of `channels` 8-bit components holds.
fn grid_len(width: u32, height: u32, channels: usize) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(channels)
}

/// Encode 8-bit CMYK samples as PNG, converting to RGB.
///
/// Cut to the grid and refused when short, for the reason [`encode_gray`] gives.
/// Both lengths come from [`grid_len`] as well, so a direct caller handing in an
/// enormous `/Width` × `/Height` pair is refused rather than overflowing the
/// four bytes a pixel needs or the three the conversion writes.
pub fn encode_cmyk(width: u32, height: u32, samples: &[u8]) -> Option<Vec<u8>> {
    let len = grid_len(width, height, 4)?;
    let samples = samples.get(..len)?;
    let mut rgb = Vec::with_capacity(grid_len(width, height, 3)?);
    for chunk in samples.as_chunks::<4>().0 {
        let (c, m, y, k) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        // Naive CMYK -> RGB; PDF CMYK is device-dependent, so exactness is impossible.
        rgb.push(((255 - c) as u16 * (255 - k) as u16 / 255) as u8);
        rgb.push(((255 - m) as u16 * (255 - k) as u16 / 255) as u8);
        rgb.push(((255 - y) as u16 * (255 - k) as u16 / 255) as u8);
    }
    encode_rgb(width, height, &rgb)
}

/// The `/DecodeParms` of a `/CCITTFaxDecode` image stream.
///
/// `/Columns` is the encoded row width. The parser falls back to the image's
/// own `/Width` when it is absent, rather than to the specification's 1728
/// default: an image XObject already states its grid, and using 1728 columns
/// for a 620-pixel scan would produce a picture four times too wide.
#[derive(Debug, Clone)]
pub struct CcittParams {
    /// `K`: negative is Group 4, zero is Group 3 one-dimensional, positive is
    /// Group 3 two-dimensional.
    pub k: i64,
    pub end_of_line: bool,
    pub end_of_block: bool,
    pub encoded_byte_align: bool,
}

impl Default for CcittParams {
    fn default() -> Self {
        Self {
            k: 0,
            end_of_line: false,
            end_of_block: true,
            encoded_byte_align: false,
        }
    }
}

/// Decode a `/CCITTFaxDecode` image to 8-bit grayscale samples.
///
/// The result is `columns * rows` bytes, black at 0 and white at 255, which is
/// the layout [`encode_gray`] consumes. CCITT encodes run colours rather than
/// sample values, so `/BlackIs1` (which selects 0 or 1 as black in the PDF
/// samples) does not change the pixels an OCR pass needs to see.
pub fn decode_ccitt(data: &[u8], columns: u32, rows: u32, params: &CcittParams) -> Option<Vec<u8>> {
    decode_ccitt_limited(data, columns, rows, params, MAX_IMAGE_PIXELS)
}

/// [`decode_ccitt`] with an explicit ceiling on the declared pixel grid.
///
/// The cap is checked before the grayscale output buffer is allocated. A PDF
/// may state an arbitrarily large `/Columns` and `/Height`; the parser passes
/// its configured limit here, while direct callers get the shared default.
pub fn decode_ccitt_limited(
    data: &[u8],
    columns: u32,
    rows: u32,
    params: &CcittParams,
    max_pixels: u64,
) -> Option<Vec<u8>> {
    if columns == 0 || rows == 0 {
        return None;
    }
    let pixels = u64::from(columns) * u64::from(rows);
    if pixels > max_pixels {
        return None;
    }
    let output_len = usize::try_from(pixels).ok()?;
    let encoding = match params.k {
        k if k < 0 => EncodingMode::Group4,
        0 => EncodingMode::Group3_1D,
        _ => EncodingMode::Group3_2D,
    };
    let settings = CcittSettings {
        columns,
        rows,
        end_of_block: params.end_of_block,
        end_of_line: params.end_of_line,
        rows_are_byte_aligned: params.encoded_byte_align,
        encoding,
        strict: false,
    };

    struct Luma8 {
        output: Vec<u8>,
        columns: usize,
        decoded_rows: u32,
        overflowed: bool,
    }

    impl CcittDecoder for Luma8 {
        fn push_row(&mut self, changes: &[u32]) {
            if self.overflowed {
                return;
            }
            let Some(start) = (self.decoded_rows as usize).checked_mul(self.columns) else {
                self.overflowed = true;
                return;
            };
            let Some(end) = start.checked_add(self.columns) else {
                self.overflowed = true;
                return;
            };
            let Some(row) = self.output.get_mut(start..end) else {
                self.overflowed = true;
                return;
            };

            for pair in changes.chunks(2) {
                let first = pair[0] as usize;
                let last = pair.get(1).copied().unwrap_or(self.columns as u32) as usize;
                let end = last.min(row.len());
                if first < end {
                    row[first..end].fill(0);
                }
            }
            self.decoded_rows += 1;
        }
    }

    let mut decoder = Luma8 {
        output: vec![0xFF; output_len],
        columns: columns as usize,
        decoded_rows: 0,
        overflowed: false,
    };
    let mut context = hayro_ccitt::DecoderContext::new(settings);
    let result = hayro_ccitt::decode(data, &mut decoder, &mut context);
    if decoder.overflowed || decoder.decoded_rows == 0 {
        return None;
    }
    if result.is_err() {
        // A malformed tail after at least one row is common in real scans; the
        // rows that did decode are still better than dropping the page.
        tracing::debug!(
            "a CCITT image decoded {} of {rows} rows",
            decoder.decoded_rows
        );
    }
    Some(decoder.output)
}

/// Decode a PDF `/JBIG2Decode` image stream to 8-bit grayscale samples.
///
/// `globals` is the decoded `/DecodeParms /JBIG2Globals` stream, when the
/// document shares a symbol dictionary between pages. The dimensions come from
/// the JBIG2 page-information segment rather than the PDF image dictionary,
/// because the segment is what the coded rows are laid out against.
pub fn decode_jbig2(data: &[u8], globals: Option<&[u8]>) -> Option<(u32, u32, Vec<u8>)> {
    decode_jbig2_limited(data, globals, MAX_IMAGE_PIXELS)
}

/// [`decode_jbig2`] with a ceiling on the page grid.
///
/// A JBIG2 page-information segment states its own width and height, so the
/// PDF image dictionary is not the only place a bomb can ask for a huge grid.
/// The ceiling is checked before the grayscale buffer is allocated.
pub fn decode_jbig2_limited(
    data: &[u8],
    globals: Option<&[u8]>,
    max_pixels: u64,
) -> Option<(u32, u32, Vec<u8>)> {
    let settings = Jbig2Settings { strict: false };
    let globals = match globals {
        Some(bytes) => Some(hayro_jbig2::Globals::new(bytes, &settings).ok()?),
        None => None,
    };
    let page = hayro_jbig2::Page::new_embedded(data, globals.as_ref(), &settings).ok()?;
    let width = page.width();
    let height = page.height();
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > max_pixels {
        return None;
    }
    let output_len = (width as usize).checked_mul(height as usize)?;

    struct Luma8 {
        output: Vec<u8>,
        pos: usize,
        overflowed: bool,
    }

    impl Jbig2Decoder for Luma8 {
        fn push_pixel(&mut self, black: bool) {
            let Some(pixel) = self.output.get_mut(self.pos) else {
                self.overflowed = true;
                return;
            };
            if black {
                *pixel = 0;
            }
            self.pos += 1;
        }

        fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
            let Some(count) = (chunk_count as usize).checked_mul(8) else {
                self.overflowed = true;
                return;
            };
            let Some(end) = self.pos.checked_add(count) else {
                self.overflowed = true;
                return;
            };
            let Some(chunk) = self.output.get_mut(self.pos..end) else {
                self.overflowed = true;
                return;
            };
            if black {
                chunk.fill(0);
            }
            self.pos = end;
        }

        fn next_line(&mut self) {}
    }

    let mut decoder = Luma8 {
        output: vec![0xFF; output_len],
        pos: 0,
        overflowed: false,
    };
    page.decode(&mut decoder).ok()?;
    if decoder.overflowed {
        return None;
    }
    Some((width, height, decoder.output))
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
    fn ccitt_group3_decodes_a_black_row() {
        // The specification's one-row example: a white run of 0 then a black
        // run of 8. hayro's own filter test uses the same bytes.
        let params = CcittParams::default();
        let pixels = decode_ccitt(&[0x35, 0x14], 8, 1, &params).expect("the example decodes");
        assert_eq!(pixels, vec![0; 8]);
    }

    #[test]
    fn ccitt_rejects_a_degenerate_grid() {
        let params = CcittParams::default();
        assert!(decode_ccitt(&[0x35, 0x14], 0, 1, &params).is_none());
        assert!(decode_ccitt(&[0x35, 0x14], 8, 0, &params).is_none());
    }

    fn packed_bits(bits: &[u8]) -> Vec<u8> {
        bits.chunks(8)
            .map(|chunk| {
                chunk
                    .iter()
                    .enumerate()
                    .fold(0u8, |byte, (index, bit)| byte | (bit << (7 - index)))
            })
            .collect()
    }

    #[test]
    fn ccitt_group3_two_dimensional_decodes_a_pass_line() {
        // Group 3 2D prefixes each line with a tag: 1 for a 1D-coded line, 0
        // for a 2D-coded one. The first row is the known white-0/black-8 1D
        // example; the second is a 2D pass over that all-black reference, so it
        // comes out all white.
        let data = packed_bits(&[
            1, // 1D row
            0, 0, 1, 1, 0, 1, 0, 1, // white run 0
            0, 0, 0, 1, 0, 1, 0, 0, // black run 8
            0, // 2D row
            0, 0, 0, 1, // pass mode
        ]);
        let params = CcittParams {
            k: 1,
            ..CcittParams::default()
        };
        let pixels = decode_ccitt(&data, 8, 2, &params).expect("the 2D stream decodes");
        assert_eq!(pixels[..8], [0; 8], "the reference row is black");
        assert_eq!(pixels[8..], [255; 8], "the pass row is white");
    }

    #[test]
    fn ccitt_refuses_a_declared_grid_over_the_pixel_cap() {
        // The row data is valid, so an uncapped decode would return 8,000
        // samples; the declared grid is refused before that buffer is made.
        let params = CcittParams::default();
        assert!(decode_ccitt_limited(&[0x35, 0x14], 8, 1_000, &params, 100).is_none());
    }

    /// A minimal embedded JBIG2 page: one page-information segment with the
    /// requested grid and no regions. Hayro can decode it as a blank page.
    fn jbig2_page_info(width: u32, height: u32) -> Vec<u8> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&0u32.to_be_bytes()); // segment number
        stream.push(0x30); // page information, one-byte page association
        stream.push(0); // no referred-to segments
        stream.push(1); // page 1
        stream.extend_from_slice(&19u32.to_be_bytes()); // page-information data
        stream.extend_from_slice(&width.to_be_bytes());
        stream.extend_from_slice(&height.to_be_bytes());
        stream.extend_from_slice(&0u32.to_be_bytes()); // x resolution unknown
        stream.extend_from_slice(&0u32.to_be_bytes()); // y resolution unknown
        stream.push(0); // flags
        stream.extend_from_slice(&0u16.to_be_bytes()); // no striping
        stream
    }

    #[test]
    fn jbig2_refuses_a_declared_grid_over_the_pixel_cap() {
        let stream = jbig2_page_info(100, 100);
        assert!(decode_jbig2_limited(&stream, None, 9_999).is_none());
        assert_eq!(
            decode_jbig2_limited(&stream, None, 10_000).map(|(width, height, _)| (width, height)),
            Some((100, 100)),
            "the minimal page decodes when its grid fits"
        );
    }

    #[test]
    fn jbig2_rejects_an_empty_stream() {
        assert!(decode_jbig2(&[], None).is_none());
    }

    #[test]
    fn gray_encodes_to_png() {
        let png = encode_gray(2, 2, &[0, 128, 200, 255]).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    /// The PNG encoder asserts its buffer is exactly the grid, so a stream that
    /// carried more samples than the image declares must be cut here rather
    /// than reach the encoder (a fuzzed PDF took the process down that way).
    #[test]
    fn a_longer_sample_buffer_is_cut_to_the_grid() {
        let png = encode_rgb(2, 2, &[7; 13]).expect("the first twelve samples encode");
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        let png = encode_gray(2, 2, &[7; 5]).expect("the first four samples encode");
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        let png = encode_rgb(2, 2, &[7; 12]).expect("exactly the grid encodes");
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        let png = encode_cmyk(2, 2, &[7; 17]).expect("the first sixteen samples encode");
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
    }

    #[test]
    fn a_sample_buffer_shorter_than_the_grid_is_refused() {
        assert!(encode_gray(2, 2, &[7; 3]).is_none());
        assert!(encode_rgb(2, 2, &[7; 11]).is_none());
        assert!(encode_rgb(u32::MAX, u32::MAX, &[7; 12]).is_none());
        assert!(encode_cmyk(2, 2, &[7; 15]).is_none());
        // The four bytes a CMYK pixel needs are computed from the grid, so the
        // dimensions a multiplication would overflow are refused here.
        assert!(encode_cmyk(u32::MAX, u32::MAX, &[7; 24]).is_none());
        assert!(encode_cmyk(1 << 31, 1 << 31, &[7; 24]).is_none());
    }
}
