//! An SVG drawing in an office package, drawn into pixels.
//!
//! Word and PowerPoint store a vector picture as an SVG part: a whole graphics
//! language — paths, fills, strokes, gradients, clipping, text — rather than an
//! encoding of a pixel grid. Naming such a part in a document model leaves the
//! drawing as markup, and every consumer downstream then has to draw it itself
//! (or show nothing); so an office package's SVG is played here, exactly as a
//! metafile is played by `pdfextractor_metafile`, and travels on as a PNG.
//!
//! Two decisions belong to the document rather than to the drawing. The size is
//! the size the package asks the picture to be *shown* at, at [`DEFAULT_DPI`]
//! (144 — the resolution Docling itself renders office drawings at); where the
//! package states no box, the drawing's own viewport is the only size there is.
//! The background is the drawing's own: an SVG is transparent unless it paints
//! a background, and inventing a white page would put a box around a picture
//! placed on a coloured slide.
//!
//! The shape stays the drawing's too. A frame whose proportions differ from the
//! drawing's — an author who dragged a picture's corner, a package that places a
//! 40 × 24 drawing in a square — is fitted by the drawing's own
//! `preserveAspectRatio`, exactly as `<svg>` fits a `viewBox` into its viewport.
//! By default that is `xMidYMid meet`: one scale on both axes, centred, with the
//! rest of the frame transparent margin. `none` stretches the two axes
//! independently, `slice` fills the frame and is clipped by it, and any of the
//! nine alignments places the drawing in the margin it leaves.
//!
//! Text is drawn in the host's fonts. An SVG names families and glyph runs, not
//! outlines, so a drawing with words can only be rendered against some font
//! database; the host's is loaded lazily, and only for a drawing that mentions
//! text at all, so a drawing of shapes never touches the font configuration.

use std::sync::{Arc, OnceLock};

use resvg::tiny_skia;
use resvg::usvg;
use svgtypes::{Align, AspectRatio};
use usvg::fontdb;

/// Resolution a drawing is rasterised at.
///
/// 144 is the resolution `pdfextractor_metafile` plays an EMF or WMF at, which
/// is what Docling renders an office drawing at, so a picture's pixel grid is
/// the same size whichever kind of drawing it came from.
pub const DEFAULT_DPI: f64 = 144.0;

/// Largest raster we will allocate, in pixels on either side.
const MAX_SIDE: u32 = 8192;
/// Largest raster we will allocate, in pixels altogether (64 MiB of RGBA).
const MAX_AREA: u64 = 16 * 1024 * 1024;

/// How an SVG should be rasterised.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Output resolution. A drawing placed in a document is measured in points,
    /// so this is what turns the placement into a pixel grid.
    pub dpi: f64,
    /// The size the document asks the drawing to be drawn at, in points. When
    /// the document states one it wins: that is the size a reader sees, and the
    /// drawing's own viewport is only what it was authored against.
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

/// A rasterised drawing: RGBA8, row-major from the top left, **not**
/// premultiplied.
///
/// A pixel the drawing never painted has an alpha of zero, which is what the
/// drawing itself says — an SVG has no background unless it draws one.
#[derive(Debug, Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
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

    /// Pixels the drawing actually painted.
    ///
    /// A drawing that came out blank is not a picture of anything, so this is
    /// what a caller uses to tell "drew nothing" from "drew something".
    pub fn inked_pixels(&self) -> usize {
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count()
    }
}

/// Why a drawing could not be rasterised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not SVG markup: no `<svg>` element is in them.
    NotAnSvg,
    /// The markup parses as a document but not as a drawing. Also where a
    /// document that mentions text fails when no font can be found for it.
    Malformed(&'static str),
    /// The drawing is understood but has nothing to draw at the size asked for.
    Undrawable(&'static str),
    /// The raster could not be encoded.
    Encode,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotAnSvg => write!(f, "not SVG markup"),
            Error::Malformed(what) => write!(f, "malformed SVG: {what}"),
            Error::Undrawable(why) => write!(f, "nothing to draw: {why}"),
            Error::Encode => write!(f, "cannot encode the raster"),
        }
    }
}

impl std::error::Error for Error {}

/// Rasterise an SVG drawing.
pub fn rasterize(data: &[u8], options: Options) -> Result<Raster, Error> {
    let markup = std::str::from_utf8(data).map_err(|_| Error::NotAnSvg)?;
    if !mentions_svg(markup) {
        return Err(Error::NotAnSvg);
    }
    let tree = usvg::Tree::from_str(markup, &parse_options(mentions_text(markup)))
        .map_err(|_| Error::Malformed("the markup does not parse"))?;
    let native = tree.size();
    let (width, height) = canvas_size(&options, (native.width(), native.height()))?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        // `canvas_size` has already refused anything this large.
        .ok_or(Error::Undrawable("the drawing is too large to rasterise"))?;
    // The transform fits the drawing's own coordinate system into the pixel
    // grid, so a drawing authored at 40 × 24 and shown at twice the points
    // comes out twice as many pixels across without being redrawn. How it fits
    // is the drawing's statement: the root's `preserveAspectRatio` decides the
    // two scales, and which edge or corner of the slack they are pinned to.
    let transform = frame_transform(
        (native.width(), native.height()),
        (width, height),
        root_preserve_aspect_ratio(markup),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(Raster {
        width,
        height,
        // tiny-skia paints premultiplied; a caller wants the colours the
        // drawing states, not colours darkened by their own alpha.
        pixels: pixmap.take_demultiplied(),
    })
}

/// The transform that maps a drawing's own viewport into the pixel grid.
///
/// This is the same fit `<svg>` applies when it puts its `viewBox` into its
/// viewport: `none` takes the two scales independently, `slice` takes the
/// larger of them and is clipped by the frame, and every other alignment takes
/// the smaller and leaves the rest as margin on the sides its name pins it away
/// from. With no `viewBox` to fit, the drawing's own size is the viewport the
/// frame stands in for, which is why the fit is still meaningful.
fn frame_transform(
    native: (f32, f32),
    frame: (u32, u32),
    aspect: AspectRatio,
) -> tiny_skia::Transform {
    let scale_x = frame.0 as f32 / native.0;
    let scale_y = frame.1 as f32 / native.1;
    let (scale_x, scale_y) = match aspect.align {
        Align::None => (scale_x, scale_y),
        _ => {
            let scale = if aspect.slice {
                scale_x.max(scale_y)
            } else {
                scale_x.min(scale_y)
            };
            (scale, scale)
        }
    };
    // What the two scales leave over; negative under `slice`, where the frame
    // crops instead of padding. Aligning by that surplus is what puts the
    // drawing at the frame's named edge, or centred in it.
    let slack_x = frame.0 as f32 - native.0 * scale_x;
    let slack_y = frame.1 as f32 - native.1 * scale_y;
    let (inset_x, inset_y) = match aspect.align {
        Align::None | Align::XMinYMin => (0.0, 0.0),
        Align::XMidYMin => (slack_x / 2.0, 0.0),
        Align::XMaxYMin => (slack_x, 0.0),
        Align::XMinYMid => (0.0, slack_y / 2.0),
        Align::XMidYMid => (slack_x / 2.0, slack_y / 2.0),
        Align::XMaxYMid => (slack_x, slack_y / 2.0),
        Align::XMinYMax => (0.0, slack_y),
        Align::XMidYMax => (slack_x / 2.0, slack_y),
        Align::XMaxYMax => (slack_x, slack_y),
    };
    tiny_skia::Transform::from_row(scale_x, 0.0, 0.0, scale_y, inset_x, inset_y)
}

/// The `preserveAspectRatio` the root `<svg>` element states.
///
/// A missing or invalid value is the format's default, `xMidYMid meet`, which
/// is also what a reader that did not know the attribute would have done. Only
/// the root element's statement governs the drawing's placement in the frame;
/// nested elements state their own fit for their own content.
fn root_preserve_aspect_ratio(markup: &str) -> AspectRatio {
    root_svg_attributes(markup)
        .and_then(|attributes| attribute(attributes, "preserveAspectRatio"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_default()
}

/// The attributes of the document's root `<svg>` element.
///
/// XML declarations, comments and a doctype may stand ahead of the root; they
/// are skipped rather than searched for `<svg`, so a comment mentioning one is
/// not mistaken for the element.
fn root_svg_attributes(markup: &str) -> Option<&str> {
    let mut rest = markup.trim_start_matches('\u{feff}');
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = &after[after.find("-->")? + 3..];
        } else if let Some(after) = rest.strip_prefix("<?") {
            rest = &after[after.find("?>")? + 2..];
        } else if let Some(after) = rest.strip_prefix("<!") {
            rest = &after[after.find('>')? + 1..];
        } else {
            break;
        }
    }
    let after_open = rest.strip_prefix('<')?;
    let name_end = after_open
        .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
        .unwrap_or(after_open.len());
    if &after_open[..name_end] != "svg" {
        return None;
    }
    let tag_end = end_of_tag(rest)?;
    Some(&rest[1 + name_end..tag_end])
}

/// Where a start tag ends, ignoring any `>` inside a quoted value.
///
/// Returns the index of the closing `>`, so the tag's attributes are the text
/// between its name and it.
fn end_of_tag(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (at, c) in tag.char_indices().skip(1) {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(at),
            (None, _) => {}
        }
    }
    None
}

/// One quoted attribute's value from an element's attribute text.
fn attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    let bytes = attributes.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b'/') {
            at += 1;
        }
        let name_start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'=' {
            at += 1;
        }
        let attribute_name = &attributes[name_start..at];
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() || bytes[at] != b'=' {
            continue;
        }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let Some(&quote) = bytes
            .get(at)
            .filter(|quote| **quote == b'"' || **quote == b'\'')
        else {
            // An unquoted value is not XML; its token is skipped rather than
            // read as a value.
            while at < bytes.len() && !bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            continue;
        };
        at += 1;
        let value_start = at;
        while at < bytes.len() && bytes[at] != quote {
            at += 1;
        }
        let value = &attributes[value_start..at];
        at += 1;
        if attribute_name == name {
            return Some(value);
        }
    }
    None
}

/// Whether these bytes are SVG markup at all.
///
/// The check is the root element rather than a parse: what is not a drawing is
/// refused as "not an SVG" here, while something that claims to be one and does
/// not parse is a malformed drawing, and it is worth telling the two apart.
fn mentions_svg(markup: &str) -> bool {
    markup.contains("<svg")
}

/// Whether the drawing has words in it, which decides whether the host's fonts
/// are loaded at all.
fn mentions_text(markup: &str) -> bool {
    markup.contains("<text") || markup.contains("<tspan") || markup.contains("<textPath")
}

/// The font database, loaded once per process.
///
/// An SVG names families and lets the renderer find glyphs, so there is no way
/// to draw text without a database, and no drawing carries one: the host's
/// fonts are the only glyphs there are. A drawing made of shapes never loads
/// them ([`mentions_text`]).
fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut database = fontdb::Database::new();
            database.load_system_fonts();
            Arc::new(database)
        })
        .clone()
}

/// How usvg should read the markup.
///
/// The DPI is left at the format's own 96: the drawing's viewport is in CSS
/// pixels, and the size the document asks for is applied by the transform, not
/// by re-reading the drawing at a different resolution.
fn parse_options(load_fonts: bool) -> usvg::Options<'static> {
    let mut options = usvg::Options::default();
    if load_fonts {
        options.fontdb = fonts();
        // A drawing names a family, not glyphs, and an office document names
        // the families the author had: a host without Calibri would otherwise
        // lose the words of every drawing made on a Windows machine. The
        // substitute is the one a browser makes — draw what the named family
        // would have drawn in the first font there is.
        let named = usvg::FontResolver::default_font_selector();
        options.font_resolver = usvg::FontResolver {
            select_font: Box::new(move |font, database| {
                named(font, database).or_else(|| database.faces().next().map(|face| face.id))
            }),
            ..usvg::FontResolver::default()
        };
    }
    options
}

/// The pixel grid a drawing is rasterised into.
///
/// `native` is the drawing's own viewport in CSS pixels, `options.size_pt` the
/// size the document asks for; the document wins when it states one, and either
/// way the aspect ratio is the drawing's. A drawing with no extent is refused
/// rather than turned into a 1 × 1 picture.
fn canvas_size(options: &Options, native: (f32, f32)) -> Result<(u32, u32), Error> {
    if !(native.0.is_finite() && native.1.is_finite()) || native.0 <= 0.0 || native.1 <= 0.0 {
        return Err(Error::Undrawable("the drawing has no extent"));
    }
    let dpi = if options.dpi.is_finite() && options.dpi > 0.0 {
        options.dpi
    } else {
        DEFAULT_DPI
    };
    let (width, height) = match options
        .size_pt
        .filter(|(width, height)| *width > 0.0 && *height > 0.0)
    {
        Some((width_pt, height_pt)) => {
            if !(width_pt.is_finite() && height_pt.is_finite()) {
                return Err(Error::Undrawable("the drawing has no extent"));
            }
            let scale = dpi / 72.0;
            (width_pt * scale, height_pt * scale)
        }
        None => (native.0 as f64, native.1 as f64),
    };
    let width = width.round().max(1.0);
    let height = height.round().max(1.0);
    if width > MAX_SIDE as f64 || height > MAX_SIDE as f64 || width * height > MAX_AREA as f64 {
        return Err(Error::Undrawable("the drawing is too large to rasterise"));
    }
    Ok((width as u32, height as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drawing like the ones an office package carries: a pale panel with a
    /// blue line on it, authored at 40 × 24.
    const DIAGRAM: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24" viewBox="0 0 40 24"><rect width="40" height="24" rx="3" fill="#e8eef7"/><polyline points="4,19 14,11 22,15 36,5" fill="none" stroke="#2f6db5" stroke-width="2"/></svg>"##;

    fn pixel(raster: &Raster, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * raster.width + x) * 4) as usize;
        [
            raster.pixels[at],
            raster.pixels[at + 1],
            raster.pixels[at + 2],
            raster.pixels[at + 3],
        ]
    }

    /// The size the document asks for decides the pixel grid, at the same 144
    /// dpi a metafile is played at: 40 × 24 points is 80 × 48 pixels.
    #[test]
    fn the_placement_decides_the_pixel_grid() {
        let raster = rasterize(DIAGRAM.as_bytes(), Options::default().at_size(40.0, 24.0))
            .expect("the drawing rasterises");
        assert_eq!((raster.width, raster.height), (80, 48));
        // The panel's pale fill is inside the picture. Its corner is the one
        // place it is not: the panel is rounded.
        assert_eq!(pixel(&raster, 40, 24), [0xe8, 0xee, 0xf7, 0xff]);

        let bigger = rasterize(DIAGRAM.as_bytes(), Options::default().at_size(120.0, 72.0))
            .expect("the drawing rasterises");
        assert_eq!((bigger.width, bigger.height), (240, 144));
    }

    /// The default fit — no `preserveAspectRatio` stated — is `xMidYMid meet`:
    /// a frame whose proportions differ from the drawing's is filled by the
    /// drawing at its own shape, not by a squashed copy of it. Both axes take
    /// the same scale, the drawing is centred, and the rest of the canvas stays
    /// transparent. A 40 × 24 drawing in a 40 × 40 frame is 80 × 48 inside the
    /// 80 × 80 canvas, with 16 clear pixels above and below it.
    #[test]
    fn a_frame_of_another_shape_letterboxes_the_drawing() {
        let tall = rasterize(DIAGRAM.as_bytes(), Options::default().at_size(40.0, 40.0))
            .expect("the drawing rasterises");
        assert_eq!((tall.width, tall.height), (80, 80), "the frame's own grid");
        assert_eq!(pixel(&tall, 40, 8)[3], 0, "the top margin was painted");
        assert_eq!(pixel(&tall, 40, 70)[3], 0, "the bottom margin was painted");
        assert_eq!(
            pixel(&tall, 40, 40),
            [0xe8, 0xee, 0xf7, 0xff],
            "the panel is not centred in its frame"
        );
        // The panel runs from the top of the drawing to its bottom, so its own
        // height is the 48 pixels the uniform scale gives it — a stretched copy
        // would reach y = 16 and y = 64 as well, but also x = 80, which the
        // narrow frame shows it does not.
        let wide = rasterize(DIAGRAM.as_bytes(), Options::default().at_size(80.0, 24.0))
            .expect("the drawing rasterises");
        assert_eq!((wide.width, wide.height), (160, 48), "the frame's own grid");
        assert_eq!(pixel(&wide, 8, 24)[3], 0, "the left margin was painted");
        assert_eq!(pixel(&wide, 152, 24)[3], 0, "the right margin was painted");
        assert_eq!(
            pixel(&wide, 80, 24),
            [0xe8, 0xee, 0xf7, 0xff],
            "the panel is not centred in its frame"
        );
    }

    /// `preserveAspectRatio="none"` is the drawing asking for the frame to be
    /// filled: the two axes take their own scales and the drawing is stretched
    /// to the frame's proportions. In the same 80 × 24 frame that letterboxes by
    /// default, the 40 × 24 drawing now spans all 160 pixels across.
    #[test]
    fn preserve_aspect_ratio_none_stretches_the_drawing_into_the_frame() {
        let stretched = DIAGRAM.replace("<svg ", r##"<svg preserveAspectRatio="none" "##);
        let raster = rasterize(stretched.as_bytes(), Options::default().at_size(80.0, 24.0))
            .expect("the drawing rasterises");
        assert_eq!((raster.width, raster.height), (160, 48));
        assert_eq!(
            pixel(&raster, 8, 24),
            [0xe8, 0xee, 0xf7, 0xff],
            "the left margin was left clear instead of stretched into"
        );
        assert_eq!(
            pixel(&raster, 152, 24),
            [0xe8, 0xee, 0xf7, 0xff],
            "the right margin was left clear instead of stretched into"
        );
    }

    /// `preserveAspectRatio="... slice"` is the drawing asking to cover the
    /// frame: the larger scale is used, and the overflow is clipped by the
    /// canvas. The 40 × 24 drawing in a square frame now reaches every edge,
    /// where the default `meet` had left 16 clear pixels above and below.
    #[test]
    fn preserve_aspect_ratio_slice_fills_and_clips_the_drawing() {
        let sliced = DIAGRAM.replace("<svg ", r##"<svg preserveAspectRatio="xMidYMid slice" "##);
        let raster = rasterize(sliced.as_bytes(), Options::default().at_size(40.0, 40.0))
            .expect("the drawing rasterises");
        assert_eq!((raster.width, raster.height), (80, 80));
        for (x, y) in [(40, 0), (0, 40), (79, 40), (40, 79)] {
            assert_eq!(
                pixel(&raster, x, y),
                [0xe8, 0xee, 0xf7, 0xff],
                "({x}, {y}) was clear: the drawing did not cover the frame"
            );
        }
    }

    /// An alignment other than the centre pins the drawing to the edge its
    /// name gives. Under `xMaxYMax meet` the same 40 × 24 drawing in an 80 × 24
    /// frame sits at the right instead of the middle, so the 80 clear pixels
    /// are all on its left.
    #[test]
    fn preserve_aspect_ratio_aligns_a_drawing_the_frame_is_wider_than() {
        let cornered = DIAGRAM.replace("<svg ", r##"<svg preserveAspectRatio="xMaxYMax meet" "##);
        let raster = rasterize(cornered.as_bytes(), Options::default().at_size(80.0, 24.0))
            .expect("the drawing rasterises");
        assert_eq!((raster.width, raster.height), (160, 48));
        assert_eq!(pixel(&raster, 8, 24)[3], 0, "the left margin was painted");
        assert_eq!(
            pixel(&raster, 152, 24),
            [0xe8, 0xee, 0xf7, 0xff],
            "the drawing was not pinned to the right"
        );
    }

    /// `slice` and an alignment act together: the larger scale crops the
    /// drawing, and the alignment chooses which part the frame keeps. Under
    /// `xMaxYMax slice` a square frame around a 40 × 24 drawing shows its right
    /// edge — the blue half here — while centring would have started at the red.
    #[test]
    fn preserve_aspect_ratio_slice_crops_at_the_stated_corner() {
        let halves = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="24" viewBox="0 0 40 24" preserveAspectRatio="xMaxYMax slice"><rect width="10" height="24" fill="#f00"/><rect x="10" width="30" height="24" fill="#00f"/></svg>"##;
        let raster = rasterize(halves, Options::default().at_size(40.0, 40.0))
            .expect("the drawing rasterises");
        assert_eq!((raster.width, raster.height), (80, 80));
        assert_eq!(
            pixel(&raster, 0, 40),
            [0x00, 0x00, 0xff, 0xff],
            "the frame kept the red (left) edge instead of the blue (right) one"
        );
        assert_eq!(pixel(&raster, 79, 40), [0x00, 0x00, 0xff, 0xff]);
    }

    /// The root tag's statement is what is read: an XML declaration or a
    /// comment ahead of it is not the root, a value in either kind of quote is
    /// still that value, and an absent, malformed or unquoted one is the
    /// default. A nested element's `preserveAspectRatio` governs its own
    /// content and is not the drawing's placement.
    #[test]
    fn the_root_preserve_aspect_ratio_is_read_and_its_absence_is_the_default() {
        let aspect = |markup: &str| root_preserve_aspect_ratio(markup);
        let default = AspectRatio::default();
        assert_eq!(aspect(r##"<svg width="1" height="1"/>"##), default);
        assert_eq!(
            aspect(r##"<svg preserveAspectRatio="none"/><svg>"##).align,
            Align::None
        );
        assert_eq!(
            aspect("<svg preserveAspectRatio='xMinYMax slice'/>").align,
            Align::XMinYMax
        );
        assert!(aspect("<svg preserveAspectRatio='xMinYMax slice'/>").slice);
        assert_eq!(
            aspect(r##"<?xml version="1.0"?><!-- <svg preserveAspectRatio="none"/> --><svg preserveAspectRatio="xMaxYMin meet"/>"##)
                .align,
            Align::XMaxYMin
        );
        assert_eq!(
            aspect(r##"<svg><g preserveAspectRatio="none"/></svg>"##),
            default,
            "a nested element's fit is the drawing's placement"
        );
        assert_eq!(aspect(r##"<svg preserveAspectRatio="stretch"/>"##), default);
        assert_eq!(aspect(r##"<svg preserveAspectRatio=none/>"##), default);
        // An attribute whose value holds `>` does not end the root tag early.
        assert_eq!(
            aspect(r##"<svg data-note="a > b" preserveAspectRatio="none"/><svg>"##).align,
            Align::None
        );
    }

    /// With no box from the document, the drawing's own viewport is the size
    /// there is: 40 × 24 CSS pixels, one pixel each.
    #[test]
    fn a_drawing_with_no_placement_is_its_own_size() {
        let raster = rasterize(DIAGRAM.as_bytes(), Options::default()).expect("rasterises");
        assert_eq!((raster.width, raster.height), (40, 24));
    }

    /// A drawing gets the background it paints, and no other: transparent
    /// pixels stay transparent instead of becoming a white box.
    #[test]
    fn a_drawing_keeps_its_own_transparency() {
        let circle = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><circle cx="5" cy="5" r="4" fill="#c00"/></svg>"##;
        let raster = rasterize(circle, Options::default()).expect("rasterises");
        assert_eq!(pixel(&raster, 0, 0)[3], 0, "the corner was painted");
        assert_eq!(pixel(&raster, 5, 5), [0xcc, 0x00, 0x00, 0xff]);
        assert!(raster.inked_pixels() > 0);
    }

    /// The PNG that leaves this crate is the raster: same size, straight
    /// (not premultiplied) colour, and a decoder reads it back.
    #[test]
    fn the_raster_is_what_the_png_holds() {
        let raster = rasterize(DIAGRAM.as_bytes(), Options::default().at_size(40.0, 24.0))
            .expect("rasterises");
        let png = raster.to_png().expect("the raster encodes");
        let decoded = image::load_from_memory(&png)
            .expect("the PNG decodes")
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (80, 48));
        assert_eq!(decoded.get_pixel(40, 24).0, [0xe8, 0xee, 0xf7, 0xff]);
    }

    /// Bytes that are not a drawing are refused as such; markup that claims to
    /// be one and does not parse is refused as malformed. Neither is guessed at
    /// from a partial picture.
    #[test]
    fn refuses_what_is_not_a_drawing() {
        for not_markup in [
            b"not a drawing at all".as_slice(),
            b"".as_slice(),
            &[0xff, 0xfe, 0x00],
            b"<html><body/></html>".as_slice(),
        ] {
            assert_eq!(
                rasterize(not_markup, Options::default()).err(),
                Some(Error::NotAnSvg)
            );
        }
        assert!(matches!(
            rasterize(b"<svg", Options::default()),
            Err(Error::Malformed(_))
        ));
    }

    /// A drawing with no extent cannot be placed, and a size too large to hold
    /// is refused rather than allocated.
    #[test]
    fn refuses_a_drawing_with_no_extent_or_a_huge_one() {
        let no_extent = br##"<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0"/><svg>"##;
        assert!(
            rasterize(no_extent, Options::default()).is_err(),
            "a drawing with no extent was given one: {:?}",
            rasterize(no_extent, Options::default())
        );

        let too_large = rasterize(
            DIAGRAM.as_bytes(),
            Options::default().at_size(100_000.0, 100_000.0),
        );
        assert!(
            matches!(too_large, Err(Error::Undrawable(_))),
            "{too_large:?}"
        );
    }

    /// A drawing with words in it is drawn with the host's fonts, whatever
    /// family it names: a family the host does not have is substituted rather
    /// than dropping the words, as a browser substitutes one.
    #[test]
    fn a_drawing_with_words_is_drawn_with_the_hosts_fonts() {
        if fonts().is_empty() {
            eprintln!("no system fonts; skipping the text rendering test");
            return;
        }
        for family in ["sans-serif", "Arial", "Calibri"] {
            let labelled = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="20" viewBox="0 0 60 20"><rect width="60" height="20" fill="#fff"/><text x="4" y="14" font-family="{family}" font-size="12" fill="#000">Ink</text></svg>"##
            );
            let raster = rasterize(labelled.as_bytes(), Options::default().at_size(60.0, 20.0))
                .expect("the drawing rasterises");
            // The background is white over the whole canvas, so the words are
            // what adds ink beyond it: a drawing whose text was dropped would be
            // 120 × 40 pixels of white.
            let dark = raster
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|px| px[3] != 0 && px[0] < 128 && px[1] < 128 && px[2] < 128)
                .count();
            assert!(dark > 0, "{family} drew no glyph: {dark} dark pixels");
        }
    }
}
