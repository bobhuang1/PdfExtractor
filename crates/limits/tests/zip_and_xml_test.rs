use std::io::{Cursor, Write};

use pdfextractor_limits::{LimitError, Limits, Package};
use zip::write::SimpleFileOptions;

fn package(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes, deflate) in files {
        let method = if *deflate {
            zip::CompressionMethod::Deflated
        } else {
            zip::CompressionMethod::Stored
        };
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(method),
            )
            .expect("start a ZIP entry");
        writer.write_all(bytes).expect("write a ZIP entry");
    }
    writer.finish().expect("finish the ZIP").into_inner()
}

/// The caller's entry ceiling governs the read itself, not only the preflight:
/// a part over it is not decoded at all, and nothing is charged for it.
#[test]
fn a_custom_entry_budget_governs_the_actual_read() {
    let data = package(&[("word/document.xml", b"<document/>", false)]);
    let limits = Limits {
        max_zip_entry_bytes: 4,
        ..Limits::default()
    };
    let mut archive = Package::new(&data, &limits).expect("a ZIP");
    assert!(
        archive.read_name("word/document.xml").is_none(),
        "a part over the caller's ceiling is not read"
    );
    assert_eq!(archive.decoded_bytes(), 0);
}

/// The package total is charged as parts are read, so a package of many parts
/// cannot expand past it even when every part is under its own ceiling.
#[test]
fn a_package_total_stops_the_reads_after_it_is_spent() {
    let data = package(&[("a.xml", b"<a/>", false), ("b.xml", b"<b/>", false)]);
    let limits = Limits {
        max_zip_total_bytes: 4,
        ..Limits::default()
    };
    let mut archive = Package::new(&data, &limits).expect("a ZIP");
    let first = archive.read_name("a.xml").expect("the first part fits");
    assert_eq!(first.len(), 4);
    assert_eq!(archive.decoded_bytes(), 4);
    assert!(
        archive.read_name("b.xml").is_none(),
        "the package total is already spent"
    );
    assert_eq!(archive.decoded_bytes(), 4, "a refused read is not counted");
}

#[test]
fn a_declared_zip_bomb_is_refused_before_it_expands() {
    let data = package(&[("word/document.xml", &vec![b' '; 2 * 1024 * 1024], true)]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&data)).expect("a ZIP");
    let limits = Limits {
        max_zip_entry_bytes: 1024,
        ..Limits::default()
    };
    let error = limits.check_zip(&mut archive).unwrap_err();
    assert!(
        matches!(error, LimitError::ZipEntryTooLarge { .. }),
        "unexpected refusal: {error}"
    );
    assert!(error.to_string().contains("word/document.xml"));
}

#[test]
fn a_zip_with_too_many_entries_is_refused() {
    let data = package(&[
        ("a.xml", b"<a/>", false),
        ("b.xml", b"<b/>", false),
        ("c.xml", b"<c/>", false),
    ]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&data)).expect("a ZIP");
    let limits = Limits {
        max_zip_entries: 2,
        ..Limits::default()
    };
    let error = limits.check_zip(&mut archive).unwrap_err();
    assert!(matches!(
        error,
        LimitError::TooManyZipEntries { actual: 3, .. }
    ));
}

#[test]
fn a_deeply_nested_xml_part_is_refused_at_the_package_gate() {
    let mut xml = String::new();
    for _ in 0..12 {
        xml.push_str("<a>");
    }
    for _ in 0..12 {
        xml.push_str("</a>");
    }
    let data = package(&[("word/document.xml", xml.as_bytes(), true)]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&data)).expect("a ZIP");
    let limits = Limits {
        max_xml_depth: 8,
        ..Limits::default()
    };
    let error = limits.check_zip(&mut archive).unwrap_err();
    assert!(matches!(error, LimitError::XmlTooDeep { .. }));
}

#[test]
fn a_deeply_nested_relationship_part_is_refused_at_the_package_gate() {
    let data = package(&[(
        "word/_rels/document.xml.rels",
        b"<Relationships><Relationship><a><b><c/></b></a></Relationship></Relationships>",
        true,
    )]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&data)).expect("a ZIP");
    let limits = Limits {
        max_xml_depth: 3,
        ..Limits::default()
    };
    let error = limits.check_zip(&mut archive).unwrap_err();
    assert!(matches!(error, LimitError::XmlTooDeep { .. }));
}

#[test]
fn malformed_svg_media_is_carried_for_the_renderer_to_refuse() {
    let data = package(&[(
        "word/media/image1.svg",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"/></svg>",
        true,
    )]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&data)).expect("a ZIP");
    Limits::default()
        .check_zip(&mut archive)
        .expect("a malformed drawing is still the package's bytes");
}

#[test]
fn a_deeply_nested_svg_media_part_is_refused_at_the_package_gate() {
    let data = package(&[(
        "word/media/image1.svg",
        b"<svg><g><g><g/></g></g></svg>",
        true,
    )]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&data)).expect("a ZIP");
    let limits = Limits {
        max_xml_depth: 3,
        ..Limits::default()
    };
    let error = limits.check_zip(&mut archive).unwrap_err();
    assert!(matches!(error, LimitError::XmlTooDeep { .. }));
}

#[test]
fn a_doctype_in_a_relationship_part_is_refused() {
    let data = package(&[(
        "word/_rels/document.xml.rels",
        b"<!DOCTYPE Relationships [<!ENTITY x \"boom\">]><Relationships>&x;</Relationships>",
        false,
    )]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&data)).expect("a ZIP");
    let error = Limits::default().check_zip(&mut archive).unwrap_err();
    assert_eq!(error, LimitError::XmlDoctype);
}

#[test]
fn a_normal_package_passes_the_gate() {
    let data = package(&[
        ("word/document.xml", b"<document><body/></document>", true),
        ("word/media/image1.png", b"\x89PNG\r\n\x1a\n", false),
    ]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&data)).expect("a ZIP");
    Limits::default()
        .check_zip(&mut archive)
        .expect("within budget");
}

#[test]
fn a_doctype_is_refused_without_entity_expansion() {
    let data = package(&[(
        "word/document.xml",
        b"<!DOCTYPE a [<!ENTITY x \"boom\">]><a>&x;</a>",
        false,
    )]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&data)).expect("a ZIP");
    let error = Limits::default().check_zip(&mut archive).unwrap_err();
    assert_eq!(error, LimitError::XmlDoctype);
}
