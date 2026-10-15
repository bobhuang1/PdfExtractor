# Project Progress

**Project**: PDF & Office Document Extractor (Docling-compatible)
**Started**: October 11, 2026
**Last Updated**: October 13, 2026
**Language**: Rust (toolchain verified: cargo 1.99.0, rustc 1.99.0)
**Approach**: Hybrid — reuse open-source infrastructure where it is commodity (DEFLATE, ZIP/XML, image codecs, Tesseract); implement the domain logic (PDF object model, content-stream interpretation, image decoding, layout, export) ourselves.

**OCR is a required feature** and is wired into the pipeline, on by default.

## Verified status (authoritative section)

Every row below was actually executed; exit statuses were captured with `pipefail`/`PIPESTATUS`.

| Check | Command | Result |
|---|---|---|
| Build | `cargo build --workspace` | ✅ clean, **0 warnings** |
| Lint | `cargo clippy --workspace --all-targets` | ✅ clean, **0 warnings** |
| Lint (OCR off) | `cargo clippy --workspace --all-targets --no-default-features` | ✅ clean |
| Tests | `cargo test --workspace` | ✅ **366 passed, 0 failed** (38 suites) |
| CLI end-to-end | `cargo test -p pdfextractor-cli --test cli_test` | ✅ **21 passed** — the real binary run as a child process, including **a metafile drawing and an embedded object's preview coming out as named PNGs**, and `basicapi.pdf`'s three `page N / 3` lines coming out as the **furniture layer** while staying in the body order |
| Page furniture (unit) | `cargo test -p pdfextractor-layout --lib` | ✅ **9 new tests, 37 in the crate** — a page number down the margin of every page is `page_footer` on all three and the body beside it is untouched; a running head across four pages is `page_header`; a margin line on **one** page only (`irs_f1040.pdf`'s small print) stays a paragraph, because a pass that can miss furniture must never invent it; the same words **mid-page** are not furniture however often they repeat; one page cannot make furniture of itself, however many times the line is printed on it; `page 1 / 3` and `page 12 / 30` are one piece of furniture (only the digits differ), as are `Form 1040 (2025)` and `(2026)`; relabelling keeps the text where the page printed it and running again changes nothing; an `13` read as a `section_header` at the foot of a page becomes a `page_footer` with no level and no marker; and a **sideways block inside the text** is still the document's, while one in the page's own outer margin (the 20 pt arXiv stamp) is `page_header` |
| Page furniture over the corpus | `cargo test -p pdfextractor-layout --test real_corpus_test` | ✅ **5 passed** — the document-wide pass run over every real PDF: **86** items across the 7 PDFs are labelled, nothing in any page's foot band is a heading or a list item, and every bare number a document prints at the foot of two or more pages is a `page_footer` (the pass may not quietly stop firing: the corpus total must stay ≥ 60) |
| Page furniture vs Docling | the labels above, against `test-data/golden/docling` | ✅ **84 of the 86** are items Docling labels `page_header`/`page_footer` too (98% precision): all 15 on `arxiv_attention`, 3/3 `basicapi`, 11/11 `math_batchnorm`, 10/10 `math_gan`, 14/14 `math_layernorm`, 31/31 `math_seq2seq`; the two that differ are Docling's own omissions or its own reading — `math_batchnorm` page 11's `11` (Docling has no furniture for page 11) and the IRS form's sideways `Form 1 0 4 0` masthead, which Docling calls a `section_header`. Nothing is claimed on `dummy.pdf` or `tracemonkey.pdf`, which have no repeated margin line at all. The other side: 6 of Docling's 90 are left as ours read them — `arxiv_attention`'s page-number-less page 1 conference line, and the five small-print lines of the IRS form, four of which stand on a single page |
| Page footnotes (unit) | `cargo test -p pdfextractor-layout --lib` | ✅ **49 in the crate** — a note under the page's rule is a `footnote` while the author lines above it that use its markers are not; a **numbered list** at the foot is not one, because layout took its marker out of the text; a rule with no marker, and a marker with no rule, are not notes; a marker mid-page is a sentence; a marker the body does not use is not one (`arxiv_attention`'s conference line); `footnote` is deliberately **not** a furniture label, so the note stays the document's text; and the page's **rules** are what keep a note block out of the table list — a marker-and-text block that passes every table test becomes a table when the page drew no rule over it, stays the notes when the page did, and is not notes when the rule stands in the body rather than the foot (`a_note_block_under_the_pages_rule_is_not_a_table`, `the_same_rows_with_no_rule_over_them_are_a_table`, `a_rule_above_a_note_block_high_on_the_page_does_not_make_it_notes`) |
| Page footnotes over the corpus | `cargo test -p pdfextractor-layout --test real_corpus_test` | ✅ **6 passed** — the pass over every real PDF finds exactly **23** notes, held to a committed list (document, page and opening words) so a note gained or lost is a deliberate edit; each one is checked to stand **under a rule**, so a note found by something else fails the test rather than the list; the four pages whose note block used to be read as a table (`math_seq2seq` page 4, `math_layernorm` pages 6–8) come back as the notes they are; and the four the symbol fonts were hiding (`math_gan`'s `∗`/`†`/`‡` author notes, `math_seq2seq`'s `∗CIFAR Senior Fellow`) now come back with their markers |
| Page footnotes vs Docling | `cargo test -p pdfextractor-cli --test docling_golden_test our_output -- --nocapture` | ✅ **5/5** of `arxiv_attention.pdf`'s notes, **5/5** `math_layernorm`, **9/9** `math_seq2seq`, **4/4** `math_gan`, **0/2** `tracemonkey`, **2/2** `calibre_demo.docx` (a DOCX, where a note is structural) — of the **27** notes the corpus's references hold (25 in the five PDFs, 2 in the `calibre_demo.docx`), **25 are labelled**, **2 are held as the paragraph they were read as**, and **not one is missing** from the output. Two marks did it: the page's **rule** above a note block handed the eight that used to be read as a **two-column table** back to the pass (`math_seq2seq` 3, 4, 6, 7, `math_layernorm` 2–5), and the **font program's own encoding** named the markers the other four open with (`math_gan`'s `∗`/`†`/`‡` author notes and `math_seq2seq`'s `∗CIFAR Senior Fellow`). The four PDFs with no reference notes (`basicapi`, `dummy`, `irs_f1040`, `math_batchnorm`) get none. The gap is measured, not asserted away: `FOOTNOTE_BUDGETS` carries **two floors** per document — the notes *held* and the notes *labelled*, every one the whole of Docling's count — and the comparison prints every note it read as text or could not find |
| UI logic | `cargo test -p pdfextractor-ui` | ✅ **105 passed** (84 unit + 7 process + 5 compare + 9 corpus report) — settings → argv, the `--progress` parser, the event payload shapes, spawning/streaming/stopping a real process, the comparison of real output against the committed Docling references, and the floor the window holds each document to (see the row above) |
| Compare mode tests | `cargo test -p pdfextractor-ui --test compare_test` | ✅ **5 passed** — the real CLI run over corpus documents and compared with `test-data/golden/docling`: three equations read as three **notation** rows and nothing one-sided (`math-nary.docx`), all five drawings of `VariousPictures.docx` answered by one of ours, `mathtype.docx`'s OLE preview shown by both sides, a document with no reference reported as such, and the three PDFs whose references carry geometry — 77 rows paired by position and **60 placed rows standing for 369 items** across them, every placed row being the reference's items with no letters in them that stand along one line of one page, named together with the extent they cover, on a named page, with its box on the note |
| The containment rule (unit) | `cargo test -p pdfextractor-ui --lib` | ✅ **8 tests** — a run of reference items inside one block of ours is one **contained** row (and the whole run is kept on the row), a single contained item is a containment rather than "text differs", words in another order are not containment, a piece of one of our words is not, an item with no characters is not, a label the readings disagree about survives a containment, and letter-spaced text of ours still matches |
| The fold (unit) | same | ✅ **3 tests** — an item the reference kept apart from the run is folded onto the block that holds it (its `Amount withheld` sitting between two halves of one of our lines), a row the readings *agreed* on gains the items inside our block and becomes a containment, and a wording difference (our `12` against the reference's `13`) is **not** hidden behind the fold |
| A form line's dotted leaders (unit) | `cargo test -p pdfextractor-ui --lib` | ✅ **5 new tests, 69 in the library** — a line read as two word fragments with leader items between them is **one row** (four items, two by words and two by place, and no fragment left one-sided), a leader standing *among* the fragments of a line we read is taken while one standing alone where we read a caption is **not** (it is the reference's own reading, and the row the position join makes of them says so), a leader on another page is never taken, an item with words we do not hold keeps its own row however well its box fits inside our block, and the reference's items on a row stand in the reference's order rather than behind the label the alignment paired |
| A form's remaining leaders (unit) | `cargo test -p pdfextractor-ui --lib` | ✅ **5 new tests, 76 in the library** — a form line our reading produced **no block at all** for (two runs of dots and the rule between them) is answered by **placement** rather than listed: a row naming what it is and the page and box it lies at, none of them in the column of text we did not read (the items of such a line are **one** row — see the row below); **the box the panel draws** is on the row as a share of the page's own shape (a leader from 50,560 to 300,580 of a 600×800 page is a twelfth in from the left and seven tenths down, wholly inside the outline so the drawing cannot spill out of it), while a row that is not a placement carries none, because a box nothing draws is bytes on every row of every document; and a page whose **size the reference never states** is named (its box in points in the note, `page 1, box 50,560–300,580`) and left undrawn rather than given a shape invented for it; an item with no letters in it on a page we never read is placed the same way rather than taken onto the line of ours it does not belong to (the test that used to assert `[ours-only, theirs-only]` for it now asserts `[ours-only, placed]`); an item with *letters* in it and no block of ours beside it stays **only the reference**, because text we did not read is text we did not read however well it stands alone; and a letterless item with **no box** — every flow format — is compared exactly as it was |
| A form line's items as one placed row (unit) | `cargo test -p pdfextractor-ui --lib` | ✅ **4 new tests, 79 in the library** — a form line our reading produced **no block at all** for (two runs of dots and the rule between them) is **one row**, not one per item, and so is any line the reference cut into items: the items of one line of a form are **one row**, the reference's two runs of dots and the rule under them stand on one line of one page and are placed together, the row naming `3 items with no letters in them along one line of page 1, box 50,560–550,581` and drawing that same extent as fractions of the page; items on **another line** of the same page are their own row with their own extent, even when they are next in the reference's order; a block of **ours** read on that line (holding nothing of the reference's) does not come between two leaders, so they are still one run — the row's extent covering it, because that is what the line looks like — while an item of the **reference's own** does come between them and keeps them apart, because they are not neighbours in Docling's order; and the header's `placed` counts *rows*, so three items named together are one, not three |
| Placed items by page (unit) | `cargo test -p pdfextractor-ui --lib` | ✅ **1 new test, 80 in the library** — the placed items of a document are grouped by the page they lie on for a **per-page drawing**: the *items*, not the rows, so a page shows its whole leader pattern rather than a row's merged extent; pages come in ascending order with the reference's own order kept within each; each box is the item's own place in fractions of the page and lies inside it; and the picture is built from the rows *before* they are cut to `MAX_ROWS`, so a long form's page is whole where its rows are not. A page whose size the reference never states is drawn nowhere rather than given a shape invented for it — the row-level test `a_placement_with_no_page_size_is_named_but_not_drawn` now asserts the page list is empty too. Measured: the three PDF references' **369** placed items are drawn as **6** page pictures (the corpus's 69 placed rows / 380 items as **7** pictures holding 380 boxes) |
| The boxes in the score (unit) | `cargo test -p pdfextractor-ui --lib` | ✅ **5 new tests, 64 in the library** — two pictures the readings list in opposite orders are paired by their **boxes** (and the sizes on the rows then agree), two *identical* pictures the same way rather than accusing each of being somewhere else, a block both readings read the same words of in different places (or on different pages) is a **differs** row naming both places, and blocks in the same place whose words link nowhere are still two rows joined by position |
| Compare by page and box (measured) | `cargo test -p pdfextractor-ui --test compare_test pdfs_are_compared -- --nocapture` | ✅ **1 passed**, **77** rows joined across the three references — of the rows the alignment leaves one-sided, **60** are paired by position on `tracemonkey.pdf` (446 rows), **11** on `arxiv_attention.pdf` (334), **6** on `irs_f1040.pdf` (500): the same block in the same box, in a different order. Only leftovers are joined now, because the boxes also take part in the score (see the row above and the corpus row below), so 265 → 138 rows need the boxes at all — and none of the 77 is a form line's leader, which the fold answers for on its own line's row |
| Compare mode over the whole corpus | `cargo test -p pdfextractor-ui --test corpus_report_test -- --nocapture` | ✅ **1 passed**, 17 s — all **37** documents converted in one CLI run and compared with their references, worst first: `math_gan` 74% of its rows not simply agreed, `arxiv_attention` 66%, `math_batchnorm` 63%, `math_seq2seq` 60%, `tracemonkey` 51%, `math_layernorm` 45%, `basicapi` 30%, `irs_f1040` 13%; 0 documents without a reading or a reference, 138 rows paired by position, **288** rows where the reference cut one of our blocks finer (252 before the leaders of a form line were folded with them), **69** rows placed where our reading read no line, **standing for 380** of the reference's items, and 773 rows only the reference has (1,454 before; on `irs_f1040` alone 682 → 416 → **89**, the 327 that left it now placed as **25** rows, one per line of the form). Labelling the page's furniture moved **83** rows out of `differs` and into `same` across the six papers whose pages carry numbers — 12 of the 37 documents, no field of any other one touched (`math_seq2seq` 155 → 125, `arxiv_attention` 112 → 100, `math_layernorm` 104 → 90, `math_batchnorm` 98 → 87, `math_gan` 83 → 73, `basicapi` 4 → **1**) — because a row whose readings agreed about the words used to disagree about the *label* (`paragraph` against `page_footer`) and now does not |
| Corpus ratchet (the report may only improve) | `cargo test -p pdfextractor-ui --test corpus_report_test` | ✅ **1 passed** — `Every document is at or better than the baseline it remembers (…/test-data/golden/compare-baseline.json, 37 document(s))`, with the baseline regenerated for the leaders of a form line: exactly **8** fields moved across 4 documents and every one of them an improvement (`irs_f1040` rows 766 → 500 and only-the-reference 682 → 416, `arxiv_attention` 369 → 334 and 64 → 29, `tracemonkey` differ 172 → 140 and contained 23 → 55, `math_seq2seq` differ 159 → 155 and contained 48 → 52); no document gained a `differs`, a one-sided row, or lost a character. An earlier form of the rule, which let an item with no letters in it be taken by the *nearest* block rather than the one it lies most inside, was refused by this ratchet and by the diffs it prints: `tracemonkey: rows only ours 89 → 93`, `math_seq2seq: rows only ours 99 → 100`. Deliberately doctored baselines fail it, all at once and with the counts named: `math_gan: rows that differ 80 → 83, character agreement 100.0% → 99.9%`, `dummy: not in the baseline (a new document: …)`, `nonexistent: in the baseline, but this run compared no such document` → **exit 101**; a baseline *better* than the run, or one with `same`/`contained`/`notation`/`joined` moved, still passes, so an improvement is never blocked; and with the file removed the run fails with `no baseline at … Create it from a run you have looked at: CORPUS_BASELINE=update …`. Regenerated once more for the **placement** rule, where exactly **4** fields moved and every one of them the same improvement — `theirsOnly` 416 → **89** on `irs_f1040`, 138 → **97** on `tracemonkey`, 52 → 41 on `math_seq2seq`, 29 → 28 on `arxiv_attention`: the four documents with an item that has no letters in it standing where our reading read no line at all, 69 such rows across the corpus standing for 380 items (one row per line of a form after the collapse below), and no other field of any document moved by so much as one. Regenerated a third time for **page furniture**, where exactly **12** fields moved across **6** documents and every one of them an improvement of the same kind — `same` up and `differs` down, never the reverse (`differs` `math_seq2seq` 155 → **125**, `arxiv_attention` 112 → 100, `math_layernorm` 104 → 90, `math_batchnorm` 98 → 87, `math_gan` 83 → 73, `basicapi` 4 → **1**; `same` 15 → 45, 55 → 67, 60 → 74, 5 → 16, 19 → 29, 3 → 6 in the same order), with no other field of any document moved and no document gaining a one-sided row. Regenerated a fourth time to put the **placed rows on the ratchet**, because nothing else in the file could see them come apart: **4 new fields on all 37 documents** (`placed` 69 rows, `placedItems` 380, `placedBoxes` 380, `placedPagePictures` 7 across the corpus) and, with them, a rule — the items a row stands for may not fall as a *rate*, so the same items in more rows fails, and so does the same boxes in more page pictures; the fields are compared by cross-multiplication, so it is exact on integers and a document with nothing placed (0 rows, 0 items) is never a regression from nothing. Also corrected by that run: `rows` on the four documents whose leaders had been **collapsed** after the baseline was last written, which had left the field counting the rows of the reading before the collapse (`irs_f1040` 500 → **198**, `tracemonkey` 446 → 439, `math_seq2seq` 373 → 371, `arxiv_attention` — its `rows` unchanged at 334), and the two further improvements the **footnote pass** had already earned and never recorded (`arxiv_attention` `differs` 100 → **97** with `same` 67 → 70; `math_seq2seq` `differs` 125 → **123** with `same` 45 → 47). Every moved field is an improvement and none is a one-sided row. The un-collapse itself was shown to fail end to end, not only in the unit tests: a baseline that remembers `irs_f1040` fully collapsed (1 row for its 327 items, 1 page picture) against this run prints `irs_f1040: form line(s) placed: 1 row(s) for 327 item(s) → 25 row(s) for 327 item(s), form pattern drawn: 1 picture(s) for 327 box(es) → 2 picture(s) for 327 box(es)` and exits **101**. Regenerated a fifth time for the **fonts' own encodings**, where **28 fields moved across 5 documents** and the movement is the trade this row exists to make visible: the reading is right and the *reference* is the one that substitutes. Naming a TeX glyph (`nabla`, `summationdisplay`, `bracketleftBig`, `epsilon1`) replaces the Latin-1 byte the code happened to land on, and a wrong character is still an alphanumeric the reference may hold somewhere, so `agreement` fell on three documents — `math_gan` 99.9% → **99.7%**, `math_seq2seq` 100.0% → **99.9%**, `tracemonkey` 99.1% → **99.1%** — while `math_layernorm` *rose* 0.996 → **0.999**, `math_batchnorm` 0.99920 → **0.99935**, and `math_batchnorm` gained one reference-only row (71 → 72) and `math_seq2seq` one only-ours (97 → 98). Against that, `differs` fell on **all five** documents (`math_layernorm` 94 → **84**, `math_seq2seq` 125 → **121**, `tracemonkey` 140 → **138**, `math_batchnorm` 87 → **86**, `math_gan` 76 → **75**), `rows` moved on four (`math_layernorm` 391 → **372**, `math_seq2seq` 369 → **366**, `math_gan` 225 → **221**, and `math_batchnorm` 277 → **278**, the one row it gained), `theirsOnly` fell on three (`math_layernorm` 78 → **62**, `math_gan` 14 → **13**, `math_seq2seq` 37 → **36**) and `oursOnly` on one (`math_gan` 94 → **91**, while `math_seq2seq`'s rose by its one row), and the reference's own text keeps the substitute: `math_gan`'s reference holds 17 more `r` than we do and no `∇` at all — the same 17 glyphs. The trade is recorded in `test-data/golden/README.md` under the two readings of the TeX names. A **sixth rule** was added (the baseline's numbers did not move): a form's placed items are now ratcheted as a **count per document**, not only as a rate. The rate (`items × rows`) refuses a line that came apart, but it is kept by any *proportional* fall — half the items in half the rows is the same ratio and half the panel — and at the limit a document whose form is answered no more at all is `0` rows for `0` items, which `0 * 327 < 25 * 0` is false for, so the items can leave the panel entirely with nothing moving. `placedItems` may now only fall for a document that placed nothing to begin with, and the failure names the count (`form item(s) placed: 327 → 160 item(s) in 12 row(s)`). The floor yields to nothing else, because no other count here can tell "answered as text instead" from "lost": that reading change has to be argued for by regenerating the baseline, like every other deliberate trade. Three unit tests hold it — a halved panel whose ratio is kept, `0` for `0` where a form was remembered, and the two directions the floor must not stand on (nothing placed before; items that only rose). Shown **end to end**, on the real corpus and the real baseline: a baseline doctored to remember a form for `dummy.pdf`, which this run places nothing for (1 row, **5 item(s)**), and `irs_f1040.pdf`'s panel twice over (50 rows, **654 item(s)** — the same ratio) against this run reports exactly those two and exits **101**: `dummy: form item(s) placed: 5 → 0 item(s) in 0 row(s)` and `irs_f1040: form item(s) placed: 654 → 327 item(s) in 25 row(s)`. Neither the rate rule nor the picture rule fired on either document — with the old rules both panels could have gone and the run would have said nothing |
| The placed rows on the ratchet (unit) | `cargo test -p pdfextractor-ui --test corpus_report_test` | ✅ **8 in the module, 9 in the suite** — the un-collapse is refused with both counts named (`25 row(s) for 327 item(s) → 327 row(s) for 327 item(s)`, the IRS form read dot by dot again, which moves no other number in the file), the same items in *fewer* rows passes (25 for 327 against 327 for 327, and against 40 for 327), placing **more** items is not a regression (69 rows for 380 items against a document with nothing placed — 0 rows and 0 items, which must never be a regression from nothing — and 120 for 800 against 60 for 369), a run that places more items *and* comes apart is still refused, the per-page drawing is held to the same rule (`2 picture(s) for 327 box(es) → 25 picture(s)` fails, one picture for the same boxes passes), **and the items themselves are a floor of their own: half the items in half the rows (`327 → 160 item(s) in 12 row(s)`) fails although the ratio is kept, `0` items in `0` rows where the baseline remembered a form fails although `0 * 327 < 25 * 0` is false, and the floor stands on neither a document that placed nothing before nor items that only rose**; the rule is the library's (`diff::Numbers`, which the window's `Report::hold_to_floor` applies), so a test here pins the two together: the sentence a refused panel carries is the one this ratchet prints, and the halved panel earns that complaint and no other |
| The floor in the window | `pdfextractor-ui --selftest <doc> --selftest-ref test-data/golden/docling` | ✅ **exit 0**, window and all — the ratchet's **rule and its numbers moved into the library** (`crates/ui/src/diff.rs`: `Numbers` with `of`/`worse_than`/`below_placed_floor`, `Memory`, `Report::hold_to_floor`), so the panel holds a document to the same floor the corpus report fails on rather than to a second copy of it; the corpus suite now imports them, and regenerating its baseline from the moved code left the file **byte-identical** (`cmp` clean, `md5` `b3ed2f7e…` unchanged before and after the move) — the refactor measures the same numbers and writes the same JSON, so nothing but the panel changed. The window finds the memory *beside the references it is about* (`test-data/golden/docling/` → `test-data/golden/compare-baseline.json`, or the folder above a single reference file) with no flag of its own, and a document whose placed items have fallen is **refused**: against the committed memory `irs_f1040.pdf` reports `floors=0 diff-rows=198 placed-drawn=25 placed-pages=2` with the verdict it always had (`same=10 differs=49 contained=11 ours-only=14 theirs-only=89 placed=25 paired-by-position=6`, 99.4%), while with the memory doctored — `irs_f1040` remembered as 50 rows for **654 items** (the same ratio, kept) and `dummy` remembered with a form (1 row, **5 items**) — the same two runs report `diff-verdict="below-floor" diff-rows=0 placed-drawn=0 placed-pages=0 floors=1 floor="form item(s) placed: 654 → 327 item(s) in 25 row(s)"` and `floor="form item(s) placed: 5 → 0 item(s) in 0 row(s)"`: the comparison is *withheld*, no row and no page picture drawn, where neither the rate rule nor the picture rule would have said anything at all. The file was restored byte-identically afterwards (`cmp` clean), and the document run with `--ocr auto` against a memory measured with `--ocr never` did **not** fire spuriously — the floor is a property of the reading rather than of the settings, so it stands whatever the run was measured with; the *other* rules are the ones that ask whether the two measurements match, which is what the window and the command line now do (the two rows below). Rendered and looked at through the real front end in a browser (`dist/index.html?mock=1`, driven with the page's own mock): a refused panel draws its head as `irs_f1040.pdf · below the floor · form item(s) placed: 654 → 327 item(s) in 25 row(s)` with the failure in a red-ruled banner and **0** rows and **0** page maps in the DOM, while the same report with no floor draws its row and its page picture and reports `floors=0`. **4 new unit tests** (84 in the library) hold the reduction (`Numbers::of` counts the items from the rows — the items of a line of a form are one row — and the boxes from the drawing), the refusal (the halved panel the rate cannot see is `Some("form item(s) placed: 327 → 160 item(s) in 12 row(s)")`, and it is the *only* complaint), the documents that keep their comparison (whole, more items, unknown to the memory, and 0 for 0), and the memory's own discovery (beside the folder of references, beside a single reference file, absent → `Ok(None)`, present but unreadable → `Err`, which the panel *says* rather than passing over); `1 new test` in the corpus suite (**9** there) asserts the sentence the panel carries is the ratchet's own, character for character |
| The baseline records how it was measured | `cargo test -p pdfextractor-ui --test corpus_report_test` | ✅ **1 passed** — `compare-baseline.json` carries a `measured` block beside its `note` and its 37 documents (`by` the test that measured it, `corpus` `test-data/real`, `ocr` `never`, and room for any other setting worth recording), written by the run that writes the numbers and **checked** when the file is read: a baseline whose `measured` disagrees with the run reading it fails naming both (`compare-baseline.json was written by a different measurement … remembered: … this run: …`) instead of being silently held against numbers it cannot speak for. That block is what tells the window and the command line how much of the ratchet a run is held to — every rule when the run's settings match the remembered ones, the floor under a form's placed items when they do not (`Report::hold_to_floor`, and the row below for the gate) — and it is the memory's own story for a human who opens the file. `1 new unit test` in the library (`a_memory_measured_otherwise_is_refused`) and the corpus suite's own check hold it; the file regenerated from the run that added the block was otherwise **byte-identical** apart from the block itself |
| Compare in the command line (a fallen panel fails a build) | `cargo test -p pdfextractor-cli --test compare_mode_test` | ✅ **8 passed**, the whole suite in **0.5 s** — the window's Compare hold at the end of a command line (`pdfextractor-cli --compare <reference>`, in `crates/cli/src/compare.rs` through the same `pdfextractor_ui::diff` the panel calls): every finished document is compared with its reference, held to the memory beside that reference, and the run **exits 1** naming every refused document and what fell. Shown end to end on `dummy.pdf` (one row, disagreeing, nothing placed — converted eight times in half a second): a memory remembering a form for it (1 row, **5 items**) refuses with `form item(s) placed: 5 → 0 item(s) in 0 row(s)`; a memory remembering no disagreeing rows refuses with `rows that differ 0 → 1` when the run is measured the way the memory says it was (`--ocr never` against `measured.ocr: never`) and the **same** memory passes with `--ocr auto`, which is the settings rule reaching the gate; an exact memory passes saying `at or above its floor`; no memory beside the reference passes saying so (`no memory beside the reference: nothing is held to a floor`); a memory that cannot be read fails **before** any conversion (`is not a compare baseline`) — the test asserts no output was written — and a reference path that is not there is refused up front; and a document with no reference beside it fails as `not compared · no reference for this document`, because a gate that held nothing while claiming to hold everything is worse than no gate. To let the CLI link the comparison without the window, `pdfextractor-ui`'s desktop shell (Tauri, the file chooser) grew a `desktop` feature — on by default for the window, off for the CLI — with `tauri_build` gated in `build.rs` to match: `cargo tree -p pdfextractor-cli -i tauri` is empty |
| The panel says which rules held a document, and why | `cargo test -p pdfextractor-ui --lib` + `pdfextractor-ui --selftest test-data/real/irs_f1040.pdf --selftest-ref test-data/golden/docling` | ✅ **85 in the library**, selftest **exit 0** — `Report::held` records which of the ratchet's rules held a document and why (`Held`/`HeldRules`: **ratchet** — every rule, because the run's settings match the memory's `measured` block — or **floor** — only the floor under a form's placed items, because OCR moves the counts that measure how the two readings agree), `Some` on every document the memory has whether or not anything fell, `None` where there was nothing to hold it to. The panel shows the sentence under its header (`Held to the memory: only the floor under a form's placed items applies: this run was measured with --ocr auto and the memory with --ocr never, and OCR moves the counts that measure how the two readings agree`), on a refused panel as well, so a whole-ratchet refusal and a floor-only one are told apart on the panel itself; the self-test reads it back as `held=floor` beside the same numbers as before (`… diff-rows=198 floors=0 held=floor`). **3 unit tests extended**: the halved panel records `Floor` with the no-measurement sentence; a matching run records `Ratchet` naming `--ocr never` and the same document measured `--ocr auto` records `Floor` naming both modes; an at-or-above document is held and says so, where a document the memory does not know is held to nothing. The page's key contract (`lib.rs`) pins `held` among the report's keys. Looked at through the real front end in a browser (`dist/index.html?mock=1`, driven with the page's own mock): both panels carry the line with `dataset.held=floor`, transparent 12px muted text — and one bug found by *looking* rather than by any test: the line's class was `held ${rules}`, so on a floor-only panel it also matched the refusal banner's `.diff > .floor` and painted red as though refused; the class is `held` alone now, with a comment in `app.js` saying why |
| One gate (the corpus report holds through the same Session) | `cargo test -p pdfextractor-ui --test corpus_report_test` | ✅ **9 passed**, 218 s — the ratchet's three holders now run their documents through one thing: `pdfextractor_ui::gate::Session`, opened once with the baseline the corpus test both reads and writes (`with_memory`, because a file the test may *rewrite* cannot also be the one that discovers it beside the references) and `--ocr never` as the measurement, each document compared and held with `session.compare` as it arrives, and the ratchet collecting the gate's own floor sentence per line rather than calling `worse_than` a second time. The complaint strings are **byte-identical** to the old direct path — shown by doctoring the committed baseline (`dummy` remembered with no disagreeing rows) against the real corpus: the routed run fails with `dummy: rows that differ 0 → 1` and **exit 101**, and the baseline was restored byte-identically afterwards (`md5` `827bcf23…` before and after). The baseline's own recorded measurement is still checked *before* any document is held (a mismatched file fails naming both measurements rather than silently downgrading the corpus to the floor alone), and the not-in-the-baseline / no-such-document completeness failures remain the corpus's own. The CLI's `compare.rs` is now a thin face on the gate — print and exit code — and its standing line reads `Report::held` instead of looking the document's stem up in the memory a second time; the window's `compare()` opens the same Session. **5 new unit tests** (90 in the library) hold the Session itself: a missing reference is refused, a broken memory is reported, no memory beside the reference is no error and holds nothing, the announce line's two shapes, and a no-memory session leaving a report untouched; the window selftest is unchanged to the character (`… floors=0 held=floor`, exit 0), which is the point — only who holds changed, not what is held |
| The gate in CI | `.github/workflows/compare-gate.yml` (on every push) | ✅ — one job: apt for the Tesseract bindings the CLI links even with OCR off (`libtesseract-dev libleptonica-dev libclang-dev` — `leptess` → `tesseract-sys`/`leptonica-sys` link the libraries and build their bindings with libclang), an `actions/cache` of the cargo registry and `target` keyed on `Cargo.lock`, `cargo build --locked --package pdfextractor-cli`, then the gate itself — `pdfextractor-cli --input test-data/real --output "$RUNNER_TEMP/corpus" --ocr never --parallel 4 --compare test-data/golden/docling` — whose exit code is the verdict and whose per-document lines are in the log either way. `--ocr never` is the measurement the baseline records for itself, so every rule of the ratchet applies rather than the floor alone. Verified by running the workflow's exact steps locally: the YAML parses with both triggers (`push`, `workflow_dispatch`), the `--locked` build is clean, and the gate exits **0** over the committed corpus in 3 m2 s at `--parallel 4` — 37 documents, all `at or above its floor`, header naming the memory (`37 document(s), measured with --ocr never`) — with the output written to a temp directory rather than beside the corpus; the failure direction is the one `cargo test -p pdfextractor-cli --test compare_mode_test` pins (a document below its floor exits 1 with the count named) |
| The gate speaks the panel's language, and CI runs the corpus report too | `cargo test -p pdfextractor-cli --test compare_mode_test` + both workflow commands | ✅ — two small things closed. **(1)** The CLI's standing line now names *which rules held* a document, in the panel's own words: `at or above the whole ratchet` when the run's settings match the memory's (every rule applied) and `at or above the floor` when only the floor could apply — read from `Report::held` rather than a second lookup of the stem in the memory, so a build log and a window say the same thing about the same document. Both branches are pinned: the `--ocr never` pass asserts the ratchet wording (test renamed to `a_document_at_or_above_the_whole_ratchet_passes_and_says_so`), the `--ocr auto` pass the floor wording (**8 passed**). **(2)** `.github/workflows/compare-gate.yml` gained a second job — the **corpus report** (`cargo test --locked --package pdfextractor-ui --no-default-features --test corpus_report_test -- --nocapture`), which holds the same documents through the same Session and can also see what one gate run cannot: a reference with no document, a baseline entry no run produced, and a baseline that says it was measured some other way. `--no-default-features` keeps the window out of the build, verified by running the job's exact command locally: **9 passed** in 223 s with no `gtk`/`webkit`/`glib`/`-sys` crate anywhere in the build-dependency tree (`tauri-build` is compiled as a pure-Rust build-dependency, but its `build()` never runs with `desktop` off), so the job needs only the Tesseract libraries the CLI links. The gate job re-run with the new binary exits **0** with all **37** documents `at or above the whole ratchet` |
| Compare mode E2E | `pdfextractor-ui --selftest <doc> --selftest-ref test-data/golden/docling` | ✅ **exit 0**, window and all — `diffs=1 diff-agreement="100.0%" diff-verdict="same=1 notation=0 differs=0 contained=0 ours-only=1 theirs-only=1 placed=0 paired-by-position=0 truncated=0" placed-drawn=0 placed-pages=0 diff-rows=3 floors=0` for `mathtype.docx` and `diff-agreement="100.0%" diff-verdict="same=0 notation=3 differs=0 contained=0 ours-only=0 theirs-only=0 placed=0 paired-by-position=0 truncated=0" placed-drawn=0 placed-pages=0 diff-rows=3 floors=0` for `math-nary.docx` (both DOCX: no boxes, so nothing to place, nothing to pair by position and nothing to draw), `diff-agreement="99.1%" diff-verdict="same=25 notation=0 differs=138 contained=56 ours-only=89 theirs-only=97 placed=34 paired-by-position=60 truncated=0" placed-drawn=1 placed-pages=3 diff-rows=401 floors=0` for `tracemonkey.pdf` (33 of its 34 places fall past the 400 rows a panel draws — the 401st row drawn is the "and N more" line — and its 34 items stand on 3 pages), and `diff-agreement="99.4%" diff-verdict="same=10 notation=0 differs=49 contained=11 ours-only=14 theirs-only=89 placed=25 paired-by-position=6 truncated=0" placed-drawn=25 placed-pages=2 diff-rows=198 floors=0` for `irs_f1040.pdf` (its `only the reference` count falls 416 → **89** with the leaders carried as places, all 25 of those places — one per line of the form, the items of a line named as one row — are boxes drawn on a page, and the form's whole leader pattern is **2** page pictures rather than 25 thumbnails; the boxes in the score, the containment verdict, the fold, the leaders taken by place and the joining all reaching the panel through the real window (its differ rows fall 172 → 140 and its containment rows rise 23 → 55 as the figure pages' glyph items of no letters are answered on the rows they lie along). The panel now agrees with the corpus report on the furniture too: `--selftest test-data/real/math_seq2seq.pdf --selftest-ref test-data/golden/docling` reports `same=45 differs=125` where the baseline remembers `same=15 differs=155`, and `--selftest test-data/real/basicapi.pdf --selftest-ref …` reports `same=6 differs=1` where it remembered `same=3 differs=4` — both **exit 0**, the same numbers the report prints for those documents, and `floors=0` on every document the memory remembers nothing placed for (the panel holds each of them to the floor the memory has for it, and none is below it; `tracemonkey`'s differ rows have since fallen 140 → **138** and its containment rows risen 55 → **56** with the fonts' own encodings, as the ratchet row above records) |
| UI smoke test | `pdfextractor-ui --selftest <dir>` | ✅ **exit 0** — `webview-ready`, `selftest-started run=1`, then `selftest-rendered state="5 / 5 documents · 4 converted · 1 failed · done" title="Failed" bar="100%" rows=[converted,failed,converted,converted,converted] logs=3` (the page's own DOM, after a real 5-document conversion) |
| UI stop test | `pdfextractor-ui --selftest <dir> --selftest-stop 2600` | ✅ **exit 0** — `selftest-stop after=2600ms stopped=Some(1)`, then `selftest-rendered … title="Stopped" bar="44.4%" rows=[queued,queued,converted,…]`; the 4 finished `*.json` all parse, **no temp files left** |
| Stream through the window | `--progress` records vs the page's DOM | ✅ plan → 5 records → finished, all delivered and drawn in order (see the smoke-test line above) |
| Real PDF corpus | `cargo test -p pdfextractor-pdf-parser --test real_world_test` | ✅ **9 passed** |
| Real DOCX corpus | `cargo test -p pdfextractor-docx-parser --test real_docx_test` | ✅ **14 passed** |
| Windows metafiles | `cargo test -p pdfextractor-metafile` | ✅ **12 passed** — the EMF/WMF players: the record walk, a hand-built drawing, and **all five `word/media` drawings of the corpus** |
| Real `.doc` corpus | `cargo test -p pdfextractor-doc-parser --test real_doc_test` | ✅ **8 passed** — container, piece table, field instructions, pictures, headers/footers/comments as furniture |
| Flow-format smoke | `cargo test -p pdfextractor-text-parser --test real_text_test` | ✅ **2 passed** — repository README and corpus README as real Markdown |
| Layout over the corpus | `cargo test -p pdfextractor-layout --test real_corpus_test` | ✅ **6 passed** — text integrity, box sanity, picture placement, reading order, the furniture pass and the footnote snapshot (each see their own rows above) |
| **Golden schema** | `cargo test -p pdfextractor-cli --test docling_golden_test` | ✅ **23 passed, 0 problems** — all 37 corpus documents held to the `docling-core` schema (fields per item class, enums, references, hierarchy) |
| **Golden vs real Docling** | same, with `DOCLING_PY=~/.venvs/pdfextractor-docling/bin/python` | ✅ **docling-core accepts all 37 documents**; **11 real Docling conversions compared, 0 mismatches** — text agreement 0.999–1.000, page counts equal, equation counts equal, no duplicated block |
| Full corpus E2E | `-i test-data/real -o /tmp/pdfe` | ✅ **37 of 37 converted** (36 documents + README.md), 0 failures |
| New formats E2E | `-i /tmp/fmt -o /tmp/fmt/out` (rtf/html/md/csv/txt) | ✅ all 5 converted with the right mimetype; headings, lists, tables and code blocks preserved |
| Layout output | same run, JSON inspected | ✅ **2,180 text items, 66 tables, 92 pictures (25 with an image), 8 titles, 183 section headers, 84 list items, 49 captions, 40 formulas, 26 footnotes, 21 page headers, 65 page footers, 1 code item** (the corpus README counts as a document, so its own prose moves these numbers; the table count fell from 70 and the note count rose from 22 as the eight ruled note blocks stopped being grids and the symbol fonts' markers came back) |
| Image artifacts | every `*_images/*` checked with `file(1)` | ✅ **25 images, 0 extension/format mismatches** — including the drawings played into PNGs |
| Preview E2E | `--preview -i test-data/real/arxiv_attention.pdf` | ✅ every page rendered, **297 block boxes**, 0 broken CSS declarations, 0 console errors |
| Stream lengths | `RUST_LOG=…=debug` over the corpus | ✅ 1 stream repaired from `/Length` (`basicapi.pdf` obj 20); **no document lost a character** |
| Box alignment (measured) | ink density inside each box vs a shifted control | ✅ **1.6× / 2.1×** more ink inside (OCR and text layer) |
| Preview fallback | run with `PATH` stripped of `pdftoppm` | ✅ degrades to text reconstruction, no `_pages` dir |
| E2E text PDF | `-i test-data/hello.pdf` | ✅ `"Hello World"` |
| E2E FlateDecode | `-i test-data/compressed.pdf` | ✅ both lines |
| **E2E OCR (scanned)** | `-i test-data/scanned.pdf` | ✅ **6 lines recognised** |
| `--ocr auto` on text PDF | no OCR run | ✅ text layer preserved |
| `--ocr never` on scan | text skipped | ✅ 0 items |
| Directory + `-p 4` | all three fixtures | ✅ all converted |
| Default output | no `-o` | ✅ writes `<input>.json` beside input |
| Parallel clamp | `-p 1000` | ✅ warns, clamps to 256 |
| Error paths | missing input / no docs / `.docx` | ✅ exit 1, clear messages |

### OCR evidence
`test-data/scanned.pdf` is a page that is *only* a JPEG (no text layer). The pipeline
extracts the embedded image, OCRs it, and emits line-level text items with boxes scaled
into PDF points:

```
'ACME Supplies'           bbox=(57.6, 62.9, 313.0, 92.6)
'Invoice Number 12345'    bbox=(59.5, 119.0, 300.0, 133.4)
'Date 2026-10-12'         bbox=(59.5, 157.4, 236.2, 171.8)
'Widget assembly 42.00'   bbox=(58.1, 224.6, 323.5, 243.4)
'Delivery charge 8.00'    bbox=(59.5, 258.2, 301.4, 277.0)
'Total Due 50.00'         bbox=(57.6, 330.7, 259.7, 348.5)
```

## System requirements for OCR

Installed and verified in this environment:

```
libtesseract-dev  libleptonica-dev  tesseract-ocr  tesseract-ocr-eng  libclang-dev
tesseract 5.3.4 | leptonica 1.82.0 | eng traineddata
```

`libclang` is needed because `leptess` uses `bindgen` at build time. `pkg-config` must be
able to find `tesseract` and `lept`. Builds without any of this still work via
`--no-default-features`, in which case OCR calls return a clear error.

## What works now

### PDF text pipeline
- **Lexer** (`crates/pdf-parser/src/lexer.rs`): byte-span-aware tokens, escape decoding,
  and — importantly — **stream payloads are skipped at the byte level**, never tokenised
  as PDF syntax.
- **Structural parser** (`parser.rs`): indirect objects and references, real payload
  slicing, `/FlateDecode`, page-tree traversal, `/MediaBox`, inherited `/Resources`.
- **Content interpreter** (`content.rs`): postfix operand/operator machine
  (`Tj TJ ' " Tf Td TD Tm T* TL BT ET`), text-matrix tracking, bottom-left → top-left
  conversion, font size and font name capture.
- **Character codes → text** (`encoding.rs`): `/ToUnicode` first, then `/Encoding`
  (`/BaseEncoding` + `/Differences`), then — for a font that declares none — the encoding
  the embedded Type 1 program carries, named through the glyph-name table (see *The encoding
  a font program carries* above).
- **Image decoding** (`images.rs`): DCTDecode (JPEG) passthrough, FlateDecode raw samples
  for DeviceGray/RGB/CMYK and 1-bit bilevel, plus **PNG and TIFF predictor** undo.
- **Docling export** (`core` + `json-exporter`): pages, text items, provenance bboxes
  (topleft), body tree refs, origin metadata and binary hash.

### Font advances from `/Widths` (new)
- **`encoding.rs` now builds a width table per font**: simple fonts read `/FirstChar`
  + `/Widths` (and `/DW` for the ones that fall outside), Type0 fonts read the `/W`
  array including its `c [w1 w2 …]` ranges, and missing values fall back to the
  average-glyph-width estimate that was used before.
- **The text interpreter charges the pen by real advance**, so a run's right edge is
  measured instead of assumed. The visible proof is `arxiv_attention.pdf`'s first
  paragraph: the previous estimate made the opening line 573 pt wide while its own
  continuation lines ended at 487.9 pt; with real advances the three lines form a
  block that lands on the same right edge (487.3 / 487.9 / 346.5).
- The estimator survives as a fallback (`font_has_real_widths` per line), so a PDF
  without `/Widths` still produces a box rather than none.

### The encoding a font program carries (new)
- **A symbol font's codes are not in the PDF.** A TeX note's markers are set in `CMSY`,
  whose PDF font dictionary declares no `/Encoding` at all — so every code fell through to
  Latin-1, and the font's own layout put `asteriskmath` (`\u{2217}`) at code 3 and
  `dagger`/`daggerdbl` (`\u{2020}`/`\u{2021}`) at 121/122. As Latin-1 those are a *control
  character*, which `sanitize` deletes, and the letters `y`/`z`: the page printed `∗`, `†`
  and `‡` and our text held nothing, `y` and `z`. That is why the footnote pass could not
  follow the markers of `math_gan.pdf`'s three author notes or of `math_seq2seq.pdf`'s
  `∗CIFAR Senior Fellow`.
- **The font program is the encoding of record (PDF 1.7, 9.6.6.2).** A Type 1 program
  carries its encoding in cleartext before `eexec`, as `dup <code> /<glyph> put` lines —
  and a subset font's array is the *subset's*, so the codes the content stream wrote are
  exactly the ones to look up. `parse_type1_encoding` reads that array and
  `build_font_decoder` installs it for any one-byte font that declares no `/Encoding` of its
  own; `/Differences` and `/ToUnicode` still win, and a code the program leaves `.notdef`
  still falls back to the base encoding. `FontFile` (Type 1) only: a CFF program is a
  different format, and a composite font's codes are its CMap's.
- **The glyph names are named.** `glyph_text` gained the Computer Modern names — Greek
  letters (`alpha`, `theta`, `epsilon1`), operators and relations (`minus`, `plusminus`,
  `element`, `negationslash`, `summationdisplay`, `integraldisplay`, `radical`, `nabla`,
  `proportional`), arrows, the card suits, and the plain character set (`five`, `colon`,
  `at`) — plus the rule that an extensible delimiter is one character at a larger size
  (`parenleftbig`, `bracketrightbt`, `braceleftBigg`).
- **The same table fixes a second reading.** Those names also arrive in `/Differences`,
  which is how the Type 1C (CFF) fonts of `math_batchnorm.pdf` are encoded: their `Θ`, `×`,
  `∈`, `←`, `∑` and `√` were Latin-1 letters and digits before and are the page's own
  symbols now.
- **The corpus moved with it, and the ratchet saw it.** 28 fields across 5 documents
  (see the ratchet row in the verified status), and the footnote count over the corpus went
  **22 → 26** while the notes Docling's references hold and we label went **21 → 25 of 27**.

### Layout analysis — Phase 5 (new)
`crates/layout` turns per-run geometry into the document a reader perceives. The
pipeline, in order:

1. **Rows** — spans sharing a baseline band merge, so a bold word inside a sentence
   (and each cell of a table row) is one row.
2. **Runs** — a row is cut wherever neighbouring spans are more than `1.5 em` apart.
   *This is the only place a column gutter can be detected*: the lines of two columns
   share a baseline, so the boundary is inside a row, never between rows.
3. **Tables** — consecutive rows with the same number of left-aligned short cells are a
   table, found **before** the cut so that no vertical whitespace band can tear the
   columns apart. A single span with wide internal spaces is deliberately *not* a table.
4. **Recursive XY-cut** — the page is split on whitespace bands. A band counts only if
   it is **uncovered**: no block may straddle it. (Without that check a band measured
   between two non-adjacent rows cuts a paragraph in half at a gap nothing occupies.)
   A vertical band is only a column gutter when **both sides run down the page**: the
   16 pt stamp in the far-left margin of `basicapi.pdf` page 3 is not a column, and
   splitting on it read the stamp *before* the heading at the top of the page.
5. **Paragraphs** — a line that stops short *while the next one starts full* ends a
   paragraph, which is how justified and ragged text both signal it; a small first-line
   indent also starts one (a large shift in the left edge is a centred line, not an
   indent). Every bullet or number starts its own item.
6. **Labels** — `title` is the first unnumbered block in the upper third of page 1 set at
   the largest size *in that region*, and it must be set horizontally: a sideways margin
   stamp is furniture even when its 20 pt outranks a 17 pt title. Larger-than-body text
   becomes `section_header` with its numbering depth; `Figure 3:` becomes `caption`; a
   leading bullet or number makes a `list_item`.
7. **Pictures** are emitted before the first block that starts below them, so a figure
   lands where it appears rather than being appended after all the text.

Markers are stripped from the text and kept in the `marker` field with an `enumerated`
flag, matching Docling's model. `heading_level` reads `2.1` as depth 2.

### PDF tables (new)
A table is a run of consecutive rows whose cells are spans with aligned left edges and a
short entry in each row, and it needs either three rows or two rows carrying a number (a
footnote marker beside its text is geometrically a two-row grid too). This is
deliberately conservative: a table missed stays readable prose, while prose misread as a
grid corrupts the text. The one exception is the page's own drawing: a marker-and-text
block at the foot of a page **under a rule** is handed on as the page's notes rather than
built into a grid, so the two-row case a note block always is does not have to be read as
a table at all (see the footnote pass below). 65 tables are found across the corpus — 12 in
`arxiv_attention.pdf`, 12 in `drawing.docx` (a genuine 12-table document), 7 in
`math_batchnorm.pdf` and 7 in `tracemonkey.pdf`.

### Page furniture (new — Phase 5)
A page's own line — its running head, its footer, its page number — is labelled apart
from the document's text, and it is a **separate pass over the whole document**
(`mark_furniture` in `crates/layout/src/furniture.rs`) for the reason that makes it
decidable at all: a page number says nothing on the page it numbers. What makes it one is
that the *next* page has one too, one higher, in the same place.

- **What decides it.** Two facts together — the block lies *wholly* inside the outer 7% of
the page's height, and the same text (every run of digits generalised to `#`, so `2`, `3`,
… and `page 1 / 3`, `page 2 / 3`, … are each one line) stands in that same band on **two or
more pages**. A page counts once however many times it prints the line.
- **Why not the margin alone.** The band that takes `arxiv_attention.pdf`'s page numbers
  (52 pt up from the bottom, 6.6% of the page) also takes the paragraph that opens
  `math_batchnorm.pdf`'s page 2 (31.6 pt from the top), and taking the *small* margin lines
  instead would not save it: the reference's own reading sets `optimization. J. Mach. Learn.
  Res.` smaller than that page's body text. Measured over the corpus, the rule fires on
  **86** items and every one of them is a page number or a running head — **84** are
  `page_header`/`page_footer` in Docling's reading too, which is 98% precision, and the two
  that are not are Docling's own omissions or its own reading (`math_batchnorm` page 11's
  `11`; the IRS form's sideways `Form 1 0 4 0` masthead, a `section_header` there).
- **What it fixes for a reader.** A bare number at the foot of a page was being read as a
  **section heading** — `arxiv_attention.pdf`'s `13` and `basicapi.pdf`'s `page 3 / 3` were
  in the outline of a data structure. The pass clears the level and the marker with the
  label, so a page number cannot be a heading at any depth.
- **A sideways stamp in the page's margin is the page's too.** Docling files
  `arXiv:1706.03762v7` — a 20 pt stamp down the left edge of page 1, larger than the title
  beside it — as a `page_header`; so do we, at the page level, where there is no second page
  to compare with (`MARGIN_STAMP` in `crates/layout/src/lib.rs`). A rotated block *inside*
  the text is still the document's.
- **Where the text goes.** Nowhere: the item keeps its place in the body order and its box,
  and is marked `content_layer: "furniture"` with the body as its parent — which is exactly
  how Docling writes its own page numbers. The alternative, a furniture *group*, would have
  moved a page number away from the page that printed it and lost it from any consumer that
  reads only the body. (`crates/core/src/builder.rs`, `add_furniture_on_page`.)
- **What it bought in the comparison.** 83 rows left `differs` for `same` across the six
  papers whose pages carry numbers, out of 12 fields moved in the ratchet and none the other
  way (see the table above). The rows were already agreeing about the *words*; it was the
  label that disagreed.
- **What it costs.** The pass only claims what repetition proves. It leaves
  `irs_f1040.pdf`'s small print (four of its five lines stand on a single page) and
  `arxiv_attention.pdf`'s page-1 conference footer as the text they were read as, and
  `tracemonkey.pdf` and `dummy.pdf` produce no furniture at all.

### DOCX list numbering (new — Phase 3)
- `word/numbering.xml` is parsed into `numId → level → {numFmt, lvlText, start}`,
  resolving the `w:num → w:abstractNum` indirection.
- Paragraphs carry `w:ilvl` and `w:numId`, and markers are expanded from `lvlText`
  (`%1.%2`), with counters that restart when a shallower item appears — so a nested list
  prints `1.`, `1.1`, `a)`, then `2.` in the right order.
- `decimal`, `lowerLetter`, `upperLetter`, `lowerRoman`, `upperRoman`, `decimalZero` and
  `bullet` formats are formatted; `numFmt="none"` prints nothing; `numId="0"` (how Word
  cancels numbering) is treated as absent.
- `w:lvlOverride` is applied per list: a nested `w:lvl` **replaces** that level's
  definition (so one list can switch from `1.` to `A)`), and `w:startOverride` restarts
  its counter at the value Word recorded, without touching the abstract scheme every
  other list shares.
- Word stores Symbol/Wingdings bullets in the private use area, so `\u{F0B7}` and friends
  are mapped to real glyphs instead of reaching the JSON as invisible characters.
- `calibre_demo.docx` now reports 19 of its 21 list items with a marker (`•`, then `1.`),
  where before there were labels but no markers at all.

### Stream filters (new — Phase 4)
`crates/pdf-parser/src/filters.rs` implements `/ASCIIHexDecode`, `/ASCII85Decode`,
`/LZWDecode` and `/RunLengthDecode` (plus their abbreviations), all wired into
`decode_stream` so content streams, CMaps and images all benefit. Every decoder returns
`None` on malformed input and the caller then keeps the raw bytes: a partially decoded
stream is worse than an undecoded one.

- **LZW** follows the specification's variable-width scheme (big-endian packing, nine-bit
  codes, clear/EOD, `/DecodeParms /EarlyChange`), including the `KwKwK` case where a code
  is used in the same step it is defined. It is checked against the specification's own
  example (`80 0B 60 50 22 0C 0C 85 01` → `-----A---B`) *and* against a reference encoder
  written in the test module, round-tripped at both `EarlyChange` settings.
- Not handled: `/DecodeParms` predictors on content streams (they are undone for image
  samples, which is where they actually occur).

### Pictures and figures (new)
- **`content.rs` tracks graphics state** through `q`/`Q`/`cm`, so the CTM in effect at a
  `/XObject Do` gives the image's **real on-page rectangle** instead of assuming it
  covers the page.
- **Vector figures are found** by clustering painted path boxes (`re`, `m`/`l`, `c`,
  `v`/`y` + a painting operator), filtered to 1.2%–50% of the page area and ≥24 pt a
  side. This is what finds the plots in papers that contain **no raster image at all**.
- Raster images are written beside the JSON in a `<name>_images/` directory, with a
  **truthful extension** per MIME type, real pixel `size`, and `dpi` derived from the
  placement box. Vector figures are emitted as picture items with a box and no `image`.
- **A DOCX drawing is rasterised** by `crates/metafile` — an EMF or a WMF, either Word's
  own or the preview of an embedded object — and carried as a PNG, so a drawing that would
  otherwise be a picture with no `image` is one a reader (and Docling) can open (bug 50).
- OCR boxes are mapped through the **actual placement box**, so figures with text are no
  longer mis-scaled from the page rectangle.

### DOCX (new — Phase 3)
- `crates/docx-parser` walks `word/document.xml` with `quick-xml` in document order and
  emits paragraphs, headings (`Heading1` → `section_header`), list items (`w:numPr`, with
  their marker and level resolved from `word/numbering.xml`), tables as real grids, and
  embedded pictures resolved through the drawing's relationship id into `word/media/`.
- **Office Math (`m:oMath`) is linearised** to readable text: fractions, super/subscripts,
  radicals with degree, n-ary operators, accents, matrices, limits, delimiters, functions.
  Operators are read from `m:chr` **attributes**, which is where they actually live.
- A paragraph containing nothing but math becomes a `formula` item; math inside a sentence
  is inlined into the paragraph text so the sentence stays readable.
- **Headers and footers** are separate parts, extracted into the document's `furniture`
  group rather than dropped (this is the only place `headerPic.docx` has an image).
- **A vector drawing is played into a PNG before it leaves the parser**: `image/emf` and
  `image/wmf` (`drawing.rs`) are handed to `crates/metafile` at the size the document
  asks for, so a drawing is carried as pixels like every other picture. The same path
  reads `w:object` — an embedded OLE object, whose preview is a `v:imagedata` pointing at
  a metafile — which is how `mathtype.docx`'s equation and `drawing.docx`'s CorelDRAW logo
  become pictures.
- Page size comes from `w:sectPr` in twips (A4 in the LibreOffice fixtures).
- Zip entry names are snapshotted up front, so media bytes are read before the XML walk
  and the borrows stay simple.

### Legacy `.doc` (new)
- `crates/doc-parser` implements the **OLE compound file** container from the ground up
  (`cfb.rs`): header, DIFAT/FAT, mini-FAT and mini stream, directory entries. Full sectors
  (512/4096) and 64-byte mini sectors are both handled, so a small `1Table` is read from
  the mini stream rather than as garbage.
- `word.rs` reads the binary **Fib** (stepping over the version-dependent `csw`/`cslw`
  arrays rather than assuming their sizes) and follows **`fcClx`** into the piece table.
  Every run is decoded in its own form: **compressed** runs in CP1252 (with the C1 range
  mapped properly, so curly quotes and en-dashes are not control characters) and others as
  UTF-16.
- The character stream is resolved into blocks: paragraph marks end paragraphs; a field's
  **instruction** (`\x13`..`\x14`) is dropped while its **result** is kept; a lone
  `EMBED`/`SHAPE` field yields a picture placeholder; page breaks split paragraphs;
  optional hyphens vanish and non-breaking spaces survive.
- Inline pictures become `picture` items with no pixels. OfficeArt raster extraction needs
  `PAPX`/`CHPX` character formatting to know where the blobs sit, which this parser does
  not read, so the picture is reported as a placeholder rather than guessed at.
- Known limits, stated plainly: no paragraph styles (so no headings), cell marks flatten to
  tabs (no table grids), and the page size is the Word default because the section table is
  not read.

### Flow text formats (new)
- `crates/text-parser` reads five more families with no third-party format library:
  - **RTF** (`rtf.rs`): group stack, control words and symbols, **destination groups**
    (font table, colour table, stylesheet, `\pict`, fields' instructions) dropped, `\ucN`
    honoured so a `\uN` character's `?' fallback is not duplicated, and `\'hh` bytes
    decoded in CP1252.
  - **HTML** (`html.rs`): a tag scanner that respects quoted attribute values, maps
    `h1`..`h6`/`p`/`li`/block containers to blocks and `<table>` to a grid, drops `head`,
    `script`, `style` and comments, and decodes named and numeric entities.
  - **Markdown** (`text.rs`): ATX and setext headings, bullets and ordered lists with
    depth and marker, fenced code blocks kept verbatim, and GFM pipe tables.
  - **CSV/TSV** (`csv.rs`): RFC 4180 with quoted fields holding delimiters, newlines and
    doubled quotes.
- All of them report a single nominal page (these formats are flow-based) and use the same
  `Block`/`TextDocument` shape, so the CLI emits them through one path.

### UI (new — Phase 8)
- `crates/ui` is a **Tauri 2 window** (system webview: WebKitGTK / WebView2 /
  WKWebView) over a static HTML/CSS/JS frontend in `crates/ui/dist`. No Node or
  bundler is involved, so the UI builds with the same cargo command as everything
  else.
- It is deliberately a *caller* of the CLI, not a second pipeline: the form is
  compiled into an argument vector (`--input`, `--output`, `--parallel`,
  `--recursive`, `--ocr`, `--preview`, `--progress`), the `pdfextractor-cli`
  binary is spawned, and the exact command line is shown while it runs so any
  failure can be reproduced by hand. The CLI is found beside the UI binary, or
  via `PDFEXTRACTOR_CLI`.
- **A conversion is never waited on.** The CLI is started with `--progress`, and
  two reader threads turn its two streams into events for the page: a plan, one
  report per finished document, the CLI's log lines, and a final verdict. The
  window therefore stays live while a folder converts, and its bar, document
  list and log are drawn from that stream rather than counted in JavaScript.
- **Stop really stops it.** The child stays reachable (`Job`, guarded by a
  mutex), `cancel_conversion` kills it, and the exit is observed by a waiter
  thread polling `try_wait` every 40 ms — `Child::wait` would hold the child and
  leave the stop command unable to reach it. A stop is only reported as a
  cancellation when the process really was stopped, so clicking Stop as the last
  document lands does not misreport a completed run. Closing the window stops the
  run too.
- Four commands are exposed to the page: `pick_path` (native file/folder chooser
  via `rfd`), `start_conversion`, `cancel_conversion`, and `frontend_ready` (a
  readiness ping). History: the window used to run the CLI with a blocking
  `run_conversion`, which froze the window for the whole conversion and could not
  be cancelled — the whole point of this change.
- **The logic lives outside the window** (`crates/ui/src/lib.rs`): `Settings`,
  its `to_args()`/`problem()` validation, `parse`/`pump` for the progress stream,
  `Job` for the process, and the event payload structs. That is what makes the UI
  testable without a display; the process handling is tested against real system
  binaries, and the page's own DOM is verified by `--selftest`.
- `--selftest [INPUT]` mounts the window, and if `INPUT` is given converts it
  through the CLI on the same path the window uses, then hands back what the page
  rendered and exits. `--selftest-stop MS` does the same but stops the run part
  way through — so both the stream and the stop path are covered without a human.
  The one thing it does differently from the window is where the output goes: a
  self-test converts into `/tmp/pdfextractor-selftest-<pid>/`, never beside the
  document it read, because the smoke test is pointed at the corpus itself and a
  stray `*.json` (or `<stem>_images/`) among the corpus documents is a document
  with an unsupported extension to the golden suite.
- The page can also be driven in an ordinary browser: `dist/index.html?mock=1`
  loads `dist/mock.js`, a small stand-in for the Tauri API that plays a scripted
  run. That is how the layout and the Stop button were checked during
  development, and it is what found the frontend bug listed below.
- Tauri needs a **capability** to let the page subscribe to events
  (`crates/ui/capabilities/default.json`, `core:default`); without one the page
  loads but never draws anything (bug 43).
- An app icon is generated as an RGBA PNG (`crates/ui/icons/icon.png`); Tauri
  requires RGBA and rejects RGB.

### Compare mode (new — Phase 8)
- The window can put every document it converts **beside Docling's reading of the
  same document**. The reference is a file, or a folder of files named after the
  documents they belong to — `test-data/golden/docling/` is exactly that — and it
  travels beside the process rather than into its arguments: the extractor is never
  told what it is being compared with.
- `crates/ui/src/diff.rs` does the comparing and is pure: documents in, verdicts out,
  no window and no filesystem beyond reading the two JSON files. Both sides are
  laid out by **the document's own body tree** (`body.children`, then `furniture`),
  with groups (`#/groups/N` — how Docling encloses a picture area, a list or a run of
  formulas) walked where they stand rather than after every paragraph, and a block no
  tree mentions appended rather than dropped. The two lists are aligned the way a diff
  aligns two files: a block pairs with at most one *thing* on the other side (one block,
  or the run of reference items wholly inside it — see containment below), pairs never
  cross, and a block with no counterpart of its own stands alone on its side, against a
  similarity floor so that one reworded paragraph does not drag the rest out of line.
- **The boxes take part in the score itself, not only in the joining afterwards.** The
  words keep the gate — two blocks are a link at all only if their words clear
  `LINK_THRESHOLD`, and no box can invent one or unmake one — but where the words offer
  more than one link the boxes decide which is taken: a pair both readings put in the same
  place on the same page is worth `PLACE_BONUS` more (a tenth of a link), and a pair they
  put in different places loses `PLACE_COST` (as much as a link needs at all). The two
  sizes are not a taste. The bonus is what tells apart two pairings the words cannot: a
  drawing a document lists twice, in one order on each side, is the same media type at the
  same size twice, and only the box says which copy is which — and pairing **those** by
  their place in the list means each row then pairs a drawing with the other one's box,
  which the new rule would call a difference. The cost is what makes two pairs the boxes
  contradict lose to the one pair they agree about (`2 - 2 * PLACE_COST` against
  `1 + PLACE_BONUS`), so a crossing the order alignment cannot resolve is resolved by
  where the blocks stand. What it is *not* is a veto: a block the reference read somewhere
  else is still one row — a **differs** row whose note says where each reading put it
  (`not in the same place: page 1, 8% of the smaller box shared`), or `page 1 vs page 2`
  when the disagreement is the page — rather than two rows claiming the other reading has
  nothing there. Two pairs are left out of all of it: two equations, whose text is a
  rendering rather than words, so a pair of them is a pair for reasons that have nothing
  to do with where either sits.
- Pairing is not only by content. The alignment respects order, so two readings that
  put the same drawing or table in the same place in a *different* order cannot both be
  paired by it — pairing both would make the pairs cross. The boxes decide which of those
  pairs the alignment takes, and what is left over is joined
  afterwards by **page and box**: `crates/ui/src/diff.rs` reads `prov[0].page_no` and
  `prov[0].bbox` from both sides, turns them into one frame (Docling measures from the
  page's bottom-left, we measure from its top-left, so the page height does the turn),
  and joins two blocks that stand alone, are the same kind of thing, and share at least
  half of the smaller box. Measured on the corpus: 60 of `tracemonkey.pdf`'s rows, 11 of
  `arxiv_attention.pdf`'s and 6 of `irs_f1040.pdf`'s are joined this way — and it is a
  *last* resort, running after the alignment (which the boxes now steer) and after the
  fold, so the more evidence a pairing has the fewer rows this step has left to guess at
  (265 → 138 across the corpus since containment). Three things
  keep it from inventing pairs: content is always consulted first and only leftovers are
  joined, an equation beside a paragraph in the same box is left alone (that one reading
  found an equation at all is itself the finding), and a box whose frame cannot be
  reconstructed — a bottom-left box with no published page size — is not used at all.
  Every joined row carries the page and the overlap that joined it, the count is in the
  panel's header, and a panel with a joined row opens itself: this step is a heuristic,
  and a reader has to be able to tell it from a comparison.
  It also made the picture floor honest: the rule that pairs "(no image)" with a PNG is
  for the *same* picture read twice, so it is now withheld when the boxes put the two
  pictures on different pages or in non-overlapping places — those are two pictures, and
  saying "differs" about them invented a pairing instead of reporting two findings.
- **Containment: the reference's item that is wholly inside one of our blocks.** Docling's
  PDF side writes an item per line where we keep a paragraph whole, so a comparison that
  paired only by content called every one of those items text the reference has and we do
  not. The alignment can now consume one of our blocks together with the *run* of
  consecutive reference items it holds, worth a little per item so that a run beats
  splitting into one-sided rows and a single item never outbids a block that really
  matches. The row is **contained**: our side is the one block, the reference's side is the
  whole run, so a paragraph the reference read as a dozen lines is one line of the panel
  and no block is ever shown in two rows. "Wholly inside" is letters from one of our word
  boundaries to another, so a piece of one of our words is not inside it (the reference's
  `3` is not a piece of our `1937`) while a field label our own reading spaced out letter
  by letter is; words in another order are not containment, and an item with no letters in
  it is not a run of anything — it is the fold's, by where it lies (below). A label the two
  readings disagree about keeps the row **differs** with the containment named in its
  note, so a heading we merged into a paragraph cannot hide behind it.
- **The fold: the items a run cannot take.** The alignment consumes a run only while its
  items stand *together*, and the reference is rarely tidy: a form's field label read
  between two halves of a line we kept whole breaks the run, and both halves then stood
  alone as text we do not have. So after the alignment every remaining one-sided item
  whose letters are a run of words inside a block of ours is **moved onto that block's
  row**, which says how many it took, and its own row is gone (`fold_contained` in
  `crates/ui/src/diff.rs`). Content decides this wherever there is content to decide it,
  and that is why the fold runs *before* the box joining: an item's letters lying inside
  our block is evidence that a shared box never is. The one thing content cannot decide is
  an item with **no letters in it at all** — Docling writes one for every dotted leader and
  every rule it meets, so the lines of a filled-in form arrive as word fragments with
  leader items between them, and every leader stood as a line saying the reference has text
  we do not (266 of them on `irs_f1040` alone). Such an item is taken by where it lies,
  under three conditions that keep it from claiming more than it can show: the item is
  taken only onto a block of ours that **already holds words** of the reference there (a
  leader standing among the fragments of a line we read is part of that line, while a rule
  alone in our block's area is the reference's own reading of it, which the box joining
  pairs and says it paired); among the blocks that hold it, the one it lies **most**
  inside takes it, not the nearest; and both producers have to have published a box
  (`place_agreement`), so a DOCX is compared exactly as before. The row names those items
  apart from the ones it took by their words — `3 of the reference's items are wholly
  inside ours; 16 items with no letters in them lie in the same place` — and shows the
  reference's items in the reference's order, the ones lying *before* the item the
  alignment paired going in front of it. What the row already disagreed about it keeps: a
  row that agreed with the reference gains the words and becomes a containment, while one
  that differed about the words stays **differs** with the containment named in its note —
  and a label disagreement is named either way — so nothing the comparison found
  disappears behind the fold. On the corpus the
  alignment's own containment took **233** rows out of "only the reference" and this fold
  then took **792** more off (2,217 → 1,425) and added **25** rows to the containment
  column (233 → 258) — measured when the fold landed; putting the boxes in the score and
  taking the leaders with them has moved those figures since (**288** contained, **773**
  only the reference, **380** of which are placed rather than listed), because a block the
  boxes point somewhere else takes a different
  partner, and 266 of the rows the leaders took off are `irs_f1040`'s. `README`'s one-sided rows went 200 → 144 → **96**,
  `arxiv_attention`'s 338 → 234 → 64 → **29**, `math_seq2seq`'s 196 → 168 → 52 → **41**,
  `tracemonkey`'s 245 → 241 → 131 → 138 → **97**, `irs_f1040`'s 799 → 682 → 416 → **89**
  (327 of it placed), and the rows
  themselves moved with them (`tracemonkey` 549 → 439 → 446, `irs_f1040` 883 → 766 →
  **500**).
- **What a form's leaders are finally left as: placed, not listed.** Everything above
  answers a letterless item where a block of ours lies along its line (the fold) or where a
  block of ours stands alone in its place (the joining by position). What neither can
  answer is the rest of a form: a line our reading produced no block for at all, which
  Docling still read as dots and rules. Nothing can be compared with such an item — it has
  no letters to match — so it is **placed** (`place_letterless` in
  `crates/ui/src/diff.rs`): a row whose note says what it is and the page and box it lies
  at (the items of one line of a form are one row — the bullet below)
  (`a dotted leader or a rule at page 5, box 50,560–300,580: an item with no
  letters in it at all, so there are no words to compare`), counted in the header as
  `placed` and deliberately *not* among the rows only the reference has, which mean text we
  did not read. It is the last step of the pipeline, so it speaks only for what the words,
  the fold and the boxes all left standing, and it asks an item for a place to name: a
  document without geometry — every flow format — is compared exactly as before. On
  `irs_f1040` that is **327** of the form's items, and the panel's "only the reference"
  count falls 416 → **89** with them: the rows are places now, not accusations.
- **A line of a form is one row, not one per item.** A placed item is not placed on its
  own: Docling writes each run of dots of a dotted leader as an item and the rule under
  them as another, all sharing a line, so a row each would make one line of a form
  twenty-odd rows of the panel (`irs_f1040`'s 327 items stand on 25 lines) and bury the
  finding in the reading of it. The items of one line are therefore **placed together**
  (`collapse_lines` in `crates/ui/src/diff.rs`, the step `place_letterless` hands its rows
  to): one row that names how many items it stands for and the extent they cover (`3 items
  with no letters in them along one line of page 1, box 50,560–550,581: the dots of a
  leader and the rule under them, placed as the one line they stand on rather than one row
  each`), with the header's `placed` counting *rows* rather than items. Adjacency is the
  **reference's**, not the panel's: two items have to be neighbours in Docling's own order,
  so a row that holds reference items comes between them and they are placed apart — while
  a block of *ours* read on that line, holding nothing of the reference's, does not, because
  it is one line of the form whatever each reading wrote along it, and the row's extent then
  covers the block standing between the two leaders. Two lines of the same page are two rows
  with their own extents; one line is one row. Only what the words, the fold and the boxes
  all left standing is *placed* at all, so this only ever merges rows that were already
  places rather than comparisons. Across the corpus it turns **380** placed rows into **69**
  (`irs_f1040` 327 → **25** — one per line of the form — `tracemonkey.pdf` 41 → 34,
  `math_seq2seq.docx` 11 → 9, `arxiv_attention.pdf` 1 → 1), and `irs_f1040`'s panel now shows
  the form's 25 lines as 25 drawn boxes where the per-item rows had drawn 282 of 327 before.
- **A place is shown, not only told.** A row that stands for where something is and says
  so in two pairs of coordinates is still asking a reader to do the arithmetic — which dot
  run belongs to which line of a form is exactly the question a placement exists to
  answer. So a placed row is **drawn**: beside its note stands the page the item lies on,
  to its own shape as the reference stated it (`Placement` in `crates/ui/src/diff.rs`:
  `width`/`height` the page in points, `x`/`y`/`w`/`h` the box as fractions of it), with
  the box outlined on it and the whole thing captioned on hover (`page 5: box
  50,560–300,580`). The note keeps the points, because a row has to survive being read and
  copied — the drawing is what makes it quick to see. It is the only field of a row that
  exists for the panel rather than for the comparison (`Row::placement`,
  `#[serde(skip_serializing_if = "Option::is_none")]`, so no other row pays for it), and it
  draws *nothing* when the reference never stated a page size: those rows are named and
  left undrawn rather than being given a page shape invented for them, which is why a page
  of unknown height keeps its items in the one-sided count instead of pretending to a
  place. Nothing about the comparison changes with it — the drawing is downstream of the
  verdict on every path, and the corpus report and the ratchet read the same numbers as
  before it. What the self-test reads back about it is the panel's own count of boxes
  actually drawn (`placed-drawn=`), counted out of the DOM rather than from the summary,
  so a window that rendered no thumbnail cannot pass on the backend's JSON alone: on
  `irs_f1040` it is **25** of the 25 lines (198 rows, and every one of them within the first
  400 a panel draws), on `tracemonkey.pdf` **1** of 34 (33 of its places lie past that end),
  and 0 on every DOCX. The count is deliberately smaller than `placed` rather than made to look
  equal to it: what was not shown is not claimed to have been.
- **A form is a page, not a row: every placed item drawn at once.** A row's own drawing
  answers *which line is this item on*, which is what the reader of one row wants — but a
  form's *leader pattern* is a fact about the page, and twenty-five thumbnails down a panel
  do not show it (`irs_f1040`'s 327 items are 25 rows on 2 pages). So the placed items are
  gathered the other way round as well (`placed_pages` in `crates/ui/src/diff.rs`, into
  `Report::placedPages`): every placed item of a page, grouped by the page it lies on, and
  the panel draws each page **once**, above its rows, with all of that page's boxes on it.
  The picture holds *items* rather than the rows' merged extents — it is a drawing of what
  the reference wrote, dot run by dot run — and it is built from the rows *before* they are
  cut to `MAX_ROWS`, so a long form's page is whole even where its rows are not (the one
  thing a per-row drawing cannot promise: 33 of `tracemonkey.pdf`'s 34 places fall past the
  400 rows a panel lists, and its whole page is drawn anyway). Pages come in ascending
  order with the reference's own order kept within each, and an item whose page stated no
  size is left out — the same rule as the row's own drawing, both read through one
  `fractions` helper so a box cannot be one shape on its row and another on its page (a
  page is listed only if it has something drawable, and never listed empty). Measured
  through the real window: `irs_f1040.pdf` draws the form's 25 lines as **2** page pictures
  (`placed-pages=2`) where it drew 25 thumbnails, `tracemonkey.pdf` **3**,
  `arxiv_attention.pdf` **1** — **6** pictures across the three references holding all 369
  of their placed items, and **7** pictures holding all 380 across the corpus. The panel
  counts the pictures it really drew from the DOM (`placed-pages=`), as it does the boxes,
  so a window that rendered none cannot pass on the backend's JSON alone.
- Seven verdicts, and each says what it means: **same** (same kind, label and words —
  with Docling's `text` and our `paragraph` treated as one label, or every row of every
  document would be noise), **notation** (an equation both sides found, whose *spelling*
  differs because Docling writes LaTeX and we linearise Office Math to Unicode),
  **differs** (with the reason: `label: paragraph vs section_header`, the words, or
  `not in the same place: page 1, 8% of the smaller box shared`),
  **contained** (the reference's item, or the run of them, lies inside one block of ours —
  items the fold moved there are counted in the row's note, the ones with no letters in
  them named apart from the ones held by their words),
  **placed** (an item with no letters in it at all that no block of ours holds and that
  nothing of ours lies along: there is nothing to compare and nothing to miss, so the row
  says what it is and where it lies — the items of one line of a form standing as one row
  that names how many they are and the extent they cover — and the header counts it apart),
  **only ours**, **only the reference**. A picture with no image says so instead of
  looking like a picture; a table is read row by row from the row each cell states.
- The header repeats the golden suite's own figure — the share of the reference's
  alphanumeric characters we also hold, as a multiset, equations excluded — so the two
  readings of "agreement" cannot drift apart. It is deliberately *not* the golden suite:
  that one decides pass or fail against budgets and is the authority; this one shows a
  person where the two readings differ.
- Bounded, because a window must not hang on a pathological document: at most 2,000
  blocks a side are aligned (the alignment is quadratic in that), at most 2,000 rows are
  drawn, and each text is clipped visibly at 300 characters — and what was left out is
  counted in the panel rather than silently missing. A reference that is not there, or
  is not Docling JSON, is reported as the reason a document could not be compared: a mode
  that shows nothing is indistinguishable from a mode that is broken.
- Verified end to end by `--selftest <doc> --selftest-ref <reference>`, which converts a
  corpus document through the CLI, compares it with the committed reference and prints
  what the page drew; the page itself was driven in a browser through `dist/mock.js`,
  which now plays a comparison too — including a row the backend paired by position and a
  row it **placed**, so the page's note, its header count and the page thumbnail a
  placement draws are exercised in a plain browser. Together they found
  that Docling puts a picture area's pictures in a **group** (walked now: without it the
  five drawings of `VariousPictures.docx` were only compared because they were appended
  as unreached blocks), and that `mathtype.docx`'s two readings order the OLE preview and
  the text the other way round. That last one is still shown as the same drawing twice,
  once a side — the document is a DOCX, and neither reading publishes a box to join  the two rows by; the same disagreement in a PDF is now joined.
- **The whole corpus, worst first** is a report rather than a view:
  `cargo test -p pdfextractor-ui --test corpus_report_test -- --nocapture` converts all
  37 documents in **one** CLI run (30 s, `--ocr never`) and compares each with its
  reference through `diff::compare_document`, then prints them ordered by the share of
  rows the reference does not simply agree with (rows that differ + rows only we have).
  It judges nothing — the golden suite is the authority on whether a reading is good
  enough — and asserts that the summary is *complete*: every reference compared, every
  tally equal to the rows sent, no document silently missing. The table has a `cont`
  column for the containment rows and the "only the reference" rows are still not part of
  the order (what is left of them is text we did not read: a leader whose line our reading
  produced a block for is on that line's row, so the ones still listed are the leaders of
  lines we did not read at all), and documents with fewer than 10 rows are listed unranked,
  where a share of two rows means nothing.
- **A document below the floor is refused, in the window as well as in the report.** The
  memory lives beside the references it is about (`test-data/golden/compare-baseline.json`
  against `test-data/golden/docling/`), so the panel finds it with no flag of its own, and
  every finished document is held to the ratchet that memory remembers: the **floor**
  under a form's placed items whatever the run was measured with (it is about the
  *reading*, not about the settings it was made with), and every other rule — `differs`,
  the one-sided rows, the character agreement — when the run's settings match the
  memory's `measured` block, which is what lets the window keep its own default
  (`--ocr auto`) from firing against a file measured with `--ocr never`. When a document
  falls below, the panel says what fell
  (`below the floor · form item(s) placed: 654 → 327 item(s) in 25 row(s)`) and **withholds
  the comparison** — no rows, no page pictures, no verdict — because a reading whose form
  has left the panel is news, not a list of differences to review. A memory that is there
  and unreadable is said on the panel rather than passed over, and with no memory beside
  the reference (every comparison against a document of your own) nothing changes at all.
- **A fallen panel fails a build, not only a window.** The same hold is on the command
  line: `pdfextractor-cli --compare <reference>` compares every finished document with
  its reference through the very library the panel calls (`crates/cli/src/compare.rs`),
  holds each one to the memory beside that reference — the whole ratchet when the run's
  settings match the recorded measurement, the floor when they do not — and exits
  non-zero naming every refused document and what fell, so a corpus run is a build gate
  (`--ocr never --compare test-data/golden/docling`). What it refuses besides a fallen
  document is a run it could not hold at all: a reference path that is not there (refused
  before any conversion starts), a memory that is there and cannot be read, and a
  document with no reference beside it — a gate that held nothing while claiming to hold
  everything is worse than no gate. No memory beside the reference, by contrast, is the
  ordinary case of a document of your own: the comparison still prints and no floor can
  fire.
- **The report is a ratchet, so the numbers cannot quietly move.** What a run measured is
  kept in the repository — `test-data/golden/compare-baseline.json`, 37 entries of rows,
  verdicts, joined rows and the character agreement, one per document — and the test fails
  when one document's panel says more `differs`, more rows only ours, more rows only the
  reference, or holds fewer of the reference's characters than that file remembers,
  naming the document and the count. A change that helps one document and quietly harms
  another therefore cannot pass; accepting a trade is a deliberate act
  (`CORPUS_BASELINE=update …`) whose visible result is the diff of the baseline file, in
  front of whoever reads it beside the change. `same`, `contained`, `notation` and
  `joined` are deliberately *not* ratcheted — a row that changes *how* the two readings
  agree is not a worsening — and the one-sided counts are what stop a `differs` row
  leaving its column unnoticed. Verified in both directions: three doctored baselines
  fail with the documents named (`… rows that differ 80 → 83, character agreement 100.0%
  → 99.9%`, an entry missing, an entry stale) and exit non-zero, while a baseline *better*
  than the run still passes. Worth saying for the record: the boxes-in-the-score change
  above would have tripped this ratchet on `math_batchnorm` (`refs` 53 → 71), so that
  trade would now have to be argued in the baseline's diff rather than discovered later
  in a table. The rule and the numbers it reads now live in the **library** rather than in
  that test (`crates/ui/src/diff.rs`: `Numbers::of` reduces a comparison to the numbers,
  `worse_than` applies every rule to a remembered document, `below_placed_floor` is the
  floor, `Memory` is the file), because the **window** applies one of them: the memory is
  found beside the references it is about with no flag of its own, and each finished
  document is held to its floor — a reading whose placed items have fallen is *refused*
  rather than drawn (the floor row above). The corpus suite imports the same code, and
  regenerating its baseline from the moved types left the file byte-identical, which is
  what says the refactor changed nothing but the panel.

```text
The Compare mode over the whole corpus — worst first

Ordered by the share of rows the reference does not simply agree with
(rows that differ + rows only we have). `cont` counts the rows where the
reference read an item per line inside one block of ours — the same words,
cut differently — so `refs` is text we did not read rather than Docling's
finer side; `agree` is the share of the reference's characters we also hold,
the golden suite's own figure. `plcd` counts the rows that are not a comparison
at all: the reference wrote an item with no letters in it — a dotted leader or
a rule — where our reading read no line, so the row says where it lies rather
than standing among the text we did not read.

document                            rows  same  notn   diff  cont  ours  refs  plcd joined   agree ours+diff
math_gan                             226    19     0     83     12    95    17     0      8   99.9%       79%
arxiv_attention                      334    55     0    112     18   120    28     1     11   99.4%       69%
math_seq2seq                         373    15     0    155     52    99    41    11     11  100.0%       68%
math_batchnorm                       277     5     0     98     15    88    71     0     14   99.9%       67%
basicapi                              10     3     0      4      0     2     1     0      1  100.0%       60%
tracemonkey                          446    24     0    140     55    89    97    41     60   99.1%       51%
math_layernorm                       393    60     0    104     59    88    82     0     27   99.6%       49%
VariousPictures                       12     2     0      5      0     0     5     0      0  100.0%       42%
README                               196     4     0     40     32    24    96     0      0   95.6%       33%
floatingtbl_with_formula              11     1     0      2      0     1     7     0      0  100.0%       27%
FloatingPictures                     203    83     0     35      2     6    77     0      0   96.8%       20%
testPictures                          44     8     0      7      0     1    28     0      0   94.1%       18%
drawing                              259   106     0     11     28    29    85     0      0   99.2%       15%
calibre_demo                         120    74     0      4      4    13    25     0      0   99.9%       14%
irs_f1040                            500    10     0     49     11    14    89   327      6   99.4%       13%
chartex                               19     5     0      0      0     2    12     0      0  100.0%       11%
math-mso2k7                           10     0     9      0      0     0     1     0      0  100.0%        0%

Not ranked — fewer than 10 rows, where a share means nothing
    EquationAsScientificNumbering    2 row(s): 0 same, 1 notation, 1 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    EquationWithAboveAndBelowCaption 6 row(s): 0 same, 2 notation, 2 differ, 0 contained, 0 only ours, 2 only the reference, 0 placed
    dummy                            1 row(s): 0 same, 0 notation, 1 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    equation                         1 row(s): 0 same, 0 notation, 1 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    headerPic                        3 row(s): 0 same, 0 notation, 1 differ, 0 contained, 0 only ours, 2 only the reference, 0 placed
    math-accents                     1 row(s): 0 same, 1 notation, 0 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    math-bold-italic                 9 row(s): 1 same, 6 notation, 0 differ, 0 contained, 0 only ours, 2 only the reference, 0 placed
    math-escaping                    1 row(s): 0 same, 1 notation, 0 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    math-lim                         1 row(s): 0 same, 1 notation, 0 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    math-literal                     1 row(s): 0 same, 1 notation, 0 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    math-matrix                      1 row(s): 0 same, 1 notation, 0 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    math-nary                        3 row(s): 0 same, 3 notation, 0 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    math-overstrike                  2 row(s): 0 same, 0 notation, 0 differ, 0 contained, 1 only ours, 1 only the reference, 0 placed
    math-placeholders                1 row(s): 0 same, 1 notation, 0 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    math-rad                         2 row(s): 0 same, 2 notation, 0 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    math-subscripts                  6 row(s): 0 same, 5 notation, 0 differ, 0 contained, 1 only ours, 0 only the reference, 0 placed
    math-vertical_stacks             4 row(s): 0 same, 4 notation, 0 differ, 0 contained, 0 only ours, 0 only the reference, 0 placed
    mathtype                         3 row(s): 1 same, 0 notation, 0 differ, 0 contained, 1 only ours, 1 only the reference, 0 placed
    shapes-with-text                 6 row(s): 5 same, 0 notation, 0 differ, 0 contained, 0 only ours, 1 only the reference, 0 placed
    two_images                       6 row(s): 2 same, 0 notation, 1 differ, 0 contained, 1 only ours, 2 only the reference, 0 placed

Lowest character agreement — the reference's text we do not hold
    testPictures                     94.1% held         5.9%
    README                           95.6% held         4.4%
    FloatingPictures                 96.8% held         3.2%
    tracemonkey                      99.1% held         0.9%
    drawing                          99.2% held         0.8%

Most rows only the reference has (text we did not read)
    floatingtbl_with_formula         7 row(s), 4 with no letters, 0 placed where we read no line  63.6%
    testPictures                     28 row(s), 26 with no letters, 0 placed where we read no line  63.6%
    chartex                          12 row(s), 6 with no letters, 0 placed where we read no line  63.2%
    README                           96 row(s), 27 with no letters, 0 placed where we read no line  49.0%
    VariousPictures                  5 row(s), 5 with no letters, 0 placed where we read no line  41.7%

Most rows where the reference cut one of our blocks finer than we did
    README                           32 row(s)         16.3%
    math_layernorm                   59 row(s)         15.0%
    math_seq2seq                     52 row(s)         13.9%
    tracemonkey                      55 row(s)         12.3%
    drawing                          28 row(s)         10.8%

Most rows only we have (a block the reference read and we did not)
    math_gan                         95 row(s)         42.0%
    arxiv_attention                  120 row(s)        35.9%
    math_batchnorm                   88 row(s)         31.8%
    math_seq2seq                     99 row(s)         26.5%
    math_layernorm                   88 row(s)         22.4%

Most differing rows (both readings have the block and disagree about it)
    VariousPictures                  5 row(s)          41.7%
    math_seq2seq                     155 row(s)        41.6%
    basicapi                         4 row(s)          40.0%
    math_gan                         83 row(s)         36.7%
    math_batchnorm                   98 row(s)         35.4%

37 document(s) compared, 0 without a reading or a reference, 138 row(s) paired by position,
288 row(s) where the reference cut one of our blocks finer, 773 row(s) only it has,
380 row(s) placed where our reading read no line
```

- **What the ranking says.** The top is the math-heavy PDFs (`math_gan` 79% of its rows
  not simply agreed, `math_seq2seq` 69%, `math_batchnorm` 67%, `arxiv_attention` 63%,
  `tracemonkey` 59%, `math_layernorm` 49%) and `basicapi` (60%): these are the panels
  where most rows are a disagreement or a row of ours with no counterpart. `math_gan`
  reads 95 blocks the reference does not have and `math_batchnorm` disagrees with it 98
  times, which is what a document full of equations and figures does to a block-by-block
  comparison. Every share here is *higher* than it was before the fold, and that is the
  fold working: the same disagreements and the same rows of ours, over the fewer rows the
  reference's per-line items no longer occupy.
- **What putting the boxes in the score cost, and bought.** Across the corpus it moved 23
  rows out of `differs` (908 → 885), added 24 rows only ours (647 → 671) and 29 only the
  reference (1,425 → 1,454), took 6 out of the containment column (258 → 252), put 5 more
  rows in `same` (469 → 474) and joined 7 more by position (131 → 138). Two things are
  worth naming. It *buys* fewer false place differences: with the score terms taken out
  again (`PLACE_BONUS = PLACE_COST = 0`, reproduced to compare) the old behaviour accuses
  **60** rows of the three geometry references of standing somewhere else — the alignment
  pairs by list order and 60 of those pairs then contradict their own boxes — while with
  the boxes in the score it is **17** (`tracemonkey.pdf` 47 → 15, `arxiv_attention.pdf`
  6 → 1, `irs_f1040.pdf` 7 → 1), and `tracemonkey`'s picture rows fall **6 → 3**, two of
  them having been `(no image)` beside `(no image)` at each other's box. It *costs*
  one-sided rows: a block the boxes point somewhere else takes the partner they agree
  about, which leaves the block it used to be paired with standing alone — and that
  block's text is text we *did* read, so `refs` is the column that pays for it.
  `math_batchnorm` is the extreme case (`differs` 113 → 98, `ours` 72 → 88, `refs`
  53 → 71); its share of the ranking fell for the same reason (71% → 67%). The trade is
  the point of the rule rather than an accident of it: the boxes are evidence about
  *which* block this is, and the price of using them is that they can also take a pairing
  away. Part of that price has since been bought back by taking the leaders of a line with
  it (below): `refs` across the corpus is 1,454 → **1,153**, and **773** once the leaders
  of a form line our reading read no block for are placed rather than listed, with no
  `differs` and no row of ours gained anywhere — and `math_batchnorm`, the extreme case
  above, is untouched by either, because a document of equations has no dotted leaders to
  gain.
- **Segmentation is what the `cont` column took out of "only the reference".** Docling
  emits an item per line or formatting run where we keep the block whole, and every row
  where that is the *whole* difference is a `contained` row rather than a row saying the
  reference has text we do not: **288** across the corpus at present (25 of them reached
  by the fold above when it landed), and the counts they came from are visible —
  `README` 200 → 144 → **96**, `drawing` 167 → 91 → **85**, `arxiv_attention`
  338 → 234 → 64 → **29**, `math_seq2seq` 196 → 168 → **52**, `tracemonkey` 245 → 241 →
  **138** — and the rows themselves fall with them (`drawing` 341 → 265 → 259, `README`
  300 → 244 → 196). What is left in `refs`
  is text we did not read. The leaders of a line our reading produced a block for are no
  longer part of it (they are on that line's row): **593 of `irs_f1040`'s 912 items carry
  no alphanumerics** (an item per dotted line and per rule on the form) and **266 of them
  went onto their lines' rows**, and the **327** that are left are **placed** rather than
  listed (`place_letterless`): rows naming the page and box they lie at, which takes the
  form's `refs` from 416 to **89**. They are the leaders of lines our reading produced no
  block for at all, and a placement says where they are rather than that we lost them.
  That also corrects a number this file used to quote: a *character* test called 94% of
  `irs_f1040`'s items "fragments of one of our blocks" because a `.` is a substring of
  everything, and empties trivially pass — the rule the mode uses now asks for the
  reference's letters to run from one of our word boundaries to another, and for the item
  to carry letters at all.
- **`tracemonkey`'s residual is punctuation, not segmentation.** The reference carries 11
  `∗` (U+2217) affiliation markers in the author list against **0** in ours, and our text
  has 138 `#` against its 4 and 59 `+` against its 16 — plus one line of obfuscated author
  email that we read and the reference does not carry at all. Characters of that kind do
  not move the agreement figure, which is why the row comparison is what found them.
- **The worst *agreement* documents are the reference's fault, not ours**, and the golden
  README already says why: `testPictures` (94.1%) is missing exactly one 7-character item,
  the `Caption` paragraph style LibreOffice invented when the `.doc` reference was made,
  and `FloatingPictures` (96.8%) is missing the footer LibreOffice re-rendered —
  `John Greer  Page 7  10/12/2026`, with the page number and the date of the run. We
  report what the file stores. Re-running the report after regenerating the references is
  how those numbers move.
- **`README` is the one to look at.** 95.6% held, 38 of its 244 rows differing, and the
  difference is visible in the rows: our item text keeps the Markdown *source*
  (`**figures at exactly 500 dpi**`, `` Legacy `.doc` (Word 97 binary) ``,
  `[arXiv:1706.03762](https://arxiv.org/abs/1706.03762) "Attention Is All You Need"`)
  where the reference is the *rendered* text (`figures at exactly 500 dpi`,
  `arXiv:1706.03762 "Attention Is All You Need"`). The words agree and the characters
  mostly do; what disagrees is syntax we passed through where Docling rendered it.
  Part of its 4.4% residual is the finer-split and repeated-text effect the golden README
  documents; the rows are the syntax.


### Box overlay preview (`--preview`) — new
- `crates/cli/src/preview.rs` writes a self-contained `.preview.html` beside the JSON.
  Every extracted box is drawn at its true coordinates, colour-coded by source
  (text layer / OCR / vector figure / image), with the recognised text rendered inside
  the box at the size the box implies.
- A **second layer draws the layout blocks**: one dotted box per element layout analysis
  produced, labelled with its kind (`title`, `section_header`, `list_item`, `caption`,
  `table`), so the block structure can be judged next to the raw runs that produced it.
  Toggling the two layers independently is what makes a bad boundary obvious.
- A **linked sidebar** pairs each row with its box (hover highlights both, click scrolls),
  so a 1,500-box page stays navigable.
- **Page backdrop is best-effort**: scans already have their image, and `pdftoppm` is used
  for text pages *when installed*. Without a renderer the HTML reconstructs the page from
  the extracted text and says so. Extraction never depends on an external renderer.
- **Fit-to-width on load** plus a stacked layout on narrow panels, because a fixed scale
  pushes the page out of view exactly where the overlay is most useful.
- Flow-based formats have no overlay: DOCX, legacy `.doc`, RTF, HTML, Markdown and text
  carry no page geometry, so the CLI says so once and skips the preview.

### Layout into the CLI (new)
Both text paths feed the same analysis: a page with a text layer contributes its spans,
and a page that is OCRed contributes its recognised lines (with a font size estimated
from the line box). When OCR runs, it *replaces* the text layer rather than duplicating it,
while the text layer stays visible in the overlay for comparison. Elements come out in
reading order as `Text` (with label, level, marker), `Table` and `Picture` items.

### OCR (required, default-on)
- `crates/ocr-engine` binds Tesseract via `leptess`.
- Word geometry comes from Tesseract **TSV** output and is grouped into lines by
  `(block, paragraph, line)`.
- The CLI runs OCR **only when a page has no text layer** (`auto`), always (`always`),
  or never (`never`).
- OCR boxes (image pixels) are scaled into PDF points using the page `/MediaBox`.

### CLI (`crates/cli`)
- `-i/--input`, `-o/--output`, `-p/--parallel`, `-r/--recursive`, `--ocr auto|always|never`,
  `--progress`.
- `--progress` streams a run: a plan line listing the whole queue, then one JSON record per
  document **the moment it finishes** (the parallel loop reports inside the worker instead
  of collecting results and printing them at the end, and reporting is serialised so `done`
  counts up in order). A failure is a record too, carrying its reason, and the run still
  finishes the other documents. Log records go to stderr, so stdout is records only.
- File or directory input, parallel conversion via `rayon`, per-file errors with a
  non-zero exit when anything fails, default output beside the input.
- Parallelism clamped to 256 with a warning (spec asks 1–1000; more threads is harmful).
- Supported extensions: `pdf`, `docx`, `doc`, `rtf`, `html`/`htm`, `txt`, `md`/`markdown`,
  `csv`/`tsv`. Each format gets the right `origin.mimetype`.
- Directory runs resolve every output path first: when two inputs share a stem but not an
  extension (`report.pdf` and `report.txt`), replacing both extensions would write the same
  file twice and silently drop one, so those keep their full name (`report.txt.json`).

## Bugs found and fixed in this session

Real defects from earlier work that had never been compiled:

1. **`lexer.rs` had lost its entire implementation** — only the `Token` enum survived while
   `parser.rs` called `Lexer::new`/`tokenize`. Nothing compiled. Restored and rewritten.
2. **Text extraction was inverted** — the content parser looked for `(` *after* seeing `Tj`,
   but PDF is postfix (`(Hello World) Tj`). Result was `"texts": []`. Rewritten as an
   operand stack + operator dispatch.
3. **Stream payloads were tokenised as PDF syntax** — a `(` byte inside JPEG data swallowed
   the real `endstream`, so embedded images decoded to **zero bytes**. The lexer now skips
   payload bytes directly, and the `Stream` token's span *is* the payload.
4. **`%%EOF` was skipped as a comment** — the check ran before whitespace skipping, so a
   preceding newline hid it. Reordered.
5. **Unbounded recursion** in the page-tree walk — depth guard added.
6. **CLI referenced a non-existent function** and had **inverted directory-recursion depth**.
7. **Tesseract was a hard dependency** that broke builds without native libs (now a
   default-on feature that can be disabled).
8. **Feature-gated code produced warnings** when OCR was off; guarded with `cfg`.

### Bugs found by testing real downloaded documents

Hand-written fixtures missed all of these; they only appeared against documents
produced by pdfTeX, Quartz, TCPDF and the IRS.

9. **Linearized PDFs contain an `%%EOF` mid-file.** After the first-page
   cross-reference section the document continues. The lexer stopped at the first
   `%%EOF`, so `irs_f1040.pdf` parsed **2 of 295 objects** and produced **zero
   pages**. Tokenizing now continues past `%%EOF`; the file yields 2451 objects
   and 2 pages.
10. **Object streams (`/Type /ObjStm`) were ignored** — PDF 1.5+ files hide the
    page tree inside them. Implemented expansion via header pairs and `/First`
    offsets; 30 streams expanded on the IRS form.
11. **Words were concatenated** (`Providedproperattribution`). Producers separate
    words by repositioning the pen, not with space characters. Now inferred from
    large negative `TJ` kerning offsets and same-line `Td` advances. `Tm` is
    deliberately excluded, because it is used for precise inline placement
    (`[13]` would otherwise become `[ 13`).
12. **Character codes were emitted raw.** Implemented `crates/pdf-parser/src/encoding.rs`:
    `/ToUnicode` CMaps (`bfchar`/`bfrange`, including multi-unit destinations),
    base encodings (WinAnsi/cp1252 so `0x97` becomes an em dash), and
    `/Differences` with a glyph-name table (so the `fi` ligature decodes).
    Identity-H 2-byte codes now decode to real text.
13. **`Box2` derived `Default`, so `empty` started as `false`.** The first `add`
    therefore kept the zero origin and every drawn box was stretched back to the page
    corner — silently corrupting **every** vector figure box. The test caught it; `Default`
    is now implemented by hand with `empty: true`.
14. **Extracted image files lied about their format.** EMF and WMF payloads were written
    with a `.png` extension because the extension was a `_ => "png"` fallback. Now every
    MIME type maps to its real extension, and `file(1)` agrees with all 26 saved images.
15. **A form's background was reported as a figure.** The IRS form's own border covers
    ~86% of the page and passed the minimum-area filter, producing two bogus "pictures".
    A maximum-area rule (50%) rejects page furniture; a 41% panel still counts.
16. **Office Math operators are carried in attributes.** `<m:chr m:val="∑"/>` holds the
    character, not child text, so a math tree that collected only text lost every Σ, Π and
    ∮. The math tree now records attributes.
17. **Declared-but-unknown image sizes were reported as `0 x 0`.** EMF/WMF have no pixel
    grid, so `image.size` and `image.dpi` are now omitted rather than fabricated.
18. **`clippy::type_complexity`** on the graphics-state tuple — replaced with a named
    `GState` struct, which is what it should have been.

### Bugs found by actually looking at the preview in a browser

None of these are visible from the JSON; they only appear when the overlay is rendered.

19. **The page could never fit the panel.** In the stacked layout `#pages` inherited
    `align-items:flex-start`, so a column flex item sized itself to its *content* — the
    700 px page. Fit-to-width then measured 698 px of "available" width and concluded
    114%, so the page stayed clipped no matter how the script was written. Fixed with
    `align-self:stretch`; the fit now resolves to 64% in a 439 px panel.
20. **The overlay text fought the page render.** Drawing our text on top of the true
    glyphs produced unreadable doubled text. With text shown, the render now fades to
    28% so the two can be compared; toggling text off restores it.
21. **Overlay font size overshot the original** — `0.92 × box height` made each run
    visibly larger than the text beneath it. A typeface's em is roughly 0.72 of a
    single-spaced line box, which is what it now uses and what matches the render.
22. **A preview could be requested for a single file with a directory output**, writing
    into the wrong place. That was my invocation error, not a defect — for a single input
    `-o` is an output *file* (documented), and the correct form is
    `-o target/preview/scanned.json`.
23. **A missing `;` silently voided two CSS declarations per box.** The geometry helper
    returned `…height:calc(…)` with no terminator, so the appended `font-size` merged into
    it: `height:calc(…)font-size:calc(…)` is one invalid declaration and the browser drops
    *both*. Every box therefore had no height and every run fell back to the inherited
    13px — the heading happened to look right, so it read as "the render is misaligned".
    Found by measuring `getComputedStyle().fontSize`, which was 13px for every box despite
    four different declared sizes. Two tests now guard it: one asserts every inline style
    declaration is independently valid (exactly one `:` per segment), and one asserts a
    12pt box keeps both its height and its 8.64pt font size.

### Bugs found by the layout phase's own corpus tests

These surfaced the moment layout was run over the whole real corpus, which is the first
time anything checked that boxes and text were *self-consistent*.

24. **`BT` did not reset the text matrix — the largest bug found in this session.**
    `BT` starts a new text object, and per the specification it sets both `Tm` and `Tlm`
    to the identity. The interpreter left the previous position in place, so a block that
    positions itself relatively (`Td`) accumulated onto the *last* block's origin. Pages
    drifted further out of the mediabox with every text object: on `arxiv_attention.pdf`
    the title landed at y = **-537** and the authors at **-1079** on a 792 pt page, and
    `basicapi.pdf` walked to y = **-2763** and x = **1243** on a 595×842 page. Because the
    old output emitted one item per span, this was invisible in the JSON — the boxes were
    wrong but nobody compared them to the page. Everything downstream was affected: the
    overlay drew those spans off-page, and layout analysis clamped them all into the
    top-right corner. Fixed with a two-line reset and a unit test that asserts two `BT`
    blocks positioned with `Td` keep their own coordinates.
25. **XY-cut bands were never checked for coverage.** Bands were taken from *any* pair of
    blocks, so a band measured between two non-adjacent rows (e.g. row 0's bottom and
    row 3's top) split a paragraph in half at a gap that no whitespace actually occupied.
    Found by a test asserting four tightly spaced OCR lines stay one block; now a band is
    only usable when no block straddles it.
26. **Two columns merged into one line per row.** Line grouping used vertical overlap
    alone, and two columns' lines share a baseline, so `left 0 right 0` became a single
    line and no gutter could ever be found. Rows are now cut into runs by horizontal
    proximity, which is what makes column detection possible at all.
27. **A centred line read as a first-line indent.** The indent rule only asked whether the
    next line started further right, so the third (centred) line of a paragraph became its
    own `section_header` — visible in the preview as a stray teal block. An indent is now
    bounded above as well (between 0.5 and 4 em), and a test pins both sides.
28. **Every large line could claim to be the title.** The title test compared against the
    largest size *in its own leaf*, so `Abstract` and a margin stamp were titled too, and
    `arxiv_attention.pdf` (whose 20 pt arXiv stamp outranks its 17 pt title) ended up with
    **no** title at all. It is now the first unnumbered block in the upper third of page 1
    at the largest size *in that region*, tracked once per document.
29. **Chart axis labels became list items.** A decimal such as `0.9` matches the
    multi-level numbering pattern, so `math_batchnorm.pdf` reported 12 axis labels as
    `list_item` with markers `0.9`, `0.8`, `0.7`. Numbering segments never start with a
    zero, so that is now the discriminator.
30. **LZW's `KwKwK` case dropped the stream.** When a code is used in the same step it is
    defined, the dictionary lookup for its first byte asked for a code that does not exist
    yet and the whole decode returned `None`. Caught by the specification's own example
    vector; the reference-encoder round-trip now covers both `EarlyChange` settings.

### Bugs found by fixing the defects the limitation list admitted to

Each of these was on the "known limitations" list; the fix was measured, not assumed.

31. **A rotated margin stamp could not be the title — except that it was.** Tracking the
    text matrix's rotation gave the arXiv stamp the right *box*, but the title rule then
    saw a 20 pt block in the upper third and titled it, leaving *Attention Is All You
    Need* as a `section_header`. A title must now be set horizontally, and rotated rows
    no longer raise the size bar for the region. `texts[2]` of `arxiv_attention.json` is
    the title again and the stamp keeps its vertical box at `l=12, t=165, r=32, b=555`.
32. **A `/Predictor` on a *content* stream was never undone.** Predictors were handled in
    the image path only, so a page stream that carried one stayed compressed. The predictor
    is now undone at the filter that produced its input — respecting `/DecodeParms`
    positional arrays — and the image path no longer applies it a second time. Three unit
    tests cover the direct, array-positional and "no predictor" cases.
33. **A payload's own bytes were being cut off by the terminator search.** The lexer finds
    `endstream` by byte search, which is all it can do before the dictionary is understood,
    and its "drop the EOL before the terminator" rule also ate a payload whose last byte
    was `\r`. `basicapi.pdf` object 20 is exactly that case: one byte short, so the deflate
    stream failed to inflate and page 3's footer was lost. `/Length` (directly or through a
    reference) is now authoritative, used only when the byte after it really is the
    terminator. Measured effect: **+3 text items, +55 characters in `basicapi.pdf`, nothing
    lost in any other document**.
34. **A margin stamp was read as a column, inverting the page.** `basicapi.pdf` page 3 has
    a heading at the top, a footer line at the bottom of the body, and the producer's
    16 pt stamp in the far-left corner. XY-cut preferred a vertical split, which put the
    stamp *first* in reading order. A vertical band is now only a gutter when both sides
    cover at least a quarter of the group's height. Removing the check reproduces the
    inversion (asserted in both the unit test and the corpus ordering test).
35. **`w:lvlOverride` was ignored.** A list that restarted or changed its marker part way
    through Word would keep printing its abstract numbering. Level overrides and
    `w:startOverride` are now applied per `numId`. Found while adding the override support:
    the first cut removed the abstract scheme from the map instead of borrowing it, so the
    *next* list that referenced it lost its markers — the test that asserts `1. 2. A) B)
    5. 6.` in one document caught it.
36. **A printed year was the document title.** `irs_f1040.pdf` sets `2025` at 24 pt above
    everything else, so the largest-in-the-upper-third rule titled the form with a year.
    A title has to be made of words. The corpus now reports exactly 7 titles, each one a
    document's real name, and `2025` survives as a `section_header` with its box intact.

### Bugs found by testing the command line and the UI

37. **`--recursive false` was documented but rejected.** `recursive` was declared as a bare
    `bool` flag with `default_value = "true"`, which clap turns into a value-less switch:
    `--recursive false` failed with "unexpected argument 'false'", and there was no way to
    turn recursion off. The README had advertised `-r, --recursive <BOOL>` all along. The
    flag now takes an optional boolean value (`num_args = 0..=1`,
    `default_missing_value = "true"`, `action = ArgAction::Set`), so `--recursive false`,
    `--recursive=true` and a bare `--recursive` all work. Caught by
    `recursive_false_stays_at_the_top_level`.
38. **Colliding output names silently dropped a document.** In a directory run,
    `report.pdf` and `report.txt` both mapped to `report.json`; the second overwrote the
    first. Output paths are now resolved up front and colliding inputs keep their full name
    (`report.txt.json`). Caught by `colliding_output_names_are_disambiguated`.
39. **Tauri rejected a non-RGBA icon.** The generated app icon was RGB (PNG colour type 2)
    and `generate_context!` refused to build until it was RGBA (type 6). Recorded here
    because the failure is opaque ("icon ... is not RGBA").
40. **The log records were landing on stdout.** `tracing_subscriber::fmt()` writes to
    stdout by default, so with `--progress` the first line of the machine-readable stream
    was `2026-10-12T… INFO pdfextractor_cli: 5 document(s) to convert` — prose where a
    record was expected. Caught immediately by `progress_streams_the_plan_then_every_document`,
    whose reader refuses to skip a line it cannot parse. Log records now go to **stderr**,
    which is the conventional split anyway: stdout belongs to the run's own output.
41. **A cancelled run could leave a truncated document.** `export_to_file` wrote straight
    to the destination, so a kill during the write left a `*.json` that was still *some*
    JSON — worse than no file, because nothing downstream could tell. The bytes now go to
    a hidden sibling (`.<name>.<pid>.tmp`) and are renamed into place, which is atomic on
    the same filesystem. Tested by `no_temporary_file_is_left_behind` and
    `an_export_over_an_existing_file_replaces_it`; verified on a real stop by checking
    that every remaining `*.json` parses and no temp file survives.
42. **The page dropped the first events of a new run.** Events can arrive before the
    `start_conversion` call has answered (the CLI is already running). The page matched
    them against the run *still on screen*, so their ids disagreed and the plan, the log
    and any fast first document were discarded — the second conversion of a session showed
    an empty list. Found by driving the page twice in a row in a browser. Fixed properly by
    making the run itself authoritative: the backend emits `conversion-started` before it
    can read a byte from the CLI, and the page builds its state from that event instead of
    from the call's answer. The buffering code that papered over the race is gone.
43. **The window loaded but never drew anything.** The page subscribes to events with
    `listen`, and a Tauri app grants nothing without a capability file — the call was
    rejected, `boot()` threw, and `frontend_ready` was never invoked, so `--selftest` hung
    with an empty stdout and a window that looked fine. (`invoke` for the app's own
    commands is always allowed, which is why the earlier blocking version worked without
    one.) Fixed with `crates/ui/capabilities/default.json` granting `core:default`, and
    pinned by two window labels in `tauri.conf.json`. Worth recording because the failure
    mode is invisible: no error reaches the terminal.

### Bugs found by comparing against real Docling

The comparison is `crates/cli/tests/docling_golden_test.rs`; the references and the
regeneration commands are in `test-data/golden/README.md`. Two of these were found by
Docling *rejecting* our documents, the rest by diffing content against Docling's own
reading of the same file.

44. **Our documents were not loadable by Docling at all.** `docling-core` refused the
    whole file, so every downstream claim about compatibility was untested. Six separate
    disagreements with the schema, all of them silent in our own JSON:
    * `coord_origin` was written lower case (`"topleft"`); Docling spells it `TOPLEFT`.
    * `level`, `marker` and `enumerated` were copied onto **every** text item, but each
      item class allows only its own fields — a section header has `level`, a list item
      has `marker` and `enumerated`, a title has neither — so the extras were rejected
      (`extra_forbidden`). The structure the model actually has is now applied per
      label, and a heading's number is folded back into its text
      (`"3.1 Encoder and Decoder Stacks"`, which is how Docling keeps it) rather than
      dropped to win a schema argument.
    * `image.dpi` was a fraction where Docling declares an integer.
    * `binary_hash` was `None` or a *signed* hash that can come out negative; it is now
      unsigned.
    * An `image` reference to a media type Docling's registry does not know (`image/wmf`,
      `image/emf`, `image/jp2`) invalidates the whole document. Those pictures are now
      reported as a picture with a box and no `image` — a drawing that cannot be named is
      better than a document that cannot be read.
    * `version` was our own guess at the format revision. It is now the default of the
      `version` field in the committed schema, and a test asserts the two are equal, so
      the exporter and the golden schema cannot drift apart.
45. **The furniture listed children that claimed the body as their parent.** The parent
    reference was written once for everything, so `#/furniture` contained header items
    whose own `parent` said `#/body`. The JSON Schema cannot express that agreement, and
    Docling enforces it at load time ("Document hierarchy is inconsistent"); the golden
    check now walks every group's children and compares each one with its own parent
    declaration.
46. **Our DOCX reading disagreed with Docling's**, which is what a golden diff is for.
    Five defects, all in the same parser:
    * **Every shape was read twice.** Word writes each shape twice — DrawingML in
      `mc:Choice`, a VML copy for old readers in `mc:Fallback` — and reading both put
      every shape's text in the document twice in a row. Found by a check that no block
      repeats itself, which needs no reference to be true.
    * **Tabs were dropped.** A `w:tab` in a run is a real tab: the table of contents in
      `calibre_demo.docx` separates its title from its page number with one, and dropping
      it produced `Demonstration of DOCX support in calibre1`. Tab *stops* use the same
      element inside `w:pPr` and are still not text.
    * **Chart titles were missing.** A chart's title is written only in the chart part
      (`word/charts/chart1.xml`); the body holds a reference to it, so looking at the
      drawing finds no words at all. All six of `chartex.docx`'s titles now come out as
      captions — including the two modern chart types that write theirs under the `cx:`
      namespace, which Docling itself misses.
    * **Footnotes and endnotes were missing.** They live in `word/footnotes.xml` and
      `word/endnotes.xml`, not in `document.xml`, so nothing ever read them; they are now
      `footnote` items at the end of the flow, where Docling puts them too.
    * **A paragraph of math became one item.** `math-nary.docx` has three equations in
      one paragraph and we reported one formula; one `m:oMath` is one equation, on the
      page and in Docling's output, so each is now its own item.
    * **We published a page nothing was on.** A DOCX declares a paper size in `w:sectPr`,
      so a single nominal page was reported with no item on it. Word repaginates as the
      text is edited and no block has a page rectangle; Docling publishes no pages for a
      DOCX and neither do we, for DOCX, `.doc` or any other flow format. The declared
      size stays a parser-level fact (`DocxDocument`).

47. **The text inside a figure was never read.** A figure included from another file is a
    **form XObject** — a page inside the page, with its own content stream, its own font
    resource names and its own `/Matrix`. `Do` recorded the placement and stopped there,
    so every label a chart draws — its legend, its axis numbers, its subplot titles — was
    invisible: `math_layernorm.pdf` agreed at 0.976, and Docling's twelve legend items
    (`Skip-Thoughts + LN` under each of six subplots) had no counterpart in ours. Walking
    it exposed two geometry bugs that an identity CTM had been hiding, both long-standing
    and neither figure-specific:
    * **The text matrix and the CTM were composed in the wrong order.** PDF 1.7 (9.4.2)
      says `Trm = Tm × CTM`: the text matrix places the glyph in the page's coordinate
      system and the CTM maps that to device space. We composed the other way round, which
      is *identical* while the CTM is the identity — so it survived the whole page corpus —
      and wrong by the CTM's translation and scale for any page with a base `cm`, and for
      every form.
    * **The advance and up directions were left unnormalised** and then multiplied by a
      device-space size, which counted the CTM's scale twice: a box 1.7× too tall on a form
      drawn at 0.43.

    `ContentParser` now takes the page's form tree (`forms_of`, guarded against a form that
    places itself by object id and bounded by `MAX_FORM_DEPTH`) and descends into one under
    the placement CTM composed with the form's `/Matrix`, keeping its text spans and its
    painted paths — so a chart is also reported as a picture (`math_layernorm.pdf` went
    from 2 to 9). Verified against the source: `tracemonkey.pdf`'s `Interpret` is at
    405.402-426.371 with `pdftotext` and 405.402-426.371 with us. Agreement:
    `math_layernorm` 0.976→0.996, `math_seq2seq` 0.989→1.000, `arxiv_attention`
    0.959→0.994, `tracemonkey` 0.985→0.991.
48. **The duplicate rule fired on text a figure draws twice.** With figure text readable,
    "no block may appear twice in a row" reported `arxiv_attention.pdf`'s attention
    visualisation, which prints every word twice by design (the source does: `pdftotext`
    reads each one twice) and six subplots that repeat one axis label. A repeat whose box
    lies inside a picture is now the drawing rather than our extraction; a repeated block
    in the body text is still a defect, so the rule that found Word's `mc:Fallback` copy
    (bug 46) still catches it.
49. **The comparison compared content only.** Every character could be present and the
    document still be wrong: a heading read as body text has lost a boundary, a table read
    as prose has lost a table. The golden comparison now compares labels and block
    boundaries too, anchored by **characters** rather than by index (Docling splits a
    paragraph at every formatting run, so item counts can never match): a reference
    heading must be held under a heading label, a structural block of ours must not hold a
    reference body block of comparable size, and a reference table cell must be in a table
    of ours — read as prose under a table we drew is a failure, prose with no table over it
    is counted and printed (our table detection is deliberately conservative). It found 38
    mislabelled headings and five lost boundaries across the corpus on the day it was
    written; both are itemised, budgeted and printed so they may only shrink.
50. **A Word drawing is not always a bitmap.** Word stores a vector drawing — and the
    preview of an embedded object, a MathType equation or a pasted drawing — as an
    Enhanced Metafile (`image/emf`) or a Windows Metafile (`image/wmf`): a list of GDI
    calls, not a grid of pixels. Neither media type is in Docling's registry, so
    `VariousPictures.docx`'s three drawings were reported as pictures with a box and no
    `image` — the drawing was invisible to everything downstream — and the CorelDRAW WMF
    of `drawing.docx` was not reported at all, because the `w:object` holding it was not
    read as a picture either (`mathtype.docx`'s equation preview comes out the same way,
    and it is the equation's only picture). `crates/metafile` now plays the records —
    both WMF record layouts, the world transform, the GDI shapes and paths, clipping
    rectangles, bitmaps decoded from their DIBs, the common raster operations, the stock
    objects, and text — into a raster, and `docx-parser` carries a drawing as a PNG at the
    size the document asks for, at the 144 dpi Docling's own backend renders these files
    at. The three metafiles of `VariousPictures.docx` are a Santa, a cow and a wrench;
    `drawing.docx`'s is the CorelDRAW logo; `mathtype.docx`'s is the equation's preview.
    Three limits are stated rather than hidden. Text is drawn in a **built-in 5×7 font**,
    because a metafile names a font and fetches its outlines from the system, which a
    document extractor has no business doing: glyph *shapes* are therefore an
    approximation while their position, size and colour are the file's. A record the
    player does not understand is not skipped silently — it is named in `Raster::unsupported`
    — and the corpus's five drawings all play with none. And a drawing we cannot play
    keeps its own payload and stays a box with no image rather than becoming a blank
    picture: `drawing.rs` refuses a malformed, truncated, oversized or ink-free metafile.
51. **A form's dotted leaders were reported as text we do not read, and the first rule
    written for them took them from the wrong line.** `irs_f1040`'s lines are ours word for
    word — `37 38 Estimated tax penalty (see instructions) …38` — and Docling reads each of
    them as its fragments plus an item per run of dots, so 593 of the form's 912 items
    carry no alphanumerics at all and its panel stood at 766 rows with 682 of them text we
    do not read. The leaders of a line *we* read belong on that line's row; the rule that
    says so is not "whichever block it lies in", because an item with no letters in it has
    nothing to be compared and its box is the only evidence — and two blocks of ours can
    both hold it. Two wrong versions were measured before the right one, and the corpus
    report is what caught both. Letting the *alignment* consume such an item with the run
    of its line took a glyph line of `tracemonkey`'s figures off the row it lies along and
    onto a neighbour, adding **rows only ours** where the position join had paired them
    (`tracemonkey` 89 → 93, `math_seq2seq` 99 → 100); restricting the runs to *words* and
    leaving every leader to the fold fixed three of the four but not the fourth, because
    the fold was still taking each one to the **nearest** row rather than the one it lies
    most inside (`tracemonkey` 89 → 90). Both numbers are what the ratchet printed, which is the point of
    it: an improvement that quietly costs another document a row cannot pass unnoticed. The
    rule that stands is three limits together — the item is taken by the block it lies most
    inside, only onto a row that already holds **words** of the reference there, and only
    where both producers published a box — and the doc comment in `diff.rs` says which of
    them answers which failure. What it changes: `irs_f1040` 766 → **500** rows and 682 →
    **416** rows only the reference (266 leaders onto their lines), `arxiv_attention`
    369 → 334 and 64 → 29, `tracemonkey` differ 172 → 140, `math_seq2seq` differ 159 → 155 —
    with no document worse in any ratcheted column, which is how the baseline was accepted.
52. **The rest of a form's leaders were still listed as text we do not read; they are
    placed now.** The fold answers a leader where a block of ours lies along its line, which
    took `irs_f1040` from 682 rows only the reference to 416 — but 327 of the items left
    carry no letters at all: the leaders of lines our reading produced **no block** for, in
    a panel that said the reference had text we do not. Nothing in the comparison supports
    that claim, so such an item is **placed** (`place_letterless` in `crates/ui/src/diff.rs`):
    a row of its own that names what Docling wrote and the page and box it lies at, counted
    in the header as `placed` and *not* among the rows only the reference has. The step runs
    last — after the alignment, the fold and the joining by position — so it speaks only for
    what none of them claimed, and it asks an item for a box to name, which is why every
    DOCX is compared exactly as before. Measured through the real window: `irs_f1040`
    `theirs-only` 416 → **89** with `placed=327`, `tracemonkey` 138 → 97 with `placed=41`,
    `math_seq2seq` 52 → 41 with `placed=11`, `arxiv_attention` 29 → 28 with `placed=1` — 380
    rows across the corpus, and the ratchet's baseline diff is exactly those four
    `theirsOnly` fields and nothing else, which is how it was accepted. What a placement
    cannot say: whether our reading *should* have read that line (the words are not there to
    check), and it placed each item on its own — 327 leaders were 327 rows, a reader's
    scrolling rather than a finding, which section 53 answers. What it *does* do about that
    is draw itself: the box the row names is outlined on an outline of the page it lies on,
    beside the row (`Placement`, built by `placement_of` and drawn by `pageThumb` in
    `dist/app.js`), so the line of the form a leader belongs to is seen rather than worked
    out from the coordinates — and a page whose size the reference never stated is named
    without a picture rather than given a shape.
53. **A line of a form is one row, not one per item.** Placement answered the accusation
    "text we do not read", but it did it per item: Docling writes every run of dots of a
    dotted leader as an item and the rule under them as another, all sharing a line, so
    `irs_f1040`'s 327 items came out as 327 rows on 25 lines — the finding buried in the
    reading of it. The items of one line are therefore **placed together** (`collapse_lines`
    in `crates/ui/src/diff.rs`, the step `place_letterless` hands its rows to, which keeps
    the first row's place in the list and holds every item the run stands for): one row that
    names how many items it stands for and the extent they cover, with the header's `placed`
    counting rows rather than items. Adjacency is the **reference's**, not the panel's — two
    items have to be neighbours in Docling's own order — so a row that holds reference items
    comes between them and they are placed apart, while a block of *ours* read on that line
    (holding nothing of the reference's) is not a separation, and the row's extent then covers
    the block standing between the two leaders. Two boxes are one line when their vertical
    ranges meet within a point (`LINE_SLACK`, a fifth of the shortest line the corpus sets,
    because a leader's dots and the rule under them are published as boxes whose edges meet
    rather than overlap), and two lines of one page are two rows with their own extents. Only
    what the words, the fold and the boxes all left standing is placed at all, so the
    collapse only ever merges rows that were already places. Across the corpus this turns
    **380** placed rows into **69**, the same items named as lines (`irs_f1040` 327 → **25**,
    `tracemonkey` 41 → 34, `math_seq2seq` 11 → 9, `arxiv_attention` 1 → 1), and the panel
    moves with it: `irs_f1040`'s `placed` is 327 → **25** and the self-test's `placed-drawn`
    for it is **25** of 25 (all the lines fit in the 400 rows a panel draws) where it had
    drawn 282 of the 327 per-item rows. The corpus ratchet's per-document fields are
    untouched, because a placed row was never in `theirsOnly`: the collapse moves `placed`
    (not one of the ratcheted columns) and nothing else. The units: a form line read as two
    runs of dots and the rule under them is **one** row, a block of ours between two leaders
    does not split them while an item of the reference's own between them does, two lines of
    a page are two rows, and the header counts placed rows rather than items.

## Test coverage (`cargo test --workspace`, 366 tests)

| Suite | Count | What it asserts |
|---|---|---|
| `pdf-parser` lexer | 4 | header/obj/stream/endstream keywords, `%%EOF`, escape decoding, `n g R` |
| `pdf-parser` parser | 1 | indirect objects, version, object 4 is a stream, catalog found |
| `pdf-parser` streams (unit) | 6 | **a predictor on a content stream is undone**, a predictor read from its own entry in a `/DecodeParms` array, `/DecodeParms` without a predictor changes nothing, **`/Length` beats a payload that contains `endstream`**, an indirect `/Length` is followed, a trailing `\r` belongs to the payload |
| `pdf-parser` content | 8 | `"Hello World"`; `/MediaBox` size; **CTM placement of an image**; vector figure box; page-covering background rejected; 41% panel kept; thin rules ignored; inline images don't leak into text; `cm` nesting order |
| `pdf-parser` images | 1 | embedded JPEG extracted with correct magic, size, payload >10 KB |
| `pdf-parser` images (unit) | 7 | 1-bit expansion, PNG/TIFF predictors, PNG encoding |
| `pdf-parser` encoding (unit) | 10 | cp1252 punctuation, `/Differences` override, `bfchar`/`bfrange`, 2-byte codes, Latin-1 round-trip, **the plain character set's glyph names**, **the Computer Modern symbol names (`asteriskmath`, `dagger`, `plusminus`, `summationdisplay`, `epsilon1`, the extensible delimiters)**, **a Type 1 program's own `dup <code> /<glyph> put` encoding, and nothing after `eexec`**, **a declared `/Differences` still beats it** |
| `layout` unit | 28 | baseline merging, gutter splitting, hyphen joining, paragraph endings, first-line indent (and a centred line that is not one), headings with numbering depth, title selection, bullets/years/decimals not confused, captions, picture placement, table detection (and prose that is not a table), tightly spaced OCR lines, numbered sections vs list items, heading depth, **a rotated stamp joins no line**, **a rotated stamp is not the title**, **a margin stamp does not reorder the page**, **a two-row marker block is not a table** |
| `layout` real-corpus | 4 | **text is never lost or duplicated** (character multiset, all 32 PDFs), **every picture placed exactly once**, **no box escapes or collapses**, single-column pages stay in top-to-bottom order |
| `pdf-parser` filters | 6 | hex (padding, terminator), base-85 (`z`, partial groups, `<~ ~>`), run-length (literals, repeats, truncation), LZW against the spec vector, LZW round-trip at both `EarlyChange` values, malformed LZW does not panic |
| `ocr-engine` unit | 2 | TSV parsing, word grouping into lines, blank-row handling |
| `ocr-engine` integration | 1 | **real Tesseract** reads the scanned fixture (content assertions) |
| `pdf-parser` real-world | 10 | linearized + `/ObjStm` page tree, cp1252 punctuation, ligature `/Differences`, Identity-H, inferred word spaces, full paper, **vector-only figures**, **IRS background not a figure**, **placed image boxes stay on the page**, **the `∗`/`†`/`‡` of a TeX symbol font recovered from the font program** |
| `metafile` unit | 7 | the record chain walked in both layouts (the padded one Word and CorelDRAW write), polygon points as x-then-y, a DIB with a palette / bottom-up / 8-bit / a refused compressed one, the built-in font's glyph grid, and text laid out in a row |
| `metafile` real-corpus | 5 | a hand-built WMF (the coordinate order that caught the flipped Santa), a metafile that draws nothing and a truncated one refused, **all five `word/media` drawings of the corpus rasterise with no unsupported record, fill the frame they declare, and encode as a PNG a decoder reads back at the same pixel size** |
| `docx-parser` unit | 18 | paragraphs + heading/list labels, table grid, **equation linearisation**, **one formula item per `m:oMath`**, picture relationship resolution + EMU→point size, `w:sectPr` page size, external targets not read from the package, **list markers/levels from `numbering.xml`** (nested counters, roman start, symbol bullets), **`w:lvlOverride` relabels and `w:startOverride` restarts one list without disturbing the next**, a `numId` with no definition, **a shape written twice is extracted once**, **a tab in the text survives but a tab stop does not**, **a chart title is kept as a caption**, **footnotes and endnotes become `footnote` items** |
| `docx-parser` real-world | 14 | every math fixture yields a formula; radicals keep their degree; **n-ary operators from attributes**; binomial theorem; headings/lists/tables; embedded images (PNG/JPEG/EMF/WMF); **the three drawings of `VariousPictures.docx` played into PNGs, with no `image/emf` or `image/wmf` left**; **`mathtype.docx`'s `w:object` preview played into a PNG at its declared 30.85 x 28.3 pt (62 x 57 px)**; header content as furniture; A4 page size; prose kept alongside 12 tables; **`chartex.docx`'s six chart titles**; **`shapes-with-text.docx` extracted exactly once, tabs intact**; **`calibre_demo.docx`'s footnote and endnote** |
| `doc-parser` unit | 8 | paragraphs split on the paragraph mark, **a field instruction is not document text**, a **nested** field's instruction dropped and its result kept, a picture-only field yields a picture, cell marks become tabs and control bytes are dropped, drawn objects are pictures, CP1252 punctuation |
| `doc-parser` real-world | 8 | every fixture opens; an embedded object leaves no instruction text and one picture; two images with captions; `SHAPE` field dropped while captions survive; long prose read; a non-`.doc` file rejected clearly |
| `text-parser` unit | 25 | plain text per line; Markdown headings (ATX + setext), list markers/depth, pipe tables, verbatim code fences; HTML headings/paragraphs, script/head dropped, entities, tables, list depth, quoted `>` in attributes; RTF paragraphs, font/colour tables dropped, `\pict` skipped, `\uN` fallback not duplicated, `\uc0`, CP1252 hex escapes, ignorable destinations, escaped braces; CSV quoting, doubled quotes, CRLF, TSV |
| `text-parser` real-world | 2 | the repository README parses as structured Markdown (headings, prose, tables, marker stripped); the corpus README keeps its tables |
| `json-exporter` unit | 3 | the document is written where it was asked for, **no temporary file is left behind**, and an export replaces an existing file rather than appending to it |
| `cli` integration | 21 | the **real binary run as a child process**: default and explicit output paths, recursive directory mirroring, `--recursive false`, collision disambiguation, one parser+mimetype per format, **a flow document reports no page while a PDF keeps its own**, legacy `.doc`, parallel runs, the parallelism clamp warning, preview written for a PDF and skipped for flow formats, the unsupported/empty/missing-input error paths, **the `--progress` stream** (plan first, one record per document, counts running in order, every reported output path existing), **a failing document** reported as its own record while the rest of the run continues, and **the two drawing paths end to end** (a metafile drawing and an embedded object's preview each named in an `image` reference whose file is a real PNG, with no `.emf`/`.wmf` left beside the JSON) |
| `cli` golden (`docling_golden_test.rs`) | 23 | **every corpus document is a legal `DoclingDocument`** against the committed schema (required fields, no field a class disallows, enum spellings, resolvable references, children that agree with their parents), the format revision is the schema's own default, **`docling-core` itself accepts all 37 documents** (when `DOCLING_PY` is set), the comparison agrees with all 37 real Docling conversions within its budgets, and **nineteen unit tests exercise the comparison itself** (a matching pair, a missing page, a page of another size, lost text, LaTeX vs Unicode formulas, a dropped equation, a block that repeats and the figure exemption, a repeated character that must be present twice, a heading kept/demoted, a heading that swallowed its body, a fragment that is not a boundary, a table cell in a table / read as prose / uncovered) |
| `ui` unit | 84 | the argument vector matches the CLI (including output/`--recursive false`/`--ocr always`/`--preview`/`--progress`), **a `--selftest` run's own form** (a file's output named in a temporary directory of the run's own rather than beside the input, and a directory's laid out in it, since the smoke test is pointed at the corpus), a run stopped from the page, the CLI located beside the binary, an empty or missing input is rejected, the OCR value mapping, **the `--progress` parser** (plan, success, failure, a JSON object that is not a record, a `tracing` line, CRLF, colour escapes), **the tally** across a whole stream, **the field names of every event payload the page reads**, and **the comparison itself on hand-built documents**: two readings that agree row for row, blocks in body-tree order, **a group walked where it stands (nested included)**, an unmentioned block still compared, a reworded block as one row rather than a shift, a paragraph added or lost, `paragraph` against `text` not reported as a labelling difference, a mislabelled heading paired and named, an equation as notation and an equation only one side found, a picture with no image, rows read from the cell indices, the multiset agreement, an unpaired dollar as literal text, a missing page and a page of another size, a clipped row, a document too large to align (truncated and said so), a different kind of block never paired, a reference found as a file or as a folder of them, an unreadable reference reported rather than panicked, **the containment rule**: a run of reference items wholly inside one block of ours drawn as one contained row (the whole run kept on the row), a single contained item a containment rather than "text differs", wrong word order, a piece of one of our words and an item with no characters all not containment, a label the readings disagree about still named, an item the reference kept apart from the run folded onto the block that holds it, a row the readings agreed on gaining the items inside our block, a wording difference not hidden behind the fold, and letter-spaced text of ours still matching; **the leaders of a form line**: a line read as two word fragments with leader items between them one row (four items, two by words and two by place, nothing left one-sided), a leader among the fragments of a line we read taken while one standing alone where we read a caption left to the joining by position, a leader on another page never taken (and now **placed** there rather than listed), an item with words we do not hold keeping its own row however well its box fits, and the reference's items on a row standing in the reference's order rather than behind the label the alignment paired; **the rest of a form's leaders**: a line our reading produced no block for at all answered by placement (a row naming what it is and its page and box, none of them among the rows of text we did not read; the items of such a line are **one** row — see the group after this one), an item with no letters on a page we never read placed the same way rather than taken onto the line of ours it does not belong to, an item with letters and no block of ours beside it still **only the reference**, and a letterless item with no box (a flow format) left exactly as it was; **a line of a form as one row**: the items of one line placed together, the row saying how many it stands for and the extent they cover, another line of the same page their own row, a block of ours on the line not splitting two leaders while an item of the reference's own between them does, and the header counting placed *rows* rather than items; **the box a placement is drawn from**: a placed row carrying the page's own size with the box as fractions of it, inside that outline and never empty, a row that is not a placement carrying none, and a page whose size was never stated named with its points in the note and drawn nowhere (and the per-page picture empty rather than given a shape); **the placed items of a page**: the items of a document grouped by the page they lie on for the per-page drawing — the *items* and not the rows, pages in ascending order with the reference's own order kept within each, each box the item's own place in fractions of the page and inside it, and the picture holding every placed item exactly once; **the boxes in the score**: two pictures the readings list in opposite orders paired by their boxes rather than by the list (and the sizes on the rows agreeing because of it), two *identical* pictures the same way instead of each being accused of standing somewhere else, a block both readings read the same words of in different places (or on different pages) a **differs** row naming both, and blocks in the same place whose words link nowhere still two rows joined by position rather than paired by the nudge; and **the joining by page and box**: a picture and a paragraph the readings ordered differently, the joined row taking the place of the first of the two it replaces, two boxes on different pages, two boxes that barely touch, a bottom-left box with no page size (left alone), a formula beside a paragraph in the same box, two pictures swapped between the readings, two pictures on different pages, and a document with no boxes at all compared exactly as before |
| `ui` compare (`tests/compare_test.rs`) | 5 | the real CLI converted corpus documents compared with the committed Docling references: **three equations as three notation rows** with nothing one-sided (`math-nary.docx`), **all five drawings of `VariousPictures.docx` answered by one of ours** (each paired, each an image, and the only one-sided rows the reference's own empty text items), **`mathtype.docx`'s OLE preview shown by both sides** (the two readings order it and the text the other way round, so each side's picture stands alone beside the same prose), a document with no reference reported with the folder it was looked in, and **the three PDF references, whose geometry both producers publish**: every block on both sides carries its page, every row joined by position has both sides and says which page and how much box joined it, at least one such row exists in the corpus (60/11/6 rows across `tracemonkey.pdf`/`arxiv_attention.pdf`/`irs_f1040.pdf`); and **the placed rows are a form's leaders** — every row whose verdict is `placed` is the reference's items with no letters in them at all — the items of one line of a form named as one row — on a named page, whose note names the box it lies at and says there are no words to compare, whose `placement` is that box as fractions of a page's own size and lies inside it, and the summary's `placed` count is exactly the number of such rows: **60** of them across the three references standing for **369** items, **25** of them `irs_f1040`'s standing for 327 |
| `ui` corpus report (`tests/corpus_report_test.rs`) | 9 | the mode over **every** document the corpus has a reference for: all 37 converted in one CLI run, each compared through `diff::compare_document`, printed worst first — and, because a report missing a document is worse than no report, every reference compared, every verdict tally equal to the rows sent, and no document without a reading; plus **the ratchet against `test-data/golden/compare-baseline.json`**: a document with more `differs`, more rows one side only or a lower character agreement fails with itself and the counts named, a document the baseline does not have fails, an entry no document produced fails, an improvement (or a move in `same`/`contained`/`joined`) passes, and `CORPUS_BASELINE=update` rewrites the file from the run instead of checking it; and, with it, the **placed rows of a form**, which are a *shape* rather than a count: the same items standing in more rows (327 items back as 327 rows instead of 25) fails, the same boxes in more page pictures fails, *fewer* rows for the same items or *more* items placed pass, and a document that placed nothing before is never a regression from nothing — **plus a floor of its own on the placed items themselves**, which the rate is not a substitute for: half the panel in half the rows keeps the ratio and fails, a form remembered for a document and answered no more at all (0 items in 0 rows, which no ratio refuses) fails, and the floor stands on neither a document that placed nothing before nor items that only rose. Seven tests of `mod placed_rows` hold the rule itself; the corpus row's eighth shows it end to end. |
| `ui` process (`tests/job_test.rs`) | 7 | arguments forwarded to a real program and its stdout captured; a non-zero exit kept with its stderr; a missing program returned as an error; **output arriving while the run is still going** (a line is received within seconds of a 30-second process); **a running process stopped** (the stop is recorded, the wait returns promptly, the verdict is *cancelled* with no exit code); stopping a finished run does nothing; and a run that completed as it was stopped is reported as a success |

## Real-corpus results (after the fixes above)

**37 of 37 documents convert** (the 36 documents plus the corpus README, which is
Markdown). The four legacy `.doc` files that used to be the only failures now convert
too; the E2E run ends with 0 failures.

### Pictures found across the corpus (92 total, 25 with an image)

"Image" is a picture whose payload Docling can name in an `image` reference; "box only"
is a picture reported with its box and no `image` — a PDF vector figure we do not
rasterise, an OfficeArt drawing we cannot locate the pixels of, or a media type Docling's
own registry rejects.

| Document | Pictures | Image | Box only |
|---|---|---|---|
| `FloatingPictures.doc` | 18 | 0 | 18 (OfficeArt placeholders) |
| `math_gan.pdf` | 12 | 6 | 6 (vector panels) |
| `math_seq2seq.pdf` | 11 | 0 | 11 (vector) |
| `math_layernorm.pdf` | 9 | 0 | 9 (vector) |
| `arxiv_attention.pdf` | 8 | 3 | 5 (vector) |
| `testPictures.doc` | 8 | 0 | 8 (OfficeArt placeholders) |
| `tracemonkey.pdf` | 5 | 0 | 5 (vector) |
| `VariousPictures.docx` | 5 | **5** | 0 (the EMF/WMF drawings are played) |
| `drawing.docx` | 5 | **5** | 0 (incl. the CorelDRAW WMF behind a `w:object`) |
| `calibre_demo.docx` | 4 | 4 | 0 |
| `math_batchnorm.pdf` | 2 | 0 | 2 (vector) |
| `two_images.doc` | 2 | 0 | 2 (placeholders; the payloads are not located yet) |
| `equation.doc` / `headerPic.docx` / `mathtype.docx` | 1 each | 1 each | 0 |
| `chartex.docx` | **0** | — | — |
| `irs_f1040.pdf` | **0** | — | — |

`irs_f1040.pdf` reporting zero is the *correct* result: its only large painted region is
the form's own background. `basicapi.pdf` also reports zero because it genuinely has no
image XObject. `chartex.docx` reports zero pictures and six captions instead: a chart is
drawn by a chart part, and the words a reader sees are its title, which is now where it is
reported from. The dpi values are a useful sanity check: the arXiv figures come out at
exactly **500 dpi** — the round number their producer would have used — and every drawing
that was played comes out at **144 dpi**, the resolution it was rasterised at.

### Formulas from the DOCX math fixtures

Docling's own output for the same files is `\iiint_{1}^{2}x+1`, `\prod_{a}^{}b`,
`\sum_{}^{2}x` — the same three equations, in the same order, and the golden comparison
holds us to the count (`math-nary.docx`: **3/3**, `math-matrix.docx` and
`math-literal.docx`: **1/1** each).

```
math-rad.docx                   √(4)   √[3](x+1)
math-nary.docx                  ∰_1^2 x+1   ∏_a b   ∑^2 x
math-matrix.docx                ([1, 2; 3, 4])
math-lim.docx                   lim_(x→1) x
math-accents.docx               áàǎăåa⃗ãâȧa⃗a⃗ãa(bar)
EquationWithAboveAndBelowCaption.docx
                                (x+a)^n=∑_(k=0)^n (n/k)x^ka^(n-k)
                                (1+x)^n=1+nx/1!+(n(n-1)x^2)/2!+…
math-bold-italic.docx           f(x)=a_0+∑_(n=1)^∞ (a_ncos (nπx)/L+b_nsin (nπx)/L)
```

### Text volumes, before and after layout analysis

"Spans" is what the parser produced - one item per text run, in drawing order. "Blocks" is
what layout analysis reports, in reading order. The collapse is the point: runs become
paragraphs instead of fragments.

| Document | Pages | Text items | Tables | Pictures | Notes |
|---|---|---|---|---|---|
| `tracemonkey.pdf` | 14 | 296 | 7 | 5 | 2 footnotes, 22 section headers, 14 page footers; 21 control chars remain (see below) |
| `math_layernorm.pdf` | 14 | 295 | 6 | 9 | 5 footnotes, 25 section headers; TeX math fonts name their glyphs now (see below) |
| `arxiv_attention.pdf` | 15 | 285 | 12 | 8 | **1 title** (the sideways stamp is furniture); **5 footnotes**; prose merged into paragraphs |
| `math_seq2seq.pdf` | 15 | 308 | 2 | 11 | **9 footnotes**; three "titles" before the fix, one now |
| `irs_f1040.pdf` | 2 | 74 | 10 | 0 | was 0 pages before the fixes; the form's field lines are grids, not notes |
| `calibre_demo.docx` | 0 (flow) | 85 | 6 | 4 | 17 headings, 21 list items (19 with markers), **a footnote and an endnote** |
| `drawing.docx` | 0 (flow) | 157 | 12 | 5 | 12 real table grids |
| `basicapi.pdf` | 3 | 9 | 0 | 0 | Identity-H decoded, ToC lines, **both footers recovered by `/Length`** |
| `dummy.pdf` | 1 | 1 | 0 | 0 | sanity case |

Across the whole corpus: **2,180 text items, 66 tables and 92 pictures** - of the text
items, **8 are titles, 183 section headers, 84 list items, 49 captions, 40 formulas, 26
footnotes, 21 page headers and 65 page footers**.
The collapse from the parser's per-run spans to these items is the point: `arxiv_attention`
starts from 850 runs in drawing order and ends as 285 items in reading order. Every title
is a document's real title (`Attention Is All You Need`, `Table Of Content`,
`Trace-based Just-in-Time Type Specialization for Dynamic Languages`, ...), and numbered
sections come out as `section_header` with their depth (`3.2.1` -> level 3, marker `3.2.1`).

### Comparison with real Docling (37 references, 0 mismatches)

Measured by `cargo test -p pdfextractor-cli --test docling_golden_test -- --nocapture`.
"Agreement" is the share of the reference's alphanumeric characters that are also ours,
counted as multisets (a character the reference repeats twice must appear twice here);
formula characters are excluded because the two write different notation for the same
equation, and the equation counts carry that check instead.

The run prints one line per reference — pages, agreement, formulas and the label
counts — so the numbers below are reproduced rather than transcribed here.

What is compared: page count and page size (within a point), text as a character multiset,
equation counts, and "no block appears twice in a row". What is deliberately not compared:
formula notation (LaTeX vs Unicode), item labels and block boundaries (Docling splits a
paragraph at every formatting run — this is the whole of the remaining 0.1% on
`calibre_demo.docx`), and bounding boxes (there are none on either side for a flow format).
The suite, the label and boundary comparison, the budgets it records and its two Python
artefacts are documented in `test-data/golden/README.md`; the full per-document report
(agreement, headings, merges, table cells) is printed by the test on every run.

Where the corpus stands (all 37 references, after the fixes in bugs 47-49):

* **Text agreement 0.887-1.000.** `README.md` is the lowest at 0.887 (Docling splits its
  Markdown at every formatting run); the four PDFs with the most math are 0.991-1.000.
* **Pages** match on all seven PDFs; **formulas** match everywhere a reference
  carries one. No document has a block that repeats (bug 46's rule), and no table cell
  is read as prose under a table we drew.
* **Headings:** 157 of the 195 reference headings are held under a heading label. The 38
  misses are itemised and budgeted (see the README): `drawing.docx` 15,
  `math_seq2seq.pdf` 10, `math_layernorm.pdf` 4, `math_gan.pdf` 3, `FloatingPictures.doc`
  3, and one each on `dummy.pdf`, `irs_f1040.pdf` and `tracemonkey.pdf`.
* **Boundaries:** five blocks merged across one — four in the `irs_f1040.pdf` header
  block, one between `arxiv_attention.pdf`'s attribution stamp and its title.
* **Tables:** 104 reference table cells are in a table of ours, 0 are prose under one of
  our tables, and 99 are prose with no table of ours over them — our conservative table
  detection, counted and printed rather than asserted away.

## Known limitations (honest list)

- **Legacy `.doc` is read, but without character formatting.** The container, Fib and
  piece table are parsed and the text (and its field/page-break/picture structure) comes
  out, but `PAPX`/`CHPX` are not read, so three consequences follow: paragraphs get no
  style (so no headings or list markers), table cell marks flatten to tabs instead of
  becoming grids, and inline pictures are placeholders because the OfficeArt raster blobs
  cannot be located without the character formatting that points at them. The page size is
  the Word default for the same reason.
- **Flow formats carry no page geometry.** DOCX, legacy `.doc`, RTF, HTML, Markdown and
  plain text are streams of blocks: text/table/picture items have an empty `prov` and the
  document reports **no pages at all** — Word repaginates as the text is edited and no
  block sits on a page rectangle, which is also what Docling publishes for one. The size a
  DOCX declares in `w:sectPr` is read by the parser but is not turned into a page. A
  `--preview` is skipped for them with a log line.
- **A DOCX drawing is played into a PNG; a PDF vector figure is not yet.** Word's EMF and
  WMF drawings — and the metafile preview of an embedded object — are rasterised by
  `crates/metafile`, so they are named in an `image` reference like any other picture
  (bug 50). Nothing on the PDF side is rasterised, so a PDF figure is still a picture with
  a box and no `image`. `image/wmf`/`image/emf`/`image/jp2` remain unnameable in an
  `image` reference, which is why a metafile that will not play keeps its own payload and
  its box and is not referenced — the drawing is then written unreferenced beside the JSON
  rather than turning into a blank picture.
- **RTF headings and tables are not rebuilt.** Their structure lives in the skipped
  `\stylesheet` and in `\trowd`/`\cellx` runs; paragraphs and tables therefore flatten to
  tab-separated text rather than `section_header`/`table` items.
- **RTF `\'hh` bytes are decoded as CP1252** regardless of `\ansicpg`; a document in
  another code page loses the non-ASCII characters outside its ASCII range.
- **Markdown and HTML keep inline markup as written.** `**bold**`/`<em>` are not stripped,
  so a `paragraph`'s text is exactly the source characters between the block markers.
- **A directory run disambiguates colliding output names.** Two inputs with the same stem
  but different extensions (`report.pdf`, `report.txt`) would otherwise write the same
  JSON; the colliding ones keep their full name (`report.txt.json`).
- **DOCX blocks carry no page geometry.** Word is flow-based: there is no page
  rectangle to point at, so text/table/picture items have an empty `prov`. The page
  itself is reported from `w:sectPr`. Layout modelling would be needed to place blocks.
- **OMML is linearised, not converted to LaTeX.** `x^2`, `a/b`, `root[3](x+1)`,
  `sum_(k=0)^n` - readable and structure-preserving, but not round-trippable and not
  guaranteed to match TeX. Nested scripts flatten (`x^6_3_4^5_2^1`).
- **MathType/OLE equations are unrecoverable as text.** They are an embedded binary
  object, not OMML; `mathtype.docx` yields prose but no formula text. Its *preview* is a
  picture now, and the text inside it is drawn in the built-in font, so the characters are
  approximations of the equation rather than its glyphs.
- **Pages that mix a text layer with scanned regions** are not OCRed in `auto` mode
  (OCR runs only when a page has *no* text at all). Use `--ocr always`.
- **Rotated text is placed and classified, but its angle is not *used*.** The interpreter
  composes the text matrix with the CTM, so a sideways run gets a rotated box and joins no
  horizontal line, and a rotated run can no longer be titled. A rotated block standing in
  the page's **outer margin** is labelled as the page's own furniture (`page_header`,
  Docling's label for it); one inside the text is read as the document's, which is as far
  as the flag can take it — the *angle* itself is discarded, so nothing downstream knows a
  box is 90° from the rest, and rotated *tables* are still read as ordinary text.
- **Page furniture is claimed only where the document proves it.** A running head, a footer
  or a page number is labelled when the same line (digits aside) stands in the same margin
  of two or more pages; a stray word that happens to sit 6% up from the bottom of one page
  is left exactly as it was read, and so is everything on the pages it stands alone on. Six
  of Docling's 90 furniture items across the corpus are therefore still the text we read them
  as: `irs_f1040.pdf`'s five lines of small print (four of them on a single page) and
  `arxiv_attention.pdf`'s page-1 conference footer. The pass also cannot reach a footer read
  as part of a **table** (it relabels text items, and nothing in the corpus's furniture lands
  in a detected table).
- **A note at the foot of a page is found by two marks, and both are asked for.** The
  **rule** the page draws above its notes, and the **marker** a note opens with
  (`4To illustrate why the dot products get large…`), which has to be one the *body of that
  page* also uses — `arxiv_attention.pdf`'s page-1 `31st Conference on Neural Information
  Processing Systems (NIPS 2017)` line stands under the very rule its three notes stand
  under and opens with a number, and its number is not one the text uses, which is what
  keeps it the `page_footer` Docling calls it instead of a note. The band is the page's
  lower 30% (the highest note in the corpus, `arxiv_attention.pdf` page 1's
  `∗Equal contribution…`, opens 24.8% down) and the rule may stand up to 30 pt above the
  block (on `math_seq2seq.pdf` page 4 the rule and the note are 23 pt apart). The pass runs
  *after* the furniture, which is what stops it taking a page number: a note is the
  document's text, the label `footnote` is not a furniture label, and a page number must
  stay the page's.
- **One thing stops a note being labelled, and it is the page's, not the pass's.** Measured
  against Docling's 25 reference notes in the five PDFs: **23 are labelled** (all five of
  `arxiv_attention`'s, all five of `math_layernorm`'s, all nine of `math_seq2seq`'s, all four
  of `math_gan`'s) and **2 are read as paragraphs** because the page draws **no rule** above
  them (`tracemonkey`'s two, which Docling still calls footnotes). **None is missing.** The
  eight that were the table misread (`math_seq2seq` notes 3, 4, 6, 7 and `math_layernorm`
  notes 2–5) are labelled now that `analyze_page` is given the page's rules, and the four the
  symbol fonts were hiding (`math_gan`'s three author lines and `math_seq2seq`'s `∗CIFAR
  Senior Fellow`) now that the font program's own encoding is read. The two false positives an
  earlier, looser rule produced are gone: the `arxiv_attention` conference line and a
  reference-list item opening `1:`.
- **Layout is geometry, not semantics.** Reading order comes from whitespace bands, so a
  footnote or a running head that sits at the same height as body text can be ordered into
  it. A vertical band only counts as a gutter when both sides run down the page, which is
  what stopped `basicapi.pdf`'s corner stamp from being read as a left column ahead of the
  heading; a genuine two-column page is unaffected.
- **A paragraph break needs a signal.** It is recognised when a short line is followed by
  a full-width line, on an indent, on a size change, or at a bullet. Two short lines in a
  row stay merged, because nothing in the geometry separates them from the last two lines
  of a single paragraph.
- **Table detection is conservative and span-based.** A row written as one text run with
  wide spaces is not treated as a table (missing one is cheaper than corrupting prose),
  cells come from span geometry rather than an estimator, and a table whose columns are
  separated by a full-height gutter is read as columns of prose. The one thing that
  overrides it is the page's own rule: a note block at the foot *under a rule* is the
  page's notes, not a grid, which is why `analyze_page` takes the page's rules.
- **OCR line size is estimated** from Tesseract's box height (`0.8 x height`), so heading
  detection on a scanned page is relative. It is consistent within a page, which is what
  the ratio tests need.
- **Figures are found heuristically**: clusters of painted paths between 1.2% and 50% of
  the page area, at least 24 pt on a side. Vector figures that a producer draws larger
  than half the page are missed; a figure sitting behind a raster image is suppressed.
- **A PDF figure with no raster pixels is reported as a box with no `image`.** Nothing on
  the PDF side is rasterised, so `image.uri` is absent and no sidecar file is written - by
  design. A DOCX drawing is rasterised (bug 50) and so is never in this state unless the
  player refuses it.
- **`image.size`/`image.dpi` are omitted when genuinely unknown** — a metafile we could
  not play has no pixel grid. For raster images and played drawings dpi is derived from
  the pixel grid and the placement box, which yields exactly 500 dpi for the arXiv figures
  and 144 dpi (the resolution they were rasterised at) for the played DOCX drawings.
- **`/Length` is honoured, but only when it is checkable.** A stream's declared length is
  used when the byte after it really is `endstream`; otherwise the lexer's terminator
  search stands. The search itself still decides where payload *tokenisation* resumes, so
  a file whose declared length disagrees with its terminator can leave the parser
  resynchronising by scanning for `N G obj` triples - which is what it does for every
  malformed object anyway.
- **Glyphs whose code is absent from both `/ToUnicode` and `/Differences` cannot be
  recovered** - the mapping lives in the embedded font program, which we do not parse.
  This is the residual 21 control characters in `tracemonkey.pdf` (0.03% of its text) and
  the reason TeX math fonts (`CMSY`/`CMMI`/`CMEX`) produce unusable glyphs. Unmappable
  control characters are stripped rather than emitted.
- **A run is held to as much of the ratchet as its settings can speak for.** The memory
  beside the references records how it was measured (its `measured` block: the OCR mode),
  and a run whose settings match — the corpus run, or the window set to `--ocr never` — is
  held to every rule. A run made with different settings is held only to the **floor**
  under a form's placed items, because `differs`, the one-sided rows and the character
  agreement are counts an OCR mode moves, and a baseline measured with OCR off cannot
  judge them. So the window at its default `--ocr auto` refuses only forms that fell, and
  everything else about a document that got worse — its `differs` rising, its agreement
  dropping — is held by the corpus report and by `--compare` runs measured `--ocr never`,
  not by a window that was measuring something else. The memory is found by *name* and
  keyed by file stem, so a document of yours that happens to share a stem with a corpus
  document would be held to that corpus document's floor, and a memory that is there and
  cannot be read is reported on the panel (and refused by `--compare`) rather than
  silently ignored.
- **The Compare mode's boxes are evidence, not proof.** Where a block sits is part of how
  two blocks are matched: a pair the two readings put in the same place is worth a tenth
  of a link more than its words say, and a pair they put in different places loses as much
  as a link needs, so the boxes decide which of two pairings the words cannot tell apart is
  taken (`PLACE_BONUS`/`PLACE_COST` in `crates/ui/src/diff.rs`) and a block that moved is a
  **differs** row naming both places. It is a heuristic with two constants, not a proof,
  and it can take a pairing away as well as add one: a block the boxes point somewhere
  else takes the partner they agree about, leaving the block it used to be paired with
  standing alone (measured: 29 more rows only the reference across the corpus, while
  `differs` fell by 23 and 7 more rows were joined). A containment row — a run of the
  reference's items inside one of our blocks — is not asked where it sits, because the
  words are what make a run a run, and a paragraph of ours crossing a page break would
  otherwise have every item on its later half reported as standing somewhere else. Two
  equations are exempt from the rule as well — their text is a rendering rather than
  words, so there is nothing for the boxes to weigh — and boxes
  exist only where the producer publishes them, which the PDF producers do and the flow
  formats (DOCX, `.doc`, Markdown) do not, so `mathtype.docx`'s ordering disagreement is
  still shown twice, once a side. Item *counts* are not
  compared either, for the golden suite's own reason: Docling splits a paragraph at every
  formatting run and writes an item with nothing in it where it has nothing. Those are
  now named rather than counted as a loss where the reference's letters are a run of our
  words (**contained** — and where a run cannot reach an item because the reference put
  something we do not hold in the middle of it, the fold takes that item onto the row of
  the block that holds its words — its leaders with it), which leaves one honest one: an
  item whose letters lie inside one of our blocks but not in the words of any single one of
  them — split across two blocks, or reworded — stays one-sided. The leaders of a form line
  our reading produced a block for are on that line's row, and those of a line we read *no*
  block for are **placed** — `irs_f1040`'s remaining 327 letterless items are now places with
  a page and a box on them (`place_letterless`), not rows calling punctuation text we lost.
  What a placement cannot say is whether our reading *should* have read that line: it
  reports that the reference wrote something with no letters there and that we have no block
  for it, which is all the words and the boxes can support. What it does about the count is
  group the items of one line: a line's runs of dots and the rule under them are **one** row
  that names how many it stands for and the extent they cover (`collapse_lines`), so a line
  our reading read no block for is one row of the panel rather than twenty — and two such
  items that are not neighbours in Docling's order, or that stand on different lines, are
  still placed apart.
- **Comparing is per document, and needs a reference per document.** The reference folder
  is matched by stem (`report.json`) and then by whole name (`report.txt.json`, for a run
  that had to disambiguate a collision); a document with neither is reported as
  uncompared, with the folder it was looked in.
- **`/Comment`-style unknown filters are still returned raw** rather than decoded; a
  stream using one keeps its bytes and the caller decides what to do with them.
- **The UI renders at most 300 rows and 2000 log lines.** A folder of ten thousand
  documents still converts — the bar and the counts are complete — but the list shows the
  first 300 with a `…and N more documents` line, and the log keeps the most recent 2000
  lines, so the page cannot be made to crawl by a large run.
- **The UI's document list is only as detailed as the CLI's stream.** Queued rows come from
  the plan line and each row is updated when its document finishes, so there is no
  per-document *progress* (a single huge PDF shows as one row going from queued to
  converted), and no pause/resume — only stop.
- **The UI still has no drag-and-drop, and only one window.** `--parallel` is the only way
  to use several cores (`rayon` inside the CLI), so the window itself stays idle while a
  run proceeds.
- **`--preview` from the UI only produces an overlay for PDFs.** The CLI says so and skips
  flow formats, and the UI merely forwards the flag.
- **No GPU path.**

## Next steps (priority order)

Everything the previous revision of this list called a defect has been fixed and covered by
an assertion; what is left is genuinely new capability.

1. ~~**Golden references for the whole corpus**~~ **Done** — all 37 corpus documents have a
   reference in `test-data/golden/docling/`, and the corpus report and its ratchet compare
   every one of them (`test-data/golden/README.md` has the provenance and the version
   caveat).
2. ~~**Running heads, footers and page numbers as furniture**~~ **Done** — the document-wide
   pass above labels 86 items across the seven real PDFs as `page_header`/`page_footer` on
   the furniture layer, 84 of them agreeing with Docling, and it moved 83 rows from
   `differs` to `same`.
   - ~~**Footnotes**~~ **Done** — `mark_footnotes` follows the marker in the body and the rule
     above the note; layout is given the page's rules too, so a marker-and-text block at the
     foot *under a rule* is handed to the pass as rows rather than built into a table; and the
     font program's own encoding is read, so the `∗`/`†`/`‡` a TeX symbol font draws are the
     characters they are. **25** of the corpus's **27** reference notes come out labelled (23
     of the 25 the five PDFs carry, plus `calibre_demo`'s two, which are structural rather
     than drawn); the other **2 are still in the text**, read as paragraphs, and **none is
     missing**. The two that remain are `tracemonkey`'s, which stand under **no rule at all**
     although Docling calls them notes: the marker and the foot of the page are all the
     evidence there is, and accepting that alone is what brought the two false positives back
     (see the limitation above).
   - **Captions** are still the body's past the ones that announce themselves (`Figure 3:`),
     and so is the rest of this item — the small print of a form and a one-off footer — which
     needs a rule that does not confuse it with the paragraph that opens a page.
3. **A rotation angle on every box**, so rotated *tables* and sideways headings can be
   grouped and ordered deliberately rather than merely kept apart. (The flag itself already
   decides the page's margin stamps; the *angle* is what is still thrown away.)
4. **Rasterise a PDF's vector figures** - the DOCX half of this is done: `crates/metafile`
   plays Word's EMF/WMF drawings into PNGs (bug 50). A PDF figure is still a box with no
   `image`, and the same treatment would make the picture items uniform. `math_seq2seq.pdf`
   (11 figures, none with an image) is the case to judge it on.
5. **AcroForm fields** - `irs_f1040.pdf` carries 199 `/Annot` objects; form-field text is
   not surfaced yet.
6. **Character formatting for `.doc`** (`PAPX`/`CHPX`) - it would unlock paragraph
   styles (headings, lists), real table grids, and the OfficeArt offsets needed to raster
   the inline pictures that are currently placeholders.
7. **ODF and the rest of OOXML** - `.odt`/`.ods`/`.odp` are ZIP + XML like DOCX, and
   `.pptx`/`.xlsx` share that container, so a single package walker could cover them.
8. **Per-document progress inside a file, and pause/resume** - the window now streams
   the queue, the per-file results and the CLI's log, and can stop a run; what it cannot
   show is how far *one* large document has got, because the CLI only reports a document
   when it finishes. Reporting per page (or per document phase) is the next increment,
   and it would also make a pause meaningful.
9. **Bundling** - `bundle.active` is false: the UI runs from `cargo run`. Producing
   `.deb`/`.app`/`.msi` installers needs an icon set per platform and a signing story.
10. ~~**Stop segmentation from deciding the panel.**~~ **Done in part** — the "contained"
   verdict above collapses one block of ours against the run of reference items wholly
   inside it, and the fold then takes the items a run cannot reach onto the row of the
   block that holds them, the leaders of the line among them (288 rows across the corpus at
   present; `README` 200 → 96 "only the reference", `drawing` 167 → 85, `irs_f1040`
   682 → **416** → **89**, the 327 that left it now placed rather than listed). What is left:
   an item whose letters lie inside one of our blocks but *not* inside any single block's
   words in order — split across two blocks of ours, or reworded — still stands alone, and a
   placement claims only that the reference wrote something with no letters there and that we
   have no block for it. Pairing whole regions (by box, not just
   leftovers) is what would read the rest of the panel as the differences they are.
11. **Look at `tracemonkey`'s affiliation markers.** Our text carries `#`, `$` and `+`
   where the reference carries `∗` (U+2217): 138 `#` against 4 and 59 `+` against 16, on
   an author line the two tools otherwise read identically. It costs no characters in the
   agreement figure (punctuation is not counted), so only the row comparison can see it,
   and it is either a glyph mapping we get wrong or a font whose own encoding says
   something else — the panel says which lines to look at; nothing has been diagnosed.
12. ~~**Let the boxes correct a pairing, not only create one**~~ **Done** — the box overlap
   is in the alignment's own score (`link_score` in `crates/ui/src/diff.rs`): the words
   still decide whether two blocks are a link at all, and the boxes decide which link is
   taken where the words offer more than one (a bonus of a tenth of a link where they
   agree, a cost of as much as a link needs where they contradict). Two pictures with the
   *same* text that the readings list in opposite orders are paired by their boxes now
   rather than by the list, and a block both readings read the same words of in different
   places is a **differs** row that names both places (`not in the same place: page 1, 8%
   of the smaller box shared`, or `page 1 vs page 2`). Measured cost and what it bought:
   see the corpus prose above (23 rows out of `differs`, 29 into `only the reference`,
   6 out of `contained`, 7 more joined, and 60 → 17 rows accused of standing somewhere
   else across the three geometry references). What is left: the score is a heuristic with
   two constants rather than a proof, an equation pair is exempt from it (its text is a
   rendering, so the boxes have nothing to add), and two readings are still compared per
   document with item counts deliberately not held against each other.

## Recovery notes

- Build and verify: `cargo build --workspace && cargo test --workspace`.
- Smoke-test the whole UI feature (window → webview → frontend → IPC → CLI events → DOM)
  without a human and without looking at the screen:
  `cargo run -p pdfextractor-ui -- --selftest <path>`. It must print `webview-ready`,
  `selftest-started run=1 …` and then `selftest-rendered …` carrying the page's own state
  (`state="5 / 5 documents · 4 converted · 1 failed · done"`, a bar width, one row status per
  document), and exit 0. Add `--selftest-stop 2600` to check the stop path: it must add
  `selftest-stop … stopped=Some(1)`, and the page must report `title="Stopped"`, with every
  `*.json` written before the stop still parsing and no `*.tmp` left behind. The path may
  be the corpus itself: a self-test writes what it converts into
  `/tmp/pdfextractor-selftest-<pid>/` (`Settings::selftest` in `crates/ui/src/lib.rs`) and
  never beside its input, so nothing has to be copied out of the way first and
  `test-data/real` is left exactly as it was found. That directory is named on the
  `selftest-started` line, which is where to look at what was converted.
- Look at the page without building the app: serve `crates/ui/dist` (`python3 -m http.server`)
  and open `index.html?mock=1`; `dist/mock.js` plays a scripted run (three documents, the
  middle one failing) driven by the real page code.
- Run the window for real: `cargo run -p pdfextractor-ui`, then pick a file, press Convert,
  and watch the rows fill in; press Stop and confirm it reports *Stopped* rather than a failure.
- Exercise the CLI the way the UI does:
  `cargo test -p pdfextractor-cli --test cli_test` (20 end-to-end tests through the binary).
- Watch the stream by hand:
  `cargo run -p pdfextractor-cli -- -i test-data/real -o /tmp/real --progress | head -5`
  — the plan line first, then one record per document as each finishes.
- Reproduce OCR end to end:
  `cargo run -p pdfextractor-cli -- -i test-data/scanned.pdf -o /tmp/s.json`
  then confirm `texts` contains `ACME Supplies` and `Total Due`.
- Fixtures: `hello.pdf` (plain), `compressed.pdf` (FlateDecode), `scanned.pdf`
  (JPEG-only page). All were generated with Python (`zlib` and PIL) and are committed.
- Real corpus: `test-data/real/` (see its README for provenance). Run the whole set with
  `cargo run -p pdfextractor-cli -- -i test-data/real -o /tmp/real -p 4` and confirm
  `irs_f1040.pdf` yields pages (it is the linearized + object-stream case), that
  `equation.json` has one picture and no text, and that the run ends with **0 failures**.
- Reproduce a legacy `.doc` conversion:
  `cargo run -p pdfextractor-cli -- -i test-data/real/two_images.doc -o /tmp/two.json`,
  then confirm `texts` holds `the jpg file` / `the png file` with two `pictures` between
  them.
- Extracted images land in `<output-stem>_images/`. Verify their extensions are truthful:
  `for f in /tmp/real/*_images/*; do file -b "$f"; done` — an EMF must not be named `.png`,
  and after the drawings are played no `*.emf`/`*.wmf` is written at all.
- Reproduce the drawing path (bug 50):
  `cargo run -p pdfextractor-cli -- -i test-data/real/VariousPictures.docx -o /tmp/v.json`,
  then every `pictures[].image` names a file in `/tmp/v_images/` and all three metafiles of
  the package have become PNGs — upscaled, they are a Santa, a cow and a wrench. The
  embedded-object case is
  `… -i test-data/real/mathtype.docx -o /tmp/mt.json` (a 62x57 PNG: the equation's preview)
  and `drawing.docx`, whose CorelDRAW WMF is the first picture. The players themselves:
  `cargo test -p pdfextractor-metafile`.
- Reproduce the DOCX path:
  `cargo run -p pdfextractor-cli -- -i test-data/real/math-nary.docx -o /tmp/m.json`
  then confirm `texts` contains a `formula` labelled `∑`.
- Reproduce the overlay:
  `cargo run -p pdfextractor-cli -- --preview -i test-data/scanned.pdf -o /tmp/s.json`
  then open `/tmp/s.preview.html` — it must show the page with 6 orange OCR boxes, and
  `/tmp/s_pages/page-1.png` must exist when `pdftoppm` is installed.
- Measure box alignment without looking: crop each box from the rendered page and compare
  its ink density against the same box shifted by 55% of its width. Inside must win by a
  clear margin (observed 1.6× on OCR, 2.1× on a text layer).
- To check the no-renderer path, run the binary with `PATH` emptied of `pdftoppm`:
  `env PATH=/tmp/emptybin ./target/release/pdfextractor-cli --preview …` — the HTML must
  still be produced, with the extracted image drawn inline instead of a page backdrop.
- If Tesseract is missing: `apt-get install -y libtesseract-dev libleptonica-dev
  tesseract-ocr tesseract-ocr-eng libclang-dev`, or build with `--no-default-features`.
- Reproduce layout analysis end to end:
  `cargo run -p pdfextractor-cli -- -i test-data/real/arxiv_attention.pdf -o /tmp/a.json`
  then confirm `texts[1]` is the `title` "Attention Is All You Need" (the Google
  attribution stamp above it is a `section_header`), that numbered sections are
  `section_header` with `marker` `3.1`/`3.2` and `level` 2, and that the document reports
  13 `tables`. Before the `BT` fix the same command produced boxes at
  y = -537 and beyond.
- Layout invariants over the whole corpus: `cargo test -p pdfextractor-layout`. The
  `real_corpus_test.rs` suite parses every PDF in `test-data/real/` and asserts that text
  is neither lost nor duplicated (character multiset), that every offered picture is
  placed exactly once, and that no box escapes or collapses.
- Stream filters: `cargo test -p pdfextractor-pdf-parser filters::` - includes the
  specification's own LZW example and a reference-encoder round trip.
- DOCX list markers:
  `cargo test -p pdfextractor-docx-parser resolves_list_markers_and_levels_from_numbering`.
  On the real corpus, `calibre_demo.docx` is the case to eyeball: its list items must carry
  `marker` `\u2022` or `1.` with `level` 1.
- Overlay with blocks: `--preview` now draws a `block` layer. Toggle it off in the header to
  see the raw runs underneath; a block whose outline does not wrap its own text is a
  boundary bug, not a rendering bug.
- Judging the title rule: `python3 -c "import json;d=json.load(open('/tmp/a.json'));print([t['text'] for t in d['texts'] if t['label']=='title'])"`
  must print the document's real title once. On `arxiv_attention.pdf` a 20 pt sideways
  arXiv stamp sits above a 17 pt title, so this is the case that proves rotation is
  excluded from both the title slot and the region's size bar.
- Stream lengths: `RUST_LOG=pdfextractor_pdf_parser=debug cargo run --release -p
  pdfextractor-cli -- -i test-data/real/basicapi.pdf -o /tmp/b.json` must log exactly one
  `stream (20, 0): /Length 281 wins over the terminator search, which found 280`, and
  `/tmp/b.json` must contain `page 3 / 3` and `Powered by TCPDF (www.tcpdf.org)`. Without
  the `/Length` pass those three items are missing.
- DOCX level overrides:
  `cargo test -p pdfextractor-docx-parser applies_level_overrides_from_numbering` - one
  document must print `1. 2.` for one list, `A) B)` for a relabelled list, and `5. 6.`
  for a restarted one, all from the same abstract scheme.
- Titles across the corpus: run the extractor over `test-data/real`, then check every
  `texts` entry labelled `title` in the JSON. There must be exactly **8**, each the
  document's real name - no stamp, no printed year, no sideways text. `arxiv_attention`
  and `irs_f1040` are the two cases that make this non-trivial.
- Watch out for a stray `*.json` **inside `test-data/real/`**: the golden suite walks it
  as a document and fails with `unsupported extension: json`. A `--selftest` run no longer
  writes there (it converts into `/tmp/pdfextractor-selftest-<pid>/`), so such a file is
  either from an older build or from a hand-run `pdfextractor-cli` with no `--output` —
  `rm test-data/real/*.json`, and `mathtype_images/` with it, before the golden suite.
- Reproduce the furniture pass: `cargo run -p pdfextractor-cli -- -i
  test-data/real/math_seq2seq.pdf -o /tmp/s.json --ocr never`, then count what was moved
  out of the text: `python3 -c "import json;d=json.load(open('/tmp/s.json'));print(sum(1 for t
  in d['texts'] if t['label'] in ('page_header','page_footer')))"` must print **31** (the
  ICLR running head on all 15 pages and the 15 page numbers, plus page 1's arXiv stamp).
  `basicapi.json` must report 3, `tracemonkey.json` and `dummy.json` 0. The pass over every
  real page, with its invariants, is `cargo test -p pdfextractor-layout --test
  real_corpus_test`; the rules themselves are `cargo test -p pdfextractor-layout --lib` and
  the JSON shape a consumer sees is `cargo test -p pdfextractor-cli --test cli_test
  a_page_number_comes_out_as_the_pages_own_furniture`.
- Reproduce the footnote pass: `cargo run -p pdfextractor-cli -- -i
  test-data/real/arxiv_attention.pdf -o /tmp/a.json --ocr never`, then
  `python3 -c "import json;d=json.load(open('/tmp/a.json'));print(sum(1 for t in d['texts'] if
  t['label']=='footnote'))"` must print **5** (the three symbol notes, `4To illustrate…` and
  `5We used values…`), while `math_seq2seq.pdf` prints **9**, `math_layernorm.pdf` **5**,
  `math_gan.pdf` **4**, and `tracemonkey.pdf`, `math_batchnorm.pdf`, `basicapi.pdf`,
  `dummy.pdf` and `irs_f1040.pdf` **0**. The notes and the rule each stands under are locked by `cargo test -p
  pdfextractor-layout --test real_corpus_test`; the rule itself, and what is deliberately not
  a note, by `cargo test -p pdfextractor-layout --lib`. The measurement against Docling is
  `cargo test -p pdfextractor-cli --test docling_golden_test our_output_agrees -- --nocapture`,
  which prints, per document, `footnotes: N/M kept, K read as text, L not found`.
- Compare mode, both ways: `cargo test -p pdfextractor-ui --test compare_test` runs the
  real CLI over corpus documents and compares with `test-data/golden/docling` (and prints
  every row that is not `same` with `--nocapture`);
  `cargo run -p pdfextractor-ui -- --selftest test-data/real/math-nary.docx
  --selftest-ref test-data/golden/docling` does the same through the window and reports
  the panel's own tallies, which for that document must be `notation=3`.
- Golden tests against Docling, both layers:
  `cargo test -p pdfextractor-cli --test docling_golden_test`. Without `DOCLING_PY` this
  checks every corpus document against the committed `docling-core` schema (fields per
  item class, enum spellings, resolvable references, children agreeing with their
  parents) and compares against the committed Docling conversions. With
  `DOCLING_PY=~/.venvs/pdfextractor-docling/bin/python` it additionally loads every
  document with `docling-core` itself, which is the authority on the format:
  `… --test docling_golden_test docling_core_accepts_our_documents -- --nocapture`
  must print `37/37 document(s) accepted by docling-core`.
- See the comparison rather than a verdict:
  `… --test docling_golden_test our_output_agrees -- --nocapture` prints, per reference,
  `pages ours/theirs, text agreement, formulas ours/theirs`. Every reference is at
  `1.000` except `calibre_demo.docx` at `0.999`, whose remainder is Docling splitting a
  paragraph at every formatting run (`sup` + `erscript`) rather than text we lost.
- Add a reference for a new document:
  `~/.venvs/pdfextractor-docling/bin/python test-data/golden/regenerate.py <name>` — the
  generator, the version caveat and what the comparison deliberately ignores are all in
  `test-data/golden/README.md`.
- Known-good toolchain: rustc/cargo 1.99.0, edition 2021.

## Release engineering (new)

Releases are tag-driven. A `v*` tag opens a draft release, builds the artifacts
on all three platforms, uploads them into the draft, and the last job checks the
tag against the workspace version (and the `VERSION` every CLI archive reports
about itself), writes `SHA256SUMS.txt` and publishes. The same workflow runs by
hand — `workflow_dispatch` builds and packages everything and publishes nothing,
which is how a change to it is tested before a tag depends on it.

What a release carries, per platform: the command line (`tar.gz`/`zip`, with the
Tesseract DLLs beside it on Windows) and the window's installer — `.deb` and
`.AppImage` (Linux), `.msi` (Windows), `.dmg` (macOS). Three things had to be
built for that to be true, all of it new:

- **Icons**: `.ico` (seven sizes) and `.icns` (eight, retina included) grown
  from the existing 128px `icon.png`, which the deb already used.
- **The bundle configuration** (`crates/ui/tauri.conf.json`) is now kept in the
  repository — the `*.json` ignore rule carves out one more exception, as it did
  for the golden references — with `bundle.active` on and the deb declaring
  `libtesseract5`/`liblept5`: the bundler's own dependency list knows WebKitGTK
  and GTK but not the OCR runtime an installed window converts with.
- **The command line rides inside every bundle**, in the resource directory,
  and the window names that copy at startup (`crates/ui/src/main.rs`): an
  installed window converts without a separate install. The resource *globs*
  are passed on the release build's command line rather than committed, because
  tauri-build resolves them at compile time and fails any build where one
  matches nothing — a plain `cargo build`, the compare gate included, must not
  need a release binary to exist. On Windows the vcpkg Tesseract DLLs are
  copied beside the binary before bundling, which is where both the bundler's
  glob and Windows' own loader look for them.

Two things the first run of the workflow taught, both now fixed in it: Actions
artifact storage is a per-account quota this account has run out of, so the
artifacts travel as release assets (draft first, `gh release upload` per
platform, publish last) where release storage is per-release; and the
committed resource globs broke every ordinary build, as above. The Windows
jobs build Tesseract from vcpkg sources (~40 minutes cold, measured) behind a
binary cache, so the first Windows run of a workflow change is the slow one
and later runs are minutes — but only a *green* run saves the cache: a failed
job skips its post step, which is why the fix below paid for the build twice.

The artifacts are unsigned (signing keys are not in this repository), so
Gatekeeper and SmartScreen ask on first open; that is the open line in the
plan now split off from "installers/bundling".

The second hand-run failed both Windows jobs inside `leptonica-sys` —
"package leptonica is not installed for vcpkg triplet
x64-windows-static-md". vcpkg-rs, the crate that finds vcpkg's libraries,
picks that triplet unless told otherwise, while the workflow installs
`tesseract:x64-windows`; the four other jobs were green, which is what a
hand-run is for. Both build steps now export `VCPKGRS_DYNAMIC=1` — the
crate's required opt-in for a dynamic triplet, which also selects
`x64-windows` to match — and the failure, the reasoning and the fix are in
the workflow's own comments.

The same run exposed a gap no platform had yet noticed: nothing shipped
`eng.traineddata`. The deb declared the OCR runtime but no language data,
and Windows and a bare container have none to find — an installed window
could start a conversion and have Tesseract refuse to load. Every package
now carries `tessdata/eng.traineddata` beside the executable (fetched from
tesseract-ocr/tessdata_fast at a pinned commit and checked by sha256 in the
workflow), and `ocr-engine` looks there first: `OcrEngine::new` points
Tesseract at the package's own `tessdata` when the binary has one beside it
and passes `None` when it does not, so an ordinary build behaves exactly as
it did before — the compare gate included. The bundle mapping rides the
same `--config` resources, and the mapping itself is tauri's rule ("if
processing a glob and `current_dest` is not empty we put all globbed paths
under `current_dest` preserving the file name"), read in tauri-utils'
`ResourcePathsIter` rather than guessed, because a file *named* `tessdata`
would have looked installed and never worked. Three tests pin the rule
down: the layout is recognised, a package laid out this way starts OCR, and
an empty datapath does *not* silently read the system copy — without that
third one, the second would pass on any machine that has Tesseract already.
End to end, the release binary run from a shipped layout with
`TESSDATA_PREFIX` poisoned to `/nonexistent` recognised the scanned invoice
in full (`ACME`, `12345`, `Total Due`); with the shipped data removed and
the same poison, Tesseract reported `Error opening data file
/nonexistent/eng.traineddata` and the CLI continued without OCR, which is
its stated policy for an engine that will not start.

A local dry-run of the whole packaging pipeline (the workflow's own steps,
run by hand: archive, deb, AppImage, version gates, `SHA256SUMS.txt`) caught
one thing before a tag could ship it: the `pdfextractor-cli*` resource glob
also matched cargo's `pdfextractor-cli.d` dep-info file, so every bundle
carried a stray text file of this machine's build paths. The bundle config
now names the command line exactly (`pdfextractor-cli`, and
`pdfextractor-cli.exe` on Windows — which also excludes cargo's `.pdb`), and
the rebuilt deb holds only the window binary, its desktop file and icon, and
the command line with its `tessdata`. The deb's declared `Depends` came out
of a real build as `libtesseract5, liblept5, libwebkit2gtk-4.1-0,
libgtk-3-0` — the two we name, merged with the bundler's own defaults.
