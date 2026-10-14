//! Lexer for the PDF lexical layer (PDF 1.7, clause 7.2).
//!
//! Produces tokens optionally paired with their byte range, which the parser
//! needs in order to slice raw stream data out of the file.

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Header(String),
    Number(f64),
    Integer(i64),
    String(String),
    Name(String),
    LAngle,
    RAngle,
    LDoubleAngle,
    RDoubleAngle,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Obj,
    EndObj,
    Stream,
    EndStream,
    XRef,
    Trailer,
    StartXRef,
    Eof,
    True,
    False,
    Null,
    R,
    F,
    N,
}

/// A token together with its byte range `[start, end)` in the source buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    pub start: usize,
    pub end: usize,
    pub token: Token,
}

pub struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

/// First index at which `needle` occurs in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn is_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\0')
}

fn is_delimiter(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            bytes: data,
            pos: 0,
        }
    }

    /// Tokenize, dropping byte positions.
    pub fn tokenize(&mut self) -> Vec<Token> {
        self.tokenize_spanned()
            .into_iter()
            .map(|s| s.token)
            .collect()
    }

    /// Tokenize, keeping the byte range of every token.
    pub fn tokenize_spanned(&mut self) -> Vec<SpannedToken> {
        let mut out: Vec<SpannedToken> = Vec::new();

        // Header, e.g. "%PDF-1.4". Must be read before generic comment skipping.
        if self.bytes.starts_with(b"%PDF-") {
            let mut end = 5;
            while end < self.bytes.len() && !matches!(self.bytes[end], b'\r' | b'\n') {
                end += 1;
            }
            let header = String::from_utf8_lossy(&self.bytes[..end])
                .trim_end()
                .to_string();
            out.push(SpannedToken {
                start: 0,
                end,
                token: Token::Header(header),
            });
            self.pos = end;
        }

        loop {
            // Whitespace first: %%EOF may be preceded by a newline.
            self.skip_whitespace_and_comments();
            if self.pos >= self.bytes.len() {
                break;
            }

            // %%EOF marks the end of a cross-reference section. A linearized
            // file contains one *mid-file* (after the first-page section) before
            // the bulk of the objects, so tokenizing must continue past it.
            if self.bytes[self.pos..].starts_with(b"%%EOF") {
                let start = self.pos;
                self.pos += 5;
                if self.bytes[self.pos..].starts_with(b"\r\n") {
                    self.pos += 2;
                } else if self.pos < self.bytes.len()
                    && matches!(self.bytes[self.pos], b'\r' | b'\n')
                {
                    self.pos += 1;
                }
                out.push(SpannedToken {
                    start,
                    end: self.pos,
                    token: Token::Eof,
                });
                continue;
            }

            let start = self.pos;
            let token = match self.bytes[self.pos] {
                b'/' => {
                    self.pos += 1;
                    Token::Name(self.read_name())
                }
                b'(' => {
                    self.pos += 1;
                    Token::String(self.read_literal_string())
                }
                b'<' => {
                    if self.peek(1) == Some(b'<') {
                        self.pos += 2;
                        Token::LDoubleAngle
                    } else {
                        self.pos += 1;
                        Token::String(self.read_hex_string())
                    }
                }
                b'>' => {
                    if self.peek(1) == Some(b'>') {
                        self.pos += 2;
                        Token::RDoubleAngle
                    } else {
                        self.pos += 1;
                        Token::RAngle
                    }
                }
                b'[' => {
                    self.pos += 1;
                    Token::LBracket
                }
                b']' => {
                    self.pos += 1;
                    Token::RBracket
                }
                b'{' => {
                    self.pos += 1;
                    Token::LBrace
                }
                b'}' => {
                    self.pos += 1;
                    Token::RBrace
                }
                b')' => {
                    // Unbalanced ')' is malformed; skip it to guarantee progress.
                    self.pos += 1;
                    continue;
                }
                c if c == b'+' || c == b'-' || c == b'.' || c.is_ascii_digit() => {
                    let (value, is_int) = self.read_number();
                    if is_int {
                        Token::Integer(value as i64)
                    } else {
                        Token::Number(value)
                    }
                }
                c if c.is_ascii_alphabetic() => {
                    let word = self.read_word();
                    if word == "stream" {
                        // Stream payloads are opaque binary. Scanning them as PDF
                        // syntax is wrong: a '(' inside JPEG data would swallow the
                        // real terminator. Skip the payload at the byte level and
                        // emit a Stream token whose span *is* the payload.
                        let (data_start, data_end) = self.skip_stream_payload();
                        out.push(SpannedToken {
                            start: data_start,
                            end: data_end,
                            token: Token::Stream,
                        });
                        continue;
                    }
                    match word.as_str() {
                        "obj" => Token::Obj,
                        "endobj" => Token::EndObj,
                        "stream" => Token::Stream,
                        "endstream" => Token::EndStream,
                        "xref" => Token::XRef,
                        "trailer" => Token::Trailer,
                        "startxref" => Token::StartXRef,
                        "true" => Token::True,
                        "false" => Token::False,
                        "null" => Token::Null,
                        "R" => Token::R,
                        "f" => Token::F,
                        "n" => Token::N,
                        // Unknown keyword: ignore, but keep the byte range
                        // consistent by falling through to the end push.
                        _ => continue,
                    }
                }
                _ => {
                    self.pos += 1;
                    continue;
                }
            };
            out.push(SpannedToken {
                start,
                end: self.pos,
                token,
            });
        }
        out
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    /// Called with `self.pos` just past the `stream` keyword. Skips one EOL, then
    /// the payload up to `endstream`, and leaves `self.pos` at the start of the
    /// `endstream` keyword so the caller still emits it.
    ///
    /// Returns the payload byte range, excluding the EOL before `endstream`.
    fn skip_stream_payload(&mut self) -> (usize, usize) {
        let mut start = self.pos;
        if self.bytes.get(start) == Some(&b'\r') {
            start += 1;
            if self.bytes.get(start) == Some(&b'\n') {
                start += 1;
            }
        } else if self.bytes.get(start) == Some(&b'\n') {
            start += 1;
        }

        let needle: &[u8] = b"endstream";
        let (data_end, resume) = match find_subslice(&self.bytes[start..], needle) {
            Some(offset) => {
                let at = start + offset;
                (at, at)
            }
            // No terminator: treat the rest of the file as payload.
            None => (self.bytes.len(), self.bytes.len()),
        };

        // The EOL preceding `endstream` is not part of the payload.
        let mut end = data_end;
        if end > start && self.bytes[end - 1] == b'\n' {
            end -= 1;
            if end > start && self.bytes[end - 1] == b'\r' {
                end -= 1;
            }
        } else if end > start && self.bytes[end - 1] == b'\r' {
            end -= 1;
        }

        self.pos = resume;
        (start, end)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            while self.pos < self.bytes.len() && is_whitespace(self.bytes[self.pos]) {
                self.pos += 1;
            }
            if self.pos < self.bytes.len()
                && self.bytes[self.pos] == b'%'
                && !self.bytes[self.pos..].starts_with(b"%%EOF")
            {
                while self.pos < self.bytes.len() && !matches!(self.bytes[self.pos], b'\r' | b'\n')
                {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
    }

    fn read_name(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.bytes.len()
            && !is_whitespace(self.bytes[self.pos])
            && !is_delimiter(self.bytes[self.pos])
        {
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.bytes[start..self.pos]).to_string()
    }

    /// Read a literal `( ... )` string, resolving escapes and honouring
    /// balanced nested parentheses.
    fn read_literal_string(&mut self) -> String {
        let mut out = String::new();
        let mut depth = 1usize;
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            self.pos += 1;
            match c {
                b'\\' => {
                    if self.pos >= self.bytes.len() {
                        break;
                    }
                    let e = self.bytes[self.pos];
                    self.pos += 1;
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
                                match self.peek(0) {
                                    Some(d @ b'0'..=b'7') => {
                                        val = val * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            if let Some(ch) = char::from_u32(val & 0xff) {
                                out.push(ch);
                            }
                        }
                        b'\r' => {
                            if self.peek(0) == Some(b'\n') {
                                self.pos += 1;
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
        out
    }

    fn read_hex_string(&mut self) -> String {
        let mut digits = String::new();
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'>' {
            digits.push(self.bytes[self.pos] as char);
            self.pos += 1;
        }
        if self.pos < self.bytes.len() {
            self.pos += 1; // consume '>'
        }
        let cleaned: String = digits.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        let mut out = String::new();
        let mut chars = cleaned.chars();
        while let Some(hi) = chars.next() {
            let lo = chars.next().unwrap_or('0');
            if let Ok(byte) = u8::from_str_radix(&format!("{hi}{lo}"), 16) {
                out.push(byte as char);
            }
        }
        out
    }

    fn read_number(&mut self) -> (f64, bool) {
        let start = self.pos;
        if matches!(self.peek(0), Some(b'+') | Some(b'-')) {
            self.pos += 1;
        }
        let mut has_dot = false;
        while let Some(c) = self.peek(0) {
            if c.is_ascii_digit() {
                self.pos += 1;
            } else if c == b'.' && !has_dot {
                has_dot = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = String::from_utf8_lossy(&self.bytes[start..self.pos]).to_string();
        (text.parse::<f64>().unwrap_or(0.0), !has_dot)
    }

    fn read_word(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek(0) {
            if c.is_ascii_alphanumeric() {
                self.pos += 1;
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&self.bytes[start..self.pos]).to_string()
    }
}
