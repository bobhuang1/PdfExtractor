//! JSON exporter for Docling-compatible output
use pdfextractor_core::DoclingDocument;
use std::path::{Path, PathBuf};

/// Write `doc` as pretty JSON to `path`.
///
/// The bytes land in a sibling temporary file that is renamed into place, so a
/// reader never sees a half-written document: a run that is cancelled or killed
/// mid-write leaves the previous file (or nothing) rather than a truncated one
/// that still parses as JSON. The rename is atomic because the temporary file
/// sits in the same directory, and therefore on the same filesystem, as the
/// destination.
pub fn export_to_file(doc: &DoclingDocument, path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(doc)?;
    let temp = temp_path_beside(path);

    std::fs::write(&temp, json)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        // Do not leave the temporary file behind for a failed export.
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

pub fn export_to_string(doc: &DoclingDocument) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(doc)?)
}

/// A uniquely named temporary file beside `path`.
///
/// The name is hidden and carries the process id so that two processes writing
/// the same document, and two documents written in one directory run, cannot
/// collide. `export_to_file` never reads a temporary file back, so a stale one
/// left by a hard kill only wastes space.
fn temp_path_beside(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.json".to_string());
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
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
