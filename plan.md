# Plan: finish, test and fix PdfExtractor

*Rewritten 2026-11-19. The plan this replaces — the full roadmap with Phases 1–11 and
workstreams W1–W7, every ticked box and its evidence — is kept unchanged in
[docs/history/plan-2026-11-19.md](docs/history/plan-2026-11-19.md); the 13,700-line
progress log it pointed at is [docs/history/progress-2026-11-19.md](docs/history/progress-2026-11-19.md).
The monitor for this plan is [progress.md](progress.md).*

## Where the project stands

A Rust workspace (25 crates) that converts PDF, DOCX, legacy `.doc`, ODT/ODS/ODP,
XLSX, PPTX, RTF, HTML, Markdown, text, CSV and images into Docling-compatible JSON,
and writes Markdown, HTML, text, YAML and RAG chunks beside it; a Tauri window wraps
the command line. OCR is Tesseract through `leptess`, on by default.

**Done** (verified by the suites, see progress.md): every reader above, the five
writers and the chunker, page ranges, OCR languages/PSM/rotation/auto-upright, PDF
links, footnotes and page furniture, DOCX merged cells and OMML→LaTeX, the window
with its Compare mode, and a corpus ratchet that fails any document whose agreement
with Docling falls. Releases build for Linux, Windows and macOS from a `v*` tag.

**Merged 2026-11-20:** `origin/main` moved on 2026-11-19 by 50 owner commits (shared
resource budgets and fuzz limits, decoded-stream reuse, CCITT G3/G4 and JBIG2 decoding,
redline mode, lint clean-up). They were merged into this branch (`1ceaaba`, a merge, so
the SHAs cited here stay valid), conflicts resolved toward the owner's code; see
progress.md, *2026-11-20 — merge*. CCITT/JBIG2 decoding left the deferred list.

**Merged 2026-11-21:** `origin/main` then gained this finish round itself
(`397168a..0642d2d`: the standard security handler and `--pdf-password`, the
trailer-`/Root` repair, the window's password field, this plan). Merged as
`1fdb79d` together with the budget-flag work (`f593eac`); only `plan.md`
conflicted, and the owner's rewrite was kept whole (the old plan stays in
`docs/history/`). The suite facts below are the merged tree's, and the review
of the merge is logged in progress.md, *2026-11-21*.

**Found on 2026-11-19, before any change:**

1. **This clone's `main` is stale.** The remote's history was rewritten on
   2026-11-03 (attribution lines stripped from every message) and then grew to
   `e8e8267`; the local `main` is the pre-rewrite clone at `e6c0520`, sharing no
   commit with `origin/main`. All work here is on the local branch
   **`finish-plan-2026-11`, cut from `origin/main` (`e8e8267`)**. Local `main` is
   left untouched (see *Blocked* below).
2. **OCR cannot be built natively on this Windows machine**: there is no vcpkg
   Tesseract and no LLVM/libclang. The native suite runs with
   `--no-default-features`; the full suite (OCR on, the window, clippy, rustfmt, the
   compare gate) runs in an `ubuntu:24.04` container built to match the CI image.
3. **Missing capability the old plan still listed as open** and that is in reach
   without ML models: decrypting encrypted PDFs (`--pdf-password`, old W4 step b)
   and a measured answer to "xref repair" (old W4).
4. **A latent PDF bug**: the page tree was taken from whichever `/Type /Catalog` a
   hash map yielded first, not from the trailer's `/Root`, so a file whose
   incremental update added a catalog under a new object number could read either
   revision, differently from run to run.

## Workstreams

Ordered; each has an exit check that is a command or a file, not a judgement.

### WS1 — Baseline on the right history
Branch from `origin/main`, record the divergence, build natively.
**Exit:** `git log -1 origin/main` is an ancestor of `finish-plan-2026-11`; native
`cargo test --workspace --no-default-features --no-run` compiles.

### WS2 — Native Windows suite green
`cargo test --workspace --no-default-features` on Windows (no Tesseract, no
LibreOffice, no Poppler rasteriser); fix every failure that is a defect, and record
every test that skips for a missing tool.
**Exit:** the command exits 0; skips listed in progress.md.

### WS3 — CI-parity Linux run
In a container from `ubuntu:24.04` with the packages `test.yml` installs and
rustc 1.99.0 (the pinned toolchain): `cargo test --workspace --locked` (OCR on,
window built),
`cargo clippy --workspace --all-targets --locked -- -D warnings` (with and without
default features), `cargo fmt --all --check`, and the compare gate
(`pdfextractor-cli --compare` over `test-data/real`, `--ocr never`).
**Exit:** all four exit 0; the documented workspace count below equals
`cargo test --workspace -- --list` there (the documentation test checks it).

### WS4 — Encrypted PDFs, step (b): `--pdf-password`
The standard security handler in `crates/pdf-parser/src/crypt.rs`: RC4 40–128
(revisions 2–3), crypt filters with RC4/AES-128 (revision 4), AES-256 (revisions 5
and 6), user *and* owner passwords, `/EncryptMetadata false`, `/Crypt` identity
streams; decryption before object streams are expanded; cipher primitives from
RustCrypto (`md-5`, `sha2`, `aes`) under the reuse policy, key derivation ours.
CLI: `PDFE_PDF_PASSWORD` (the `--pdf-password` argument is refused, since `ps`
shows it); an empty user password opens
without asking; a wrong or missing password and an unsupported handler are refused
by name with no output written. Fixtures from `test-data/make_encrypted_variants.py`,
each read back by Poppler's `pdftotext -upw`/`-opw` before being committed.
**Exit:** `cargo test -p pdfextractor-pdf-parser --test crypt_test` and
`cargo test -p pdfextractor-cli --test encrypted_test` green; README states it.

### WS5 — Damaged PDFs (xref repair) and the `/Root` fix
The parser already scans every `N G obj` rather than trusting `startxref`, so
"repair" is the normal path; this workstream measures it and fixes the catalog
choice: the newest classic trailer's `/Root`, else the last cross-reference
stream's, else the highest-numbered catalog (deterministic).
**Exit:** `cargo test -p pdfextractor-pdf-parser --test repair_test` green — a lying
`startxref`, no xref/trailer at all, every offset shifted, truncation at 10/50/90%,
and an incremental update that adds a new catalog (read 8 times, same answer).

### WS6 — The window's password
The window offers no PDF password (the old W7 note said there was nothing to send).
Add one: a password field in the settings, passed to the CLI through
`PDFE_PDF_PASSWORD` rather than the argument vector, so it never appears in the
command line the result pane shows or in a process listing.
**Exit:** the UI crate's unit tests pin that the password is in the child's
environment and absent from the argv and the displayed command.

### WS7 — Documentation reset
This plan; progress.md as a monitor (one row per workstream, dated log); README's
PDF row and flag table brought up to the password work; the stale unticked boxes of
the old plan that the code closes noted in progress.md.
**Exit:** `cargo test -p pdfextractor-cli --test documentation_test` green (it holds
the counts below to the suite).

### WS8 — The conversion as a library (pipeline extraction)
`crates/pipeline` (`pdfextractor-pipeline`) holds the conversion and nothing of the
command line's interface: `build_document`/`build_document_with_format`, the reader
dispatch they choose between, the worker OCR engine cache, picture extraction, the
preview writer, `relabel` and the run's budgets. It takes `ConvertOptions` instead of
clap's `Args`, so a caller converts a document without spawning the binary; the CLI
keeps argument parsing, the input walk, the output writers, compare and redline, the
extension gate and its refusals.

**Why a crate and not a `[lib]` target in the CLI.** The one thing the extraction buys
is a dependency graph an embedder can take: the pipeline excludes clap, rayon,
`tracing-subscriber`, `pdfextractor-ui` (the gate and the comparison) and the two
exporters. A library target inside `crates/cli` would drag all of them into any
consumer, which is the cost the split exists to avoid. The window does not embed it — it
spawns the binary and reads its `--progress` records — so the crate's value is the
future embedder, and the plan should not pretend to more than that.

**Phases.** Each phase ends on the same parity checks, so an unfinished phase is a red
check rather than a judgment call. Nothing here restates the counts below; those are the
suite's.

| Phase | What it moves | Parity that must be green |
|---|---|---|
| P1 — the reading (landed in the working tree; the round note below records it) | `build_document`, `build_document_with_format`, the reader dispatch, the worker engine cache, pictures, preview, budgets, `relabel`; `ConvertOptions` replaces clap's `Args`; `crates/cli/src/preview.rs` moves in | `cli_test` unchanged and green — the file not moving is the proof, because it runs the built binary as a child and cannot see a new API; the conversion unit tests move beside the code rather than being rewritten; the CLI's extension and limit-refusal tests stay where they are |
| P2 — the manifest trim (landed; the round note in `progress.md` records it) | the reader crates the CLI no longer calls: `[dev-dependencies]` for the ones only `limits_matrix_test` uses (`pdf-parser`, `docx-parser`, `odt-parser`, `ods-parser`, `odp-parser`, `doc-parser`, `xlsx-parser`, `pptx-parser`, `text-parser`), and deletion for the ones nothing under `crates/cli` names (`chart`, `svg`, `layout`, `ocr-engine`) | the same `cli_test` run, plus `cargo check -p pdfextractor-cli --all-targets --no-default-features`: a binary that no longer links the readers must still convert every fixture, and the test-only paths must still compile |
| P3 — a caller-chosen picture and preview location | `image_base: Option<&Path>` and `preview: Option<&Path>` on `ConvertOptions`; `output_path` stops being a required argument to the reading; `image_dir_for` moves back to the CLI as its naming helper | `cli_test`'s picture and `--preview` cases unchanged; the `-o -` note that names the folder still prints the same path; `md-exporter`'s existing `image_base` is the only base an export resolves a relative URI against |
| P4 — a typed boundary and per-conversion limits | `PipelineError` at the two public entry points (`Limit(LimitError)`, `Encrypted`, `UnsupportedFormat`, `PageRange`, `OcrLangEmpty`, `Io`, `Other(anyhow::Error)`), with `anyhow` left inside the readers; `limits: Limits` on `ConvertOptions` in place of the `OnceLock` and `set_limits`; the flag-named refusals (`--ocr-lang`, `--psm`, `--page-range`) formatted in the CLI | the CLI's exit status `3` for a budget refusal still comes from matching the typed case; `is_limit_refusal` has one definition, in the CLI, and the public one is gone; two documents converted in one process with different ceilings each get their own |
| Not in this workstream | exporters (`md-exporter`, `json-exporter`), compare/redline/gate (`pdfextractor_ui`), `--progress`, cancellation, publishing the crate | each already has a home, so moving it would duplicate a library that exists; cancellation has no design to extract, and a phase for it would be an API invention rather than a move |

**Parity checks, every phase.** `cargo test -p pdfextractor-cli --test cli_test`;
`cargo test -p pdfextractor-cli --test documentation_test` (this section's crate count is
one of the numbers that test holds, so a new member and this sentence move together);
`cargo build -p pdfextractor-cli --no-default-features` (the `ocr` feature is forwarded
to the pipeline, which forwards it to the engine — a broken forward is only visible in
that build); `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo fmt
--all --check`.

**Order.** P2 first: it is the cheapest, and it is what makes the dependency cut real
rather than nominal. P3 and P4 are independent of each other and each is smaller than
two of the per-file phases a horizontal plan would have produced, which is why the
workstream is four phases and not eight.

## Suite facts (held by `documentation_test`)

The workspace's minimum Rust is 1.92 (`rust-version` in `Cargo.toml`); the toolchain it is
built with is pinned at 1.99.0 in `rust-toolchain.toml`, and README states both.

The workspace builds with zero warnings, and the suite holds 1,360 tests on Linux (`cargo test --workspace -- --list`; Windows lists fewer, `job_test` being Unix-only).
There are 140 end-to-end tests in `cli_test` that run the built binary as a child process.
The golden binary `docling_golden_test` has forty-three tests in the binary: the corpus-wide conformance check, nineteen fixture checks, a version check, twenty comparison units and two acceptance tests.
58 real Docling conversions are committed under `test-data/golden/docling/`.

## Review round — 2026-11-22

A full read of the workspace after the security-handler rounds, with the command line
exercised as a user runs it. The defects found, and what was done:

| Finding | Severity | State |
|---|---|---|
| `-i notes.md --to md` (also `.txt`→`--to text`, `.html`→`--to html`) writes the converted document over the file it read | considered, not a defect | left as it is: in-place normalization is tested behaviour (six `cli_test` cases read the Markdown back after the run), the whole document is read before the first write, and every writer replaces its file through a temporary so a failed write leaves the source whole |
| `-o -` piped into `head` panicked (`failed printing to stdout: Broken pipe`) and exited 101 after the first screenful | crash on documented use | fixed — every stdout write goes through `write_stdout`, which stops quietly on `BrokenPipe`; `a_consumer_that_stops_reading_ends_the_stream_without_a_panic` |
| `-i report.pdf -o out` with `out` an existing folder wrote `out.json` beside the folder instead of `out/report.json` inside it | wrong output path | fixed — an existing folder, or a path ending in a separator, is a destination folder; `an_existing_output_folder_receives_the_document` |
| a symlink back to its own folder was followed until the kernel's 40-symlink limit: 41 conversions of one document and a nested copy of the output tree | duplicated work and litter | fixed — the walk remembers resolved directories; `a_symlink_cycle_is_walked_once` |
| the LibreOffice profile was handed over as `file://{path}`, a URI only when the path is a Unix absolute path — a Windows temporary directory with a space in it or a `#` anywhere produced one LibreOffice cannot open | bridge failure on Windows | fixed — `profile_url` writes forward slashes and percent-encodes; `a_profile_path_is_a_file_uri` |
| `-o -` left extracted pictures beside the input without saying so | surprising side effect | fixed — a stderr note names the folder when the document has pictures it wrote (never in `embedded` mode, whose pictures travel in the stream), and the README's `-o` row says it; a new **Exit status** note documents the budget refusal's `3` |
| `test-data/xref_encrypt_no_type.pdf` is untracked, so a fresh checkout's `crypt_test` fails; `tracked_fixtures_test` walks only the golden directories, so it cannot notice | fixture missing from the repository | fixed — committed with the `/Encrypt` round and its `scripts/fixture-owners.tsv` row; the runner's self-test rejects a row for a file that is not in the index, which is how this was found |
| the encrypted fixtures had no `scripts/fixture-owners.tsv` rows, so `scripts/test.sh --path` fell back to the full suite | slow selection | fixed |
| `./scripts/test.sh --self-test` failed on the committed tree: `test-data/make_encrypted_unicode.sh` is a generator, but the coverage audit exempted only `*.py` generators | gate red on a clean checkout | fixed — the rule is the `make_*` name, and the synthetic coverage case pins a shell generator |
| `plan.md` said the branch was `origin/main` plus four commits; it is eight | stale documentation | fixed |

No reading changed in this round. The README's ⚠️ rows (threaded XLSX comments, PPTX
SmartArt and equations, ODT headers and footers, `.doc` character formatting without
LibreOffice) and the *Deferred* list below remain the open capability work.

## Dependency and gate round — 2026-11-23

The dependencies, the toolchain, and the two checks that run on a schedule rather
than on a push.

| Change | Why |
|---|---|
| `quick-xml` 0.36.2 → 0.42.0, in the workspace and in `fuzz/` | the version `plist`/`tauri` already resolved, so the tree keeps one copy. The reader moved: `QName` is converted at the byte boundary, `normalized_value(XmlVersion::Implicit1_0)` replaces the deprecated `unescape_value`, text and CDATA arrive raw, and a reference is its own `Event::GeneralRef` (handled at every reader through the shared `limits::push_reference`). |
| Text and CDATA read with `xml_content(XmlVersion::Implicit1_0)` at 17 sites | restores XML 1.0's EOL normalization (CRLF and a lone CR are one LF), which reading the event bytes directly had dropped. |
| `test-data/crlf_text.docx`, `test-data/make_crlf_text_docx.py`, and the docx test `crlf_in_text_and_cdata_is_normalized_to_lf` | pins it: CRLF, a bare CR and an LF inside a `<w:t>` and inside a CDATA section. The test fails with the `as_ref()` reading (checked by reverting it), so a later simplification cannot pass silently. |
| `unsafe_code = "forbid"` in `[workspace.lints.rust]`, with `[lints] workspace = true` in every member (24 then, 25 once `crates/pipeline` joined) | a new parser cannot bring `unsafe` in quietly; no dependency was added. |
| `rust-toolchain.toml` pins 1.99.0 (clippy, rustfmt); README states the 1.92 minimum | one rustc for the suite, CI and a release. Every job that runs `cargo` installs the pin in a step of its own — `dtolnay/rust-toolchain`, since its automatic install is deprecated — rather than leaving rustup to fetch it. The gates name 1.99.0 on the action's `toolchain` input; fmt and both release legs read the channel out of `rust-toolchain.toml` and pass it to the action, so a bumped pin cannot leave the formatter or a release on the old toolchain while the file stays the one place the version is written. Fuzz stays on nightly through `cargo +nightly`, which never resolves the pin. |
| `.github/workflows/deny.yml` and `deny.toml` | weekly (Mondays, 05:23 UTC) and by hand: `cargo deny check advisories bans licenses` over the `all-features` graph, with the advisory database cached. |
| `.github/workflows/fuzz.yml` | weekly (Mondays, 04:43 UTC) and by hand: seven targets, a `seconds` bound of 1–600, the corpus seeded and cached, findings uploaded for 30 days. The 110-minute job timeout covers 7 × 600 s plus the build, the seeding and the upload, and a failing target no longer skips the rest: the step runs every target, then fails. README's CI prose and `fuzz/README.md` state these figures, including the 30-day retention. |
| `zip` 2.4.2 → 9.8.2 (`default-features = false, features = ["deflate"]`) and `cookie` 0.18.2 → 0.18.3 | the eight advisories the database lists for `zip` are all in paths this workspace never reaches — `extract()`, `ZipStreamReader`, symlink targets, WinZip AES, the legacy Shrink/Reduce/Implode decoders — and 9.8.2 is above every patched floor they state; upgrading clears them without an ignore in `deny.toml`, and the deflate-only build leaves those paths out of the binary at all. `ZipFile::name()` is fallible now, so an undecodable entry name skips that entry (the package gate reports it as `LimitError::Zip`). `cookie` 0.18.3 clears the advisory the first gate run reported for it. |

`deny.toml` lists no vulnerability: its only ignore remains the unmaintained
`chrono` (`RUSTSEC-2026-0543`), and the check ends `advisories ok, bans ok,
licenses ok`.

Checks run this round: `cargo clippy --workspace --all-targets --locked -- -D
warnings` and `cargo fmt --all --check` clean; the limits, chart, docx, xlsx,
pptx, odp, ods, odt and metafile suites green; `cli_test` 137 passed;
`documentation_test` green with the count at 1,320; `cargo deny check advisories
bans licenses` green. The full workspace suite was not re-run, and no workflow was
dispatched.

## Outline heading round — 2026-11-24

- **Measured first.** Across the nine corpus PDFs, six carry outlines: 89 entries.
  67 already agreed with a `section_header` (62 on the same page, 66 at the
  outline's depth); 15 more sat inside a paragraph block the font heuristic had
  not split; 7 were not in the extracted text at all. The unnamed destinations
  were the reason the first implementation saw nothing on five of the six: the
  parser now reads `/Dests` and `/Names /Dests`, not only inline page arrays.
- **Implementation.** `PdfDocument::outline()` reads titles, nesting and direct
  or named destinations; layout takes per-page `HeadingHint`s, matches them on
  letters and digits at a line boundary, splits a heading off a merged paragraph
  and sets its level from the bookmark depth. The old heuristics remain for
  documents with no outline.
- **On the corpus.** Headings 158 -> 167: seq2seq's appendix `A.1`-`B.2`
  recovered at levels 2/3, and one batch-normalization heading no longer carries
  its following body line. No heading changed on the four outline-less PDFs.
  The corpus ratchet recorded the two resulting deltas as accepted, with the
  argument in `compare-baseline.json`'s `note` and in the round note below.
- **Pinned.** `test-data/outline_headings.pdf` (generated by
  `make_outline_headings.py`) has four 10 pt headings that only the outline
  names, nested one level; `test-data/golden/markdown/outline_headings.md` pins
  the `##`/`###` hierarchy.

## OCR engine reuse round — 2026-11-24

- **Benchmark first.** A small std-clock bench (`engine_cost`, no framework) on a
  real scan measured engine construction at **173–176 ms** against **~460–475 ms**
  for one page (20 iterations, release, Linux x86-64). A document that built its
  own engine therefore spent about a quarter of its OCR time on construction.
- **Reuse.** A thread-local engine per Rayon worker now outlives one document and
  is lent to the next; language and page-segmentation mode are part of the slot,
  so a changed setting rebuilds it. A non-OCR document does not start Tesseract.
- **After.** Ten documents through one engine vs ten fresh builds: **667 ms ->
  468 ms median per document (-29.8%)** in the final bench run; earlier runs of
  the same bench measured 593 -> 503 ms and 606 -> 485 ms, so the range is about
  -15% to -30% on this machine. End-to-end debug batch of 16 scans on 4 workers:
  **~37.0 s -> ~35.9 s** (two runs each), with byte-identical output.

## Pipeline extraction round — 2026-11-24

- **Started with `build_document`.** The conversion code now lives in
  `pdfextractor-pipeline`: `build_document`/`build_document_with_format` and the
  readers they dispatch to, OCR engine cache, picture extraction, preview
  writer, budgets and `relabel`. The crate takes `ConvertOptions` instead of
  clap's `Args`, so a caller converts a file without spawning the binary.
- **What stayed.** Argument parsing, input walking, output writers, compare and
  redline, the extension gate and its user-facing refusals. That gate calls the
  library only after the name is accepted, so its messages are unchanged.
- **Parity.** `crates/cli/tests/cli_test.rs` is untouched and runs against the
  binary built on the pipeline. The conversion unit tests moved beside the code
  to the new crate; the CLI keeps its extension and limit-error tests.
- **What remains.** WS8 above holds the phases still to land (caller-chosen
  picture and preview location, typed boundary and per-conversion limits) and the
  checks each one ends on.

## Blocked — needs a decision only the owner can make

- **Pushing the branch.** `main` is `origin/main` plus twenty local commits, from
  `f593eac` (budget flags and the derived-limits fuzz target), `1fdb79d` (the merge)
  and `b59e0e3` (the review's hardening) through the security and budget round, the
  three handler rounds, the `/Encrypt` rounds and the review, dependency, outline,
  OCR and pipeline rounds recorded above — and nothing here pushes without the owner
  asking.
- **Code signing** of the installers: needs signing identities that are not in this
  repository.
- **macOS testing**: no Mac here; the release workflow's macOS job is the only check.
- **Releasing** (tag, push, publish): the owner pushes after review.

## Deferred — recorded, not in this plan

Each is real work that is larger than a finishing pass, or needs measurement on the
Linux OCR floors, or ML; the old plan's text for each is in
[docs/history/plan-2026-11-19.md](docs/history/plan-2026-11-19.md).

- OCR: deskew/despeckle preprocessing; full-page OCR
  of vector-only pages (rasteriser); non-English and fax fixtures.
- PDF: table cells and spans; heuristic formula regions; a rotation angle on
  boxes; captions as furniture.
- Formats: EPUB and e-mail; `.doc` character formatting without LibreOffice; DOCX
  page geometry; slide transitions.
- GPU (Phase 9), a custom OCR engine, a service mode, model-based layout.
- Benchmarks and performance tests with varied sizes and parallelism.
