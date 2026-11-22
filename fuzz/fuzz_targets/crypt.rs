#![no_main]

//! The standard security handler against mutated `/Encrypt` dictionaries and
//! mutated stream ciphertext.
//!
//! Two shapes run per input. The committed fixtures keep their real `/Encrypt`
//! dictionary, so the handler authenticates for real and then decrypts payloads
//! overwritten with the input; and the input is spliced into a minimal
//! `/Encrypt` dictionary, which the handler authenticates against and mostly
//! refuses. A refusal must be typed — the target asserts the error chain holds
//! a `CryptError` or a `LimitError` — so an untyped failure, a panic or a
//! partially read document is the finding.

use libfuzzer_sys::fuzz_target;
use pdfextractor_fuzz::{limits, note};
use pdfextractor_limits::LimitError;
use pdfextractor_pdf_parser::crypt::CryptError;
use pdfextractor_pdf_parser::PdfDocument;

/// A real encrypted file per cipher family: RC4-40 opens with the empty
/// password, AES-256 with `/U`'s password, so both reach `decrypt_object`.
const FIXTURES: [(&str, &[u8]); 2] = [
    (
        "rc4_40",
        include_bytes!("../../test-data/encrypted.pdf"),
    ),
    (
        "aes256",
        include_bytes!("../../test-data/encrypted_aes256.pdf"),
    ),
];

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

/// Overwrite every stream payload with `noise`, keeping each payload's length
/// and the file's structure: `/Length`, the dictionaries and `/Encrypt` are
/// untouched, so the handler authenticates and then decrypts arbitrary bytes.
fn overwrite_streams(bytes: &mut [u8], noise: &[u8]) -> usize {
    let mut streams = 0;
    let mut at = 0;
    while let Some(found) = bytes[at..].windows(6).position(|window| window == b"stream") {
        let mut start = at + found + 6;
        while start < bytes.len() && (bytes[start] == b'\r' || bytes[start] == b'\n') {
            start += 1;
        }
        let Some(end_found) = bytes[start..].windows(9).position(|window| window == b"endstream") else {
            break;
        };
        let mut end = start + end_found;
        while end > start && (bytes[end - 1] == b'\r' || bytes[end - 1] == b'\n') {
            end -= 1;
        }
        if end > start && !noise.is_empty() {
            for (index, byte) in bytes[start..end].iter_mut().enumerate() {
                *byte = noise[(streams + index) % noise.len()];
            }
            streams += 1;
        }
        at = start + end_found + 9;
    }
    streams
}

/// A one-page PDF whose `/Encrypt` dictionary body is `body`, with a stream
/// holding the same bytes: whatever the dictionary asks for, there is a
/// payload for the handler to decrypt when it authenticates.
fn with_encrypt_dictionary(body: &[u8]) -> Vec<u8> {
    let mut pdf = Vec::with_capacity(body.len() * 2 + 512);
    pdf.extend_from_slice(
        b"%PDF-1.7\n\
          1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
          2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
          3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>\nendobj\n\
          4 0 obj\n<< /Length 4 >>\nstream\n\x00\x00\x00\x00\nendstream\nendobj\n",
    );
    pdf.extend_from_slice(b"5 0 obj\n<< /Filter /Standard ");
    pdf.extend_from_slice(body);
    pdf.extend_from_slice(b" >>\nendobj\n");
    pdf.extend_from_slice(format!("6 0 obj\n<< /Length {} >>\nstream\n", body.len()).as_bytes());
    pdf.extend_from_slice(body);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    pdf.extend_from_slice(
        b"trailer\n<< /Size 7 /Root 1 0 R /Encrypt 5 0 R \
          /ID [<00112233445566778899aabbccddeeff><00112233445566778899aabbccddeeff>] >>\n%%EOF\n",
    );
    pdf
}

fuzz_target!(|data: &[u8]| {
    let limits = limits();

    // Real handler, mutated ciphertext: the fixture's own `/Encrypt` opens, so
    // every stream below is decrypted from bytes the fuzzer chose.
    for (name, fixture) in FIXTURES {
        let mut bytes = fixture.to_vec();
        let streams = overwrite_streams(&mut bytes, data);
        // The empty input has nothing to write; any other input must land in a
        // payload, or the target has stopped reaching the cipher.
        assert!(
            data.is_empty() || streams > 0,
            "{name}: the fixture carries streams"
        );
        // The passwords the two fixtures actually open with: the empty one and
        // AES-256's `/U` password. Other refusals are the aggregate test's job.
        for password in [None, Some(&b"user"[..])] {
            let parsed =
                PdfDocument::parse_with_limits_and_password(&bytes, limits.clone(), password);
            match &parsed {
                Ok(document) => {
                    let pages = document.extract_checked();
                    if let Err(error) = &pages {
                        assert!(typed(error.as_ref()), "{name}: untyped refusal: {error:#}");
                    }
                    note(name, &pages);
                }
                Err(error) => assert!(typed(error.as_ref()), "{name}: untyped refusal: {error:#}"),
            }
            note(name, &parsed);
        }
    }

    // Mutated `/Encrypt` dictionary: the handler reads it, refuses it, or (with
    // a dictionary the mutation made consistent) decrypts the stream after it.
    let body = &data[..data.len().min(4096)];
    let pdf = with_encrypt_dictionary(body);
    for password in [None, Some(data)] {
        match PdfDocument::parse_with_limits_and_password(&pdf, limits.clone(), password) {
            Ok(document) => {
                let pages = document.extract_checked();
                if let Err(error) = &pages {
                    assert!(typed(error.as_ref()), "dict: untyped refusal: {error:#}");
                }
                note("crypt dict", &pages);
            }
            Err(error) => assert!(typed(error.as_ref()), "dict: untyped refusal: {error:#}"),
        }
    }
});
