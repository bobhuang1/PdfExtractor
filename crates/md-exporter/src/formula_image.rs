//! An equation drawn as a picture.
//!
//! A DOCX carries Office Math, and Docling writes the OMML accents in
//! `unicode-math` names, which the LaTeX reader does not always know. The HTML
//! writer re-spells every one of them — as the mark's shape where LaTeX has no
//! name for it — and typesets the equation as MathML. For a command outside the
//! parser's vocabulary all the same, its parser reports an error, so the
//! equation would otherwise be lost to the page. Printing the TeX instead shows
//! *how* the equation was written, not the equation, so this module draws it:
//! the small subset of LaTeX a formula from these documents uses is read into a
//! tree, laid out on a baseline, and written as a standalone SVG.
//!
//! The SVG is carried inline as a base64 `data:` URI, the way an embedded
//! picture already travels: one page, no side files, nothing to resolve. The
//! LaTeX itself is the `<img>`'s `alt` text, which is where the reader takes the
//! formula back from — a page written here reads back as the formula item it
//! came from, not as a picture of one.
//!
//! Nothing is fetched and no font is embedded: the drawing is lines and
//! positioned text, so it renders wherever the page does. The text is set in the
//! math fonts a browser already has, and each run is given the width this
//! layout computed, so the picture of the equation is stable from viewer to
//! viewer.

use base64::{engine::general_purpose::STANDARD, Engine as _};

/// The size a formula's main line is set at, in SVG units.
const SIZE: f64 = 16.0;
/// Breathing room around the drawing, so a glyph never sits on the edge.
const MARGIN: f64 = 5.0;

/// A formula drawn as a picture.
pub(crate) struct Drawing {
    /// The picture's bytes as a base64 `data:` URI.
    pub(crate) uri: String,
    /// How far the picture reaches below the equation's baseline, in its own
    /// units. An `<img>` sits on the text baseline by its bottom edge, so an
    /// inline equation is dropped by this much to stand on the line it is in.
    pub(crate) descent: f64,
}

/// The formula as a picture, or `None` when there is nothing to draw — an empty
/// formula has no picture.
pub(crate) fn drawing(latex: &str) -> Option<Drawing> {
    let (svg, descent) = svg(latex)?;
    Some(Drawing {
        uri: format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg)),
        descent,
    })
}

/// The formula as a standalone SVG document and the descent its baseline has,
/// or `None` when it holds no glyph at all.
fn svg(latex: &str) -> Option<(String, f64)> {
    let nodes = parse(latex);
    if nodes.is_empty() {
        return None;
    }
    let body = layout(&nodes, SIZE);
    if body.w <= 0.0 || body.above + body.below <= 0.0 {
        return None;
    }
    let width = body.w + 2.0 * MARGIN;
    let height = body.above + body.below + 2.0 * MARGIN;
    // Coordinates in a box are relative to its own baseline; the page needs
    // them moved down under the top margin.
    let baseline = MARGIN + body.above;

    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" role=\"img\" width=\"{width}\" \
         height=\"{height}\" viewBox=\"0 0 {width} {height}\">\
         <g fill=\"#000000\" stroke=\"#000000\" font-family=\"STIX Two Math, Cambria Math, \
         DejaVu Serif, Times New Roman, serif\">",
        width = num(width),
        height = num(height),
    );
    for prim in &body.prims {
        match prim {
            Prim::Text { x, y, size, text } => {
                out.push_str(&format!(
                    "<text x=\"{}\" y=\"{}\" font-size=\"{}\" textLength=\"{}\" \
                     lengthAdjust=\"spacingAndGlyphs\">{}</text>",
                    num(MARGIN + x),
                    num(baseline + y),
                    num(*size),
                    num(text_width(text, *size)),
                    xml_escape(text),
                ));
            }
            Prim::Line {
                x1,
                y1,
                x2,
                y2,
                width,
            } => {
                out.push_str(&format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke-width=\"{}\"/>",
                    num(MARGIN + x1),
                    num(baseline + y1),
                    num(MARGIN + x2),
                    num(baseline + y2),
                    num(*width),
                ));
            }
            Prim::Path { points, width } => {
                out.push_str("<path fill=\"none\" stroke-width=\"");
                out.push_str(&num(*width));
                out.push_str("\" d=\"");
                for (index, (x, y)) in points.iter().enumerate() {
                    out.push_str(&format!(
                        "{}{} {}",
                        if index == 0 { "M" } else { "L" },
                        num(MARGIN + x),
                        num(baseline + y),
                    ));
                }
                out.push_str("\"/>");
            }
        }
    }
    out.push_str("</g></svg>");
    Some((out, body.below + MARGIN))
}

/// A laid-out row of glyphs: its width, how far it reaches above and below the
/// baseline, and the marks that draw it.
#[derive(Debug, Default)]
struct Boxy {
    w: f64,
    above: f64,
    below: f64,
    prims: Vec<Prim>,
}

/// One mark of a drawing. A text run sits on its baseline; a line or a path is
/// already in the row's coordinates.
#[derive(Debug, Clone)]
enum Prim {
    Text {
        x: f64,
        y: f64,
        size: f64,
        text: String,
    },
    Line {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        width: f64,
    },
    /// A polyline: an accent, which is drawn rather than set because the mark
    /// a viewer's font has is not the mark that was asked for.
    Path { points: Vec<(f64, f64)>, width: f64 },
}

impl Prim {
    fn shift(&mut self, dx: f64, dy: f64) {
        match self {
            Prim::Text { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            Prim::Line { x1, y1, x2, y2, .. } => {
                *x1 += dx;
                *y1 += dy;
                *x2 += dx;
                *y2 += dy;
            }
            Prim::Path { points, .. } => {
                for (x, y) in points {
                    *x += dx;
                    *y += dy;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The LaTeX a formula here is written in
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Space(f64),
    Sup(Vec<Node>),
    Sub(Vec<Node>),
    Frac(Vec<Node>, Vec<Node>),
    /// An optional index, then the radicand.
    Sqrt(Vec<Node>, Vec<Node>),
    /// A mark set over the first glyph of the body.
    Accent(Mark, Vec<Node>),
    /// An arrow over the body — `\vec`.
    Vector(Vec<Node>),
    /// A rule under (`true`: over) the body.
    RuleAbove(Vec<Node>),
    RuleBelow(Vec<Node>),
    /// A body between a left and a right delimiter; `\0` is a missing one.
    Delimited(char, char, Vec<Node>),
    /// Rows of cells, with the delimiters the environment brings.
    Matrix(Vec<Vec<Vec<Node>>>, char, char),
    /// A matrix's cell and row separators, before the rows are split.
    Align,
    Break,
}

fn parse(latex: &str) -> Vec<Node> {
    let mut parser = Parser::new(latex);
    parser.sequence(true)
}

struct Parser {
    chars: Vec<char>,
    i: usize,
    /// How many groups deep the reader is. A formula from a document is a dozen
    /// levels at most; a page of braces is not worth a stack overflow.
    depth: usize,
}

impl Parser {
    fn new(latex: &str) -> Self {
        Self {
            chars: latex.chars().collect(),
            i: 0,
            depth: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.i + ahead).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }

    fn eat(&mut self, wanted: char) -> bool {
        if self.peek() == Some(wanted) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn skip_spaces(&mut self) {
        while self.peek().is_some_and(|c| c.is_whitespace()) {
            self.i += 1;
        }
    }

    /// The name of the control sequence at the cursor, without consuming it.
    fn peek_command(&self) -> Option<String> {
        if self.peek() != Some('\\') {
            return None;
        }
        let mut name = String::new();
        let mut j = self.i + 1;
        while let Some(&c) = self.chars.get(j) {
            if c.is_ascii_alphabetic() {
                name.push(c);
                j += 1;
            } else {
                break;
            }
        }
        if name.is_empty() {
            name.push(*self.chars.get(j)?);
        }
        Some(name)
    }

    /// Consume a control sequence and return its name. `\\` is not one: a row
    /// break is recognised before this is called.
    fn command(&mut self) -> String {
        debug_assert_eq!(self.peek(), Some('\\'));
        self.i += 1;
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() {
                name.push(c);
                self.i += 1;
            } else {
                break;
            }
        }
        if name.is_empty() {
            if let Some(c) = self.bump() {
                name.push(c);
            }
        }
        name
    }

    /// Read a body up to its closing brace — or, at the top level, to the end.
    /// `\right` and `\end` are left at the cursor for whoever opened them.
    fn sequence(&mut self, nested: bool) -> Vec<Node> {
        self.depth += 1;
        if self.depth > 96 {
            // Past any nesting a real formula has: the rest of the group is
            // skipped rather than parsed, and the group's content is dropped.
            let mut open = 1usize;
            while let Some(c) = self.bump() {
                match c {
                    '{' => open += 1,
                    '}' => {
                        open -= 1;
                        if open == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            self.depth -= 1;
            return Vec::new();
        }
        let mut nodes = Vec::new();
        while let Some(c) = self.peek() {
            match c {
                '}' => {
                    self.i += 1;
                    if nested {
                        break;
                    }
                }
                '\\' => {
                    if self.peek_at(1) == Some('\\') {
                        self.i += 2;
                        nodes.push(Node::Break);
                        continue;
                    }
                    if matches!(self.peek_command().as_deref(), Some("right") | Some("end")) {
                        break;
                    }
                    let name = self.command();
                    self.control(&name, &mut nodes);
                }
                '^' | '_' => {
                    self.i += 1;
                    let body = self.argument();
                    nodes.push(if c == '^' {
                        Node::Sup(body)
                    } else {
                        Node::Sub(body)
                    });
                }
                '{' => {
                    self.i += 1;
                    let body = self.sequence(true);
                    nodes.extend(body);
                }
                '&' => {
                    self.i += 1;
                    nodes.push(Node::Align);
                }
                '~' | ' ' | '\n' | '\r' | '\t' => {
                    self.i += 1;
                    if c != '~' {
                        // A source break is not a space in TeX, but it is one
                        // where a formula is written for reading.
                        nodes.push(Node::Space(0.24));
                    } else {
                        nodes.push(Node::Space(0.33));
                    }
                }
                '$' => {
                    self.i += 1;
                }
                _ => {
                    let mut text = String::new();
                    while let Some(c) = self.peek() {
                        if matches!(
                            c,
                            '\\' | '{'
                                | '}'
                                | '^'
                                | '_'
                                | '&'
                                | '~'
                                | '$'
                                | ' '
                                | '\n'
                                | '\r'
                                | '\t'
                        ) {
                            break;
                        }
                        text.push(c);
                        self.i += 1;
                    }
                    if !text.is_empty() {
                        nodes.push(Node::Text(text));
                    }
                }
            }
        }
        self.depth -= 1;
        nodes
    }

    /// One argument: a braced body, a control sequence, or a single character.
    fn argument(&mut self) -> Vec<Node> {
        self.skip_spaces();
        if self.eat('{') {
            return self.sequence(true);
        }
        let Some(c) = self.peek() else {
            return Vec::new();
        };
        if c == '\\' {
            if self.peek_at(1) == Some('\\') {
                self.i += 2;
                return Vec::new();
            }
            let name = self.command();
            let mut nodes = Vec::new();
            self.control(&name, &mut nodes);
            return nodes;
        }
        self.i += 1;
        vec![Node::Text(c.to_string())]
    }

    /// The body of a `\sqrt`'s optional index, `[…]`.
    fn index(&mut self) -> Vec<Node> {
        let mut raw = String::new();
        let mut depth = 0usize;
        while let Some(c) = self.bump() {
            match c {
                '{' => {
                    depth += 1;
                    raw.push(c);
                }
                '}' => {
                    depth = depth.saturating_sub(1);
                    raw.push(c);
                }
                ']' if depth == 0 => break,
                _ => raw.push(c),
            }
        }
        Parser::new(&raw).sequence(true)
    }

    /// The delimiter after `\left`/`\right`, as the glyph to draw.
    fn delimiter(&mut self) -> char {
        self.skip_spaces();
        let Some(c) = self.peek() else {
            return '\0';
        };
        if c == '\\' {
            return match self.command().as_str() {
                "{" | "lbrace" => '{',
                "}" | "rbrace" => '}',
                "|" | "vert" => '|',
                "Vert" | "parallel" => '‖',
                "langle" => '⟨',
                "rangle" => '⟩',
                "lceil" => '⌈',
                "rceil" => '⌉',
                "lfloor" => '⌊',
                "rfloor" => '⌋',
                "backslash" => '\\',
                "." => '\0',
                _ => '|',
            };
        }
        self.i += 1;
        if c == '.' {
            '\0'
        } else {
            c
        }
    }

    fn control(&mut self, name: &str, out: &mut Vec<Node>) {
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let numerator = self.argument();
                let denominator = self.argument();
                out.push(Node::Frac(numerator, denominator));
            }
            "sqrt" => {
                self.skip_spaces();
                let index = if self.eat('[') {
                    self.index()
                } else {
                    Vec::new()
                };
                let radicand = self.argument();
                out.push(Node::Sqrt(index, radicand));
            }
            // The text of these is not an equation, so it is read as it is
            // written — a `\text{if }` keeps the space it was given.
            "text" | "textrm" | "textnormal" | "mbox" | "hbox" | "operatorname" | "textit"
            | "textbf" | "texttt" | "textsf" => {
                let body = self.argument();
                out.extend(body.into_iter().map(|node| match node {
                    Node::Text(text) => Node::Text(text.replace(['~', '\u{b}'], " ")),
                    other => other,
                }));
            }
            // A style selection changes no glyph this draws, only its font.
            "mathrm"
            | "mathbf"
            | "mathit"
            | "mathsf"
            | "mathtt"
            | "mathcal"
            | "mathbb"
            | "mathfrak"
            | "boldsymbol"
            | "bm"
            | "operatornamewithlimits" => {
                out.extend(self.argument());
            }
            "left" => {
                let left = self.delimiter();
                let body = self.sequence(true);
                let right = if self.peek_command().as_deref() == Some("right") {
                    self.command();
                    self.delimiter()
                } else {
                    '\0'
                };
                out.push(Node::Delimited(left, right, body));
            }
            // A `\right` with no `\left` has nothing to close; the body it was
            // in already ended at it.
            "right" => {}
            "begin" => self.environment(out),
            "end" => {}
            "underline" => {
                let body = self.argument();
                out.push(Node::RuleBelow(body));
            }
            "overline" => {
                let body = self.argument();
                out.push(Node::RuleAbove(body));
            }
            "overbrace" | "underbrace" | "overbracket" | "underbracket" => {
                let body = self.argument();
                out.push(Node::RuleAbove(body));
            }
            "vec" => {
                let body = self.argument();
                out.push(Node::Vector(body));
            }
            "acute" | "grave" | "check" | "breve" | "ocirc" | "circ" | "hat" | "tilde" | "bar"
            | "macron" | "dot" | "ddot" => {
                let body = self.argument();
                if let Some(mark) = accent(name) {
                    out.push(Node::Accent(mark, body));
                } else {
                    out.extend(body);
                }
            }
            "quad" => out.push(Node::Space(1.0)),
            "qquad" => out.push(Node::Space(2.0)),
            "enspace" => out.push(Node::Space(0.5)),
            "," => out.push(Node::Space(0.17)),
            ";" => out.push(Node::Space(0.28)),
            ":" => out.push(Node::Space(0.22)),
            " " | "space" | "thinspace" => out.push(Node::Space(0.33)),
            "!" => out.push(Node::Space(0.0)),
            // Layout hints that do not draw, and labels that take an argument
            // the page never shows.
            "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle" | "limits"
            | "nolimits" | "notag" | "nonumber" | "not" => {}
            "label" | "tag" | "hspace" | "vspace" | "phantom" | "hphantom" | "vphantom" => {
                let _ = self.argument();
            }
            "prime" => out.push(Node::Text("′".to_string())),
            other => match glyph(other) {
                Some(glyph) => out.push(Node::Text(glyph.to_string())),
                // A command outside the vocabulary is drawn as the TeX it is:
                // an equation missing a piece is worse than one that says
                // which piece it could not read.
                None => out.push(Node::Text(format!("\\{other}"))),
            },
        }
    }

    /// A `matrix`-family environment: its rows are split at `\\` and `&`, and
    /// its delimiters come from its name.
    fn environment(&mut self, out: &mut Vec<Node>) {
        self.skip_spaces();
        let mut name = String::new();
        if self.eat('{') {
            while let Some(c) = self.bump() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
        }
        let name = name.trim().to_string();
        // `array` (and an `align` written as one) carries a column spec of its
        // own: `{cc}` says how many columns there are, which the cells already
        // say.
        self.skip_spaces();
        if matches!(
            name.as_str(),
            "array" | "tabular" | "align" | "align*" | "aligned" | "alignedat"
        ) && self.peek() == Some('{')
        {
            let _ = self.argument();
        }
        let body = self.sequence(true);
        if self.peek_command().as_deref() == Some("end") {
            self.command();
            self.skip_spaces();
            if self.peek() == Some('{') {
                let _ = self.argument();
            }
        }
        let (left, right) = match name.as_str() {
            "pmatrix" => ('(', ')'),
            "bmatrix" => ('[', ']'),
            "Bmatrix" => ('{', '}'),
            "vmatrix" => ('|', '|'),
            "Vmatrix" => ('‖', '‖'),
            "cases" => ('{', '\0'),
            _ => ('\0', '\0'),
        };
        if matches!(
            name.as_str(),
            "matrix"
                | "smallmatrix"
                | "pmatrix"
                | "bmatrix"
                | "Bmatrix"
                | "vmatrix"
                | "Vmatrix"
                | "cases"
                | "array"
                | "eqnarray"
                | "eqnarray*"
                | "aligned"
                | "align"
                | "align*"
                | "alignedat"
                | "gathered"
                | "gather"
                | "gather*"
                | "split"
                | "subarray"
        ) {
            out.push(Node::Matrix(split_rows(&body), left, right));
        } else {
            // An unknown environment still holds an equation's rows; they are
            // kept as rows rather than dropped.
            out.push(Node::Delimited(left, right, body));
        }
    }
}

/// Split a matrix body at its `&` and `\\` markers. Separators a nested
/// environment owns are inside its own [`Node::Matrix`], so they do not reach
/// this.
fn split_rows(nodes: &[Node]) -> Vec<Vec<Vec<Node>>> {
    let mut rows = Vec::new();
    let mut row: Vec<Vec<Node>> = Vec::new();
    let mut cell: Vec<Node> = Vec::new();
    for node in nodes {
        match node {
            Node::Align => row.push(std::mem::take(&mut cell)),
            Node::Break => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            other => cell.push(other.clone()),
        }
    }
    row.push(cell);
    rows.push(row);
    rows.retain(|row| !(row.len() == 1 && row[0].is_empty()));
    rows
}

/// The mark an accent command sets over its glyph.
///
/// These are the spacing marks a font draws on their own, not combining marks
/// attached to a letter: combining forms depend on the font having that pair, and
/// a drawing that renders in whatever font the viewer has cannot rely on it. The
/// position is this layout's, not the font's.
fn accent(name: &str) -> Option<Mark> {
    Some(match name {
        "acute" => Mark::Acute,
        "grave" => Mark::Grave,
        "check" => Mark::Check,
        "breve" => Mark::Breve,
        // Docling writes a hat this way, the OMML name for a circumflex.
        "ocirc" | "circ" | "hat" => Mark::Hat,
        "tilde" => Mark::Tilde,
        "bar" | "macron" => Mark::Bar,
        "dot" => Mark::Dot,
        "ddot" => Mark::Ddot,
        _ => return None,
    })
}

/// The accent marks a formula is written with.
///
/// Each is drawn, like the radical and the fraction rules: a spacing accent
/// glyph sits wherever the viewer's font puts it, so it cannot be placed over
/// the letter without knowing that font. A short path can.
#[derive(Debug, Clone, Copy)]
enum Mark {
    Acute,
    Grave,
    Hat,
    Check,
    Breve,
    Tilde,
    Bar,
    Dot,
    Ddot,
}

/// The glyph a symbol command stands for.
fn glyph(name: &str) -> Option<char> {
    Some(match name {
        "alpha" => 'α',
        "beta" => 'β',
        "gamma" => 'γ',
        "delta" => 'δ',
        "epsilon" => 'ϵ',
        "varepsilon" => 'ε',
        "zeta" => 'ζ',
        "eta" => 'η',
        "theta" => 'θ',
        "vartheta" => 'ϑ',
        "iota" => 'ι',
        "kappa" => 'κ',
        "lambda" => 'λ',
        "mu" => 'μ',
        "nu" => 'ν',
        "xi" => 'ξ',
        "omicron" => 'ο',
        "pi" => 'π',
        "varpi" => 'ϖ',
        "rho" => 'ρ',
        "varrho" => 'ϱ',
        "sigma" => 'σ',
        "varsigma" => 'ς',
        "tau" => 'τ',
        "upsilon" => 'υ',
        "phi" => 'ϕ',
        "varphi" => 'φ',
        "chi" => 'χ',
        "psi" => 'ψ',
        "omega" => 'ω',
        "Gamma" => 'Γ',
        "Delta" => 'Δ',
        "Theta" => 'Θ',
        "Lambda" => 'Λ',
        "Xi" => 'Ξ',
        "Pi" => 'Π',
        "Sigma" => 'Σ',
        "Upsilon" => 'Υ',
        "Phi" => 'Φ',
        "Psi" => 'Ψ',
        "Omega" => 'Ω',
        "pm" => '±',
        "mp" => '∓',
        "times" => '×',
        "div" => '÷',
        "cdot" => '⋅',
        "ast" => '∗',
        "star" => '⋆',
        "circ" => '∘',
        "bullet" => '∙',
        "cap" => '∩',
        "cup" => '∪',
        "uplus" => '⊎',
        "sqcap" => '⊓',
        "sqcup" => '⊔',
        "vee" | "lor" => '∨',
        "wedge" | "land" => '∧',
        "setminus" => '∖',
        "leq" | "le" => '≤',
        "geq" | "ge" => '≥',
        "neq" | "ne" => '≠',
        "approx" => '≈',
        "equiv" => '≡',
        "sim" => '∼',
        "simeq" => '≃',
        "cong" => '≅',
        "propto" => '∝',
        "ll" => '≪',
        "gg" => '≫',
        "infty" => '∞',
        "partial" => '∂',
        "nabla" => '∇',
        "sum" => '∑',
        "prod" => '∏',
        "coprod" => '∐',
        "int" => '∫',
        "iint" => '∬',
        "oint" => '∮',
        "in" => '∈',
        "notin" => '∉',
        "ni" => '∋',
        "subset" => '⊂',
        "supset" => '⊃',
        "subseteq" => '⊆',
        "supseteq" => '⊇',
        "forall" => '∀',
        "exists" => '∃',
        "nexists" => '∄',
        "neg" | "lnot" => '¬',
        "top" => '⊤',
        "bot" | "perp" => '⊥',
        "parallel" => '∥',
        "to" | "rightarrow" => '→',
        "leftarrow" | "gets" => '←',
        "Rightarrow" | "implies" => '⇒',
        "Leftarrow" => '⇐',
        "leftrightarrow" => '↔',
        "Leftrightarrow" | "iff" => '⇔',
        "mapsto" => '↦',
        "uparrow" => '↑',
        "downarrow" => '↓',
        "ldots" | "dots" => '…',
        "cdots" => '⋯',
        "vdots" => '⋮',
        "ddots" => '⋱',
        "degree" => '°',
        "angle" => '∠',
        "triangle" => '△',
        "square" => '□',
        "checkmark" => '✓',
        "dagger" => '†',
        "ddagger" => '‡',
        "hbar" => 'ℏ',
        "ell" => 'ℓ',
        "Re" => 'ℜ',
        "Im" => 'ℑ',
        "aleph" => 'ℵ',
        "emptyset" | "varnothing" => '∅',
        "mid" => '∣',
        "vert" => '|',
        "Vert" => '‖',
        "langle" => '⟨',
        "rangle" => '⟩',
        "lceil" => '⌈',
        "rceil" => '⌉',
        "lfloor" => '⌊',
        "rfloor" => '⌋',
        "lbrace" => '{',
        "rbrace" => '}',
        "backslash" => '\\',
        "&" => '&',
        "%" => '%',
        "$" => '$',
        "#" => '#',
        "_" => '_',
        "colon" => ':',
        "therefore" => '∴',
        "because" => '∵',
        "surd" => '√',
        "oplus" => '⊕',
        "otimes" => '⊗',
        "odot" => '⊙',
        "ominus" => '⊖',
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Laying it out
// ---------------------------------------------------------------------------

/// Lay a body out at `size`, one atom after another on its baseline.
fn layout(nodes: &[Node], size: f64) -> Boxy {
    let mut out = Boxy::default();
    let mut x = 0.0;
    for node in nodes {
        let atom = atom(node, size);
        out.above = out.above.max(atom.above);
        out.below = out.below.max(atom.below);
        for mut prim in atom.prims {
            prim.shift(x, 0.0);
            out.prims.push(prim);
        }
        x += atom.w;
    }
    out.w = x;
    out
}

fn atom(node: &Node, size: f64) -> Boxy {
    match node {
        Node::Text(text) => text_box(text, size),
        Node::Space(em) => Boxy {
            w: em * size,
            ..Boxy::default()
        },
        Node::Sup(body) => {
            let inner = layout(body, size * 0.7);
            // The script sits above the baseline, its own descent hanging
            // where a superscript's does.
            let lift = 0.42 * size;
            let mut out = shifted(inner, 0.0, -lift);
            out.above += lift;
            out
        }
        Node::Sub(body) => {
            let inner = layout(body, size * 0.7);
            let drop = 0.18 * size;
            let mut out = shifted(inner, 0.0, drop);
            out.below += drop;
            out
        }
        Node::Frac(numerator, denominator) => fraction(numerator, denominator, size),
        Node::Sqrt(index, radicand) => root(index, radicand, size),
        Node::Accent(mark, body) => accent_box(*mark, body, size),
        Node::Vector(body) => vector_box(body, size),
        Node::RuleAbove(body) => {
            let mut inner = layout(body, size);
            let y = -(inner.above + 0.08 * size);
            inner.prims.push(Prim::Line {
                x1: 0.0,
                y1: y,
                x2: inner.w,
                y2: y,
                width: 0.05 * size,
            });
            inner.above += 0.13 * size;
            inner
        }
        Node::RuleBelow(body) => {
            let mut inner = layout(body, size);
            let y = inner.below + 0.08 * size;
            inner.prims.push(Prim::Line {
                x1: 0.0,
                y1: y,
                x2: inner.w,
                y2: y,
                width: 0.05 * size,
            });
            inner.below += 0.13 * size;
            inner
        }
        Node::Delimited(left, right, body) => {
            let inner = layout(body, size);
            delimited(*left, *right, inner, size)
        }
        Node::Matrix(rows, left, right) => {
            let inner = matrix(rows, size);
            delimited(*left, *right, inner, size)
        }
        // A separator outside a matrix is the space between what it split.
        Node::Align | Node::Break => Boxy {
            w: 0.4 * size,
            ..Boxy::default()
        },
    }
}

fn shifted(mut body: Boxy, dx: f64, dy: f64) -> Boxy {
    for prim in &mut body.prims {
        prim.shift(dx, dy);
    }
    body
}

fn text_box(text: &str, size: f64) -> Boxy {
    // A big operator is set larger and centred on the axis, the way a sum or an
    // integral stands out from the line it is on.
    let big = text
        .chars()
        .any(|c| matches!(c, '∑' | '∏' | '∐' | '∫' | '∬' | '∮' | '⋃' | '⋂'));
    let size = if big { size * 1.15 } else { size };
    let mut prims = Vec::new();
    let mut x = 0.0;
    let mut run = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (index, &c) in chars.iter().enumerate() {
        if !is_operator(c) {
            run.push(c);
            continue;
        }
        // A relation set straight against its operands (`a=b`, `x>0`) is not
        // how TeX sets one: it carries a space on each side that an operand
        // meets. Nothing is put before a sign that opens the run — `-x` is a
        // negative, not a subtraction.
        push_run(&mut prims, &mut run, &mut x, size);
        if index > 0 && operand(chars[index - 1]) {
            x += 0.18 * size;
        }
        prims.push(Prim::Text {
            x,
            y: 0.0,
            size,
            text: c.to_string(),
        });
        x += glyph_width(c) * size;
        if chars.get(index + 1).is_some_and(|c| operand(*c)) {
            x += 0.12 * size;
        }
    }
    push_run(&mut prims, &mut run, &mut x, size);
    Boxy {
        w: x,
        above: if big { 1.0 * size } else { 0.72 * size },
        below: if big { 0.28 * size } else { 0.22 * size },
        prims,
    }
}

/// Emit the letters collected so far, advancing the row past them.
fn push_run(prims: &mut Vec<Prim>, run: &mut String, x: &mut f64, size: f64) {
    if run.is_empty() {
        return;
    }
    let text = std::mem::take(run);
    let width = text_width(&text, size);
    prims.push(Prim::Text {
        x: *x,
        y: 0.0,
        size,
        text,
    });
    *x += width;
}

/// The characters TeX sets with space around them.
fn is_operator(c: char) -> bool {
    matches!(
        c,
        '=' | '+'
            | '-'
            | '−'
            | '<'
            | '>'
            | '≤'
            | '≥'
            | '≠'
            | '≈'
            | '×'
            | '÷'
            | '±'
            | '∓'
            | '→'
            | '⇒'
            | '↔'
    )
}

/// Whether a character is something an operator can stand beside.
fn operand(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, ')' | ']' | '}' | '(' | '[' | '{' | '′')
}

fn fraction(numerator: &[Node], denominator: &[Node], size: f64) -> Boxy {
    let top = layout(numerator, size * 0.95);
    let bottom = layout(denominator, size * 0.95);
    let axis = 0.24 * size;
    let gap = 0.14 * size;
    // The two halves are centred on the same axis the rule runs along, each
    // cleared of the rule by the same gap.
    let top_baseline = -axis - gap - top.below;
    let bottom_baseline = -axis + gap + bottom.above;
    let w = top.w.max(bottom.w) + 0.28 * size;

    let mut out = Boxy {
        w,
        above: -top_baseline + top.above,
        below: bottom_baseline + bottom.below,
        prims: Vec::new(),
    };
    let (top_x, bottom_x) = ((w - top.w) / 2.0, (w - bottom.w) / 2.0);
    out.prims.extend(shifted(top, top_x, top_baseline).prims);
    out.prims
        .extend(shifted(bottom, bottom_x, bottom_baseline).prims);
    out.prims.push(Prim::Line {
        x1: 0.05 * size,
        y1: -axis,
        x2: w - 0.05 * size,
        y2: -axis,
        width: 0.05 * size,
    });
    out
}

fn root(index: &[Node], radicand: &[Node], size: f64) -> Boxy {
    let inner = layout(radicand, size);
    let pad = 0.1 * size;
    let top = -(inner.above + pad);
    let bottom = inner.below + 0.06 * size;
    let stroke = 0.05 * size;
    let hook = 0.18 * size;
    let glyph = 0.4 * size;
    // The radical is drawn, not set: a sign that has to reach past the tallest
    // glyph in it cannot be a fixed glyph of some other size.
    let content_x = glyph + hook + 0.06 * size;
    let mut out = Boxy {
        w: content_x + inner.w + 0.05 * size,
        above: -top,
        below: bottom + stroke,
        prims: Vec::new(),
    };
    let tick = 0.3 * size;
    out.prims.push(Prim::Line {
        x1: 0.0,
        y1: tick,
        x2: hook,
        y2: bottom,
        width: stroke,
    });
    out.prims.push(Prim::Line {
        x1: hook,
        y1: bottom,
        x2: hook + glyph,
        y2: top,
        width: stroke,
    });
    out.prims.push(Prim::Line {
        x1: hook + glyph,
        y1: top,
        x2: out.w,
        y2: top,
        width: stroke,
    });
    out.prims.extend(shifted(inner, content_x, 0.0).prims);

    // A root's index sits in the crook of the sign, smaller than the line.
    if !index.is_empty() {
        let small = layout(index, size * 0.6);
        let lift = top - small.below - 0.02 * size;
        out.above = out.above.max(-(lift - small.above));
        out.prims.extend(shifted(small, 0.0, lift).prims);
    }
    out
}

fn accent_box(mark: Mark, body: &[Node], size: f64) -> Boxy {
    let mut inner = layout(body, size);
    // The body of an accent is one glyph in every formula these documents hold,
    // but a word keeps its mark on its first letter all the same: the mark is
    // centred over that glyph, not over the word.
    let centre = first_glyph_centre(&inner, inner.w / 2.0);
    let stroke = 0.06 * size;
    for points in mark_paths(mark, centre, inner.above + 0.04 * size, size) {
        inner.prims.push(Prim::Path {
            points,
            width: stroke,
        });
    }
    // A mark stands clear of the letter's top by its own height.
    inner.above += 0.26 * size;
    inner
}

/// The strokes an accent is drawn with, in the accent's own coordinates: `top`
/// is the height the letter below it reaches, `centre` its middle.
fn mark_paths(mark: Mark, centre: f64, top: f64, size: f64) -> Vec<Vec<(f64, f64)>> {
    let y = -top;
    let t = size;
    match mark {
        Mark::Acute => vec![vec![
            (centre - 0.10 * t, y + 0.02 * t),
            (centre + 0.05 * t, y - 0.16 * t),
        ]],
        Mark::Grave => vec![vec![
            (centre + 0.10 * t, y + 0.02 * t),
            (centre - 0.05 * t, y - 0.16 * t),
        ]],
        Mark::Hat => vec![vec![
            (centre - 0.14 * t, y + 0.04 * t),
            (centre, y - 0.10 * t),
            (centre + 0.14 * t, y + 0.04 * t),
        ]],
        Mark::Check => vec![vec![
            (centre - 0.14 * t, y - 0.04 * t),
            (centre, y + 0.10 * t),
            (centre + 0.14 * t, y - 0.04 * t),
        ]],
        Mark::Breve => vec![vec![
            (centre - 0.14 * t, y - 0.02 * t),
            (centre - 0.07 * t, y + 0.08 * t),
            (centre + 0.07 * t, y + 0.08 * t),
            (centre + 0.14 * t, y - 0.02 * t),
        ]],
        Mark::Tilde => vec![vec![
            (centre - 0.15 * t, y + 0.05 * t),
            (centre - 0.075 * t, y - 0.03 * t),
            (centre, y + 0.01 * t),
            (centre + 0.075 * t, y - 0.07 * t),
            (centre + 0.15 * t, y - 0.01 * t),
        ]],
        Mark::Bar => vec![vec![
            (centre - 0.14 * t, y - 0.04 * t),
            (centre + 0.14 * t, y - 0.04 * t),
        ]],
        Mark::Dot => vec![vec![(centre, y - 0.02 * t), (centre, y - 0.12 * t)]],
        Mark::Ddot => vec![
            vec![
                (centre - 0.08 * t, y - 0.02 * t),
                (centre - 0.08 * t, y - 0.12 * t),
            ],
            vec![
                (centre + 0.08 * t, y - 0.02 * t),
                (centre + 0.08 * t, y - 0.12 * t),
            ],
        ],
    }
}

/// Where the first glyph a box draws is centred, falling back to `default`.
fn first_glyph_centre(body: &Boxy, default: f64) -> f64 {
    for prim in &body.prims {
        if let Prim::Text { x, size, text, .. } = prim {
            if let Some(first) = text.chars().next() {
                return x + glyph_width(first) * size / 2.0;
            }
        }
    }
    default
}

fn vector_box(body: &[Node], size: f64) -> Boxy {
    let mut inner = layout(body, size);
    let centre = first_glyph_centre(&inner, inner.w / 2.0);
    let half = 0.2 * size;
    let stroke = 0.05 * size;
    let y = -(inner.above + 0.1 * size);
    inner.prims.push(Prim::Line {
        x1: centre - half,
        y1: y,
        x2: centre + half,
        y2: y,
        width: stroke,
    });
    inner.prims.push(Prim::Line {
        x1: centre + half - 0.1 * size,
        y1: y - 0.08 * size,
        x2: centre + half,
        y2: y,
        width: stroke,
    });
    inner.above = inner.above.max(-y + 0.08 * size);
    inner
}

/// A body between two delimiters, the signs stretched to its height.
fn delimited(left: char, right: char, inner: Boxy, size: f64) -> Boxy {
    if left == '\0' && right == '\0' {
        return inner;
    }
    let height = inner.above + inner.below;
    let glyph_size = (height + 0.2 * size).max(size);
    // A stretched parenthesis has to be centred on the body it closes, not
    // hung from the baseline.
    let centre = (inner.below - inner.above) / 2.0;
    let baseline = centre + 0.24 * glyph_size;
    // The advance is a little wider than the sign's ink, so a script set after
    // a closing parenthesis clears it.
    let glyph_width = 0.4 * glyph_size;
    let left_w = if left == '\0' { 0.0 } else { glyph_width };
    let right_w = if right == '\0' { 0.0 } else { glyph_width };
    let gap = if left == '\0' { 0.0 } else { 0.06 * size }
        + if right == '\0' { 0.0 } else { 0.06 * size };

    let mut out = Boxy {
        w: left_w + gap + inner.w + right_w,
        above: inner.above.max((0.72 * glyph_size - baseline).max(0.0)),
        below: inner.below.max(baseline + 0.25 * glyph_size),
        prims: Vec::new(),
    };
    if left != '\0' {
        out.prims.push(Prim::Text {
            x: 0.0,
            y: baseline,
            size: glyph_size,
            text: left.to_string(),
        });
    }
    let inner_w = inner.w;
    out.prims.extend(shifted(inner, left_w + gap, 0.0).prims);
    if right != '\0' {
        out.prims.push(Prim::Text {
            x: left_w + gap + inner_w + 0.04 * size,
            y: baseline,
            size: glyph_size,
            text: right.to_string(),
        });
    }
    out
}

/// Rows of cells on one grid: columns as wide as their widest cell, rows
/// cleared of each other, the first row on the baseline.
fn matrix(rows: &[Vec<Vec<Node>>], size: f64) -> Boxy {
    let cells: Vec<Vec<Boxy>> = rows
        .iter()
        .map(|row| row.iter().map(|cell| layout(cell, size * 0.95)).collect())
        .collect();
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    let gap_x = 0.5 * size;
    let gap_y = 0.3 * size;
    let mut widths = vec![0.0_f64; columns];
    let mut row_above = Vec::with_capacity(cells.len());
    let mut row_below = Vec::with_capacity(cells.len());
    for row in &cells {
        let mut above: f64 = 0.0;
        let mut below: f64 = 0.0;
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.w);
            above = above.max(cell.above);
            below = below.max(cell.below);
        }
        row_above.push(above);
        row_below.push(below);
    }

    let w = widths.iter().sum::<f64>() + gap_x * columns.saturating_sub(1) as f64;
    let mut prims = Vec::new();
    let mut baseline = 0.0;
    for (index, row) in cells.iter().enumerate() {
        if index > 0 {
            baseline += row_below[index - 1] + gap_y + row_above[index];
        }
        let mut x = 0.0;
        for (column, cell) in row.iter().enumerate() {
            // Cells of one row share a baseline, so a tall fraction and a
            // short letter sit on the same line.
            let dx = x + (widths[column] - cell.w) / 2.0;
            let dy = baseline;
            prims.extend(cell.prims.iter().cloned().map(|mut prim| {
                prim.shift(dx, dy);
                prim
            }));
            x += widths[column] + gap_x;
        }
    }
    let above = row_above.first().copied().unwrap_or(0.0);
    let below = baseline + row_below.last().copied().unwrap_or(0.0);
    Boxy {
        w,
        above,
        below,
        prims,
    }
}

/// How wide a run is at `size`, in the same estimate the layout uses.
fn text_width(text: &str, size: f64) -> f64 {
    text.chars().map(|c| glyph_width(c) * size).sum()
}

/// One glyph's advance, as a fraction of the font size.
///
/// These are estimates of a math face's advances, not a font's own metrics:
/// the drawing positions every run itself and gives each the width it computed
/// (`textLength`), so the estimate *is* the layout, and the picture is the same
/// in every viewer rather than as wide as whatever font it found.
fn glyph_width(c: char) -> f64 {
    match c {
        // A combining mark takes no room of its own: it is drawn over the
        // glyph before it.
        c if ('\u{300}'..='\u{36f}').contains(&c) || c == '\u{20d7}' => 0.0,
        ' ' => 0.28,
        'i' | 'j' | 'l' | '.' | ',' | '\'' | '`' | '!' | '|' | ':' | ';' => 0.26,
        'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '{' | '}' => 0.36,
        'm' | 'w' | 'M' | 'W' => 0.85,
        '∑' | '∏' | '∐' | '∫' | '∬' | '∮' => 0.9,
        '√' => 0.5,
        '+' | '−' | '-' | '=' | '<' | '>' | '±' | '∓' | '×' | '÷' | '≤' | '≥' | '≠' => {
            0.66
        }
        '→' | '←' | '⇒' | '⇐' | '↔' => 0.8,
        c if c.is_ascii_digit() => 0.5,
        c if c.is_ascii_uppercase() => 0.66,
        c if c.is_ascii_lowercase() => 0.5,
        _ => 0.62,
    }
}

/// A coordinate as the SVG writes it: two decimals, no trailing zeroes.
fn num(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    match text {
        "" | "-0" => "0".to_string(),
        text => text.to_string(),
    }
}

/// The characters that would end an XML element if written raw.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawn(latex: &str) -> String {
        svg(latex)
            .unwrap_or_else(|| panic!("{latex:?} was not drawn"))
            .0
    }

    fn texts(svg: &str) -> Vec<&str> {
        svg.match_indices("<text")
            .map(|(start, _)| {
                let close = svg[start..].find('>').expect("a text has a start tag");
                let end = svg[start..].find("</text>").expect("a text is closed");
                &svg[start + close + 1..start + end]
            })
            .collect()
    }

    /// An accent whose name the LaTeX reader refuses is exactly the case the
    /// fallback exists for: the accent is drawn on the letter, not printed as
    /// the command that asked for it. Docling's own names are all re-spelled and
    /// set (`html::tests::every_docling_accent_is_typeset`), so the drawing is
    /// reached by a name this writer does not know either.
    #[test]
    fn an_accent_the_mathml_reader_refuses_is_drawn() {
        let svg = drawn("\\macron{a}");
        assert!(svg.starts_with("<svg "), "{svg}");
        // The letter, and a chevron over it: the mark is drawn, not set from
        // whatever accent glyph the viewer's font happens to have.
        assert_eq!(texts(&svg), vec!["a"], "{svg}");
        assert_eq!(svg.matches("<path").count(), 1, "one mark: {svg}");
        assert!(!svg.contains("macron"), "{svg}");
        let baseline = texts_with_y(&svg)[0];
        let ys = path_ys(&svg);
        assert!(
            ys.iter().all(|y| *y < baseline),
            "the mark is above the letter's baseline: {svg}"
        );
        assert!(svg.ends_with("</g></svg>"), "one closed group: {svg}");
    }

    /// A mark is centred over the glyph it marks, and stands clear of it
    /// rather than on it.
    #[test]
    fn an_accent_keeps_its_mark_over_the_letter() {
        let svg = drawn("\\acute{a}");
        let xs = path_xs(&svg);
        assert_eq!(xs.len(), 2, "one stroke: {svg}");
        let middle = (xs[0] + xs[1]) / 2.0;
        // The letter `a` is drawn at x = 5 and eight units wide.
        assert!((middle - 9.0).abs() < 1.0, "centred: {svg}");
        // The drawing is not a lifted glyph: the box has room for the mark.
        let height: f64 = svg
            .split("height=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .and_then(|value| value.parse().ok())
            .expect("a height");
        assert!(height > 25.0, "room for the mark: {svg}");
    }

    /// Every coordinate in the drawing's paths, top to bottom.
    fn path_ys(svg: &str) -> Vec<f64> {
        path_points(svg).into_iter().map(|(_, y)| y).collect()
    }

    fn path_xs(svg: &str) -> Vec<f64> {
        path_points(svg).into_iter().map(|(x, _)| x).collect()
    }

    fn path_points(svg: &str) -> Vec<(f64, f64)> {
        svg.match_indices(" d=\"")
            .flat_map(|(start, _)| {
                let body = &svg[start + 4..];
                let path = &body[..body.find('"').unwrap()];
                path.split('L')
                    .map(|pair| {
                        let mut numbers = pair
                            .trim_start_matches('M')
                            .split_whitespace()
                            .map(|value| value.parse::<f64>().unwrap());
                        (numbers.next().unwrap(), numbers.next().unwrap())
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// A fraction is two halves cleared of a rule between them: the rule is
    /// drawn, which is what makes it a fraction and not two stacked words.
    #[test]
    fn a_fraction_has_a_rule_between_its_halves() {
        let svg = drawn("\\frac{a+b}{c}");
        assert_eq!(texts(&svg), vec!["a", "+", "b", "c"], "{svg}");
        assert_eq!(svg.matches("<line").count(), 1, "one rule: {svg}");
        // The numerator's baseline is above the rule, so its y is the smaller.
        let ys: Vec<f64> = texts_with_y(&svg);
        assert!(ys[0] < ys[3], "numerator sits over the denominator: {svg}");
    }

    /// A relation set straight against its operands is opened up, the way TeX
    /// sets one: `a=b` is not a single cramped word.
    #[test]
    fn a_relation_is_set_off_from_its_operands() {
        let svg = drawn("a=b");
        assert_eq!(texts(&svg), vec!["a", "=", "b"], "{svg}");
        let x = texts_with_x(&svg);
        assert!(x[1] - x[0] > 8.0, "space before the relation: {svg}");
        assert!(x[2] - x[1] > 10.56, "space after the relation: {svg}");
    }

    /// A sign that opens a run is a negative, not a subtraction: `-x` keeps its
    /// sign against the operand, and only clears the letters after it.
    #[test]
    fn a_leading_sign_opens_a_negative() {
        let svg = drawn("e^{-x}");
        assert_eq!(texts(&svg), vec!["e", "-", "x"], "{svg}");
        let x = texts_with_x(&svg);
        assert!(
            (x[1] - x[0] - 8.0).abs() < 0.01,
            "no space before the sign: {svg}"
        );
        assert!(x[2] - x[1] > 7.0, "space after the sign: {svg}");
    }

    fn texts_with_y(svg: &str) -> Vec<f64> {
        texts_with(svg, "y=\"")
    }

    fn texts_with_x(svg: &str) -> Vec<f64> {
        texts_with(svg, "x=\"")
    }

    /// `attribute` is the start of the one attribute wanted, e.g. `y="`.
    fn texts_with(svg: &str, attribute: &str) -> Vec<f64> {
        svg.match_indices("<text")
            .map(|(start, _)| {
                let tag = &svg[start..start + svg[start..].find('>').unwrap()];
                let value = tag.split(attribute).nth(1).unwrap();
                value.split('"').next().unwrap().parse().unwrap()
            })
            .collect()
    }

    /// A root's sign is drawn to the height of what is under it, so a tall
    /// radicand still has a sign that reaches over it.
    #[test]
    fn a_root_is_drawn_over_its_radicand() {
        let svg = drawn("\\sqrt{\\frac{x}{y}}");
        assert_eq!(texts(&svg), vec!["x", "y"], "{svg}");
        // The hook, the riser and the overline, plus the fraction's rule.
        assert_eq!(svg.matches("<line").count(), 4, "{svg}");
    }

    /// A script is set smaller and off the baseline: it is a superscript, not
    /// a digit beside the letter.
    #[test]
    fn a_superscript_is_smaller_and_higher() {
        let svg = drawn("x^{2}");
        assert_eq!(texts(&svg), vec!["x", "2"], "{svg}");
        let y = texts_with_y(&svg);
        assert!(y[1] < y[0], "the script is raised: {svg}");
        assert!(svg.contains("font-size=\"11.2\""), "{svg}");
    }

    /// A matrix keeps its rows and its cells, and the delimiters come from the
    /// environment's name.
    #[test]
    fn a_matrix_keeps_its_rows_and_delimiters() {
        let svg = drawn("\\begin{pmatrix}a & b \\\\ c & d\\end{pmatrix}");
        assert_eq!(texts(&svg), vec!["(", "a", "b", "c", "d", ")"], "{svg}");
        let y = texts_with_y(&svg);
        assert!(y[3] > y[1], "the second row is below the first: {svg}");
        assert!(y[4] > y[2], "the second row is below the first: {svg}");
    }

    /// A command outside the vocabulary is drawn as the TeX it is: the page
    /// says what it could not set rather than quietly dropping it.
    #[test]
    fn an_unknown_command_is_drawn_as_its_tex() {
        let svg = drawn("\\foo + 1");
        assert!(svg.contains("\\foo"), "{svg}");
    }

    /// A page of nothing but braces is skipped rather than parsed: the reader
    /// has a depth limit, so a formula cannot run the stack out.
    #[test]
    fn nesting_is_bounded() {
        let latex = format!("{}a{}", "{".repeat(300), "}".repeat(300));
        assert!(svg(&latex).is_none(), "no glyph was drawn");
    }

    /// No equation, no picture: an empty formula is left to the writer's own
    /// text fallback rather than becoming an empty image.
    #[test]
    fn an_empty_formula_is_not_drawn() {
        assert!(svg("").is_none());
        assert!(svg("   ").is_none());
        assert!(drawing("").is_none());
    }

    /// The page carries the drawing itself, as bytes it needs nothing else to
    /// read.
    #[test]
    fn the_drawing_travels_as_a_data_uri() {
        let drawing = drawing("\\ocirc{a}").expect("drawn");
        let encoded = drawing
            .uri
            .strip_prefix("data:image/svg+xml;base64,")
            .expect("an SVG data URI");
        let bytes = STANDARD.decode(encoded).expect("base64");
        assert_eq!(String::from_utf8(bytes).unwrap(), drawn("\\ocirc{a}"));
        // The picture says how far it reaches below its baseline, so an inline
        // equation can be stood on the line it is in.
        assert!(drawing.descent > 0.0, "a descent: {}", drawing.descent);
    }

    /// A `<` in a formula would end the text run it is drawn in, so the drawing
    /// escapes it like any other text. An escaped `&` is a glyph, not a cell
    /// separator.
    #[test]
    fn a_drawn_text_run_is_escaped() {
        let svg = drawn("a < b \\& c");
        assert_eq!(texts(&svg), vec!["a", "&lt;", "b", "&amp;", "c"], "{svg}");
    }
}
