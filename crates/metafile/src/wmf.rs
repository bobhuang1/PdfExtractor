//! Windows Metafile playback ([MS-WMF]).
//!
//! A WMF is the older of the two metafile formats: a header, then records of
//! 16-bit coordinates, which is why a drawing that has been through Word
//! several times comes out as one. The records are the same GDI calls the EMF
//! player handles, and the device context works the same way; what differs is
//! the file's own bookkeeping.
//!
//! Two things about that bookkeeping are worth knowing before reading the code.
//! The first is the *placeable* header: a WMF may begin with a bounding box and
//! a resolution (twips per inch) in front of the metafile proper, and that box
//! is the rectangle the drawing is meant to fill — it is the only statement of
//! the drawing's own size a WMF makes. The second is that the padding some
//! writers put after each record's size field makes a standard walk of the file
//! read nonsense: the chain is walked both ways and the layout that lands
//! exactly on the end-of-file record wins.

use crate::dib;
use crate::font::{self, TextAlign};
use crate::paint::{pt, Canvas, Clip, Color, FillRule, Point, Rect, Rop, Transform};
use crate::{canvas_size, Error, Options, Raster};
use std::collections::HashMap;

/// The key that begins a placeable WMF header.
const PLACEABLE_KEY: u32 = 0x9ac6_cdd7;

/// `PS_NULL`: a pen that draws nothing.
const PS_NULL: u32 = 5;
/// `BS_NULL`: a brush that fills nothing.
const BS_NULL: u32 = 1;

pub(crate) fn looks_like(data: &[u8]) -> bool {
    if data.len() >= 22 && read_u32(data, 0) == Some(PLACEABLE_KEY) {
        return true;
    }
    // A bare metafile starts with its `METAHEADER`: a memory or disk file, and
    // a nine-word header.
    data.len() >= 22
        && matches!(read_u16(data, 0), Some(1) | Some(2))
        && read_u16(data, 2) == Some(9)
}

/// Play a WMF into a raster.
pub(crate) fn play(data: &[u8], options: Options) -> Result<Raster, Error> {
    let header = Header::read(data)?;
    let (records, padded) = header.records(data).ok_or(Error::Malformed(
        "no walk of the record chain reaches the end of the file",
    ))?;
    let (width, height) = canvas_size(&options, header.size_pt())?;
    let mut player = Player::new(data, &header, width, height, padded);
    player.run(&records);
    if !player.canvas.drew() {
        return Err(Error::Undrawable("the record stream draws nothing"));
    }
    Ok(Raster {
        width,
        height,
        pixels: player.canvas.into_pixels(),
        unsupported: player.unsupported,
    })
}

/// Where the records start, how big the drawing says it is, and the box it is
/// meant to fill.
struct Header {
    /// Offset of the first record.
    start: usize,
    /// The rectangle the drawing occupies, in the metafile's own units.
    box_: Rect,
    /// Twips per inch, when a placeable header states one.
    twips_per_inch: Option<f64>,
    /// Whether a placeable header was found: without one the box comes from the
    /// window the records set, and is only known once they have been played.
    placeable: bool,
}

impl Header {
    fn read(data: &[u8]) -> Result<Header, Error> {
        if looks_like(data) && read_u32(data, 0) == Some(PLACEABLE_KEY) {
            if data.len() < 22 + 18 {
                return Err(Error::Malformed(
                    "a placeable header with no metafile after it",
                ));
            }
            let left = read_i16(data, 6).unwrap_or(0) as f64;
            let top = read_i16(data, 8).unwrap_or(0) as f64;
            let right = read_i16(data, 10).unwrap_or(0) as f64;
            let bottom = read_i16(data, 12).unwrap_or(0) as f64;
            let twips = read_u16(data, 14).unwrap_or(0);
            let header_words = read_u16(data, 24).unwrap_or(9) as usize;
            if header_words == 0 || header_words > 64 {
                return Err(Error::Malformed("a metafile header with no size"));
            }
            return Ok(Header {
                start: 22 + header_words * 2,
                box_: Rect::new(left, top, right, bottom).normalised(),
                twips_per_inch: (twips > 0).then_some(twips as f64),
                placeable: true,
            });
        }
        if !looks_like(data) {
            return Err(Error::NotAMetafile);
        }
        let header_words = read_u16(data, 2).unwrap_or(9) as usize;
        if header_words == 0 || header_words > 64 {
            return Err(Error::Malformed("a metafile header with no size"));
        }
        // Without a placeable header there is no declared box; the window the
        // records set stands in for it, and until they do the drawing has no
        // extent to rasterise.
        Ok(Header {
            start: header_words * 2,
            box_: Rect::new(0.0, 0.0, 1.0, 1.0),
            twips_per_inch: None,
            placeable: false,
        })
    }

    /// The drawing's own size in points, from its box and resolution.
    fn size_pt(&self) -> Option<(f64, f64)> {
        let per_inch = self.twips_per_inch?;
        if self.box_.is_empty() {
            return None;
        }
        let to_pt = 72.0 / per_inch;
        Some((self.box_.width() * to_pt, self.box_.height() * to_pt))
    }

    /// Walk the record chain, in either of the two layouts a writer may have
    /// used.
    ///
    /// A record is a size in words, a function, then its parameters. Some
    /// writers — Word and CorelDRAW among them — put a pad word after the size,
    /// so the function sits four bytes in rather than two. A walk is accepted
    /// only if it lands exactly on the end-of-file record at the end of the
    /// data, which no misaligned walk does by accident.
    fn records(&self, data: &[u8]) -> Option<(Vec<Record>, bool)> {
        // Both layouts are tried; the one that walks the file cleanly wins, and
        // if both do (which a small file can manage) the longer chain is the
        // one that actually read the records.
        let standard = walk(data, self.start, false);
        let padded = walk(data, self.start, true);
        match (standard, padded) {
            (Some(chain), None) if chain.len() > 1 => Some((chain, false)),
            (None, Some(chain)) if chain.len() > 1 => Some((chain, true)),
            (Some(standard), Some(padded)) => {
                if padded.len() > standard.len() {
                    Some((padded, true))
                } else {
                    Some((standard, false))
                }
            }
            _ => None,
        }
    }
}

/// Where one record of the chain begins, how long it is in bytes, and the GDI
/// function it names.
///
/// Offsets rather than borrowed slices: a record's parameters are 16-bit aligned
/// in the standard layout and 32-bit aligned in the padded one, and the player
/// reads them by offset so both come out the same.
type Record = (usize, usize, u16);

fn walk(data: &[u8], start: usize, padded: bool) -> Option<Vec<Record>> {
    let header = if padded { 6 } else { 4 };
    if start + header > data.len() {
        return None;
    }
    let mut offset = start;
    let mut out = Vec::new();
    while offset + header <= data.len() {
        let size = if padded {
            read_u32(data, offset)? as usize
        } else {
            read_u16(data, offset)? as usize
        };
        let function = read_u16(data, offset + header - 2)?;
        let bytes = size.checked_mul(2)?;
        if bytes < header || offset + bytes > data.len() {
            return None;
        }
        out.push((offset, bytes, function));
        if function == 0 {
            // The end-of-file record has to be the last one, and it has to end
            // exactly where the data does.
            return (offset + bytes == data.len() && out.len() > 1).then_some(out);
        }
        offset += bytes;
    }
    None
}

#[derive(Debug, Clone, Copy)]
struct Pen {
    style: u32,
    width: f64,
    color: Color,
}

#[derive(Debug, Clone, Copy)]
struct Brush {
    style: u32,
    color: Color,
}

#[derive(Debug, Clone)]
struct Font {
    height: f64,
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

/// A GDI object a WMF can create and select.
///
/// A region is not among them: every record that would make or use one is
/// reported rather than played (see the `META_*REGION` arms of the record
/// table), so there is no handle that could name one.
#[derive(Debug, Clone)]
enum Object {
    Pen(Pen),
    Brush(Brush),
    Font(Font),
}

#[derive(Debug, Clone)]
struct Dc {
    window_org: (f64, f64),
    window_ext: (f64, f64),
    viewport_org: (f64, f64),
    viewport_ext: (f64, f64),
    fill_rule: FillRule,
    align: TextAlign,
    text_color: Color,
    bk_color: Color,
    bk_transparent: bool,
    pen: Pen,
    brush: Brush,
    font: Font,
    current: Point,
}

struct Player<'a> {
    data: &'a [u8],
    canvas: Canvas,
    dc: Dc,
    saved: Vec<(Dc, Clip)>,
    objects: HashMap<u32, Object>,
    /// Device space to the canvas: the drawing's box mapped onto the raster.
    device_to_canvas: Transform,
    /// The box the drawing occupies, in its own units.
    box_: Rect,
    /// A placeable WMF states its box; a bare one has to take it from the
    /// window records, which move the device-space mapping with it.
    placeable: bool,
    /// Whether the records carry a pad word after their size, which moves where
    /// each record's parameters begin.
    padded: bool,
    unsupported: Vec<String>,
    noted: Vec<String>,
}

impl<'a> Player<'a> {
    fn new(data: &'a [u8], header: &Header, width: u32, height: u32, padded: bool) -> Player<'a> {
        let box_ = if header.placeable {
            header.box_
        } else {
            Rect::new(0.0, 0.0, 1.0, 1.0)
        };
        let device_to_canvas = Transform::translate(-box_.left, -box_.top).then(&Transform::scale(
            width as f64 / box_.width().max(1.0),
            height as f64 / box_.height().max(1.0),
        ));
        Player {
            data,
            canvas: Canvas::new(width, height),
            dc: Dc {
                // The window and viewport an unplayed WMF starts with: the
                // window is the unit square, and the viewport is the drawing's
                // own box — the rectangle a WMF is played into.
                window_org: (0.0, 0.0),
                window_ext: (1.0, 1.0),
                viewport_org: (box_.left, box_.top),
                viewport_ext: (box_.width().max(1.0), box_.height().max(1.0)),
                fill_rule: FillRule::Alternate,
                align: TextAlign::default(),
                text_color: Color::BLACK,
                bk_color: Color::WHITE,
                bk_transparent: false,
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
            },
            saved: Vec::new(),
            objects: HashMap::new(),
            device_to_canvas,
            box_,
            placeable: header.placeable,
            padded,
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

    fn to_canvas(&self) -> Transform {
        let sx = nonzero(self.dc.viewport_ext.0) / nonzero(self.dc.window_ext.0);
        let sy = nonzero(self.dc.viewport_ext.1) / nonzero(self.dc.window_ext.1);
        Transform::new(
            sx,
            0.0,
            0.0,
            sy,
            -self.dc.window_org.0 * sx + self.dc.viewport_org.0,
            -self.dc.window_org.1 * sy + self.dc.viewport_org.1,
        )
        .then(&self.device_to_canvas)
    }

    fn run(&mut self, records: &[Record]) {
        for &(offset, bytes, function) in records {
            let record = &self.data[offset..offset + bytes];
            // The parameters start after the size and function — four bytes in,
            // or six when the writer put a pad word after the size.
            let header = if self.padded { 6 } else { 4 };
            let params = record.get(header..).unwrap_or(&[]);
            self.record(function, params);
            if function == 0 {
                return;
            }
        }
    }

    fn record(&mut self, function: u16, p: &[u8]) {
        match function {
            0x0000 => {}
            0x001e => self.saved.push((self.dc.clone(), self.canvas.clip())),
            0x0035 | 0x0037 | 0x00f7 | 0x0139 | 0x0231 | 0x0234 | 0x0436 | 0x012e | 0x0107
            | 0x0108 | 0x0103 => self.set_simple(function, p),
            0x0102 => self.dc.bk_transparent = read_u16(p, 0) == Some(1),
            0x0104 => self.set_rop2(p),
            0x0106 => {
                self.dc.fill_rule = match read_u16(p, 0) {
                    Some(2) => FillRule::Winding,
                    _ => FillRule::Alternate,
                }
            }
            0x0127 => {
                let steps = read_i16(p, 0).unwrap_or(-1);
                let steps = if steps < 0 {
                    (-steps).max(1) as usize
                } else {
                    1
                };
                for _ in 0..steps {
                    match self.saved.pop() {
                        Some((dc, clip)) => {
                            self.dc = dc;
                            self.canvas.set_clip(clip);
                        }
                        None => self.dc = self.initial_dc(),
                    }
                }
            }
            0x012d => self.select_object(read_u16(p, 0).unwrap_or(0) as u32),
            0x01f0 => {
                self.objects.remove(&(read_u16(p, 0).unwrap_or(0) as u32));
            }
            0x0201 => self.dc.bk_color = read_colorref(p, 0),
            0x0209 => self.dc.text_color = read_colorref(p, 0),
            0x020b => self.set_window_org(p),
            0x020c => self.set_window_ext(p),
            0x020d => self.set_viewport_org(p),
            0x020e => self.set_viewport_ext(p),
            0x020f | 0x0211 => self.offset_org(function, p),
            0x0213 => self.line_to(p),
            0x0214 => {
                let (Some(x), Some(y)) = (read_i16(p, 2), read_i16(p, 0)) else {
                    return;
                };
                self.dc.current = pt(x as f64, y as f64);
            }
            0x02fa => self.create_pen(p),
            0x02fb => self.create_font(p),
            0x02fc => self.create_brush(p),
            0x0324 | 0x0325 => self.polygon(function, p),
            0x0538 => self.poly_polygon(p),
            0x041b | 0x0418 | 0x061c => self.shape(function, p),
            0x0817 | 0x081a | 0x0830 => self.arc(function, p),
            0x041f => {
                let (Some(y), Some(x)) = (read_i16(p, 2), read_i16(p, 4)) else {
                    return;
                };
                let color = read_colorref(p, 0);
                let point = self.to_canvas().apply(pt(x as f64, y as f64));
                self.canvas
                    .put_pixel(point.x.floor() as i32, point.y.floor() as i32, color);
            }
            0x0416 => {
                if let Some(rect) = read_box(p, 0) {
                    self.canvas.intersect_clip(self.transform_rect(rect));
                }
            }
            0x0415 => self.note("META_EXCLUDECLIPRECT"),
            0x012c => self.note("META_SELECTCLIPREGION"),
            0x0142 => self.note("META_DIBCREATEPATTERNBRUSH (a patterned brush)"),
            0x01f9 => self.note("META_CREATEPATTERNBRUSH (a patterned brush)"),
            0x0521 => self.text_out(p),
            0x0a32 => self.ext_text_out(p),
            0x0b41 | 0x0940 | 0x0f43 => self.dib_blt(function, p),
            0x061d => self.pat_blt(p),
            0x0922 | 0x0b23 => self.note("META_BITBLT (a block of pixels taken from a device)"),
            0x0228 | 0x0429 | 0x012a | 0x012b | 0x06ff => self.note("a region fill"),
            0x0419 | 0x0548 => self.note("a flood fill"),
            0x0d33 => self.note("META_SETDIBTODEV"),
            0x0626 => self.note("META_ESCAPE"),
            other => {
                let name = format!("unimplemented WMF record {other:#06x}");
                self.note(&name);
            }
        }
    }

    fn initial_dc(&self) -> Dc {
        Dc {
            window_org: (0.0, 0.0),
            window_ext: (1.0, 1.0),
            viewport_org: (self.box_.left, self.box_.top),
            viewport_ext: (self.box_.width().max(1.0), self.box_.height().max(1.0)),
            fill_rule: FillRule::Alternate,
            align: TextAlign::default(),
            text_color: Color::BLACK,
            bk_color: Color::WHITE,
            bk_transparent: false,
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

    /// The records whose effect on the drawing is nothing: they set up palettes,
    /// justification or a mapping mode, and none of them changes where a line
    /// goes or what colour it is.
    fn set_simple(&mut self, function: u16, p: &[u8]) {
        match function {
            0x012e => self.dc.align = decode_align(read_u16(p, 0).unwrap_or(0) as u32),
            // `META_SETMAPMODE`: coordinates are mapped from the window and viewport
            // extents, which is MM_TEXT (1) and MM_ANISOTROPIC (8); any other mode
            // states a unit this player does not convert, so it is reported.
            0x0103 => match read_u16(p, 0) {
                Some(1) | Some(8) => {}
                _ => self.note("META_SETMAPMODE (a mapping mode other than MM_TEXT)"),
            },
            _ => {}
        }
    }

    /// `SetROP2` sets how ink combines with the surface, as in the EMF player.
    fn set_rop2(&mut self, p: &[u8]) {
        match read_u16(p, 0) {
            Some(1) => self.canvas.set_rop(Rop::Blackness),
            Some(4) => self.canvas.set_rop(Rop::NotSrcCopy),
            Some(6) => self.canvas.set_rop(Rop::DstInvert),
            Some(11) => self.canvas.set_rop(Rop::Blackness),
            Some(13) | None => self.canvas.set_rop(Rop::SrcCopy),
            Some(16) => self.canvas.set_rop(Rop::Whiteness),
            _ => {
                self.canvas.set_rop(Rop::SrcCopy);
                self.note("META_SETROP2 (a mix mode that needs the pen as well as the surface)");
            }
        }
    }

    fn set_window_org(&mut self, p: &[u8]) {
        let (Some(y), Some(x)) = (read_i16(p, 0), read_i16(p, 2)) else {
            return;
        };
        self.dc.window_org = (x as f64, y as f64);
        self.follow_window();
    }

    fn set_window_ext(&mut self, p: &[u8]) {
        let (Some(y), Some(x)) = (read_i16(p, 0), read_i16(p, 2)) else {
            return;
        };
        self.dc.window_ext = (x as f64, y as f64);
        self.follow_window();
    }

    fn set_viewport_org(&mut self, p: &[u8]) {
        let (Some(y), Some(x)) = (read_i16(p, 0), read_i16(p, 2)) else {
            return;
        };
        self.dc.viewport_org = (x as f64, y as f64);
    }

    fn set_viewport_ext(&mut self, p: &[u8]) {
        let (Some(y), Some(x)) = (read_i16(p, 0), read_i16(p, 2)) else {
            return;
        };
        self.dc.viewport_ext = (x as f64, y as f64);
    }

    fn offset_org(&mut self, function: u16, p: &[u8]) {
        let (Some(y), Some(x)) = (read_i16(p, 0), read_i16(p, 2)) else {
            return;
        };
        if function == 0x020f {
            self.dc.window_org.0 += x as f64;
            self.dc.window_org.1 += y as f64;
            self.follow_window();
        } else {
            self.dc.viewport_org.0 += x as f64;
            self.dc.viewport_org.1 += y as f64;
        }
    }

    /// A WMF with no placeable header declares its size by setting a window; the
    /// rectangle it draws into has to follow, or the drawing lands outside the
    /// raster.
    fn follow_window(&mut self) {
        if self.placeable {
            return;
        }
        let box_ = Rect::new(
            self.dc.window_org.0,
            self.dc.window_org.1,
            self.dc.window_org.0 + self.dc.window_ext.0,
            self.dc.window_org.1 + self.dc.window_ext.1,
        )
        .normalised();
        if box_.is_empty() {
            return;
        }
        self.box_ = box_;
        self.dc.viewport_org = (box_.left, box_.top);
        self.dc.viewport_ext = (box_.width(), box_.height());
        self.device_to_canvas =
            Transform::translate(-box_.left, -box_.top).then(&Transform::scale(
                self.canvas.width() as f64 / box_.width(),
                self.canvas.height() as f64 / box_.height(),
            ));
    }

    fn transform_rect(&self, rect: Rect) -> Rect {
        let transform = self.to_canvas();
        let (corner, opposite) = (
            transform.apply(pt(rect.left, rect.top)),
            transform.apply(pt(rect.right, rect.bottom)),
        );
        Rect::new(corner.x, corner.y, opposite.x, opposite.y).normalised()
    }

    // --- objects ----------------------------------------------------------

    fn select_object(&mut self, handle: u32) {
        match self.objects.get(&handle).cloned() {
            Some(Object::Pen(pen)) => self.dc.pen = pen,
            Some(Object::Brush(brush)) => self.dc.brush = brush,
            Some(Object::Font(font)) => self.dc.font = font,
            None => self.select_stock(handle),
        }
    }

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

    fn create_pen(&mut self, p: &[u8]) {
        let style = read_u16(p, 0).unwrap_or(0) as u32;
        let width = read_i16(p, 2).unwrap_or(1) as f64;
        let color = read_colorref(p, 6);
        self.objects.insert(
            self.pending_handle(),
            Object::Pen(Pen {
                style,
                width: if width.abs() < 1.0 { 1.0 } else { width.abs() },
                color,
            }),
        );
    }

    fn create_brush(&mut self, p: &[u8]) {
        let style = read_u16(p, 0).unwrap_or(0) as u32;
        let color = read_colorref(p, 2);
        let hatch = read_u16(p, 6).unwrap_or(0);
        if style > 1 || (style == 1 && hatch > 8) {
            self.note("a hatched brush (drawn with its colour)");
        }
        self.objects.insert(
            self.pending_handle(),
            Object::Brush(Brush {
                style: if style == 1 || style == 3 { BS_NULL } else { 0 },
                color,
            }),
        );
    }

    fn create_font(&mut self, p: &[u8]) {
        let height = read_i16(p, 0).unwrap_or(-12) as f64;
        let escapement = read_i16(p, 4).unwrap_or(0) as f64;
        self.objects.insert(
            self.pending_handle(),
            Object::Font(Font {
                height: if height == 0.0 { -12.0 } else { height },
                escapement,
            }),
        );
    }

    /// A WMF's `CreatePen` and friends have no handle in the record: the
    /// handle is the next free slot in the object table, which is what
    /// `SelectObject` then refers to.
    fn pending_handle(&self) -> u32 {
        (0u32..)
            .find(|candidate| !self.objects.contains_key(candidate))
            .unwrap_or(1)
    }

    // --- drawing ----------------------------------------------------------

    fn line_to(&mut self, p: &[u8]) {
        let (Some(y), Some(x)) = (read_i16(p, 0), read_i16(p, 2)) else {
            return;
        };
        let from = self.dc.current;
        let to = pt(x as f64, y as f64);
        self.stroke_contour(&[from, to], false);
        self.dc.current = to;
    }

    fn polygon(&mut self, function: u16, p: &[u8]) {
        let Some(count) = read_u16(p, 0).map(|count| count as usize) else {
            return;
        };
        let mut points = Vec::with_capacity(count);
        for i in 0..count {
            let at = 2 + i * 4;
            // The points of a `POLYGON` are `POINT` structures, which write x
            // before y — unlike `MOVETO`/`LINETO`, whose two words are y then x.
            let (Some(x), Some(y)) = (read_i16(p, at), read_i16(p, at + 2)) else {
                return;
            };
            points.push(pt(x as f64, y as f64));
        }
        let filled = function == 0x0324;
        if filled {
            self.fill_and_stroke(&[points]);
        } else {
            self.stroke_contour(&points, false);
        }
    }

    fn poly_polygon(&mut self, p: &[u8]) {
        let Some(count) = read_u16(p, 0).map(|count| count as usize) else {
            return;
        };
        let mut counts = Vec::with_capacity(count);
        for i in 0..count {
            let Some(count) = read_u16(p, 2 + i * 2) else {
                return;
            };
            counts.push(count as usize);
        }
        let mut at = 2 + count * 2;
        let mut contours = Vec::with_capacity(count);
        for count in counts {
            let mut contour = Vec::with_capacity(count);
            for _ in 0..count {
                // A `POINT`, x before y, as in `POLYGON`.
                let (Some(x), Some(y)) = (read_i16(p, at), read_i16(p, at + 2)) else {
                    return;
                };
                contour.push(pt(x as f64, y as f64));
                at += 4;
            }
            contours.push(contour);
        }
        self.fill_and_stroke(&contours);
    }

    fn shape(&mut self, function: u16, p: &[u8]) {
        // A round rectangle writes its corner size before the rectangle.
        let at = if function == 0x061c { 4 } else { 0 };
        let Some(rect) = read_box(p, at) else { return };
        let rect = rect.normalised();
        let contour = match function {
            0x041b => vec![
                pt(rect.left, rect.top),
                pt(rect.right, rect.top),
                pt(rect.right, rect.bottom),
                pt(rect.left, rect.bottom),
            ],
            0x0418 => ellipse_contour(rect, 64),
            _ => {
                let (Some(height), Some(width)) = (read_i16(p, 0), read_i16(p, 2)) else {
                    return;
                };
                round_rect_contour(rect, width.abs() as f64, height.abs() as f64)
            }
        };
        self.fill_and_stroke(&[contour]);
    }

    fn arc(&mut self, function: u16, p: &[u8]) {
        let (Some(y_end), Some(x_end), Some(y_start), Some(x_start)) = (
            read_i16(p, 0),
            read_i16(p, 2),
            read_i16(p, 4),
            read_i16(p, 6),
        ) else {
            return;
        };
        let Some(rect) = read_box(p, 8) else { return };
        let rect = rect.normalised();
        let kind = match function {
            0x0817 => 45,
            0x0830 => 46,
            _ => 47,
        };
        let contour = arc_contour(
            rect,
            pt(x_start as f64, y_start as f64),
            pt(x_end as f64, y_end as f64),
            kind,
        );
        if kind == 45 {
            self.stroke_contour(&contour, false);
        } else {
            self.fill_and_stroke(&[contour]);
        }
    }

    /// Fill with the brush and outline with the pen, as the GDI shapes and
    /// polygons do.
    fn fill_and_stroke(&mut self, contours: &[Vec<Point>]) {
        let transform = self.to_canvas();
        let canvas_contours: Vec<Vec<Point>> = contours
            .iter()
            .map(|contour| contour.iter().map(|p| transform.apply(*p)).collect())
            .collect();
        if self.dc.brush.style != BS_NULL {
            self.canvas
                .fill_polygons(&canvas_contours, self.dc.fill_rule, self.dc.brush.color);
        }
        if self.dc.pen.style != PS_NULL {
            let width = self.dc.pen.width * transform.scale_of().1;
            for contour in &canvas_contours {
                self.canvas.stroke(contour, true, width, self.dc.pen.color);
            }
        }
    }

    fn stroke_contour(&mut self, contour: &[Point], closed: bool) {
        let transform = self.to_canvas();
        let points: Vec<Point> = contour.iter().map(|p| transform.apply(*p)).collect();
        if self.dc.pen.style == PS_NULL {
            return;
        }
        let width = self.dc.pen.width * transform.scale_of().1;
        self.canvas
            .stroke(&points, closed, width, self.dc.pen.color);
    }

    // --- text -------------------------------------------------------------

    fn text_out(&mut self, p: &[u8]) {
        let Some(count) = read_u16(p, 0).map(|count| count as usize) else {
            return;
        };
        let Some(bytes) = p.get(2..2 + count) else {
            return;
        };
        let text: String = bytes.iter().map(|byte| *byte as char).collect();
        // The reference point follows the string, padded to an even length.
        let at = 2 + count + (count % 2);
        let (Some(y), Some(x)) = (read_i16(p, at), read_i16(p, at + 2)) else {
            return;
        };
        let align = self.dc.align;
        self.draw_text(&text, pt(x as f64, y as f64), align);
    }

    fn ext_text_out(&mut self, p: &[u8]) {
        let (Some(y), Some(x), Some(count), Some(options)) = (
            read_i16(p, 0),
            read_i16(p, 2),
            read_i16(p, 4),
            read_i16(p, 6),
        ) else {
            return;
        };
        let count = count.max(0) as usize;
        let Some(bytes) = p.get(8..8 + count) else {
            return;
        };
        let text: String = bytes.iter().map(|byte| *byte as char).collect();
        // A rectangle may follow the string, optionally after the per-character
        // advances; when the record asks for clipping, that rectangle bounds the
        // text.
        let remaining = p.len().saturating_sub(8 + count);
        let clipped = options & 0x0004 != 0 && remaining >= 8;
        let saved = self.canvas.clip();
        if clipped {
            let at = p.len() - 8;
            if let Some(rect) = read_box(p, at) {
                let rect = self.transform_rect(rect.normalised());
                self.canvas.intersect_clip(rect);
            }
        }
        let align = self.dc.align;
        self.draw_text(&text, pt(x as f64, y as f64), align);
        if clipped {
            self.canvas.set_clip(saved);
        }
    }

    fn draw_text(&mut self, text: &str, reference: Point, align: TextAlign) {
        let transform = self.to_canvas();
        let (_, scale_y) = transform.scale_of();
        let cell = self.dc.font.height.abs() * scale_y;
        if cell <= 0.5 {
            return;
        }
        let background = (!self.dc.bk_transparent).then_some(self.dc.bk_color);
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

    // --- bitmaps ----------------------------------------------------------

    /// `DIBStretchBlt`, `DIBBitBlt` and `StretchDIB`: a bitmap inside the
    /// record, at rectangles given in the drawing's own coordinates.
    fn dib_blt(&mut self, function: u16, p: &[u8]) {
        // `StretchDIB` inserts a usage word before the source size.
        let base = if function == 0x0f43 { 2 } else { 0 };
        let Some(rop) = read_u32(p, 0).and_then(Rop::from_code) else {
            self.note("a bitmap blit with a raster operation it cannot combine");
            return;
        };
        let numbers: Vec<Option<i16>> = (0..8).map(|i| read_i16(p, 4 + base + i * 2)).collect();
        let values: Vec<f64> = numbers
            .iter()
            .map(|value| value.unwrap_or(0) as f64)
            .collect();
        let dib_at = 4 + base + 16;
        let bitmap = match dib::decode(p.get(dib_at..).unwrap_or(&[])) {
            Ok(bitmap) => bitmap,
            Err(reason) => {
                self.note(&format!("a bitmap blit with {reason}"));
                return;
            }
        };
        let source = Rect::new(
            values[3],
            values[2],
            values[3] + values[1],
            values[2] + values[0],
        );
        let destination = Rect::new(
            values[7],
            values[6],
            values[7] + values[5],
            values[6] + values[4],
        );
        let transform = self.to_canvas();
        let (corner, opposite) = (
            transform.apply(pt(destination.left, destination.top)),
            transform.apply(pt(destination.right, destination.bottom)),
        );
        let rect = Rect::new(corner.x, corner.y, opposite.x, opposite.y);
        self.canvas.stretch_blit(&bitmap, source, rect, rop);
    }

    /// `PatBlt`: the brush over a rectangle through a raster operation.
    fn pat_blt(&mut self, p: &[u8]) {
        let Some(rop) = read_u32(p, 0).and_then(Rop::from_code) else {
            self.note("META_PATBLT with a raster operation it cannot combine");
            return;
        };
        let (Some(height), Some(width), Some(y), Some(x)) = (
            read_i16(p, 4),
            read_i16(p, 6),
            read_i16(p, 8),
            read_i16(p, 10),
        ) else {
            return;
        };
        let rect = Rect::new(
            x as f64,
            y as f64,
            x as f64 + width as f64,
            y as f64 + height as f64,
        );
        let rect = self.transform_rect(rect);
        self.canvas.fill_rect_rop(rect, self.dc.brush.color, rop);
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
            2 => crate::font::Horizontal::Right,
            6 => crate::font::Horizontal::Center,
            _ => crate::font::Horizontal::Left,
        },
        vertical: match value & 0x18 {
            8 => crate::font::Vertical::Bottom,
            24 => crate::font::Vertical::Baseline,
            _ => crate::font::Vertical::Top,
        },
    }
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    data.get(at..at + 2)
        .map(|bytes| u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_i16(data: &[u8], at: usize) -> Option<i16> {
    data.get(at..at + 2)
        .map(|bytes| i16::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_colorref(data: &[u8], at: usize) -> Color {
    Color::from_colorref(read_u32(data, at).unwrap_or(0))
}

/// A rectangle in the rectangle-most WMF shape: bottom, right, top, left.
fn read_box(data: &[u8], at: usize) -> Option<Rect> {
    let bottom = read_i16(data, at)? as f64;
    let right = read_i16(data, at + 2)? as f64;
    let top = read_i16(data, at + 4)? as f64;
    let left = read_i16(data, at + 6)? as f64;
    Some(Rect::new(left, top, right, bottom))
}

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
        46 => points.push(points[0]),
        47 => {
            points.insert(0, pt(cx, cy));
            points.push(pt(cx, cy));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Both layouts of the record chain must be recognised: a file whose size
    /// field is followed by a pad word is the one Word writes, and a walk that
    /// assumed the standard layout would read the file as noise.
    #[test]
    fn both_record_layouts_are_walked() {
        // A standard chain: SETBKMODE, then end of file.
        let mut standard = Vec::new();
        standard.extend_from_slice(&5u16.to_le_bytes());
        standard.extend_from_slice(&0x0102u16.to_le_bytes());
        standard.extend_from_slice(&[1, 0, 0, 0, 0, 0]);
        standard.extend_from_slice(&3u16.to_le_bytes());
        standard.extend_from_slice(&0u16.to_le_bytes());
        standard.extend_from_slice(&[0, 0]);
        let bare = walk(&standard, 0, false).expect("standard layout");
        assert_eq!(bare.len(), 2);
        assert_eq!(bare[0].2, 0x0102);
        assert!(walk(&standard, 0, true).is_none(), "not padded");

        // The same chain with a pad word after each size.
        let mut padded = Vec::new();
        padded.extend_from_slice(&5u32.to_le_bytes());
        padded.extend_from_slice(&0x0102u16.to_le_bytes());
        padded.extend_from_slice(&[1, 0, 0, 0]);
        padded.extend_from_slice(&3u32.to_le_bytes());
        padded.extend_from_slice(&0u16.to_le_bytes());
        let with_pad = walk(&padded, 0, true).expect("padded layout");
        assert_eq!(with_pad.len(), 2);
        assert_eq!(with_pad[0].2, 0x0102);
        assert!(walk(&padded, 0, false).is_none(), "not standard");
    }
}
