//! Structural PDF parser: indirect objects, streams, page tree.

use crate::content::{ContentParser, FormXObject, TextSpan, IDENTITY, MAX_FORM_DEPTH};
use crate::encoding::{self, BaseEncoding, FontDecoder};
use crate::filters;
use crate::images::{self, PageImage};
use crate::lexer::{Lexer, SpannedToken, Token};
use crate::objects::{ObjId, PdfValue};
use std::collections::HashMap;
use std::io::Read;

/// An image XObject together with where the content stream actually put it.
///
/// The placement comes from the CTM in effect at the `Do` operator, so this is
/// the true on-page rectangle rather than an assumption that the image spans
/// the whole page.
#[derive(Debug, Clone)]
pub struct PlacedImage {
    /// Placement box `[l, t, r, b]`, top-left origin, in points.
    pub bbox: [f64; 4],
    pub image: PageImage,
}

/// A page with its extracted text spans and any embedded images.
#[derive(Debug, Clone)]
pub struct ExtractedPage {
    pub page_no: i32,
    pub width: f64,
    pub height: f64,
    pub spans: Vec<TextSpan>,
    /// Decoded image XObjects, used as the OCR source for scanned pages.
    pub images: Vec<PageImage>,
    /// The subset of `images` that a `Do` operator placed, with its box.
    pub placed_images: Vec<PlacedImage>,
    /// Figure-sized clusters of vector drawing, `[l, t, r, b]`, top-left origin.
    pub figures: Vec<[f64; 4]>,
    /// Horizontal rules — painted paths too thin to be a figure and too wide to be
    /// an underline — as `[l, t, r, b]`, top-left origin. A page draws one above a
    /// footnote, and it is what tells a note at the foot of a page from body text.
    pub rules: Vec<[f64; 4]>,
}

#[derive(Debug)]
pub struct PdfDocument {
    pub version: String,
    pub objects: HashMap<ObjId, PdfValue>,
    /// Where each stream's payload begins in the file. Recorded so that
    /// `/Length` can be honoured after the fact: the lexer decides a payload's
    /// end by searching for `endstream`, which is all it can do before the
    /// dictionary is understood, and that search is fooled by a payload that
    /// happens to contain those bytes.
    stream_offsets: HashMap<ObjId, usize>,
}

impl PdfDocument {
    pub fn parse(data: &[u8]) -> anyhow::Result<Self> {
        let mut lexer = Lexer::new(data);
        let tokens = lexer.tokenize_spanned();

        let mut version = "1.4".to_string();
        if let Some(first) = tokens.first() {
            if let Token::Header(h) = &first.token {
                version = h.trim_start_matches("%PDF-").to_string();
            }
        }

        let mut objects: HashMap<ObjId, PdfValue> = HashMap::new();
        let mut stream_offsets: HashMap<ObjId, usize> = HashMap::new();
        let mut i = 0usize;
        while i + 2 < tokens.len() {
            let (num, gen) = match (&tokens[i].token, &tokens[i + 1].token, &tokens[i + 2].token) {
                (Token::Integer(n), Token::Integer(g), Token::Obj) => (*n as usize, *g as i32),
                _ => {
                    i += 1;
                    continue;
                }
            };
            i += 3; // past "n g obj"

            let value = Self::parse_value(&tokens, &mut i, data);

            // A dictionary immediately followed by `stream` becomes a stream.
            // The lexer already skipped the opaque payload, so the Stream token's
            // span *is* the payload byte range.
            if let Some(PdfValue::Dictionary(dict)) = value.clone() {
                if i < tokens.len() && matches!(tokens[i].token, Token::Stream) {
                    let raw = data
                        .get(tokens[i].start..tokens[i].end)
                        .map(<[u8]>::to_vec)
                        .unwrap_or_default();
                    stream_offsets.insert((num, gen), tokens[i].start);
                    objects.insert((num, gen), PdfValue::Stream(dict, raw));
                    i += 1; // past Stream
                    if i < tokens.len() && matches!(tokens[i].token, Token::EndStream) {
                        i += 1;
                    }
                    // Skip to endobj, never past the next indirect object.
                    while i < tokens.len() && !matches!(tokens[i].token, Token::EndObj) {
                        if i + 2 < tokens.len()
                            && matches!(
                                (&tokens[i].token, &tokens[i + 1].token, &tokens[i + 2].token),
                                (Token::Integer(_), Token::Integer(_), Token::Obj)
                            )
                        {
                            break;
                        }
                        i += 1;
                    }
                    if i < tokens.len() && matches!(tokens[i].token, Token::EndObj) {
                        i += 1;
                    }
                    continue;
                }
            }

            if let Some(value) = value {
                objects.insert((num, gen), value);
            }

            // Skip to endobj without swallowing the next object.
            while i < tokens.len() && !matches!(tokens[i].token, Token::EndObj) {
                if i + 2 < tokens.len()
                    && matches!(
                        (&tokens[i].token, &tokens[i + 1].token, &tokens[i + 2].token),
                        (Token::Integer(_), Token::Integer(_), Token::Obj)
                    )
                {
                    break;
                }
                i += 1;
            }
            if i < tokens.len() && matches!(tokens[i].token, Token::EndObj) {
                i += 1;
            }
        }

        let mut doc = Self {
            version,
            objects,
            stream_offsets,
        };
        doc.honour_stream_lengths(data);
        doc.expand_object_streams();
        Ok(doc)
    }

    /// Replace a stream's payload with the bytes `/Length` points at.
    ///
    /// `/Length` is authoritative, but it is not trusted blindly: producers get
    /// it wrong, and it is often an indirect reference, so a declared end is
    /// only used when the byte after it really is the `endstream` terminator.
    fn honour_stream_lengths(&mut self, data: &[u8]) {
        for id in self.stream_offsets.keys().copied().collect::<Vec<_>>() {
            let offset = self.stream_offsets[&id];
            let Some((declared, found)) = self.objects.get(&id).and_then(|value| match value {
                PdfValue::Stream(dict, raw) => Some((self.declared_length(dict)?, raw.len())),
                _ => None,
            }) else {
                continue;
            };
            let declared = declared.max(0) as usize;
            if declared == found {
                continue;
            }
            let end = offset.saturating_add(declared);
            if end > data.len() || !terminator_follows(data, end) {
                continue;
            }
            tracing::debug!(
                "stream {id:?}: /Length {declared} wins over the terminator search, which found {found}"
            );
            if let Some(PdfValue::Stream(_, slot)) = self.objects.get_mut(&id) {
                *slot = data[offset..end].to_vec();
            }
        }
    }

    /// `/Length` as a byte count, following one indirect reference.
    fn declared_length(&self, dict: &HashMap<String, PdfValue>) -> Option<i64> {
        let number = |value: &PdfValue| match value {
            PdfValue::Integer(n) => Some(*n),
            PdfValue::Number(n) => Some(*n as i64),
            PdfValue::Ref(target) => match self.objects.get(target) {
                Some(PdfValue::Integer(n)) => Some(*n),
                Some(PdfValue::Number(n)) => Some(*n as i64),
                _ => None,
            },
            _ => None,
        };
        dict.get("Length").and_then(number)
    }

    /// Expand `/Type /ObjStm` object streams (PDF 1.5+).
    ///
    /// Modern producers hide most indirect objects — including the page tree —
    /// inside these streams, so without this a file can appear to have no pages
    /// at all. An object stream holds `N` pairs of `objnum offset` integers
    /// after which the object bodies follow, with offsets relative to `/First`.
    fn expand_object_streams(&mut self) {
        let stream_ids: Vec<ObjId> = self
            .objects
            .iter()
            .filter_map(|(id, value)| match value {
                PdfValue::Stream(dict, _) => match dict.get("Type") {
                    Some(PdfValue::Name(name)) if name == "ObjStm" => Some(*id),
                    _ => None,
                },
                _ => None,
            })
            .collect();

        for id in stream_ids {
            // Clone out of the map so we can insert into it afterwards.
            let (dict, raw) = match self.objects.get(&id) {
                Some(PdfValue::Stream(d, b)) => (d.clone(), b.clone()),
                _ => continue,
            };

            let count = dict.get("N").and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
            let first = dict.get("First").and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
            if count == 0 {
                continue;
            }

            let decoded = self.decode_stream(&PdfValue::Stream(dict, raw));
            let tokens = Lexer::new(&decoded).tokenize_spanned();

            // Header: `count` pairs of integers, then the object bodies.
            let mut cursor = 0usize;
            let mut pairs: Vec<(usize, usize)> = Vec::with_capacity(count);
            for _ in 0..count {
                let num = match tokens.get(cursor).map(|t| &t.token) {
                    Some(Token::Integer(n)) => *n,
                    _ => break,
                };
                cursor += 1;
                let offset = match tokens.get(cursor).map(|t| &t.token) {
                    Some(Token::Integer(n)) => *n,
                    _ => break,
                };
                cursor += 1;
                if num < 0 || offset < 0 {
                    break;
                }
                pairs.push((num as usize, offset as usize));
            }

            // Parse each body. Offsets are authoritative, so skip tokens that
            // fall before the declared start of the next object.
            for (objnum, offset) in pairs {
                let target = first + offset;
                while cursor < tokens.len() && tokens[cursor].start < target {
                    cursor += 1;
                }
                if let Some(value) = Self::parse_value(&tokens, &mut cursor, &decoded) {
                    // A direct object always wins over a compressed duplicate.
                    self.objects.entry((objnum, 0)).or_insert(value);
                }
            }
        }
    }

    fn parse_value(tokens: &[SpannedToken], i: &mut usize, data: &[u8]) -> Option<PdfValue> {
        let tok = tokens.get(*i)?.token.clone();
        let _ = data;
        match tok {
            Token::LDoubleAngle => {
                *i += 1;
                let mut dict = HashMap::new();
                while *i < tokens.len() {
                    match &tokens[*i].token {
                        Token::RDoubleAngle => {
                            *i += 1;
                            break;
                        }
                        Token::Name(key) => {
                            let key = key.clone();
                            *i += 1;
                            if let Some(v) = Self::parse_value(tokens, i, data) {
                                dict.insert(key, v);
                            }
                        }
                        _ => *i += 1,
                    }
                }
                Some(PdfValue::Dictionary(dict))
            }
            Token::LBracket => {
                *i += 1;
                let mut arr = Vec::new();
                while *i < tokens.len() {
                    if matches!(tokens[*i].token, Token::RBracket) {
                        *i += 1;
                        break;
                    }
                    match Self::parse_value(tokens, i, data) {
                        Some(v) => arr.push(v),
                        None => *i += 1,
                    }
                }
                Some(PdfValue::Array(arr))
            }
            Token::Name(n) => {
                *i += 1;
                Some(PdfValue::Name(n))
            }
            Token::String(s) => {
                *i += 1;
                Some(PdfValue::String(s))
            }
            Token::Integer(v) => {
                *i += 1;
                // Possible indirect reference: "n g R"
                if let (
                    Some(SpannedToken {
                        token: Token::Integer(g),
                        ..
                    }),
                    Some(SpannedToken {
                        token: Token::R, ..
                    }),
                ) = (tokens.get(*i), tokens.get(*i + 1))
                {
                    let gen = *g as i32;
                    *i += 2;
                    Some(PdfValue::Ref((v.max(0) as usize, gen)))
                } else {
                    Some(PdfValue::Integer(v))
                }
            }
            Token::Number(v) => {
                *i += 1;
                Some(PdfValue::Number(v))
            }
            Token::True => {
                *i += 1;
                Some(PdfValue::Boolean(true))
            }
            Token::False => {
                *i += 1;
                Some(PdfValue::Boolean(false))
            }
            Token::Null => {
                *i += 1;
                Some(PdfValue::Null)
            }
            _ => None,
        }
    }

    /// Follow indirect references (bounded) to a concrete value.
    pub fn resolve<'a>(&'a self, value: &'a PdfValue) -> Option<&'a PdfValue> {
        let mut current = value;
        for _ in 0..32 {
            match current {
                PdfValue::Ref(id) => {
                    current = self.objects.get(id)?;
                }
                other => return Some(other),
            }
        }
        None
    }

    fn collect_pages<'a>(&'a self, node: &'a PdfValue, out: &mut Vec<&'a PdfValue>, depth: usize) {
        if depth > 32 {
            return;
        }
        let dict = match self.resolve(node).and_then(|v| v.as_dict()) {
            Some(d) => d,
            None => {
                // Not a node; if it is a bare reference, resolution already failed.
                return;
            }
        };
        let node_type = dict
            .get("Type")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_name());

        if node_type == Some("Page") {
            out.push(node);
            return;
        }

        // /Pages (or an untyped intermediate) -> recurse into /Kids.
        if let Some(kids) = dict
            .get("Kids")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_array())
        {
            for kid in kids {
                self.collect_pages(kid, out, depth + 1);
            }
        }
    }

    /// Extract every page's text together with its size.
    pub fn extract(&self) -> Vec<ExtractedPage> {
        let mut page_nodes: Vec<&PdfValue> = Vec::new();

        // Prefer the trailer's /Root; fall back to scanning for /Type /Catalog.
        let mut root: Option<&PdfValue> = None;
        for v in self.objects.values() {
            if let Some(dict) = v.as_dict() {
                let is_catalog = dict
                    .get("Type")
                    .and_then(|t| self.resolve(t))
                    .and_then(|t| t.as_name())
                    == Some("Catalog");
                if is_catalog {
                    root = Some(v);
                    break;
                }
            }
        }

        if let Some(root) = root {
            if let Some(pages) = root.as_dict().and_then(|d| d.get("Pages")) {
                self.collect_pages(pages, &mut page_nodes, 0);
            }
        }

        let mut pages = Vec::new();
        for (idx, node) in page_nodes.iter().enumerate() {
            let dict = match self.resolve(node).and_then(|v| v.as_dict()) {
                Some(d) => d,
                None => continue,
            };

            // MediaBox: [x0 y0 x1 y1], possibly inherited from a parent; default US Letter.
            let (mut width, mut height) = (612.0_f64, 792.0_f64);
            if let Some(mb) = dict
                .get("MediaBox")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_array())
            {
                if mb.len() == 4 {
                    let nums: Vec<f64> = mb
                        .iter()
                        .filter_map(|v| self.resolve(v).and_then(|v| v.as_number()))
                        .collect();
                    if nums.len() == 4 {
                        width = (nums[2] - nums[0]).abs();
                        height = (nums[3] - nums[1]).abs();
                    }
                }
            }

            // Contents may be a single stream or an array of streams.
            let mut content_bytes: Vec<u8> = Vec::new();
            if let Some(contents) = dict.get("Contents") {
                let resolved = self.resolve(contents);
                match resolved {
                    Some(PdfValue::Array(items)) => {
                        for item in items {
                            if let Some(v) = self.resolve(item) {
                                content_bytes.extend(self.decode_stream(v));
                                content_bytes.push(b'\n');
                            }
                        }
                    }
                    Some(v) => {
                        content_bytes.extend(self.decode_stream(v));
                    }
                    None => {}
                }
            }

            let fonts = self.page_fonts(dict);
            let resources = self
                .inherited(dict, "Resources")
                .and_then(|v| self.resolve(v));
            let forms = self.forms_of(resources, resources, &mut Vec::new());
            let parser = ContentParser::with_height(height)
                .with_fonts(fonts)
                .with_forms(forms);
            let content = parser.extract_page(&content_bytes);

            let images = self.page_images(dict);
            // Keep a placement for every `Do` that names an image we decoded.
            let placed_images: Vec<PlacedImage> = content
                .xobjects
                .iter()
                .filter_map(|x| {
                    images.get(&x.name).map(|img| PlacedImage {
                        bbox: x.bbox,
                        image: img.clone(),
                    })
                })
                .collect();

            // A figure drawn behind a raster image is that image's clip or
            // border, not a separate picture.
            let figures = content
                .figures
                .into_iter()
                .filter(|f| !placed_images.iter().any(|p| mostly_covers(f, &p.bbox)))
                .collect();
            let rules = content.rules;

            pages.push(ExtractedPage {
                page_no: (idx + 1) as i32,
                width,
                height,
                spans: content.spans,
                images: images.into_values().collect(),
                placed_images,
                figures,
                rules,
            });
        }

        pages
    }

    /// The `/Filter` chain of a stream dictionary, as names.
    fn stream_filters(&self, dict: &HashMap<String, PdfValue>) -> Vec<String> {
        dict.get("Filter")
            .and_then(|f| self.resolve(f))
            .map(|f| match f {
                PdfValue::Name(n) => vec![n.clone()],
                PdfValue::Array(items) => items
                    .iter()
                    .filter_map(|v| {
                        self.resolve(v)
                            .and_then(|v| v.as_name().map(str::to_string))
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default()
    }

    /// Look up a key on a page, walking `/Parent` if it is inherited.
    fn inherited<'a>(
        &'a self,
        dict: &'a HashMap<String, PdfValue>,
        key: &str,
    ) -> Option<&'a PdfValue> {
        let mut current = dict;
        for _ in 0..32 {
            if let Some(v) = current.get(key) {
                return Some(v);
            }
            let parent = current.get("Parent").and_then(|p| self.resolve(p))?;
            current = parent.as_dict()?;
        }
        None
    }

    /// Build a decoder for every font in a page's `/Resources /Font`.
    fn page_fonts(&self, page_dict: &HashMap<String, PdfValue>) -> HashMap<String, FontDecoder> {
        let resources = self
            .inherited(page_dict, "Resources")
            .and_then(|v| self.resolve(v));
        self.fonts_of(resources)
    }

    /// Build a decoder for every font in a `/Resources` dictionary.
    ///
    /// A form declares its own resources, whose names are its own: the `/F1` a
    /// form's content stream selects is the form's, whatever the page's `/F1`
    /// is, so a form's fonts are decoded from its own dictionary (PDF 1.7,
    /// 8.10.1).
    fn fonts_of(&self, resources: Option<&PdfValue>) -> HashMap<String, FontDecoder> {
        let mut out = HashMap::new();
        let fonts = match resources
            .and_then(|r| r.get("Font"))
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_dict())
        {
            Some(f) => f,
            None => return out,
        };

        for (name, value) in fonts {
            let font = match self.resolve(value).and_then(|v| v.as_dict()) {
                Some(d) => d,
                None => continue,
            };
            out.insert(name.clone(), self.build_font_decoder(font));
        }
        out
    }

    /// Every form XObject a `/Resources` dictionary offers, keyed by the name
    /// its content stream uses in `Do`.
    ///
    /// A form is a page inside the page: it has its own content stream, its own
    /// resources and its own `/Matrix`, and it can place further forms. The
    /// tree is built here so the content walk can descend into one wherever it
    /// is drawn, but only while it is finite — `path` holds the ancestors'
    /// object ids so a form that places itself ends the walk instead of
    /// recursing forever, and the depth cap bounds a longer chain.
    ///
    /// A form may omit `/Resources`, in which case it uses its placer's
    /// (PDF 1.7, 8.10.1); `inherited` carries those down as the fallback.
    fn forms_of(
        &self,
        resources: Option<&PdfValue>,
        inherited: Option<&PdfValue>,
        path: &mut Vec<ObjId>,
    ) -> HashMap<String, FormXObject> {
        let mut out = HashMap::new();
        if path.len() >= MAX_FORM_DEPTH {
            return out;
        }
        let xobjects = match resources
            .and_then(|r| r.get("XObject"))
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_dict())
        {
            Some(x) => x,
            None => return out,
        };

        for (name, value) in xobjects {
            let id = match value {
                PdfValue::Ref(id) => Some(*id),
                _ => None,
            };
            if id.is_some_and(|id| path.contains(&id)) {
                continue;
            }
            let resolved = match self.resolve(value) {
                Some(v) => v,
                None => continue,
            };
            let dict = match resolved.as_dict() {
                Some(d) => d,
                None => continue,
            };
            if dict
                .get("Subtype")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_name())
                != Some("Form")
            {
                continue;
            }
            let content = self.decode_stream(resolved);
            if content.is_empty() {
                continue;
            }
            // `/Matrix` is the transform from form space into the space of
            // whoever placed it; absent, the coordinate systems coincide.
            let matrix = dict
                .get("Matrix")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| self.resolve(v).and_then(|v| v.as_number()))
                        .collect::<Vec<f64>>()
                })
                .filter(|values| values.len() == 6)
                .map(|values| {
                    [
                        values[0], values[1], values[2], values[3], values[4], values[5],
                    ]
                })
                .unwrap_or(IDENTITY);

            if let Some(id) = id {
                path.push(id);
            }
            let own = self
                .inherited(dict, "Resources")
                .and_then(|v| self.resolve(v))
                .or(inherited);
            let fonts = self.fonts_of(own);
            let nested = self.forms_of(own, inherited, path);
            if id.is_some() {
                path.pop();
            }

            out.insert(
                name.clone(),
                FormXObject {
                    content: std::sync::Arc::new(content),
                    matrix,
                    fonts: std::sync::Arc::new(fonts),
                    forms: std::sync::Arc::new(nested),
                },
            );
        }
        out
    }

    fn build_font_decoder(&self, font: &HashMap<String, PdfValue>) -> FontDecoder {
        let subtype = font
            .get("Subtype")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_name())
            .unwrap_or("");
        // Composite fonts use 2-byte codes; simple fonts use 1.
        let two_byte = subtype == "Type0";
        let mut decoder = FontDecoder::new(two_byte);

        // `/ToUnicode` is authoritative when present.
        if let Some(stream) = font.get("ToUnicode").and_then(|v| self.resolve(v)) {
            let cmap = self.decode_stream(stream);
            if !cmap.is_empty() {
                decoder.set_to_unicode(encoding::parse_to_unicode(&cmap));
            }
        }

        // `/Encoding` is either a base-encoding name or a dictionary with
        // `/BaseEncoding` and `/Differences`.
        let mut declared = false;
        match font.get("Encoding").and_then(|v| self.resolve(v)) {
            Some(PdfValue::Name(n)) => {
                if let Some(base) = BaseEncoding::from_name(n) {
                    decoder.set_base(base);
                    declared = true;
                }
            }
            Some(PdfValue::Dictionary(enc)) => {
                if let Some(base) = enc
                    .get("BaseEncoding")
                    .and_then(|v| self.resolve(v))
                    .and_then(|v| v.as_name())
                    .and_then(BaseEncoding::from_name)
                {
                    decoder.set_base(base);
                    declared = true;
                }
                if let Some(diffs) = enc
                    .get("Differences")
                    .and_then(|v| self.resolve(v))
                    .and_then(|v| v.as_array())
                {
                    let mut map = HashMap::new();
                    let mut code: i64 = 0;
                    for item in diffs {
                        match self.resolve(item) {
                            Some(PdfValue::Integer(n)) => code = *n,
                            Some(PdfValue::Name(n)) => {
                                if (0..=255).contains(&code) {
                                    map.insert(code as u8, n.clone());
                                }
                                code += 1;
                            }
                            _ => {}
                        }
                    }
                    decoder.set_differences(map);
                    declared = true;
                }
            }
            _ => {}
        }

        // A font that declares no encoding of its own is decoded by the one the
        // font program carries (PDF 1.7, 9.6.6.2), which for a TeX symbol font
        // is the only place its codes are decided at all: `cmsy` keeps
        // `\u{2217}` at code 3 and `\u{2020}`/`\u{2021}` at 121/122, and as
        // Latin-1 those are a control character and the letters `y`/`z`.
        if !declared && !two_byte {
            if let Some(encoding) = self.builtin_font_encoding(font) {
                decoder.set_builtin_encoding(encoding);
            }
        }

        decoder.set_widths(self.font_widths(font, two_byte));
        decoder
    }

    /// The encoding a Type 1 font program carries itself, when the font has one
    /// embedded. `None` for a font with no `/FontFile` (an unembedded base-14
    /// font, which the reader supplies), and for a composite font, whose codes
    /// are two bytes wide and come from its own CMap rather than a Type 1
    /// encoding array.
    fn builtin_font_encoding(
        &self,
        font: &HashMap<String, PdfValue>,
    ) -> Option<HashMap<u8, String>> {
        let descriptor = font
            .get("FontDescriptor")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_dict())?;
        let program = descriptor.get("FontFile").and_then(|v| self.resolve(v))?;
        let bytes = self.decode_stream(program);
        if bytes.is_empty() {
            return None;
        }
        let encoding = encoding::parse_type1_encoding(&bytes);
        (!encoding.is_empty()).then_some(encoding)
    }

    /// Glyph advances for a font, in 1/1000 em (PDF 1.7, 9.2.4 and 9.7.4.3).
    ///
    /// Simple fonts list `/Widths` positionally from `/FirstChar`. Composite
    /// fonts use `/W` on the descendant font, which mixes two forms; both are
    /// expanded here. An absent table stays empty so callers know to fall back
    /// to an average width instead of trusting a fabricated one.
    fn font_widths(&self, font: &HashMap<String, PdfValue>, two_byte: bool) -> HashMap<u32, f64> {
        let mut out = HashMap::new();
        let numbers = |v: &PdfValue| -> Vec<f64> {
            v.as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| self.resolve(i).and_then(|i| i.as_number()))
                        .collect()
                })
                .unwrap_or_default()
        };

        if two_byte {
            let descendant = font
                .get("DescendantFonts")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_dict());
            let Some(descendant) = descendant else {
                return out;
            };
            let Some(w) = descendant
                .get("W")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_array())
            else {
                return out;
            };
            let mut i = 0usize;
            while i < w.len() {
                let Some(first) = self.resolve(&w[i]).and_then(|v| v.as_number()) else {
                    break;
                };
                i += 1;
                if i >= w.len() {
                    break;
                }
                let next = self.resolve(&w[i]);
                match next {
                    // `c [w1 w2 ...]`: consecutive codes from `c`.
                    Some(PdfValue::Array(_)) => {
                        let widths = next.map(numbers).unwrap_or_default();
                        for (k, width) in widths.iter().enumerate() {
                            out.insert(first as u32 + k as u32, *width);
                        }
                        i += 1;
                    }
                    // `cfirst clast w`: one width for the whole range.
                    _ => {
                        let last = next.and_then(|v| v.as_number()).unwrap_or(first);
                        i += 1;
                        let width = w
                            .get(i)
                            .and_then(|v| self.resolve(v))
                            .and_then(|v| v.as_number());
                        i += 1;
                        if let Some(width) = width {
                            let last = (last as u32).min(first as u32 + 65_535);
                            for code in first as u32..=last {
                                out.insert(code, width);
                            }
                        }
                    }
                }
            }
            return out;
        }

        let first_char = font
            .get("FirstChar")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as u32;
        if let Some(widths) = font
            .get("Widths")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_array())
        {
            for (k, item) in widths.iter().enumerate() {
                if let Some(width) = self.resolve(item).and_then(|v| v.as_number()) {
                    out.insert(first_char + k as u32, width);
                }
            }
        }
        out
    }

    /// Decode every usable image XObject referenced by a page, keyed by the
    /// resource name the content stream uses in `Do`.
    fn page_images(&self, page_dict: &HashMap<String, PdfValue>) -> HashMap<String, PageImage> {
        let mut out = HashMap::new();
        let resources = match self
            .inherited(page_dict, "Resources")
            .and_then(|v| self.resolve(v))
        {
            Some(r) => r,
            None => return out,
        };
        let xobjects = match resources
            .get("XObject")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_dict())
        {
            Some(x) => x,
            None => return out,
        };
        for (name, value) in xobjects.iter() {
            let resolved = match self.resolve(value) {
                Some(v) => v,
                None => continue,
            };
            let dict = match resolved.as_dict() {
                Some(d) => d,
                None => continue,
            };
            let subtype = dict
                .get("Subtype")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_name());
            if subtype != Some("Image") {
                continue;
            }
            // Stencil masks outline glyphs rather than showing a page scan.
            let is_mask = dict
                .get("ImageMask")
                .and_then(|v| self.resolve(v))
                .map(|v| matches!(v, PdfValue::Boolean(true)))
                .unwrap_or(false);
            if is_mask {
                continue;
            }
            if let Some(img) = self.decode_image_xobject(resolved) {
                if img.is_usable() {
                    out.insert(name.clone(), img);
                }
            }
        }
        out
    }

    /// Turn an image XObject into PNG or JPEG bytes.
    fn decode_image_xobject(&self, value: &PdfValue) -> Option<PageImage> {
        let (dict, raw) = match value {
            PdfValue::Stream(d, b) => (d, b),
            _ => return None,
        };
        let number = |key: &str| -> Option<i64> {
            dict.get(key)
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_number())
                .map(|n| n as i64)
        };

        let width = number("Width")?;
        let height = number("Height")?;
        if width <= 0 || height <= 0 {
            return None;
        }
        let width = width as u32;
        let height = height as u32;

        let filters = self.stream_filters(dict);
        // Encoded formats pass straight through: the image decoder reads them.
        if filters.iter().any(|f| f == "DCTDecode") {
            return Some(PageImage {
                width,
                height,
                data: raw.clone(),
                mime: "image/jpeg",
            });
        }
        if filters.iter().any(|f| f == "JPXDecode") {
            return Some(PageImage {
                width,
                height,
                data: raw.clone(),
                mime: "image/jp2",
            });
        }

        let data = self.decode_stream(value);
        let bits = number("BitsPerComponent").unwrap_or(8).clamp(1, 16) as usize;

        let components = match dict
            .get("ColorSpace")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_name())
        {
            Some("DeviceRGB") | Some("RGB") | Some("CalRGB") => 3,
            Some("DeviceCMYK") | Some("CMYK") => 4,
            _ => 1, // DeviceGray, CalGray, Indexed, unspecified
        };

        // A `/Predictor` was already undone by `decode_stream`, at the filter
        // that produced the samples.
        let png = match (bits, components) {
            (8, 1) => images::encode_gray(width, height, &data),
            (8, 3) => images::encode_rgb(width, height, &data),
            (8, 4) => images::encode_cmyk(width, height, &data),
            (1, 1) => images::expand_1bit_gray(width, height, &data)
                .and_then(|expanded| images::encode_gray(width, height, &expanded)),
            _ => None,
        }?;
        Some(PageImage {
            width,
            height,
            data: png,
            mime: "image/png",
        })
    }

    /// Return a stream's decoded bytes, applying `/Filter /FlateDecode`.
    fn decode_stream(&self, value: &PdfValue) -> Vec<u8> {
        let (dict, raw) = match value {
            PdfValue::Stream(d, b) => (d, b),
            _ => return Vec::new(),
        };

        let filter = self.stream_filters(dict);

        // Only well-defined filters are handled. Anything else is returned raw
        // so the caller can still attempt text extraction.
        let early_change = self.early_change(dict);
        // `/DecodeParms` is positional, one entry per filter: a predictor belongs
        // to the filter that produced its input, so it is undone right after that
        // filter rather than once at the end.
        let parms = self.decode_params(dict, filter.len());
        let mut data = raw.clone();
        for (index, f) in filter.iter().enumerate() {
            match f.as_str() {
                "ASCIIHexDecode" | "AHx" => match filters::ascii_hex(&data) {
                    Some(out) => data = out,
                    None => {
                        tracing::warn!("ASCIIHexDecode failed; using raw stream bytes");
                        return raw.clone();
                    }
                },
                "ASCII85Decode" | "A85" => match filters::ascii85(&data) {
                    Some(out) => data = out,
                    None => {
                        tracing::warn!("ASCII85Decode failed; using raw stream bytes");
                        return raw.clone();
                    }
                },
                "LZWDecode" | "LZW" => match filters::lzw(&data, early_change) {
                    Some(out) => data = out,
                    None => {
                        tracing::warn!("LZWDecode failed; using raw stream bytes");
                        return raw.clone();
                    }
                },
                "RunLengthDecode" | "RL" => match filters::run_length(&data) {
                    Some(out) => data = out,
                    None => {
                        tracing::warn!("RunLengthDecode failed; using raw stream bytes");
                        return raw.clone();
                    }
                },
                "FlateDecode" | "Fl" => {
                    let mut out = Vec::new();
                    let mut decoder = flate2::read::ZlibDecoder::new(&data[..]);
                    if decoder.read_to_end(&mut out).is_ok() {
                        data = out;
                    } else {
                        // Some producers omit the zlib header; retry raw deflate.
                        let mut out2 = Vec::new();
                        let mut dec = flate2::read::DeflateDecoder::new(&data[..]);
                        if dec.read_to_end(&mut out2).is_ok() {
                            data = out2;
                        } else {
                            tracing::warn!("FlateDecode failed; using raw stream bytes");
                            return raw.clone();
                        }
                    }
                }
                other => {
                    tracing::debug!("unsupported stream filter {other}; using raw bytes");
                    return raw.clone();
                }
            }
            // A predictor is defined for LZW and Flate, so it is only undone
            // after those two.
            if matches!(f.as_str(), "FlateDecode" | "Fl" | "LZWDecode" | "LZW") {
                if let Some(parms) = parms.get(index).and_then(|p| p.as_ref()) {
                    let predictor = parms
                        .get("Predictor")
                        .and_then(|v| self.resolve(v))
                        .and_then(|v| v.as_number())
                        .unwrap_or(1.0) as i64;
                    if predictor > 1 {
                        match self.apply_predictor(&data, parms, dict, predictor) {
                            Some(out) => data = out,
                            None => {
                                tracing::warn!(
                                    "predictor could not be undone; using raw stream bytes"
                                );
                                return raw.clone();
                            }
                        }
                    }
                }
            }
        }
        data
    }

    /// `/DecodeParms` normalised to one entry per filter.
    ///
    /// A single dictionary describes a single filter; an array is positional and
    /// may hold `null` where a filter takes no parameters.
    fn decode_params(
        &self,
        dict: &HashMap<String, PdfValue>,
        filters: usize,
    ) -> Vec<Option<HashMap<String, PdfValue>>> {
        let mut out = vec![None; filters];
        let Some(parms) = dict
            .get("DecodeParms")
            .or_else(|| dict.get("DP"))
            .and_then(|p| self.resolve(p))
        else {
            return out;
        };
        match parms {
            PdfValue::Array(items) => {
                for (i, item) in items.iter().enumerate().take(filters) {
                    out[i] = self.resolve(item).and_then(|v| v.as_dict()).cloned();
                }
            }
            other => {
                if let (Some(slot), Some(d)) = (out.first_mut(), other.as_dict()) {
                    *slot = Some(d.clone());
                }
            }
        }
        out
    }

    /// Undo one filter's `/Predictor` on the bytes it produced.
    fn apply_predictor(
        &self,
        data: &[u8],
        parms: &HashMap<String, PdfValue>,
        dict: &HashMap<String, PdfValue>,
        predictor: i64,
    ) -> Option<Vec<u8>> {
        let get = |d: &HashMap<String, PdfValue>, key: &str| -> Option<i64> {
            d.get(key)
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_number())
                .map(|n| n as i64)
        };
        // Defaults from the image dictionary, which is where a predictor on a
        // raster normally gets its sample layout. A content stream has none of
        // these keys, so the specification's own defaults (1 sample, 8 bits,
        // 1 column) apply.
        let colors = get(parms, "Colors")
            .or_else(|| {
                match dict
                    .get("ColorSpace")
                    .and_then(|v| self.resolve(v))
                    .and_then(|v| v.as_name())
                {
                    Some("DeviceRGB") | Some("RGB") | Some("CalRGB") => Some(3),
                    Some("DeviceCMYK") | Some("CMYK") => Some(4),
                    _ => None,
                }
            })
            .unwrap_or(1);
        let bits = get(parms, "BitsPerComponent")
            .or_else(|| get(dict, "BitsPerComponent"))
            .unwrap_or(8);
        let columns = get(parms, "Columns")
            .or_else(|| get(dict, "Width"))
            .unwrap_or(1);
        images::apply_predictor(
            data,
            predictor,
            colors.clamp(1, 32) as usize,
            bits.clamp(1, 16) as usize,
            columns.max(1) as usize,
        )
    }

    /// `/DecodeParms /EarlyChange`, which the specification defaults to 1.
    ///
    /// It is read once for the whole stream: the parameter belongs to an LZW
    /// filter, and a chain with two of them is not something to guess about.
    fn early_change(&self, dict: &HashMap<String, PdfValue>) -> bool {
        let params = dict
            .get("DecodeParms")
            .or_else(|| dict.get("DP"))
            .and_then(|p| self.resolve(p));
        // `/DecodeParms` may be an array, one entry per filter.
        let first = match params {
            Some(PdfValue::Array(items)) => items.first().and_then(|v| self.resolve(v)),
            other => other,
        };
        first
            .and_then(|p| p.as_dict())
            .and_then(|d| d.get("EarlyChange"))
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_number())
            .map(|v| v != 0.0)
            .unwrap_or(true)
    }

    /// Convenience wrapper returning extracted text for all pages.
    pub fn extract_text(&self) -> String {
        self.extract()
            .into_iter()
            .flat_map(|p| p.spans.into_iter().map(|s| s.text))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Is the `endstream` keyword at `at`, ignoring the EOL that `/Length` does not
/// count?
fn terminator_follows(data: &[u8], at: usize) -> bool {
    let mut i = at;
    if data.get(i) == Some(&b'\r') {
        i += 1;
    }
    if data.get(i) == Some(&b'\n') {
        i += 1;
    }
    data.get(i..)
        .is_some_and(|rest| rest.starts_with(b"endstream"))
}

/// Whether box `a` covers most of box `b`.
///
/// Used to drop the rectangle a producer may stroke around an image, which is
/// otherwise indistinguishable from a figure.
fn mostly_covers(a: &[f64; 4], b: &[f64; 4]) -> bool {
    let inter_w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let inter_h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = inter_w * inter_h;
    let b_area = ((b[2] - b[0]) * (b[3] - b[1])).max(0.0);
    b_area > 0.0 && inter / b_area > 0.8
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write as _;

    /// A PNG `Up` predictor with one byte per row: every data byte is deltaed
    /// against the one before it, so a decoder that skips the predictor gets
    /// visibly wrong bytes rather than the same ones.
    fn up_predict(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() * 2);
        let mut previous = 0u8;
        for &byte in data {
            out.push(2); // filter type: Up
            out.push(byte.wrapping_sub(previous));
            previous = byte;
        }
        out
    }

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("encode");
        encoder.finish().expect("finish")
    }

    fn document() -> PdfDocument {
        PdfDocument {
            version: "1.4".to_string(),
            objects: HashMap::new(),
            stream_offsets: HashMap::new(),
        }
    }

    /// The lexer ends a payload at the first `endstream` it sees, so a payload
    /// that contains those bytes would be truncated. `/Length` says where the
    /// payload really ends, and it wins as long as the terminator agrees.
    #[test]
    fn a_declared_length_beats_a_payload_that_contains_the_terminator() {
        let payload: &[u8] = b"(hidden) endstream (more)";
        let mut file: Vec<u8> = Vec::new();
        file.extend_from_slice(b"%PDF-1.4\n");
        file.extend_from_slice(
            format!("2 0 obj\n<< /Length {} >>\nstream\n", payload.len()).as_bytes(),
        );
        file.extend_from_slice(payload);
        file.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");

        let doc = PdfDocument::parse(&file).expect("parse");
        match doc.objects.get(&(2, 0)) {
            Some(PdfValue::Stream(_, raw)) => {
                assert_eq!(raw.as_slice(), payload, "the payload was truncated")
            }
            other => panic!("expected a stream, got {other:?}"),
        }
    }

    /// `basicapi.pdf` compresses its footers with a payload whose last byte is a
    /// carriage return. The lexer strips the EOL before `endstream`, which eats
    /// that byte too, and the deflate stream then fails to inflate. `/Length`
    /// counts it as payload, which is what it is.
    #[test]
    fn a_trailing_carriage_return_belongs_to_the_payload() {
        let payload: &[u8] = b"BT (x) Tj ET\r";
        let mut file: Vec<u8> = Vec::new();
        file.extend_from_slice(b"%PDF-1.4\n");
        file.extend_from_slice(
            format!("2 0 obj\n<< /Length {} >>\nstream\n", payload.len()).as_bytes(),
        );
        file.extend_from_slice(payload);
        file.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");

        let doc = PdfDocument::parse(&file).expect("parse");
        match doc.objects.get(&(2, 0)) {
            Some(PdfValue::Stream(_, raw)) => {
                assert_eq!(raw.as_slice(), payload, "the trailing byte was dropped")
            }
            other => panic!("expected a stream, got {other:?}"),
        }
    }

    /// An indirect `/Length` is the common case in real files, and it has to be
    /// followed to the integer it points at.
    #[test]
    fn a_length_written_as_a_reference_is_followed() {
        let payload: &[u8] = b"(hidden) endstream (more)";
        let mut file: Vec<u8> = Vec::new();
        file.extend_from_slice(b"%PDF-1.4\n");
        file.extend_from_slice(b"2 0 obj\n<< /Length 3 0 R >>\nstream\n");
        file.extend_from_slice(payload);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        file.extend_from_slice(format!("3 0 obj\n{}\nendobj\n", payload.len()).as_bytes());
        file.extend_from_slice(b"%%EOF\n");

        let doc = PdfDocument::parse(&file).expect("parse");
        match doc.objects.get(&(2, 0)) {
            Some(PdfValue::Stream(_, raw)) => {
                assert_eq!(raw.as_slice(), payload, "the payload was truncated")
            }
            other => panic!("expected a stream, got {other:?}"),
        }
    }

    fn dict(pairs: &[(&str, PdfValue)]) -> HashMap<String, PdfValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// A content stream that carries a `/Predictor` is what a scanner produces
    /// when it compresses page data row by row. Undoing it at the wrong point -
    /// or not at all - leaves the operator stream unusable.
    #[test]
    fn a_predictor_on_a_content_stream_is_undone() {
        let content = b"BT /F1 12 Tf 72 700 Td (Predictor works) Tj ET\n".to_vec();
        let encoded = deflate(&up_predict(&content));
        let stream = PdfValue::Stream(
            dict(&[
                ("Filter", PdfValue::Name("FlateDecode".to_string())),
                (
                    "DecodeParms",
                    PdfValue::Dictionary(dict(&[
                        ("Predictor", PdfValue::Integer(12)),
                        ("Colors", PdfValue::Integer(1)),
                        ("BitsPerComponent", PdfValue::Integer(8)),
                        ("Columns", PdfValue::Integer(1)),
                    ])),
                ),
            ]),
            encoded,
        );
        assert_eq!(document().decode_stream(&stream), content);
    }

    /// In a filter chain the parameters are positional, so the predictor must be
    /// taken from the entry that belongs to the Flate filter, not the first one.
    #[test]
    fn a_predictor_is_read_from_its_own_filter_in_a_chain() {
        let content = b"BT (Chain) Tj ET\n".to_vec();
        // ASCIIHex over the deflated data, so the predictor's entry is the
        // second one in the array.
        let mut hex: Vec<u8> = deflate(&up_predict(&content))
            .iter()
            .flat_map(|b| format!("{b:02X}").into_bytes())
            .collect();
        hex.push(b'>');
        let stream = PdfValue::Stream(
            dict(&[
                (
                    "Filter",
                    PdfValue::Array(vec![
                        PdfValue::Name("ASCIIHexDecode".to_string()),
                        PdfValue::Name("FlateDecode".to_string()),
                    ]),
                ),
                (
                    "DecodeParms",
                    PdfValue::Array(vec![
                        PdfValue::Null,
                        PdfValue::Dictionary(dict(&[
                            ("Predictor", PdfValue::Integer(12)),
                            ("Columns", PdfValue::Integer(1)),
                        ])),
                    ]),
                ),
            ]),
            hex,
        );
        assert_eq!(document().decode_stream(&stream), content);
    }

    /// A stream with no predictor must come through untouched: `/DecodeParms`
    /// often carries only `/EarlyChange` or `/Colors`.
    #[test]
    fn decode_params_without_a_predictor_change_nothing() {
        let content = b"BT (Plain) Tj ET".to_vec();
        let stream = PdfValue::Stream(
            dict(&[
                ("Filter", PdfValue::Name("FlateDecode".to_string())),
                (
                    "DecodeParms",
                    PdfValue::Dictionary(dict(&[("Colors", PdfValue::Integer(1))])),
                ),
            ]),
            deflate(&content),
        );
        assert_eq!(document().decode_stream(&stream), content);
    }
}
