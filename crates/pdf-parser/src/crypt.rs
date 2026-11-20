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
    /// No password tried (the empty one included) opens the file.
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
    pub fn authenticate(
        encrypt: &HashMap<String, PdfValue>,
        id0: &[u8],
        password: Option<&[u8]>,
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
        let encrypt_metadata = !matches!(encrypt.get("EncryptMetadata"), Some(PdfValue::Boolean(false)));
        let password = password.unwrap_or(b"");

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

        let handler = |key: Vec<u8>, opened_with| Self {
            key,
            strings,
            streams,
            encrypt_metadata,
            opened_with,
        };

        match r {
            2..=4 => {
                if let Some(key) = legacy_user(password, &o, &u, p, id0, r, length, encrypt_metadata) {
                    return Ok(handler(key, "user password"));
                }
                // As the owner: algorithm 7 recovers the user password from /O.
                let user = legacy_owner_to_user(password, &o, r, length);
                if let Some(key) = legacy_user(&user, &o, &u, p, id0, r, length, encrypt_metadata) {
                    return Ok(handler(key, "owner password"));
                }
                Err(CryptError::WrongPassword)
            }
            5 | 6 => {
                let ue = bytes_of(encrypt.get("UE")).unwrap_or_default();
                let oe = bytes_of(encrypt.get("OE")).unwrap_or_default();
                // Revision 6 passwords are UTF-8 (SASLprep is not applied: an
                // ASCII password, the overwhelming case, is unchanged by it),
                // truncated to 127 bytes.
                let password = &password[..password.len().min(127)];
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
                    return Ok(handler(aes256_cbc_no_iv_decrypt(&k, &ue[..32]), "user password"));
                }
                if hash(password, &o[32..40], &u[..48])[..] == o[..32] {
                    let k = hash(password, &o[40..48], &u[..48]);
                    return Ok(handler(aes256_cbc_no_iv_decrypt(&k, &oe[..32]), "owner password"));
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
            let kind = dict.get("Type").and_then(PdfValue::as_name);
            let exempt = kind == Some("XRef") || (kind == Some("Metadata") && !self.encrypt_metadata);
            if !exempt {
                let method = match stream_crypt_filter(dict) {
                    Some(StreamFilter::Identity) => Method::Identity,
                    _ => self.streams,
                };
                *raw = self.decrypt(method, num, gen, raw);
            }
        }
        self.decrypt_strings(num, gen, value);
    }

    fn decrypt_strings(&self, num: usize, gen: i32, value: &mut PdfValue) {
        match value {
            PdfValue::String(s) => {
                let plain = self.decrypt(self.strings, num, gen, &latin1_bytes(s));
                *s = latin1_string(&plain);
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

    fn decrypt(&self, method: Method, num: usize, gen: i32, data: &[u8]) -> Vec<u8> {
        match method {
            Method::Identity => data.to_vec(),
            Method::Rc4 => rc4(&self.object_key(num, gen, false), data),
            Method::Aes128 => aes_cbc_decrypt(&self.object_key(num, gen, true), data),
            Method::Aes256 => aes_cbc_decrypt(&self.key, data),
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

/// A stream's own `/Filter /Crypt` entry, which overrides `/StmF`.
enum StreamFilter {
    Identity,
    Named,
}

fn stream_crypt_filter(dict: &HashMap<String, PdfValue>) -> Option<StreamFilter> {
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
    let name = parms.and_then(|p| p.get("Name")).and_then(PdfValue::as_name);
    Some(match name {
        None | Some("Identity") => StreamFilter::Identity,
        Some(_) => StreamFilter::Named,
    })
}

/// The method a `/V 4`/`/V 5` handler names under `/StrF` or `/StmF`.
fn crypt_filter(encrypt: &HashMap<String, PdfValue>, key: &str) -> Result<Method, CryptError> {
    let name = encrypt.get(key).and_then(PdfValue::as_name).unwrap_or("Identity");
    if name == "Identity" {
        return Ok(Method::Identity);
    }
    let cfm = encrypt
        .get("CF")
        .and_then(PdfValue::as_dict)
        .and_then(|cf| cf.get(name))
        .and_then(PdfValue::as_dict)
        .and_then(|f| f.get("CFM"))
        .and_then(PdfValue::as_name)
        .unwrap_or("None");
    match cfm {
        "None" => Ok(Method::Identity),
        "V2" => Ok(Method::Rc4),
        "AESV2" => Ok(Method::Aes128),
        "AESV3" => Ok(Method::Aes256),
        other => Err(CryptError::Unsupported(format!("crypt filter method /{other}"))),
    }
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
        let k1: Vec<u8> = block.iter().copied().cycle().take(block.len() * 64).collect();
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
    if key.is_empty() {
        return data.to_vec();
    }
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|&byte| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            byte ^ s[s[i as usize].wrapping_add(s[j as usize]) as usize]
        })
        .collect()
}

/// AES-CBC with the IV in the first block and PKCS#7 padding, as a PDF writes
/// it. A payload too short for an IV, or whose padding is malformed, comes back
/// as best decrypted rather than lost: a damaged stream is the filters' problem.
fn aes_cbc_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    if data.len() < 32 {
        return Vec::new();
    }
    let (iv, body) = data.split_at(16);
    let body = &body[..body.len() - body.len() % 16];
    let mut out = cbc_decrypt(key, iv, body);
    if let Some(&n) = out.last() {
        let n = n as usize;
        if (1..=16).contains(&n) && n <= out.len() && out[out.len() - n..].iter().all(|&b| b as usize == n) {
            out.truncate(out.len() - n);
        }
    }
    out
}

/// `/UE` and `/OE`: AES-256-CBC with a zero IV and no padding.
fn aes256_cbc_no_iv_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    cbc_decrypt(key, &[0u8; 16], data)
}

fn cbc_decrypt(key: &[u8], iv: &[u8], body: &[u8]) -> Vec<u8> {
    enum Cipher {
        A128(aes::Aes128),
        A256(aes::Aes256),
    }
    let cipher = match key.len() {
        16 => Cipher::A128(aes::Aes128::new(GenericArray::from_slice(key))),
        32 => Cipher::A256(aes::Aes256::new(GenericArray::from_slice(key))),
        _ => return Vec::new(),
    };
    let mut prev = [0u8; 16];
    prev.copy_from_slice(iv);
    let mut out = Vec::with_capacity(body.len());
    for chunk in body.chunks_exact(16) {
        let mut block = GenericArray::clone_from_slice(chunk);
        match &cipher {
            Cipher::A128(c) => c.decrypt_block(&mut block),
            Cipher::A256(c) => c.decrypt_block(&mut block),
        }
        for (b, p) in block.iter_mut().zip(prev.iter()) {
            *b ^= p;
        }
        out.extend_from_slice(&block);
        prev.copy_from_slice(chunk);
    }
    out
}

/// AES-128-CBC encryption with no padding (the input is a multiple of 16).
fn aes128_cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    let cipher = aes::Aes128::new(GenericArray::from_slice(key));
    let mut prev = [0u8; 16];
    prev.copy_from_slice(iv);
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(16) {
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
        padded.extend(std::iter::repeat(3u8).take(3));
        let mut data = iv.to_vec();
        data.extend(aes128_cbc_encrypt(&key, &iv, &padded));
        assert_eq!(aes_cbc_decrypt(&key, &data), plain);
    }

    #[test]
    fn a_non_standard_handler_is_unsupported_not_a_wrong_password() {
        let mut dict = HashMap::new();
        dict.insert("Filter".into(), PdfValue::Name("Adobe.PubSec".into()));
        let err = SecurityHandler::authenticate(&dict, b"", None).unwrap_err();
        assert!(matches!(err, CryptError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn padding_truncates_and_fills_to_32_bytes() {
        assert_eq!(padded(b""), PAD.to_vec());
        assert_eq!(&padded(b"ab")[..2], b"ab");
        assert_eq!(padded(&[1u8; 40]), vec![1u8; 32]);
    }
}
