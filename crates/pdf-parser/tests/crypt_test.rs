//! The standard security handler against real encrypted files.
//!
//! The fixtures are written by `test-data/make_encrypted_pdf.py` (RC4 40-bit,
//! revision 2, empty user password) and `test-data/make_encrypted_variants.py`
//! (RC4 128-bit revision 3, AES-128 revision 4 and AES-256 revision 6, each with
//! user password `user` and owner password `owner`). Poppler's `pdftotext`
//! reads every one of them back, so these are files a real reader accepts.

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
        let doc = PdfDocument::parse_with_password(&fixture(name), Some(b"user")).expect("parse");
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
        let doc = PdfDocument::parse_with_password(&fixture(name), Some(b"owner")).expect("parse");
        assert_eq!(doc.locked, None, "{name}");
        assert_eq!(doc.opened_with, Some("owner password"), "{name}");
        assert!(page_text(&doc).contains("Secret ledger"), "{name}");
    }
}

#[test]
fn a_missing_or_wrong_password_is_locked_and_reads_no_noise() {
    for name in VARIANTS {
        for password in [None, Some(&b"guess"[..])] {
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

#[test]
fn a_plain_file_is_neither_encrypted_nor_locked() {
    let doc = PdfDocument::parse(&fixture("hello.pdf")).expect("parse");
    assert!(!doc.encrypted);
    assert_eq!(doc.locked, None);
    assert_eq!(doc.opened_with, None);
}
