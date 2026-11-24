//! The security handler against mutated `/Encrypt` dictionaries and ciphertext.
//!
//! The `crypt` fuzz target walks the same two shapes under libFuzzer; this is
//! the deterministic half. A seeded mutation loop feeds the committed fixtures
//! (mutated ciphertext under a real, authenticated handler) and synthesized
//! security dictionaries straight to `SecurityHandler::authenticate`, and
//! asserts that every failure comes back as a `CryptError` — or a `LimitError`
//! from the budget — rather than a panic or a partially read document. The RNG
//! is xorshift64*, so a failing iteration replays from this seed alone.

use std::collections::HashMap;

use pdfextractor_limits::{LimitError, Limits};
use pdfextractor_pdf_parser::crypt::{latin1_string, CryptError, SecurityHandler};
use pdfextractor_pdf_parser::objects::PdfValue;
use pdfextractor_pdf_parser::PdfDocument;

const FIXTURES: [&str; 4] = [
    "encrypted.pdf",
    "encrypted_rc4_128.pdf",
    "encrypted_aes128.pdf",
    "encrypted_aes256.pdf",
];

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("../../test-data/{name}")).expect("the fixture is committed")
}

/// xorshift64*: one seed replays every case.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }
}

/// Whether a refusal carries the parser's or the handler's own error type.
fn typed(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut cause = Some(error);
    while let Some(error) = cause {
        if error.downcast_ref::<CryptError>().is_some()
            || error.downcast_ref::<LimitError>().is_some()
        {
            return true;
        }
        cause = error.source();
    }
    false
}

/// A mutated `/Encrypt` dictionary: the keys the standard handler reads, each
/// sometimes present, with the values a mutation would leave.
fn security_dictionary(rng: &mut Rng) -> HashMap<String, PdfValue> {
    let mut dict = HashMap::new();
    let filter = if rng.below(8) == 0 {
        "Adobe.PubSec"
    } else {
        "Standard"
    };
    dict.insert("Filter".into(), PdfValue::Name(filter.into()));
    dict.insert("V".into(), PdfValue::Integer(rng.below(6) as i64));
    dict.insert("R".into(), PdfValue::Integer(rng.below(8) as i64));
    dict.insert(
        "Length".into(),
        PdfValue::Integer([40, 128, 256][rng.below(3)] as i64),
    );
    dict.insert(
        "P".into(),
        PdfValue::Integer(rng.next() as u32 as i32 as i64),
    );
    // `/O` and `/U` are 32 or 48 bytes; a mutation can leave any length.
    let o_len = [0, 32, 48][rng.below(3)];
    let o = rng.bytes(o_len);
    let u_len = [0, 32, 48][rng.below(3)];
    let u = rng.bytes(u_len);
    dict.insert("O".into(), PdfValue::String(latin1_string(&o)));
    dict.insert("U".into(), PdfValue::String(latin1_string(&u)));
    if rng.below(2) == 0 {
        dict.insert(
            "EncryptMetadata".into(),
            PdfValue::Boolean(rng.below(2) == 0),
        );
    }
    if rng.below(3) == 0 {
        let mut std_cf = HashMap::new();
        let method = ["V2", "AESV2", "AESV3", "None"][rng.below(4)];
        std_cf.insert("CFM".into(), PdfValue::Name(method.into()));
        std_cf.insert("Length".into(), PdfValue::Integer(rng.below(64) as i64));
        let mut cf = HashMap::new();
        cf.insert("StdCF".into(), PdfValue::Dictionary(std_cf));
        dict.insert("CF".into(), PdfValue::Dictionary(cf));
        dict.insert("StmF".into(), PdfValue::Name("StdCF".into()));
        dict.insert("StrF".into(), PdfValue::Name("StdCF".into()));
    }
    dict
}

/// The bytes a mutation may change without touching the file's syntax: every
/// byte of every stream payload (what the handler decrypts), and every hex
/// digit of the `/Encrypt` object's string fields (the salts and stored hashes
/// authentication is computed from).
///
/// A mutation anywhere else spends a case on structure — a broken offset, a
/// truncated dictionary — which never reaches the cipher, so the loop below
/// picks from here instead, and picks one class per case: a payload case stays
/// in the ciphertext, a hex case stays in `/Encrypt`, and neither can turn the
/// other into a password refusal.
fn targets(bytes: &[u8]) -> (Vec<usize>, Vec<usize>) {
    let mut payloads = Vec::new();
    let mut hex = Vec::new();

    // Stream payloads: after the EOL that follows `stream`, up to `endstream`.
    let mut at = 0;
    while let Some(found) = bytes[at..].windows(6).position(|w| w == b"stream") {
        let keyword = at + found;
        if keyword >= 3 && &bytes[keyword - 3..keyword] == b"end" {
            at = keyword + 6;
            continue;
        }
        let mut start = keyword + 6;
        while start < bytes.len() && matches!(bytes[start], b'\r' | b'\n') {
            start += 1;
        }
        let Some(end_found) = bytes[start..].windows(9).position(|w| w == b"endstream") else {
            break;
        };
        let mut end = start + end_found;
        while end > start && matches!(bytes[end - 1], b'\r' | b'\n') {
            end -= 1;
        }
        payloads.extend(start..end);
        at = start + end_found + 9;
    }

    if let Some(range) = encrypt_dictionary(bytes) {
        let mut i = range.start;
        while i < range.end {
            if bytes[i] == b'<' && bytes.get(i + 1).is_some_and(|&next| next != b'<') {
                if let Some(close) = bytes[i + 1..range.end].iter().position(|&b| b == b'>') {
                    hex.extend(i + 1..i + 1 + close);
                    i = i + 1 + close;
                }
            }
            i += 1;
        }
    }
    (payloads, hex)
}

/// The body of the object the trailer's `/Encrypt` names.
fn encrypt_dictionary(bytes: &[u8]) -> Option<std::ops::Range<usize>> {
    let at = bytes.windows(8).position(|w| w == b"/Encrypt")?;
    let rest = &bytes[at + 8..];
    let start = rest.iter().position(u8::is_ascii_digit)?;
    let number: usize = rest[start..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .map(|&b| b as char)
        .collect::<String>()
        .parse()
        .ok()?;
    let header = format!("{number} 0 obj");
    let header_at = bytes
        .windows(header.len())
        .position(|w| w == header.as_bytes())?;
    let dict_start = header_at + header.len();
    let dict_end = dict_start
        + bytes[dict_start..]
            .windows(6)
            .position(|w| w == b"endobj")?;
    Some(dict_start..dict_end)
}

#[test]
fn every_mutation_is_read_or_refused_with_a_typed_error() {
    let mut rng = Rng(0x0dec_0ded_5eed_1234);
    let limits = Limits::default();

    for name in FIXTURES {
        let base = fixture(name);
        let (payloads, hex) = targets(&base);
        assert!(!payloads.is_empty(), "{name}: the ciphertext is reachable");
        assert!(
            !hex.is_empty(),
            "{name}: the /Encrypt strings are reachable"
        );
        // The password the fixture actually opens with: a payload mutation
        // leaves `/Encrypt` alone, so authentication must still succeed and the
        // ciphertext reaches the cipher. `encrypted.pdf` has an empty user
        // password; the variants use `user`.
        let good = if name == "encrypted.pdf" {
            None
        } else {
            Some("user")
        };
        for _ in 0..64 {
            let mut bytes = base.clone();
            // Three cases in four mutate only ciphertext; a hex case mutates
            // only `/Encrypt`, so authentication is what it exercises.
            let payload_only = rng.below(4) != 0;
            let pick = if payload_only { &payloads } else { &hex };
            for _ in 0..1 + rng.below(4) {
                let at = pick[rng.below(pick.len())];
                if payload_only {
                    bytes[at] ^= (rng.next() as u8) | 1;
                } else {
                    bytes[at] = b"0123456789abcdef"[rng.below(16)];
                }
            }
            for password in [None, Some("user"), Some("owner")] {
                match PdfDocument::parse_with_limits_and_password(&bytes, limits.clone(), password)
                {
                    Ok(document) => {
                        if password == good && payload_only {
                            assert!(
                                document.locked.is_none(),
                                "{name}: a payload mutation blocked authentication: {:?}",
                                document.locked
                            );
                        }
                        if let Err(error) = document.extract_checked() {
                            assert!(
                                typed(error.as_ref()),
                                "{name}: an untyped refusal: {error:#}"
                            );
                        }
                    }
                    Err(error) => assert!(
                        typed(error.as_ref()),
                        "{name}: an untyped refusal: {error:#}"
                    ),
                }
            }
        }
    }

    // Synthesized security dictionaries straight to the handler: every refusal
    // is a `CryptError` by signature, and a handler that does authenticate
    // decrypts arbitrary payloads without a panic.
    for _ in 0..256 {
        let dict = security_dictionary(&mut rng);
        let id = rng.bytes(16);
        let id_text = latin1_string(&id);
        for password in [None, Some("user"), Some(id_text.as_str())] {
            match SecurityHandler::authenticate(&dict, &id, password) {
                Ok(handler) => {
                    let payload_len = 1 + rng.below(256);
                    let payload = rng.bytes(payload_len);
                    let mut stream = PdfValue::Stream(HashMap::new(), payload);
                    handler.decrypt_object(4, 0, &mut stream);
                    let _plaintext = handler.plaintext_len(&stream);
                }
                Err(error) => assert!(
                    matches!(
                        error,
                        CryptError::WrongPassword | CryptError::Unsupported(_)
                    ),
                    "a refusal the caller cannot name: {error:?}"
                ),
            }
        }
    }
}
