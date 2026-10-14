//! Page furniture: the running head, the footer, and the page number.
//!
//! Whether a line standing in a page's margin belongs to the document or to the
//! page is not a fact about that page. A page number says nothing on the page it
//! numbers; what makes it a page number is that the *next* page has one too, one
//! higher, in the same place. So this is a document-wide pass over every page's
//! elements, run after [`crate::analyze_page`] has laid each page out, and it
//! only relabels: the text keeps its place in the body order, because a page
//! number is still printed on the page, and dropping it would lose content.
//!
//! **What the margin alone cannot say.** A band wide enough to take
//! `arxiv_attention.pdf`'s page numbers — 52 pt up from the bottom edge, 6.6% of
//! the page — also takes `math_batchnorm.pdf`'s opening paragraph, which starts
//! 31.6 pt from the top edge of page 2. Relabelling every block that stands in a
//! margin would therefore delete a paragraph from that document, and relabelling
//! the *small* ones would not save it either: the same page's `optimization. J.
//! Mach. Learn. Res.` is set smaller than its own body text.
//!
//! **What the repetition says.** The same text, digits aside, standing in the
//! same band of two or more pages. That is the running head, the footer and the
//! page number, and nothing else in the corpus: over the seven real PDFs the rule
//! fires on 85 items, every one of them a page number or a running head, and on
//! none of the three PDFs that have neither (`tracemonkey.pdf`, `dummy.pdf` and
//! the IRS form's one-off small print are left exactly as they were read). It is
//! also what stops a page number being read as a section heading: a bare `13` in
//! the bottom margin outranks nothing, but it used to be labelled one.

use std::collections::{HashMap, HashSet};

use crate::Element;

/// How much of a page's height at each edge counts as its margin.
///
/// Measured, not chosen: the deepest item that is furniture in the corpus is
/// `arxiv_attention.pdf`'s page number, whose box ends 52.1 pt (6.6%) above the
/// bottom of a 792 pt page, and the highest item that is *not* — a paragraph
/// opening `math_batchnorm.pdf`'s page 2 — ends 63.8 pt (8.1%) below the top.
/// Seven per cent takes the first and leaves the second.
pub const FURNITURE_BAND: f64 = 0.07;

/// One page as the furniture pass and the emitter see it: its own size, and the
/// elements layout found on it, in reading order.
#[derive(Debug, Clone)]
pub struct PageElements {
    pub size: [f64; 2],
    pub elements: Vec<Element>,
    /// The horizontal rules the page draws — thin, wide painted paths, `[l, t, r, b]`
    /// with a top-left origin. Empty for a producer that publishes no drawing, and
    /// needed by nothing the corpus labels `page_header`/`page_footer`: a rule is what
    /// tells a *note* at the foot of a page from a paragraph that merely ran on.
    pub rules: Vec<[f64; 4]>,
}

/// Whether a label is one of a page's own furniture.
///
/// The emitter asks this to decide whether an item is the document or the page.
pub fn is_furniture_label(label: &str) -> bool {
    matches!(label, HEADER | FOOTER)
}

/// The two labels a page's own furniture can carry.
const HEADER: &str = "page_header";
const FOOTER: &str = "page_footer";

/// The label a note at the foot of a page carries.
///
/// Not one of the page's two: a note is the document's own text, set where the page
/// had room for it, and `is_furniture_label` rightly keeps it in the body. It is found
/// here because it is found the way the running heads are — from the page's own marks
/// — not because it belongs to the page.
const FOOTNOTE: &str = "footnote";

/// How deep the foot of a page reaches: the share of its height in which a note may
/// stand.
///
/// Measured, not chosen: the highest note the corpus holds is `arxiv_attention.pdf`
/// page 1's `∗Equal contribution…`, whose box opens 24.8% down the page.
pub const FOOTNOTE_BAND: f64 = 0.30;

/// How far above a block a rule may stand and still be the rule that opens it.
///
/// Generous, because the rule and the note are laid out independently: on
/// `math_seq2seq.pdf` page 4 the rule stands 23 pt above the note's first line.
const RULE_GAP: f64 = 30.0;

/// Whether a block stands in the foot of a page — the part of it a note is set in.
///
/// The band, and nothing else: what a block *is* also takes the rule above it and the
/// marker it opens with, and those are asked where they are read.
pub(crate) fn in_the_foot(bbox: [f64; 4], height: f64) -> bool {
    bbox[1] >= height * (1.0 - FOOTNOTE_BAND)
}

/// Whether one of the page's rules stands above a block.
///
/// The rule has to be in the page's lower half — a table's border high on the page is
/// not the mark above a note — at or above the block's top, and overlapping it
/// horizontally. A rule beside the block, or across the other column, is not one.
///
/// Layout asks this too, before it reads a block as a table: the rule a page draws above
/// its notes stands where a table's top border would, and it is what tells the two apart.
pub(crate) fn rule_above(bbox: [f64; 4], rules: &[[f64; 4]], height: f64) -> bool {
    rules.iter().any(|rule| {
        rule[3] <= bbox[1] + RULE_GAP
            && rule[1] >= height / 2.0
            && rule[0] < bbox[2]
            && rule[2] > bbox[0]
    })
}

/// Whether a block runs out to a form's dotted leader: three or more dots in a row,
/// with single spaces allowed between them (`8 Additional income from Schedule 1, line
/// 10 . . . . . .`).
///
/// A note never carries one, and a filled-in form's field line always does — which is
/// what tells the two apart now that layout hands both over. The rule above a note
/// stands where a form draws a line too, so the rule alone cannot: `irs_f1040.pdf`'s
/// ruled field lines opened with a number under a rule exactly as `math_seq2seq.pdf`'s
/// notes do. Measured against Docling: the longest run of dots in any of the corpus's 27
/// reference notes is one, and every one of the form's lines has a leader.
fn has_leader(text: &str) -> bool {
    let mut run = 0usize;
    let mut gap = 0usize;
    for character in text.chars() {
        match character {
            // A dot restarts the count of spaces: `e.g.,` is a full stop followed by a
            // word, and `. . . .` is a leader.
            '.' => {
                run += 1;
                gap = 0;
            }
            '…' => {
                run += 3;
                gap = 0;
            }
            ' ' if run > 0 => {
                gap += 1;
                if gap > 1 {
                    run = 0;
                    gap = 0;
                }
            }
            _ => {
                run = 0;
                gap = 0;
            }
        }
        if run >= 3 {
            return true;
        }
    }
    false
}

/// The marker a note opens with: a short number, or one of the symbols a document
/// uses for notes — or `None` where the block opens with something else.
///
/// A note's number is part of its *text*, because our reading glues a superscript to
/// the word it marks: `4To illustrate…`. A list's number is not — layout has already
/// taken it out of the text and into the element's `marker` — which is what keeps a
/// numbered list at the foot of a page from reading as a note.
fn footnote_marker(text: &str) -> Option<&str> {
    let text = text.trim_start();
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        // Longer runs are years and quantities, not markers.
        return (digits <= 2).then(|| &text[..digits]);
    }
    let first = text.chars().next()?;
    matches!(first, '*' | '∗' | '†' | '‡' | '§' | '¶').then(|| &text[..first.len_utf8()])
}

/// Label the document's footnotes: the small print a page sets at its foot, under the
/// rule it draws above it.
///
/// A note is found the way a reader finds it, by the two marks that make one: the
/// **rule** the page draws above its notes, and the **marker** the note opens with,
/// which the body of the page also uses (`4To illustrate…` against the `4` in the
/// text). Both are asked for, and the place as well: a rule on its own is a table's
/// border or a box's edge, a number on its own is a list or a sentence that happens to
/// begin with one, and a page's conference line under its own rule is neither — its
/// number is not one the text uses.
///
/// Returns how many items were relabelled, the number worth reporting.
pub fn mark_footnotes(pages: &mut [PageElements]) -> usize {
    let mut labelled = 0;
    for page in pages.iter_mut() {
        let PageElements {
            size,
            elements,
            rules,
        } = page;
        let height = size[1];
        // Every text of the page, by position, so the marker search can skip the block
        // it is asking about and keep asking about the others.
        let body: Vec<Option<String>> = elements
            .iter()
            .map(|element| match element {
                Element::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        for (index, element) in elements.iter_mut().enumerate() {
            let Element::Text {
                text, label, bbox, ..
            } = element
            else {
                continue;
            };
            if !matches!(label.as_str(), "paragraph" | "text" | "reference" | "code") {
                continue;
            }
            // A form's field line is not a note however well it satisfies the three
            // conditions above: it runs out to a dotted leader, which no note does.
            if has_leader(text) {
                continue;
            }
            let Some(open) = footnote_marker(text) else {
                continue;
            };
            // The block stands in the foot of the page, under a rule, and the marker it
            // opens with is one the page's body also uses. All three: a rule on its own
            // is a box's edge, a marker on its own is a sentence that opens with a
            // number, and a note is a note to something.
            if !in_the_foot(*bbox, height) {
                continue;
            }
            if !rule_above(*bbox, rules, height) {
                continue;
            }
            let used = body.iter().enumerate().any(|(other, body)| {
                other != index && body.as_deref().is_some_and(|body| body.contains(open))
            });
            if !used {
                continue;
            }
            *label = FOOTNOTE.to_string();
            labelled += 1;
        }
    }
    labelled
}

/// Which edge of the page a piece of furniture stands at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    Header,
    Footer,
}

impl Side {
    fn label(self) -> &'static str {
        match self {
            Side::Header => HEADER,
            Side::Footer => FOOTER,
        }
    }
}

/// Label the document's running heads, footers and page numbers.
///
/// Returns how many items were relabelled — how much of the document turned out
/// to be the page rather than the text, which is the number worth reporting.
///
/// A page counts once towards the repetition however many times a line is printed
/// on it: the same words twice on one page is a coincidence, and only a second
/// page makes it furniture.
pub fn mark_furniture(pages: &mut [PageElements]) -> usize {
    let mut stands_in: HashMap<(String, Side), HashSet<usize>> = HashMap::new();
    for (index, page) in pages.iter().enumerate() {
        for element in &page.elements {
            if let Some(key) = furniture_key_of(element, page.size) {
                stands_in.entry(key).or_default().insert(index);
            }
        }
    }
    let repeated: HashSet<(String, Side)> = stands_in
        .into_iter()
        .filter(|(_, pages)| pages.len() >= 2)
        .map(|(key, _)| key)
        .collect();
    if repeated.is_empty() {
        return 0;
    }

    let mut labelled = 0;
    for page in pages.iter_mut() {
        let size = page.size;
        for element in page.elements.iter_mut() {
            let key = match furniture_key_of(element, size) {
                Some(key) => key,
                None => continue,
            };
            if !repeated.contains(&key) {
                continue;
            }
            if let Element::Text {
                label,
                level,
                marker,
                ..
            } = element
            {
                if *label == key.1.label() {
                    continue;
                }
                *label = key.1.label().to_string();
                // Furniture is not a heading at whatever depth, and a page
                // number's "marker" is the whole of it.
                *level = None;
                *marker = None;
                labelled += 1;
            }
        }
    }
    labelled
}

/// Where a piece of furniture stands, and what it says — or `None` where the
/// element is not in a margin at all.
///
/// The box must lie *wholly* inside the band: a paragraph whose first line opens
/// near the top edge runs on into the text, and only a line that begins and ends
/// within the margin is the page's own.
fn furniture_key_of(element: &Element, size: [f64; 2]) -> Option<(String, Side)> {
    let Element::Text { text, bbox, .. } = element else {
        return None;
    };
    let band = size[1] * FURNITURE_BAND;
    let side = if bbox[3] <= band {
        Side::Header
    } else if bbox[1] >= size[1] - band {
        Side::Footer
    } else {
        return None;
    };
    let key = furniture_key(text);
    (!key.is_empty()).then_some((key, side))
}

/// A block's text with everything that does not identify it taken out.
///
/// Case and spacing do not distinguish a running head from itself, and neither
/// does the number in it: *every* run of digits becomes one `#`, which is what
/// lets a document's page numbers be one line rather than one per page — `2`,
/// `3`, … and `page 1 / 3`, `page 2 / 3`, … each reduce to a single key. It also
/// means `2025` and `2026` in a margin are the same furniture, which is right:
/// the year of a form's revision is the page's, not the text's.
fn furniture_key(text: &str) -> String {
    let mut key = String::with_capacity(text.len());
    let mut in_digits = false;
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                key.push('#');
            }
            in_digits = true;
            in_space = false;
        } else if ch.is_whitespace() {
            in_digits = false;
            in_space = true;
        } else {
            if in_space && !key.is_empty() {
                key.push(' ');
            }
            key.extend(ch.to_lowercase());
            in_digits = false;
            in_space = false;
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The size of the corpus papers' pages, so a band is 55.4 pt.
    const LETTER: [f64; 2] = [612.0, 792.0];

    fn line(text: &str, bbox: [f64; 4]) -> Element {
        Element::Text {
            text: text.to_string(),
            label: "paragraph".to_string(),
            level: None,
            marker: None,
            bbox,
        }
    }

    fn page(elements: Vec<Element>) -> PageElements {
        PageElements {
            size: LETTER,
            elements,
            rules: Vec::new(),
        }
    }

    /// A page with the rules it draws, `[l, t, r, b]` top-left.
    fn ruled(elements: Vec<Element>, rules: Vec<[f64; 4]>) -> PageElements {
        PageElements {
            size: LETTER,
            elements,
            rules,
        }
    }

    fn label_of(page: &PageElements, index: usize) -> &str {
        match &page.elements[index] {
            Element::Text { label, .. } => label,
            other => panic!("expected text, found {other:?}"),
        }
    }

    #[test]
    fn a_page_number_down_the_margin_of_every_page_is_furniture() {
        // The page numbers of the arXiv papers: 303.5, 749.8 to 308.5, 759.8 in
        // a 792 pt page, and one digit apart on each page.
        let mut pages: Vec<PageElements> = (1..=3)
            .map(|n| {
                page(vec![
                    line("Introduction", [72.0, 100.0, 200.0, 112.0]),
                    line(&n.to_string(), [303.5, 749.8, 308.5, 759.8]),
                ])
            })
            .collect();
        assert_eq!(mark_furniture(&mut pages), 3);
        for page in &pages {
            assert_eq!(label_of(page, 0), "paragraph");
            assert_eq!(label_of(page, 1), "page_footer");
        }
    }

    #[test]
    fn a_running_head_across_the_pages_is_furniture() {
        let mut pages: Vec<PageElements> = (1..=4)
            .map(|n| {
                page(vec![
                    line(
                        "Published as a conference paper at ICLR 2015",
                        [108.0, 25.4, 293.0, 35.4],
                    ),
                    line(
                        &format!("body text of page {n}"),
                        [72.0, 100.0, 400.0, 112.0],
                    ),
                ])
            })
            .collect();
        assert_eq!(mark_furniture(&mut pages), 4);
        for page in &pages {
            assert_eq!(label_of(page, 0), "page_header");
            assert_eq!(label_of(page, 1), "paragraph");
        }
    }

    #[test]
    fn a_margin_line_on_one_page_only_is_left_as_text() {
        // `irs_f1040.pdf`: the form's small print runs along the bottom of page
        // 1 alone, and Docling's own reading keeps page 2's (`Go to
        // www.irs.gov/…`) as a footer — but nothing on the page can tell us that
        // it is the page's rather than the form's, so it stays what it was read
        // as. The pass is allowed to miss furniture; it is not allowed to invent
        // it.
        let mut pages = vec![
            page(vec![line(
                "For Disclosure, Privacy Act, and Paperwork Reduction Act Notice",
                [36.0, 764.4, 400.0, 772.2],
            )]),
            page(vec![line(
                "Go to www.irs.gov/Form1040",
                [36.0, 757.7, 292.0, 766.1],
            )]),
        ];
        assert_eq!(mark_furniture(&mut pages), 0);
        assert_eq!(label_of(&pages[0], 0), "paragraph");
        assert_eq!(label_of(&pages[1], 0), "paragraph");
    }

    #[test]
    fn the_same_words_mid_page_are_not_furniture() {
        // Repetition alone is not furniture: a link repeated at the foot of a
        // landing page, a table header in the middle of two pages, a running
        // example — all of those are the document's text, and they are only
        // furniture in the margin.
        let mut pages = vec![
            page(vec![line("see Appendix A", [72.0, 300.0, 200.0, 312.0])]),
            page(vec![line("see Appendix A", [72.0, 300.0, 200.0, 312.0])]),
        ];
        assert_eq!(mark_furniture(&mut pages), 0);
    }

    #[test]
    fn one_page_cannot_make_furniture_of_itself() {
        let mut pages = vec![page(vec![
            line("1", [303.5, 749.8, 308.5, 759.8]),
            line("1", [303.5, 749.8, 308.5, 759.8]),
        ])];
        assert_eq!(mark_furniture(&mut pages), 0);
    }

    #[test]
    fn a_numbered_footer_groups_with_its_neighbours() {
        // The keys are what make a strip of changing numbers one piece of
        // furniture: only the digits differ.
        assert_eq!(furniture_key("page 1 / 3"), furniture_key("page 12 / 30"));
        assert_eq!(furniture_key("  Page   2 "), furniture_key("page 2"));
        assert_eq!(
            furniture_key("Form 1040 (2025)"),
            furniture_key("Form 1040 (2026)")
        );
        assert_ne!(furniture_key("2"), furniture_key("part 2"));
    }

    #[test]
    fn furniture_keeps_its_place_in_the_body_order() {
        // Relabelling is all it does: the text stays where the page printed it,
        // because a reader looking at the document should still find it.
        let mut pages = vec![
            page(vec![
                line("body", [72.0, 100.0, 200.0, 112.0]),
                line("1", [303.5, 749.8, 308.5, 759.8]),
            ]),
            page(vec![
                line("body", [72.0, 100.0, 200.0, 112.0]),
                line("2", [303.5, 749.8, 308.5, 759.8]),
            ]),
        ];
        mark_furniture(&mut pages);
        let texts: Vec<String> = pages[0]
            .elements
            .iter()
            .map(|e| match e {
                Element::Text { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(texts, vec!["body".to_string(), "1".to_string()]);
        // And running again changes nothing: the pass is its own fixed point.
        assert_eq!(mark_furniture(&mut pages), 0);
    }

    #[test]
    fn a_note_under_the_rule_the_page_draws_is_a_footnote() {
        // `arxiv_attention.pdf` page 1: the rule 108→251 at y 596, and the three
        // notes under it, whose markers the author lines above also use.
        let mut pages = vec![ruled(
            vec![
                line(
                    "Ashish Vaswani∗ Noam Shazeer∗ Niki Parmar∗",
                    [108.0, 232.0, 504.0, 243.0],
                ),
                line("Aidan N. Gomez∗ †", [300.0, 282.0, 400.0, 293.0]),
                line(
                    "∗Equal contribution. Listing order is random.",
                    [108.0, 596.0, 504.0, 686.0],
                ),
                line(
                    "†Work performed while at Google Brain.",
                    [120.0, 687.0, 400.0, 697.0],
                ),
            ],
            vec![[108.0, 596.0, 251.0, 596.0]],
        )];
        assert_eq!(
            mark_footnotes(&mut pages),
            2,
            "the rule separates the two notes"
        );
        assert_eq!(label_of(&pages[0], 0), "paragraph");
        assert_eq!(label_of(&pages[0], 1), "paragraph");
        assert_eq!(label_of(&pages[0], 2), "footnote");
        assert_eq!(label_of(&pages[0], 3), "footnote");
    }

    #[test]
    fn a_numbered_list_at_the_foot_is_not_a_note() {
        // The marker of a list is not in the text: layout has taken it out into the
        // element's `marker`. That is the whole difference between a note and a list
        // that happens to end a page, and it is what this test pins.
        let mut pages = vec![ruled(
            vec![
                line(
                    "a body line that mentions 1 in passing",
                    [72.0, 300.0, 400.0, 312.0],
                ),
                Element::Text {
                    text: "First step of the procedure".to_string(),
                    label: "list_item".to_string(),
                    level: None,
                    marker: Some("1".to_string()),
                    bbox: [72.0, 700.0, 400.0, 712.0],
                },
            ],
            vec![[72.0, 690.0, 300.0, 690.0]],
        )];
        assert_eq!(mark_footnotes(&mut pages), 0);
        assert_eq!(label_of(&pages[0], 1), "list_item");
    }

    #[test]
    fn a_rule_alone_does_not_make_a_note() {
        // A box edge or a table's rule is not a note, and the marker is what says so.
        let mut pages = vec![ruled(
            vec![line(
                "a paragraph that merely ran to the foot of the page",
                [72.0, 700.0, 400.0, 712.0],
            )],
            vec![[72.0, 690.0, 400.0, 690.0]],
        )];
        assert_eq!(mark_footnotes(&mut pages), 0);
    }

    #[test]
    fn a_marker_high_on_the_page_is_not_a_note() {
        // The marker and the body are there; the *place* is not. A sentence that
        // opens with a number is a sentence.
        let mut pages = vec![page(vec![
            line(
                "2011 was the year the 1 st model was published",
                [72.0, 300.0, 400.0, 312.0],
            ),
            line("1 st model", [72.0, 320.0, 200.0, 332.0]),
        ])];
        // `2011` is four digits, and a marker is one or two; the second line opens with
        // `1` but stands mid-page with no rule over it.
        assert_eq!(mark_footnotes(&mut pages), 0);
    }

    #[test]
    fn a_marker_in_the_body_with_no_rule_over_it_is_not_a_note() {
        // `tracemonkey.pdf` page 8: `2Instead of aborting the outer recording…`
        // stands at the foot of the page with `2` used in the body above it, and the
        // page draws no rule. Docling calls it a footnote; the marker and the foot are
        // not enough here, because they are also what a page's last paragraph looks
        // like, and a note is pinned by the mark the page drew above it.
        let mut pages = vec![page(vec![
            line(
                "the array implementation. 2 we reach it.",
                [54.0, 650.0, 293.0, 660.0],
            ),
            line(
                "2Instead of aborting the outer recording, we could suspend it",
                [54.0, 685.0, 293.0, 706.0],
            ),
        ])];
        assert_eq!(mark_footnotes(&mut pages), 0);
        assert_eq!(label_of(&pages[0], 1), "paragraph");
    }

    #[test]
    fn a_form_line_that_runs_out_to_a_leader_is_not_a_note() {
        // `irs_f1040.pdf` page 1's ruled field lines, which the page's own rules handed
        // over like notes once layout stopped reading them as a table: a number under a
        // rule at the foot of the page, with the marker in the body too. The dotted
        // leader is what says it is a filled-in form's line and not a note.
        let mut pages =
            vec![ruled(
                vec![
                line("1z Total amount from Form(s) W-2 8", [96.0, 640.0, 612.0, 652.0]),
                line(
                    "8 Additional income from Schedule 1, line 10 . . . . . . . . . . . . . . .8",
                    [96.0, 715.7, 612.0, 723.7],
                ),
            ],
                vec![[96.0, 712.0, 612.0, 712.0]],
            )];
        assert_eq!(mark_footnotes(&mut pages), 0);
        assert_eq!(label_of(&pages[0], 1), "paragraph");
    }

    #[test]
    fn a_note_with_a_full_stop_and_a_word_after_it_is_still_a_note() {
        // The leader test counts *dots in a row*: `e.g.,` and a sentence's full stop are
        // one dot each, however many of them a note has.
        let mut pages = vec![ruled(
            vec![
                line(
                    "see Cho et al. , 2014a; and Sec. 3.2. 1 we reach it.",
                    [54.0, 650.0, 504.0, 660.0],
                ),
                line(
                    "1 We mean by the state-of-the-art performance, the performance of the conventional",
                    [54.0, 700.0, 504.0, 710.0],
                ),
            ],
            vec![[108.0, 697.4, 251.5, 697.4]],
        )];
        assert_eq!(mark_footnotes(&mut pages), 1);
        assert_eq!(label_of(&pages[0], 1), "footnote");
    }

    #[test]
    fn a_marker_the_body_does_not_use_is_not_a_note() {
        // `arxiv_attention.pdf` page 1's conference line sits under the same rule as
        // its notes and opens with a number, and its number is not one the text uses:
        // `31` is the ordinal of the conference, not a note marker. This is the line
        // the third condition exists to keep, and Docling reads it as `page_footer`.
        let mut pages = vec![ruled(
            vec![
                line(
                    "Ashish Vaswani∗ Noam Shazeer∗",
                    [108.0, 232.0, 504.0, 243.0],
                ),
                line(
                    "31st Conference on Neural Information Processing Systems (NIPS 2017)",
                    [108.0, 700.0, 504.0, 712.0],
                ),
            ],
            vec![[108.0, 690.0, 251.0, 690.0]],
        )];
        assert_eq!(mark_footnotes(&mut pages), 0);
        assert_eq!(label_of(&pages[0], 1), "paragraph");
    }

    #[test]
    fn a_footnote_is_the_documents_text_not_the_pages() {
        // It is found beside the running heads and carries a label of its own, but it
        // stays in the body: Docling keeps a note in `texts`, and the furniture layer
        // is for what the page prints about itself.
        assert!(!is_furniture_label("footnote"));
        assert!(is_furniture_label("page_footer"));
    }

    #[test]
    fn a_heading_in_the_footer_band_is_no_longer_a_heading() {
        // What the pass fixes for a reader: `13` at the foot of a page is a page
        // number, and it was being read as a section heading.
        let mut pages: Vec<PageElements> = (12..=14)
            .map(|n| {
                page(vec![
                    line("closing paragraph", [72.0, 600.0, 400.0, 612.0]),
                    Element::Text {
                        text: n.to_string(),
                        label: "section_header".to_string(),
                        level: Some(1),
                        marker: Some(n.to_string()),
                        bbox: [303.5, 749.8, 308.5, 759.8],
                    },
                ])
            })
            .collect();
        assert_eq!(mark_furniture(&mut pages), 3);
        match &pages[0].elements[1] {
            Element::Text {
                label,
                level,
                marker,
                ..
            } => {
                assert_eq!(label, "page_footer");
                assert_eq!(*level, None, "a page number is not a heading at any depth");
                assert_eq!(*marker, None);
            }
            other => panic!("expected text, found {other:?}"),
        }
    }
}
