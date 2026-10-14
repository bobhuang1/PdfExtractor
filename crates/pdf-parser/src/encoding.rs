//! Text decoding for fonts (PDF 1.7, clauses 9.6-9.10).
//!
//! A string in a content stream holds *character codes*, not Unicode. Turning
//! them into text needs the font: a `/ToUnicode` CMap when present, otherwise
//! the font's `/Encoding` (a base encoding plus optional `/Differences`).
//! Without this, bytes like `0x97` (em dash) or a ligature code come out as
//! control characters.

use std::collections::HashMap;

/// Base encodings we understand. Anything else falls back to Latin-1, which is
/// correct for all of ASCII and most of the accented range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BaseEncoding {
    #[default]
    WinAnsi,
    MacRoman,
    Standard,
}

impl BaseEncoding {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "WinAnsiEncoding" => Some(Self::WinAnsi),
            "MacRomanEncoding" => Some(Self::MacRoman),
            "StandardEncoding" | "PDFDocEncoding" => Some(Self::Standard),
            _ => None,
        }
    }
}

/// cp1252 additions over Latin-1, for bytes 0x80..=0x9F. `None` means the byte
/// is undefined in cp1252.
#[rustfmt::skip]
const WINANSI_HIGH: [Option<char>; 32] = [
    Some('\u{20AC}'), None,            Some('\u{201A}'), Some('\u{0192}'),
    Some('\u{201E}'), Some('\u{2026}'),Some('\u{2020}'), Some('\u{2021}'),
    Some('\u{02C6}'), Some('\u{2030}'),Some('\u{0160}'), Some('\u{2039}'),
    Some('\u{0152}'), None,            Some('\u{017D}'), None,
    None,             Some('\u{2018}'),Some('\u{2019}'), Some('\u{201C}'),
    Some('\u{201D}'), Some('\u{2022}'),Some('\u{2013}'), Some('\u{2014}'),
    Some('\u{02DC}'), Some('\u{2122}'),Some('\u{0161}'), Some('\u{203A}'),
    Some('\u{0153}'), None,            Some('\u{017E}'), Some('\u{0178}'),
];

/// Decode one byte using a base encoding.
fn decode_base(byte: u8, base: BaseEncoding) -> char {
    match base {
        BaseEncoding::WinAnsi => {
            if (0x80..=0x9F).contains(&byte) {
                WINANSI_HIGH[(byte - 0x80) as usize].unwrap_or('\u{FFFD}')
            } else {
                byte as char // 0x00..=0x7F and 0xA0..=0xFF match Latin-1
            }
        }
        // StandardEncoding and MacRoman agree with Latin-1 across ASCII, which
        // is where the overwhelming majority of bytes in practice sit.
        BaseEncoding::Standard | BaseEncoding::MacRoman => byte as char,
    }
}

/// Map a glyph name to text. Covers single letters/digits directly, `uniXXXX`
/// names, the punctuation and ligatures that actually appear in documents, and
/// the TeX/math symbol names a Computer Modern font declares.
fn glyph_text(name: &str) -> Option<String> {
    if name.chars().count() == 1 {
        return Some(name.to_string());
    }
    if let Some(hex) = name.strip_prefix("uni") {
        if let Ok(code) = u32::from_str_radix(hex, 16) {
            return char::from_u32(code).map(|c| c.to_string());
        }
    }
    if let Some(text) = latin_glyph(name) {
        return Some(text);
    }
    math_glyph(name).map(|ch| ch.to_string())
}

/// The PostScript names whose glyph is ordinary Latin text.
fn latin_glyph(name: &str) -> Option<String> {
    let ch = match name {
        "fi" => "fi",
        "fl" => "fl",
        "ff" => "ff",
        "ffi" => "ffi",
        "ffl" => "ffl",
        "quoteright" => "\u{2019}",
        "quoteleft" => "\u{2018}",
        "quotedblleft" => "\u{201C}",
        "quotedblright" => "\u{201D}",
        "quotesinglbase" => "\u{201A}",
        "quotedblbase" => "\u{201E}",
        "endash" => "\u{2013}",
        "emdash" => "\u{2014}",
        "hyphen" => "-",
        "minus" => "\u{2212}",
        "bullet" => "\u{2022}",
        "dagger" => "\u{2020}",
        "daggerdbl" => "\u{2021}",
        "ellipsis" => "\u{2026}",
        "trademark" => "\u{2122}",
        "copyright" => "\u{00A9}",
        "registered" => "\u{00AE}",
        "degree" => "\u{00B0}",
        "section" => "\u{00A7}",
        "paragraph" => "\u{00B6}",
        "periodcentered" => "\u{00B7}",
        "germandbls" => "\u{00DF}",
        "AE" => "\u{00C6}",
        "ae" => "\u{00E6}",
        "OE" => "\u{0152}",
        "oe" => "\u{0153}",
        "Oslash" => "\u{00D8}",
        "oslash" => "\u{00F8}",
        "Thorn" => "\u{00DE}",
        "thorn" => "\u{00FE}",
        "Eth" => "\u{00D0}",
        "eth" => "\u{00F0}",
        "space" => " ",
        "nobreakspace" => "\u{00A0}",
        "sterling" => "\u{00A3}",
        "yen" => "\u{00A5}",
        "cent" => "\u{00A2}",
        "euro" => "\u{20AC}",
        "florin" => "\u{0192}",
        "perthousand" => "\u{2030}",
        "guilsinglleft" => "\u{2039}",
        "guilsinglright" => "\u{203A}",
        "guillemotleft" => "\u{00AB}",
        "guillemotright" => "\u{00BB}",
        // The names a font program uses for the plain character set.
        "suppress" => " ",
        "exclam" => "!",
        "quotedbl" => "\"",
        "numbersign" => "#",
        "dollar" => "$",
        "percent" => "%",
        "ampersand" => "&",
        "parenleft" => "(",
        "parenright" => ")",
        "asterisk" => "*",
        "plus" => "+",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        "zero" => "0",
        "one" => "1",
        "two" => "2",
        "three" => "3",
        "four" => "4",
        "five" => "5",
        "six" => "6",
        "seven" => "7",
        "eight" => "8",
        "nine" => "9",
        "colon" => ":",
        "semicolon" => ";",
        "less" => "<",
        "equal" => "=",
        "greater" => ">",
        "question" => "?",
        "at" => "@",
        "bracketleft" => "[",
        "backslash" => "\\",
        "bracketright" => "]",
        "asciicircum" => "^",
        "underscore" => "_",
        "grave" => "`",
        "braceleft" => "{",
        "bar" => "|",
        "braceright" => "}",
        "asciitilde" => "~",
        _ => return None,
    };
    Some(ch.to_string())
}

/// The glyph names a Computer Modern (TeX) font declares — the family that
/// needs this table most, because a TeX symbol font carries its encoding in the
/// font program and puts its glyphs at codes Latin-1 reads as letters
/// (`\u{2217}` at 3, `\u{2020}`/`\u{2021}` at 121/122) or as nothing at all.
fn math_glyph(name: &str) -> Option<char> {
    // An extensible delimiter is one character at a larger size
    // (`parenleftbig`, `bracketrightbt`, `braceleftBigg`): strip the size and
    // keep the character a reader sees.
    for suffix in ["Bigg", "bigg", "Big", "big", "bt", "ex", "tp"] {
        if let Some(base) = name.strip_suffix(suffix) {
            // `parenleftbig` is `(` and `radicalbigg` is `\u{221A}`: the base
            // may be a symbol name or a plain one.
            if let Some(ch) = math_glyph(base) {
                return Some(ch);
            }
            if let Some(text) = latin_glyph(base) {
                return text.chars().next();
            }
        }
    }
    let ch = match name {
        // Lower-case Greek, with the variants CMMI keeps beside the plain ones.
        "alpha" => '\u{03B1}',
        "beta" => '\u{03B2}',
        "gamma" => '\u{03B3}',
        "delta" => '\u{03B4}',
        "epsilon" => '\u{03B5}',
        "epsilon1" => '\u{03F5}',
        "zeta" => '\u{03B6}',
        "eta" => '\u{03B7}',
        "theta" => '\u{03B8}',
        "theta1" => '\u{03D1}',
        "iota" => '\u{03B9}',
        "kappa" => '\u{03BA}',
        "lambda" => '\u{03BB}',
        "mu" => '\u{03BC}',
        "nu" => '\u{03BD}',
        "xi" => '\u{03BE}',
        "omicron" => '\u{03BF}',
        "pi" => '\u{03C0}',
        "pi1" => '\u{03D6}',
        "rho" => '\u{03C1}',
        "sigma" => '\u{03C3}',
        "sigma1" => '\u{03C2}',
        "tau" => '\u{03C4}',
        "upsilon" => '\u{03C5}',
        "phi" => '\u{03C6}',
        "phi1" => '\u{03D5}',
        "chi" => '\u{03C7}',
        "psi" => '\u{03C8}',
        "omega" => '\u{03C9}',
        "Gamma" => '\u{0393}',
        "Delta" => '\u{0394}',
        "Theta" => '\u{0398}',
        "Lambda" => '\u{039B}',
        "Xi" => '\u{039E}',
        "Pi" => '\u{03A0}',
        "Sigma" => '\u{03A3}',
        "Upsilon" => '\u{03A5}',
        "Phi" => '\u{03A6}',
        "Psi" => '\u{03A8}',
        "Omega" => '\u{03A9}',
        // Operators and relations.
        "minus" => '\u{2212}',
        "plusminus" => '\u{00B1}',
        "minusplus" => '\u{2213}',
        "multiply" => '\u{00D7}',
        "divide" => '\u{00F7}',
        "asteriskmath" => '\u{2217}',
        "star" => '\u{22C6}',
        "openbullet" => '\u{25E6}',
        "periodcentered" => '\u{00B7}',
        "bullet" => '\u{2022}',
        "diamondmath" => '\u{22C4}',
        "circlemultiply" => '\u{2297}',
        "circleplus" => '\u{2295}',
        "circleminus" => '\u{2296}',
        "circledivide" => '\u{2298}',
        "circledot" => '\u{2299}',
        "circlecopyrt" => '\u{00A9}',
        "circumflex" => '^',
        "tilde" => '~',
        "macron" => '\u{00AF}',
        "dagger" => '\u{2020}',
        "daggerdbl" => '\u{2021}',
        "prime" => '\u{2032}',
        "dprime" => '\u{2033}',
        "tprime" => '\u{2034}',
        "qprime" => '\u{2057}',
        "backprime" => '\u{2035}',
        "lessequal" => '\u{2264}',
        "greaterequal" => '\u{2265}',
        "lessmuch" => '\u{226A}',
        "greatermuch" => '\u{226B}',
        "precedes" => '\u{227A}',
        "follows" => '\u{227B}',
        "negativeverymuch" => '\u{2261}',
        "equivalence" => '\u{2261}',
        "equivasymptotic" => '\u{224D}',
        "similar" => '\u{223C}',
        "similarequal" => '\u{2243}',
        "approxequal" => '\u{2248}',
        "congruent" => '\u{2245}',
        "proportional" => '\u{221D}',
        "negationslash" => '\u{2260}',
        "logicalnot" => '\u{00AC}',
        "logicaland" => '\u{2227}',
        "logicalor" => '\u{2228}',
        "element" => '\u{2208}',
        "notelement" => '\u{2209}',
        "propersubset" => '\u{2282}',
        "propersuperset" => '\u{2283}',
        "reflexsubset" => '\u{2286}',
        "reflexsuperset" => '\u{2287}',
        "union" => '\u{222A}',
        "intersection" => '\u{2229}',
        "unionmulti" => '\u{2A03}',
        "therefor" | "therefore" => '\u{2234}',
        "because" => '\u{2235}',
        "perpendicular" => '\u{22A5}',
        "latticetop" => '\u{22A4}',
        "angle" => '\u{2220}',
        "nabla" => '\u{2207}',
        "partialdiff" => '\u{2202}',
        "infinity" => '\u{221E}',
        "emptyset" => '\u{2205}',
        "hbar" => '\u{210F}',
        "aleph" => '\u{2135}',
        "weierstrass" | "wp" => '\u{2118}',
        "imath" => '\u{0131}',
        "jmath" => '\u{0237}',
        "ell" => '\u{2113}',
        "Re" => '\u{211C}',
        "Im" => '\u{2111}',
        "mho" => '\u{2127}',
        "radical" => '\u{221A}',
        "summation" | "summationdisplay" | "summationtext" => '\u{2211}',
        "product" | "productdisplay" => '\u{220F}',
        "integral" | "integraldisplay" | "integraltext" => '\u{222B}',
        "uniondisplay" => '\u{22C3}',
        "bardbl" => '\u{2016}',
        "vextenddouble" => '\u{2016}',
        "vextendsingle" => '|',
        "floorleft" => '\u{230A}',
        "floorright" => '\u{230B}',
        "ceilingleft" => '\u{2308}',
        "ceilingright" => '\u{2309}',
        // Arrows.
        "arrowleft" => '\u{2190}',
        "arrowright" => '\u{2192}',
        "arrowup" => '\u{2191}',
        "arrowdown" => '\u{2193}',
        "arrowboth" => '\u{2194}',
        "arrowdblleft" => '\u{21D0}',
        "arrowdblright" => '\u{21D2}',
        "arrowdblup" => '\u{21D1}',
        "arrowdbldown" => '\u{21D3}',
        "arrowdblboth" => '\u{21D4}',
        "mapsto" => '\u{21A6}',
        "arrownortheast" => '\u{2197}',
        "arrownorthwest" => '\u{2196}',
        "arrowsoutheast" => '\u{2198}',
        "arrowsouthwest" => '\u{2199}',
        // Card suits and music.
        "clubsuit" => '\u{2663}',
        "diamondsuit" => '\u{2666}',
        "heartsuit" => '\u{2665}',
        "spadesuit" => '\u{2660}',
        "flat" => '\u{266D}',
        "natural" => '\u{266E}',
        "sharp" => '\u{266F}',
        "checkmark" => '\u{2713}',
        _ => return None,
    };
    Some(ch)
}

/// A per-font decoder built from the font dictionary.
#[derive(Debug, Clone, Default)]
pub struct FontDecoder {
    /// True for composite (Type0) fonts, whose codes are 2 bytes wide.
    pub two_byte: bool,
    /// Explicit code -> text map from `/ToUnicode`.
    to_unicode: HashMap<u32, String>,
    base: BaseEncoding,
    /// Code -> glyph name from `/Encoding /Differences`.
    differences: HashMap<u8, String>,
    /// Code -> glyph name from the *font program's own* `/Encoding`, for a font
    /// that declares none in the PDF (PDF 1.7, 9.6.6.2). A TeX symbol font is
    /// the case that needs it: `cmsy` keeps `\u{2217}` at code 3 and
    /// `\u{2020}`/`\u{2021}` at 121/122, which as Latin-1 are a control
    /// character and the letters `y`/`z`.
    builtin: HashMap<u8, String>,
    /// True when `/ToUnicode` should be authoritative and unmapped codes are
    /// garbled rather than Latin-1.
    has_to_unicode: bool,
    /// Glyph advance per character code, in 1/1000 of the font size (PDF 1.7,
    /// 9.2.4). Empty when the font declares none, in which case callers fall
    /// back to an average width rather than inventing exactness.
    widths: HashMap<u32, f64>,
}

impl FontDecoder {
    pub fn new(two_byte: bool) -> Self {
        Self {
            two_byte,
            ..Default::default()
        }
    }

    pub fn set_base(&mut self, base: BaseEncoding) {
        self.base = base;
    }

    pub fn set_differences(&mut self, differences: HashMap<u8, String>) {
        self.differences = differences;
    }

    /// Install the encoding the font program itself carries.
    pub fn set_builtin_encoding(&mut self, encoding: HashMap<u8, String>) {
        self.builtin = encoding;
    }

    /// Whether the font program's own encoding was recovered.
    pub fn has_builtin_encoding(&self) -> bool {
        !self.builtin.is_empty()
    }

    pub fn set_to_unicode(&mut self, map: HashMap<u32, String>) {
        self.has_to_unicode = !map.is_empty();
        self.to_unicode = map;
    }

    pub fn has_to_unicode(&self) -> bool {
        self.has_to_unicode
    }

    /// Install the font's advance widths, in 1/1000 em.
    pub fn set_widths(&mut self, widths: HashMap<u32, f64>) {
        self.widths = widths;
    }

    pub fn has_widths(&self) -> bool {
        !self.widths.is_empty()
    }

    /// Total advance for a string of character codes, in em units.
    ///
    /// `None` when the font declares no widths at all: the caller then falls
    /// back to an average glyph width, which is honest about being a guess.
    /// A code the font does not list is skipped, since its real advance is
    /// unknowable and assuming the average here would silently skew the box.
    pub fn advance_em(&self, codes: &[u8]) -> Option<f64> {
        if self.widths.is_empty() {
            return None;
        }
        let mut total = 0.0;
        if self.two_byte {
            for pair in codes.chunks(2) {
                let code = if pair.len() == 2 {
                    ((pair[0] as u32) << 8) | pair[1] as u32
                } else {
                    pair[0] as u32
                };
                if let Some(w) = self.widths.get(&code) {
                    total += w / 1000.0;
                }
            }
        } else {
            for &byte in codes {
                if let Some(w) = self.widths.get(&(byte as u32)) {
                    total += w / 1000.0;
                }
            }
        }
        Some(total)
    }

    /// Decode a string of character codes into text.
    pub fn decode(&self, codes: &[u8]) -> String {
        let mut out = String::with_capacity(codes.len());
        if self.two_byte {
            // Codes are 2 bytes, big-endian. With `/ToUnicode` the mapping is
            // authoritative; without it, the widespread convention is that the
            // code equals the Unicode code point.
            for pair in codes.chunks(2) {
                let code = if pair.len() == 2 {
                    ((pair[0] as u32) << 8) | pair[1] as u32
                } else {
                    pair[0] as u32
                };
                if let Some(text) = self.to_unicode.get(&code) {
                    out.push_str(text);
                } else if !self.has_to_unicode {
                    if let Some(c) = char::from_u32(code) {
                        out.push(c);
                    }
                }
            }
        } else {
            for &byte in codes {
                if let Some(text) = self.to_unicode.get(&(byte as u32)) {
                    out.push_str(text);
                } else if let Some(name) = self.differences.get(&byte) {
                    match glyph_text(name) {
                        Some(text) => out.push_str(&text),
                        None => out.push(decode_base(byte, self.base)),
                    }
                } else if let Some(name) = self.builtin.get(&byte) {
                    match glyph_text(name) {
                        Some(text) => out.push_str(&text),
                        None => out.push(decode_base(byte, self.base)),
                    }
                } else if self.has_to_unicode {
                    // The CMap exists but omits this code; nothing sensible to show.
                } else {
                    out.push(decode_base(byte, self.base));
                }
            }
        }
        out
    }
}

/// Drop C0 control characters that survived decoding.
///
/// A symbol whose glyph name we still cannot resolve comes out as a control
/// code, and a C0 code is never meaningful text, so it is removed rather than
/// emitted. The TeX font programs' own encodings are read
/// ([`parse_type1_encoding`]), so the symbols they put at low codes — `cmsy`'s
/// `\u{2217}` at 3, for one — no longer reach here. Tabs and newlines are
/// legitimate and kept.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| !((*c as u32) < 0x20 && *c != '\n' && *c != '\t'))
        .collect()
}

/// Recover the raw bytes from a string produced by our Latin-1 content lexer.
/// The lexer maps each byte to the code point of the same value, so this is
/// exact (every char it produces is in `U+0000..=U+00FF`).
pub fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| (c as u32 & 0xFF) as u8).collect()
}

/// Recover the encoding a Type 1 font program carries itself.
///
/// The cleartext part of a Type 1 program declares its encoding as an array of
/// `dup <code> /<glyph> put` lines before the encrypted portion begins at
/// `eexec`. It is the only place a font that declares no `/Encoding` in the PDF
/// decides its codes (PDF 1.7, 9.6.6.2), and a subset font's array is the
/// *subset's* — so the codes the content stream wrote are exactly the ones to
/// look up. A position the program leaves as `.notdef` is left out, and the
/// caller falls back to the base encoding for it.
pub fn parse_type1_encoding(data: &[u8]) -> HashMap<u8, String> {
    let mut map = HashMap::new();
    let text = String::from_utf8_lossy(data);
    // Everything after `eexec` is encrypted, so only the header is readable.
    let cleartext = match text.find("eexec") {
        Some(at) => &text[..at],
        None => &text[..],
    };
    let tokens: Vec<&str> = cleartext.split_whitespace().collect();
    let mut i = 0usize;
    while i + 2 < tokens.len() {
        if tokens[i] == "dup" {
            if let Ok(code) = tokens[i + 1].parse::<u16>() {
                let name = tokens[i + 2].trim_start_matches('/');
                if code <= 255 && !name.is_empty() && name != ".notdef" {
                    map.insert(code as u8, name.to_string());
                }
            }
        }
        i += 1;
    }
    map
}

/// Parse a `/ToUnicode` CMap stream into a code -> text map.
pub fn parse_to_unicode(data: &[u8]) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let tokens = cmap_tokens(data);

    let mut i = 0usize;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "beginbfchar" => {
                i += 1;
                while i < tokens.len() && tokens[i] != "endbfchar" {
                    // <src> <dst>
                    if i + 1 >= tokens.len() {
                        break;
                    }
                    if let (Some(src), Some(dst)) =
                        (hex_token(&tokens[i]), hex_token(&tokens[i + 1]))
                    {
                        if let Some(code) = be_int(&src) {
                            let text = utf16be(&dst);
                            if !text.is_empty() {
                                map.insert(code, text);
                            }
                        }
                    }
                    i += 2;
                }
            }
            "beginbfrange" => {
                i += 1;
                while i < tokens.len() && tokens[i] != "endbfrange" {
                    if i + 2 >= tokens.len() {
                        break;
                    }
                    let lo = hex_token(&tokens[i]).and_then(|b| be_int(&b));
                    let hi = hex_token(&tokens[i + 1]).and_then(|b| be_int(&b));
                    match (lo, hi) {
                        (Some(lo), Some(hi)) if hi >= lo && hi - lo < 65536 => {
                            if tokens[i + 2] == "[" {
                                // Explicit destination list.
                                let mut j = i + 3;
                                let mut code = lo;
                                while j < tokens.len() && tokens[j] != "]" {
                                    if let Some(dst) = hex_token(&tokens[j]) {
                                        let text = utf16be(&dst);
                                        if !text.is_empty() {
                                            map.insert(code, text);
                                        }
                                    }
                                    code += 1;
                                    j += 1;
                                }
                                i = j;
                            } else if let Some(dst) = hex_token(&tokens[i + 2]) {
                                // A single destination, incremented per code.
                                let base = utf16be(&dst);
                                if !base.is_empty() {
                                    for k in 0..=(hi - lo) {
                                        map.insert(lo + k, increment_last(&base, k));
                                    }
                                }
                                i += 2;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    map
}

/// Split a CMap into tokens: hex strings, brackets, and words.
fn cmap_tokens(data: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(data);
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        match c {
            '<' => {
                // Ignore `<<`.
                if chars.peek() == Some(&'<') {
                    chars.next();
                    continue;
                }
                let mut hex = String::new();
                for d in chars.by_ref() {
                    if d == '>' {
                        break;
                    }
                    hex.push(d);
                }
                out.push(format!("<{hex}>"));
            }
            '[' | ']' => out.push(c.to_string()),
            '%' => {
                // Comment to end of line.
                for d in chars.by_ref() {
                    if d == '\n' {
                        break;
                    }
                }
            }
            _ => {
                let mut word = String::new();
                word.push(c);
                while let Some(&d) = chars.peek() {
                    if d.is_whitespace() || d == '<' || d == '[' || d == ']' || d == '%' {
                        break;
                    }
                    word.push(d);
                    chars.next();
                }
                out.push(word);
            }
        }
    }
    out
}

fn hex_token(tok: &str) -> Option<Vec<u8>> {
    let inner = tok.strip_prefix('<')?.strip_suffix('>')?;
    let cleaned: String = inner.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let mut bytes = Vec::with_capacity(cleaned.len() / 2);
    let mut it = cleaned.chars();
    while let Some(hi) = it.next() {
        let lo = it.next().unwrap_or('0');
        bytes.push(u8::from_str_radix(&format!("{hi}{lo}"), 16).ok()?);
    }
    Some(bytes)
}

/// Big-endian integer from a hex string (CMap codes are 1 or 2 bytes).
fn be_int(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 4 {
        return None;
    }
    Some(bytes.iter().fold(0u32, |acc, b| (acc << 8) | *b as u32))
}

/// Interpret hex bytes as UTF-16BE text.
fn utf16be(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        if pair.len() == 2 {
            units.push(((pair[0] as u16) << 8) | pair[1] as u16);
        } else {
            units.push(pair[0] as u16);
        }
    }
    String::from_utf16_lossy(&units)
}

/// Increment the final UTF-16 unit of `s` by `delta` (used by `bfrange`).
fn increment_last(s: &str, delta: u32) -> String {
    let mut units: Vec<u16> = s.encode_utf16().collect();
    if let Some(last) = units.last_mut() {
        *last = last.saturating_add(delta as u16);
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winansi_decodes_smart_punctuation() {
        let d = FontDecoder::new(false);
        // cp1252 0x93/0x94 are curly quotes, 0x97 an em dash.
        assert_eq!(d.decode(&[0x93, b'a', 0x94]), "\u{201C}a\u{201D}");
        assert_eq!(d.decode(b"a\x97b"), "a\u{2014}b");
        assert_eq!(d.decode(b"plain ASCII"), "plain ASCII");
    }

    #[test]
    fn differences_override_the_base_encoding() {
        let mut d = FontDecoder::new(false);
        let mut diffs = HashMap::new();
        diffs.insert(2u8, "fi".to_string());
        d.set_differences(diffs);
        // Code 2 is a non-printing control byte in cp1252, but maps to a ligature.
        assert_eq!(d.decode(&[2]), "fi");
    }

    #[test]
    fn to_unicode_handles_bfchar_and_bfrange() {
        let cmap = b"begincmap\n\
            1 beginbfchar\n<0043> <0043>\nendbfchar\n\
            1 beginbfrange\n<0061> <0063> <0061>\nendbfrange\n\
            endcmap\n";
        let map = parse_to_unicode(cmap);
        assert_eq!(map.get(&0x43).map(String::as_str), Some("C"));
        assert_eq!(map.get(&0x61).map(String::as_str), Some("a"));
        assert_eq!(map.get(&0x62).map(String::as_str), Some("b"));
        assert_eq!(map.get(&0x63).map(String::as_str), Some("c"));
    }

    #[test]
    fn to_unicode_two_byte_codes() {
        let cmap = b"1 beginbfchar\n<0002> <00660069>\nendbfchar";
        let map = parse_to_unicode(cmap);
        assert_eq!(map.get(&2).map(String::as_str), Some("fi"));

        let mut d = FontDecoder::new(true);
        d.set_to_unicode(map);
        assert_eq!(d.decode(&[0x00, 0x02]), "fi");
    }

    #[test]
    fn identity_two_byte_without_cmap_treats_codes_as_utf16() {
        let d = FontDecoder::new(true);
        // 0x0043 0x0068 0x0061 0x0070 -> "Chap"
        assert_eq!(
            d.decode(&[0x00, 0x43, 0x00, 0x68, 0x00, 0x61, 0x00, 0x70]),
            "Chap"
        );
    }

    #[test]
    fn latin_glyph_names_cover_the_plain_character_set() {
        assert_eq!(glyph_text("five").as_deref(), Some("5"));
        assert_eq!(glyph_text("colon").as_deref(), Some(":"));
        assert_eq!(glyph_text("at").as_deref(), Some("@"));
        assert_eq!(glyph_text("bar").as_deref(), Some("|"));
    }

    #[test]
    fn symbol_glyph_names_map_to_unicode() {
        assert_eq!(glyph_text("asteriskmath").as_deref(), Some("\u{2217}"));
        assert_eq!(glyph_text("dagger").as_deref(), Some("\u{2020}"));
        assert_eq!(glyph_text("daggerdbl").as_deref(), Some("\u{2021}"));
        assert_eq!(glyph_text("plusminus").as_deref(), Some("\u{00B1}"));
        assert_eq!(glyph_text("summationdisplay").as_deref(), Some("\u{2211}"));
        assert_eq!(glyph_text("epsilon1").as_deref(), Some("\u{03F5}"));
        // An extensible delimiter keeps the character a reader sees.
        assert_eq!(glyph_text("parenleftbig").as_deref(), Some("("));
        assert_eq!(glyph_text("braceleftBigg").as_deref(), Some("{"));
        assert_eq!(glyph_text("bracketrighttp").as_deref(), Some("]"));
        assert_eq!(glyph_text("radicalbigg").as_deref(), Some("\u{221A}"));
    }

    #[test]
    fn type1_builtin_encoding_recovers_a_symbol_font() {
        // The cleartext header of a `cmsy` subset, as TeX embeds it.
        let program = b"%!PS-AdobeFont-1.0: CMSY7 003.002\n\
            /FontName /ABCDEF+CMSY7 def\n\
            /Encoding 256 array\n\
            0 1 255 {1 index exch /.notdef put} for\n\
            dup 3 /asteriskmath put\n\
            dup 53 /five put\n\
            dup 121 /dagger put\n\
            dup 122 /daggerdbl put\n\
            readonly def\n\
            currentfile eexec\n\
            dup 65 /wrong put\n";
        let map = parse_type1_encoding(program);
        assert_eq!(map.get(&3).map(String::as_str), Some("asteriskmath"));
        assert_eq!(map.get(&53).map(String::as_str), Some("five"));
        assert_eq!(map.get(&121).map(String::as_str), Some("dagger"));
        assert_eq!(map.get(&122).map(String::as_str), Some("daggerdbl"));
        // Nothing after `eexec` is read, and `.notdef` positions are left out.
        assert_eq!(map.get(&65), None);

        let mut d = FontDecoder::new(false);
        d.set_builtin_encoding(map);
        assert!(d.has_builtin_encoding());
        assert_eq!(d.decode(&[3]), "\u{2217}");
        assert_eq!(d.decode(&[121, 122]), "\u{2020}\u{2021}");
        assert_eq!(d.decode(&[53]), "5");
        // A code the program does not name still falls back to the base.
        assert_eq!(d.decode(&[65]), "A");
    }

    #[test]
    fn a_declared_encoding_wins_over_the_font_programs_own() {
        let mut d = FontDecoder::new(false);
        let mut diffs = HashMap::new();
        diffs.insert(3u8, "fi".to_string());
        d.set_differences(diffs);
        let mut builtin = HashMap::new();
        builtin.insert(3u8, "asteriskmath".to_string());
        d.set_builtin_encoding(builtin);
        assert_eq!(d.decode(&[3]), "fi");
    }

    #[test]
    fn latin1_round_trip() {
        assert_eq!(latin1_bytes("a\u{0097}b"), vec![0x61, 0x97, 0x62]);
    }
}
