//! What a file actually is, and the one family we hand to somebody else.
//!
//! An extension is a claim, not a fact. `.wps` alone has named at least three
//! unrelated formats: Microsoft Works' DOS-era record stream, the compound-file
//! Works of 3.0 and later, and Kingsoft WPS Office's own files — and WPS Office
//! also writes OOXML under that name when the user saves in its format. So the
//! bytes decide: [`sniff`] reads the magic, [`detect`] turns that into the
//! reader to use, and the extension is consulted only where the bytes leave a
//! question open. That is what makes a DOCX called `report.wps` readable, and
//! what keeps a Word binary `.doc` named `letter.wps` going to the Word reader.
//!
//! The Works family has no reader here, and a cheap one does not exist: there
//! is no published specification, the only documentation is libwps' reverse
//! engineering, and the last writer of these files stopped around 2007. What
//! does exist is LibreOffice's importer, so that is the bridge — the same route
//! Docling itself takes for legacy `.doc`. The converted document is read by
//! our own readers afterwards, and the command line gives the result the
//! identity of the file the user handed over (its name, MIME type and hash, in
//! `relabel`): what was read is what the user gave us, however it was read.
//!
//! This is a crate rather than a module of the command line because more than
//! one thing asks its question and they must not answer it differently. The run
//! asks it to choose a reader for each file ([`detect`]); the `--compare` mode's
//! gate asks it to decide, before anything is converted, which documents this
//! machine may hold ([`reads_through_libreoffice`], in `crates/cli/src/compare.rs`);
//! and the corpus report (`crates/ui/tests/corpus_report_test.rs`) asks it for
//! the same reason and must get the same answer — a report that kept its own
//! list of extensions beside this one would leave out a document the gate holds,
//! or hold one the gate leaves out, on nothing but the name it was saved under.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The MIME type a Works document is published under; WordPerfect's and the
/// legacy Excel one are here for the same reason.
const WORKS_MIME: &str = "application/vnd.ms-works";
const WORDPERFECT_MIME: &str = "application/vnd.wordperfect";
const EXCEL_MIME: &str = "application/vnd.ms-excel";
const POWERPOINT_MIME: &str = "application/vnd.ms-powerpoint";

/// What a file's first bytes say it is — before the extension gets a vote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sniffed {
    /// `%PDF-`.
    Pdf,
    /// `D0 CF 11 E0 A1 B1 1A E1`: an OLE2 compound file — Word binary, `.xls`,
    /// `.ppt`, and Works from 3.0 on all live in one.
    Compound,
    /// `01 FE`: Works for DOS and Works 2 for Windows wrote their own stream,
    /// not a compound file (PRONOM fmt/163).
    WorksDos,
    /// `PK\x03\x04`: an OPC package or another ZIP container.
    Zip,
    /// A plain image, named by the media type its signature declares.
    Image(&'static str),
    /// `{\rtf`.
    Rtf,
    /// `<!DOCTYPE html` or `<html`.
    Html,
    /// No binary marker: letters, in UTF-8 or UTF-16.
    Text,
    /// Binary bytes with no marker this knows.
    Unknown,
}

/// Which reader a file needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Docx,
    Xlsx,
    /// PowerPoint's OPC package, read by `pptx-parser`.
    Pptx,
    /// One plain image, read as a single page. The media type is the one its
    /// bytes declare, so an image called `scan.dat` still reports what it is.
    Image {
        mimetype: &'static str,
    },
    /// Word 97 binary, read by `doc-parser` through the compound file.
    Doc,
    /// One of the flow formats, under the name the text readers take (`rtf`,
    /// `html`, `md`, `csv`, `tsv`, `txt`).
    Flow(&'static str),
    /// No reader here: LibreOffice reads it and we read what it writes.
    Bridge {
        target: BridgeTarget,
        mimetype: &'static str,
        /// What to call the format in a message, e.g. "a Microsoft Works
        /// document".
        label: &'static str,
    },
    /// A format this tool does not read, with the phrase that says what it is.
    Unsupported(&'static str),
}

/// The format LibreOffice should write for a bridged read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeTarget {
    /// `.wps`, `.wp`, `.wpd` — a writer document.
    Docx,
    /// `.wks`, `.xls` — a grid with sheets.
    Xlsx,
    /// `.wdb` — a database whose main table is what a CSV can hold.
    Csv,
    /// `.ppt` — a deck of slides, which Impress writes as a package our own
    /// PPTX reader reads.
    Pptx,
}

impl BridgeTarget {
    /// The extension LibreOffice writes under, which is also its
    /// `--convert-to` filter name.
    pub fn extension(self) -> &'static str {
        match self {
            BridgeTarget::Docx => "docx",
            BridgeTarget::Xlsx => "xlsx",
            BridgeTarget::Csv => "csv",
            BridgeTarget::Pptx => "pptx",
        }
    }

    /// What to tell somebody whose machine has no LibreOffice: the one action
    /// they can take without installing anything.
    fn escape(self) -> &'static str {
        match self {
            BridgeTarget::Docx => "re-save it as .docx",
            BridgeTarget::Xlsx => "re-save it as .xlsx",
            BridgeTarget::Csv => "export it as .csv",
            BridgeTarget::Pptx => "re-save it as .pptx",
        }
    }
}

/// The magic bytes at the top of a file, as far as they go.
///
/// A UTF-16 file is text, not binary, even though every other byte is a NUL —
/// the text readers decode it, so the NUL test has to look past the BOM.
pub fn sniff(data: &[u8]) -> Sniffed {
    if data.starts_with(b"%PDF-") {
        return Sniffed::Pdf;
    }
    // An image's signature is as good as a PDF's: a TIFF starts with NULs and
    // a WebP with ASCII, so the binary/text test below cannot name either.
    if let Some(mimetype) = pdfextractor_image_parser::mimetype(data) {
        return Sniffed::Image(mimetype);
    }
    if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Sniffed::Compound;
    }
    if data.starts_with(&[0x01, 0xFE]) {
        return Sniffed::WorksDos;
    }
    if data.starts_with(b"PK\x03\x04") {
        return Sniffed::Zip;
    }
    if data.starts_with(b"{\\rtf") {
        return Sniffed::Rtf;
    }
    // A UTF-16 file is text, whatever its bytes look like without the BOM.
    if data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]) {
        return Sniffed::Text;
    }
    // One NUL in the first half-kilobyte is what separates a binary from a
    // text file here. A BOM-less UTF-16 file lands on `Unknown`, and its
    // extension still sends it to the text reader — a `.txt` stays readable.
    let head = &data[..data.len().min(512)];
    if head.contains(&0) {
        return Sniffed::Unknown;
    }
    let start = String::from_utf8_lossy(&head[..head.len().min(64)])
        .trim_start()
        .to_ascii_lowercase();
    if start.starts_with("<!doctype html") || start.starts_with("<html") {
        return Sniffed::Html;
    }
    Sniffed::Text
}

/// The format a file holds, decided by its bytes and then its extension.
pub fn detect(path: &Path) -> Result<Format> {
    let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(detect_bytes(path, &data))
}

/// [`detect`] over bytes already in hand, for tests and for callers that have
/// read the file anyway.
pub fn detect_bytes(path: &Path, data: &[u8]) -> Format {
    let ext = extension(path);
    match sniff(data) {
        Sniffed::Pdf => Format::Pdf,
        Sniffed::Image(mimetype) => Format::Image { mimetype },
        Sniffed::Zip => zip_family(data),
        Sniffed::Compound => compound_family(data, &ext),
        // The DOS-era signature is Works' own, and only Works wrote it: the
        // bridge is the only reader that exists.
        Sniffed::WorksDos => Format::Bridge {
            target: BridgeTarget::Docx,
            mimetype: WORKS_MIME,
            label: "a Microsoft Works document",
        },
        Sniffed::Rtf => Format::Flow("rtf"),
        Sniffed::Html => Format::Flow("html"),
        // Plain words under an extension we read natively keep that reader, so
        // a truncated PDF is still reported as a broken PDF; anything else is
        // text under the name its extension asks for.
        // A truncated image still goes to the image reader, so the failure
        // names a broken PNG rather than a file of no known format.
        Sniffed::Text => match image_family(&ext) {
            Some(mimetype) => Format::Image { mimetype },
            None => match ext.as_str() {
                "pdf" => Format::Pdf,
                "docx" => Format::Docx,
                "xlsx" => Format::Xlsx,
                "pptx" => Format::Pptx,
                "doc" => Format::Doc,
                other => text_family(other),
            },
        },
        // Binary bytes with no marker this knows. Older Works files are their
        // own streams rather than containers — every one of the 26 Works
        // spreadsheets starts `00 00` or `FF 00`, none is a compound file — so
        // for that family the extension is the only identifier there is, and
        // LibreOffice, which sniffs the same header itself, is the reader.
        Sniffed::Unknown => by_extension(&ext).unwrap_or_else(|| {
            if let Some(mimetype) = image_family(&ext) {
                return Format::Image { mimetype };
            }
            match ext.as_str() {
                // A text extension still means text, and the reader decodes
                // whatever it is given; anything else is named and refused
                // rather than poured out as characters.
                "txt" | "md" | "markdown" | "csv" | "tsv" | "html" | "htm" | "rtf" => {
                    text_family(&ext)
                }
                _ => Format::Unsupported("a binary file of no format this reads"),
            }
        }),
    }
}

/// A ZIP's family is in its part names. The central directory stores them
/// plainly, so one pass over the file finds the part that names the format.
fn zip_family(data: &[u8]) -> Format {
    if contains(data, b"word/document.xml") {
        return Format::Docx;
    }
    if contains(data, b"xl/workbook.xml") {
        return Format::Xlsx;
    }
    if contains(data, b"ppt/presentation.xml") {
        return Format::Pptx;
    }
    if contains(data, b"META-INF/container.xml") {
        return Format::Unsupported("an EPUB book — not read yet");
    }
    if contains(data, b"content.xml") {
        return Format::Unsupported("an OpenDocument file — not read yet");
    }
    Format::Unsupported("a ZIP package with no document part this reads")
}

/// A compound file is Word's when it holds a `WordDocument` stream, and
/// PowerPoint's when it holds a `PowerPoint Document` stream. The directory
/// stores names in UTF-16LE with no terminator, so the name itself is the
/// signature; Word writes the first, and no Works file in the 40-sample corpus
/// has it (Word's four `.doc` fixtures all do). Legacy PowerPoint has no reader
/// here — Impress does, and it writes a package the PPTX reader reads — so the
/// stream sends the file to the bridge rather than to a parser, and BIFF's
/// `Workbook` (or BIFF5's `Book`) does the same for a legacy workbook. Those
/// three are the compound families whose bytes name them, so a file is read for
/// what it holds even when the name promises something else; the rest — Works,
/// WordPerfect, an older PowerPoint that named its stream otherwise — are the
/// extension's to say, because nothing in their bytes is theirs alone.
fn compound_family(data: &[u8], ext: &str) -> Format {
    if contains_utf16(data, "WordDocument") {
        return Format::Doc;
    }
    if contains_utf16(data, "PowerPoint Document") {
        return Format::Bridge {
            target: BridgeTarget::Pptx,
            mimetype: POWERPOINT_MIME,
            label: "a legacy PowerPoint presentation",
        };
    }
    // `Book` is BIFF5's name for the same stream, and what Calc opens either way.
    if contains_utf16(data, "Workbook") || contains_utf16(data, "Book") {
        return Format::Bridge {
            target: BridgeTarget::Xlsx,
            mimetype: EXCEL_MIME,
            label: "a legacy Excel workbook",
        };
    }
    by_extension(ext).unwrap_or(match ext {
        "doc" => Format::Unsupported("a compound file with no Word stream"),
        _ => Format::Unsupported("a compound (OLE2) file of no format this reads"),
    })
}

/// The bridge for an extension the bytes cannot name on their own, or whose
/// family has no reader here at all.
fn by_extension(ext: &str) -> Option<Format> {
    Some(match ext {
        "wps" | "wp" => Format::Bridge {
            target: BridgeTarget::Docx,
            mimetype: WORKS_MIME,
            label: "a Microsoft Works document",
        },
        "wks" => Format::Bridge {
            target: BridgeTarget::Xlsx,
            mimetype: WORKS_MIME,
            label: "a Microsoft Works spreadsheet",
        },
        "wdb" => Format::Bridge {
            target: BridgeTarget::Csv,
            mimetype: WORKS_MIME,
            label: "a Microsoft Works database",
        },
        "wpd" => Format::Bridge {
            target: BridgeTarget::Docx,
            mimetype: WORDPERFECT_MIME,
            label: "a WordPerfect document",
        },
        // BIFF is a compound file with `Workbook`/`Book` streams; Calc reads it
        // and writes a workbook this already reads, so it costs one line.
        "xls" => Format::Bridge {
            target: BridgeTarget::Xlsx,
            mimetype: EXCEL_MIME,
            label: "a legacy Excel workbook",
        },
        // PowerPoint's binary format is a compound file whose stream names it,
        // but an older producer that wrote the file with the stream under
        // another name is still Impress' to read: the extension is the promise.
        "ppt" => Format::Bridge {
            target: BridgeTarget::Pptx,
            mimetype: POWERPOINT_MIME,
            label: "a legacy PowerPoint presentation",
        },
        _ => return None,
    })
}

/// Whether a name is read only through LibreOffice.
///
/// The bridge is what the help's `*` marks mean: a `.wps` or an `.xls` is
/// accepted, and a machine without LibreOffice cannot read it, so the list a
/// user is shown says which names carry that condition instead of leaving it to
/// the failure message.
pub fn is_bridged(extension: &str) -> bool {
    matches!(by_extension(extension), Some(Format::Bridge { .. }))
}

/// Whether what a document *reads as* depends on LibreOffice having read it.
///
/// The wider question, and not the same one as [`is_bridged`]: a legacy `.doc`
/// has a reader here, so it is not bridged — and the two readers do not make the
/// same reading of it. Writer gives the headings, the list markers, the merged
/// tables and the pictures; the built-in reader gives the container's words, with
/// a paragraph where a heading was, no marker on a list item, and tabs where a
/// table was. A caller holding a reading against numbers that were measured
/// somewhere else therefore has to know about both: what only the bridge can read
/// at all, and what it reads another way.
pub fn reads_through_libreoffice(format: Format) -> bool {
    matches!(format, Format::Bridge { .. } | Format::Doc)
}

/// The image media type an extension asks for, when the bytes said nothing.
///
/// This is the extension's only job: naming the reader so a truncated file is
/// reported as the image it was meant to be. It never overrides a signature.
/// The names come from the shared picture table, narrowed to the encodings the
/// image reader can turn into a page: a carried format that is only ever a
/// picture inside a document (an SVG, a metafile) names no reader here.
fn image_family(ext: &str) -> Option<&'static str> {
    let format = pdfextractor_picture_formats::format_for_extension(ext)?;
    pdfextractor_image_parser::is_document_media_type(format.mime).then_some(format.mime)
}

/// The flow reader an extension asks for; an unknown name is plain text.
fn text_family(ext: &str) -> Format {
    match ext {
        "md" | "markdown" => Format::Flow("md"),
        "csv" => Format::Flow("csv"),
        "tsv" => Format::Flow("tsv"),
        "html" | "htm" => Format::Flow("html"),
        "rtf" => Format::Flow("rtf"),
        _ => Format::Flow("txt"),
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// A name as a compound file's directory stores it: UTF-16LE.
fn contains_utf16(haystack: &[u8], text: &str) -> bool {
    let needle: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    contains(haystack, &needle)
}

// --- the bridge -----------------------------------------------------------

/// LibreOffice's executable, if there is one.
///
/// `PDFE_SOFFICE` names it outright, which is what lets a machine with
/// LibreOffice installed somewhere unusual — or a test — point this at the
/// binary instead of hoping for a `PATH` that has it.
pub fn libreoffice() -> Option<PathBuf> {
    resolve_soffice(
        std::env::var_os("PDFE_SOFFICE").map(PathBuf::from),
        std::env::var_os("PATH"),
    )
}

/// The lookup behind [`libreoffice`], with its inputs passed in so a test does
/// not have to touch the process environment.
fn resolve_soffice(
    explicit: Option<PathBuf>,
    search: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return path.is_file().then_some(path);
    }
    for directory in std::env::split_paths(&search?) {
        for name in ["soffice", "libreoffice"] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// A document LibreOffice wrote for us, and the scratch directory holding it.
///
/// The directory goes away when this value does, so a run leaves nothing behind
/// — the converted file is a means of reading, not an output.
pub struct Bridged {
    path: PathBuf,
    _scratch: Scratch,
}

impl Bridged {
    /// The converted file, ready for the reader its format names.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Convert `input` with LibreOffice and hand back the file it wrote.
///
/// `label` names the format in the failure, because "LibreOffice is not
/// installed" is not useful without saying what needed it.
pub fn bridge(input: &Path, target: BridgeTarget, label: &str) -> Result<Bridged> {
    let Some(soffice) = libreoffice() else {
        bail!(
            "this is {label}; reading it needs LibreOffice (`soffice`), which is not on PATH — \
             install LibreOffice, or {}",
            target.escape()
        );
    };

    let scratch = Scratch::new("bridge")?;
    let out = scratch.path.join("out");
    std::fs::create_dir_all(&out)?;
    // A profile of its own: a LibreOffice the user already has open keeps the
    // default one locked, and a locked profile makes `--headless` exit without
    // converting anything.
    let profile = scratch.path.join("profile");
    let output = Command::new(&soffice)
        .arg("--headless")
        .arg("--norestore")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.to_string_lossy()
        ))
        .arg("--convert-to")
        .arg(target.extension())
        .arg("--outdir")
        .arg(&out)
        .arg(input)
        .output()
        .with_context(|| format!("run {}", soffice.display()))?;

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let produced = out.join(format!("{stem}.{}", target.extension()));
    if !output.status.success() || !produced.is_file() {
        // LibreOffice's last stderr line is the one that names the problem.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no diagnostic");
        bail!(
            "LibreOffice could not read {label} ({}): {detail}",
            output.status
        );
    }
    Ok(Bridged {
        path: produced,
        _scratch: scratch,
    })
}

/// A directory under the system's temporary area, removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Result<Self> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pdfe-{tag}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    /// A compound-file header, so `sniff` calls the bytes one.
    fn compound(streams: &[&str]) -> Vec<u8> {
        let mut data = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        for stream in streams {
            let name: Vec<u8> = stream.encode_utf16().flat_map(u16::to_le_bytes).collect();
            data.extend_from_slice(&name);
            data.extend_from_slice(&[0, 0]);
        }
        data
    }

    /// A ZIP whose central directory names `parts`, as a real package does.
    fn package(parts: &[&str]) -> Vec<u8> {
        let mut data = b"PK\x03\x04".to_vec();
        for part in parts {
            data.extend_from_slice(part.as_bytes());
            data.extend_from_slice(&[0, 0]);
        }
        data
    }

    #[test]
    fn the_extension_does_not_decide_the_format() {
        assert_eq!(
            detect_bytes(&named("report.wps"), b"%PDF-1.7\n%\xE2\xE3"),
            Format::Pdf
        );
        assert_eq!(
            detect_bytes(&named("letter.wps"), b"{\\rtf1\\ansi"),
            Format::Flow("rtf")
        );
        assert_eq!(
            detect_bytes(&named("page.wps"), b"<!DOCTYPE html><html>"),
            Format::Flow("html")
        );
    }

    #[test]
    fn a_zip_is_what_its_parts_say() {
        assert_eq!(
            detect_bytes(&named("letter.wps"), &package(&["word/document.xml"])),
            Format::Docx
        );
        assert_eq!(
            detect_bytes(&named("sheet.wps"), &package(&["xl/workbook.xml"])),
            Format::Xlsx
        );
        assert_eq!(
            detect_bytes(&named("book.epub"), &package(&["META-INF/container.xml"])),
            Format::Unsupported("an EPUB book — not read yet")
        );
        assert_eq!(
            detect_bytes(&named("deck.wps"), &package(&["ppt/presentation.xml"])),
            Format::Pptx,
            "a deck is a deck whatever it is called"
        );
    }

    #[test]
    fn a_plain_image_is_named_by_its_signature() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(
            detect_bytes(&named("scan.dat"), png),
            Format::Image {
                mimetype: "image/png"
            },
            "an image is an image whatever its name says"
        );
        assert_eq!(
            detect_bytes(&named("photo.dat"), &[0xFF, 0xD8, 0xFF, 0xE0]),
            Format::Image {
                mimetype: "image/jpeg"
            }
        );
        assert_eq!(
            detect_bytes(&named("page.dat"), b"II*\0\x08\0\0\0"),
            Format::Image {
                mimetype: "image/tiff"
            }
        );
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        assert_eq!(
            detect_bytes(&named("shot.dat"), &bmp),
            Format::Image {
                mimetype: "image/bmp"
            }
        );
        assert_eq!(
            detect_bytes(&named("still.dat"), b"RIFF\x18\0\0\0WEBPVP8X"),
            Format::Image {
                mimetype: "image/webp"
            }
        );
        // A JP2 opens with its signature box; the box's own length and the
        // four magic bytes are what name it, so the extension gets no vote.
        assert_eq!(
            detect_bytes(
                &named("scan.dat"),
                b"\0\0\0\x0cjP  \r\n\x87\n\0\0\0\x14ftypjp2 "
            ),
            Format::Image {
                mimetype: "image/jp2"
            }
        );
    }

    #[test]
    fn a_broken_image_keeps_the_reader_its_extension_asks_for() {
        // Words under an image's name are a broken image, not a document:
        // the parse error names what was expected.
        for (name, mimetype) in [
            ("broken.png", "image/png"),
            ("broken.jpg", "image/jpeg"),
            ("broken.tiff", "image/tiff"),
            ("broken.bmp", "image/bmp"),
            ("broken.webp", "image/webp"),
            ("broken.jp2", "image/jp2"),
        ] {
            assert_eq!(
                detect_bytes(&named(name), b"not really an image"),
                Format::Image { mimetype },
                "{name}"
            );
        }
    }

    #[test]
    fn a_word_stream_makes_a_compound_file_a_word_document() {
        assert_eq!(
            detect_bytes(&named("letter.wps"), &compound(&["WordDocument", "1Table"])),
            Format::Doc
        );
    }

    #[test]
    fn works_compound_files_go_to_the_bridge_by_family() {
        let writer = compound(&["CompObj", "Contents"]);
        assert_eq!(
            detect_bytes(&named("old.wps"), &writer),
            Format::Bridge {
                target: BridgeTarget::Docx,
                mimetype: WORKS_MIME,
                label: "a Microsoft Works document",
            }
        );
        assert_eq!(
            detect_bytes(&named("old.wks"), &writer),
            Format::Bridge {
                target: BridgeTarget::Xlsx,
                mimetype: WORKS_MIME,
                label: "a Microsoft Works spreadsheet",
            }
        );
        assert_eq!(
            detect_bytes(&named("old.xls"), &compound(&["Workbook"])),
            Format::Bridge {
                target: BridgeTarget::Xlsx,
                mimetype: EXCEL_MIME,
                label: "a legacy Excel workbook",
            }
        );
        assert_eq!(
            detect_bytes(&named("manual.wpd"), &compound(&["PerfectOffice_MAIN"])),
            Format::Bridge {
                target: BridgeTarget::Docx,
                mimetype: WORDPERFECT_MIME,
                label: "a WordPerfect document",
            }
        );
    }

    #[test]
    fn a_powerpoint_stream_sends_a_compound_file_to_the_bridge() {
        // The stream is the signature a real PowerPoint 97-2003 file carries;
        // it decides even when the name says nothing (`deck.bin` is refused
        // before detection runs, so the name here is a `.pptx` that is not one).
        assert_eq!(
            detect_bytes(
                &named("deck.ppt"),
                &compound(&["PowerPoint Document", "Current User"])
            ),
            Format::Bridge {
                target: BridgeTarget::Pptx,
                mimetype: POWERPOINT_MIME,
                label: "a legacy PowerPoint presentation",
            }
        );
        assert_eq!(
            detect_bytes(&named("deck.pptx"), &compound(&["PowerPoint Document"])),
            Format::Bridge {
                target: BridgeTarget::Pptx,
                mimetype: POWERPOINT_MIME,
                label: "a legacy PowerPoint presentation",
            },
            "a compound deck is a legacy deck whatever it is called"
        );
    }

    #[test]
    fn a_workbook_stream_sends_a_compound_file_to_the_bridge() {
        // BIFF's stream is its signature in the same way: a workbook saved under
        // a name that promises a package is Calc's to read, not ours to refuse.
        assert_eq!(
            detect_bytes(&named("ledger.xls"), &compound(&["Workbook", "CompObj"])),
            Format::Bridge {
                target: BridgeTarget::Xlsx,
                mimetype: EXCEL_MIME,
                label: "a legacy Excel workbook",
            }
        );
        assert_eq!(
            detect_bytes(&named("ledger.docx"), &compound(&["Workbook"])),
            Format::Bridge {
                target: BridgeTarget::Xlsx,
                mimetype: EXCEL_MIME,
                label: "a legacy Excel workbook",
            },
            "a compound workbook is a legacy workbook whatever it is called"
        );
        // BIFF5 called the same stream `Book`.
        assert_eq!(
            detect_bytes(&named("ledger.docx"), &compound(&["Book"])),
            Format::Bridge {
                target: BridgeTarget::Xlsx,
                mimetype: EXCEL_MIME,
                label: "a legacy Excel workbook",
            }
        );
    }

    #[test]
    fn a_legacy_presentation_without_the_stream_is_still_bridged_by_name() {
        // PowerPoint 4 and 95 wrote a binary deck that is not this container;
        // Impress reads those too, so the extension is the promise the run
        // makes and the bridge is where it is kept.
        let ppt4 = [0x00, 0x00, 0x0E, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(
            detect_bytes(&named("deck.ppt"), &ppt4),
            Format::Bridge {
                target: BridgeTarget::Pptx,
                mimetype: POWERPOINT_MIME,
                label: "a legacy PowerPoint presentation",
            }
        );
        // A compound file with no PowerPoint stream is the same promise: the
        // extension says what it is meant to be, and LibreOffice decides.
        assert_eq!(
            detect_bytes(&named("deck.ppt"), &compound(&["CompObj"])),
            Format::Bridge {
                target: BridgeTarget::Pptx,
                mimetype: POWERPOINT_MIME,
                label: "a legacy PowerPoint presentation",
            }
        );
    }

    #[test]
    fn an_older_works_file_is_recognised_by_its_header_and_name() {
        // Every one of the 26 Works spreadsheet samples starts with its own
        // header rather than a container signature — `FF 00 02 00 04 04 05 54`
        // is Works 4/5's — so the extension is what names the format there.
        let sheet = [0xFF, 0x00, 0x02, 0x00, 0x04, 0x04, 0x05, 0x54];
        assert_eq!(
            detect_bytes(&named("SHEET1.WKS"), &sheet),
            Format::Bridge {
                target: BridgeTarget::Xlsx,
                mimetype: WORKS_MIME,
                label: "a Microsoft Works spreadsheet",
            }
        );
    }

    #[test]
    fn the_dos_era_works_signature_is_recognised() {
        let dos = [0x01, 0xFE, 0x01, 0x00, 0x0D, 0x00, 0x00, 0x00, 0xD0, 0x02];
        assert!(matches!(
            detect_bytes(&named("SIMPLE.WPS"), &dos),
            Format::Bridge {
                target: BridgeTarget::Docx,
                ..
            }
        ));
    }

    #[test]
    fn text_keeps_the_reader_its_extension_asks_for() {
        assert_eq!(
            detect_bytes(&named("notes.md"), b"# heading"),
            Format::Flow("md")
        );
        assert_eq!(
            detect_bytes(&named("t.csv"), b"a,b\n1,2"),
            Format::Flow("csv")
        );
        assert_eq!(detect_bytes(&named("t.tsv"), b"a\tb"), Format::Flow("tsv"));
        // A `.wps` that is only words is read as words rather than refused.
        assert_eq!(
            detect_bytes(&named("letter.wps"), b"hello"),
            Format::Flow("txt")
        );
        // A UTF-16 text file is text, not binary.
        let utf16 = [0xFF, 0xFE, b'h', 0, b'i', 0];
        assert_eq!(detect_bytes(&named("t.txt"), &utf16), Format::Flow("txt"));
    }

    #[test]
    fn a_broken_pdf_is_still_reported_as_a_pdf() {
        // Bytes that say nothing must not turn a declared PDF into plain text:
        // the parse error is the useful answer, not a page of binary.
        assert_eq!(
            detect_bytes(&named("broken.pdf"), b"not really a pdf"),
            Format::Pdf
        );
    }

    #[test]
    fn an_unknown_binary_is_named_and_refused() {
        // An ELF header has NULs in the first eight bytes: binary, and no text
        // extension to excuse it.
        let mut binary = vec![0x7F, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00];
        binary.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(
            detect_bytes(&named("a.bin"), &binary),
            Format::Unsupported("a binary file of no format this reads")
        );
        // The same bytes under a `.txt` name are still text: the text reader
        // decodes whatever it is given, and refusing would help nobody.
        assert_eq!(detect_bytes(&named("a.txt"), &binary), Format::Flow("txt"));
        assert_eq!(
            detect_bytes(&named("a.doc"), &compound(&["CompObj"])),
            Format::Unsupported("a compound file with no Word stream")
        );
    }

    #[test]
    fn what_the_bridge_reads_is_more_than_what_only_it_can_read() {
        // A legacy `.doc`: read here, and read differently here than through
        // Writer, so its reading depends on the bridge without the bridge being
        // its only reader.
        assert_eq!(
            detect_bytes(&named("brief.doc"), &compound(&["WordDocument"])),
            Format::Doc
        );
        assert!(reads_through_libreoffice(Format::Doc));
        assert!(!is_bridged("doc"), "a `.doc` has a reader here");

        // A Works file: no reader here at all, so the bridge is not a better
        // reading of it but the only one.
        let works = [0x01, 0xFE, 0x00, 0x00];
        assert!(reads_through_libreoffice(detect_bytes(
            &named("ledger.wks"),
            &works
        )));
        assert!(is_bridged("wks"));

        // And the formats that are neither: what is read here is read here.
        assert!(!reads_through_libreoffice(Format::Pdf));
        assert!(!reads_through_libreoffice(Format::Flow("html")));
        assert!(!reads_through_libreoffice(detect_bytes(
            &named("report.docx"),
            &package(&["word/document.xml"])
        )));
    }

    #[test]
    fn libreoffice_is_found_by_name_or_by_override() {
        // An explicit override is used or refused; it never falls back to
        // `PATH` behind the caller's back.
        let missing = std::env::temp_dir().join("pdfe-no-such-soffice");
        assert_eq!(resolve_soffice(Some(missing), None), None);
        let real = std::env::current_exe().expect("the test binary");
        assert_eq!(resolve_soffice(Some(real.clone()), None), Some(real));
        // Without one, a directory on the search path holding `soffice` wins.
        let directory = std::env::temp_dir().join(format!("pdfe-soffice-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("scratch dir");
        let fake = directory.join("soffice");
        std::fs::write(&fake, b"#!/bin/sh\n").expect("fake binary");
        let search = std::ffi::OsString::from(directory.as_os_str());
        assert_eq!(resolve_soffice(None, Some(search)), Some(fake));
        let _ = std::fs::remove_dir_all(&directory);
    }
}
