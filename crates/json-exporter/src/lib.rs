//! JSON exporter for Docling-compatible output
use pdfextractor_core::DoclingDocument;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Write `doc` as pretty JSON to `path`.
///
/// The bytes land in a sibling temporary file that is renamed into place, so a
/// reader never sees a half-written document: a run that is cancelled or killed
/// mid-write leaves the previous file (or nothing) rather than a truncated one
/// that still parses as JSON. The rename is atomic because the temporary file
/// sits in the same directory, and therefore on the same filesystem, as the
/// destination.
///
/// The pretty form is streamed into the temporary file rather than built as a
/// second full copy of the document in memory, and the file is created with
/// `O_EXCL`: a predictable temporary name in a shared directory (or a symlink
/// planted under it) is never followed or overwritten.
pub fn export_to_file(doc: &DoclingDocument, path: &Path) -> anyhow::Result<()> {
    write_beside(path, |file| {
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, doc)?;
        writer.flush()?;
        Ok(())
    })
}

/// Write `path` through an exclusively created sibling, renaming it into place
/// once `fill` has finished.
fn write_beside(
    path: &Path,
    fill: impl FnOnce(&mut std::fs::File) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut last = None;
    for attempt in 0..16 {
        let temp = temp_path_beside(path, attempt);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Something (an earlier crash, or another user) owns that name:
                // another one costs nothing.
                last = Some(error);
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let written = fill(&mut file);
        drop(file);
        let outcome = written.and_then(|()| std::fs::rename(&temp, path).map_err(Into::into));
        if outcome.is_err() {
            // Do not leave the temporary file behind for a failed export.
            let _ = std::fs::remove_file(&temp);
        }
        return outcome;
    }
    Err(last.map_or_else(
        || {
            anyhow::anyhow!(
                "could not create a temporary file beside {}",
                path.display()
            )
        },
        anyhow::Error::from,
    ))
}

pub fn export_to_string(doc: &DoclingDocument) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(doc)?)
}

/// A uniquely named temporary file beside `path`.
///
/// The name is hidden and carries the process id and an attempt number so that
/// two processes writing the same document, and two documents written in one
/// directory run, cannot collide. `export_to_file` never reads a temporary file
/// back, so a stale one left by a hard kill only wastes space.
fn temp_path_beside(path: &Path, attempt: u32) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.json".to_string());
    path.with_file_name(format!(".{name}.{}.{attempt}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfextractor_core::DocumentBuilder;

    fn document() -> DoclingDocument {
        let mut builder = DocumentBuilder::new("demo.pdf");
        builder.add_text("Hello", 1);
        builder.build()
    }

    fn scratch(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pdfe-export-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    #[test]
    fn the_document_is_written_where_it_was_asked_for() {
        let dir = scratch("plain");
        let path = dir.join("out.json");

        export_to_file(&document(), &path).expect("export");

        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(text.contains("\"DoclingDocument\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let dir = scratch("clean");
        let path = dir.join("out.json");

        export_to_file(&document(), &path).expect("export");

        let entries: Vec<String> = std::fs::read_dir(&dir)
            .expect("list dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["out.json".to_string()], "left {entries:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sibling temporary name that already exists is not opened or truncated:
    /// the export takes another name. This is what a planted symlink (or another
    /// process's scratch file) meets.
    #[test]
    fn a_taken_temporary_name_is_not_followed() {
        let dir = scratch("taken");
        let path = dir.join("out.json");
        let taken = temp_path_beside(&path, 0);
        std::fs::write(&taken, b"do not touch").expect("plant the first name");

        export_to_file(&document(), &path).expect("export");

        assert_eq!(
            std::fs::read_to_string(&taken).expect("the planted file"),
            "do not touch",
            "the export wrote through a name it did not create"
        );
        assert!(std::fs::read_to_string(&path)
            .expect("read back")
            .contains("\"DoclingDocument\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_export_over_an_existing_file_replaces_it() {
        let dir = scratch("replace");
        let path = dir.join("out.json");
        std::fs::write(&path, b"{\"stale\": true}").expect("seed the old file");

        export_to_file(&document(), &path).expect("export");

        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(!text.contains("stale"), "the old contents survived: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
