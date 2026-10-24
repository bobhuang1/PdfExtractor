//! The parts of a shape drawing that both shape languages share.
//!
//! A Word file states a drawing in one of two languages — VML (`v:shape`,
//! `w:pict`) or DrawingML (`wps:wsp`, `w:drawing`) — but both are painted the
//! same way: a small XML tree is read, the shapes are reduced to filled polygons,
//! strokes and strings in points, and those are drawn onto a canvas at 144 dpi.
//! [`vml`](crate::vml) and [`drawingml`](crate::drawingml) differ in how they
//! read their markup, not in how the ink reaches a PNG, so the frame arithmetic,
//! the op list, the shape outline helpers and the rasteriser live here.
//!
//! What is drawn is the *drawing*, never the words in it: a shape's text is the
//! document's own text, reported where it stands, and painting it again would
//! put the same words in the document twice. The one exception is a VML
//! WordArt, whose string lives in an attribute no text item ever sees.
//!
//! The canvas is the union of the drawn frames at 144 dpi, which is two device
//! pixels per point — the resolution the metafiles are played at. A drawing that
//! puts no ink down is no picture rather than a blank one.

use pdfextractor_metafile as metafile;
use pdfextractor_metafile::Color;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// The resolution a drawing is rasterised at: the metafiles' 144 dpi, which is
/// two device pixels per point.
pub const DPI: f64 = metafile::DEFAULT_DPI;

/// The largest canvas we will allocate, in pixels on either side and in all.
const MAX_SIDE: u32 = 8192;
const MAX_AREA: u64 = 16 * 1024 * 1024;

/// An element, reduced to what a drawing needs: its name, its attributes
/// (prefixes stripped) and its children.
#[derive(Debug, Default)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Whether a boolean attribute says yes. VML writes `t` for true and `f`
    /// for false; an attribute that is absent is not an answer.
    pub fn is(&self, name: &str) -> Option<bool> {
        self.attr(name)
            .map(|value| value.trim().eq_ignore_ascii_case("t"))
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The first child with one of `names`, in document order.
    pub fn child_of(&self, names: &[&str]) -> Option<&Node> {
        self.children
            .iter()
            .find(|child| names.contains(&child.name.as_str()))
    }
}

/// Read one XML part into a tree, or `None` when it does not parse.
pub fn parse(xml: &[u8]) -> Option<Node> {
    let mut reader = Reader::from_reader(std::io::Cursor::new(xml));
    reader.config_mut().trim_text(true);
    let mut root: Option<Node> = None;
    let mut stack: Vec<Node> = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => stack.push(element(&e)),
            Ok(Event::Empty(e)) => {
                let node = element(&e);
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Ok(Event::End(_)) => {
                let node = stack.pop()?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                tracing::debug!(
                    "stopping a shape parse at byte {}: {err}",
                    reader.buffer_position()
                );
                return None;
            }
            _ => {}
        }
        buf.clear();
    }
    root
}

fn element(e: &BytesStart) -> Node {
    let mut node = Node {
        name: local_name(e.name().as_ref()).to_string(),
        ..Default::default()
    };
    for attribute in e.attributes().flatten() {
        let Ok(value) = attribute.unescape_value() else {
            continue;
        };
        node.attrs.push((
            local_name(attribute.key.as_ref()).to_string(),
            value.into_owned(),
        ));
    }
    node
}

/// Strip a namespace prefix: `v:imagedata` -> `imagedata`.
pub fn local_name(name: &[u8]) -> &str {
    let name = match name.iter().rposition(|b| *b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    };
    std::str::from_utf8(name).unwrap_or_default()
}

/// A rectangle in the drawing's own space: points, top-left origin.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    pub fn right(&self) -> f64 {
        self.left + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.top + self.height
    }

    /// This frame grown to hold `other`.
    pub fn union(self, other: Frame) -> Frame {
        let left = self.left.min(other.left);
        let top = self.top.min(other.top);
        Frame {
            left,
            top,
            width: self.right().max(other.right()) - left,
            height: self.bottom().max(other.bottom()) - top,
        }
    }

    pub fn center(&self) -> (f64, f64) {
        (self.left + self.width / 2.0, self.top + self.height / 2.0)
    }
}

/// How a group's coordinate space maps onto the drawing: `point = origin +
/// (p - coordorigin) * scale`.
#[derive(Debug, Clone, Copy)]
pub struct Space {
    pub sx: f64,
    pub sy: f64,
    pub ox: f64,
    pub oy: f64,
    pub dx: f64,
    pub dy: f64,
}

impl Space {
    /// The space a drawing's top level is in: its own points.
    pub const POINTS: Space = Space {
        sx: 1.0,
        sy: 1.0,
        ox: 0.0,
        oy: 0.0,
        dx: 0.0,
        dy: 0.0,
    };

    pub fn point(&self, x: f64, y: f64) -> metafile::Point {
        metafile::pt(
            self.dx + (x - self.ox) * self.sx,
            self.dy + (y - self.oy) * self.sy,
        )
    }
}

/// A picture a shape's fill paints with: its pixels, and the part of them the
/// fill shows.
///
/// The painter draws geometry, so a fill's media arrives already decoded — the
/// reader that holds the package resolves the relationship and hands the pixels
/// over, and what is done with them here is drawing, not decoding.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// RGBA8, straight (not premultiplied) alpha, row-major from the top left.
    pub pixels: Vec<u8>,
    /// The crop `a:srcRect` takes off the picture, as a fraction of it: left,
    /// top, right, bottom. A fill with no `a:srcRect` shows all of it.
    pub crop: [f64; 4],
}

impl Image {
    /// The pixel at `u`, `v` in the part of the picture the fill shows: 0 at the
    /// picture's start, 1 at its end.
    fn at(&self, u: f64, v: f64) -> Option<[u8; 4]> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let [left, top, right, bottom] = self.crop;
        let span_u = (1.0 - left - right).max(0.0);
        let span_v = (1.0 - top - bottom).max(0.0);
        if span_u <= 0.0 || span_v <= 0.0 {
            return None;
        }
        let u = (left + u.clamp(0.0, 1.0) * span_u).clamp(0.0, 1.0);
        let v = (top + v.clamp(0.0, 1.0) * span_v).clamp(0.0, 1.0);
        let x = ((u * f64::from(self.width)) as usize).min(self.width as usize - 1);
        let y = ((v * f64::from(self.height)) as usize).min(self.height as usize - 1);
        let at = (y * self.width as usize + x) * 4;
        Some([
            *self.pixels.get(at)?,
            *self.pixels.get(at + 1)?,
            *self.pixels.get(at + 2)?,
            *self.pixels.get(at + 3)?,
        ])
    }
}

/// One thing to paint, in the drawing's points.
#[derive(Debug)]
pub enum Op {
    Fill {
        contours: Vec<Vec<metafile::Point>>,
        color: Color,
    },
    /// A fill whose colour depends on where it lands: the ramp a `gradFill`
    /// states, painted across the shape it fills.
    GradientFill {
        contours: Vec<Vec<metafile::Point>>,
        gradient: Gradient,
    },
    /// A soft shadow under a shape: its silhouette, moved by `offset`, blurred
    /// by `blur` and laid down at `alpha` before the shape itself is painted.
    Shadow {
        contours: Vec<Vec<metafile::Point>>,
        offset: (f64, f64),
        blur: f64,
        color: Color,
        alpha: f64,
    },
    /// A picture painted through a shape's geometry: `contours` is the clip —
    /// the shape's own outline — and `quad` is the box the picture is stretched
    /// across, its four corners in the drawing's points (top-left, top-right,
    /// bottom-right, bottom-left) after the shape's mirror and turn.
    Picture {
        contours: Vec<Vec<metafile::Point>>,
        image: Image,
        quad: [metafile::Point; 4],
    },
    Stroke {
        points: Vec<metafile::Point>,
        closed: bool,
        width: f64,
        color: Color,
    },
    Text {
        text: String,
        frame: Frame,
        color: Color,
        /// Whether the string is stretched across the shape (WordArt's
        /// `fitshape`), or drawn at the size it declares and centred.
        stretch: bool,
        font_pt: f64,
    },
}

/// Where a gradient runs, in the drawing's points.
#[derive(Debug, Clone, Copy)]
pub enum GradientKind {
    /// A ramp along `angle` degrees, clockwise from the x axis and measured
    /// across `frame`: the frame's middle is the middle of the ramp, and the
    /// two ends of it are the ramp's ends however the shape is drawn inside it.
    ///
    /// `scaled` is DrawingML's `a:lin@scaled`, which is what an application
    /// writes for a shape's own fill: with it the angle is taken in the shape's
    /// own proportions, so 45° on a wide box runs corner to corner and looks
    /// shallow; without it the ramp runs at the true angle and the geometry
    /// inside the box merely clips it.
    Linear {
        frame: Frame,
        angle: f64,
        scaled: bool,
    },
    /// A ramp out from an inner ellipse at `center`, reaching the last stop at
    /// `radius` — counted from the centre, so a wide shape fades wider than it
    /// does tall. DrawingML's `a:path` (`circle`, `rect`, `shape`) is this, from
    /// the middle of the shape.
    ///
    /// `focus` is the radii of the ellipse the first stop fills, in the same
    /// points as `radius`: a radial ramp can start from a region rather than
    /// from a point, which is what VML's `focussize` states and what makes its
    /// inner colour flat instead of a pinprick. Zero radii — the default and
    /// DrawingML's — put the whole ramp between the centre and `radius`.
    Radial {
        center: (f64, f64),
        radius: (f64, f64),
        focus: (f64, f64),
    },
}

/// One stop of a gradient: where along the ramp it sits, the colour there, and
/// how opaque it is.
#[derive(Debug, Clone, Copy)]
pub struct GradientStop {
    /// 0 at the ramp's start, 1 at its end.
    pub position: f64,
    pub color: Color,
    /// The stop's own opacity; a gradient can fade a shape out.
    pub alpha: f64,
}

/// A gradient fill: its stops in ramp order, and the geometry they run across.
#[derive(Debug)]
pub struct Gradient {
    pub stops: Vec<GradientStop>,
    pub kind: GradientKind,
}

impl Gradient {
    /// The colour at a point of the drawing, and its opacity.
    ///
    /// The ramp position is interpolated between the two stops it falls
    /// between, and the ends are the end stops — a point past the last stop is
    /// the last stop's colour, not the canvas'.
    pub fn at(&self, point: metafile::Point) -> (Color, f64) {
        let t = self.position(point);
        let first = match self.stops.first() {
            Some(first) => first,
            None => return (Color::WHITE, 0.0),
        };
        if t <= first.position {
            return (first.color, first.alpha);
        }
        let mut previous = first;
        for stop in &self.stops[1..] {
            if t <= stop.position {
                let span = stop.position - previous.position;
                let share = if span > 0.0 {
                    ((t - previous.position) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return (
                    mix(previous.color, stop.color, share),
                    previous.alpha + (stop.alpha - previous.alpha) * share,
                );
            }
            previous = stop;
        }
        (previous.color, previous.alpha)
    }

    /// How far along the ramp a point sits, before the stops are consulted.
    fn position(&self, point: metafile::Point) -> f64 {
        match self.kind {
            GradientKind::Linear {
                frame,
                angle,
                scaled,
            } => {
                let (cx, cy) = frame.center();
                let (dx, dy) = (angle.to_radians().cos(), angle.to_radians().sin());
                let (half_x, half_y) = (frame.width / 2.0, frame.height / 2.0);
                // How far the point is along the ramp, and how far the box
                // reaches that way. Both are measured in the same space, so the
                // ramp starts and ends on the box's edges: the box's own
                // proportions for a scaled ramp, the drawing's points for one
                // that states its true angle.
                let (along, reach) = if scaled {
                    let (half_x, half_y) = (half_x.max(1e-6), half_y.max(1e-6));
                    (
                        (point.x - cx) / half_x * dx + (point.y - cy) / half_y * dy,
                        dx.abs() + dy.abs(),
                    )
                } else {
                    (
                        (point.x - cx) * dx + (point.y - cy) * dy,
                        half_x * dx.abs() + half_y * dy.abs(),
                    )
                };
                if reach <= 0.0 {
                    return 0.0;
                }
                (along / (2.0 * reach) + 0.5).clamp(0.0, 1.0)
            }
            GradientKind::Radial {
                center,
                radius,
                focus,
            } => {
                let (rx, ry) = (radius.0.max(1e-6), radius.1.max(1e-6));
                let (dx, dy) = ((point.x - center.0) / rx, (point.y - center.1) / ry);
                let outer = (dx * dx + dy * dy).sqrt();
                let (fx, fy) = (focus.0.max(0.0), focus.1.max(0.0));
                if fx <= 0.0 || fy <= 0.0 {
                    return outer.clamp(0.0, 1.0);
                }
                // How far the point is in the inner ellipse's own frame, where
                // 1 is its edge. Both counts scale with the distance along the
                // ray, so the point's place between the two ellipse edges is
                // where its inner count sits between their boundary counts.
                let (ix, iy) = (point.x - center.0, point.y - center.1);
                let inner = ((ix / fx) * (ix / fx) + (iy / fy) * (iy / fy)).sqrt();
                let span = inner - outer;
                if span.abs() <= 1e-9 {
                    // The two ellipses are the same one: every point past it is
                    // the last stop.
                    return outer.clamp(0.0, 1.0);
                }
                (((inner - 1.0) * outer) / span).clamp(0.0, 1.0)
            }
        }
    }
}

/// `factor` of the way from one colour to another.
fn mix(from: Color, to: Color, factor: f64) -> Color {
    let channel = |a: u8, b: u8| {
        (f64::from(a) + (f64::from(b) - f64::from(a)) * factor)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
    }
}

/// A subpath of a shape's outline, in the drawing's points.
pub struct Contour {
    pub points: Vec<metafile::Point>,
    pub closed: bool,
    /// Whether the contour is a region to fill, or only a line of the drawing.
    /// A cube's back edge and a cylinder's rim are part of the picture and part
    /// of its stroke, but filling them would paint a band across the shape.
    pub filled: bool,
}

pub fn polygon(points: &[(f64, f64)]) -> Contour {
    Contour {
        points: points.iter().map(|(x, y)| metafile::pt(*x, *y)).collect(),
        closed: true,
        filled: true,
    }
}

/// A rounded box, its corners quarter-circles.
pub fn rounded(frame: Frame, radius: f64) -> Contour {
    let radius = radius.min(frame.width.min(frame.height) / 2.0);
    let mut points = Vec::new();
    // Each corner turns inside a square of `radius`, eight segments to the
    // quarter, starting at the top-left's top edge.
    let corners = [
        (frame.left + radius, frame.top + radius, 180.0f64),
        (frame.right() - radius, frame.top + radius, 270.0),
        (frame.right() - radius, frame.bottom() - radius, 0.0),
        (frame.left + radius, frame.bottom() - radius, 90.0),
    ];
    for (cx, cy, start) in corners {
        for step in 0..=8 {
            let angle = (start + step as f64 * 90.0 / 8.0).to_radians();
            points.push(metafile::pt(
                cx + radius * angle.cos(),
                cy + radius * angle.sin(),
            ));
        }
    }
    Contour {
        points,
        closed: true,
        filled: true,
    }
}

pub fn ellipse(frame: Frame) -> Contour {
    let cx = frame.left + frame.width / 2.0;
    let cy = frame.top + frame.height / 2.0;
    let points = (0..64)
        .map(|step| {
            let angle = step as f64 * std::f64::consts::TAU / 64.0;
            metafile::pt(
                cx + frame.width / 2.0 * angle.cos(),
                cy + frame.height / 2.0 * angle.sin(),
            )
        })
        .collect();
    Contour {
        points,
        closed: true,
        filled: true,
    }
}

/// The frame that just holds `points`, with `stroke` of room for the stroke.
pub fn ink_frame(points: &[metafile::Point], stroke: f64) -> Frame {
    let margin = stroke / 2.0;
    let mut frame: Option<Frame> = None;
    for point in points {
        let single = Frame {
            left: point.x,
            top: point.y,
            width: 0.0,
            height: 0.0,
        };
        frame = Some(match frame {
            Some(frame) => frame.union(single),
            None => single,
        });
    }
    let frame = frame.expect("a shape's outline has a point");
    Frame {
        left: frame.left - margin,
        top: frame.top - margin,
        width: frame.width + stroke,
        height: frame.height + stroke,
    }
}

/// An arrowhead: the shape it is cut as, and how big it is in points.
///
/// DrawingML states one as an `a:headEnd`/`a:tailEnd` and VML as a
/// `v:stroke`'s `startarrow`/`endarrow`; the two vocularies differ, the ink
/// does not, so the head is cut here and both painters put it on the line.
#[derive(Debug, Clone, Copy)]
pub struct ArrowEnd {
    pub kind: ArrowKind,
    /// How far the head reaches back along the line.
    pub length: f64,
    /// How wide the head is across the line.
    pub width: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowKind {
    Triangle,
    Stealth,
    Diamond,
    Oval,
    Open,
}

/// The ink of an arrowhead whose tip is `tip` and which points along
/// `direction`, which is the unit vector outward from the line's end.
///
/// A head is filled or stroked depending on its shape: the open head is two
/// lines meeting at the tip, and the rest are regions with the tip on the
/// line's own end point and their base a length back inside it.
///
/// The stroke under a head is shortened by [`shortened`] so it does not run
/// through a concave one; this is the head alone.
pub fn arrowhead(
    tip: metafile::Point,
    direction: (f64, f64),
    end: ArrowEnd,
    color: Color,
) -> Vec<Op> {
    let (dx, dy) = direction;
    let length = end.length.max(0.1);
    let half = (end.width / 2.0).max(0.1);
    // Outward is `direction`; the head runs back along it.
    let back = metafile::pt(tip.x - dx * length, tip.y - dy * length);
    let side = metafile::pt(-dy * half, dx * half);
    let wing = |sign: f64| metafile::pt(back.x + side.x * sign, back.y + side.y * sign);

    match end.kind {
        ArrowKind::Triangle => vec![Op::Fill {
            contours: vec![vec![tip, wing(1.0), wing(-1.0)]],
            color,
        }],
        ArrowKind::Stealth => {
            // The barbs at the base and a notch cut back toward the tip:
            // the concave head a slide's default arrow is.
            let notch = metafile::pt(tip.x - dx * length * 0.6, tip.y - dy * length * 0.6);
            vec![Op::Fill {
                contours: vec![vec![tip, wing(1.0), notch, wing(-1.0)]],
                color,
            }]
        }
        ArrowKind::Diamond => {
            let middle = metafile::pt(tip.x - dx * length / 2.0, tip.y - dy * length / 2.0);
            let flank =
                |sign: f64| metafile::pt(middle.x + side.x * sign, middle.y + side.y * sign);
            vec![Op::Fill {
                contours: vec![vec![tip, flank(1.0), back, flank(-1.0)]],
                color,
            }]
        }
        ArrowKind::Oval => {
            let centre = metafile::pt(tip.x - dx * length / 2.0, tip.y - dy * length / 2.0);
            let points = (0..32)
                .map(|step| {
                    let angle = step as f64 * std::f64::consts::TAU / 32.0;
                    let along = length / 2.0 * angle.cos();
                    let across = half * angle.sin();
                    metafile::pt(
                        centre.x + dx * along - dy * across,
                        centre.y + dy * along + dx * across,
                    )
                })
                .collect();
            vec![Op::Fill {
                contours: vec![points],
                color,
            }]
        }
        ArrowKind::Open => {
            // Two strokes meeting at the tip: the open arrow, which is
            // drawn rather than filled.
            vec![
                Op::Stroke {
                    points: vec![wing(1.0), tip],
                    closed: false,
                    width: end.width.max(0.5) * 0.15,
                    color,
                },
                Op::Stroke {
                    points: vec![tip, wing(-1.0)],
                    closed: false,
                    width: end.width.max(0.5) * 0.15,
                    color,
                },
            ]
        }
    }
}

/// A line's points with the stroke pulled back off the arrowheads it ends in:
/// the head is drawn from the end point, and a stroke run under it would show
/// through the concave ones.
pub fn shortened(
    points: &[metafile::Point],
    head: Option<ArrowEnd>,
    tail: Option<ArrowEnd>,
) -> Vec<metafile::Point> {
    let mut points = points.to_vec();
    if let Some(head) = head {
        if points.len() >= 2 {
            if let Some(direction) = unit(points[0], points[1]) {
                points[0] = metafile::pt(
                    points[0].x + direction.0 * head.length,
                    points[0].y + direction.1 * head.length,
                );
            }
        }
    }
    if let Some(tail) = tail {
        let last = points.len() - 1;
        if last >= 1 {
            if let Some(direction) = unit(points[last], points[last - 1]) {
                points[last] = metafile::pt(
                    points[last].x + direction.0 * tail.length,
                    points[last].y + direction.1 * tail.length,
                );
            }
        }
    }
    points
}

/// The unit vector from `from` to `to`, or `None` for a zero-length segment.
pub fn unit(from: metafile::Point, to: metafile::Point) -> Option<(f64, f64)> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = (dx * dx + dy * dy).sqrt();
    (length > 1e-9).then(|| (dx / length, dy / length))
}

/// Blur an alpha mask by a box of `radius` pixels, separably: a row pass and
/// then a column pass, each a sliding sum, so the cost is the mask's size rather
/// than the radius'.
fn blur_mask(mask: &mut [f32], width: usize, height: usize, radius: usize) {
    if radius == 0 || width == 0 || height == 0 {
        return;
    }
    let window = (radius * 2 + 1) as f32;
    let mut line = vec![0.0f32; width.max(height)];

    // Rows: a copy of the row slides under the window while the sum is carried
    // from pixel to pixel.
    for y in 0..height {
        let row = &mut mask[y * width..(y + 1) * width];
        line[..width].copy_from_slice(row);
        let mut sum: f32 = line[..=radius.min(width - 1)].iter().sum();
        for x in 0..width {
            row[x] = sum / window;
            if x >= radius {
                sum -= line[x - radius];
            }
            if let Some(entering) = line.get(x + radius + 1) {
                sum += entering;
            }
        }
    }

    // Columns, the same way, over the rows the first pass rewrote.
    for x in 0..width {
        for y in 0..height {
            line[y] = mask[y * width + x];
        }
        let mut sum: f32 = line[..=radius.min(height - 1)].iter().sum();
        for y in 0..height {
            mask[y * width + x] = sum / window;
            if y >= radius {
                sum -= line[y - radius];
            }
            if let Some(entering) = line.get(y + radius + 1) {
                sum += entering;
            }
        }
    }
}

/// Paint a picture through a shape's geometry.
///
/// The shape is the clip: the picture is laid down only where the shape's own
/// outline covers it, which is what an application does with a picture fill —
/// the reason a reader that reported the bare picture was showing the fill's
/// contents rather than the shape on the page. `quad` is the box the picture is
/// stretched across, so a shape that is mirrored or turned carries its fill with
/// it, and `a:srcRect` crops what of the picture is shown.
fn paint_picture(
    canvas: &mut metafile::Canvas,
    size: (u32, u32),
    bounds: Frame,
    scale: f64,
    contours: &[Vec<metafile::Point>],
    image: &Image,
    quad: &[metafile::Point; 4],
) {
    let (width, height) = size;
    if image.width == 0 || image.height == 0 || contours.iter().all(|contour| contour.len() < 3) {
        return;
    }
    let to_canvas = |point: metafile::Point| {
        metafile::pt(
            (point.x - bounds.left) * scale,
            (point.y - bounds.top) * scale,
        )
    };

    // The clip: the shape's silhouette as a mask, black where the shape is and
    // white where it is not, so the picture only lands inside it.
    let polygons: Vec<Vec<metafile::Point>> = contours
        .iter()
        .map(|contour| contour.iter().copied().map(to_canvas).collect())
        .collect();
    let mut silhouette = metafile::Canvas::new(width, height);
    silhouette.fill_polygons(
        &polygons,
        metafile::FillRule::Alternate,
        metafile::Color::BLACK,
    );
    let mask = silhouette.into_pixels();

    // The box the picture is stretched across, as a parallelogram: a point
    // inside it is `origin + u * edge_u + v * edge_v`, with `u` and `v` the
    // picture's own coordinates within its shown part.
    let corners: Vec<metafile::Point> = quad.iter().copied().map(to_canvas).collect();
    let origin = corners[0];
    let edge_u = metafile::pt(corners[1].x - origin.x, corners[1].y - origin.y);
    let edge_v = metafile::pt(corners[3].x - origin.x, corners[3].y - origin.y);
    let determinant = edge_u.x * edge_v.y - edge_u.y * edge_v.x;
    if determinant.abs() < 1e-9 {
        return;
    }

    let (mut least_x, mut most_x) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut least_y, mut most_y) = (f64::INFINITY, f64::NEG_INFINITY);
    for corner in &corners {
        least_x = least_x.min(corner.x);
        most_x = most_x.max(corner.x);
        least_y = least_y.min(corner.y);
        most_y = most_y.max(corner.y);
    }
    let widest = i64::from(width.saturating_sub(1));
    let tallest = i64::from(height.saturating_sub(1));
    let left = (least_x.floor().max(0.0) as i64).min(widest);
    let right = (most_x.ceil().max(0.0) as i64).min(widest);
    let top = (least_y.floor().max(0.0) as i64).min(tallest);
    let bottom = (most_y.ceil().max(0.0) as i64).min(tallest);

    for y in top..=bottom {
        for x in left..=right {
            let at = ((y as usize) * width as usize + x as usize) * 4;
            // The mask is the shape's coverage: black inside, and antialiased
            // along the edge.
            let coverage = 1.0 - f64::from(mask[at]) / 255.0;
            if coverage <= 0.0 {
                continue;
            }
            let dx = x as f64 + 0.5 - origin.x;
            let dy = y as f64 + 0.5 - origin.y;
            let u = (dx * edge_v.y - dy * edge_v.x) / determinant;
            let v = (edge_u.x * dy - edge_u.y * dx) / determinant;
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                continue;
            }
            let Some([r, g, b, a]) = image.at(u, v) else {
                continue;
            };
            if a == 0 {
                continue;
            }
            let alpha = coverage * f64::from(a) / 255.0;
            if alpha > 0.0 {
                canvas.blend_pixel(x as i32, y as i32, Color { r, g, b }, alpha);
            }
        }
    }
}

/// Paint everything collected into a PNG, or `None` when there is nothing
/// to see.
///
/// The PNG's size is the bounds at [`DPI`], and the frame it covers in the
/// drawing's points is returned with it, so a caller can report the picture at
/// the size it was drawn.
pub fn rasterize(ops: &[Op], bounds: Option<Frame>) -> Option<(Vec<u8>, f64, f64)> {
    let bounds = bounds?;
    if !(bounds.width > 0.0 && bounds.height > 0.0) {
        return None;
    }
    let scale = DPI / 72.0;
    let width = (bounds.width * scale).round().max(1.0);
    let height = (bounds.height * scale).round().max(1.0);
    if width > MAX_SIDE as f64 || height > MAX_SIDE as f64 || width * height > MAX_AREA as f64 {
        tracing::debug!("a shape drawing is too large to rasterise ({width}x{height})");
        return None;
    }
    let mut canvas = metafile::Canvas::new(width as u32, height as u32);
    let map = |point: metafile::Point| {
        metafile::pt(
            (point.x - bounds.left) * scale,
            (point.y - bounds.top) * scale,
        )
    };

    for op in ops {
        match op {
            Op::Fill { contours, color } => {
                let polygons: Vec<Vec<metafile::Point>> = contours
                    .iter()
                    .map(|contour| contour.iter().copied().map(map).collect())
                    .collect();
                canvas.fill_polygons(&polygons, metafile::FillRule::Alternate, *color);
            }
            Op::GradientFill { contours, gradient } => {
                let polygons: Vec<Vec<metafile::Point>> = contours
                    .iter()
                    .map(|contour| contour.iter().copied().map(map).collect())
                    .collect();
                // The gradient is stated in the drawing's points; the fill is
                // asked for in canvas pixels, so each pixel's centre is taken
                // back to the point it covers before the ramp is read.
                let to_points =
                    |x: f64, y: f64| metafile::pt(bounds.left + x / scale, bounds.top + y / scale);
                canvas.fill_polygons_shaded(
                    &polygons,
                    metafile::FillRule::Alternate,
                    &mut |x, y| gradient.at(to_points(x, y)),
                );
            }
            Op::Shadow {
                contours,
                offset,
                blur,
                color,
                alpha,
            } => {
                let moved: Vec<Vec<metafile::Point>> = contours
                    .iter()
                    .map(|contour| {
                        contour
                            .iter()
                            .map(|point| map(metafile::pt(point.x + offset.0, point.y + offset.1)))
                            .collect()
                    })
                    .collect();
                if moved.iter().all(|contour| contour.len() < 2) || *alpha <= 0.0 {
                    continue;
                }
                // The silhouette is cut into an alpha mask on a canvas of the
                // same size — black where the shape will be, white elsewhere —
                // then blurred and laid down under the ink. A shadow is the
                // shape's own outline seen through a soft edge, which is what a
                // mask is.
                let mut silhouette = metafile::Canvas::new(width as u32, height as u32);
                silhouette.fill_polygons(
                    &moved,
                    metafile::FillRule::Alternate,
                    metafile::Color::BLACK,
                );
                let pixels = silhouette.into_pixels();
                let mut mask: Vec<f32> = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| 1.0 - f32::from(pixel[0]) / 255.0)
                    .collect();
                let radius = (blur * scale / 2.0).round() as usize;
                if radius > 0 {
                    // Two boxes make a soft edge that falls off like a shadow's
                    // rather than one hard step; the radius is half the stated
                    // blur because the file's number is the whole spread.
                    blur_mask(&mut mask, width as usize, height as usize, radius);
                    blur_mask(&mut mask, width as usize, height as usize, radius);
                }
                for y in 0..height as usize {
                    for x in 0..width as usize {
                        let coverage = mask[y * width as usize + x];
                        if coverage > 0.0 {
                            canvas.blend_pixel(
                                x as i32,
                                y as i32,
                                *color,
                                f64::from(coverage) * alpha,
                            );
                        }
                    }
                }
            }
            Op::Picture {
                contours,
                image,
                quad,
            } => paint_picture(
                &mut canvas,
                (width as u32, height as u32),
                bounds,
                scale,
                contours,
                image,
                quad,
            ),
            Op::Stroke {
                points,
                closed,
                width,
                color,
            } => {
                let points: Vec<metafile::Point> = points.iter().copied().map(map).collect();
                canvas.stroke(&points, *closed, width * scale, *color);
            }
            Op::Text {
                text,
                frame,
                color,
                stretch,
                font_pt,
            } => {
                let cell = if *stretch {
                    // WordArt stretches its string to the shape: the height
                    // that makes the line as wide as the box, or the box's
                    // own height where the box is the narrower limit.
                    let per_unit = metafile::text_width(text, 1.0);
                    let fitted = if per_unit > 0.0 {
                        frame.width / per_unit
                    } else {
                        frame.height
                    };
                    fitted.min(frame.height).max(0.0)
                } else {
                    *font_pt
                };
                let width = metafile::text_width(text, cell);
                let origin = map(metafile::pt(
                    frame.left + (frame.width - width) / 2.0,
                    frame.top + (frame.height - cell) / 2.0,
                ));
                metafile::draw_text(
                    &mut canvas,
                    text,
                    origin,
                    cell * scale,
                    metafile::TextAlign {
                        horizontal: metafile::Horizontal::Left,
                        vertical: metafile::Vertical::Top,
                    },
                    0.0,
                    *color,
                    None,
                );
            }
        }
    }

    let pixels = canvas.into_pixels();
    let inked = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[0] != 255 || pixel[1] != 255 || pixel[2] != 255);
    if !inked {
        tracing::debug!("a shape drawing rasterised to an empty picture");
        return None;
    }
    let raster = metafile::Raster {
        width: width as u32,
        height: height as u32,
        pixels,
        unsupported: Vec::new(),
    };
    let png = raster.to_png().ok()?;
    Some((png, bounds.width, bounds.height))
}
