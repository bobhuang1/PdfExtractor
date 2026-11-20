//! What the CLI does with an encrypted PDF.
//!
//! `test-data/encrypted.pdf` is a real standard-security document (RC4 40-bit,
//! revision 2, empty user password) written by `test-data/make_encrypted_pdf.py`;
//! `test-data/make_encrypted_variants.py` writes RC4-128, AES-128 and AES-256
//! files whose user password is `user` (owner `owner`). The contract: a file
//! that asks nothing of its reader is read like any other, a file that needs a
//! password is read with `--pdf-password` (or `PDFE_PDF_PASSWORD`), and one the
//! password does not open is refused by name with no document written.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data").join(name)
}

fn out_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pdfe-encrypted-{tag}-{}.json", std::process::id()))
}

fn run(name: &str, out: &PathBuf, extra: &[&str], env_password: Option<&str>) -> Output {
    let _ = std::fs::remove_file(out);
    let mut command = Command::new(env!("CARGO_BIN_EXE_pdfextractor-cli"));
    command
        .args([
            "-i",
            fixture(name).to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--ocr",
            "never",
        ])
        .args(extra)
        .env_remove("PDFE_PDF_PASSWORD");
    if let Some(password) = env_password {
        command.env("PDFE_PDF_PASSWORD", password);
    }
    command.output().expect("spawn the CLI binary")
}

fn assert_read(name: &str, out: &PathBuf, output: &Output) {
    assert!(
        output.status.success(),
        "{name} was not read:\nstderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = std::fs::read_to_string(out).expect("a document was written");
    assert!(json.contains("Secret ledger"), "{name}: the page text is not plain:\n{json}");
    let _ = std::fs::remove_file(out);
}

fn assert_refused(name: &str, out: &PathBuf, output: &Output, says: &str) {
    assert!(
        !output.status.success(),
        "{name} was read without the password it needs"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("encrypted"), "{name}: {stderr}");
    assert!(stderr.contains(says), "{name}: expected {says:?} in {stderr}");
    assert!(!out.exists(), "{name}: a refused run wrote a document anyway");
}

#[test]
fn the_fixture_is_really_an_encrypted_pdf() {
    for name in [
        "encrypted.pdf",
        "encrypted_rc4_128.pdf",
        "encrypted_aes128.pdf",
        "encrypted_aes256.pdf",
    ] {
        let bytes = std::fs::read(fixture(name)).expect("the fixture is committed");
        assert!(bytes.starts_with(b"%PDF-"), "{name}: not a PDF");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("/Encrypt") && text.contains("/Filter /Standard"),
            "{name}: the fixture states no standard security handler"
        );
        assert!(!text.contains("Secret ledger"), "{name}: the page text is in the clear");
    }
}

#[test]
fn an_empty_user_password_is_read_without_asking() {
    let out = out_path("empty");
    let output = run("encrypted.pdf", &out, &[], None);
    assert_read("encrypted.pdf", &out, &output);
}

#[test]
fn a_password_protected_pdf_is_read_with_its_password() {
    for (name, password) in [
        ("encrypted_rc4_128.pdf", "user"),
        ("encrypted_aes128.pdf", "owner"),
        ("encrypted_aes256.pdf", "user"),
        ("encrypted_aes256.pdf", "owner"),
    ] {
        let out = out_path(&format!("{password}-{name}"));
        let output = run(name, &out, &["--pdf-password", password], None);
        assert_read(name, &out, &output);
    }
}

#[test]
fn the_password_can_come_from_the_environment() {
    let out = out_path("env");
    let output = run("encrypted_aes128.pdf", &out, &[], Some("user"));
    assert_read("encrypted_aes128.pdf", &out, &output);
}

#[test]
fn a_missing_password_is_refused_by_name_and_says_how_to_give_one() {
    let out = out_path("missing");
    let output = run("encrypted_aes256.pdf", &out, &[], None);
    assert_refused("encrypted_aes256.pdf", &out, &output, "--pdf-password");
}

#[test]
fn a_wrong_password_is_refused_by_name() {
    let out = out_path("wrong");
    let output = run("encrypted_rc4_128.pdf", &out, &["--pdf-password", "guess"], None);
    assert_refused("encrypted_rc4_128.pdf", &out, &output, "does not open it");
}
