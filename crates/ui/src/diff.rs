//! Comparing our output with a Docling reference, block by block.
//!
//! The window has a **Compare** mode: point it at a reference that Docling
//! produced for the same document — a file, or a folder of them named after each
//! document, which is the layout of `test-data/golden/docling/` — and every
//! document that finishes is shown beside the reference, line by line.
//!
//! This is not the golden comparison (`cli/tests/docling_golden_test.rs`). That
//! one decides *pass or fail* against budgets and is the authority on whether
//! our reading is good enough; this one *shows* where the two readings differ,
//! so a person can look. The definitions do have to agree where they overlap —
//! the metric printed under each panel is the same "share of the reference's
//! characters that ours also has", and equations are excluded from it for the
//! same reason (Docling writes LaTeX where we linearise Office Math to Unicode,
//! so an equation's *spelling* is not evidence about anything).
//!
//! How the two documents are lined up: each is reduced to a flat list of blocks
//! in the order its body tree puts them, and the two lists are aligned the way a
//! diff aligns two files — a block is paired with at most one on the other side,
//! pairs never cross, and a block with no counterpart of its own is shown alone
//! on its side. `body.children` is what makes the order the document's own
//! rather than "every text, then every table", and a group (`#/groups/N` —
//! Docling encloses a picture area, a list or a run of formulas in one) is walked
//! where it stands, so a picture inside a picture area still comes out between
//! the paragraphs that surround it. A block the tree never mentions is appended
//! rather than dropped.
//!
//! Where both producers published a box, that box takes part in the score the
//! alignment picks pairs by, not just in the joining afterwards. The words keep the
//! gate — a pair is a link at all only if their words clear [`LINK_THRESHOLD`], and
//! no box can invent one or unmake one — but the boxes decide *which* of two links
//! the alignment takes: a pair both producers put in the same place on the same page
//! is worth [`PLACE_BONUS`] more, which is what tells apart two pairs the words
//! cannot (a picture a document lists twice, in one order on each side: only the box
//! says which copy is which), and a pair they put in different places loses
//! [`PLACE_COST`], which is what makes two such pairs lose to the one pair the boxes
//! agree about. A pair the boxes contradict is never hidden by any of this: it is a
//! row like any other, which then says where each reading put the block. Nothing of
//! it happens for a pair of equations (their text is a rendering, not words, so the
//! boxes have nothing to add to what [`EQUATION_SIMILARITY`] already does) nor for a
//! document without geometry, which is every flow format — a DOCX is compared by
//! content exactly as before.
//!
//! Two readings do not always agree about the order of the same things — a
//! drawing Docling puts after the caption it belongs to and we put before it,
//! say — and an alignment that respects order cannot pair such a pair at all:
//! pairing both would make the pairs cross. The boxes therefore decide which of
//! the crossing pairs the alignment can take (the one they agree about) and which
//! is left standing, and what is left over is joined afterwards, by **page and
//! box**: two blocks that stand alone, are the same kind of thing, and occupy the
//! same place on the same page are one thing read twice, and the row says so.
//! Only leftovers can be joined this way — content still decides everything it
//! can — and only where the producer published a box, which the PDF producers do
//! and the flow formats (DOCX, `.doc`, Markdown) do not, so an ordering
//! disagreement between two readings of a DOCX still shows one row a side. A
//! joined row carries the overlap that joined it, and the summary counts them,
//! because this step is a heuristic rather than a comparison and a reader has to
//! be able to tell the two apart.
//!
//! Docling's side of a PDF is *finer* than ours: its backend writes an item per
//! line, where the layout pass here joins the lines of a paragraph into one block.
//! An alignment that pairs by content alone therefore reports each of those
//! per-line items as one the reference has and we do not, and a panel full of that
//! says "missing" about words we hold in full. A reference item whose own *words*
//! stand, in order, inside one of our blocks is therefore a row of its own kind —
//! [`Verdict::Contained`] — and a run of consecutive items that one block of ours
//! holds is one row, so a paragraph the reference read as a dozen lines is one line
//! of the panel rather than a dozen, and what is left as "only the reference" is
//! text we really did not read. What counts as "inside" is either the reference's
//! letters running from one of our word boundaries to another — a reference item that
//! is a piece of one of our numbers is two readings, not one, and stays one-sided,
//! while a field label our own reading spaced out letter by letter is still
//! recognised as ours — or, for an item the reference wrote with **no letters in it
//! at all**, its box lying inside a block of ours *beside whose words it stands*.
//! That second arm is what a filled-in form looks like: Docling writes a dotted leader
//! or a rule as an item of its own, so on `irs_f1040` a line whose words we hold in
//! full is read by the reference as two or three word fragments with leader items
//! between them, and every leader would otherwise stand as a line saying the reference
//! holds text we do not — 266 such rows of the one form's reading. The alignment can
//! consume a run only while its items *stand together*, so a reference item we do not
//! hold sitting between two we do breaks it: those two words are still ours, and a
//! second step (`fold_contained`) takes them onto the block that holds them, the
//! leader items of the line along with them, which is why it runs before the box
//! joining.
//!
//! Three limits keep that second arm from claiming more than it can show. It decides
//! by *where* alone, so it needs a box from *both* producers (the PDFs publish one,
//! the flow formats do not): a document without geometry is compared by content
//! exactly as before. It is asked only of an item that has nothing else to offer. And
//! it takes such an item only onto a block of ours that already holds *words* of the
//! reference: a leader standing among the fragments of a line we read is part of that
//! line, while a rule standing alone in our block's area is the reference's own
//! reading of it — one thing, not a piece of another — and if the two boxes are simply
//! in the same place then the joining by position is what pairs them, and says on the
//! row that the boxes are what paired them.
//!
//! What that leaves is the rest of a form's dotted leaders: items with no letters in
//! them at all that no block of ours holds and none stands under — lines our reading
//! produced no block for, and rules we did not read as anything. Nothing can be compared
//! with such an item: it carries no letters to match, and there is nothing of ours lying
//! along it. It is therefore **placed** rather than listed: a row that says
//! what Docling wrote and where it lies, counted in [`Summary::placed`] instead of among
//! the rows only the reference has, because a panel that called punctuation its words
//! cannot match "text we do not have" would be claiming to know something it does not.
//!
//! A line of a form is not one item, though: Docling writes *each dot* of a dotted leader
//! as an item of its own, and the rule under them as another, all sharing a line and a
//! vertical range. A row each would make one line of a form twenty-odd rows of the panel —
//! `irs_f1040`'s 327 items stand on 25 lines — and the finding would be buried in the
//! reading of it. So adjacent items standing on one line of one page are placed
//! **together**: one row that names how many items it stands for and the extent they
//! cover, with [`Summary::placed`] counting rows rather than items. What lies between two
//! such items decides whether they are a run at all: they have to be neighbours in
//! *Docling's* order, so a row that holds reference items comes between them and they are
//! placed apart from it — while a block of ours read on that line holds nothing of the
//! reference's and does not, because the line is one line of the form whatever each
//! reading wrote along it.
//!
//! A placement is not left to be read off coordinates, either: the row carries the item's
//! box as fractions of the page it lies on ([`Placement`]), which the panel draws inside an
//! outline of that page — the outline being the page's own size as the reference stated it
//! — so which line of a form a leader belongs to is seen rather than worked out. A page
//! whose size the reference never states has nothing to draw the box *on*: those rows are
//! placed and named like the rest, and drawn nowhere.
//!
//! One row is one line of a form, and a form is many lines: a row's own drawing answers
//! *which line is this item on*, not *what does the form look like*. So the placed items
//! are collected the other way round as well — every one of them, grouped by the page it
//! lies on ([`PlacedPage`]) — and the panel draws each page once, with all of its boxes at
//! once, so a form's leader pattern is one picture rather than one thumbnail per row. It
//! is built before the rows are cut to [`MAX_ROWS`], so a long form's page is whole even
//! where its rows are not, and it holds *items* rather than rows: the picture is of what
//! the reference wrote, dot run by dot run, not of the lines the rows were grouped into.
//!
//! The work is bounded, because a window must not hang on a pathological
//! document: at most [`MAX_BLOCKS`] blocks a side are aligned (the alignment is
//! quadratic in that), at most [`MAX_ROWS`] rows are sent to the page, and every
//! text is clipped to [`MAX_TEXT`] characters. What was left out is counted in
//! [`Summary::truncated`], so a truncated view never claims to be the whole one.
//!
//! A comparison can also be held against a **memory**: the numbers a previous run
//! measured for the same document, kept in the repository beside the references
//! (`test-data/golden/compare-baseline.json`, written and checked by
//! `tests/corpus_report_test.rs`, which also records there *how* it was measured —
//! [`Measurement`]). A document the memory has falls below a [`Report::floor`] and the
//! panel refuses to present it as an ordinary comparison: a reading whose numbers went
//! the wrong way is news, not a list of rows to review. How much of the ratchet is held
//! follows the two measurements: when the run's settings match the remembered ones,
//! every rule applies ([`Numbers::worse_than`]); when they do not — the window's default
//! `--ocr auto` against a baseline measured with OCR off — only the **floor** under a
//! form's placed items applies ([`Numbers::below_placed_floor`]), because that floor is
//! about the reading rather than about how the two readings agree, while `differs` and
//! the character agreement are counts an OCR mode moves. The rules are the corpus
//! report's own and the numbers are [`Numbers`], reduced by [`Numbers::of`] from a
//! [`Report`] on both sides, so the panel, the ratchet and the command line's
//! `--compare` mode (which runs the same [`Report::hold_to_floor`] to fail a build) all
//! mean the same thing.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How similar two blocks have to be before they are paired. Below this they are
/// two one-sided rows, which is what a shortened or reworded block looks like.
const LINK_THRESHOLD: f64 = 0.5;

/// What a reference item wholly inside one of our blocks is worth to the
/// alignment, *per item*. Runs are made of words ([`contained_runs`]), so what this
/// buys is a line the reference read as several items being one row.
///
/// The alignment maximises its total, so this only has to beat leaving the item
/// alone, which is worth nothing; and it must not outbid a block that really
/// matches another, which clears [`LINK_THRESHOLD`] — a single contained item ties
/// that at worst. Being per item is what makes a run one row rather than a row for
/// the first item and a one-sided row for each of the rest.
const CONTAINED_SCORE: f64 = LINK_THRESHOLD;

/// Blocks aligned per side. The alignment is quadratic: 2,000 x 2,000 is four
/// million cells, which is a moment's work and a few tens of megabytes.
const MAX_BLOCKS: usize = 2000;

/// Rows sent to the page. A pair of 2,000-block documents cannot produce more
/// than 4,000, and a panel nobody can read is not a diff.
const MAX_ROWS: usize = 2000;

/// Characters kept of each side's text in a row.
const MAX_TEXT: usize = 300;

/// A hair, for a character share that has been through JSON: the baseline file records
/// the exact `f64`, and this only keeps a round-trip from being read as a regression.
const AGREEMENT_EPSILON: f64 = 1e-9;

/// The labels both tools use for ordinary body text.
///
/// Docling writes it as `text`; we write it as `paragraph`. The golden
/// comparison treats the whole set as one label for exactly the same reason: a
/// panel that called that a difference on every row of every document would
/// bury the differences a reader is looking for.
const BODY_LABELS: [&str; 4] = ["text", "paragraph", "code", "reference"];

/// How similar two equations are assumed to be.
///
/// An equation's text is never comparable — LaTeX against Unicode — so any two
/// equations are treated as the same equation as far as the *alignment* is
/// concerned, and the row is marked as a notation difference rather than a
/// defect. What the reference has and we do not is still a one-sided row.
const EQUATION_SIMILARITY: f64 = 0.9;

/// The least two pictures are assumed to have in common when nothing says they
/// are different pictures.
///
/// A picture with no image on one side and a PNG on the other are the *same*
/// picture read differently — that is the whole point of the Rasterise mode of
/// this window — but their texts (a media type and a size against "(no image)")
/// share too few bigrams to clear the link threshold on their own. Two pictures
/// are therefore paired as long as nothing better is on offer: a pair whose
/// media type and size agree exactly still scores 1.0 and is preferred. The
/// floor is withheld where the boxes put the two pictures in different places —
/// see [`picture_similarity`].
const PICTURE_FLOOR: f64 = 0.6;

/// How much of the smaller of two boxes has to lie inside the other before the
/// two blocks are taken to be the same thing in the same place.
///
/// Half is deliberately forgiving: two producers draw the box of the same table
/// or drawing slightly differently — one may take in the frame, the other only
/// the pixels — and it is the *ordering* disagreement this rule exists for. Two
/// blocks that merely brush past each other are left alone.
const PLACE_OVERLAP: f64 = 0.5;

/// What the boxes are worth to the alignment *on their own*, where the words put
/// two candidate pairings on the same footing.
///
/// A tenth of what a link is worth: enough to decide between two pairs the words
/// call equally alike — a document that lists the same drawing twice, in one order
/// on each side, where only the box says which drawing is which — and small on
/// purpose, because this nudge is how far the boxes may outweigh the words. It can
/// never make a link of its own: the words keep the gate (see [`link_score`]), so
/// two blocks in the same place whose words are not a link at all are still not
/// one (the joining by position pairs those, and says so on the row).
const PLACE_BONUS: f64 = LINK_THRESHOLD / 10.0;

/// What a pairing the boxes contradict loses to one they agree about.
///
/// As much as a link needs at all ([`LINK_THRESHOLD`]). The size is not a taste:
/// two identical pictures listed in opposite orders are offered to the alignment as
/// two pairs, each worth the full similarity, while the one pairing the boxes do
/// agree about is worth one such pair plus [`PLACE_BONUS`]. Losing this much apiece
/// is what makes those two pairs (worth `2 - 2 * PLACE_COST`) lose to the one right
/// one, so the alignment takes the pair the boxes point at and leaves the other two
/// blocks to be joined in the same place afterwards.
///
/// It is a cost in preference, not a veto: the words still decide whether there is
/// a link at all, so a block the reference read somewhere else is still one row — a
/// difference that says where each side put it — rather than two one-sided rows
/// claiming the other reading has nothing there.
const PLACE_COST: f64 = LINK_THRESHOLD;

/// Which array of a Docling document a block came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Table,
    Picture,
}

impl Kind {
    /// The kind a body-tree reference names, if it names one at all.
    fn of_ref(reference: &str) -> Option<Kind> {
        match reference.trim_start_matches("#/").split('/').next()? {
            "texts" => Some(Kind::Text),
            "tables" => Some(Kind::Table),
            "pictures" => Some(Kind::Picture),
            _ => None,
        }
    }
}

/// One comparable block of a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub kind: Kind,
    pub label: String,
    /// What the block carries: a text item's text, a table's cells, or a
    /// picture's media type and size. A picture with no image says so — that is
    /// a reading worth seeing.
    pub text: String,
    /// The page it sits on. Flow formats (DOCX, `.doc`, Markdown) publish no
    /// geometry, so this is absent for them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    /// The box it occupies: what the alignment weighs a pairing by, and what
    /// joins two rows that stand in the same place. Not sent to the page: the page
    /// paints a row from `page`, and a box no reader can see is not worth the
    /// bytes of every row of every document.
    #[serde(skip)]
    pub(crate) place: Option<Place>,
}

/// Where a block sits on its page.
///
/// Both producers publish boxes, but not in the same frame: ours are measured
/// from the page's top-left corner (`CoordOrigin::Topleft`) and Docling's from
/// its bottom-left, and the page's height is what turns one into the other. A
/// `Place` is therefore always top-left — the upper edge is the smaller `y` — so
/// that two boxes can be compared without asking which producer wrote them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Place {
    /// The page, numbered the way both producers number pages: from 1.
    pub page: i64,
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    /// The page's own size in points, when the document states it: what turns a box
    /// into a shape that can be drawn, and nothing else. No comparison uses it — a
    /// document that publishes no page sizes still has geometry wherever its producer
    /// wrote top-left boxes — and it is `None` when the size is not published, in
    /// which case the one row that draws a box draws nothing.
    pub page_size: Option<(f64, f64)>,
}

/// What a row says about the two sides.
/// (Kebab case: the page paints a row by its name, so `ours-only` is a class.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Both sides read the block the same way: same kind, same label, same text.
    Same,
    /// Both sides have the block and only its *notation* differs, which is an
    /// equation written as LaTeX on one side and Unicode on the other.
    Notation,
    /// Both sides have the block, and they disagree about something a reader
    /// would notice: the label, or the words.
    Differs,
    /// Both sides have the block and the reference's part of it lies wholly inside
    /// ours: Docling read an item per line where we read a block per paragraph, so
    /// `ours` holds one block and `theirs` the run of items inside it. The words the
    /// reference has are all there; what the two readings disagree about is where an
    /// item ends. One item of the run may hold no letters at all — a dotted leader or
    /// a rule — and be there for lying along our line, which the row's note says
    /// apart from the ones that were taken by their words.
    Contained,
    /// Only our output has it.
    OursOnly,
    /// Only the reference has it.
    TheirsOnly,
    /// The reference's item has no letters in it at all — a dotted leader, a rule — and
    /// no block of ours holds it or lies along it, so there is nothing to compare and
    /// nothing to miss: the row stands for *where* the item is rather than for text the
    /// other reading lacks. What it is and where it lies are on the row's note, and the
    /// count is [`Summary::placed`].
    Placed,
}

/// One line of the side-by-side comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub kind: Kind,
    pub verdict: Verdict,
    /// Our side of the row: one block, or none when only the reference has this.
    /// A list rather than one field because a row is a *partition* of both
    /// readings, and the reference's finer one can put a run of its items against
    /// a single block of ours — so no block is ever shown in two rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ours: Vec<Block>,
    /// The reference's side: one block, such a run, or none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub theirs: Vec<Block>,
    /// Why the row is not `same`, in words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Where the reference's item lies, on a row that stands for a *place* rather than
    /// for a comparison — the panel draws it, so a reader sees where the item is
    /// instead of reading two pairs of coordinates. Absent on every other row: a box no
    /// reader can see is not worth the bytes of every row of every document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
}

/// A box on a page, in the form a panel draws one: where it is as fractions of the page
/// and what shape that page is.
///
/// Both producers measure in points, so the fractions are of the page's own size, and
/// the outline the panel draws is the shape of the page the item was really found on.
/// Only a row that stands for a place carries one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// The page the item lies on, numbered as both producers number pages: from 1.
    pub page: i64,
    /// The page's size in points, which is the shape the box is drawn in.
    pub width: f64,
    pub height: f64,
    /// The box's left edge and top edge as fractions of the page, and its width and
    /// height the same way, all measured from the page's top-left corner — the frame
    /// the comparison puts both producers' boxes in.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Every placed item of one page, so the panel can draw a page's leader pattern in one
/// picture rather than one thumbnail per row.
///
/// The rows say *what* was placed, one line of a form at a time; this says where every
/// one of those items lies, together, on the shape of the page they share. It is built
/// from the rows *before* they are cut to [`MAX_ROWS`], so a long form's page is whole
/// even where its rows are not — the drawing is a picture of the document, not of the
/// part of it the panel had room to list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacedPage {
    /// The page, numbered as both producers number pages: from 1.
    pub page: i64,
    /// The page's own size in points, the shape the boxes are drawn in. A page whose
    /// size the reference never states has no outline to draw boxes on, so it is not
    /// listed here at all — its items are still named, one line at a time, on their rows.
    pub width: f64,
    pub height: f64,
    /// Every placed item on the page, in the reference's own order, as a box in fractions
    /// of the page — the same fractions a row's own `placement` carries.
    pub boxes: Vec<PlacedBox>,
}

/// One placed item's box, as a page drawing needs it: fractions of the page, and nothing
/// else. The page's number and size live once on [`PlacedPage`] rather than repeated on
/// every box of it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacedBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// What comparing a document found, in numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub rows: usize,
    pub same: usize,
    pub notation: usize,
    pub differs: usize,
    /// Rows where the reference's item (or the run of them) lies wholly inside one
    /// of our blocks: the same words, cut into items by the other reading — or a
    /// dotted leader or a rule, which has no words in it and lies along our line.
    pub contained: usize,
    pub ours_only: usize,
    pub theirs_only: usize,
    /// Rows that stand for a *place* rather than for a comparison: the reference's items
    /// there have no letters in them at all (dotted leaders, a rule), no block of ours
    /// holds them or lies along them, and the row says where they lie. Deliberately not
    /// counted among the rows only the reference has, which mean text we do not hold.
    ///
    /// Rows, not items: the items of one line of a form are placed as one row, which
    /// names how many of them it stands for. Counting items here instead would put a
    /// number in this tally that no row in the panel shows.
    pub placed: usize,
    /// Rows formed by joining two one-sided rows that stand in the same place:
    /// the two readings read the same thing, in a different order. Reported
    /// rather than left implicit, because content did not decide these.
    pub paired_by_position: usize,
    /// Share of the reference's alphanumeric characters that ours also has,
    /// counted as multisets so a repeated character has to be present twice.
    /// Equations are excluded; see the module note.
    pub text_agreement: f64,
    pub pages_ours: usize,
    pub pages_theirs: usize,
    /// Blocks and rows left out of this view because the work is bounded.
    pub truncated: usize,
    /// What the comparison noticed beyond the block rows: a page missing from
    /// one side, a page measured differently by more than a point.
    pub notes: Vec<String>,
}

impl Summary {
    fn of(rows: &[Row]) -> Summary {
        let mut summary = Summary {
            rows: rows.len(),
            same: 0,
            notation: 0,
            differs: 0,
            contained: 0,
            ours_only: 0,
            theirs_only: 0,
            placed: 0,
            paired_by_position: 0,
            text_agreement: 1.0,
            pages_ours: 0,
            pages_theirs: 0,
            truncated: 0,
            notes: Vec::new(),
        };
        for row in rows {
            match row.verdict {
                Verdict::Same => summary.same += 1,
                Verdict::Notation => summary.notation += 1,
                Verdict::Differs => summary.differs += 1,
                Verdict::Contained => summary.contained += 1,
                Verdict::OursOnly => summary.ours_only += 1,
                Verdict::TheirsOnly => summary.theirs_only += 1,
                Verdict::Placed => summary.placed += 1,
            }
        }
        summary
    }
}

/// One document compared with its reference, as the page receives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The document that was converted.
    pub input: String,
    /// The reference it was compared with.
    pub reference: String,
    /// Why there is nothing to compare, when there is nothing to compare.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    pub summary: Summary,
    /// The placed items of the document, grouped by the page they lie on: one picture
    /// per page showing the whole leader pattern, rather than one thumbnail per row.
    /// Empty unless something was placed, and absent from the JSON when it is — a
    /// document with no places pays nothing for the view.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placed_pages: Vec<PlacedPage>,
    pub rows: Vec<Row>,
    /// The floor this reading fell through, in words, when the memory that lives beside
    /// the reference (`compare-baseline.json`, written by the corpus report) remembers a
    /// form for this document and this run places fewer of the reference's items: the
    /// panel then refuses to present the document as a comparison, because a reading whose
    /// panel has lost items is not a reading to be reviewed row by row. `None` on every
    /// document at or above the floor, and on every run made without a memory beside its
    /// reference — which is every run of a user's own document. See
    /// [`Report::hold_to_floor`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<String>,
    /// Which rules of the ratchet held this document to the memory, and why — `Some` on
    /// every document the memory has, whether it fell or stood, and `None` where there
    /// was nothing to hold it to (no memory beside the reference, or a memory that does
    /// not know this document). The panel carries the sentence, so "this comparison was
    /// held to the whole ratchet" or "to the floor alone" is read off the panel rather
    /// than guessed from two settings screens. See [`Report::hold_to_floor`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<Held>,
}

impl Report {
    fn uncomparable(input: &str, reference: &Path, problem: String) -> Report {
        Report {
            input: input.to_string(),
            reference: reference.display().to_string(),
            problem: Some(problem),
            summary: Summary::of(&[]),
            placed_pages: Vec::new(),
            floor: None,
            held: None,
            rows: Vec::new(),
        }
    }

    /// Hold this reading to the ratchet the memory remembers for it, and answer whether
    /// it fell through.
    ///
    /// When the run's measurement settings match what the memory remembers, every ratchet
    /// rule applies (`Numbers::worse_than`); otherwise only the floor rule under a form's
    /// placed items applies (`Numbers::below_placed_floor`), because settings like OCR can
    /// change agreement counts while the structural floor about placed items remains.
    /// Which of the two it was is recorded on the report itself ([`Report::held`]), in
    /// the same words for the panel and for the command line's gate.
    ///
    /// A document the memory does not have stands under no floor, and is held to nothing.
    pub fn hold_to_floor(
        &mut self,
        memory: &Memory,
        current_measurement: Option<&Measurement>,
    ) -> bool {
        self.floor = None;
        self.held = None;
        let Some(name) = document_name(&self.input) else {
            return false;
        };
        let Some(was) = memory.documents.get(&name) else {
            return false;
        };
        let now = Numbers::of(self);
        let settings_match = match (current_measurement, &memory.measured) {
            (Some(current), measured) => measurements_match(current, measured),
            _ => false,
        };
        // The memory holds this document, so the report says *how* it was held — the
        // whole ratchet when the two measurements agree, the floor alone when they
        // cannot — whether or not anything then fell.
        self.held = Some(Held {
            rules: if settings_match {
                HeldRules::Ratchet
            } else {
                HeldRules::Floor
            },
            why: held_why(current_measurement, &memory.measured, settings_match),
        });
        if settings_match {
            let worse = now.worse_than(was);
            if !worse.is_empty() {
                self.floor = Some(worse.join(", "));
            }
        } else {
            self.floor = now.below_placed_floor(was);
        }
        self.floor.is_some()
    }
}

/// The name a document is remembered under: its file stem.
///
/// The corpus report names every document by its stem (`irs_f1040` for
/// `test-data/real/irs_f1040.pdf`), and a comparison in the window carries the input path
/// the CLI was given, so this is what has to be looked up.
fn document_name(input: &str) -> Option<String> {
    Some(Path::new(input).file_stem()?.to_string_lossy().into_owned())
}

/// One document's numbers: what the panel says about it, in the form a ratchet can keep.
///
/// One struct serves both the corpus report and the window, and deliberately: the report
/// (`tests/corpus_report_test.rs`) reduces every document to these numbers and keeps them
/// in `test-data/golden/compare-baseline.json` under the document's name, and the window
/// reduces each document to the same numbers to hold it against that file. One reduction,
/// one rule ([`Numbers::worse_than`]) and one floor ([`Numbers::below_placed_floor`]) is the
/// only way the panel and the ratchet can be guaranteed to mean the same thing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Numbers {
    pub rows: usize,
    pub same: usize,
    pub notation: usize,
    pub differs: usize,
    pub contained: usize,
    pub ours_only: usize,
    pub theirs_only: usize,
    /// Rows that stand for a *place* rather than for a comparison: the reference's items
    /// with no letters in them at all that our reading has no block for. Counted apart
    /// from `theirs_only`, which means text we did not read.
    pub placed: usize,
    /// The items of the reference those rows stand for — the number a reader would
    /// otherwise have to add up from the rows, and half of what says whether a form's line
    /// is still one row: the same items in more rows is a line that came apart.
    pub placed_items: usize,
    /// The boxes the per-page view draws for those items, and the page pictures it draws
    /// them on. A form's leader pattern is one picture per page it lies on.
    pub placed_boxes: usize,
    pub placed_page_pictures: usize,
    pub joined: usize,
    /// Share of the reference's alphanumeric characters our reading also holds — the
    /// golden suite's own figure, so the two cannot drift apart.
    pub agreement: f64,
}

impl Numbers {
    /// The numbers of a comparison, as the ratchet remembers them.
    ///
    /// The placed items are counted from the *rows* rather than from [`Summary`]: the
    /// items of one line of a form are placed as one row, and the row is what names how
    /// many of the reference's items it stands for. The boxes and the pictures come from
    /// [`Report::placed_pages`], which is built before the rows are cut to [`MAX_ROWS`], so
    /// a long form's page is counted whole even where its rows are not.
    pub fn of(report: &Report) -> Numbers {
        let summary = &report.summary;
        Numbers {
            rows: summary.rows,
            same: summary.same,
            notation: summary.notation,
            differs: summary.differs,
            contained: summary.contained,
            ours_only: summary.ours_only,
            theirs_only: summary.theirs_only,
            placed: summary.placed,
            placed_items: report
                .rows
                .iter()
                .filter(|row| row.verdict == Verdict::Placed)
                .map(|row| row.theirs.len())
                .sum(),
            placed_boxes: report
                .placed_pages
                .iter()
                .map(|page| page.boxes.len())
                .sum(),
            placed_page_pictures: report.placed_pages.len(),
            joined: summary.paired_by_position,
            agreement: summary.text_agreement,
        }
    }

    /// How this document is worse than the run the baseline remembers, if it is.
    ///
    /// Three counts may only fall, because each is a row a reader will see as a
    /// worse reading: `differs` (both readings have the block and disagree about
    /// it), `ours_only` (a block only we read) and `theirs_only` (a row only the
    /// reference has, which is text we did not read). `agreement` — the golden
    /// suite's own figure, the share of the reference's characters we hold — may
    /// only rise.
    ///
    /// Nothing else is ratcheted, and that is on purpose: `same`, `contained`,
    /// `notation` and `joined` may move either way while those hold, because a
    /// row that changes the *way* the two readings agree is not a worsening (a
    /// `same` row that becomes `contained` is the reference's finer reading of the
    /// same words, and the baseline is a record of where they landed anyway). The
    /// one-sided counts are also what keep that from being a loophole: a `differs`
    /// row can only leave the column by becoming another verdict, and the two that
    /// would hide it — `ours_only` and `theirs_only` — are ratcheted themselves.
    ///
    /// The placed rows are ratcheted as a **rate**. A form's letterless items are placed
    /// as one row per line of the form, and one page picture per page: neither the count
    /// of rows nor the count of pictures can be ratcheted on its own, because placing
    /// *more* items is a reading that found less (the improvement direction), and because
    /// a change that may only ever lower one number would forbid the corpus from ever
    /// reading a form as anything but the reading it has today. What may not happen is a
    /// line coming *apart*: the same items standing in more rows, or the same boxes in
    /// more pictures, is a form read dot by dot again, and the ratchet refuses it. The
    /// rows are compared by cross-multiplication — exact on integers, `now.items *
    /// was.rows < was.items * now.rows` — and a document that had nothing placed before
    /// (0 rows, 0 items) is never a regression from nothing. The pictures are counted
    /// rather than multiplied, because one picture covers a whole page: more of them while
    /// the boxes behind them are still there is the fragmentation this guards, and *fewer*
    /// boxes on the same pages is items answered elsewhere. A document with nothing drawn
    /// before — 0 pictures — is, like the rows, never a regression from nothing.
    ///
    /// The **items themselves** are a floor ([`Numbers::below_placed_floor`]), and the rate
    /// above is not a substitute for it.
    pub fn worse_than(&self, was: &Numbers) -> Vec<String> {
        let mut worse = Vec::new();
        let mut count = |name: &str, now: usize, before: usize| {
            if now > before {
                worse.push(format!("{name} {before} → {now}"));
            }
        };
        count("rows that differ", self.differs, was.differs);
        count("rows only ours", self.ours_only, was.ours_only);
        count("rows only the reference", self.theirs_only, was.theirs_only);
        if self.agreement < was.agreement - AGREEMENT_EPSILON {
            worse.push(format!(
                "character agreement {:.1}% → {:.1}%",
                was.agreement * 100.0,
                self.agreement * 100.0
            ));
        }
        if self.placed_items * was.placed < was.placed_items * self.placed {
            worse.push(format!(
                "form line(s) placed: {} row(s) for {} item(s) → {} row(s) for {} item(s)",
                was.placed, was.placed_items, self.placed, self.placed_items
            ));
        }
        // ...and the items as a count, because the ratio is kept by any proportional
        // fall, and 0 for 0 is not a ratio at all.
        if let Some(floor) = self.below_placed_floor(was) {
            worse.push(floor);
        }
        // The drawing is where the count *is* the shape: one picture covers a whole page,
        // so the number of pictures says nothing on its own. More pictures for at least as
        // many boxes is the leader pattern answered row by row again; the same boxes in
        // *fewer* pictures is what the view is for, and fewer boxes on the same pages is
        // items the panel now answers elsewhere — an improvement, not a regression.
        if was.placed_page_pictures > 0
            && self.placed_boxes >= was.placed_boxes
            && self.placed_page_pictures > was.placed_page_pictures
        {
            worse.push(format!(
                "form pattern drawn: {} picture(s) for {} box(es) → {} picture(s) for {} box(es)",
                was.placed_page_pictures,
                was.placed_boxes,
                self.placed_page_pictures,
                self.placed_boxes
            ));
        }
        worse
    }

    /// The floor a form's placed items stand under, in words, or `None` when the document
    /// is at or above it.
    ///
    /// The rate above is not a substitute for this: it is kept by any *proportional*
    /// fall — half the items in half the rows is the same ratio and half the panel — and
    /// at the limit a document whose form is answered no more at all is 0 rows for 0
    /// items, which `0 * 327 < 25 * 0` is false for, so the placed items can leave the
    /// panel entirely with nothing else in the file moving. So the count itself may only
    /// fall for a document that placed nothing to begin with, and a fall is named with the
    /// counts (`form item(s) placed: 327 → 160 item(s) in 12 row(s)`).
    ///
    /// It yields to nothing else, because no other count here can tell "answered as text
    /// instead" from "lost": that reading change has to be argued for by regenerating the
    /// baseline, like every other deliberate trade. This is also the one rule that applies
    /// **whatever the run was measured with** — it is about the reading, not about the
    /// settings it was made with — so it is the rule the window and the command line's
    /// `--compare` mode hold a document to even when the two measurements differ; the
    /// other rules join it when they match (see [`Report::hold_to_floor`]), and the corpus
    /// report holds all of them always.
    pub fn below_placed_floor(&self, was: &Numbers) -> Option<String> {
        (was.placed_items > 0 && self.placed_items < was.placed_items).then(|| {
            format!(
                "form item(s) placed: {} → {} item(s) in {} row(s)",
                was.placed_items, self.placed_items, self.placed
            )
        })
    }
}

/// Which of the ratchet's rules held a document, and the sentence saying why.
///
/// The memory beside the reference either speaks for this run's settings — the
/// two measurements match, and *every* rule applies — or it cannot: the settings
/// an OCR mode moves are the counts that measure how the two readings agree, so
/// only the floor under a form's placed items is left, which is about the reading
/// rather than about the settings. Which of the two happened is a fact about the
/// *run*, and a reader of the panel should not have to reconstruct it from two
/// settings screens: the holding records it, in the words the command line's
/// `--compare` gate prints beside the same document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    pub rules: HeldRules,
    pub why: String,
}

/// The two things "held to the memory" can mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HeldRules {
    /// Every rule of the ratchet: the run's settings match the ones the memory
    /// records, so a `differs` count that rose or an agreement that fell refuses
    /// a document as much as a form leaving the panel does.
    Ratchet,
    /// Only the floor under a form's placed items: the run was measured with
    /// settings the memory cannot vouch for, and the floor is the one rule that
    /// is about the reading rather than about how the two readings agree.
    Floor,
}

/// What a run measured, per document: the ratchet's memory.
///
/// Stored in the repository (`test-data/golden/compare-baseline.json`) so that a change to
/// it is visible in a diff beside the change that caused it: `camelCase`, like the report
/// the page is handed, and one entry per document under the document's own name (its file
/// stem). The corpus report writes it and checks every document against it; the window
/// reads it to hold a document to its numbers, and the command line's `--compare` mode
/// to fail a build on one that fell below them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    /// How it was measured, for a reader who opens the file.
    #[serde(default)]
    pub note: String,
    /// How each baseline was measured, recorded so the window can tell whether its
    /// current run's settings match those the memory remembers.
    #[serde(default)]
    pub measured: Measurement,
    pub documents: BTreeMap<String, Numbers>,
}

/// How the baseline was measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    /// What measured the baseline (e.g. the test that produced it).
    #[serde(default)]
    pub by: String,
    /// The corpus the baseline was measured over.
    #[serde(default)]
    pub corpus: String,
    /// The OCR mode used when measuring the baseline (`auto`, `always`, or `never`).
    #[serde(default)]
    pub ocr: String,
    /// Any other settings worth recording.
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
}

impl Measurement {
    pub fn from_ocr(ocr: &str) -> Self {
        Measurement {
            ocr: ocr.to_string(),
            ..Default::default()
        }
    }
}

impl Memory {
    /// The file's name, in the folder that holds the references it is about.
    pub const FILE: &str = "compare-baseline.json";

    /// The memory that belongs to `reference`, when there is one.
    ///
    /// It lives *beside the references*: a reference folder is `test-data/golden/docling/`
    /// and the memory is its parent's `compare-baseline.json`, so both the folder itself
    /// and the folder that holds it are looked in — and, for a single reference file, the
    /// folder it is in and the one above that. A file that is not there is `Ok(None)`: the
    /// compare mode works without a memory, and a document compared against a user's own
    /// reference has none. A file that *is* there and cannot be read or parsed is an
    /// `Err`, because a memory that silently did nothing would be a floor that silently
    /// did nothing.
    pub fn beside(reference: &Path) -> Result<Option<Memory>, String> {
        let folder = if reference.is_dir() {
            reference.to_path_buf()
        } else {
            match reference.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return Ok(None),
            }
        };
        let mut candidates = vec![folder.join(Self::FILE)];
        if let Some(parent) = folder.parent() {
            candidates.push(parent.join(Self::FILE));
        }
        for path in candidates {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
            };
            return serde_json::from_str(&text)
                .map(Some)
                .map_err(|error| format!("{} is not a compare baseline: {error}", path.display()));
        }
        Ok(None)
    }

    /// One document's remembered numbers, by the name it is remembered under.
    pub fn remember(&self, name: &str) -> Option<&Numbers> {
        self.documents.get(name)
    }
}

/// The sentence a held document carries about which rules held it and why.
///
/// Named the way a reader of the panel can check against the two settings screens:
/// the OCR mode each side was measured with is the setting that decides which of the
/// two cases this is, so it is named in the sentence either way it goes.
fn held_why(
    current: Option<&Measurement>,
    remembered: &Measurement,
    settings_match: bool,
) -> String {
    let ocr = |measurement: &Measurement| {
        if measurement.ocr.is_empty() {
            "an OCR mode that is not recorded".to_string()
        } else {
            format!("--ocr {}", measurement.ocr)
        }
    };
    if settings_match {
        if remembered.ocr.is_empty() && remembered.settings.is_empty() {
            "every rule of the ratchet applies: the memory records no settings to differ from"
                .to_string()
        } else {
            format!(
                "every rule of the ratchet applies: this run was measured the way the \
                 memory records ({})",
                ocr(remembered)
            )
        }
    } else {
        match current {
            Some(current) if current.ocr != remembered.ocr => format!(
                "only the floor under a form's placed items applies: this run was measured \
                 with {} and the memory with {}, and OCR moves the counts that measure how \
                 the two readings agree",
                ocr(current),
                ocr(remembered)
            ),
            _ => "only the floor under a form's placed items applies: this run's settings are \
                  not the ones the memory records"
                .to_string(),
        }
    }
}

/// Whether the current run's measurement settings match those the memory remembers.
fn measurements_match(current: &Measurement, remembered: &Measurement) -> bool {
    if !remembered.ocr.is_empty() && current.ocr != remembered.ocr {
        return false;
    }
    // If both specify other settings, require them to match.
    for (key, value) in &remembered.settings {
        if current.settings.get(key) != Some(value) {
            return false;
        }
    }
    true
}

/// Where the reference for a document is.
///
/// A reference is either the document's own reference file or a folder of
/// references named after the documents they belong to (which is what
/// `test-data/golden/docling/` is). The stem is tried first; the whole file name
/// is tried after it, because a run that had to disambiguate two inputs of the
/// same stem writes `report.txt.json` rather than `report.json`.
pub fn reference_for(reference: &Path, input: &Path) -> Option<PathBuf> {
    if reference.is_file() {
        return Some(reference.to_path_buf());
    }
    if !reference.is_dir() {
        return None;
    }
    let file_name = input.file_name()?.to_string_lossy().into_owned();
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    for candidate in [format!("{stem}.json"), format!("{file_name}.json")] {
        let path = reference.join(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Compare one finished document with the reference for it, wherever that is.
///
/// `output` is where the CLI wrote our document. A reference that cannot be
/// found for this document is reported rather than skipped: a *mode* that shows
/// nothing at all is indistinguishable from one that is broken, and the reader
/// needs to know the file was not there.
pub fn compare_document(input: &str, output: &Path, reference: &Path) -> Report {
    match reference_for(reference, Path::new(input)) {
        Some(found) => compare_files(input, output, &found),
        None => Report::uncomparable(
            input,
            reference,
            format!("no reference for this document ({})", reference.display()),
        ),
    }
}

/// Read our document and its reference and compare them.
///
/// Never fails: a missing, unreadable or malformed side is reported as the
/// reason the document could not be compared, because that is news for the
/// window rather than an error that should end the run.
pub fn compare_files(input: &str, ours: &Path, reference: &Path) -> Report {
    let ours_value = match read(ours) {
        Ok(value) => value,
        Err(problem) => return Report::uncomparable(input, reference, problem),
    };
    let theirs_value = match read(reference) {
        Ok(value) => value,
        Err(problem) => return Report::uncomparable(input, reference, problem),
    };
    let (rows, summary, placed_pages) = compare_documents(&ours_value, &theirs_value);
    Report {
        input: input.to_string(),
        reference: reference.display().to_string(),
        problem: None,
        summary,
        placed_pages,
        // A comparison knows nothing of any memory: it is the window that holds a document
        // to the floor one remembers (`Report::hold_to_floor`), so that comparing two files
        // stays a pure function of the two files.
        floor: None,
        held: None,
        rows,
    }
}

/// Parse one side of a comparison.
fn read(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not Docling JSON: {e}", path.display()))
}

/// Compare two parsed documents: the rows, what they add up to, and the placed items
/// grouped by the page they lie on.
pub fn compare_documents(ours: &Value, reference: &Value) -> (Vec<Row>, Summary, Vec<PlacedPage>) {
    let mut ours_blocks = blocks(ours);
    let mut theirs_blocks = blocks(reference);

    let mut summary = Summary::of(&[]);
    summary.truncated = ours_blocks.len().saturating_sub(MAX_BLOCKS)
        + theirs_blocks.len().saturating_sub(MAX_BLOCKS);
    if summary.truncated > 0 {
        summary.notes.push(format!(
            "compared the first {MAX_BLOCKS} blocks of {} ours / {} reference",
            ours_blocks.len(),
            theirs_blocks.len()
        ));
    }
    ours_blocks.truncate(MAX_BLOCKS);
    theirs_blocks.truncate(MAX_BLOCKS);

    // Content first, then position: the alignment pairs what it can without
    // crossing and consumes the runs of reference items our blocks hold where they
    // stand; what it leaves one-sided but *inside* a block of ours is folded onto
    // that block's row; and only the rows still standing alone are joined where
    // they stand in the same place on the same page (see the module note).
    let (rows, joined_by_position) =
        pair_by_place(fold_contained(align(&ours_blocks, &theirs_blocks)));
    // Last, so that it speaks only for what nothing else claimed: an item with no
    // letters in it is answered by the words, by the fold or by the boxes where any of
    // them can, and is *placed* only where none of them can.
    let mut rows = place_letterless(rows);
    // The per-page picture is taken from the rows *before* they are cut to `MAX_ROWS`:
    // a page's leader pattern is a fact about the document, not about how many rows the
    // panel had room to list, so a long form's page is whole even where its rows are not.
    let placed_pages = placed_pages(&rows);
    if rows.len() > MAX_ROWS {
        summary.truncated += rows.len() - MAX_ROWS;
        rows.truncate(MAX_ROWS);
    }
    for row in rows.iter_mut() {
        clip(row);
    }

    let mut counted = Summary::of(&rows);
    counted.paired_by_position = joined_by_position;
    counted.text_agreement = text_agreement(ours, reference);
    counted.pages_ours = ours["pages"].as_object().map_or(0, serde_json::Map::len);
    counted.pages_theirs = reference["pages"]
        .as_object()
        .map_or(0, serde_json::Map::len);
    counted.notes = page_notes(ours, reference);
    counted.truncated += summary.truncated;
    counted.notes.extend(summary.notes);
    (rows, counted, placed_pages)
}

/// The blocks of a document, in the order its own body tree puts them.
///
/// `body.children` and `furniture.children` are walked, resolving each `$ref`
/// into the array it names. A block no tree mentions is appended at the end:
/// comparing what the body forgot is still comparing it, and dropping it would
/// make the two sides disagree for a reason that is not in the documents.
fn blocks(document: &Value) -> Vec<Block> {
    let mut out = Vec::new();
    let mut seen = vec![
        false;
        document
            .get("texts")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
            + document
                .get("tables")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
            + document
                .get("pictures")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
    ];

    for group in ["body", "furniture"] {
        walk(document, &document[group]["children"], &mut out, &mut seen);
    }
    // Anything the tree did not reach, in array order.
    let mut index = 0usize;
    for (kind, key) in [
        (Kind::Text, "texts"),
        (Kind::Table, "tables"),
        (Kind::Picture, "pictures"),
    ] {
        for item in document[key].as_array().into_iter().flatten() {
            let reached = seen.get(index).copied().unwrap_or(false);
            index += 1;
            if !reached {
                out.push(block_of(kind, item, document));
            }
        }
    }
    out
}

/// Walk a children list in order, descending into nested groups where they
/// stand.
///
/// Docling's tree has two kinds of child: an item (`#/texts/3`, `#/tables/0`,
/// `#/pictures/1`) and a group (`#/groups/0`), which is a container with
/// children of its own — a picture area, a list, a run of formulas. A group is
/// not a thing to compare, but where it stands is where its children stand, so
/// its children are walked in its place rather than after every paragraph.
fn walk(document: &Value, children: &Value, out: &mut Vec<Block>, seen: &mut [bool]) {
    for child in children.as_array().into_iter().flatten() {
        if let Some(block) = block_of_ref(document, child, seen) {
            out.push(block);
            continue;
        }
        if let Some(nested) = group_children(document, child) {
            walk(document, nested, out, seen);
        }
    }
}

/// A group entry's own children, when the entry is a group.
fn group_children<'a>(document: &'a Value, child: &Value) -> Option<&'a Value> {
    let reference = child["$ref"].as_str()?;
    let index = reference
        .trim_start_matches("#/")
        .strip_prefix("groups/")?
        .parse::<usize>()
        .ok()?;
    document["groups"].as_array()?.get(index)?.get("children")
}

/// The block a body-tree entry points at, marking it as reached.
///
/// The same index space is shared by the three arrays so a `$ref` can be
/// remembered in one `Vec<bool>`, which is enough to tell "the tree placed this"
/// from "the tree never mentioned this".
fn block_of_ref(document: &Value, child: &Value, seen: &mut [bool]) -> Option<Block> {
    let reference = child["$ref"].as_str()?;
    let mut parts = reference.trim_start_matches("#/").split('/');
    let (array, index) = (parts.next()?, parts.next()?.parse::<usize>().ok()?);
    let kind = Kind::of_ref(reference)?;
    let offset = match kind {
        Kind::Text => 0,
        Kind::Table => document["texts"].as_array().map_or(0, Vec::len),
        Kind::Picture => {
            document["texts"].as_array().map_or(0, Vec::len)
                + document["tables"].as_array().map_or(0, Vec::len)
        }
    };
    if let Some(flag) = seen.get_mut(offset + index) {
        *flag = true;
    }
    let item = document[array].as_array()?.get(index)?;
    Some(block_of(kind, item, document))
}

/// One item of a document, as a comparable block.
fn block_of(kind: Kind, item: &Value, document: &Value) -> Block {
    let text = match kind {
        Kind::Text => item["text"].as_str().unwrap_or_default().to_string(),
        Kind::Table => table_text(item),
        Kind::Picture => picture_text(item),
    };
    Block {
        kind,
        label: item["label"]
            .as_str()
            .unwrap_or(match kind {
                Kind::Text => "text",
                Kind::Table => "table",
                Kind::Picture => "picture",
            })
            .to_string(),
        text,
        page: page_of(item),
        place: place_of(item, document),
    }
}

/// A table's cells, one row a line and one cell an interval.
///
/// The row is taken from the cell's own index when it states one, so a table is
/// laid out as the document laid it out rather than as the array happened to be
/// ordered.
fn table_text(table: &Value) -> String {
    let cells = table["data"]["table_cells"]
        .as_array()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if cells.is_empty() {
        return String::new();
    }
    let mut rows: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for (position, cell) in cells.iter().enumerate() {
        let index = cell["start_row_offset_idx"]
            .as_i64()
            .unwrap_or(position as i64);
        rows.entry(index)
            .or_default()
            .push(cell["text"].as_str().unwrap_or_default().to_string());
    }
    rows.into_values()
        .map(|cells| cells.join(" | "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A picture's identity: what it is, how big it is, and whether it has pixels.
fn picture_text(picture: &Value) -> String {
    let image = &picture["image"];
    let mime = image["mimetype"].as_str();
    let size = (
        image["size"]["width"].as_f64(),
        image["size"]["height"].as_f64(),
    );
    if mime.is_none() && size.0.is_none() {
        return "(no image)".to_string();
    }
    let mut text = mime.unwrap_or("image").to_string();
    if let (Some(width), Some(height)) = size {
        text.push_str(&format!(" {width:.0}x{height:.0}"));
    }
    if let Some(dpi) = image["dpi"].as_u64() {
        text.push_str(&format!(" @{dpi}dpi"));
    }
    text
}

/// The page an item sits on, from its first provenance entry.
fn page_of(item: &Value) -> Option<i64> {
    item["prov"].as_array()?.first()?.get("page_no")?.as_i64()
}

/// Where an item sits, when its producer published a box and that box can be
/// put in the shared frame.
///
/// A box measured from the bottom-left needs the page's height to be turned
/// around, so a document that does not publish its page sizes leaves that side
/// without geometry rather than compared in a frame of its own: two boxes read
/// in different frames overlap by accident, and a pairing made by accident is
/// worse than the two one-sided rows it would replace.
fn place_of(item: &Value, document: &Value) -> Option<Place> {
    let prov = item["prov"].as_array()?.first()?;
    let page = prov["page_no"].as_i64()?;
    let bbox = &prov["bbox"];
    let (l, t, r, b) = (
        bbox["l"].as_f64()?,
        bbox["t"].as_f64()?,
        bbox["r"].as_f64()?,
        bbox["b"].as_f64()?,
    );
    // Docling's own default is the bottom-left corner, so a box that does not
    // say which frame it is in is one of its.
    let (top, bottom) = match bbox["coord_origin"].as_str().unwrap_or("BOTTOMLEFT") {
        "TOPLEFT" => (t.min(b), t.max(b)),
        "BOTTOMLEFT" => {
            let height = document["pages"][page.to_string()]["size"]["height"].as_f64()?;
            // `t` and `b` are distances from the bottom, so the upper edge is
            // the larger of them and the nearer to the top of the page.
            (height - t.max(b), height - t.min(b))
        }
        // A frame this does not know. Guessing at one would compare
        // coordinates that mean different things on the two sides.
        _ => return None,
    };
    Some(Place {
        page,
        left: l.min(r),
        top,
        right: l.max(r),
        bottom,
        page_size: page_size(document, page),
    })
}

/// A page's size in points, as the document states it, or `None` where it does not.
fn page_size(document: &Value, page: i64) -> Option<(f64, f64)> {
    let size = &document["pages"][page.to_string()]["size"];
    let width = size["width"].as_f64()?;
    let height = size["height"].as_f64()?;
    (width > 0.0 && height > 0.0).then_some((width, height))
}

/// Pair the two sides up, one block of ours to at most one thing on the other
/// side, in order.
///
/// This is the usual sequence alignment: the best-scoring way to walk both lists
/// together, where pairing two blocks is worth what [`link_score`] makes of their
/// words *and* their boxes, a reference item wholly inside the block of ours is
/// worth [`CONTAINED_SCORE`], and leaving a block unpaired costs nothing. Because
/// a pair is only a move at all where [`link_score`] says the words make it a link
/// — and then only when it is the best use of both blocks — a block that was
/// reworded past recognition (or replaced by something else entirely) shows up as
/// one row a side rather than dragging the rest of the document out of line, and
/// the boxes decide which of two pairings the words cannot tell apart is taken.
fn align(ours: &[Block], theirs: &[Block]) -> Vec<Row> {
    let ours_keys: Vec<Key> = ours.iter().map(Key::of).collect();
    let theirs_keys: Vec<Key> = theirs.iter().map(Key::of).collect();
    let (n, m) = (ours.len(), theirs.len());

    // `score[i][j]`: the best total for ours[..i] against theirs[..j].
    // `paired[i][j]`: whether that best total ends with ours[i-1] beside
    // theirs[j-1], which is what the walk back follows. `grouped[i][j]`: how many
    // reference items it ends with beside ours[i-1] instead, all of them inside it.
    // (`u16` is enough: a run cannot be longer than the reference's own blocks, and
    // only [`MAX_BLOCKS`] of those are ever aligned.)
    let mut score = vec![vec![0.0f64; m + 1]; n + 1];
    let mut paired = vec![vec![false; m + 1]; n + 1];
    let mut grouped = vec![vec![0u16; m + 1]; n + 1];
    for i in 1..=n {
        // The runs of reference items this block of ours wholly holds, worked out
        // once: every cell of the row below asks how long a run ends at it.
        let runs = contained_runs(&ours[i - 1], &ours_keys[i - 1], theirs, &theirs_keys);
        for j in 1..=m {
            let link = link_score(
                &ours[i - 1],
                &ours_keys[i - 1],
                &theirs[j - 1],
                &theirs_keys[j - 1],
            );
            // A pair is a move at all only where the words make it a link; it is
            // worth what the boxes make of that link.
            let together = match link {
                Some(worth) => score[i - 1][j - 1] + worth,
                None => f64::NEG_INFINITY,
            };
            let run = runs[j];
            let inside = if run > 0 {
                score[i - 1][j - run] + CONTAINED_SCORE * run as f64
            } else {
                f64::NEG_INFINITY
            };
            let ours_alone = score[i - 1][j];
            let theirs_alone = score[i][j - 1];
            score[i][j] = together.max(inside).max(ours_alone).max(theirs_alone);
            // Two moves worth the same prefer the pairing by content: it is the
            // better evidence of the two readings agreeing, and a single contained
            // item is rendered as a containment either way.
            if together >= ours_alone && together >= theirs_alone && together >= inside {
                paired[i][j] = true;
            } else if run > 0
                && inside >= ours_alone
                && inside >= theirs_alone
                && inside >= together
            {
                grouped[i][j] = run as u16;
            }
        }
    }

    let mut rows = Vec::with_capacity(n + m);
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && paired[i][j] {
            rows.push(row_for(&ours[i - 1..i], &theirs[j - 1..j]));
            i -= 1;
            j -= 1;
        } else if i > 0 && j > 0 && grouped[i][j] > 0 {
            let run = grouped[i][j] as usize;
            rows.push(row_for(&ours[i - 1..i], &theirs[j - run..j]));
            i -= 1;
            j -= run;
        } else if i > 0 && (j == 0 || score[i - 1][j] > score[i][j - 1]) {
            rows.push(row_for(&ours[i - 1..i], &[]));
            i -= 1;
        } else {
            // A tie goes to the reference, which puts "only ours" above "only the
            // reference" once the rows are turned the right way round: a block we
            // have and Docling does not reads as a line removed, and a diff puts
            // that before the line added.
            rows.push(row_for(&[], &theirs[j - 1..j]));
            j -= 1;
        }
    }
    rows.reverse();
    rows
}

/// For one block of ours, how many reference items each position of the
/// reference ends with, all of them *by their words* inside it.
///
/// `runs[j]` counts the items `theirs[..j]` ends with — 0 where the item before
/// `j` is not inside ours, which also makes each run the longest one ending there.
/// A run has to be consecutive, so the alignment can consume it in one move without
/// stepping over an item neither reading pairs.
///
/// Words only, unlike [`contained_in`] itself: an item with no letters in it at all
/// is not a run of anything, and where it belongs is settled once, by where it lies,
/// in [`fold_contained`]. Letting the alignment consume one as well would hand it to
/// whichever block the walk happened to reach — and leave the block it lies most
/// inside, which is the line it was written along, reporting a line the reference has
/// and we do not.
fn contained_runs(
    ours: &Block,
    ours_key: &Key,
    theirs: &[Block],
    theirs_keys: &[Key],
) -> Vec<usize> {
    let mut runs = vec![0usize; theirs.len() + 1];
    for j in 1..=theirs.len() {
        if words_contained(ours, ours_key, &theirs[j - 1], &theirs_keys[j - 1]) {
            runs[j] = runs[j - 1] + 1;
        }
    }
    runs
}

/// One row, with the verdict its two sides warrant.
///
/// Our side is one block or none; the reference's is one block, a run of items all
/// inside one block of ours, or none.
fn row_for(ours: &[Block], theirs: &[Block]) -> Row {
    let kind = ours
        .first()
        .or_else(|| theirs.first())
        .map_or(Kind::Text, |block| block.kind);
    let (verdict, note) = match (ours.first(), theirs) {
        (Some(ours), []) => (
            Verdict::OursOnly,
            Some(format!("only ours: {}", describe(ours))),
        ),
        (None, []) => (Verdict::Same, None),
        (None, run) => (
            Verdict::TheirsOnly,
            Some(format!("only the reference: {}", describe(&run[0]))),
        ),
        (Some(ours), [theirs]) => one_to_one(ours, theirs),
        (Some(ours), run) => containment_row(ours, run),
    };
    Row {
        kind,
        verdict,
        ours: ours.to_vec(),
        theirs: theirs.to_vec(),
        note,
        // Set afterwards, and only on a row that stands for a place: everything the
        // panel shows about the two readings' words needs no box.
        placement: None,
    }
}

/// The verdict of a row with one block on each side.
fn one_to_one(ours: &Block, theirs: &Block) -> (Verdict, Option<String>) {
    // An equation is the one block whose text is never comparable: we linearise
    // Office Math to Unicode where Docling writes LaTeX. Both sides having one is
    // the fact being compared.
    if ours.label == "formula" && theirs.label == "formula" {
        return (Verdict::Notation, None);
    }
    let label_agrees = same_label(&ours.label, &theirs.label);
    // Where the block sits is part of what the row has to say. Two readings that
    // found the same words in different parts of the page have not found the same
    // block, and the row is a difference that names where each of them put it —
    // whether or not the *words* also differ.
    let moved = place_agreement(ours, theirs) == Some(false);
    if label_agrees && same_text(ours, theirs) && !moved {
        return (Verdict::Same, None);
    }
    let ours_key = Key::of(ours);
    let theirs_key = Key::of(theirs);
    let contained = contained_in(ours, &ours_key, theirs, &theirs_key);
    let mut reasons = Vec::new();
    if !label_agrees {
        reasons.push(format!("label: {} vs {}", ours.label, theirs.label));
    }
    if moved {
        reasons.push(place_reason(ours, theirs));
    }
    if contained {
        // The one item on this row is inside ours either by its words or by where it
        // sits, and the row says which (they cannot both hold: an item with no letters
        // is a run of nothing).
        let by_place = usize::from(letterless_in_place(ours, theirs, &theirs_key));
        reasons.push(containment_reason(1 - by_place, by_place));
    } else if !same_text(ours, theirs) {
        reasons.push("text differs".to_string());
    }
    // Only where the words sitting inside ours is the *whole* disagreement is the
    // row a pure containment: a label the readings disagree about is a finding of
    // its own, and must not be swallowed by the words happening to be ours already
    // — and neither is a box the two readings put somewhere else.
    let verdict = if contained && label_agrees && !moved {
        Verdict::Contained
    } else {
        Verdict::Differs
    };
    (verdict, Some(reasons.join("; ")))
}

/// The verdict of a row whose reference side is a run of items all wholly inside
/// the one block of ours: what the readings disagree about is where those items
/// end — unless the reference labels one of them something else, which is a finding
/// this row has to keep.
///
/// Where the run sits is not asked, unlike a one-to-one row: the *words* are what
/// make a run a run, and a block of ours whose paragraph runs over a page break
/// would otherwise have every item on its later half reported as standing somewhere
/// else.
fn containment_row(ours: &Block, theirs: &[Block]) -> (Verdict, Option<String>) {
    let labels = differing_labels(ours, theirs);
    let (by_words, by_place) = containment_counts(ours, theirs);
    // A run the alignment consumed is a run it found *inside* ours, item for item:
    // every one of them is in one of the two counts.
    debug_assert_eq!(by_words + by_place, theirs.len(), "{theirs:#?}");
    let mut reasons = vec![containment_reason(by_words, by_place)];
    if labels.is_empty() {
        return (Verdict::Contained, Some(reasons.join("; ")));
    }
    reasons.push(label_reason(ours, &labels));
    (Verdict::Differs, Some(reasons.join("; ")))
}

/// How a run of the reference's items lies inside one of our blocks: how many of
/// them by their words, and how many by their place alone — an item with no letters
/// in it, which has no words to be inside anything (see [`contained_in`]).
fn containment_counts(ours: &Block, theirs: &[Block]) -> (usize, usize) {
    let ours_key = Key::of(ours);
    theirs
        .iter()
        .fold((0usize, 0usize), |(by_words, by_place), block| {
            let key = Key::of(block);
            if run_of_words(&ours_key, &key) {
                (by_words + 1, by_place)
            } else if letterless_in_place(ours, block, &key) {
                (by_words, by_place + 1)
            } else {
                (by_words, by_place)
            }
        })
}

/// The labels a run of the reference's items carries that ours does not agree
/// with, once each and in a stable order.
fn differing_labels(ours: &Block, theirs: &[Block]) -> Vec<String> {
    let mut labels: Vec<String> = theirs
        .iter()
        .map(|block| block.label.clone())
        .filter(|label| !same_label(label, &ours.label))
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// How a row names a label the two readings disagree about.
fn label_reason(ours: &Block, labels: &[String]) -> String {
    format!("label: {} vs {}", ours.label, labels.join("/"))
}

/// What a row says about the reference's side lying inside ours: the items whose
/// words are ours, and the ones with no letters at all that were taken by where they
/// sit. The two are named apart because only one of them is what the words say: an
/// item with nothing in it is on the row because it lies along a line of ours, and a
/// reader has to be able to tell that from a run of our words.
fn containment_reason(by_words: usize, by_place: usize) -> String {
    let mut reasons = Vec::new();
    match by_words {
        0 => {}
        1 => reasons.push("the reference's item is wholly inside ours".to_string()),
        count => reasons.push(format!(
            "{count} of the reference's items are wholly inside ours"
        )),
    }
    match by_place {
        0 => {}
        1 => reasons.push("an item with no letters in it lies in the same place".to_string()),
        count => reasons.push(format!(
            "{count} items with no letters in them lie in the same place"
        )),
    }
    reasons.join("; ")
}

/// A one-line description of a block, for a one-sided row's note.
fn describe(block: &Block) -> String {
    let text = clip_text(&block.text);
    if text.is_empty() {
        format!("{} with no text", block.label)
    } else {
        format!("{} \"{text}\"", block.label)
    }
}

/// Move the reference's leftover items onto the block of ours that holds them.
///
/// The alignment consumes one of our blocks together with a run of reference items
/// it holds only while those items stand *together*: where the reference puts an
/// item between them that we do not hold, the run breaks and the halves are left
/// standing alone. Their words are still ours, though — a form's field label read
/// in the middle of a line we kept whole is exactly this — so every remaining
/// one-sided item whose letters are a run of words inside a block of ours is moved
/// onto that block's row, which says how many it took. An item with no letters in it
/// at all is moved the same way where its box lies inside the block (a rule along a
/// line we read is part of that line, see [`contained_in`]), and nowhere else. Content
/// decides this wherever there is content to decide it, which is why it runs before
/// the box joining: an item's letters lying inside our block is evidence a shared box
/// never is.
///
/// What the row already disagreed about it keeps: a wording difference the
/// alignment found must not disappear behind a containment. So a row whose only
/// complaint was where an item ended becomes a containment, and one that disagreed
/// about the words stays a difference and gains the containment in its note.
fn fold_contained(rows: Vec<Row>) -> Vec<Row> {
    // The blocks that can take an item, with the key each containment test needs —
    // worked out once, because every leftover asks about every block — and whether
    // the row already holds words of the reference inside it, which is what tells a
    // line of ours from a block that merely stands in the same place.
    let holders: Vec<(usize, Block, Key, bool)> = rows
        .iter()
        .enumerate()
        .filter_map(|(position, row)| {
            let [block] = row.ours.as_slice() else {
                return None;
            };
            let key = Key::of(block);
            let holds_words = row
                .theirs
                .iter()
                .any(|item| words_contained(block, &key, item, &Key::of(item)));
            Some((position, block.clone(), key, holds_words))
        })
        .collect();
    if holders.is_empty() {
        return rows;
    }

    // Each leftover, and the block of ours that holds it. An item with words in it is
    // held by the words, and the *nearest* such block takes it, so a label this
    // document repeats is taken where it stands rather than crossed the document for.
    //
    // An item with no letters in it at all is a different matter twice over. It is
    // taken only by a block whose row already holds *words* of the reference: a leader
    // of a line we read stands among the fragments of that line and is part of it,
    // while a rule standing alone in the area of one of our blocks is the reference's
    // own reading of it — and if the two blocks are simply in the same place, the joining by
    // position pairs them and says that is what paired them, which is the honest
    // answer for a pair whose *content* is nothing at all. And where there is a
    // choice, the block it lies *most* inside takes it, not the nearest one: lying
    // inside is a judgement of degree, and a leader beside the words of one line and
    // the edge of the next belongs to the line it lies mostly in.
    let mut taken = vec![false; rows.len()];
    let mut absorbed: Vec<Vec<(usize, Block)>> = vec![Vec::new(); rows.len()];
    for (position, row) in rows.iter().enumerate() {
        let ([], [item]) = (row.ours.as_slice(), row.theirs.as_slice()) else {
            continue;
        };
        let key = Key::of(item);
        let letterless = key.anchor.is_empty();
        let mut holder: Option<(usize, &Block)> = None;
        for (at, block, block_key, holds_words) in &holders {
            if letterless && !holds_words {
                continue;
            }
            if !contained_in(block, block_key, item, &key) {
                continue;
            }
            let better = match holder {
                None => true,
                Some((held_at, held)) if letterless => {
                    let (here, there) = (place_overlap(block, item), place_overlap(held, item));
                    here > there
                        || (here == there && at.abs_diff(position) < held_at.abs_diff(position))
                }
                Some((held_at, _)) => at.abs_diff(position) < held_at.abs_diff(position),
            };
            if better {
                holder = Some((*at, block));
            }
        }
        if let Some((holder, _)) = holder {
            taken[position] = true;
            absorbed[holder].push((position, item.clone()));
        }
    }
    if !taken.iter().any(|taken| *taken) {
        return rows;
    }

    let mut out = Vec::with_capacity(rows.len());
    for (position, row) in rows.into_iter().enumerate() {
        if taken[position] {
            continue;
        }
        let mut row = row;
        if !absorbed[position].is_empty() {
            // The row's items stand in the reference's order: the ones the fold found
            // *before* the item this row already holds go in front of it and the ones
            // after it go behind. A form line whose label the alignment paired while
            // the field numbers and leaders around it were left over is one row of the
            // line's items in the order the reference wrote them, not the label first.
            absorbed[position].sort_by_key(|(position, _)| *position);
            let mut before = Vec::new();
            let mut after = Vec::new();
            for (at, block) in &absorbed[position] {
                if *at < position {
                    before.push(block.clone());
                } else {
                    after.push(block.clone());
                }
            }
            absorb(&mut row, &before, &after);
        }
        out.push(row);
    }
    out
}

/// Give a row the reference's items that lie inside its block of ours: the ones
/// standing before the item it already holds, and the ones standing after.
///
/// The row is never silent about it: the note is rebuilt from what the row still
/// says and the containment. What the row said is kept where it was about words the
/// two readings really disagree on — a wording difference the alignment found must
/// not disappear behind the fold — and dropped where it was only about an item's
/// edges, because the row now says that from scratch. A label disagreement is named
/// once either way.
fn absorb(row: &mut Row, before: &[Block], after: &[Block]) {
    let ours = row.ours[0].clone();
    let ours_key = Key::of(&ours);
    let was = row.verdict;
    // The item the alignment paired, when it is not one of ours to begin with: the
    // reasons for that pairing are this row's own and stay.
    let paired_inside = row
        .theirs
        .first()
        .is_some_and(|block| contained_in(&ours, &ours_key, block, &Key::of(block)));
    let keeps_reasons = was == Verdict::Differs && !paired_inside;
    let taken: Vec<Block> = before.iter().chain(after.iter()).cloned().collect();

    let mut items: Vec<Block> = Vec::with_capacity(before.len() + row.theirs.len() + after.len());
    items.extend_from_slice(before);
    items.append(&mut row.theirs);
    items.extend_from_slice(after);
    row.theirs = items;
    let (by_words, by_place) = containment_counts(&ours, &row.theirs);
    // Where the pairing's own reasons are kept, the labels they already name are not
    // named a second time: only the ones the fold brought are.
    let labels = if keeps_reasons {
        differing_labels(&ours, &taken)
    } else {
        differing_labels(&ours, &row.theirs)
    };
    let mut reasons: Vec<String> = if keeps_reasons {
        row.note.clone().into_iter().collect()
    } else {
        Vec::new()
    };
    reasons.push(containment_reason(by_words, by_place));
    if !labels.is_empty() {
        reasons.push(label_reason(&ours, &labels));
    }
    row.verdict = match was {
        // Nothing but where the reference's items ended was wrong — or nothing was
        // paired at all — and that is now the whole row.
        Verdict::Same | Verdict::Contained | Verdict::OursOnly if labels.is_empty() => {
            Verdict::Contained
        }
        _ => Verdict::Differs,
    };
    row.note = Some(reasons.join("; "));
}

/// Join the one-sided rows that are the same thing in the same place.
///
/// The alignment above pairs by content and in order, which is what keeps a
/// reworded block one row instead of a shift of everything after it — but it
/// also means two readings that put the same picture or table at the same place
/// in a *different* order cannot pair them all: pairing both would make the pairs
/// cross. The boxes decide which of those crossing pairs the alignment takes (see
/// [`link_score`]), and whichever of them is left standing — one row a side — is
/// what this step joins. So are the blocks the words link nowhere at all: a
/// picture with no image against the drawing the other reading cut out, which are
/// the same box and nothing else.
///
/// The pairing is a heuristic, so it is never silent: a joined row says on
/// itself which page and how much overlap joined it, and the count is reported
/// in the summary. Only leftovers are considered — a row content already paired
/// is not touched — and a joined row takes the place of whichever of the two it
/// replaces came first, so nothing else moves.
fn pair_by_place(rows: Vec<Row>) -> (Vec<Row>, usize) {
    // The rows that stand alone, by the side that has them.
    let mut ours: Vec<(usize, &Block)> = Vec::new();
    let mut theirs: Vec<(usize, &Block)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        match (row.ours.as_slice(), row.theirs.as_slice()) {
            ([block], []) => ours.push((index, block)),
            ([], [block]) => theirs.push((index, block)),
            _ => {}
        }
    }
    if ours.is_empty() || theirs.is_empty() {
        return (rows, 0);
    }

    // Each alone row names the counterpart it would rather be joined to, and
    // the offers are taken best first with every row used at most once. Asking
    // both sides keeps the work linear in the rows: a page of a document has
    // hundreds of blocks, and pairing them all against each other would be the
    // quadratic blow-up the rest of this module is careful to avoid.
    let mut offers: Vec<(f64, usize, usize, i64)> = Vec::new();
    for (index, block) in &ours {
        if let Some((score, other, page)) = best_place(block, &theirs) {
            offers.push((score, *index, other, page));
        }
    }
    for (index, block) in &theirs {
        if let Some((score, other, page)) = best_place(block, &ours) {
            offers.push((score, other, *index, page));
        }
    }
    // Happening to overlap more joins first; the index in the offer keeps a tie
    // from depending on the order the page happened to be walked in.
    offers.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    let mut joined: Vec<(usize, Row)> = Vec::new();
    let mut ours_taken = vec![false; rows.len()];
    let mut theirs_taken = vec![false; rows.len()];
    for (score, ours_at, theirs_at, page) in offers {
        if ours_taken[ours_at] || theirs_taken[theirs_at] {
            continue;
        }
        let ours_block = rows[ours_at]
            .ours
            .first()
            .filter(|block| block.place.is_some());
        let theirs_block = rows[theirs_at]
            .theirs
            .first()
            .filter(|block| block.place.is_some());
        let (Some(ours_block), Some(theirs_block)) = (ours_block, theirs_block) else {
            continue;
        };
        ours_taken[ours_at] = true;
        theirs_taken[theirs_at] = true;
        let mut row = row_for(
            std::slice::from_ref(ours_block),
            std::slice::from_ref(theirs_block),
        );
        note_place(&mut row, page, score);
        joined.push((ours_at.min(theirs_at), row));
    }
    if joined.is_empty() {
        return (rows, 0);
    }

    let count = joined.len();
    joined.sort_by_key(|(position, _)| *position);
    let mut slots: Vec<Option<Row>> = rows.into_iter().map(Some).collect();
    for (index, slot) in slots.iter_mut().enumerate() {
        if ours_taken[index] || theirs_taken[index] {
            *slot = None;
        }
    }
    let mut joined = joined.into_iter().peekable();
    let mut out = Vec::with_capacity(slots.len());
    for (index, slot) in slots.into_iter().enumerate() {
        while matches!(joined.peek(), Some((position, _)) if *position == index) {
            if let Some((_, row)) = joined.next() {
                out.push(row);
            }
        }
        if let Some(row) = slot {
            out.push(row);
        }
    }
    (out, count)
}

/// Place the reference's items that carry nothing to compare and that nothing of ours
/// lies along, so that a form's dotted leaders are answered instead of listed.
///
/// A dotted leader or a rule Docling wrote as an item of its own is answered by the fold
/// where a block of ours lies along its line (see [`fold_contained`]) and by the joining
/// by position where a block of ours stands alone in its place. What is left after both
/// is an item with no letters in it at all that no block of ours holds and none stands
/// under: a line of a form our reading produced no block for, or a rule we did not read
/// as anything. Its words cannot be matched — there are none — so a row standing as "text
/// the reference has and we do not" would claim to know what this comparison does not:
/// that we missed words. It is therefore **placed**: a row that says what Docling wrote
/// and where it lies, counted in [`Summary::placed`] rather than among the one-sided rows.
///
/// Only what nothing else claimed is looked at, and only an item that published a box:
/// placing an item takes a place to name, and a document without geometry — every flow
/// format — is compared exactly as before.
fn place_letterless(mut rows: Vec<Row>) -> Vec<Row> {
    for row in rows.iter_mut() {
        let ([], [item]) = (row.ours.as_slice(), row.theirs.as_slice()) else {
            continue;
        };
        if row.verdict != Verdict::TheirsOnly {
            continue;
        }
        let Some(place) = placed_place(item) else {
            continue;
        };
        row.verdict = Verdict::Placed;
        row.note = Some(placed_reason(place, 1));
        // Drawn as well as named, where the page's own size was published: a placement
        // is about *where* something is, and a box on an outline says that at a glance
        // where two pairs of coordinates have to be read.
        row.placement = placement_of(place);
    }
    collapse_lines(rows)
}

/// How much two boxes may be apart vertically and still be one line.
///
/// A point, which is not a tolerance so much as a rounding: the dots of a form's leader
/// and the rule drawn under them are published as boxes whose edges *meet* rather than
/// overlap, and they are one line of the form. It is a fifth of the shortest line the
/// corpus sets (about 5 pt), so no two lines of text can be joined by it.
const LINE_SLACK: f64 = 1.0;

/// Place the items of one line of a form as one row.
///
/// Adjacency is the *reference's*, not the panel's: what the two items have to be is
/// next to each other in Docling's own order, and a row holding nothing of the
/// reference's — a block of ours read on that line — does not come between them, however
/// the alignment happened to order the rows. A row that *does* hold reference items does
/// come between them, and the two leaders are then placed apart from it. Each group keeps
/// the first row's place in the list, holds every item it stands for, and says on its note
/// how many they are and the box that covers them all.
fn collapse_lines(rows: Vec<Row>) -> Vec<Row> {
    // Whether something of the reference's own stands between the last placed row and the
    // next one. A block of ours holds none of it, so it is not a separation.
    let mut separated = false;
    let mut out: Vec<Row> = Vec::with_capacity(rows.len());
    for row in rows {
        let placed = row.verdict == Verdict::Placed;
        if placed && !separated {
            if let Some(last) = out.last_mut() {
                if last.verdict == Verdict::Placed {
                    let runs_on = match (run_place(last), run_place(&row)) {
                        (Some(one), Some(other)) => on_one_line(one, other),
                        _ => false,
                    };
                    if runs_on {
                        last.theirs.extend(row.theirs);
                        let whole = run_place(last).expect("a placed run has a place");
                        last.note = Some(placed_reason(whole, last.theirs.len()));
                        last.placement = placement_of(whole);
                        continue;
                    }
                }
            }
        }
        separated = !placed && !row.theirs.is_empty();
        out.push(row);
    }
    out
}

/// Whether two places stand on one line of the same page: their vertical ranges meet.
fn on_one_line(one: Place, other: Place) -> bool {
    one.page == other.page
        && one.top <= other.bottom + LINE_SLACK
        && other.top <= one.bottom + LINE_SLACK
}

/// Where a placed row's items lie, taken together: the page they stand on, and the box
/// that covers every one of them.
///
/// `None` unless every item published a place on one page — which is what a row this is
/// asked about always has, and what leaves an item with no box (a flow format) off a row
/// rather than in it.
fn run_place(row: &Row) -> Option<Place> {
    let mut whole: Option<Place> = None;
    for item in &row.theirs {
        let place = item.place?;
        whole = Some(match whole {
            None => place,
            Some(whole) if whole.page == place.page => Place {
                page: whole.page,
                left: whole.left.min(place.left),
                top: whole.top.min(place.top),
                right: whole.right.max(place.right),
                bottom: whole.bottom.max(place.bottom),
                page_size: whole.page_size.or(place.page_size),
            },
            // Two pages in one row: nothing merges them, and the first is what the row
            // already stood for, so it stands as it was.
            Some(whole) => whole,
        });
    }
    whole
}

/// The place of an item this mode can only *place*, or `None` when the item is not one:
/// it has to be ordinary text with no letters in it at all, and it has to publish a box.
///
/// A table's or a picture's "text" is a rendering this project invented rather than words
/// a producer wrote, an equation's is a notation rather than a reading, and an item with
/// no box stands nowhere that can be named — for all of those the row stands as it did.
fn placed_place(block: &Block) -> Option<Place> {
    if block.kind != Kind::Text || block.label == "formula" {
        return None;
    }
    if !anchor_and_ends(&block.text).0.is_empty() {
        return None;
    }
    block.place
}

/// What a placed row says: how many items it stands for, and where they lie in the page's
/// own points.
///
/// The count is on the row's note rather than in a tally because it is a fact about *this*
/// line of *this* form, and the reader of one row wants to know what they are looking at.
fn placed_reason(place: Place, items: usize) -> String {
    if items == 1 {
        return format!(
            "a dotted leader or a rule at page {}, box {:.0},{:.0}–{:.0},{:.0}: an item with no \
             letters in it at all, so there are no words to compare",
            place.page, place.left, place.top, place.right, place.bottom
        );
    }
    format!(
        "{items} items with no letters in them along one line of page {}, box \
         {:.0},{:.0}–{:.0},{:.0}: the dots of a leader and the rule under them, placed as the \
         one line they stand on rather than one row each",
        place.page, place.left, place.top, place.right, place.bottom
    )
}

/// A place as the panel draws it — the box as fractions of its page, and the shape the
/// page is — or `None` where the document never stated the page's size, in which case
/// there is no outline to draw the box on and the row's note is all there is.
fn placement_of(place: Place) -> Option<Placement> {
    let (width, height) = place.page_size?;
    let (x, y, w, h) = fractions(place)?;
    Some(Placement {
        page: place.page,
        width,
        height,
        x,
        y,
        w,
        h,
    })
}

/// A place as `(x, y, w, h)` in fractions of its page, or `None` when the page's size was
/// never stated.
///
/// The fractions are kept inside the page: a box the producer measured a little past an
/// edge is drawn at the edge rather than off it. Shared by the row's own `placement` and
/// by the per-page drawing, so a box is one thing read two ways rather than two sums that
/// could drift apart.
fn fractions(place: Place) -> Option<(f64, f64, f64, f64)> {
    let (width, height) = place.page_size?;
    let inside = |value: f64| value.clamp(0.0, 1.0);
    let x = inside(place.left / width);
    let y = inside(place.top / height);
    Some((
        x,
        y,
        (inside(place.right / width) - x).max(0.0),
        (inside(place.bottom / height) - y).max(0.0),
    ))
}

/// Group the document's placed items by the page they lie on, so the panel can draw a
/// page's whole leader pattern in one picture rather than one thumbnail per row.
///
/// Every *item* is a box of its own, taken from the block's own place rather than from the
/// row's merged extent — the picture is of what the reference wrote, dot run by dot run,
/// not of the lines the rows were grouped into. Pages come in ascending order, and the
/// items on one page stay in the reference's own order. An item whose page stated no size
/// is left out: there is no outline to draw it on, and a page with nothing drawable is not
/// listed at all rather than listed empty.
fn placed_pages(rows: &[Row]) -> Vec<PlacedPage> {
    let mut pages: BTreeMap<i64, PlacedPage> = BTreeMap::new();
    for row in rows {
        if row.verdict != Verdict::Placed {
            continue;
        }
        for item in &row.theirs {
            let Some(place) = item.place else {
                continue;
            };
            let Some((width, height)) = place.page_size else {
                continue;
            };
            let Some((x, y, w, h)) = fractions(place) else {
                continue;
            };
            let page = pages.entry(place.page).or_insert_with(|| PlacedPage {
                page: place.page,
                width,
                height,
                boxes: Vec::new(),
            });
            page.boxes.push(PlacedBox { x, y, w, h });
        }
    }
    pages.into_values().collect()
}

/// The counterpart a lone block would rather be joined to: the one in the same
/// place it shares the most box with, when one shares enough.
///
/// Returns the overlap, the candidate's position in its own list, and the page
/// the two agree on.
fn best_place(block: &Block, candidates: &[(usize, &Block)]) -> Option<(f64, usize, i64)> {
    let place = block.place?;
    let mut best: Option<(f64, usize, i64)> = None;
    for (index, other) in candidates {
        let Some(other_place) = other.place else {
            continue;
        };
        if !same_thing(block, other) {
            continue;
        }
        let score = overlap(place, other_place);
        // A tie keeps the first candidate, which is the one the document's own
        // order puts first.
        if score >= PLACE_OVERLAP && best.map_or(true, |(best, _, _)| score > best) {
            best = Some((score, *index, place.page));
        }
    }
    best
}

/// Whether two blocks are the same kind of thing to compare.
///
/// The kind has to agree, and so does whether the block is an equation: an
/// equation we found and a paragraph the reference read in the same place are
/// not one thing read twice — that one reading found an equation at all is
/// itself the finding, and joining those two rows would hide it. (Two equations
/// in the same place are paired by content instead, as a notation row.)
fn same_thing(one: &Block, other: &Block) -> bool {
    one.kind == other.kind && (one.label == "formula") == (other.label == "formula")
}

/// The share of the smaller of two boxes that lies inside the other.
///
/// 1.0 when one box contains the other, and 0.0 when they are on different
/// pages, do not touch, or either is flat or empty. The *smaller* box is the
/// denominator on purpose: the two sides describe the same thing at slightly
/// different sizes — one may take in a table's frame, the other only the cells —
/// and neither should be punished for the difference at its edge.
fn overlap(one: Place, other: Place) -> f64 {
    if one.page != other.page {
        return 0.0;
    }
    let smaller = area(one).min(area(other));
    if smaller <= 0.0 {
        return 0.0;
    }
    let width = (one.right.min(other.right) - one.left.max(other.left)).max(0.0);
    let height = (one.bottom.min(other.bottom) - one.top.max(other.top)).max(0.0);
    (width * height / smaller).min(1.0)
}

/// How much of its page a place covers.
fn area(place: Place) -> f64 {
    (place.right - place.left).max(0.0) * (place.bottom - place.top).max(0.0)
}

/// Whether the two producers put two blocks in the same place: the same page, and
/// enough of the smaller box inside the other ([`PLACE_OVERLAP`]).
///
/// `None` when either side published no box that can be compared — a flow format,
/// or a bottom-left box on a page whose height the document never states. Position
/// then has nothing to say, and the words decide alone.
fn place_agreement(ours: &Block, theirs: &Block) -> Option<bool> {
    let (ours, theirs) = (ours.place?, theirs.place?);
    Some(ours.page == theirs.page && overlap(ours, theirs) >= PLACE_OVERLAP)
}

/// How much of the smaller of two blocks' boxes the two share, and 0 where either
/// published no box to compare — the answer [`overlap`] gives two blocks that never
/// met, which is what "no box" has to mean to a caller asking which of two blocks a
/// block lies more inside.
fn place_overlap(one: &Block, other: &Block) -> f64 {
    match (one.place, other.place) {
        (Some(one), Some(other)) => overlap(one, other),
        _ => 0.0,
    }
}

/// Say on a row itself why content left it alone and position joined it.
///
/// This is the only thing in the panel that content did not decide, so it is
/// never silent: a reader has to be able to tell "the two readings put this in
/// the same place" from "the two readings read this the same way".
fn note_place(row: &mut Row, page: i64, overlap: f64) {
    let reason = format!(
        "paired by position: page {page}, {:.0}% of the smaller box shared",
        overlap * 100.0
    );
    row.note = Some(match row.note.take() {
        Some(existing) => format!("{existing}; {reason}"),
        None => reason,
    });
}

/// What a row says when the words are the same but the boxes are not: the two
/// producers put the block in different parts of the page, and both are named.
///
/// The mirror of [`note_place`]: that one says the boxes *are* what paired two
/// rows, this one says they are what a row content paired disagrees about.
fn place_reason(one: &Block, other: &Block) -> String {
    let (Some(one), Some(other)) = (one.place, other.place) else {
        return "not in the same place".to_string();
    };
    if one.page != other.page {
        format!(
            "not in the same place: page {} vs page {}",
            one.page, other.page
        )
    } else {
        format!(
            "not in the same place: page {}, {:.0}% of the smaller box shared",
            one.page,
            overlap(one, other) * 100.0
        )
    }
}

/// Whether two labels name the same thing to a reader.
fn same_label(one: &str, other: &str) -> bool {
    one == other || (BODY_LABELS.contains(&one) && BODY_LABELS.contains(&other))
}

/// Whether two blocks carry the same words, case and punctuation included.
///
/// Whitespace is collapsed first: a producer that wraps a line differently has
/// not read different words, and a diff that says so on every row is noise.
fn same_text(one: &Block, other: &Block) -> bool {
    normalised(&one.text) == normalised(&other.text)
}

/// Text with its whitespace collapsed, for comparison.
fn normalised(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The comparable parts of a block, worked out once and reused for every
/// candidate pair — the alignment asks for millions of similarities, and no pair
/// may cost an allocation.
struct Key {
    /// Whitespace-collapsed text, for the equality test.
    text: String,
    /// Lower-cased alphanumerics: what the containment test searches, and the
    /// source of the bigrams whose overlap says how similar two blocks are.
    anchor: String,
    /// Where each word of the anchor ends: what tells a word from a piece of one.
    word_ends: Vec<usize>,
    /// Bigrams of the anchor, sorted and deduplicated.
    bigrams: Vec<u32>,
}

impl Key {
    fn of(block: &Block) -> Key {
        let (anchor, word_ends) = anchor_and_ends(&block.text);
        let mut bigrams: Vec<u32> = match anchor.chars().count() {
            0 => Vec::new(),
            1 => vec![anchor.chars().next().unwrap() as u32],
            _ => anchor
                .chars()
                .collect::<Vec<char>>()
                .windows(2)
                .map(|pair| (pair[0] as u32) << 16 | pair[1] as u32)
                .collect(),
        };
        bigrams.sort_unstable();
        bigrams.dedup();
        Key {
            text: normalised(&block.text),
            anchor,
            word_ends,
            bigrams,
        }
    }
}

/// A text's alphanumerics, lower-cased, and where each word of them ends.
///
/// What is compared regardless of how a producer spaced, punctuated or hyphenated
/// its text — and the ends are what keep a containment honest: the reference's text
/// has to begin and end on one of them, so a piece of one of our words is not
/// "inside" it while text our own reading spaced out letter by letter still is.
fn anchor_and_ends(text: &str) -> (String, Vec<usize>) {
    let mut anchor = String::with_capacity(text.len());
    let mut ends = Vec::new();
    let mut in_word = false;
    for character in text.chars() {
        if character.is_alphanumeric() {
            anchor.extend(character.to_lowercase());
            in_word = true;
        } else if in_word {
            ends.push(anchor.len());
            in_word = false;
        }
    }
    if in_word {
        ends.push(anchor.len());
    }
    (anchor, ends)
}

/// Whether the reference's block lies wholly inside ours: either the reference's
/// letters a run of our words, or — for an item with no letters in it at all — its box
/// inside ours.
///
/// The second arm is the one place where *where* a block sits decides a containment,
/// and it is only for what has nothing else to offer: an item with no letters and no
/// digits in it carries nothing to be a run of our words, and Docling writes one for
/// every dotted leader and every rule it meets — one item per line of a form like
/// `irs_f1040`. A rule drawn along the line whose words are ours is part of that
/// line, and reporting it as something the reference has and we do not says
/// "missing" about punctuation we hold. It needs a box from *both* producers to be
/// asked at all (`place_agreement`), which the PDFs publish and the flow formats do
/// not, so a document without geometry is compared exactly as before — and an empty
/// item that sits somewhere else, or beside a picture rather than a line of text, is
/// still the reference's own reading. Whether it stands among words of ours is asked
/// as well, one row at a time: see [`fold_contained`].
fn contained_in(ours: &Block, ours_key: &Key, theirs: &Block, theirs_key: &Key) -> bool {
    words_contained(ours, ours_key, theirs, theirs_key)
        || (comparable(ours, theirs) && letterless_in_place(ours, theirs, theirs_key))
}

/// Whether the reference's item lies inside ours by its *words*: its letters, in
/// order, a run of our words.
///
/// [`contained_in`] is this and the one other way in — where the item sits, for an
/// item with no letters to be a run of — and the alignment uses only this one (see
/// [`contained_runs`]).
fn words_contained(ours: &Block, ours_key: &Key, theirs: &Block, theirs_key: &Key) -> bool {
    comparable(ours, theirs) && run_of_words(ours_key, theirs_key)
}

/// Whether the two blocks are the kind of thing one can lie inside the other: text on
/// both sides, an equation on neither.
///
/// Tables and pictures are left out: their "text" is a rendering this project
/// invented (cells joined, a media type and a size) rather than words a producer
/// wrote, and one being a piece of another would mean nothing.
fn comparable(ours: &Block, theirs: &Block) -> bool {
    ours.kind == Kind::Text
        && theirs.kind == Kind::Text
        && ours.label != "formula"
        && theirs.label != "formula"
}

/// Whether an item the reference wrote with no letters in it at all lies inside one
/// of our blocks by where it sits, which is all there is to go on.
fn letterless_in_place(ours: &Block, theirs: &Block, theirs_key: &Key) -> bool {
    theirs_key.anchor.is_empty() && place_agreement(ours, theirs) == Some(true)
}

/// Whether the reference's text is a run of our words: its letters, in order, are
/// one word of ours after another.
///
/// The boundaries are what keep this honest. The reference's text has to begin
/// where one of our words begins and end where one of them ends, so a piece of a
/// word is not inside ours — the reference's `3` is not a piece of our `1937` — and
/// the order is kept, so the words of `the cat sat on the mat` do not make up
/// `the mat sat on the cat`. Text our own reading spaced out letter by letter, as a
/// form's field labels come out, still matches: each letter is a word of ours. Text
/// with no letters in it at all matches nothing here — it is not a run of anything,
/// and [`letterless_in_place`] is what takes those, by where they sit.
fn run_of_words(ours: &Key, theirs: &Key) -> bool {
    let text = theirs.anchor.as_str();
    if text.is_empty() || text.len() >= ours.anchor.len() || !ours.anchor.contains(text) {
        return false;
    }
    // Where the words of ours begin: the start of its text, then each word's end
    // (which is where the word after it begins).
    let mut begins = std::iter::once(0usize).chain(
        ours.word_ends[..ours.word_ends.len().saturating_sub(1)]
            .iter()
            .copied(),
    );
    begins.any(|start| {
        ours.anchor[start..].starts_with(text)
            && ours.word_ends.binary_search(&(start + text.len())).is_ok()
    })
}

/// How similar two blocks are, from 0 (nothing in common) to 1 (the same words).
fn similarity(ours: &Block, ours_key: &Key, theirs: &Block, theirs_key: &Key) -> f64 {
    if ours.kind != theirs.kind {
        return 0.0;
    }
    if ours.label == "formula" && theirs.label == "formula" {
        return EQUATION_SIMILARITY;
    }
    if ours_key.text == theirs_key.text {
        return 1.0;
    }
    let dice = dice(ours_key, theirs_key);
    if ours.kind == Kind::Picture {
        return picture_similarity(ours, theirs, dice);
    }
    dice
}

/// How similar two pictures are, given how similar their texts are.
///
/// The floor is for one picture read twice, so it stands only while nothing
/// says otherwise. Two boxes that do not share a page, or do not touch at all,
/// are two pictures: pairing them would take a row away from the picture each of
/// them really belongs beside, and would report a media type disagreement about
/// a drawing the other reading never claimed to have found there. Where a
/// producer published no box — as the flow formats do — there is nothing to
/// contradict the floor, and it stands.
fn picture_similarity(ours: &Block, theirs: &Block, dice: f64) -> f64 {
    match (ours.place, theirs.place) {
        (Some(ours), Some(theirs)) if ours.page != theirs.page || overlap(ours, theirs) <= 0.0 => {
            dice
        }
        _ => dice.max(PICTURE_FLOOR),
    }
}

/// What the alignment asks of a pair of blocks: whether their words make them a
/// link at all, and what that link is worth once the boxes have had their say.
///
/// `None` is the words saying no: two blocks that do not clear [`LINK_THRESHOLD`]
/// are two rows, and no box changes that. What the boxes change is *which* link the
/// alignment takes where the words offer it more than one: a pair both producers
/// put in the same place is worth [`PLACE_BONUS`] more, one they put in different
/// places [`PLACE_COST`] less. Two pairs are left out of that: two equations, whose
/// text is a rendering rather than words, so that a pair of them is a pair for
/// reasons that have nothing to do with where either sits.
fn link_score(ours: &Block, ours_key: &Key, theirs: &Block, theirs_key: &Key) -> Option<f64> {
    let content = similarity(ours, ours_key, theirs, theirs_key);
    if content < LINK_THRESHOLD {
        return None;
    }
    if ours.label == "formula" && theirs.label == "formula" {
        return Some(content);
    }
    Some(match place_agreement(ours, theirs) {
        Some(true) => content + PLACE_BONUS,
        Some(false) => content - PLACE_COST,
        None => content,
    })
}

/// Dice's coefficient over two keys: the share of the pairs of letters the two
/// texts have in common, which says "mostly the same" for a block that was
/// reworded or had a clause appended.
fn dice(one: &Key, other: &Key) -> f64 {
    if one.bigrams.is_empty() || other.bigrams.is_empty() {
        // Nothing to compare: two blocks with no alphanumerics at all (a rule, a
        // blank line) are only the same if they are the same.
        return 0.0;
    }
    2.0 * shared(one, other) as f64 / (one.bigrams.len() + other.bigrams.len()) as f64
}

/// How many bigrams two keys have in common, both sorted.
fn shared(one: &Key, other: &Key) -> usize {
    let (mut i, mut j, mut common) = (0usize, 0usize, 0usize);
    while i < one.bigrams.len() && j < other.bigrams.len() {
        match one.bigrams[i].cmp(&other.bigrams[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    common
}

/// Clip a row's two sides to what a panel can show.
fn clip(row: &mut Row) {
    for block in row.ours.iter_mut().chain(row.theirs.iter_mut()) {
        block.text = clip_text(&block.text);
    }
}

/// Text clipped to [`MAX_TEXT`] characters, with the clipping made visible.
fn clip_text(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(MAX_TEXT).collect();
    clipped.push('…');
    clipped
}

/// Share of the reference's alphanumeric characters that ours also has.
///
/// The same metric the golden comparison prints and `progress.md` quotes: a
/// multiset intersection, so a character the reference repeats twice has to be
/// there twice to count twice. Equations are left out of both sides — their
/// notation differs by design — which is also why an equation pair is a row of
/// its own rather than a character-level loss.
fn text_agreement(ours: &Value, reference: &Value) -> f64 {
    let mut available = characters(ours);
    let theirs = characters(reference);
    let mut matched = 0usize;
    for (character, count) in &theirs {
        let left = available.entry(*character).or_default();
        let used = (*count).min(*left);
        matched += used;
        *left -= used;
    }
    let total: usize = theirs.values().sum();
    if total == 0 {
        1.0
    } else {
        matched as f64 / total as f64
    }
}

/// The alphanumeric characters of a document's text items and table cells, as a
/// multiset. Math is excluded: see [`without_inline_math`].
fn characters(document: &Value) -> BTreeMap<char, usize> {
    let mut counts = BTreeMap::new();
    let mut add = |text: &str| {
        for character in without_inline_math(text)
            .chars()
            .filter(|character| character.is_alphanumeric())
        {
            *counts.entry(character).or_insert(0usize) += 1;
        }
    };
    for item in document["texts"].as_array().into_iter().flatten() {
        if item["label"].as_str() == Some("formula") {
            continue;
        }
        add(item["text"].as_str().unwrap_or_default());
    }
    for table in document["tables"].as_array().into_iter().flatten() {
        for cell in table["data"]["table_cells"]
            .as_array()
            .into_iter()
            .flatten()
        {
            add(cell["text"].as_str().unwrap_or_default());
        }
    }
    counts
}

/// `text` with Docling's `$...$` inline math removed.
///
/// The dollars have to *pair up*: a document that prints a price has a single
/// `$` in an item, and reading it as a delimiter would swallow the rest of that
/// item and make that side look empty. An unpaired dollar is literal text. (This
/// is the rule the golden comparison arrived at the hard way; see bug 48.)
fn without_inline_math(text: &str) -> String {
    let total = text.matches('$').count();
    let unpaired_last = total % 2 == 1;
    let mut seen = 0usize;
    let mut in_math = false;
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if character == '$' {
            seen += 1;
            if unpaired_last && seen == total {
                out.push(character);
                continue;
            }
            in_math = !in_math;
            continue;
        }
        if !in_math {
            out.push(character);
        }
    }
    out
}

/// What the two documents say about their pages: how many, how big, and whether
/// one of them is missing a page the other has.
fn page_notes(ours: &Value, reference: &Value) -> Vec<String> {
    let mut notes = Vec::new();
    for (page, size) in reference["pages"].as_object().into_iter().flatten() {
        let Some(theirs) = size.get("size") else {
            continue;
        };
        let Some(ours_size) = ours["pages"].get(page).and_then(|page| page.get("size")) else {
            notes.push(format!("page {page} is missing from our output"));
            continue;
        };
        for side in ["width", "height"] {
            let (a, b) = (ours_size[side].as_f64(), theirs[side].as_f64());
            if let (Some(a), Some(b)) = (a, b) {
                if (a - b).abs() > 1.0 {
                    notes.push(format!(
                        "page {page} {side}: ours {a}, reference {b} (more than a point apart)"
                    ));
                }
            }
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A document with the given blocks, in the given order.
    ///
    /// Built the way a real one is written: the arrays hold the items and the
    /// body tree says what order they come in.
    fn document(blocks: &[Value]) -> Value {
        let mut texts = Vec::new();
        let mut tables = Vec::new();
        let mut pictures = Vec::new();
        let mut children = Vec::new();
        for block in blocks {
            let kind = block["kind"].as_str().unwrap_or("text");
            let item = match kind {
                "table" => {
                    let at = tables.len();
                    tables.push(block.clone());
                    format!("#/tables/{at}")
                }
                "picture" => {
                    let at = pictures.len();
                    pictures.push(block.clone());
                    format!("#/pictures/{at}")
                }
                _ => {
                    let at = texts.len();
                    texts.push(block.clone());
                    format!("#/texts/{at}")
                }
            };
            children.push(json!({ "$ref": item }));
        }
        json!({
            "body": { "self_ref": "#/body", "children": children },
            "furniture": { "self_ref": "#/furniture", "children": [] },
            "texts": texts,
            "tables": tables,
            "pictures": pictures,
            "pages": {},
        })
    }

    fn text(label: &str, text: &str) -> Value {
        json!({ "label": label, "text": text, "prov": [{ "page_no": 1, "bbox": { "l": 0.0, "t": 0.0, "r": 1.0, "b": 1.0 } }] })
    }

    fn picture(mime: &str, width: f64, height: f64) -> Value {
        json!({ "kind": "picture", "label": "picture", "image": { "mimetype": mime, "dpi": 144, "size": { "width": width, "height": height } } })
    }

    fn table(rows: &[&[&str]]) -> Value {
        let mut cells = Vec::new();
        for (row, texts) in rows.iter().enumerate() {
            for (column, text) in texts.iter().enumerate() {
                cells.push(json!({
                    "text": text,
                    "start_row_offset_idx": row,
                    "start_col_offset_idx": column,
                }));
            }
        }
        json!({ "kind": "table", "label": "table", "data": { "table_cells": cells } })
    }

    /// The rows of two documents compared, for terseness in the tests below.
    fn rows(ours: &Value, theirs: &Value) -> Vec<Row> {
        compare_documents(ours, theirs).0
    }

    fn summary(ours: &Value, theirs: &Value) -> Summary {
        compare_documents(ours, theirs).1
    }

    /// The placed items grouped by page, for the tests of the per-page drawing.
    fn page_map(ours: &Value, theirs: &Value) -> Vec<PlacedPage> {
        compare_documents(ours, theirs).2
    }

    fn verdicts(rows: &[Row]) -> Vec<Verdict> {
        rows.iter().map(|row| row.verdict).collect()
    }

    #[test]
    fn two_readings_of_the_same_document_agree_row_for_row() {
        let ours = document(&[
            text("title", "A study of things"),
            table(&[&["a", "b"], &["c", "d"]]),
            picture("image/png", 271.0, 291.0),
            text("paragraph", "The body of the text."),
        ]);
        let reference = document(&[
            text("title", "A study of things"),
            table(&[&["a", "b"], &["c", "d"]]),
            picture("image/png", 271.0, 291.0),
            text("paragraph", "The body of the text."),
        ]);

        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Same; 4], "{rows:#?}");
        let compared = summary(&ours, &reference);
        assert_eq!(compared.same, 4);
        assert_eq!(compared.differs, 0);
        assert_eq!(compared.text_agreement, 1.0);
    }

    #[test]
    fn blocks_come_out_in_the_order_the_body_tree_puts_them() {
        // A table between two paragraphs must stay between them: ordering by
        // array would put every text first and every table last.
        let ours = document(&[
            text("paragraph", "before"),
            table(&[&["x"]]),
            text("paragraph", "after"),
        ]);
        let blocks = blocks(&ours);
        assert_eq!(
            blocks.iter().map(|b| b.kind).collect::<Vec<_>>(),
            vec![Kind::Text, Kind::Table, Kind::Text]
        );
        assert_eq!(blocks[1].text, "x");
    }

    #[test]
    fn a_group_is_walked_where_it_stands() {
        // Docling encloses a picture area in a group, so its children belong
        // between the paragraphs that surround it rather than after every
        // paragraph of the document. The inner group is there to prove the walk
        // descends as far as the tree does.
        let ours = json!({
            "body": { "children": [
                { "$ref": "#/texts/0" },
                { "$ref": "#/groups/0" },
                { "$ref": "#/texts/1" },
            ] },
            "furniture": { "children": [] },
            "groups": [
                { "label": "picture_area", "children": [
                    { "$ref": "#/texts/2" },
                    { "$ref": "#/pictures/0" },
                    { "$ref": "#/groups/1" },
                ] },
                { "label": "inline", "children": [ { "$ref": "#/texts/3" } ] },
            ],
            "texts": [
                text("paragraph", "before"),
                text("paragraph", "after"),
                text("caption", "the caption"),
                text("paragraph", "nested"),
            ],
            "tables": [],
            "pictures": [picture("image/png", 10.0, 10.0)],
            "pages": {},
        });

        let texts: Vec<String> = blocks(&ours).into_iter().map(|block| block.text).collect();
        assert_eq!(
            texts,
            vec![
                "before",
                "the caption",
                "image/png 10x10 @144dpi",
                "nested",
                "after",
            ]
        );
    }

    #[test]
    fn a_block_the_tree_never_mentions_is_still_compared() {
        let mut ours = document(&[text("paragraph", "first")]);
        ours["texts"]
            .as_array_mut()
            .unwrap()
            .push(text("paragraph", "orphan"));
        let reference = document(&[text("paragraph", "first")]);

        let blocks = blocks(&ours);
        assert_eq!(blocks.len(), 2, "the orphan was dropped: {blocks:#?}");
        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Same, Verdict::OursOnly]);
    }

    #[test]
    fn a_reworded_block_is_one_row_not_a_shift() {
        // The failure this guards: one changed paragraph dragging every later
        // row out of line, which reads as a document-wide difference.
        let ours = document(&[
            text("paragraph", "The quick brown fox jumps over the lazy dog."),
            text("paragraph", "Second paragraph of the document."),
            text("paragraph", "Third paragraph of the document."),
        ]);
        let reference = document(&[
            text("paragraph", "The quick brown fox jumps over the lazy cat."),
            text("paragraph", "Second paragraph of the document."),
            text("paragraph", "Third paragraph of the document."),
        ]);

        let rows = rows(&ours, &reference);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::Differs, Verdict::Same, Verdict::Same],
            "{rows:#?}"
        );
        assert_eq!(rows[0].note.as_deref(), Some("text differs"));
    }

    #[test]
    fn a_paragraph_that_was_added_or_lost_is_shown_alone() {
        let ours = document(&[
            text("paragraph", "One."),
            text("paragraph", "Two, which the reference does not have."),
            text("paragraph", "Three."),
        ]);
        let reference = document(&[
            text("paragraph", "One."),
            text("paragraph", "Three."),
            text("paragraph", "Four, which we do not have."),
        ]);

        let rows = rows(&ours, &reference);
        assert_eq!(
            verdicts(&rows),
            vec![
                Verdict::Same,
                Verdict::OursOnly,
                Verdict::Same,
                Verdict::TheirsOnly
            ],
            "{rows:#?}"
        );
        assert!(rows[1].theirs.is_empty());
        assert!(rows[1].note.as_deref().unwrap().contains("only ours"));
        assert!(rows[3].ours.is_empty());
    }

    #[test]
    fn ordinary_body_text_is_not_reported_as_a_labelling_difference() {
        // Docling says `text`, we say `paragraph`: the same thing under two
        // names, and a panel full of that would hide everything else.
        let ours = document(&[text("paragraph", "The body of the text.")]);
        for label in ["text", "code", "reference", "paragraph"] {
            let reference = document(&[text(label, "The body of the text.")]);
            let rows = rows(&ours, &reference);
            assert_eq!(
                rows[0].verdict,
                Verdict::Same,
                "{label} was called a difference"
            );
            assert_eq!(rows[0].note, None);
        }
        // A heading against body text is a difference, and still paired.
        let reference = document(&[text("section_header", "The body of the text.")]);
        let rows = rows(&ours, &reference);
        assert_eq!(rows[0].verdict, Verdict::Differs);
        assert_eq!(
            rows[0].note.as_deref(),
            Some("label: paragraph vs section_header")
        );
    }

    /// A paragraph of ours that the reference read as a run of lines: the same
    /// words, cut in a different place.
    fn one_block_against_a_run_of_items() -> (Value, Value) {
        let ours = document(&[text(
            "paragraph",
            "The encoder is composed of a stack of six identical layers.",
        )]);
        let reference = document(&[
            text("text", "The encoder is composed of a stack"),
            text("text", "of six identical layers."),
        ]);
        (ours, reference)
    }

    #[test]
    fn the_reference_cutting_our_block_into_lines_is_one_row_not_two() {
        // The case this rule exists for: Docling writes an item per line, we join
        // a paragraph's lines into one block. Without it both items are rows
        // "only the reference has", which says missing about words we hold in full.
        let (ours, reference) = one_block_against_a_run_of_items();
        let rows = rows(&ours, &reference);

        assert_eq!(rows.len(), 1, "{rows:#?}");
        assert_eq!(rows[0].verdict, Verdict::Contained);
        assert_eq!(rows[0].ours.len(), 1, "our side is the one block");
        assert_eq!(
            rows[0].theirs.len(),
            2,
            "the reference's whole run is kept on the row: {rows:#?}"
        );
        assert!(rows[0].theirs[0].text.starts_with("The encoder"));
        assert!(rows[0].theirs[1].text.starts_with("of six"));
        assert_eq!(
            rows[0].note.as_deref(),
            Some("2 of the reference's items are wholly inside ours")
        );

        let compared = summary(&ours, &reference);
        assert_eq!(compared.contained, 1);
        assert_eq!(compared.theirs_only, 0, "no item of the run is one-sided");
        assert_eq!(compared.rows, 1, "the tally must add up to the rows sent");
        assert_eq!(compared.text_agreement, 1.0);
    }

    #[test]
    fn one_item_of_the_reference_inside_ours_is_a_containment_not_a_wording_difference() {
        // The same rule at its smallest: one item of the reference wholly inside
        // one block of ours is a difference about where an item ends, not a
        // "text differs" about the words.
        let ours = document(&[text(
            "paragraph",
            "The quick brown fox jumps over the lazy dog.",
        )]);
        let reference = document(&[text("text", "quick brown fox jumps")]);

        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Contained], "{rows:#?}");
        assert_eq!(
            rows[0].note.as_deref(),
            Some("the reference's item is wholly inside ours")
        );
        let compared = summary(&ours, &reference);
        assert_eq!(compared.contained, 1);
        assert_eq!(compared.theirs_only, 0);
        assert_eq!(compared.differs, 0, "the words are all there");
    }

    #[test]
    fn sharing_the_words_in_another_order_is_not_being_inside() {
        // Every word of the reference's item is ours, in a different order: two
        // readings, not one reading cut up. Calling this contained would hide a
        // real difference behind a shared vocabulary.
        let ours = document(&[text("paragraph", "the cat sat on the mat")]);
        let reference = document(&[text("text", "the mat sat on the cat")]);

        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Differs], "{rows:#?}");
        assert_eq!(rows[0].note.as_deref(), Some("text differs"));
        assert_eq!(summary(&ours, &reference).contained, 0);
    }

    #[test]
    fn an_item_the_reference_kept_apart_is_folded_onto_the_block_that_holds_it() {
        // The alignment can take a run of items inside one of our blocks only while
        // they stand together, and here the reference puts an item we do not hold
        // between two we do. Those two sets of words are still ours: both are taken
        // onto the block's row rather than shown as text we are missing.
        let ours = document(&[text(
            "paragraph",
            "Field 12 Direct deposit? See instructions. Routing number 45",
        )]);
        let reference = document(&[
            text("text", "Direct deposit?"),
            text("text", "Amount withheld"),
            text("text", "See instructions."),
        ]);

        let rows = rows(&ours, &reference);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::TheirsOnly, Verdict::Contained],
            "{rows:#?}"
        );
        assert!(rows[0].theirs[0].text.contains("Amount withheld"));
        assert_eq!(rows[1].theirs.len(), 2, "{rows:#?}");
        assert_eq!(
            rows[1].note.as_deref(),
            Some("2 of the reference's items are wholly inside ours")
        );

        let compared = summary(&ours, &reference);
        assert_eq!(compared.contained, 1);
        assert_eq!(
            compared.theirs_only, 1,
            "the item we really do not hold stays one-sided"
        );
        assert_eq!(compared.ours_only, 0);
        assert_eq!(compared.rows, 2);
    }

    #[test]
    fn a_row_the_readings_agreed_on_gains_the_items_inside_our_block() {
        // Both readings have this line, and the reference also has a piece of it as an
        // item of its own. The row is a containment, and the piece is on it rather
        // than standing alone as text we are missing.
        let ours = document(&[text(
            "paragraph",
            "The board recommends a final dividend of 12p per share.",
        )]);
        let reference = document(&[
            text(
                "text",
                "The board recommends a final dividend of 12p per share.",
            ),
            text("text", "a final dividend of 12p"),
        ]);

        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Contained], "{rows:#?}");
        assert_eq!(rows[0].theirs.len(), 2);
        assert_eq!(
            rows[0].note.as_deref(),
            Some("the reference's item is wholly inside ours")
        );

        let compared = summary(&ours, &reference);
        assert_eq!(compared.contained, 1);
        assert_eq!(compared.same, 0, "the row is about where the items end now");
        assert_eq!(compared.theirs_only, 0);
        assert_eq!(compared.rows, 1);
    }

    #[test]
    fn a_wording_difference_is_not_hidden_by_the_fold() {
        // Our line and the reference's item disagree about the words — 12 against 13 —
        // and the reference also has an item inside ours. Taking that item onto the
        // row must not turn the wording difference into a containment.
        let ours = document(&[text("paragraph", "Average flow was 12 units today")]);
        let reference = document(&[
            text("text", "Average flow was 13 units today"),
            text("text", "flow was"),
        ]);

        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Differs], "{rows:#?}");
        let note = rows[0].note.as_deref().unwrap_or_default();
        assert!(note.contains("text differs"), "{note}");
        assert!(note.contains("wholly inside ours"), "{note}");
        assert_eq!(rows[0].theirs.len(), 2);

        let compared = summary(&ours, &reference);
        assert_eq!(compared.contained, 0, "the words still differ");
        assert_eq!(compared.differs, 1);
        assert_eq!(compared.theirs_only, 0);
    }

    #[test]
    fn a_label_our_reading_spaced_out_letter_by_letter_is_still_ours() {
        // Our reading of a form's field labels comes out letter by letter where the
        // reference writes the words (the IRS form in the corpus does exactly this).
        // The reference's item is still a run of our words — the boundaries are
        // words of ours one character wide — and it is a row of the panel rather
        // than text reported missing.
        let ours = document(&[text(
            "paragraph",
            "35a Direct deposit?S e e  i n s t r u c t i o n s .b Routing number",
        )]);
        let reference = document(&[
            text("text", "Direct deposit?"),
            text("text", "See instructions."),
        ]);

        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Contained], "{rows:#?}");
        assert_eq!(rows[0].theirs.len(), 2, "the whole run is on the row");
        assert_eq!(summary(&ours, &reference).theirs_only, 0);
    }

    #[test]
    fn a_reference_item_that_is_a_piece_of_one_of_our_words_is_not_contained() {
        // Containment is word for word: a character test would call the
        // reference's `3` part of our `1937` and show a shared digit as one
        // reading cut up.
        let ours = document(&[text("paragraph", "Form 1937, filed twice in 1937.")]);
        let reference = document(&[text("text", "3")]);

        let rows = rows(&ours, &reference);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::OursOnly, Verdict::TheirsOnly],
            "{rows:#?}"
        );
        assert_eq!(summary(&ours, &reference).contained, 0);
    }

    #[test]
    fn an_empty_item_of_the_reference_is_not_inside_anything() {
        // Docling writes an item with no characters in it where it read a picture
        // area. It carries nothing to be contained by, and it is shown as the
        // reference's own reading rather than folded into a block of ours.
        let ours = document(&[text("paragraph", "A caption.")]);
        let reference = document(&[text("text", "")]);

        assert_eq!(
            verdicts(&rows(&ours, &reference)),
            vec![Verdict::OursOnly, Verdict::TheirsOnly]
        );
        assert_eq!(summary(&ours, &reference).contained, 0);
    }

    #[test]
    fn a_run_inside_ours_keeps_a_label_the_readings_disagree_about() {
        // We read a heading as the head of a paragraph, so the reference's two
        // items both lie inside our block. The row is about where those items
        // end — but the label is a finding of its own, and a run must not swallow
        // it (the golden suite's label budgets are exactly this).
        let ours = document(&[text(
            "paragraph",
            "3.1 Encoder Stacks and the layers below them",
        )]);
        let reference = document(&[
            text("section_header", "3.1 Encoder Stacks"),
            text("text", "and the layers below them"),
        ]);

        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Differs], "{rows:#?}");
        let note = rows[0].note.as_deref().unwrap_or_default();
        assert!(note.contains("wholly inside ours"), "{note}");
        assert!(
            note.contains("label: paragraph vs section_header"),
            "{note}"
        );
        assert_eq!(rows[0].theirs.len(), 2);
    }

    #[test]
    fn a_mislabelled_block_aligns_and_says_what_disagrees() {
        // The interesting case for this mode: Docling calls it a heading, we
        // call it body text. The words are the same, so the row must pair —
        // showing it as two one-sided rows would hide the disagreement.
        let ours = document(&[text("paragraph", "3.1 Encoder and Decoder Stacks")]);
        let reference = document(&[text("section_header", "3.1 Encoder and Decoder Stacks")]);

        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Differs]);
        assert_eq!(
            rows[0].note.as_deref(),
            Some("label: paragraph vs section_header")
        );
        assert_eq!(rows[0].ours[0].label, "paragraph");
    }

    #[test]
    fn an_equation_is_a_notation_row_not_a_defect() {
        // Docling writes `\sum_{}^{2}x`; we linearise the same equation to
        // `∑^2 x`. Both found the equation, so the row is paired and marked.
        let ours = document(&[text("paragraph", "Around it:"), text("formula", "∑^2 x")]);
        let reference = document(&[
            text("paragraph", "Around it:"),
            text("formula", "\\sum_{}^{2}x"),
        ]);

        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Same, Verdict::Notation]);
        let compared = summary(&ours, &reference);
        assert_eq!(compared.notation, 1);
        assert_eq!(compared.differs, 0);
        assert_eq!(
            compared.text_agreement, 1.0,
            "equation notation must not count against the agreement"
        );
    }

    /// A block placed on a page: the same item, with the box its producer
    /// published for it, in the frame that producer measures in.
    fn at(mut item: Value, page: i64, bbox: [f64; 4], origin: &str) -> Value {
        item["prov"] = json!([{
            "page_no": page,
            "bbox": {
                "l": bbox[0],
                "t": bbox[1],
                "r": bbox[2],
                "b": bbox[3],
                "coord_origin": origin,
            },
        }]);
        item
    }

    /// A page size on a document: what turns a bottom-left box around, and so
    /// what a producer has to publish for its boxes to be comparable at all.
    fn sized(document: Value, width: f64, height: f64) -> Value {
        pages(document, &[(1, width, height)])
    }

    /// The page sizes of a document, as `(page, width, height)`.
    fn pages(mut document: Value, sizes: &[(i64, f64, f64)]) -> Value {
        let mut map = serde_json::Map::new();
        for (page, width, height) in sizes {
            map.insert(
                page.to_string(),
                json!({ "size": { "width": width, "height": height } }),
            );
        }
        document["pages"] = Value::Object(map);
        document
    }

    /// The page is 600 x 800, so a box 200 points below the top is 600 above
    /// the bottom. `top` is the box as we publish it (top-left); this is the
    /// same box as Docling publishes it.
    fn page_box(top: [f64; 4]) -> [f64; 4] {
        [top[0], 800.0 - top[1], top[2], 800.0 - top[3]]
    }

    /// A picture and the paragraph that belongs to it, in opposite orders in
    /// the two readings, with the boxes both producers published for them.
    fn two_readings_in_opposite_order() -> (Value, Value) {
        let caption = [50.0, 40.0, 300.0, 60.0];
        let drawing = [50.0, 200.0, 250.0, 320.0];
        let ours = sized(
            document(&[
                at(picture("image/png", 200.0, 120.0), 1, drawing, "TOPLEFT"),
                at(
                    text("paragraph", "The figure above."),
                    1,
                    caption,
                    "TOPLEFT",
                ),
            ]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    text("text", "The figure above."),
                    1,
                    page_box(caption),
                    "BOTTOMLEFT",
                ),
                at(
                    picture("image/png", 200.0, 120.0),
                    1,
                    page_box(drawing),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );
        (ours, theirs)
    }

    #[test]
    fn a_block_the_readings_ordered_differently_is_one_row_not_two() {
        // The case this rule exists for: a picture the reference puts after the
        // paragraph we put it before. Content cannot pair the two — pairing both
        // pictures *and* both paragraphs would make the pairs cross — so the two
        // one-sided picture rows are joined by the boxes the producers published.
        let (ours, theirs) = two_readings_in_opposite_order();
        let rows = rows(&ours, &theirs);

        assert_eq!(rows.len(), 2, "the join must not add a row: {rows:#?}");
        assert_eq!(verdicts(&rows), vec![Verdict::Same, Verdict::Same]);
        assert_eq!(rows[0].kind, Kind::Picture);
        assert_eq!(rows[1].kind, Kind::Text);

        let compared = summary(&ours, &theirs);
        assert_eq!(compared.paired_by_position, 1, "{compared:#?}");
        assert_eq!(compared.ours_only, 0, "no row is one-sided any more");
        assert_eq!(compared.theirs_only, 0);
        assert_eq!(compared.rows, 2);

        // A pairing content did not decide is never silent.
        let note = rows[0].note.as_deref().unwrap_or_default();
        assert!(note.contains("paired by position"), "{note}");
        assert!(note.contains("page 1"), "{note}");
        assert_eq!(rows[1].note, None, "the paired row says nothing extra");
    }

    #[test]
    fn a_joined_row_takes_the_place_of_the_first_of_the_two_it_replaces() {
        // Our order is the one on screen, so the joined row sits where our row
        // was and the paragraphs around it do not move.
        let first = [50.0, 40.0, 300.0, 60.0];
        let drawing = [50.0, 200.0, 250.0, 320.0];
        let last = [50.0, 400.0, 300.0, 420.0];
        let ours = sized(
            document(&[
                at(text("paragraph", "One."), 1, first, "TOPLEFT"),
                at(picture("image/png", 200.0, 120.0), 1, drawing, "TOPLEFT"),
                at(text("paragraph", "Two."), 1, last, "TOPLEFT"),
            ]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(text("text", "One."), 1, page_box(first), "BOTTOMLEFT"),
                at(text("text", "Two."), 1, page_box(last), "BOTTOMLEFT"),
                at(
                    picture("image/png", 200.0, 120.0),
                    1,
                    page_box(drawing),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        let texts: Vec<String> = rows
            .iter()
            .map(|row| {
                row.ours
                    .first()
                    .map_or("-".to_string(), |block| block.text.clone())
            })
            .collect();
        assert_eq!(
            texts,
            vec!["One.", "image/png 200x120 @144dpi", "Two."],
            "{rows:#?}"
        );
        assert_eq!(verdicts(&rows), vec![Verdict::Same; 3]);
        assert_eq!(summary(&ours, &theirs).paired_by_position, 1);
    }

    #[test]
    fn one_block_in_the_same_place_on_another_page_is_not_the_same_block() {
        // The same coordinates, one page later: each reading found *something*
        // here, but not the same thing, and the panel must not join them.
        let caption = [50.0, 40.0, 300.0, 60.0];
        let ours = sized(
            document(&[at(
                text("paragraph", "The figure above."),
                1,
                caption,
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let mut theirs = document(&[at(
            text("text", "Something else entirely"),
            2,
            page_box(caption),
            "BOTTOMLEFT",
        )]);
        theirs["pages"] = json!({
            "1": { "size": { "width": 600.0, "height": 800.0 } },
            "2": { "size": { "width": 600.0, "height": 800.0 } },
        });

        assert_eq!(summary(&ours, &theirs).paired_by_position, 0);
        assert_eq!(
            verdicts(&rows(&ours, &theirs)),
            vec![Verdict::OursOnly, Verdict::TheirsOnly]
        );
    }

    #[test]
    fn boxes_that_barely_touch_do_not_join_two_rows() {
        // A tenth of the smaller box in common is two blocks that happen to be
        // near each other, not one block read twice.
        let ours = sized(
            document(&[at(
                text("paragraph", "A line of text."),
                1,
                [100.0, 300.0, 300.0, 320.0],
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[at(
                text("text", "Quite another line."),
                1,
                page_box([100.0, 318.0, 300.0, 338.0]),
                "BOTTOMLEFT",
            )]),
            600.0,
            800.0,
        );

        assert_eq!(summary(&ours, &theirs).paired_by_position, 0);
        assert_eq!(
            verdicts(&rows(&ours, &theirs)),
            vec![Verdict::OursOnly, Verdict::TheirsOnly]
        );
    }

    #[test]
    fn a_box_without_a_page_size_is_left_to_content() {
        // The reference measured from the bottom-left but published no page
        // size, so its boxes cannot be turned into the shared frame. Comparing
        // them as they stand would compare two different things, so nothing is
        // joined and the picture stays two one-sided rows — which is true.
        let (ours, mut theirs) = two_readings_in_opposite_order();
        theirs["pages"] = json!({});

        let compared = summary(&ours, &theirs);
        assert_eq!(compared.paired_by_position, 0, "{compared:#?}");
        assert_eq!(
            verdicts(&rows(&ours, &theirs)),
            vec![Verdict::OursOnly, Verdict::Same, Verdict::TheirsOnly],
            "the prose was paired by content, the picture was not paired at all"
        );
    }

    #[test]
    fn a_form_line_read_with_its_dotted_leaders_is_one_row_not_five() {
        // The shape `irs_f1040` has, and what this rule is for: a line of the form
        // whose words are ours — the two field numbers and the label — read by the
        // reference as an item per fragment, with the dotted leaders between them as
        // items of their own that carry no letters at all. Two of the fragments are
        // ours word for word, the two leaders have nothing in them to be ours by, and
        // they lie inside the line we read: all four are one row, and none of them is
        // a row saying the reference has text we do not.
        let line = [50.0, 400.0, 550.0, 420.0];
        let ours = sized(
            document(&[at(
                text(
                    "paragraph",
                    "37 38 Estimated tax penalty (see instructions) . . . . . . .38",
                ),
                1,
                line,
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    text("text", "37"),
                    1,
                    page_box([50.0, 400.0, 70.0, 420.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", ". . . . . ."),
                    1,
                    page_box([70.0, 400.0, 400.0, 420.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", ". ."),
                    1,
                    page_box([400.0, 400.0, 410.0, 420.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", "Estimated tax penalty (see instructions)"),
                    1,
                    page_box([410.0, 400.0, 550.0, 420.0]),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(rows.len(), 1, "the whole line is one row: {rows:#?}");
        assert_eq!(rows[0].verdict, Verdict::Contained, "{rows:#?}");
        assert_eq!(rows[0].ours.len(), 1, "our side is the one block");
        assert_eq!(rows[0].theirs.len(), 4, "{rows:#?}");
        assert_eq!(
            rows[0].note.as_deref(),
            Some(
                "2 of the reference's items are wholly inside ours; 2 items with no \
                 letters in them lie in the same place"
            ),
            "the row says which items were taken by their words and which by their place"
        );

        let compared = summary(&ours, &theirs);
        assert_eq!(compared.contained, 1);
        assert_eq!(
            compared.theirs_only, 0,
            "no fragment of the line is one-sided"
        );
        assert_eq!(compared.rows, 1, "the tally must add up to the rows sent");
    }

    #[test]
    fn an_item_with_no_letters_among_our_words_is_taken_but_one_standing_alone_is_not() {
        // Two shapes that look alike and are not. A leader standing *among* fragments
        // of a line we read is part of that line and is taken onto its row. A rule
        // standing alone where we read a caption is not a fragment of anything we read:
        // it is the reference's own reading of that area, and saying what the two
        // readings made of it is the joining by position's job — which pairs them,
        // because they are the same box and nothing else, and says so on the row.
        let line = [50.0, 600.0, 300.0, 620.0];

        // Standing alone: nothing of ours holds its words, so the fold leaves it.
        let theirs = sized(
            document(&[at(
                text("text", ". . . . . ."),
                1,
                page_box(line),
                "BOTTOMLEFT",
            )]),
            600.0,
            800.0,
        );
        let ours = sized(
            document(&[at(text("paragraph", "A caption."), 1, line, "TOPLEFT")]),
            600.0,
            800.0,
        );
        let alone = rows(&ours, &theirs);
        assert_eq!(alone.len(), 1, "{alone:#?}");
        assert_eq!(summary(&ours, &theirs).paired_by_position, 1, "{alone:#?}");
        assert_eq!(alone[0].verdict, Verdict::Contained, "{alone:#?}");
        let note = alone[0].note.clone().unwrap_or_default();
        assert!(
            note.contains("an item with no letters in it lies in the same place")
                && note.contains("paired by position"),
            "the row says what the rule is and that the boxes paired the two: {note}"
        );

        // Among words: the same rule, on a line whose words are ours.
        let ours = sized(
            document(&[at(
                text("paragraph", "A caption. . . . . . more of it"),
                1,
                line,
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    text("text", "A caption."),
                    1,
                    page_box([50.0, 600.0, 200.0, 620.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", ". . . . . ."),
                    1,
                    page_box([200.0, 600.0, 300.0, 620.0]),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );
        let among = rows(&ours, &theirs);
        assert_eq!(among.len(), 1, "{among:#?}");
        assert_eq!(among[0].verdict, Verdict::Contained, "{among:#?}");
        assert_eq!(among[0].theirs.len(), 2, "{among:#?}");
        assert_eq!(
            among[0].note.as_deref(),
            Some(
                "the reference's item is wholly inside ours; an item with no letters in \
                 it lies in the same place"
            )
        );
        let compared = summary(&ours, &theirs);
        assert_eq!(
            compared.theirs_only, 0,
            "the leader is not a line of its own"
        );
        assert_eq!(compared.paired_by_position, 0);
    }

    #[test]
    fn an_item_with_no_letters_somewhere_else_is_placed_not_taken() {
        // The same rule, read a page later: it is not part of the line we read, and
        // taking it would invent an absorption out of two blocks that never met. It is
        // still an item with no letters in it that nothing of ours lies along, so it is
        // **placed** — the row says what Docling wrote and where it is — rather than
        // listed as text we are missing, which for a rule would say we lost words there.
        let line = [50.0, 600.0, 300.0, 620.0];
        let ours = sized(
            document(&[at(text("paragraph", "A caption."), 1, line, "TOPLEFT")]),
            600.0,
            800.0,
        );
        let theirs = pages(
            document(&[at(
                text("text", ". . . . . ."),
                2,
                page_box(line),
                "BOTTOMLEFT",
            )]),
            &[(1, 600.0, 800.0), (2, 600.0, 800.0)],
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::OursOnly, Verdict::Placed],
            "a rule on another page is the reference's own reading, placed where it is"
        );
        // Its own row, not the line's: our caption keeps its row and the rule is
        // answered beside it, with the place and the reason on the note.
        assert!(rows[1].ours.is_empty());
        assert_eq!(rows[1].theirs.len(), 1);
        let note = rows[1].note.clone().unwrap_or_default();
        assert!(note.contains("page 2"), "{note}");
        assert!(note.contains("box"), "{note}");
        assert!(note.contains("no letters"), "{note}");

        let compared = summary(&ours, &theirs);
        assert_eq!(compared.contained, 0);
        assert_eq!(compared.placed, 1, "{compared:#?}");
        assert_eq!(
            compared.theirs_only, 0,
            "a rule with no letters in it is not text we are missing"
        );
        assert_eq!(compared.rows, 2, "the tally must add up to the rows sent");
    }

    #[test]
    fn the_leaders_of_a_form_line_we_did_not_read_are_placed_as_one_row() {
        // The rest of what `irs_f1040`'s panel still showed as "only the reference": a
        // form line our reading produced no block for at all, which Docling read as two
        // runs of dots and the rule under them. None of them is inside our line's block
        // and none stands where a block of ours stands alone, so neither the fold nor the
        // joining by position can answer them — they are placed, and the page and box they
        // name are what says which line of the form they belong to.
        //
        // They are placed as *one* row, though: three items standing on one line of one
        // page are one thing to a reader, and the row names how many it stands for and the
        // extent they cover rather than making the count of rows a property of how finely
        // the other reading cut a line.
        let our_line = [50.0, 600.0, 550.0, 620.0];
        // The line under it, which the reference read as two runs of dots and the rule
        // between them: the same line, cut into items the way Docling cuts a form.
        let their_line = [50.0, 560.0, 550.0, 580.0];
        let left = [their_line[0], their_line[1], 300.0, their_line[3]];
        let right = [300.0, their_line[1], their_line[2], their_line[3]];
        let ours = sized(
            document(&[at(
                text("paragraph", "Estimated tax penalty"),
                1,
                our_line,
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    text("text", "Estimated tax penalty"),
                    1,
                    page_box([50.0, 600.0, 300.0, 620.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", ". . . . . . . "),
                    1,
                    page_box(left),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", ". . . . . ."),
                    1,
                    page_box(right),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", "-"),
                    1,
                    page_box([50.0, 580.0, 550.0, 581.0]),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::Same, Verdict::Placed],
            "the three items of the line are one placed row: {rows:#?}"
        );
        let placed = &rows[1];
        assert!(
            placed.ours.is_empty(),
            "a placed row has no side of ours: {placed:#?}"
        );
        assert_eq!(
            placed.theirs.len(),
            3,
            "the row stands for every item of the line: {placed:#?}"
        );
        let note = placed.note.clone().unwrap_or_default();
        assert!(
            note.starts_with("3 items with no letters in them along one line of page 1, box "),
            "{note}"
        );
        // ...and the extent they cover: from the left-most dot to the right-most, and from
        // the top of the dots down to the rule under them.
        assert!(note.contains("50,560–550,581"), "{note}");
        let placement = placed.placement.expect("a placed row carries its box");
        let near = |one: f64, other: f64| (one - other).abs() < 1e-9;
        assert!(
            near(placement.x, 50.0 / 600.0)
                && near(placement.y, 560.0 / 800.0)
                && near(placement.w, 500.0 / 600.0)
                && near(placement.h, 21.0 / 800.0),
            "the panel draws the extent the row names: {placement:?}"
        );

        let compared = summary(&ours, &theirs);
        assert_eq!(compared.placed, 1, "a placed row is one row: {compared:#?}");
        assert_eq!(
            compared.theirs_only, 0,
            "nothing of the form line is called missing"
        );
        assert_eq!(compared.contained, 0);
        assert_eq!(compared.rows, 2, "the tally must add up to the rows sent");
    }

    #[test]
    fn two_lines_of_a_form_are_placed_apart() {
        // The grouping is per line: items standing on another line of the same page are
        // their own row, with their own extent, even when they are the next thing in the
        // reference's order.
        let first = [50.0, 560.0, 300.0, 580.0];
        let second = [50.0, 520.0, 200.0, 540.0];
        let ours = sized(
            document(&[at(
                text("paragraph", "A line of the form"),
                1,
                [50.0, 600.0, 300.0, 620.0],
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    text("text", ". . . . . ."),
                    1,
                    page_box(first),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", ". . . . . ."),
                    1,
                    page_box(second),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::OursOnly, Verdict::Placed, Verdict::Placed],
            "{rows:#?}"
        );
        assert_eq!(
            rows[1].theirs.len(),
            1,
            "one item on that line: {:#?}",
            rows[1]
        );
        assert_eq!(
            rows[2].theirs.len(),
            1,
            "one item on that line: {:#?}",
            rows[2]
        );
        assert!(
            rows[1]
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("50,560–300,580"),
            "{:#?}",
            rows[1].note
        );
        assert!(
            rows[2]
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("50,520–200,540"),
            "{:#?}",
            rows[2].note
        );
        assert_eq!(summary(&ours, &theirs).placed, 2);
    }

    #[test]
    fn a_block_of_ours_between_two_leaders_does_not_split_them() {
        // Adjacency is the reference's, not the panel's. Docling wrote the two runs of
        // dots next to each other; we read a block of our own on that line, which holds
        // nothing of the reference's and has a row of its own. The two leaders are still
        // one run, one line of the form — and the row's extent covers the block of ours
        // standing between them, which is what the line looks like.
        let left = [50.0, 560.0, 150.0, 580.0];
        let right = [450.0, 560.0, 550.0, 580.0];
        let ours = sized(
            document(&[at(
                text("paragraph", "Our own label"),
                1,
                [220.0, 560.0, 380.0, 580.0],
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(text("text", ". . . . . ."), 1, page_box(left), "BOTTOMLEFT"),
                at(
                    text("text", ". . . . . ."),
                    1,
                    page_box(right),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::OursOnly, Verdict::Placed],
            "{rows:#?}"
        );
        assert_eq!(
            rows[1].theirs.len(),
            2,
            "both leaders are one row: {:#?}",
            rows[1]
        );
        assert!(
            rows[1]
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("50,560–550,580"),
            "{:#?}",
            rows[1].note
        );
        assert_eq!(summary(&ours, &theirs).placed, 1);
    }

    #[test]
    fn an_item_of_the_references_own_between_two_leaders_keeps_them_apart() {
        // The other side of that rule: something the reference *did* write comes between
        // them on the same line — here an item of its own that our reading holds inside
        // our block, so the row between the two leaders is the containment — and they are
        // not a run, because the reference did not write them next to each other.
        let left = [50.0, 560.0, 150.0, 580.0];
        let right = [450.0, 560.0, 550.0, 580.0];
        let ours = sized(
            document(&[at(
                text("paragraph", "Amount withheld 12"),
                1,
                [180.0, 560.0, 400.0, 580.0],
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(text("text", ". . . . . ."), 1, page_box(left), "BOTTOMLEFT"),
                at(
                    text("text", "Amount withheld"),
                    1,
                    page_box([180.0, 560.0, 400.0, 580.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", ". . . . . ."),
                    1,
                    page_box(right),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::Placed, Verdict::Contained, Verdict::Placed],
            "{rows:#?}"
        );
        assert_eq!(rows[0].theirs.len(), 1, "{:#?}", rows[0]);
        assert_eq!(rows[2].theirs.len(), 1, "{:#?}", rows[2]);
        assert_eq!(summary(&ours, &theirs).placed, 2);
    }

    #[test]
    fn a_placed_row_carries_its_box_as_fractions_of_the_page() {
        // The panel *draws* a placement, so what it is sent has to be the box as a share
        // of the page it lies on: the note names the points for a reader, and the drawing
        // needs the shape of the page. Here the page is 600 x 800 and the leader lies from
        // 50,560 to 300,580 of it — an outline of that page, with the box a twelfth of the
        // way in from the left and seven tenths of the way down.
        let leader = [50.0, 560.0, 300.0, 580.0];
        let ours = sized(
            document(&[at(
                text("paragraph", "A caption."),
                1,
                [50.0, 600.0, 300.0, 620.0],
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[at(
                text("text", ". . . . . ."),
                1,
                page_box(leader),
                "BOTTOMLEFT",
            )]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        let placed = rows
            .iter()
            .find(|row| row.verdict == Verdict::Placed)
            .unwrap_or_else(|| panic!("no placed row: {rows:#?}"));
        let placement = placed.placement.expect("a placed row carries its box");
        assert_eq!(placement.page, 1);
        assert_eq!((placement.width, placement.height), (600.0, 800.0));
        assert!((placement.x - 50.0 / 600.0).abs() < 1e-9, "{placement:?}");
        assert!((placement.y - 560.0 / 800.0).abs() < 1e-9, "{placement:?}");
        assert!((placement.w - 250.0 / 600.0).abs() < 1e-9, "{placement:?}");
        assert!((placement.h - 20.0 / 800.0).abs() < 1e-9, "{placement:?}");
        // Inside the page, so the drawing cannot spill out of its outline.
        assert!(placement.x + placement.w <= 1.0, "{placement:?}");
        assert!(placement.y + placement.h <= 1.0, "{placement:?}");
    }

    #[test]
    fn a_placement_with_no_page_size_is_named_but_not_drawn() {
        // A document that states no page sizes still has geometry where its producer
        // wrote top-left boxes, and an item with no letters in it that nothing of ours
        // holds is still *placed* — but there is no outline to draw the box on, so the
        // row carries the note and no drawing rather than a shape invented for it.
        let ours = document(&[at(
            text("paragraph", "A caption."),
            1,
            [50.0, 600.0, 300.0, 620.0],
            "TOPLEFT",
        )]);
        let theirs = document(&[at(
            text("text", ". . . . . ."),
            1,
            [50.0, 560.0, 300.0, 580.0],
            "TOPLEFT",
        )]);

        let rows = rows(&ours, &theirs);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::OursOnly, Verdict::Placed],
            "{rows:#?}"
        );
        assert_eq!(rows[1].placement, None, "nothing to draw the box on");
        assert!(rows[1]
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("page 1, box 50,560–300,580"));
        // ...and the per-page view is the same: a page with no stated size has no
        // outline, so it is not drawn at all rather than given a shape invented for it.
        assert!(
            page_map(&ours, &theirs).is_empty(),
            "a page whose size was never stated is drawn nowhere"
        );
    }

    #[test]
    fn the_placed_items_of_a_page_are_drawn_together() {
        // Two lines of a form on page 1 and one line on page 2. The rows are one per
        // line (see the tests above); the *per-page view* is built from the items, so a
        // page carries its whole leader pattern — and the pages come in order, each with
        // the shape the reference gave it and each box its own item's place.
        let ours = pages(
            document(&[
                at(
                    text("paragraph", "Line one"),
                    1,
                    [50.0, 600.0, 550.0, 620.0],
                    "TOPLEFT",
                ),
                at(
                    text("paragraph", "Line two"),
                    2,
                    [50.0, 600.0, 550.0, 620.0],
                    "TOPLEFT",
                ),
            ]),
            &[(1, 600.0, 800.0), (2, 600.0, 800.0)],
        );
        let theirs = pages(
            document(&[
                at(
                    text("text", "Line one"),
                    1,
                    page_box([50.0, 600.0, 550.0, 620.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", ". . . . . . . "),
                    1,
                    page_box([50.0, 560.0, 300.0, 580.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", ". . . . . ."),
                    1,
                    page_box([300.0, 560.0, 550.0, 580.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", "Line two"),
                    2,
                    page_box([50.0, 600.0, 550.0, 620.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", "-"),
                    2,
                    page_box([50.0, 560.0, 200.0, 580.0]),
                    "BOTTOMLEFT",
                ),
            ]),
            &[(1, 600.0, 800.0), (2, 600.0, 800.0)],
        );

        // The rows still say one line of the form each...
        let compared = summary(&ours, &theirs);
        assert_eq!(compared.placed, 2, "one row per line: {compared:#?}");
        let placed_items: usize = rows(&ours, &theirs)
            .iter()
            .filter(|row| row.verdict == Verdict::Placed)
            .map(|row| row.theirs.len())
            .sum();
        assert_eq!(placed_items, 3, "two items on page 1 and one on page 2");

        // ...and the page view draws every one of those items, on the page it lies on.
        let map = page_map(&ours, &theirs);
        assert_eq!(map.len(), 2, "two pages carry places: {map:#?}");
        assert_eq!(map[0].page, 1, "pages come in order");
        assert_eq!(map[0].boxes.len(), 2, "page 1's two leaders: {:#?}", map[0]);
        assert_eq!(map[1].page, 2);
        assert_eq!(map[1].boxes.len(), 1, "page 2's one: {:#?}", map[1]);
        // The outline is the page's own shape, as the reference stated it...
        assert_eq!((map[0].width, map[0].height), (600.0, 800.0));
        assert_eq!((map[1].width, map[1].height), (600.0, 800.0));
        // ...and a box is *the item's* place, in fractions of that page, kept inside it.
        let first = map[0].boxes[0];
        let near = |one: f64, other: f64| (one - other).abs() < 1e-9;
        assert!(
            near(first.x, 50.0 / 600.0)
                && near(first.y, 560.0 / 800.0)
                && near(first.w, 250.0 / 600.0)
                && near(first.h, 20.0 / 800.0),
            "{first:?}"
        );
        assert!(
            map[1].boxes[0].x + map[1].boxes[0].w <= 1.0,
            "{:#?}",
            map[1]
        );
        // Every placed item is on the map exactly once: the picture is complete.
        let drawn: usize = map.iter().map(|page| page.boxes.len()).sum();
        assert_eq!(drawn, placed_items, "the picture holds every placed item");
    }

    #[test]
    fn an_item_with_letters_and_no_block_of_ours_stays_only_the_reference() {
        // The rule is only ever asked of an item with nothing else to offer. Text the
        // reference read that we did not is text we did not read, however well it stands
        // on its own — that is the finding the row has to keep.
        let line = [50.0, 560.0, 550.0, 580.0];
        let ours = sized(
            document(&[at(
                text("paragraph", "A caption."),
                1,
                [50.0, 600.0, 300.0, 620.0],
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    text("text", "A caption."),
                    1,
                    page_box([50.0, 600.0, 300.0, 620.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", "Line 24 Other taxes"),
                    1,
                    page_box(line),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::Same, Verdict::TheirsOnly],
            "{rows:#?}"
        );
        let compared = summary(&ours, &theirs);
        assert_eq!(compared.placed, 0);
        assert_eq!(compared.theirs_only, 1);
    }

    #[test]
    fn an_item_with_no_letters_and_no_box_is_not_placed() {
        // A flow format publishes no geometry, so there is no place to name and the row
        // stays what it was: this is what keeps every DOCX compared exactly as before.
        let ours = document(&[text("paragraph", "A caption.")]);
        let theirs = document(&[text("text", ". . . . . .")]);

        let rows = rows(&ours, &theirs);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::OursOnly, Verdict::TheirsOnly],
            "{rows:#?}"
        );
        let compared = summary(&ours, &theirs);
        assert_eq!(compared.placed, 0);
        assert_eq!(compared.theirs_only, 1);
    }

    #[test]
    fn the_rows_items_stand_in_the_order_the_reference_wrote_them() {
        // One more thing a form line needs: the alignment pairs the block with the
        // item its words best match, and here that is the field's label at the end of
        // the line — so the field number and the dotted leader standing *before* it are
        // the fold's. The row shows them in the reference's order for all that, not
        // with the label first.
        let line = [50.0, 400.0, 550.0, 420.0];
        let ours = sized(
            document(&[at(
                text(
                    "paragraph",
                    "37 38 Estimated tax penalty (see instructions) . . . . . . .38",
                ),
                1,
                line,
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    text("text", ". . . . . ."),
                    1,
                    page_box([50.0, 400.0, 250.0, 420.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", "37"),
                    1,
                    page_box([250.0, 400.0, 280.0, 420.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", "Estimated tax penalty (see instructions)"),
                    1,
                    page_box([280.0, 400.0, 550.0, 420.0]),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(rows.len(), 1, "{rows:#?}");
        let texts: Vec<&str> = rows[0]
            .theirs
            .iter()
            .map(|block| block.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec![
                ". . . . . .",
                "37",
                "Estimated tax penalty (see instructions)"
            ],
            "the line's items in the reference's order: {rows:#?}"
        );
        assert_eq!(rows[0].verdict, Verdict::Contained, "{rows:#?}");
        assert_eq!(
            rows[0].note.as_deref(),
            Some(
                "2 of the reference's items are wholly inside ours; an item with no \
                 letters in it lies in the same place"
            )
        );
        assert_eq!(summary(&ours, &theirs).theirs_only, 0);
    }

    #[test]
    fn an_item_with_words_we_do_not_hold_is_not_taken_by_its_box() {
        // Where a block sits is only ever the reason for taking an item that has
        // nothing else to offer. An item with words in it that are not ours is text
        // we did not read, and it keeps its own row however well its box fits inside
        // our block — while the items of ours on either side of it are still taken.
        let line = [50.0, 400.0, 550.0, 420.0];
        let ours = sized(
            document(&[at(
                text(
                    "paragraph",
                    "Field 12 Direct deposit? See instructions. Routing number 45",
                ),
                1,
                line,
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    text("text", "Direct deposit?"),
                    1,
                    page_box([50.0, 400.0, 200.0, 420.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", "Amount withheld"),
                    1,
                    page_box([200.0, 400.0, 350.0, 420.0]),
                    "BOTTOMLEFT",
                ),
                at(
                    text("text", "See instructions."),
                    1,
                    page_box([350.0, 400.0, 550.0, 420.0]),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::TheirsOnly, Verdict::Contained],
            "{rows:#?}"
        );
        assert!(rows[0].theirs[0].text.contains("Amount withheld"));
        assert_eq!(rows[1].theirs.len(), 2, "{rows:#?}");
        assert_eq!(
            rows[1].note.as_deref(),
            Some("2 of the reference's items are wholly inside ours")
        );
        let compared = summary(&ours, &theirs);
        assert_eq!(
            compared.theirs_only, 1,
            "an item we really do not hold stays one-sided"
        );
        assert_eq!(compared.contained, 1);
    }

    #[test]
    fn a_formula_and_a_paragraph_in_the_same_place_are_not_one_thing() {
        // An equation we linearised and a paragraph the reference read in the
        // very same box: joining the two rows would hide that one reading found
        // an equation at all, which is the finding worth seeing.
        let box_at = [50.0, 200.0, 400.0, 240.0];
        let ours = sized(
            document(&[at(text("formula", "x^2 + y^2"), 1, box_at, "TOPLEFT")]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[at(
                text("text", "something written in words"),
                1,
                page_box(box_at),
                "BOTTOMLEFT",
            )]),
            600.0,
            800.0,
        );

        assert_eq!(summary(&ours, &theirs).paired_by_position, 0);
        assert_eq!(
            verdicts(&rows(&ours, &theirs)),
            vec![Verdict::OursOnly, Verdict::TheirsOnly]
        );
    }

    #[test]
    fn two_pictures_on_different_pages_are_not_one_picture() {
        // The floor that pairs "(no image)" with a PNG is for the *same* picture
        // read twice. A drawing of ours on page 1 and a box the reference read
        // on page 2 are two pictures, and calling the row "text differs" would
        // invent a pairing out of two things that never met.
        let box_at = [50.0, 200.0, 250.0, 320.0];
        let ours = sized(
            document(&[at(picture("image/png", 200.0, 120.0), 1, box_at, "TOPLEFT")]),
            600.0,
            800.0,
        );
        let theirs = pages(
            document(&[at(
                json!({ "kind": "picture", "label": "picture" }),
                2,
                page_box(box_at),
                "BOTTOMLEFT",
            )]),
            &[(1, 600.0, 800.0), (2, 600.0, 800.0)],
        );

        assert_eq!(
            verdicts(&rows(&ours, &theirs)),
            vec![Verdict::OursOnly, Verdict::TheirsOnly],
            "a picture on one page and a picture on another are two pictures"
        );
    }

    #[test]
    fn both_readings_swapping_two_pictures_become_one_row_each() {
        // What the corpus actually contains: a picture area of two drawings,
        // listed in one order by Docling and the other by us. Neither reading is
        // wrong, so both drawings pair instead of each being reported missing.
        let first = [50.0, 200.0, 250.0, 320.0];
        let second = [300.0, 200.0, 500.0, 320.0];
        let without = json!({ "kind": "picture", "label": "picture" });
        let ours = sized(
            document(&[
                at(without.clone(), 1, first, "TOPLEFT"),
                at(picture("image/png", 200.0, 120.0), 1, second, "TOPLEFT"),
            ]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    picture("image/png", 200.0, 120.0),
                    1,
                    page_box(second),
                    "BOTTOMLEFT",
                ),
                at(without, 1, page_box(first), "BOTTOMLEFT"),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(verdicts(&rows), vec![Verdict::Same; 2], "{rows:#?}");
        assert!(rows.iter().all(|row| row.kind == Kind::Picture));
        let compared = summary(&ours, &theirs);
        assert_eq!(compared.paired_by_position, 1, "{compared:#?}");
        assert_eq!(compared.rows, 2, "the tally must add up to the rows sent");
    }

    #[test]
    fn two_pictures_the_readings_list_in_opposite_orders_are_paired_by_their_boxes() {
        // Two drawings on one page, each read at its own size, listed the other
        // way round by the reference. The words cannot tell the two pairings
        // apart — each drawing's media type and size is close enough to the
        // other's to link either way — so pairing them by their place in the list
        // reports two pictures at the wrong size, and only the boxes say which
        // drawing is which.
        let left = [50.0, 200.0, 250.0, 320.0];
        let right = [300.0, 200.0, 500.0, 320.0];
        let ours = sized(
            document(&[
                at(picture("image/png", 200.0, 120.0), 1, left, "TOPLEFT"),
                at(picture("image/png", 240.0, 150.0), 1, right, "TOPLEFT"),
            ]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    picture("image/png", 240.0, 150.0),
                    1,
                    page_box(right),
                    "BOTTOMLEFT",
                ),
                at(
                    picture("image/png", 200.0, 120.0),
                    1,
                    page_box(left),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(verdicts(&rows), vec![Verdict::Same; 2], "{rows:#?}");
        for row in &rows {
            // Each row holds the drawing *and* the box the other reading read it
            // in — which is also what makes the two sizes on the row agree.
            assert_eq!(row.ours[0].place, row.theirs[0].place, "{row:#?}");
            assert_eq!(row.ours[0].text, row.theirs[0].text, "{row:#?}");
        }
        let compared = summary(&ours, &theirs);
        assert_eq!(compared.differs, 0, "{compared:#?}");
        assert_eq!(compared.rows, 2, "the tally must add up to the rows sent");
    }

    #[test]
    fn identical_pictures_the_readings_list_in_opposite_orders_agree_row_for_row() {
        // The same drawing twice on one page, listed one way by us and the other
        // way round by the reference. Nothing in the words can tell the two copies
        // apart — same media type, same size — so an alignment that goes by the
        // list pairs each copy with the other's box, and each of those rows then
        // says the two readings put the block in a different place: two
        // differences out of nothing. Where the copies stand is what the pairing
        // has to go on, and going by it leaves two rows that agree.
        let first = [50.0, 200.0, 250.0, 320.0];
        let second = [300.0, 200.0, 500.0, 320.0];
        let ours = sized(
            document(&[
                at(picture("image/png", 200.0, 120.0), 1, first, "TOPLEFT"),
                at(picture("image/png", 200.0, 120.0), 1, second, "TOPLEFT"),
            ]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[
                at(
                    picture("image/png", 200.0, 120.0),
                    1,
                    page_box(second),
                    "BOTTOMLEFT",
                ),
                at(
                    picture("image/png", 200.0, 120.0),
                    1,
                    page_box(first),
                    "BOTTOMLEFT",
                ),
            ]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(verdicts(&rows), vec![Verdict::Same; 2], "{rows:#?}");
        for row in &rows {
            // One pair is the alignment's, taken where the boxes agree; the other
            // is joined afterwards, and says so — but no row claims the two
            // readings put the block somewhere else.
            let note = row.note.as_deref().unwrap_or_default();
            assert!(!note.contains("not in the same place"), "{note}");
            assert_eq!(row.ours[0].place, row.theirs[0].place, "{row:#?}");
        }
        let compared = summary(&ours, &theirs);
        assert_eq!(compared.differs, 0, "{compared:#?}");
        assert_eq!(compared.paired_by_position, 1, "{compared:#?}");
        assert_eq!(compared.rows, 2, "the tally must add up to the rows sent");
    }

    #[test]
    fn a_block_the_readings_put_in_different_places_is_a_difference() {
        // The same words on both sides — but we read them at the foot of the page
        // and Docling at the head of it. Content alone would call that row "same"
        // and say nothing about where it is; the boxes are what make it a
        // difference, and the words are what keep it *one* row rather than two
        // orphans.
        let head = [50.0, 40.0, 300.0, 60.0];
        let foot = [50.0, 700.0, 300.0, 720.0];
        let ours = sized(
            document(&[at(
                text("paragraph", "The figure above."),
                1,
                foot,
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[at(
                text("text", "The figure above."),
                1,
                page_box(head),
                "BOTTOMLEFT",
            )]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(
            rows.len(),
            1,
            "the words agree: one row, not two: {rows:#?}"
        );
        assert_eq!(rows[0].verdict, Verdict::Differs, "{rows:#?}");
        let note = rows[0].note.as_deref().unwrap_or_default();
        assert!(note.contains("not in the same place"), "{note}");
        assert!(note.contains("page 1"), "{note}");
        let compared = summary(&ours, &theirs);
        assert_eq!(
            compared.paired_by_position, 0,
            "the words paired it, not the boxes"
        );
        assert_eq!((compared.ours_only, compared.theirs_only), (0, 0));
    }

    #[test]
    fn a_block_the_readings_put_on_different_pages_is_a_difference() {
        // The same words, and the page each of them sits on is the whole of the
        // disagreement: a block the other reading found a page later is not the
        // same block, and the row names both pages rather than two coordinates.
        let box_at = [50.0, 200.0, 300.0, 220.0];
        let ours = sized(
            document(&[at(
                text("paragraph", "The figure above."),
                1,
                box_at,
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = pages(
            document(&[at(
                text("text", "The figure above."),
                2,
                page_box(box_at),
                "BOTTOMLEFT",
            )]),
            &[(1, 600.0, 800.0), (2, 600.0, 800.0)],
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(rows.len(), 1, "{rows:#?}");
        assert_eq!(rows[0].verdict, Verdict::Differs, "{rows:#?}");
        let note = rows[0].note.as_deref().unwrap_or_default();
        assert!(
            note.contains("not in the same place: page 1 vs page 2"),
            "{note}"
        );
    }

    #[test]
    fn blocks_in_the_same_place_the_words_do_not_link_are_still_joined_by_position() {
        // The bonus is a nudge, not evidence: two blocks that sit in the very same
        // box and share no words are two blocks, and the only thing that may pair
        // them is the joining by position — which says so on the row. It is also
        // what keeps a joined row from being accused of being in the wrong place:
        // the boxes are what paired it.
        let box_at = [50.0, 200.0, 300.0, 220.0];
        let ours = sized(
            document(&[at(
                text("paragraph", "Alpha beta gamma delta."),
                1,
                box_at,
                "TOPLEFT",
            )]),
            600.0,
            800.0,
        );
        let theirs = sized(
            document(&[at(
                text("text", "Nothing alike written here"),
                1,
                page_box(box_at),
                "BOTTOMLEFT",
            )]),
            600.0,
            800.0,
        );

        let rows = rows(&ours, &theirs);
        assert_eq!(rows.len(), 1, "{rows:#?}");
        let note = rows[0].note.as_deref().unwrap_or_default();
        assert_eq!(rows[0].verdict, Verdict::Differs, "{rows:#?}");
        assert!(note.contains("text differs"), "{note}");
        assert!(note.contains("paired by position"), "{note}");
        assert!(!note.contains("not in the same place"), "{note}");
        assert_eq!(summary(&ours, &theirs).paired_by_position, 1);
    }

    #[test]
    fn a_document_without_boxes_is_compared_exactly_as_before() {
        // DOCX, `.doc` and Markdown publish no geometry at all, so nothing can
        // be joined and the rows are the alignment's own. This is the whole
        // golden corpus: it is why the rule above changes nothing there.
        let ours = document(&[
            text("paragraph", "One."),
            at(
                picture("image/png", 10.0, 10.0),
                1,
                [0.0, 0.0, 1.0, 1.0],
                "TOPLEFT",
            ),
        ]);
        let reference = document(&[text("paragraph", "One.")]);

        let compared = summary(&ours, &reference);
        assert_eq!(compared.paired_by_position, 0);
        assert_eq!(
            verdicts(&rows(&ours, &reference)),
            vec![Verdict::Same, Verdict::OursOnly]
        );
    }

    #[test]
    fn an_equation_only_one_side_found_is_a_one_sided_row() {
        // The same sentence, and only we found the equation in it.
        let ours = document(&[text("paragraph", "Which gives:"), text("formula", "x^2")]);
        let reference = document(&[text("paragraph", "Which gives:")]);
        assert_eq!(
            verdicts(&rows(&ours, &reference)),
            vec![Verdict::Same, Verdict::OursOnly],
        );
    }

    #[test]
    fn a_picture_without_an_image_says_so() {
        let ours = document(&[json!({ "kind": "picture", "label": "picture" })]);
        let mut reference = document(&[picture("image/png", 100.0, 50.0)]);
        reference["pages"] = json!({ "1": { "size": { "width": 595.0, "height": 842.0 } } });

        let rows = rows(&ours, &reference);
        assert_eq!(rows[0].ours[0].text, "(no image)");
        assert_eq!(rows[0].theirs[0].text, "image/png 100x50 @144dpi");
        assert_eq!(rows[0].verdict, Verdict::Differs);
    }

    #[test]
    fn a_table_is_read_row_by_row_however_the_cells_are_ordered() {
        let ours = document(&[table(&[&["row one"], &["row two"]])]);
        // The same two rows, with the *cells* in the other order in the array:
        // the row index each cell states is the layout, the array is not.
        let cells_reversed = json!({
            "kind": "table",
            "label": "table",
            "data": { "table_cells": [
                { "text": "row two", "start_row_offset_idx": 1, "start_col_offset_idx": 0 },
                { "text": "row one", "start_row_offset_idx": 0, "start_col_offset_idx": 0 },
            ] }
        });
        let reference = document(&[cells_reversed]);

        let table = &ours["tables"][0];
        assert_eq!(table_text(table), "row one\nrow two");
        assert_eq!(verdicts(&rows(&ours, &reference)), vec![Verdict::Same]);
    }

    #[test]
    fn an_extra_shape_of_reading_is_counted_one_side_only() {
        let ours = document(&[
            text("paragraph", "Here is the figure:"),
            picture("image/png", 10.0, 10.0),
        ]);
        let reference = document(&[text("paragraph", "Here is the figure:")]);
        let rows = rows(&ours, &reference);
        assert_eq!(verdicts(&rows), vec![Verdict::Same, Verdict::OursOnly]);
        assert_eq!(rows[1].kind, Kind::Picture);
    }

    #[test]
    fn the_agreement_counts_a_repeated_character_twice() {
        // The reference says the word twice, so one copy of it here is half of
        // what the reference has — the multiset rule the golden check uses.
        let ours = document(&[text("paragraph", "abcabc")]);
        let reference = document(&[text("paragraph", "abcabcabcabc")]);
        assert_eq!(summary(&ours, &reference).text_agreement, 0.5);
    }

    #[test]
    fn an_unpaired_dollar_is_literal_text_not_math() {
        // The IRS form prints prices; reading `$15,750` as the start of inline
        // math swallowed the rest of the item and made that side look empty.
        let ours = document(&[text("paragraph", "you want $3 to go to this fund")]);
        let reference = document(&[text("paragraph", "you want $3 to go to this fund")]);
        assert_eq!(summary(&ours, &reference).text_agreement, 1.0);
        assert_eq!(without_inline_math("a $x$ b"), "a  b");
    }

    #[test]
    fn a_page_only_one_side_has_is_noted() {
        let ours = document(&[text("paragraph", "one page")]);
        let mut reference = document(&[text("paragraph", "one page")]);
        reference["pages"] = json!({
            "1": { "size": { "width": 595.0, "height": 842.0 } },
            "2": { "size": { "width": 595.0, "height": 842.0 } },
        });
        let compared = summary(&ours, &reference);
        assert_eq!(compared.pages_ours, 0);
        assert_eq!(compared.pages_theirs, 2);
        assert!(
            compared
                .notes
                .iter()
                .any(|note| note.contains("page 1 is missing")),
            "{:?}",
            compared.notes
        );

        // A page measured differently is worth saying too.
        let mut ours = document(&[text("paragraph", "x")]);
        ours["pages"] = json!({ "1": { "size": { "width": 612.0, "height": 792.0 } } });
        let mut reference = document(&[text("paragraph", "x")]);
        reference["pages"] = json!({ "1": { "size": { "width": 595.0, "height": 842.0 } } });
        let notes = summary(&ours, &reference).notes;
        assert!(
            notes.iter().any(|note| note.contains("page 1 width")),
            "{notes:?}"
        );
    }

    #[test]
    fn a_row_too_long_for_a_panel_is_clipped_visibly() {
        let long = "word ".repeat(200);
        let ours = document(&[text("paragraph", &long)]);
        let reference = document(&[text("paragraph", &long)]);
        let rows = rows(&ours, &reference);
        let shown = &rows[0].ours[0].text;
        assert!(shown.chars().count() <= MAX_TEXT + 1, "{}", shown.len());
        assert!(shown.ends_with('…'), "the clipping was not made visible");
        // The verdict is still about the whole text.
        assert_eq!(rows[0].verdict, Verdict::Same);
    }

    #[test]
    fn a_document_too_large_to_align_is_truncated_and_says_so() {
        let many = |count: usize| {
            document(
                &(0..count)
                    .map(|index| text("paragraph", &format!("block number {index}")))
                    .collect::<Vec<_>>(),
            )
        };
        let ours = many(MAX_BLOCKS + 5);
        let reference = many(MAX_BLOCKS + 5);

        let (rows, summary, _) = compare_documents(&ours, &reference);
        assert_eq!(rows.len(), MAX_ROWS, "the rows sent must be bounded");
        // Five blocks left out of each side.
        assert_eq!(summary.truncated, 10, "{summary:#?}");
        assert!(
            summary
                .notes
                .iter()
                .any(|note| note.contains("compared the first")),
            "{:?}",
            summary.notes
        );
    }

    #[test]
    fn a_different_kind_of_block_is_never_paired() {
        // A table and a paragraph with the same words are not the same thing.
        let ours = document(&[table(&[&["same words"]])]);
        let reference = document(&[text("paragraph", "same words")]);
        let rows = rows(&ours, &reference);
        assert_eq!(
            verdicts(&rows),
            vec![Verdict::OursOnly, Verdict::TheirsOnly],
            "{rows:#?}"
        );
    }

    #[test]
    fn a_reference_is_a_file_or_a_folder_of_them() {
        let dir = scratch("reference-for");
        std::fs::write(dir.join("report.json"), "{}").unwrap();
        std::fs::write(dir.join("notes.txt.json"), "{}").unwrap();
        std::fs::write(dir.join("other.json"), "{}").unwrap();

        // A folder: the document's stem names the reference.
        assert_eq!(
            reference_for(&dir, Path::new("/docs/report.pdf")),
            Some(dir.join("report.json"))
        );
        // A stem that is not there falls back to the whole file name, which is
        // what a colliding output is called.
        assert_eq!(
            reference_for(&dir, Path::new("/docs/notes.txt")),
            Some(dir.join("notes.txt.json"))
        );
        // Neither name: there is nothing to compare, which is not an error.
        assert_eq!(reference_for(&dir, Path::new("/docs/missing.pdf")), None);

        // A file is used as it is, whatever it is called.
        let file = dir.join("other.json");
        assert_eq!(
            reference_for(&file, Path::new("/docs/whatever.pdf")),
            Some(file)
        );
        // A path that is not there at all.
        assert_eq!(
            reference_for(&dir.join("nope"), Path::new("/docs/a.pdf")),
            None
        );
    }

    #[test]
    fn a_reference_that_is_not_readable_is_reported_not_panicked() {
        let dir = scratch("unreadable");
        let ours = dir.join("ours.json");

        let missing = compare_files("doc.pdf", &ours, &dir.join("missing.json"));
        assert!(missing.problem.unwrap().contains("cannot read"));

        std::fs::write(&ours, "{not json}").unwrap();
        let malformed = compare_files("doc.pdf", &ours, &ours);
        assert!(malformed.problem.unwrap().contains("is not Docling JSON"));
        assert_eq!(malformed.rows.len(), 0);
        assert_eq!(malformed.summary.same, 0);
    }

    #[test]
    fn a_whole_comparison_reads_both_files_and_reports_both_paths() {
        let dir = scratch("comparison");
        let ours = dir.join("ours.json");
        let reference = dir.join("theirs.json");
        std::fs::write(&ours, document(&[text("paragraph", "same")]).to_string()).unwrap();
        std::fs::write(
            &reference,
            document(&[text("section_header", "same")]).to_string(),
        )
        .unwrap();

        let compared = compare_files("/docs/doc.pdf", &ours, &reference);
        assert_eq!(compared.problem, None);
        assert_eq!(compared.input, "/docs/doc.pdf");
        assert_eq!(compared.reference, reference.display().to_string());
        assert_eq!(compared.rows.len(), 1);
        assert_eq!(compared.rows[0].verdict, Verdict::Differs);
    }

    /// A report whose placed rows stand for `items` of the reference's items over `lines`
    /// lines of a form, spread as evenly as the count allows.
    ///
    /// Built by hand rather than compared, because the floor reads the rows and nothing
    /// else of the comparison: what it is about is how many of the reference's items are
    /// still answered by a place in the panel.
    fn form_report(input: &str, items: usize, lines: usize) -> Report {
        let rows: Vec<Row> = (0..lines)
            .map(|line| {
                let len = items / lines + usize::from(line < items % lines);
                Row {
                    kind: Kind::Text,
                    verdict: Verdict::Placed,
                    ours: Vec::new(),
                    theirs: (0..len)
                        .map(|_| Block {
                            kind: Kind::Text,
                            label: "text".to_string(),
                            text: ". . . .".to_string(),
                            page: Some(1),
                            place: None,
                        })
                        .collect(),
                    note: Some("a line of the form".to_string()),
                    placement: None,
                }
            })
            .collect();
        Report {
            input: input.to_string(),
            reference: "theirs.json".to_string(),
            problem: None,
            summary: Summary::of(&rows),
            placed_pages: Vec::new(),
            floor: None,
            held: None,
            rows,
        }
    }

    /// The numbers a memory would remember for a document: `lines` lines of a form standing
    /// for `items` of the reference, drawn as two pages, which is the shape the rules read.
    fn remembered(lines: usize, items: usize) -> Numbers {
        Numbers {
            rows: lines,
            same: 0,
            notation: 0,
            differs: 0,
            contained: 0,
            ours_only: 0,
            theirs_only: 0,
            placed: lines,
            placed_items: items,
            placed_boxes: items,
            placed_page_pictures: 2,
            joined: 0,
            agreement: 1.0,
        }
    }

    /// A memory that remembers one document.
    fn memory(name: &str, was: Numbers) -> Memory {
        Memory {
            note: "a memory, for the tests".to_string(),
            measured: Measurement {
                ocr: "never".to_string(),
                ..Default::default()
            },
            documents: BTreeMap::from([(name.to_string(), was)]),
        }
    }

    #[test]
    fn a_report_reduces_to_the_numbers_a_ratchet_can_remember() {
        // The reduction both the corpus report and the window make, so that a document means
        // the same thing to the ratchet and to the panel. The placed items are counted from
        // the *rows* — the items of one line of a form are one row — while the boxes and the
        // pages come from the drawing, which is built before the rows are cut.
        let mut report = form_report("/docs/irs_f1040.pdf", 160, 12);
        report.placed_pages = vec![PlacedPage {
            page: 1,
            width: 600.0,
            height: 800.0,
            boxes: vec![
                PlacedBox {
                    x: 0.1,
                    y: 0.1,
                    w: 0.5,
                    h: 0.01,
                },
                PlacedBox {
                    x: 0.1,
                    y: 0.2,
                    w: 0.5,
                    h: 0.01,
                },
            ],
        }];

        let numbers = Numbers::of(&report);
        assert_eq!(numbers.rows, report.rows.len());
        assert_eq!(numbers.placed, 12, "rows, not items: {numbers:?}");
        assert_eq!(numbers.placed_items, 160, "{numbers:?}");
        assert_eq!(numbers.placed_boxes, 2);
        assert_eq!(numbers.placed_page_pictures, 1);
        assert_eq!(numbers.agreement, report.summary.text_agreement);
    }

    #[test]
    fn the_window_refuses_a_document_below_the_floor_its_memory_remembers() {
        // The halved panel the *rate* cannot see — half the items in half the rows keeps the
        // ratio — and the window refuses it rather than drawing a comparison from a reading
        // whose form has left the panel.
        let was = remembered(25, 327);
        let mut report = form_report("test-data/real/irs_f1040.pdf", 160, 12);
        assert!(report.hold_to_floor(&memory("irs_f1040", was.clone()), None));
        assert_eq!(
            report.floor.as_deref(),
            Some("form item(s) placed: 327 → 160 item(s) in 12 row(s)")
        );
        // The sentence the panel carries is the ratchet's own complaint, and it is the only
        // one: the ratio was kept, so nothing else in the rule set has anything to say.
        assert_eq!(
            Numbers::of(&report).worse_than(&was),
            vec![report.floor.clone().expect("the floor")]
        );
        // The report also says *how* it was held: this run carried no measurement to
        // compare with the memory's, so only the floor rule could have refused it.
        assert_eq!(
            report.held,
            Some(Held {
                rules: HeldRules::Floor,
                why: "only the floor under a form's placed items applies: this run's settings \
                      are not the ones the memory records"
                    .to_string(),
            })
        );
    }

    #[test]
    fn the_window_holds_every_ratchet_rule_when_the_run_matches_the_memorys_measurement() {
        // The memory says how it was measured (`Memory::measured`), and this run was
        // measured the same way: the panel then refuses the document under *every* rule
        // the corpus report fails on — a `differs` count that rose is as much a fall here
        // as it is there, rather than only the floor being left.
        let was = remembered(25, 327);

        // The same form, still whole (327 items in 25 rows, nothing for the floor to
        // catch) — but one row now differs where the memory held none.
        fn with_a_differing_row() -> Report {
            let mut report = form_report("test-data/real/irs_f1040.pdf", 327, 25);
            report.rows.push(Row {
                kind: Kind::Text,
                verdict: Verdict::Differs,
                ours: vec![Block {
                    kind: Kind::Text,
                    label: "paragraph".to_string(),
                    text: "one reading of the sentence".to_string(),
                    page: Some(1),
                    place: None,
                }],
                theirs: vec![Block {
                    kind: Kind::Text,
                    label: "paragraph".to_string(),
                    text: "the other reading of it".to_string(),
                    page: Some(1),
                    place: None,
                }],
                note: Some("the words disagree".to_string()),
                placement: None,
            });
            report.summary = Summary::of(&report.rows);
            report
        }

        // Measured the way the memory was measured: every rule applies, and the panel
        // carries the ratchet's own complaint word for word.
        let like_the_memory = Measurement::from_ocr("never");
        let mut matched = with_a_differing_row();
        assert!(matched.hold_to_floor(&memory("irs_f1040", was.clone()), Some(&like_the_memory)));
        assert_eq!(
            matched.floor.as_deref(),
            Some("rows that differ 0 → 1"),
            "a matching run is held to the whole ratchet, not only the floor"
        );
        // …and the report says which rules did the refusing, in the words the panel
        // shows: the whole ratchet, because this run was measured as the memory was.
        let held = matched.held.clone().expect("held is recorded");
        assert_eq!(held.rules, HeldRules::Ratchet);
        assert!(held.why.contains("--ocr never"), "{}", held.why);

        // Measured some other way: the counts that measure how the two readings *agree*
        // belong to the settings they were made with, so only the floor under a form's
        // placed items applies — and this document stands above that floor.
        let unlike_the_memory = Measurement::from_ocr("auto");
        let mut differing_settings = with_a_differing_row();
        assert!(
            !differing_settings.hold_to_floor(&memory("irs_f1040", was), Some(&unlike_the_memory))
        );
        assert_eq!(
            differing_settings.floor, None,
            "without matching settings only the floor rule may refuse a document"
        );
        // Held to something, and the report says what and why — a panel a reader can
        // tell "held to the floor alone" on rather than one that looks identical to a
        // whole-ratchet pass.
        let held = differing_settings.held.clone().expect("held is recorded");
        assert_eq!(held.rules, HeldRules::Floor);
        assert!(
            held.why.contains("--ocr auto") && held.why.contains("--ocr never"),
            "the sentence names both measurements: {}",
            held.why
        );
    }

    #[test]
    fn a_document_at_or_above_the_floor_is_shown_as_it_always_was() {
        let was = remembered(25, 327);
        // The same form, still whole: no floor, and the rate is quiet too.
        let mut whole = form_report("test-data/real/irs_f1040.pdf", 327, 25);
        assert!(!whole.hold_to_floor(&memory("irs_f1040", was.clone()), None));
        assert_eq!(whole.floor, None);
        // At or above the floor, but *held* — and the report says to what, so a panel
        // that passed and a panel that was never held are not the same silence.
        assert_eq!(
            whole.held.as_ref().map(|held| held.rules),
            Some(HeldRules::Floor)
        );
        assert!(Numbers::of(&whole).worse_than(&was).is_empty());

        // More items than the memory remembers is the improvement the floor must allow.
        let mut more = form_report("test-data/real/irs_f1040.pdf", 400, 25);
        assert!(!more.hold_to_floor(&memory("irs_f1040", was.clone()), None));
        assert_eq!(more.floor, None);

        // A document the memory never measured stands under no floor: there is nothing for
        // it to fall from — and nothing held it, which the report says too. (The window
        // compares a user's own documents this way, with no memory beside the reference.)
        let mut unknown = form_report("test-data/real/mine.pdf", 12, 3);
        assert!(!unknown.hold_to_floor(&memory("irs_f1040", was.clone()), None));
        assert_eq!(unknown.floor, None);
        assert_eq!(unknown.held, None);

        // ...and neither does a document the memory placed nothing for, however hard this
        // reading places nothing: 0 is not a fall from 0.
        let mut nothing = form_report("test-data/real/dummy.pdf", 0, 0);
        assert!(!nothing.hold_to_floor(&memory("dummy", remembered(0, 0)), None));
        assert_eq!(nothing.floor, None);
        assert_eq!(
            nothing.held.as_ref().map(|held| held.rules),
            Some(HeldRules::Floor)
        );
    }

    #[test]
    fn the_floor_is_found_beside_the_references_the_memory_is_about() {
        // `compare-baseline.json` lives in the folder that holds the references it remembers
        // (`test-data/golden/compare-baseline.json` against `test-data/golden/docling/`), so
        // the window finds it without being told where it is — and a comparison against a
        // reference with no memory beside it is simply not held to one.
        let dir = scratch("memory");
        let references = dir.join("docling");
        std::fs::create_dir_all(&references).unwrap();
        assert_eq!(
            Memory::beside(&references),
            Ok(None),
            "a reference with no memory beside it is not an error"
        );

        let written = Memory {
            note: "written by hand".to_string(),
            measured: Measurement {
                ocr: "never".to_string(),
                ..Measurement::default()
            },
            documents: BTreeMap::from([("irs_f1040".to_string(), remembered(25, 327))]),
        };
        std::fs::write(
            dir.join(Memory::FILE),
            serde_json::to_string(&written).unwrap(),
        )
        .unwrap();

        let found = Memory::beside(&references)
            .expect("read the memory")
            .unwrap();
        assert_eq!(found, written);
        assert_eq!(found.remember("irs_f1040"), Some(&remembered(25, 327)));
        // A single reference file looks the same way: the folder it is in, then the one above.
        assert_eq!(
            Memory::beside(&references.join("irs_f1040.json")),
            Ok(Some(written.clone()))
        );
        // The folder of references itself may hold it too.
        std::fs::write(
            references.join(Memory::FILE),
            serde_json::to_string(&written).unwrap(),
        )
        .unwrap();
        assert_eq!(Memory::beside(&references), Ok(Some(written)));

        // A file that is there and is not a memory is *said*, not passed over: a floor that
        // quietly did nothing would be worse than no floor at all.
        std::fs::write(references.join(Memory::FILE), "{not a memory}").unwrap();
        let problem = Memory::beside(&references).expect_err("a broken memory is reported");
        assert!(problem.contains("is not a compare baseline"), "{problem}");
    }

    /// A scratch directory that removes itself.
    fn scratch(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pdfe-ui-diff-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        path
    }
}
