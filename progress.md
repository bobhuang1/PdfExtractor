# Progress — monitor for [plan.md](plan.md)

*Started 2026-11-19 on branch `finish-plan-2026-11` (cut from `origin/main` at
`e8e8267`). The history before this plan — every verified-status row, the bug list,
the known limitations — is [docs/history/progress-2026-11-09.md](docs/history/progress-2026-11-09.md).*

## Workstreams

| WS | What | Status | Evidence |
|---|---|---|---|
| WS1 | Baseline on the right history | done | `finish-plan-2026-11` from `origin/main` (`e8e8267`); local `main` untouched |
| WS2 | Native Windows suite green (`--no-default-features`) | done | `cargo test --workspace --no-default-features --no-fail-fast`: **1,063 passed, 0 failed, 14 ignored** (the OCR tests, ignored without the `ocr` feature); `cargo clippy --workspace --all-targets --locked --no-default-features -- -D warnings` clean; `cargo fmt --all --check` clean |
| WS3 | CI-parity Linux run | done | `ubuntu:24.04` container, rustc 1.98.1, the `test.yml` packages: `cargo test --workspace --locked` **1,105 passed, 0 failed**; clippy `-D warnings` clean with and without default features; `cargo fmt --all --check` clean; `--list` counts **1,105**; compare gate (`--compare test-data/golden/docling`, `--ocr never`) exit 0, every document at or above the ratchet, 4 `.doc` not held (no LibreOffice) |
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
