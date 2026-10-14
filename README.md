# PDF & Office Document Extractor

A Rust document extractor that converts PDF, DOCX/DOC, RTF, HTML, Markdown,
plain text and CSV/TSV into
[Docling](https://github.com/docling-project/docling)-compatible JSON.

**Approach:** hybrid. Commodity infrastructure is reused (DEFLATE, ZIP, XML, image
codecs, GPU API); the domain logic — PDF object model, content-stream interpretation,
layout analysis, Docling export — is implemented here.

## Status

| Area | State |
|---|---|
| PDF text extraction | ✅ works — Flate/ASCII85/LZW/RunLength/ASCIIHex, object streams, linearized files, font encodings (cp1252, `/Differences`, `/ToUnicode`, Identity-H, **and the encoding an embedded Type 1 program carries itself**, which is where a TeX symbol font keeps `∗`/`†`/`‡`), **real glyph advances from `/Widths`** |
| PDF pictures | ✅ raster images extracted with their true placement box; **vector-only figures** detected for papers that contain no image at all |
| **OCR for scanned pages** | ✅ works (Tesseract, wired into the pipeline) |
| **DOCX** | ✅ works — paragraphs, headings, lists with their real bullet/number and level (`numbering.xml`, incl. `w:lvlOverride` restarts and relabels), table grids, embedded images (incl. headers/footers), **Office Math equations** (one item per equation), **chart titles as captions**, footnotes and endnotes, tabs, **vector EMF/WMF drawings played into PNGs** (Word's own metafiles and the previews of embedded objects) |
| **Layout analysis** | ✅ works — rows and runs, column gutters, reading order, paragraph merging, `title`/`section_header`/`list_item`/`caption` labels, rotated margin stamps kept in their own box, **PDF table detection**, pictures placed in reading order |
| **Page furniture and footnotes** | ✅ works — the running heads, footers and page numbers a document repeats across pages are labelled `page_header`/`page_footer` and moved to the furniture layer, and a **note at the foot of a page** is labelled `footnote` by the two marks that make one: the **rule** the page drew above it and the **marker** it opens with, which the body of that page also uses. The page's own rules are given to layout analysis as well, which is what keeps a note block out of the table list: a marker-and-text block at the foot *under a rule* is the page's notes, not a two-column grid. The encoding a TeX symbol font carries in its own program is read too, so a marker is the character the page drew rather than the Latin-1 byte its code lands on. Measured against Docling: of the 25 notes the corpus's five PDFs carry, **23 come out labelled** (all five of `arxiv_attention`'s, all five of `math_layernorm`'s, all nine of `math_seq2seq`'s, all four of `math_gan`'s); the other two are read as a paragraph because their pages draw no rule at all, and **not one is missing**. See `progress.md` |
| Docling JSON export | ✅ pages, text items, tables, pictures, provenance bboxes, furniture, body tree |
| **Docling compatibility (golden tests)** | ✅ **verified against real Docling**: every corpus document passes `docling-core`'s own model validation and tree check, and agrees with real Docling conversions of eleven of them |
| **Box overlay preview** (`--preview`) | ✅ works — HTML page render with every text/OCR/figure box and every **layout block** drawn on it |
| CLI (file/dir, parallel, output rules, `--ocr`, `--preview`) | ✅ works |
| **Legacy `.doc`** (OLE compound file) | ✅ works — container, Fib and piece table read directly; paragraphs, page breaks, pictures as placeholders; field instructions dropped |
| **RTF / HTML / Markdown / text / CSV** | ✅ works — RTF control groups, HTML block structure, Markdown headings/lists/code/tables, RFC 4180 CSV, all decoded to blocks |
| DOCX equations as LaTeX | ⚠️ linearised to readable text (`x^2`, `a/b`, `√[3](x+1)`), not TeX |
| Legacy `.doc` styles, tables, raster images | ⚠️ needs character formatting (`PAPX`/`CHPX`), which the parser does not read: no headings, tables flatten to tabs, images are placeholders |
| Flow formats (DOCX/RTF/HTML/`.doc`) page geometry | ⚠️ none — no page is reported at all (Word repaginates; Docling publishes none either); `--preview` is not produced |
| **Compare mode** (UI) | ✅ works — a reference (a file, or a folder of them — `test-data/golden/docling/`) puts every converted document beside Docling's reading of the same one: what agrees, what differs by label or wording, what only one side has, equations marked as notation, **what the reference cut finer than we did** (a reference item wholly inside one of our blocks — by its words, or, for an item with no letters in it at all, by lying among the words of a line we hold — drawn as the run it is, with the items a broken run leaves behind taken onto the row of the block that holds them, leaders and all), and the share of the reference's characters we also hold. An item with no letters in it that nothing of ours answers is **placed** — named, one line of a form per row, and **drawn page by page**: each page that carries places is drawn once above the rows with all of its items at once, so a form's leader pattern is one picture rather than one thumbnail per row. Rows content cannot pair are joined by **page and box**, so a drawing the two readings ordered differently is one row rather than one a side — where the producers publish boxes (PDFs), with the overlap that joined the row on the row itself |
| **UI** (`crates/ui`) | ✅ works — Tauri window with a file/folder picker and a settings panel. A conversion runs in the background and is **streamed**: the window lists every document as it finishes, shows a progress bar and the CLI's own log, and can **stop** the run at any time. Cross-platform, no JavaScript toolchain needed. |

See [progress.md](progress.md) for the verified check results and known limitations,
and [plan.md](plan.md) for the full roadmap.

## Requirements

- Rust 1.75+ (verified on 1.99.0)
- **OCR (required by default):** Tesseract + Leptonica development libraries, plus
  `libclang` because the bindings use `bindgen`:

  ```bash
  apt-get install -y libtesseract-dev libleptonica-dev tesseract-ocr tesseract-ocr-eng libclang-dev
  ```

  Without these, build with `--no-default-features`; OCR calls then return a clear error.
- **UI (optional):** Tauri uses the system webview. On Linux that is
  `webkit2gtk-4.1` + `libsoup-3.0` + `gtk3` (plus `libclang`, already needed for
  OCR); Windows uses WebView2 and macOS uses WKWebView. Nothing else is required
  — the frontend is plain HTML/CSS/JS, so no Node or bundler is involved.

## Build and test

```bash
cargo build --workspace      # 0 warnings
cargo test  --workspace      # 344 tests
cargo clippy --workspace --all-targets

# The compatibility suite, held to real Docling. The schema layer always runs;
# pointing DOCLING_PY at a Python with docling-core installed adds the layer that
# loads every document with Docling's own model.
DOCLING_PY=~/.venvs/pdfextractor-docling/bin/python \
  cargo test -p pdfextractor-cli --test docling_golden_test
```

### Compatibility with Docling

The output is checked two ways (`test-data/golden/README.md`):

* **The schema** `docling-core` generates from its own model, committed and checked in
the Rust suite: required fields, no field an item class does not allow, legal enum
spellings, resolvable references, and a hierarchy whose children agree with their
parents. Every document the CLI writes for the whole corpus has to pass.
* **Real Docling conversions** of eleven corpus documents, compared by content — page
count and page size, text as a character multiset, equation counts, and no block
appearing twice. Text agreement is 0.999–1.000 across all of them. Formula *notation* is
deliberately not compared: Docling writes LaTeX, we linearise Office Math to Unicode.

## Usage

```bash
# Single file; output defaults to <input>.json next to the input
cargo run -p pdfextractor-cli -- -i document.pdf

# Explicit output, 4 parallel conversions
cargo run -p pdfextractor-cli -- -i document.pdf -o out.json -p 4

# Whole directory (recursive by default); output tree mirrors the input
cargo run -p pdfextractor-cli -- -i ./docs -o ./out -p 8

# OCR a scanned PDF (default: only pages with no text layer)
cargo run -p pdfextractor-cli -- -i scan.pdf --ocr auto

# Force OCR even when a text layer exists
cargo run -p pdfextractor-cli -- -i mixed.pdf --ocr always

# Skip OCR entirely
cargo run -p pdfextractor-cli -- -i doc.pdf --ocr never

# Write an HTML overlay of every extracted box (see "Box overlay" below)
cargo run -p pdfextractor-cli -- -i scan.pdf -o scan.json --preview

# Top level only
cargo run -p pdfextractor-cli -- -i ./docs --recursive false
```

| Flag | Meaning |
|---|---|
| `-i, --input <PATH>` | file or directory (required) |
| `-o, --output <PATH>` | output file (single input) or directory (directory input); defaults to beside the input |
| `-p, --parallel <N>` | concurrent conversions, 1–1000 (clamped to 256) |
| `-r, --recursive <BOOL>` | recurse into subdirectories (default `true`) |
| `--ocr <MODE>` | `auto` (default, scans only), `always`, or `never` |
| `--preview` | also write `<output>.preview.html`, a page overlay of every extracted box |
| `--preview-dpi <N>` | resolution of the preview's page backdrop (default 110) |
| `--progress` | stream one JSON record per finished document to stdout (see below) |
| `--compare <PATH>` | compare every finished document with a Docling reference (a file, or a folder of them) and exit non-zero if any falls below the floor its memory remembers (see "Compare mode") |

Log records go to **stderr**; stdout is left for the output of the run itself, so
`--progress` records can be read a line at a time with nothing to filter out. Set
`RUST_LOG=info` for the log.

### Streaming progress

`--progress` turns a run into a stream a parent process can follow, which is what the
desktop window renders. A plan line lists the whole queue before any work starts, then
each document is reported **the moment it finishes** rather than being collected and
printed at the end:

```jsonc
{"kind":"plan","total":2,"inputs":["/docs/a.pdf","/docs/b.docx"]}
{"kind":"progress","done":1,"total":2,"input":"/docs/a.pdf","ok":true,"output":"/out/a.json"}
{"kind":"progress","done":2,"total":2,"input":"/docs/b.docx","ok":false,"error":"not a DOCX (ZIP) package: ..."}
```

Every line is a record, `done` counts up in the order documents finish, and a failure is
reported as a record rather than as silence. Without the flag the same news goes to the
log (`tracing` for successes, stderr for failures) and stdout stays empty.

Output files are written through a temporary sibling and renamed into place, so
stopping a run — or the machine losing power during one — cannot leave a half-written
`*.json` that still looks like a document.

## UI

A small Tauri window wraps the same pipeline: pick one file or one folder, set
the options, press **Convert**. It does not reimplement anything — it builds the
argument vector, runs the `pdfextractor-cli` binary with `--progress`, and draws
the stream it gets back.

```bash
cargo run -p pdfextractor-ui
```

A conversion is **never waited on**, so the window stays live throughout:

- the queue appears as soon as the run starts, one row per document;
- each row turns *converted* or *failed* as its document finishes, with the
  failure's reason attached;
- a progress bar, a document count, and the CLI's log (colour escapes removed)
  sit under it, alongside the exact command line being run;
- **Stop** kills the process at any point. Documents already written are kept —
  each one is written through a temporary file and renamed, so a stopped run
  leaves whole files or none. Closing the window stops the run too.

The frontend is plain HTML/CSS/JS in `crates/ui/dist`, loaded by the system
webview, so there is no Node build step. `dist/index.html?mock=1` loads a small
stand-in for the Tauri API (`dist/mock.js`, shipped but never referenced by the
app) that plays a scripted run in an ordinary browser, which is how the page can
be worked on without rebuilding the app. The window's permissions live in
`crates/ui/capabilities/default.json`; `core:event:allow-listen` there is what
lets the page subscribe to the stream at all.

The CLI is looked for beside the UI binary; set `PDFEXTRACTOR_CLI=/path/to/pdfextractor-cli`
to point somewhere else.

### Compare mode

A second use of the same window: turn on **Compare** and give it a Docling reference.
Every document that finishes is then drawn beside the reference for it — either the
reference file itself, or, when the reference is a folder, the file named after the
document (`test-data/golden/docling/` is laid out that way). The reference is never
handed to the extractor: it is what the two readings are compared *with*, so the CLI
is never told about it.

The document's own body tree lays both sides out — `body.children`, and groups
(`#/groups/N`, which is how Docling encloses a picture area, a list or a run of
formulas) walked where they stand — and the two lists are lined up the way a diff
lines up two files: a block is paired with at most one on the other side, pairs never
cross, and a block with no counterpart stands alone on its side.

Where both producers published a box for a block — Docling's PDF backend and ours do,
the flow formats do not — that box takes part in the matching itself, not only in the
joining afterwards. The words still decide whether two blocks are a link at all, and
the boxes decide which link is taken where the words offer more than one: a pair both
readings put in the same place counts for a tenth of a link more, which is what tells
apart two pairs the words cannot (a drawing a document lists twice, in one order on
each side — only the box says which copy is which), and a pair they put in different
places counts for half a perfect match, which is what makes two of those lose to the
one pair the boxes agree about. Nothing is hidden by any of it: a pair the boxes
contradict is still one row, whose note says where each reading put the block
(`not in the same place: page 1, 8% of the smaller box shared`).

Two readings do not always agree about the *order* of the same things, either — a
drawing Docling lists after the caption we list before it — and an alignment that
respects order cannot pair both halves of that at once. The boxes decide which of
those pairs the alignment takes; what is left over is joined afterwards by **page and
box**: two blocks that stand alone, are the same kind of thing, and occupy the same
place on the same page are one thing read twice. Only leftovers are joined this way,
and each joined row says so on itself (`paired by position: page 2, 88% of the smaller
box shared`), with the total in the panel's header, because that step is a heuristic
rather than a comparison. It also needs both producers to publish boxes at all — so
two readings of a DOCX that disagree about order still show one row a side.

Docling's side of a PDF is *finer* than ours — its backend writes an item per line,
where we keep a paragraph whole — so a comparison that paired only by content would
report every one of those per-line items as text the reference has and we do not. A
reference item whose letters run from one of our word boundaries to another is
therefore recognised as **contained** rather than shown as "only the reference", and
consecutive items one of our blocks holds are drawn as the run they are: our side of
the panel stays one block per row. A piece of a word is not inside ours — the
reference's `3` is not a piece of our `1937` — so a shared digit or a shared ending is
not mistaken for one reading cut up, and text our own reading spaced out letter by
letter (a form's field labels) still matches, because each letter is a word of ours.
An item the alignment could not reach that way — the reference put something we do not
hold between it and the run, so the run broke — is moved onto the row of the block whose
words hold it, which says how many items it took; the row keeps whatever it already
disagreed about, so a wording difference is never hidden behind a containment.

That last step is also where an item with **no letters in it at all** is answered for,
which is what a filled-in form is made of: Docling writes each dotted leader and each rule
as an item of its own, so a line of `irs_f1040` whose words we hold in full ("37 38
Estimated tax penalty (see instructions) …38") is read by the reference as word fragments
with leader items between them, and every leader would otherwise stand as a line saying
the reference holds text we do not. Such an item is taken onto the row of a block that
already holds *words* of the reference — a leader standing among the fragments of a line
we read is part of that line — and the row's note says so apart from the items taken by
their words (`3 of the reference's items are wholly inside ours; 16 items with no letters
in them lie in the same place`), with the row showing the reference's items in the order
the reference wrote them. It is the one place where *where* decides, so it asks three
things: a box from both producers, a block that already holds words there, and no letters
in the item itself — an item with letters in it that are not ours is text we did not read,
and a letterless item standing alone in our block's area is the reference's own reading of
that area rather than a piece of anything of ours. Both of those are left where they were:
one-sided, unless the joining by position finds them the same box on the same page and
nothing else, in which case the row says that is what paired them.

What is left of a form after all that is the rest of its dotted leaders: items with no
letters in them at all that no block of ours holds and that nothing of ours lies along —
lines our reading produced no block for at all. Nothing can be compared with an item like
that: it has no letters to match. So it is **placed** rather than listed — a row that says
what Docling wrote and where it lies (`a dotted leader or a rule at page 5, box
50,560–300,580: an item with no letters in it at all, so there are no words to compare`),
counted in the header as *placed* instead of among the rows only the reference has,
because a panel that called punctuation its words cannot compare "text we do not have"
would be claiming to know something it does not. The items of **one line** of a form are
placed together, though, because Docling writes each run of dots of a leader as an item of
its own: one row that names how many items it stands for and the extent they cover (the
runs of dots and the rule under them of one line are one row, not three), so `irs_f1040`'s
**327** items are **25** rows — one per line of the form — rather than 327 rows that used
to read as punctuation we had lost.

A placement is *drawn* as well as named: beside the row's note stands the page the item
lies on, drawn to its own shape as the reference stated it, with the item's box outlined
on it — so which line of a form a leader stands on is seen rather than worked out from the
coordinates (which stay in the note, where a row can still be read and copied). A page
whose size the reference never states has nothing to draw the box *on*: those rows are
named like the rest and drawn nowhere.

A row's own box answers *which line*, though, not *what the form looks like*. So the
placed items are gathered the other way round as well, **page by page**: above the rows
the panel draws each page that carries a place once, with every placed item of it at once,
so a form's leader pattern is one picture rather than one thumbnail per row. That picture
is built from the items *before* the rows are cut to what the panel lists, so a long
form's page is whole even where its rows are not — and it holds items, not rows, so it is
a drawing of what the reference wrote, dot run by dot run, rather than of the lines the
rows were grouped into.

Each row carries one of seven verdicts:

| Verdict | Means |
|---|---|
| **same** | Both sides read the block the same way — same kind, same label, same words, and, where both published a box, the same place on the page |
| **notation** | Both sides found it and only its *notation* differs: an equation Docling writes as LaTeX (`\sum_{}^{2}x`) and we linearise to Unicode (`∑² x`) |
| **differs** | Both found it and disagree about something a reader would notice — the label (`section_header` against `paragraph`), the words, or where the block sits: two readings that found the same words in different parts of the page are a difference, and the row names both places. The row says which |
| **contained** | Both found the words and disagree only about where an item *ends*: Docling writes an item per line, we keep the paragraph whole, so the reference's item (or the run of them) lies inside one block of ours. Our side of the row is that one block and the reference's side is the run, so a paragraph the reference read as a dozen lines is one line of the panel — and a form line's dotted leaders and rules are on that row too, taken by where they lie rather than by words they do not have |
| **placed** | The reference wrote an item with no letters in it at all — a dotted leader, a rule — where our reading produced no block for the line it lies along. There is nothing to compare and nothing to miss, so the row stands for *where* the item is — its box drawn on an outline of the page it lies on, beside the row, with the items of one line standing as one row that names how many they are and the extent they cover — and the header counts it apart from the rows only the reference has. Every placed item is also drawn on its page in the per-page view above the rows, so a form's whole leader pattern is one picture |
| **only ours** / **only the reference** | One side read something there and the other did not: a table read as prose, a drawing that came out as a box with no image. Two lonely rows that share a place on a page are joined into one row instead |

The panel's header shows the same numbers the golden suite quotes — the share of the
reference's alphanumeric characters that ours also holds, counted as a multiset and
with equations excluded, because an equation's *spelling* is not evidence about
anything. Two differences of scale are shown rather than hidden: the two tools publish
their own dpi on a picture, and a drawing rasterised from its declared size lands a
point or two from Docling's render of it.

This is not the golden suite: that one decides **pass or fail** against budgets, this
one **shows where the two readings differ** so a person can look. Where they overlap
they use the same definitions (`crates/ui/src/diff.rs`). The work is bounded, so a
pathological document cannot hang the window: at most 2,000 blocks a side are aligned,
at most 2,000 rows are drawn, and each text is clipped visibly at 300 characters — with
what was left out reported in the panel.

The whole corpus can be run through the mode in one pass, worst first — which is what to
reach for after a change, to see which documents it helped and which it did not:

```bash
cargo test -p pdfextractor-ui --test corpus_report_test -- --nocapture
```

That converts all 37 corpus documents in a single CLI run and compares each with its
reference, then prints the verdict mix per document, ordered by the share of rows the
reference does not simply agree with (rows that differ + rows only we have), beside the
character-agreement figure, the rows paired by position and the rows **placed**. The
`cont` column is what the reference cut finer inside our blocks, and what is left in
`refs` after it is text we did not read — the dotted leaders of a form line *we* read go
onto that line's row, and a leader whose line our reading produced no block for is in
`plcd` rather than `refs` — one row per line of the form — because there is nothing in it
to have missed. Those rows are
listed separately rather than ranked.
[progress.md](progress.md) has the report as it stands and what it found.

The report is also a **ratchet**: the numbers it measured last time live in
`test-data/golden/compare-baseline.json`, and a document whose panel now says more
`differs`, more rows only ours, more rows only the reference, or holds fewer of the
reference's characters than that file remembers fails the test — naming itself and the
count — so a change that helps one document and quietly harms another cannot pass. The
rule and the gate that applies it are the library's (`crates/ui/src/diff.rs` and
`gate.rs`): the panel, this report and the command line's `--compare` all hold a
document through the same Session, so there is one gate rather than three that can
drift. The
columns that are not ratcheted (`same`, `contained`, `notation`, `joined`) may move
either way, and a `differs` row cannot leave its column without landing in one of the
one-sided ones. A form's **placed rows** are ratcheted as a rate rather than a count: the
items those rows stand for and the rows they stand in are compared by cross-multiplication,
so the same 327 items coming back as 327 rows instead of 25 is refused, while fewer rows for
the same items is the improvement it is. The per-page drawing is the same rule one level up —
more pictures for the same boxes is the leader pattern answered row by row again. A rate
alone answers nothing at the limit, though, and it keeps any *proportional* fall: half the
panel in half the rows is the same ratio, and a document whose form is answered no more at
all is 0 rows for 0 items. So the **items themselves** are a floor of their own — the placed
item count may only fall on a document that placed nothing to begin with, and the failure
names it (`form item(s) placed: 327 → 160 item(s) in 12 row(s)`). Nothing else in the file can
tell "answered as text instead" from "lost", so that reading change is argued for the way
every deliberate trade is, by regenerating the baseline and reading its diff. Accept a trade on purpose with
`CORPUS_BASELINE=update cargo test -p pdfextractor-ui --test corpus_report_test`, and let
the diff of that file be the argument for it.

The window holds that **same floor itself**, by the same code: `compare-baseline.json`
lives beside the references it is about, so the Compare panel finds it without being told
where it is (`test-data/golden/docling/` → `test-data/golden/compare-baseline.json`) and
measures each finished document against the numbers it remembers. A document whose items
have left the panel is **refused** rather than drawn: the panel names what fell
(`below the floor · form item(s) placed: 654 → 327 item(s) in 25 row(s)`), withholds the
rows, the page pictures and the verdicts, and says how to fix the reading or accept the
trade. How much of the ratchet a run is held to follows from the settings it was made with:
the baseline records them (`measured`: the OCR mode, and anything else worth recording),
and when the run's own settings match the remembered ones — the corpus run itself, or the
window set to `--ocr never` — **every** rule applies, `differs` rising and agreement
falling included. When they do not — the window's default `--ocr auto` against a
baseline measured with OCR off — only the floor applies, because OCR is the setting that
moves the counts measuring how the two readings *agree*, while whether a form's items
are still in the panel belongs to the reading. None of this happens without a memory
beside the reference, which is every comparison against a document of your own, and a
memory that is there but unreadable is said on the panel rather than passed over in
silence.

The panel says which of the two it was, on every document the memory has: a quiet line
under the panel's header —
`Held to the memory: every rule of the ratchet applies: this run was measured the way the
memory records (--ocr never)`, or
`Held to the memory: only the floor under a form's placed items applies: this run was
measured with --ocr auto and the memory with --ocr never, and OCR moves the counts that
measure how the two readings agree`. A refused panel carries the line too, so whether the
whole ratchet refused the document or the floor alone is read rather than guessed; a
document the memory does not have carries no line, because there was nothing to hold it
to.

A panel nobody is watching does not fail a build, so the same hold is on the command
line: `--compare` holds every finished document to the memory beside its reference,
exactly as the panel does, and **exits non-zero** when one is refused — naming the
document and what fell, so a fallen panel fails a build and not only a window:

```bash
cargo run -p pdfextractor-cli -- -i test-data/real -o /tmp/out --ocr never \
  --compare test-data/golden/docling
#   compare: test-data/golden/docling · memory: 37 document(s), measured with --ocr never
#   compare: test-data/real/EquationAsScientificNumbering.docx: 2 row(s), agreement 100.0% · at or above the whole ratchet
#   … one line per document …
#   compare: test-data/real/tracemonkey.pdf: 439 row(s), agreement 99.1% · at or above the whole ratchet
#   (exit 0 — nothing fell; a run that does fall ends)
#   error: the compare mode refused 1 document(s): …/dummy.pdf: below the floor · form item(s) placed: 5 → 0 item(s) in 0 row(s)
```

With no memory beside the reference the comparisons still print and no floor can fire;
what the mode refuses instead is a run it could not hold at all — a reference path that
is not there, a memory that is there and cannot be read, or a document with no reference
beside it — because a gate that held nothing while claiming to hold everything is worse
than no gate.

Every push runs that gate over the corpus in CI
(`.github/workflows/compare-gate.yml`): the same command over `test-data/real`,
measured `--ocr never` so every rule of the ratchet applies — a document that falls
below its floor fails the build with its name and its count in the job's log.

### Smoke-testing the whole stack without a human

`--selftest [INPUT]` opens the window, proves the webview and IPC round-trip,
converts the given path through the CLI, and prints what the page actually drew;
`--selftest-ref PATH` turns Compare on for that run, so the panel is exercised too.

```bash
# Smoke-test the whole stack without a human: the window opens, the webview
# loads the frontend, the IPC round-trips, the given path is converted through
# the CLI, and the page reports back what it actually rendered.
cargo run -p pdfextractor-ui -- --selftest path/to/document.doc

# The same, stopped 2.5 seconds in, to check the stop path end to end.
cargo run -p pdfextractor-ui -- --selftest ./docs --selftest-stop 2500

# The whole stack with Compare on: one corpus document against the committed
# Docling reference for it, with the panel's own tallies reported back.
cargo run -p pdfextractor-ui -- --selftest test-data/real/math-nary.docx \
  --selftest-ref test-data/golden/docling
#   … diffs=1 diff-agreement="100.0%" diff-verdict="same=0 notation=3 differs=0 contained=0 ours-only=0 theirs-only=0 placed=0 paired-by-position=0 truncated=0" placed-drawn=0 placed-pages=0 diff-rows=3 floors=0 held=floor

# A PDF reference carries the boxes the two readings found each block at, so those
# boxes take part in the matching, the rows they leave one-sided are joined by page
# and box, and the placed leaders of a form line — one row per line, naming how many
# items it stands for and the extent they cover — are **drawn** on the page they lie on,
# reported back as the panel's own `paired-by-position` count, the `placed-drawn` boxes it
# really put on a page rather than was told about, and the `placed-pages` pictures it drew
# with every placed item of a page at once.
cargo run -p pdfextractor-ui -- --selftest test-data/real/tracemonkey.pdf \
  --selftest-ref test-data/golden/docling
#   … diff-agreement="99.1%" diff-verdict="same=25 notation=0 differs=138 contained=56 ours-only=89 theirs-only=97 placed=34 paired-by-position=60 truncated=0" placed-drawn=1 placed-pages=3 diff-rows=401 floors=0 held=floor
#   (a panel draws at most 400 rows and this document has 439, so 33 of its 34 placed
#   rows are past the end of what was drawn — the 401st row drawn is the "and N more"
#   line — and its 34 items fall on 3 pages; `test-data/real/irs_f1040.pdf`, whose 198
#   rows carry the form's 25 lines as 25 placements on **2** pages, comes back with
#   placed-drawn=25 placed-pages=2 diff-rows=198 floors=0 held=floor, and `floors` counts the
#   documents this run refused: a panel whose placed items have fallen below the count
#   the memory beside the reference remembers draws no comparison at all, and says so.
#   `held` is which rules held the panel to its memory — `floor` here, because the
#   window measures with `--ocr auto` and the memory records `--ocr never`; it reads
#   `ratchet` when the two measurements match and every rule applies)
```

The self-test converts into a directory of its own under the system temporary
directory, never beside the document it read, so pointing one at the corpus leaves the
corpus alone: a stray `.json` among the documents is a document with an unsupported
extension to the golden suite, and a `<stem>_images/` directory is litter of the same
kind. The run says where it wrote — the command line it prints carries the `--output` —
so what it converted is still there to look at (and left there, since that is what a
temporary directory is for):

```
selftest-started run=1 command=…/pdfextractor-cli --input …/tracemonkey.pdf --output /tmp/pdfextractor-selftest-4242/tracemonkey.json --parallel 4 --recursive true --ocr auto --progress
```

The first prints `webview-ready`, `selftest-started run=...`, then a
`selftest-rendered ...` line carrying the state **as drawn** — title, verdict,
progress bar width, one status per row, log line count — so a mistake in the page
is not mistaken for a working window. The second adds `selftest-stop ... stopped=Some(1)`
and reports a cancelled run. The last two add `diffs=...`, `diff-agreement=...`,
`diff-verdict=...`, `placed-drawn=...`, `placed-pages=...`, `diff-rows=...`, `floors=...`
and, where there is one, `floor="..."`: the comparison panels, their tallies, how many
placed boxes were actually drawn, how many per-page pictures the panel drew with all of a
page's placed items on them, how many rows it drew (0 on a document it refused), how many
documents fell through the floor their memory remembers, and what that floor said — all as
the page drew them. All of them exit 0 if the stack is healthy.

## Test documents

The extractor is regression-tested against real documents produced by other tools
(`test-data/real/`, see its [README](test-data/real/README.md) for provenance):

| Document | Producer | Exercises |
|---|---|---|
| arXiv paper | pdfTeX | object streams, ligature encodings, kerning word gaps, 500 dpi figures |
| pdf.js `tracemonkey` | Quartz | multi-column paper, `Td` word gaps, a vector-only figure |
| pdf.js `basicapi` | TCPDF | Identity-H composite fonts, no images at all |
| IRS Form 1040 | PDF 1.7 | linearized file, cp1252 punctuation, forms, page-covering background |
| 4 arXiv papers | pdfTeX | math-heavy text, vector diagrams, raster + vector figures |
| 16 OMML `math*.docx` | LibreOffice/Word | real Office Math: fractions, radicals, Σ/∏/∮, matrices, accents, limits |
| `mathtype.docx` | Word + MathType | an OLE equation object (binary, not OMML) |
| `VariousPictures.docx` | Word | PNG, JPEG, EMF and WMF drawings — the three metafiles are played into PNGs |
| `drawing.docx` | Word | 105 tables mixed with prose |
| `headerPic.docx` | Word | a picture that lives only in the header part |
| `calibre_demo.docx` | Word | headings, lists, 38 tables, footnotes, a table of contents with tabs |
| `chartex.docx` | Word | six charts whose titles exist only in the chart parts |
| `shapes-with-text.docx` | Word | shapes, not images — each written twice by Word |
| 4 legacy `.doc` | Word 97 | OLE container, piece table, field instructions, inline pictures |

Run the whole corpus at once:

```bash
cargo run -p pdfextractor-cli -- -i test-data/real -o /tmp/real -p 4
```

Eleven of them also have a real Docling conversion committed beside them, which the
golden suite compares against; see [test-data/golden/README.md](test-data/golden/README.md).

## Layout

```
crates/
├── core/          # Docling types + document builder
├── pdf-parser/    # lexer, object model, structural parser, content interpreter,
│                  # image decoding, font encodings, stream filters, pictures/figures
├── docx-parser/   # OPC package, WordprocessingML, images, OMML equations
├── doc-parser/    # legacy OLE compound files, Word Fib + piece table
├── text-parser/   # RTF, HTML, Markdown/text, CSV/TSV
├── layout/        # rows/runs, XY-cut columns, reading order, labels, tables
├── ocr-engine/    # Tesseract OCR (required, default-on); TSV word/line boxes
├── json-exporter/ # Docling JSON serialization
├── cli/           # command line interface
└── ui/            # Tauri window (static frontend in ui/dist) over the CLI
test-data/
├── hello.pdf, compressed.pdf, scanned.pdf   # hand-made fixtures
├── real/          # corpus from other producers (see its README)
└── golden/        # the docling-core schema, real Docling conversions, docling-core check
docs/              # schema and format notes
```

## Box overlay

`--preview` writes a self-contained HTML file next to the JSON showing every box
that was extracted, drawn on the page it came from — so extraction quality is
visible rather than inferred from numbers. It is the quickest way to spot a
mis-scaled OCR box, a figure that was missed, or a text run whose box is in the
wrong place.

```bash
cargo run -p pdfextractor-cli -- -i test-data/scanned.pdf -o scan.json --preview
# then open scan.preview.html
```

Boxes are colour-coded by where they came from (text layer, OCR, vector figure,
image) and a second layer draws the **layout blocks** — one dotted box per element
the analysis produced, labelled `title`, `section_header`, `list_item`, `caption`
or `table`. Checkboxes isolate one kind, so you can compare what the page says with
what the block structure claims. The recognised text is drawn inside each box at the
size the box implies, so a box that is too wide or too narrow is obvious; toggling
**show text** off fades the page render back in for a clean side-by-side. A linked
sidebar lets you hover a row to highlight its box and click to scroll to it, so a
long page is navigable rather than a wall of boxes.

A page backdrop is used when one exists. Scanned pages already have their image
extracted, and for text pages `pdftoppm` is used **when it happens to be
installed** — rendering is best-effort and preview-only, so the extractor itself
never depends on an external renderer. Without one the HTML still works: it
reconstructs the page from the extracted text at its true coordinates and says so
in the header.

Flow-based formats get no overlay: DOCX, legacy `.doc`, RTF, HTML, Markdown and
text carry no page geometry, so blocks have no rectangle to draw on. The CLI says
so and skips the preview for them instead of writing an empty one.

## Reading order and structure

Text runs are grouped into the document a reader perceives: rows are cut into runs
at column gutters, whitespace bands split the page recursively (columns first),
runs merge into paragraphs, and each block is labelled from its size, position and
literal prefix. A run of rows with aligned short cells becomes a table. Pictures are
inserted where they appear rather than appended, and list markers and heading numbers
are kept in `marker`/`level` fields instead of the text.

Each item class carries only the structure it has, because Docling rejects a field an
item does not allow: a heading keeps its numbering depth in `level` and its number folded
back into the text (which is where Docling keeps it), a list item keeps the bullet or
number the page printed in `marker` together with `enumerated`, and a title has neither.

```jsonc
{ "label": "title",          "text": "Attention Is All You Need" }
{ "label": "section_header", "text": "3.1 Encoder and Decoder Stacks", "level": 2 }
{ "label": "list_item",      "text": "first thing", "marker": "\u2022", "enumerated": false }
```

Blocks are ordered top-to-bottom and, on a multi-column page, column by column. On the
real corpus this yields 2,180 text items and 66 tables, among them 8 titles, 183 section
headers, 84 list items, 40 equations and 26 footnotes - where the parser alone produced one
item per text run, in drawing order.

## Pictures and equations

Extracted raster images are written to a `<output-stem>_images/` directory next to the
JSON and referenced from each picture item, with the MIME type, pixel size, and the
resolution implied by the box the document drew them into. Figures drawn as pure vector
paths — common in scientific papers — have no pixels to extract, so they are reported as
picture items with a placement box and no `image`. OCR boxes for a figure are mapped
through that same placement box.

A DOCX drawing is the exception that is no longer an exception: Word stores a vector
drawing — and the preview of an embedded object such as a MathType equation — as an
Enhanced Metafile (`image/emf`) or a Windows Metafile (`image/wmf`), neither of which can
be named in an `image` reference. `crates/metafile` plays their GDI records into a raster
and the drawing is carried as a PNG at the size the document asks for, so it is a picture
with pixels rather than a box with no image. Text inside such a drawing is drawn in a
built-in 5×7 font, because a metafile names a font and fetches its outlines from the
system; the glyph positions, sizes and colours are the file's.

DOCX equations (Office Math) are linearised to readable text rather than converted to
LaTeX: `(x+a)^n=∑_(k=0)^n (n/k)x^ka^(n-k)`, `√[3](x+1)`, `lim_(x→1) x`. The structure is
preserved and the content is checkable; it is not guaranteed to round-trip.

## OCR

OCR is a required capability and is enabled by default. For scanned pages it extracts the
embedded image XObject, runs Tesseract over it, and emits line-level text items whose
bounding boxes are scaled into PDF points.

The backend is Tesseract via `leptess`. Word geometry comes from Tesseract's TSV output and
is grouped into lines. To build without Tesseract (the workspace then compiles, but OCR
returns an error):

```bash
cargo build -p pdfextractor-ocr-engine --no-default-features
```
