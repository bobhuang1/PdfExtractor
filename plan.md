# Project Plan: PDF & Office Document Extractor (Docling-compatible)

## Overview
Build a PDF and DOCX/DOC extractor similar to Docling, outputting documents in Docling-compatible JSON format. The key constraint is implementing everything from the ground up (including OCR), learning from Tesseract's approach rather than using existing OCR/PDF/DOCX libraries as black-box dependencies.

**Language**: Rust  
**Platforms**: Windows, Linux, macOS (single codebase)  
**Output**: Docling-compatible JSON  
**Architecture**: CLI-first with simple UI wrapper, parallel processing (1-1000 concurrent conversions)

> **Status:** this file is the forward-looking plan. For what is actually built, tested and
> verified (with commands and results), see [progress.md](progress.md). Current state:
> the workspace builds with zero warnings, 403 tests pass, and PDF, DOCX, legacy `.doc`,
> RTF, HTML, Markdown, plain text and CSV all convert to Docling JSON end to end.
> **A Tauri UI wraps the CLI**: one file/folder picker and one settings panel, verified by
> a `--selftest` that drives window, webview, IPC and a real conversion.
> **OCR is implemented and required**: scanned pages are
> recognised with Tesseract via `leptess` and emitted as line-level text items with
> bounding boxes. **Pictures work for both formats** — raster images come out with their
> true placement box, and vector-only figures are detected even when the document contains
> no image at all. **DOCX is implemented**: paragraphs, headings, lists, real table grids,
> embedded images (including headers/footers as furniture) and Office Math equations
> linearised to readable text. **`--preview` writes an HTML overlay** with every extracted
> box drawn on the page it came from, so extraction quality is visible at a glance. Layout
> analysis, numbering overrides and rotated-text placement are all in place, and a page's
> own **furniture — running heads, footers and page numbers — is labelled**
> (`page_header`/`page_footer`, on the furniture layer but in the body order, which is how
> Docling writes them). **Legacy
> `.doc`** (OLE compound file: container, Fib, piece table) and a **flow-format parser**
> (RTF, HTML, Markdown, plain text, CSV/TSV) are implemented. **v0.1.0 is released**: a
> `v*` tag builds every installer in CI and publishes the GitHub release. GPU and the
> other OOXML/ODF containers are not yet implemented; `.doc` character formatting
> (styles, table grids, raster Art) is not read. What Docling and its competitors have
> that we do not yet, and the plan to close it, is written up as **Phase 11** below.

## Key Constraints & Feasibility Notes

### "From the Ground Up" / Pragmatic Reuse Constraint
The requirement has been clarified: **use open source code if possible; if not available, write it ourselves.**

This means:
- **Reuse infrastructure** where reimplementing is not the core value (ZIP decompression, XML parsing, image codecs, GPU APIs, inference runtimes). These are well-solved problems.
- **Write domain logic ourselves** (PDF object model, DOCX document structure parsing, layout analysis, reading order, JSON export format handling, etc.)
- **For OCR**: Start by using Tesseract via Rust bindings (e.g., `leptess`, `rusty-tesseract`) as it's mature and open source. Only reimplement OCR if Tesseract doesn't meet specific constraints.
- Learn from existing implementations (like Tesseract) when writing our own code, but don't copy code.

**Reality Check**: This hybrid approach is much more feasible than pure-from-scratch. We still need to implement significant domain logic (PDF parsing, layout analysis), but we avoid reimplementing codecs, compression, or a full OCR engine initially.

## Tech Stack Recommendations

| Component | Technology | Rationale |
|---|---|---|
| Language | [Rust](https://www.rust-lang.org/) | Performance, memory safety, cross-platform, great concurrency, single-binary distribution |
| CLI | [clap](https://github.com/clap-rs/clap) (derive) | Standard CLI UX, follows Unix/Linux conventions as required |
| Parallelism | [rayon](https://github.com/rayon-rs/rayon) | Simple parallel iterators, easy to configure thread pools (1-1000) |
| Image decoding | [image](https://github.com/image-rs/image) | Reuse for JPG/PNG/other formats. Codec impl is not core domain logic. |
| Image processing | [imageproc](https://github.com/image-rs/imageproc) | Useful primitives; implement custom algos only where needed for our pipeline. |
| GPU compute | [wgpu](https://github.com/gfx-rs/wgpu) | Cross-platform GPU API - reuse as infrastructure. Implement our own compute kernels. |
| XML parsing (DOCX) | [quick-xml](https://github.com/tafia/quick-xml) or [roxmltree](https://github.com/RazrFalcon/roxmltree) | Reuse for XML parsing. Document structure interpretation is our logic. |
| Zip extraction (DOCX) | [zip](https://github.com/zip-rs/zip) | Reuse for ZIP/DEFLATE. OPC package handling and parsing is our logic. |
| OCR (initial) | [leptess](https://docs.rs/leptess) or [rusty-tesseract](https://github.com/thomasgruebl/rusty-tesseract) | Use Tesseract (open source) via bindings/wrapper. Reimplement only if constraints require it. |
| UI | [tauri](https://github.com/tauri-apps/tauri) | Cross-platform, simple (file picker + settings), calls CLI. Alternative: egui (pure Rust) |
| Logging | [tracing](https://github.com/tokio-rs/tracing) | Structured debugging for complex pipeline |
| Error handling | [thiserror](https://github.com/dtolnay/thiserror), [anyhow](https://github.com/dtolnay/anyhow) | Robust error handling for malformed documents |

## Architecture Design

### Crate Structure (Modular)
```
pdf-extractor/
├── crates/
│   ├── pdf-parser/      # PDF parsing from spec
│   ├── docx-parser/     # DOCX (OPC + XML) parsing
│   ├── doc-parser/      # legacy .doc (OLE compound file + Word Fib/piece table)
│   ├── text-parser/     # RTF, HTML, Markdown/text, CSV/TSV
│   ├── layout/          # Layout analysis, reading order, blocks, tables
│   ├── ocr-engine/      # OCR pipeline (inspired by Tesseract)
│   ├── core/            # Shared types, Docling JSON schema
│   ├── json-exporter/   # Serialize to Docling JSON format
│   ├── cli/             # CLI interface
│   └── ui/              # Tauri UI wrapper
├── tests/               # Golden tests with sample docs
└── docs/                # Architecture notes, specs
```

### Data Flow
1. Input file/dir discovery (CLI)
2. Format detection (PDF, DOCX, legacy `.doc`, RTF/HTML/Markdown/text/CSV)
3. Parsing (extract text, images, fonts, structure, positions)
4. Layout analysis (blocks, columns, tables, reading order)
5. OCR (if rasterized content/images; skip if digital text available)
6. Post-processing (normalization, reading order)
7. Export to Docling JSON
8. Write to output path (or same dir as input)

## Implementation Phases

### Phase 1: Research & Design (1-2 weeks) - Critical
**Goal**: Reduce risk by clarifying scope and prototyping.

- [ ] **Get Docling JSON schema**: Study [Docling](https://github.com/docling-project/docling) format. Create Rust types matching schema exactly. Collect example JSON outputs for test docs.
- [ ] **Clarify "from ground up" boundaries**: Document what is allowed vs forbidden (image codecs, zip, wgpu, etc.). Get stakeholder alignment.
- [ ] **Study PDF spec**: Read PDF 1.7 reference. Understand objects, streams, fonts, graphics.
- [ ] **Study Tesseract**: Read Tesseract architecture docs, source layout. Focus on: binarization, page segmentation (PSM), layout analysis, line/word/char segmentation, recognition pipeline.
- [ ] **Prototype PDF parsing**: Parse trivial "hello world" PDF from raw bytes. Validate feasibility.
- [ ] **Prototype OCR pipeline**: Implement minimal binarization + line detection on simple test image.
- [ ] **Define MVP scope**: Decide which features are must-have for first release vs later.

### Phase 2: Core Types & JSON Schema (3-5 days)
- [ ] Define Rust structs for Docling JSON (pages, blocks, text, tables, images, bbox/coords, metadata)
- [ ] Implement serialization (serde) to match exact JSON format
- [ ] Golden tests with known Docling outputs

### Phase 3: DOCX Parser (1-2 weeks) — largely done
- [x] Parse DOCX as OPC package (ZIP of XML), reusing the `zip` crate.
- [x] Parse `document.xml`; read styles for heading/list/caption labels.
- [x] Extract text, paragraphs, table grids, images (relationship id → `word/media/`).
- [x] Vector drawings (`image/emf`, `image/wmf`) — Word's own, and the preview of an
  embedded `w:object` such as a MathType equation — played into PNGs by
  `crates/metafile`, so a drawing is a picture with pixels instead of a box with no image.
  A bitmap a record embeds (`STRETCHDIBITS`, `DIBSTRETCHBLT`) is decoded from its DIB, and
  that DIB header has one parser in `crates/metafile::dib` — public because an RTF
  `\wbitmap` payload is the same header with no file header and the RTF reader wraps it,
  so the wrapper and the renderer read the palette and mask offsets the same way.
- [x] An embedded **SVG** part — Word's `asvg:svgBlip` drawing, PowerPoint's own
  `r:embed`, a shape's `a:blipFill` — played into a PNG at the frame the package places
  the picture in, 144 dpi, by `crates/svg` (resvg): a whole graphics language is not an
  encoding anyone downstream can show, so no office picture is left as markup. Where
  the package states no box the drawing's own viewport is the size, and the background
  is the drawing's own — transparent unless it paints one. A drawing that does not
  parse, or paints nothing, keeps its bytes and its `image/svg+xml` type rather than
  becoming a blank picture. Flow pages are untouched: a Markdown or HTML `svg` is not an
  office package's picture.
- [x] VML drawings (`w:pict`) — an autoshape, a group of them or a WordArt with no image
  part behind it — painted into PNGs at 144 dpi, their preset `path` formulas resolved;
  a shape's `mc:Fallback` copy is read when its DrawingML gave no picture. The words in a
  shape's textbox stay text; a WordArt's `v:textpath` string is drawn.
- [x] DrawingML shapes (`wps:wsp`, `wpg:wgp`, `wpc:wpc`) — the shape a modern file
  states with no image part and, in a LibreOffice file, no VML copy at all — painted
  into PNGs at 144 dpi: preset outlines (`a:prstGeom`) and a shape's own `a:custGeom`
  path, its transform (offset, flip, rotation), a group's child coordinate space, and
  fills and lines resolved through `wps:style`'s `fillRef`/`lnRef` and the package's
  theme (`word/theme/theme1.xml`), including `shade`/`tint`. A shape's words stay text.
- [x] The preset library (`a:prstGeom`) — beyond Word's handful of boxes and stars: the
  rounded and snipped rectangles, rings and frames, mathematical signs, brackets and
  braces, the many-pointed stars, flowchart symbols, cylinders, cubes and bevels (their
  rim and front edges stroked as lines, never filled), the arrows, the decorative
  outlines, the arcs and the straight, bent and curved connectors. A preset outside the
  table is still no picture rather than a box in its place.
- [x] `asvg:svgBlip` — an SVG picture Word 2016 and later states twice, the PNG
  fallback on the `a:blip` and the drawing in the extension beside it: the drawing is
  preferred whenever its part is in the package, and the fallback is used only when
  the extension names something that is not a drawing (or a part that is not here).
- [x] Picture bullets (`w:numPicBullet`, named by a level's `w:lvlPicBulletId`) — the
  one part of a list item that is a drawing rather than words: the bullet's media part
  is reported as a picture of its own, ahead of the words it introduces (which then
  carry no marker), at the frame the bullet is drawn in — `wp:extent` for a DrawingML
  bullet, the VML `style` for the older spelling, and the drawing's own viewport when
  neither states one. An SVG bullet is therefore played into a PNG like any other
  picture in the package, and the fallback beside its `asvg:svgBlip` is never what
  leaves.
- [x] Headers/footers parsed from their own parts and reported as furniture.
- [x] Office Math (`m:oMath`) linearised, with operators read from `m:chr` attributes.
- [x] Unit tests, plus `real_docx_test.rs` over 19 real fixtures (POI, LibreOffice, calibre).
- [x] Numbering definitions (`numbering.xml`) — `numId`/`abstractNum` resolved, markers
  expanded from `lvlText` with per-level counters that restart, bullet glyphs mapped out
  of the symbol private-use area, and `level` reported.
- [x] `w:lvlOverride` — a single list relabelling (`w:lvl`) or restarting
  (`w:startOverride`) itself is applied per `numId`, leaving every other list alone.
- [ ] Layout/sections into pages: DOCX items carry no page geometry.
- [x] Legacy `.doc`: OLE compound container, binary Fib, piece table (CP1252 + UTF-16
  runs), fields, page breaks and inline pictures as placeholders.
- [x] Legacy `.doc` read through the **Writer bridge** when LibreOffice is installed:
  `--convert-to docx` and the DOCX reader, so headings, list markers, merged tables
  and pictures all come through, with the original name, MIME type and hash kept;
  the built-in piece-table reader remains the path when LibreOffice is absent. Pinned
  by the `legacy_brief.doc` / `brief.docx` pair and a whole-document comparison.
- [ ] `.doc` character formatting (`PAPX`/`CHPX`) in the built-in reader, so the
  no-LibreOffice path keeps paragraph styles, table grids and the OfficeArt rasters.
- [x] Flow formats: RTF (groups/destinations/`\ucN`, and `\pict` pictures as items —
  a PNG or JPEG carried as its own bytes, a metafile blip played into a PNG, an old
  Word `\wbitmap` DIB wrapped in the file header RTF leaves out, held by
  `word_bitmap.rtf` to the source BMP byte for byte, and by `rtf_bitmap.rtf` — a real
  word processor's own output — to a readable picture with the source's pixels), HTML
  (tags, entities, tables, lists), Markdown (headings, lists, fences, pipe tables, inline links into
  `hyperlink`), plain text, RFC 4180 CSV/TSV.

### Phase 4: PDF Parser (3-6 weeks) - High complexity — done except where noted
- [x] Lexer/parser for PDF objects (dictionaries, arrays, streams, numbers, strings)
- [x] Stream decoding: Flate, ASCIIHex, ASCII85, LZW (with `/EarlyChange`), RunLength.
  `/DecodeParms` predictors are undone at the filter that produced the data (image
  samples and content streams alike); `/Length` decides a payload's extent whenever the
  terminator agrees with it.
- [x] Font handling (Type1/TrueType) - basic text extraction
- [x] Page tree, content streams, graphics operators
- [x] Extract text with positions (for layout analysis), with real advances from `/Widths`
  and `/W` (simple and Type0), and an average-glyph fallback when they are absent
- [x] Extract embedded images; rasterization is left to `pdftoppm` in the preview
- [x] Handle common PDF structures (linearized files, object streams)
- [x] Unit tests across varied PDFs, plus a real-corpus suite
- [x] Rotated text: the text matrix is composed with the CTM, so a sideways run gets its
  rotated box, joins no horizontal line, and can never be read as the document title.
- [ ] A rotation *angle* on the box, so rotated tables and sideways headings can be
  grouped deliberately rather than only kept apart. (The flag is already used: a sideways
  block in the page's own outer margin is labelled `page_header`, as Docling labels it.)
- [ ] Encrypted PDFs: out of scope (a clear error, not a silent failure)

### Phase 5: Layout Analysis (1-2 weeks) — done
- [x] Detect text blocks, paragraphs, columns (baseline rows cut into gutter-separated runs)
- [x] Table detection (aligned span columns, deliberately conservative)
- [x] Caption and picture detection; pictures placed in reading order
- [x] Reading order reconstruction (recursive XY-cut, columns first, uncovered bands
  only, and a gutter only where both sides run down the page)
- [x] Page segmentation into labelled blocks (`title`, `section_header`, `list_item`,
  `caption`, `paragraph`, `table`, `picture`) with markers and heading depth
- [x] **Page furniture**: the running heads, footers and page numbers of a document are
  labelled `page_header`/`page_footer` by a second, document-wide pass (`mark_furniture`) —
  the same line, digits aside, in the same margin of two or more pages is the page's rather
  than the text's, and a page number can no longer be read as a section heading. 86 items
  across the seven real PDFs, 84 of them agreeing with Docling's own labels.
- [x] **Footnotes**: a note at the foot of a page is labelled `footnote` by the two marks
  that make one (`mark_footnotes` in `crates/layout/src/furniture.rs`) — the rule the page
  drew above it and the marker it opens with, which the body of that page also uses. The
  page's rules are given to `analyze_page` too, so a marker-and-text note block is not built
  into a two-column table first, and a Type 1 font's own encoding is read, so the `∗`/`†`/`‡`
  a TeX symbol font draws come out as those characters. Measured against Docling: 25 of the
  corpus's 27 reference notes labelled, the other 2 held as body text, none missing (see
  `test-data/golden/README.md`)
- [ ] Captions as furniture: a caption is only labelled where it announces itself
- [x] Coordinate system normalization (top-left origin, boxes clamped to the page)
- [x] Corpus-level invariants: no text lost or duplicated, no box collapsed or off-page

### Phase 6: OCR Engine (DONE)
**Tesseract via `leptess`, required and default-on.**

- [x] Integrate Tesseract via `leptess` (system libs + `libclang` for bindgen).
- [x] Extract embedded image XObjects (DCTDecode, FlateDecode, predictors) as the OCR source.
- [x] Map Tesseract TSV word boxes into line-level text items with PDF-point bboxes.
- [x] `--ocr auto|always|never`; failures degrade to a warning, never a crash.
- [ ] Rasterise vector pages for OCR (today only embedded images are OCRed).
- [ ] Use the content-stream `cm` matrix to place OCR boxes on partially-covered pages.

**If custom implementation needed later:**
- [ ] Learn from Tesseract architecture; implement stepwise (preprocessing, segmentation, recognition)
- [ ] Start simple (feature-based for printed text) if going custom route.
- [ ] Consider `ocrs` as alternative Rust-based inference stack if modification needed.

**Note**: For MVP, leverage Tesseract's mature engine. Custom OCR is a major effort - defer unless necessary.

### Phase 7: CLI Implementation (3-5 days)
- [x] Define CLI args per requirements (plus `--ocr auto|always|never`, `--preview`,
  `--preview-dpi`):
  - Input: file path or directory (`-i, --input`)
  - Output: optional (`-o, --output`). If not provided, output in same folder as input (preserve structure for dirs)
  - Parallelism: `-p, --parallel` or in config (1-1000)
  - Other options: format, overwrite, etc. Follow Linux conventions.
- [x] File discovery (recursive if directory)
- [x] Parallel processing with rayon, respect parallelism limit
- [x] Progress reporting: one `tracing` line per converted file plus a summary, written
  to **stderr** (stdout is for the run's own output), and reported per document as it
  finishes rather than collected and printed at the end
- [x] `--progress`: a machine-readable stream — a plan line, then one JSON record per
  finished document — so a parent process can follow a run a line at a time
- [x] Exit codes, error messages
- [x] Output written through a temporary sibling and renamed into place, so an
  interrupted run cannot leave a half-written `*.json`
- [x] 17 end-to-end tests that run the built binary as a child process

### Phase 8: UI (Simple) (1 week)
- [x] **Box overlay preview** (`--preview`) — an HTML page overlay of every extracted box,
  colour-coded by source, with the recognised text drawn in place and a linked item list.
  This is the first increment of the UI phase, and the one that makes extraction quality
  reviewable: the JSON shows what we found, the overlay shows whether it is right. It is
  intentionally plain HTML/CSS with a little inline JS, so it needs no build step and no
  server, and it degrades to a text reconstruction when no page renderer is available.
  A second layer draws the layout blocks, so block boundaries can be judged against the
  runs that produced them.
- [x] Tauri 2 app: file/directory picker (`rfd`), settings panel (parallelism 1–1000,
  output dir, recursion, OCR mode, preview), result pane with the exact command line
- [x] Calls the CLI binary — the UI builds the argument vector and spawns
  `pdfextractor-cli` with `--progress`; the pure logic (`Job`, `pump`, `Settings`) is
  unit-tested without a window, and the process handling is tested against real binaries
- [x] Cross-platform from one codebase: system webview (WebKitGTK / WebView2 / WKWebView),
  static HTML/CSS/JS frontend in `crates/ui/dist`, **no Node or bundler required**
- [x] A conversion is never waited on: the CLI's stream is read on two threads and pushed
  to the page as events (`conversion-started` / `-plan` / `-progress` / `-log` / `-finished`),
  so the window stays live; the document list, progress bar and log are drawn from them
- [x] **Stop**, including on window close: the child process stays reachable, and the
  answer to a click is the run's own exit status, so a stop that arrives after the last
  document does not misreport a completed run
- [x] Minimal, functional UI, with `--selftest [INPUT] [--selftest-stop MS]` to smoke-test
  the whole stack headlessly — including what the page actually rendered, and the stop path
- [x] A dev stand-in for the Tauri API (`dist/mock.js`) so the page can be driven in an
  ordinary browser without rebuilding the app
- [x] **Compare mode**: give the window a Docling reference — a file, or a folder of them
  named after the documents (`test-data/golden/docling/`) — and every converted document is
  drawn beside it block by block, with seven verdicts (same / notation only / differs /
  contained / placed / only ours / only the reference), the label or wording that disagrees, and the
  same character-agreement figure the golden suite prints. A reference item whose letters
  run from one of our word boundaries to another lies **inside** one of our blocks —
  Docling writes an item per line where we keep a paragraph whole — and one block of ours
  against the run of items inside it is a single row, so the panel shows the differences
  that are left rather than one row of "only the reference" per line. An item that run
  cannot reach — the reference put something we do not hold between it and the run — is
  folded onto the row of the block whose words hold it, keeping whatever that row already
  disagreed about, and the *leaders* of such a line come with it: an item the reference
  wrote with no letters in it at all is taken by where it lies, but only onto a block that
  already holds words of the reference there, so a form line whose words we hold in full is
  one row (the dotted leaders named apart from the words) while a rule standing alone in
  our block's area stays the reference's own reading, for the joining by position to pair
  and to label as such. A leader no block of ours holds and none lies along — a line our
  reading produced no block for at all — cannot be compared with anything, so it is
  **placed**: a row that says what it is, **how many items it stands for and the extent
  they cover** — the items of one line of a form are one row, named together, so one line
  of a form is one row of the panel rather than twenty — counted in the header apart from
  the rows only the reference has rather than as text we did not read —
  and **drawn**: the box on an outline of the page's own shape beside the row, so a reader
  sees which line of the form a leader stands on rather than working it out from
  coordinates (a page whose size the reference never states is named without a picture).
  A row's own box answers *which line*, so the placed items are gathered the other way
  round too, **page by page**: every placed item of a page is drawn on that page at once,
  one picture per page above the rows, and each is built before the row list is cut to
  what a panel lists — so a form's leader pattern is one picture rather than one thumbnail
  per row, and a long form's page is whole where its rows are not. Blocks are paired by content in
  body order first, and the boxes the producers published (the PDFs) are part of that
  score: of two pairings the words cannot tell apart the place decides which is taken, and
  a pair the two readings put in different places is a difference that names both. The rows
  that leaves one-sided are joined by **page and box**, so a drawing or table the two
  readings ordered differently is one row rather than one a side — with the page and
  overlap that joined each row on the row itself and the total in the header.  The mode can also be run over the
  whole corpus at once and printed worst first (`crates/ui/tests/corpus_report_test.rs`),
  so which documents a change helped — and which it did not — is a command rather than a
  reading of 37 panels. That report is a **ratchet**: the per-document numbers are kept in
  the repository (`test-data/golden/compare-baseline.json`) and a document that comes out
  with more `differs`, more one-sided rows or a lower character agreement fails the run,
  so a change cannot help one document and quietly harm another; a deliberate trade is
  accepted with `CORPUS_BASELINE=update`, whose diff of the baseline file is the argument.
  The window reads that same file — it lives beside the references it is about, and says
  there how it was measured — and holds every document it finishes to the ratchet it
  remembers: the **floor** under a form's placed items whatever the run was measured with
  (a reading whose items have left the panel is refused rather than drawn, with the count
  that fell named on the panel), and every other rule when the run's settings match the
  recorded measurement (`Report::hold_to_floor`, in the library so the panel, the report
  and the command line's `--compare` mode — which exits non-zero on a fallen document, so
  a fallen panel fails a build — cannot drift apart). It is the golden comparison's
  definitions turned into something a person can read rather than a budget that passes
  or fails, and it is bounded (2,000 blocks a side, 2,000 rows, 300 characters a text) so
  a pathological document cannot hang the window. Exercised end to end by
  `--selftest <doc> --selftest-ref <reference>`
- [x] Installers/bundling: a `v*` tag builds and publishes them — `.deb` and
  `.AppImage` (Linux), `.msi` (Windows), `.dmg` (macOS) — with per-platform
  icons (`.ico`/`.icns` grown from the PNG), this build of the command line
  inside the bundle (which the window names at startup) and the `tessdata` the
  OCR reads beside it, so an installed window converts without a separate
  install (`.github/workflows/release.yml`)
- [ ] Code signing: the artifacts are unsigned, so Gatekeeper and SmartScreen
  ask on first open; signing keys are not in this repository

### Phase 9: GPU Acceleration (Optional - after MVP)
- [ ] Detect GPU adapters at runtime (wgpu)
- [ ] Implement compute kernels for bottlenecks: binarization, morphology, convolution, line detection
- [ ] CPU/GPU fallback logic
- [ ] Benchmarking to validate speedup
- [ ] Feature flag/config to enable/disable

### Phase 10: Testing, Optimization, Docs (Ongoing)
- [ ] Golden test suite (input docs → expected Docling JSON)
- [ ] Performance tests with varied sizes
- [ ] Cross-platform testing
- [ ] Error handling tests (malformed docs)
- [ ] Documentation (architecture, API, usage)
- [ ] Benchmarks (CPU/GPU, parallelism scaling)

## Phase 11: The Next Phase (target v0.2.0) — outputs, OCR, and parity with the field

*Written from a research pass in October 2026 against: Docling 2.138 / docling-core
2.101 — the very versions the test venv carries, so every claim about Docling below can
be re-checked and turned into a test on this machine — plus Marker, MinerU, olmOCR,
Nougat, Unstructured, Surya, LlamaParse and Stirling-PDF, and the mature PDF libraries
PyMuPDF, pdfminer/pdfplumber, Poppler, PDFBox, PDF.js and PyPDFium2. Sources are listed
at the end.*

### The gap, in one table

| Capability | Docling (2.138) | Other converters | Mature PDF libs | Us (v0.1.0) |
|---|---|---|---|---|
| Export formats | 12 `--to` values (`md` **default**, `json`, `yaml`, `html`, `html_split_page`, `text`, `doctags`, `vtt`, `doclang`, `dclx`, `chunks`, `latex`) | Marker: md/html/json/chunks; MinerU: md/html/latex/docx/epub | `pdftotext`, `pdftohtml` | **JSON only** (the preview HTML is a debug overlay, not an export) |
| RAG chunks | `--to chunks`, hybrid + hierarchical chunkers, `--chunks-max-tokens` | Unstructured: by-title/page/similarity; Marker chunks | — | none |
| OCR languages | `--ocr-lang` comma list (engine codes or `iso:` BCP-47) | Surya: 91 languages | — | **hardcoded `eng`** |
| OCR page modes | `--ocr-mode full_page/…`, `--psm` 0–13 | Marker `--force_ocr` / `--strip_existing_ocr` | — | `auto|always|never`, over embedded images only |
| Rotation | engine OSD (Tesseract), olmOCR auto-rotation | | PyMuPDF page rotation | **`/Rotate` ignored** |
| Scan clean-up | deskew/despeckle options in the pipeline | | | none |
| Image codecs | via backends | | `pdfimages`: JPEG, **JBIG2, JPX, CCITT** + masks + Indexed/Separation | Flate + DCT only (a JP2's bytes pass through undecoded, and `image/jp2` cannot be named in an `ImageRef`) |
| Page selection | `--page-range` | MinerU page groups | `pdftotext -f/-l` | none |
| Hyperlinks | schema carries it (`TextItem.hyperlink`; our own golden references contain it) | Marker links; MinerU locators | PyMuPDF links; pdfplumber annotations | PDF `/Link`, DOCX external `w:hyperlink`, HTML `a[href]` and Markdown `[text](url)` all read (internal PDF dests as `#page=N`) and re-emitted by the Markdown (`[text](url)`) and HTML (`<a href>`) writers, so a Markdown round trip keeps its links |
| Outline / TOC | outlines used for heading-depth recovery | Marker TOC in metadata | PyMuPDF/PDFBox/PDF.js TOC | none |
| Encrypted PDFs | `--pdf-password` (docling-parse) | pass-through where supported | passwords everywhere (PyMuPDF, PDFBox, PDF.js) | **no `/Encrypt` handling at all** |
| Malformed PDFs | qpdf-based parser repairs | | MuPDF repairs, PDFBox lenient | no xref rebuild |
| Merged table cells | TableFormer predicts spans; our own references carry 27 spanned cells | Surya full-mode HTML spans | | DOCX `gridSpan`/`vMerge`/`tblHeader` now read into real spans; **PDF cells still 1×1** |
| Formulas | layout model labels `formula` (132 labels in our references), optional LaTeX enrichment | Marker/Surya/Nougat → LaTeX | | DOCX math linearised to Unicode under label `formula`; **no PDF formula detection**; notation deliberately not compared |
| New inputs | PPTX, XLSX, ODF, images, HTML family, markup, mail, … | Marker: +EPUB; MinerU: +EPUB/OFD | | pdf/docx/doc/rtf/html/txt/md/csv/pptx/images/legacy `.ppt` (bridged) — ODF, mail and EPUB still out |
| Image embedding | `--image-export-mode embedded/referenced/placeholder` | Marker embeds in md | | all three modes under the same names: images written beside the JSON (the default, *referenced*), base64 `data:` URIs inside md/html (*embedded*), or `<!-- image -->` where a picture stood (*placeholder*); a Markdown `![alt](source)` or HTML `<img src>` is read back — from beside its file, or, for an embedded page, by decoding its `data:` URI (base64, or the percent-encoded form RFC 2397 allows) into the picture's own bytes — and copied the same way, so both a referenced and an embedded round trip keep their pictures |
| Service mode | docling-serve REST (+demo UI) | MinerU API/WebUI; Unstructured API | | none (desktop window + CLI) |

Everything in the last column except *Service mode* is reachable without ML models; that
is this phase's shape.

### W1 — Output formats: Markdown, HTML, text, YAML (P0, ~1–2 weeks)

The single largest difference between us and every other converter: they all lead with
**Markdown** and we write JSON only. The `DoclingDocument` tree already holds everything
a renderer needs.

- [x] `--to <FMT>` (repeatable). The default stays `json` — the compare mode, the
  ratchet and the window read it — and the README says so plainly; Docling defaulting
  to md is a documented difference, not one to imitate silently. **Done for all five
  formats**: `json`, `md`, `html`, `text` and `yaml`.
- [x] Several `--to` values write siblings (`doc.json`, `doc.md`, …); `-o -` with a single
  format writes to **stdout** (today stdout is reserved for `--progress`, so this is an
  explicit contract, not a side effect). Refused combinations — two formats, a directory
  of inputs, `--progress` — fail before a document is converted.
- [x] `--image-export-mode referenced|embedded|placeholder`: *referenced* is today's
  behaviour, *embedded* is base64 data URIs inside md/html, *placeholder* marks the spot.
  **Done**: the mode is Docling's own enum and every name means what Docling means by it
  — `referenced` writes `![Image](uri)`/`<img src="uri">`, `embedded` writes the
  picture's bytes as `data:<its own media type>;base64,…` (read from beside the output,
  where the extraction put them; never re-encoded, never fetched from a remote URI), and
  `placeholder` writes `<!-- image -->`, Docling's own placeholder text, in both formats.
  A picture with no image at all — vector art, a chart, an unnameable media type — and
  one whose bytes cannot be read fall back to the placeholder in `embedded` mode rather
  than claim a file that may not travel with the page. The image files are still written
  beside the document in every mode (the extraction is shared with JSON/YAML, and the
  embedded bytes are read from them); JSON and YAML keep their references, since the mode
  is a choice about the prose readings. Pinned by `pictures_are_referenced_embedded_or_marked`
  in Markdown and HTML, `an_already_embedded_picture_is_kept_as_written`,
  `a_picture_with_no_bytes_is_a_placeholder_not_a_broken_link`, and the CLI's
  `the_image_export_mode_decides_how_a_picture_travels`.
- [x] **Markdown writer**: body tree → headings (`#` from `title`/`section_header` and
  `level`), lists with their own `marker`s (a printed `1.` an ordered list, a roman
  marker kept as `- i. …`, a bullet glyph dropped for Markdown's own `-`), tables as GFM
  pipe tables (**merged cells flattened**, exactly as Docling's own md export flattens
  them), pictures as images, `formula` items as `$$…$$` once W5 lands LaTeX (until then
  the Unicode reading) — and the reader turns a `$$` block back into a `formula` item,
  so the math golden re-renders byte for byte — `hyperlink`s as `[text](url)` (the
  item's whole text, since the model keeps one URL per item; a destination Markdown
  would end early is angle-bracketed), **furniture omitted** — Docling suppresses
  `page_header`/`page_footer` from md, and we already hold those labels — and the
  **notes layer omitted unless `--include-notes`**, which puts a deck's speaker notes
  after their slide and its comments beside the slide they discuss, each paragraph as
  a blockquote line headed `**Notes:**` so a remark is never mistaken for slide
  content, the speaker's paragraph breaks survive, and a comment opening
  `[author: …]:` is not read as a Markdown link reference definition and dropped from
  the page — every line begins with the label, so no line begins with `[`.
  The reader is its
  inverse — `#` is the title, `##` a level-1 section header, as Docling's own Markdown
  backend reads them — so a rendering read back does not gain a level; consecutive
  quoted lines behind the writer's `**Notes:**` label come back as one note on the
  notes layer, paragraphs split where the lines were, so a deck's remarks are not read
  back as slides. (`crates/md-exporter`, `crates/text-parser`)
- [x] **HTML writer**: the same walk, with native `rowspan`/`colspan` — the merge
  Markdown must flatten is written once here, with the cells it covers not written at
  all; `<thead>`/`<th>` only when the document marks its first row a header, list runs
  as `<ul>`/`<ol>` with each printed number kept in `value`, a `hyperlink` as
  `<a href="…">` around the item's escaped text, the whole page standalone
  (doctype, `utf-8`, the document's name as `<title>`), text escaped. The spans
  are golden-tested beside Markdown's flattening: `merged_cells.docx`, a fixture
  whose one table declares `w:gridSpan`, `w:vMerge` and `w:tblHeader`, pins both
  readings, and  `calibre_demo.docx`'s own merged tables carry the same fidelity
  onto corpus goldens; hand-written tables in `crates/md-exporter`'s unit tests
  still pin the edge cases. `--include-notes` adds the notes layer here too, each
  remark an `<aside class="notes">` labelled `Notes:` so it is visibly beside the
  slides rather than among them, its paragraphs each a `<p>` of their own (only the
  first carries the label), and the HTML reader is its inverse: an aside with that
  class becomes a `Block::Note` on the notes layer — its label dropped, each `<p>` a
  paragraph — so an HTML round trip keeps a deck's remarks remarks, breaks and all
  (`crates/text-parser`). The same reader turns an `<img src>` into a picture item,
  splitting the sentence around it the way the Markdown reader does; the CLI loads a
  local source from beside the page — or decodes the page's own `data:` URI — and
  copies it beside the output, so an HTML round trip keeps its pictures too, embedded
  or referenced. A page's head `<title>` is read as the document's `title` when the
  body has no `h1` — the one thing taken from a head that is otherwise not document
  text — so a title that lives only in the tab is written back instead of emptied;
  a body `h1` still wins and the head's title is dropped with the rest of the head.
- [x] **Text writer**: plain reading order — headings are their words, list markers the
  printed ones, tables the same pipe grid as md (one shared writer), code/formulas
  verbatim, no image paths; `--text-dehyphenate` joins line-end hyphens by default
  (pdftotext's default behaviour), and `--text-dehyphenate false` is the documented
  off switch, in `--help` and in the README; `--include-notes` adds the deck's
  remarks to the reading.
- [x] **YAML writer**: the whole model — pages, groups, furniture, provenance, as
  Docling's `--to yaml` writes it — with `texts`, `tables` and `pictures` listed in
  the walk's reading order and every `self_ref`/`$ref` rewritten to match, so the
  reordering is lossless and the file still parses as the same document; keys come
  out in `--to json`'s order, so two runs of one extract are byte-identical.
- [x] Golden tests: committed `.md` expectations under `test-data/golden/markdown/`,
  byte-exact for ten documents (a one-line PDF, a real form with tables, a paper
  fixture, a DOCX with lists/tables/pictures, a DOCX with Office Math, an HTML file,
  a Markdown file read back as Markdown, a Markdown file whose picture is a real file
  beside it — copied beside the output and referenced as
  `![Image](pictures_images/img1.jpg)` — a DOCX whose table merges cells, and a PDF
  whose link annotation must come out as `[text](url)`), regenerated deliberately with
  `UPDATE_GOLDEN_MD=1`. The nine documents the format goldens share with that set are
  pinned as `.html`, `.txt` and `.yaml` under
  `test-data/golden/{html,text,yaml}/`, regenerated with
  `UPDATE_GOLDEN_FORMATS=1`, and a run asking for all five formats at once is pinned
  too, so one conversion is reviewable as bytes. `chart_deck.pptx` joins that set as
  the document whose only picture is a drawing with no bytes — a chart, held as a
  `PictureItem` with a `tabular_chart` and no `image` — so the writers' `referenced`
  page says nothing for it, `embedded` and `placeholder` mark the spot, and the
  picture gate's mark check has a case it could not otherwise reach (its placeholder
  pass reads the document a golden came from, and holds the pages block by block for
  a mark that replaces no block). A round-trip gate re-reads every
  committed `.md` golden and renders it again, holding the item list and both
  Markdowns identical, so the reader stays the writer's inverse. **Every `.txt` and
  `.yaml` golden is held to the same standard**: `every_text_golden_is_a_fixed_point`
  reads each one back with the real binary and requires its own bytes, then reads the
  result again and requires those — which is what shapes the plain-text reader: a run
  of non-empty lines is one block, a single line a paragraph, a longer run verbatim,
  the writer's pipe grid a table; `every_yaml_golden_is_a_fixed_point` parses each one
  into the document model and renders it again, so the notation settles too (sorted
  keys, a `pages` map, every `self_ref`). The JSON schema layer and the
  compare ratchet keep running
  untouched — they only ever read JSON.
- [ ] Window: a format picker in settings; the result pane names every file written.

Acceptance: `pdfextractor-cli -i doc.pdf --to md --to html` writes both; the md of our
Docling-corpus documents is *readably* equivalent to `docling --to md` of the same files
(a reviewable diff, never a byte-match — where the two readings split runs is already
documented in `test-data/golden/README.md`); every existing test stays green.

### W2 — RAG chunk export (P0 after W1, ~3–5 days)

Docling's `--to chunks` and Unstructured's chunkers exist because the primary consumer
of these documents is now an LLM pipeline — and our body tree, with reading order and
heading levels, is exactly the input such a chunker wants.

- [ ] `--to chunks` → JSONL of records shaped like
  `docling_core.transforms.chunker.doc_chunk.DocChunk` (the class is importable from the
  test venv; read its fields at implementation time rather than guessing).
- [ ] `--chunks-type hierarchical|hybrid`: hierarchical splits on the body tree
  (headings/sections); hybrid packs sentences to `--chunks-max-tokens` with overlap,
  counting with the `tokenizers` crate (open-source infrastructure, the same class of
  reuse as `zip`/`quick-xml`) so a Docling-compatible tokenizer can be loaded via
  `--chunks-tokenizer`.
- [ ] Every chunk carries the provenance of its span (page + box) so a RAG consumer can
  cite and retrieve — the stable page/block-locator idea MinerU builds its library on.
- [ ] Tests: chunks join back to the body text, the token budget is respected, and a
  corpus spot-check prints chunk counts per document.

### W3 — OCR completeness (P0, ~1–2 weeks)

OCR is a *required* capability of this project; today it is English-only,
right-side-up-only, and blind to any scan whose image we cannot decode.

- [x] **`--ocr-lang`** — a comma list (`eng,deu,fran`) passed to Tesseract, with the same
  setting in the window (the window lands with W7). A package's own `tessdata` is pointed
  at only when it carries *every* requested language, so a shipped English does not hide
  a system's German. (Adding more languages' data to the installers stays open.)
- [x] **`--psm`** — Tesseract page segmentation 0–13, refused out of range before any
  work, and a mode Tesseract will not take falls back to its default with a warning
  rather than losing the text.
- [x] **Honour `/Rotate`**: the page is turned — boxes, page size, the lot — at
  extraction, and a scan's bitmap is turned with the page before recognition, so a
  sideways-captured scan (`scanned_rotated.pdf`) comes back upright with its boxes in
  the displayed page's space. This is the page frame, distinct from Phase 4's
  box-*angle* item, which is about grouping rotated runs.
- [x] **OSD auto-upright**: when auto-OCR meets a page with no text layer, find which way
  up it is and rotate the bitmap before recognition — what olmOCR calls auto-rotation.
  **Done**: `ocr-engine`'s `detect_upright` reads the page in a reduced copy's stored
  turn and half turn (and, when the stored reading is weak, both quarter turns), scores
  each by its confident characters, and turns the page only when another turn reads
  clearly better. No extra `osd.traineddata` is needed, so a packaged build keeps
  working with the language data it already ships. The CLI applies it to a scanned PDF
  page with no `/Rotate` whose picture covers the page, and to a plain image with no
  EXIF tag (an explicit tag is still trusted); the page size and every box follow the
  turned grid. Tesseract's OSD is deliberately not used: it needs a data file that
  would then have to be shipped and kept beside every language.
- [ ] **Deskew/despeckle before recognition** (`imageproc`: projection-profile skew angle,
  rotate small angles; Sauvola threshold on the OCR input only, never on the output).
  Flag `--ocr-preprocess auto|on|off`, default auto.
- [ ] **Decode the scans we refuse today**: `CCITTFaxDecode` (G3/G4) and `JBIG2Decode`
  image XObjects — the two codecs scanned documents in the wild actually use — plus
  `JPXDecode` (JPEG 2000), whose bytes today pass through with nothing able to decode
  them. These are codecs, i.e. infrastructure, so an open-source decoder is the correct
  spend under the reuse policy.
- [ ] **Full-page OCR** for vector-only pages: rasterise the page, then the existing
  pipeline (Docling's `--ocr-mode full_page`). Interim path: shell to `pdftoppm` when
  present — the preview already soft-depends on it and degrades with a clear message;
  the honest long-term path is a small in-tree rasteriser (text, images, fills at
  216 DPI, Docling's own default scale), tracked here rather than hidden.
- [ ] Fixtures and tests: a rotated page, a non-English scan, a JBIG2 scan and a CCITT
  fax scan (generated from the existing scanned fixture), asserting recognised text,
  box placement, and no ratchet movement on the PDF corpus.

### W4 — PDF depth: links, outlines, encryption, repair (P1, ~1–2 weeks)

What every mature PDF library treats as table stakes, measured against PyMuPDF,
PDFBox and PDF.js.

- [x] **Link annotations → `hyperlink`**: read `/Annots` `/Subtype /Link` (`/A /URI` and
  internal `/Dest`), join each rectangle to the runs inside it, set
  `TextItem.hyperlink`. The field exists in the committed schema
  (`test-data/golden/DoclingDocument.json` names both `hyperlink` and `FormulaItem`) and
  Docling's own references already carry hyperlinks, so compatibility is proven rather
  than assumed.
  **Done**: every page's annotations are read in `crates/pdf-parser` — `/A` `/S` `/URI`
  kept as written; a `/Dest` (or a `/GoTo` action's `/D`) resolved to the page it names
  and kept as `#page=N`, a *named* destination dropped rather than guessed at without
  the name tree; hidden annotations (flag bit 2) and non-link subtypes ignored; the
  `/Rect` flipped out of PDF's bottom-left origin and turned with the page's rotation
  like any other box. The CLI joins a rectangle to a text item's box when more than
  half the item stands inside it — a rectangle touching part of a paragraph claims
  none of it — which is where Docling's own PDF references put their links (a
  footnote's lines, wrapped whole). Fixture `test-data/link_annotations.pdf` carries
  all four cases (URI, internal dest, hidden, non-Link) and the round trip runs through
  `cli_test.rs`.
- [x] **`w:hyperlink` in DOCX** and **`a[href]` in HTML** — the same field from the other
  two backends (Docling's DOCX conversions populate it too; our references contain it).
  **Done**: a DOCX `w:hyperlink r:id` resolves through the relationship's external
  target (`TargetMode=\"External\"` targets are kept exactly as written) and splits its
  sentence the way Docling's own reference of `calibre_demo.docx` splits it — the words
  before, the link's words carrying the URL, the words after — while `w:anchor` jumps
  name no URL and their words stay in the sentence, the reading that same reference
  shows. HTML `a[href]` cuts at the anchor's boundaries the same way, and neither side
  re-prints a list's bullet or depth on a continuation: the first piece carries them,
  the rest keep the label they shared. The three moved goldens (calibre's split, the
  flow file's split, both files' YAML carrying the URL) are the review.
- [ ] **Outlines as a heading signal**: read `/Outlines`, resolve targets to pages, and
  use agreement with our layout-derived `level` as evidence — Docling uses bookmarks
  exactly this way. Outlines are *not* written into the `DoclingDocument` (the schema has
  no home for them); they surface in `--progress` and the preview sidebar.
- [ ] **Encrypted PDFs, two steps**: (a) recognise `/Encrypt` and fail with a clear,
  actionable error instead of today's undefined behaviour; (b) implement the standard
  security handler (RC4 40/128, AESV2, AESV3 up to revision 6) behind `--pdf-password`
  — Docling-parity flag — with test vectors from public corpora (pdf.js, qpdf).
- [ ] **xref repair**: when `startxref` or the xref table is broken or lies, rebuild by
  scanning `N G obj` declarations — the tolerant-parse path every mature engine has
  (MuPDF repairs, PDFBox lenient mode, qpdf behind docling-parse). Fixtures: truncated
  files and corrupted trailers generated from our own corpus, with expected-page
  assertions so that repair is measured, not vibes.
- [ ] Deliberate omissions: AcroForm values and tagged-PDF structure-tree reading order
  are **not** in this slice — Docling extracts neither from PDFs today; both are recorded
  under "out of scope" below.

### W5 — Table & formula fidelity (P1, ~1–2 weeks)

- [x] **DOCX merged cells**: `w:gridSpan` → `col_span`, `w:vMerge` restart/continue →
  `row_span`, `w:tblHeader` → `column_header` (today's row-0/col-0 heuristic stays as the
  fallback where the markup says nothing). Every one of the references' 27 spanned cells
  becomes a correct row instead of a compare-mode disagreement. **Done**: the parser
  resolves each cell's grid column (`w:gridBefore`/`w:gridAfter` included), extends the
  cell that opened a `w:vMerge` over the rows that continue it (continuing cells are
  covered slots, not cells), and honours `w:tblHeader` — a `GridTable` path in the
  builder carries the spans the rectangular rows could not express, and
  `merged_cells.docx` golden-tests the whole reading in HTML, Markdown, text and YAML.
- [ ] **PDF table cells**: where the conservative detector already sees aligned columns,
  cut the table into cells (aligned span runs × ruling lines), give each cell its own
  provenance box (`TableCell.bbox` is in the schema and currently unused), and infer a
  span where a cell's area is not covered by a neighbour.
- [ ] **OMML → LaTeX**: extend the lineariser so `formula` items carry Docling's
  notation — fractions, scripts, radicals, matrices, operators, Greek first — while
  `orig` keeps the Unicode reading. Only then let the compare mode start scoring formula
  notation (skipped deliberately today, `test-data/golden/README.md`), as one reviewed
  ratchet reset.
- [ ] **PDF formula regions**: a first *heuristic* detector (math-font runs, symbol
  density, a centred isolated line) emitting `label: formula`, conservative by
  construction — an unsure region stays a paragraph, and the corpus ratchet proves no
  document got worse. Model-based detection (Docling's CodeFormulaV2 class of solution)
  is Phase 9 territory and is not promised here.

### W6 — New input formats: images, XLSX, PPTX, ODF (P1→P2, ~3–5 days each after the first)

- [x] **Images as input** (`png/jpeg/tiff/bmp/webp`): one image is one page, straight
  into the OCR pipeline with the right `origin.mimetype`. **Done**: `crates/image-parser`
  names the format from its signature and reads the page from the header alone — the
  pixel grid plus the resolution the file declares (PNG `pHYs`, JPEG JFIF then EXIF,
  TIFF `XResolution`/`ResolutionUnit`, BMP pixels per metre), with 72 dpi when it
  declares none, which is exactly how Docling's image backend sizes the page. A JP2
  is read from its `jp2h`/`ihdr` box, the only place a JP2 states its grid, and from
  the `res`/`resc` box when its writer stated a capture resolution — samples per
  metre, so a 150 dpi scan is 5906 of them; a JP2 stating none is 72 dpi like a
  WebP's. A GIF's grid is in its logical screen descriptor, and every frame of an
  animation is a page: `gif_frames` decodes them all, composited on that screen the
  way a player shows them and laid on white where a frame leaves the screen
  untouched, so a hundred-frame animation is a hundred pages. (Docling's own
  backend reads the first frame only; the pages after it are a departure this
  project takes deliberately rather than a reading it copies.) The CLI
  maps that to page 1 in points, keeps `image/png`/`image/jpeg`/… as the document's
  MIME type rather than calling it a PDF, runs Tesseract over the original bytes —
  over the reader's decoded PNG for a JP2 — and feeds the line boxes through the same
  layout pass as a scanned PDF. `--ocr never`
  still writes the page, with no text, because the flag says so. `invoice_scan.png`
  (written by `test-data/make_scan_images.py`) is OCRed end to end, and its real Docling
  conversion (`test-data/golden/docling/invoice_scan.json`) is compared by the golden
  suite: **1/1 page, equal size, text agreement 1.000**, accepted by docling-core. The
  same scan in JPEG, TIFF, BMP, WebP, GIF and JPEG 2000 (`test-data/images/`) is held to the
  page facts its own header declares. A **photographed-page corpus** (`test-data/photos/`, written
  by `test-data/make_photo_corpus.py`) now scores OCR quality itself: five skew/uneven-
  light/soft pages with ground truth, scored by character and word edit distance and
  held to per-page and aggregate floors by `crates/cli/tests/ocr_quality_test.rs`.
  A camera's **EXIF orientation** is read from JPEG APP1, PNG `eXIf`, TIFF tag 274 and
  WebP `EXIF`: the page is measured from the upright grid (a quarter turn swaps the
  axes and moves the x resolution to the y axis), and the CLI turns the decoded bitmap
  upright before Tesseract sees it, so the line boxes are in the upright pixel space.
  `06_rotated.jpg` — the invoice stored 90° counter-clockwise with orientation 6 — is
  read at 100% and reports an A4 portrait page, and the same page with the tag cleared
  reads as noise, so the fixture pins the tag and not just the rotation.
  Not read yet: a multi-frame TIFF's later frames, PDF-style `--preview`, and a text
  layer (there is none to read). An animated GIF's frames after the first *are* read:
  each is a page of its own — the logical screen's size, its own pixels, its own words
  — and `test-data/images/turning.gif` (three frames that name themselves) is held to
  three pages with their own text by `crates/cli/tests/cli_test.rs`.
- [x] **XLSX**: a sheet is a table — the highest-value Office gap. Reuse the table
  machinery (and W5's span handling for merged ranges) and the OPC plumbing; map each
  sheet to a page in the shape Docling's XLSX backend produces — convert the same fixture
  with the installed Docling and hold to it, exactly as the DOCX parser's suite does.
  **Done**: `crates/xlsx-parser` reads the package's own parts — `xl/workbook.xml` for
  the sheet names, their order and their visibility, the workbook relationships for each
  sheet's part, `sharedStrings.xml` for the strings cells index and `styles.xml` for the
  number formats that make a serial a date — and cuts a worksheet into its connected
  regions of cells (four-directional, no gap tolerance: Docling's default), each one a
  `GridTable` whose merged ranges are spans and whose covered slots are not written at
  all. The CLI maps a sheet to a page in cell-index units, a `sheet` group per worksheet
  with its tables beneath it, and a hidden worksheet to the `invisible` content layer.
  `inventory.xlsx` (written by `test-data/make_inventory_xlsx.py`, read by the crate's
  fixture test and by `cli_test.rs`) carries a header block, an empty row that splits the
  sheet, a column merge, a boolean, a date and a hidden sheet, and its real Docling
  conversion (`test-data/golden/docling/inventory.json`) is compared by the golden suite:
  **3/3 pages, text agreement 1.000**, accepted by docling-core. The conformance checker
  now validates `groups` and their hierarchy too, which no earlier document exercised.
  A worksheet's **pictures** are read as well: the worksheet part names its drawing
  part, that drawing's relationship part names the media part each anchor reaches, and
  the anchor (two cells, a cell and a size, or a position in EMU) gives the box in
  cells — offsets inside a cell included — the same units a table's box is in. The CLI
  hands each picture on under the office rule (a PNG or a JPEG as itself, everything
  else re-encoded as PNG) and places it among the sheet's tables in reading order.
  `worksheet_pictures.xlsx` (`test-data/make_pictures_xlsx.py`, written with
  `xlsxwriter`) carries a BMP, a GIF, a PNG and a JPEG across two sheets, and the CLI
  test holds each written file to the pixels of the part it came from. Not read yet:
  charts, cell comments, Docling's `treat_singleton_as_text` option, and its split of a
  merged label row out of the table; the workbooks are fixtures beside the corpus rather
  than in it.
- [x] **Borrowed readers, and format detection that trusts the bytes**: the formats
  no specification describes — Microsoft Works (`.wps`, `.wp`, `.wks`, `.wdb`),
  WordPerfect (`.wpd`) and legacy Excel (`.xls`) — are read through **LibreOffice**
  when it is installed, and through nothing else. The CLI sniffs every input first
  (PDF, compound file, `01 FE` Works, ZIP, RTF, HTML, text) and lets the bytes pick
  the reader, so `.wps` is read as whatever it actually holds: a DOCX, a Word 97
  binary, or Works proper. A bridged document keeps the original file's name, MIME
  type and hash. **Done**: `crates/cli/src/formats.rs` — `sniff`/`detect` over magic
  bytes and package part names (`word/document.xml`, `xl/workbook.xml`, the
  `WordDocument` stream name in UTF-16LE, which is in all four Word fixtures and
  none of the 40 Works samples), and `bridge`, which runs `soffice --headless
  --convert-to docx|xlsx|csv` under a private profile and reads the result with our
  own readers. Measured on libwps' corpus: **38/40 Writer, 24/26 spreadsheets, 2/2
  databases**; the two failures are truncated bug-tracker attachments. **Legacy
  `.ppt` is bridged too**: a compound file whose `PowerPoint Document` stream names
  it goes to `--convert-to pptx` and through the PPTX reader, with the original
  file's name, `application/vnd.ms-powerpoint` and hash kept, and `test-data/legacy_deck.ppt`
  (Impress' own PowerPoint 97 write of the deck fixture, by `make_legacy_ppt.py`)
  pinned in `cli_test.rs`. The fixture is regenerated from the noted deck, and the
  bridge test holds the **whole converted document** to the native `q3_review.pptx`
  — pages, groups, every item, label, layer, word, table cell and picture, plus the
  extracted image's bytes. Only what the package owns is normalised away: the
  original identity (asserted separately), the boxes Impress re-lays out, its comment
  ids, and the image directory named after each output. A legacy deck therefore
  keeps everything across the format boundary, not only its remarks. **Legacy Word
  `.doc` is bridged to the same Writer side**: where LibreOffice is installed the
  CLI converts it to `docx` and reads that, because the built-in piece-table reader
  has no paragraph or character properties; the pair `legacy_brief.doc` /
  `brief.docx` (by `make_legacy_doc.py`) is held to the **whole converted document**
  — every item, label, marker, level, word, link, table cell, span and picture, with
  only the container's identity and the image directory normalised away — and
  without LibreOffice the built-in reader still opens the file, so the bridge is a
  better reader rather than a requirement.
  Not bridged: Kingsoft-era `.wps`/`.et`/`.dps`
  (LibreOffice's Works filter does not accept them, and no public sample exists to
  try), `.pub`.
- [x] **PPTX**: slides as pages (geometry from `presentation.xml`), `txBody`
  runs, `graphicFrame` tables, media through the relationships. **Done**:
  `crates/pptx-parser` reads the package the way its own parts describe it —
  `ppt/presentation.xml` for the slide order and the deck's EMU size, the
  presentation relationships for each slide's part, a slide's relationships for
  its layout and its pictures, the layout's for the master. Each slide is a
  `chapter` group in the CLI's document, with the slide's shapes in visual order
  (top to bottom, then left to right, within 0.05 in on a line). A paragraph is
  read from its `txBody` as runs with bold/italic/underline/size and joined into
  one line — a `<a:br>` a space — and its bullet or number is resolved through
  paragraph → shape list style → layout placeholder → master text style, which
  is why a body placeholder's paragraphs are list items on a real deck; an
  auto-numbered list counts from `startAt` and continues at its level. Tables
  keep `gridSpan`/`rowSpan` and leave the covered slots out; pictures are the
  media part their `r:embed` names, written beside the JSON — or, when the picture
  states an SVG in an `asvg:svgBlip` extension, the drawing it names rather than
  the PNG fallback beside it, played into a PNG at the shape's frame. A paragraph's
  **picture bullet** (`a:buBlip`) is a picture too — a square of the item's type
  size at the corner its words start from, since a slide states no line boxes —
  and a shape filled with a picture (`a:blipFill`) is that picture at the shape's
  own frame. A deck's own
  remarks are read on the **notes** content layer: its **speaker notes**, from
  the notes part the slide's relationship names — the body placeholder, read the
  way python-pptx reads a text frame, so the two readings can be held to each
  other —  and its **comments**, each in a `comment_section` group of its own
  under the body, named `comment-slide<N>-<idx>` and carrying the author, time
  and words the file records in the `[author: …, time: …]: …` form Docling
  writes — the time at PowerPoint's millisecond precision, so the same deck
  bridged through LibreOffice records the same remark as its native original. A **chart** is data rather than a picture file: the `graphicFrame`
  names a chart part whose caches hold its title, categories and series, and
  the CLI reconstructs the plotted grid into the picture's `tabular_chart`
  `chart_data` — a header row naming the series over one column each, the
  categories down the first column, and a point the chart omits left as a gap
  rather than a zero — classifies the picture by the first plot's family
  (`bar_chart`, `line_chart`, `pie_chart`, `scatter_chart`, `other_chart`) and
  adds the chart's title as a `caption` item the picture points back at, with no
  image attached unless the chart was rendered (this does not render it). A placeholder with
  no `a:xfrm` inherits its box from the layout and then the master (mapping a
  content placeholder to the master's `body`), which is where a real deck keeps
  its title and body geometry. A hidden slide (`show="0"`) is kept on the
  `invisible` layer. `q3_review.pptx` (written by `test-data/make_deck_pptx.py`) is
  three slides of titles, master-bulleted body text, a nested bullet, an
  auto-numbered text box, a multi-run paragraph, a merged-cell table, a picture
  and a chart, and its real Docling conversion
  (`test-data/golden/docling/q3_review.json`) is compared by the golden suite:
  **3/3 pages, text agreement 1.000**, every heading kept, accepted by
  docling-core, with its notes, comments and chart in the reference the golden
  suite compares against. **The shapes a slide paints itself** — an autoshape, a
  connector, a group of them — have no image part behind them, so each is drawn
  into a PNG by `crates/drawingml` (the painter a Word shape goes through, now
  shared rather than copied) at its own box: the box from `a:xfrm`, the outline
  from `a:prstGeom`/`a:custGeom`, the fill and line from `p:spPr` or from
  `p:style`'s `a:fillRef`/`a:lnRef` against the master's theme (followed from the
  slide's layout to its master and there to `ppt/theme/theme1.xml`), and the words
  of a `p:txBody` never painted — they are the slide's text. A text box with no
  fill and no line puts no ink down and is no picture; a connector's frame has no
  height, so its box is its stroke; a group is one picture, its children mapped
  into the group's frame. `shape_deck.pptx`
  (`test-data/make_shape_deck.py`, written with python-pptx) holds a rounded box,
  a group, a theme-styled star, a connector, a no-ink text box and an embedded PNG,
  and its parser and CLI tests hold each painted PNG to its colours. A shape is
  shaded the way the file states, not flushed to one colour: an `a:gradFill` is
  the ramp it names — its stops with their own opacity, along the `a:lin` angle
  (in the shape's proportions, or at its true angle for `scaled="0"`) or out from
  the middle for an `a:path` — an `a:outerShdw` is the shape's silhouette moved,
  blurred and laid down under the ink, and an `a:headEnd`/`a:tailEnd` is a
  triangle, stealth, diamond, oval or open head at that end of a line, sized from
  the line's own width, with the stroke stopped at its base. `shaded_deck.pptx`
  (`test-data/make_shaded_deck.py`) holds one of each, and `crates/drawingml`'s
  unit tests pin the ramp's colours, the shadow's offset and falloff, and each
  head's geometry. Not read yet:
  SmartArt, equations, a comment's anchor (which names no shape and has nowhere to
  go in the model), linked pictures and vector media (EMF/WMF is placed with its
  box and no image); patterns, 3-D, the effects beyond an outer shadow and a
  shape's text are not painted. Legacy `.ppt` is not read here; it goes through the
  bridge above.
- [ ] **ODF** (`odt` first, then `ods`/`odp`): `content.xml` walks the flow-format path,
  `styles.xml` supplies heading levels.
- [ ] Each format joins the golden corpus with real-world fixtures (POI/LibreOffice-
  saved, like the DOCX suite) and a Docling-converted reference under
  `test-data/golden/docling/`, so the schema layer and the ratchet cover it with no new
  machinery.
- Order agreed for the rest of the work: **PPTX → images → ODF (odt, ods, odp) →
  EPUB and email (.eml) → the legacy binaries through the bridge** (`.ppt` done,
  and any OneNote/Visio/Publisher work that earns its place). Every format in that list
  before the bridge has a public specification *and* a Docling backend, so the golden
  ratchet covers it; the bridge family is the one place the reference is LibreOffice's
  reading instead, which is why it is its own item and never a silent fallback.

### W7 — CLI & window surface (P0 alongside W1, ~2–3 days)

- [ ] `--page-range 1-4,9` (PDF first; images/XLSX when they land), applied before
  layout so a range costs only the pages asked for.
- [ ] Wire W1–W3's flags into the window's settings — formats, OCR languages, PSM, page
  range, a password field that never echoes — and let the result pane open the output
  folder.
- [ ] `--selftest` grows one case per new flag; the README flag table gains the same
  rows; the v0.2.0 release notes list them.

### Sequencing

| Order | Work | Priority | Size | Risk | Depends on |
|---|---|---|---|---|---|
| 1 | W1 output formats + W7 flags | P0 | 1–2 wk | low — pure rendering over the existing tree | — |
| 2 | W3 OCR completeness | P0 | 1–2 wk | medium — codecs and rotation reach into the parser | — |
| 3 | W2 chunks | P0 | 3–5 d | low | W1's export plumbing |
| 4 | W4 PDF depth | P1 | 1–2 wk | medium — encryption is fiddly, repair is empirical | — |
| 5 | W5 tables & formulas | P1 | 1–2 wk | medium — every ratchet reset must be argued per document | W1 for LaTeX in md |
| 6 | W6 images → XLSX → PPTX → ODF | P1–P2 | ~1 wk each | low after the first | W5 spans, for XLSX |

Roughly six to eight weeks sequential; W1/W7, W3 and W4 are independent and could run
in parallel.

### Deliberately out of scope for v0.2.0 (recorded, not forgotten)

- **Service mode** — a docling-serve-style HTTP API. The window and the CLI cover our
  stated scope; MinerU's document library and search is the reference if we ever want
  it.
- **Enrichments and VLM pipelines** — formula/picture-description/chart/code
  enrichment, the `--pipeline vlm|asr` world. ML by construction; belongs with Phase 9
  (GPU/ONNX) if anywhere.
- **Model-based layout and TableFormer-class table recognition** — the heuristics plus
  W5 close most of the measured gap; a learned detector is Phase 9 work that must first
  prove an accuracy and latency budget.
- **AcroForm values, tagged-PDF structure tree, page labels, Info-dict metadata** —
  Docling extracts none of these from PDFs today (metadata is on its "coming soon"
  list) and most have no home in the `DoclingDocument` schema. Revisit when Docling's
  schema grows.
- Carried, unchanged: **code signing** (Phase 8), **custom OCR engine** (Phase 6), **GPU**
  (Phase 9), **benchmarks and performance tests** (Phase 10), **`.doc` character
  formatting and DOCX page geometry** (Phase 3), **box rotation angle** (Phase 4),
  **captions as furniture** (Phase 5).

### Phase acceptance

- Zero-warning build; the whole suite, clippy and the `DOCLING_PY` schema layer green on
  all three platforms.
- The compare ratchet holds; any baseline movement is a reviewed, per-document argument
  through the existing `CORPUS_BASELINE=update` discipline.
- Every new flag has an end-to-end test through the built binary (the child-process
  suite), and `--selftest` covers the window's new settings.
- v0.2.0 ships through the unchanged release workflow, with this plan and the README's
  flag table updated in the same change.

### Sources

**Docling**: [CLI reference](https://docling-project.github.io/docling/reference/cli/),
[supported formats](https://docling-project.github.io/docling/usage/supported_formats/),
[serialization](https://docling-project.github.io/docling/concepts/serialization/),
[OCR](https://docling-project.github.io/docling/concepts/OCR/),
[advanced options](https://docling-project.github.io/docling/usage/advanced_options/),
[model catalog](https://docling-project.github.io/docling/usage/model_catalog/),
[enrichments](https://docling-project.github.io/docling/usage/enrichments/),
[docling-serve REST](https://docling-project.github.io/docling/usage/api_server/rest_api/),
[docling-core](https://github.com/docling-project/docling-core),
[docling-parse](https://github.com/docling-project/docling-parse),
[technical report](https://arxiv.org/html/2408.09869v5).
**Converters**: [Marker](https://github.com/datalab-to/marker),
[MinerU](https://github.com/opendatalab/MinerU), [olmOCR](https://github.com/allenai/olmOCR),
[Nougat](https://github.com/facebookresearch/nougat),
[Unstructured chunking](https://docs.unstructured.io/concepts/chunking),
[Surya](https://github.com/datalab-to/surya),
[LlamaParse](https://developers.llamaindex.ai/llamaparse/parse/),
[Stirling-PDF](https://github.com/Stirling-Tools/Stirling-PDF).
**PDF libraries**: [PyMuPDF](https://pymupdf.readthedocs.io/en/latest/),
[pdfminer.six](https://pdfminersix.readthedocs.io/en/latest/),
[pdfplumber](https://github.com/jsvine/pdfplumber), Poppler
[`pdftotext`](https://manpages.debian.org/testing/poppler-utils/pdftotext.1.en.html)/
[`pdfimages`](https://manpages.debian.org/unstable/poppler-utils/pdfimages.1.en.html)/
[`pdfinfo`](https://manpages.debian.org/experimental/poppler-utils/pdfinfo.1.en.html),
[PDFBox 3](https://pdfbox.apache.org/3.0/migration.html),
[PDF.js](https://mozilla.github.io/pdf.js/api/draft/module-pdfjsLib.html),
[PyPDFium2](https://pypdfium2.readthedocs.io/en/v4/python_api.html).

## Cross-Cutting Concerns

### Parallelism (1-1000)
- Use `rayon::ThreadPoolBuilder::new().num_threads(n).build()` or global pool with configurable limit
- Clamp to reasonable range (e.g., 1..=256 or 1..=num_cpus * 4). 1000 threads causes excessive context switching on typical machines. Recommend clamping sensibly with warning/log.
- Process files in parallel; also parallelize page-level work where safe (OCR per page).
- Memory constraints with high parallelism - need resource limits.

### Cross-Platform
- Use platform-agnostic APIs (std + crates). wgpu handles GPU differences.
- Path handling: use `std::path`, handle Windows backslashes
- File I/O: test on all three OSes
- Tauri for UI gives consistent cross-platform packaging.

### GPU Utilization
- **Detection**: `wgpu::Instance::enumerate_adapters(wgpu::Backends::all())`
- **When to use**: Batch processing (many pages/images), large images. CPU is fine for text-only small docs.
- **Strategy**: Implement CPU path first, profile, then offload hotspots.
- **Graceful fallback**: If GPU init fails, use CPU silently or log.

### OCR Strategy Details
**Recommended approach (hybrid):** Start with Tesseract via bindings for maturity and broad language support. Only reimplement if specific constraints (licensing, deployment, customization) require it.

**Options:**
- [leptess](https://docs.rs/leptess) - direct Tesseract bindings (requires Tesseract installed)
- [rusty-tesseract](https://github.com/thomasgruebl/rusty-tesseract) - CLI wrapper (simple)
- [ocrs](https://github.com/robertknight/ocrs) - Rust inference stack (early preview, Latin-only)

**If custom implementation needed:** Study Tesseract, OCRopus, Kraken for algorithms. Target MVP scope (printed Latin text, clear scans) if going custom.

**References to study:**
- [Tesseract source](https://github.com/tesseract-ocr/tesseract)
- [Tesseract documentation](https://tesseract-ocr.github.io/)
- [ocrs](https://github.com/robertknight/ocrs)

## Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|---|---|---|---|
| "From ground up" vs reuse boundaries unclear | Medium | Medium | Document per-component decisions (reuse/wrap/reimplement). Reuse infrastructure (codecs, zip, xml, gpu apis); write domain logic ourselves. |
| PDF parsing complexity (fonts, streams) | High | High | Prototype early. Start with simple text PDFs; handle complex cases later. Write our own PDF object model based on spec. |
| OCR effort high if custom needed | Medium | Low | Start with Tesseract via bindings (proven, fast to integrate). Only reimplement if constraints require it - defer to later phase if needed. |
| DOCling JSON compatibility drift | Medium | Medium | Create comprehensive golden tests against real Docling outputs. Version the schema mapping. |
| Performance with parallelism=1000 | Medium | Medium | Clamp to sensible limits (CPU cores * factor). Make configurable with bounds. |
| GPU kernel complexity | Medium | Medium | CPU-first. Profile before implementing GPU. Use wgpu compute shaders incrementally. |
| Training data for OCR (if ML-based) | High | Medium | Start with feature-based recognition (no training data needed). Add ML later only if required. |

## Open Questions
1. ~~**Reuse policy**: Confirm acceptable crates.~~ Resolved: reuse commodity infrastructure (zip, quick-xml, image, flate2, wgpu, leptess); implement domain logic ourselves.
2. ~~**OCR choice**: Tesseract via bindings?~~ Resolved: `leptess` (Tesseract 5.3.4 + Leptonica 1.82), required and on by default.
3. **Docling JSON schema**: Get exact schema/version to match. Are there edge cases (tables, formulas, code)?
4. ~~**Supported doc formats**: DOCX/DOC - what about DOC legacy (binary)?~~ Resolved: legacy `.doc` is read (container + piece table; text and structure, no character formatting). RTF, HTML, Markdown, plain text and CSV/TSV were added too; ODF/PPTX/XLSX remain open.
5. **Image extraction**: How should embedded images be represented in Docling JSON? Need to match format.
6. **Tables/forms**: Scope - basic tables vs complex nested tables, forms?
7. **Languages**: What OCR languages needed? (affects Tesseract language data)

## Success Metrics
- CLI works on all 3 platforms with standard Unix flags
- Correctly extracts text from simple PDFs and DOCX files
- Produces Docling-compatible JSON (validated against golden set)
- Parallel processing works with settings 1-1000 (scales reasonably)
- UI launches, can select files/dirs, change settings, calls CLI successfully
- Cross-platform single codebase builds cleanly

## Recommendation Summary
**Go ahead, but phase carefully.** Rust is the right choice. GPU is feasible via wgpu. OCR reimplementation is the critical path - start simple (feature-based, printed text) and avoid deep learning initially. The biggest risk is the "from ground up" scope - resolve that boundary first. With phased MVP approach, this is achievable, but full Tesseract parity is a long-term goal, not a v1 requirement.

**Suggested first action**: Phase 1 (research + clarify boundaries + prototyping) before writing any production parsing code.