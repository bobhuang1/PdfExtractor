//! Reading an OpenDocument presentation into its slides and shapes.

use anyhow::{Context, Result};
use pdfextractor_limits as limits;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::Cursor;

use pdfextractor_picture_formats::{format_for_extension, format_for_mime};

mod chart;

/// Slide size when no page layout states one: 10 × 7.5 inches in EMU,
/// PowerPoint's 4:3 default, which is also LibreOffice's common 25.4 × 19.05 cm
/// presentation size.
const DEFAULT_WIDTH: f64 = 9_144_000.0;
const DEFAULT_HEIGHT: f64 = 6_858_000.0;
const EMU_PER_POINT: f64 = 12_700.0;

/// A repeated row or cell above this is filler, not content.
const MAX_REPEAT: usize = 1024;

/// A parsed presentation.
#[derive(Debug, Clone)]
pub struct OdpDocument {
    /// Slide width and height in EMU, from the page layout.
    pub width: f64,
    pub height: f64,
    /// The slides in presentation order; a slide is a page.
    pub slides: Vec<Slide>,
}

/// One slide: its page number, whether it is hidden, the shapes it inherits
/// from its master page, its own shapes in document order, its transition, its
/// animations, its speaker notes and its comments.
#[derive(Debug, Clone)]
pub struct Slide {
    /// 1-based, the page number a reader gives the slide.
    pub number: i32,
    /// `draw:page/@show="false"` — Impress' hidden-slide flag. The slide is
    /// kept and handed to the `invisible` layer.
    pub hidden: bool,
    /// The master page's regular (non-placeholder) shapes, inherited by this
    /// slide in master-page order. Empty when the slide hides background
    /// objects or its master page draws none.
    pub background: Vec<Shape>,
    pub shapes: Vec<Shape>,
    /// The slide's transition, when it states one. A slide transition changes
    /// the whole page, so it is kept apart from the shape effects.
    pub transition: Option<Transition>,
    /// The slide's animation effects, in the timing tree's document order.
    pub animations: Vec<Animation>,
    /// The text of the slide's `presentation:notes` frame, paragraphs joined by
    /// a newline and trimmed. `None` when the slide has no notes, or they are
    /// empty.
    pub notes: Option<String>,
    /// The slide's `officeooo:annotation` comments, in file order.
    pub comments: Vec<Comment>,
}

/// One comment on a slide.
#[derive(Debug, Clone)]
pub struct Comment {
    /// 1-based within the slide, the number a reader gives the comment.
    pub index: i32,
    /// `dc:creator`, when the file names one.
    pub author: Option<String>,
    /// `dc:date`, when the file states one.
    pub date: Option<String>,
    /// The comment's paragraphs, joined by a newline and trimmed.
    pub text: String,
}

/// One animation effect on a slide.
///
/// ODF states an effect as an `anim:` node — a `set`, `animate`,
/// `transitionFilter`, `audio` and so on — whose `smil:targetElement` names the
/// `draw:id` of the shape it changes. The shape's `draw:name` is resolved here,
/// so a reader meets the name the producer gave the shape rather than the
/// package's generated id.
#[derive(Debug, Clone)]
pub struct Animation {
    /// The targeted shape's `draw:name`, or the raw target id when the shape
    /// has no name.
    pub target: Option<String>,
    /// The effect node's ODF name: `set`, `animate`, `animateMotion`,
    /// `animateColor`, `animateTransform`, `transitionFilter`, `audio` or
    /// `command`.
    pub effect: String,
    /// `smil:attributeName`, the property the effect changes.
    pub attribute: Option<String>,
    /// `smil:from`, when stated.
    pub from: Option<String>,
    /// `smil:to`, when stated.
    pub to: Option<String>,
    /// `presentation:preset-class` (`entrance`, `emphasis`, `exit`, `path`).
    pub preset_class: Option<String>,
    /// `presentation:preset-id`, the effect template's name.
    pub preset_id: Option<String>,
    /// `smil:begin` of the effect's timing container (or of the effect
    /// itself when it is later), the delay a reader meets.
    pub begin: Option<String>,
    /// `smil:dur`, when stated.
    pub duration: Option<String>,
}

/// One slide transition.
///
/// A transition is a whole-page `anim:transitionFilter` whose timing begins at
/// `<page draw:id>.begin`, or the older drawing-page style properties that
/// state the same effect. `smil:type` and `smil:subtype` name the SMIL
/// transition (for example `slideWipe` / `fromBottom`); the style also states
/// the producer's speed when it has no explicit duration.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// `smil:type` (or the legacy `presentation:transition-type`).
    pub effect: Option<String>,
    /// `smil:subtype` (or the legacy `presentation:transition-style`).
    pub subtype: Option<String>,
    /// `smil:direction` (or the legacy
    /// `presentation:transition-direction`).
    pub direction: Option<String>,
    /// `smil:dur`, the transition's duration.
    pub duration: Option<String>,
    /// `presentation:transition-speed`, the legacy `slow`/`medium`/`fast`
    /// setting.
    pub speed: Option<String>,
}

/// One shape of a slide.
#[derive(Debug, Clone)]
pub enum Shape {
    Text(TextShape),
    Table(TableShape),
    Picture(PictureShape),
    Chart(pdfextractor_chart::ChartShape),
}

/// A text box, title, outline or custom shape with paragraphs.
#[derive(Debug, Clone)]
pub struct TextShape {
    /// `[l, t, r, b]` in EMU from the slide's top-left.
    pub bbox: [f64; 4],
    /// Whether the shape is a title placeholder.
    pub title: bool,
    pub paragraphs: Vec<Paragraph>,
}

/// One paragraph of a text shape.
#[derive(Debug, Clone)]
pub struct Paragraph {
    pub text: String,
    /// `Some(level)` for a list item, `None` for body text.
    pub level: Option<i32>,
    /// Whether the paragraph came from a `text:list-item`.
    pub list: bool,
}

/// A table on a slide.
#[derive(Debug, Clone)]
pub struct TableShape {
    pub bbox: [f64; 4],
    pub table: Table,
}

/// A table's grid and its cells that are not covered by a merge.
#[derive(Debug, Clone)]
pub struct Table {
    pub num_rows: usize,
    pub num_cols: usize,
    pub cells: Vec<TableCell>,
}

/// One cell of a slide table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub text: String,
}

/// A `draw:image` and the media part its `xlink:href` names.
#[derive(Debug, Clone)]
pub struct PictureShape {
    pub bbox: [f64; 4],
    pub mime: &'static str,
    pub data: Vec<u8>,
}

impl OdpDocument {
    pub fn parse_file(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::parse_bytes(&data)
    }

    pub fn parse_bytes(data: &[u8]) -> Result<Self> {
        Self::parse_bytes_with_limits(data, limits::Limits::default())
    }

    /// Read an ODP under an explicit input, ZIP, XML and page budget.
    pub fn parse_bytes_with_limits(data: &[u8], budget: limits::Limits) -> Result<Self> {
        budget.check_input(data.len())?;
        let guard = budget.guard();
        guard.check("reading a package")?;
        let mut archive = limits::Package::new(data, &budget)
            .map_err(|e| anyhow::anyhow!("not an OpenDocument (ZIP) package: {e}"))?;
        budget.check_zip(archive.archive_mut())?;

        let content = read_part(&mut archive, "content.xml")
            .ok_or_else(|| anyhow::anyhow!("package has no content.xml"))?;
        let styles = read_part(&mut archive, "styles.xml");
        let layouts = styles
            .as_deref()
            .map(parse_page_layouts)
            .transpose()?
            .unwrap_or_default();
        let media = read_parts(&mut archive, "Pictures/");
        let objects = read_object_parts(&mut archive);
        let backgrounds = styles
            .as_deref()
            .map(|xml| parse_master_backgrounds(xml, &media, &objects))
            .transpose()?
            .unwrap_or_default();

        let root = Node::parse(&content).context("malformed content.xml")?;
        let presentation = root
            .find("presentation")
            .context("content.xml has no office:presentation")?;
        let page_visibility = page_background_visibility(&root);
        let page_transitions = page_transitions(&root);
        budget.check_pages(presentation.children_named("page").count())?;

        let mut slides = Vec::new();
        let mut page_size = None;
        for (index, page) in presentation.children_named("page").enumerate() {
            guard.check("reading an ODP page")?;
            let size = page_size_for(page, &layouts);
            page_size.get_or_insert(size);

            let mut shapes = Vec::new();
            for child in &page.children {
                match child.name.as_str() {
                    "frame" | "custom-shape" | "caption" | "g" => {
                        collect_shape(child, &media, &objects, &mut shapes)
                    }
                    "table" => shapes.push(Shape::Table(TableShape {
                        bbox: bbox(page),
                        table: parse_table(child),
                    })),
                    "image" => {
                        if let Some(picture) = picture(child, bbox(page), &media) {
                            shapes.push(Shape::Picture(picture));
                        }
                    }
                    _ => {}
                }
            }

            let background = if background_objects_visible(page, &page_visibility) {
                page.attr("master-page-name")
                    .and_then(|name| backgrounds.get(name))
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let names = shape_names(page);
            let (animations, timing_transition) = read_animations(page, &names);
            let style_transition = page
                .attr("style-name")
                .and_then(|name| page_transitions.get(name))
                .cloned();
            let transition = merge_transition(timing_transition, style_transition);

            slides.push(Slide {
                number: (index + 1) as i32,
                hidden: is_hidden(page),
                background,
                shapes,
                transition,
                animations,
                notes: read_notes(page),
                comments: read_comments(page),
            });
        }

        let (width, height) = page_size.unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT));
        Ok(Self {
            width,
            height,
            slides,
        })
    }
}

fn read_part(archive: &mut limits::Package<'_>, name: &str) -> Option<Vec<u8>> {
    archive.read_name(name)
}

fn read_parts(archive: &mut limits::Package<'_>, prefix: &str) -> HashMap<String, Vec<u8>> {
    let mut parts = HashMap::new();
    for index in 0..archive.len() {
        let (name, is_dir) = match archive.by_index(index) {
            Ok(file) => (file.name().to_string(), file.is_dir()),
            Err(_) => continue,
        };
        if is_dir || !name.starts_with(prefix) {
            continue;
        }
        if let Some(buffer) = archive.read_name(&name) {
            parts.insert(name, buffer);
        }
    }
    parts
}

/// The embedded objects a package holds, keyed by the directory a
/// `draw:object` names (`Object 1`). Each is the object's `content.xml`.
fn read_object_parts(archive: &mut limits::Package<'_>) -> HashMap<String, Vec<u8>> {
    let mut objects = HashMap::new();
    for index in 0..archive.len() {
        let name = match archive.by_index(index) {
            Ok(file) => file.name().to_string(),
            Err(_) => continue,
        };
        let Some(object) = name.strip_suffix("/content.xml") else {
            continue;
        };
        let object = object.to_string();
        if !object.starts_with("Object ") {
            continue;
        }
        if let Some(buffer) = archive.read_name(&name) {
            objects.insert(object, buffer);
        }
    }
    objects
}

/// The page size a slide's master page names, in EMU.
fn page_size_for(page: &Node, layouts: &HashMap<String, (f64, f64)>) -> (f64, f64) {
    let layout = page
        .attr("master-page-name")
        .and_then(|name| layouts.get(name))
        .or_else(|| {
            page.attr("page-layout-name")
                .and_then(|name| layouts.get(name))
        })
        .or_else(|| layouts.get("__default__"));
    layout.copied().unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT))
}

/// Read `style:page-layout` rectangles and `style:master-page` references from
/// `styles.xml`.
fn parse_page_layouts(xml: &[u8]) -> Result<HashMap<String, (f64, f64)>> {
    let root = Node::parse(xml).context("malformed styles.xml")?;
    let mut layouts = HashMap::new();
    let mut first = None;
    for layout in root.descendants_named("page-layout") {
        let Some(name) = layout.attr("name") else {
            continue;
        };
        let Some(properties) = layout.child("page-layout-properties") else {
            continue;
        };
        let Some(width) = properties.attr("page-width").and_then(length_to_emu) else {
            continue;
        };
        let Some(height) = properties.attr("page-height").and_then(length_to_emu) else {
            continue;
        };
        if width > 0.0 && height > 0.0 {
            first.get_or_insert((width, height));
            layouts.insert(name.to_string(), (width, height));
        }
    }

    for master in root.descendants_named("master-page") {
        let (Some(name), Some(layout)) = (master.attr("name"), master.attr("page-layout-name"))
        else {
            continue;
        };
        if let Some(size) = layouts.get(layout).copied() {
            layouts.insert(name.to_string(), size);
        }
    }

    // A page that names only a `draw:style-name` has no master reference to
    // follow. The first real page layout is the best statement left.
    if let Some(size) = first {
        layouts.insert("__default__".to_string(), size);
    }
    Ok(layouts)
}

/// Read each `style:master-page`'s regular shapes from `styles.xml`.
///
/// A master page's background objects are frames on the `backgroundobjects`
/// layer whose producer gave them no `presentation:class`: the classes name
/// *placeholders* (`title`, `outline`, `date-time`, `footer`, `page-number`,
/// `notes`), which are slots a slide fills rather than content of their own.
/// Only shapes without a class are inherited, so a slide never repeats the
/// master's "Click to edit Master title style" prompt.
fn parse_master_backgrounds(
    xml: &[u8],
    media: &HashMap<String, Vec<u8>>,
    objects: &HashMap<String, Vec<u8>>,
) -> Result<HashMap<String, Vec<Shape>>> {
    let root = Node::parse(xml).context("malformed styles.xml")?;
    let mut backgrounds = HashMap::new();
    for master in root.descendants_named("master-page") {
        let Some(name) = master.attr("name") else {
            continue;
        };
        let mut shapes = Vec::new();
        for child in &master.children {
            if matches!(
                child.name.as_str(),
                "frame" | "custom-shape" | "caption" | "g"
            ) && child.attr("class").is_none()
            {
                collect_shape(child, media, objects, &mut shapes);
            }
        }
        if !shapes.is_empty() {
            backgrounds.insert(name.to_string(), shapes);
        }
    }
    Ok(backgrounds)
}

/// The `presentation:background-objects-visible` each drawing-page style in
/// `content.xml` states.
///
/// ODF allows the flag on the `draw:page` itself or on the drawing-page style
/// its `draw:style-name` names; LibreOffice writes it on the style.
fn page_background_visibility(root: &Node) -> HashMap<String, bool> {
    let mut visibility = HashMap::new();
    let Some(styles) = root.find("automatic-styles") else {
        return visibility;
    };
    for style in styles.descendants_named("style") {
        let (Some(name), Some(properties)) =
            (style.attr("name"), style.child("drawing-page-properties"))
        else {
            continue;
        };
        if style.attr("family") != Some("drawing-page") {
            continue;
        }
        if let Some(visible) = properties.attr("background-objects-visible") {
            visibility.insert(
                name.to_string(),
                !(visible.eq_ignore_ascii_case("false") || visible == "0"),
            );
        }
    }
    visibility
}

/// Whether a page inherits its master page's background objects.
///
/// The page's own `presentation:background-objects-visible` wins; otherwise
/// the drawing-page style it names states the flag; a page that states nothing
/// inherits them.
/// The transition each drawing-page style in `content.xml` states.
///
/// A producer that writes SMIL timing also mirrors the transition on the style
/// (`smil:type`, `smil:subtype`, `smil:dur`, `presentation:transition-speed`)
/// for readers without SMIL support. Older producers write only these style
/// properties, so reading them is what makes a transition visible in both.
fn page_transitions(root: &Node) -> HashMap<String, Transition> {
    let mut transitions = HashMap::new();
    let Some(styles) = root.find("automatic-styles") else {
        return transitions;
    };
    for style in styles.descendants_named("style") {
        if style.attr("family") != Some("drawing-page") {
            continue;
        }
        let (Some(name), Some(properties)) =
            (style.attr("name"), style.child("drawing-page-properties"))
        else {
            continue;
        };
        if let Some(transition) = transition_from_properties(properties) {
            transitions.insert(name.to_string(), transition);
        }
    }
    transitions
}

fn transition_from_properties(node: &Node) -> Option<Transition> {
    let effect = node.attr("type").or_else(|| node.attr("transition-type"))?;
    if effect.eq_ignore_ascii_case("none") {
        return None;
    }
    Some(Transition {
        effect: Some(effect.to_string()),
        subtype: node
            .attr("subtype")
            .or_else(|| node.attr("transition-style"))
            .map(str::to_string),
        direction: node
            .attr("direction")
            .or_else(|| node.attr("transition-direction"))
            .map(str::to_string),
        duration: node.attr("dur").map(str::to_string),
        speed: node.attr("transition-speed").map(str::to_string),
    })
}

/// The SMIL timing is the authoritative transition; the mirrored style fills in
/// the speed the timing does not state.
fn merge_transition(timing: Option<Transition>, style: Option<Transition>) -> Option<Transition> {
    match (timing, style) {
        (None, style) => style,
        (timing, None) => timing,
        (Some(timing), Some(style)) => Some(Transition {
            effect: timing.effect.or(style.effect),
            subtype: timing.subtype.or(style.subtype),
            direction: timing.direction.or(style.direction),
            duration: timing.duration.or(style.duration),
            speed: timing.speed.or(style.speed),
        }),
    }
}

fn background_objects_visible(page: &Node, styles: &HashMap<String, bool>) -> bool {
    let stated = |visible: &str| !(visible.eq_ignore_ascii_case("false") || visible == "0");
    if let Some(visible) = page.attr("background-objects-visible") {
        return stated(visible);
    }
    page.attr("style-name")
        .and_then(|name| styles.get(name))
        .copied()
        .unwrap_or(true)
}

fn length_to_emu(value: &str) -> Option<f64> {
    let value = value.trim();
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.trim().parse().ok()?;
    let points = match unit.trim().to_ascii_lowercase().as_str() {
        "cm" => number * 28.346_456_692_913_4,
        "mm" => number * 2.834_645_669_291_34,
        "in" => number * 72.0,
        "pc" => number * 12.0,
        "px" => number * 0.75,
        "" | "pt" => number,
        _ => return None,
    };
    // An EMU is the smallest unit the model records, so a length in cm is
    // rounded to it rather than carried as a repeating fraction of one.
    Some((points * EMU_PER_POINT).round())
}

fn bbox(node: &Node) -> [f64; 4] {
    let value = |name: &str| node.attr(name).and_then(length_to_emu).unwrap_or(0.0);
    let x = value("x");
    let y = value("y");
    [x, y, x + value("width"), y + value("height")]
}

fn is_hidden(page: &Node) -> bool {
    page.attr("show")
        .is_some_and(|show| show.eq_ignore_ascii_case("false") || show == "0")
}

fn collect_shape(
    node: &Node,
    media: &HashMap<String, Vec<u8>>,
    objects: &HashMap<String, Vec<u8>>,
    shapes: &mut Vec<Shape>,
) {
    let bbox = bbox(node);
    let title = matches!(node.attr("class"), Some("title" | "ctrTitle"));

    // An embedded chart is the object, not the preview image beside it: the
    // preview is a rendering of the same chart, so reading both would report
    // the chart twice.
    let chart = node
        .child("object")
        .and_then(|object| object.attr("href"))
        .map(|href| href.trim_start_matches("./"))
        .and_then(|name| objects.get(name))
        .and_then(|bytes| chart::read_chart(bytes, bbox));
    let has_chart = chart.is_some();
    if let Some(chart) = chart {
        shapes.push(Shape::Chart(chart));
    }

    let mut paragraphs = Vec::new();
    if let Some(text_box) = node.child("text-box") {
        collect_paragraphs(text_box, &mut paragraphs);
    } else {
        collect_paragraphs(node, &mut paragraphs);
    }
    if !paragraphs.is_empty() {
        shapes.push(Shape::Text(TextShape {
            bbox,
            title,
            paragraphs,
        }));
    }

    for child in &node.children {
        match child.name.as_str() {
            "table" => shapes.push(Shape::Table(TableShape {
                bbox,
                table: parse_table(child),
            })),
            "image" => {
                if !has_chart {
                    if let Some(picture) = picture(child, bbox, media) {
                        shapes.push(Shape::Picture(picture));
                    }
                }
            }
            "object" => {}
            "frame" | "custom-shape" | "caption" => collect_shape(child, media, objects, shapes),
            _ => {}
        }
    }
}

fn collect_paragraphs(node: &Node, out: &mut Vec<Paragraph>) {
    for child in &node.children {
        match child.name.as_str() {
            "p" | "h" => {
                let text = paragraph_text(child).trim().to_string();
                if !text.is_empty() {
                    out.push(Paragraph {
                        text,
                        level: None,
                        list: false,
                    });
                }
            }
            "list" => collect_list(child, 1, out),
            "text-box" => collect_paragraphs(child, out),
            _ => {}
        }
    }
}

fn collect_list(node: &Node, level: i32, out: &mut Vec<Paragraph>) {
    for item in node.children_named("list-item") {
        for child in &item.children {
            match child.name.as_str() {
                "p" | "h" => {
                    let text = paragraph_text(child).trim().to_string();
                    if !text.is_empty() {
                        out.push(Paragraph {
                            text,
                            level: Some(level),
                            list: true,
                        });
                    }
                }
                "list" => collect_list(child, level + 1, out),
                _ => {}
            }
        }
    }
}

fn paragraph_text(node: &Node) -> String {
    let mut text = String::new();
    inline_text(node, &mut text);
    text
}

fn inline_text(node: &Node, out: &mut String) {
    // Text and child elements are read in the order they occurred: a span in
    // the middle of a sentence must not move its words to one end.
    for content in &node.content {
        match content {
            Content::Text(text) => out.push_str(text),
            Content::Child(index) => {
                let Some(child) = node.children.get(*index) else {
                    continue;
                };
                match child.name.as_str() {
                    "s" => out.push_str(&" ".repeat(count(child, "c"))),
                    "tab" => out.push('\t'),
                    "line-break" => out.push('\n'),
                    "image" | "frame" | "note" | "annotation" => {}
                    _ => inline_text(child, out),
                }
            }
        }
    }
}

/// The slide's comments: `officeooo:annotation` children of its page.
fn read_comments(page: &Node) -> Vec<Comment> {
    page.children_named("annotation")
        .enumerate()
        .filter_map(|(position, annotation)| {
            let mut paragraphs = Vec::new();
            collect_paragraphs(annotation, &mut paragraphs);
            let text = paragraphs
                .iter()
                .map(|paragraph| paragraph.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            Some(Comment {
                index: (position + 1) as i32,
                author: child_text(annotation, "creator"),
                date: child_text(annotation, "date"),
                text: text.to_string(),
            })
        })
        .collect()
}

fn child_text(node: &Node, name: &str) -> Option<String> {
    let text = paragraph_text(node.child(name)?).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The `draw:id` a shape is targeted by, mapped to the name a reader knows it
/// by. Animations refer to the id; the name is the producer's own label.
fn shape_names(page: &Node) -> HashMap<String, String> {
    fn collect(node: &Node, names: &mut HashMap<String, String>) {
        for child in &node.children {
            if let (Some(id), Some(name)) = (child.attr("id"), child.attr("name")) {
                names.insert(id.to_string(), name.to_string());
            }
            collect(child, names);
        }
    }
    let mut names = HashMap::new();
    collect(page, &mut names);
    names
}

/// The effect nodes ODF defines. A timing container is walked through; an
/// effect is the leaf a reader meets.
const EFFECTS: [&str; 8] = [
    "set",
    "animate",
    "animateMotion",
    "animateColor",
    "animateTransform",
    "transitionFilter",
    "audio",
    "command",
];

fn is_animation_node(name: &str) -> bool {
    EFFECTS.contains(&name) || matches!(name, "par" | "seq" | "excl" | "iter")
}

/// Read a page's animation effects in document order.
/// The page-wide facts every effect in a timing tree is read against.
struct AnimationContext<'a> {
    page_id: Option<&'a str>,
    transition_begin: Option<&'a str>,
    names: &'a HashMap<String, String>,
}

fn read_animations(
    page: &Node,
    names: &HashMap<String, String>,
) -> (Vec<Animation>, Option<Transition>) {
    let page_id = page.attr("id");
    let transition_begin = page_id.map(|id| format!("{id}.begin"));
    let context = AnimationContext {
        page_id,
        transition_begin: transition_begin.as_deref(),
        names,
    };
    let mut animations = Vec::new();
    let mut transition = None;
    for child in &page.children {
        if is_animation_node(&child.name) {
            walk_animations(
                child,
                None,
                None,
                &context,
                &mut animations,
                &mut transition,
            );
        }
    }
    (animations, transition)
}

/// Walk a timing tree, carrying the timing a parent container gives its
/// effects: an effect's own `smil:begin` is relative to its container, so a
/// non-zero container begin is the delay a reader sees.
fn walk_animations<'a>(
    node: &'a Node,
    begin: Option<&'a str>,
    duration: Option<&'a str>,
    context: &AnimationContext<'a>,
    out: &mut Vec<Animation>,
    transition: &mut Option<Transition>,
) {
    let own_begin = node.attr("begin").filter(|value| !is_zero_time(value));
    let effective_begin = own_begin.or(begin).or_else(|| node.attr("begin"));
    let effective_duration = node.attr("dur").or(duration);

    if EFFECTS.contains(&node.name.as_str()) {
        // A transition filter that targets the page (or begins when the page
        // begins) changes the slide as a whole; a shape's transitionFilter
        // names the shape it belongs to and stays an animation.
        if node.name == "transitionFilter"
            && is_page_transition(
                node,
                effective_begin,
                context.page_id,
                context.transition_begin,
            )
        {
            if let Some(effect) = node.attr("type") {
                if transition.is_none() {
                    *transition = Some(Transition {
                        effect: Some(effect.to_string()),
                        subtype: node.attr("subtype").map(str::to_string),
                        direction: node.attr("direction").map(str::to_string),
                        duration: effective_duration.map(str::to_string),
                        speed: None,
                    });
                }
            }
            return;
        }
        out.push(Animation {
            target: node.attr("targetElement").map(|id| {
                context
                    .names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| id.to_string())
            }),
            effect: node.name.clone(),
            attribute: node.attr("attributeName").map(str::to_string),
            from: node.attr("from").map(str::to_string),
            to: node.attr("to").map(str::to_string),
            preset_class: node.attr("preset-class").map(str::to_string),
            preset_id: node.attr("preset-id").map(str::to_string),
            begin: effective_begin.map(str::to_string),
            duration: effective_duration.map(str::to_string),
        });
        return;
    }

    for child in &node.children {
        walk_animations(
            child,
            effective_begin,
            effective_duration,
            context,
            out,
            transition,
        );
    }
}

/// Whether a transition filter changes the page rather than one of its shapes.
/// LibreOffice writes `smil:begin="<page-id>.begin"` without a target; other
/// producers may name the page in `smil:targetElement` instead.
fn is_page_transition(
    node: &Node,
    begin: Option<&str>,
    page_id: Option<&str>,
    transition_begin: Option<&str>,
) -> bool {
    match node.attr("targetElement") {
        Some(target) => Some(target) == page_id,
        None => begin.is_some() && begin == transition_begin,
    }
}

fn is_zero_time(value: &str) -> bool {
    matches!(value.trim(), "0" | "0s" | "0.0s" | "0ms")
}

fn read_notes(page: &Node) -> Option<String> {
    let notes = page.child("notes")?;
    let mut paragraphs = Vec::new();
    for frame in notes.children_named("frame") {
        if frame.attr("class") != Some("notes") {
            continue;
        }
        collect_paragraphs(frame, &mut paragraphs);
    }
    let text = paragraphs
        .iter()
        .map(|paragraph| paragraph.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn picture(node: &Node, bbox: [f64; 4], media: &HashMap<String, Vec<u8>>) -> Option<PictureShape> {
    let href = node.attr("href")?.trim_start_matches("./");
    let data = media.get(href)?.clone();
    let mime = node
        .attr("mime-type")
        .and_then(format_for_mime)
        .map(|format| format.mime)
        .or_else(|| pdfextractor_image_parser::mimetype(&data))
        .or_else(|| {
            std::path::Path::new(href)
                .extension()
                .and_then(|extension| extension.to_str())
                .and_then(format_for_extension)
                .map(|format| format.mime)
        })?;
    Some(PictureShape { bbox, mime, data })
}

fn parse_table(node: &Node) -> Table {
    let mut rows = Vec::new();
    let mut declared_cols = 0usize;
    collect_rows(node, &mut rows, &mut declared_cols);

    let placed = rows
        .iter()
        .flatten()
        .map(|cell| cell.col + cell.col_span.max(1))
        .max()
        .unwrap_or(0);

    Table {
        num_rows: rows.len(),
        num_cols: declared_cols.max(placed),
        cells: rows.into_iter().flatten().collect(),
    }
}

fn collect_rows(node: &Node, rows: &mut Vec<Vec<TableCell>>, declared_cols: &mut usize) {
    for child in &node.children {
        match child.name.as_str() {
            "table-header-rows" | "table-rows" => collect_rows(child, rows, declared_cols),
            "table-row" => {
                let repeat = count(child, "number-rows-repeated").min(MAX_REPEAT);
                for _ in 0..repeat {
                    // Each copy is a row of its own, so its cells carry the
                    // index a reader gives that row.
                    rows.push(parse_row(child, rows.len()));
                }
            }
            "table-column" => {
                *declared_cols += count(child, "number-columns-repeated").min(MAX_REPEAT);
            }
            _ => {}
        }
    }
}

fn parse_row(node: &Node, row: usize) -> Vec<TableCell> {
    let mut cells = Vec::new();
    let mut col = 0usize;
    let mut covered_by_span = 0usize;
    for child in &node.children {
        match child.name.as_str() {
            "table-cell" => {
                let col_span = count(child, "number-columns-spanned");
                let row_span = count(child, "number-rows-spanned");
                let repeat = count(child, "number-columns-repeated").min(MAX_REPEAT);
                let text = cell_text(child);
                for _ in 0..repeat {
                    cells.push(TableCell {
                        row,
                        col,
                        row_span,
                        col_span,
                        text: text.clone(),
                    });
                    col += col_span;
                    covered_by_span += col_span.saturating_sub(1);
                }
            }
            "covered-table-cell" => {
                let repeat = count(child, "number-columns-repeated").min(MAX_REPEAT);
                for _ in 0..repeat {
                    if covered_by_span > 0 {
                        covered_by_span -= 1;
                    } else {
                        col += 1;
                    }
                }
            }
            _ => {}
        }
    }
    cells
}

fn cell_text(node: &Node) -> String {
    let mut paragraphs = Vec::new();
    collect_paragraphs(node, &mut paragraphs);
    paragraphs
        .iter()
        .map(|paragraph| paragraph.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A count an element states, at least one.
fn count(node: &Node, key: &str) -> usize {
    node.attr(key)
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(1)
        .max(1)
}

// --- XML tree -------------------------------------------------------------

#[derive(Debug, Clone)]
enum Content {
    Text(String),
    Child(usize),
}

#[derive(Debug, Clone, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    /// Text and child positions in document order. A child's index is its
    /// position in `children`; the child is filled in when its end tag is read.
    content: Vec<Content>,
}

impl Node {
    fn parse(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(Cursor::new(xml));
        reader.config_mut().trim_text(false);
        let mut stack = vec![Node {
            name: "#document".to_string(),
            ..Node::default()
        }];
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(start)) => {
                    let index = stack.last().expect("a root").children.len();
                    stack
                        .last_mut()
                        .expect("a root")
                        .content
                        .push(Content::Child(index));
                    stack.push(Node::from_start(&start));
                }
                Ok(Event::Empty(start)) => {
                    let node = Node::from_start(&start);
                    let parent = stack.last_mut().expect("a root");
                    let index = parent.children.len();
                    parent.children.push(node);
                    parent.content.push(Content::Child(index));
                }
                Ok(Event::End(_)) => {
                    if stack.len() > 1 {
                        let node = stack.pop().expect("a node");
                        stack.last_mut().expect("a parent").children.push(node);
                    }
                }
                Ok(Event::Text(text)) => {
                    let text = text
                        .unescape()
                        .context("malformed text in an OpenDocument part")?;
                    stack
                        .last_mut()
                        .expect("a root")
                        .content
                        .push(Content::Text(text.into_owned()));
                }
                Ok(Event::CData(text)) => {
                    stack
                        .last_mut()
                        .expect("a root")
                        .content
                        .push(Content::Text(String::from_utf8_lossy(&text).into_owned()));
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(error) => anyhow::bail!("malformed OpenDocument XML: {error}"),
            }
            buffer.clear();
        }
        Ok(stack.pop().expect("a root"))
    }

    fn from_start(start: &BytesStart<'_>) -> Self {
        Self {
            name: local(start),
            attrs: attributes(start),
            ..Self::default()
        }
    }

    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |child| child.name == name)
    }

    fn find(&self, name: &str) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(name))
    }

    fn descendants_named<'a>(&'a self, name: &'a str) -> Vec<&'a Node> {
        let mut found = Vec::new();
        self.collect_descendants(name, &mut found);
        found
    }

    fn collect_descendants<'a>(&'a self, name: &str, found: &mut Vec<&'a Node>) {
        for child in &self.children {
            if child.name == name {
                found.push(child);
            }
            child.collect_descendants(name, found);
        }
    }
}

fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|byte| *byte == b':') {
        Some(index) => &name[index + 1..],
        None => name,
    }
}

fn local(element: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(local_name(element.name().as_ref())).into_owned()
}

fn attributes(element: &BytesStart<'_>) -> Vec<(String, String)> {
    element
        .attributes()
        .flatten()
        .filter_map(|attribute| {
            attribute.unescape_value().ok().map(|value| {
                (
                    String::from_utf8_lossy(local_name(attribute.key.as_ref())).into_owned(),
                    value.into_owned(),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            for (name, data) in parts {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    const CONTENT: &str = r#"<document-content><body><presentation>
        <page name="One" show="false" master-page-name="Master">
          <frame class="title" x="1cm" y="1cm" width="5cm" height="1cm">
            <text-box><p>Deck <span>title</span>!</p></text-box>
          </frame>
          <frame x="1cm" y="2cm" width="5cm" height="2cm">
            <text-box><list>
              <list-item><p>First</p><list><list-item><p>Nested</p></list-item></list></list-item>
            </list></text-box>
          </frame>
          <frame x="1cm" y="4cm" width="5cm" height="2cm">
            <table><table-row number-rows-repeated="2"><table-cell><p>A</p></table-cell><table-cell><p>B</p></table-cell></table-row></table>
          </frame>
          <frame x="1cm" y="6cm" width="2cm" height="1cm">
            <image href="Pictures/p.png" mime-type="image/png"/>
          </frame>
          <annotation>
            <creator>Ada Lovelace</creator>
            <date>2026-10-18T09:15:00</date>
            <p>Check the numbers.</p>
          </annotation>
          <frame x="1cm" y="7cm" width="4cm" height="2cm">
            <object href="./Object 1"/>
            <image href="./ObjectReplacements/Object 1"/>
          </frame>
          <notes>
            <frame class="notes"><text-box><p>Speak first.</p><p>Then pause.</p></text-box></frame>
            <p>A comment, not a speaker note.</p>
          </notes>
        </page>
      </presentation></body></document-content>"#;

    const STYLES: &str = r#"<document-styles>
        <automatic-styles><page-layout name="PL"><page-layout-properties page-width="20cm" page-height="10cm"/></page-layout></automatic-styles>
        <master-styles><master-page name="Master" page-layout-name="PL"/></master-styles>
      </document-styles>"#;

    const CHART: &str = r#"<document-content><body><chart>
        <chart class="chart:bar">
          <title><text:p>Units by region</text:p></title>
          <plot-area>
            <axis dimension="x"><categories cell-range-address="local-table.$A$2:.$A$4"/></axis>
            <series values-cell-range-address="local-table.$B$2:.$B$4" label-cell-address="local-table.$B$1"/>
            <series values-cell-range-address="local-table.$C$2:.$C$4" label-cell-address="local-table.$C$1"/>
          </plot-area>
        </chart>
        <table name="local-table">
          <table-row><table-cell/><table-cell><p>Q2</p></table-cell><table-cell><p>Q3</p></table-cell></table-row>
          <table-row><table-cell><p>North America</p></table-cell><table-cell><p>120</p></table-cell><table-cell><p>145</p></table-cell></table-row>
          <table-row><table-cell><p>Europe</p></table-cell><table-cell><p>98</p></table-cell><table-cell><p>121</p></table-cell></table-row>
          <table-row><table-cell><p>Asia Pacific</p></table-cell><table-cell><p>74</p></table-cell><table-cell><p>NaN</p></table-cell></table-row>
        </table>
      </chart></body></document-content>"#;

    /// A spent deadline refuses the read by name rather than walking the part.
    #[test]
    fn a_spent_deadline_is_enforced() {
        let pkg = package(&[("content.xml", CONTENT.as_bytes())]);
        let budget = pdfextractor_limits::Limits {
            timeout: std::time::Duration::ZERO,
            ..pdfextractor_limits::Limits::default()
        };
        let error =
            OdpDocument::parse_bytes_with_limits(&pkg, budget).expect_err("the deadline is spent");
        assert!(error.to_string().contains("timed out"), "{error}");
    }

    #[test]
    fn a_custom_zip_budget_is_enforced() {
        let pkg = package(&[("content.xml", CONTENT.as_bytes())]);
        let budget = pdfextractor_limits::Limits {
            max_zip_entries: 0,
            ..pdfextractor_limits::Limits::default()
        };
        let error = OdpDocument::parse_bytes_with_limits(&pkg, budget)
            .expect_err("the package has one entry");
        assert!(error.to_string().contains("entries"), "{error}");
    }

    #[test]
    fn a_custom_page_budget_is_enforced() {
        let content = r#"<document-content><body><presentation>
            <page name="One"/><page name="Two"/>
          </presentation></body></document-content>"#;
        let pkg = package(&[("content.xml", content.as_bytes())]);
        let budget = pdfextractor_limits::Limits {
            max_pages: 1,
            ..pdfextractor_limits::Limits::default()
        };
        let error =
            OdpDocument::parse_bytes_with_limits(&pkg, budget).expect_err("the deck has two pages");
        assert!(error.to_string().contains("2 pages"), "{error}");
        assert!(error.to_string().contains("limit is 1"), "{error}");
    }

    #[test]
    fn a_slide_reads_text_lists_tables_pictures_and_notes() {
        let document = OdpDocument::parse_bytes(&package(&[
            ("content.xml", CONTENT.as_bytes()),
            ("styles.xml", STYLES.as_bytes()),
            ("Pictures/p.png", b"not decoded by the parser"),
            ("Object 1/content.xml", CHART.as_bytes()),
        ]))
        .expect("the slide package parses");

        assert_eq!(document.width, 7_200_000.0);
        assert_eq!(document.height, 3_600_000.0);
        assert_eq!(document.slides.len(), 1);
        let slide = &document.slides[0];
        assert_eq!(slide.number, 1);
        assert!(slide.hidden);
        assert_eq!(
            slide.notes.as_deref(),
            Some("Speak first.\nThen pause."),
            "only the notes frame is a speaker note"
        );

        let text: Vec<_> = slide
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(text.len(), 2);
        assert!(text[0].title);
        assert_eq!(
            text[0].paragraphs[0].text, "Deck title!",
            "a span in the middle of a sentence stays there"
        );
        assert_eq!(text[0].bbox, [360_000.0, 360_000.0, 2_160_000.0, 720_000.0]);
        assert_eq!(text[1].paragraphs[0].level, Some(1));
        assert!(text[1].paragraphs[0].list);
        assert_eq!(text[1].paragraphs[1].text, "Nested");
        assert_eq!(text[1].paragraphs[1].level, Some(2));

        let table = slide
            .shapes
            .iter()
            .find_map(|shape| match shape {
                Shape::Table(table) => Some(table),
                _ => None,
            })
            .expect("the table");
        assert_eq!(table.table.num_rows, 2);
        assert_eq!(table.table.num_cols, 2);
        assert_eq!(table.table.cells[1].text, "B");
        assert_eq!((table.table.cells[2].row, table.table.cells[2].col), (1, 0));
        assert_eq!(table.table.cells[2].text, "A");

        let picture = slide
            .shapes
            .iter()
            .find_map(|shape| match shape {
                Shape::Picture(picture) => Some(picture),
                _ => None,
            })
            .expect("the picture");
        assert_eq!(picture.mime, "image/png");
        assert_eq!(picture.data, b"not decoded by the parser");

        assert_eq!(slide.comments.len(), 1);
        let comment = &slide.comments[0];
        assert_eq!(comment.index, 1);
        assert_eq!(comment.author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(comment.date.as_deref(), Some("2026-10-18T09:15:00"));
        assert_eq!(comment.text, "Check the numbers.");

        let chart = slide
            .shapes
            .iter()
            .find_map(|shape| match shape {
                Shape::Chart(chart) => Some(chart),
                _ => None,
            })
            .expect("the embedded chart");
        assert_eq!(chart.title.as_deref(), Some("Units by region"));
        assert_eq!(chart.kind, pdfextractor_chart::ChartKind::Bar);
        assert_eq!(
            chart.categories,
            vec!["North America", "Europe", "Asia Pacific"]
        );
        assert_eq!(chart.series[0].name, "Q2");
        assert_eq!(
            chart.series[0].values,
            vec![Some(120.0), Some(98.0), Some(74.0)]
        );
        assert_eq!(chart.series[1].name, "Q3");
        assert_eq!(
            chart.series[1].values,
            vec![Some(145.0), Some(121.0), None],
            "a missing plotted point stays a gap"
        );
        assert_eq!(
            chart.bbox,
            [360_000.0, 2_520_000.0, 1_800_000.0, 3_240_000.0]
        );
    }

    const MASTER_STYLES: &str = r#"<document-styles>
        <automatic-styles><page-layout name="PL"><page-layout-properties page-width="20cm" page-height="10cm"/></page-layout></automatic-styles>
        <master-styles><master-page name="Master" page-layout-name="PL">
          <frame class="title" x="2cm" y="1cm" width="10cm" height="2cm">
            <text-box><p>Click to edit Master title style</p></text-box>
          </frame>
          <frame name="Confidential" x="2cm" y="8cm" width="10cm" height="1cm">
            <text-box><p>Confidential</p></text-box>
          </frame>
        </master-page></master-styles>
      </document-styles>"#;

    const MASTER_CONTENT: &str = r#"<document-content>
        <automatic-styles>
          <style name="hidden" family="drawing-page"><drawing-page-properties background-objects-visible="false"/></style>
          <style name="fade" family="drawing-page"><drawing-page-properties presentation:transition-type="fade" presentation:transition-speed="medium"/></style>
        </automatic-styles>
        <body><presentation>
        <page name="One" id="pageOne" master-page-name="Master">
          <frame name="Body" id="id1" x="2cm" y="3cm" width="8cm" height="2cm">
            <text-box><p>Fade this in.</p></text-box>
          </frame>
          <anim:par presentation:node-type="timing-root">
            <anim:par smil:begin="pageOne.begin"><anim:transitionFilter smil:dur="2s" smil:type="slideWipe" smil:subtype="fromBottom" smil:direction="reverse"/></anim:par>
            <anim:seq presentation:node-type="main-sequence">
            <anim:par smil:begin="0s"><anim:set smil:begin="0s" smil:dur="0.5s" smil:targetElement="id1" smil:attributeName="visibility" smil:to="visible"/></anim:par>
            <anim:par smil:begin="1s"><anim:animate smil:begin="0s" smil:dur="2s" smil:targetElement="missing" smil:attributeName="x" smil:from="0" smil:to="5" presentation:preset-class="emphasis" presentation:preset-id="ooo-emphasis-spin"/></anim:par>
          </anim:seq></anim:par>
        </page>
        <page name="Two" style-name="fade" master-page-name="Master" presentation:background-objects-visible="false">
          <frame name="Own" x="2cm" y="3cm" width="8cm" height="2cm">
            <text-box><p>Own words.</p></text-box>
          </frame>
        </page>
        <page name="Three" style-name="hidden" master-page-name="Master">
          <frame name="Styled" x="2cm" y="3cm" width="8cm" height="2cm">
            <text-box><p>Styled page.</p></text-box>
          </frame>
        </page>
      </presentation></body></document-content>"#;

    #[test]
    fn master_backgrounds_are_inherited_and_animations_name_their_shapes() {
        let document = OdpDocument::parse_bytes(&package(&[
            ("content.xml", MASTER_CONTENT.as_bytes()),
            ("styles.xml", MASTER_STYLES.as_bytes()),
        ]))
        .expect("the master-page package parses");

        let slide = &document.slides[0];
        assert_eq!(
            slide.background.len(),
            1,
            "only the master's regular shape is inherited"
        );
        match &slide.background[0] {
            Shape::Text(text) => {
                assert!(!text.title);
                assert_eq!(text.paragraphs[0].text, "Confidential");
                assert_eq!(
                    text.bbox,
                    [720_000.0, 2_880_000.0, 4_320_000.0, 3_240_000.0]
                );
            }
            other => panic!("the master background is not text: {other:?}"),
        }

        let transition = slide
            .transition
            .as_ref()
            .expect("the whole-page transition");
        assert_eq!(transition.effect.as_deref(), Some("slideWipe"));
        assert_eq!(transition.subtype.as_deref(), Some("fromBottom"));
        assert_eq!(transition.direction.as_deref(), Some("reverse"));
        assert_eq!(transition.duration.as_deref(), Some("2s"));

        assert_eq!(
            slide.animations.len(),
            2,
            "the slide transition is not one of the shape effects"
        );
        let first = &slide.animations[0];
        assert_eq!(first.target.as_deref(), Some("Body"));
        assert_eq!(first.effect, "set");
        assert_eq!(first.attribute.as_deref(), Some("visibility"));
        assert_eq!(first.to.as_deref(), Some("visible"));
        assert_eq!(first.begin.as_deref(), Some("0s"));
        assert_eq!(first.duration.as_deref(), Some("0.5s"));

        let second = &slide.animations[1];
        assert_eq!(
            second.target.as_deref(),
            Some("missing"),
            "an id naming no shape is kept as the target"
        );
        assert_eq!(second.effect, "animate");
        assert_eq!(second.from.as_deref(), Some("0"));
        assert_eq!(second.to.as_deref(), Some("5"));
        assert_eq!(
            second.begin.as_deref(),
            Some("1s"),
            "the effect's own begin is relative to its container"
        );
        assert_eq!(second.duration.as_deref(), Some("2s"));
        assert_eq!(second.preset_class.as_deref(), Some("emphasis"));
        assert_eq!(second.preset_id.as_deref(), Some("ooo-emphasis-spin"));

        let page_two = &document.slides[1];
        assert!(
            page_two.background.is_empty(),
            "a page that hides background objects inherited them anyway"
        );
        let style_transition = page_two
            .transition
            .as_ref()
            .expect("the transition on the drawing-page style");
        assert_eq!(style_transition.effect.as_deref(), Some("fade"));
        assert_eq!(style_transition.speed.as_deref(), Some("medium"));
        assert!(page_two.animations.is_empty());

        let page_three = &document.slides[2];
        assert!(
            page_three.background.is_empty(),
            "the drawing-page style's hidden background flag was not read"
        );
        assert!(page_three.transition.is_none());
    }

    #[test]
    fn a_package_without_styles_uses_the_slide_default() {
        let document = OdpDocument::parse_bytes(&package(&[("content.xml", CONTENT.as_bytes())]))
            .expect("the slide package parses");
        assert_eq!(document.width, DEFAULT_WIDTH);
        assert_eq!(document.height, DEFAULT_HEIGHT);
    }
}
