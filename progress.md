# Progress — monitor for [plan.md](plan.md)

*Started 2026-11-19 on branch `finish-plan-2026-11` (cut from `origin/main` at
`e8e8267`). The history before this plan — every verified-status row, the bug list,
the known limitations — is [docs/history/progress-2026-11-09.md](docs/history/progress-2026-11-09.md).*

## Workstreams

| WS | What | Status | Evidence |
|---|---|---|---|
| WS1 | Baseline on the right history | done | branch from `origin/main`; native `--no-default-features --no-run` builds (after the CLI `ocr` feature fix) |
| WS2 | Native Windows suite green (`--no-default-features`) | in progress | first run: OCR tests fail without Tesseract (to gate on the `ocr` feature); golden/stdout tests fail on CRLF from `core.autocrlf=true` checkout (needs `.gitattributes eol=lf` or a decision) |
| WS3 | CI-parity Linux run (test, clippy ×2, fmt, compare gate) | in progress | image `pdfx-ci:local` built; baseline run on clean `origin/main` started in container `pdfx-ci-base` (logs in session scratchpad); branch run not yet done |
| WS4 | Encrypted PDFs: `--pdf-password` | code done, partly verified | `encrypted_test` 6/6 pass natively; fixtures read back by Poppler `pdftotext -upw/-opw`; `crypt_test` not yet run |
| WS5 | Damaged PDFs and the `/Root` fix | code done, unverified | `repair_test` written, not yet run |
| WS6 | The window's password (environment, never argv) | code done, unverified | three unit tests added in `crates/ui/src/lib.rs`, not yet run |
| WS7 | Documentation reset | in progress | plan/progress rewritten, README updated; suite count `1,099` in plan.md unverified on Linux |

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
- Stopped here on the usage limit: the remaining steps are in the table above.
