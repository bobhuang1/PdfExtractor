//! Format sniffing and the header fields that make a page.
//!
//! Every reader here is deliberately small: the pixel grid, the declared
//! resolution and the camera's orientation are the only facts a page needs,
//! and all three live in the file's opening records. Decoding the pixels would
//! duplicate what Tesseract's own image loader does, and this crate is not the
//! OCR engine — it only says which way round the reader should take them.

use anyhow::{bail, Context, Result};
use std::io::Cursor;
use std::sync::Once;

/// The resolution assumed when a file does not declare one. 72 dpi makes one
/// pixel one point, which is the same default Docling's image backend takes.
pub const DEFAULT_DPI: f64 = 72.0;

/// Which of EXIF's eight ways a camera stored the picture.
///
/// The stored grid is not always the page's grid: a phone held sideways writes
/// its pixels on their side and an orientation tag that says how far to turn
/// them. Orientation 1 is the only one where the stored pixels are already the
/// page; the others have to be mirrored, turned, or both before they are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// Stored the way it is meant to be seen (EXIF 1).
    #[default]
    Normal,
    /// Mirrored left to right (EXIF 2).
    FlipHorizontal,
    /// Upside down (EXIF 3).
    Rotate180,
    /// Mirrored top to bottom (EXIF 4).
    FlipVertical,
    /// Transposed over the top-left/bottom-right diagonal (EXIF 5).
    Transpose,
    /// Stored on its side; turn 90° clockwise to read it upright (EXIF 6).
    Rotate90,
    /// Transversed over the other diagonal (EXIF 7).
    Transverse,
    /// Stored on its other side; turn 270° clockwise (EXIF 8).
    Rotate270,
}

impl Orientation {
    /// The orientation an EXIF tag value names. Values outside 1–8 are not an
    /// orientation this reader knows, and an image that declares one is read as
    /// stored rather than turned by a guess.
    pub fn from_exif(value: u8) -> Self {
        match value {
            2 => Orientation::FlipHorizontal,
            3 => Orientation::Rotate180,
            4 => Orientation::FlipVertical,
            5 => Orientation::Transpose,
            6 => Orientation::Rotate90,
            7 => Orientation::Transverse,
            8 => Orientation::Rotate270,
            _ => Orientation::Normal,
        }
    }

    /// The EXIF tag value this orientation is written as.
    pub fn exif(self) -> u8 {
        match self {
            Orientation::Normal => 1,
            Orientation::FlipHorizontal => 2,
            Orientation::Rotate180 => 3,
            Orientation::FlipVertical => 4,
            Orientation::Transpose => 5,
            Orientation::Rotate90 => 6,
            Orientation::Transverse => 7,
            Orientation::Rotate270 => 8,
        }
    }

    /// True when turning the stored pixels upright swaps their width and
    /// height: the four quarter-turn orientations.
    pub fn swaps_axes(self) -> bool {
        matches!(
            self,
            Orientation::Transpose
                | Orientation::Rotate90
                | Orientation::Transverse
                | Orientation::Rotate270
        )
    }

    /// The turn that puts the stored pixels upright, in the vocabulary of the
    /// decoder that applies it. `None` when the pixels are already upright.
    fn turn(self) -> Option<image::metadata::Orientation> {
        use image::metadata::Orientation as Turn;
        Some(match self {
            Orientation::Normal => return None,
            Orientation::FlipHorizontal => Turn::FlipHorizontal,
            Orientation::Rotate180 => Turn::Rotate180,
            Orientation::FlipVertical => Turn::FlipVertical,
            Orientation::Transpose => Turn::Rotate90FlipH,
            Orientation::Rotate90 => Turn::Rotate90,
            Orientation::Transverse => Turn::Rotate270FlipH,
            Orientation::Rotate270 => Turn::Rotate270,
        })
    }
}

/// A picture whose stored pixels have been turned upright, and the type of the
/// bytes that carry them.
#[derive(Debug, Clone, PartialEq)]
pub struct UprightPicture {
    /// The re-encoded picture. It no longer carries an orientation tag, because
    /// the pixels are what it says they are.
    pub data: Vec<u8>,
    /// `image/jpeg` when the picture was a JPEG, `image/png` otherwise.
    pub mimetype: &'static str,
    pub width: u32,
    pub height: u32,
}

/// A picture re-encoded as a PNG, with the pixel grid of the bytes it carries.
///
/// The type is fixed because the point of the transcode is a type Docling can
/// name. The size is the grid the picture was written at, which is the size
/// its PNG header states — a TIFF sheet is measured after its own orientation
/// turn, so this is its upright grid rather than the grid the directory stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PngPicture {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The picture's pixels turned the way its EXIF orientation says, re-encoded
/// so that the file needs no tag to be read upright.
///
/// `None` when the picture is already upright, declares an orientation this
/// reader does not know, or is in a format whose pixels this cannot decode —
/// an untouched picture rather than a lossy guess. A JPEG stays a JPEG
/// (re-encoded at quality 90); every other format is written losslessly as a
/// PNG, which is the encoder this can always rely on.
///
/// This is what a consumer that writes a picture out has to use: the package
/// keeps the pixels the camera stored, while the office application shows the
/// picture turned the way the tag says.
pub fn turn_upright(data: &[u8]) -> Option<UprightPicture> {
    let document = ImageDocument::parse_bytes(data).ok()?;
    let turn = document.orientation.turn()?;
    let mut bitmap = image::load_from_memory(data).ok()?;
    bitmap.apply_orientation(turn);
    let (mimetype, encoded) = if document.mimetype == "image/jpeg" {
        let mut encoded = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 90);
        bitmap.write_with_encoder(encoder).ok()?;
        ("image/jpeg", encoded)
    } else {
        ("image/png", encode_png(&bitmap)?)
    };
    Some(UprightPicture {
        width: bitmap.width(),
        height: bitmap.height(),
        data: encoded,
        mimetype,
    })
}

/// The pixels turned by an EXIF orientation, as a lossless PNG.
///
/// A recognition engine reads the stored pixels without consulting the tag, so
/// this is the form a sideways picture has to reach it in. `None` when the
/// orientation is already upright or the pixels cannot be decoded.
pub fn upright_png(data: &[u8], orientation: Orientation) -> Option<Vec<u8>> {
    let mut bitmap = image::load_from_memory(data).ok()?;
    bitmap.apply_orientation(orientation.turn()?);
    encode_png(&bitmap)
}

/// The same pixels under a media type Docling can name: a PNG.
///
/// Docling validates an `ImageRef`'s media type against its own registry, which
/// has no entry for `image/webp` or `image/jp2`. A picture in either encoding
/// would therefore be a box with no image however good its pixels are. They can
/// be decoded, so they are handed on as PNG — the same choice
/// `crates/metafile` makes for a Word drawing stored as EMF or WMF, and the
/// reason that drawing is a picture a page can point at.
///
/// `None` when the bytes do not decode, so a caller keeps the payload it had
/// rather than inventing pixels. WebP is decoded by `image` itself; JPEG 2000
/// by `hayro-jpeg2000`, whose decoder registers itself with `image` the first
/// time one is read. A GIF or an animated WebP is `image`'s too, and what it
/// decodes is the first frame — the one picture of a file that may hold an
/// animation, whose other frames [`gif_frames`] and [`webp_frames`] read as the
/// pages they are. A TIFF is read the same way, first directory only: a picture
/// carried inside a document is one still, while [`tiff_frames`] is what a
/// scanned batch's other sheets are read by.
///
/// A picture whose pixels are already upright is transcoded as stored: the
/// callers that care about the EXIF tag (a Word or PowerPoint package) apply
/// [`turn_upright`] first, and one that has no tag has nothing to turn.
pub fn transcode_to_png(data: &[u8]) -> Option<PngPicture> {
    // Registration is process-wide and idempotent; `Once` keeps every picture
    // after the first from going through the hook check again.
    static JPEG2000: Once = Once::new();
    JPEG2000.call_once(|| {
        hayro_jpeg2000::integration::register_decoding_hook();
    });

    let bitmap = image::load_from_memory(data).ok()?;
    let (width, height) = (bitmap.width(), bitmap.height());
    Some(PngPicture {
        data: encode_png(&bitmap)?,
        width,
        height,
    })
}

fn encode_png(bitmap: &image::DynamicImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    bitmap
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// Every frame of a GIF, in the order the file plays them.
///
/// A GIF can hold an animation: frames drawn one after another over a single
/// logical screen. [`ImageDocument`] states the page — that screen — and
/// [`transcode_to_png`] the first frame, because a picture carried *inside* a
/// document is one still. A GIF arriving as the input file is the whole
/// animation, though, and each frame is a page a reader turns: this returns all
/// of them, each composited on the screen the way a player shows it, so a frame
/// that only repaints part of the canvas keeps what the frames before it left
/// there.
///
/// A frame is laid on white. The GIF's transparent index is absence of ink, not
/// a hole in the page — a page is white paper, and a recogniser handed a
/// transparent background reads its own composite of it.
///
/// Empty when the bytes are not a GIF or do not decode, so a caller keeps the
/// single page it would otherwise write.
pub fn gif_frames(data: &[u8]) -> Vec<PngPicture> {
    use image::AnimationDecoder;

    if mimetype(data) != Some("image/gif") {
        return Vec::new();
    }
    let Ok(decoder) = image::codecs::gif::GifDecoder::new(Cursor::new(data)) else {
        return Vec::new();
    };
    let Ok(frames) = decoder.into_frames().collect_frames() else {
        return Vec::new();
    };
    frames.into_iter().filter_map(picture_of_frame).collect()
}

/// Every frame of an animated WebP, in the order the file plays them.
///
/// A WebP can be an animation as a GIF can: an `ANIM` chunk states the canvas
/// and each `ANMF` chunk is a frame painted on it. [`ImageDocument`] states the
/// page — the canvas — and [`transcode_to_png`] the first frame, because a
/// picture carried *inside* a document is one still. A WebP arriving as the
/// input file is the whole animation, and each frame is a page a reader turns:
/// this returns all of them, each composited on the canvas the way the decoder
/// shows it, so a frame that repaints only part of the canvas keeps what the
/// frames before it left there. A still WebP returns nothing; the caller
/// already has its one page.
///
/// A frame is laid on white for the reason a GIF's frame is: a page is white
/// paper, not a hole in it.
///
/// Empty when the bytes are not a WebP, are not animated, or do not decode, so
/// a caller keeps the single page it would otherwise write.
pub fn webp_frames(data: &[u8]) -> Vec<PngPicture> {
    use image::AnimationDecoder;

    if mimetype(data) != Some("image/webp") {
        return Vec::new();
    }
    let Ok(decoder) = image::codecs::webp::WebPDecoder::new(Cursor::new(data)) else {
        return Vec::new();
    };
    if !decoder.has_animation() {
        return Vec::new();
    }
    let Ok(frames) = decoder.into_frames().collect_frames() else {
        return Vec::new();
    };
    frames.into_iter().filter_map(picture_of_frame).collect()
}

/// One decoded animation frame as a PNG on white, with its pixel grid.
fn picture_of_frame(frame: image::Frame) -> Option<PngPicture> {
    let mut buffer = frame.into_buffer();
    let (width, height) = buffer.dimensions();
    for pixel in buffer.pixels_mut() {
        let [red, green, blue, alpha] = pixel.0;
        let over_white = |channel: u8| {
            ((u16::from(channel) * u16::from(alpha) + 255 * (255 - u16::from(alpha))) / 255) as u8
        };
        *pixel = image::Rgba([over_white(red), over_white(green), over_white(blue), 255]);
    }
    let bitmap = image::DynamicImage::ImageRgba8(buffer);
    Some(PngPicture {
        data: encode_png(&bitmap)?,
        width,
        height,
    })
}

/// Every page of a TIFF, in the order the file stores them, each as its own
/// PNG.
///
/// A TIFF is a chain of image file directories rather than one picture: a
/// batch run through a scanner's feeder, a fax, a document an archive saved as
/// "TIFF, multi-page" are one file with a sheet per directory. [`ImageDocument`]
/// states the first page — its grid, resolution and orientation — and
/// [`transcode_to_png`] the first page's pixels, because a picture carried
/// *inside* a document is one still. A TIFF arriving as the input file is the
/// whole stack, though, and each sheet is a page a reader turns: this returns
/// all of them, each at its own pixel grid, since a batch's sheets need not be
/// the same size, and each turned the way its own directory's orientation tag
/// says. A sheet's tag is the sheet's statement: a scanner that fed one page
/// sideways does not say anything about the pages after it.
///
/// A page is laid on white where it declares transparency, for the reason a
/// GIF's frames are: a page is white paper, not a hole in it.
///
/// Empty when the bytes are not a TIFF or its directories cannot be read, so a
/// caller keeps the single page it would otherwise write. A page whose colour
/// type or sample size this does not convert — a palette, a CMYK, a YCbCr, a
/// bilevel fax page — is left out rather than guessed at; the pages that do
/// read are still pages of the file.
pub fn tiff_frames(data: &[u8]) -> Vec<PngPicture> {
    if mimetype(data) != Some("image/tiff") {
        return Vec::new();
    }
    let Ok(mut decoder) = tiff::decoder::Decoder::new(Cursor::new(data)) else {
        return Vec::new();
    };
    let mut pages = Vec::new();
    loop {
        if let Some((width, height, rgba)) = tiff_page(&mut decoder) {
            let bitmap = image::RgbaImage::from_raw(width, height, rgba)
                .map(image::DynamicImage::ImageRgba8);
            if let Some(encoded) = bitmap.as_ref().and_then(encode_png) {
                pages.push(PngPicture {
                    data: encoded,
                    width,
                    height,
                });
            }
        }
        if !decoder.more_images() {
            break;
        }
        if decoder.next_image().is_err() {
            break;
        }
    }
    pages
}

/// One directory of a TIFF as RGBA pixels, laid on white and turned upright by
/// that directory's own orientation tag.
///
/// `None` when the directory states no grid this can read, or a colour type and
/// sample size this does not convert. A 16-bit sample keeps its top byte: an
/// 8-bit page is what a page is written as, and the alternative is throwing a
/// computed rescale at pages that already read.
#[allow(clippy::type_complexity)]
fn tiff_page(decoder: &mut tiff::decoder::Decoder<Cursor<&[u8]>>) -> Option<(u32, u32, Vec<u8>)> {
    use tiff::decoder::DecodingResult;

    let (width, height) = decoder.dimensions().ok()?;
    let orientation = tiff_orientation(decoder);
    let color = decoder.colortype().ok()?;
    let channels = match color {
        tiff::ColorType::Gray(8 | 16) => 1,
        tiff::ColorType::GrayA(8 | 16) => 2,
        tiff::ColorType::RGB(8 | 16) => 3,
        tiff::ColorType::RGBA(8 | 16) => 4,
        _ => return None,
    };
    let samples: Vec<u8> = match decoder.read_image().ok()? {
        DecodingResult::U8(values) => values,
        DecodingResult::U16(values) => values.into_iter().map(|value| (value >> 8) as u8).collect(),
        _ => return None,
    };
    let pixels = (width as usize).checked_mul(height as usize)?;
    if samples.len() < pixels * channels {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels * 4);
    for pixel in samples.chunks_exact(channels) {
        let (red, green, blue, alpha) = match channels {
            1 => (pixel[0], pixel[0], pixel[0], 255),
            2 => (pixel[0], pixel[0], pixel[0], pixel[1]),
            3 => (pixel[0], pixel[1], pixel[2], 255),
            _ => (pixel[0], pixel[1], pixel[2], pixel[3]),
        };
        // Over white, as a GIF's frame is: a page is white paper.
        let over_white = |channel: u8| {
            ((u16::from(channel) * u16::from(alpha) + 255 * (255 - u16::from(alpha))) / 255) as u8
        };
        rgba.extend_from_slice(&[over_white(red), over_white(green), over_white(blue), 255]);
    }
    // The tag describes the pixels this directory stores, not the first
    // directory's: each sheet of a batch is turned by its own statement.
    let mut bitmap =
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(width, height, rgba)?);
    if let Some(turn) = orientation.turn() {
        bitmap.apply_orientation(turn);
    }
    Some((
        bitmap.width(),
        bitmap.height(),
        bitmap.into_rgba8().into_raw(),
    ))
}

/// The orientation tag of the directory the decoder is on.
fn tiff_orientation(decoder: &mut tiff::decoder::Decoder<Cursor<&[u8]>>) -> Orientation {
    decoder
        .get_tag_unsigned::<u16>(tiff::tags::Tag::Orientation)
        .ok()
        .and_then(|value| u8::try_from(value).ok())
        .map(Orientation::from_exif)
        .unwrap_or_default()
}

/// The media types a plain image can be read as a page.
///
/// A picture carried inside a document may be in any format the shared table
/// names; an *input* is a document only when its pixels are one of these, the
/// encodings whose header this reader knows. An SVG arriving as the input file
/// is therefore not a document here. A JPEG 2000 is one in both its spellings:
/// the JP2 container, whose header boxes state the page, and the bare
/// codestream (`.j2c`, `.j2k`, `.jpc`, `.jpf`), whose `SIZ` marker does. Which
/// spelling a file is, is read from its first bytes — a container opens with
/// its signature box, a codestream with `FF 4F FF 51` — so the extension has no
/// vote. A GIF is one: its
/// logical screen states the page, and [`gif_frames`] gives every frame of an
/// animation as a page of its own.
pub const DOCUMENT_MEDIA_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/tiff",
    "image/bmp",
    "image/webp",
    "image/gif",
    "image/jp2",
];

/// Whether [`ImageDocument::parse_bytes`] can read `mimetype` as a page.
pub fn is_document_media_type(mimetype: &str) -> bool {
    DOCUMENT_MEDIA_TYPES.contains(&mimetype)
}

/// A plain image, read as one page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageDocument {
    /// The image's own media type: `image/png`, `image/jpeg`, `image/tiff`,
    /// `image/bmp`, `image/webp`, `image/gif` or `image/jp2`.
    pub mimetype: &'static str,
    /// The stored pixel grid.
    pub width: u32,
    pub height: u32,
    /// The resolution the file declares, in dots per inch. `None` means it
    /// declares none, not that the resolution is zero.
    pub dpi: Option<(f64, f64)>,
    /// How the stored pixels have to be turned to be read, from the EXIF
    /// orientation tag. [`Orientation::Normal`] when the file carries none.
    pub orientation: Orientation,
}

impl ImageDocument {
    /// Read an image's page facts from its header.
    pub fn parse_bytes(data: &[u8]) -> Result<Self> {
        let mimetype = mimetype(data).with_context(|| {
            "not a PNG, JPEG, TIFF, BMP, WebP, GIF or JPEG 2000 image".to_string()
        })?;
        let parsed = match mimetype {
            "image/png" => png(data)?,
            "image/jpeg" => jpeg(data)?,
            "image/tiff" => tiff(data)?,
            "image/bmp" => bmp(data)?,
            "image/webp" => webp(data)?,
            "image/gif" => gif(data)?,
            "image/jp2" => jpeg2000(data)?,
            other => bail!("unsupported image type {other}"),
        };
        if parsed.width == 0 || parsed.height == 0 {
            bail!("the image has no pixels");
        }
        Ok(ImageDocument { mimetype, ..parsed })
    }

    /// The pixel grid as the page is seen: the stored grid, swapped for the
    /// four orientations that put the page on its side.
    pub fn upright_size(&self) -> (u32, u32) {
        if self.orientation.swaps_axes() {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// The page's width in points: the upright pixel grid at the declared
    /// resolution, or one pixel per point when none is declared.
    pub fn width_pt(&self) -> f64 {
        let (width, dpi) = self.upright_axis(true);
        width as f64 * 72.0 / dpi
    }

    /// The page's height in points, measured the same way.
    pub fn height_pt(&self) -> f64 {
        let (height, dpi) = self.upright_axis(false);
        height as f64 * 72.0 / dpi
    }

    /// One upright axis: its pixel count and the resolution measured along it.
    /// A quarter turn moves the stored x resolution to the upright y axis.
    fn upright_axis(&self, horizontal: bool) -> (u32, f64) {
        let (x, y) = self.effective_dpi();
        let swapped = self.orientation.swaps_axes();
        match (horizontal, swapped) {
            (true, true) => (self.height, y),
            (true, false) => (self.width, x),
            (false, true) => (self.width, x),
            (false, false) => (self.height, y),
        }
    }

    /// The declared resolution, or [`DEFAULT_DPI`] for each axis it does not
    /// declare. A non-positive or non-finite value is no declaration at all.
    fn effective_dpi(&self) -> (f64, f64) {
        let fallback = DEFAULT_DPI;
        match self.dpi {
            Some((x, y)) if x.is_finite() && y.is_finite() && x > 0.0 && y > 0.0 => (x, y),
            _ => (fallback, fallback),
        }
    }
}

/// The media type of the image in `data`, from its signature alone.
///
/// A truncated file whose signature is intact still names its format: the
/// caller then reports a broken image rather than a file of no known type.
pub fn mimetype(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 8 && data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if data.len() >= 3 && data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if data.len() >= 4 && (data.starts_with(b"II*\0") || data.starts_with(b"MM\0*")) {
        return Some("image/tiff");
    }
    // `BM` alone is two ASCII letters; a BMP's file header also leaves its
    // four reserved bytes zero, which keeps a text file opening with "BM"
    // from being read as a broken bitmap.
    if data.len() >= 14 && data.starts_with(b"BM") && data[6..10] == [0; 4] {
        return Some("image/bmp");
    }
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    // A GIF's signature is its first six bytes: 87a is the original encoding,
    // 89a the one that added animation and transparency.
    if data.len() >= 6 && (data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        return Some("image/gif");
    }
    // A JPEG 2000 opens one of two ways: a JP2 with its signature box — the
    // box's length, the `jP  ` type and the four magic bytes — or a bare
    // codestream with `SOC` and the `SIZ` marker that states its grid
    // (`FF 4F FF 51`). Both are JPEG 2000; the codestream is the same picture
    // without the container, so it is named by the same media type.
    if data.starts_with(JP2_SIGNATURE) || data.starts_with(JPEG2000_CODESTREAM) {
        return Some("image/jp2");
    }
    None
}

/// The signature box a JP2 file opens with: `LBox = 12`, `TBox = 'jP  '`, then
/// the standard's four magic bytes.
const JP2_SIGNATURE: &[u8] = &[
    0, 0, 0, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// What a bare codestream opens with: `SOC` (`FF 4F`), the start of the
/// codestream, then `SIZ` (`FF 51`), the marker that states its grid. The
/// standard requires `SIZ` to be the first marker after `SOC`, so the four
/// bytes are the signature of an unwrapped JPEG 2000.
const JPEG2000_CODESTREAM: &[u8] = &[0xFF, 0x4F, 0xFF, 0x51];

// --- PNG ------------------------------------------------------------------

fn png(data: &[u8]) -> Result<ImageDocument> {
    let mut width = None;
    let mut height = None;
    let mut dpi = None;
    let mut orientation = Orientation::Normal;
    let mut pos = 8usize;
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().expect("four bytes")) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body_start = pos + 8;
        let body = data
            .get(body_start..body_start.saturating_add(len))
            .with_context(|| format!("truncated PNG {} chunk", String::from_utf8_lossy(kind)))?;
        match kind {
            b"IHDR" => {
                if body.len() < 13 {
                    bail!("a PNG IHDR chunk is 13 bytes, found {}", body.len());
                }
                width = Some(u32::from_be_bytes(
                    body[0..4].try_into().expect("four bytes"),
                ));
                height = Some(u32::from_be_bytes(
                    body[4..8].try_into().expect("four bytes"),
                ));
            }
            b"pHYs" => {
                // Nine bytes: pixels per unit on each axis, then the unit —
                // 1 means metre, 0 means the ratio is only an aspect ratio.
                if body.len() >= 9 && body[8] == 1 {
                    let x = u32::from_be_bytes(body[0..4].try_into().expect("four bytes"));
                    let y = u32::from_be_bytes(body[4..8].try_into().expect("four bytes"));
                    if x > 0 && y > 0 {
                        dpi = Some((x as f64 * 0.0254, y as f64 * 0.0254));
                    }
                }
            }
            // eXIf carries an Exif profile for a PNG. The specification puts
            // the TIFF header first; some writers keep the JPEG `Exif\0\0`
            // prefix, which is skipped when it is there.
            b"eXIf" => {
                orientation = exif_payload(body)
                    .and_then(exif_directory)
                    .map(|exif| exif.orientation())
                    .unwrap_or(Orientation::Normal);
            }
            b"IEND" => break,
            _ => {}
        }
        // Length, type, payload and CRC.
        pos = body_start + len + 4;
    }
    let (Some(width), Some(height)) = (width, height) else {
        bail!("the PNG has no IHDR chunk");
    };
    Ok(ImageDocument {
        mimetype: "image/png",
        width,
        height,
        dpi,
        orientation,
    })
}

// --- JPEG -----------------------------------------------------------------

fn jpeg(data: &[u8]) -> Result<ImageDocument> {
    let mut width = None;
    let mut height = None;
    let mut dpi = None;
    let mut exif = None;
    let mut orientation = Orientation::Normal;
    let mut i = 2usize;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // Fill bytes and standalone markers carry no length.
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if matches!(marker, 0xD8 | 0xD9 | 0xD0..=0xD7 | 0x01) {
            i += 2;
            continue;
        }
        // After the scan header the entropy-coded data is not a marker stream
        // this needs to walk.
        if marker == 0xDA {
            break;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if len < 2 {
            bail!("a JPEG segment declares length {len}");
        }
        let body = data
            .get(i + 4..i + 2 + len)
            .context("truncated JPEG segment")?;
        match marker {
            // APP0: JFIF's density. The unit decides whether the numbers are
            // dots per inch (1) or per centimetre (2); unit 0 is an aspect
            // ratio, not a resolution.
            0xE0 if body.starts_with(b"JFIF\0") && body.len() >= 12 => {
                let unit = body[7];
                let x = u16::from_be_bytes([body[8], body[9]]) as f64;
                let y = u16::from_be_bytes([body[10], body[11]]) as f64;
                if x > 0.0 && y > 0.0 {
                    match unit {
                        1 => dpi = Some((x, y)),
                        2 => dpi = Some((x * 2.54, y * 2.54)),
                        _ => {}
                    }
                }
            }
            // APP1: EXIF's own resolution, which is what a camera writes when
            // the JFIF header carries no unit.
            0xE1 if body.starts_with(b"Exif\0\0") => {
                exif = Some(&body[6..]);
            }
            // SOF0..SOF15, excluding DHT/JPG/DAC: the frame header is where
            // the pixel grid is.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) && body.len() >= 5 => {
                height = Some(u16::from_be_bytes([body[1], body[2]]) as u32);
                width = Some(u16::from_be_bytes([body[3], body[4]]) as u32);
            }
            _ => {}
        }
        i += 2 + len;
    }
    let (Some(width), Some(height)) = (width, height) else {
        bail!("the JPEG has no frame header");
    };
    if let Some(exif) = exif.and_then(exif_directory) {
        if dpi.is_none() {
            dpi = exif.dpi();
        }
        orientation = exif.orientation();
    }
    Ok(ImageDocument {
        mimetype: "image/jpeg",
        width,
        height,
        dpi,
        orientation,
    })
}

// --- TIFF -----------------------------------------------------------------

fn tiff(data: &[u8]) -> Result<ImageDocument> {
    if data.len() < 8 {
        bail!("truncated TIFF header");
    }
    let little = match &data[..2] {
        b"II" => true,
        b"MM" => false,
        _ => bail!("a TIFF starts with II or MM"),
    };
    let ifd = read_u32(data, 4, little) as usize;
    let entries = ifd_entries(data, ifd, little)?;
    let width = entry_number(&entries, little, 256);
    let height = entry_number(&entries, little, 257);
    let x = entry_rational(&entries, data, little, 282);
    let y = entry_rational(&entries, data, little, 283);
    let unit = entry_number(&entries, little, 296).unwrap_or(2.0);
    let orientation = entry_number(&entries, little, 274)
        .map(orientation_of)
        .unwrap_or(Orientation::Normal);
    let dpi = match (x, y) {
        (Some(x), Some(y)) if x > 0.0 && y > 0.0 => {
            let scale = if unit == 3.0 { 2.54 } else { 1.0 };
            Some((x * scale, y * scale))
        }
        (Some(x), None) if x > 0.0 => {
            let scale = if unit == 3.0 { 2.54 } else { 1.0 };
            Some((x * scale, x * scale))
        }
        _ => None,
    };
    let (Some(width), Some(height)) = (width, height) else {
        bail!("the TIFF's first frame declares no size");
    };
    Ok(ImageDocument {
        mimetype: "image/tiff",
        width: width as u32,
        height: height as u32,
        dpi,
        orientation,
    })
}

/// One IFD entry: tag, type, count, and the four-byte value field, whose
/// meaning depends on the type and count.
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    field: [u8; 4],
}

fn ifd_entries(data: &[u8], offset: usize, little: bool) -> Result<Vec<Entry>> {
    if offset + 2 > data.len() {
        bail!("TIFF image file directory at {offset} is out of bounds");
    }
    let count = read_u16(data, offset, little) as usize;
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let at = offset + 2 + index * 12;
        if at + 12 > data.len() {
            bail!("truncated TIFF entry {index}");
        }
        entries.push(Entry {
            tag: read_u16(data, at, little),
            kind: read_u16(data, at + 2, little),
            count: read_u32(data, at + 4, little),
            field: data[at + 8..at + 12].try_into().expect("four bytes"),
        });
    }
    Ok(entries)
}

/// An unsigned integer entry (SHORT or LONG, one value).
fn entry_number(entries: &[Entry], little: bool, tag: u16) -> Option<f64> {
    let entry = entries.iter().find(|entry| entry.tag == tag)?;
    if entry.count == 0 {
        return None;
    }
    match entry.kind {
        // A SHORT is 2 bytes, so it sits at the start of the value field.
        3 => Some(if little {
            u16::from_le_bytes([entry.field[0], entry.field[1]]) as f64
        } else {
            u16::from_be_bytes([entry.field[0], entry.field[1]]) as f64
        }),
        4 => Some(if little {
            u32::from_le_bytes(entry.field) as f64
        } else {
            u32::from_be_bytes(entry.field) as f64
        }),
        _ => None,
    }
}

/// A RATIONAL entry (two unsigned longs: numerator, denominator).
fn entry_rational(entries: &[Entry], data: &[u8], little: bool, tag: u16) -> Option<f64> {
    let entry = entries.iter().find(|entry| entry.tag == tag)?;
    if entry.kind != 5 || entry.count == 0 {
        return None;
    }
    let offset = if little {
        u32::from_le_bytes(entry.field) as usize
    } else {
        u32::from_be_bytes(entry.field) as usize
    };
    if offset + 8 > data.len() {
        return None;
    }
    let numerator = read_u32(data, offset, little) as f64;
    let denominator = read_u32(data, offset + 4, little) as f64;
    (denominator > 0.0).then_some(numerator / denominator)
}

/// The Exif payload without the JPEG `Exif\0\0` prefix, when it has one.
fn exif_payload(data: &[u8]) -> Option<&[u8]> {
    Some(data.strip_prefix(b"Exif\0\0").unwrap_or(data)).filter(|body| !body.is_empty())
}

/// The first directory of an Exif payload, which begins with a TIFF header.
/// That is where a camera writes both its resolution and its orientation.
struct ExifDirectory<'a> {
    data: &'a [u8],
    little: bool,
    entries: Vec<Entry>,
}

impl ExifDirectory<'_> {
    fn number(&self, tag: u16) -> Option<f64> {
        entry_number(&self.entries, self.little, tag)
    }

    fn rational(&self, tag: u16) -> Option<f64> {
        entry_rational(&self.entries, self.data, self.little, tag)
    }

    /// The resolution the camera wrote, in dots per inch.
    fn dpi(&self) -> Option<(f64, f64)> {
        let x = self.rational(282)?;
        let y = self.rational(283).unwrap_or(x);
        let unit = self.number(296).unwrap_or(2.0);
        let scale = if unit == 3.0 { 2.54 } else { 1.0 };
        (x > 0.0 && y > 0.0).then_some((x * scale, y * scale))
    }

    /// How the camera stored the picture. A tag the payload does not carry, or
    /// a value outside 1–8, leaves the pixels as stored.
    fn orientation(&self) -> Orientation {
        self.number(274)
            .map(orientation_of)
            .unwrap_or(Orientation::Normal)
    }
}

/// The orientation a numeric EXIF tag value names. The value is a SHORT, so
/// anything that does not fit one byte is not an orientation at all — it must
/// not be truncated into one it looks like.
fn orientation_of(value: f64) -> Orientation {
    u8::try_from(value as u32)
        .ok()
        .map(Orientation::from_exif)
        .unwrap_or(Orientation::Normal)
}

fn exif_directory(exif: &[u8]) -> Option<ExifDirectory<'_>> {
    if exif.len() < 8 {
        return None;
    }
    let little = match &exif[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let ifd = read_u32(exif, 4, little) as usize;
    let entries = ifd_entries(exif, ifd, little).ok()?;
    Some(ExifDirectory {
        data: exif,
        little,
        entries,
    })
}

// --- BMP ------------------------------------------------------------------

fn bmp(data: &[u8]) -> Result<ImageDocument> {
    if data.len() < 26 {
        bail!("truncated BMP header");
    }
    let dib = read_u32(data, 14, true) as usize;
    let (width, height, dpi) = if dib == 12 {
        (
            read_u16(data, 18, true) as u32,
            read_u16(data, 20, true) as u32,
            None,
        )
    } else if dib >= 40 && data.len() >= 46 {
        // A negative height means the rows are stored top-down; the page is
        // as tall as the magnitude says either way.
        let width = read_i32(data, 18, true).unsigned_abs();
        let height = read_i32(data, 22, true).unsigned_abs();
        let x = read_i32(data, 38, true);
        let y = read_i32(data, 42, true);
        let dpi = if x > 0 && y > 0 {
            // PIL reads BMP resolution the same way, so the page matches what
            // Docling makes of the same file.
            Some((x as f64 / 39.3701, y as f64 / 39.3701))
        } else {
            None
        };
        (width, height, dpi)
    } else {
        bail!("a BMP with a {dib}-byte DIB header is not read");
    };
    Ok(ImageDocument {
        mimetype: "image/bmp",
        width,
        height,
        dpi,
        // The BMP header has no orientation field; pixels are read as stored.
        orientation: Orientation::Normal,
    })
}

// --- WebP -----------------------------------------------------------------

fn webp(data: &[u8]) -> Result<ImageDocument> {
    let mut width = None;
    let mut height = None;
    let mut orientation = Orientation::Normal;
    let mut pos = 12usize;
    while pos + 8 <= data.len() {
        let kind = &data[pos..pos + 4];
        let len =
            u32::from_le_bytes(data[pos + 4..pos + 8].try_into().expect("four bytes")) as usize;
        let body = data
            .get(pos + 8..pos + 8 + len)
            .context("truncated WebP chunk")?;
        match kind {
            // The extended header states the canvas, and does so for every
            // encoding: the frame headers below are the fallback.
            b"VP8X" if body.len() >= 10 => {
                width = Some(read_u24_le(body, 4) + 1);
                height = Some(read_u24_le(body, 7) + 1);
            }
            b"VP8 " if width.is_none() && body.len() >= 10 => {
                // A lossy frame: 3-byte frame tag, the 9D 01 2A start code,
                // then 14-bit width and height.
                if body[3..6] == [0x9D, 0x01, 0x2A] {
                    width = Some((u16::from_le_bytes([body[6], body[7]]) & 0x3FFF) as u32);
                    height = Some((u16::from_le_bytes([body[8], body[9]]) & 0x3FFF) as u32);
                }
            }
            b"VP8L" if width.is_none() && body.len() >= 5 && body[0] == 0x2F => {
                // A lossless frame: 14 bits of width then 14 bits of height,
                // both stored as one less than the real size.
                let bits = u32::from_le_bytes([body[1], body[2], body[3], body[4]]);
                width = Some((bits & 0x3FFF) + 1);
                height = Some(((bits >> 14) & 0x3FFF) + 1);
            }
            // The EXIF chunk carries the same first directory as a JPEG's APP1.
            b"EXIF" => {
                orientation = exif_payload(body)
                    .and_then(exif_directory)
                    .map(|exif| exif.orientation())
                    .unwrap_or(Orientation::Normal);
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        pos += 8 + len + (len & 1);
    }
    let (Some(width), Some(height)) = (width, height) else {
        bail!("the WebP declares no canvas size");
    };
    Ok(ImageDocument {
        mimetype: "image/webp",
        width,
        height,
        // WebP has no resolution field of its own; EXIF can carry one, but
        // PIL (and so Docling's backend) does not read it for WebP either.
        dpi: None,
        orientation,
    })
}

// --- GIF ------------------------------------------------------------------

/// A GIF's page, from its logical screen descriptor.
///
/// A GIF opens with `GIF87a` or `GIF89a`, then a descriptor that states the
/// logical screen — the canvas every frame is placed on — as two little-endian
/// `u16` pixel counts. That is the page, and the frame is what is printed on
/// it: a GIF may hold an animation, several frames over the one canvas, so
/// [`gif_frames`] reads them all — one page each — while [`transcode_to_png`]
/// decodes the first, which is the picture a document carrying a GIF holds.
/// Pillow, and so Docling's own backend, reads a GIF as one still image:
/// `Image.open` composites the first frame onto that screen.
///
/// A GIF has no resolution field. The byte after the screen size is a pixel
/// aspect ratio, which says how a pixel is stretched, not how many fit in an
/// inch, so the page is one pixel per point — the default, as for a WebP.
fn gif(data: &[u8]) -> Result<ImageDocument> {
    // The signature and the seven-byte descriptor that follows it, before any
    // colour table or block: the grid is the first four of the seven.
    if data.len() < 10 {
        bail!("the GIF has no logical screen descriptor");
    }
    let width = u16::from_le_bytes([data[6], data[7]]) as u32;
    let height = u16::from_le_bytes([data[8], data[9]]) as u32;
    Ok(ImageDocument {
        mimetype: "image/gif",
        width,
        height,
        dpi: None,
        orientation: Orientation::Normal,
    })
}

// --- JPEG 2000 ------------------------------------------------------------

/// A JPEG 2000 picture's page, from the header its spelling states.
///
/// A JPEG 2000 has two spellings. A JP2 is a tree of boxes, and its `jp2h`
/// superbox states the grid and, when the writer states one, the capture
/// resolution. A bare codestream is markers, and its `SIZ` marker states the
/// grid. Which one this is says nothing about the pixels — the decoder reads
/// both — so the page is read from whichever header is there.
fn jpeg2000(data: &[u8]) -> Result<ImageDocument> {
    if data.starts_with(JPEG2000_CODESTREAM) {
        return codestream(data);
    }
    jp2(data)
}

/// A bare JPEG 2000 codestream's page, from its `SIZ` marker.
///
/// A codestream is a run of markers: `SOC` opens it, `SIZ` states the reference
/// grid, `COD` the coding style, `QCD` the quantisation, `SOT` and `SOD` the
/// tiles and their data. Only `SIZ` is the page: `Xsiz` and `Ysiz` are the grid's
/// far edge and `XOsiz`/`YOsiz` its origin, so the grid is `Xsiz - XOsiz` by
/// `Ysiz - YOsiz` samples — an origin is not extra width. The tile grid and the
/// per-component subsampling that follow do not move a sample on the page.
///
/// A codestream states no density. The `res` box that places a JP2 is a
/// container box, not a marker, so a bare codestream is one pixel per point —
/// the same 72 dpi default a WebP or a GIF gets — rather than a resolution it
/// never wrote. A codestream with no `SIZ` before its tile data, or one whose
/// grid is empty, is a broken file rather than a page guessed at.
fn codestream(data: &[u8]) -> Result<ImageDocument> {
    // `SOC` carries no length: the marker walk starts after it, at the `SIZ`
    // the signature already found.
    let mut pos = 2usize;
    while pos + 2 <= data.len() {
        if data[pos] != 0xFF {
            bail!("the JPEG 2000 codestream is not a marker stream at byte {pos}");
        }
        let marker = data[pos + 1];
        // A marker may be preceded by any number of `0xFF` fill bytes, which the
        // standard allows between segments.
        if marker == 0xFF {
            pos += 1;
            continue;
        }
        // `SOD` opens the code-blocks' data and `EOC` ends the codestream: no
        // `SIZ` is coming after either.
        if marker == 0x93 || marker == 0xD9 {
            break;
        }
        if pos + 4 > data.len() {
            bail!("truncated JPEG 2000 {marker:02X} marker");
        }
        let length = read_u16(data, pos + 2, false) as usize;
        if length < 2 {
            bail!("a JPEG 2000 marker shorter than its own length field");
        }
        let end = pos
            .checked_add(2 + length)
            .filter(|end| *end <= data.len())
            .with_context(|| format!("truncated JPEG 2000 {marker:02X} marker"))?;
        if marker == 0x51 {
            let (width, height) = siz(&data[pos + 4..end])?;
            return Ok(ImageDocument {
                mimetype: "image/jp2",
                width,
                height,
                // A codestream states its grid and no density.
                dpi: None,
                // A codestream states no EXIF orientation either.
                orientation: Orientation::Normal,
            });
        }
        pos = end;
    }
    bail!("the JPEG 2000 codestream states no SIZ marker")
}

/// The grid a `SIZ` marker states, as the page's pixel size.
///
/// The marker's body is `Rsiz`, then `Xsiz`, `Ysiz`, `XOsiz` and `YOsiz` — the
/// reference grid and its origin — followed by the tile grid and the components,
/// which the page does not need. The extent is what the page is: the far edge
/// less the origin.
fn siz(body: &[u8]) -> Result<(u32, u32)> {
    // Rsiz, Xsiz, Ysiz, XOsiz and YOsiz: eighteen bytes before a component can
    // be stated, and the marker is longer than that in any real codestream.
    if body.len() < 18 {
        bail!(
            "a JPEG 2000 SIZ marker states its grid in 18 bytes, found {}",
            body.len()
        );
    }
    let xsiz = read_u32(body, 2, false);
    let ysiz = read_u32(body, 6, false);
    let xosiz = read_u32(body, 10, false);
    let yosiz = read_u32(body, 14, false);
    if xsiz <= xosiz || ysiz <= yosiz {
        bail!("the JPEG 2000 SIZ marker states an empty grid");
    }
    Ok((xsiz - xosiz, ysiz - yosiz))
}

/// A JP2's page, from its image header box and its capture resolution box.
///
/// A JP2 is a tree of boxes: the file opens with the signature box, states what
/// it is in `ftyp`, and puts the picture's facts in a `jp2h` superbox whose
/// `ihdr` child holds the grid — height then width, both as they are stored. The
/// `jp2c` box that follows carries the codestream, which is the pixels and not
/// the page, so it is not read: a reader of the page only needs the header.
///
/// A resolution is declared in the same superbox, by a `res` superbox whose
/// `resc` child states how many samples the picture was captured at — a scan
/// stating 150 dpi is a 620 pixel wide page at 2.98 inches, not 8.6. Most
/// writers state none, and then the page is the 72 dpi default, the same
/// reading a WebP gets, which has no resolution field at all.
fn jp2(data: &[u8]) -> Result<ImageDocument> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let (kind, body, next) = jp2_box(data, pos)?;
        if kind == b"jp2h" {
            let (width, height) = jp2_image_header(body)?;
            return Ok(ImageDocument {
                mimetype: "image/jp2",
                width,
                height,
                dpi: jp2_capture_resolution(body),
                // JP2 states no EXIF orientation.
                orientation: Orientation::Normal,
            });
        }
        // The header box has to come before the codestream in a conforming
        // file; one that reverses them has no page to report.
        if kind == b"jp2c" {
            break;
        }
        pos = next;
    }
    bail!("the JP2 states no image header")
}

/// The grid in a `jp2h` superbox's `ihdr` child.
fn jp2_image_header(data: &[u8]) -> Result<(u32, u32)> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let (kind, body, next) = jp2_box(data, pos)?;
        if kind == b"ihdr" {
            // HEIGHT, WIDTH, then the component count and bit depths, which
            // the page does not need.
            if body.len() < 8 {
                bail!("a JP2 image header is 14 bytes, found {}", body.len());
            }
            return Ok((read_u32(body, 4, false), read_u32(body, 0, false)));
        }
        pos = next;
    }
    bail!("the JP2 header states no image size")
}

/// The resolution a `jp2h` superbox declares, in dots per inch.
///
/// A `res` superbox holds a `resc` box of ten bytes: the vertical numerator and
/// denominator, the horizontal pair, and one signed base-ten exponent for each
/// axis. A resolution is `numerator / denominator × 10^exponent` samples per
/// metre — the unit the standard counts in — so an inch of it is 0.0254 of that
/// number, and a 150 dpi scan is stated as 5906/1 metres.
///
/// `None` when no box declares one, and also when the box that does cannot be
/// believed: a zero numerator or denominator, a truncated box, or a value that
/// is not a positive finite number is no resolution, which leaves the page at
/// the default rather than at a made-up one. The header's grid is still read —
/// a broken resolution box is not a broken page.
fn jp2_capture_resolution(data: &[u8]) -> Option<(f64, f64)> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let (kind, body, next) = jp2_box(data, pos).ok()?;
        if kind != b"res " {
            pos = next;
            continue;
        }
        let mut at = 0usize;
        while at + 8 <= body.len() {
            let (kind, resc, next) = jp2_box(body, at).ok()?;
            if kind == b"resc" {
                // VRcN, VRcD, HRcN, HRcD, VRcE, HRcE.
                if resc.len() < 10 {
                    return None;
                }
                let axis = |numerator_at: usize, exponent_at: usize| -> Option<f64> {
                    let numerator = f64::from(read_u16(resc, numerator_at, false));
                    let denominator = f64::from(read_u16(resc, numerator_at + 2, false));
                    let exponent = i32::from(resc[exponent_at] as i8);
                    let per_metre = numerator / denominator * 10f64.powi(exponent);
                    let dpi = per_metre * 0.0254;
                    (dpi.is_finite() && dpi > 0.0).then_some(dpi)
                };
                return Some((axis(4, 9)?, axis(0, 8)?));
            }
            at = next;
        }
        return None;
    }
    None
}

/// The box at `pos`: its four-character type, its body, and where the next box
/// begins.
///
/// `LBox` is the box's own length, header included. 0 means the box runs to the
/// end of the file, and 1 means the real length is a 64-bit `XLBox` after the
/// type — both spellings the standard allows for a box that does not fit four
/// bytes. A box that claims more bytes than the file holds is an error, not a
/// shorter box.
fn jp2_box(data: &[u8], pos: usize) -> Result<(&[u8], &[u8], usize)> {
    let l_box = read_u32(data, pos, false);
    let (length, header) = match l_box {
        0 => (data.len() - pos, 8),
        1 => {
            let xl = data.get(pos + 8..pos + 16).context("truncated JP2 XLBox")?;
            let length = u64::from_be_bytes(xl.try_into().expect("eight bytes"));
            (
                usize::try_from(length).context("a JP2 box larger than memory")?,
                16,
            )
        }
        length => (length as usize, 8),
    };
    let kind = &data[pos + 4..pos + 8];
    if length < header {
        bail!("a JP2 box shorter than its own header");
    }
    let next = pos
        .checked_add(length)
        .filter(|end| *end <= data.len())
        .with_context(|| format!("truncated JP2 {} box", jp2_tag(kind)))?;
    Ok((kind, &data[pos + header..next], next))
}

/// A box's four-character type, for a message: `jp2h`, `jp2c`.
fn jp2_tag(kind: &[u8]) -> String {
    String::from_utf8_lossy(kind).escape_debug().to_string()
}

// --- little helpers -------------------------------------------------------

fn read_u16(data: &[u8], at: usize, little: bool) -> u16 {
    let bytes = [data[at], data[at + 1]];
    if little {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    }
}

fn read_u32(data: &[u8], at: usize, little: bool) -> u32 {
    let bytes = [data[at], data[at + 1], data[at + 2], data[at + 3]];
    if little {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    }
}

fn read_i32(data: &[u8], at: usize, little: bool) -> i32 {
    let bytes = [data[at], data[at + 1], data[at + 2], data[at + 3]];
    if little {
        i32::from_le_bytes(bytes)
    } else {
        i32::from_be_bytes(bytes)
    }
}

fn read_u24_le(data: &[u8], at: usize) -> u32 {
    data[at] as u32 | (data[at + 1] as u32) << 8 | (data[at + 2] as u32) << 16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG with an IHDR and, when `dpi` is given, a pHYs chunk before it.
    fn png(width: u32, height: u32, dpi: Option<u32>) -> Vec<u8> {
        png_chunks(width, height, dpi, None)
    }

    /// The same, optionally with an eXIf chunk carrying this Exif payload.
    fn png_chunks(width: u32, height: u32, dpi: Option<u32>, exif: Option<&[u8]>) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut chunk = |kind: &[u8; 4], body: &[u8]| {
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            out.extend_from_slice(&[0, 0, 0, 0]);
        };
        let mut header = width.to_be_bytes().to_vec();
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(b"IHDR", &header);
        if let Some(dpi) = dpi {
            let ppm = (dpi as f64 / 0.0254).round() as u32;
            let mut phys = ppm.to_be_bytes().to_vec();
            phys.extend_from_slice(&ppm.to_be_bytes());
            phys.push(1);
            chunk(b"pHYs", &phys);
        }
        if let Some(exif) = exif {
            chunk(b"eXIf", exif);
        }
        chunk(b"IEND", &[]);
        out
    }

    /// A little-endian Exif payload: one directory holding the orientation tag.
    fn exif(orientation: u16) -> Vec<u8> {
        let mut out = b"II*\0".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&274u16.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&orientation.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&0u32.to_le_bytes()); // no next directory
        out
    }

    #[test]
    fn a_png_page_is_its_pixels_at_its_declared_resolution() {
        let image = ImageDocument::parse_bytes(&png(600, 300, Some(150))).expect("a PNG");
        assert_eq!(image.mimetype, "image/png");
        assert_eq!((image.width, image.height), (600, 300));
        assert!(
            (image.width_pt() - 288.0).abs() < 0.1,
            "{}",
            image.width_pt()
        );
        assert!((image.height_pt() - 144.0).abs() < 0.1);
    }

    #[test]
    fn an_image_with_no_declared_resolution_is_one_pixel_per_point() {
        let image = ImageDocument::parse_bytes(&png(120, 80, None)).expect("a PNG");
        assert_eq!(image.dpi, None);
        assert_eq!((image.width_pt(), image.height_pt()), (120.0, 80.0));
    }

    /// A JPEG with a JFIF APP0 segment and a baseline frame header.
    fn jpeg(width: u16, height: u16, unit: u8, density: u16) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        let mut app0 = b"JFIF\0".to_vec();
        app0.extend_from_slice(&[1, 1, unit]);
        app0.extend_from_slice(&density.to_be_bytes());
        app0.extend_from_slice(&density.to_be_bytes());
        app0.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&[0xFF, 0xE0]);
        out.extend_from_slice(&((app0.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app0);
        frame(&mut out, width, height);
        out
    }

    /// A JPEG that carries this Exif payload in an APP1 segment.
    fn jpeg_exif(width: u16, height: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(payload);
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        frame(&mut out, width, height);
        out
    }

    /// SOF0 (precision, height, width, one component) and end of image.
    fn frame(out: &mut Vec<u8>, width: u16, height: u16) {
        out.extend_from_slice(&[0xFF, 0xC0, 0, 11, 8]);
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&[1, 1, 0x11, 0]);
        out.extend_from_slice(&[0xFF, 0xD9]);
    }

    #[test]
    fn a_jpeg_reads_its_frame_and_jfif_density() {
        let image = ImageDocument::parse_bytes(&jpeg(400, 200, 1, 300)).expect("a JPEG");
        assert_eq!(image.mimetype, "image/jpeg");
        assert_eq!((image.width, image.height), (400, 200));
        assert_eq!(image.dpi, Some((300.0, 300.0)));
        // Unit 2 is dots per centimetre, which the page must convert.
        let image = ImageDocument::parse_bytes(&jpeg(400, 200, 2, 100)).expect("a JPEG");
        assert_eq!(image.dpi, Some((254.0, 254.0)));
        // Unit 0 is only an aspect ratio, not a resolution.
        let image = ImageDocument::parse_bytes(&jpeg(400, 200, 0, 500)).expect("a JPEG");
        assert_eq!(image.dpi, None);
    }

    /// A little-endian TIFF: one IFD with width, height, resolution, unit and
    /// orientation.
    fn tiff(width: u32, height: u32, dpi: u32, unit: u16, orientation: u16) -> Vec<u8> {
        let mut out = b"II*\0".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        let tags = [
            (256u16, 4u16, 1u32, width),
            (257, 4, 1, height),
            (274, 3, 1, orientation as u32),
            (282, 5, 1, 0),
            (283, 5, 1, 0),
            (296, 3, 1, unit as u32),
        ];
        out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
        let rationals_at = 8 + 2 + tags.len() * 12 + 4;
        for (tag, kind, count, value) in tags {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            if kind == 5 {
                out.extend_from_slice(&(rationals_at as u32).to_le_bytes());
            } else if kind == 3 {
                out.extend_from_slice(&(value as u16).to_le_bytes());
                out.extend_from_slice(&[0, 0]);
            } else {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        for _ in 0..2 {
            out.extend_from_slice(&dpi.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
        }
        out
    }

    #[test]
    fn a_tiff_reads_its_first_directory() {
        let image = ImageDocument::parse_bytes(&tiff(800, 600, 200, 2, 1)).expect("a TIFF");
        assert_eq!(image.mimetype, "image/tiff");
        assert_eq!((image.width, image.height), (800, 600));
        assert_eq!(image.dpi, Some((200.0, 200.0)));
        assert_eq!(image.orientation, Orientation::Normal);
        // Unit 3 is dots per centimetre.
        let image = ImageDocument::parse_bytes(&tiff(800, 600, 100, 3, 1)).expect("a TIFF");
        assert_eq!(image.dpi, Some((254.0, 254.0)));
    }

    #[test]
    fn every_exif_orientation_is_read_from_a_jpeg() {
        for (value, orientation) in [
            (1, Orientation::Normal),
            (2, Orientation::FlipHorizontal),
            (3, Orientation::Rotate180),
            (4, Orientation::FlipVertical),
            (5, Orientation::Transpose),
            (6, Orientation::Rotate90),
            (7, Orientation::Transverse),
            (8, Orientation::Rotate270),
        ] {
            let image = ImageDocument::parse_bytes(&jpeg_exif(200, 400, &exif(value)))
                .expect("a JPEG with Exif");
            assert_eq!(image.orientation, orientation, "EXIF {value}");
            assert_eq!(u16::from(image.orientation.exif()), value);
            assert_eq!((image.width, image.height), (200, 400));
            // A quarter turn is the only kind that puts the page on its side.
            let upright = if image.orientation.swaps_axes() {
                (400, 200)
            } else {
                (200, 400)
            };
            assert_eq!(image.upright_size(), upright, "EXIF {value}");
        }
    }

    #[test]
    fn a_quarter_turn_page_is_measured_upright() {
        // A phone held sideways stores a portrait page as a 400 x 200 grid and
        // tags it 6: turn it 90° clockwise to read it. The page is 200 x 400.
        let image = ImageDocument::parse_bytes(&jpeg_exif(400, 200, &exif(6))).expect("a JPEG");
        assert_eq!(image.upright_size(), (200, 400));
        assert_eq!((image.width_pt(), image.height_pt()), (200.0, 400.0));
        assert!(image.orientation.swaps_axes());
        assert!(!Orientation::Rotate180.swaps_axes());
    }

    #[test]
    fn an_orientation_tag_outside_one_to_eight_is_read_as_stored() {
        // 262 would truncate to 6, a real quarter turn: a value too wide for
        // the tag must not become one by wrapping.
        for value in [0, 9, 255, 262] {
            let image =
                ImageDocument::parse_bytes(&jpeg_exif(200, 100, &exif(value))).expect("a JPEG");
            assert_eq!(image.orientation, Orientation::Normal, "EXIF {value}");
            assert_eq!(image.upright_size(), (200, 100));
        }
    }

    #[test]
    fn png_tiff_and_webp_carry_their_orientation_too() {
        // PNG's eXIf chunk holds the Exif payload without its JPEG prefix.
        let image = ImageDocument::parse_bytes(&png_chunks(400, 200, Some(150), Some(&exif(6))))
            .expect("a PNG");
        assert_eq!(image.orientation, Orientation::Rotate90);
        assert_eq!(image.upright_size(), (200, 400));
        assert!(
            (image.width_pt() - 96.0).abs() < 0.1,
            "{}",
            image.width_pt()
        );

        let image = ImageDocument::parse_bytes(&tiff(400, 200, 150, 2, 8)).expect("a TIFF");
        assert_eq!(image.orientation, Orientation::Rotate270);
        assert_eq!(image.upright_size(), (200, 400));

        let image =
            ImageDocument::parse_bytes(&webp_chunks(400, 200, Some(&exif(3)))).expect("a WebP");
        assert_eq!(image.orientation, Orientation::Rotate180);
        assert_eq!(image.upright_size(), (400, 200));
    }

    /// A TIFF batch where each directory states its own orientation. The
    /// first sheet is stored on its side and tagged 6, the second is stored
    /// upright and tagged 1, and the third is upside down and tagged 3: the
    /// first sheet's turn must not be applied to the others, and the second's
    /// lack of one must not leave the first on its side.
    #[test]
    fn each_tiff_directory_is_turned_by_its_own_orientation() {
        use tiff::encoder::colortype::RGB8;
        use tiff::tags::Tag;

        // 4 x 2, red on the left and blue on the right, in every directory.
        let red_blue = [
            255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255, // row 0
            255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255, // row 1
        ];
        let mut file = Cursor::new(Vec::new());
        {
            let mut encoder = tiff::encoder::TiffEncoder::new(&mut file).expect("a TIFF encoder");
            for orientation in [6u16, 1, 3] {
                let mut image = encoder
                    .new_image::<RGB8>(4, 2)
                    .expect("a directory per sheet");
                image
                    .encoder()
                    .write_tag(Tag::Orientation, orientation)
                    .expect("an orientation tag");
                image.write_data(&red_blue).expect("the sheet's pixels");
            }
        }
        let pages = tiff_frames(&file.into_inner());
        assert_eq!(pages.len(), 3, "one page for every directory");
        let rgb = |page: &PngPicture, x: u32, y: u32| {
            image::load_from_memory(&page.data)
                .expect("the page decodes")
                .to_rgb8()
                .get_pixel(x, y)
                .0
        };
        let red = [255, 0, 0];
        let blue = [0, 0, 255];

        // Tag 6 on a stored 4 x 2 sheet is a 2 x 4 page, the red left half
        // turned to the top and the blue right half to the bottom.
        assert_eq!((pages[0].width, pages[0].height), (2, 4));
        assert_eq!(rgb(&pages[0], 0, 0), red);
        assert_eq!(rgb(&pages[0], 1, 0), red);
        assert_eq!(rgb(&pages[0], 0, 3), blue);
        assert_eq!(rgb(&pages[0], 1, 3), blue);

        // The second sheet is stored upright and says so: the first sheet's
        // quarter turn must not reach it.
        assert_eq!((pages[1].width, pages[1].height), (4, 2));
        assert_eq!(rgb(&pages[1], 0, 0), red);
        assert_eq!(rgb(&pages[1], 3, 0), blue);

        // The third is upside down: its own tag turns it, without a quarter
        // turn, so its grid stays 4 x 2 but its colours swap sides.
        assert_eq!((pages[2].width, pages[2].height), (4, 2));
        assert_eq!(rgb(&pages[2], 0, 0), blue);
        assert_eq!(rgb(&pages[2], 3, 0), red);
    }

    /// A BMP with a 40-byte BITMAPINFOHEADER.
    fn bmp(width: i32, height: i32, ppm: i32) -> Vec<u8> {
        let mut out = b"BM".to_vec();
        out.extend_from_slice(&[0; 12]);
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&[1, 0, 24, 0]); // planes, bits per pixel
        out.extend_from_slice(&0u32.to_le_bytes()); // compression
        out.extend_from_slice(&0u32.to_le_bytes()); // image size
        out.extend_from_slice(&ppm.to_le_bytes());
        out.extend_from_slice(&ppm.to_le_bytes());
        out.extend_from_slice(&[0; 8]); // palette counts
        out
    }

    #[test]
    fn a_bmp_reads_its_dib_header_and_metre_resolution() {
        let image = ImageDocument::parse_bytes(&bmp(120, -80, 5906)).expect("a BMP");
        assert_eq!(image.mimetype, "image/bmp");
        // A negative height is top-down storage, not a negative page.
        assert_eq!((image.width, image.height), (120, 80));
        let (x, _) = image.dpi.expect("a declared resolution");
        assert!((x - 150.0).abs() < 0.1, "{x}");
    }

    /// A WebP with a VP8X canvas.
    fn webp(width: u32, height: u32) -> Vec<u8> {
        webp_chunks(width, height, None)
    }

    /// The same, optionally with an EXIF chunk.
    fn webp_chunks(width: u32, height: u32, exif: Option<&[u8]>) -> Vec<u8> {
        let mut body = vec![0u8; 10];
        let put = |body: &mut Vec<u8>, at: usize, value: u32| {
            body[at] = (value & 0xFF) as u8;
            body[at + 1] = ((value >> 8) & 0xFF) as u8;
            body[at + 2] = ((value >> 16) & 0xFF) as u8;
        };
        put(&mut body, 4, width - 1);
        put(&mut body, 7, height - 1);
        let mut chunks = Vec::new();
        let mut chunk = |kind: &[u8; 4], body: &[u8]| {
            chunks.extend_from_slice(kind);
            chunks.extend_from_slice(&(body.len() as u32).to_le_bytes());
            chunks.extend_from_slice(body);
            if body.len() % 2 == 1 {
                chunks.push(0);
            }
        };
        chunk(b"VP8X", &body);
        if let Some(exif) = exif {
            chunk(b"EXIF", exif);
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(4u32 + chunks.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(&chunks);
        out
    }

    #[test]
    fn a_webp_reads_its_canvas_size() {
        let image = ImageDocument::parse_bytes(&webp(320, 200)).expect("a WebP");
        assert_eq!(image.mimetype, "image/webp");
        assert_eq!((image.width, image.height), (320, 200));
        assert_eq!(image.dpi, None);
        assert_eq!((image.width_pt(), image.height_pt()), (320.0, 200.0));
    }

    /// A GIF's header: the signature, the logical screen descriptor, and the
    /// trailer. Nothing else is needed to state the page.
    fn gif_header(width: u16, height: u16) -> Vec<u8> {
        let mut out = b"GIF89a".to_vec();
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        // Packed fields (no global colour table), background index, pixel
        // aspect ratio, then the file's trailer.
        out.extend_from_slice(&[0x00, 0, 0, b';']);
        out
    }

    #[test]
    fn a_gif_reads_its_logical_screen() {
        let image = ImageDocument::parse_bytes(&gif_header(320, 200)).expect("a GIF");
        assert_eq!(image.mimetype, "image/gif");
        assert_eq!((image.width, image.height), (320, 200));
        // A GIF states no resolution: the byte after the screen is a pixel
        // aspect ratio, which is not one.
        assert_eq!(image.dpi, None);
        assert_eq!((image.width_pt(), image.height_pt()), (320.0, 200.0));
        // The original encoding names the same format as the animated one.
        let mut original = b"GIF87a".to_vec();
        original.extend_from_slice(&gif_header(0, 0)[6..]);
        assert_eq!(mimetype(&original), Some("image/gif"));
    }

    /// An animated GIF is one page: the first frame, on the logical screen. The
    /// second frame is a different colour, so a decoder that returned the last
    /// frame — or laid the two over each other — would show it.
    #[test]
    fn an_animated_gif_page_is_its_first_frame() {
        fn frame(colour: [u8; 4]) -> image::Frame {
            image::Frame::from_parts(
                image::RgbaImage::from_pixel(8, 6, image::Rgba(colour)),
                0,
                0,
                image::Delay::from_numer_denom_ms(100, 1),
            )
        }
        let mut animated = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut animated);
            encoder
                .encode_frames([frame([255, 0, 0, 255]), frame([0, 0, 255, 255])])
                .expect("encode the animation");
        }

        let page = ImageDocument::parse_bytes(&animated).expect("an animated GIF");
        assert_eq!(page.mimetype, "image/gif");
        assert_eq!((page.width, page.height), (8, 6));

        // The pixels are the first frame's, not the animation's.
        let png = transcode_to_png(&animated).expect("the first frame decodes");
        assert_eq!((png.width, png.height), (8, 6));
        let pixels = image::load_from_memory(&png.data)
            .expect("the transcoded PNG decodes")
            .to_rgba8();
        assert_eq!(pixels.get_pixel(0, 0), &image::Rgba([255, 0, 0, 255]));
        assert_eq!(pixels.get_pixel(7, 5), &image::Rgba([255, 0, 0, 255]));
    }

    /// An animation is not one page. The frame reader returns every frame of a
    /// GIF in the order the file plays them, each on the logical screen and
    /// laid on white — the pixels a page holds — so a document built from the
    /// file has one page per frame instead of stopping at the first.
    #[test]
    fn every_frame_of_a_gif_animation_is_a_page() {
        fn frame(colour: [u8; 4]) -> image::Frame {
            image::Frame::from_parts(
                image::RgbaImage::from_pixel(8, 6, image::Rgba(colour)),
                0,
                0,
                image::Delay::from_numer_denom_ms(100, 1),
            )
        }
        let mut animated = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut animated);
            encoder
                .encode_frames([
                    frame([255, 0, 0, 255]),
                    frame([0, 255, 0, 255]),
                    frame([0, 0, 255, 255]),
                ])
                .expect("encode the animation");
        }

        let frames = gif_frames(&animated);
        assert_eq!(frames.len(), 3, "one page per frame");
        for (page, colour) in
            frames
                .iter()
                .zip([[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]])
        {
            // Every frame is drawn on the logical screen, so every page is it.
            assert_eq!((page.width, page.height), (8, 6));
            let pixels = image::load_from_memory(&page.data)
                .expect("the frame's PNG decodes")
                .to_rgba8();
            assert_eq!(pixels.get_pixel(0, 0), &image::Rgba(colour));
            assert_eq!(pixels.get_pixel(7, 5), &image::Rgba(colour));
        }

        // A GIF with a single frame is one page, and bytes that are not a GIF
        // are not read for frames at all: the caller keeps its own page.
        let mut single = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut single);
            encoder
                .encode_frames([frame([255, 0, 0, 255])])
                .expect("encode the still");
        }
        assert_eq!(gif_frames(&single).len(), 1);
        assert!(gif_frames(&gif_header(8, 6)).is_empty());
        assert!(gif_frames(b"not a picture at all").is_empty());
    }

    /// A JP2: the signature box, a file-type box, the header superbox with its
    /// `ihdr`, then an (empty) codestream box. A capture resolution box is
    /// added when the `resc` body to write is given.
    fn jp2_with_resc_body(width: u32, height: u32, resc_body: Option<&[u8]>) -> Vec<u8> {
        let mut out = JP2_SIGNATURE.to_vec();
        let mut boxed = |kind: &[u8; 4], body: &[u8]| {
            out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
        };
        boxed(b"ftyp", b"jp2 \0\0\0\0jp2 ");
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&3u16.to_be_bytes());
        ihdr.extend_from_slice(&[7, 0, 0, 0]);
        let mut header = Vec::new();
        header.extend_from_slice(&((ihdr.len() + 8) as u32).to_be_bytes());
        header.extend_from_slice(b"ihdr");
        header.extend_from_slice(&ihdr);
        if let Some(resc_body) = resc_body {
            let mut res = Vec::new();
            res.extend_from_slice(&((resc_body.len() + 8) as u32).to_be_bytes());
            res.extend_from_slice(b"resc");
            res.extend_from_slice(resc_body);
            header.extend_from_slice(&((res.len() + 8) as u32).to_be_bytes());
            header.extend_from_slice(b"res ");
            header.extend_from_slice(&res);
        }
        boxed(b"jp2h", &header);
        boxed(b"jp2c", &[]);
        out
    }

    fn jp2(width: u32, height: u32) -> Vec<u8> {
        jp2_with_resc_body(width, height, None)
    }

    fn jp2_with(width: u32, height: u32, resc_body: [u8; 10]) -> Vec<u8> {
        jp2_with_resc_body(width, height, Some(&resc_body))
    }

    /// The ten bytes of a `resc` box: both axes stated as `numerator` /
    /// `denominator` samples per metre, at the same base-ten `exponent`.
    fn resc(numerator: u16, denominator: u16, exponent: i8) -> [u8; 10] {
        let mut body = Vec::new();
        for _ in 0..2 {
            body.extend_from_slice(&numerator.to_be_bytes());
            body.extend_from_slice(&denominator.to_be_bytes());
        }
        body.push(exponent as u8);
        body.push(exponent as u8);
        body.try_into().expect("ten bytes")
    }

    #[test]
    fn a_jp2_reads_its_image_header() {
        let image = ImageDocument::parse_bytes(&jp2(620, 350)).expect("a JP2");
        assert_eq!(image.mimetype, "image/jp2");
        assert_eq!((image.width, image.height), (620, 350));
        // No capture resolution box and no EXIF: one pixel per point.
        assert_eq!(image.dpi, None);
        assert_eq!(image.orientation, Orientation::Normal);
        assert_eq!((image.width_pt(), image.height_pt()), (620.0, 350.0));
    }

    /// A scan states how finely it was captured. The box's numbers are samples
    /// per metre, so the page is the grid at that resolution — 5906 samples per
    /// metre is 150 dpi, and the 620 pixel wide scan is 297.6 points, not 620.
    #[test]
    fn a_jp2_capture_resolution_is_the_pages_resolution() {
        let image =
            ImageDocument::parse_bytes(&jp2_with(620, 350, resc(5906, 1, 0))).expect("a JP2");
        let (x, y) = image.dpi.expect("a declared resolution");
        assert!((x - 150.0).abs() < 0.1, "{x}");
        assert!((y - 150.0).abs() < 0.1, "{y}");
        assert!(
            (image.width_pt() - 297.575).abs() < 0.1,
            "{}",
            image.width_pt()
        );
        assert!(
            (image.height_pt() - 167.986).abs() < 0.1,
            "{}",
            image.height_pt()
        );

        // The exponent is a signed power of ten: 59060 metres^-1 at 10^-1 is the
        // same 150 dpi scan, and a negative exponent is not a huge one.
        let image =
            ImageDocument::parse_bytes(&jp2_with(620, 350, resc(59060, 1, -1))).expect("a JP2");
        let (x, _) = image.dpi.expect("a declared resolution");
        assert!((x - 150.0).abs() < 0.1, "{x}");
    }

    /// A resolution box that cannot be believed is no declaration, not a
    /// broken page: the grid is still read and the page is one pixel per point.
    #[test]
    fn an_unbelievable_capture_resolution_is_no_resolution() {
        // A denominator of zero states no number at all.
        let image = ImageDocument::parse_bytes(&jp2_with(40, 24, resc(300, 0, 0))).expect("a JP2");
        assert_eq!((image.width, image.height), (40, 24));
        assert_eq!(image.dpi, None);
        assert_eq!(image.width_pt(), 40.0);

        // A `resc` box shorter than its ten bytes states no resolution either.
        let short = jp2_with_resc_body(40, 24, Some(&[0, 1, 0, 1]));
        let image = ImageDocument::parse_bytes(&short).expect("the grid is still a page");
        assert_eq!((image.width, image.height), (40, 24));
        assert_eq!(image.dpi, None);
    }

    #[test]
    fn a_jp2_without_a_usable_header_is_refused() {
        // The signature names the format; the missing header is the error.
        let complete = jp2(40, 24);
        let truncated = &complete[..20];
        assert_eq!(mimetype(truncated), Some("image/jp2"));
        assert!(ImageDocument::parse_bytes(truncated).is_err());

        // A header superbox whose only child is not an `ihdr` states no size,
        // and one whose child runs past the end of the file is broken rather
        // than a page.
        let header_at = JP2_SIGNATURE.len() + 20;
        let mut no_ihdr = complete.clone();
        let ihdr_kind = header_at + 8 + 4;
        no_ihdr[ihdr_kind..ihdr_kind + 4].copy_from_slice(b"colr");
        assert!(ImageDocument::parse_bytes(&no_ihdr).is_err());
        // Cut inside the `ihdr` body: the box claims 38 bytes of header the file
        // does not hold.
        assert!(ImageDocument::parse_bytes(&complete[..50]).is_err());

        // A bare codestream is named as JPEG 2000 too, and a codestream whose
        // `SIZ` never arrives is a broken file rather than a guessed page.
        assert_eq!(mimetype(&[0xFF, 0x4F, 0xFF, 0x51]), Some("image/jp2"));
        assert!(ImageDocument::parse_bytes(&[0xFF, 0x4F, 0xFF, 0x51]).is_err());
    }

    /// A bare codestream: `SOC`, a `SIZ` marker stating `width × height` at an
    /// origin, then `EOC`. The tile grid and the one component are stated the
    /// way an encoder writes them, though the page is read from neither.
    fn codestream(width: u32, height: u32, origin: (u32, u32)) -> Vec<u8> {
        let (x0, y0) = origin;
        let mut siz = Vec::new();
        siz.extend_from_slice(&0u16.to_be_bytes()); // Rsiz: no profile
        siz.extend_from_slice(&(width + x0).to_be_bytes()); // Xsiz
        siz.extend_from_slice(&(height + y0).to_be_bytes()); // Ysiz
        siz.extend_from_slice(&x0.to_be_bytes()); // XOsiz
        siz.extend_from_slice(&y0.to_be_bytes()); // YOsiz
        siz.extend_from_slice(&(width + x0).to_be_bytes()); // XTsiz
        siz.extend_from_slice(&(height + y0).to_be_bytes()); // YTsiz
        siz.extend_from_slice(&0u32.to_be_bytes()); // XTOsiz
        siz.extend_from_slice(&0u32.to_be_bytes()); // YTOsiz
        siz.extend_from_slice(&1u16.to_be_bytes()); // Csiz
        siz.extend_from_slice(&[7, 1, 1]); // Ssiz, XRsiz, YRsiz

        let mut out = JPEG2000_CODESTREAM.to_vec(); // SOC, then the SIZ marker
        out.extend_from_slice(&((siz.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&siz);
        out.extend_from_slice(&[0xFF, 0xD9]); // EOC
        out
    }

    /// A bare codestream is a JPEG 2000 without the box tree around it, and its
    /// `SIZ` marker is the page: the reference grid's extent, which an origin
    /// moves without widening. It states no density — the `res` box that places
    /// a JP2 is a container box — so the page is one pixel per point, the same
    /// default a WebP gets.
    #[test]
    fn a_codestream_reads_its_siz_grid() {
        let image =
            ImageDocument::parse_bytes(&codestream(620, 350, (0, 0))).expect("a bare codestream");
        assert_eq!(image.mimetype, "image/jp2");
        assert_eq!((image.width, image.height), (620, 350));
        assert_eq!(image.dpi, None);
        assert_eq!(image.orientation, Orientation::Normal);
        assert_eq!((image.width_pt(), image.height_pt()), (620.0, 350.0));

        // The far edge less the origin: a grid starting at 20,10 is still 620 x
        // 350 samples, not 640 x 360.
        let image =
            ImageDocument::parse_bytes(&codestream(620, 350, (20, 10))).expect("a bare codestream");
        assert_eq!((image.width, image.height), (620, 350));
        assert_eq!((image.width_pt(), image.height_pt()), (620.0, 350.0));
    }

    /// A codestream states no page until `SIZ` arrives, and a `SIZ` that states
    /// no extent states no page either: both are broken files rather than pages
    /// guessed at.
    #[test]
    fn a_codestream_without_a_usable_siz_is_refused() {
        // The signature names the format; the marker's body stops short.
        let complete = codestream(40, 24, (0, 0));
        assert_eq!(mimetype(&complete[..10]), Some("image/jp2"));
        assert!(ImageDocument::parse_bytes(&complete[..10]).is_err());

        // An empty `SIZ` body, and a grid whose origin reaches its far edge.
        assert!(
            ImageDocument::parse_bytes(&[0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x02, 0xFF, 0xD9]).is_err()
        );
        assert!(ImageDocument::parse_bytes(&codestream(0, 0, (0, 0))).is_err());

        // `SOC` and `EOC` with no marker between them is no picture at all.
        assert!(ImageDocument::parse_bytes(&[0xFF, 0x4F, 0xFF, 0xD9]).is_err());
    }

    #[test]
    fn a_jp2_box_may_state_its_length_in_eight_bytes() {
        // The same picture, with the header superbox written as `LBox = 1` and
        // a 64-bit `XLBox`: both spellings describe the same grid.
        let narrow = jp2(400, 200);
        let mut wide = narrow.clone();
        let header_at = JP2_SIGNATURE.len() + 20;
        let inner = &narrow[header_at + 8..header_at + 38];
        wide.truncate(header_at);
        wide.extend_from_slice(&1u32.to_be_bytes());
        wide.extend_from_slice(b"jp2h");
        wide.extend_from_slice(&((inner.len() + 16) as u64).to_be_bytes());
        wide.extend_from_slice(inner);
        wide.extend_from_slice(&8u32.to_be_bytes());
        wide.extend_from_slice(b"jp2c");

        let image = ImageDocument::parse_bytes(&wide).expect("a JP2 with an XLBox");
        assert_eq!((image.width, image.height), (400, 200));
    }

    #[test]
    fn a_truncated_or_unknown_image_is_refused() {
        // The signature names the format; the missing header is the error.
        let broken = &png(10, 10, None)[..20];
        assert_eq!(mimetype(broken), Some("image/png"));
        assert!(ImageDocument::parse_bytes(broken).is_err());
        assert!(ImageDocument::parse_bytes(b"not an image at all").is_err());
        assert_eq!(mimetype(b"not an image at all"), None);
        // A GIF's signature is enough to name it; the descriptor still has to
        // be there.
        assert_eq!(mimetype(b"GIF89a"), Some("image/gif"));
        assert!(ImageDocument::parse_bytes(b"GIF89a").is_err());
    }
}
