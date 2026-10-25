//! Reading a shape's own markup into ink: the geometry, the fill, the line, and
//! the theme the colours come from.
//!
//! The two office formats name the same body differently — Word's `wps:spPr`
//! and PowerPoint's `p:spPr`, Word's `wps:txbx/w:txbxContent` and PowerPoint's
//! `p:txBody` — so the reading here works on local names and takes either. A
//! shape is `sp`, `wsp` or `cxnSp`; a group is `wgp`, `wpc` or `grpSp`; the text
//! of a shape is `txbxContent` or `txBody`, and is never painted (it is the
//! document's own text) nor descended into.

use std::collections::HashMap;

use pdfextractor_metafile as metafile;
use pdfextractor_metafile::Color;

use crate::paint::{self, ellipse, polygon, rounded, Contour, Frame, Node, Op, Space};

/// An EMU is 1/12700 of a point; the drawing's lengths are written in it.
const EMU_PER_POINT: f64 = 12700.0;

/// The stroke a shape has when neither it nor its theme style names a width:
/// DrawingML's default hairline.
const DEFAULT_STROKE_PT: f64 = 0.75;

/// The widths a theme's line styles declare, in EMU, indexed by `a:lnRef idx`
/// (index 0 is "no line").
const THEME_LINE_WIDTHS_EMU: [f64; 3] = [6350.0, 12700.0, 19050.0];

/// Shapes drawn into pixels, and the size they were drawn at.
pub struct Picture {
    pub png: Vec<u8>,
    /// The width of the ink, in points: what the drawing occupies, which is the
    /// frame of the shape or of the group that holds them.
    pub width_pt: f64,
    pub height_pt: f64,
}

/// The colours a package's theme declares, by slot (`dk1`, `lt1`, `accent1`, …),
/// and the adjustments each of its line styles applies to the colour a shape's
/// `a:lnRef` names.
///
/// A shape's fill is often `a:fillRef idx="1"` with an `a:schemeClr` child, so
/// the file states a theme slot rather than a colour. An absent or unreadable
/// theme resolves to nothing, and the shape paints only what it states itself.
#[derive(Default, Debug)]
pub struct Theme {
    colors: HashMap<String, Color>,
    /// `a:lnStyleLst` in index order: the adjustments its colour states
    /// (`shade`, `satMod`, …), which every application applies to a line that
    /// takes its colour from `a:lnRef`.
    lines: Vec<Vec<(String, f64)>>,
}

impl Theme {
    /// The theme of a package's `word/theme/theme1.xml`, or an empty theme.
    pub fn parse(xml: &[u8]) -> Theme {
        let Some(root) = paint::parse(xml) else {
            return Theme::default();
        };
        let Some(scheme) = find(&root, "clrScheme") else {
            return Theme::default();
        };
        let mut colors = HashMap::new();
        for slot in &scheme.children {
            let Some(color) = slot.children.first().and_then(|node| color(node, None)) else {
                continue;
            };
            colors.insert(slot.name.clone(), color);
        }
        let mut lines = Vec::new();
        if let Some(styles) = find(&root, "lnStyleLst") {
            for style in &styles.children {
                let adjustments = style
                    .child("solidFill")
                    .and_then(|fill| fill.children.first())
                    .map(|colour| colour.children.iter().filter_map(adjustment).collect())
                    .unwrap_or_default();
                lines.push(adjustments);
            }
        }
        Theme { colors, lines }
    }

    /// The colour the theme's line style at `index` makes of `base`.
    ///
    /// The theme's own line styles are where a default shape's outline comes
    /// from: the first shades the accent and lifts its saturation, so a line
    /// of the accent's colour is not the fill's colour — which is what makes a
    /// mark inside a shape visible against the shape itself.
    fn line(&self, index: i32, base: Color) -> Color {
        let Some(adjustments) = usize::try_from(index)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| self.lines.get(index))
        else {
            return base;
        };
        adjusted(base, adjustments)
    }

    fn scheme(&self, name: &str) -> Option<Color> {
        let name = match name {
            // The aliases a shape uses for text and background: `tx1` is the
            // theme's dark 1, `bg1` its light 1.
            "tx1" => "dk1",
            "bg1" => "lt1",
            "tx2" => "dk2",
            "bg2" => "lt2",
            other => other,
        };
        self.colors.get(name).copied()
    }
}

/// Draw the shapes of the `w:drawing` in `drawing`, or `None` when it holds
/// nothing a picture can be made of: no shape this understands, no size, or no
/// ink.
/// Paint the shapes of a Word `w:drawing` — the markup a `w:p` holds under its
/// shape, whole — or `None` when it holds nothing a picture can be made of: no
/// shape this understands, no size, or no ink.
pub fn render(drawing: &[u8], theme: &Theme) -> Option<Picture> {
    render_with(drawing, theme, &NoMedia)
}

/// Paint the shapes of a `w:drawing` whose fills name media, drawing each fill
/// through the shape's own geometry.
///
/// A shape can be filled with a picture rather than a colour, and a fill is
/// part of the shape: the application draws the picture clipped to the shape's
/// outline, under its line, so a reader that reported the bare picture at the
/// shape's frame was showing what the fill *contains* rather than what the page
/// shows. The caller holds the package, so it resolves each `r:embed` it is
/// asked for and hands the bytes over; the painter decodes them, draws the
/// picture through the shape and keeps the shape's line on top.
pub fn render_with(drawing: &[u8], theme: &Theme, media: &dyn Media) -> Option<Picture> {
    let root = paint::parse(drawing)?;
    // `wp:extent` is the drawing's own box: a shape that states no transform of
    // its own is placed in it, which is how a `wps:wsp` written straight into
    // the drawing is meant.
    let extent = find(&root, "extent").and_then(|extent| {
        let width = extent.attr("cx")?.parse::<f64>().ok()? / EMU_PER_POINT;
        let height = extent.attr("cy")?.parse::<f64>().ok()? / EMU_PER_POINT;
        (width > 0.0 && height > 0.0).then_some(Frame {
            left: 0.0,
            top: 0.0,
            width,
            height,
        })
    });
    let mut renderer = Renderer {
        ops: Vec::new(),
        bounds: None,
        theme,
        extent,
        media,
    };
    renderer.walk(&root, &Space::POINTS, true);
    renderer.finish()
}

/// Paint one shape — or one group, with everything in it — as the slide or the
/// document holds it.
///
/// A slide's shapes are already in the reader's tree, one element each, so a
/// caller that has that tree paints the shape itself rather than searching for
/// it again. There is no drawing extent to fall back on: a PowerPoint shape
/// always states its own transform, and one that does not is not placed at all.
pub fn render_node(node: &Node, theme: &Theme) -> Option<Picture> {
    render_node_with(node, theme, &NoMedia)
}

/// Paint one shape — or one group, with everything in it — resolving the media
/// its fills name. The caller supplies the package's pictures; see
/// [`render_with`].
pub fn render_node_with(node: &Node, theme: &Theme, media: &dyn Media) -> Option<Picture> {
    let mut renderer = Renderer {
        ops: Vec::new(),
        bounds: None,
        theme,
        extent: None,
        media,
    };
    renderer.walk_one(node, &Space::POINTS, true);
    renderer.finish()
}

/// Whether a drawing states a shape filled with a picture.
///
/// A fill is part of the shape that holds it, so a reader that has the package
/// paints such a drawing whole rather than handing over the fill's bare picture.
/// The test is the shape's own body — an `a:blipFill` under a `wps:spPr` or a
/// `p:spPr` — and not any `blipFill`: a picture is written as a `pic:pic` with a
/// `pic:blipFill` of its own, and that is a picture, not a fill.
///
/// `false` when the markup does not parse, so a drawing this reader cannot read
/// is left to the reader that can.
pub fn paints_a_picture_fill(drawing: &[u8]) -> bool {
    fn filled(node: &Node) -> bool {
        if is_shape(&node.name)
            && node
                .child("spPr")
                .is_some_and(|properties| properties.child("blipFill").is_some())
        {
            return true;
        }
        node.children.iter().any(filled)
    }
    paint::parse(drawing).is_some_and(|root| filled(&root))
}

/// The pictures a shape's fills name.
///
/// A fill states its picture the way a picture states itself — a relationship
/// id on its `a:blip`, and the drawing in an `asvg:svgBlip` extension beside it
/// — and only the reader that holds the package can resolve one. The painter
/// asks for the media a shape names and draws whatever comes back.
pub trait Media {
    /// The bytes and media type of the picture a relationship id names.
    fn picture(&self, id: &str) -> Option<(String, Vec<u8>)>;
}

/// A drawing whose fills name no media: the painter then draws a fill only
/// where the shape states a colour or a gradient.
pub struct NoMedia;

impl Media for NoMedia {
    fn picture(&self, _id: &str) -> Option<(String, Vec<u8>)> {
        None
    }
}

/// A shape's own transform: its box, and what to do with the geometry inside it.
#[derive(Debug, Default)]
struct Xfrm {
    flip_h: bool,
    flip_v: bool,
    /// A turn in degrees, clockwise as the file states it (in 1/60000 of one).
    rotation: f64,
}

struct Renderer<'a> {
    ops: Vec<Op>,
    bounds: Option<Frame>,
    theme: &'a Theme,
    /// The drawing's own box, for a top-level shape that states no transform.
    extent: Option<Frame>,
    /// The package's pictures, for the fills that name one.
    media: &'a dyn Media,
}

impl Renderer<'_> {
    /// Everything collected so far, as a PNG.
    fn finish(self) -> Option<Picture> {
        let (png, width_pt, height_pt) = paint::rasterize(&self.ops, self.bounds)?;
        Some(Picture {
            png,
            width_pt,
            height_pt,
        })
    }

    fn walk(&mut self, node: &Node, space: &Space, top: bool) {
        for child in &node.children {
            match child.name.as_str() {
                // A shape's text is the document's own text, and a picture
                // inside it is placed by its own drawing.
                name if is_text(name) => {}
                name if is_shape(name) => self.draw_shape(child, space, top),
                name if is_group(name) => self.draw_group(child, space, top),
                // A shape is placed in EMU until a group gives it a space of
                // its own, however deep the wrapper around it is.
                _ => self.walk(child, space, top),
            }
        }
    }

    fn draw_shape(&mut self, node: &Node, space: &Space, top: bool) {
        let Some(sp_pr) = node.child("spPr") else {
            return;
        };
        let Some((frame, xfrm)) = self.frame(sp_pr, space, top) else {
            return;
        };

        let fill = fill(sp_pr, node, frame, xfrm.rotation, self.theme, self.media);
        let stroke = stroke(node, sp_pr, self.theme);
        let shadow = shadow(sp_pr, self.theme);
        let mut contours = match outline(sp_pr, frame) {
            Some(contours) => contours,
            // A body that names no geometry at all — a PowerPoint placeholder
            // or text box that only says how it is painted — is the rectangle
            // the format draws by default. Only a shape that states ink is
            // taken that way; one that states none stays no picture.
            None if fill.is_some() || stroke.is_some() => vec![polygon(&[
                (frame.left, frame.top),
                (frame.right(), frame.top),
                (frame.right(), frame.bottom()),
                (frame.left, frame.bottom()),
            ])],
            None => return,
        };
        transform(&mut contours, frame, &xfrm);
        let filled: Vec<Vec<metafile::Point>> = contours
            .iter()
            .filter(|contour| contour.filled && contour.points.len() >= 2)
            .map(|contour| contour.points.clone())
            .collect();
        let mut drew = false;

        // The shadow goes down first: it is behind everything the shape shows.
        if let Some(shadow) = shadow {
            if !filled.is_empty() {
                self.ops.push(Op::Shadow {
                    contours: filled.clone(),
                    offset: shadow.offset,
                    blur: shadow.blur,
                    color: shadow.color,
                    alpha: shadow.alpha,
                });
                drew = true;
            }
        }

        if let Some(fill) = fill {
            if !filled.is_empty() {
                self.ops.push(match fill {
                    Fill::Solid(color) => Op::Fill {
                        contours: filled.clone(),
                        color,
                    },
                    Fill::Gradient(gradient) => Op::GradientFill {
                        contours: filled.clone(),
                        gradient,
                    },
                    // A picture fill is the shape's own outline filled with an
                    // image: the geometry clips it, and the quad is the box it
                    // is stretched across, turned with the shape.
                    Fill::Picture(image) => Op::Picture {
                        contours: filled.clone(),
                        image,
                        quad: quad_of(frame, &xfrm),
                    },
                });
                drew = true;
            }
        }

        let mut stroke_width = 0.0f64;
        if let Some(line) = stroke {
            stroke_width = line.width;
            for contour in &contours {
                if contour.points.len() < 2 {
                    continue;
                }
                // An arrowhead sits at the end of an open line, tip on the
                // shape's own last point, and the line stops at its base: a
                // stroke run to the tip would show through a stealth or a
                // diamond.
                let path = if contour.closed {
                    contour.points.clone()
                } else {
                    paint::shortened(&contour.points, line.head, line.tail)
                };
                self.ops.push(Op::Stroke {
                    points: path,
                    closed: contour.closed,
                    width: line.width,
                    color: line.color,
                });
                if !contour.closed {
                    self.arrow_ends(&contour.points, &line);
                }
                drew = true;
            }
        }
        if drew {
            let mut ink = self.shape_box(&contours, frame, stroke_width);
            // A shadow and an arrowhead reach past the shape's own box, and the
            // canvas is only as large as the ink it holds.
            if let Some(shadow) = shadow {
                ink = ink.union(shadow.frame(frame));
            }
            if let Some(line) = stroke {
                let reach = line
                    .head
                    .iter()
                    .chain(line.tail.iter())
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
            }
            self.include(ink);
        }
    }

    /// The ink of a line's arrowheads: a head at each end of the line.
    fn arrow_ends(&mut self, points: &[metafile::Point], line: &StrokeInk) {
        if points.len() < 2 {
            return;
        }
        let (first, second) = (points[0], points[1]);
        let (last, before) = (points[points.len() - 1], points[points.len() - 2]);
        // Each end's direction points out of the line, so the head's tip is the
        // line's own end point and its base is a length back inside — which is
        // the neighbour seen from the end, not the end seen from the neighbour.
        if let Some(head) = line.head {
            if let Some(direction) = paint::unit(second, first) {
                self.ops
                    .extend(paint::arrowhead(first, direction, head, line.color));
            }
        }
        if let Some(tail) = line.tail {
            if let Some(direction) = paint::unit(before, last) {
                self.ops
                    .extend(paint::arrowhead(last, direction, tail, line.color));
            }
        }
    }

    /// The box a shape's ink occupies.
    ///
    /// A shape is placed by its frame; a straight line is the one shape whose
    /// frame has no area — a horizontal connector is written with a height of
    /// zero — and there the ink is the stroke, so the box is the stroke's own
    /// extent. Without that a connector would be a canvas of nothing, and a
    /// slide's lines are the ink a reader most needs to see.
    fn shape_box(&self, contours: &[Contour], frame: Frame, stroke: f64) -> Frame {
        if frame.width > 0.0 && frame.height > 0.0 {
            return frame;
        }
        let points: Vec<metafile::Point> = contours
            .iter()
            .flat_map(|contour| contour.points.iter().copied())
            .collect();
        match points.first() {
            Some(_) => paint::ink_frame(&points, stroke.max(0.0)),
            None => frame,
        }
    }

    fn draw_group(&mut self, node: &Node, parent: &Space, top: bool) {
        // A canvas keeps its shapes under `whole`; a plain group holds them as
        // its own children, with its transform in `grpSpPr`.
        let (group, children): (&Node, &[Node]) = match node.child("whole") {
            Some(whole) => (whole, whole.children.as_slice()),
            None => (node, node.children.as_slice()),
        };
        let xfrm = group.child("grpSpPr").and_then(|props| props.child("xfrm"));
        let Some(xfrm) = xfrm else {
            for child in children {
                self.walk_one(child, parent, top);
            }
            return;
        };
        let Some((frame, _)) = self.frame_of_xfrm(xfrm, parent, top) else {
            return;
        };
        // `chOff`/`chExt` are the coordinate space the children are written in;
        // the group's frame is where that space lands.
        let child_off = point_of(xfrm.child("chOff"), false).unwrap_or((0.0, 0.0));
        let child_ext = point_of(xfrm.child("chExt"), false).unwrap_or((frame.width, frame.height));
        if !(child_ext.0 > 0.0 && child_ext.1 > 0.0) {
            return;
        }
        let space = Space {
            sx: frame.width / child_ext.0,
            sy: frame.height / child_ext.1,
            ox: child_off.0,
            oy: child_off.1,
            dx: frame.left,
            dy: frame.top,
        };
        let drawn = self.ops.len();
        let bounds = self.bounds;
        for child in children {
            self.walk_one(child, &space, false);
        }
        // The group's frame is the picture's frame: its children are placed in
        // it, and a child that reaches past it is clipped by the application
        // too.
        if self.ops.len() > drawn {
            self.include(frame);
        } else {
            self.bounds = bounds;
        }
    }

    fn walk_one(&mut self, node: &Node, space: &Space, top: bool) {
        match node.name.as_str() {
            name if is_text(name) => {}
            name if is_shape(name) => self.draw_shape(node, space, top),
            name if is_group(name) => self.draw_group(node, space, top),
            _ => self.walk(node, space, top),
        }
    }

    fn frame(&self, sp_pr: &Node, space: &Space, top: bool) -> Option<(Frame, Xfrm)> {
        match sp_pr.child("xfrm") {
            Some(xfrm) => self.frame_of_xfrm(xfrm, space, top),
            // A shape with no transform of its own sits in the drawing's box.
            None if top => self.extent.map(|frame| (frame, Xfrm::default())),
            None => None,
        }
    }

    fn frame_of_xfrm(&self, xfrm: &Node, space: &Space, top: bool) -> Option<(Frame, Xfrm)> {
        let off = point_of(xfrm.child("off"), top)?;
        let ext = point_of(xfrm.child("ext"), top)?;
        // A straight line states no extent in the direction it does not run:
        // a horizontal connector's `ext` has no height at all. One axis of zero
        // is a shape that still has ink; two are a shape that is not there.
        if ext.0 < 0.0 || ext.1 < 0.0 || (ext.0 == 0.0 && ext.1 == 0.0) {
            return None;
        }
        let origin = space.point(off.0, off.1);
        let frame = Frame {
            left: origin.x,
            top: origin.y,
            width: ext.0 * space.sx,
            height: ext.1 * space.sy,
        };
        let rotation = xfrm
            .attr("rot")
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .map(|sixteenths| sixteenths / 60000.0)
            .unwrap_or(0.0);
        Some((
            frame,
            Xfrm {
                flip_h: flag(xfrm, "flipH"),
                flip_v: flag(xfrm, "flipV"),
                rotation,
            },
        ))
    }

    fn include(&mut self, frame: Frame) {
        self.bounds = Some(match self.bounds {
            Some(bounds) => bounds.union(frame),
            None => frame,
        });
    }
}

/// An `a:off`/`a:ext`/`a:chOff`/`a:chExt` point, in points at the top level
/// (where the file writes EMU) and in the parent's units inside a group.
fn point_of(node: Option<&Node>, top: bool) -> Option<(f64, f64)> {
    let node = node?;
    // `off`/`ext` spell their pair `x`/`y` and `cx`/`cy`; `chOff`/`chExt` use
    // the same two spellings.
    let x = node
        .attr("x")
        .or_else(|| node.attr("cx"))
        .and_then(|raw| raw.trim().parse::<f64>().ok())?;
    let y = node
        .attr("y")
        .or_else(|| node.attr("cy"))
        .and_then(|raw| raw.trim().parse::<f64>().ok())?;
    let scale = if top { 1.0 / EMU_PER_POINT } else { 1.0 };
    Some((x * scale, y * scale))
}

/// The shape's outline, from its preset geometry or its own path.
fn outline(sp_pr: &Node, frame: Frame) -> Option<Vec<Contour>> {
    if let Some(geometry) = sp_pr.child("prstGeom") {
        let preset = geometry.attr("prst")?;
        let adjust = Adjustments::new(geometry);
        return preset_outline(preset, frame, &adjust);
    }
    let custom = sp_pr.child("custGeom")?;
    custom_outline(custom, frame)
}

/// The adjustment handles a preset declares: `a:avLst/a:gd`, whose `fmla` is
/// `val N` with `N` in 1/100000 of the shape's size.
#[derive(Default)]
struct Adjustments {
    values: HashMap<String, f64>,
}

impl Adjustments {
    fn new(geometry: &Node) -> Adjustments {
        let mut values = HashMap::new();
        if let Some(list) = geometry.child("avLst") {
            for guide in list.children.iter().filter(|child| child.name == "gd") {
                let Some(name) = guide.attr("name") else {
                    continue;
                };
                let value = guide
                    .attr("fmla")
                    .and_then(|fmla| fmla.trim().strip_prefix("val "))
                    .and_then(|raw| raw.trim().parse::<f64>().ok())
                    .map(|value| value / 100000.0);
                if let Some(value) = value {
                    values.insert(name.to_string(), value);
                }
            }
        }
        Adjustments { values }
    }

    /// A handle, by any of the names a preset may give it (`adj`, `adj1` being
    /// the first one).
    fn get(&self, names: &[&str], default: f64) -> f64 {
        for name in names {
            if let Some(value) = self.values.get(*name) {
                return *value;
            }
        }
        self.values.get("adj").copied().unwrap_or(default)
    }
}

/// A preset outline in the shape's frame.
///
/// The presets are the ones an office package actually states: the basic
/// outlines, the regular polygons and stars, the arrows, the flowchart symbols,
/// the callouts, and the shapes an arc or a wave gives its character to. A
/// preset this does not know answers `None` — a box drawn for a star would be a
/// picture of something the file did not draw.
fn preset_outline(name: &str, frame: Frame, adjust: &Adjustments) -> Option<Vec<Contour>> {
    plain_outline(name, frame, adjust).or_else(|| curved_outline(name, frame, adjust))
}

/// The presets whose outline is corner to corner: boxes, polygons, arrows and
/// the flowchart symbols that are a polygon with a corner cut off.
fn plain_outline(name: &str, frame: Frame, adjust: &Adjustments) -> Option<Vec<Contour>> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let rect = || vec![polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)])];
    let contours = match name {
        // `textBox` is what a text box's geometry is called in some writers.
        "rect" | "textBox" | "flowChartProcess" => rect(),
        // A predefined process is a box with a bar down either side.
        "flowChartPredefinedProcess" => vec![
            polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)]),
            line(&[(x + w * 0.125, y), (x + w * 0.125, y + h)]),
            line(&[(x + w * 0.875, y), (x + w * 0.875, y + h)]),
        ],
        // Internal storage is a box with the two bars that mark its corner.
        "flowChartInternalStorage" => vec![
            polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)]),
            line(&[(x + w * 0.125, y), (x + w * 0.125, y + h)]),
            line(&[(x, y + h * 0.125), (x + w, y + h * 0.125)]),
        ],
        "roundRect" | "flowChartAlternateProcess" => vec![rounded(
            frame,
            adjust.get(&["adj"], 16667.0 / 100000.0) * w.min(h),
        )],
        // A terminator is a pill: a rounded box with the corners all the way in.
        "flowChartTerminator" => vec![rounded(frame, w.min(h) / 2.0)],
        "ellipse" | "flowChartConnector" => vec![ellipse(frame)],
        "triangle" => vec![polygon(&[(x + w / 2.0, y), (x + w, y + h), (x, y + h)])],
        "rtTriangle" => vec![polygon(&[(x, y), (x + w, y + h), (x, y + h)])],
        "diamond" | "flowChartDecision" => vec![polygon(&[
            (x + w / 2.0, y),
            (x + w, y + h / 2.0),
            (x + w / 2.0, y + h),
            (x, y + h / 2.0),
        ])],
        // A sort is a decision with a bar across its middle. The preset states
        // that bar as a path of its own with no fill, so it is a line of the
        // drawing: stroked in the shape's line colour and never painted in its
        // fill, which would be a bar of the shape's own colour and so no bar at
        // all — and would leave a sort looking exactly like a decision.
        "flowChartSort" => vec![
            polygon(&[
                (x + w / 2.0, y),
                (x + w, y + h / 2.0),
                (x + w / 2.0, y + h),
                (x, y + h / 2.0),
            ]),
            line(&[(x, y + h / 2.0), (x + w, y + h / 2.0)]),
        ],
        "parallelogram" | "flowChartInputOutput" => {
            let slant = adjust.get(&["adj"], 0.25) * w.min(h);
            vec![polygon(&[
                (x + slant, y),
                (x + w, y),
                (x + w - slant, y + h),
                (x, y + h),
            ])]
        }
        "trapezoid" => {
            let inset = adjust.get(&["adj"], 0.25) * w.min(h);
            vec![polygon(&[
                (x + inset, y),
                (x + w - inset, y),
                (x + w, y + h),
                (x, y + h),
            ])]
        }
        // The regular polygons and stars, a point at the top so a star reads
        // the way Word draws it.
        "pentagon" => vec![regular(frame, 5, -90.0)],
        "hexagon" | "flowChartPreparation" => vec![polygon(&[
            (x, y + h / 2.0),
            (x + w / 4.0, y),
            (x + 3.0 * w / 4.0, y),
            (x + w, y + h / 2.0),
            (x + 3.0 * w / 4.0, y + h),
            (x + w / 4.0, y + h),
        ])],
        "heptagon" => vec![regular(frame, 7, -90.0)],
        "octagon" => {
            let cut = w.min(h) / 3.0;
            vec![polygon(&[
                (x + cut, y),
                (x + w - cut, y),
                (x + w, y + cut),
                (x + w, y + h - cut),
                (x + w - cut, y + h),
                (x + cut, y + h),
                (x, y + h - cut),
                (x, y + cut),
            ])]
        }
        "decagon" => vec![regular(frame, 10, 0.0)],
        "dodecagon" => vec![regular(frame, 12, 0.0)],
        "star4" => vec![star(frame, 4, adjust.get(&["adj"], 0.125))],
        "star5" => vec![star(frame, 5, adjust.get(&["adj"], 0.19098))],
        "star6" => vec![star(frame, 6, adjust.get(&["adj"], 0.28867))],
        "star7" => vec![star(frame, 7, adjust.get(&["adj"], 0.34601))],
        "star8" => vec![star(frame, 8, adjust.get(&["adj"], 0.32524))],
        "star10" => vec![star(frame, 10, adjust.get(&["adj"], 0.42533))],
        "plus" | "mathPlus" => {
            let t = adjust.get(&["adj"], 0.25) * w.min(h);
            vec![polygon(&[
                (x + t, y),
                (x + w - t, y),
                (x + w - t, y + (h - t) / 2.0),
                (x + w, y + (h - t) / 2.0),
                (x + w, y + (h + t) / 2.0),
                (x + w - t, y + (h + t) / 2.0),
                (x + w - t, y + h),
                (x + t, y + h),
                (x + t, y + (h + t) / 2.0),
                (x, y + (h + t) / 2.0),
                (x, y + (h - t) / 2.0),
                (x + t, y + (h - t) / 2.0),
            ])]
        }
        "rightArrow" => vec![arrow(frame, Direction::Right, adjust)],
        "leftArrow" => vec![arrow(frame, Direction::Left, adjust)],
        "upArrow" => vec![arrow(frame, Direction::Up, adjust)],
        "downArrow" => vec![arrow(frame, Direction::Down, adjust)],
        "leftRightArrow" => vec![double_arrow(frame, adjust, false)],
        "upDownArrow" => vec![double_arrow(frame, adjust, true)],
        "chevron" => {
            let depth = adjust.get(&["adj"], 0.5) * w.min(h);
            vec![polygon(&[
                (x, y),
                (x + w - depth, y),
                (x + w, y + h / 2.0),
                (x + w - depth, y + h),
                (x, y + h),
                (x + depth, y + h / 2.0),
            ])]
        }
        "homePlate" => {
            let depth = adjust.get(&["adj"], 0.5) * w.min(h);
            vec![polygon(&[
                (x, y),
                (x + w - depth, y),
                (x + w, y + h / 2.0),
                (x + w - depth, y + h),
                (x, y + h),
            ])]
        }
        // A callout is its bubble; the tail that points at the words is not
        // drawn, because the words are the document's own text.
        "wedgeRectCallout"
        | "borderCallout1"
        | "borderCallout2"
        | "borderCallout3"
        | "accentBorderCallout1"
        | "accentBorderCallout2"
        | "accentBorderCallout3"
        | "callout1"
        | "callout2"
        | "callout3"
        | "accentCallout1"
        | "accentCallout2"
        | "accentCallout3" => rect(),
        "wedgeRoundRectCallout" => vec![rounded(
            frame,
            adjust.get(&["adj1", "adj"], 16667.0 / 100000.0) * w.min(h),
        )],
        "wedgeEllipseCallout" => vec![ellipse(frame)],
        "line" | "straightConnector1" => vec![Contour {
            points: vec![metafile::pt(x, y), metafile::pt(x + w, y + h)],
            closed: false,
            filled: true,
        }],
        // The connectors: a straight line, an elbow with one bend and a Z with
        // two. They carry no region of their own, only the rule between the
        // shapes they join.
        "straightConnector2" | "straightConnector3" => vec![line(&[(x, y), (x + w, y + h)])],
        "bentConnector2" => vec![line(&[(x, y), (x + w, y), (x + w, y + h)])],
        "bentConnector3" => {
            let bend = adjust.get(&["adj1"], 0.5).clamp(0.0, 1.0);
            vec![line(&[
                (x, y),
                (x + bend * w, y),
                (x + bend * w, y + h),
                (x + w, y + h),
            ])]
        }

        // More stars: Office names them by their point count, and they all
        // declare the same inner radius.
        "star12" => vec![star(frame, 12, adjust.get(&["adj"], 0.375))],
        "star16" => vec![star(frame, 16, adjust.get(&["adj"], 0.375))],
        "star24" => vec![star(frame, 24, adjust.get(&["adj"], 0.375))],
        "star32" => vec![star(frame, 32, adjust.get(&["adj"], 0.375))],

        // The rounded and snipped rectangles: Office states which corners are
        // rounded, or cut straight across, rather than one radius for all four.
        "round1Rect" => {
            let r = radius(frame, adjust, &["adj"], 0.16667);
            vec![corner_shape(
                frame,
                [
                    Corner::Square,
                    Corner::Round(r),
                    Corner::Square,
                    Corner::Square,
                ],
            )]
        }
        "round2SameRect" => {
            let top = radius(frame, adjust, &["adj1"], 0.16667);
            let bottom = radius(frame, adjust, &["adj2"], 0.0);
            vec![corner_shape(
                frame,
                [
                    Corner::Round(top),
                    Corner::Round(top),
                    Corner::Round(bottom),
                    Corner::Round(bottom),
                ],
            )]
        }
        "round2DiagRect" => {
            let first = radius(frame, adjust, &["adj1"], 0.16667);
            let second = radius(frame, adjust, &["adj2"], 0.0);
            vec![corner_shape(
                frame,
                [
                    Corner::Round(first),
                    Corner::Round(second),
                    Corner::Round(first),
                    Corner::Round(second),
                ],
            )]
        }
        "snip1Rect" => {
            let r = radius(frame, adjust, &["adj"], 0.16667);
            vec![corner_shape(
                frame,
                [
                    Corner::Square,
                    Corner::Snip(r),
                    Corner::Square,
                    Corner::Square,
                ],
            )]
        }
        "snip2SameRect" => {
            let top = radius(frame, adjust, &["adj1"], 0.16667);
            let bottom = radius(frame, adjust, &["adj2"], 0.0);
            vec![corner_shape(
                frame,
                [
                    Corner::Snip(top),
                    Corner::Snip(top),
                    Corner::Snip(bottom),
                    Corner::Snip(bottom),
                ],
            )]
        }
        "snip2DiagRect" => {
            let first = radius(frame, adjust, &["adj1"], 0.16667);
            let second = radius(frame, adjust, &["adj2"], 0.0);
            vec![corner_shape(
                frame,
                [
                    Corner::Snip(first),
                    Corner::Snip(second),
                    Corner::Snip(first),
                    Corner::Snip(second),
                ],
            )]
        }
        "snipRoundRect" => {
            let rounded = radius(frame, adjust, &["adj1"], 0.16667);
            let snipped = radius(frame, adjust, &["adj2"], 0.16667);
            vec![corner_shape(
                frame,
                [
                    Corner::Round(rounded),
                    Corner::Snip(snipped),
                    Corner::Snip(snipped),
                    Corner::Snip(snipped),
                ],
            )]
        }
        // A plaque is a rounded box whose corners are cut with an arc too, and
        // is close enough to the plain round rectangle.
        "plaque" => vec![rounded(frame, radius(frame, adjust, &["adj"], 0.16667))],

        // Rings: the alternate fill rule turns a contour inside another into a
        // hole, which is what a donut, a frame and a no-smoking sign are.
        "donut" => {
            let inset = adjust.get(&["adj"], 0.25).clamp(0.0, 0.49);
            vec![
                ellipse(frame),
                ellipse(Frame {
                    left: x + inset * w,
                    top: y + inset * h,
                    width: w * (1.0 - 2.0 * inset),
                    height: h * (1.0 - 2.0 * inset),
                }),
            ]
        }
        "noSmoking" => {
            let inset = adjust.get(&["adj"], 0.25).clamp(0.0, 0.49);
            vec![
                ellipse(frame),
                ellipse(Frame {
                    left: x + inset * w,
                    top: y + inset * h,
                    width: w * (1.0 - 2.0 * inset),
                    height: h * (1.0 - 2.0 * inset),
                }),
                line(&[(x + w * 0.85, y + h * 0.85), (x + w * 0.15, y + h * 0.15)]),
            ]
        }
        "frame" => {
            let inset_x = adjust.get(&["adj1"], 0.125).clamp(0.0, 0.45) * w;
            let inset_y = adjust.get(&["adj2"], 0.125).clamp(0.0, 0.45) * h;
            vec![
                polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)]),
                polygon(&[
                    (x + inset_x, y + inset_y),
                    (x + w - inset_x, y + inset_y),
                    (x + w - inset_x, y + h - inset_y),
                    (x + inset_x, y + h - inset_y),
                ]),
            ]
        }
        // A half frame is a frame whose right side is open.
        "halfFrame" => {
            let tx = (adjust.get(&["adj1"], 0.33333) * w).clamp(0.0, w / 2.0);
            let ty = (adjust.get(&["adj2"], 0.33333) * h).clamp(0.0, h / 2.0);
            vec![polygon(&[
                (x, y),
                (x + w, y),
                (x + w, y + ty),
                (x + tx, y + ty),
                (x + tx, y + h - ty),
                (x + w, y + h - ty),
                (x + w, y + h),
                (x, y + h),
            ])]
        }
        "corner" => {
            let tx = (adjust.get(&["adj1"], 0.5) * w).clamp(0.0, w);
            let ty = (adjust.get(&["adj2"], 0.5) * h).clamp(0.0, h);
            vec![polygon(&[
                (x, y),
                (x + w, y),
                (x + w, y + ty),
                (x + tx, y + ty),
                (x + tx, y + h),
                (x, y + h),
            ])]
        }
        "diagStripe" => {
            let band = (adjust.get(&["adj"], 0.5) * w).clamp(0.0, w);
            vec![polygon(&[
                (x, y + h),
                (x + w - band, y),
                (x + w, y),
                (x + band, y + h),
            ])]
        }

        // The bars, the cross and the division sign of a mathematical text.
        "mathMinus" => {
            let half = (adjust.get(&["adj"], 0.2352) / 2.0).clamp(0.0, 0.4);
            vec![unit_polygon(
                frame,
                &[
                    (0.0, 0.5 - half),
                    (1.0, 0.5 - half),
                    (1.0, 0.5 + half),
                    (0.0, 0.5 + half),
                ],
            )]
        }
        "mathEqual" | "mathNotEqual" => {
            let thickness = adjust.get(&["adj1"], 0.08).clamp(0.0, 0.2);
            let gap = adjust.get(&["adj2", "adj"], 0.12).clamp(0.0, 0.3);
            let (upper, lower) = (0.5 - gap / 2.0, 0.5 + gap / 2.0);
            let mut contours = vec![
                unit_polygon(
                    frame,
                    &[
                        (0.05, upper - thickness),
                        (0.95, upper - thickness),
                        (0.95, upper),
                        (0.05, upper),
                    ],
                ),
                unit_polygon(
                    frame,
                    &[
                        (0.05, lower),
                        (0.95, lower),
                        (0.95, lower + thickness),
                        (0.05, lower + thickness),
                    ],
                ),
            ];
            // The two bars of "not equal" are crossed by a rule.
            if name == "mathNotEqual" {
                contours.push(line(&[
                    (x + w * 0.68, y + h * 0.92),
                    (x + w * 0.32, y + h * 0.08),
                ]));
            }
            contours
        }
        "mathDivide" => {
            let thickness = adjust.get(&["adj1"], 0.08).clamp(0.0, 0.2);
            let dot = adjust.get(&["adj2", "adj"], 0.07).clamp(0.0, 0.2) * h.min(w);
            let (cx, cy) = frame.center();
            vec![
                unit_polygon(
                    frame,
                    &[
                        (0.05, 0.5 - thickness / 2.0),
                        (0.95, 0.5 - thickness / 2.0),
                        (0.95, 0.5 + thickness / 2.0),
                        (0.05, 0.5 + thickness / 2.0),
                    ],
                ),
                ellipse(Frame {
                    left: cx - dot,
                    top: cy - h * 0.28 - dot,
                    width: dot * 2.0,
                    height: dot * 2.0,
                }),
                ellipse(Frame {
                    left: cx - dot,
                    top: cy + h * 0.28 - dot,
                    width: dot * 2.0,
                    height: dot * 2.0,
                }),
            ]
        }
        // A multiplication sign is the plus turned on its diagonal.
        "mathMultiply" => {
            let t = (adjust.get(&["adj"], 0.2) * w.min(h)).clamp(0.0, w.min(h) / 3.0);
            let (cx, cy) = frame.center();
            let mut contour = polygon(&[
                (cx - t / 2.0, y),
                (cx + t / 2.0, y),
                (cx + t / 2.0, cy - t / 2.0),
                (x + w, cy - t / 2.0),
                (x + w, cy + t / 2.0),
                (cx + t / 2.0, cy + t / 2.0),
                (cx + t / 2.0, y + h),
                (cx - t / 2.0, y + h),
                (cx - t / 2.0, cy + t / 2.0),
                (x, cy + t / 2.0),
                (x, cy - t / 2.0),
                (cx - t / 2.0, cy - t / 2.0),
            ]);
            transform(
                std::slice::from_mut(&mut contour),
                frame,
                &Xfrm {
                    rotation: 45.0,
                    ..Xfrm::default()
                },
            );
            vec![contour]
        }

        // A bracket is a spine with an arm top and bottom; a brace is a line
        // rather than a region, so it is stroked and never filled.
        "leftBracket" | "rightBracket" => {
            let thick = (adjust.get(&["adj1"], 0.08333) * w).clamp(0.0, w / 2.0);
            let reach = (adjust.get(&["adj2"], 0.5) * w).clamp(thick, w);
            let step = if name == "leftBracket" { 1.0 } else { -1.0 };
            let spine = if name == "leftBracket" { x } else { x + w };
            let arm = spine + step * reach;
            let inner = spine + step * thick;
            vec![polygon(&[
                (spine, y),
                (arm, y),
                (arm, y + thick),
                (inner, y + thick),
                (inner, y + h - thick),
                (arm, y + h - thick),
                (arm, y + h),
                (spine, y + h),
            ])]
        }
        "leftBrace" | "rightBrace" => {
            let t = (adjust.get(&["adj1"], 0.08333) * w.min(h)).clamp(0.0, w / 3.0);
            let left = name == "leftBrace";
            let spine = if left { x + t } else { x + w - t };
            let tip = if left { x } else { x + w };
            let arm = if left { x + w } else { x };
            vec![line(&[
                (arm, y),
                (spine, y),
                (spine, y + h / 2.0 - t),
                (tip, y + h / 2.0),
                (spine, y + h / 2.0 + t),
                (spine, y + h),
                (arm, y + h),
            ])]
        }

        // Flowchart symbols that are a polygon with a side slanted or a corner
        // cut off, or that come to a point.
        "flowChartManualInput" => vec![unit_polygon(
            frame,
            &[(0.0, 0.18), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
        )],
        "flowChartManualOperation" => vec![unit_polygon(
            frame,
            &[(0.0, 0.0), (1.0, 0.0), (0.82, 1.0), (0.18, 1.0)],
        )],
        "flowChartMerge" => vec![unit_polygon(frame, &[(0.0, 0.0), (1.0, 0.0), (0.5, 1.0)])],
        "flowChartExtract" => vec![unit_polygon(frame, &[(0.5, 0.0), (1.0, 1.0), (0.0, 1.0)])],
        "flowChartOffpageConnector" => vec![unit_polygon(
            frame,
            &[(0.0, 0.0), (1.0, 0.0), (1.0, 0.75), (0.5, 1.0), (0.0, 0.75)],
        )],
        "flowChartPunchedCard" => vec![unit_polygon(
            frame,
            &[(0.0, 0.18), (0.18, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
        )],
        "flowChartMagneticTape" => vec![rounded(frame, w.min(h) / 2.0)],

        // Arrows with more than one head, a bend, or a notched tail.
        "quadArrow" => {
            let body = (adjust.get(&["adj1"], 0.225) / 2.0).clamp(0.0, 0.4);
            let head = (adjust.get(&["adj2"], 0.33333) / 2.0).clamp(0.0, 0.4);
            let (c, b, hd) = (0.5, body, head);
            vec![unit_polygon(
                frame,
                &[
                    (c + b, c - b),
                    (1.0 - hd, c - b),
                    (1.0 - hd, c - hd),
                    (1.0, c),
                    (1.0 - hd, c + hd),
                    (1.0 - hd, c + b),
                    (c + b, c + b),
                    (c + b, 1.0 - hd),
                    (c + hd, 1.0 - hd),
                    (c, 1.0),
                    (c - hd, 1.0 - hd),
                    (c - b, 1.0 - hd),
                    (c - b, c + b),
                    (hd, c + b),
                    (hd, c + hd),
                    (0.0, c),
                    (hd, c - hd),
                    (hd, c - b),
                    (c - b, c - b),
                    (c - b, hd),
                    (c - hd, hd),
                    (c, 0.0),
                    (c + hd, hd),
                    (c + b, hd),
                ],
            )]
        }
        "bentArrow" => {
            let b = (adjust.get(&["adj1"], 0.25) / 2.0).clamp(0.02, 0.4);
            let head = adjust.get(&["adj2"], 0.2).clamp(0.02, 0.4);
            let arm = 0.4;
            vec![unit_polygon(
                frame,
                &[
                    (0.0, 1.0),
                    (2.0 * b, 1.0),
                    (2.0 * b, arm + b),
                    (1.0 - head, arm + b),
                    (1.0 - head, arm + head),
                    (1.0, arm),
                    (1.0 - head, arm - head),
                    (1.0 - head, arm - b),
                    (0.0, arm - b),
                ],
            )]
        }
        "bentUpArrow" => {
            let b = (adjust.get(&["adj1"], 0.25) / 2.0).clamp(0.02, 0.4);
            let head = adjust.get(&["adj2"], 0.2).clamp(0.02, 0.4);
            let arm = 0.6;
            vec![unit_polygon(
                frame,
                &[
                    (0.0, 1.0),
                    (arm + b, 1.0),
                    (arm + b, head),
                    (arm + head, head),
                    (arm, 0.0),
                    (arm - head, head),
                    (arm - b, head),
                    (arm - b, 1.0 - 2.0 * b),
                    (0.0, 1.0 - 2.0 * b),
                ],
            )]
        }
        "notchedRightArrow" => {
            let body = adjust.get(&["adj1"], 0.5).clamp(0.2, 0.9);
            let head = adjust.get(&["adj2"], 0.5).clamp(0.1, 0.9);
            let shoulder = (body / 2.0 + 0.1).min(0.95);
            vec![unit_polygon(
                frame,
                &[
                    (0.0, 0.5 - body / 2.0),
                    (1.0 - head, 0.5 - body / 2.0),
                    (1.0 - head, 0.5 - shoulder),
                    (1.0, 0.5),
                    (1.0 - head, 0.5 + shoulder),
                    (1.0 - head, 0.5 + body / 2.0),
                    (0.0, 0.5 + body / 2.0),
                    (0.1, 0.5),
                ],
            )]
        }

        // A cylinder and a cube are drawn with their rim and their front edges:
        // the lines that make them read as solid rather than as a silhouette.
        "cube" => {
            let depth = radius(frame, adjust, &["adj"], 0.25);
            vec![
                polygon(&[
                    (x, y + depth),
                    (x + depth, y),
                    (x + w, y),
                    (x + w, y + h - depth),
                    (x + w - depth, y + h),
                    (x, y + h),
                ]),
                line(&[(x, y + depth), (x + w - depth, y + depth)]),
                line(&[(x + w - depth, y + depth), (x + w - depth, y + h)]),
            ]
        }
        "bevel" => {
            let depth = radius(frame, adjust, &["adj"], 0.25);
            vec![
                polygon(&[
                    (x + depth, y),
                    (x + w - depth, y),
                    (x + w, y + depth),
                    (x + w, y + h - depth),
                    (x + w - depth, y + h),
                    (x + depth, y + h),
                    (x, y + h - depth),
                    (x, y + depth),
                ]),
                closed_line(&[
                    (x + depth, y + depth),
                    (x + w - depth, y + depth),
                    (x + w - depth, y + h - depth),
                    (x + depth, y + h - depth),
                ]),
            ]
        }
        "foldedCorner" => {
            let fold = radius(frame, adjust, &["adj"], 0.16667);
            vec![
                polygon(&[
                    (x, y),
                    (x + w, y),
                    (x + w, y + h - fold),
                    (x + w - fold, y + h),
                    (x, y + h),
                ]),
                line(&[
                    (x + w, y + h - fold),
                    (x + w - fold, y + h - fold),
                    (x + w - fold, y + h),
                ]),
            ]
        }
        _ => return None,
    };
    Some(contours)
}

/// The presets whose outline needs a curve: the cylinders, the flowchart's
/// document and punched tape, the arcs, and the connectors that bend round a
/// corner.
fn curved_outline(name: &str, frame: Frame, adjust: &Adjustments) -> Option<Vec<Contour>> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let contours = match name {
        "can" | "flowChartDatabase" => cylinder(frame, adjust, false),
        "flowChartMagneticDisk" | "flowChartMagneticDrum" => cylinder(frame, adjust, true),
        "flowChartDocument" => vec![polygon(&document_outline(frame))],
        "flowChartPunchedTape" => vec![polygon(&tape_outline(frame))],
        "wave" => vec![polygon(&wave_outline(frame))],
        "cloud" | "cloudCallout" => vec![polygon(&cloud_outline(frame))],
        "heart" => vec![polygon(&heart_outline(frame))],
        // The moon is a disc with a bite taken out of its right side. The
        // crescent is one contour rather than a disc with a hole: its outer
        // edge is the disc's left half, and its inner edge is where the bite
        // ends along each ray from the middle, so the horns taper.
        "moon" => {
            let shift = adjust.get(&["adj"], 0.5).clamp(0.0, 1.5);
            let (cx, cy) = frame.center();
            let (rx, ry) = (w / 2.0, h / 2.0);
            let (bite_x, bite_rx, bite_ry) = (shift * rx * 0.55, rx * 0.9, ry);
            let mut points = arc_points(cx, cy, rx, ry, 270.0, 90.0, 48);
            for step in 0..=48 {
                let angle = (90.0 + 180.0 * step as f64 / 48.0).to_radians();
                let (dx, dy) = (angle.cos(), angle.sin());
                // How far the ray from the middle runs before it leaves the
                // bite: the ray's second crossing of that ellipse.
                let a = (dx / bite_rx).powi(2) + (dy / bite_ry).powi(2);
                let b = -2.0 * (bite_x * dx / bite_rx.powi(2));
                let c = (bite_x / bite_rx).powi(2) - 1.0;
                let reach = (-b + (b * b - 4.0 * a * c).max(0.0).sqrt()) / (2.0 * a);
                points.push((cx + reach * dx, cy + reach * dy));
            }
            vec![polygon(&points)]
        }
        "sun" => {
            let radius = (adjust.get(&["adj"], 0.25).clamp(0.05, 0.5) * w.min(h)).max(0.5);
            let (cx, cy) = frame.center();
            let mut contours = vec![ellipse(Frame {
                left: cx - radius,
                top: cy - radius,
                width: radius * 2.0,
                height: radius * 2.0,
            })];
            // Eight rays, each a narrow wedge from the disc to the frame.
            let point = |angle: f64, rx: f64, ry: f64| {
                let angle = angle.to_radians();
                (cx + rx * angle.cos(), cy + ry * angle.sin())
            };
            for step in 0..8 {
                let middle = step as f64 * 45.0;
                let (base, tip) = (0.16, 0.05);
                contours.push(polygon(&[
                    point(middle - base, radius, radius),
                    point(middle - tip, w / 2.0, h / 2.0),
                    point(middle + tip, w / 2.0, h / 2.0),
                    point(middle + base, radius, radius),
                ]));
            }
            contours
        }
        "smileyFace" => {
            let eye = (w.min(h) * 0.08).max(0.5);
            let (cx, cy) = frame.center();
            vec![
                ellipse(frame),
                ellipse(Frame {
                    left: x + w * 0.3 - eye,
                    top: y + h * 0.34 - eye,
                    width: eye * 2.0,
                    height: eye * 2.0,
                }),
                ellipse(Frame {
                    left: x + w * 0.7 - eye,
                    top: y + h * 0.34 - eye,
                    width: eye * 2.0,
                    height: eye * 2.0,
                }),
                line(&arc_points(cx, cy, w * 0.3, h * 0.3, 20.0, 160.0, 16)),
            ]
        }
        // A drop is a disc whose bottom-left quarter is a point.
        "teardrop" => {
            let radius = w.min(h) / 2.0;
            let (cx, cy) = (x + w - radius, y + radius);
            let mut points = arc_points(cx, cy, radius, radius, 180.0, 450.0, 32);
            points.push((x, y + h));
            vec![polygon(&points)]
        }
        // The arcs: `adj1` and `adj2` are angles in 1/60000 of a degree, which
        // the adjustment table has divided by 100000.
        "arc" | "pie" | "chord" => {
            let start = angle(adjust, "adj1", 162.0);
            let end = angle(adjust, "adj2", 0.0);
            let sweep = if end <= start {
                end + 360.0 - start
            } else {
                end - start
            };
            let (cx, cy) = frame.center();
            let mut points = arc_points(cx, cy, w / 2.0, h / 2.0, start, start + sweep, 24);
            match name {
                "pie" => {
                    points.push((cx, cy));
                    vec![polygon(&points)]
                }
                "chord" => vec![polygon(&points)],
                _ => vec![open_outline(points)],
            }
        }
        "blockArc" => {
            let start = angle(adjust, "adj1", 108.0);
            let end = angle(adjust, "adj2", 0.0);
            let thickness = adjust.get(&["adj3"], 0.25).clamp(0.0, 0.9);
            let sweep = if end <= start {
                end + 360.0 - start
            } else {
                end - start
            };
            let (cx, cy) = frame.center();
            let (rx, ry) = (w / 2.0, h / 2.0);
            let mut points = arc_points(cx, cy, rx, ry, start, start + sweep, 24);
            points.extend(arc_points(
                cx,
                cy,
                rx * (1.0 - thickness),
                ry * (1.0 - thickness),
                start + sweep,
                start,
                24,
            ));
            vec![polygon(&points)]
        }
        // A delay is a box whose right end is a half circle.
        "flowChartDelay" => {
            let radius = (h / 2.0).min(w);
            let mut points = vec![(x, y), (x + w - radius, y)];
            points.extend(arc_points(
                x + w - radius,
                y + h / 2.0,
                radius,
                h / 2.0,
                -90.0,
                90.0,
                24,
            ));
            points.push((x + w - radius, y + h));
            points.push((x, y + h));
            vec![polygon(&points)]
        }
        // The arrows that bend rather than step: a band that leaves one edge
        // of the frame, curves round and ends in a head on another, a u-turn
        // that doubles back, a ring with a head on its end, and the swoosh.
        "curvedRightArrow" => curved_right_arrow(frame, adjust),
        // The left arrow is that band mirrored, and the up and down arrows are
        // the same ribbon drawn as a bowl, one turned over.
        "curvedLeftArrow" => curved_left_arrow(frame, adjust),
        "curvedUpArrow" => curved_up_arrow(frame, adjust),
        "curvedDownArrow" => curved_down_arrow(frame, adjust),
        "uturnArrow" => uturn_arrow(frame, adjust),
        "circularArrow" => circular_arrow(frame, adjust),
        "swooshArrow" => vec![swoosh_arrow(frame, adjust)],
        "curvedConnector2" => {
            let mut points = vec![(x, y)];
            points.extend(curve_points(
                (x, y),
                (x + w * 0.66, y),
                (x + w, y + h * 0.34),
                (x + w, y + h),
            ));
            vec![line(&points)]
        }
        "curvedConnector3" => {
            let mut points = vec![(x, y)];
            points.extend(curve_points(
                (x, y),
                (x + w * 0.33, y),
                (x + w * 0.5, y + h * 0.25),
                (x + w * 0.5, y + h * 0.5),
            ));
            points.extend(curve_points(
                (x + w * 0.5, y + h * 0.5),
                (x + w * 0.5, y + h * 0.75),
                (x + w * 0.67, y + h),
                (x + w, y + h),
            ));
            vec![line(&points)]
        }
        _ => return None,
    };
    Some(contours)
}

/// A line of the drawing rather than a region of it: a cube's back edge and a
/// cylinder's rim are part of the picture and of its stroke, but filling them
/// would paint a band across the shape.
fn line(points: &[(f64, f64)]) -> Contour {
    Contour {
        points: points.iter().map(|&(x, y)| metafile::pt(x, y)).collect(),
        closed: false,
        filled: false,
    }
}

/// The same, for a line that comes back to its start.
fn closed_line(points: &[(f64, f64)]) -> Contour {
    Contour {
        points: points.iter().map(|&(x, y)| metafile::pt(x, y)).collect(),
        closed: true,
        filled: false,
    }
}

/// A path being walked the way `a:pathLst` states one: a current point, lines
/// to more points, and arcs whose ellipse is worked out from where the arc
/// starts.
struct Path {
    points: Vec<(f64, f64)>,
}

impl Path {
    fn at(point: (f64, f64)) -> Path {
        Path {
            points: vec![point],
        }
    }

    fn then_line(mut self, point: (f64, f64)) -> Path {
        self.points.push(point);
        self
    }

    /// `a:arcTo`: an arc on the ellipse of radii `rx` × `ry`, starting at the
    /// current point and sweeping `sweep` degrees clockwise — the direction the
    /// drawing's y-down angles run.
    ///
    /// The angles are directions of a ray from the ellipse's middle, not the
    /// ellipse's own parameter: the arc meets the ray where it crosses the
    /// curve, which on a wide ellipse is further round than the parameter says.
    /// The ellipse is placed so that it passes through the current point.
    fn then_arc(mut self, rx: f64, ry: f64, start: f64, sweep: f64) -> Path {
        let Some(&from) = self.points.last() else {
            return self;
        };
        let on = |angle: f64| {
            let (sin, cos) = angle.to_radians().sin_cos();
            let turn = (rx * sin).atan2(ry * cos);
            (rx * turn.cos(), ry * turn.sin())
        };
        let (fx, fy) = on(start);
        let (cx, cy) = (from.0 - fx, from.1 - fy);
        let steps = ((sweep.abs() / 3.0).ceil() as usize).clamp(10, 160);
        for step in 1..=steps {
            let (u, v) = on(start + sweep * step as f64 / steps as f64);
            self.points.push((cx + u, cy + v));
        }
        self
    }

    /// `a:quadBezTo`: the quadratic the file states as a control point and an
    /// end point.
    fn then_quad(mut self, control: (f64, f64), end: (f64, f64)) -> Path {
        let Some(&from) = self.points.last() else {
            return self;
        };
        for step in 1..=12 {
            let t = step as f64 / 12.0;
            let u = 1.0 - t;
            self.points.push((
                u * u * from.0 + 2.0 * u * t * control.0 + t * t * end.0,
                u * u * from.1 + 2.0 * u * t * control.1 + t * t * end.1,
            ));
        }
        self
    }

    /// The region the path encloses.
    fn filled(self) -> Contour {
        polygon(&self.points)
    }
}

/// An adjustment as the guide formulas state it: 1/100000 of the shape's size.
fn percent(adjust: &Adjustments, name: &str, default: f64) -> f64 {
    adjust.get(&[name], default) * 100_000.0
}

/// DrawingML's `at2`: the direction of the vector `(x, y)`, as an angle in
/// degrees.
fn at2(x: f64, y: f64) -> f64 {
    y.atan2(x).to_degrees()
}

/// A square root that answers zero rather than a NaN when rounding has put a
/// formula's radicand below zero.
fn root(value: f64) -> f64 {
    value.max(0.0).sqrt()
}

/// A `curvedRightArrow`: a band that leaves the frame's left edge, sweeps up
/// over its top and comes down at its right, where a head points out of the
/// frame's right edge.
///
/// The guides are the preset's own: `adj1` is the band's thickness, `adj2` how
/// wide the head is and `adj3` how far it reaches. The shape states its outline
/// in two contours — the band, and the crook over its top where the ribbon turns
/// away from the edge — and the two together are the arrow.
fn curved_right_arrow(frame: Frame, adjust: &Adjustments) -> Vec<Contour> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let ss = w.min(h);
    let max_adj2 = 50_000.0 * h / ss;
    let a2 = percent(adjust, "adj2", 0.5).clamp(0.0, max_adj2);
    let a1 = percent(adjust, "adj1", 0.25).clamp(0.0, a2);
    let thickness = ss * a1 / 100_000.0;
    let aw = ss * a2 / 100_000.0;

    // The band is cut from an ellipse as wide as the frame and half as tall as
    // the band and the head leave room for.
    let hr = h / 2.0 - (thickness + aw) / 4.0;
    let q7 = hr * 2.0;
    let idx = root(q7 * q7 - thickness * thickness) * w / q7;
    let max_adj3 = 100_000.0 * idx / ss;
    let a3 = percent(adjust, "adj3", 0.25).clamp(0.0, max_adj3);
    let reach = ss * a3 / 100_000.0;

    // Where the band's edges leave the ellipse, and how far apart the head's
    // two barbs stand.
    let y3 = hr + thickness;
    let dy = root(w * w - reach * reach) * hr / w;
    let y5 = hr + dy;
    let y7 = y3 + dy;
    let half = (aw - thickness) / 2.0;
    let y4 = y5 - half;
    let y8 = y7 + half;
    let y6 = h - aw / 2.0;
    let x1 = w - reach;
    let sw = at2(reach, dy);

    let at = |u: f64, v: f64| (x + u, y + v);
    // The band, and the crook over its top where the ribbon turns away from
    // the frame's edge: the two together are the arrow's outline.
    vec![
        Path::at(at(0.0, hr))
            .then_arc(w, hr, 180.0, -sw)
            .then_line(at(x1, y4))
            .then_line(at(w, y6))
            .then_line(at(x1, y8))
            .then_line(at(x1, y7))
            .then_arc(w, hr, 180.0 - sw, sw)
            .filled(),
        Path::at(at(w, thickness))
            .then_arc(w, hr, 270.0, at2(idx, thickness / 2.0) - 90.0)
            .then_arc(
                w,
                hr,
                180.0 - at2(idx, thickness / 2.0),
                90.0 - at2(idx, thickness / 2.0),
            )
            .filled(),
    ]
}

/// A `curvedLeftArrow`: the same band reaching the other way, with the head at
/// the frame's left edge.
fn curved_left_arrow(frame: Frame, adjust: &Adjustments) -> Vec<Contour> {
    let middle = frame.left + frame.width / 2.0;
    curved_right_arrow(frame, adjust)
        .into_iter()
        .map(|mut contour| {
            for point in &mut contour.points {
                point.x = 2.0 * middle - point.x;
            }
            contour
        })
        .collect()
}

/// A `curvedUpArrow` and a `curvedDownArrow`: a band that leaves one edge of
/// the frame, turns round the far side and comes back to the head, which points
/// out of the edge it left from.
///
/// `adj1` is the band's thickness, `adj2` the width of the head's base and
/// `adj3` how far the head reaches past the band. The two presets are the same
/// shape the other way up, so one outline serves both: it is drawn in a space
/// with the head at the top and turned over for the down arrow.
fn curved_bowl(frame: Frame, adjust: &Adjustments, upward: bool) -> Vec<Contour> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let ss = w.min(h);
    let a1 = percent(adjust, "adj1", 0.25).clamp(0.0, 100_000.0);
    let a2 = percent(adjust, "adj2", 0.5).clamp(0.0, 50_000.0 * w / ss);
    let a3 = percent(adjust, "adj3", 0.25).clamp(0.0, 50_000.0);
    let thickness = ss * a1 / 100_000.0;
    let aw = ss * a2 / 100_000.0;
    let ah = ss * a3 / 100_000.0;

    // The bowl the band is cut from: a circle whose middle sits above the left
    // arm, so the band runs down from the top edge, round the bottom and back
    // up into the head.
    let centre = (w / 2.0 - thickness * 0.28, thickness * 0.3);
    let outer = h * 0.934;
    let inner = (outer - thickness).max(0.5);
    // Where the band is cut at the frame's top edge, and where the head takes
    // over on the way back up.
    let mouth = 185.4;
    let end = 12.8;

    let turn = |v: f64| if upward { v } else { h - v };
    let at = |u: f64, v: f64| (x + u, y + turn(v));
    let ring = |radius: f64, angle: f64| {
        let (sin, cos) = angle.to_radians().sin_cos();
        at(centre.0 + radius * cos, centre.1 + radius * sin)
    };

    let steps = 60;
    let mut points = Vec::with_capacity(steps * 2 + 4);
    for step in 0..=steps {
        let angle = mouth + (end - mouth) * step as f64 / steps as f64;
        points.push(ring(outer, angle));
    }
    // The head: its base is wider than the band and its point reaches past the
    // frame's edge.
    let tip = w * 0.875;
    points.push(at(tip + aw / 2.0, ah));
    points.push(at(tip, 0.0));
    points.push(at(tip - aw / 2.0, ah));
    for step in 0..=steps {
        let angle = end + (mouth - end) * step as f64 / steps as f64;
        points.push(ring(inner, angle));
    }
    vec![polygon(&points)]
}

/// A `curvedUpArrow`: the bowl with its head at the frame's top edge.
fn curved_up_arrow(frame: Frame, adjust: &Adjustments) -> Vec<Contour> {
    curved_bowl(frame, adjust, true)
}

/// A `curvedDownArrow`: the bowl with its head at the frame's bottom edge.
fn curved_down_arrow(frame: Frame, adjust: &Adjustments) -> Vec<Contour> {
    curved_bowl(frame, adjust, false)
}

/// A `uturnArrow`: a band that leaves the frame's bottom edge on the left, runs
/// up and over and comes back down on the right, where a head points down.
///
/// `adj1` is the band's thickness, `adj2` half the head's width, `adj3` the
/// head's height, `adj4` the radius of the two corners it turns on and `adj5`
/// where down the right edge the head sits.
fn uturn_arrow(frame: Frame, adjust: &Adjustments) -> Vec<Contour> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let ss = w.min(h);
    let a2 = percent(adjust, "adj2", 0.25).clamp(0.0, 25_000.0);
    let a1 = percent(adjust, "adj1", 0.25).clamp(0.0, 2.0 * a2);
    let q2 = a1 * ss / h;
    let max_adj3 = (100_000.0 - q2) * h / ss;
    let a3 = percent(adjust, "adj3", 0.25).clamp(0.0, max_adj3);
    let min_adj5 = (a3 + a1) * ss / h;
    let a5 = percent(adjust, "adj5", 0.75).clamp(min_adj5, 100_000.0);

    let thickness = ss * a1 / 100_000.0;
    let arm = ss * a2 / 100_000.0;
    let head = ss * a3 / 100_000.0;
    let inset = arm - thickness / 2.0;

    let y5 = h * a5 / 100_000.0;
    let y4 = y5 - head;
    let x9 = w - inset;
    let max_adj4 = (x9 / 2.0).min(y4) * 100_000.0 / ss;
    let a4 = percent(adjust, "adj4", 0.4375).clamp(0.0, max_adj4);
    let corner = ss * a4 / 100_000.0;
    let inner = (corner - thickness).max(0.0);

    let x3 = thickness + inner;
    let x8 = w - arm;
    let x6 = x8 - arm;
    let x7 = x6 + inset;
    let x4 = x9 - corner;

    let at = |u: f64, v: f64| (x + u, y + v);
    vec![Path::at(at(0.0, h))
        .then_line(at(0.0, corner))
        .then_arc(corner, corner, 180.0, 90.0)
        .then_line(at(x4, 0.0))
        .then_arc(corner, corner, 270.0, 90.0)
        .then_line(at(x9, y4))
        .then_line(at(w, y4))
        .then_line(at(x8, y5))
        .then_line(at(x6, y4))
        .then_line(at(x7, y4))
        .then_line(at(x7, x3))
        .then_arc(inner, inner, 0.0, -90.0)
        .then_line(at(x3, thickness))
        .then_arc(inner, inner, 270.0, -90.0)
        .then_line(at(thickness, h))
        .filled()]
}

/// A `circularArrow`: a ring with a gap, and a head on the gap's end.
///
/// `adj1` is the ring's thickness, `adj2` the head's angle, `adj3` where the
/// ring ends and `adj4` where it starts — both in 1/60000ths of a degree — and
/// `adj5` how far the head's barbs stand past the ring on each side.
fn circular_arrow(frame: Frame, adjust: &Adjustments) -> Vec<Contour> {
    let (w, h) = (frame.width, frame.height);
    let ss = w.min(h);
    let (hc, vc) = frame.center();
    let a5 = percent(adjust, "adj5", 0.125).clamp(0.0, 25_000.0);
    let a1 = percent(adjust, "adj1", 0.125).clamp(0.0, 2.0 * a5);
    let thickness = ss * a1 / 100_000.0;
    let barb = ss * a5 / 100_000.0;

    // The ring's own radii: the frame's, less the room the head's barbs need.
    let rw1 = w / 2.0 + thickness / 2.0 - barb;
    let rh1 = h / 2.0 + thickness / 2.0 - barb;
    let rw2 = (rw1 - thickness).max(0.0);
    let rh2 = (rh1 - thickness).max(0.0);
    let start = percent(adjust, "adj4", 108.0) / 60_000.0;
    let end = percent(adjust, "adj3", 204.57681) / 60_000.0;
    let sweep = (end - start).rem_euclid(360.0);
    let head = percent(adjust, "adj2", 11.42319) / 60_000.0;

    // The band's middle line, where the head's point sits, and how far the
    // head's barbs stand past the ring on each side.
    let (rxm, rym) = ((rw1 + rw2) / 2.0, (rh1 + rh2) / 2.0);
    let flare = if rxm.max(rym) > 0.0 {
        (barb / rxm.max(rym)).to_degrees() * 0.45
    } else {
        0.0
    };

    // The point a ray at `angle` meets the ellipse of these radii at.
    let on = |rx: f64, ry: f64, angle: f64| {
        let (sin, cos) = angle.to_radians().sin_cos();
        let turn = (rx * sin).atan2(ry * cos);
        (hc + rx * turn.cos(), vc + ry * turn.sin())
    };
    let arc = |rx: f64, ry: f64, from: f64, to: f64| {
        let steps = (((to - from).abs() / 3.0).ceil() as usize).clamp(8, 160);
        (0..=steps)
            .map(|step| on(rx, ry, from + (to - from) * step as f64 / steps as f64))
            .collect::<Vec<_>>()
    };

    let mut points = arc(rw1, rh1, start, start + sweep);
    // The head: the ring's end runs out to the barbs on either side and back
    // to the point, which carries on the way the ring was travelling.
    points.push(on(rw1, rh1, end + flare));
    points.push(on(rxm, rym, end + head));
    points.push(on(rw2, rh2, end - flare));
    points.extend(arc(rw2, rh2, end - flare, start));
    vec![polygon(&points)]
}

/// A `swooshArrow`: a tail at the bottom left that sweeps up into a head at the
/// top right, drawn with the two quadratic curves the file states.
///
/// `adj1` is how deep the curve runs, `adj2` how far in from the right edge the
/// head's tail corners sit.
fn swoosh_arrow(frame: Frame, adjust: &Adjustments) -> Contour {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let ss = w.min(h);
    let a1 = percent(adjust, "adj1", 0.25).clamp(1.0, 75_000.0);
    let max_adj2 = 70_000.0 * w / ss;
    let a2 = percent(adjust, "adj2", 0.16667).clamp(0.0, max_adj2);
    let depth = h * a1 / 100_000.0;
    let inset = ss * a2 / 100_000.0;

    // Both tangents leave at the same shallow angle, which is what makes the
    // tail slope into the curve rather than meet it at a corner.
    let slope = (90.0f64 / 14.0).to_radians().tan();
    let eighth = ss / 8.0;
    let xb = w - inset;
    let yb = eighth;
    let dx0 = eighth * slope;
    let xc = xb - dx0;
    let xf = xb - depth * slope;
    let yf = yb + depth;
    let xe = xf - dx0;
    let ye = yf + eighth;
    let yd = ye / 2.0 + h / 20.0;

    let at = |u: f64, v: f64| (x + u, y + v);
    polygon(
        &Path::at(at(0.0, h))
            .then_quad(at(w / 6.0, 0.0), at(xb, yb))
            .then_line(at(xc, 0.0))
            .then_line(at(w, yd))
            .then_line(at(xe, ye))
            .then_line(at(xf, yf))
            .then_quad(at(w / 4.0, yf - h / 12.0), at(0.0, h))
            .points,
    )
}

/// An outline that is filled when its shape is, but is not a closed region:
/// the `arc` preset is the quarter circumference, not the pie it cuts off.
fn open_outline(points: Vec<(f64, f64)>) -> Contour {
    Contour {
        points: points
            .into_iter()
            .map(|(x, y)| metafile::pt(x, y))
            .collect(),
        closed: false,
        filled: true,
    }
}

/// A polygon given in unit coordinates of the frame.
fn unit_polygon(frame: Frame, points: &[(f64, f64)]) -> Contour {
    let mapped: Vec<(f64, f64)> = points
        .iter()
        .map(|&(u, v)| (frame.left + u * frame.width, frame.top + v * frame.height))
        .collect();
    polygon(&mapped)
}

/// A corner radius as a fraction of the frame's shorter side, in points.
fn radius(frame: Frame, adjust: &Adjustments, names: &[&str], default: f64) -> f64 {
    adjust.get(names, default).clamp(0.0, 0.5) * frame.width.min(frame.height)
}

/// An angle guide, in degrees clockwise from three o'clock. The file states it
/// in 1/60000 of a degree; the adjustment table has already divided it by
/// 100000.
fn angle(adjust: &Adjustments, name: &str, default: f64) -> f64 {
    adjust.get(&[name], default) * 5.0 / 3.0
}

/// What happens to one corner of a rounded, snipped or square rectangle.
#[derive(Clone, Copy)]
enum Corner {
    Square,
    Round(f64),
    Snip(f64),
}

/// A box whose four corners are treated one by one: rounded, cut straight
/// across, or square. The corners are given clockwise from the top left.
fn corner_shape(frame: Frame, corners: [Corner; 4]) -> Contour {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let limit = w.min(h) / 2.0;
    let size = |corner: Corner| match corner {
        Corner::Square => 0.0,
        Corner::Round(r) | Corner::Snip(r) => r.clamp(0.0, limit),
    };
    let [tl, tr, br, bl] = corners.map(size);
    let mut points = Vec::new();
    match corners[0] {
        Corner::Round(_) if tl > 0.0 => {
            points.extend(arc_points(x + tl, y + tl, tl, tl, 180.0, 270.0, 8))
        }
        Corner::Snip(_) if tl > 0.0 => {
            points.push((x, y + tl));
            points.push((x + tl, y));
        }
        _ => points.push((x, y)),
    }
    match corners[1] {
        Corner::Round(_) if tr > 0.0 => {
            points.extend(arc_points(x + w - tr, y + tr, tr, tr, 270.0, 360.0, 8))
        }
        Corner::Snip(_) if tr > 0.0 => {
            points.push((x + w - tr, y));
            points.push((x + w, y + tr));
        }
        _ => points.push((x + w, y)),
    }
    match corners[2] {
        Corner::Round(_) if br > 0.0 => {
            points.extend(arc_points(x + w - br, y + h - br, br, br, 0.0, 90.0, 8))
        }
        Corner::Snip(_) if br > 0.0 => {
            points.push((x + w, y + h - br));
            points.push((x + w - br, y + h));
        }
        _ => points.push((x + w, y + h)),
    }
    match corners[3] {
        Corner::Round(_) if bl > 0.0 => {
            points.extend(arc_points(x + bl, y + h - bl, bl, bl, 90.0, 180.0, 8))
        }
        Corner::Snip(_) if bl > 0.0 => {
            points.push((x + bl, y + h));
            points.push((x, y + h - bl));
        }
        _ => points.push((x, y + h)),
    }
    polygon(&points)
}

/// Points along an ellipse's arc, `from` and `to` in degrees clockwise from
/// three o'clock (y grows downward, so 90 is the bottom).
fn arc_points(
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
    from: f64,
    to: f64,
    steps: usize,
) -> Vec<(f64, f64)> {
    (0..=steps)
        .map(|step| {
            let angle = (from + (to - from) * step as f64 / steps as f64).to_radians();
            (cx + rx * angle.cos(), cy + ry * angle.sin())
        })
        .collect()
}

/// The points of a cubic curve, its start excluded and its end included.
fn curve_points(
    from: (f64, f64),
    control1: (f64, f64),
    control2: (f64, f64),
    to: (f64, f64),
) -> Vec<(f64, f64)> {
    (1..=8)
        .map(|step| cubic(from, control1, control2, to, step as f64 / 8.0))
        .collect()
}

/// A cylinder: its caps' arcs, and the rim where the front of the near cap is
/// seen. `sideways` lays it down, which is the disk and the drum.
fn cylinder(frame: Frame, adjust: &Adjustments, sideways: bool) -> Vec<Contour> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let mut contours = Vec::new();
    if sideways {
        let rx = (adjust.get(&["adj"], 0.25).clamp(0.0, 0.5) * w / 2.0).max(0.5);
        let cy = y + h / 2.0;
        let mut points = vec![(x + rx, y), (x + w - rx, y)];
        points.extend(arc_points(x + w - rx, cy, rx, h / 2.0, -90.0, 90.0, 24));
        points.push((x + w - rx, y + h));
        points.push((x + rx, y + h));
        points.extend(arc_points(x + rx, cy, rx, h / 2.0, 90.0, 270.0, 24));
        contours.push(polygon(&points));
        contours.push(line(&arc_points(
            x + w - rx,
            cy,
            rx,
            h / 2.0,
            90.0,
            270.0,
            24,
        )));
    } else {
        let ry = (adjust.get(&["adj"], 0.25).clamp(0.0, 0.5) * h / 2.0).max(0.5);
        let cx = x + w / 2.0;
        let mut points = vec![(x, y + ry)];
        points.extend(arc_points(cx, y + ry, w / 2.0, ry, 180.0, 360.0, 24));
        points.push((x + w, y + h - ry));
        points.extend(arc_points(cx, y + h - ry, w / 2.0, ry, 0.0, 180.0, 24));
        contours.push(polygon(&points));
        contours.push(line(&arc_points(cx, y + ry, w / 2.0, ry, 0.0, 180.0, 24)));
    }
    contours
}

/// A flowchart document: a box whose bottom edge dips and rises.
fn document_outline(frame: Frame) -> Vec<(f64, f64)> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let base = y + h * 0.8;
    let mut points = vec![(x, y), (x + w, y), (x + w, base)];
    points.extend(curve_points(
        (x + w, base),
        (x + w * 0.66, y + h * 1.06),
        (x + w * 0.33, y + h * 0.54),
        (x, base),
    ));
    points
}

/// A punched tape: a band with a wave along both its long edges.
fn tape_outline(frame: Frame) -> Vec<(f64, f64)> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let mut points = vec![(x, y + h * 0.25)];
    points.extend(curve_points(
        (x, y + h * 0.25),
        (x + w * 0.33, y - h * 0.05),
        (x + w * 0.66, y + h * 0.35),
        (x + w, y + h * 0.05),
    ));
    points.push((x + w, y + h * 0.85));
    points.extend(curve_points(
        (x + w, y + h * 0.85),
        (x + w * 0.66, y + h * 0.65),
        (x + w * 0.33, y + h * 1.05),
        (x, y + h * 0.65),
    ));
    points
}

/// A wave: a band whose long edges run the same course, half a period apart.
fn wave_outline(frame: Frame) -> Vec<(f64, f64)> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let mut points = vec![(x, y + h * 0.25)];
    points.extend(curve_points(
        (x, y + h * 0.25),
        (x + w * 0.25, y),
        (x + w * 0.75, y + h * 0.5),
        (x + w, y + h * 0.25),
    ));
    points.push((x + w, y + h * 0.75));
    points.extend(curve_points(
        (x + w, y + h * 0.75),
        (x + w * 0.75, y + h * 0.5),
        (x + w * 0.25, y + h),
        (x, y + h * 0.75),
    ));
    points
}

/// A cloud: the outline of a body and three bumps that overlap it, traced by
/// taking the outermost of them in every direction. That is the silhouette the
/// union of the discs would have, without the seams between them.
fn cloud_outline(frame: Frame) -> Vec<(f64, f64)> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    let discs: [(f64, f64, f64); 4] = [
        (0.50, 0.62, 0.34),
        (0.22, 0.52, 0.20),
        (0.44, 0.34, 0.24),
        (0.70, 0.45, 0.22),
    ];
    let (cx, cy) = (0.5, 0.55);
    let steps = 144;
    let mut points = Vec::with_capacity(steps);
    for step in 0..steps {
        let angle = step as f64 / steps as f64 * std::f64::consts::TAU;
        let (dx, dy) = (angle.cos(), angle.sin());
        let mut reach: f64 = 0.0;
        for &(ox, oy, radius) in &discs {
            let (ox, oy) = (ox - cx, oy - cy);
            let along = dx * ox + dy * oy;
            let offset = ox * ox + oy * oy - radius * radius;
            let discriminant = along * along - offset;
            if discriminant >= 0.0 {
                reach = reach.max(along + discriminant.sqrt());
            }
        }
        // The flat bottom is where the discs leave a band of nothing.
        let v = (cy + dy * reach).min(0.9);
        points.push((x + (cx + dx * reach) * w, y + v * h));
    }
    points
}

/// A heart: the classic 16 sin³ curve, fitted to the frame.
fn heart_outline(frame: Frame) -> Vec<(f64, f64)> {
    let (x, y, w, h) = (frame.left, frame.top, frame.width, frame.height);
    (0..64)
        .map(|step| {
            let t = step as f64 / 64.0 * std::f64::consts::TAU;
            let px = 16.0 * t.sin().powi(3);
            let py =
                13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
            // The curve spans 32 in width and 22 in height, from +5 at the
            // top to -17 at the point.
            (x + (0.5 + px / 32.0) * w, y + (5.0 - py) / 22.0 * h)
        })
        .collect()
}

/// A regular polygon, its first point at `start` degrees (y grows downward, so
/// -90 is the top).
fn regular(frame: Frame, sides: usize, start: f64) -> Contour {
    let (cx, cy) = frame.center();
    let points = (0..sides)
        .map(|step| {
            let angle = (start + step as f64 * 360.0 / sides as f64).to_radians();
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

/// A star: `points` outer vertices on the frame and as many inner ones at
/// `inner` of the radius between them.
fn star(frame: Frame, points: usize, inner: f64) -> Contour {
    let inner = inner.clamp(0.01, 0.99);
    let (cx, cy) = frame.center();
    let mut contour = Vec::with_capacity(points * 2);
    for step in 0..points * 2 {
        let angle = (-90.0 + step as f64 * 360.0 / (points * 2) as f64).to_radians();
        let radius = if step % 2 == 0 { 1.0 } else { inner };
        contour.push(metafile::pt(
            cx + frame.width / 2.0 * radius * angle.cos(),
            cy + frame.height / 2.0 * radius * angle.sin(),
        ));
    }
    Contour {
        points: contour,
        closed: true,
        filled: true,
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Right,
    Left,
    Up,
    Down,
}

/// A one-headed arrow in its frame: the body a fraction of the cross axis, the
/// head a fraction of the arrow's length (Word's `adj1`/`adj2`).
fn arrow(frame: Frame, direction: Direction, adjust: &Adjustments) -> Contour {
    let body = adjust.get(&["adj1", "adj"], 0.5).clamp(0.0, 1.0);
    let head = adjust.get(&["adj2"], 0.5).clamp(0.0, 1.0).min(0.95);
    let unit = [
        (0.0, (1.0 - body) / 2.0),
        (1.0 - head, (1.0 - body) / 2.0),
        (1.0 - head, 0.0),
        (1.0, 0.5),
        (1.0 - head, 1.0),
        (1.0 - head, (1.0 + body) / 2.0),
        (0.0, (1.0 + body) / 2.0),
    ];
    let place = |(u, v): (f64, f64)| {
        let (u, v) = match direction {
            Direction::Right => (u, v),
            Direction::Left => (1.0 - u, v),
            Direction::Down => (v, u),
            Direction::Up => (v, 1.0 - u),
        };
        (frame.left + u * frame.width, frame.top + v * frame.height)
    };
    polygon(&unit.map(place))
}

/// A two-headed arrow: `vertical` turns the horizontal one on its side.
fn double_arrow(frame: Frame, adjust: &Adjustments, vertical: bool) -> Contour {
    let body = adjust.get(&["adj1", "adj"], 0.5).clamp(0.0, 1.0);
    let head = adjust.get(&["adj2"], 0.5).clamp(0.0, 0.45);
    let unit = [
        (0.0, 0.5),
        (head, 0.0),
        (head, (1.0 - body) / 2.0),
        (1.0 - head, (1.0 - body) / 2.0),
        (1.0 - head, 0.0),
        (1.0, 0.5),
        (1.0 - head, 1.0),
        (1.0 - head, (1.0 + body) / 2.0),
        (head, (1.0 + body) / 2.0),
        (head, 1.0),
    ];
    let place = |(u, v): (f64, f64)| {
        let (u, v) = if vertical { (v, u) } else { (u, v) };
        (frame.left + u * frame.width, frame.top + v * frame.height)
    };
    polygon(&unit.map(place))
}

/// A shape's own path: `a:custGeom/a:pathLst`, each `a:path` in its own
/// `w`×`h` space (the shape's box when it states none).
fn custom_outline(geom: &Node, frame: Frame) -> Option<Vec<Contour>> {
    let list = geom.child("pathLst")?;
    let mut contours = Vec::new();
    for path in list.children.iter().filter(|child| child.name == "path") {
        let width = path
            .attr("w")
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|value| *value > 0.0)
            .unwrap_or(frame.width);
        let height = path
            .attr("h")
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|value| *value > 0.0)
            .unwrap_or(frame.height);
        let place = |point: &Node| -> Option<metafile::Point> {
            let x = point.attr("x")?.trim().parse::<f64>().ok()?;
            let y = point.attr("y")?.trim().parse::<f64>().ok()?;
            Some(metafile::pt(
                frame.left + x / width * frame.width,
                frame.top + y / height * frame.height,
            ))
        };
        let mut current: Option<Contour> = None;
        for command in &path.children {
            match command.name.as_str() {
                "moveTo" | "lnTo" => {
                    let Some(point) = command.child("pt").and_then(place) else {
                        continue;
                    };
                    if command.name == "moveTo" {
                        if let Some(done) = current.take() {
                            contours.push(done);
                        }
                        current = Some(Contour {
                            points: vec![point],
                            closed: false,
                            filled: true,
                        });
                    } else if let Some(contour) = current.as_mut() {
                        contour.points.push(point);
                    }
                }
                "cubicBezTo" | "quadBezTo" => {
                    let points: Vec<metafile::Point> = command
                        .children
                        .iter()
                        .filter(|child| child.name == "pt")
                        .filter_map(&place)
                        .collect();
                    let Some(contour) = current.as_mut() else {
                        continue;
                    };
                    // A curve is flattened to eight segments, the same reading
                    // the VML renderer gives one.
                    let from = contour
                        .points
                        .last()
                        .copied()
                        .unwrap_or(metafile::pt(frame.left, frame.top));
                    let from = (from.x, from.y);
                    match command.name.as_str() {
                        "cubicBezTo" if points.len() == 3 => {
                            let c1 = (points[0].x, points[0].y);
                            let c2 = (points[1].x, points[1].y);
                            let to = (points[2].x, points[2].y);
                            for step in 1..=8 {
                                let (x, y) = cubic(from, c1, c2, to, step as f64 / 8.0);
                                contour.points.push(metafile::pt(x, y));
                            }
                        }
                        "quadBezTo" if points.len() == 2 => {
                            let control = (points[0].x, points[0].y);
                            let to = (points[1].x, points[1].y);
                            for step in 1..=8 {
                                let (x, y) = quadratic(from, control, to, step as f64 / 8.0);
                                contour.points.push(metafile::pt(x, y));
                            }
                        }
                        _ => {}
                    }
                }
                "close" => {
                    if let Some(contour) = current.as_mut() {
                        contour.closed = true;
                    }
                }
                _ => {}
            }
        }
        if let Some(done) = current.take() {
            contours.push(done);
        }
    }
    contours.retain(|contour| contour.points.len() >= 2);
    (!contours.is_empty()).then_some(contours)
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

/// Mirror and turn a shape's outline the way its `a:xfrm` says.
fn transform(contours: &mut [Contour], frame: Frame, xfrm: &Xfrm) {
    for contour in contours {
        for point in &mut contour.points {
            *point = turned(*point, frame, xfrm);
        }
    }
}

/// Mirror and turn one point the way a shape's `a:xfrm` says: about the shape's
/// own middle, which is the box a fill is stretched across too.
fn turned(point: metafile::Point, frame: Frame, xfrm: &Xfrm) -> metafile::Point {
    let (cx, cy) = frame.center();
    let rotation = xfrm.rotation.to_radians();
    let (sin, cos) = rotation.sin_cos();
    let mut x = point.x;
    let mut y = point.y;
    if xfrm.flip_h {
        x = 2.0 * cx - x;
    }
    if xfrm.flip_v {
        y = 2.0 * cy - y;
    }
    if rotation != 0.0 {
        let dx = x - cx;
        let dy = y - cy;
        x = cx + dx * cos - dy * sin;
        y = cy + dx * sin + dy * cos;
    }
    metafile::pt(x, y)
}

/// The four corners of a shape's box, mirrored and turned with it: the box a
/// picture fill is stretched across.
fn quad_of(frame: Frame, xfrm: &Xfrm) -> [metafile::Point; 4] {
    [
        (frame.left, frame.top),
        (frame.right(), frame.top),
        (frame.right(), frame.bottom()),
        (frame.left, frame.bottom()),
    ]
    .map(|(x, y)| turned(metafile::pt(x, y), frame, xfrm))
}

/// What fills a shape: one colour, the ramp of a gradient, or a picture.
enum Fill {
    Solid(Color),
    Gradient(paint::Gradient),
    Picture(paint::Image),
}

/// The ink of a shape's line: its colour, its width, and the arrowheads the
/// line puts on its two ends.
#[derive(Debug, Clone, Copy)]
struct StrokeInk {
    color: Color,
    width: f64,
    /// The head at the line's first point.
    head: Option<paint::ArrowEnd>,
    /// The head at the line's last point.
    tail: Option<paint::ArrowEnd>,
}

/// A shape's soft shadow, as the file states it.
#[derive(Debug, Clone, Copy)]
struct ShadowInk {
    /// How far the shadow is moved from the shape, in points, x then y.
    offset: (f64, f64),
    /// The blur radius the shadow spreads over.
    blur: f64,
    color: Color,
    /// The shadow's own opacity, 0..=1.
    alpha: f64,
}

impl ShadowInk {
    /// The shadow's box, for the canvas that has to hold it.
    fn frame(&self, shape: Frame) -> Frame {
        let margin = self.blur;
        Frame {
            left: shape.left + self.offset.0 - margin,
            top: shape.top + self.offset.1 - margin,
            width: shape.width + margin * 2.0,
            height: shape.height + margin * 2.0,
        }
    }
}

/// What fills a shape, or `None` when it is not filled.
///
/// A fill the shape states itself wins; otherwise the `a:fillRef` of its
/// `style` says which theme fill it takes (`idx="0"` meaning none), and the
/// colour beside that reference stands for it. Both formats write that style as
/// `wps:style` or `p:style`, with the same `a:` children inside.
///
/// A `a:gradFill` is the ramp it states: its stops with their own opacity, and
/// the line or the path they run along. `frame` is the shape's own box, which
/// is what the ramp is measured across — a gradient is stated relative to the
/// shape it fills, so the same markup on a wider shape fades over a longer
/// distance. `rotation` is the shape's turn, which the ramp takes with it
/// (`rotWithShape`, which is what every writer states for a shape's own fill).
fn fill(
    sp_pr: &Node,
    shape: &Node,
    frame: Frame,
    rotation: f64,
    theme: &Theme,
    media: &dyn Media,
) -> Option<Fill> {
    if let Some(ink) = sp_pr.child_of(&["solidFill", "gradFill", "blipFill", "pattFill", "noFill"])
    {
        return match ink.name.as_str() {
            "noFill" => None,
            "gradFill" => gradient(ink, frame, rotation, theme).map(Fill::Gradient),
            "blipFill" => picture_fill(ink, frame, media).map(Fill::Picture),
            // A pattern's first colour stands in for it, as a gradient's first
            // stop used to.
            _ => first_color(ink, theme).map(Fill::Solid),
        };
    }
    let reference = shape.child("style")?.child("fillRef")?;
    let index = reference
        .attr("idx")
        .and_then(|raw| raw.trim().parse::<i32>().ok())?;
    (index > 0)
        .then(|| first_color(reference, theme).or_else(|| theme.scheme("accent1")))
        .flatten()
        .map(Fill::Solid)
}

/// The picture a shape's `a:blipFill` paints with, decoded into pixels.
///
/// A fill states its picture the way a picture states itself: the raster on the
/// `a:blip`, the drawing in the `asvg:svgBlip` extension beside it when the
/// package carries one. The drawing is the picture the file asks for — the
/// raster beside it is only what an older application shows — so it is taken
/// first, and the raster only when its part is not there. `None` when neither
/// resolves or neither decodes: the shape then keeps its line, and no invented
/// colour stands in for the picture.
fn picture_fill(fill: &Node, frame: Frame, media: &dyn Media) -> Option<paint::Image> {
    let blip = fill.child("blip")?;
    let drawing = find(blip, "svgBlip").and_then(|svg| svg.attr("embed"));
    let (mime, data) = drawing
        .and_then(|id| media.picture(id))
        .or_else(|| blip.attr("embed").and_then(|id| media.picture(id)))?;
    let mut image = decode(&mime, &data, frame)?;
    image.crop = crop(fill);
    Some(image)
}

/// The part of the picture a fill shows: `a:srcRect`'s percentages, as the
/// fraction of the picture taken off each edge, in the order left, top, right,
/// bottom.
fn crop(fill: &Node) -> [f64; 4] {
    let Some(rect) = fill.child("srcRect") else {
        return [0.0; 4];
    };
    let share = |name: &str| {
        rect.attr(name)
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .map(|thousandths| (thousandths / 100000.0).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    };
    [share("l"), share("t"), share("r"), share("b")]
}

/// The pixels a fill's picture holds.
///
/// A drawing is not an encoding: it is played at the size the fill gives it, at
/// the 144 dpi every other drawing in a package is played at. A raster is
/// decoded, and one the image crate has no decoder for — WebP, JPEG 2000 — is
/// transcoded to PNG first, exactly as a picture in the document's flow is.
/// `None` when the picture cannot be read at all.
fn decode(mime: &str, data: &[u8], frame: Frame) -> Option<paint::Image> {
    if mime.starts_with("image/svg") {
        let raster = pdfextractor_svg::rasterize(
            data,
            pdfextractor_svg::Options::default().at_size(frame.width, frame.height),
        )
        .ok()?;
        return Some(paint::Image {
            width: raster.width,
            height: raster.height,
            pixels: raster.pixels,
            crop: [0.0; 4],
        });
    }
    let bitmap = match image::load_from_memory(data) {
        Ok(bitmap) => bitmap,
        Err(_) => {
            let png = pdfextractor_image_parser::transcode_to_png(data)?;
            image::load_from_memory(&png.data).ok()?
        }
    }
    .to_rgba8();
    Some(paint::Image {
        width: bitmap.width(),
        height: bitmap.height(),
        pixels: bitmap.into_raw(),
        crop: [0.0; 4],
    })
}

/// The ramp a `a:gradFill` states, across `frame` and turned by `rotation`.
///
/// A gradient with no stops is no fill at all: there is nothing to interpolate
/// between, and painting the shape flat would be inventing a colour the file
/// never named.
fn gradient(fill: &Node, frame: Frame, rotation: f64, theme: &Theme) -> Option<paint::Gradient> {
    let mut stops: Vec<paint::GradientStop> = fill
        .child("gsLst")?
        .children
        .iter()
        .filter(|stop| stop.name == "gs")
        .filter_map(|stop| {
            let position = stop
                .attr("pos")
                .and_then(|raw| raw.trim().parse::<f64>().ok())
                .map(|raw| (raw / 100000.0).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let color = first_color(stop, theme)?;
            Some(paint::GradientStop {
                position,
                color,
                alpha: alpha(stop),
            })
        })
        .collect();
    if stops.len() < 2 {
        // One stop is flat ink, which is a fill; none is nothing to paint.
        return stops.pop().map(|stop| paint::Gradient {
            kind: paint::GradientKind::Linear {
                frame,
                angle: rotation,
                scaled: true,
            },
            stops: vec![stop; 2],
        });
    }
    // Stops are written in ramp order by every writer, but a file is free to
    // state them in any order and the ramp has to read the same either way.
    stops.sort_by(|a, b| a.position.total_cmp(&b.position));

    let kind = match fill.child("lin") {
        Some(lin) => {
            // `ang` is in 60000ths of a degree, clockwise from the x axis, and
            // `scaled` — true when absent — takes that angle in the shape's own
            // proportions. The shape's turn moves the ramp with it.
            let angle = lin
                .attr("ang")
                .and_then(|raw| raw.trim().parse::<f64>().ok())
                .map(|raw| raw / 60000.0)
                .unwrap_or(0.0);
            let scaled = lin
                .attr("scaled")
                .map(|raw| !matches!(raw.trim().to_ascii_lowercase().as_str(), "0" | "false"))
                .unwrap_or(true);
            paint::GradientKind::Linear {
                frame,
                angle: angle + rotation,
                scaled,
            }
        }
        // A path gradient (`circle`, `rect`, `shape`) runs out from the middle
        // of the shape to its edges: the ellipse the shape's own box states.
        None => paint::GradientKind::Radial {
            center: frame.center(),
            radius: (frame.width / 2.0, frame.height / 2.0),
            // DrawingML's path gradients start from a point: the shape's own
            // middle, with no focus region to hold the first stop flat.
            focus: (0.0, 0.0),
        },
    };
    Some(paint::Gradient { stops, kind })
}

/// A colour's own opacity, as an `a:alpha` states it: 100000 is opaque and
/// absent is opaque.
///
/// The transform sits inside the colour element — `a:srgbClr/a:alpha` — but a
/// shadow may write it beside the effect's other attributes, and a file is free
/// to state several transforms under one colour, of which the last is the one
/// applied. So the deepest `a:alpha` under `node` is the one that counts.
fn alpha(node: &Node) -> f64 {
    fn deepest(node: &Node) -> Option<f64> {
        let mut found = None;
        for child in &node.children {
            if child.name == "alpha" {
                if let Some(raw) = child
                    .attr("val")
                    .and_then(|raw| raw.trim().parse::<f64>().ok())
                {
                    found = Some((raw / 100000.0).clamp(0.0, 1.0));
                }
            }
            if let Some(inner) = deepest(child) {
                found = Some(inner);
            }
        }
        found
    }
    deepest(node).unwrap_or(1.0)
}

/// The soft shadow a shape states on its own properties, or `None`.
///
/// Only an `a:outerShdw` directly on the shape is read: that is what an
/// application writes when someone gives this shape a shadow, and it is the one
/// effect whose geometry is a question of where the ink goes rather than of how
/// it is coloured.
fn shadow(sp_pr: &Node, theme: &Theme) -> Option<ShadowInk> {
    let drop = sp_pr.child("effectLst")?.child("outerShdw")?;
    let emu = |name: &str| {
        drop.attr(name)
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .map(|raw| raw / EMU_PER_POINT)
            .unwrap_or(0.0)
    };
    // The direction is 60000ths of a degree, clockwise from the x axis; the
    // drawing's y grows downward, so the sine is the downward offset.
    let direction = drop
        .attr("dir")
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .map(|raw| raw / 60000.0)
        .unwrap_or(0.0)
        .to_radians();
    let distance = emu("dist");
    Some(ShadowInk {
        offset: (direction.cos() * distance, direction.sin() * distance),
        blur: emu("blurRad"),
        // A shadow that states no colour is black; its opacity is its own
        // `a:alpha`, and a shadow that states none is opaque — an application
        // writes a soft one by stating the alpha it wants.
        color: first_color(drop, theme).unwrap_or(Color::BLACK),
        alpha: alpha(drop),
    })
}

/// The colour and width of a shape's line, its arrowheads, or `None` when it
/// has no line.
fn stroke(shape: &Node, sp_pr: &Node, theme: &Theme) -> Option<StrokeInk> {
    let style = shape.child("style").and_then(|style| style.child("lnRef"));
    let style_width = |index: Option<i32>| {
        let index = index?;
        if index <= 0 {
            return None;
        }
        let emu = THEME_LINE_WIDTHS_EMU[(index as usize - 1).min(THEME_LINE_WIDTHS_EMU.len() - 1)];
        Some(emu / EMU_PER_POINT)
    };

    let line = sp_pr.child("ln");
    if let Some(line) = line {
        if line.child("noFill").is_some() {
            return None;
        }
        // A colour the line states itself is used as written; one taken from
        // the shape's `a:lnRef` is the theme's line style's colour, adjustments
        // and all.
        let color = line
            .child("solidFill")
            .and_then(|fill| first_color(fill, theme));
        let color = color.or_else(|| {
            let reference = style?;
            let base = first_color(reference, theme)?;
            let index = reference
                .attr("idx")
                .and_then(|raw| raw.trim().parse::<i32>().ok())
                .unwrap_or(0);
            Some(theme.line(index, base))
        });
        let width = line
            .attr("w")
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .map(|emu| emu / EMU_PER_POINT)
            .or_else(|| {
                style_width(
                    style
                        .and_then(|reference| reference.attr("idx"))
                        .and_then(|raw| raw.trim().parse::<i32>().ok()),
                )
            })
            .unwrap_or(DEFAULT_STROKE_PT);
        // A line with no colour of its own takes the theme's text colour, the
        // way the application's default outline does.
        return Some(StrokeInk {
            color: color.or_else(|| theme.scheme("dk1"))?,
            width,
            head: arrow_end(line, "headEnd", width),
            tail: arrow_end(line, "tailEnd", width),
        });
    }

    let reference = style?;
    let index = reference
        .attr("idx")
        .and_then(|raw| raw.trim().parse::<i32>().ok())?;
    if index <= 0 {
        return None;
    }
    let base = first_color(reference, theme).or_else(|| theme.scheme("accent1"))?;
    let color = theme.line(index, base);
    let width = style_width(Some(index)).unwrap_or(DEFAULT_STROKE_PT);
    Some(StrokeInk {
        color,
        width,
        head: None,
        tail: None,
    })
}

/// One end of a line: the arrowhead `a:headEnd`/`a:tailEnd` states.
///
/// The size is written as `w`/`len` of `sm`, `med` or `lg`, which are multiples
/// of the line's own width — 2×, 3× and 5×, the factors the applications use —
/// so a hairline's arrow is a hairline's arrow and a thick line's is thick.
fn arrow_end(line: &Node, name: &str, width: f64) -> Option<paint::ArrowEnd> {
    let end = line.child(name)?;
    let kind = match end.attr("type")?.trim() {
        "triangle" => paint::ArrowKind::Triangle,
        "stealth" => paint::ArrowKind::Stealth,
        "diamond" => paint::ArrowKind::Diamond,
        "oval" => paint::ArrowKind::Oval,
        "arrow" => paint::ArrowKind::Open,
        // `none` — and any name this does not know, which draws nothing rather
        // than something the file did not ask for.
        _ => return None,
    };
    let factor = |value: Option<&str>| match value.map(str::trim) {
        Some("sm") => 2.0,
        Some("lg") => 5.0,
        // `med`, and the default when the attribute is absent.
        _ => 3.0,
    };
    // A line that states no width is a hairline; an arrowhead on one is still
    // visible, so it is sized from the smallest width the applications use
    // rather than from zero.
    let base = width.max(0.5);
    Some(paint::ArrowEnd {
        kind,
        length: factor(end.attr("len")) * base,
        width: factor(end.attr("w")) * base,
    })
}

/// The first colour element among `node`'s children, or the node itself when it
/// is one.
fn first_color(node: &Node, theme: &Theme) -> Option<Color> {
    if let Some(color) = color(node, Some(theme)) {
        return Some(color);
    }
    node.children
        .iter()
        .find_map(|child| color(child, Some(theme)))
}

/// The RGB a DrawingML colour element states, with its `shade`/`tint` applied.
///
/// `theme` is absent only while the theme itself is being read, where the
/// colours are literal.
fn color(node: &Node, theme: Option<&Theme>) -> Option<Color> {
    let base = match node.name.as_str() {
        "srgbClr" => parse_hex(node.attr("val")?),
        "schemeClr" => theme.and_then(|theme| theme.scheme(node.attr("val")?)),
        "sysClr" => parse_hex(node.attr("lastClr")?),
        "scrgbClr" => {
            let channel = |name: &str| -> Option<u8> {
                let raw = node.attr(name)?.trim().parse::<f64>().ok()?;
                Some((raw / 100000.0 * 255.0).round().clamp(0.0, 255.0) as u8)
            };
            Some(Color {
                r: channel("r")?,
                g: channel("g")?,
                b: channel("b")?,
            })
        }
        "prstClr" => named_color(node.attr("val")?),
        _ => None,
    }?;
    let adjustments: Vec<(String, f64)> = node.children.iter().filter_map(adjustment).collect();
    Some(adjusted(base, &adjustments))
}

/// One adjustment of a colour element, as its name and the value it states
/// (a percentage of 100000).
fn adjustment(node: &Node) -> Option<(String, f64)> {
    let raw = node.attr("val")?;
    Some((node.name.clone(), raw.trim().parse::<f64>().ok()?))
}

/// `color` with a list of DrawingML adjustments applied, in order.
///
/// Only the ones this reader can state exactly are applied: `shade` mixes
/// toward black, `tint` toward white, and `satMod` scales saturation in HSL —
/// a theme's line style is where that last one appears, lifting an outline of
/// the fill's own hue off the fill. An adjustment it does not know leaves the
/// colour as it was rather than guessing.
fn adjusted(color: Color, adjustments: &[(String, f64)]) -> Color {
    adjustments.iter().fold(color, |color, (name, value)| {
        let share = (value / 100000.0).clamp(0.0, 1.0);
        match name.as_str() {
            "shade" => scale(color, share),
            "tint" => blend(Color::WHITE, color, share),
            "satMod" => saturation(color, value / 100000.0),
            _ => color,
        }
    })
}

/// `color` with its HSL saturation multiplied by `factor`, its hue and
/// lightness untouched.
fn saturation(color: Color, factor: f64) -> Color {
    let (r, g, b) = (
        f64::from(color.r) / 255.0,
        f64::from(color.g) / 255.0,
        f64::from(color.b) / 255.0,
    );
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f64::EPSILON {
        return color;
    }
    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    } * factor;
    let saturation = saturation.clamp(0.0, 1.0);
    let turn = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    let hue = turn * 60.0;
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let second = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match hue {
        h if h < 60.0 => (chroma, second, 0.0),
        h if h < 120.0 => (second, chroma, 0.0),
        h if h < 180.0 => (0.0, chroma, second),
        h if h < 240.0 => (0.0, second, chroma),
        h if h < 300.0 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let offset = lightness - chroma / 2.0;
    let channel = |value: f64| ((value + offset) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color {
        r: channel(r),
        g: channel(g),
        b: channel(b),
    }
}

fn scale(color: Color, factor: f64) -> Color {
    let channel = |value: u8| (f64::from(value) * factor).round().clamp(0.0, 255.0) as u8;
    Color {
        r: channel(color.r),
        g: channel(color.g),
        b: channel(color.b),
    }
}

/// `factor` of the way from `from` to `to`.
fn blend(from: Color, to: Color, factor: f64) -> Color {
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

fn parse_hex(raw: &str) -> Option<Color> {
    let raw = raw.trim().trim_start_matches('#');
    if raw.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(raw, 16).ok()?;
    Some(Color {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    })
}

fn named_color(name: &str) -> Option<Color> {
    let (r, g, b) = match name.trim().to_ascii_lowercase().as_str() {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "green" => (0, 128, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "gray" | "grey" => (128, 128, 128),
        "darkblue" => (0, 0, 139),
        "lightblue" => (173, 216, 230),
        "orange" => (255, 165, 0),
        "purple" => (128, 0, 128),
        _ => return None,
    };
    Some(Color { r, g, b })
}

/// Whether a transform's boolean attribute says yes. DrawingML writes `1` or
/// `true`; a VML `t` is accepted too, since a writer may reuse one.
/// The elements a shape is written as: Word's `wps:wsp` and `wps:cxnSp`,
/// PowerPoint's `p:sp` and `p:cxnSp`.
fn is_shape(name: &str) -> bool {
    matches!(name, "sp" | "wsp" | "cxnSp")
}

/// The elements a group of shapes is written as: Word's `wps:wgp` and its
/// `wpc:wpc` canvas, PowerPoint's `p:grpSp`.
fn is_group(name: &str) -> bool {
    matches!(name, "wgp" | "grpSp" | "wpc")
}

/// The elements a shape's text lives in: Word's `w:txbxContent`, PowerPoint's
/// `p:txBody`. The words are the document's own text, and a picture inside them
/// is placed by its own drawing, so neither is painted nor descended into.
fn is_text(name: &str) -> bool {
    matches!(name, "txbxContent" | "txBody")
}

fn flag(node: &Node, name: &str) -> bool {
    node.attr(name).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "t"
        )
    })
}

/// The first element named `name`, in document order.
fn find<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
    if node.name == name {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal Office theme: dark and light, and one accent.
    const THEME: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
        <a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
          <a:themeElements><a:clrScheme name="Office">
            <a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
            <a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
            <a:accent1><a:srgbClr val="156082"/></a:accent1>
          </a:clrScheme></a:themeElements>
        </a:theme>"#;

    fn drawing(inner: &str) -> Vec<u8> {
        format!(
            r#"<w:drawing xmlns:w="w" xmlns:wp="wp" xmlns:a="a" xmlns:wps="wps">
                 <wp:inline><wp:extent cx="1828800" cy="914400"/><a:graphic><a:graphicData>
                   {inner}
                 </a:graphicData></a:graphic></wp:inline>
               </w:drawing>"#
        )
        .into_bytes()
    }

    fn theme() -> Theme {
        Theme::parse(THEME.as_bytes())
    }

    /// A rect with an explicit fill and no line is a picture of its box in that
    /// colour, at 144 dpi.
    #[test]
    fn an_explicitly_filled_shape_becomes_a_picture() {
        let shape = r##"<wps:wsp><wps:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
              <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
              <a:solidFill><a:srgbClr val="ff0000"/></a:solidFill>
              <a:ln><a:noFill/></a:ln>
            </wps:spPr></wps:wsp>"##;
        let picture = render(&drawing(shape), &theme()).expect("a filled shape is a picture");
        assert_eq!((picture.width_pt, picture.height_pt), (72.0, 36.0));
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        assert_eq!(image.dimensions(), (144, 72));
        assert_eq!(image.get_pixel(72, 36).0, [255, 0, 0, 255]);
    }

    /// A shape coloured only by its theme style — `a:fillRef idx="1"` with
    /// `accent1` — takes the theme's accent, and its `lnRef` a dark shade of it.
    /// That is how a real Word file states a shape's colours.
    #[test]
    fn a_theme_styled_shape_takes_the_theme_colours() {
        let shape = r##"<wps:wsp><wps:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm>
              <a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>
              <a:ln w="127000"/>
            </wps:spPr>
            <wps:style>
              <a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef>
              <a:lnRef idx="2"><a:schemeClr val="accent1"><a:shade val="15000"/></a:schemeClr></a:lnRef>
            </wps:style></wps:wsp>"##;
        let picture = render(&drawing(shape), &theme()).expect("a theme-styled shape is a picture");
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        // The middle is the accent; the ellipse is smaller than its frame, so
        // the centre is fill and not line whatever the width.
        assert_eq!(image.get_pixel(72, 36).0, [21, 96, 130, 255]);
        // #156082 shaded to 15% is (3, 14, 19) on the rim, which the line's
        // own 10 pt width makes wide enough to sample inside.
        let rim = image.get_pixel(72, 2).0;
        assert!(
            rim[0] < 60 && rim[1] < 80 && rim[2] < 90,
            "the rim is not the shaded accent: {rim:?}"
        );
    }

    /// The picture a fill names, as a PNG: one colour, `size` pixels.
    fn solid_png(size: (u32, u32), color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(size.0, size.1, image::Rgba(color));
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("the fill encodes");
        png
    }

    /// A package's pictures, by the id a fill names. A drawing reads one id, so
    /// a test says which picture that is.
    struct Pictures(Vec<(&'static str, &'static str, Vec<u8>)>);

    impl Media for Pictures {
        fn picture(&self, id: &str) -> Option<(String, Vec<u8>)> {
            self.0
                .iter()
                .find(|(named, _, _)| *named == id)
                .map(|(_, mime, data)| ((*mime).to_string(), data.clone()))
        }
    }

    /// A shape can be filled with a picture, and the fill belongs to the shape:
    /// the picture is painted through the outline — inside the ellipse, not
    /// across its box — with the shape's own line drawn on top of it. What
    /// leaves the painter is the shape the page shows, not the fill's contents.
    #[test]
    fn a_picture_fill_is_painted_through_the_shape() {
        let shape = r##"<wps:wsp><wps:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="1143000" cy="635000"/></a:xfrm>
              <a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>
              <a:blipFill><a:blip r:embed="rId7"/><a:stretch><a:fillRect/></a:stretch></a:blipFill>
              <a:ln w="63500"><a:solidFill><a:srgbClr val="0000ff"/></a:solidFill></a:ln>
            </wps:spPr></wps:wsp>"##;
        let pictures = Pictures(vec![(
            "rId7",
            "image/png",
            solid_png((60, 36), [224, 140, 40, 255]),
        )]);
        let picture =
            render_with(&drawing(shape), &theme(), &pictures).expect("a picture-filled shape");
        assert_eq!((picture.width_pt, picture.height_pt), (90.0, 50.0));
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        assert_eq!(image.dimensions(), (180, 100));

        // The middle is the picture's own colour, at a size the 60 × 36 source
        // never had: the fill is stretched across the shape.
        assert_eq!(image.get_pixel(90, 50).0, [224, 140, 40, 255]);
        // The frame's corner is outside the ellipse, and no fill reached it.
        assert_eq!(
            image.get_pixel(1, 1).0,
            [255, 255, 255, 255],
            "the fill was laid down as a box instead of through the shape"
        );
        // The line is drawn over the fill on the shape's own rim.
        let rim = image.get_pixel(1, 50).0;
        assert!(
            rim[2] > 150 && rim[0] < 120,
            "the shape's line is not over its fill: {rim:?}"
        );
    }

    /// `a:srcRect` crops the picture a fill shows: the shape is filled with the
    /// part the file states, stretched across it, which is what a cropped fill
    /// looks like on the page.
    #[test]
    fn a_fill_shows_the_part_of_its_picture_the_file_states() {
        // A two-colour picture: red on the left, blue on the right.
        let mut halves = image::RgbaImage::new(40, 20);
        for (x, _, pixel) in halves.enumerate_pixels_mut() {
            *pixel = if x < 20 {
                image::Rgba([200, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 200, 255])
            };
        }
        let mut png = Vec::new();
        halves
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("the fill encodes");

        let shape = r##"<wps:wsp><wps:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
              <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
              <a:blipFill><a:blip r:embed="rId7"/><a:srcRect l="50000"/>
                <a:stretch><a:fillRect/></a:stretch></a:blipFill>
              <a:ln><a:noFill/></a:ln>
            </wps:spPr></wps:wsp>"##;
        let pictures = Pictures(vec![("rId7", "image/png", png)]);
        let picture = render_with(&drawing(shape), &theme(), &pictures).expect("a picture");
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        // The left half is cropped away, so the box shows only the blue half.
        assert_eq!(
            image.get_pixel(10, 36).0,
            [0, 0, 200, 255],
            "the crop did not take the picture's left half out"
        );
        assert!(
            image.pixels().all(|pixel| pixel.0 != [200, 0, 0, 255]),
            "the cropped-away half was painted"
        );
    }

    /// A star's outline is its points, not a box: the centre is filled and the
    /// corner of the frame is not.
    #[test]
    fn a_star_is_drawn_as_a_star() {
        let shape = r##"<wps:wsp><wps:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm>
              <a:prstGeom prst="star5"><a:avLst/></a:prstGeom>
              <a:solidFill><a:srgbClr val="00aa00"/></a:solidFill>
              <a:ln><a:noFill/></a:ln>
            </wps:spPr></wps:wsp>"##;
        let picture = render(&drawing(shape), &theme()).expect("a star is a picture");
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        assert_eq!(image.get_pixel(72, 72).0, [0, 170, 0, 255], "its middle");
        assert_eq!(
            image.get_pixel(4, 4).0,
            [255, 255, 255, 255],
            "between two points"
        );
    }

    /// A flip turns a connector the other way up: the line runs from the
    /// bottom-left to the top-right of its frame.
    #[test]
    fn a_flipped_connector_runs_the_other_way() {
        let shape = r##"<wps:wsp><wps:spPr>
              <a:xfrm flipV="1"><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
              <a:prstGeom prst="straightConnector1"><a:avLst/></a:prstGeom>
              <a:ln w="25400"><a:solidFill><a:srgbClr val="0000ff"/></a:solidFill></a:ln>
            </wps:spPr></wps:wsp>"##;
        let picture = render(&drawing(shape), &theme()).expect("a connector is a picture");
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        assert_eq!(image.get_pixel(3, 69).0[2], 255, "the lower left");
        assert_eq!(image.get_pixel(140, 3).0[2], 255, "the upper right");
        assert_eq!(image.get_pixel(140, 69).0, [255, 255, 255, 255]);
    }

    /// A group places its children in its own coordinate space, and its frame
    /// is the picture's frame.
    #[test]
    fn a_group_places_its_children() {
        let group = r##"<wps:wgp><wpg:grpSpPr xmlns:wpg="wpg">
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/>
                <a:chOff x="0" y="0"/><a:chExt x="1000" y="500"/></a:xfrm>
            </wpg:grpSpPr>
            <wps:wsp><wps:spPr>
              <a:xfrm><a:off x="600" y="0"/><a:ext cx="400" cy="500"/></a:xfrm>
              <a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>
              <a:solidFill><a:srgbClr val="6da33a"/></a:solidFill>
              <a:ln><a:noFill/></a:ln>
            </wps:spPr></wps:wsp>
          </wps:wgp>"##;
        let picture = render(&drawing(group), &theme()).expect("a group is a picture");
        assert_eq!((picture.width_pt, picture.height_pt), (72.0, 36.0));
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        assert_eq!(image.get_pixel(120, 36).0, [109, 163, 58, 255], "the oval");
        assert_eq!(
            image.get_pixel(20, 36).0,
            [255, 255, 255, 255],
            "left of it"
        );
    }

    /// A shape's text is the document's own text, and a text box with no fill
    /// and no line puts no ink down, so it is no picture.
    #[test]
    fn a_text_box_with_no_ink_is_not_a_picture() {
        let shape = r##"<wps:wsp><wps:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
              <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
              <a:noFill/><a:ln><a:noFill/></a:ln>
            </wps:spPr>
            <wps:style>
              <a:fillRef idx="0"><a:schemeClr val="accent1"/></a:fillRef>
              <a:lnRef idx="0"><a:schemeClr val="accent1"/></a:lnRef>
            </wps:style>
            <wps:txbx><w:txbxContent><w:p><w:r><w:t>Words</w:t></w:r></w:p></w:txbxContent></wps:txbx>
          </wps:wsp>"##;
        assert!(render(&drawing(shape), &theme()).is_none());
        // Nor is markup that is not a drawing at all.
        assert!(render(b"not xml", &theme()).is_none());
        assert!(render(&drawing(""), &theme()).is_none());
    }

    /// A theme's slots resolve, and a shade darkens the colour it applies to.
    #[test]
    fn a_theme_resolves_its_slots_and_shades() {
        let theme = theme();
        assert_eq!(
            theme.scheme("accent1"),
            Some(Color {
                r: 0x15,
                g: 0x60,
                b: 0x82
            })
        );
        assert_eq!(theme.scheme("tx1"), Some(Color::BLACK), "tx1 is dark 1");
        assert_eq!(theme.scheme("bg1"), Some(Color::WHITE), "bg1 is light 1");
    }

    /// PowerPoint spells the same shape `p:sp`, with its body in `p:spPr`, its
    /// theme references in `p:style` and its words in `p:txBody` — and the words
    /// are the slide's text, not ink.
    #[test]
    fn a_powerpoint_shape_is_painted_from_its_own_markup() {
        let xml = br#"<p:sp xmlns:p="p" xmlns:a="a">
              <p:spPr>
                <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
                <a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>
              </p:spPr>
              <p:style>
                <a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef>
                <a:lnRef idx="0"><a:schemeClr val="accent1"/></a:lnRef>
              </p:style>
              <p:txBody><a:bodyPr/><a:p><a:r><a:t>Words</a:t></a:r></a:p></p:txBody>
            </p:sp>"#;
        let tree = paint::parse(xml).expect("xml");
        let picture = render_node(&tree, &theme()).expect("a slide shape with ink is a picture");
        assert_eq!((picture.width_pt, picture.height_pt), (72.0, 36.0));
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        assert_eq!(
            image.get_pixel(72, 36).0,
            [21, 96, 130, 255],
            "the theme's accent in the middle"
        );
    }

    /// A horizontal connector states no height at all — PowerPoint writes
    /// `ext cy="0"` for a line that does not rise. The frame is no canvas, so
    /// the box is the stroke: the line is still the ink a reader needs.
    #[test]
    fn a_line_with_no_height_is_still_a_picture() {
        let xml = br#"<p:cxnSp xmlns:p="p" xmlns:a="a"><p:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="0"/></a:xfrm>
              <a:prstGeom prst="line"><a:avLst/></a:prstGeom>
              <a:ln w="25400"><a:solidFill><a:srgbClr val="1f4e79"/></a:solidFill></a:ln>
            </p:spPr></p:cxnSp>"#;
        let tree = paint::parse(xml).expect("xml");
        let picture = render_node(&tree, &theme()).expect("a line is a picture");
        // 72 pt long, and the stroke's own 2 pt tall.
        assert_eq!((picture.width_pt, picture.height_pt), (74.0, 2.0));
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        assert_eq!(image.dimensions(), (148, 4));
        assert_eq!(image.get_pixel(74, 2).0, [31, 78, 121, 255], "the line");
    }

    /// One preset painted into a 90 × 50 pt frame, as a slide's shape.
    fn arrow(name: &str) -> image::RgbaImage {
        let xml = format!(
            r##"<p:sp xmlns:p="p" xmlns:a="a"><p:spPr>
                   <a:xfrm><a:off x="0" y="0"/><a:ext cx="1143000" cy="635000"/></a:xfrm>
                   <a:prstGeom prst="{name}"><a:avLst/></a:prstGeom>
                   <a:solidFill><a:srgbClr val="1f4e79"/></a:solidFill>
                   <a:ln><a:noFill/></a:ln>
                 </p:spPr></p:sp>"##
        );
        painted(&xml)
    }

    /// The arrows that bend round: each is the band and the head its outline
    /// states, not a box standing in for it.
    #[test]
    fn the_curved_arrows_draw_their_band_and_their_head() {
        // Every one of them fills its frame at 144 dpi.
        for name in [
            "curvedRightArrow",
            "curvedLeftArrow",
            "curvedUpArrow",
            "curvedDownArrow",
            "uturnArrow",
            "circularArrow",
            "swooshArrow",
        ] {
            let image = arrow(name);
            assert_eq!(image.dimensions(), (180, 100), "{name} is not its frame");
        }

        // A curved right arrow leaves the frame's left edge as a band and puts
        // its head against the right edge, so the left edge has ink and the
        // head's point does too.
        let right = arrow("curvedRightArrow");
        assert_eq!(right.get_pixel(1, 40).0, INK, "the band at the left edge");
        assert!(
            (0..100).any(|y| right.get_pixel(179, y).0 == INK),
            "the head does not reach the right edge"
        );
        // Its left arrow is the same band the other way round.
        let left = arrow("curvedLeftArrow");
        assert_eq!(left.get_pixel(178, 40).0, INK, "the band at the right edge");
        assert!(
            (0..100).any(|y| left.get_pixel(0, y).0 == INK),
            "the head does not reach the left edge"
        );

        // The up arrow is a bowl: its arms are inked at the top and the middle
        // between them is empty.
        let up = arrow("curvedUpArrow");
        assert_eq!(up.get_pixel(6, 4).0, INK, "the left arm");
        assert_eq!(up.get_pixel(157, 4).0, INK, "the head");
        assert_eq!(up.get_pixel(145, 60).0, INK, "the right arm");
        assert_eq!(up.get_pixel(90, 30).0, WHITE, "the bowl's mouth");
        // And the down arrow is that bowl the other way up.
        let down = arrow("curvedDownArrow");
        assert_eq!(down.get_pixel(6, 95).0, INK, "the left arm");
        assert_eq!(down.get_pixel(157, 95).0, INK, "the head");
        assert_eq!(down.get_pixel(145, 40).0, INK, "the right arm");
        assert_eq!(down.get_pixel(90, 70).0, WHITE, "the bowl's mouth");

        // A u-turn is two arms joined at the top, with the head pointing down
        // between them over an empty middle.
        let uturn = arrow("uturnArrow");
        assert_eq!(uturn.get_pixel(10, 50).0, INK, "the left arm");
        assert_eq!(uturn.get_pixel(150, 50).0, INK, "the right arm");
        assert_eq!(uturn.get_pixel(90, 40).0, WHITE, "the middle");

        // A circular arrow is a ring with a gap rather than a disc: the band
        // runs over the top, and the middle of the circle is empty.
        let circular = arrow("circularArrow");
        assert_eq!(circular.get_pixel(90, 6).0, INK, "the band");
        assert_eq!(circular.get_pixel(90, 50).0, WHITE, "the hole");
        assert_eq!(circular.get_pixel(90, 95).0, WHITE, "the gap under it");

        // A swoosh rises from its tail at the bottom left to its head at the
        // top right, so the two corners it names are inked and the middle
        // below the band is not.
        let swoosh = arrow("swooshArrow");
        assert!(
            (0..100).any(|y| swoosh.get_pixel(2, y).0 == INK),
            "the tail"
        );
        assert!(
            (0..100).any(|y| swoosh.get_pixel(176, y).0 == INK),
            "the head"
        );
        assert_eq!(swoosh.get_pixel(140, 95).0, WHITE, "below the band");
    }

    /// The presets beyond the handful a Word document tends to state: each
    /// answers an outline of its own rather than `None`, which is what leaves
    /// the shape out of the document altogether.
    #[test]
    fn the_wider_preset_library_draws_its_shapes() {
        const PRESETS: &[&str] = &[
            // Rounded and snipped rectangles.
            "round1Rect",
            "round2SameRect",
            "round2DiagRect",
            "snip1Rect",
            "snip2SameRect",
            "snip2DiagRect",
            "snipRoundRect",
            "plaque",
            // Rings, frames and bands.
            "donut",
            "noSmoking",
            "frame",
            "halfFrame",
            "corner",
            "diagStripe",
            // The mathematical signs.
            "mathMinus",
            "mathEqual",
            "mathNotEqual",
            "mathDivide",
            "mathMultiply",
            // Brackets and braces.
            "leftBracket",
            "rightBracket",
            "leftBrace",
            "rightBrace",
            // The many-pointed stars.
            "star12",
            "star16",
            "star24",
            "star32",
            // Solids.
            "cube",
            "bevel",
            "foldedCorner",
            "can",
            // Flowchart symbols.
            "flowChartMagneticDisk",
            "flowChartMagneticDrum",
            "flowChartMagneticTape",
            "flowChartDatabase",
            "flowChartDocument",
            "flowChartPunchedCard",
            "flowChartPunchedTape",
            "flowChartDelay",
            "flowChartManualInput",
            "flowChartManualOperation",
            "flowChartMerge",
            "flowChartExtract",
            "flowChartOffpageConnector",
            "flowChartPredefinedProcess",
            "flowChartInternalStorage",
            "flowChartPreparation",
            "flowChartSort",
            // Arrows.
            "quadArrow",
            "bentArrow",
            "bentUpArrow",
            "notchedRightArrow",
            // The arrows that bend round rather than step.
            "curvedRightArrow",
            "curvedLeftArrow",
            "curvedUpArrow",
            "curvedDownArrow",
            "uturnArrow",
            "circularArrow",
            "swooshArrow",
            // Decorative outlines.
            "wave",
            "cloud",
            "heart",
            "moon",
            "sun",
            "smileyFace",
            "teardrop",
            // Parts of a circle.
            "arc",
            "pie",
            "chord",
            "blockArc",
            // Connectors.
            "straightConnector2",
            "straightConnector3",
            "bentConnector2",
            "bentConnector3",
            "curvedConnector2",
            "curvedConnector3",
            // The callouts a writer names without a border.
            "callout1",
            "accentCallout2",
            "accentBorderCallout3",
        ];
        let frame = Frame {
            left: 0.0,
            top: 0.0,
            width: 90.0,
            height: 50.0,
        };
        for name in PRESETS {
            let contours = preset_outline(name, frame, &Adjustments::default())
                .unwrap_or_else(|| panic!("{name} is not drawn"));
            assert!(
                contours.iter().all(|contour| contour.points.len() >= 2),
                "{name} has a contour with no point"
            );
            assert!(!contours.is_empty(), "{name} has no outline");
        }
        // A preset the table does not know is not a box: it is no picture.
        assert!(preset_outline("notAPreset", frame, &Adjustments::default()).is_none());
    }

    /// A line of the drawing — a cube's front edge, a cylinder's rim, a brace's
    /// spine — is stroked and never filled, because filling it would paint a
    /// band across the shape it belongs to.
    #[test]
    fn a_line_of_the_drawing_is_never_filled() {
        let frame = Frame {
            left: 0.0,
            top: 0.0,
            width: 90.0,
            height: 50.0,
        };
        let cube = preset_outline("cube", frame, &Adjustments::default()).expect("a cube");
        assert!(
            cube.iter().any(|contour| contour.filled),
            "the cube has no faces"
        );
        assert!(
            cube.iter().any(|contour| !contour.filled),
            "the cube's front edge is missing"
        );

        for name in [
            "leftBrace",
            "rightBrace",
            "bentConnector3",
            "curvedConnector2",
        ] {
            let contours = preset_outline(name, frame, &Adjustments::default())
                .unwrap_or_else(|| panic!("{name} is not drawn"));
            assert!(
                contours.iter().all(|contour| !contour.filled),
                "{name} was filled as if it were a region"
            );
        }
    }

    /// A sort's bar is a path of the preset with no fill, so it is a line of
    /// the drawing and takes the shape's line colour. That bar is the only
    /// thing between a sort and a decision, which are otherwise the same
    /// diamond filled the same way.
    #[test]
    fn a_flowchart_sort_draws_its_bar_in_the_line_colour() {
        let flowchart = |name: &str| {
            painted(&format!(
                r##"<p:sp xmlns:p="p" xmlns:a="a"><p:spPr>
                      <a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="914400"/></a:xfrm>
                      <a:prstGeom prst="{name}"><a:avLst/></a:prstGeom>
                      <a:solidFill><a:srgbClr val="1f4e79"/></a:solidFill>
                      <a:ln><a:solidFill><a:srgbClr val="c00000"/></a:solidFill></a:ln>
                    </p:spPr></p:sp>"##
            ))
        };
        const LINE: [u8; 4] = [192, 0, 0, 255];
        let sort = flowchart("flowChartSort");
        let decision = flowchart("flowChartDecision");
        let (x, y) = (sort.width() / 2, sort.height() / 2);
        assert_eq!(sort.get_pixel(x, y).0, LINE, "the bar across the middle");
        assert_eq!(sort.get_pixel(x, y - 20).0, INK, "the fill above the bar");
        assert_eq!(
            decision.get_pixel(x, y).0,
            INK,
            "a decision has no bar in the middle of its diamond"
        );
    }

    /// A donut is a ring: the inner contour is a hole under the alternate fill
    /// rule, not another disc of fill.
    #[test]
    fn a_ring_leaves_its_hole_empty() {
        let shape = r##"<wps:wsp><wps:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm>
              <a:prstGeom prst="donut"><a:avLst/></a:prstGeom>
              <a:solidFill><a:srgbClr val="156082"/></a:solidFill>
              <a:ln><a:noFill/></a:ln>
            </wps:spPr></wps:wsp>"##;
        let picture = render(&drawing(shape), &theme()).expect("a donut is a picture");
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        assert_eq!(image.get_pixel(72, 72).0, [255, 255, 255, 255], "the hole");
        assert_eq!(image.get_pixel(72, 8).0, [21, 96, 130, 255], "the ring");
    }

    /// `blockArc`'s angles are guides in 1/60000 of a degree, and the default
    /// is half a ring: the band runs along the top, not a sliver of it.
    #[test]
    fn an_arc_reads_its_angles_as_degrees() {
        let shape = r##"<wps:wsp><wps:spPr>
              <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
              <a:prstGeom prst="blockArc"><a:avLst/></a:prstGeom>
              <a:solidFill><a:srgbClr val="156082"/></a:solidFill>
              <a:ln><a:noFill/></a:ln>
            </wps:spPr></wps:wsp>"##;
        let picture = render(&drawing(shape), &theme()).expect("a block arc is a picture");
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        // The band runs along the top of the frame; a quarter of the way out
        // it is still fill, where a 60° arc would already have ended.
        assert_eq!(image.get_pixel(72, 4).0, [21, 96, 130, 255], "the top");
        assert_eq!(image.get_pixel(41, 9).0, [21, 96, 130, 255], "the band");
        assert_eq!(
            image.get_pixel(72, 60).0,
            [255, 255, 255, 255],
            "the middle"
        );
    }

    /// Units: an `a:off`/`a:ext` is EMU at the top level and path units inside a
    /// group.
    #[test]
    fn a_transform_is_read_in_the_unit_of_its_space() {
        let xfrm = paint::parse(
            br#"<a:xfrm><a:off x="12700" y="25400"/><a:ext cx="914400" cy="457200"/></a:xfrm>"#,
        )
        .expect("xml");
        let top = point_of(xfrm.child("off"), true).expect("an offset");
        assert_eq!(top, (1.0, 2.0));
        let ext = point_of(xfrm.child("ext"), true).expect("an extent");
        assert_eq!(ext, (72.0, 36.0));
    }

    /// The picture a slide's shape paints to.
    fn picture_of(xml: &str) -> Picture {
        let tree = paint::parse(xml.as_bytes()).expect("xml");
        render_node(&tree, &theme()).expect("a picture")
    }

    fn painted(xml: &str) -> image::RgbaImage {
        image::load_from_memory(&picture_of(xml).png)
            .expect("a PNG")
            .to_rgba8()
    }

    const INK: [u8; 4] = [31, 78, 121, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    /// A horizontal connector, 72 pt long and 2 pt wide, with the arrowheads
    /// the two ends name.
    ///
    /// Its box is its ink: the stroke reaches 1 pt past the line either way and
    /// the widest head reaches 10 pt, so the canvas starts 11 pt left of the
    /// line — 22 px at 144 dpi — and the line runs from x = 22 px to x = 166 px.
    fn connector(head: &str, tail: &str) -> image::RgbaImage {
        let xml = format!(
            r##"<p:cxnSp xmlns:p="p" xmlns:a="a"><p:spPr>
                   <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="0"/></a:xfrm>
                   <a:prstGeom prst="line"><a:avLst/></a:prstGeom>
                   <a:ln w="25400"><a:solidFill><a:srgbClr val="1f4e79"/></a:solidFill>
                     {head}{tail}</a:ln>
                 </p:spPr></p:cxnSp>"##
        );
        painted(&xml)
    }

    /// A `a:gradFill` is the ramp it states, measured across the shape: red at
    /// the left, blue at the right, the mix between them — and the same colour
    /// all the way down each column, because the ramp is linear.
    #[test]
    fn a_gradient_fill_ramps_across_the_shape() {
        let image = painted(
            r##"<p:sp xmlns:p="p" xmlns:a="a"><p:spPr>
                  <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
                  <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                  <a:gradFill><a:gsLst>
                    <a:gs pos="0"><a:srgbClr val="ff0000"/></a:gs>
                    <a:gs pos="100000"><a:srgbClr val="0000ff"/></a:gs>
                  </a:gsLst><a:lin ang="0" scaled="1"/></a:gradFill>
                  <a:ln><a:noFill/></a:ln>
                </p:spPr></p:sp>"##,
        );
        assert_eq!(image.dimensions(), (144, 72));
        let start = image.get_pixel(2, 36).0;
        let end = image.get_pixel(141, 36).0;
        let middle = image.get_pixel(72, 36).0;
        assert!(
            start[0] > 230 && start[2] < 25,
            "the ramp starts red: {start:?}"
        );
        assert!(end[2] > 230 && end[0] < 25, "the ramp ends blue: {end:?}");
        assert!(
            (i32::from(middle[0]) - 127).abs() < 8 && (i32::from(middle[2]) - 127).abs() < 8,
            "the middle is halfway between the two stops: {middle:?}"
        );
        assert_eq!(
            image.get_pixel(30, 8).0,
            image.get_pixel(30, 64).0,
            "a linear ramp is the same colour down a column"
        );
    }

    /// `a:lin@scaled` takes the angle in the shape's own proportions: 45° runs
    /// corner to corner on a wide box, where the true 45° of `scaled="0"` is a
    /// shallower ramp that reaches the corner only later.
    #[test]
    fn a_scaled_ramp_takes_its_angle_in_the_shapes_proportions() {
        let ramp = |scaled: &str| {
            painted(&format!(
                r##"<p:sp xmlns:p="p" xmlns:a="a"><p:spPr>
                       <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
                       <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                       <a:gradFill><a:gsLst>
                         <a:gs pos="0"><a:srgbClr val="000000"/></a:gs>
                         <a:gs pos="100000"><a:srgbClr val="ffffff"/></a:gs>
                       </a:gsLst><a:lin ang="2700000" scaled="{scaled}"/></a:gradFill>
                       <a:ln><a:noFill/></a:ln>
                     </p:spPr></p:sp>"##
            ))
        };
        // Half way down the left edge: a quarter of the way along a ramp that
        // runs corner to corner, a sixth of the way along a true 45° one.
        let scaled = ramp("1").get_pixel(1, 36).0[0];
        let true_angle = ramp("0").get_pixel(1, 36).0[0];
        assert!(
            (55..=70).contains(&scaled),
            "the corner-to-corner ramp is a quarter along: {scaled}"
        );
        assert!(
            (35..=48).contains(&true_angle),
            "the true 45° ramp is a sixth along: {true_angle}"
        );
    }

    /// A stop carries its own opacity, so a shape can fade out where the ramp
    /// says: the end stop here is fully transparent and the fill there is the
    /// surface underneath.
    #[test]
    fn a_gradient_stop_fades_the_shape_out() {
        let image = painted(
            r##"<p:sp xmlns:p="p" xmlns:a="a"><p:spPr>
                  <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
                  <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                  <a:gradFill><a:gsLst>
                    <a:gs pos="0"><a:srgbClr val="ff0000"><a:alpha val="100000"/></a:srgbClr></a:gs>
                    <a:gs pos="100000"><a:srgbClr val="0000ff"><a:alpha val="0"/></a:srgbClr></a:gs>
                  </a:gsLst><a:lin ang="0"/></a:gradFill>
                  <a:ln><a:noFill/></a:ln>
                </p:spPr></p:sp>"##,
        );
        assert!(
            image.get_pixel(2, 36).0[0] > 240,
            "the opaque end is red: {:?}",
            image.get_pixel(2, 36).0
        );
        let faded = image.get_pixel(143, 36).0;
        assert!(
            faded.iter().all(|channel| *channel > 225),
            "the transparent end is the white surface: {faded:?}"
        );
    }

    /// A path gradient (`a:path path="circle"`) runs out from the middle of the
    /// shape to its edges, so on a wide box it is an ellipse that follows the
    /// frame rather than a circle inside it.
    #[test]
    fn a_path_gradient_runs_out_from_the_middle() {
        let image = painted(
            r##"<p:sp xmlns:p="p" xmlns:a="a"><p:spPr>
                  <a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>
                  <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                  <a:gradFill><a:gsLst>
                    <a:gs pos="0"><a:srgbClr val="ff0000"/></a:gs>
                    <a:gs pos="100000"><a:srgbClr val="0000ff"/></a:gs>
                  </a:gsLst><a:path path="circle"><a:fillToRect l="50000" t="50000" r="50000" b="50000"/></a:path></a:gradFill>
                  <a:ln><a:noFill/></a:ln>
                </p:spPr></p:sp>"##,
        );
        let middle = image.get_pixel(72, 36).0;
        assert!(
            middle[0] > 240 && middle[2] < 15,
            "red at the middle: {middle:?}"
        );
        let side = image.get_pixel(140, 36).0;
        let top = image.get_pixel(72, 3).0;
        assert!(
            side[2] > 200 && side[0] < 45,
            "blue at the right edge: {side:?}"
        );
        assert!(
            top[2] > 200,
            "and blue at the top edge, because the radius is the box's: {top:?}"
        );
    }

    /// A soft shadow is the shape's silhouette, moved by `dist` along `dir` and
    /// laid down under it at the alpha it states.
    #[test]
    fn a_shadow_lands_below_and_beside_the_shape() {
        let picture = picture_of(
            r##"<p:sp xmlns:p="p" xmlns:a="a"><p:spPr>
                  <a:xfrm><a:off x="0" y="0"/><a:ext cx="457200" cy="228600"/></a:xfrm>
                  <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                  <a:solidFill><a:srgbClr val="ff0000"/></a:solidFill>
                  <a:ln><a:noFill/></a:ln>
                  <a:effectLst><a:outerShdw blurRad="0" dist="254000" dir="2700000" rotWithShape="0">
                    <a:srgbClr val="808080"><a:alpha val="50000"/></a:srgbClr>
                  </a:outerShdw></a:effectLst>
                </p:spPr></p:sp>"##,
        );
        // 36 × 18 pt of shape, and 20 pt of shadow at 45°: the box is the ink,
        // so it is both wider and taller than the shape.
        assert!(
            picture.width_pt > 40.0 && picture.height_pt > 25.0,
            "the canvas is the ink: {} × {}",
            picture.width_pt,
            picture.height_pt
        );
        let image = image::load_from_memory(&picture.png)
            .expect("a PNG")
            .to_rgba8();
        assert_eq!(
            image.get_pixel(36, 18).0,
            [255, 0, 0, 255],
            "the shape is over its own shadow"
        );
        let shade = image.get_pixel(90, 50).0;
        assert!(
            shade[0].abs_diff(shade[1]) < 4 && shade[1].abs_diff(shade[2]) < 4,
            "the shadow is the grey it states, not the fill: {shade:?}"
        );
        assert!(
            (180..=205).contains(&shade[0]),
            "half of #808080 over white is the shadow: {shade:?}"
        );
    }

    /// The bottom-right corner of the red box a shadow test paints.
    fn red_corner(image: &image::RgbaImage) -> (u32, u32) {
        let mut corner = (0, 0);
        for (x, y, pixel) in image.enumerate_pixels() {
            if pixel.0 == [255, 0, 0, 255] {
                corner.0 = corner.0.max(x);
                corner.1 = corner.1.max(y);
            }
        }
        corner
    }

    /// `blurRad` softens the shadow's edge: a blurred one reaches past the
    /// silhouette it is cut from, where a hard one stops at it.
    #[test]
    fn a_blurred_shadow_reaches_past_the_silhouette() {
        let shadow = |blur: &str| {
            painted(&format!(
                r##"<p:sp xmlns:p="p" xmlns:a="a"><p:spPr>
                       <a:xfrm><a:off x="0" y="0"/><a:ext cx="457200" cy="228600"/></a:xfrm>
                       <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
                       <a:solidFill><a:srgbClr val="ff0000"/></a:solidFill>
                       <a:ln><a:noFill/></a:ln>
                       <a:effectLst><a:outerShdw blurRad="{blur}" dist="254000" dir="2700000" rotWithShape="0">
                         <a:srgbClr val="000000"><a:alpha val="50000"/></a:srgbClr>
                       </a:outerShdw></a:effectLst>
                     </p:spPr></p:sp>"##
            ))
        };
        let hard = shadow("0");
        let soft = shadow("254000");
        // The shadow is 20 pt at 45° from the shape, so its far corner is 28 px
        // down and right of the box'. A hard shadow covers that corner exactly;
        // a blurred one only half covers it, so it is lighter there.
        let corner = |image: &image::RgbaImage| {
            let (right, bottom) = red_corner(image);
            image.get_pixel(right + 28, bottom + 28).0
        };
        let hard_corner = corner(&hard)[0];
        assert!(
            hard_corner < 150,
            "a hard shadow is as opaque at its edge as inside: {hard_corner}"
        );
        let soft_corner = corner(&soft)[0];
        assert!(
            soft_corner > hard_corner + 15,
            "a blurred shadow falls off at its edge: {soft_corner} against {hard_corner}"
        );
        assert!(
            soft_corner < 250,
            "and still puts ink past the silhouette: {soft_corner}"
        );
    }

    /// A connector's arrowheads, each cut as its own shape: the triangle filled
    /// to its base, the stealth notched back into it, the diamond reaching
    /// behind the tip, the oval round at both ends, and the open one two
    /// strokes rather than a fill.
    #[test]
    fn an_arrowhead_is_drawn_as_the_kind_it_states() {
        // The tail's tip is at x = 166 px and its base 10 pt — 20 px — back.
        let triangle = connector("", r#"<a:tailEnd type="triangle" w="lg" len="lg"/>"#);
        let cy = triangle.height() / 2;
        assert_eq!(triangle.get_pixel(164, cy).0, INK, "the tip");
        assert_eq!(triangle.get_pixel(150, cy).0, INK, "the body");
        assert_eq!(triangle.get_pixel(150, cy - 14).0, WHITE, "outside it");

        let stealth = connector("", r#"<a:tailEnd type="stealth" w="lg" len="lg"/>"#);
        let cy = stealth.height() / 2;
        assert_eq!(stealth.get_pixel(160, cy).0, INK, "inside the V");
        assert_eq!(
            stealth.get_pixel(151, cy).0,
            WHITE,
            "the notch cut back toward the tip, and the stroke stopped at the base"
        );

        let diamond = connector("", r#"<a:tailEnd type="diamond" w="lg" len="lg"/>"#);
        let cy = diamond.height() / 2;
        assert_eq!(diamond.get_pixel(164, cy).0, INK, "the tip");
        assert_eq!(diamond.get_pixel(150, cy - 1).0, INK, "the waist");
        assert_eq!(
            diamond.get_pixel(150, cy - 6).0,
            WHITE,
            "narrower than the triangle would be there"
        );

        let oval = connector("", r#"<a:tailEnd type="oval" w="lg" len="lg"/>"#);
        let cy = oval.height() / 2;
        assert_eq!(oval.get_pixel(164, cy).0, INK, "the tip");
        assert_eq!(oval.get_pixel(156, cy - 9).0, INK, "its round middle");
        assert_eq!(
            oval.get_pixel(147, cy - 9).0,
            WHITE,
            "and round again at its back, where the triangle is still wide"
        );

        let open = connector("", r#"<a:tailEnd type="arrow" w="lg" len="lg"/>"#);
        let cy = open.height() / 2;
        assert_eq!(
            open.get_pixel(160, cy).0,
            WHITE,
            "the open arrow is two strokes, so the axis behind the tip is empty"
        );
        assert_eq!(open.get_pixel(156, cy - 5).0, INK, "along one stroke");
    }

    /// A head at the start of a line points the other way: out of the line at
    /// its first point, not back along it.
    #[test]
    fn an_arrowhead_on_the_start_of_a_line_points_out_of_it() {
        let image = connector(r#"<a:headEnd type="triangle" w="lg" len="lg"/>"#, "");
        let cy = image.height() / 2;
        // The line starts at x = 22 px: the tip is there and the base 20 px on,
        // inside the line.
        assert_eq!(image.get_pixel(24, cy).0, INK, "the tip at the start");
        assert_eq!(image.get_pixel(36, cy - 5).0, INK, "the body behind it");
        assert_eq!(
            image.get_pixel(36, cy - 14).0,
            WHITE,
            "and beyond its edge there is nothing"
        );
        assert_eq!(
            image.get_pixel(16, cy).0,
            WHITE,
            "nor past the line's own start, where the head would point the wrong way"
        );
    }
}
