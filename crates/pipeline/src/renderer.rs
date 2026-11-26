//! The external renderer: where `pdftoppm` is, and how a page is drawn with it.
//!
//! Two callers draw pages and neither may assume a renderer. The preview draws
//! page backdrops when the machine has one and stays complete when it does not;
//! the reader rasterises a page whose text exists only as vector art, and that
//! is a capability the caller has to ask for (`--ocr-vector-pages`), so it is
//! refused by name when it was asked for and cannot be done.
//!
//! **The rule.** The extractor never *requires* an external renderer to read a
//! document: without the ask it is never looked for, no default path reaches
//! it, and every document is read from its own bytes. What this module adds is
//! a renderer the caller can request, not a dependency the reader assumes.
//!
//! `PATH` is deliberately not searched: it is read from the environment, so a
//! planted `pdftoppm` in any directory it names would run with the user's own
//! privileges. Only the fixed paths the packages write are used, or the program
//! `PDFE_PDFTOPPM` names — the rule [`crate::preview`] already followed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Resolution a vector-only page is rendered at for recognition.
///
/// The corpus scans are 150 dpi, and a page whose text is paths reads every
/// line at it; a render costs four bytes a pixel, so the cheapest resolution
/// that read the fixture is the one used. A page with very small labels is the
/// case to raise it for, and the experiment to raise it on.
pub(crate) const PAGE_DPI: u32 = 150;

/// `pdftoppm`'s executable, if this machine has it installed.
///
/// `PDFE_PDFTOPPM` names the program outright — the override for an install in
/// an unusual place, or for a test. When it is set it is the only place looked,
/// so a value that names no program is refused rather than quietly replaced.
///
/// Without it the lookup reads the fixed paths the packages write
/// ([`fixed_pdftoppm_locations`]).
pub(crate) fn pdftoppm() -> Option<PathBuf> {
    resolve_pdftoppm(std::env::var_os("PDFE_PDFTOPPM").map(PathBuf::from))
}

/// The lookup behind [`pdftoppm`], with its input passed in so a test does not
/// have to touch the process environment.
fn resolve_pdftoppm(explicit: Option<PathBuf>) -> Option<PathBuf> {
    match explicit {
        Some(path) => verified_program(&path),
        None => fixed_pdftoppm_locations()
            .into_iter()
            .find_map(|path| verified_program(&path)),
    }
}

/// Where the poppler utilities' installer puts `pdftoppm`, by platform.
fn fixed_pdftoppm_locations() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from("/opt/homebrew/bin/pdftoppm"),
            PathBuf::from("/usr/local/bin/pdftoppm"),
            PathBuf::from("/usr/bin/pdftoppm"),
        ]
    }
    #[cfg(target_os = "windows")]
    {
        vec![
            PathBuf::from(r"C:\Program Files\poppler\Library\bin\pdftoppm.exe"),
            PathBuf::from(r"C:\Program Files\poppler\bin\pdftoppm.exe"),
        ]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        vec![
            PathBuf::from("/usr/bin/pdftoppm"),
            PathBuf::from("/usr/local/bin/pdftoppm"),
        ]
    }
}

/// `path`, when it is a file this process may execute. A directory, a dangling
/// link or a file with no execute bit is not a program, so it is not reported
/// as one.
fn verified_program(path: &Path) -> Option<PathBuf> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(path.to_path_buf())
}

/// Draw one page of `pdf` to PNG at `dpi`: its bytes and its pixel grid.
///
/// The page is drawn as displayed, with its `/Rotate` applied, which is what
/// the caller's boxes are measured against. One page per call rather than the
/// whole document, because the only pages this draws are the ones with nothing
/// else to read.
///
/// The render lands in a directory of its own under the system temporary
/// directory — `pdftoppm` is given a prefix, not a file — and the directory is
/// removed before returning, whatever the outcome.
pub(crate) fn render_page(
    program: &Path,
    pdf: &Path,
    page_no: i32,
    dpi: u32,
) -> std::io::Result<(Vec<u8>, u32, u32)> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pdfextractor-page-{}-{}-{}",
        std::process::id(),
        page_no,
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir)?;
    let outcome = draw(program, pdf, page_no, dpi, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

/// The render itself, with the scratch directory already made.
fn draw(
    program: &Path,
    pdf: &Path,
    page_no: i32,
    dpi: u32,
    dir: &Path,
) -> std::io::Result<(Vec<u8>, u32, u32)> {
    let prefix = dir.join("page");
    let status = Command::new(program)
        .args(["-png", "-r", &dpi.to_string()])
        .args(["-f", &page_no.to_string(), "-l", &page_no.to_string()])
        .arg("-singlefile")
        .arg(pdf)
        .arg(&prefix)
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "pdftoppm exited with {status} on page {page_no}"
        )));
    }
    let png = prefix.with_extension("png");
    let bytes = std::fs::read(&png)?;
    // The grid is read from the header rather than by decoding the pixels:
    // Tesseract decodes them anyway, and the caller only needs the scale.
    let (width, height) = image::ImageReader::open(&png)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(std::io::Error::other)?
        .into_dimensions()
        .map_err(std::io::Error::other)?;
    Ok((bytes, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer is found at a fixed path or named by `PDFE_PDFTOPPM`, and a
    /// name that is not a program is refused rather than replaced: `PATH` is
    /// never searched, so a planted `pdftoppm` cannot run on a conversion.
    #[test]
    fn the_renderer_override_is_verified_and_never_falls_back() {
        let missing =
            std::env::temp_dir().join(format!("pdfe-no-such-pdftoppm-{}", std::process::id()));
        assert_eq!(resolve_pdftoppm(Some(missing)), None);
        let real = std::env::current_exe().expect("the test binary");
        assert_eq!(resolve_pdftoppm(Some(real.clone())), Some(real));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let directory =
                std::env::temp_dir().join(format!("pdfe-pdftoppm-{}", std::process::id()));
            std::fs::create_dir_all(&directory).expect("scratch dir");
            let fake = directory.join("pdftoppm");
            std::fs::write(&fake, b"#!/bin/sh\n").expect("fake binary");
            assert_eq!(
                resolve_pdftoppm(Some(fake.clone())),
                None,
                "a file with no execute bit is not a program"
            );
            let mut permissions = std::fs::metadata(&fake).expect("metadata").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&fake, permissions).expect("chmod");
            assert_eq!(resolve_pdftoppm(Some(fake.clone())), Some(fake));
            std::fs::remove_dir_all(&directory).ok();
        }
    }
}
