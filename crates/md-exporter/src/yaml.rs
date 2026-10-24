//! Render a [`DoclingDocument`] as YAML — Docling's `--to yaml` shape.
//!
//! A YAML file is the *whole* document model, unlike Markdown, HTML and text,
//! which write the body and leave furniture out: `pages`, `groups`,
//! `furniture`, key-value items and provenance all appear, because the YAML
//! is the document rather than a reading of it. What the walk contributes is
//! **where the content sits in it**: the same traversal as every other writer
//! visits the body in reading order, [`Order::reorder`] lists `texts`,
//! `tables` and `pictures` in exactly that order at the top of the file —
//! with the furniture the body never reaches still present, following the
//! content instead of interleaving with it — and every `self_ref` and `$ref`
//! is rewritten to match, so the file still parses as the same document.
//!
//! The document is serialised through the JSON value first, deliberately:
//! keys then come out in the same order `--to json` writes them (serde_json's
//! map is sorted), so the same document in the two notations differ only in
//! notation — and two runs of an extract, whose `pages` map would otherwise
//! inherit a hash map's whim, are byte-identical.

use crate::walk::Order;
use pdfextractor_core::DoclingDocument;
use std::path::Path;

/// Render `doc` as the full document model in YAML, reading order first.
pub fn render(doc: &DoclingDocument) -> anyhow::Result<String> {
    let ordered = Order::of(doc).reorder(doc);
    let value: serde_json::Value = serde_json::to_value(&ordered)?;
    Ok(serde_yaml::to_string(&value)?)
}

/// Write `doc` as YAML to `path`, atomically, as every other writer does.
pub fn export_to_file(doc: &DoclingDocument, path: &Path) -> anyhow::Result<()> {
    let yaml = render(doc)?;

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.yaml".to_string());
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));

    std::fs::write(&temp, yaml)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfextractor_core::{DocumentBuilder, Group};

    /// A document whose arrays disagree with its reading order on purpose:
    /// the furniture is *listed* first because it is on every page, and the
    /// body is what a reader meets.
    fn shuffled_document() -> DoclingDocument {
        let mut b = DocumentBuilder::new("demo");
        b.add_furniture_on_page("Running head", 1, [0.0, 0.0, 1.0, 1.0], "page_header");
        b.add_text_block("Body", 1, [0.0, 0.0, 1.0, 1.0], "paragraph");
        let mut doc = b.build();

        // The builder files page furniture under the body as well; a real
        // document keeps it in a furniture group, which is also what makes
        // the model disagree with the reading order on purpose: `#/texts/0`
        // is listed *first* everywhere, and the walk meets it last.
        doc.furniture = Some(Group {
            self_ref: "#/furniture".to_string(),
            parent: None,
            children: vec![pdfextractor_core::Ref {
                ref_path: "#/texts/0".to_string(),
            }],
            content_layer: "furniture".to_string(),
            name: "_furniture_".to_string(),
            label: "unspecified".to_string(),
        });
        doc.body.children.retain(|r| r.ref_path != "#/texts/0");
        doc
    }

    /// Every pointer in the document must name an item that exists — the
    /// reorder rewrites where things are, never whether they are.
    fn assert_refs_resolve(doc: &DoclingDocument) {
        let check = |path: &str| {
            let Some(rest) = path.strip_prefix("#/") else {
                return;
            };
            let Some((kind, index)) = rest.split_once('/') else {
                return;
            };
            let Ok(index) = index.parse::<usize>() else {
                return;
            };
            let len = match kind {
                "texts" => doc.texts.len(),
                "tables" => doc.tables.len(),
                "pictures" => doc.pictures.len(),
                "groups" => doc.groups.len(),
                _ => return,
            };
            assert!(index < len, "{path} points past the end of {kind}({len})");
        };

        for r in &doc.body.children {
            check(&r.ref_path);
        }
        if let Some(furniture) = &doc.furniture {
            for r in &furniture.children {
                check(&r.ref_path);
            }
        }
        for group in &doc.groups {
            for r in &group.children {
                check(&r.ref_path);
            }
        }
        // The three arrays hold three types, so each takes its own pass.
        for item in &doc.texts {
            check(&item.self_ref);
            for r in &item.children {
                check(&r.ref_path);
            }
            if let Some(parent) = &item.parent {
                check(&parent.ref_path);
            }
        }
        for item in &doc.tables {
            check(&item.self_ref);
            for r in &item.children {
                check(&r.ref_path);
            }
            if let Some(parent) = &item.parent {
                check(&parent.ref_path);
            }
        }
        for item in &doc.pictures {
            check(&item.self_ref);
            for r in &item.children {
                check(&r.ref_path);
            }
            if let Some(parent) = &item.parent {
                check(&parent.ref_path);
            }
        }
    }

    #[test]
    fn reading_order_comes_first_and_the_arrays_are_rewritten_to_match() {
        let doc = shuffled_document();
        let yaml = render(&doc).expect("render");
        let parsed: DoclingDocument = serde_yaml::from_str(&yaml).expect("parses back");

        // The body item is now `#/texts/0`, because that is what the walk
        // meets first; the furniture follows instead of leading.
        assert_eq!(parsed.texts.len(), 2, "nothing was dropped: {yaml:#}");
        assert_eq!(parsed.texts[0].text, "Body");
        assert_eq!(parsed.texts[0].content_layer, "body");
        assert_eq!(parsed.texts[0].self_ref, "#/texts/0");
        assert_eq!(parsed.texts[1].text, "Running head");
        assert_eq!(parsed.texts[1].content_layer, "furniture");
        // Every pointer followed them: the body's child now names `Body` at
        // `#/texts/0`, and the furniture group's child now names the running
        // head at `#/texts/1` where the reorder put it. Un-remapped, the
        // furniture group would be claiming the body paragraph.
        assert_eq!(parsed.body.children[0].ref_path, "#/texts/0");
        let furniture = parsed
            .furniture
            .as_ref()
            .expect("the model keeps furniture");
        assert_eq!(furniture.children[0].ref_path, "#/texts/1");
        assert_refs_resolve(&parsed);
    }

    #[test]
    fn the_yaml_is_the_same_document_as_the_model() {
        let doc = shuffled_document();
        let yaml = render(&doc).expect("render");
        let parsed: DoclingDocument = serde_yaml::from_str(&yaml).expect("parses back");
        // Reading the whole model through YAML changed notation, not data.
        assert_eq!(
            serde_json::to_value(&parsed).expect("value"),
            serde_json::to_value(Order::of(&doc).reorder(&doc)).expect("value"),
            "the round trip through YAML lost or gained something"
        );
    }

    #[test]
    fn the_render_does_not_change_the_document_it_read() {
        let doc = shuffled_document();
        let _ = render(&doc).expect("render");
        assert_eq!(doc.texts[0].text, "Running head");
        assert_eq!(doc.texts[0].self_ref, "#/texts/0");
    }

    #[test]
    fn two_renders_of_one_document_are_byte_identical() {
        // The `pages` map is a hash map; keys are written sorted so this is
        // an identity, not a coin flip.
        let doc = shuffled_document();
        let first = render(&doc).expect("first");
        let second = render(&doc).expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn keys_come_out_in_the_order_the_json_writer_uses() {
        let yaml = render(&shuffled_document()).expect("render");
        assert!(yaml.starts_with("body:"), "{yaml}");
        assert!(yaml.contains("schema_name: DoclingDocument"), "{yaml}");
    }

    #[test]
    fn an_empty_document_is_still_a_document() {
        let yaml = render(&DocumentBuilder::new("demo").build()).expect("render");
        let parsed: DoclingDocument = serde_yaml::from_str(&yaml).expect("parses back");
        assert_eq!(parsed.name, "demo");
        assert!(parsed.texts.is_empty());
    }

    #[test]
    fn export_writes_where_asked_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("pdfe-yaml-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("out.yaml");

        export_to_file(&shuffled_document(), &path).expect("export");

        let yaml = std::fs::read_to_string(&path).unwrap();
        assert!(yaml.contains("texts:"), "{yaml}");
        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["out.yaml".to_string()], "left {entries:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
