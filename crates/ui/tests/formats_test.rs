//! The window's format chooser against the real CLI.
//!
//! The unit tests in `lib.rs` read hand-written answers; this asks the binary the
//! window would ask, so the headings the chooser groups by are the ones `--help`
//! actually prints. A reader added to the command line's list must land in a
//! family and in the flat list both, or the chooser and a run would disagree about
//! what this build takes — which is the drift the whole ask exists to prevent.
//!
//! It skips (loudly) when the CLI has not been built, the way the other UI suites
//! do: what is being tested is the window's chooser, not the extractor.

use pdfextractor_ui::{ask_input_formats, chooser_filters};
use std::path::PathBuf;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The command line binary, or `None` when it has not been built.
fn locate_cli() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PDFEXTRACTOR_CLI") {
        return Some(PathBuf::from(path));
    }
    let candidate = repository_root().join("target/debug/pdfextractor-cli");
    candidate.exists().then_some(candidate)
}

/// The chooser is the CLI's answer, not a grouping of this side's: each family is
/// a heading with names the run takes, the families between them are exactly the
/// flat list with nothing twice, and the filters the native dialog is handed are
/// "All documents" followed by one per family, in the CLI's order.
#[test]
fn the_chooser_is_built_from_the_clis_own_families() {
    let Some(cli) = locate_cli() else {
        eprintln!("skipped: pdfextractor-cli is not built");
        return;
    };
    let formats = ask_input_formats(&cli.to_string_lossy()).expect("the CLI's format list");
    assert!(!formats.names.is_empty(), "the CLI named no formats at all");
    assert!(
        !formats.families.is_empty(),
        "the CLI printed no families, so the chooser has nothing to group by"
    );

    let mut grouped: Vec<&String> = Vec::new();
    for family in &formats.families {
        assert!(
            !family.name.trim().is_empty(),
            "a family has no name to head a filter with"
        );
        assert!(
            !family.input_formats.is_empty(),
            "the {:?} family named no format, so it is an empty filter",
            family.name
        );
        for name in &family.input_formats {
            assert!(
                formats.names.contains(name),
                "{name} is in the {:?} family but not in the flat list a run takes",
                family.name
            );
            assert!(
                !grouped.contains(&name),
                "{name} appears in more than one family"
            );
            grouped.push(name);
        }
    }
    let mut expected: Vec<&String> = formats.names.iter().collect();
    let mut actual = grouped;
    expected.sort();
    actual.sort();
    assert_eq!(
        actual, expected,
        "the families and the flat list are not the same set"
    );

    let filters = chooser_filters(&formats);
    assert_eq!(
        filters.len(),
        formats.families.len() + 1,
        "one filter per family, after All documents"
    );
    assert_eq!(filters[0].0, "All documents");
    assert_eq!(filters[0].1.len(), formats.names.len());
    for (filter, family) in filters[1..].iter().zip(&formats.families) {
        assert_eq!(
            filter.0, family.name,
            "the filters are not in the help's order"
        );
        assert!(
            !filter.1.is_empty() && filter.1.iter().all(|name| !name.starts_with('.')),
            "the {:?} filter is not a list of bare extensions",
            filter.0
        );
    }
}
