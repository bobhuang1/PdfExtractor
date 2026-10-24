//! The surface a metafile paints on, and the rasterisers behind its drawing
//! calls.
//!
//! Everything here works in *canvas* pixels, top-left origin, y downwards:
//! mapping a metafile's own coordinate space onto that canvas is the job of the
//! two players, which hand this module points that have already been
//! transformed.
//!
//! Filling is done with a scanline rasteriser and lines are drawn aliased, which
//! is what the GDI paths these files describe did — a metafile is a recording of
//! non-antialiased drawing, and rendering it smooth would be inventing detail
//! the file does not carry.

use std::cmp::Ordering;

/// A colour as the metafiles hold it: opaque, no alpha channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
    };
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };

    /// A `COLORREF`: `0x00BBGGRR`, as the pen, brush and text colours are
    /// written.
    pub fn from_colorref(value: u32) -> Color {
        Color {
            r: (value & 0xff) as u8,
            g: ((value >> 8) & 0xff) as u8,
            b: ((value >> 16) & 0xff) as u8,
        }
    }

    /// A palette entry, which the DIB headers write as `0x00BBGGRR` too but the
    /// text records use as-is.
    pub fn from_bgr(value: u32) -> Color {
        Self::from_colorref(value)
    }
}

/// A point in whatever space the caller is working in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

pub const fn pt(x: f64, y: f64) -> Point {
    Point { x, y }
}

/// An axis-aligned rectangle. Not necessarily normalised: a metafile is free to
/// give the corners in any order, so [`Rect::normalised`] is applied before use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Rect {
    pub const fn new(left: f64, top: f64, right: f64, bottom: f64) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn normalised(self) -> Rect {
        Rect {
            left: self.left.min(self.right),
            top: self.top.min(self.bottom),
            right: self.left.max(self.right),
            bottom: self.top.max(self.bottom),
        }
    }

    pub fn width(self) -> f64 {
        self.right - self.left
    }

    pub fn height(self) -> f64 {
        self.bottom - self.top
    }

    pub fn is_empty(self) -> bool {
        !(self.width() > 0.0 && self.height() > 0.0)
    }
}

/// An affine transform, applied as `x' = a·x + c·y + e`, `y' = b·x + d·y + f`
/// — the same layout as the `XFORM` the metafiles store.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Transform {
        Transform { a, b, c, d, e, f }
    }

    pub fn translate(dx: f64, dy: f64) -> Transform {
        Transform::new(1.0, 0.0, 0.0, 1.0, dx, dy)
    }

    pub fn scale(sx: f64, sy: f64) -> Transform {
        Transform::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }

    /// This transform, then `next`.
    pub fn then(&self, next: &Transform) -> Transform {
        Transform {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    pub fn apply(&self, p: Point) -> Point {
        pt(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// How much this transform scales each axis. A metafile's `SetWindowExt`/
    /// `SetViewportExt` pair is how a drawing declares its units, and text is
    /// sized through the same scale as the geometry.
    pub fn scale_of(&self) -> (f64, f64) {
        let sx = (self.a * self.a + self.b * self.b).sqrt();
        let sy = (self.c * self.c + self.d * self.d).sqrt();
        (sx, sy)
    }

    pub fn is_identity(&self) -> bool {
        *self == Transform::IDENTITY
    }
}

/// Which pixels of a polygon are inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    /// Alternate: the region between crossings 1-2, 3-4, ….
    Alternate,
    /// Winding: the region where the running count of edge directions is not
    /// zero, so a hole drawn the other way round stays a hole.
    Winding,
}

/// A clipping rectangle in canvas pixels, half-open: `[left, right)` by
/// `[top, bottom)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clip {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Clip {
    pub fn is_empty(&self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }

    fn intersect(self, other: Clip) -> Clip {
        Clip {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }
}

/// A decoded bitmap: RGBA8, opaque, row-major from the top left.
#[derive(Debug, Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Bitmap {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Bitmap> {
        if pixels.len() != (width as usize) * (height as usize) * 4 {
            return None;
        }
        Some(Bitmap {
            width,
            height,
            pixels,
        })
    }

    /// A 1×1 bitmap, which is what a brush pattern reduces to when its own
    /// bitmap is missing or undecodable.
    pub fn solid(color: Color) -> Bitmap {
        Bitmap {
            width: 1,
            height: 1,
            pixels: vec![color.r, color.g, color.b, 255],
        }
    }

    pub fn color_at(&self, x: i32, y: i32) -> Option<Color> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        let idx = ((y as usize) * (self.width as usize) + x as usize) * 4;
        Some(Color {
            r: self.pixels[idx],
            g: self.pixels[idx + 1],
            b: self.pixels[idx + 2],
        })
    }
}

/// The raster operations the corpus uses, named as Windows names them.
///
/// A metafile states how the source and destination combine, and the
/// transparent-icon idiom depends on it: an AND mask followed by an XOR of the
/// colour image is how a small picture with transparency was drawn before alpha
/// channels existed. Copying the source and ignoring the operator would paint a
/// black box over the drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rop {
    SrcCopy,
    SrcAnd,
    SrcInvert,
    SrcPaint,
    DstInvert,
    NotSrcCopy,
    PatCopy,
    Blackness,
    Whiteness,
}

impl Rop {
    /// Decode a raster-operation code. The high bits of a WMF's `rop` carry
    /// flags, so only the operation itself is matched.
    pub fn from_code(code: u32) -> Option<Rop> {
        Some(match code & 0x00ff_ffff {
            0x00cc_0020 | 0x0000_0020 => Rop::SrcCopy,
            0x0088_00c6 | 0x0000_00c6 => Rop::SrcAnd,
            0x0066_0046 | 0x0000_0046 => Rop::SrcInvert,
            0x00ee_0086 | 0x0000_0086 => Rop::SrcPaint,
            0x0055_0009 | 0x0000_0009 => Rop::DstInvert,
            0x0033_0008 | 0x0000_0008 => Rop::NotSrcCopy,
            0x00f0_0021 | 0x0000_0021 => Rop::PatCopy,
            0x0000_0042 => Rop::Blackness,
            0x00ff_0062 => Rop::Whiteness,
            _ => return None,
        })
    }

    /// Combine one source pixel with the destination it lands on.
    ///
    /// `src` is the pixel being drawn — a bitmap sample, or the brush colour
    /// when the operation paints with the brush.
    pub fn apply(self, src: Color, dst: Color) -> Color {
        let channel = |f: fn(u8, u8) -> u8| Color {
            r: f(src.r, dst.r),
            g: f(src.g, dst.g),
            b: f(src.b, dst.b),
        };
        match self {
            Rop::SrcCopy | Rop::PatCopy => src,
            Rop::Blackness => Color::BLACK,
            Rop::Whiteness => Color::WHITE,
            Rop::SrcAnd => channel(|s, d| s & d),
            Rop::SrcInvert => channel(|s, d| s ^ d),
            Rop::SrcPaint => channel(|s, d| s | d),
            Rop::DstInvert => channel(|_, d| !d),
            Rop::NotSrcCopy => channel(|s, _| !s),
        }
    }
}

/// The pixel buffer a drawing is rasterised into.
#[derive(Debug, Clone)]
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    clip: Clip,
    /// How ink combines with what is already on the surface — the `SetROP2`
    /// mode of a device context. Every fill, stroke and glyph goes through it.
    rop: Rop,
    /// Whether any drawing call actually put ink down. A metafile that only
    /// sets state is not a picture of anything.
    drew: bool,
}

impl Canvas {
    /// A white canvas, the colour a metafile is played onto.
    pub fn new(width: u32, height: u32) -> Canvas {
        let mut pixels = vec![255u8; (width as usize) * (height as usize) * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Canvas {
            width,
            height,
            pixels,
            clip: Clip {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            },
            rop: Rop::SrcCopy,
            drew: false,
        }
    }

    /// Set how ink combines with the surface, as `SetROP2` does.
    pub fn set_rop(&mut self, rop: Rop) {
        self.rop = rop;
    }

    pub fn rop(&self) -> Rop {
        self.rop
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn data(&self) -> &[u8] {
        &self.pixels
    }

    pub fn clip(&self) -> Clip {
        self.clip
    }

    pub fn set_clip(&mut self, clip: Clip) {
        self.clip = clip;
    }

    /// Whether anything was drawn at all.
    pub fn drew(&self) -> bool {
        self.drew
    }

    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// Shrink the clip to the intersection with `rect`, given in canvas pixels.
    pub fn intersect_clip(&mut self, rect: Rect) {
        let rect = rect.normalised();
        let clip = Clip {
            left: rect.left.floor() as i32,
            top: rect.top.floor() as i32,
            right: rect.right.ceil() as i32,
            bottom: rect.bottom.ceil() as i32,
        };
        self.clip = self.clip.intersect(clip);
    }

    /// Put a pixel down through the current raster operation.
    pub fn put_pixel(&mut self, x: i32, y: i32, color: Color) {
        if !self.visible(x, y) {
            return;
        }
        let out = match self.rop {
            Rop::SrcCopy | Rop::PatCopy => color,
            Rop::Blackness => Color::BLACK,
            Rop::Whiteness => Color::WHITE,
            other => other.apply(color, self.color_at(x, y)),
        };
        self.store(x, y, out);
    }

    fn visible(&self, x: i32, y: i32) -> bool {
        x >= self.clip.left && x < self.clip.right && y >= self.clip.top && y < self.clip.bottom
    }

    /// Write a pixel as given, ignoring the raster operation: a blit record
    /// carries its own operator and has already applied it.
    fn store(&mut self, x: i32, y: i32, color: Color) {
        if !self.visible(x, y) {
            return;
        }
        let idx = ((y as usize) * (self.width as usize) + x as usize) * 4;
        self.pixels[idx] = color.r;
        self.pixels[idx + 1] = color.g;
        self.pixels[idx + 2] = color.b;
        self.pixels[idx + 3] = 255;
        self.drew = true;
    }

    fn color_at(&self, x: i32, y: i32) -> Color {
        let idx = ((y as usize) * (self.width as usize) + x as usize) * 4;
        Color {
            r: self.pixels[idx],
            g: self.pixels[idx + 1],
            b: self.pixels[idx + 2],
        }
    }

    /// Composite `color` over the pixel at `(x, y)` at `alpha` (0..=1).
    ///
    /// A gradient stop and a shadow are both a colour seen *through* something —
    /// a stop's transparency, or a shadow's own alpha — so they have to blend
    /// with what is already on the surface rather than replace it. The canvas
    /// is opaque, so this is a plain source-over mix.
    pub fn blend_pixel(&mut self, x: i32, y: i32, color: Color, alpha: f64) {
        if !self.visible(x, y) || alpha <= 0.0 {
            return;
        }
        if alpha >= 1.0 {
            self.store(x, y, color);
            return;
        }
        let under = self.color_at(x, y);
        let mix = |src: u8, dst: u8| {
            (f64::from(dst) + (f64::from(src) - f64::from(dst)) * alpha)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        self.store(
            x,
            y,
            Color {
                r: mix(color.r, under.r),
                g: mix(color.g, under.g),
                b: mix(color.b, under.b),
            },
        );
    }

    /// Fill the polygons in `contours`, each implicitly closed.
    pub fn fill_polygons(&mut self, contours: &[Vec<Point>], rule: FillRule, color: Color) {
        self.fill_polygons_shaded(contours, rule, &mut |_, _| (color, 1.0));
    }

    /// Fill the polygons with the colour that belongs at each pixel.
    ///
    /// `shade` is given the centre of the pixel on the canvas and answers the
    /// colour to put there and how opaque it is, which is what a gradient needs:
    /// the colour is a function of where it lands, not of the shape it fills.
    /// The scan conversion — crossings, the fill rule and the span arithmetic —
    /// is the same either way.
    pub fn fill_polygons_shaded(
        &mut self,
        contours: &[Vec<Point>],
        rule: FillRule,
        shade: &mut dyn FnMut(f64, f64) -> (Color, f64),
    ) {
        if self.clip.is_empty() {
            return;
        }
        let mut edges: Vec<(f64, f64, f64, f64)> = Vec::new();
        for contour in contours {
            if contour.len() < 2 {
                continue;
            }
            for i in 0..contour.len() {
                let a = contour[i];
                let b = contour[(i + 1) % contour.len()];
                // A horizontal edge can never be crossed by a scanline, so it
                // contributes neither a crossing nor a winding direction.
                if a.y == b.y {
                    continue;
                }
                edges.push((a.x, a.y, b.x, b.y));
            }
        }
        if edges.is_empty() {
            return;
        }
        let y_min = edges
            .iter()
            .map(|e| e.1.min(e.3))
            .fold(f64::INFINITY, f64::min);
        let y_max = edges
            .iter()
            .map(|e| e.1.max(e.3))
            .fold(f64::NEG_INFINITY, f64::max);
        let first = ((y_min - 0.5).ceil() as i32).max(self.clip.top);
        let last = (((y_max - 0.5).ceil() as i32) - 1).min(self.clip.bottom - 1);
        for y in first..=last {
            let scan = y as f64 + 0.5;
            let mut crossings: Vec<(f64, i32)> = Vec::new();
            for &(x0, y0, x1, y1) in &edges {
                let (top, bottom) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                // Half-open in y, so a vertex shared by two edges is crossed
                // exactly once.
                if scan < top || scan >= bottom {
                    continue;
                }
                let t = (scan - y0) / (y1 - y0);
                let x = x0 + t * (x1 - x0);
                crossings.push((x, if y1 > y0 { 1 } else { -1 }));
            }
            if crossings.len() < 2 {
                continue;
            }
            crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
            match rule {
                FillRule::Alternate => {
                    for pair in crossings.chunks(2) {
                        if let [from, to] = pair {
                            self.fill_shaded_span(y, from.0, to.0, shade);
                        }
                    }
                }
                FillRule::Winding => {
                    let mut winding = 0i32;
                    let mut from = 0.0f64;
                    for (x, direction) in crossings {
                        if winding == 0 {
                            from = x;
                        }
                        winding += direction;
                        if winding == 0 {
                            self.fill_shaded_span(y, from, x, shade);
                        }
                    }
                }
            }
        }
    }

    /// Fill from `x0` up to but not including `x1`, on the row whose centre is
    /// `y + 0.5`, with the colour `shade` gives each pixel.
    fn fill_shaded_span(
        &mut self,
        y: i32,
        x0: f64,
        x1: f64,
        shade: &mut dyn FnMut(f64, f64) -> (Color, f64),
    ) {
        if y < self.clip.top || y >= self.clip.bottom {
            return;
        }
        // A pixel is inside the span when its centre is.
        let first = ((x0 - 0.5).ceil() as i32).max(self.clip.left);
        let after = ((x1 - 0.5).ceil() as i32).min(self.clip.right);
        let scan = y as f64 + 0.5;
        for x in first..after {
            let (color, alpha) = shade(x as f64 + 0.5, scan);
            if alpha >= 1.0 {
                self.put_pixel(x, y, color);
            } else {
                self.blend_pixel(x, y, color, alpha);
            }
        }
    }

    /// Fill the intersection of `rect` with the canvas.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.fill_rect_rop(rect, color, Rop::SrcCopy);
    }

    /// Fill `rect` through a raster operation, with `color` as the source.
    pub fn fill_rect_rop(&mut self, rect: Rect, color: Color, rop: Rop) {
        let rect = rect.normalised();
        let first_x = (rect.left.floor() as i32).max(self.clip.left);
        let last_x = (rect.right.ceil() as i32).min(self.clip.right);
        let first_y = (rect.top.floor() as i32).max(self.clip.top);
        let last_y = (rect.bottom.ceil() as i32).min(self.clip.bottom);
        for y in first_y..last_y {
            for x in first_x..last_x {
                let out = match rop {
                    Rop::SrcCopy | Rop::PatCopy => color,
                    Rop::Blackness => Color::BLACK,
                    Rop::Whiteness => Color::WHITE,
                    other => other.apply(color, self.color_at(x, y)),
                };
                self.store(x, y, out);
            }
        }
    }

    /// Draw a polyline, or a polygon when `closed`, with a stroke of `width`
    /// canvas pixels.
    ///
    /// A one-pixel stroke is drawn as a line, which is what GDI did with a
    /// cosmetic pen; anything thicker becomes the quads between the segment
    /// offsets plus a square at each vertex, so joins and ends are covered.
    pub fn stroke(&mut self, points: &[Point], closed: bool, width: f64, color: Color) {
        if points.len() < 2 || width <= 0.0 {
            return;
        }
        let mut path: Vec<Point> = points.to_vec();
        if closed && points.len() > 2 {
            path.push(points[0]);
        }
        let width = width.max(1.0);
        if width <= 1.0 {
            for segment in path.windows(2) {
                self.line(segment[0], segment[1], color);
            }
            return;
        }
        let half = width / 2.0;
        for segment in path.windows(2) {
            let (a, b) = (segment[0], segment[1]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let length = (dx * dx + dy * dy).sqrt();
            if length < 1e-9 {
                continue;
            }
            let (nx, ny) = (-dy / length * half, dx / length * half);
            let quad = vec![
                pt(a.x + nx, a.y + ny),
                pt(b.x + nx, b.y + ny),
                pt(b.x - nx, b.y - ny),
                pt(a.x - nx, a.y - ny),
            ];
            self.fill_polygons(&[quad], FillRule::Alternate, color);
        }
        let square_half = half;
        for vertex in &path {
            let square = vec![
                pt(vertex.x - square_half, vertex.y - square_half),
                pt(vertex.x + square_half, vertex.y - square_half),
                pt(vertex.x + square_half, vertex.y + square_half),
                pt(vertex.x - square_half, vertex.y + square_half),
            ];
            self.fill_polygons(&[square], FillRule::Alternate, color);
        }
    }

    /// A single-pixel line, walked so that no pixel is skipped between two
    /// points less than a pixel apart.
    pub fn line(&mut self, a: Point, b: Point, color: Color) {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as i32;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let x = (a.x + dx * t).floor() as i32;
            let y = (a.y + dy * t).floor() as i32;
            self.put_pixel(x, y, color);
        }
        self.drew = true;
    }

    /// Draw a bitmap into `dest`, taking the part named by `src_rect` and
    /// combining it with what is already there through `rop`.
    ///
    /// Sampling is nearest-neighbour: a metafile states a source rectangle and
    /// a destination rectangle and expects the pixels copied across, which is
    /// exactly what a stretch blit did.
    pub fn stretch_blit(&mut self, src: &Bitmap, src_rect: Rect, dest: Rect, rop: Rop) {
        let dest = dest.normalised();
        if dest.is_empty() || src.width == 0 || src.height == 0 {
            return;
        }
        let src_rect = src_rect.normalised();
        if src_rect.is_empty() {
            return;
        }
        let first_x = (dest.left.floor() as i32).max(self.clip.left);
        let last_x = (dest.right.ceil() as i32).min(self.clip.right);
        let first_y = (dest.top.floor() as i32).max(self.clip.top);
        let last_y = (dest.bottom.ceil() as i32).min(self.clip.bottom);
        for y in first_y..last_y {
            let v = (y as f64 + 0.5 - dest.top) / dest.height();
            let sy = src_rect.top + v * src_rect.height();
            for x in first_x..last_x {
                let u = (x as f64 + 0.5 - dest.left) / dest.width();
                let sx = src_rect.left + u * src_rect.width();
                let Some(sample) = src.color_at(sx.floor() as i32, sy.floor() as i32) else {
                    continue;
                };
                let out = match rop {
                    Rop::SrcCopy | Rop::PatCopy => sample,
                    Rop::Blackness => Color::BLACK,
                    Rop::Whiteness => Color::WHITE,
                    other => other.apply(sample, self.color_at(x, y)),
                };
                self.store(x, y, out);
            }
        }
    }
}
