//! Enhanced Metafile playback ([MS-EMF]).
//!
//! An EMF is a header followed by records, each `type`, `size`, then its
//! parameters, and the records are the GDI calls that produced the picture:
//! select a pen, move to a point, fill a polygon. Playing them means keeping the
//! device context they act on — the pen, brush and font in force, the mapping
//! from logical to device coordinates, the clipping rectangle and the stack of
//! saved contexts — and drawing each call onto the canvas.
//!
//! Two details of the format matter more than they look. Coordinates in the
//! records are *logical*, and the mapping to device space is the window/viewport
//! pair the file sets: a drawing whose extent says 2540 units across an image 40
//! units wide has to go through that ratio, or everything lands in one corner.
//! And the header's `rclFrame` is the drawing's physical size in hundredths of a
//! millimetre, which is how a picture scaled for the page rather than for the
//! screen gets the resolution it asks for.

use crate::dib;
use crate::font::{self, Horizontal, TextAlign, Vertical};
use crate::paint::{pt, Bitmap, Canvas, Clip, Color, FillRule, Point, Rect, Rop, Transform};
use crate::{canvas_size, Error, Options, Raster};
use std::collections::HashMap;

/// The EMF signature: the bytes `" EMF"` in the header's `dSignature` field.
const SIGNATURE: u32 = 0x464d_4520;

pub(crate) fn looks_like(data: &[u8]) -> bool {
    data.len() >= 88 && read_u32(data, 0) == Some(1) && read_u32(data, 40) == Some(SIGNATURE)
}

/// Play an EMF into a raster.
pub(crate) fn play(data: &[u8], options: Options) -> Result<Raster, Error> {
    let header = Header::read(data)?;
    let native = header.size_pt();
    let (width, height) = canvas_size(&options, native)?;
    let mut player = Player::new(data, &header, width, height);
    player.run()?;
    if !player.canvas.drew() {
        return Err(Error::Undrawable(
            "the record stream draws nothing (an EMF+ only file keeps its drawing in a comment record)",
        ));
    }
    Ok(Raster {
        width,
        height,
        pixels: player.canvas.into_pixels(),
        unsupported: player.unsupported,
    })
}

/// The fields of the header record this crate needs.
struct Header {
    /// Drawing bounds in device units, inclusive of the far edges.
    bounds: Rect,
    /// The physical size of the picture, in hundredths of a millimetre.
    frame: Rect,
}

impl Header {
    fn read(data: &[u8]) -> Result<Header, Error> {
        if data.len() < 88 {
            return Err(Error::Malformed("a header shorter than 88 bytes"));
        }
        let bounds = Rect::new(
            read_f64(data, 8).ok_or(Error::Malformed("bounds"))?,
            read_f64(data, 12).ok_or(Error::Malformed("bounds"))?,
            read_f64(data, 16).ok_or(Error::Malformed("bounds"))?,
            read_f64(data, 20).ok_or(Error::Malformed("bounds"))?,
        )
        .normalised();
        let frame = Rect::new(
            read_f64(data, 24).ok_or(Error::Malformed("frame"))?,
            read_f64(data, 28).ok_or(Error::Malformed("frame"))?,
            read_f64(data, 32).ok_or(Error::Malformed("frame"))?,
            read_f64(data, 36).ok_or(Error::Malformed("frame"))?,
        )
        .normalised();
        if bounds.is_empty() {
            return Err(Error::Undrawable("the drawing bounds are empty"));
        }
        Ok(Header { bounds, frame })
    }

    /// The picture's own size in points: hundredths of a millimetre to
    /// seventy-seconds of an inch.
    fn size_pt(&self) -> Option<(f64, f64)> {
        if self.frame.is_empty() {
            return None;
        }
        const HUNDREDTH_MM_TO_PT: f64 = 72.0 / 25.4 / 100.0;
        Some((
            self.frame.width() * HUNDREDTH_MM_TO_PT,
            self.frame.height() * HUNDREDTH_MM_TO_PT,
        ))
    }
}

/// A pen as `CreatePen` and `ExtCreatePen` declare one.
#[derive(Debug, Clone, Copy)]
struct Pen {
    style: u32,
    width: f64,
    color: Color,
}

/// `PS_NULL`: a pen that draws nothing.
const PS_NULL: u32 = 5;

/// A brush as `CreateBrushIndirect` declares one.
#[derive(Debug, Clone, Copy)]
struct Brush {
    style: u32,
    color: Color,
}

/// `BS_NULL`/`BS_HOLLOW`: a brush that fills nothing.
const BS_NULL: u32 = 1;

/// A font as `ExtCreateFontIndirectW` declares one.
#[derive(Debug, Clone)]
struct Font {
    /// Cell height in logical units. Negative means the character height.
    height: f64,
    /// Baseline angle in tenths of a degree.
    escapement: f64,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            height: -12.0,
            escapement: 0.0,
        }
    }
}

#[derive(Debug, Clone)]
enum Object {
    Pen(Pen),
    Brush(Brush),
    Font(Font),
}

/// The device context the records act on.
#[derive(Debug, Clone)]
struct Dc {
    window_org: (f64, f64),
    window_ext: (f64, f64),
    viewport_org: (f64, f64),
    viewport_ext: (f64, f64),
    world: Transform,
    fill_rule: FillRule,
    align: TextAlign,
    text_color: Color,
    bk_color: Color,
    bk_transparent: bool,
    pen: Pen,
    brush: Brush,
    font: Font,
    /// Current position, in logical units.
    current: Point,
}

impl Default for Dc {
    fn default() -> Self {
        Dc {
            window_org: (0.0, 0.0),
            window_ext: (1.0, 1.0),
            viewport_org: (0.0, 0.0),
            viewport_ext: (1.0, 1.0),
            world: Transform::IDENTITY,
            fill_rule: FillRule::Alternate,
            align: TextAlign::default(),
            text_color: Color::BLACK,
            bk_color: Color::WHITE,
            bk_transparent: false,
            // The device context an EMF starts with: a one-pixel black pen, a
            // white brush, and a black 12-point font.
            pen: Pen {
                style: 0,
                width: 1.0,
                color: Color::BLACK,
            },
            brush: Brush {
                style: 0,
                color: Color::WHITE,
            },
            font: Font::default(),
            current: pt(0.0, 0.0),
        }
    }
}

struct Player<'a> {
    data: &'a [u8],
    canvas: Canvas,
    dc: Dc,
    saved: Vec<(Dc, Clip)>,
    objects: HashMap<u32, Object>,
    /// The transform from *device* space to the canvas, fixed for the file: the
    /// header's bounds mapped onto the raster.
    device_to_canvas: Transform,
    /// Contours collected between `BeginPath` and `EndPath`, in logical units.
    open_path: Option<Vec<Vec<Point>>>,
    /// The path the last `EndPath` closed.
    path: Option<Vec<Vec<Point>>>,
    unsupported: Vec<String>,
    noted: Vec<String>,
}

impl<'a> Player<'a> {
    fn new(data: &'a [u8], header: &Header, width: u32, height: u32) -> Player<'a> {
        // The bounds are inclusive of their far edges, so the span is one wider
        // than the difference; ignoring that piles a sliver of the drawing onto
        // the last pixel row.
        let across = header.bounds.width() + 1.0;
        let down = header.bounds.height() + 1.0;
        let device_to_canvas = Transform::translate(-header.bounds.left, -header.bounds.top).then(
            &Transform::scale(width as f64 / across, height as f64 / down),
        );
        Player {
            data,
            canvas: Canvas::new(width, height),
            dc: Dc::default(),
            saved: Vec::new(),
            objects: HashMap::new(),
            device_to_canvas,
            open_path: None,
            path: None,
            unsupported: Vec::new(),
            noted: Vec::new(),
        }
    }

    fn note(&mut self, what: &str) {
        if !self.noted.iter().any(|seen| seen == what) {
            self.noted.push(what.to_string());
            self.unsupported.push(what.to_string());
        }
    }

    /// Logical coordinates to canvas pixels.
    fn to_canvas(&self) -> Transform {
        let (window_w, window_h) = (nonzero(self.dc.window_ext.0), nonzero(self.dc.window_ext.1));
        let (viewport_w, viewport_h) = (
            nonzero(self.dc.viewport_ext.0),
            nonzero(self.dc.viewport_ext.1),
        );
        let sx = viewport_w / window_w;
        let sy = viewport_h / window_h;
        let window_to_device = Transform::new(
            sx,
            0.0,
            0.0,
            sy,
            -self.dc.window_org.0 * sx + self.dc.viewport_org.0,
            -self.dc.window_org.1 * sy + self.dc.viewport_org.1,
        );
        window_to_device
            .then(&self.dc.world)
            .then(&self.device_to_canvas)
    }

    fn transform(&self, p: Point) -> Point {
        self.to_canvas().apply(p)
    }

    /// A rectangle in logical coordinates as the canvas sees it. A clip region
    /// is written in the drawing's own space, so it has to travel through the
    /// same transform as the geometry.
    fn transform_rect(&self, rect: Rect) -> Rect {
        let transform = self.to_canvas();
        let (corner, opposite) = (
            transform.apply(pt(rect.left, rect.top)),
            transform.apply(pt(rect.right, rect.bottom)),
        );
        Rect::new(corner.x, corner.y, opposite.x, opposite.y).normalised()
    }

    fn run(&mut self) -> Result<(), Error> {
        let header_size = read_u32(self.data, 4).ok_or(Error::Malformed("header size"))? as usize;
        if header_size < 8 || header_size > self.data.len() {
            return Err(Error::Malformed("a header size outside the file"));
        }
        let mut offset = header_size;
        while offset + 8 <= self.data.len() {
            let kind = read_u32(self.data, offset).unwrap_or(14);
            let size = read_u32(self.data, offset + 4).unwrap_or(0) as usize;
            if size < 8 || offset + size > self.data.len() {
                return Err(Error::Malformed("a record whose size runs past the file"));
            }
            let record = &self.data[offset..offset + size];
            self.record(kind, record, offset);
            if kind == 14 {
                return Ok(());
            }
            offset += size;
        }
        Ok(())
    }

    /// Play one record. `offset` is where it sits in the file, which the
    /// records that embed a bitmap address by offset.
    fn record(&mut self, kind: u32, record: &[u8], offset: usize) {
        match kind {
            1 | 14 => {}
            2 | 5 => self.poly_beziers(kind, record),
            3 | 4 | 6 => self.poly32(kind, record),
            7 | 8 => self.poly_poly32(kind, record),
            9 => self.set_window_ext(record),
            10 => self.set_window_org(record),
            11 => self.set_viewport_ext(record),
            12 => self.set_viewport_org(record),
            15 => self.set_pixel(record),
            17 => self.set_map_mode(record),
            18 => self.set_bk_mode(record),
            19 => self.set_fill_mode(record),
            20 => self.set_rop2(record),
            21 => {}
            22 => self.set_text_align(record),
            24 => self.dc.text_color = colorref(record, 8),
            25 => self.dc.bk_color = colorref(record, 8),
            27 => {
                if let (Some(x), Some(y)) = (read_f64(record, 8), read_f64(record, 12)) {
                    self.dc.current = pt(x, y);
                }
            }
            30 => self.intersect_clip(record),
            31 | 32 => self.scale_extent(kind, record),
            33 => self.saved.push((self.dc.clone(), self.canvas.clip())),
            34 => self.restore_dc(record),
            35 => self.set_world(record),
            36 => self.modify_world(record),
            37 => self.select_object(record),
            38 => self.create_pen(record),
            39 => self.create_brush(record),
            40 => {
                if let Some(handle) = read_u32(record, 8) {
                    self.objects.remove(&handle);
                }
            }
            41 => self.angle_arc(record),
            42..=44 => self.shape(kind, record),
            45..=47 => self.arc_shape(kind, record),
            48..=53 => {}
            54 => self.line_to(record),
            55 => self.note("EMR_EXTFLOODFILL"),
            56 | 92 => self.note("EMR_POLYDRAW"),
            57 => self.note("EMR_SETARCDIRECTION"),
            59 => self.open_path = Some(Vec::new()),
            60 => self.path = self.open_path.take(),
            61 => self.close_figure(),
            62..=64 => self.finish_path(kind),
            65 | 66 => {}
            67 => self.note("EMR_SELECTCLIPPATH"),
            68 => {
                self.open_path = None;
                self.path = None;
            }
            70 => {}
            75 => self.select_clip_region(record),
            76 => self.note("EMR_BITBLT"),
            77 => self.stretch_blt_record(record, offset),
            80 | 81 => self.stretch_dibits(record, offset),
            82 => self.create_font(record),
            83 | 84 => self.text_out(kind, record),
            85 | 88 => self.poly_beziers(kind, record),
            86 | 87 | 89 => self.poly16(kind, record),
            90 | 91 => self.poly_poly16(kind, record),
            93 | 94 => self.note("EMR_CREATEDIBPATTERNBRUSHPT"),
            95 => self.create_pen(record),
            96 | 97 => self.note("EMR_POLYTEXTOUT"),
            other => {
                let name = format!("unimplemented EMF record {other}");
                self.note(&name);
            }
        }
    }

    // --- device context ---------------------------------------------------

    fn set_window_ext(&mut self, record: &[u8]) {
        if let (Some(x), Some(y)) = (read_f64(record, 8), read_f64(record, 12)) {
            self.dc.window_ext = (x, y);
        }
    }

    fn set_window_org(&mut self, record: &[u8]) {
        if let (Some(x), Some(y)) = (read_f64(record, 8), read_f64(record, 12)) {
            self.dc.window_org = (x, y);
        }
    }

    fn set_viewport_ext(&mut self, record: &[u8]) {
        if let (Some(x), Some(y)) = (read_f64(record, 8), read_f64(record, 12)) {
            self.dc.viewport_ext = (x, y);
        }
    }

    fn set_viewport_org(&mut self, record: &[u8]) {
        if let (Some(x), Some(y)) = (read_f64(record, 8), read_f64(record, 12)) {
            self.dc.viewport_org = (x, y);
        }
    }

    /// `ScaleWindowExtent`/`ScaleViewportExtent`: the extent is multiplied by
    /// one ratio and divided by another.
    fn scale_extent(&mut self, kind: u32, record: &[u8]) {
        let numbers = [
            read_f64(record, 8),
            read_f64(record, 12),
            read_f64(record, 16),
            read_f64(record, 20),
        ];
        let (Some(x_num), Some(x_den), Some(y_num), Some(y_den)) =
            (numbers[0], numbers[1], numbers[2], numbers[3])
        else {
            return;
        };
        let extent = if kind == 31 {
            &mut self.dc.viewport_ext
        } else {
            &mut self.dc.window_ext
        };
        if x_den != 0.0 && y_den != 0.0 {
            extent.0 = extent.0 * x_num / x_den;
            extent.1 = extent.1 * y_num / y_den;
        }
    }

    fn set_map_mode(&mut self, record: &[u8]) {
        // The mapping is the window/viewport pair whatever the mode says, so
        // only the modes that carry their own unit are worth noting — and in an
        // EMF even those are expressed through the extents.
        if let Some(mode) = read_u32(record, 8) {
            if !matches!(mode, 1 | 8) {
                self.note("EMR_SETMAPMODE (a mapping mode other than MM_TEXT)");
            }
        }
    }

    fn set_bk_mode(&mut self, record: &[u8]) {
        self.dc.bk_transparent = read_u32(record, 8) == Some(1);
    }

    fn set_fill_mode(&mut self, record: &[u8]) {
        self.dc.fill_rule = match read_u32(record, 8) {
            Some(2) => FillRule::Winding,
            _ => FillRule::Alternate,
        };
    }

    /// `SetROP2` sets how ink combines with the surface.
    ///
    /// `R2_COPYPEN` is the ordinary one and the default. The modes that draw
    /// the pen over, under or through the surface are expressed with the
    /// destination pixel, which this canvas can do; the ones that are *not* a
    /// function of the pen and the destination alone are noted instead of being
    /// drawn as if they were.
    fn set_rop2(&mut self, record: &[u8]) {
        let Some(mode) = read_u32(record, 8) else {
            return;
        };
        match mode {
            1 => self.canvas.set_rop(Rop::Blackness),
            4 => self.canvas.set_rop(Rop::NotSrcCopy),
            6 => self.canvas.set_rop(Rop::DstInvert),
            11 => self.canvas.set_rop(Rop::Blackness),
            13 => self.canvas.set_rop(Rop::SrcCopy),
            16 => self.canvas.set_rop(Rop::Whiteness),
            _ => {
                self.canvas.set_rop(Rop::SrcCopy);
                self.note("EMR_SETROP2 (a mix mode that needs the pen as well as the surface)");
            }
        }
    }

    fn set_text_align(&mut self, record: &[u8]) {
        if let Some(value) = read_u32(record, 8) {
            self.dc.align = decode_align(value);
        }
    }

    fn intersect_clip(&mut self, record: &[u8]) {
        let Some(rect) = read_rect(record, 8) else {
            return;
        };
        self.canvas.intersect_clip(self.transform_rect(rect));
    }

    /// `ExtSelectClipRgn`: a region in any combining mode. Only the empty region
    /// — which resets the clip to the whole surface — and a rectangular region
    /// can be honoured here.
    fn select_clip_region(&mut self, record: &[u8]) {
        let data_size = read_u32(record, 8).unwrap_or(0) as usize;
        let mode = read_u32(record, 12).unwrap_or(5);
        if data_size == 0 {
            // An empty region replaces the clip: back to the whole surface.
            if mode == 5 {
                self.reset_clip();
            }
            return;
        }
        let Some(rect) = region_rectangle(record, 16, data_size) else {
            self.note("EMR_EXTSELECTCLIPRGN (a non-rectangular region)");
            return;
        };
        match mode {
            5 => {
                self.reset_clip();
                self.canvas.intersect_clip(self.transform_rect(rect));
            }
            1 | 2 => self.canvas.intersect_clip(self.transform_rect(rect)),
            _ => self.note("EMR_EXTSELECTCLIPRGN (a combining mode that is not intersect or copy)"),
        }
    }

    fn reset_clip(&mut self) {
        self.canvas.set_clip(Clip {
            left: 0,
            top: 0,
            right: self.canvas.width() as i32,
            bottom: self.canvas.height() as i32,
        });
    }

    /// `RestoreDC`: the argument counts saved contexts back, so −1 restores the
    /// most recent one.
    fn restore_dc(&mut self, record: &[u8]) {
        let Some(relative) = read_i32(record, 8) else {
            return;
        };
        let steps = if relative < 0 {
            (-relative).max(1) as usize
        } else {
            1
        };
        for _ in 0..steps {
            match self.saved.pop() {
                Some((dc, clip)) => {
                    self.dc = dc;
                    self.canvas.set_clip(clip);
                }
                None => self.dc = Dc::default(),
            }
        }
    }

    fn set_world(&mut self, record: &[u8]) {
        if let Some(xform) = read_xform(record, 8) {
            self.dc.world = xform;
        }
    }

    fn modify_world(&mut self, record: &[u8]) {
        let (Some(xform), Some(mode)) = (read_xform(record, 8), read_u32(record, 32)) else {
            return;
        };
        self.dc.world = match mode {
            1 => Transform::IDENTITY,
            2 => xform.then(&self.dc.world),
            3 => self.dc.world.then(&xform),
            _ => xform,
        };
    }

    // --- objects ----------------------------------------------------------

    fn select_object(&mut self, record: &[u8]) {
        let Some(handle) = read_u32(record, 8) else {
            return;
        };
        match self.objects.get(&handle).cloned() {
            Some(Object::Pen(pen)) => self.dc.pen = pen,
            Some(Object::Brush(brush)) => self.dc.brush = brush,
            Some(Object::Font(font)) => self.dc.font = font,
            None => self.select_stock(handle),
        }
    }

    /// The stock objects a context can select without creating one.
    fn select_stock(&mut self, handle: u32) {
        match handle & 0x7fff_ffff {
            0 => {
                self.dc.brush = Brush {
                    style: 0,
                    color: Color::WHITE,
                }
            }
            1 => {
                self.dc.brush = Brush {
                    style: 0,
                    color: Color {
                        r: 192,
                        g: 192,
                        b: 192,
                    },
                }
            }
            2 => {
                self.dc.brush = Brush {
                    style: 0,
                    color: Color {
                        r: 128,
                        g: 128,
                        b: 128,
                    },
                }
            }
            3 => {
                self.dc.brush = Brush {
                    style: 0,
                    color: Color {
                        r: 64,
                        g: 64,
                        b: 64,
                    },
                }
            }
            4 => {
                self.dc.brush = Brush {
                    style: 0,
                    color: Color::BLACK,
                }
            }
            5 => {
                self.dc.brush = Brush {
                    style: BS_NULL,
                    color: Color::WHITE,
                }
            }
            6 => {
                self.dc.pen = Pen {
                    style: 0,
                    width: 1.0,
                    color: Color::WHITE,
                }
            }
            7 => {
                self.dc.pen = Pen {
                    style: 0,
                    width: 1.0,
                    color: Color::BLACK,
                }
            }
            8 => {
                self.dc.pen = Pen {
                    style: PS_NULL,
                    width: 1.0,
                    color: Color::BLACK,
                }
            }
            9..=16 => self.dc.font = Font::default(),
            _ => {}
        }
    }

    fn create_pen(&mut self, record: &[u8]) {
        let Some(handle) = read_u32(record, 8) else {
            return;
        };
        // `ExtCreatePen` carries an `EXTLOGPEN` where `CreatePen` carries a
        // `LOGPEN`: style, width, then either a colour or a brush style, colour
        // and hatch.
        let (style, width, color) = if record.len() >= 52 {
            (
                read_u32(record, 28).unwrap_or(0),
                read_f64(record, 32).unwrap_or(1.0),
                colorref(record, 40),
            )
        } else {
            (
                read_u32(record, 12).unwrap_or(0),
                read_f64(record, 16).unwrap_or(1.0),
                colorref(record, 24),
            )
        };
        self.objects.insert(
            handle,
            Object::Pen(Pen {
                style,
                width: if width.abs() < 1.0 { 1.0 } else { width.abs() },
                color,
            }),
        );
    }

    fn create_brush(&mut self, record: &[u8]) {
        let Some(handle) = read_u32(record, 8) else {
            return;
        };
        let style = read_u32(record, 12).unwrap_or(0);
        let color = colorref(record, 16);
        if !matches!(style, 0 | 1) {
            // A hatched or patterned brush tints with its own colour here.
            self.note("a hatched or patterned brush (drawn with its colour)");
        }
        self.objects.insert(
            handle,
            Object::Brush(Brush {
                style: if style == 1 { BS_NULL } else { 0 },
                color,
            }),
        );
    }

    fn create_font(&mut self, record: &[u8]) {
        let Some(handle) = read_u32(record, 8) else {
            return;
        };
        // `ExtCreateFontIndirectW` carries an `EXTLOGFONTW`, whose `lfHeight`
        // is at the same offset as the `LOGFONTW`'s.
        let height = read_f64(record, 12).unwrap_or(-12.0);
        let escapement = read_f64(record, 20).unwrap_or(0.0);
        self.objects.insert(
            handle,
            Object::Font(Font {
                height: if height == 0.0 { -12.0 } else { height },
                escapement,
            }),
        );
    }

    // --- drawing ----------------------------------------------------------

    fn set_pixel(&mut self, record: &[u8]) {
        let (Some(x), Some(y)) = (read_f64(record, 8), read_f64(record, 12)) else {
            return;
        };
        let color = colorref(record, 16);
        let point = self.transform(pt(x, y));
        self.canvas
            .put_pixel(point.x.floor() as i32, point.y.floor() as i32, color);
    }

    fn line_to(&mut self, record: &[u8]) {
        let (Some(x), Some(y)) = (read_f64(record, 8), read_f64(record, 12)) else {
            return;
        };
        let from = self.dc.current;
        let to = pt(x, y);
        self.draw_contour(&[from, to], false, false);
        self.dc.current = to;
    }

    fn poly32(&mut self, kind: u32, record: &[u8]) {
        let Some(points) = read_points32(record, 24) else {
            return;
        };
        self.polygon_like(kind, points);
    }

    fn poly16(&mut self, kind: u32, record: &[u8]) {
        let Some(points) = read_points16(record, 24) else {
            return;
        };
        self.polygon_like(kind, points);
    }

    /// A record that is a polygon (filled and closed) or a polyline (stroked).
    fn polygon_like(&mut self, kind: u32, mut points: Vec<Point>) {
        if kind == 6 || kind == 89 {
            // `PolylineTo`: the first segment starts at the current position,
            // and the last point becomes it.
            points.insert(0, self.dc.current);
            if let Some(last) = points.last() {
                self.dc.current = *last;
            }
        }
        let closed = kind == 3 || kind == 86;
        self.draw_contour(&points, closed, closed);
    }

    fn poly_poly32(&mut self, kind: u32, record: &[u8]) {
        let Some(contours) = read_poly_polygon32(record) else {
            return;
        };
        self.draw_contours(&contours, kind == 8, kind == 8);
    }

    fn poly_poly16(&mut self, kind: u32, record: &[u8]) {
        let Some(contours) = read_poly_polygon16(record) else {
            return;
        };
        self.draw_contours(&contours, kind == 91, kind == 91);
    }

    /// `PolyBezier` and `PolyBezierTo`, in either point width.
    fn poly_beziers(&mut self, kind: u32, record: &[u8]) {
        let points = if kind >= 85 {
            read_points16(record, 24)
        } else {
            read_points32(record, 24)
        };
        let Some(points) = points else { return };
        let start = if kind == 5 || kind == 88 {
            Some(self.dc.current)
        } else {
            None
        };
        let curves = flatten_beziers(&points, start);
        let last = curves.last().copied().unwrap_or(self.dc.current);
        let closed = false;
        self.draw_contour(&curves, closed, closed);
        self.dc.current = last;
    }

    fn shape(&mut self, kind: u32, record: &[u8]) {
        let Some(rect) = read_rect(record, 8) else {
            return;
        };
        let rect = rect.normalised();
        let contour = match kind {
            42 => ellipse_contour(rect, 64),
            44 => {
                let corner_x = read_f64(record, 24).unwrap_or(0.0);
                let corner_y = read_f64(record, 28).unwrap_or(0.0);
                round_rect_contour(rect, corner_x, corner_y)
            }
            _ => vec![
                pt(rect.left, rect.top),
                pt(rect.right, rect.top),
                pt(rect.right, rect.bottom),
                pt(rect.left, rect.bottom),
            ],
        };
        if self.open_path.is_some() {
            self.push_contour(&contour, true);
            return;
        }
        self.draw_contours(std::slice::from_ref(&contour), true, true);
    }

    fn arc_shape(&mut self, kind: u32, record: &[u8]) {
        let (Some(rect), Some(start), Some(end)) = (
            read_rect(record, 8),
            read_point32(record, 24),
            read_point32(record, 32),
        ) else {
            return;
        };
        let rect = rect.normalised();
        let contour = arc_contour(rect, start, end, kind);
        if self.open_path.is_some() {
            self.push_contour(&contour, false);
            return;
        }
        let closed_and_filled = kind != 45;
        self.draw_contours(
            std::slice::from_ref(&contour),
            closed_and_filled,
            closed_and_filled,
        );
    }

    fn angle_arc(&mut self, record: &[u8]) {
        let (Some(center), Some(radius)) = (read_point32(record, 8), read_f64(record, 24)) else {
            return;
        };
        let rect = Rect::new(
            center.x - radius,
            center.y - radius,
            center.x + radius,
            center.y + radius,
        );
        let contour = ellipse_contour(rect, 64);
        self.draw_contours(std::slice::from_ref(&contour), false, false);
    }

    // --- paths ------------------------------------------------------------

    fn close_figure(&mut self) {
        if let Some(path) = self.open_path.as_mut() {
            if let Some(contour) = path.last_mut() {
                if let Some(first) = contour.first().copied() {
                    contour.push(first);
                }
            }
        } else if let Some(path) = self.path.as_mut() {
            if let Some(contour) = path.last_mut() {
                if let Some(first) = contour.first().copied() {
                    contour.push(first);
                }
            }
        }
    }

    fn push_contour(&mut self, contour: &[Point], closed: bool) {
        if let Some(path) = self.open_path.as_mut() {
            let mut contour = contour.to_vec();
            if closed {
                if let Some(first) = contour.first().copied() {
                    contour.push(first);
                }
            }
            path.push(contour);
        }
    }

    /// Draw one contour: appended to the open path when one is being built,
    /// otherwise filled and/or stroked as the record says.
    fn draw_contour(&mut self, contour: &[Point], closed: bool, filled: bool) {
        if self.open_path.is_some() {
            self.push_contour(contour, closed);
            return;
        }
        self.draw_contours(&[contour.to_vec()], closed, filled);
    }

    fn draw_contours(&mut self, contours: &[Vec<Point>], closed: bool, filled: bool) {
        if contours.is_empty() {
            return;
        }
        let transform = self.to_canvas();
        let canvas_contours: Vec<Vec<Point>> = contours
            .iter()
            .map(|contour| contour.iter().map(|p| transform.apply(*p)).collect())
            .collect();
        if filled && self.dc.brush.style != BS_NULL {
            self.canvas
                .fill_polygons(&canvas_contours, self.dc.fill_rule, self.dc.brush.color);
        }
        if self.dc.pen.style != PS_NULL {
            let width = self.dc.pen.width * transform.scale_of().1;
            for contour in &canvas_contours {
                self.canvas
                    .stroke(contour, closed, width, self.dc.pen.color);
            }
        }
    }

    fn finish_path(&mut self, kind: u32) {
        let Some(path) = self.path.take() else { return };
        let transform = self.to_canvas();
        let contours: Vec<Vec<Point>> = path
            .iter()
            .map(|contour| contour.iter().map(|p| transform.apply(*p)).collect())
            .collect();
        if kind != 64 && self.dc.brush.style != BS_NULL {
            self.canvas
                .fill_polygons(&contours, self.dc.fill_rule, self.dc.brush.color);
        }
        if kind != 62 && self.dc.pen.style != PS_NULL {
            let width = self.dc.pen.width * transform.scale_of().1;
            for contour in &contours {
                self.canvas.stroke(contour, false, width, self.dc.pen.color);
            }
        }
    }

    // --- text -------------------------------------------------------------

    fn text_out(&mut self, kind: u32, record: &[u8]) {
        if record.len() < 76 {
            return;
        }
        let Some(count) = read_u32(record, 44) else {
            return;
        };
        let Some(string_at) = read_u32(record, 48) else {
            return;
        };
        let options = read_u32(record, 52).unwrap_or(0);
        let (Some(x), Some(y)) = (read_f64(record, 36), read_f64(record, 40)) else {
            return;
        };
        if options & 0x10 != 0 {
            // `ETO_GLYPH_INDEX`: the codes are glyph numbers, not characters.
            self.note("EMR_EXTTEXTOUT with glyph indices");
            return;
        }
        let count = count as usize;
        let start = string_at as usize;
        let text = if kind == 83 {
            let Some(bytes) = record.get(start..start + count) else {
                return;
            };
            bytes.iter().map(|b| *b as char).collect::<String>()
        } else {
            let Some(bytes) = record.get(start..start + count * 2) else {
                return;
            };
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        };
        let align = decode_align(options & 0xffff);
        let reference = if options & 1 != 0 {
            // `TA_UPDATECP`: the current position is the reference point, and
            // the call leaves it after the text it drew.
            let reference = self.dc.current;
            let advance = text.chars().count() as f64 * self.dc.font.height.abs() * 6.0 / 7.0;
            self.dc.current = pt(self.dc.current.x + advance, self.dc.current.y);
            reference
        } else {
            pt(x, y)
        };
        self.draw_text_run(&text, reference, align);
    }

    fn draw_text_run(&mut self, text: &str, reference: Point, align: TextAlign) {
        let transform = self.to_canvas();
        let (_, scale_y) = transform.scale_of();
        // The font's height is in logical units like everything else, so the
        // same scale that places the reference point sizes the glyphs.
        let cell = self.dc.font.height.abs() * scale_y;
        if cell <= 0.5 {
            return;
        }
        let background = self.background();
        font::draw_text(
            &mut self.canvas,
            text,
            transform.apply(reference),
            cell,
            align,
            self.dc.font.escapement,
            self.dc.text_color,
            background,
        );
    }

    /// The colour GDI fills behind text in its opaque mode.
    fn background(&self) -> Option<Color> {
        (!self.dc.bk_transparent).then_some(self.dc.bk_color)
    }

    // --- bitmaps ----------------------------------------------------------

    /// `StretchBlt`, which carries a bitmap inside the record at an offset from
    /// the record's own start.
    fn stretch_blt_record(&mut self, record: &[u8], offset: usize) {
        if record.len() < 88 {
            return;
        }
        let (Some(dest_x), Some(dest_y)) = (read_f64(record, 24), read_f64(record, 28)) else {
            return;
        };
        let (Some(dest_w), Some(dest_h)) = (read_f64(record, 32), read_f64(record, 36)) else {
            return;
        };
        let Some(rop) = read_u32(record, 40).and_then(Rop::from_code) else {
            self.note("EMR_STRETCHBLT with a raster operation it cannot combine");
            return;
        };
        let (Some(src_x), Some(src_y)) = (read_f64(record, 44), read_f64(record, 48)) else {
            return;
        };
        let Some(bmi_at) = read_u32(record, 76) else {
            return;
        };
        let bitmap = match dib::decode(record_at(self.data, offset, bmi_at as usize)) {
            Ok(bitmap) => bitmap,
            Err(reason) => {
                self.note(&format!("EMR_STRETCHBLT with {reason}"));
                return;
            }
        };
        let source = Rect::new(src_x, src_y, src_x + dest_w, src_y + dest_h);
        let destination = Rect::new(dest_x, dest_y, dest_x + dest_w, dest_y + dest_h);
        self.blit(&bitmap, source, destination, rop);
    }

    /// `StretchDIBits` and `SetDIBitsToDevice`, which carry a bitmap inside the
    /// record at an offset from its own start.
    fn stretch_dibits(&mut self, record: &[u8], offset: usize) {
        if record.len() < 72 {
            return;
        }
        let (Some(dest_x), Some(dest_y)) = (read_f64(record, 24), read_f64(record, 28)) else {
            return;
        };
        let (Some(src_x), Some(src_y)) = (read_f64(record, 32), read_f64(record, 36)) else {
            return;
        };
        let (Some(width), Some(height)) = (read_f64(record, 40), read_f64(record, 44)) else {
            return;
        };
        let Some(bmi_at) = read_u32(record, 48) else {
            return;
        };
        let Some(rop_code) = read_u32(record, 68) else {
            return;
        };
        let Some(rop) = Rop::from_code(rop_code) else {
            self.note("StretchDIBits with a raster operation it cannot combine");
            return;
        };
        let bitmap = match dib::decode(record_at(self.data, offset, bmi_at as usize)) {
            Ok(bitmap) => bitmap,
            Err(reason) => {
                self.note(&format!("StretchDIBits with {reason}"));
                return;
            }
        };
        let source = Rect::new(src_x, src_y, src_x + width, src_y + height);
        let destination = Rect::new(dest_x, dest_y, dest_x + width, dest_y + height);
        self.blit(&bitmap, source, destination, rop);
    }

    fn blit(&mut self, bitmap: &Bitmap, source: Rect, destination: Rect, rop: Rop) {
        let transform = self.to_canvas();
        let (a, b) = (
            transform.apply(pt(destination.left, destination.top)),
            transform.apply(pt(destination.right, destination.bottom)),
        );
        let rect = Rect::new(a.x, a.y, b.x, b.y);
        self.canvas.stretch_blit(bitmap, source, rect, rop);
    }
}

// --- helpers ---------------------------------------------------------------

fn nonzero(value: f64) -> f64 {
    if value == 0.0 {
        1.0
    } else {
        value
    }
}

fn decode_align(value: u32) -> TextAlign {
    TextAlign {
        horizontal: match value & 0x6 {
            2 => Horizontal::Right,
            6 => Horizontal::Center,
            _ => Horizontal::Left,
        },
        vertical: match value & 0x18 {
            8 => Vertical::Bottom,
            24 => Vertical::Baseline,
            _ => Vertical::Top,
        },
    }
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_i32(data: &[u8], at: usize) -> Option<i32> {
    data.get(at..at + 4)
        .map(|bytes| i32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_i16(data: &[u8], at: usize) -> Option<i16> {
    data.get(at..at + 2)
        .map(|bytes| i16::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_f64(data: &[u8], at: usize) -> Option<f64> {
    read_i32(data, at).map(|value| value as f64)
}

fn read_f32(data: &[u8], at: usize) -> Option<f32> {
    data.get(at..at + 4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
}

fn colorref(data: &[u8], at: usize) -> Color {
    Color::from_colorref(read_u32(data, at).unwrap_or(0))
}

fn read_rect(data: &[u8], at: usize) -> Option<Rect> {
    Some(Rect::new(
        read_f64(data, at)?,
        read_f64(data, at + 4)?,
        read_f64(data, at + 8)?,
        read_f64(data, at + 12)?,
    ))
}

fn read_point32(data: &[u8], at: usize) -> Option<Point> {
    Some(pt(read_f64(data, at)?, read_f64(data, at + 4)?))
}

fn read_xform(data: &[u8], at: usize) -> Option<Transform> {
    Some(Transform::new(
        read_f32(data, at)? as f64,
        read_f32(data, at + 4)? as f64,
        read_f32(data, at + 8)? as f64,
        read_f32(data, at + 12)? as f64,
        read_f32(data, at + 16)? as f64,
        read_f32(data, at + 20)? as f64,
    ))
}

/// The points of a polygon record: a count, then that many points.
fn read_points32(record: &[u8], at: usize) -> Option<Vec<Point>> {
    let count = read_u32(record, at)? as usize;
    let mut points = Vec::with_capacity(count);
    for i in 0..count {
        points.push(read_point32(record, at + 4 + i * 8)?);
    }
    Some(points)
}

fn read_points16(record: &[u8], at: usize) -> Option<Vec<Point>> {
    let count = read_u32(record, at)? as usize;
    let mut points = Vec::with_capacity(count);
    for i in 0..count {
        let offset = at + 4 + i * 4;
        points.push(pt(
            read_i16(record, offset)? as f64,
            read_i16(record, offset + 2)? as f64,
        ));
    }
    Some(points)
}

/// A `PolyPolygon` record: a count of contours, the total point count, the
/// per-contour counts, then all the points.
fn read_poly_polygon32(record: &[u8]) -> Option<Vec<Vec<Point>>> {
    let contours = read_u32(record, 24)? as usize;
    let counts = read_counts32(record, 32, contours)?;
    let mut points = Vec::new();
    let mut at = 32 + contours * 4;
    for count in counts {
        let mut contour = Vec::with_capacity(count);
        for _ in 0..count {
            contour.push(read_point32(record, at)?);
            at += 8;
        }
        points.push(contour);
    }
    Some(points)
}

fn read_poly_polygon16(record: &[u8]) -> Option<Vec<Vec<Point>>> {
    let contours = read_u32(record, 24)? as usize;
    let counts = read_counts32(record, 32, contours)?;
    let mut points = Vec::new();
    let mut at = 32 + contours * 4;
    for count in counts {
        let mut contour = Vec::with_capacity(count);
        for _ in 0..count {
            contour.push(pt(
                read_i16(record, at)? as f64,
                read_i16(record, at + 2)? as f64,
            ));
            at += 4;
        }
        points.push(contour);
    }
    Some(points)
}

fn read_counts32(record: &[u8], at: usize, count: usize) -> Option<Vec<usize>> {
    let mut counts = Vec::with_capacity(count);
    for i in 0..count {
        counts.push(read_u32(record, at + i * 4)? as usize);
    }
    Some(counts)
}

/// A slice of the file starting at `offset + at`, for the records that address
/// their own embedded data by offset.
fn record_at(data: &[u8], offset: usize, at: usize) -> &[u8] {
    data.get(offset + at..).unwrap_or(&[])
}

/// A rectangular region out of an `ExtSelectClipRgn` record.
fn region_rectangle(record: &[u8], at: usize, data_size: usize) -> Option<Rect> {
    // `RGNDATAHEADER`: size, type, count, region size, bounds. Only the
    // rectangle region type (1) is understood, and only its first rectangle.
    if data_size < 32 || read_u32(record, at + 4)? != 1 {
        return None;
    }
    read_rect(record, at + 16)
}

/// An ellipse as a closed polygon.
fn ellipse_contour(rect: Rect, steps: usize) -> Vec<Point> {
    let (cx, cy) = (
        rect.left + rect.width() / 2.0,
        rect.top + rect.height() / 2.0,
    );
    let (rx, ry) = (rect.width() / 2.0, rect.height() / 2.0);
    (0..steps)
        .map(|i| {
            let angle = i as f64 / steps as f64 * std::f64::consts::TAU;
            pt(cx + rx * angle.cos(), cy + ry * angle.sin())
        })
        .collect()
}

/// Arc, chord and pie all come from the same two radial endpoints.
fn arc_contour(rect: Rect, start: Point, end: Point, kind: u32) -> Vec<Point> {
    let (cx, cy) = (
        rect.left + rect.width() / 2.0,
        rect.top + rect.height() / 2.0,
    );
    let (rx, ry) = (rect.width() / 2.0, rect.height() / 2.0);
    if rx <= 0.0 || ry <= 0.0 {
        return Vec::new();
    }
    let angle_of = |p: Point| ((p.y - cy) / ry).atan2((p.x - cx) / rx);
    let (from, mut to) = (angle_of(start), angle_of(end));
    // A GDI arc runs anticlockwise from the start to the end point, which in
    // this y-down space means the angle decreases.
    while to > from {
        to -= std::f64::consts::TAU;
    }
    let steps = 64;
    let mut points: Vec<Point> = (0..=steps)
        .map(|i| {
            let angle = from + (to - from) * i as f64 / steps as f64;
            pt(cx + rx * angle.cos(), cy + ry * angle.sin())
        })
        .collect();
    match kind {
        46 => {
            // A chord closes across the two endpoints.
            points.push(points[0]);
        }
        47 => {
            // A pie closes through the centre.
            points.insert(0, pt(cx, cy));
            points.push(pt(cx, cy));
        }
        // A plain arc is a line, and is left open for the pen.
        _ => {}
    }
    points
}

fn round_rect_contour(rect: Rect, corner_x: f64, corner_y: f64) -> Vec<Point> {
    let rx = corner_x.abs().min(rect.width() / 2.0);
    let ry = corner_y.abs().min(rect.height() / 2.0);
    if rx <= 0.0 || ry <= 0.0 {
        return vec![
            pt(rect.left, rect.top),
            pt(rect.right, rect.top),
            pt(rect.right, rect.bottom),
            pt(rect.left, rect.bottom),
        ];
    }
    let mut points = Vec::new();
    let corners = [
        (rect.right - rx, rect.bottom - ry, 0.0),
        (
            rect.left + rx,
            rect.bottom - ry,
            std::f64::consts::FRAC_PI_2,
        ),
        (rect.left + rx, rect.top + ry, std::f64::consts::PI),
        (rect.right - rx, rect.top + ry, std::f64::consts::PI * 1.5),
    ];
    for (cx, cy, from) in corners {
        for i in 0..=8 {
            let angle = from + std::f64::consts::FRAC_PI_2 * i as f64 / 8.0;
            points.push(pt(cx + rx * angle.cos(), cy + ry * angle.sin()));
        }
    }
    points
}

/// Turn cubic Beziers into the segments their record describes.
fn flatten_beziers(points: &[Point], start: Option<Point>) -> Vec<Point> {
    let mut out = Vec::new();
    let mut current = start;
    let mut remaining = points;
    while remaining.len() >= 3 {
        let from = match current {
            Some(point) => point,
            None => remaining[0],
        };
        let (control1, control2, to) = (remaining[0], remaining[1], remaining[2]);
        let usable_from = if current.is_some() {
            from
        } else {
            remaining[0]
        };
        let steps = 24;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            out.push(bezier(usable_from, control1, control2, to, t));
        }
        current = Some(to);
        remaining = &remaining[3..];
    }
    out
}

fn bezier(from: Point, c1: Point, c2: Point, to: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    pt(
        a * from.x + b * c1.x + c * c2.x + d * to.x,
        a * from.y + b * c1.y + c * c2.y + d * to.y,
    )
}
