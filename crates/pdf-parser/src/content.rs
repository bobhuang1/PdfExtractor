//! Content-stream interpreter (PDF 1.7, clauses 8.2 and 9).
//!
//! Content streams are postfix: operands precede the operator that consumes
//! them, e.g. `(Hello World) Tj`. We scan a full expression into an operand
//! stack and act when the operator token arrives.
//!
//! Besides text this tracks graphics state, so it can report:
//! * where each `/XObject` image is actually placed on the page (via the CTM),
//! * the bounding boxes of painted vector paths, which is how most figures in
//!   scientific papers are drawn (they contain no raster image at all).
//!
//! A **form XObject** is a page inside the page (PDF 1.7, 8.10): its own
//! content stream, its own resource names, drawn wherever a `Do` places it. It
//! is where a figure that came from another file lives — a LaTeX paper includes
//! `figures/sr.pdf` as one form, so a chart's legend and axis labels are text
//! inside it, not on the page. [`ContentParser::with_forms`] therefore lets the
//! walk descend into one, under the CTM it is drawn with composed with the
//! form's own `/Matrix`.
//!
//! Coordinates are converted from PDF device space (origin bottom-left) to the
//! top-left origin Docling uses by default.

use crate::encoding::FontDecoder;
use std::collections::HashMap;
use std::sync::Arc;

/// A run of text with an approximate bounding box `[l, t, r, b]` in points,
/// top-left origin.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    pub text: String,
    pub bbox: [f64; 4],
    pub font_size: f64,
    /// Resource name of the font in effect (e.g. `F1`).
    pub font_name: Option<String>,
    /// The run was drawn along a rotated or sheared axis. Its box is then the
    /// axis-aligned bounds of the rotated run, and it does not sit on the same
    /// baseline as the horizontal text around it.
    pub rotated: bool,
}

/// An image XObject placed on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedXObject {
    /// Resource name, e.g. `Im0`.
    pub name: String,
    /// Placement box `[l, t, r, b]`, top-left origin.
    pub bbox: [f64; 4],
}

/// Everything a content stream contributes to one page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageContent {
    pub spans: Vec<TextSpan>,
    pub xobjects: Vec<PlacedXObject>,
    /// Bounding boxes of drawn vector graphics, merged into figure-sized clusters.
    pub figures: Vec<[f64; 4]>,
    /// The same painted paths *before* clustering, in device space. A form's
    /// paths are handed back this way so the page they are drawn on can cluster
    /// them together with its own: a chart split across a form boundary is one
    /// figure, not two.
    pub drawn: Vec<[f64; 4]>,
    /// Horizontal rules: painted paths too thin and too wide to be a figure, kept
    /// as `[l, t, r, b]` in top-left origin. The rule a page draws above a footnote
    /// is one, and nothing else in the pipeline sees it — it is not a figure and it
    /// is not text — so a note standing at the foot of a page cannot be told from a
    /// paragraph that merely ran to the bottom without it.
    pub rules: Vec<[f64; 4]>,
}

/// A form XObject, ready to be walked as part of whatever page places it.
///
/// The maps are shared rather than copied: one form can be placed many times on
/// one page (a running header, a plot repeated in a grid), and rebuilding its
/// fonts and nested forms for each placement would be pure waste.
#[derive(Debug, Clone, Default)]
pub struct FormXObject {
    /// The decoded content stream.
    pub content: Arc<Vec<u8>>,
    /// The form's `/Matrix`, applied to its coordinates before the CTM.
    pub matrix: Matrix,
    /// Decoders for the form's *own* `/Resources /Font`, whose names are
    /// independent of the page's: a form's `/F1` is not the page's `/F1`.
    pub fonts: Arc<HashMap<String, FontDecoder>>,
    /// Forms this form places, by resource name.
    pub forms: Arc<HashMap<String, FormXObject>>,
}

#[derive(Debug, Clone)]
enum Operand {
    Str(String),
    Num(f64),
    Name(String),
    Arr(Vec<Operand>),
    Other,
}

impl Operand {
    fn as_num(&self) -> Option<f64> {
        match self {
            Operand::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Operand::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Advance used only when a font declares no `/Widths` at all.
const AVG_GLYPH_WIDTH: f64 = 0.5;

/// A `TJ` offset at or below this is a word gap (see the module docs).
const SPACE_KERNING: f64 = -120.0;

/// A same-line `Td` that overshoots what has been drawn since the previous
/// repositioning by more than this many em is a gap the producer expressed by
/// moving the pen instead of writing a space. One that lands where the glyph
/// advances already stopped is not a gap at all — it is the producer drawing
/// each run as its own `Td`, which is how `test-data/real/dummy.pdf` is written
/// and how its words used to come apart (`Dumm y  PDF  fi le`).
///
/// The corpus's two kinds of `Td` separate cleanly on the measured overshoot:
/// a repositioning lands within rounding of the advance (`dummy.pdf` worst
/// 0.004 em, `irs_f1040`'s per-letter draws *negative*), while the narrowest
/// word gap a producer actually expresses this way is 0.0164 em
/// (`math_layernorm`'s `μ and σ`) and ordinary ones run 0.25–0.37 em. 0.01 em
/// sits in the empty band between them.
const TD_GAP_EM: f64 = 0.01;

/// A `Td` that lands *short* of where the drawn advance ended, by more than
/// this, is also a break: the producer's own metrics disagree with the
/// declared `/Widths` by that much, and it does not jump back that far inside
/// a word. `math_gan`'s algorithmic `for number of training iterations  do`
/// lands 0.75 em back of ours (its `Td` counts none of the `-250` kerns it
/// wrote between the words) — while the letter-by-letter repositioning of
/// `irs_f1040` never goes past 0.02 em back. The empty band between them is
/// why this is a second constant and not `TD_GAP_EM` applied to the
/// difference.
const TD_GAP_BACKWARD_EM: f64 = 0.12;

/// Vertical movement at or above this (points) starts a new line.
const NEW_LINE_DY: f64 = 0.5;

/// A figure must be at least this fraction of the page area.
const FIGURE_MIN_AREA_FRACTION: f64 = 0.012;
/// ...and at most this much. A cluster covering most of the page is the form's
/// own background or border, not an embedded figure.
const FIGURE_MAX_AREA_FRACTION: f64 = 0.5;
/// ...and at least this many points in each direction.
const FIGURE_MIN_SIDE: f64 = 24.0;

/// A horizontal rule is a painted path at most this thick...
const RULE_MAX_THICKNESS: f64 = 3.0;
/// ...and at least this wide. An underline, a table hairline and a cell border are
/// all thin, but none of them spans 120 pt; the rule a page draws above a footnote
/// spans the text column.
const RULE_MIN_WIDTH: f64 = 120.0;
/// Boxes closer than this are merged into one figure.
const FIGURE_MERGE_TOLERANCE: f64 = 8.0;

/// A 2-D affine transform `[a b c d e f]` (PDF matrix convention).
pub type Matrix = [f64; 6];

/// The graphics state saved by `q` and restored by `Q` (PDF 1.7, 8.4.2).
///
/// Everything the content interpreter needs to know about a `q`/`Q` pair: the
/// CTM, the text state, and the text line matrix.
#[derive(Clone)]
struct GState {
    ctm: Matrix,
    font_size: f64,
    font_name: Option<String>,
    leading: f64,
    tm: (f64, f64),
    tlm: (f64, f64),
}

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m1 × m2` under PDF's row-vector convention.
pub fn mat_mul(m1: Matrix, m2: Matrix) -> Matrix {
    [
        m1[0] * m2[0] + m1[1] * m2[2],
        m1[0] * m2[1] + m1[1] * m2[3],
        m1[2] * m2[0] + m1[3] * m2[2],
        m1[2] * m2[1] + m1[3] * m2[3],
        m1[4] * m2[0] + m1[5] * m2[2] + m2[4],
        m1[4] * m2[1] + m1[5] * m2[3] + m2[5],
    ]
}

fn mat_apply(m: Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

#[derive(Clone)]
struct Box2 {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    empty: bool,
}

/// A fresh box is empty. Deriving `Default` here would set `empty` to `false`,
/// which makes the first `add` keep the bogus (0, 0) origin and stretches every
/// figure back to the page corner.
impl Default for Box2 {
    fn default() -> Self {
        Self {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 0.0,
            max_y: 0.0,
            empty: true,
        }
    }
}

impl Box2 {
    fn add(&mut self, x: f64, y: f64) {
        if self.empty {
            self.min_x = x;
            self.max_x = x;
            self.min_y = y;
            self.max_y = y;
            self.empty = false;
        } else {
            self.min_x = self.min_x.min(x);
            self.max_x = self.max_x.max(x);
            self.min_y = self.min_y.min(y);
            self.max_y = self.max_y.max(y);
        }
    }
    fn to_array(&self) -> Option<[f64; 4]> {
        if self.empty {
            None
        } else {
            Some([self.min_x, self.min_y, self.max_x, self.max_y])
        }
    }
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn overlaps(a: [f64; 4], b: [f64; 4], tol: f64) -> bool {
    a[0] - tol <= b[2] && b[0] - tol <= a[2] && a[1] - tol <= b[3] && b[1] - tol <= a[3]
}

/// Merge boxes that touch into larger clusters.
fn cluster(boxes: &[[f64; 4]], tol: f64) -> Vec<[f64; 4]> {
    let mut boxes = boxes.to_vec();
    let mut changed = true;
    while changed {
        changed = false;
        let mut merged: Vec<[f64; 4]> = Vec::new();
        'next: for b in boxes.drain(..) {
            for m in merged.iter_mut() {
                if overlaps(*m, b, tol) {
                    *m = union(*m, b);
                    changed = true;
                    continue 'next;
                }
            }
            merged.push(b);
        }
        boxes = merged;
    }
    boxes
}

/// How deep a chain of forms placing forms is followed. Real documents nest one
/// or two levels (a figure inside a page's contents); a form that reaches this
/// depth is treated as the end of the chain rather than trusted forever.
pub const MAX_FORM_DEPTH: usize = 8;

pub struct ContentParser {
    /// Height of the page, used to flip the y axis.
    page_height: f64,
    /// Character-code decryptors keyed by font resource name (e.g. `F1`).
    fonts: HashMap<String, FontDecoder>,
    /// Transform the stream's coordinates are already in. The identity for a
    /// page; a form's placement for a form being walked.
    initial_ctm: Matrix,
    /// Form XObjects this stream may place, by resource name.
    forms: HashMap<String, FormXObject>,
    /// How deep this walk already is, so [`MAX_FORM_DEPTH`] can end a cycle.
    depth: usize,
}

impl ContentParser {
    pub fn new() -> Self {
        Self {
            page_height: 792.0,
            fonts: HashMap::new(),
            initial_ctm: IDENTITY,
            forms: HashMap::new(),
            depth: 0,
        }
    }

    pub fn with_height(page_height: f64) -> Self {
        Self {
            page_height,
            ..Self::new()
        }
    }

    /// Supply the page's fonts so character codes can be turned into text.
    pub fn with_fonts(mut self, fonts: HashMap<String, FontDecoder>) -> Self {
        self.fonts = fonts;
        self
    }

    /// Supply the page's form XObjects, so `Do` can walk into one instead of
    /// stopping at its placement box.
    pub fn with_forms(mut self, forms: HashMap<String, FormXObject>) -> Self {
        self.forms = forms;
        self
    }

    /// The transform this stream's coordinates are already in. A form is
    /// walked with the CTM of the `Do` that placed it composed with its
    /// `/Matrix`, so its text lands where the page drew it.
    pub fn with_ctm(mut self, ctm: Matrix) -> Self {
        self.initial_ctm = ctm;
        self
    }

    /// Text spans only (convenience wrapper).
    pub fn extract(&self, data: &[u8]) -> Vec<TextSpan> {
        self.extract_page(data).spans
    }

    /// Extract text spans, placed images and figure boxes from a content stream.
    pub fn extract_page(&self, data: &[u8]) -> PageContent {
        let mut spans = Vec::new();
        let mut xobjects: Vec<PlacedXObject> = Vec::new();
        let mut drawn: Vec<[f64; 4]> = Vec::new();

        let mut operands: Vec<Operand> = Vec::new();
        // Text state.
        let mut font_size = 12.0_f64;
        let mut font_name: Option<String> = None;
        let mut tm = (0.0_f64, 0.0_f64);
        let mut tlm = (0.0_f64, 0.0_f64);
        // Linear part of the text matrix (`a b c d`), which is what rotates text.
        let mut tm_linear = [1.0_f64, 0.0, 0.0, 1.0];
        let mut leading = 0.0_f64;

        // Graphics state.
        let mut ctm = self.initial_ctm;
        let mut stack: Vec<GState> = Vec::new();
        let mut path = Box2::default();

        // Current line accumulation. `line_advance` is the pen's travel across
        // the line in em units: real glyph advances where the font declares
        // them, plus kerning and same-line repositioning, so the reported box is
        // measured rather than assumed.
        let mut line = String::new();
        let mut line_advance = 0.0_f64;
        // `line_advance` as of the last same-line `Td`. The difference between
        // the two is what has been drawn since that repositioning, which is
        // what tells the next `Td` apart from a word gap (see `TD_GAP_EM`).
        let mut anchor_em = 0.0_f64;
        let mut line_has_real_widths = false;
        let mut line_origin = (0.0_f64, 0.0_f64);
        let mut line_font_size = font_size;
        let mut line_font_name: Option<String> = None;
        // Device-space direction the pen travels along, and the glyph's own
        // `up` direction. Both come from the text matrix, so rotated text is
        // reported where it actually is.
        let mut line_advance_dir = (1.0_f64, 0.0_f64);
        let mut line_up_dir = (0.0_f64, 1.0_f64);
        let mut line_rotated = false;
        let mut pending_space = false;

        macro_rules! flush {
            () => {
                if !line.trim().is_empty() {
                    let width = if line_has_real_widths {
                        line_advance * line_font_size
                    } else {
                        // No `/Widths`: fall back to an average glyph width.
                        line.chars().count() as f64 * line_font_size * AVG_GLYPH_WIDTH
                    };
                    // The run is a parallelogram: the baseline direction times
                    // its width, and the glyph height along `up`. Bounding its
                    // four corners is what makes rotated text land correctly;
                    // for horizontal text this reduces to the old formula.
                    let (ox, oy) = line_origin;
                    let (ax, ay) = line_advance_dir;
                    let (ux, uy) = line_up_dir;
                    let size = line_font_size;
                    let corners = [
                        (ox, oy),
                        (ox + ax * width, oy + ay * width),
                        (ox + ux * size, oy + uy * size),
                        (ox + ax * width + ux * size, oy + ay * width + uy * size),
                    ];
                    let mut bbox = [
                        f64::INFINITY,
                        f64::INFINITY,
                        f64::NEG_INFINITY,
                        f64::NEG_INFINITY,
                    ];
                    for (x, y) in corners {
                        // Device y grows upward; the reported box has a top-left origin.
                        let ty = self.page_height - y;
                        bbox[0] = bbox[0].min(x);
                        bbox[1] = bbox[1].min(ty);
                        bbox[2] = bbox[2].max(x);
                        bbox[3] = bbox[3].max(ty);
                    }
                    spans.push(TextSpan {
                        text: line.clone(),
                        bbox,
                        font_size: line_font_size,
                        font_name: line_font_name.clone(),
                        rotated: line_rotated,
                    });
                }
                line.clear();
            };
        }

        macro_rules! begin_line {
            () => {
                if line.is_empty() {
                    // Starting a fresh line, so the pen has not travelled yet.
                    line_advance = 0.0;
                    anchor_em = 0.0;
                    line_has_real_widths = false;
                    // Text space -> device space: the text matrix first, then the
                    // CTM. The order matters and is the spec's (PDF 1.7, 9.4.2:
                    // `Trm = Tm × CTM`): the text matrix places the glyph in the
                    // page's coordinate system, the CTM then maps that to device
                    // space. Composing the other way round is invisible while the
                    // CTM is the identity — which is why it survived the whole
                    // page corpus — and wrong by the CTM's translation and scale
                    // once a page has a base `cm` or text is drawn inside a form.
                    let full = mat_mul(
                        [
                            tm_linear[0],
                            tm_linear[1],
                            tm_linear[2],
                            tm_linear[3],
                            tm.0,
                            tm.1,
                        ],
                        ctm,
                    );
                    line_origin = (full[4], full[5]);
                    // One em along the text's x axis, in device points. Its length
                    // is the scale the CTM applies, which the reported font size
                    // is, so the directions are normalised and the magnitudes
                    // carried by `line_font_size` and the advances alone. Keeping
                    // the raw vectors and then multiplying by the device-space
                    // size counted the scale twice — invisible at an identity CTM,
                    // and a 1.7x-too-tall box on a form drawn at 0.43.
                    let scale = (full[0] * full[0] + full[1] * full[1]).sqrt().max(1e-6);
                    line_advance_dir = (full[0] / scale, full[1] / scale);
                    line_up_dir = (full[2] / scale, full[3] / scale);
                    line_font_size = font_size * scale;
                    line_rotated = full[1].abs() + full[2].abs() > 0.01 * scale;
                    line_font_name = font_name.clone();
                }
            };
        }

        /// The decoder in effect for the current line, if any.
        macro_rules! decoder {
            () => {
                line_font_name
                    .as_ref()
                    .or(font_name.as_ref())
                    .and_then(|n| self.fonts.get(n))
            };
        }

        macro_rules! decode_text {
            ($s:expr) => {{
                let codes = crate::encoding::latin1_bytes($s);
                let raw = match decoder!() {
                    Some(decoder) => decoder.decode(&codes),
                    None => $s.to_string(),
                };
                crate::encoding::sanitize(&raw)
            }};
        }

        /// Add the pen travel for a shown string to the current line.
        macro_rules! advance {
            ($s:expr) => {{
                let codes = crate::encoding::latin1_bytes($s);
                if let Some(em) = decoder!().and_then(|d| d.advance_em(&codes)) {
                    line_advance += em;
                    line_has_real_widths = true;
                }
            }};
        }

        // Double braces so the expansion is an expression.
        macro_rules! push_space {
            () => {{
                begin_line!();
                if !line.is_empty() && !line.ends_with(' ') {
                    line.push(' ');
                }
                pending_space = false;
            }};
        }

        macro_rules! show {
            ($text:expr) => {{
                begin_line!();
                if pending_space && !line.is_empty() && !line.ends_with(' ') {
                    line.push(' ');
                }
                pending_space = false;
                advance!($text);
                let decoded = decode_text!($text);
                line.push_str(&decoded);
            }};
        }

        let bytes = data;
        let mut i = 0usize;
        while i < bytes.len() {
            let c = bytes[i];

            if matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0') {
                i += 1;
                continue;
            }

            if c == b'%' {
                while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') {
                    i += 1;
                }
                continue;
            }

            // Inline images carry raw samples between `BI` and `EI`, which must
            // not be tokenised as PDF syntax.
            if bytes[i..].starts_with(b"BI")
                && bytes
                    .get(i + 2)
                    .map(|b| b.is_ascii_whitespace())
                    .unwrap_or(false)
            {
                match find_ei(bytes, i + 2) {
                    Some(end) => {
                        i = end;
                        operands.clear();
                        continue;
                    }
                    None => break,
                }
            }

            if c == b'(' {
                i += 1;
                let (s, ni) = read_content_string(bytes, i);
                i = ni;
                operands.push(Operand::Str(s));
                continue;
            }

            if c == b'<' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'<' {
                    i += 2;
                    let (_, ni) = read_dict(bytes, i);
                    i = ni;
                    operands.push(Operand::Other);
                    continue;
                }
                i += 1;
                let mut digits = String::new();
                while i < bytes.len() && bytes[i] != b'>' {
                    digits.push(bytes[i] as char);
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                let cleaned: String = digits.chars().filter(|c| c.is_ascii_hexdigit()).collect();
                let mut s = String::new();
                let mut it = cleaned.chars();
                while let Some(hi) = it.next() {
                    let lo = it.next().unwrap_or('0');
                    if let Ok(b) = u8::from_str_radix(&format!("{hi}{lo}"), 16) {
                        s.push(b as char);
                    }
                }
                operands.push(Operand::Str(s));
                continue;
            }

            if c == b'[' {
                i += 1;
                let (arr, ni) = read_array(bytes, i);
                i = ni;
                operands.push(Operand::Arr(arr));
                continue;
            }

            if c == b'/' {
                i += 1;
                let start = i;
                while i < bytes.len() && !is_ws(bytes[i]) && !is_delim(bytes[i]) {
                    i += 1;
                }
                let name = String::from_utf8_lossy(&bytes[start..i]).to_string();
                operands.push(Operand::Name(name));
                continue;
            }

            if c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.' {
                let start = i;
                if c == b'+' || c == b'-' {
                    i += 1;
                }
                let mut dot = false;
                while i < bytes.len() {
                    let d = bytes[i];
                    if d.is_ascii_digit() {
                        i += 1;
                    } else if d == b'.' && !dot {
                        dot = true;
                        i += 1;
                    } else {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&bytes[start..i]).to_string();
                operands.push(Operand::Num(text.parse::<f64>().unwrap_or(0.0)));
                continue;
            }

            let start = i;
            while i < bytes.len() && !is_ws(bytes[i]) && !is_delim(bytes[i]) {
                i += 1;
            }
            if i == start {
                i += 1;
                continue;
            }
            let op = String::from_utf8_lossy(&bytes[start..i]).to_string();
            let n = operands.len();

            match op.as_str() {
                // --- graphics state ---
                "q" => {
                    stack.push(GState {
                        ctm,
                        font_size,
                        font_name: font_name.clone(),
                        leading,
                        tm,
                        tlm,
                    });
                    operands.clear();
                }
                "Q" => {
                    if let Some(saved) = stack.pop() {
                        ctm = saved.ctm;
                        font_size = saved.font_size;
                        font_name = saved.font_name;
                        leading = saved.leading;
                        tm = saved.tm;
                        tlm = saved.tlm;
                    }
                    operands.clear();
                }
                "cm" => {
                    if n >= 6 {
                        let m = [
                            operands[n - 6].as_num().unwrap_or(1.0),
                            operands[n - 5].as_num().unwrap_or(0.0),
                            operands[n - 4].as_num().unwrap_or(0.0),
                            operands[n - 3].as_num().unwrap_or(1.0),
                            operands[n - 2].as_num().unwrap_or(0.0),
                            operands[n - 1].as_num().unwrap_or(0.0),
                        ];
                        ctm = mat_mul(m, ctm);
                    }
                    operands.clear();
                }

                // --- path construction (accumulate a box in device space) ---
                "m" | "l" => {
                    if n >= 2 {
                        let (x, y) = mat_apply(
                            ctm,
                            operands[n - 2].as_num().unwrap_or(0.0),
                            operands[n - 1].as_num().unwrap_or(0.0),
                        );
                        path.add(x, y);
                    }
                    operands.clear();
                }
                "c" => {
                    if n >= 6 {
                        for k in 0..3 {
                            let (x, y) = mat_apply(
                                ctm,
                                operands[n - 6 + k * 2].as_num().unwrap_or(0.0),
                                operands[n - 5 + k * 2].as_num().unwrap_or(0.0),
                            );
                            path.add(x, y);
                        }
                    }
                    operands.clear();
                }
                "v" | "y" => {
                    if n >= 4 {
                        for k in 0..2 {
                            let (x, y) = mat_apply(
                                ctm,
                                operands[n - 4 + k * 2].as_num().unwrap_or(0.0),
                                operands[n - 3 + k * 2].as_num().unwrap_or(0.0),
                            );
                            path.add(x, y);
                        }
                    }
                    operands.clear();
                }
                "re" => {
                    if n >= 4 {
                        let x = operands[n - 4].as_num().unwrap_or(0.0);
                        let y = operands[n - 3].as_num().unwrap_or(0.0);
                        let w = operands[n - 2].as_num().unwrap_or(0.0);
                        let h = operands[n - 1].as_num().unwrap_or(0.0);
                        for (px, py) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
                            let (dx, dy) = mat_apply(ctm, px, py);
                            path.add(dx, dy);
                        }
                    }
                    operands.clear();
                }

                // --- path painting: a non-empty box means something was drawn ---
                "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "S" | "s" => {
                    if let Some(bbox) = path.to_array() {
                        drawn.push(bbox);
                    }
                    path = Box2::default();
                    operands.clear();
                }
                "n" => {
                    path = Box2::default();
                    operands.clear();
                }

                // --- XObject placement ---
                "Do" => {
                    if n >= 1 {
                        if let Operand::Name(name) = &operands[n - 1] {
                            if let Some(form) = self.forms.get(name).cloned() {
                                // A form is part of this page: walk its content
                                // where the page drew it (`/Matrix` applies to
                                // the form's coordinates, then the CTM in
                                // effect), and keep what it contributes — its
                                // text as spans, its paths as drawn boxes.
                                if self.depth < MAX_FORM_DEPTH {
                                    let inside = self.enter(&form, ctm);
                                    spans.extend(inside.spans);
                                    drawn.extend(inside.drawn);
                                }
                            } else {
                                // The unit square maps to the placement on the page.
                                let mut b = Box2::default();
                                for (px, py) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                                    let (dx, dy) = mat_apply(ctm, px, py);
                                    b.add(dx, dy);
                                }
                                if let Some(bb) = b.to_array() {
                                    xobjects.push(PlacedXObject {
                                        name: name.clone(),
                                        bbox: flip_box(bb, self.page_height),
                                    });
                                }
                            }
                        }
                    }
                    operands.clear();
                }

                // --- text state ---
                "Tf" => {
                    if n >= 2 {
                        if let Operand::Name(name) = &operands[n - 2] {
                            font_name = Some(name.clone());
                        }
                        if let Some(sz) = operands[n - 1].as_num() {
                            font_size = sz;
                        }
                    }
                    operands.clear();
                }
                "TL" => {
                    if n >= 1 {
                        leading = operands[n - 1].as_num().unwrap_or(leading);
                    }
                    operands.clear();
                }
                "Tm" => {
                    if n >= 6 {
                        // `a b c d` is the linear part and carries any rotation.
                        let mut m = [1.0_f64, 0.0, 0.0, 1.0];
                        for (i, slot) in m.iter_mut().enumerate() {
                            *slot = operands[n - 6 + i].as_num().unwrap_or(*slot);
                        }
                        let x = operands[n - 2].as_num().unwrap_or(0.0);
                        let y = operands[n - 1].as_num().unwrap_or(0.0);
                        let moved_line = (y - tm.1).abs() >= NEW_LINE_DY;
                        tm_linear = m;
                        tm = (x, y);
                        tlm = (x, y);
                        if moved_line {
                            flush!();
                            pending_space = false;
                        }
                    }
                    operands.clear();
                }
                "Td" | "TD" => {
                    if n >= 2 {
                        let dx = operands[n - 2].as_num().unwrap_or(0.0);
                        let dy = operands[n - 1].as_num().unwrap_or(0.0);
                        if op == "TD" {
                            leading = -dy;
                        }
                        tlm = (tlm.0 + dx, tlm.1 + dy);
                        tm = tlm;
                        if dy.abs() < NEW_LINE_DY {
                            // Same line: the pen moved along the run, and what
                            // the move means depends on what has been drawn
                            // since the last one. A producer that draws each
                            // run as its own `Td` lands exactly where the glyph
                            // advances stopped — `dummy.pdf` jumps 50.1 pt
                            // after an advance of 49.6 pt — and that is not a
                            // word break: the text's own space characters are
                            // the spaces. Only a jump that overshoots the drawn
                            // advance is a gap the producer expressed by moving
                            // the pen instead of writing one, and only that
                            // earns the space `show!` inserts — as does a jump
                            // that lands far short of it (see
                            // `TD_GAP_BACKWARD_EM`).
                            let jump = if font_size.abs() > 0.0 {
                                dx / font_size
                            } else {
                                0.0
                            };
                            let gap = jump - (line_advance - anchor_em);
                            if gap > TD_GAP_EM || gap < -TD_GAP_BACKWARD_EM {
                                pending_space = true;
                            }
                            // The box keeps counting the move as travel, as
                            // before: this change is about whether a space is
                            // inserted, not about how wide the run is drawn.
                            if dx > 0.0 && font_size.abs() > 0.0 {
                                line_advance += jump;
                            }
                            anchor_em = line_advance;
                        } else {
                            flush!();
                            pending_space = false;
                        }
                    }
                    operands.clear();
                }
                "T*" => {
                    tlm = (tlm.0, tlm.1 - leading);
                    tm = tlm;
                    flush!();
                    pending_space = false;
                    operands.clear();
                }
                "BT" => {
                    // `BT` resets both matrices to the identity. Carrying the
                    // previous text object's position forward would make every
                    // later `Td` accumulate onto it, and the page would drift
                    // further out of the mediabox with each block.
                    tm = (0.0, 0.0);
                    tlm = (0.0, 0.0);
                    tm_linear = [1.0, 0.0, 0.0, 1.0];
                    operands.clear();
                }
                "ET" => {
                    flush!();
                    pending_space = false;
                    operands.clear();
                }

                // --- text showing ---
                "Tj" => {
                    if let Some(s) = n.checked_sub(1).and_then(|k| operands[k].as_str()) {
                        show!(s);
                    }
                    operands.clear();
                }
                "TJ" => {
                    if n >= 1 {
                        if let Operand::Arr(items) = &operands[n - 1] {
                            for item in items {
                                match item {
                                    Operand::Str(s) => show!(s),
                                    // A negative offset moves the pen right by v/1000 em;
                                    // large ones are word gaps.
                                    Operand::Num(v) => {
                                        if line_has_real_widths || *v <= SPACE_KERNING {
                                            line_advance -= *v / 1000.0;
                                        }
                                        if *v <= SPACE_KERNING {
                                            push_space!();
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    operands.clear();
                }
                "'" | "\"" => {
                    tlm = (tlm.0, tlm.1 - leading);
                    tm = tlm;
                    flush!();
                    pending_space = false;
                    if n >= 1 {
                        if let Some(s) = operands[n - 1].as_str() {
                            show!(s);
                        }
                    }
                    operands.clear();
                }
                _ => {
                    operands.clear();
                }
            }
        }

        flush!();

        // Merge drawn paths into figure-sized clusters, then flip to top-left.
        let clusters = cluster(&drawn, FIGURE_MERGE_TOLERANCE);
        let page_area = 612.0 * self.page_height;
        let min_area = FIGURE_MIN_AREA_FRACTION * page_area;
        let max_area = FIGURE_MAX_AREA_FRACTION * page_area;
        let figures = clusters
            .into_iter()
            .filter(|b| {
                let w = b[2] - b[0];
                let h = b[3] - b[1];
                // Thin rules and underlines are table/text furniture, not figures.
                w >= FIGURE_MIN_SIDE
                    && h >= FIGURE_MIN_SIDE
                    && w * h >= min_area
                    && w * h <= max_area
            })
            .map(|b| flip_box(b, self.page_height))
            .collect();

        // The rules, taken from the same paths before they are clustered away: a
        // page's own separator line is thinner than any figure and wider than any
        // underline, and it is the mark that says "a note starts here".
        let rules = drawn
            .iter()
            .filter(|b| {
                let w = (b[2] - b[0]).abs();
                let h = (b[3] - b[1]).abs();
                w >= RULE_MIN_WIDTH && h <= RULE_MAX_THICKNESS
            })
            .map(|b| flip_box(*b, self.page_height))
            .collect();

        PageContent {
            spans,
            xobjects,
            figures,
            drawn,
            rules,
        }
    }

    /// Walk a form's content in this page's frame of reference.
    fn enter(&self, form: &FormXObject, ctm: Matrix) -> PageContent {
        ContentParser {
            page_height: self.page_height,
            fonts: form.fonts.as_ref().clone(),
            initial_ctm: mat_mul(form.matrix, ctm),
            forms: form.forms.as_ref().clone(),
            depth: self.depth + 1,
        }
        .extract_page(&form.content)
    }

    /// Convenience wrapper: extract spans and join them with newlines.
    pub fn extract_text(&self, data: &[u8]) -> String {
        self.extract(data)
            .into_iter()
            .map(|s| s.text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for ContentParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a device-space box to top-left origin.
fn flip_box(b: [f64; 4], page_height: f64) -> [f64; 4] {
    [b[0], page_height - b[3], b[2], page_height - b[1]]
}

/// Find the end of an inline image: the `EI` keyword, delimited.
fn find_ei(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < bytes.len() {
        if bytes[i] == b'E'
            && bytes[i + 1] == b'I'
            && (i == 0 || bytes[i - 1].is_ascii_whitespace())
            && (i + 2 >= bytes.len() || bytes[i + 2].is_ascii_whitespace())
        {
            return Some(i + 2);
        }
        i += 1;
    }
    None
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
}

fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Read a `(...)` literal string starting just after the opening paren.
fn read_content_string(bytes: &[u8], mut i: usize) -> (String, usize) {
    let mut out = String::new();
    let mut depth = 1usize;
    while i < bytes.len() {
        let c = bytes[i];
        i += 1;
        match c {
            b'\\' => {
                if i >= bytes.len() {
                    break;
                }
                let e = bytes[i];
                i += 1;
                match e {
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'b' => out.push('\x08'),
                    b'f' => out.push('\x0c'),
                    b'(' => out.push('('),
                    b')' => out.push(')'),
                    b'\\' => out.push('\\'),
                    b'0'..=b'7' => {
                        let mut val = (e - b'0') as u32;
                        for _ in 0..2 {
                            if i < bytes.len() && (b'0'..=b'7').contains(&bytes[i]) {
                                val = val * 8 + (bytes[i] - b'0') as u32;
                                i += 1;
                            } else {
                                break;
                            }
                        }
                        if let Some(ch) = char::from_u32(val & 0xff) {
                            out.push(ch);
                        }
                    }
                    b'\r' => {
                        if i < bytes.len() && bytes[i] == b'\n' {
                            i += 1;
                        }
                    }
                    b'\n' => {}
                    other => out.push(other as char),
                }
            }
            b'(' => {
                depth += 1;
                out.push('(');
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                out.push(')');
            }
            other => out.push(other as char),
        }
    }
    (out, i)
}

/// Read a `[ ... ]` array starting just after the opening bracket.
fn read_array(bytes: &[u8], mut i: usize) -> (Vec<Operand>, usize) {
    let mut items = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if is_ws(c) {
            i += 1;
            continue;
        }
        if c == b']' {
            i += 1;
            break;
        }
        if c == b'(' {
            let (s, ni) = read_content_string(bytes, i + 1);
            items.push(Operand::Str(s));
            i = ni;
            continue;
        }
        if c == b'<' {
            i += 1;
            let mut digits = String::new();
            while i < bytes.len() && bytes[i] != b'>' {
                digits.push(bytes[i] as char);
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            let cleaned: String = digits.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            let mut s = String::new();
            let mut it = cleaned.chars();
            while let Some(hi) = it.next() {
                let lo = it.next().unwrap_or('0');
                if let Ok(b) = u8::from_str_radix(&format!("{hi}{lo}"), 16) {
                    s.push(b as char);
                }
            }
            items.push(Operand::Str(s));
            continue;
        }
        if c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.' {
            let start = i;
            if c == b'+' || c == b'-' {
                i += 1;
            }
            let mut dot = false;
            while i < bytes.len() {
                let d = bytes[i];
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == b'.' && !dot {
                    dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            let t = String::from_utf8_lossy(&bytes[start..i]).to_string();
            items.push(Operand::Num(t.parse::<f64>().unwrap_or(0.0)));
            continue;
        }
        i += 1;
    }
    (items, i)
}

/// Skip over a `<< ... >>` dictionary.
fn read_dict(bytes: &[u8], mut i: usize) -> (usize, usize) {
    let mut depth = 1usize;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'<' && bytes[i + 1] == b'<' {
            depth += 1;
            i += 2;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'>' && bytes[i + 1] == b'>' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                break;
            }
            continue;
        }
        i += 1;
    }
    (0, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_multiplication_follows_pdf_convention() {
        // `cm` composes so the newest matrix is the innermost transform:
        // translate(100, 300) then scale(2) puts the unit square's origin at
        // (100, 300) and its far corner at (102, 302).
        let translate = [1.0, 0.0, 0.0, 1.0, 100.0, 300.0];
        let scale = [2.0, 0.0, 0.0, 2.0, 0.0, 0.0];
        let ctm = mat_mul(scale, translate);
        assert_eq!(mat_apply(ctm, 0.0, 0.0), (100.0, 300.0));
        assert_eq!(mat_apply(ctm, 1.0, 1.0), (102.0, 302.0));
    }

    #[test]
    fn clusters_merge_overlapping_boxes() {
        let out = cluster(
            &[
                [0.0, 0.0, 10.0, 10.0],
                [5.0, 5.0, 20.0, 20.0],
                [100.0, 100.0, 110.0, 110.0],
            ],
            1.0,
        );
        assert_eq!(out.len(), 2, "the first two boxes should merge");
    }

    #[test]
    fn figure_boxes_are_detected_from_vector_paths() {
        // A 200x200 filled rectangle in the middle of a 612x792 page.
        let content = b"1 0 0 1 100 300 cm 0 0 200 200 re f";
        let parser = ContentParser::with_height(792.0);
        let page = parser.extract_page(content);
        assert_eq!(
            page.figures.len(),
            1,
            "expected one figure, got {:?}",
            page.figures
        );
        let [l, t, r, b] = page.figures[0];
        assert_eq!((l, r), (100.0, 300.0));
        // Device y 300..500 -> top-left y 292..492
        assert_eq!((t, b), (792.0 - 500.0, 792.0 - 300.0));
    }

    #[test]
    fn page_covering_backgrounds_are_not_figures() {
        // A form's own border/shading rectangle spans the printable area; it is
        // furniture, not an embedded figure.
        let content = b"0.9 g 28 24 556 744 re f";
        let page = ContentParser::with_height(792.0).extract_page(content);
        assert!(
            page.figures.is_empty(),
            "background is not a figure: {:?}",
            page.figures
        );
    }

    #[test]
    fn a_large_panel_is_still_a_figure() {
        // 400x500pt on a 612x792 page = 41% of the area: a real full-width plot.
        let content = b"1 0 0 1 100 200 cm 0 0 400 500 re f";
        let page = ContentParser::with_height(792.0).extract_page(content);
        assert_eq!(page.figures.len(), 1, "got {:?}", page.figures);
    }

    #[test]
    fn thin_rules_are_not_figures() {
        // A 2pt horizontal rule spanning the page.
        let content = b"0 0 2 600 re f";
        let page = ContentParser::with_height(792.0).extract_page(content);
        assert!(
            page.figures.is_empty(),
            "a rule is not a figure: {:?}",
            page.figures
        );
    }

    #[test]
    fn xobject_placement_uses_the_ctm() {
        // Image scaled to the whole page.
        let content = b"q 612 0 0 792 0 0 cm /Im0 Do Q";
        let page = ContentParser::with_height(792.0).extract_page(content);
        assert_eq!(page.xobjects.len(), 1);
        assert_eq!(page.xobjects[0].name, "Im0");
        assert_eq!(page.xobjects[0].bbox, [0.0, 0.0, 612.0, 792.0]);
    }

    #[test]
    fn each_text_object_starts_from_the_origin() {
        // `BT` resets the text matrix, so a block positioned relatively with
        // `Td` does not inherit the previous block's position. Without the
        // reset the second line lands at 2x its own offset and every later block
        // drifts further, which is how whole pages ended up outside the page box.
        let content =
            b"BT /F1 12 Tf 100 700 Td (first) Tj ET BT /F1 12 Tf 50 400 Td (second) Tj ET";
        let page = ContentParser::with_height(792.0).extract_page(content);
        assert_eq!(page.spans.len(), 2, "{:?}", page.spans);
        assert_eq!(page.spans[0].bbox[0], 100.0, "{:?}", page.spans[0]);
        assert_eq!(
            page.spans[1].bbox[0], 50.0,
            "the second block inherited a position: {:?}",
            page.spans[1]
        );
    }

    #[test]
    fn inline_images_do_not_leak_into_text() {
        let content =
            b"BT /F1 12 Tf 10 700 Td (before) Tj ET BI /W 2 /H 2 ID \x01\x02 EI BT (after) Tj ET";
        let page = ContentParser::with_height(792.0).extract_page(content);
        let text: String = page
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("|");
        assert!(text.contains("before"), "{text}");
        assert!(text.contains("after"), "{text}");
    }
}
