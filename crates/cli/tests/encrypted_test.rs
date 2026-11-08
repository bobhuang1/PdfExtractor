//! What the CLI does with an encrypted PDF.
//!
//! `test-data/encrypted.pdf` is a real standard-security document (RC4 40-bit,
//! revision 2, empty user password) written by `test-data/make_encrypted_pdf.py`
//! — a reader opens it, but every string and stream in it is ciphered. The tool
//! cannot decrypt it yet, so the contract is that it refuses the file by name
//! instead of writing a document full of noise, and says what to do about it.

use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/encrypted.pdf")
}

#[test]
fn the_fixture_is_really_an_encrypted_pdf() {
    let bytes = std::fs::read(fixture()).expect("the fixture is committed");
    assert!(bytes.starts_with(b"%PDF-"), "not a PDF");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("/Encrypt") && text.contains("/Filter /Standard"),
        "the fixture states no standard security handler"
    );
}

#[test]
fn an_encrypted_pdf_is_refused_by_name_and_says_the_way_out() {
    let out = std::env::temp_dir().join(format!("pdfe-encrypted-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&out);

    let output = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"))
        .args([
            "-i",
            fixture().to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--ocr",
            "never",
        ])
        .output()
        .expect("spawn the CLI binary");

    assert!(
        !output.status.success(),
        "an encrypted PDF was read as though it were plain:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("encrypted"),
        "the refusal does not say the file is encrypted: {stderr}"
    );
    assert!(
        stderr.contains("qpdf --decrypt"),
        "the refusal names no way out: {stderr}"
    );
    assert!(
        !out.exists(),
        "a refused run wrote a document anyway: {}",
        out.display()
    );
    let _ = std::fs::remove_file(&out);
}
