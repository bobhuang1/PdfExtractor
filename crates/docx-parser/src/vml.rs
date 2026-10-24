//! Drawing a VML `w:pict` into a picture.
//!
//! A Word file can carry a drawing in two languages. DrawingML (`w:drawing`) is
//! the modern one; VML (`w:pict`) is what Word 97-2003 wrote, what a `.doc` keeps
//! when LibreOffice rewrites it as DOCX, and what a compatibility-mode DOCX still
//! holds beside its DrawingML copy. Some of it the reader already knows: a
//! `v:imagedata` names an image part, which travels as a picture like any other.
//! The rest of it — an autoshape, a group of them, a WordArt — has no bytes at
//! all: Word paints it from the markup. Unread, such a drawing is nothing at
//! all in the document model, so a page that shows a shape shows no picture.
//!
//! So the shapes are painted here, the way `pdfextractor-metafile` paints the
//! records of an EMF or a WMF: boxes, ovals, rounded boxes, lines, polylines and
//! a shape's own `path` (with its preset's `@` formulas resolved) become
//! polygons and strokes on a canvas at 144 dpi, then a PNG. A group's
//! `coordsize`/`coordorigin` places its children, which is the only reason a
//! group needs to be read as a group at all.
//!
//! A shape is shaded as its markup states it: a `v:fill type="gradient"` (or
//! `"gradientRadial"`) is a two-colour ramp across the shape rather than its
//! first colour painted flat, a `v:shadow on="t"` is the shape's silhouette
//! moved and laid down under the ink, and a `v:stroke` `startarrow`/`endarrow`
//! is a head at that end of an open line, cut in the shared painter's five
//! shapes.
//!
//! What is drawn is the *drawing*, not the shape's text: the words in a shape's
//! `v:textbox` are the document's own text, reported where they stand, and
//! painting them again would put the same words in the document twice. A
//! WordArt is the other case — its string lives only in `v:textpath`, in an
//! attribute no text item ever sees — so that string is drawn, stretched to the
//! shape the way WordArt stretches it. It is drawn in
//! [`pdfextractor_metafile`]'s built-in 5×7 font, for the reason stated there: a
//! document extractor does not fetch glyph outlines from the machine it runs on.
//! The string's place, size and colour are the file's; the letterforms are a
//! substitute.
//!
//! Not drawn: patterns, 3-D, the shadow types other than a single offset copy,
//! and anything whose markup this does not understand. A shape with `o:hr` is a horizontal
//! rule — a paragraph's line, which Word draws as a border — and is left out on
//! purpose. A drawing that puts no ink on the canvas is no picture, rather than
//! a blank one.

use std::collections::HashMap;

use pdfextractor_metafile as metafile;
use pdfextractor_metafile::Color;

use pdfextractor_drawingml::paint::{
    self, ellipse, ink_frame, polygon, rounded, ArrowEnd, Contour, Frame, Node, Op, Space,
};

/// The stroke a shape has when it names no width: VML's hairline.
const DEFAULT_STROKE_PT: f64 = 0.75;

/// A bare stroke weight is read in EMU, the unit LibreOffice writes one in
/// (`weight="9360"` is its hairline). Word states points (`strokeweight="1.5pt"`),
/// which takes the other branch.
const EMU_PER_POINT: f64 = 12700.0;

/// How a shape is filled: one colour, or a ramp through two.
enum VmlFill {
    Solid(Color),
    Gradient(paint::Gradient),
}

/// A shape's shadow, as its `v:shadow` states it.
struct VmlShadow {
    /// How far the silhouette is moved, in points, x then y.
    offset: (f64, f64),
    color: Color,
    /// The shadow's own opacity, 0..=1.
    alpha: f64,
}

impl VmlShadow {
    /// The silhouette's box: the shape's frame, moved by the offset.
    fn frame(&self, shape: Frame) -> Frame {
        Frame {
            left: shape.left + self.offset.0,
            top: shape.top + self.offset.1,
            width: shape.width,
            height: shape.height,
        }
    }
}

/// A `w:pict` drawn into pixels, and the size it was drawn at.
pub(crate) struct VmlPicture {
    pub(crate) png: Vec<u8>,
    pub(crate) width_pt: f64,
    pub(crate) height_pt: f64,
}

/// Draw the `w:pict` in `pict`, or `None` when it is nothing a picture can be
/// made of: no shape this understands, no size, or no ink.
pub(crate) fn render(pict: &[u8]) -> Option<VmlPicture> {
    let root = paint::parse(pict)?;
    let mut renderer = Renderer::default();
    renderer.draw_pict(&root);
    let (png, width_pt, height_pt) = paint::rasterize(&renderer.ops, renderer.bounds)?;
    Some(VmlPicture {
        png,
        width_pt,
        height_pt,
    })
}

#[derive(Default)]
struct Renderer {
    ops: Vec<Op>,
    bounds: Option<Frame>,
}

impl Renderer {
    fn draw_pict(&mut self, root: &Node) {
        let types = shape_types(root);
        for child in &root.children {
            self.draw_node(child, &types, &Space::POINTS);
        }
    }

    fn draw_node(&mut self, node: &Node, types: &HashMap<String, &Node>, space: &Space) {
        match node.name.as_str() {
            // A definition, not a drawing: it is looked up by the shapes that
            // name it in `type`.
            "shapetype" => {}
            "group" => self.draw_group(node, types, space),
            "rect" | "roundrect" | "oval" | "line" | "polyline" | "curve" | "shape" => {
                self.draw_shape(node, types, space)
            }
            _ => {
                for child in &node.children {
                    self.draw_node(child, types, space);
                }
            }
        }
    }

    fn draw_group(&mut self, node: &Node, types: &HashMap<String, &Node>, parent: &Space) {
        let Some(group) = frame_of(node, parent) else {
            return;
        };
        let (width, height) = coordsize(node).unwrap_or((group.width, group.height));
        if !(width > 0.0 && height > 0.0) {
            return;
        }
        let (ox, oy) = coordorigin(node);
        let space = Space {
            sx: group.width / width,
            sy: group.height / height,
            ox,
            oy,
            dx: group.left,
            dy: group.top,
        };
        let drawn = self.ops.len();
        let bounds = self.bounds;
        for child in &node.children {
            self.draw_node(child, types, &space);
        }
        // The group's frame is the picture's frame: its children are placed in
        // it, and a child that reaches past the frame is clipped by Word too, so
        // their own frames are not what the group as a whole is measured by.
        if self.ops.len() > drawn {
            self.bounds = Some(match bounds {
                Some(bounds) => bounds.union(group),
                None => group,
            });
        } else {
            self.bounds = bounds;
        }
    }

    fn draw_shape(&mut self, node: &Node, types: &HashMap<String, &Node>, space: &Space) {
        // A horizontal rule is a paragraph's line, not a picture.
        if node.is("hr") == Some(true) {
            return;
        }
        let frame = frame_of(node, space);
        let preset = node
            .attr("type")
            .and_then(|name| types.get(name.trim_start_matches('#')))
            .copied();

        if let Some(textpath) = text_node(node, preset) {
            if let Some(text) = textpath.attr("string").or_else(|| node.attr("string")) {
                let text = text.trim();
                if !text.is_empty() {
                    // A WordArt is drawn in its shape's frame, so one with no
                    // frame has nowhere to be.
                    let Some(frame) = frame else {
                        return;
                    };
                    let style = textpath.attr("style").unwrap_or_default();
                    self.ops.push(Op::Text {
                        text: text.to_string(),
                        frame,
                        color: fill_color(node).unwrap_or(Color::BLACK),
                        stretch: textpath.is("fitshape") == Some(true)
                            || textpath.is("xscale") == Some(true),
                        font_pt: style_value(style, "font-size")
                            .and_then(parse_length)
                            .map(length_in_points)
                            .unwrap_or(12.0),
                    });
                    self.include(frame);
                }
                return;
            }
        }

        let Some(drawing) = geometry(node, preset, space, frame) else {
            return;
        };
        let Geometry {
            contours,
            closed,
            frame,
        } = drawing;
        let shadow = shadow(node);
        let arrows = arrow_ends(node, stroke_width(node));
        let mut drew = false;

        // The shadow goes down first: it is behind everything the shape shows.
        if let Some(shadow) = &shadow {
            let silhouette: Vec<Vec<metafile::Point>> = contours
                .iter()
                .filter(|contour| contour.filled)
                .map(|contour| contour.points.clone())
                .collect();
            if !silhouette.is_empty() {
                self.ops.push(Op::Shadow {
                    contours: silhouette,
                    offset: shadow.offset,
                    // A VML shadow states no blur: its edge is as hard as the
                    // silhouette it was cut from.
                    blur: 0.0,
                    color: shadow.color,
                    alpha: shadow.alpha,
                });
                drew = true;
            }
        }

        if let Some(fill) = shape_fill(node, frame) {
            let polygons: Vec<Vec<metafile::Point>> = contours
                .iter()
                .filter(|contour| contour.filled)
                .map(|contour| contour.points.clone())
                .collect();
            if !polygons.is_empty() {
                self.ops.push(match fill {
                    VmlFill::Solid(color) => Op::Fill {
                        contours: polygons,
                        color,
                    },
                    VmlFill::Gradient(gradient) => Op::GradientFill {
                        contours: polygons,
                        gradient,
                    },
                });
                drew = true;
            }
        }
        if let Some(color) = stroke_color(node) {
            let width = stroke_width(node);
            for contour in &contours {
                if contour.points.len() < 2 {
                    continue;
                }
                let closes = contour.closed && closed;
                // A stroke under an arrowhead would show through the concave
                // heads, so it stops at the head's base; the head is drawn from
                // the line's own end point.
                let points = if closes {
                    contour.points.clone()
                } else {
                    paint::shortened(&contour.points, arrows.0, arrows.1)
                };
                self.ops.push(Op::Stroke {
                    points,
                    closed: closes,
                    width,
                    color,
                });
                if !closes {
                    self.arrowheads(&contour.points, arrows, color);
                }
                drew = true;
            }
        }
        if drew {
            let mut ink = frame;
            // A shadow and an arrowhead reach past the shape's own box, and the
            // canvas is only as large as the ink it holds.
            if let Some(shadow) = shadow {
                ink = ink.union(shadow.frame(frame));
            }
            let reach = [arrows.0, arrows.1]
                .into_iter()
                .flatten()
                .map(|end| end.width)
                .fold(0.0f64, f64::max);
            if reach > 0.0 {
                ink = ink.union(Frame {
                    left: ink.left - reach,
                    top: ink.top - reach,
                    width: ink.width + reach * 2.0,
                    height: ink.height + reach * 2.0,
                });
            }
            self.include(ink);
        }
    }

    /// The ink of a line's arrowheads: a head at each end of an open line.
    fn arrowheads(
        &mut self,
        points: &[metafile::Point],
        ends: (Option<ArrowEnd>, Option<ArrowEnd>),
        color: Color,
    ) {
        if points.len() < 2 {
            return;
        }
        let (first, second) = (points[0], points[1]);
        let (last, before) = (points[points.len() - 1], points[points.len() - 2]);
        // Each end's direction points out of the line, so the head's tip is the
        // line's own end point and its base is a length back inside.
        if let Some(head) = ends.0 {
            if let Some(direction) = paint::unit(second, first) {
                self.ops
                    .extend(paint::arrowhead(first, direction, head, color));
            }
        }
        if let Some(tail) = ends.1 {
            if let Some(direction) = paint::unit(before, last) {
                self.ops
                    .extend(paint::arrowhead(last, direction, tail, color));
            }
        }
    }

    fn include(&mut self, frame: Frame) {
        self.bounds = Some(match self.bounds {
            Some(bounds) => bounds.union(frame),
            None => frame,
        });
    }
}

/// The preset definitions a `w:pict` carries, keyed by the `id` a shape names in
/// `type`. A definition can sit in any group, so the whole drawing is searched.
fn shape_types(root: &Node) -> HashMap<String, &Node> {
    let mut found = HashMap::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.name == "shapetype" {
            if let Some(id) = node.attr("id") {
                found.insert(id.to_string(), node);
            }
        }
        stack.extend(node.children.iter());
    }
    found
}

fn text_node<'a>(node: &'a Node, preset: Option<&'a Node>) -> Option<&'a Node> {
    node.child("textpath")
        .or_else(|| preset.and_then(|preset| preset.child("textpath")))
        .filter(|textpath| textpath.is("on") != Some(false))
}

/// The shape's box, from its `style` attribute.
///
/// `width` and `height` are required — a shape without a box is nothing this can
/// place. `left`/`top` fall back to the margins Word writes for a floating
/// shape, and to zero.
fn frame_of(node: &Node, space: &Space) -> Option<Frame> {
    let style = node.attr("style").unwrap_or_default();
    let width = style_value(style, "width").and_then(parse_length)?;
    let height = style_value(style, "height").and_then(parse_length)?;
    let left = style_value(style, "left")
        .or_else(|| style_value(style, "margin-left"))
        .and_then(parse_length);
    let top = style_value(style, "top")
        .or_else(|| style_value(style, "margin-top"))
        .and_then(parse_length);
    let frame = Frame {
        left: place(space, left, true),
        top: place(space, top, false),
        width: span(space, width, true),
        height: span(space, height, false),
    };
    (frame.width > 0.0 && frame.height > 0.0).then_some(frame)
}

/// Where a length puts a shape: a point in the drawing when the file states
/// points, and a coordinate in the parent's space when it does not.
fn place(space: &Space, length: Option<Length>, x_axis: bool) -> f64 {
    match length {
        Some(Length::Points(value)) => {
            if x_axis {
                space.dx + value
            } else {
                space.dy + value
            }
        }
        Some(Length::Local(value)) => {
            let point = space.point(
                if x_axis { value } else { 0.0 },
                if x_axis { 0.0 } else { value },
            );
            if x_axis {
                point.x
            } else {
                point.y
            }
        }
        None => {
            if x_axis {
                space.dx
            } else {
                space.dy
            }
        }
    }
}

/// How long a length is, in the drawing's points.
fn span(space: &Space, length: Length, x_axis: bool) -> f64 {
    match length {
        Length::Points(value) => value,
        Length::Local(value) => {
            if x_axis {
                value * space.sx
            } else {
                value * space.sy
            }
        }
    }
}

fn length_in_points(length: Length) -> f64 {
    match length {
        Length::Points(value) => value,
        Length::Local(value) => value,
    }
}

/// A length in a `style` or attribute.
#[derive(Debug, Clone, Copy)]
enum Length {
    /// Already in points.
    Points(f64),
    /// A number in the space it sits in.
    Local(f64),
}

fn parse_length(raw: &str) -> Option<Length> {
    let raw = raw.trim();
    let split = raw
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(raw.len());
    let (number, unit) = raw.split_at(split);
    let value: f64 = number.trim().parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    match unit.trim().to_ascii_lowercase().as_str() {
        "" => Some(Length::Local(value)),
        "pt" => Some(Length::Points(value)),
        "in" => Some(Length::Points(value * 72.0)),
        "cm" => Some(Length::Points(value * 72.0 / 2.54)),
        "mm" => Some(Length::Points(value * 7.2 / 2.54)),
        "px" => Some(Length::Points(value * 0.75)),
        "pc" => Some(Length::Points(value * 12.0)),
        "emu" => Some(Length::Points(value / EMU_PER_POINT)),
        _ => None,
    }
}

/// One declaration of a `style` attribute: `width:12pt;height:4pt`.
fn style_value<'a>(style: &'a str, key: &str) -> Option<&'a str> {
    style.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        name.trim().eq_ignore_ascii_case(key).then(|| value.trim())
    })
}

/// The shape's own coordinate space, from `coordsize`/`coordorigin`.
fn coordsize(node: &Node) -> Option<(f64, f64)> {
    let raw = node.attr("coordsize")?;
    let mut parts = raw.split(',').map(|part| part.trim().parse::<f64>().ok());
    let width = parts.next()??;
    let height = parts.next()??;
    (width > 0.0 && height > 0.0).then_some((width, height))
}

fn coordorigin(node: &Node) -> (f64, f64) {
    let Some(raw) = node.attr("coordorigin") else {
        return (0.0, 0.0);
    };
    let mut parts = raw.split(',');
    let x = parts
        .next()
        .and_then(|part| part.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    let y = parts
        .next()
        .and_then(|part| part.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    (x, y)
}

/// A shape's outline, the frame it was drawn in, and whether the outline closes.
struct Geometry {
    contours: Vec<Contour>,
    closed: bool,
    frame: Frame,
}

fn geometry(
    node: &Node,
    preset: Option<&Node>,
    space: &Space,
    frame: Option<Frame>,
) -> Option<Geometry> {
    match node.name.as_str() {
        "rect" => {
            let frame = frame?;
            Some(Geometry {
                contours: vec![polygon(&[
                    (frame.left, frame.top),
                    (frame.right(), frame.top),
                    (frame.right(), frame.bottom()),
                    (frame.left, frame.bottom()),
                ])],
                closed: true,
                frame,
            })
        }
        "roundrect" => {
            let frame = frame?;
            let radius = roundness(node) * frame.width.min(frame.height);
            Some(Geometry {
                contours: vec![rounded(frame, radius)],
                closed: true,
                frame,
            })
        }
        "oval" => {
            let frame = frame?;
            Some(Geometry {
                contours: vec![ellipse(frame)],
                closed: true,
                frame,
            })
        }
        "line" => {
            let space = shape_space(node, preset, frame).unwrap_or(*space);
            let points = vec![
                point_attr(node.attr("from")?, &space)?,
                point_attr(node.attr("to")?, &space)?,
            ];
            // A line Word gives no frame — the group-space lines of a picture
            // placeholder are written that way — is as big as the ink it draws.
            let frame = frame.unwrap_or_else(|| ink_frame(&points, stroke_width(node)));
            Some(Geometry {
                contours: vec![Contour {
                    points,
                    closed: false,
                    filled: true,
                }],
                closed: false,
                frame,
            })
        }
        "polyline" => {
            let space = shape_space(node, preset, frame).unwrap_or(*space);
            let points: Vec<metafile::Point> = node
                .attr("points")?
                .split_whitespace()
                .filter_map(|pair| point_attr(pair, &space))
                .collect();
            (points.len() >= 2).then(|| Geometry {
                frame: frame.unwrap_or_else(|| ink_frame(&points, stroke_width(node))),
                contours: vec![Contour {
                    points,
                    closed: false,
                    filled: true,
                }],
                closed: false,
            })
        }
        _ => {
            let frame = frame?;
            // A preset's path first, then the shape's own (which overrides it).
            let path = node
                .attr("path")
                .or_else(|| preset.and_then(|preset| preset.attr("path")))?;
            let (size, origin) = coordsize(node)
                .map(|size| (size, coordorigin(node)))
                .or_else(|| {
                    preset.map(|preset| {
                        (
                            coordsize(preset).unwrap_or((21600.0, 21600.0)),
                            coordorigin(preset),
                        )
                    })
                })?;
            let path_space = Space {
                sx: frame.width / size.0,
                sy: frame.height / size.1,
                ox: origin.0,
                oy: origin.1,
                dx: frame.left,
                dy: frame.top,
            };
            let adjust = Adjust::new(preset, node, size);
            let contours = parse_path(path, &adjust, &path_space);
            (!contours.is_empty()).then_some(Geometry {
                contours,
                closed: true,
                frame,
            })
        }
    }
}

/// The space a `v:line`/`v:polyline`'s coordinates are written in: its own
/// `coordsize` when it states one, the parent's space otherwise.
fn shape_space(node: &Node, preset: Option<&Node>, frame: Option<Frame>) -> Option<Space> {
    let size = coordsize(node).or_else(|| preset.and_then(coordsize))?;
    let frame = frame?;
    let (ox, oy) = coordorigin(node);
    Some(Space {
        sx: frame.width / size.0,
        sy: frame.height / size.1,
        ox,
        oy,
        dx: frame.left,
        dy: frame.top,
    })
}

fn point_attr(raw: &str, space: &Space) -> Option<metafile::Point> {
    let mut parts = raw.split(',');
    let x: f64 = parts.next()?.trim().parse().ok()?;
    let y: f64 = parts.next()?.trim().parse().ok()?;
    Some(space.point(x, y))
}

/// The corner radius a `v:roundrect` asks for, as a fraction of its shorter side.
fn roundness(node: &Node) -> f64 {
    let Some(raw) = node.attr("arcsize") else {
        return 1.0 / 6.0;
    };
    let raw = raw.trim();
    let (number, fraction) = match raw.strip_suffix('f') {
        Some(number) => (number, true),
        None => (raw, false),
    };
    let Ok(value) = number.parse::<f64>() else {
        return 1.0 / 6.0;
    };
    // `10923f` is 10923/21600 of the shorter side; a bare number is a fraction
    // of the whole, as VML writes `arcsize="0.2"`.
    let fraction = if fraction { value / 21600.0 } else { value };
    fraction.clamp(0.0, 0.5)
}

/// The `@` and `#` values a preset's path is written in.
///
/// `#n` is the `n`th adjustment handle (`adj="18462"`), `@n` the `n`th formula
/// of the preset (`<v:f eqn="prod #0 4 3"/>`), evaluated in order. Width and
/// height are the preset's own `coordsize`, which is what its formulas are
/// written against.
struct Adjust {
    handles: Vec<f64>,
    formulas: Vec<f64>,
    size: (f64, f64),
}

impl Adjust {
    fn new(preset: Option<&Node>, node: &Node, size: (f64, f64)) -> Adjust {
        let raw = node
            .attr("adj")
            .or_else(|| preset.and_then(|preset| preset.attr("adj")));
        let handles = raw
            .map(|raw| {
                raw.split(',')
                    .map(|part| part.trim().parse::<f64>().unwrap_or(0.0))
                    .collect()
            })
            .unwrap_or_default();
        let mut adjust = Adjust {
            handles,
            formulas: Vec::new(),
            size,
        };
        if let Some(formulas) = preset.and_then(|preset| preset.child("formulas")) {
            for formula in formulas.children.iter().filter(|child| child.name == "f") {
                let Some(equation) = formula.attr("eqn") else {
                    continue;
                };
                let value = adjust.evaluate(equation);
                adjust.formulas.push(value.unwrap_or(0.0));
            }
        }
        adjust
    }

    fn evaluate(&self, equation: &str) -> Option<f64> {
        let mut words = equation.split_whitespace();
        let operation = words.next()?;
        let arguments: Vec<f64> = words.filter_map(|word| self.operand(word)).collect();
        let sum = || arguments.iter().sum::<f64>();
        Some(match operation {
            "val" => *arguments.first()?,
            "sum" => sum(),
            "prod" => arguments.iter().product(),
            "mid" => (arguments.first()? + arguments.get(1)?) / 2.0,
            "abs" => arguments.first()?.abs(),
            "min" => arguments.iter().copied().fold(f64::INFINITY, f64::min),
            "max" => arguments.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            // VML's `mod` is the modulus of up to three numbers, not a remainder.
            "mod" => arguments
                .iter()
                .map(|value| value * value)
                .sum::<f64>()
                .sqrt(),
            _ => return None,
        })
    }

    fn operand(&self, word: &str) -> Option<f64> {
        if let Some(index) = word.strip_prefix('#') {
            return self.handles.get(index.parse::<usize>().ok()?).copied();
        }
        if let Some(index) = word.strip_prefix('@') {
            return self.formulas.get(index.parse::<usize>().ok()?).copied();
        }
        match word {
            "width" => Some(self.size.0),
            "height" => Some(self.size.1),
            _ => word.parse().ok(),
        }
    }
}

/// Read a VML `path` into subpaths placed in the drawing.
///
/// The grammar is a command letter and the numbers it takes: `m` moves (and
/// lines to further pairs), `l` lines, `r` lines relatively, `c` and `q` curve,
/// `x` closes the subpath, `e` ends. A repeated number is optional and reads as
/// zero, which is how Word writes `m,l,21600r21600,l21600,xe` for a rectangle.
fn parse_path(path: &str, adjust: &Adjust, space: &Space) -> Vec<Contour> {
    let mut contours: Vec<Contour> = Vec::new();
    let mut current: Option<Contour> = None;
    let mut cursor = (0.0f64, 0.0f64);

    for (command, raw) in commands(path) {
        let values: Vec<f64> = raw
            .split(',')
            .flat_map(|part| {
                let mut words: Vec<&str> = part.split_whitespace().collect();
                if words.is_empty() {
                    // An empty slot between separators: the coordinate is zero.
                    words.push("");
                }
                words
            })
            .map(|word| resolve(word, adjust).unwrap_or(0.0))
            .collect();

        match command {
            'e' => break,
            'x' => {
                if let Some(contour) = current.as_mut() {
                    contour.closed = true;
                }
            }
            'm' => {
                let mut index = 0;
                while index + 1 < values.len() {
                    cursor = (values[index], values[index + 1]);
                    if index == 0 {
                        if let Some(done) = current.take() {
                            contours.push(done);
                        }
                        current = Some(Contour {
                            points: vec![space.point(cursor.0, cursor.1)],
                            closed: false,
                            filled: true,
                        });
                    } else if let Some(contour) = current.as_mut() {
                        contour.points.push(space.point(cursor.0, cursor.1));
                    }
                    index += 2;
                }
            }
            'l' | 'r' => {
                let relative = command == 'r';
                let mut index = 0;
                while index + 1 < values.len() {
                    cursor = if relative {
                        (cursor.0 + values[index], cursor.1 + values[index + 1])
                    } else {
                        (values[index], values[index + 1])
                    };
                    if let Some(contour) = current.as_mut() {
                        contour.points.push(space.point(cursor.0, cursor.1));
                    }
                    index += 2;
                }
            }
            'c' => {
                let mut index = 0;
                while index + 5 < values.len() {
                    let from = cursor;
                    let control1 = (values[index], values[index + 1]);
                    let control2 = (values[index + 2], values[index + 3]);
                    let to = (values[index + 4], values[index + 5]);
                    if let Some(contour) = current.as_mut() {
                        for step in 1..=8 {
                            let point = cubic(from, control1, control2, to, step as f64 / 8.0);
                            contour.points.push(space.point(point.0, point.1));
                        }
                    }
                    cursor = to;
                    index += 6;
                }
            }
            'q' => {
                let mut index = 0;
                while index + 3 < values.len() {
                    let from = cursor;
                    let control = (values[index], values[index + 1]);
                    let to = (values[index + 2], values[index + 3]);
                    if let Some(contour) = current.as_mut() {
                        for step in 1..=8 {
                            let point = quadratic(from, control, to, step as f64 / 8.0);
                            contour.points.push(space.point(point.0, point.1));
                        }
                    }
                    cursor = to;
                    index += 4;
                }
            }
            _ => {}
        }
    }
    if let Some(done) = current.take() {
        contours.push(done);
    }
    contours.retain(|contour| contour.points.len() >= 2);
    contours
}

/// Split a path into its commands and the text after each.
fn commands(path: &str) -> Vec<(char, String)> {
    let mut out: Vec<(char, String)> = Vec::new();
    let mut current: Option<(char, String)> = None;
    for character in path.chars() {
        if character.is_ascii_alphabetic() {
            if let Some(done) = current.take() {
                out.push(done);
            }
            current = Some((character.to_ascii_lowercase(), String::new()));
        } else if let Some((_, raw)) = current.as_mut() {
            raw.push(character);
        }
    }
    if let Some(done) = current {
        out.push(done);
    }
    out
}

fn resolve(word: &str, adjust: &Adjust) -> Option<f64> {
    if word.trim().is_empty() {
        return Some(0.0);
    }
    adjust.operand(word.trim())
}

fn cubic(from: (f64, f64), c1: (f64, f64), c2: (f64, f64), to: (f64, f64), t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let x = u * u * u * from.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * to.0;
    let y = u * u * u * from.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * to.1;
    (x, y)
}

fn quadratic(from: (f64, f64), control: (f64, f64), to: (f64, f64), t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    (
        u * u * from.0 + 2.0 * u * t * control.0 + t * t * to.0,
        u * u * from.1 + 2.0 * u * t * control.1 + t * t * to.1,
    )
}

/// The fill a shape states: one colour, or the ramp its `v:fill` describes.
///
/// VML's gradient is two colours — `color` and `color2` — running across the
/// shape's frame: along `angle` for `type="gradient"`, with `focus` moving the
/// middle of the blend, and out from `focusposition` for
/// `type="gradientRadial"`, with `focussize` sizing the region its first stop
/// fills. A fill that names no type is a solid colour, as it always was.
fn shape_fill(node: &Node, frame: Frame) -> Option<VmlFill> {
    if node.is("filled") == Some(false) {
        return None;
    }
    if let Some(fill) = node.child("fill") {
        if fill.is("on") == Some(false) {
            return None;
        }
        let kind = fill
            .attr("type")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if matches!(kind.as_str(), "gradient" | "gradientradial") {
            if let Some(gradient) = gradient(fill, node, frame, kind == "gradientradial") {
                return Some(VmlFill::Gradient(gradient));
            }
        }
        if let Some(color) = fill.attr("color").and_then(parse_color) {
            return Some(VmlFill::Solid(color));
        }
    }
    Some(VmlFill::Solid(
        node.attr("fillcolor")
            .and_then(parse_color)
            .unwrap_or(Color::WHITE),
    ))
}

/// The two-colour ramp a `v:fill` states, across `frame`.
///
/// A gradient that names neither colour has nothing to interpolate, so it is no
/// gradient at all; one that names only `color2` runs from white, which is the
/// second colour's default.
///
/// A linear ramp's `focus` moves the middle of the blend: `color2` sits at the
/// end for the default 0, at the middle for 50%, and at the start — the ramp
/// reversed — for 100%, with `color` filling out either side. VML states it as a
/// band of one colour between two of the other, so the ramp gains a stop in the
/// middle rather than only sliding its two ends. A radial ramp's `focussize`
/// instead sizes the inner region the first stop fills, which is how Word makes
/// the start of a radial fade a disc rather than a point. The split is VML's own:
/// `focus` speaks for the linear type, while a radial fill's centre is
/// `focusposition` and its start region `focussize`.
fn gradient(fill: &Node, node: &Node, frame: Frame, radial: bool) -> Option<paint::Gradient> {
    let first = fill
        .attr("color")
        .and_then(parse_color)
        .or_else(|| node.attr("fillcolor").and_then(parse_color));
    let second = fill.attr("color2").and_then(parse_color);
    if first.is_none() && second.is_none() {
        return None;
    }
    let first = paint::GradientStop {
        position: 0.0,
        color: first.unwrap_or(Color::WHITE),
        alpha: fill.attr("opacity").map(opacity).unwrap_or(1.0),
    };
    let second = paint::GradientStop {
        position: 1.0,
        color: second.unwrap_or(Color::WHITE),
        alpha: fill.attr("opacity2").map(opacity).unwrap_or(1.0),
    };
    let kind = if radial {
        let center = focus_position(fill, frame).unwrap_or_else(|| frame.center());
        // The ramp reaches the frame's farthest edge on each axis, so a focus
        // moved off-centre is still the shape the file fills.
        let radius = (
            (frame.right() - center.0)
                .max(center.0 - frame.left)
                .max(1e-6),
            (frame.bottom() - center.1)
                .max(center.1 - frame.top)
                .max(1e-6),
        );
        let focus = focus_size(fill, frame);
        paint::GradientKind::Radial {
            center,
            radius,
            // An inner region wider than the ramp can reach would have no edge
            // to run out from, so it stops at the frame's farthest one.
            focus: (focus.0.min(radius.0), focus.1.min(radius.1)),
        }
    } else {
        // VML's angle is degrees clockwise from the horizontal, taken in the
        // shape's own proportions, exactly as DrawingML's scaled `a:lin` angle.
        paint::GradientKind::Linear {
            frame,
            angle: fill.attr("angle").and_then(degrees).unwrap_or(0.0),
            scaled: true,
        }
    };
    let stops = if radial {
        vec![first, second]
    } else {
        let focus = fill
            .attr("focus")
            .and_then(share)
            .map(|focus| focus.clamp(-1.0, 1.0))
            .unwrap_or(0.0);
        axial(&first, &second, focus)
    };
    Some(paint::Gradient { stops, kind })
}

/// The stops a linear ramp with a `focus` runs through.
///
/// The blend's middle sits at `1 - focus` for a positive focus, where the second
/// colour is, and at `-focus` for a negative one, where the first colour is — the
/// two readings VML's own examples give: 50% puts the second colour in the middle
/// with the first at both ends, -50% the other way round, and ±100% reverses the
/// whole ramp. A middle that lands on an end collapses to the two-stop ramp, so
/// the default focus of 0 writes exactly what it always did.
fn axial(
    first: &paint::GradientStop,
    second: &paint::GradientStop,
    focus: f64,
) -> Vec<paint::GradientStop> {
    let (outer, inner, middle) = if focus < 0.0 {
        (second, first, -focus)
    } else {
        (first, second, 1.0 - focus)
    };
    let mut stops = Vec::with_capacity(3);
    if middle > 0.0 {
        stops.push(paint::GradientStop {
            position: 0.0,
            ..*outer
        });
    }
    stops.push(paint::GradientStop {
        position: middle.clamp(0.0, 1.0),
        ..*inner
    });
    if middle < 1.0 {
        stops.push(paint::GradientStop {
            position: 1.0,
            ..*outer
        });
    }
    stops
}

/// The inner region a radial gradient starts from, as radii: `focussize` is the
/// region's width and height as percentages of the shape's, so a 50% region on
/// a 40 pt box is 20 pt across and reaches 10 pt from the centre. The default
/// 0,0 is no region at all — the ramp begins at the focus point.
fn focus_size(fill: &Node, frame: Frame) -> (f64, f64) {
    let Some(raw) = fill.attr("focussize") else {
        return (0.0, 0.0);
    };
    let mut parts = raw.split([',', ' ']).filter(|part| !part.trim().is_empty());
    let width = parts.next().and_then(share).unwrap_or(0.0);
    // One number states a square region, the way a single length does.
    let height = parts.next().and_then(share).unwrap_or(width);
    (
        width.max(0.0) * frame.width / 2.0,
        height.max(0.0) * frame.height / 2.0,
    )
}

/// The centre a radial gradient runs out from: `focusposition` is a pair of
/// percentages of the shape's width and height.
fn focus_position(fill: &Node, frame: Frame) -> Option<(f64, f64)> {
    let raw = fill.attr("focusposition")?;
    let mut parts = raw.split([',', ' ']).filter(|part| !part.trim().is_empty());
    let x = share(parts.next()?)?;
    let y = parts.next().and_then(share).unwrap_or(0.5);
    Some((frame.left + x * frame.width, frame.top + y * frame.height))
}

/// A percentage attribute as the fraction of the frame it names.
fn share(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    match raw.strip_suffix('%') {
        Some(number) => number.trim().parse::<f64>().ok().map(|value| value / 100.0),
        None => raw.parse::<f64>().ok(),
    }
}

/// A VML opacity, as a fraction: `0.5`, `50%` and a number are all read, and
/// anything outside 0..=1 is clamped to it.
fn opacity(raw: &str) -> f64 {
    let raw = raw.trim();
    let value = match raw.strip_suffix('%') {
        Some(number) => number.trim().parse::<f64>().ok().map(|value| value / 100.0),
        None => raw.parse::<f64>().ok(),
    };
    value.unwrap_or(1.0).clamp(0.0, 1.0)
}

fn degrees(raw: &str) -> Option<f64> {
    raw.trim()
        .trim_end_matches("deg")
        .trim()
        .parse::<f64>()
        .ok()
}

/// The shadow a shape casts, or `None` when it casts none.
///
/// `v:shadow` states a single offset copy of the silhouette: `on="t"`, an
/// `offset` in points, a `color`, and an `opacity`. Word's default is a grey
/// copy two points down and right; a shadow that states no opacity is solid.
fn shadow(node: &Node) -> Option<VmlShadow> {
    let shadow = node.child("shadow")?;
    if !truthy(shadow.attr("on")) {
        return None;
    }
    Some(VmlShadow {
        offset: shadow
            .attr("offset")
            .map(shadow_offset)
            .unwrap_or((2.0, 2.0)),
        color: shadow.attr("color").and_then(parse_color).unwrap_or(Color {
            r: 128,
            g: 128,
            b: 128,
        }),
        alpha: shadow.attr("opacity").map(opacity).unwrap_or(1.0),
    })
}

/// VML's boolean spelling: `t`, `true` and `1` say yes.
fn truthy(raw: Option<&str>) -> bool {
    raw.map(|raw| matches!(raw.trim().to_ascii_lowercase().as_str(), "t" | "true" | "1"))
        .unwrap_or(false)
}

/// A shadow's `offset` — a pair of lengths, in points when unitless.
fn shadow_offset(raw: &str) -> (f64, f64) {
    let mut parts = raw.split([',', ' ']).filter(|part| !part.trim().is_empty());
    let x = parts
        .next()
        .and_then(parse_length)
        .map(length_in_points)
        .unwrap_or(0.0);
    let y = parts
        .next()
        .and_then(parse_length)
        .map(length_in_points)
        .unwrap_or(x);
    (x, y)
}

/// The arrowheads a shape's `v:stroke` puts on the ends of its open lines.
///
/// VML names the head's shape (`block`, `classic`, `oval`, `diamond`, `open`)
/// and sizes it with `startarrowwidth`/`endarrowwidth` (`narrow`, `medium`,
/// `wide`) and `startarrowlength`/`endarrowlength` (`short`, `medium`, `long`),
/// in multiples of the line's own width.
fn arrow_ends(node: &Node, width: f64) -> (Option<ArrowEnd>, Option<ArrowEnd>) {
    let Some(stroke) = node.child("stroke") else {
        return (None, None);
    };
    (
        arrow_end(
            stroke.attr("startarrow"),
            stroke.attr("startarrowwidth"),
            stroke.attr("startarrowlength"),
            width,
        ),
        arrow_end(
            stroke.attr("endarrow"),
            stroke.attr("endarrowwidth"),
            stroke.attr("endarrowlength"),
            width,
        ),
    )
}

fn arrow_end(
    kind: Option<&str>,
    width_name: Option<&str>,
    length_name: Option<&str>,
    line_width: f64,
) -> Option<ArrowEnd> {
    let kind = match kind?.trim().to_ascii_lowercase().as_str() {
        "block" => paint::ArrowKind::Triangle,
        // Word's classic head is the concave one DrawingML calls stealth.
        "classic" => paint::ArrowKind::Stealth,
        "diamond" => paint::ArrowKind::Diamond,
        "oval" => paint::ArrowKind::Oval,
        "open" | "arrow" => paint::ArrowKind::Open,
        // `none`, and any name this does not know, draws nothing rather than a
        // head the file did not ask for.
        _ => return None,
    };
    let factor = |name: Option<&str>, small: &str, large: &str| match name.map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case(small) => 2.0,
        Some(value) if value.eq_ignore_ascii_case(large) => 5.0,
        // `medium`, and the default when the attribute is absent.
        _ => 3.0,
    };
    // A hairline still has a visible head, so it is sized from the smallest
    // width the applications use rather than from zero.
    let base = line_width.max(0.5);
    Some(ArrowEnd {
        kind,
        width: factor(width_name, "narrow", "wide") * base,
        length: factor(length_name, "short", "long") * base,
    })
}

/// The colour a shape is filled with, or `None` when it is not filled.
///
/// Word writes a themed colour with its theme index after it
/// (`fillcolor="#156082 [3204]"`); the index is a rendering hint, so only the
/// colour is read. This is the first colour of a gradient too, which is what a
/// WordArt's letters are painted in.
fn fill_color(node: &Node) -> Option<Color> {
    if node.is("filled") == Some(false) {
        return None;
    }
    if let Some(fill) = node.child("fill") {
        if fill.is("on") == Some(false) {
            return None;
        }
        if let Some(color) = fill.attr("color").and_then(parse_color) {
            return Some(color);
        }
    }
    node.attr("fillcolor")
        .and_then(parse_color)
        .or(Some(Color::WHITE))
}

fn stroke_color(node: &Node) -> Option<Color> {
    if node.is("stroked") == Some(false) {
        return None;
    }
    if let Some(stroke) = node.child("stroke") {
        if let Some(color) = stroke.attr("color").and_then(parse_color) {
            return Some(color);
        }
    }
    node.attr("strokecolor")
        .and_then(parse_color)
        .or(Some(Color::BLACK))
}

fn stroke_width(node: &Node) -> f64 {
    let raw = node.attr("strokeweight").or_else(|| {
        node.child("stroke")
            .and_then(|stroke| stroke.attr("weight"))
    });
    match raw.and_then(parse_length) {
        Some(Length::Points(value)) => value,
        Some(Length::Local(value)) => value / EMU_PER_POINT,
        None => DEFAULT_STROKE_PT,
    }
}

fn parse_color(raw: &str) -> Option<Color> {
    let token = raw.split_whitespace().next()?.trim();
    if token.eq_ignore_ascii_case("none") {
        return None;
    }
    if let Some(hex) = token.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        return Some(Color {
            r: (value >> 16) as u8,
            g: (value >> 8) as u8,
            b: value as u8,
        });
    }
    match token.to_ascii_lowercase().as_str() {
        "white" => Some(Color::WHITE),
        "black" => Some(Color::BLACK),
        "red" => Some(Color { r: 255, g: 0, b: 0 }),
        "green" => Some(Color { r: 0, g: 128, b: 0 }),
        "blue" => Some(Color { r: 0, g: 0, b: 255 }),
        "yellow" => Some(Color {
            r: 255,
            g: 255,
            b: 0,
        }),
        "gray" | "grey" => Some(Color {
            r: 128,
            g: 128,
            b: 128,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VML: &str = r#"xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:w10="urn:schemas-microsoft-com:office:word""#;

    fn pict(inner: &str) -> Vec<u8> {
        format!("<w:pict {VML}>{inner}</w:pict>").into_bytes()
    }

    fn pixels(picture: &VmlPicture) -> image::RgbaImage {
        image::load_from_memory(&picture.png)
            .expect("the drawing encodes as a PNG")
            .to_rgba8()
    }

    fn at(picture: &VmlPicture, x: u32, y: u32) -> [u8; 4] {
        pixels(picture).get_pixel(x, y).0
    }

    /// A filled box is a picture the size of its frame, its fill the colour the
    /// file names — at 144 dpi, two device pixels per point.
    #[test]
    fn a_filled_box_becomes_a_picture_of_its_frame() {
        let picture = render(&pict(
            r##"<v:rect style="width:40pt;height:20pt" fillcolor="#ff0000" stroked="f"/>"##,
        ))
        .expect("a box with a fill is a picture");
        assert_eq!((picture.width_pt, picture.height_pt), (40.0, 20.0));
        let image = pixels(&picture);
        assert_eq!(image.dimensions(), (80, 40));
        assert_eq!(at(&picture, 40, 20), [255, 0, 0, 255]);
    }

    /// A VML gradient is two colours ramped across the shape rather than its
    /// first colour painted flat: red at one edge, blue at the other, the mix
    /// between — the same box a `v:fill type="gradient"` describes with an
    /// angle. `gradientRadial` runs out from its focus instead.
    #[test]
    fn a_gradient_fills_the_shape_across_it() {
        let picture = render(&pict(
            r##"<v:rect style="width:40pt;height:20pt" stroked="f">
                  <v:fill type="gradient" color="#ff0000" color2="#0000ff" angle="0"/>
                </v:rect>"##,
        ))
        .expect("a gradient is ink");
        assert_eq!((picture.width_pt, picture.height_pt), (40.0, 20.0));
        let left = at(&picture, 2, 20);
        let right = at(&picture, 77, 20);
        let middle = at(&picture, 40, 20);
        assert!(
            left[0] > 230 && left[2] < 25,
            "the ramp starts red: {left:?}"
        );
        assert!(right[2] > 230 && right[0] < 25, "it ends blue: {right:?}");
        assert!(
            (i32::from(middle[0]) - 127).abs() < 12 && (i32::from(middle[2]) - 127).abs() < 12,
            "and is halfway between them in the middle: {middle:?}"
        );

        let radial = render(&pict(
            r##"<v:rect style="width:40pt;height:20pt" stroked="f">
                  <v:fill type="gradientRadial" color="#ff0000" color2="#0000ff"/>
                </v:rect>"##,
        ))
        .expect("a radial gradient is ink");
        let centre = at(&radial, 40, 20);
        let edge = at(&radial, 2, 20);
        assert!(
            centre[0] > 230 && centre[2] < 25,
            "the radial ramp starts at its focus: {centre:?}"
        );
        assert!(
            edge[2] > 200 && edge[0] < 55,
            "and has run out to blue by the edge: {edge:?}"
        );
    }

    /// `focus` slides the blend's middle along a linear ramp rather than leaving
    /// it at the far end: 50% puts the second colour in the middle of the shape
    /// and the first at both ends, -50% the other way round, and 100% reverses
    /// the ramp — the band VML states with one colour inside another.
    #[test]
    fn a_linear_focus_moves_the_middle_of_the_ramp() {
        let banded = |focus: &str| {
            render(&pict(&format!(
                r##"<v:rect style="width:40pt;height:20pt" stroked="f">
                      <v:fill type="gradient" color="#ff0000" color2="#0000ff"
                              angle="0" focus="{focus}"/>
                    </v:rect>"##
            )))
            .expect("a focused gradient is ink")
        };
        let red = |pixel: [u8; 4]| pixel[0] > 230 && pixel[2] < 25;
        let blue = |pixel: [u8; 4]| pixel[2] > 230 && pixel[0] < 25;

        let middle = banded("50%");
        assert!(red(at(&middle, 2, 20)), "red at the start");
        assert!(
            blue(at(&middle, 40, 20)),
            "blue in the middle: {:?}",
            at(&middle, 40, 20)
        );
        assert!(red(at(&middle, 77, 20)), "and red again at the end");

        let inner = banded("-50%");
        assert!(blue(at(&inner, 2, 20)), "the other way round, blue leads");
        assert!(red(at(&inner, 40, 20)), "red in the middle");
        assert!(blue(at(&inner, 77, 20)), "and blue at the far end");

        let reversed = banded("100%");
        assert!(blue(at(&reversed, 2, 20)), "100% reverses the ramp");
        assert!(red(at(&reversed, 77, 20)), "so it runs blue to red");
    }

    /// `focussize` makes the ramp start from a region rather than a point: a
    /// 50% focus on a 40 × 20 pt box is a 20 × 10 pt ellipse, so a point inside
    /// it is still the start colour where the same box without one has already
    /// begun to mix.
    #[test]
    fn a_radial_focus_size_holds_the_start_colour_flat() {
        let picture = |focus: &str| {
            render(&pict(&format!(
                r##"<v:rect style="width:40pt;height:20pt" stroked="f">
                      <v:fill type="gradientRadial" color="#ff0000" color2="#0000ff"
                              focusposition="50%,50%" {focus}/>
                    </v:rect>"##
            )))
            .expect("a radial gradient is ink")
        };
        let sized = picture("focussize=\"50%,50%\"");
        assert_eq!(
            at(&sized, 40, 20),
            [255, 0, 0, 255],
            "the middle is the start colour"
        );
        assert_eq!(
            at(&sized, 55, 20),
            [255, 0, 0, 255],
            "and stays that colour 15 px out, inside the 20 px focus"
        );
        let edge = at(&sized, 78, 20);
        assert!(
            edge[2] > 200 && edge[0] < 55,
            "while the edge has still run out to the second colour: {edge:?}"
        );

        let pointed = picture("");
        let mixed = at(&pointed, 55, 20);
        assert!(
            mixed[0] < 200 && mixed[2] > 50,
            "without a focus region the ramp has started by then: {mixed:?}"
        );
    }

    /// A `v:shadow` is the shape's silhouette moved by its offset and laid down
    /// under the ink, so the picture has to hold the shadow as well as the box:
    /// grey past the box's edge, nothing above or left of it.
    #[test]
    fn a_shadow_is_painted_under_the_shape() {
        let picture = render(&pict(
            r##"<v:rect style="width:30pt;height:20pt" fillcolor="#00aa00" stroked="f">
                  <v:shadow on="t" color="#000000" opacity="0.5" offset="5pt,5pt"/>
                </v:rect>"##,
        ))
        .expect("a shadow is ink");
        assert_eq!(
            (picture.width_pt, picture.height_pt),
            (35.0, 25.0),
            "the canvas holds the shadow past the box"
        );
        assert_eq!(at(&picture, 30, 20), [0, 170, 0, 255], "the box's fill");
        assert_eq!(
            at(&picture, 68, 6),
            [255, 255, 255, 255],
            "right of the box and above the shadow, where neither is"
        );
        let cast = at(&picture, 65, 45);
        assert!(
            cast[0].abs_diff(cast[1]) < 4 && cast[1].abs_diff(cast[2]) < 4,
            "the shadow is grey, not the box's colour: {cast:?}"
        );
        assert!(
            (110..=145).contains(&cast[0]),
            "and it is black at half opacity over white: {cast:?}"
        );
    }

    /// A `v:stroke` arrowhead is the ink at the end of an open line: a filled
    /// head at the line's own end point, wider than the line, with the stroke
    /// stopped at its base and the canvas large enough to hold it.
    #[test]
    fn a_line_ends_in_the_arrowhead_it_names() {
        let picture = render(&pict(
            r##"<v:line from="0,0" to="100,0" strokecolor="#1f4e79" strokeweight="2pt">
                  <v:stroke endarrow="block" endarrowwidth="medium" endarrowlength="medium"/>
                </v:line>"##,
        ))
        .expect("an arrowed line is a picture");
        assert_eq!((picture.width_pt, picture.height_pt), (114.0, 14.0));
        let image = pixels(&picture);
        assert_eq!(image.dimensions(), (228, 28));
        assert_eq!(at(&picture, 100, 14), [31, 78, 121, 255], "the line");
        assert_eq!(
            at(&picture, 210, 14),
            [31, 78, 121, 255],
            "the head's filled middle"
        );
        assert_eq!(
            at(&picture, 206, 11),
            [31, 78, 121, 255],
            "inside the head, past the line's edge"
        );
        assert_eq!(at(&picture, 220, 14), [255, 255, 255, 255], "past the tip");
    }

    /// A group's coordinates are its own: `coordsize` spans the frame, so a line
    /// from the space's corner to its opposite corner is the frame's diagonal.
    #[test]
    fn a_group_places_its_children() {
        let picture = render(&pict(
            r##"<v:group style="width:100pt;height:50pt" coordsize="1000,500" coordorigin="0,0">
                  <v:line from="0,0" to="1000,500" strokecolor="#0000ff" strokeweight="2pt"/>
                </v:group>"##,
        ))
        .expect("a group of shapes is a picture");
        assert_eq!((picture.width_pt, picture.height_pt), (100.0, 50.0));
        assert_eq!(at(&picture, 2, 1), [0, 0, 255, 255], "the line's start");
        assert_eq!(at(&picture, 197, 98), [0, 0, 255, 255], "the line's end");
        assert_eq!(at(&picture, 150, 20), [255, 255, 255, 255], "off the line");
    }

    /// WordArt's string exists only in `v:textpath`, so it is drawn — stretched
    /// across the shape, in the fill the file gives the letters.
    #[test]
    fn wordart_text_is_drawn() {
        let picture = render(&pict(
            r##"<v:shape style="width:120pt;height:30pt" fillcolor="#c00000" stroked="f">
                  <v:textpath string="Hi" style="font-size:12pt"/>
                </v:shape>"##,
        ))
        .expect("WordArt is a picture");
        let image = pixels(&picture);
        assert_eq!(image.dimensions(), (240, 60));
        let red = image
            .pixels()
            .filter(|pixel| pixel.0 == [192, 0, 0, 255])
            .count();
        assert!(red > 20, "the letters drew {red} pixel(s)");
    }

    /// A preset shape's outline is its `path`, with the preset's formulas
    /// resolved: the triangle's apex is top centre, its base the bottom edge.
    #[test]
    fn a_preset_path_is_drawn() {
        let picture = render(&pict(
            r##"<v:shapetype id="_x0000_t5" coordsize="21600,21600" adj="10800"
                              path="m@0,l,21600r21600,xe">
                  <v:formulas><v:f eqn="val #0"/></v:formulas>
                </v:shapetype>
                <v:shape type="#_x0000_t5" style="width:60pt;height:50pt"
                         fillcolor="#00aa00" stroked="f"/>"##,
        ))
        .expect("a preset shape is a picture");
        assert_eq!((picture.width_pt, picture.height_pt), (60.0, 50.0));
        assert_eq!(
            at(&picture, 60, 80),
            [0, 170, 0, 255],
            "inside the triangle"
        );
        assert_eq!(at(&picture, 4, 4), [255, 255, 255, 255], "above its slope");
    }

    /// Word's horizontal rule is a paragraph's line, drawn from VML that is not
    /// a picture: it is left out rather than turned into one.
    #[test]
    fn a_horizontal_rule_is_not_a_picture() {
        assert!(render(&pict(
            r##"<v:rect style="width:300pt;height:1.5pt" o:hr="t" fillcolor="#aca899" stroked="f"/>"##,
        ))
        .is_none());
    }

    /// A shape with nothing to draw — no path, no fill, no stroke — is no
    /// picture, rather than a white rectangle named as one.
    #[test]
    fn a_shape_with_no_ink_is_not_a_picture() {
        assert!(render(&pict(
            r#"<v:shape style="width:10pt;height:10pt" filled="f" stroked="f"/>"#
        ))
        .is_none());
        // Nor is markup that is not a drawing at all.
        assert!(render(b"not xml").is_none());
        assert!(render(&pict("")).is_none());
    }

    /// A length in a `style` attribute is read in its unit; one without a unit
    /// is in the space it sits in.
    #[test]
    fn lengths_are_read_in_their_units() {
        let points = |raw: &str| match parse_length(raw) {
            Some(Length::Points(value)) => Some(value),
            _ => None,
        };
        assert_eq!(points("12pt"), Some(12.0));
        assert_eq!(points("1in"), Some(72.0));
        assert_eq!(points("2.54cm"), Some(72.0));
        assert_eq!(points("25.4mm"), Some(72.0));
        assert_eq!(points("12pc"), Some(144.0));
        assert_eq!(points("12700emu"), Some(1.0));
        assert!(matches!(parse_length("40"), Some(Length::Local(40.0))));
        // A percentage is no length this can place.
        assert!(parse_length("50%").is_none());
        assert!(parse_length("wide").is_none());
    }

    /// A path's omitted coordinates are zero, which is how Word writes the
    /// rectangle `m,l,21600r21600,l21600,xe`, and a relative command moves the
    /// pen.
    #[test]
    fn a_path_reads_omitted_coordinates_as_zero() {
        let adjust = Adjust {
            handles: Vec::new(),
            formulas: Vec::new(),
            size: (21600.0, 21600.0),
        };
        let space = Space {
            sx: 1.0,
            sy: 1.0,
            ox: 0.0,
            oy: 0.0,
            dx: 0.0,
            dy: 0.0,
        };
        let contours = parse_path("m,l,21600r21600,l21600,xe", &adjust, &space);
        assert_eq!(contours.len(), 1);
        let points: Vec<(f64, f64)> = contours[0]
            .points
            .iter()
            .map(|point| (point.x, point.y))
            .collect();
        assert_eq!(
            points,
            vec![
                (0.0, 0.0),
                (0.0, 21600.0),
                (21600.0, 21600.0),
                (21600.0, 0.0)
            ]
        );
        assert!(contours[0].closed);
    }
}
