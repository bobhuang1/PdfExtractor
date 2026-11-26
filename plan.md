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
| P3 — a caller-chosen picture and preview location (landed; the round note in `progress.md` records it) | `image_base: Option<&Path>` and `preview: Option<&Path>` on `ConvertOptions`; `output_path` stops being a required argument to the reading; `image_dir_for` moves back to the CLI as its naming helper | `cli_test`'s picture and `--preview` cases unchanged; the `-o -` note that names the folder still prints the same path; `md-exporter`'s existing `image_base` is the only base an export resolves a relative URI against |
| P4 — a typed boundary and per-conversion limits (landed; the round note in `progress.md` records it) | `PipelineError` at the two public entry points (`Limit(LimitError)`, `Encrypted`, `UnsupportedFormat`, `PageRange`, `OcrLangEmpty`, `Io`, `Other(anyhow::Error)`), with `anyhow` left inside the readers; `limits: Limits` on `ConvertOptions` in place of the `OnceLock` and `set_limits`; the flag-named refusals (`--ocr-lang`, `--psm`, `--page-range`) formatted in the CLI | the CLI's exit status `3` for a budget refusal still comes from matching the typed case; `is_limit_refusal` has one definition, in the CLI, and the public one is gone; two documents converted in one process with different ceilings each get their own |
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

The workspace builds with zero warnings, and the suite holds 1,377 tests on Linux (`cargo test --workspace -- --list`; Windows lists fewer, `job_test` being Unix-only).
A full-suite run also needs the `docling-core` oracle. `docling_core_accepts_our_documents`
fails rather than passing silently when no interpreter loads it: point `DOCLING_PY` at a
Python with `docling-core` installed (`~/.venvs/pdfextractor-docling/bin/python` on the
machine this was developed on), or opt out in so many words with `DOCLING_SKIP_ORACLE=1`
for a run that checks the schema alone. `scripts/test.sh --full` uses that venv when the
interpreter exists, and otherwise stops before compiling with the same two options; it
never opts out on the caller's behalf, so a gate that stops here reads as setup and not
as a regression.

There are 144 end-to-end tests in `cli_test` that run the built binary as a child process.
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
- **What remains.** Nothing: P4 (the typed boundary and per-conversion limits)
  landed on 2026-11-25 with the checks that phase ends on, so all four WS8 phases
  are in.

## Review round — 2026-11-25: efficiency and security

A read of every place an untrusted file meets the process: the budget crate, the
archive and XML gates, the PDF security handler, every program the tool starts,
every file it opens or writes on a document's behalf, and both exporters' HTML.

**What held.** Every spawn takes an argv vector and never a shell (the only
`sh -c` calls are in tests); `pdftoppm` and `soffice` are found at fixed paths
rather than through `PATH`; the PDF password travels in the environment and is
removed from every child; the ZIP gate checks declared size, expansion ratio,
package total *and* the bytes actually read; XML `DOCTYPE`s are refused and its
depth bounded; `open_under` walks a document's directory with `O_NOFOLLOW` from
a descriptor it already holds; each writer replaces its file through an
exclusively created sibling; every HTML text and attribute value goes through
`escape`, embedded `data:` URIs included; and `unsafe_code = "forbid"` covers
every member.

| # | Finding | Kind | State |
|---|---|---|---|
| F1 | `image_data_uri` read the document's `uri` verbatim when the caller named no `image_base`: an absolute path, or one climbing out with `..`, was read and embedded as base64. The read with a base refuses both, so the two paths disagreed — and only the based one was tested. | security: a document could name a local file and have its bytes carried into the output | **fixed** — `read_named` makes the same two refusals without a base; `a_picture_without_a_base_still_cannot_name_a_file_outside` |
| F2 | `Preview::to_html` scanned every box once per page, and counted the five kinds in five separate passes. | efficiency: O(pages × boxes), on the path (`--preview`) whose product is the whole overlay | **fixed** — one grouping pass and one counting pass, measured below |
| F3 | `render_pages` matched each rendered PNG to its page with a linear search. | efficiency: O(files × pages) | **fixed** — one page-number map |
| F4 | `Limits::check_zip` read every non-XML entry into a fresh `Vec` only to count it, so the gate held a package's largest picture in memory for nothing. | efficiency: peak memory | **fixed** — non-XML entries are counted through one reused 64 KiB buffer, the actual-byte guarantee unchanged (`a_streamed_count_stops_one_byte_past_the_ceiling`) |
| F5 | `Package::read_name` is a linear scan per part. | efficiency | **fixed, in the half that mattered** — a present name was already a hash after the `zip` 9.8.2 upgrade, but a *miss* fell back to decoding every name: 10,000 absent lookups over 10,000 entries took 19.0 s. `Package` settles that fallback in one walk, and only for a package that stores a name which does not decode to its own bytes; the same misses take 29 ms, flat against a 100-entry package. `a_cp437_name_is_found_by_the_text_it_decodes_to` pins the decode path. |
| F6 | The gate decompresses every entry and the reader decompresses it again. | efficiency | **by design**, measured: the gate must read every entry itself — that is what makes a lying central directory harmless — and the reader's own read is what charges its budget. On a 1.3 MB corpus package the gate pass took 246–290 ms against 199–208 ms for one full reader pass. |

**Measured (F2).** The new `preview_cost` bench (ignored by default; 2,000 pages
× 20 boxes = 40,000 boxes) on this machine, the HTML byte-identical both ways
(18,025,839 bytes): per-page scan **6.18 s**, grouped **1.07 s** — **−83%**.

**Measured (F5, F6).** A present part was already a hash (`zip` 9.8.2 indexes the
central directory): 1,000 reads took 45 ms on both a 100-entry and a 10,000-entry
package. A *miss* was not — the CP437 fallback decodes every name — and readers
ask about parts a package may not have, so 10,000 absent lookups took **19.0 s**
over 10,000 entries (167 ms over 100). Settling the fallback once: **29 ms**, and
13 ms over 100 entries. On `test-data/real/calibre_demo.docx` (1,311,881 bytes,
2,571,770 expanded across 32 parts) the gate's pass took **246–290 ms** against
**199–208 ms** for one full reader pass; that gap is F6, and it stays because it
is the guarantee.

**Plan and outcome.** R1 the no-base image read (fix, test); R2 the overlay's
grouping and the renderer's page map (fix, bench, an ordering test that the
grouping is an index rather than a sort); R3 the streaming count in the ZIP gate
(fix, unit test); R4 the round's parity: `cli_test` 140 unchanged,
`documentation_test` green with the suite at 1,364, the limits, exporter and
pipeline unit suites green, and workspace clippy `-D warnings` with `cargo fmt
--all --check` clean.

## Blocked — needs a decision only the owner can make

- **Pushing the branch.** The rounds recorded above are on `origin/main`
  (`ad82c90`), pushed on the owner's ask; `main` is that plus one local commit
  (the docling gate), and nothing here pushes without the owner asking.
- **Code signing** of the installers: needs signing identities that are not in this
  repository.
- **macOS testing**: no Mac here; the release workflow's macOS job is the only check.
- **Releasing** (tag, push, publish): the owner pushes after review.

## Next workstreams — started 2026-11-26

The *Deferred* list below is the previous plan's record. Each item there is a workstream
of its own; this section orders them and states the first result, so the next round starts
from a measurement rather than from scratch.

| # | Item | First phase | Exit check |
|---|---|---|---|
| 1 | OCR preprocessing | **landed**: despeckle (`--ocr-despeckle`) and deskew (`--ocr-deskew`), with the measurements below | `cargo test -p pdfextractor-ocr-engine --lib preprocess::` |
| 2 | Vector-page rasterising | **landed** (`--ocr-vector-pages`), with the renderer rule recorded below | `a_page_whose_text_is_vector_art_is_rasterised_and_read` |
| 3 | Non-English and fax fixtures | **the fax half landed** (`fax_scan.png`, from `make_fax_scan.py`); the German scan is open | `despeckle_recovers_words_a_faxed_scan_loses`; the German words read |
| 4 | PDF tables and spans | detect ruled tables in layout and emit `Table` items with spans | a pinned fixture's grid matches and no corpus document's agreement falls |
| 5 | Formula regions | heuristic boxes over maths-like lines | the corpus equation counts do not move without an accepted delta |
| 6 | EPUB and e-mail | an `epub-parser` crate (container → OPF → spine → the existing HTML reader), then `.eml` | a generated fixture converts with its headings and pictures |
| 7 | Benchmarks with varied sizes | benches over small, medium and large documents, serial and parallel | the numbers are recorded in a round note |
| — | GPU, service mode | **not started**: no GPU on this machine or in CI, and a service mode needs a design decision (protocol, lifetime, authentication) before any code | — |

**1 — despeckle.** `--ocr-despeckle` replaces the
pixels no neighbour agrees with — a dark speck on paper, a light one inside ink — before
recognition, when the caller asks for it. It keeps the page's grid, so the boxes scale
onto the page exactly as they do without it; a page without specks comes through
unchanged, and a photograph's grain fails the same test, so a photograph is not smoothed.

**Deskew, and the measurement that unblocked it.** `--ocr-deskew` turns a page flat
before recognition. Two earlier estimators were measured on rendered fixtures and
rejected. The profile that replaced them was first written with its shear on the wrong
axis — ink binned along `x - t*y`, which straightens upright strokes — so it answered
zero to a page whose text descends four degrees. Binning along the direction the text
runs (`y - t*x`) reads the tilt: on the real scan turned 2, 4 and 6 degrees and back it
returned exactly those angles, and zero on the upright page. The estimate is acted on
only from half a degree up, so a straight page is never resampled; the turn is about the
centre on the same grid, and the boxes are mapped back through it. `invoice_scan_skewed.png`
(`make_skewed_scan.py`: the scan shrunk to 85 percent inside its frame, then turned ten
degrees about the centre) holds it: turned inside its margin the page loses nothing, and
as stored every line comes back wrong (`ACME SUPPLigg`, `IMvoice 12345`, `Total Due 42 50`,
`TPMK You for your business,`) where the turn reads all four.

**3 — the fax fixture measures the pass.** `fax_scan.png` is the scan with two percent of
its pixels replaced — salt on paper, pepper inside the ink — placed by a fixed generator so
the file is reproducible (`test-data/make_fax_scan.py`). Read as stored, the specks split
the glyphs and Tesseract returns `invoice, 12345` and `Tétal:Duie 42.50`; read with
`--ocr-despeckle` it returns `Invoice 12345` and `Total Due 42.50`, so the two lines the
noise costs are the two the pass recovers. The page is 297.6 x 168 points and one page
either way, and the boxes stay inside it: the pass filters pixels and does not move the
grid. The recovery is partial in the last line (`THiank you for your business.`), which is
what the test asserts instead of claiming the pass restores the scan.

**2 — the rasteriser, and how extraction may use a renderer.** `vector_text.pdf` is the
invoice laid out as text and flattened by Ghostscript with `-dNoOutputFonts`: no font, no
text-showing operator, no image, only filled paths, and `pdftotext` finds nothing on it. A
page like that had no text to read and no image to recognise, so `--ocr-vector-pages`
renders it and recognises the render, and the four lines come back on the page's own 612 x
792 grid.

The documented rule is that the extractor never depends on an external renderer. It holds
because the renderer is *asked for*, never assumed: with the flag off, no lookup and no
process spawn happen, and every document is still read from its own bytes; the preview's
best-effort backdrop is the only other caller. With the flag on and no `pdftoppm`, the
conversion stops at the page that needed it, naming the page and the install — a caller
who asked and cannot be served is told, not handed a page with its text missing. The
lookup itself moved to `crates/pipeline/src/renderer.rs` so both callers share one rule:
fixed install paths or `PDFE_PDFTOPPM`, never `PATH`.

One page per invocation, drawn as displayed, so the page's `/Rotate` is applied once and
the recognised boxes are scaled from the render's grid back to points. The render is at
150 dpi: 150, 200 and 300 each read all four fixture lines, so the cheapest grid that was
measured is the one taken.

## Deferred — recorded, not in this plan

Each is real work that is larger than a finishing pass, or needs measurement on the
Linux OCR floors, or ML; the old plan's text for each is in
[docs/history/plan-2026-11-19.md](docs/history/plan-2026-11-19.md).

- OCR: the German-scan fixture. Despeckle, deskew and the vector-page rasteriser have
  landed, and what is left is sequenced in *Next workstreams* above.
- PDF: table cells and spans; heuristic formula regions; a rotation angle on
  boxes; captions as furniture.
- Formats: EPUB and e-mail; `.doc` character formatting without LibreOffice; DOCX
  page geometry; slide transitions.
- GPU (Phase 9), a custom OCR engine, a service mode, model-based layout.
- Benchmarks and performance tests with varied sizes and parallelism.
