#![no_main]

//! Encrypted PDFs under budgets and a password the input itself derives.
//!
//! The input is cut into three parts: the sixteen header bytes `budgets` reads
//! as `Limits`, a length-prefixed password, and the document body. Those
//! budgets and that password then parse two files:
//!
//! * a minimal PDF whose `/Encrypt` dictionary body is the input's bytes, so
//!   the handler meets missing entries, wrong types, short salts and revisions
//!   it does not implement; and
//! * a well-formed revision-6 handler (`/V 5 /R 6`) whose `/O`, `/U`, `/OE`
//!   and `/UE` are cut from the input, so algorithm 2.B — the deliberately
//!   slow revision-6 password hash — runs for the user and the owner check on
//!   every input whose budgets let the parse begin, on salts and a password
//!   the fuzzer chose.
//!
//! A parsed file is extracted: that is where a refused password (`CryptError`)
//! and the decryption charge (`LimitError`) surface, and an untyped refusal
//! there is the finding. A refusal while parsing the spliced dictionary is not
//! held to that, because the body can break the PDF's structure itself and a
//! structural refusal is the parser's answer rather than the handler's.

use libfuzzer_sys::fuzz_target;
use pdfextractor_fuzz::{limits_from, note, BUDGET_HEADER_BYTES};
use pdfextractor_limits::{LimitError, Limits};
use pdfextractor_pdf_parser::crypt::CryptError;
use pdfextractor_pdf_parser::PdfDocument;

/// The longest password kept from the input. The handler truncates at 127
/// bytes itself; this keeps the hash's work bounded by the header instead of
/// by the whole input.
const MAX_PASSWORD_BYTES: usize = 64;

/// The budgets, the password and the body after both.
///
/// An input shorter than the budget header has room for neither a header nor a
/// password: it keeps the product's budgets, tries the empty password and is
/// the body whole, which is the same boundary `budgets` has.
fn split(data: &[u8]) -> (Limits, Option<String>, &[u8]) {
    if data.len() < BUDGET_HEADER_BYTES {
        return (Limits::default(), None, data);
    }
    let (limits, rest) = limits_from(data);
    let Some((&length, rest)) = rest.split_first() else {
        return (limits, None, rest);
    };
    let take = usize::from(length).min(MAX_PASSWORD_BYTES).min(rest.len());
    let (password, body) = rest.split_at(take);
    // One character per byte, so the control and non-ASCII characters a
    // mutation makes are exactly what PDFDocEncoding and SASLprep are handed.
    let password: String = password.iter().map(|&byte| byte as char).collect();
    (limits, Some(password), body)
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

/// Hex for `count` bytes of `body` taken in a ring from `start`, so a mutation
/// anywhere in the body reaches a salt or a stored hash.
fn hex_from(body: &[u8], count: usize, start: usize) -> String {
    let mut hex = String::with_capacity(count * 2);
    for index in 0..count {
        let byte = match body.len() {
            0 => 0,
            len => body[(start + index) % len],
        };
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// A one-page PDF whose `/Encrypt` dictionary body is `body`: whatever the
/// mutation asks for, the handler reads a dictionary and there is a stream for
/// it to decrypt when it authenticates.
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

/// A one-page PDF with a revision-6 handler whose 48-byte `/O` and `/U` and
/// 32-byte `/OE` and `/UE` are cut from `body`. The salts and stored hashes are
/// the fuzzer's, while the dictionary is well formed, so authentication reaches
/// algorithm 2.B instead of stopping at a missing or short entry.
fn revision_six(body: &[u8]) -> Vec<u8> {
    let o = hex_from(body, 48, 0);
    let u = hex_from(body, 48, 48);
    let oe = hex_from(body, 32, 96);
    let ue = hex_from(body, 32, 128);
    format!(
        "%PDF-1.7\n\
         1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
         2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
         3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>\nendobj\n\
         4 0 obj\n<< /Length 4 >>\nstream\n\x00\x00\x00\x00\nendstream\nendobj\n\
         5 0 obj\n<< /Filter /Standard /V 5 /R 6 /Length 256 /P -4 \
         /O <{o}> /U <{u}> /OE <{oe}> /UE <{ue}> \
         /CF << /StdCF << /CFM /AESV3 /Length 32 >> >> /StmF /StdCF /StrF /StdCF >>\nendobj\n\
         trailer\n<< /Size 6 /Root 1 0 R /Encrypt 5 0 R \
         /ID [<00112233445566778899aabbccddeeff><00112233445566778899aabbccddeeff>] >>\n%%EOF\n"
    )
    .into_bytes()
}

fuzz_target!(|data: &[u8]| {
    let (limits, password, body) = split(data);
    let password = password.as_deref();

    // The input spliced into `/Encrypt`: a malformed dictionary is the point,
    // so a structural refusal from the parse is noted rather than asserted
    // typed. A parsed file is still held to a typed extraction refusal.
    let pdf = with_encrypt_dictionary(&body[..body.len().min(4096)]);
    let parsed = PdfDocument::parse_with_limits_and_password(&pdf, limits.clone(), password);
    note("encrypt dictionary", &parsed);
    if let Ok(document) = &parsed {
        let pages = document.extract_checked();
        if let Err(error) = &pages {
            assert!(
                typed(error.as_ref()),
                "encrypt dictionary: untyped refusal: {error:#}"
            );
        }
        note("encrypt dictionary pages", &pages);
    }

    // A well-formed revision-6 handler over the input's salts and password: the
    // parse has no structural failure to make, so every refusal here is the
    // handler's or a budget's.
    let pdf = revision_six(body);
    match PdfDocument::parse_with_limits_and_password(&pdf, limits, password) {
        Ok(document) => {
            let pages = document.extract_checked();
            if let Err(error) = &pages {
                assert!(
                    typed(error.as_ref()),
                    "revision 6: untyped refusal: {error:#}"
                );
            }
            note("revision 6 pages", &pages);
        }
        Err(error) => assert!(
            typed(error.as_ref()),
            "revision 6: untyped parse refusal: {error:#}"
        ),
    }
});
