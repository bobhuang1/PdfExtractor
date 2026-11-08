//! `--page-range`: which pages of a document to read.
//!
//! A range is a comma-separated list of 1-based page numbers and inclusive
//! spans: `1-4,9` reads pages 1, 2, 3, 4 and 9, and `7-` reads page 7 to the
//! end. It is applied to the pages the file has, **before** any of them is
//! analysed, so the furniture pass sees exactly the pages the reader asked for:
//! a running head repeated on pages the reader kept is furniture, and one that
//! appears once only because its neighbours were dropped is not.
//!
//! Page numbers are the file's own. A document read as pages 2–3 still says its
//! pages are 2 and 3, so a citation points at the page a reader of the original
//! would turn to rather than at the passage's position in the extract.

use std::fmt;

/// The pages a run reads, as inclusive 1-based spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRange {
    /// Inclusive spans; an end of `i32::MAX` is "to the document's last page".
    spans: Vec<(i32, i32)>,
}

impl PageRange {
    /// Parse `1-4,9` (also `3`, and `7-` for "to the end").
    ///
    /// The error names the entry that could not be read and why, because a
    /// range is the sort of thing a script writes and a person debugs from the
    /// message.
    pub fn parse(spec: &str) -> Result<Self, String> {
        if spec.trim().is_empty() {
            return Err("--page-range needs at least one page, e.g. `1-4,9`".to_string());
        }
        let mut spans = Vec::new();
        for piece in spec.split(',') {
            let piece = piece.trim();
            if piece.is_empty() {
                return Err(format!(
                    "--page-range `{spec}` has an empty entry: write `1-4,9`"
                ));
            }
            let span = match piece.split_once('-') {
                Some((from, to)) => {
                    let from = page_number(from, piece)?;
                    let to = if to.trim().is_empty() {
                        i32::MAX
                    } else {
                        page_number(to, piece)?
                    };
                    if to < from {
                        return Err(format!(
                            "--page-range `{piece}` runs backwards: pages are named low to high"
                        ));
                    }
                    (from, to)
                }
                None => {
                    let page = page_number(piece, piece)?;
                    (page, page)
                }
            };
            spans.push(span);
        }
        Ok(Self { spans })
    }

    /// Whether page `page_no` — a page's own number in the file — is read.
    pub fn contains(&self, page_no: i32) -> bool {
        page_no >= 1
            && self
                .spans
                .iter()
                .any(|(from, to)| page_no >= *from && page_no <= *to)
    }
}

/// One page number, refused when it is not a number or not a page.
fn page_number(text: &str, piece: &str) -> Result<i32, String> {
    let text = text.trim();
    let number: i32 = text
        .parse()
        .map_err(|_| format!("--page-range `{piece}`: `{text}` is not a page number"))?;
    if number < 1 {
        return Err(format!("--page-range `{piece}`: pages are numbered from 1"));
    }
    Ok(number)
}

/// The range as it would be written again: `1-4,9`, `7-` for an open end.
impl fmt::Display for PageRange {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pieces: Vec<String> = self
            .spans
            .iter()
            .map(|(from, to)| {
                if *to == i32::MAX {
                    format!("{from}-")
                } else if from == to {
                    format!("{from}")
                } else {
                    format!("{from}-{to}")
                }
            })
            .collect();
        write!(out, "{}", pieces.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_of_pages_and_spans_reads_exactly_those_pages() {
        let range = PageRange::parse("1-4,9").expect("a range");
        for page in [1, 2, 3, 4, 9] {
            assert!(range.contains(page), "page {page} should be read");
        }
        for page in [0, 5, 6, 8, 10, 100] {
            assert!(!range.contains(page), "page {page} should not be read");
        }
        assert_eq!(range.to_string(), "1-4,9");
    }

    #[test]
    fn a_single_page_is_a_span_of_one() {
        let range = PageRange::parse("3").expect("a range");
        assert!(range.contains(3));
        assert!(!range.contains(2));
        assert!(!range.contains(4));
        assert_eq!(range.to_string(), "3");
    }

    #[test]
    fn an_open_end_runs_to_the_documents_last_page() {
        let range = PageRange::parse("7-").expect("a range");
        assert!(range.contains(7));
        assert!(range.contains(10_000));
        assert!(!range.contains(6));
        assert_eq!(range.to_string(), "7-");
    }

    #[test]
    fn spaces_around_the_numbers_are_the_writers_not_the_ranges() {
        let range = PageRange::parse(" 1 - 3 , 5 ").expect("a range");
        assert!(range.contains(1) && range.contains(3) && range.contains(5));
        assert!(!range.contains(4));
        assert_eq!(range.to_string(), "1-3,5");
    }

    #[test]
    fn a_range_that_cannot_be_read_is_refused_with_the_entry_named() {
        for (spec, expected) in [
            ("", "at least one page"),
            ("abc", "not a page number"),
            ("0", "numbered from 1"),
            ("3-1", "runs backwards"),
            ("1,,2", "empty entry"),
            ("-3", "not a page number"),
        ] {
            let error = PageRange::parse(spec).expect_err(spec);
            assert!(error.contains(expected), "{spec:?} gave {error:?}");
            assert!(error.contains("--page-range"), "{spec:?} gave {error:?}");
        }
    }
}
