//! Rasterising of Windows metafiles (EMF and WMF) to a PNG.
//!
//! A DOCX picture is not always a bitmap. Word stores a vector drawing as an
//! Enhanced Metafile (`image/emf`) or a Windows Metafile (`image/wmf`), and it
//! stores the *preview* of an embedded object that way too — a MathType
//! equation or a pasted drawing is a `v:imagedata` pointing at a metafile.
//! Neither media type is one Docling's model can name in an image reference, so
//! before this crate existed such a picture could only be reported as a box with
//! no image, and the drawing was invisible to everything downstream.
//!
//! So the records are played here: both formats are a long list of GDI calls
//! against a device context, and this crate keeps that context (pen, brush,
//! font, clipping, the window/viewport transform) and draws the calls into a
//! pixel buffer, which is encoded as PNG — a media type Docling accepts.
//!
//! What is drawn: lines, polylines and polygons (8- and 16-bit, with both fill
//! rules), the GDI shapes (rectangle, round rectangle, ellipse, arc, chord,
//! pie), paths (`BeginPath` … `FillPath`), clipping rectangles, the world
//! transform, bitmaps (a `StretchDIBits`/`DIBSTRETCHBLT` source is decoded from
//! its DIB), the common raster operations, the stock objects, and text.
//!
//! Two limits are stated rather than hidden. **Text is drawn in a built-in 5×7
//! font**, because a metafile names a font (`Liberation Serif`, `Tahoma`) and
//! fetches its outlines from the system, which a document extractor has no
//! business doing: glyph *shapes* are therefore an approximation while their
//! position, size and colour are the file's. And a record this crate does not
//! understand is not silently skipped — it is named in [`Raster::unsupported`].
//!
//! The corpus contains one more real-world wrinkle worth knowing about. The WMFs
//! written by Word and CorelDRAW put a pad word after each record's size field,
//! so a standard walk of the file reads garbage. [`wmf`] recognises both
//! layouts by walking the record chain: the one that ends exactly at the end of
//! the file, on the end-of-file record, is the one the file uses.

// Public because a DIB also arrives from outside a metafile: an RTF
// `\wbitmap`/`\dibitmap` payload is the same `BITMAPINFO` with no file header,
// and wrapping one needs the header's layout. Parsing it there as well would let
// the wrapper and the renderer disagree about where a palette or a colour mask
// ends — see the module's own notes.
pub mod dib;
mod emf;
mod font;
mod paint;
mod wmf;

pub use paint::{pt, Bitmap, Canvas, Clip, Color, FillRule, Point, Rect, Rop, Transform};

// The built-in font is also what a VML shape's text is drawn in: VML is another
// vector drawing a Word document can carry (`w:pict`), drawn by the DOCX reader
// into the same kind of canvas, and it faces the same problem — a shape names a
// font family and the host is asked for outlines it may not have. Exposing it
// keeps one substitute rather than two.
pub use font::{draw_text, text_width, Horizontal, TextAlign, Vertical};

use std::fmt;

/// Resolution a metafile is rasterised at.
///
/// 144 is what Docling itself renders these drawings at, so a picture's pixel
/// grid comes out the same size on both sides; it is also two device pixels per
/// point, which keeps a 10-point glyph legible.
pub const DEFAULT_DPI: f64 = 144.0;

/// Largest raster we will allocate, in pixels on either side.
const MAX_SIDE: u32 = 8192;
/// Largest raster we will allocate, in pixels altogether (64 MiB of RGBA).
const MAX_AREA: u64 = 16 * 1024 * 1024;

/// How a metafile should be rasterised.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Output resolution.
    pub dpi: f64,
    /// The size the document asks the drawing to be drawn at, in points. When
    /// the document states one it wins: that is the size a reader sees, and the
    /// metafile's own frame is only what the drawing was authored against.
    pub size_pt: Option<(f64, f64)>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dpi: DEFAULT_DPI,
            size_pt: None,
        }
    }
}

impl Options {
    pub fn at_size(mut self, width_pt: f64, height_pt: f64) -> Self {
        if width_pt > 0.0 && height_pt > 0.0 {
            self.size_pt = Some((width_pt, height_pt));
        }
        self
    }
}

/// A rasterised drawing: RGBA8, row-major from the top left, opaque.
///
/// The background is white, not transparent, which is both what the formats
/// assume (a metafile paints onto a surface) and what Docling's own rendering
/// of these files produces.
#[derive(Debug, Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// Records that could not be drawn as the file writes them, named once
    /// each: an operation this crate does not implement, or one it can only
    /// approximate. An empty list means the drawing was played in full.
    pub unsupported: Vec<String>,
}

impl Raster {
    /// Encode as PNG.
    pub fn to_png(&self) -> Result<Vec<u8>, Error> {
        let image = image::RgbaImage::from_raw(self.width, self.height, self.pixels.clone())
            .ok_or(Error::Encode)?;
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|_| Error::Encode)?;
        Ok(png)
    }

    /// Pixels that are not the white background.
    ///
    /// A drawing that came out blank is not a picture of anything, so this is
    /// what the callers use to tell "drew nothing" from "drew something".
    pub fn inked_pixels(&self) -> usize {
        self.pixels
            .chunks_exact(4)
            .filter(|px| px[0] != 255 || px[1] != 255 || px[2] != 255)
            .count()
    }
}

/// Why a metafile could not be rasterised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bytes are neither an EMF nor a WMF.
    NotAMetafile,
    /// The file claims to be a metafile but its records do not parse.
    Malformed(&'static str),
    /// The format is understood but has nothing we can draw (or draws only with
    /// records this crate does not implement), so no picture may be claimed.
    Undrawable(&'static str),
    /// The raster could not be encoded.
    Encode,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotAMetafile => write!(f, "not an EMF or WMF"),
            Error::Malformed(what) => write!(f, "malformed metafile: {what}"),
            Error::Undrawable(why) => write!(f, "nothing to draw: {why}"),
            Error::Encode => write!(f, "cannot encode the raster"),
        }
    }
}

impl std::error::Error for Error {}

/// Rasterise an EMF or WMF drawing.
pub fn rasterize(data: &[u8], options: Options) -> Result<Raster, Error> {
    if emf::looks_like(data) {
        emf::play(data, options)
    } else if wmf::looks_like(data) {
        wmf::play(data, options)
    } else {
        Err(Error::NotAMetafile)
    }
}

/// Whether these bytes are a metafile this crate can rasterise.
pub fn is_metafile(data: &[u8]) -> bool {
    emf::looks_like(data) || wmf::looks_like(data)
}

/// The pixel grid a drawing is rasterised into.
///
/// `native_pt` is the size the drawing says it is, `options.size_pt` the size
/// the document asks for; the document wins when it says anything, and either
/// way the aspect ratio is the source's. A degenerate request (a zero-sized
/// frame) is refused rather than turned into a 1×1 picture.
pub(crate) fn canvas_size(
    options: &Options,
    native_pt: Option<(f64, f64)>,
) -> Result<(u32, u32), Error> {
    let (width_pt, height_pt) = options
        .size_pt
        .filter(|(w, h)| *w > 0.0 && *h > 0.0)
        .or(native_pt)
        .ok_or(Error::Undrawable("no size is declared and none was given"))?;
    if !(width_pt.is_finite() && height_pt.is_finite()) || width_pt <= 0.0 || height_pt <= 0.0 {
        return Err(Error::Undrawable("the drawing has no extent"));
    }
    let dpi = if options.dpi.is_finite() && options.dpi > 0.0 {
        options.dpi
    } else {
        DEFAULT_DPI
    };
    let scale = dpi / 72.0;
    let width = (width_pt * scale).round().max(1.0);
    let height = (height_pt * scale).round().max(1.0);
    if width > MAX_SIDE as f64 || height > MAX_SIDE as f64 || width * height > MAX_AREA as f64 {
        return Err(Error::Undrawable("the drawing is too large to rasterise"));
    }
    Ok((width as u32, height as u32))
}
