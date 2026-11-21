# Progress — monitor for [plan.md](plan.md)

*Started 2026-11-19 on branch `finish-plan-2026-11` (cut from `origin/main` at
`e8e8267`). The history before this plan — every verified-status row, the bug list,
the known limitations — is [docs/history/progress-2026-11-19.md](docs/history/progress-2026-11-19.md).*

## Workstreams

| WS | What | Status | Evidence |
|---|---|---|---|
| WS1 | Baseline on the right history | done | `finish-plan-2026-11` from `origin/main` (`e8e8267`); local `main` untouched |
| WS2 | Native Windows suite green (`--no-default-features`) | done | `cargo test --workspace --no-default-features --no-fail-fast`: **1,063 passed, 0 failed, 14 ignored** (the OCR tests, ignored without the `ocr` feature); `cargo clippy --workspace --all-targets --locked --no-default-features -- -D warnings` clean; `cargo fmt --all --check` clean |
| WS3 | CI-parity Linux run | done (re-run after the 2026-11-20 merge: 1,282 passed) | `ubuntu:24.04` container, rustc 1.98.1, the `test.yml` packages: `cargo test --workspace --locked` **1,105 passed, 0 failed**; clippy `-D warnings` clean with and without default features; `cargo fmt --all --check` clean; `--list` counts **1,105**; compare gate (`--compare test-data/golden/docling`, `--ocr never`) exit 0, every document at or above the ratchet, 4 `.doc` not held (no LibreOffice) |
| WS4 | Encrypted PDFs: `--pdf-password` | done | `crypt_test` 5/5, `crypt` unit tests 4/4, CLI `encrypted_test` 6/6 (empty password, user and owner passwords, env var, missing and wrong password refused with nothing written); fixtures read back by Poppler `pdftotext -upw user` / `-opw owner` and refused with no password |
| WS5 | Damaged PDFs and the `/Root` fix | done | `repair_test` 5/5: lying `startxref`, no xref/trailer, every offset shifted, truncation at 10/50/90%, an incremental update adding a new catalog read 8× with the same answer |
| WS6 | The window's password | done | `the_pdf_password_travels_in_the_environment_only`, `no_password_adds_nothing_to_the_environment`, `the_page_sends_the_password_as_pdf_password` pass; field in `dist/index.html`, sent as `PDFE_PDF_PASSWORD` through `Job::spawn_with_env` |
| WS7 | Documentation reset | done | plan.md, progress.md, README rows; `documentation_test` passes on both platforms (counts 1,105 / 128 / 35 / 49) |

All workstreams finished. What is left is listed in plan.md under *Blocked* (owner decisions:
local `main`, signing, macOS, releasing) and *Deferred*.

## Log

### 2026-11-19

- Found this clone's `main` (`e6c0520`) to be the pre-rewrite history: it shares no
  commit with `origin/main` (`e8e8267`, 62 commits). Work moved to
  `finish-plan-2026-11` from `origin/main`; local `main` untouched.
- Old `plan.md` and `progress.md` moved to `docs/history/`.
- CLI linked Tesseract even with `--no-default-features` (README said otherwise): the
  CLI now has an `ocr` feature (default) gating `pdfextractor-ocr-engine/tesseract`.
- Added the standard security handler (`crates/pdf-parser/src/crypt.rs`), `--pdf-password`
  / `PDFE_PDF_PASSWORD`, three encrypted fixtures (`make_encrypted_variants.py`), the
  trailer-`/Root` fix, `repair_test`, and the window's password field.

### 2026-11-20

- **Found on the untouched `origin/main` in the Linux container** (baseline run): clippy
  failed (`type_complexity` in `crates/ods-parser`, then `large_enum_variant` in
  `md-exporter`, `enum_variant_names` in the CLI, a lint-only `drop` in `cli_test`);
  `cargo fmt --check` failed on comment alignment in three CLI test files; the plan's
  documented workspace total (1,080) was 4 short of the list (1,084). All fixed.
- Golden and stdout suites failed on Windows only because `core.autocrlf=true` turned the
  byte-exact goldens into CRLF: `.gitattributes` now checks text out as LF everywhere
  (`fdd4ea8`).
- OCR tests gated on the CLI's `ocr` feature; the documentation test lists the suite with
  the features it was built with (`2ff6af8`). Clippy/fmt clean and the count set to 1,105
  (`87636dd`).
- Final runs: native 1,063 passed / 14 ignored / 0 failed; Linux 1,105 passed / 0 failed;
  clippy ×2, fmt and the compare gate green.

### 2026-11-20 — merge of `origin/main`

- `origin/main` moved on 2026-11-19 by 50 owner commits (`e8e8267..397168a`: shared
  resource budgets and fuzz limits, decoded-stream reuse, CCITT G3/G4 and JBIG2 decoding,
  redline mode, lint clean-up, 178 files). Merged with `git merge` (`1ceaaba`), not a
  rebase, so the SHAs above stay valid. Local `main` still untouched.
- Conflicts and how they were resolved (the owner's side wherever both solved the same
  thing):
  - `crates/pdf-parser/src/parser.rs`: the owner's `Limits`, deadline, document total and
    decoded-stream cache kept whole; the password and `/Root` work joined through a new
    `parse_with_limits_and_password` (with `parse`, `parse_with_password` and
    `parse_with_limits` delegating to it). Decryption still runs after stream lengths are
    honoured and before object streams are expanded, so the owner's cache only ever holds
    plain bytes.
  - `crates/cli/src/main.rs`: the owner's `read_input` and run budgets, with our password
    and refusals in place of the old "cannot decrypt" bail.
  - `crates/ui/src/main.rs`: the owner's start path, spawning through
    `Job::spawn_with_env` so the password stays out of the argv.
  - `crates/ods-parser`, `crates/md-exporter/src/chunks.rs`, the golden test tables and
    `Cargo.lock`: the owner's versions (their own lint and fmt fixes); the lock regains our
    `md-5`/`sha2`/`aes` entries.
  - `crates/cli/tests/documentation_test.rs`: the owner's structured counts, plus our
    feature-aware listing and the non-Unix `dead_code` allowance.
  - `README.md`: our PDF text row (decryption, damaged files) beside the owner's new PDF
    pictures row (CCITT, JBIG2).
  - `plan.md`/`progress.md`: ours kept; the owner's latest versions archived as
    `docs/history/plan-2026-11-19.md` and `docs/history/progress-2026-11-19.md`. Suite
    facts updated to the merged suite: 1,282 / 130 / 43 (19 fixture checks) / 58.
- What the merge broke, and the fix: four new owner tests read text by OCR
  (`a_ccitt_or_jbig2_scan_is_read_end_to_end` and three animated-frame page-range tests)
  and failed in a build without Tesseract; gated on the `ocr` feature like the others
  (`f181351`).
- Verification after the merge:
  - Linux (`ubuntu:24.04`, rustc 1.98.1, the `test.yml` packages, a venv with
    `docling-core==2.101.1` as `DOCLING_PY`, like CI): `cargo test --workspace --locked`
    **1,282 passed, 0 failed**; `--list` **1,282**; clippy `-D warnings` clean with and
    without default features; `cargo fmt --all --check` clean; compare gate exit 0, every
    document at or above the ratchet (4 `.doc` not held, no LibreOffice).
  - Windows (`--no-default-features`, no Tesseract): workspace run 1,220 passed with only
    the 4 OCR tests above and `docling_core_accepts_our_documents` (no `DOCLING_PY` set)
    failing; after the gating, `cargo test -p pdfextractor-cli --no-default-features`
    with `DOCLING_PY` pointing at the local Python: **325 passed, 0 failed, 18 ignored**.
    Native clippy (`--no-default-features`, `-D warnings`) and `cargo fmt --check` clean.

### 2026-11-21 — merge 397168a..0642d2d and the review round

- `origin/main` moved by nine commits: the standard security handler and
  `--pdf-password`, the trailer-`/Root` repair and its measurements, the window's
  password field, the plan rewrite and the lint/fmt/OCR-feature fixes. Merged as
  `1fdb79d` on top of the budget-flag work (`f593eac`). The only conflict was
  `plan.md`; the owner's rewrite was taken whole (the old plan stays archived in
  `docs/history/`).
- Suite facts after the merge: the workspace lists **1,285** tests — 1,282 from the
  merged tree, the two budget tests from `f593eac`, and one new crypt unit test.
  `plan.md` states 1,285 and `documentation_test` is green.
- Full workspace run (`DOCLING_SKIP_ORACLE=1 cargo test --workspace --locked
  --no-fail-fast`): every target passed except **one** UI test,
  `a_refresh_re_resolves_the_cli_named_by_the_environment`, which met `ETXTBSY`
  (“Text file busy”) executing its stand-in CLI while another test thread forked.
  `ask_input_formats` now retries that single transient (20 × 5 ms) and returns any
  other error at once; `cargo test -p pdfextractor-ui --lib` passes 5/5 consecutive
  runs after the fix (121 tests each).
- Crypt review: RC4-40/128, AES-128 and AES-256 (revisions 2–6) key derivation,
  padding, the `/Encrypt`-dictionary and object-stream exemptions, and the
  trailer-`/Root` choice hold up; `crypt_test` 5/5 and the CLI `encrypted_test` 6/6
  pass against the Poppler-verified fixtures. One gap fixed: a stream whose own
  `/Filter /Crypt` names a filter other than `/Identity` was decrypted with `/StmF`;
  it now uses that `/CF` entry's method
  (`crypt::tests::a_stream_naming_its_own_crypt_filter_uses_that_filter`).
- The EXIF turn is budgeted too: `orient_for_ocr` now calls
  `image_parser::upright_png_checked` with the run's `Limits`, so
  `--max-image-pixels`/`--max-image-dimension`/`--max-image-alloc-bytes` govern it
  exactly as they govern the detected quarter turn
  (`a_one_pixel_ceiling_refuses_the_exif_turn`). Suite count moved to **1,286**.
- Left as a follow-up: the OCR photo corpus costs about three minutes a page in a
  debug build, which dominates a local workspace run.
- A budget refusal from the LibreOffice bridge is now the document's failure, not
  a reason to read it another way: the `Format::Doc` arm re-raises any error whose
  chain holds a `LimitError`, so `--bridge-timeout 0` and
  `--max-bridge-output-bytes 1` exit 3 and name the flag instead of converting
  through the built-in reader. A missing or unusable LibreOffice still falls back
  (checked by hand with `PATH=/nonexistent`, which converts `legacy_brief.doc`
  through the built-in reader, exit 0). New test
  `a_bridge_budget_refusal_fails_the_legacy_document`; suite counts 1,287 / 131.
