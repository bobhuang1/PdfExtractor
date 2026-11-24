//! The standard security handler (ISO 32000-2 §7.6.4).
//!
//! An encrypted PDF ciphers every string and stream under a key derived from a
//! password. This module authenticates a password against the `/Encrypt`
//! dictionary and decrypts one object's strings and streams with the key it
//! yields. Covered: RC4 40–128 bit (`/V 1`/`/V 2`, revisions 2–3), crypt
//! filters with RC4 or AES-128 (`/V 4`, revision 4) and AES-256 (`/V 5`,
//! revisions 5 and 6). Public-key handlers are not: they need a certificate,
//! not a password.
//!
//! The cipher primitives (MD5, SHA-2, AES) are borrowed infrastructure; RC4 is
//! six lines and written here. What a PDF does with them — padding, key
//! derivation, which objects are exempt — is ours.

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use md5::{Digest as _, Md5};
use sha2::{Sha256, Sha384, Sha512};
use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

use crate::objects::PdfValue;

/// The 32-byte padding string of ISO 32000, algorithm 2.
const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

/// Why a file could not be decrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptError {
    /// The handler or its revision is not one this module implements.
    Unsupported(String),
    /// No password tried (the empty one included) opens the file, or the
    /// password cannot be written in the encoding the file's revision uses.
    WrongPassword,
}

impl std::fmt::Display for CryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptError::Unsupported(what) => write!(f, "unsupported encryption: {what}"),
            CryptError::WrongPassword => write!(f, "the password does not open this PDF"),
        }
    }
}

impl std::error::Error for CryptError {}

/// How one class of data (strings or streams) is ciphered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Identity,
    Rc4,
    Aes128,
    Aes256,
}

/// An authenticated security handler: the file key and the methods to use.
#[derive(Debug, Clone)]
pub struct SecurityHandler {
    key: Vec<u8>,
    strings: Method,
    streams: Method,
    /// The methods `/CF` names, for a stream whose own `/Crypt` filter picks
    /// one by name instead of taking `/StmF`.
    named: HashMap<String, Method>,
    /// `/EncryptMetadata false` leaves `/Type /Metadata` streams in the clear.
    pub encrypt_metadata: bool,
    /// Which password opened the file, for the log.
    pub opened_with: &'static str,
}

fn bytes_of(value: Option<&PdfValue>) -> Option<Vec<u8>> {
    match value {
        Some(PdfValue::String(s)) => Some(latin1_bytes(s)),
        _ => None,
    }
}

fn int_of(value: Option<&PdfValue>) -> Option<i64> {
    match value {
        Some(PdfValue::Integer(n)) => Some(*n),
        Some(PdfValue::Number(n)) => Some(*n as i64),
        _ => None,
    }
}

/// The lexer keeps a string's bytes as one `char` each (U+0000–U+00FF).
pub fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

/// The inverse of [`latin1_bytes`].
pub fn latin1_string(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

impl SecurityHandler {
    /// Authenticate `password` (tried as the user password, then as the owner
    /// password) against `/Encrypt`; `id0` is the first element of the
    /// trailer's `/ID`. A `None` password tries the empty one, which is how a
    /// reader opens a file that asks nothing of the person opening it.
    ///
    /// `password` is the text a person gave, not bytes: the revision read here
    /// decides how it is written, PDFDocEncoding up to revision 4 and UTF-8
    /// after SASLprep from revision 6. A password that cannot be written that
    /// way cannot be the one the file holds.
    pub fn authenticate(
        encrypt: &HashMap<String, PdfValue>,
        id0: &[u8],
        password: Option<&str>,
    ) -> Result<Self, CryptError> {
        let filter = encrypt.get("Filter").and_then(PdfValue::as_name);
        if filter != Some("Standard") {
            return Err(CryptError::Unsupported(format!(
                "security handler /{} (only /Standard, the password handler, is read)",
                filter.unwrap_or("?")
            )));
        }
        let v = int_of(encrypt.get("V")).unwrap_or(0);
        let r = int_of(encrypt.get("R")).unwrap_or(0);
        let o = bytes_of(encrypt.get("O")).unwrap_or_default();
        let u = bytes_of(encrypt.get("U")).unwrap_or_default();
        let p = int_of(encrypt.get("P")).unwrap_or(0) as i32;
        let encrypt_metadata = !matches!(
            encrypt.get("EncryptMetadata"),
            Some(PdfValue::Boolean(false))
        );
        let password = password.unwrap_or("");

        let (strings, streams, length) = match v {
            1 => (Method::Rc4, Method::Rc4, 5),
            2 => {
                let bits = int_of(encrypt.get("Length")).unwrap_or(40);
                if !(40..=128).contains(&bits) || bits % 8 != 0 {
                    return Err(CryptError::Unsupported(format!("RC4 key of {bits} bits")));
                }
                (Method::Rc4, Method::Rc4, (bits / 8) as usize)
            }
            4 | 5 => {
                let strings = crypt_filter(encrypt, "StrF")?;
                let streams = crypt_filter(encrypt, "StmF")?;
                let length = if v == 5 {
                    // `/V 5` is AES-256, whose revisions are 5 and 6. A legacy
                    // revision cannot take this key length: the legacy
                    // derivation below is a 16-byte MD5 digest.
                    if !(5..=6).contains(&r) {
                        return Err(CryptError::Unsupported(format!(
                            "a /V {v} handler with revision {r}"
                        )));
                    }
                    32
                } else {
                    // `/Length` of a V4 handler is in bits; 128 is the default.
                    (int_of(encrypt.get("Length")).unwrap_or(128) / 8).clamp(5, 16) as usize
                };
                (strings, streams, length)
            }
            other => {
                return Err(CryptError::Unsupported(format!("/V {other}")));
            }
        };

        let named = crypt_filters(encrypt);
        let handler = |key: Vec<u8>, opened_with| Self {
            key,
            strings,
            streams,
            named: named.clone(),
            encrypt_metadata,
            opened_with,
        };

        match r {
            2..=4 => {
                // A revision 4 or older password is written in PDFDocEncoding,
                // while the text here came from argv: the bytes hashed are the
                // file's encoding's, not the terminal's UTF-8.
                let password = pdfdoc_encoding(password).ok_or(CryptError::WrongPassword)?;
                if let Some(key) =
                    legacy_user(&password, &o, &u, p, id0, r, length, encrypt_metadata)
                {
                    return Ok(handler(key, "user password"));
                }
                // As the owner: algorithm 7 recovers the user password from /O.
                let user = legacy_owner_to_user(&password, &o, r, length);
                if let Some(key) = legacy_user(&user, &o, &u, p, id0, r, length, encrypt_metadata) {
                    return Ok(handler(key, "owner password"));
                }
                Err(CryptError::WrongPassword)
            }
            5 | 6 => {
                let ue = bytes_of(encrypt.get("UE")).unwrap_or_default();
                let oe = bytes_of(encrypt.get("OE")).unwrap_or_default();
                // A revision 5 password is UTF-8 as typed; a revision 6 one is
                // prepared with SASLprep first. Both are truncated to 127 bytes,
                // as the passwords that were stored were.
                let prepared = if r == 6 {
                    saslprep(password)
                } else {
                    password.to_string()
                };
                let password = &prepared.as_bytes()[..prepared.len().min(127)];
                if u.len() < 48 || o.len() < 48 || ue.len() < 32 || oe.len() < 32 {
                    return Err(CryptError::Unsupported(
                        "an AES-256 handler whose /O, /U, /OE or /UE is short".into(),
                    ));
                }
                let hash = |pw: &[u8], salt: &[u8], udata: &[u8]| {
                    if r == 5 {
                        sha256(&[pw, salt, udata].concat()).to_vec()
                    } else {
                        hash_2b(pw, salt, udata)
                    }
                };
                if hash(password, &u[32..40], b"")[..] == u[..32] {
                    let k = hash(password, &u[40..48], b"");
                    return Ok(handler(
                        aes256_cbc_no_iv_decrypt(&k, &ue[..32]),
                        "user password",
                    ));
                }
                if hash(password, &o[32..40], &u[..48])[..] == o[..32] {
                    let k = hash(password, &o[40..48], &u[..48]);
                    return Ok(handler(
                        aes256_cbc_no_iv_decrypt(&k, &oe[..32]),
                        "owner password",
                    ));
                }
                Err(CryptError::WrongPassword)
            }
            other => Err(CryptError::Unsupported(format!("revision {other}"))),
        }
    }

    /// Decrypt every string inside `value`, and a stream's payload unless it
    /// is exempt (a cross-reference stream, or metadata left in the clear).
    pub fn decrypt_object(&self, num: usize, gen: i32, value: &mut PdfValue) {
        if let PdfValue::Stream(dict, raw) = value {
            if !stream_is_exempt(dict, self.encrypt_metadata) {
                self.decrypt_in_place(self.stream_method(dict), num, gen, raw);
            }
        }
        self.decrypt_strings(num, gen, value);
    }

    /// The bytes decrypting `value` rebuilds for its strings: each string goes
    /// through a Latin-1 working buffer of one byte per character, and nested
    /// dictionaries and arrays are counted with it.
    ///
    /// This is not the plaintext size. A stream's payload is decrypted where it
    /// stands, in the buffer the parser charged when it copied the payload out
    /// of the file, so only the string buffers are new allocation. The caller
    /// charges this before [`Self::decrypt_object`] runs, so a file whose
    /// strings would not fit the whole-document budget is refused before any of
    /// them is rebuilt.
    ///
    /// Nothing about it depends on the handler, which is why it takes no `self`.
    pub fn rebuilt_string_bytes(value: &PdfValue) -> u64 {
        match value {
            PdfValue::String(s) => s.chars().count() as u64,
            PdfValue::Array(items) => items.iter().fold(0u64, |total, item| {
                total.saturating_add(Self::rebuilt_string_bytes(item))
            }),
            PdfValue::Dictionary(dict) | PdfValue::Stream(dict, _) => {
                dict.values().fold(0u64, |total, item| {
                    total.saturating_add(Self::rebuilt_string_bytes(item))
                })
            }
            _ => 0,
        }
    }

    /// The method a stream's payload is ciphered with: its own `/Crypt` filter
    /// when it names one, and `/StmF` otherwise.
    fn stream_method(&self, dict: &HashMap<String, PdfValue>) -> Method {
        match stream_crypt_filter(dict) {
            None => self.streams,
            Some("Identity") => Method::Identity,
            // A name `/CF` does not define leaves the file malformed. The
            // default stream filter is the closest thing to a stated method —
            // it is what this stream would have been ciphered with had the
            // `/Crypt` entry been absent — so it is the guess made here.
            Some(name) => self.named.get(name).copied().unwrap_or(self.streams),
        }
    }

    fn decrypt_strings(&self, num: usize, gen: i32, value: &mut PdfValue) {
        match value {
            PdfValue::String(s) => {
                // A string holds one `char` per byte, so its bytes are built
                // once and decrypted where they lie before the text is rebuilt.
                let mut bytes = latin1_bytes(s);
                self.decrypt_in_place(self.strings, num, gen, &mut bytes);
                *s = latin1_string(&bytes);
            }
            PdfValue::Array(items) => {
                for item in items {
                    self.decrypt_strings(num, gen, item);
                }
            }
            PdfValue::Dictionary(dict) | PdfValue::Stream(dict, _) => {
                for item in dict.values_mut() {
                    self.decrypt_strings(num, gen, item);
                }
            }
            _ => {}
        }
    }

    /// Decrypt `data` where it stands.
    ///
    /// A stream's buffer is the one the parser copied out of the file and
    /// charged to the document budget, so decrypting in place allocates no
    /// second ciphertext-sized buffer per stream — the copy that would let a
    /// file of many encrypted streams outgrow the charge. An Identity payload
    /// is left untouched rather than copied for nothing.
    fn decrypt_in_place(&self, method: Method, num: usize, gen: i32, data: &mut Vec<u8>) {
        match method {
            Method::Identity => {}
            Method::Rc4 => rc4_in_place(&self.object_key(num, gen, false), data),
            Method::Aes128 => aes_cbc_decrypt_in_place(&self.object_key(num, gen, true), data),
            Method::Aes256 => aes_cbc_decrypt_in_place(&self.key, data),
        }
    }

    /// Algorithm 1: the key one object is ciphered under (RC4 and AES-128).
    fn object_key(&self, num: usize, gen: i32, aes: bool) -> Vec<u8> {
        let mut h = Md5::new();
        h.update(&self.key);
        h.update(&(num as u32).to_le_bytes()[..3]);
        h.update(&(gen as u32).to_le_bytes()[..2]);
        if aes {
            h.update(b"sAlT");
        }
        let digest = h.finalize();
        digest[..(self.key.len() + 5).min(16)].to_vec()
    }
}

/// `text` as PDFDocEncoding, the encoding a revision 4 or older handler writes
/// its password in (ISO 32000-2, Annex D). `None` when a character has no slot
/// there, which is a password no such file can be holding.
fn pdfdoc_encoding(text: &str) -> Option<Vec<u8>> {
    text.chars().map(pdfdoc_byte).collect()
}

/// The PDFDocEncoding byte for a character.
fn pdfdoc_byte(c: char) -> Option<u8> {
    let byte = match c {
        // Printable ASCII keeps its byte; 0x18–0x1F hold accents (below) and
        // 0x7F has none. Latin-1 from 0xA1 keeps its byte except the soft
        // hyphen; 0x80–0x9F are the encoding's own symbols (below), and U+00A0
        // has no slot.
        '\u{0000}'..='\u{0017}' | '\u{0020}'..='\u{007E}' => c as u8,
        '\u{00A1}'..='\u{00FF}' if c != '\u{00AD}' => c as u8,
        '\u{02D8}' => 0x18, // breve
        '\u{02C7}' => 0x19, // caron
        '\u{02C6}' => 0x1A, // circumflex
        '\u{02D9}' => 0x1B, // dot above
        '\u{02DD}' => 0x1C, // double acute
        '\u{02DB}' => 0x1D, // ogonek
        '\u{02DA}' => 0x1E, // ring above
        '\u{02DC}' => 0x1F, // small tilde
        '\u{2022}' => 0x80, // bullet
        '\u{2020}' => 0x81, // dagger
        '\u{2021}' => 0x82, // double dagger
        '\u{2026}' => 0x83, // ellipsis
        '\u{2014}' => 0x84, // em dash
        '\u{2013}' => 0x85, // en dash
        '\u{0192}' => 0x86, // florin
        '\u{2044}' => 0x87, // fraction slash
        '\u{2039}' => 0x88, // single left angle quote
        '\u{203A}' => 0x89, // single right angle quote
        '\u{2212}' => 0x8A, // minus
        '\u{2030}' => 0x8B, // per mille
        '\u{201E}' => 0x8C, // double low quote
        '\u{201C}' => 0x8D, // left double quote
        '\u{201D}' => 0x8E, // right double quote
        '\u{2018}' => 0x8F, // left single quote
        '\u{2019}' => 0x90, // right single quote
        '\u{201A}' => 0x91, // single low quote
        '\u{2122}' => 0x92, // trademark
        '\u{FB01}' => 0x93, // fi ligature
        '\u{FB02}' => 0x94, // fl ligature
        '\u{0141}' => 0x95, // L with stroke
        '\u{0152}' => 0x96, // OE ligature
        '\u{0160}' => 0x97, // S with caron
        '\u{0178}' => 0x98, // Y with diaeresis
        '\u{017D}' => 0x99, // Z with caron
        '\u{0131}' => 0x9A, // dotless i
        '\u{0142}' => 0x9B, // l with stroke
        '\u{0153}' => 0x9C, // oe ligature
        '\u{0161}' => 0x9D, // s with caron
        '\u{017E}' => 0x9E, // z with caron
        '\u{20AC}' => 0xA0, // euro
        _ => return None,
    };
    Some(byte)
}

/// RFC 4013's preparation of a revision 6 password, before it is UTF-8
/// encoded: the characters that are commonly mapped to nothing are dropped,
/// the non-ASCII spaces become a space, and the result is NFKC.
///
/// The profile's prohibition and bidirectional checks are not repeated. They
/// refuse text no prepared password can hold, and a refused password and a
/// wrong one are the same answer here, so leaving them out cannot fail a file
/// that should open.
fn saslprep(text: &str) -> String {
    let mapped: String = text
        .chars()
        .filter(|c| !maps_to_nothing(*c))
        .map(|c| if is_non_ascii_space(c) { ' ' } else { c })
        .collect();
    mapped.nfkc().collect()
}

/// Stringprep B.1, the characters a prepared password drops.
fn maps_to_nothing(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{1806}'
            | '\u{180B}'..='\u{180D}'
            | '\u{200B}'..='\u{200D}'
            | '\u{2060}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
    )
}

/// Stringprep C.1.2, the non-ASCII spaces a prepared password turns into one.
fn is_non_ascii_space(c: char) -> bool {
    matches!(
        c,
        '\u{00A0}' | '\u{1680}' | '\u{2000}'..='\u{200B}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// Whether a stream's payload is left ciphered as it stands: a cross-reference
/// stream is never ciphered, and `/EncryptMetadata false` leaves metadata in
/// the clear.
fn stream_is_exempt(dict: &HashMap<String, PdfValue>, encrypt_metadata: bool) -> bool {
    let kind = dict.get("Type").and_then(PdfValue::as_name);
    kind == Some("XRef") || (kind == Some("Metadata") && !encrypt_metadata)
}

/// The name a stream's own `/Filter /Crypt` entry gives, or `None` when the
/// stream has no such filter. A `/Crypt` entry with no `/Name` is the Identity
/// filter, which is the specification's default for the key.
fn stream_crypt_filter(dict: &HashMap<String, PdfValue>) -> Option<&str> {
    let names_crypt = match dict.get("Filter") {
        Some(PdfValue::Name(n)) => n == "Crypt",
        Some(PdfValue::Array(a)) => a.iter().any(|f| f.as_name() == Some("Crypt")),
        _ => false,
    };
    if !names_crypt {
        return None;
    }
    let parms = match dict.get("DecodeParms") {
        Some(PdfValue::Dictionary(d)) => Some(d),
        Some(PdfValue::Array(a)) => a.iter().find_map(PdfValue::as_dict),
        _ => None,
    };
    Some(
        parms
            .and_then(|p| p.get("Name"))
            .and_then(PdfValue::as_name)
            .unwrap_or("Identity"),
    )
}

/// The cipher method a `/CFM` names; `None` for a method this build does not
/// implement. An absent `/CFM` means `/None`, the identity filter.
fn method_of(cfm: Option<&str>) -> Option<Method> {
    match cfm {
        None | Some("None") => Some(Method::Identity),
        Some("V2") => Some(Method::Rc4),
        Some("AESV2") => Some(Method::Aes128),
        Some("AESV3") => Some(Method::Aes256),
        Some(_) => None,
    }
}

/// Every `/CF` entry's method, by the name a stream's `/Crypt` filter uses.
///
/// A method this build does not implement is left out rather than mapped to
/// identity, so that stream falls back to the file's `/StmF`.
fn crypt_filters(encrypt: &HashMap<String, PdfValue>) -> HashMap<String, Method> {
    let Some(cf) = encrypt.get("CF").and_then(PdfValue::as_dict) else {
        return HashMap::new();
    };
    cf.iter()
        .filter_map(|(name, entry)| {
            let cfm = entry
                .as_dict()
                .and_then(|filter| filter.get("CFM"))
                .and_then(PdfValue::as_name);
            Some((name.clone(), method_of(cfm)?))
        })
        .collect()
}

/// The method a `/V 4`/`/V 5` handler names under `/StrF` or `/StmF`.
fn crypt_filter(encrypt: &HashMap<String, PdfValue>, key: &str) -> Result<Method, CryptError> {
    let name = encrypt
        .get(key)
        .and_then(PdfValue::as_name)
        .unwrap_or("Identity");
    if name == "Identity" {
        return Ok(Method::Identity);
    }
    let cfm = encrypt
        .get("CF")
        .and_then(PdfValue::as_dict)
        .and_then(|cf| cf.get(name))
        .and_then(PdfValue::as_dict)
        .and_then(|f| f.get("CFM"))
        .and_then(PdfValue::as_name);
    method_of(cfm).ok_or_else(|| {
        CryptError::Unsupported(format!("crypt filter method /{}", cfm.unwrap_or("?")))
    })
}

fn padded(password: &[u8]) -> Vec<u8> {
    let mut out = password[..password.len().min(32)].to_vec();
    out.extend_from_slice(&PAD[..32 - out.len()]);
    out
}

/// Algorithm 2 (key) followed by algorithm 4 or 5 (check `/U`).
#[allow(clippy::too_many_arguments)]
fn legacy_user(
    password: &[u8],
    o: &[u8],
    u: &[u8],
    p: i32,
    id0: &[u8],
    r: i64,
    length: usize,
    encrypt_metadata: bool,
) -> Option<Vec<u8>> {
    let mut h = Md5::new();
    h.update(padded(password));
    h.update(&o[..o.len().min(32)]);
    h.update(p.to_le_bytes());
    h.update(id0);
    if r >= 4 && !encrypt_metadata {
        h.update([0xFF; 4]);
    }
    let mut key = h.finalize().to_vec();
    if r >= 3 {
        for _ in 0..50 {
            key = Md5::digest(&key[..length]).to_vec();
        }
    }
    key.truncate(length);

    let matches = if r == 2 {
        u.len() >= 32 && rc4(&key, &PAD) == u[..32]
    } else {
        let mut h = Md5::new();
        h.update(PAD);
        h.update(id0);
        let mut check = rc4(&key, &h.finalize());
        for i in 1..=19u8 {
            let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
            check = rc4(&k, &check);
        }
        u.len() >= 16 && check[..] == u[..16]
    };
    matches.then_some(key)
}

/// Algorithm 7: the user password an owner password unlocks from `/O`.
fn legacy_owner_to_user(owner: &[u8], o: &[u8], r: i64, length: usize) -> Vec<u8> {
    let mut key = Md5::digest(padded(owner)).to_vec();
    let n = if r >= 3 { length } else { 5 };
    if r >= 3 {
        for _ in 0..50 {
            key = Md5::digest(&key[..n]).to_vec();
        }
    }
    key.truncate(n);
    let o = &o[..o.len().min(32)];
    if r == 2 {
        rc4(&key, o)
    } else {
        let mut user = o.to_vec();
        for i in (0..=19u8).rev() {
            let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
            user = rc4(&k, &user);
        }
        user
    }
}

/// Algorithm 2.B, the revision 6 password hash.
fn hash_2b(password: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
    let mut k = sha256(&[password, salt, udata].concat()).to_vec();
    let mut round = 0usize;
    loop {
        let block: Vec<u8> = [password, &k, udata].concat();
        let k1: Vec<u8> = block
            .iter()
            .copied()
            .cycle()
            .take(block.len() * 64)
            .collect();
        let e = aes128_cbc_encrypt(&k[..16], &k[16..32], &k1);
        let m = e[..16].iter().map(|&b| b as u32).sum::<u32>() % 3;
        k = match m {
            0 => sha256(&e).to_vec(),
            1 => Sha384::digest(&e).to_vec(),
            _ => Sha512::digest(&e).to_vec(),
        };
        round += 1;
        if round >= 64 && (*e.last().unwrap() as usize) <= round - 32 {
            return k[..32].to_vec();
        }
    }
}

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// RC4, the stream cipher of the older handlers.
pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    rc4_in_place(key, &mut out);
    out
}

/// RC4 over the bytes' own buffer: the cipher XORs a keystream, so each byte
/// is replaced where it stands.
fn rc4_in_place(key: &[u8], data: &mut [u8]) {
    if key.is_empty() {
        return;
    }
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    for byte in data.iter_mut() {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        *byte ^= s[s[i as usize].wrapping_add(s[j as usize]) as usize];
    }
}

/// An AES block cipher of one of the two key sizes `/V 4` and `/V 5` use.
enum Cipher {
    A128(Box<aes::Aes128>),
    A256(Box<aes::Aes256>),
}

impl Cipher {
    /// `None` for a key length AES does not take, which a malformed `/Length`
    /// can leave a handler with.
    fn new(key: &[u8]) -> Option<Self> {
        match key.len() {
            16 => Some(Cipher::A128(Box::new(aes::Aes128::new(
                GenericArray::from_slice(key),
            )))),
            32 => Some(Cipher::A256(Box::new(aes::Aes256::new(
                GenericArray::from_slice(key),
            )))),
            _ => None,
        }
    }

    /// Decrypt one block over its own bytes.
    fn decrypt_block(&self, block: &mut [u8; 16]) {
        let block = GenericArray::from_mut_slice(block.as_mut_slice());
        match self {
            Cipher::A128(cipher) => cipher.decrypt_block(block),
            Cipher::A256(cipher) => cipher.decrypt_block(block),
        }
    }
}

/// AES-CBC with the IV in the first block and PKCS#7 padding, as a PDF writes
/// it, decrypted in the payload's own buffer: the IV is dropped and the padding
/// trimmed by moving the plaintext down, never by allocating a second copy. A
/// payload too short for an IV, or whose padding is malformed, comes back as
/// best decrypted rather than lost: a damaged stream is the filters' problem.
fn aes_cbc_decrypt_in_place(key: &[u8], data: &mut Vec<u8>) {
    let Some(cipher) = Cipher::new(key) else {
        data.clear();
        return;
    };
    if data.len() < 32 {
        data.clear();
        return;
    }
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&data[..16]);
    let body_len = (data.len() - 16) / 16 * 16;
    data.truncate(16 + body_len);
    cbc_decrypt_in_place(&cipher, &iv, &mut data[16..]);
    if let Some(&n) = data.last() {
        let n = n as usize;
        if (1..=16).contains(&n)
            && n <= body_len
            && data[data.len() - n..].iter().all(|&b| b as usize == n)
        {
            data.truncate(data.len() - n);
        }
    }
    let plain_len = data.len() - 16;
    data.copy_within(16.., 0);
    data.truncate(plain_len);
}

/// `/UE` and `/OE`: AES-256-CBC with a zero IV and no padding.
fn aes256_cbc_no_iv_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    cbc_decrypt(key, &[0u8; 16], data)
}

/// AES-CBC over a copy of `body`: the short fixed-size buffers of the password
/// checks, where there is nothing to save by decrypting in place.
fn cbc_decrypt(key: &[u8], iv: &[u8], body: &[u8]) -> Vec<u8> {
    let Some(cipher) = Cipher::new(key) else {
        return Vec::new();
    };
    let mut out = body.to_vec();
    cbc_decrypt_in_place(&cipher, iv, &mut out);
    out
}

/// AES-CBC over `body` itself: only the 16-byte ciphertext block being
/// decrypted is held aside, so the payload never exists twice.
fn cbc_decrypt_in_place(cipher: &Cipher, iv: &[u8], body: &mut [u8]) {
    let mut prev = [0u8; 16];
    prev.copy_from_slice(iv);
    for chunk in body.as_chunks_mut::<16>().0 {
        let ciphertext = *chunk;
        cipher.decrypt_block(chunk);
        for (byte, previous) in chunk.iter_mut().zip(prev.iter()) {
            *byte ^= previous;
        }
        prev = ciphertext;
    }
}

/// AES-128-CBC encryption with no padding (the input is a multiple of 16).
fn aes128_cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    let cipher = aes::Aes128::new(GenericArray::from_slice(key));
    let mut prev = [0u8; 16];
    prev.copy_from_slice(iv);
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.as_chunks::<16>().0 {
        let mut block = [0u8; 16];
        for (b, (c, p)) in block.iter_mut().zip(chunk.iter().zip(prev.iter())) {
            *b = c ^ p;
        }
        let mut block = GenericArray::from(block);
        cipher.encrypt_block(&mut block);
        out.extend_from_slice(&block);
        prev.copy_from_slice(&block);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_matches_the_published_test_vector() {
        // "Key"/"Plaintext", the classic RC4 vector.
        assert_eq!(
            rc4(b"Key", b"Plaintext"),
            [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]
        );
    }

    #[test]
    fn aes_cbc_round_trips_with_pkcs7_padding() {
        let key = [7u8; 16];
        let iv = [9u8; 16];
        let plain = b"Secret ledger";
        let mut padded = plain.to_vec();
        padded.extend(std::iter::repeat_n(3u8, 3));
        let mut data = iv.to_vec();
        data.extend(aes128_cbc_encrypt(&key, &iv, &padded));
        aes_cbc_decrypt_in_place(&key, &mut data);
        assert_eq!(data, plain);
    }

    fn handler(method: Method) -> SecurityHandler {
        SecurityHandler {
            key: vec![7u8; 16],
            strings: Method::Rc4,
            streams: method,
            named: HashMap::new(),
            encrypt_metadata: true,
            opened_with: "user password",
        }
    }

    /// A `/V 5` (AES-256) dictionary that names a legacy revision is a
    /// mismatch, not a 32-byte RC4 key: the legacy derivation is a 16-byte MD5
    /// digest, so it used to slice past the end. It is a typed refusal.
    #[test]
    fn an_aes256_key_length_with_a_legacy_revision_is_refused() {
        let mut dict = HashMap::new();
        dict.insert("Filter".into(), PdfValue::Name("Standard".into()));
        dict.insert("V".into(), PdfValue::Integer(5));
        dict.insert("R".into(), PdfValue::Integer(3));
        dict.insert("O".into(), PdfValue::String("o".repeat(32)));
        dict.insert("U".into(), PdfValue::String("u".repeat(32)));
        let error = SecurityHandler::authenticate(&dict, &[0u8; 16], None)
            .expect_err("a mismatched revision is refused");
        assert!(matches!(error, CryptError::Unsupported(_)), "{error:?}");
        assert!(error.to_string().contains("revision 3"), "{error}");
    }

    /// A stream is decrypted in the buffer the document already holds, so the
    /// whole-document charge is its only ceiling: no ciphertext-sized copy per
    /// stream is allocated past the check.
    #[test]
    fn a_stream_is_decrypted_where_it_stands() {
        let plain = b"Secret ledger".as_slice();

        // RC4 and AES-128 keep the payload's own allocation, and round-trip.
        for method in [Method::Rc4, Method::Aes128] {
            let handler = handler(method);
            let aes = method == Method::Aes128;
            let key = handler.object_key(4, 0, aes);
            let ciphertext = if aes {
                let iv = [9u8; 16];
                let mut padded = plain.to_vec();
                padded.extend(std::iter::repeat_n(3u8, 3));
                let mut data = iv.to_vec();
                data.extend(aes128_cbc_encrypt(&key, &iv, &padded));
                data
            } else {
                rc4(&key, plain)
            };
            let mut value = PdfValue::Stream(HashMap::new(), ciphertext);
            let PdfValue::Stream(_, raw) = &value else {
                unreachable!()
            };
            let buffer = raw.as_ptr();
            handler.decrypt_object(4, 0, &mut value);
            let PdfValue::Stream(_, raw) = &value else {
                unreachable!()
            };
            assert_eq!(raw.as_ptr(), buffer, "{method:?}: decrypted in place");
            assert_eq!(raw.as_slice(), plain, "{method:?}");
        }

        // A stream the handler leaves in the clear is not copied either.
        let mut value = PdfValue::Stream(HashMap::new(), plain.to_vec());
        let PdfValue::Stream(_, raw) = &value else {
            unreachable!()
        };
        let buffer = raw.as_ptr();
        handler(Method::Identity).decrypt_object(4, 0, &mut value);
        let PdfValue::Stream(_, raw) = &value else {
            unreachable!()
        };
        assert_eq!(raw.as_ptr(), buffer, "Identity: not copied");
        assert_eq!(raw.as_slice(), plain);
    }

    #[test]
    fn a_non_standard_handler_is_unsupported_not_a_wrong_password() {
        let mut dict = HashMap::new();
        dict.insert("Filter".into(), PdfValue::Name("Adobe.PubSec".into()));
        let err = SecurityHandler::authenticate(&dict, b"", None).unwrap_err();
        assert!(matches!(err, CryptError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn a_stream_naming_its_own_crypt_filter_uses_that_filter() {
        let handler = SecurityHandler {
            key: vec![1u8; 16],
            strings: Method::Identity,
            streams: Method::Rc4,
            named: HashMap::from([("StdCF".to_string(), Method::Aes128)]),
            encrypt_metadata: true,
            opened_with: "user password",
        };
        let crypt_stream = |name: &str| {
            let mut parms = HashMap::new();
            parms.insert("Name".to_string(), PdfValue::Name(name.into()));
            let mut dict = HashMap::new();
            dict.insert("Filter".to_string(), PdfValue::Name("Crypt".into()));
            dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));
            dict
        };

        assert_eq!(
            handler.stream_method(&crypt_stream("StdCF")),
            Method::Aes128,
            "a named /Crypt filter overrides /StmF"
        );
        assert_eq!(
            handler.stream_method(&crypt_stream("Identity")),
            Method::Identity,
            "/Name /Identity leaves the stream in the clear"
        );
        assert_eq!(
            handler.stream_method(&crypt_stream("Missing")),
            Method::Rc4,
            "an undefined name keeps the file's default stream filter"
        );
        assert_eq!(
            handler.stream_method(&HashMap::new()),
            Method::Rc4,
            "a stream with no /Crypt filter takes /StmF"
        );
    }

    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// NIST SP 800-38A F.2.1: AES-128-CBC over the four-block sample.
    #[test]
    fn aes_128_cbc_matches_nist_sp_800_38a_f2_1() {
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = hex("000102030405060708090a0b0c0d0e0f");
        let plain = hex(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710"
        ));
        let ciphertext = hex(concat!(
            "7649abac8119b246cee98e9b12e9197d",
            "5086cb9b507219ee95db113a917678b2",
            "73bed6b8e3c1743b7116e69e22229516",
            "3ff1caa1681fac09120eca307586e1a7"
        ));

        assert_eq!(aes128_cbc_encrypt(&key, &iv, &plain), ciphertext);

        let mut data = iv;
        data.extend(&ciphertext);
        aes_cbc_decrypt_in_place(&key, &mut data);
        assert_eq!(data, plain);
    }

    /// NIST SP 800-38A F.2.5: the same sample under AES-256-CBC.
    #[test]
    fn aes_256_cbc_matches_nist_sp_800_38a_f2_5() {
        let key = hex(concat!(
            "603deb1015ca71be2b73aef0857d7781",
            "1f352c073b6108d72d9810a30914dff4"
        ));
        let iv = hex("000102030405060708090a0b0c0d0e0f");
        let plain = hex(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710"
        ));
        let ciphertext = hex(concat!(
            "f58c4c04d6e5f1ba779eabfb5f7bfbd6",
            "9cfc4e967edb808d679f777bc6702c7d",
            "39f23369a9d9bacfa530e26304231461",
            "b2eb05e2c39be9fcda6c19078c6a9d1b"
        ));

        assert_eq!(cbc_decrypt(&key, &iv, &ciphertext), plain);
    }

    /// Algorithm 2.B, pinned on the salts and passwords of the revision 6
    /// fixtures. Poppler authenticates the same `/U` and `/O` with its own
    /// implementation and reads those files, so the numbers are not only this
    /// build agreeing with itself.
    #[test]
    fn algorithm_2b_matches_the_pinned_hashes() {
        let u = hash_2b(b"user", b"uvalsalt", b"");
        assert_eq!(
            u,
            hex("ffdf5f2ee6b251856d72c8a0f38f37e9a745f7895ba8fadbd314415cc10fef82")
        );
        assert_eq!(
            hash_2b(b"owner", b"ovalsalt", &u),
            hex("996ca37132d803c090a1900aab0157639d4006d416ae986a64da49681b17dd06")
        );
    }

    #[test]
    fn a_cf_entry_without_a_known_method_is_read_by_the_specification() {
        let mut cf = HashMap::new();
        cf.insert("StdCF".to_string(), PdfValue::Dictionary(HashMap::new()));
        let mut odd = HashMap::new();
        odd.insert("CFM".to_string(), PdfValue::Name("AESV4".into()));
        cf.insert("Odd".to_string(), PdfValue::Dictionary(odd));
        let mut encrypt = HashMap::new();
        encrypt.insert("CF".to_string(), PdfValue::Dictionary(cf));

        let filters = crypt_filters(&encrypt);
        assert_eq!(
            filters.get("StdCF"),
            Some(&Method::Identity),
            "an absent /CFM is /None, the identity"
        );
        assert_eq!(
            filters.get("Odd"),
            None,
            "a method this build does not implement is left out, not guessed"
        );
    }

    #[test]
    fn padding_truncates_and_fills_to_32_bytes() {
        assert_eq!(padded(b""), PAD.to_vec());
        assert_eq!(&padded(b"ab")[..2], b"ab");
        assert_eq!(padded(&[1u8; 40]), vec![1u8; 32]);
    }

    #[test]
    fn a_legacy_password_is_written_in_pdfdocencoding() {
        // The two letters qpdf wrote into the revision 4 fixture.
        assert_eq!(
            pdfdoc_encoding("Grüße"),
            Some(vec![0x47, 0x72, 0xFC, 0xDF, 0x65])
        );
        // Slots Latin-1 does not have: the euro sits at 0xA0, and a right
        // single quote at 0x90 rather than at its Latin-1 byte.
        assert_eq!(pdfdoc_encoding("€"), Some(vec![0xA0]));
        assert_eq!(pdfdoc_encoding("\u{2019}"), Some(vec![0x90]));
        // A character with no slot cannot be a stored password.
        assert_eq!(pdfdoc_encoding("日本"), None);
        assert_eq!(pdfdoc_encoding("\u{00A0}"), None, "no-break space");
        assert_eq!(pdfdoc_encoding("\u{00AD}"), None, "soft hyphen");
    }

    #[test]
    fn a_revision_six_password_is_prepared_with_saslprep() {
        // RFC 4013's own examples: B.1 drops the soft hyphen, and NFKC turns
        // the ordinal indicator and the Roman numeral into their letters.
        assert_eq!(saslprep("I\u{00AD}X"), "IX");
        assert_eq!(saslprep("\u{00AA}"), "a");
        assert_eq!(saslprep("\u{2168}"), "IX");
        // A non-ASCII space becomes a space, and a decomposed letter composes.
        assert_eq!(saslprep("a\u{00A0}b"), "a b");
        assert_eq!(saslprep("pa\u{0308}ss"), "päss");
    }
}
