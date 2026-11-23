//! Structural PDF parser: indirect objects, streams, page tree.

use crate::content::{ContentParser, FormXObject, TextSpan, IDENTITY, MAX_FORM_DEPTH};
use crate::encoding::{self, BaseEncoding, FontDecoder};
use crate::filters;
use crate::images::{self, PageImage};
use crate::lexer::{Lexer, SpannedToken, Token};
use crate::objects::{ObjId, PdfValue};
use pdfextractor_limits::{Deadline, LimitError, Limits};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
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

/// One `/Link` annotation: the rectangle it is drawn over, and the URI it
/// opens — an external target as written, or a `#page=N` fragment for an
/// internal destination.
#[derive(Debug, Clone)]
pub struct PageLink {
    /// `[l, t, r, b]`, top-left origin, already turned with the page.
    pub rect: [f64; 4],
    pub uri: String,
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
    /// The page's own `/Rotate`, in degrees clockwise (0, 90, 180 or 270).
    /// Every box above is already turned to match it; the raw pixels of
    /// `images` are not — they are the bytes the file holds — which is what
    /// an OCR pass over a rotated page has to be told.
    pub rotation: i32,
    /// The page's `/Link` annotations: the box each covers and where it opens.
    pub links: Vec<PageLink>,
}

/// The largest decoded stream kept for reuse. Content streams, font programs and
/// CMaps are all far smaller; a page image is decoded once anyway.
const DECODED_CACHE_STREAM_BYTES: usize = 1 << 20;

/// The most the reused decodes may hold in total, when the run's document
/// budget allows more than this. The cache is capped at the smaller of the two:
/// it is memory held on a document's behalf, so a run that lets a document
/// expand to a few kilobytes may not keep 64 MiB of decodes for reuse. A
/// document whose streams do not fit is only slower, never wrong.
const DECODED_CACHE_BYTES: usize = 64 << 20;

#[derive(Debug)]
pub struct PdfDocument {
    pub version: String,
    /// Whether the trailer (or a cross-reference stream) names `/Encrypt`:
    /// every string and stream in the file was ciphered.
    pub encrypted: bool,
    /// Why an encrypted file could not be decrypted — a wrong or missing
    /// password, or a handler this parser does not implement. `None` for a
    /// plain file and for an encrypted one that was opened, in which case every
    /// string and stream below is already plain. While this is set, extraction
    /// refuses the file.
    pub locked: Option<crate::crypt::CryptError>,
    /// Which password opened an encrypted file (`"user password"` or
    /// `"owner password"`), for the log.
    pub opened_with: Option<&'static str>,
    pub objects: HashMap<ObjId, PdfValue>,
    /// Where each stream's payload begins in the file. Recorded so that
    /// `/Length` can be honoured after the fact: the lexer decides a payload's
    /// end by searching for `endstream`, which is all it can do before the
    /// dictionary is understood, and that search is fooled by a payload that
    /// happens to contain those bytes.
    stream_offsets: HashMap<ObjId, usize>,
    /// The catalog the newest trailer (or cross-reference stream) names as
    /// `/Root`. A file updated incrementally can carry an older catalog under
    /// another object number; the page tree is the newest one's.
    root: Option<ObjId>,
    /// The budgets this document was read under, kept for extraction.
    limits: Limits,
    /// One clock for parsing and extraction, so a file cannot spend the whole
    /// budget in each phase separately.
    deadline: Deadline,
    /// The first stream that exceeded its budget, returned by extraction.
    limit_hit: RefCell<Option<LimitError>>,
    /// Every distinct stream this document has expanded and every byte of
    /// plaintext its decryption produced, so a file of many streams each
    /// inside its own ceiling cannot expand or allocate without bound.
    document_bytes: Cell<u64>,
    /// The streams already charged to it: read again, a stream is the same
    /// bytes, not new ones.
    charged_streams: RefCell<HashSet<usize>>,
    /// The streams already decoded for this document, by payload address.
    ///
    /// Extraction asks for the same stream over and over: `math_gan.pdf` holds
    /// 57 distinct streams and inflated them 151,936 times, because TeX writes
    /// each formula as its own form XObject and every form's resource dictionary
    /// names the same font programs and CMaps. The payload buffer is the
    /// stream's identity for the document's life — the same key the whole
    /// document total uses — so a stream is inflated once and handed back.
    /// Capped per stream and in total, so reuse cannot become a memory bill.
    decoded: RefCell<HashMap<usize, Vec<u8>>>,
    decoded_bytes: Cell<usize>,
    /// How many streams this document actually inflated, so a test can hold the
    /// reuse to its promise.
    #[cfg(test)]
    decodes: Cell<usize>,
}

impl PdfDocument {
    /// Parse a PDF, opening an encrypted one with the empty user password —
    /// what every reader does with a file that asks nothing of its reader.
    pub fn parse(data: &[u8]) -> anyhow::Result<Self> {
        Self::parse_with_limits_and_password(data, Limits::default(), None)
    }

    /// Parse a PDF, opening an encrypted one with `password` (tried as the user
    /// password and then as the owner password; `None` is the empty password).
    ///
    /// `password` is the text a person gave, not bytes: it is written the way
    /// the file's revision writes passwords (see
    /// [`SecurityHandler::authenticate`]), so a non-ASCII password is not hashed
    /// as whatever bytes the terminal happened to pass.
    ///
    /// A file the password does not open is still returned, with `locked`
    /// saying why, so the caller decides how to refuse it.
    pub fn parse_with_password(data: &[u8], password: Option<&str>) -> anyhow::Result<Self> {
        Self::parse_with_limits_and_password(data, Limits::default(), password)
    }

    /// Read a PDF under an explicit budget.
    pub fn parse_with_limits(data: &[u8], limits: Limits) -> anyhow::Result<Self> {
        Self::parse_with_limits_and_password(data, limits, None)
    }

    /// Read a PDF under an explicit budget, opening an encrypted one with
    /// `password` (see [`PdfDocument::parse_with_password`]).
    pub fn parse_with_limits_and_password(
        data: &[u8],
        limits: Limits,
        password: Option<&str>,
    ) -> anyhow::Result<Self> {
        limits.check_input(data.len())?;
        let deadline = limits.deadline();
        deadline.check("reading the PDF header")?;
        let mut lexer = Lexer::new(data);
        let (tokens, truncated) = lexer.tokenize_spanned_limited(limits.max_pdf_tokens);
        limits.check_pdf_tokens(tokens.len())?;
        if truncated {
            return Err(LimitError::TooManyPdfTokens {
                actual: limits.max_pdf_tokens.saturating_add(1),
                limit: limits.max_pdf_tokens,
            }
            .into());
        }
        Self::check_nesting(&tokens, limits.max_pdf_nesting)?;
        deadline.check("tokenizing the PDF")?;

        let mut version = "1.4".to_string();
        if let Some(first) = tokens.first() {
            if let Token::Header(h) = &first.token {
                version = h.trim_start_matches("%PDF-").to_string();
            }
        }

        let mut objects: HashMap<ObjId, PdfValue> = HashMap::new();
        let mut stream_offsets: HashMap<ObjId, usize> = HashMap::new();
        // The payload bytes copied out of the file so far, charged to the
        // whole-document total before each copy is made.
        let mut stream_bytes = 0u64;
        let mut encrypted = false;
        // The last trailer that names `/Encrypt`: its `/Encrypt` and `/ID` are
        // what the security handler is authenticated against.
        let mut encrypting_trailer: Option<HashMap<String, PdfValue>> = None;
        // The `/Root` of the last classic trailer in the file: the newest.
        let mut root: Option<ObjId> = None;
        let mut i = 0usize;
        while i + 2 < tokens.len() {
            if i.is_multiple_of(4096) {
                deadline.check("reading PDF objects")?;
            }
            let (num, gen) = match (&tokens[i].token, &tokens[i + 1].token, &tokens[i + 2].token) {
                (Token::Integer(n), Token::Integer(g), Token::Obj) => (*n as usize, *g as i32),
                _ => {
                    // A trailer is a dictionary rather than an indirect object,
                    // so it never reaches `objects`; it is read where it stands.
                    // `/Encrypt` in it is how a file says that every string and
                    // stream it holds is ciphered — the one thing that has to be
                    // known before any of them is believed.
                    if matches!(tokens[i].token, Token::Trailer) {
                        let mut j = i + 1;
                        if let Some(trailer) =
                            Self::parse_value(&tokens, &mut j, data, limits.max_pdf_nesting)
                        {
                            if let Some(id) = trailer.get("Root").and_then(PdfValue::as_ref) {
                                root = Some(id);
                            }
                            if trailer.get("Encrypt").is_some() {
                                encrypted = true;
                                if let PdfValue::Dictionary(dict) = trailer {
                                    encrypting_trailer = Some(dict);
                                }
                            }
                        }
                    }
                    i += 1;
                    continue;
                }
            };
            i += 3; // past "n g obj"

            let value = Self::parse_value(&tokens, &mut i, data, limits.max_pdf_nesting);

            // A dictionary immediately followed by `stream` becomes a stream.
            // The lexer already skipped the opaque payload, so the Stream token's
            // span *is* the payload byte range.
            if let Some(PdfValue::Dictionary(dict)) = value.clone() {
                if i < tokens.len() && matches!(tokens[i].token, Token::Stream) {
                    // The payload is copied out of the file into the
                    // document's own buffer — for an encrypted file, its
                    // ciphertext — so it is charged before it exists. The
                    // document outlives `data`, so borrowing the file's bytes
                    // is not an option here.
                    let payload = data.get(tokens[i].start..tokens[i].end).unwrap_or_default();
                    stream_bytes = stream_bytes.saturating_add(payload.len() as u64);
                    if stream_bytes > limits.max_document_bytes {
                        return Err(LimitError::DocumentTooLarge {
                            actual: stream_bytes,
                            limit: limits.max_document_bytes,
                        }
                        .into());
                    }
                    let raw = payload.to_vec();
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
            limits.check_pdf_objects(objects.len())?;

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

        limits.check_pdf_objects(objects.len())?;
        let mut doc = Self {
            version,
            encrypted,
            locked: None,
            opened_with: None,
            objects,
            stream_offsets,
            root,
            limits,
            deadline,
            limit_hit: RefCell::new(None),
            document_bytes: Cell::new(stream_bytes),
            charged_streams: RefCell::new(HashSet::new()),
            decoded: RefCell::new(HashMap::new()),
            decoded_bytes: Cell::new(0),
            #[cfg(test)]
            decodes: Cell::new(0),
        };
        doc.honour_stream_lengths(data);
        doc.deadline.check("honouring stream lengths")?;
        // A PDF 1.5 file names its catalog in its cross-reference streams; the
        // one written last in the file is the newest.
        if doc.root.is_none() {
            doc.root = doc
                .stream_offsets
                .iter()
                .filter_map(|(id, offset)| Some((*offset, doc.objects.get(id)?.as_dict()?)))
                .filter(|(_, dict)| dict.contains_key("Root"))
                .max_by_key(|(offset, _)| *offset)
                .and_then(|(_, dict)| dict.get("Root").and_then(PdfValue::as_ref));
        }

        // A PDF 1.5 file states its encryption in the cross-reference stream's
        // own dictionary, which is an object and has no classic trailer to say
        // it in. Object streams are themselves ciphered, so this — and the
        // decryption — happens before they are expanded.
        //
        // The entry counts on its own: a stream dictionary that names
        // `/Encrypt` is a trailer, and nothing else about it is required. `/ID`
        // is what a handler authenticates against, not what makes the bytes
        // ciphertext, and a writer that leaves out `/Type /XRef` or the `/W`
        // and `/Size` entries has still marked every string and stream as
        // ciphered: asking for either would read ciphertext as text when it is
        // missing, which is the fail-open this refuses. As with the classic
        // trailer, the newest such stream wins.
        if encrypting_trailer.is_none() {
            encrypting_trailer = doc
                .stream_offsets
                .iter()
                .filter_map(|(id, offset)| Some((*offset, doc.objects.get(id)?.as_dict()?)))
                .filter(|(_, dict)| dict.contains_key("Encrypt"))
                .max_by_key(|(offset, _)| *offset)
                .map(|(_, dict)| dict.clone());
            doc.encrypted |= encrypting_trailer.is_some();
        }
        if let Some(trailer) = encrypting_trailer {
            doc.decrypt(&trailer, password);
            // Decryption charges its plaintext before allocating it; that
            // refusal has to be reported before object streams are expanded on
            // bytes that were never decrypted.
            if let Some(error) = doc.limit_hit.take() {
                return Err(error.into());
            }
        }

        doc.expand_object_streams()?;
        doc.deadline.check("expanding object streams")?;
        if let Some(error) = doc.limit_hit.take() {
            return Err(error.into());
        }
        Ok(doc)
    }

    /// Authenticate the password against the trailer's `/Encrypt` and decrypt
    /// every object in place, or record in `locked` why that was not possible.
    fn decrypt(&mut self, trailer: &HashMap<String, PdfValue>, password: Option<&str>) {
        let encrypt_ref = trailer.get("Encrypt").and_then(PdfValue::as_ref);
        let encrypt = match trailer.get("Encrypt").and_then(|v| self.resolve(v)) {
            Some(PdfValue::Dictionary(dict)) => dict.clone(),
            _ => {
                self.locked = Some(crate::crypt::CryptError::Unsupported(
                    "/Encrypt names no dictionary".into(),
                ));
                return;
            }
        };
        let id0 = match trailer.get("ID") {
            Some(PdfValue::Array(ids)) => match ids.first() {
                Some(PdfValue::String(s)) => crate::crypt::latin1_bytes(s),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let handler = match crate::crypt::SecurityHandler::authenticate(&encrypt, &id0, password) {
            Ok(handler) => handler,
            Err(error) => {
                self.locked = Some(error);
                return;
            }
        };
        self.opened_with = Some(handler.opened_with);
        // Decryption rebuilds the document's strings in fresh buffers; a
        // stream's payload is decrypted where it stands, in the buffer the
        // parser copied out of the file and charged then. Charge the strings
        // before any is made, the way every decoded stream is charged.
        let plaintext = self
            .objects
            .iter()
            .filter(|(id, _)| Some(**id) != encrypt_ref)
            .fold(0u64, |total, (_, value)| {
                total.saturating_add(handler.plaintext_len(value))
            });
        self.charge_document(plaintext, None);
        if self.limit_hit.borrow().is_some() {
            return;
        }
        for (&(num, gen), value) in self.objects.iter_mut() {
            // The `/Encrypt` dictionary's own strings (`/O`, `/U`) are never
            // ciphered.
            if Some((num, gen)) == encrypt_ref {
                continue;
            }
            handler.decrypt_object(num, gen, value);
        }
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
            // The payload's own buffer is reused, so a longer declared
            // `/Length` allocates only the growth: charge it before it is made.
            self.charge_document((end - offset).saturating_sub(found) as u64, None);
            if self.limit_hit.borrow().is_some() {
                continue;
            }
            if let Some(PdfValue::Stream(_, slot)) = self.objects.get_mut(&id) {
                slot.clear();
                slot.extend_from_slice(&data[offset..end]);
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
    fn expand_object_streams(&mut self) -> anyhow::Result<()> {
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
            self.deadline.check("expanding an object stream")?;
            if self.limit_hit.borrow().is_some() {
                break;
            }
            // Read out of the store rather than cloned: decoding borrows the
            // payload, and nothing writes to the store until it has been read.
            let Some(stream @ PdfValue::Stream(dict, _)) = self.objects.get(&id) else {
                continue;
            };

            let count = dict.get("N").and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
            let first = dict.get("First").and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
            if count == 0 {
                continue;
            }

            // The decoded bytes are this expansion's own buffer, so they are
            // charged as the work they are and never deduped against an address
            // a later stream could inherit.
            let decoded = self.decode_stream_uncapped(stream);
            self.charge_document(decoded.len() as u64, None);
            let (tokens, truncated) =
                Lexer::new(&decoded).tokenize_spanned_limited(self.limits.max_pdf_tokens);
            if truncated {
                return Err(LimitError::TooManyPdfTokens {
                    actual: self.limits.max_pdf_tokens.saturating_add(1),
                    limit: self.limits.max_pdf_tokens,
                }
                .into());
            }
            Self::check_nesting(&tokens, self.limits.max_pdf_nesting)?;

            // A stream cannot promise more objects than the document budget
            // has left; the capacity is allocated only after that check.
            let remaining = self
                .limits
                .max_pdf_objects
                .saturating_sub(self.objects.len());
            if count > remaining {
                return Err(LimitError::TooManyPdfObjects {
                    actual: self.objects.len().saturating_add(count),
                    limit: self.limits.max_pdf_objects,
                }
                .into());
            }

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
                let target = first.saturating_add(offset);
                while cursor < tokens.len() && tokens[cursor].start < target {
                    cursor += 1;
                }
                if let Some(value) =
                    Self::parse_value(&tokens, &mut cursor, &decoded, self.limits.max_pdf_nesting)
                {
                    // A direct object always wins over a compressed duplicate.
                    self.objects.entry((objnum, 0)).or_insert(value);
                }
                self.limits.check_pdf_objects(self.objects.len())?;
            }
        }
        Ok(())
    }

    /// Refuse a token stream that nests arrays or dictionaries deeper than `max`.
    ///
    /// The budget has to be held here, at the tokens, rather than only where a
    /// value is parsed: `parse_value` gives up on a value past the ceiling by
    /// returning `None`, which drops it silently, so a document that nested too
    /// deep would be read as a shorter one instead of being refused.
    fn check_nesting(tokens: &[SpannedToken], max: usize) -> Result<(), LimitError> {
        let mut depth = 0usize;
        for token in tokens {
            match token.token {
                Token::LBracket | Token::LDoubleAngle => {
                    depth += 1;
                    if depth > max {
                        return Err(LimitError::PdfTooDeep {
                            actual: depth,
                            limit: max,
                        });
                    }
                }
                Token::RBracket | Token::RDoubleAngle => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        Ok(())
    }

    fn parse_value(
        tokens: &[SpannedToken],
        i: &mut usize,
        data: &[u8],
        max_depth: usize,
    ) -> Option<PdfValue> {
        Self::parse_value_at(tokens, i, data, max_depth, 0)
    }

    fn parse_value_at(
        tokens: &[SpannedToken],
        i: &mut usize,
        data: &[u8],
        max_depth: usize,
        depth: usize,
    ) -> Option<PdfValue> {
        if depth > max_depth {
            return None;
        }
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
                            if let Some(v) =
                                Self::parse_value_at(tokens, i, data, max_depth, depth + 1)
                            {
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
                    match Self::parse_value_at(tokens, i, data, max_depth, depth + 1) {
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

    fn collect_pages<'a>(
        &'a self,
        node: &'a PdfValue,
        out: &mut Vec<&'a PdfValue>,
        depth: usize,
    ) -> Result<(), LimitError> {
        if out.len().is_multiple_of(1024) {
            self.deadline.check("walking the PDF page tree")?;
        }
        if depth > self.limits.max_pdf_nesting {
            return Err(LimitError::PdfTooDeep {
                actual: depth,
                limit: self.limits.max_pdf_nesting,
            });
        }
        let dict = match self.resolve(node).and_then(|v| v.as_dict()) {
            Some(d) => d,
            None => {
                // Not a node; if it is a bare reference, resolution already failed.
                return Ok(());
            }
        };
        let node_type = dict
            .get("Type")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_name());

        if node_type == Some("Page") {
            if out.len() >= self.limits.max_pages {
                return Err(LimitError::TooManyPages {
                    actual: out.len() + 1,
                    limit: self.limits.max_pages,
                });
            }
            out.push(node);
            return Ok(());
        }

        // /Pages (or an untyped intermediate) -> recurse into /Kids.
        if let Some(kids) = dict
            .get("Kids")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_array())
        {
            for kid in kids {
                self.collect_pages(kid, out, depth + 1)?;
            }
        }
        Ok(())
    }

    /// The page's `/Link` annotations.
    ///
    /// `/A` `/S` `/URI` gives an external target as written; a `/Dest` (or a
    /// `/GoTo` action's `/D`) is an internal jump, kept as the `#page=N`
    /// fragment a reader can follow. A *named* destination cannot be resolved
    /// without the document's name tree and is left out rather than guessed
    /// at. Rectangles are flipped out of PDF's bottom-left origin here; the
    /// caller turns them with the page's rotation, as it does a figure.
    fn page_links(
        &self,
        page: &HashMap<String, PdfValue>,
        page_nodes: &[&PdfValue],
        page_height: f64,
    ) -> Vec<PageLink> {
        let Some(annots) = page
            .get("Annots")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_array())
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in annots {
            let Some(annot) = self.resolve(entry) else {
                continue;
            };
            if annot
                .get("Subtype")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_name())
                != Some("Link")
            {
                continue;
            }
            // Flag bit 2 is “hidden”: a link that is not drawn is not a link a
            // reader can see or press, and its rectangle covers text that
            // does not go anywhere.
            let hidden = annot
                .get("F")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_number())
                .is_some_and(|flags| (flags as i64) & 2 != 0);
            if hidden {
                continue;
            }
            let Some(uri) = self.link_uri(annot, page_nodes) else {
                continue;
            };
            let Some(rect) = self.link_rect(annot, page_height) else {
                continue;
            };
            out.push(PageLink { rect, uri });
        }
        out
    }

    /// Where a link annotation points.
    fn link_uri(&self, annot: &PdfValue, page_nodes: &[&PdfValue]) -> Option<String> {
        if let Some(action) = annot.get("A").and_then(|v| self.resolve(v)) {
            match action
                .get("S")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_name())
            {
                Some("URI") => {
                    return match action.get("URI").and_then(|v| self.resolve(v)) {
                        Some(PdfValue::String(uri)) => Some(uri.clone()),
                        _ => None,
                    };
                }
                Some("GoTo") => {
                    return action
                        .get("D")
                        .and_then(|v| self.resolve(v))
                        .and_then(|dest| self.dest_page(dest, page_nodes))
                        .map(|page| format!("#page={page}"));
                }
                // Any other action (a named action, a launch, a script) does
                // not open a URI, and inventing one would fabricate a target.
                _ => return None,
            }
        }
        // No `/A`: some producers put the destination on the annotation itself.
        annot
            .get("Dest")
            .and_then(|v| self.resolve(v))
            .and_then(|dest| self.dest_page(dest, page_nodes))
            .map(|page| format!("#page={page}"))
    }

    /// The page number (1-based) a destination names.
    ///
    /// An array destination opens with the page it points at — usually a
    /// reference, occasionally a bare name or a string a name tree would
    /// resolve; the latter two are left for [`Self::dest_page`] to refuse.
    fn dest_page(&self, dest: &PdfValue, page_nodes: &[&PdfValue]) -> Option<i32> {
        let target = dest.as_array()?.first()?;
        let resolved = self.resolve(target)?;
        for (index, node) in page_nodes.iter().enumerate() {
            let Some(node) = self.resolve(node) else {
                continue;
            };
            if std::ptr::eq(node, resolved) || node == resolved {
                return Some((index + 1) as i32);
            }
        }
        None
    }

    /// A link annotation's `/Rect`, flipped into top-left origin.
    fn link_rect(&self, annot: &PdfValue, page_height: f64) -> Option<[f64; 4]> {
        let coords: Vec<f64> = annot
            .get("Rect")
            .and_then(|v| self.resolve(v))
            .and_then(|v| v.as_array())?
            .iter()
            .filter_map(|v| self.resolve(v).and_then(|v| v.as_number()))
            .collect();
        if coords.len() != 4 {
            return None;
        }
        let (x0, y0, x1, y1) = (coords[0], coords[1], coords[2], coords[3]);
        Some([
            x0.min(x1),
            page_height - y0.max(y1),
            x0.max(x1),
            page_height - y0.min(y1),
        ])
    }

    /// Extract every page's text together with its size.
    ///
    /// This is the un-fallible facade kept for existing callers. A document
    /// that exceeded a budget, or that was never decrypted, yields no pages
    /// here; a caller that has to report the refusal uses
    /// [`Self::extract_checked`].
    pub fn extract(&self) -> Vec<ExtractedPage> {
        match self.extract_checked() {
            Ok(pages) => pages,
            Err(error) => {
                tracing::warn!("PDF extraction refused: {error}");
                Vec::new()
            }
        }
    }

    /// Extract every page, reporting a resource limit instead of silently
    /// returning a truncated document.
    ///
    /// The reader fails closed on a document the security handler did not
    /// open: it still parses, but every string and stream in it is ciphertext,
    /// so pages read from it would be noise shaped like a document.
    pub fn extract_checked(&self) -> anyhow::Result<Vec<ExtractedPage>> {
        // A file that says it is encrypted must have been opened before any of
        // it is read: `locked` names why it was not, and `encrypted` with no
        // `opened_with` is a handler that never ran at all. Either way its
        // pages are ciphertext, not the document it promises.
        if self.encrypted && self.opened_with.is_none() {
            let reason =
                "refusing to read a PDF that was never decrypted: its pages are still ciphertext";
            return Err(match self.locked.clone() {
                Some(locked) => anyhow::Error::new(locked).context(reason),
                None => anyhow::Error::msg(reason),
            });
        }
        if let Some(error) = self.limit_hit.take() {
            return Err(error.into());
        }
        self.deadline.check("starting page extraction")?;
        let mut page_nodes: Vec<&PdfValue> = Vec::new();

        // Prefer the trailer's /Root. A file whose trailer is lost or names no
        // catalog — a damaged or truncated one — falls back to scanning for
        // `/Type /Catalog`, taking the highest object number so that the choice
        // is the same on every run (and is the newer catalog when an update
        // added one).
        let is_catalog = |value: &PdfValue| {
            value.as_dict().is_some_and(|dict| {
                dict.get("Type")
                    .and_then(|t| self.resolve(t))
                    .and_then(|t| t.as_name())
                    == Some("Catalog")
            })
        };
        let named = self
            .root
            .and_then(|id| self.objects.get(&id))
            .filter(|value| is_catalog(value));
        let root: Option<&PdfValue> = named.or_else(|| {
            self.objects
                .iter()
                .filter(|(_, value)| is_catalog(value))
                .max_by_key(|(id, _)| **id)
                .map(|(_, value)| value)
        });

        if let Some(root) = root {
            if let Some(pages) = root.as_dict().and_then(|d| d.get("Pages")) {
                self.collect_pages(pages, &mut page_nodes, 0)?;
            }
        }
        self.limits.check_pages(page_nodes.len())?;

        let mut pages = Vec::new();
        for (idx, node) in page_nodes.iter().enumerate() {
            if idx % 16 == 0 {
                self.deadline.check("extracting PDF pages")?;
            }
            if pages.len() >= self.limits.max_pages {
                return Err(LimitError::TooManyPages {
                    actual: pages.len() + 1,
                    limit: self.limits.max_pages,
                }
                .into());
            }
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
                        for (index, item) in items.iter().enumerate() {
                            if index.is_multiple_of(1024) {
                                self.deadline.check("reading a page's content streams")?;
                            }
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
            self.cap_stream(&mut content_bytes);

            // `/Rotate` turns the page clockwise before the viewer shows it
            // (PDF 1.7, 7.7.3) and is inheritable like `/MediaBox`. The
            // content stream's coordinates are unrotated, so the page is
            // turned here — once — and everything downstream sees the page
            // as displayed.
            let rotation = self
                .inherited(dict, "Rotate")
                .and_then(|v| self.resolve(v))
                .and_then(|v| v.as_number())
                .map(|degrees| {
                    let snapped = (degrees.round() as i32).rem_euclid(360);
                    // The spec requires a multiple of 90; anything else is
                    // ignored rather than guessed at.
                    if snapped % 90 == 0 {
                        snapped
                    } else {
                        0
                    }
                })
                .unwrap_or(0);

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

            // A `/Link` annotation is a fact about the page, not about its
            // content stream, so it is read from the page dict and flipped out
            // of PDF's bottom-left origin here.
            let mut links = self.page_links(dict, &page_nodes, height);

            let mut spans = content.spans;
            let mut placed_images = placed_images;
            let mut figures: Vec<[f64; 4]> = figures;
            let mut rules = rules;
            if rotation != 0 {
                for span in &mut spans {
                    span.bbox = turn_box(span.bbox, rotation, width, height);
                    // The run no longer reads along the page's own reading
                    // direction — it is sideways text now, and layout keeps
                    // such runs out of the horizontal lines it builds.
                    span.rotated = true;
                }
                for placed in &mut placed_images {
                    placed.bbox = turn_box(placed.bbox, rotation, width, height);
                }
                for bbox in figures.iter_mut().chain(rules.iter_mut()) {
                    *bbox = turn_box(*bbox, rotation, width, height);
                }
                for link in &mut links {
                    link.rect = turn_box(link.rect, rotation, width, height);
                }
                if rotation == 90 || rotation == 270 {
                    std::mem::swap(&mut width, &mut height);
                }
            }

            pages.push(ExtractedPage {
                page_no: (idx + 1) as i32,
                width,
                height,
                spans,
                images: images.into_values().collect(),
                placed_images,
                figures,
                rules,
                rotation,
                links,
            });
        }

        if let Some(error) = self.limit_hit.take() {
            return Err(error.into());
        }
        Ok(pages)
    }

    /// Refuse a decoded byte sequence that grew past its budget.
    ///
    /// The first refusal is kept for the caller; the bytes are still truncated
    /// here so a decoder that a caller ignored cannot keep the allocation.
    fn cap_stream(&self, data: &mut Vec<u8>) {
        let limit = self.limits.max_decoded_stream_bytes;
        if data.len() as u64 > limit {
            if self.limit_hit.borrow().is_none() {
                *self.limit_hit.borrow_mut() = Some(LimitError::DecodedStreamTooLarge {
                    actual: data.len() as u64,
                    limit,
                });
            }
            data.truncate(limit.min(usize::MAX as u64) as usize);
        }
    }

    /// Charge one decoded item to the whole-document total.
    ///
    /// Called once per stream handed to a caller, not at each filter step: a
    /// Flate stream passes through [`Self::cap_stream`] three times as it is
    /// inflated, and charging each pass would count one stream three times.
    ///
    /// `key` names the stream the bytes belong to, so extraction walking the
    /// same page's content streams again is one expansion charged once — that
    /// repeated walk is what the total exists to bound. `None` charges the
    /// bytes and remembers nothing: it is for work that is rebuilt for a single
    /// decode, which is not the same stream read twice and must not be free the
    /// next time an address comes round — for the string buffers decryption
    /// rebuilds and the payload a declared `/Length` regrows, each charged
    /// before it is allocated.
    fn charge_document(&self, bytes: u64, key: Option<usize>) {
        if let Some(key) = key {
            if !self.charged_streams.borrow_mut().insert(key) {
                return;
            }
        }
        let total = self.document_bytes.get().saturating_add(bytes);
        self.document_bytes.set(total);
        if total > self.limits.max_document_bytes && self.limit_hit.borrow().is_none() {
            *self.limit_hit.borrow_mut() = Some(LimitError::DocumentTooLarge {
                actual: total,
                limit: self.limits.max_document_bytes,
            });
        }
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
        if self.limits.check_image(width, height).is_err() {
            tracing::warn!("an image XObject is {width}x{height}; leaving it out");
            return None;
        }

        let filters = self.stream_filters(dict);
        let parms = self.decode_params(dict, filters.len());

        // CCITT and JBIG2 are bi-level codecs, not encodings an image reader
        // can open. Decode them here so a scanned page reaches OCR as pixels.
        if let Some(index) = filters
            .iter()
            .position(|f| f == "CCITTFaxDecode" || f == "CCF")
        {
            let params = parms.get(index).and_then(|p| p.as_ref());
            let number = |key: &str| -> Option<i64> {
                params
                    .and_then(|p| p.get(key))
                    .and_then(|v| self.resolve(v))
                    .and_then(|v| v.as_number())
                    .map(|n| n as i64)
            };
            let flag = |key: &str, default: bool| -> bool {
                params
                    .and_then(|p| p.get(key))
                    .and_then(|v| self.resolve(v))
                    .map(|v| matches!(v, PdfValue::Boolean(true)))
                    .unwrap_or(default)
            };
            let columns = number("Columns")
                .filter(|n| *n > 0)
                .map(|n| n as u32)
                .unwrap_or(width);
            if self.limits.check_image(columns, height).is_err() {
                tracing::warn!("a CCITT image is {columns}x{height}; leaving it out");
                return None;
            }
            let settings = images::CcittParams {
                k: number("K").unwrap_or(0),
                end_of_line: flag("EndOfLine", false),
                end_of_block: flag("EndOfBlock", true),
                encoded_byte_align: flag("EncodedByteAlign", false),
            };
            let data = self.decode_stream_before(value, index);
            match images::decode_ccitt_limited(
                &data,
                columns,
                height,
                &settings,
                self.limits.max_image_pixels,
            )
            .and_then(|samples| images::encode_gray(columns, height, &samples))
            {
                Some(png) => {
                    return Some(PageImage {
                        width: columns,
                        height,
                        data: png,
                        mime: "image/png",
                    })
                }
                None => {
                    tracing::debug!("a CCITT image did not decode; leaving it out");
                    return None;
                }
            }
        }

        if let Some(index) = filters.iter().position(|f| f == "JBIG2Decode") {
            let params = parms.get(index).and_then(|p| p.as_ref());
            let globals = params
                .and_then(|p| p.get("JBIG2Globals"))
                .and_then(|v| self.resolve(v))
                .map(|v| self.decode_stream(v));
            let data = self.decode_stream_before(value, index);
            match images::decode_jbig2_limited(
                &data,
                globals.as_deref(),
                self.limits.max_image_pixels,
            )
            .and_then(|(w, h, samples)| images::encode_gray(w, h, &samples).map(|png| (w, h, png)))
            {
                Some((width, height, png)) => {
                    return Some(PageImage {
                        width,
                        height,
                        data: png,
                        mime: "image/png",
                    })
                }
                None => {
                    tracing::debug!("a JBIG2 image did not decode; leaving it out");
                    return None;
                }
            }
        }

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
            // A JPEG 2000 is decoded rather than passed on. Docling cannot name
            // `image/jp2` in an image reference, so a page whose picture is one
            // used to come out as the box with no image that started this line
            // of work; the pixels are the codestream's own — `/Width` and
            // `/Height` are only what the file says they are — and the picture
            // leaves as a PNG. The stream may be either of the two things a
            // `.jp2` file is made of, the container with its boxes or the bare
            // codestream, and the image reader takes both.
            match pdfextractor_image_parser::transcode_to_png_checked(raw, &self.limits).ok() {
                Some(picture) => {
                    return Some(PageImage {
                        width: picture.width,
                        height: picture.height,
                        data: picture.data,
                        mime: "image/png",
                    })
                }
                // A codestream this build cannot decode is still a picture the
                // page drew; its own bytes are carried rather than dropping it.
                None => {
                    tracing::debug!("a JPEG 2000 image did not decode; carrying it as stored");
                    return Some(PageImage {
                        width,
                        height,
                        data: raw.clone(),
                        mime: "image/jp2",
                    });
                }
            }
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

    /// The stream's bytes after the filters before its terminal image codec.
    ///
    /// A CCITT or JBIG2 stream may be wrapped in an ASCII or Flate filter. The
    /// image decoder needs the codec's own bytes, so the prefix is built as a
    /// stream of its own and passed through the ordinary filter chain. Leaving
    /// the terminal filter out is what makes this different from
    /// [`Self::decode_stream`], which would stop at the unknown codec and return
    /// the encoded bytes untouched.
    fn decode_stream_before(&self, value: &PdfValue, terminal_index: usize) -> Vec<u8> {
        let (dict, raw) = match value {
            PdfValue::Stream(d, b) => (d, b),
            _ => return Vec::new(),
        };
        if terminal_index == 0 {
            let mut data = raw.clone();
            self.cap_stream(&mut data);
            self.charge_document(data.len() as u64, Self::stream_key(value));
            return data;
        }

        let filters = self.stream_filters(dict);
        let parms = self.decode_params(dict, filters.len());
        let mut prefix = dict.clone();
        prefix.insert(
            "Filter".to_string(),
            PdfValue::Array(
                filters
                    .iter()
                    .take(terminal_index)
                    .cloned()
                    .map(PdfValue::Name)
                    .collect(),
            ),
        );
        if parms.iter().take(terminal_index).any(Option::is_some) {
            prefix.insert(
                "DecodeParms".to_string(),
                PdfValue::Array(
                    parms
                        .iter()
                        .take(terminal_index)
                        .map(|p| {
                            p.clone()
                                .map(PdfValue::Dictionary)
                                .unwrap_or(PdfValue::Null)
                        })
                        .collect(),
                ),
            );
        }
        prefix.remove("DP");
        // Decoded through the prefix but charged to the stream it came from:
        // the same image placed on a hundred pages is one expansion, which is
        // why the key is `value` and not the temporary built here.
        let data = self.decode_stream_uncapped(&PdfValue::Stream(prefix, raw.clone()));
        self.charge_document(data.len() as u64, Self::stream_key(value));
        data
    }

    /// Return a stream's decoded bytes, applying `/Filter /FlateDecode`.
    ///
    /// The bytes are charged to the document total once, when the stream is
    /// handed back: the filter chain caps each intermediate buffer but a single
    /// stream is one item, however many passes it took. A stream decoded before
    /// is served from the reuse cache, which is why walking a page of forms that
    /// all name the same fonts does not inflate them again.
    fn decode_stream(&self, value: &PdfValue) -> Vec<u8> {
        let key = Self::stream_key(value);
        if let Some(cached) = key.and_then(|key| self.decoded.borrow().get(&key).cloned()) {
            return cached;
        }
        let data = self.decode_stream_uncapped(value);
        #[cfg(test)]
        self.decodes.set(self.decodes.get() + 1);
        self.charge_document(data.len() as u64, key);
        if let Some(key) = key {
            // Reuse is memory this run holds, so it may not outgrow what the
            // run let a document expand to: the cap is the document budget
            // whenever that is the smaller of the two.
            let budget = usize::try_from(self.limits.max_document_bytes).unwrap_or(usize::MAX);
            let cap = DECODED_CACHE_BYTES.min(budget);
            let held = self.decoded_bytes.get();
            if data.len() <= DECODED_CACHE_STREAM_BYTES && held + data.len() <= cap {
                let mut decoded = self.decoded.borrow_mut();
                if let std::collections::hash_map::Entry::Vacant(entry) = decoded.entry(key) {
                    entry.insert(data.clone());
                    self.decoded_bytes.set(held + data.len());
                }
            }
        }
        data
    }

    /// The name the whole-document total knows `value` by, when it is a stream
    /// the document holds.
    ///
    /// The payload buffer belongs to the stored `PdfValue`, so it is stable for
    /// the document's life: a `HashMap` rehash moves the value but not the heap
    /// buffer it carries, and no other stream can share that buffer. Keying on
    /// the value's own address would do neither — the rehash would charge one
    /// stream twice, and the address a dropped temporary leaves behind can be
    /// handed to a different stream, which would then look already paid for.
    ///
    /// A stream rebuilt for one decode has its own buffer, so this returns a key
    /// that no other decode shares; callers decoding such a stream pass `None`
    /// instead, because the rebuild is new work rather than a second read.
    fn stream_key(value: &PdfValue) -> Option<usize> {
        match value {
            PdfValue::Stream(_, raw) => Some(raw.as_ptr() as usize),
            _ => None,
        }
    }

    /// [`Self::decode_stream`] without the whole-document charge.
    fn decode_stream_uncapped(&self, value: &PdfValue) -> Vec<u8> {
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
        self.cap_stream(&mut data);
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
                    let ceiling = self.limits.max_decoded_stream_bytes.saturating_add(1);
                    let mut out = Vec::new();
                    let mut decoder = flate2::read::ZlibDecoder::new(&data[..]).take(ceiling);
                    if decoder.read_to_end(&mut out).is_ok() {
                        self.cap_stream(&mut out);
                        data = out;
                    } else {
                        // Some producers omit the zlib header; retry raw deflate.
                        let mut out2 = Vec::new();
                        let mut dec = flate2::read::DeflateDecoder::new(&data[..]).take(ceiling);
                        if dec.read_to_end(&mut out2).is_ok() {
                            self.cap_stream(&mut out2);
                            data = out2;
                        } else {
                            tracing::warn!("FlateDecode failed; using raw stream bytes");
                            return raw.clone();
                        }
                    }
                }
                // The decryption already happened when the file was opened
                // (`crate::crypt`); in the chain it is a no-op.
                "Crypt" => {}
                other => {
                    tracing::debug!("unsupported stream filter {other}; using raw bytes");
                    return raw.clone();
                }
            }
            self.cap_stream(&mut data);
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

/// `[l, t, r, b]` on a `width × height` page, turned clockwise by `degrees`
/// and still measured top-left — the same turn the viewer gives the page
/// before showing it (`/Rotate`).
fn turn_box(bbox: [f64; 4], degrees: i32, width: f64, height: f64) -> [f64; 4] {
    let [l, t, r, b] = bbox;
    match degrees {
        90 => [height - b, l, height - t, r],
        180 => [width - r, height - b, width - l, height - t],
        270 => [b, width - r, t, width - l],
        _ => bbox,
    }
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
            encrypted: false,
            locked: None,
            opened_with: None,
            objects: HashMap::new(),
            stream_offsets: HashMap::new(),
            root: None,
            limits: Limits::default(),
            deadline: Limits::default().deadline(),
            limit_hit: RefCell::new(None),
            document_bytes: Cell::new(0),
            charged_streams: RefCell::new(HashSet::new()),
            decoded: RefCell::new(HashMap::new()),
            decoded_bytes: Cell::new(0),
            #[cfg(test)]
            decodes: Cell::new(0),
        }
    }

    /// A stream read again is the same bytes: a page that visits another page's
    /// content twice does not spend the document total twice.
    #[test]
    fn the_document_total_counts_a_stream_once_however_often_it_is_read() {
        let mut doc = document();
        doc.limits.max_document_bytes = 2_500;
        let stream = PdfValue::Stream(HashMap::new(), vec![b'A'; 2_000]);
        assert_eq!(doc.decode_stream(&stream).len(), 2_000);
        assert_eq!(doc.decode_stream(&stream).len(), 2_000);
        assert!(
            doc.limit_hit.borrow().is_none(),
            "a repeated read of one stream was charged twice: {:?}",
            doc.limit_hit.borrow()
        );
    }

    /// A stream asked for twice inflates once. Reuse is what keeps a page of
    /// forms that all name the same fonts from decoding them again — the shape
    /// `math_gan.pdf` takes, where TeX writes every formula as its own form.
    #[test]
    fn a_stream_decoded_again_is_served_from_reuse() {
        let doc = document();
        let stream = PdfValue::Stream(HashMap::new(), vec![b'A'; 64]);
        assert_eq!(doc.decode_stream(&stream).len(), 64);
        assert_eq!(doc.decode_stream(&stream).len(), 64);
        assert_eq!(doc.decodes.get(), 1, "the second read inflated again");
    }

    /// The cache is memory the run holds on a document's behalf, so a run that
    /// allows a document a kilobyte may not keep 64 MiB of decodes: the cap is
    /// the document budget when that is the smaller of the two.
    #[test]
    fn a_small_document_budget_caps_the_reuse_cache() {
        let mut doc = document();
        doc.limits.max_document_bytes = 1_000;
        let first = PdfValue::Stream(HashMap::new(), vec![b'A'; 700]);
        let second = PdfValue::Stream(HashMap::new(), vec![b'B'; 700]);
        assert_eq!(doc.decode_stream(&first).len(), 700);
        assert_eq!(doc.decode_stream(&second).len(), 700);
        assert_eq!(
            doc.decoded_bytes.get(),
            700,
            "the second decode was kept past the document budget"
        );
        assert_eq!(doc.decoded.borrow().len(), 1);
    }

    /// Two streams are two expansions: the free re-read is for the same stream
    /// only, and each is known by the payload the document holds rather than by
    /// where its value happens to sit.
    #[test]
    fn two_streams_are_charged_apart() {
        let mut doc = document();
        doc.limits.max_document_bytes = 2_500;
        let first = PdfValue::Stream(HashMap::new(), vec![b'A'; 2_000]);
        let second = PdfValue::Stream(HashMap::new(), vec![b'B'; 2_000]);
        assert_eq!(doc.decode_stream(&first).len(), 2_000);
        assert_eq!(doc.decode_stream(&second).len(), 2_000);
        assert!(
            matches!(
                doc.limit_hit.borrow().as_ref(),
                Some(LimitError::DocumentTooLarge {
                    actual: 4_000,
                    limit: 2_500
                })
            ),
            "the second stream was not charged: {:?}",
            doc.limit_hit.borrow()
        );
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

    /// A CCITT or JBIG2 image may be wrapped in an ASCII or Flate filter. The
    /// codec has to receive the prefix's output, not the raw stream, and the
    /// terminal codec must not be run by the generic filter walk.
    #[test]
    fn a_filter_wrapped_around_an_image_codec_is_undone_first() {
        let value = PdfValue::Stream(
            dict(&[(
                "Filter",
                PdfValue::Array(vec![
                    PdfValue::Name("ASCIIHexDecode".to_string()),
                    PdfValue::Name("CCITTFaxDecode".to_string()),
                ]),
            )]),
            b"35 14>".to_vec(),
        );
        let decoded = document().decode_stream_before(&value, 1);
        assert_eq!(decoded, vec![0x35, 0x14]);
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

    /// A JPEG 2000 the decoder cannot read is still a picture the page drew:
    /// its own bytes are carried rather than the picture being dropped, which
    /// is the same choice a Word or PowerPoint package makes for a JPEG 2000
    /// its decoder does not know.
    #[test]
    fn an_unreadable_jpx_image_keeps_its_bytes() {
        let stream = PdfValue::Stream(
            dict(&[
                ("Width", PdfValue::Integer(64)),
                ("Height", PdfValue::Integer(48)),
                ("Filter", PdfValue::Name("JPXDecode".to_string())),
            ]),
            b"not a codestream".to_vec(),
        );
        let image = document().decode_image_xobject(&stream).expect("a picture");
        assert_eq!(image.mime, "image/jp2");
        assert_eq!(image.data, b"not a codestream");
        assert_eq!((image.width, image.height), (64, 48));
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
