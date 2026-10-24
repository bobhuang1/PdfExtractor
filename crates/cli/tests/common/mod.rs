//! What the golden tests share.
//!
//! A golden is more than its document. A rendering that names a file written
//! beside itself — a Markdown `![Image](…)`, an HTML `<img src="…">`, a YAML
//! `uri:` — is only reproduced when that file is, and a copy that was
//! re-encoded, truncated or emptied still satisfies the reference. So every
//! asset a golden names is pinned byte for byte beside the golden, under the
//! same relative path the document writes, and regenerated with the goldens
//! themselves.
//!
//! The discovery is by reference rather than by case: a writer that starts
//! naming a new file is pinned the moment the golden names one, and a pinned
//! file no golden names any more is a failure rather than a leftover.

use std::path::{Path, PathBuf};

/// One golden's assets: where this run wrote them, where they are committed,
/// and how to regenerate them.
pub struct Pins<'a> {
    /// The case's name, for the failure messages.
    pub label: &'a str,
    /// The directory this run wrote its document into; a reference in the
    /// golden is resolved against it.
    pub produced: &'a Path,
    /// The golden directory itself: a reference resolved against it names the
    /// committed copy. Documents sit in it directly, assets in subdirectories.
    pub pinned: &'a Path,
    /// Whether to write the pins instead of comparing them.
    pub update: bool,
    /// The command that regenerates them, quoted in a failure.
    pub regenerate: &'a str,
}

impl Pins<'_> {
    /// Hold every asset `golden` names to its committed bytes, and pin any the
    /// run just wrote that nothing has pinned yet.
    pub fn check(&self, golden: &str) {
        for reference in referenced_assets(golden) {
            let produced = self.produced.join(&reference);
            let pin = self.pinned.join(&reference);
            let produced = std::fs::read(&produced).ok();
            let pinned = std::fs::read(&pin).ok();

            match (produced, pinned) {
                // A reference to something the document did not write — a link
                // to a page beside it, say — is not an asset of the golden.
                (None, None) => {}
                (None, Some(_)) => panic!(
                    "{}: the golden names {reference}, but this run wrote nothing beside the document",
                    self.label
                ),
                (Some(_), None) if !self.update => panic!(
                    "{}: {reference} is referenced by the golden but not pinned; regenerate with {}",
                    self.label, self.regenerate
                ),
                (Some(produced), None) => {
                    if let Some(parent) = pin.parent() {
                        std::fs::create_dir_all(parent).expect("create the pin directory");
                    }
                    std::fs::write(&pin, &produced).expect("pin the asset");
                }
                (Some(produced), Some(pinned)) => {
                    if produced == pinned {
                        continue;
                    }
                    if self.update {
                        // A regenerating run writes what this run produced; the
                        // diff of the pins is the review, as it is for the
                        // documents.
                        std::fs::write(&pin, &produced).expect("repin the asset");
                    } else {
                        panic!(
                            "{}: {reference} changed ({} bytes this run, {} pinned); regenerate with {}",
                            self.label,
                            produced.len(),
                            pinned.len(),
                            self.regenerate
                        );
                    }
                }
            }
        }
    }
}

/// A whole golden set's pins, checked once every document in it has been read:
/// a file left in an asset directory that no golden names is either a stale pin
/// from a document that no longer writes it, or a file committed by hand and
/// never checked. Both are failures, and a regenerating run takes it away.
///
/// This is a sweep of the set rather than of one document, because the asset
/// directories sit side by side: one document's pictures are not stale merely
/// because another document does not name them.
pub struct Sweep<'a> {
    /// The golden directory whose subdirectories hold the pins.
    pub pinned: &'a Path,
    /// Whether to remove what nothing names instead of failing on it.
    pub update: bool,
    /// The command that regenerates the set, quoted in a failure.
    pub regenerate: &'a str,
}

impl Sweep<'_> {
    pub fn check(&self, goldens: &[String]) {
        let mut named: Vec<PathBuf> = Vec::new();
        for golden in goldens {
            for reference in referenced_assets(golden) {
                let path = PathBuf::from(reference);
                if !named.contains(&path) {
                    named.push(path);
                }
            }
        }

        for stale in pinned_assets(self.pinned, &named) {
            if self.update {
                let _ = std::fs::remove_file(&stale);
                if let Some(parent) = stale.parent() {
                    // An asset directory left empty is not a pin either.
                    let _ = std::fs::remove_dir(parent);
                }
            } else {
                panic!(
                    "{} is pinned but no golden names it; regenerate with {}",
                    stale.display(),
                    self.regenerate
                );
            }
        }
    }
}

/// The local files a golden names, in the three syntaxes the writers use:
/// Markdown `[text](destination)` and `![alt](destination)`, HTML
/// `<img src="…">`, and the document model's YAML `uri:` field.
///
/// Only a relative path can name a file beside the document: an address with a
/// scheme (`https://…`, `data:…`, `mailto:…`) or a rooted path names something
/// the writer never put there, and is not an asset to pin.
pub fn referenced_assets(golden: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in golden.lines() {
        // Markdown destinations. The scan restarts after each one, so a line
        // holding several references names them all.
        let mut rest = line;
        while let Some(at) = rest.find("](") {
            let after = &rest[at + 2..];
            let Some(end) = after.find(')') else { break };
            remember(&mut found, &after[..end]);
            rest = &after[end..];
        }

        let mut rest = line;
        while let Some(at) = rest.find("src=\"") {
            let after = &rest[at + 5..];
            let Some(end) = after.find('"') else { break };
            remember(&mut found, &after[..end]);
            rest = &after[end..];
        }

        if let Some(value) = line.trim_start().strip_prefix("uri:") {
            remember(&mut found, value);
        }
    }
    found.sort();
    found
}

/// Keep `reference` when it is a relative path not already recorded. Quotes,
/// angle brackets and surrounding space are the syntax's, not the path's — the
/// Markdown writer puts an address with a space in `<…>`, and YAML may quote a
/// value.
fn remember(found: &mut Vec<String>, reference: &str) {
    let reference = reference
        .trim()
        .trim_matches(|c| c == '<' || c == '>' || c == '"' || c == '\'');
    let reference = reference.trim();
    if reference.is_empty() || !is_local(reference) {
        return;
    }
    if !found.iter().any(|seen| seen == reference) {
        found.push(reference.to_string());
    }
}

/// Whether a reference can name a file beside the document rather than an
/// address somewhere else: a relative path and nothing more. A colon is any
/// scheme (`data:`, `mailto:`, `cid:`) and a Windows drive; a leading `/` or
/// `\` is a rooted path, and `//host` is an address with its scheme dropped.
fn is_local(reference: &str) -> bool {
    !reference.is_empty() && !reference.starts_with(['/', '\\', '#']) && !reference.contains(':')
}

/// Every file under `pinned` that sits in a subdirectory — the asset
/// directories — and that `named` does not name.
fn pinned_assets(pinned: &Path, named: &[PathBuf]) -> Vec<PathBuf> {
    let mut stale = Vec::new();
    let entries = match std::fs::read_dir(pinned) {
        Ok(entries) => entries,
        Err(_) => return stale,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            stale.extend(files_under(&path, named));
        }
    }
    stale.sort();
    stale
}

fn files_under(directory: &Path, named: &[PathBuf]) -> Vec<PathBuf> {
    let mut stale = Vec::new();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return stale,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            stale.extend(files_under(&path, named));
        } else if !named.iter().any(|name| path.ends_with(name)) {
            stale.push(path);
        }
    }
    stale
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three syntaxes name the same asset, one definition is not counted
    /// twice, and an address that is not beside the document is not an asset.
    #[test]
    fn references_are_found_in_every_syntax_and_only_when_local() {
        let golden = "\
![Image](doc_images/img1.png)\n\
<p><img src=\"doc_images/img2.png\" alt=\"Image\"></p>\n\
    uri: doc_images/img1.png\n\
[the spec](https://example.com/spec)\n\
[notes](notes.txt)\n\
![spaced](<doc_images/a b.png>)\n\
![again](doc_images/img1.png)\n";
        assert_eq!(
            referenced_assets(golden),
            vec![
                "doc_images/a b.png".to_string(),
                "doc_images/img1.png".to_string(),
                "doc_images/img2.png".to_string(),
                "notes.txt".to_string(),
            ]
        );
    }

    #[test]
    fn only_relative_paths_are_local() {
        for reference in [
            "https://example.com/a.png",
            "//example.com/a.png",
            "data:image/png;base64,AAAA",
            "mailto:someone@example.com",
            "/absolute/a.png",
            "C:/a.png",
            "#anchor",
            "",
        ] {
            assert!(!is_local(reference), "{reference} should not be local");
        }
        for reference in ["a.png", "doc_images/a.png"] {
            assert!(is_local(reference), "{reference} should be local");
        }
    }
}
