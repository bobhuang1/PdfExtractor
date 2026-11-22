# Plan: finish, test and fix PdfExtractor

*Rewritten 2026-11-19. The plan this replaces — the full roadmap with Phases 1–11 and
workstreams W1–W7, every ticked box and its evidence — is kept unchanged in
[docs/history/plan-2026-11-19.md](docs/history/plan-2026-11-19.md); the 13,700-line
progress log it pointed at is [docs/history/progress-2026-11-19.md](docs/history/progress-2026-11-19.md).
The monitor for this plan is [progress.md](progress.md).*

## Where the project stands

A Rust workspace (23 crates) that converts PDF, DOCX, legacy `.doc`, ODT/ODS/ODP,
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
rustc 1.98.1: `cargo test --workspace --locked` (OCR on, window built),
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
CLI: `--pdf-password` (or `PDFE_PDF_PASSWORD`); an empty user password opens
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

## Suite facts (held by `documentation_test`)

The workspace builds with zero warnings, and the suite holds 1,297 tests on Linux (`cargo test --workspace -- --list`; Windows lists fewer, `job_test` being Unix-only).
There are 133 end-to-end tests in `cli_test` that run the built binary as a child process.
The golden binary `docling_golden_test` has forty-three tests in the binary: the corpus-wide conformance check, nineteen fixture checks, a version check, twenty comparison units and two acceptance tests.
58 real Docling conversions are committed under `test-data/golden/docling/`.

## Blocked — needs a decision only the owner can make

- **Pushing the branch.** `finish-plan-2026-11` is `origin/main` plus four
  local commits — `f593eac` (budget flags and the derived-limits fuzz target),
  `1fdb79d` (the merge), `b59e0e3` (the review's hardening) and the security
  and budget review round — and nothing here pushes without the owner asking.
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
- PDF: outlines as a heading signal; table cells and spans; heuristic formula
  regions; a rotation angle on boxes; captions as furniture.
- Formats: EPUB and e-mail; `.doc` character formatting without LibreOffice; DOCX
  page geometry; slide transitions.
- GPU (Phase 9), a custom OCR engine, a service mode, model-based layout.
- Benchmarks and performance tests with varied sizes and parallelism.
