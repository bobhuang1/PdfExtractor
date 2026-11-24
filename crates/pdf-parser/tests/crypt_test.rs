//! The standard security handler against real encrypted files.
//!
//! The fixtures are written by `test-data/make_encrypted_pdf.py` (RC4 40-bit,
//! revision 2, empty user password) and `test-data/make_encrypted_variants.py`
//! (RC4 128-bit revision 3, AES-128 revision 4 and AES-256 revisions 5 and 6,
//! each with user password `user` and owner password `owner`).
//! `encrypted_objstm.pdf` is the AES-256 one laid out the PDF 1.5 way:
//! compressed objects in an encrypted object stream, and a cross-reference
//! stream as its only trailer. `encrypted_aggregate_streams.pdf` carries its
//! page in three ciphered content streams, so the document budget is charged
//! their sum rather than one payload at a time. `encrypted_encryptmetadata.pdf` leaves the
//! metadata stream in the clear, and `encrypted_cryptfilter.pdf` gives its
//! content stream a non-default `/Crypt` filter. Poppler's `pdftotext` reads
//! every one of them back, so these are files a real reader accepts.

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

const VARIANTS: [&str; 6] = [
    "encrypted_rc4_128.pdf",
    "encrypted_aes128.pdf",
    "encrypted_aes256.pdf",
    // Revision 5 derives its password hash with one SHA-256 per salt, not 2.B.
    "encrypted_r5.pdf",
    "encrypted_encryptmetadata.pdf",
    "encrypted_cryptfilter.pdf",
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
/// own dictionary, which need not carry an `/ID`. The entry has to be detected
/// on its own, and the handler's refusal — there is no `/ID` to authenticate
/// against — has to be reported rather than the payload handed back as text.
/// The fixture's page is stored in the clear on purpose: a parser that missed
/// the entry would return its readable text, so the empty extraction below is
/// the flag doing its work rather than a page that had nothing to leak.
#[test]
fn an_xref_stream_encrypt_without_an_id_is_refused_not_read() {
    let raw = fixture("xref_encrypt_no_id.pdf");
    assert!(
        raw.windows(13).any(|window| window == b"Secret ledger"),
        "the page really is readable if the entry goes undetected"
    );
    // The control: the same bytes without the entry parse as the plain file
    // they are and hand the page back, so the refusal below is the entry being
    // seen rather than a document that had no page to read.
    let plain = String::from_utf8(raw.clone())
        .expect("the fixture is ASCII")
        .replace("/Encrypt 6 0 R", "");
    let plain = PdfDocument::parse(plain.as_bytes()).expect("the control parses");
    assert!(
        plain.extract_text().contains("Secret ledger"),
        "without the entry the page is read: {:?}",
        plain.extract_text()
    );

    let doc = PdfDocument::parse(&raw).expect("parse");
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

/// A cross-reference stream states its kind twice: `/Type /XRef`, and the `/W`
/// and `/Size` entries only such a stream carries. A writer that leaves the
/// type out still means the bytes it indexes to be ciphered, so the entry has
/// to count on the shape too; with no `/ID` the handler cannot authenticate,
/// and the page must be refused rather than read as though it were plain.
#[test]
fn an_untyped_xref_stream_encrypt_is_refused_not_read() {
    let doc = PdfDocument::parse(&fixture("xref_encrypt_no_type.pdf")).expect("parse");
    assert!(
        doc.encrypted,
        "/Encrypt without /Type /XRef is still an encrypted document"
    );
    assert_eq!(doc.opened_with, None, "no password opened it");
    assert_eq!(
        doc.locked,
        Some(CryptError::WrongPassword),
        "the handler cannot authenticate without an /ID"
    );

    let error = doc
        .extract_checked()
        .expect_err("ciphertext must not be returned as text");
    assert!(
        error.to_string().contains("ciphertext"),
        "the refusal says the pages were never decrypted: {error:#}"
    );
    assert!(
        doc.extract().is_empty(),
        "the un-fallible facade yields no pages either"
    );

    // Even a dictionary that has lost every cross-reference marker says the
    // file is encrypted: the entry cannot be doubted into a plaintext read.
    let committed = String::from_utf8_lossy(&fixture("xref_encrypt_no_type.pdf")).into_owned();
    let stripped = committed.replace(
        "<< /W [1 2 1] /Size 7 /Encrypt 6 0 R /Length 0 >>",
        "<< /Encrypt 6 0 R /Length 0 >>",
    );
    assert_ne!(
        stripped, committed,
        "the fixture's dictionary is the one this test rewrites"
    );
    let doc = PdfDocument::parse(stripped.as_bytes()).expect("parse");
    assert!(
        doc.encrypted,
        "a marker-less /Encrypt is still an encryption"
    );
    assert_eq!(doc.locked, Some(CryptError::WrongPassword));
    assert!(doc.extract().is_empty(), "no page is read from ciphertext");
}

/// qpdf wrote this file with object streams, so its cross-reference stream is
/// the trailer and `startxref` points at that object: the ordinary shape of a
/// PDF 1.5+ file, not a damaged one for the repair path to rebuild. Its
/// `/Encrypt` entry lives in the stream's own dictionary, and the writer's
/// `/Type /XRef` marker was blanked in place — the same number of bytes, so
/// every offset stays valid. The encryption still has to be seen, and the file
/// is otherwise whole: its own password opens it.
#[test]
fn a_valid_xref_stream_without_its_type_is_refused_not_read() {
    let raw = fixture("encrypted_xref_no_type.pdf");
    let text = String::from_utf8_lossy(&raw).into_owned();
    assert!(raw.starts_with(b"%PDF-1.7"), "qpdf's header");
    assert!(
        !text.contains("/Type /XRef"),
        "the fixture's marker is the one this test is about"
    );

    // `startxref` names the cross-reference stream's object, not 0: this file
    // is indexed, not reconstructed.
    let tail = text.rfind("startxref").expect("a startxref") + "startxref".len();
    let offset: usize = text[tail..]
        .split_whitespace()
        .next()
        .expect("an offset")
        .parse()
        .expect("a number");
    assert!(offset > 0, "the repair path's startxref 0 is not this file");
    let object = &raw[offset..];
    let line_end = object
        .iter()
        .position(|&b| b == b'\n')
        .expect("an object header");
    let header = std::str::from_utf8(&object[..line_end]).expect("an ASCII object header");
    let mut words = header.split(' ');
    assert!(
        words.next().is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
        }) && words.next() == Some("0")
            && words.next() == Some("obj"),
        "startxref points at an object: {header:?}"
    );
    let dictionary_end = offset
        + object
            .windows(b"stream".len())
            .position(|window| window == b"stream")
            .expect("the stream keyword");
    assert!(
        String::from_utf8_lossy(&raw[offset..dictionary_end]).contains("/Encrypt "),
        "the entry sits in that stream's dictionary"
    );

    let doc = PdfDocument::parse(&raw).expect("parse");
    assert!(
        doc.encrypted,
        "the untyped xref stream still declares /Encrypt"
    );
    assert_eq!(doc.opened_with, None, "no password opened it");
    assert_eq!(doc.locked, Some(CryptError::WrongPassword));

    let error = doc
        .extract_checked()
        .expect_err("ciphertext must not be returned as text");
    assert!(
        error.to_string().contains("ciphertext"),
        "the refusal says the pages were never decrypted: {error:#}"
    );
    assert!(
        doc.extract().is_empty(),
        "the un-fallible facade yields no pages either"
    );

    // The same bytes with the password prove the file itself is intact: the
    // refusal above was the missing marker seen, not a file that could not be
    // indexed at all.
    let opened =
        PdfDocument::parse_with_password(&raw, Some("user")).expect("parse with its password");
    assert_eq!(opened.opened_with, Some("user password"));
    assert!(
        page_text(&opened).contains("Compressed Stream Works"),
        "the page is read when the password is given: {:?}",
        page_text(&opened)
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

/// The whole-document ceiling is a total, not a per-stream check: this fixture
/// carries its page in three ciphered content streams, each under the lowered
/// ceiling used here and the three together over it. The payloads are charged
/// as they are copied out of the file — before any of them is decrypted — so the
/// refusal names their sum and no plaintext is ever produced.
#[test]
fn a_lowered_document_budget_charges_every_encrypted_stream_together() {
    let pdf = fixture("encrypted_aggregate_streams.pdf");
    let opened = PdfDocument::parse_with_limits_and_password(&pdf, Limits::default(), Some("user"))
        .expect("the fixture opens under the default budget");
    assert!(
        page_text(&opened).contains("ciphered stream"),
        "the fixture reads under the default budget: {}",
        page_text(&opened)
    );
    let payloads: Vec<u64> = opened
        .objects
        .values()
        .filter_map(|value| match value {
            PdfValue::Stream(_, raw) => Some(raw.len() as u64),
            _ => None,
        })
        .collect();
    assert_eq!(
        payloads.len(),
        3,
        "the page is three ciphered content streams: {payloads:?}"
    );
    let total: u64 = payloads.iter().sum();
    let largest = payloads.iter().copied().max().expect("a stream");
    let limit = total - 1;
    assert!(
        largest < limit,
        "every stream fits the lowered ceiling on its own ({largest} of {limit})"
    );
    assert!(
        (pdf.len() as u64) <= Limits::default().max_input_bytes,
        "the file is inside the input ceiling"
    );

    let error = PdfDocument::parse_with_limits_and_password(
        &pdf,
        Limits {
            max_document_bytes: limit,
            ..Limits::default()
        },
        Some("user"),
    )
    .expect_err("three ciphertext payloads together cross the ceiling");
    match error.downcast_ref::<LimitError>() {
        Some(LimitError::DocumentTooLarge {
            actual,
            limit: reported,
        }) => {
            assert_eq!(*actual, total, "the charge is the sum of every stream");
            assert_eq!(*reported, limit);
        }
        other => panic!("expected the whole-document ceiling, got {other:?}: {error}"),
    }
}

/// The whole-document ceiling is crossed while the ciphertext is copied out of
/// the file, before the security handler is authenticated: the three-stream
/// fixture is refused for the correct password and a wrong one alike, and the
/// refusal is the budget's rather than the handler's. Authenticating first
/// would make the wrong password a `CryptError` and leave the correct one
/// opened, so this separates the two orders.
#[test]
fn an_aggregate_ceiling_refuses_before_the_handler_runs() {
    let pdf = fixture("encrypted_aggregate_streams.pdf");
    let opened = PdfDocument::parse_with_limits_and_password(&pdf, Limits::default(), Some("user"))
        .expect("the fixture opens under the default budget");
    let payloads: Vec<u64> = opened
        .objects
        .values()
        .filter_map(|value| match value {
            PdfValue::Stream(_, raw) => Some(raw.len() as u64),
            _ => None,
        })
        .collect();
    assert_eq!(payloads.len(), 3, "three ciphered content streams");
    let total: u64 = payloads.iter().sum();

    for password in [Some("user"), Some("not-the-password")] {
        let error = PdfDocument::parse_with_limits_and_password(
            &pdf,
            Limits {
                max_document_bytes: total - 1,
                ..Limits::default()
            },
            password,
        )
        .expect_err("a ceiling under the three payloads refuses the file before it opens");
        match error.downcast_ref::<LimitError>() {
            Some(LimitError::DocumentTooLarge { actual, .. }) => {
                assert_eq!(
                    *actual, total,
                    "{password:?}: the refusal is the payload sum"
                )
            }
            other => panic!("{password:?}: expected the ceiling, got {other:?}: {error}"),
        }
        assert!(
            error.downcast_ref::<CryptError>().is_none(),
            "{password:?}: the handler must not have refused first: {error}"
        );
    }
}

/// `/EncryptMetadata false` leaves the `/Type /Metadata` stream in the clear:
/// reading it out of the parsed object alone would not show that, so the raw
/// file is checked for the XML itself while the ciphered title stays absent.
#[test]
fn metadata_stays_in_the_clear_when_encrypt_metadata_is_false() {
    let bytes = fixture("encrypted_encryptmetadata.pdf");
    let doc = PdfDocument::parse_with_password(&bytes, Some("user")).expect("parse");
    assert_eq!(doc.locked, None);

    let PdfValue::Stream(_, payload) = doc.objects.get(&(8, 0)).expect("the metadata stream")
    else {
        panic!("object 8 is the /Type /Metadata stream");
    };
    let xml = String::from_utf8_lossy(payload);
    assert!(xml.starts_with("<?xml"), "{xml:?}");
    assert!(xml.contains("<x:xmpmeta"), "{xml:?}");

    let marker = b"<x:xmpmeta";
    assert!(
        bytes.windows(marker.len()).any(|window| window == marker),
        "the metadata XML is stored in the clear"
    );
    let title = b"Ciphered title";
    assert!(
        !bytes.windows(title.len()).any(|window| window == title),
        "the /Info title is still ciphered"
    );
    assert_eq!(info_title(&doc).as_deref(), Some("Ciphered title"));
    assert!(page_text(&doc).contains("Secret ledger"));
}

/// A stream may name its own `/Crypt` filter instead of taking `/StmF`. Here
/// object 4 names `/AltCF`, a non-default `/CF` entry with `/CFM /None`, so its
/// bytes are stored as written while the title is RC4-ciphered: a reader that
/// applied `/StmF` to every stream would read the content as ciphertext.
#[test]
fn a_stream_naming_its_own_crypt_filter_is_read_with_it() {
    let bytes = fixture("encrypted_cryptfilter.pdf");
    let doc = PdfDocument::parse_with_password(&bytes, Some("user")).expect("parse");
    assert_eq!(doc.locked, None);
    assert!(
        page_text(&doc).contains("Secret ledger"),
        "{}",
        page_text(&doc)
    );
    assert_eq!(info_title(&doc).as_deref(), Some("Ciphered title"));

    let content = b"Secret ledger";
    assert!(
        bytes.windows(content.len()).any(|window| window == content),
        "the content stream is in the clear under /AltCF"
    );
    let title = b"Ciphered title";
    assert!(
        !bytes.windows(title.len()).any(|window| window == title),
        "the title still takes /StmF"
    );
}

#[test]
fn a_plain_file_is_neither_encrypted_nor_locked() {
    let doc = PdfDocument::parse(&fixture("hello.pdf")).expect("parse");
    assert!(!doc.encrypted);
    assert_eq!(doc.locked, None);
    assert_eq!(doc.opened_with, None);
}

/// An encrypted PDF 1.5 file: its page tree and `/Info` are compressed objects
/// in an encrypted `/Type /ObjStm` stream, and its only trailer is a
/// cross-reference stream. The specification encrypts the object stream whole
/// and says the strings inside it are not separately encrypted (ISO 32000-1
/// 7.5.8.2 and 7.6.1), so the title comes out of exactly one decryption — the
/// stream's — and stays plaintext after the stream is expanded.
#[test]
fn a_string_in_an_encrypted_object_stream_is_decrypted_exactly_once() {
    let bytes = fixture("encrypted_objstm.pdf");
    let doc = PdfDocument::parse_with_password(&bytes, Some("user")).expect("parse");
    assert!(doc.encrypted);
    assert_eq!(doc.locked, None);
    assert_eq!(doc.opened_with, Some("user password"));

    // The page tree is a compressed object, so a page proves the stream was
    // decrypted and expanded rather than read as ciphertext.
    assert!(
        page_text(&doc).contains("Object stream page"),
        "{}",
        page_text(&doc)
    );

    let title = match doc.objects.get(&(7, 0)).expect("the compressed /Info") {
        PdfValue::Dictionary(dict) => match dict.get("Title").expect("the title") {
            PdfValue::String(s) => s.clone(),
            other => panic!("the title is a string, not {other:?}"),
        },
        other => panic!("object 7 is the /Info dictionary, not {other:?}"),
    };
    assert_eq!(title, "Object stream title");

    // The file does not spell it in the clear: the title arrived by decryption.
    assert!(
        !bytes
            .windows(title.len())
            .any(|window| window == title.as_bytes()),
        "the title is stored in the clear"
    );

    // And it was decrypted once, with its container: the stored object stream is
    // the plaintext form, its header and the title inside it intact. A second
    // decryption of the string would leave neither.
    let PdfValue::Stream(_, payload) = doc.objects.get(&(8, 0)).expect("the object stream") else {
        panic!("object 8 is the object stream");
    };
    let payload = String::from_utf8_lossy(payload);
    assert!(payload.starts_with("2 0 "), "{payload:?}");
    assert!(
        payload.contains("/Title (Object stream title)"),
        "{payload:?}"
    );
}
