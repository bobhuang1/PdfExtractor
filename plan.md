# Project Plan: PDF & Office Document Extractor (Docling-compatible)

## Overview
Build a PDF and DOCX/DOC extractor similar to Docling, outputting documents in Docling-compatible JSON format. The key constraint is implementing everything from the ground up (including OCR), learning from Tesseract's approach rather than using existing OCR/PDF/DOCX libraries as black-box dependencies.

**Language**: Rust  
**Platforms**: Windows, Linux, macOS (single codebase)  
**Output**: Docling-compatible JSON  
**Architecture**: CLI-first with simple UI wrapper, parallel processing (1-1000 concurrent conversions)

> **Status:** this file is the forward-looking plan. For what is actually built, tested and
> verified (with commands and results), see [progress.md](progress.md). Current state:
> the workspace builds with zero warnings, 332 tests pass, and PDF, DOCX, legacy `.doc`,
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
> (RTF, HTML, Markdown, plain text, CSV/TSV) are implemented. GPU, installers/bundling and
> the other OOXML/ODF containers are not
> yet implemented; `.doc` character formatting (styles, table grids, raster Art) is not
> read.

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
- [ ] `.doc` character formatting (`PAPX`/`CHPX`) for paragraph styles, table grids and
  the OfficeArt raster images.
- [x] Flow formats: RTF (groups/destinations/`\ucN`), HTML (tags, entities, tables,
  lists), Markdown (headings, lists, fences, pipe tables), plain text, RFC 4180 CSV/TSV.

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
- [ ] Installers/bundling (`.deb`/`.app`/`.msi`, per-platform icons, signing)

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