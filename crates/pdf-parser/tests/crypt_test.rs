//! The standard security handler against real encrypted files.
//!
//! The fixtures are written by `test-data/make_encrypted_pdf.py` (RC4 40-bit,
//! revision 2, empty user password) and `test-data/make_encrypted_variants.py`
//! (RC4 128-bit revision 3, AES-128 revision 4 and AES-256 revision 6, each with
//! user password `user` and owner password `owner`). Poppler's `pdftotext`
//! reads every one of them back, so these are files a real reader accepts.

use pdfextractor_limits::{LimitError, Limits};
use pdfextractor_pdf_parser::crypt::CryptError;
use pdfextractor_pdf_parser::objects::PdfValue;
use pdfextractor_pdf_parser::PdfDocument;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("../../test-data/{name}")).expect("the fixture is committed")
}

fn page_text(doc: &PdfDocument) -> String {
    doc.extract()
        .iter()
        .flat_map(|page| page.spans.iter().map(|span| span.text.clone()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `/Title` of the `/Info` dictionary (object 7 in the variants).
fn info_title(doc: &PdfDocument) -> Option<String> {
    match doc.objects.get(&(7, 0))?.get("Title")? {
        PdfValue::String(s) => Some(s.clone()),
        _ => None,
    }
}

const VARIANTS: [&str; 3] = [
    "encrypted_rc4_128.pdf",
    "encrypted_aes128.pdf",
    "encrypted_aes256.pdf",
];

#[test]
fn an_empty_user_password_opens_without_asking() {
    let doc = PdfDocument::parse(&fixture("encrypted.pdf")).expect("parse");
    assert!(doc.encrypted);
    assert_eq!(doc.locked, None);
    assert_eq!(doc.opened_with, Some("user password"));
    assert!(
        page_text(&doc).contains("Secret ledger"),
        "{}",
        page_text(&doc)
    );
}

#[test]
fn every_handler_opens_with_the_user_password() {
    for name in VARIANTS {
        let doc = PdfDocument::parse_with_password(&fixture(name), Some("user")).expect("parse");
        assert!(doc.encrypted, "{name}");
        assert_eq!(doc.locked, None, "{name}");
        assert_eq!(doc.opened_with, Some("user password"), "{name}");
        assert!(
            page_text(&doc).contains("Secret ledger"),
            "{name}: {}",
            page_text(&doc)
        );
        assert_eq!(
            info_title(&doc).as_deref(),
            Some("Ciphered title"),
            "{name}: strings"
        );
    }
}

#[test]
fn every_handler_opens_with_the_owner_password() {
    for name in VARIANTS {
        let doc = PdfDocument::parse_with_password(&fixture(name), Some("owner")).expect("parse");
        assert_eq!(doc.locked, None, "{name}");
        assert_eq!(doc.opened_with, Some("owner password"), "{name}");
        assert!(page_text(&doc).contains("Secret ledger"), "{name}");
    }
}

#[test]
fn a_missing_or_wrong_password_is_locked_and_reads_no_noise() {
    for name in VARIANTS {
        for password in [None, Some("guess")] {
            let doc = PdfDocument::parse_with_password(&fixture(name), password).expect("parse");
            assert!(doc.encrypted, "{name}");
            assert_eq!(
                doc.locked,
                Some(CryptError::WrongPassword),
                "{name} {password:?}"
            );
            assert_eq!(doc.opened_with, None);
        }
    }
}

/// A file the password does not open still parses — `locked` says why — but its
/// pages must not be handed back: they would be read from ciphertext and look
/// like text. The checked reader reports the reason the command line reports.
#[test]
fn a_locked_document_is_refused_rather_than_read_as_ciphertext() {
    for name in VARIANTS {
        // The fixture has a page, so the refusal is the lock and not an empty file.
        let opened = PdfDocument::parse_with_password(&fixture(name), Some("user"))
            .expect("parse with the password");
        assert!(
            !opened.extract().is_empty(),
            "{name}: the fixture has a page"
        );

        let doc = PdfDocument::parse_with_password(&fixture(name), Some("guess")).expect("parse");
        let error = doc
            .extract_checked()
            .expect_err("a locked document must not yield pages");
        let refusal = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<CryptError>())
            .unwrap_or_else(|| panic!("{name}: the refusal carries the crypt error: {error:#}"));
        assert_eq!(refusal, &CryptError::WrongPassword, "{name}");
        assert!(
            error.to_string().contains("ciphertext"),
            "{name}: the refusal says the pages were never decrypted: {error:#}"
        );
        assert!(
            doc.extract().is_empty(),
            "{name}: the un-fallible facade yields no pages either"
        );
    }
}

/// A PDF 1.5 file can state its encryption only in a cross-reference stream's
/// own dictionary, which need not carry an `/ID`. The bytes are still
/// ciphertext: the entry has to be detected on its own, and the handler's
/// refusal — there is no `/ID` to authenticate against — has to be reported
/// rather than the payload handed back as text.
#[test]
fn an_xref_stream_encrypt_without_an_id_is_refused_not_read() {
    let doc = PdfDocument::parse(&fixture("xref_encrypt_no_id.pdf")).expect("parse");
    assert!(doc.encrypted, "an xref stream's /Encrypt is detected");
    assert_eq!(doc.opened_with, None, "no password opened it");
    assert_eq!(
        doc.locked,
        Some(CryptError::WrongPassword),
        "the handler cannot authenticate without an /ID"
    );

    let error = doc
        .extract_checked()
        .expect_err("ciphertext must not be returned as text");
    let refusal = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<CryptError>())
        .unwrap_or_else(|| panic!("the refusal carries the crypt error: {error:#}"));
    assert_eq!(refusal, &CryptError::WrongPassword);
    assert!(
        error.to_string().contains("ciphertext"),
        "the refusal says the pages were never decrypted: {error:#}"
    );
    assert!(
        doc.extract().is_empty(),
        "the un-fallible facade yields no pages either"
    );
}

/// qpdf wrote this revision 4 file with the password `Grüße`: the file holds it
/// in PDFDocEncoding, so the same text has to be encoded that way before it is
/// hashed — the argv's UTF-8 bytes are not it, and the file used to stay
/// locked.
#[test]
fn a_pdfdocencoding_password_opens_a_revision_four_file() {
    let doc = PdfDocument::parse_with_password(&fixture("encrypted_pdfdoc.pdf"), Some("Grüße"))
        .expect("parse");
    assert_eq!(doc.opened_with, Some("user password"));
    assert!(
        page_text(&doc).contains("Hello World"),
        "{}",
        page_text(&doc)
    );

    // A password the encoding cannot write is not the one the file holds.
    let doc = PdfDocument::parse_with_password(&fixture("encrypted_pdfdoc.pdf"), Some("日本語"))
        .expect("parse");
    assert_eq!(doc.locked, Some(CryptError::WrongPassword));
}

/// qpdf wrote this revision 6 file with the composed `päss`; a reader that
/// applies SASLprep opens it with the decomposed text a keyboard can produce as
/// well, because NFKC is what the profile does to both.
#[test]
fn an_r6_password_is_prepared_with_saslprep() {
    for password in ["päss", "pa\u{0308}ss"] {
        let doc =
            PdfDocument::parse_with_password(&fixture("encrypted_r6_saslprep.pdf"), Some(password))
                .expect("parse");
        assert_eq!(doc.locked, None, "{password:?}");
        assert!(page_text(&doc).contains("Hello World"), "{password:?}");
    }
}

/// The refusal is the state, not only the recorded error: a file that says it
/// is encrypted with no handler's success behind it is refused as well, so a
/// gap in the parser cannot produce a partial read of ciphertext. The state is
/// built by hand because today's parser sets `locked` whenever it sets
/// `encrypted`; this holds the invariant the reader relies on either way.
#[test]
fn extraction_fails_closed_when_no_handler_opened_the_file() {
    let mut doc = PdfDocument::parse(&fixture("hello.pdf")).expect("parse");
    assert!(!doc.encrypted && doc.locked.is_none() && doc.opened_with.is_none());
    doc.encrypted = true;

    let error = doc
        .extract_checked()
        .expect_err("an encrypted document that was never decrypted is refused");
    assert!(
        error.to_string().contains("ciphertext"),
        "the refusal says the pages were never decrypted: {error:#}"
    );
    assert!(
        doc.extract().is_empty(),
        "the un-fallible facade yields no pages either"
    );
}

/// A stream's payload is copied out of the file into the document's own buffer,
/// and that copy is charged before it is made — for an encrypted file, before
/// any of it is decrypted. A ceiling under the fixture's 45-byte payload refuses
/// the file; the same bytes open under the default budget, so the refusal is the
/// lowered whole-document ceiling, not the file or its password.
#[test]
fn a_lowered_document_budget_refuses_decryption() {
    let pdf = fixture("encrypted.pdf");
    let limits = Limits {
        max_document_bytes: 32,
        ..Limits::default()
    };
    assert!(
        (pdf.len() as u64) <= limits.max_input_bytes,
        "the file is inside the input ceiling"
    );

    let error = PdfDocument::parse_with_limits_and_password(&pdf, limits, None)
        .expect_err("the payload is charged before it is copied");
    assert!(
        matches!(
            error.downcast_ref::<LimitError>(),
            Some(LimitError::DocumentTooLarge { limit: 32, .. })
        ),
        "{error}"
    );

    // The same bytes open under the default budget: the refusal is the lowered
    // whole-document ceiling, not the file or its password.
    let doc = PdfDocument::parse(&pdf).expect("the default budget opens it");
    assert_eq!(doc.locked, None);
}

#[test]
fn a_plain_file_is_neither_encrypted_nor_locked() {
    let doc = PdfDocument::parse(&fixture("hello.pdf")).expect("parse");
    assert!(!doc.encrypted);
    assert_eq!(doc.locked, None);
    assert_eq!(doc.opened_with, None);
}
