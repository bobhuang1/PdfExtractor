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
  The two tests that write and run a stand-in CLI now retry that transient
  themselves (`answered_while_busy`), because only a test writes the executable it
  runs; production keeps a single `exec`. `cargo test -p pdfextractor-ui --lib`
  passes 8/8 consecutive runs (121 tests each).
- Review pass on the two fixes: the `/CFM`→method mapping is now one helper
  (`method_of`); an absent `/CFM` is the identity, as the specification says (it
  used to fall back to `/StmF`); a stream naming a filter `/CF` does not define
  still uses `/StmF`, now with that choice written down; and the `ETXTBSY` retry
  moved out of production into the tests that create the race. The EXIF turn's
  library contract is pinned where it lives too:
  `the_exif_turn_is_held_to_the_callers_budget_and_returns_upright_pixels` in
  `crates/image-parser/tests/limits_test.rs` checks the typed refusal and the
  swapped axes, not only the CLI wrapper. Suite count 1,294.
- Security and efficiency review of the whole workspace (no `unsafe` anywhere;
  the PDF parser, budgets, bridge and exporters were read end to end). Fixed
  now: the PDF password is removed from the CLI's environment after its args are
  parsed and from the bridge child's environment explicitly, so LibreOffice no
  longer inherits `PDFE_PDF_PASSWORD`; bridge and redline scratch directories are
  freshly created, private (`0700`) and unpredictable instead of a fixed
  `pid`-named path that was removed and reused; both exporters create their
  sibling temporary file with `O_EXCL` and retry a different name, so a planted
  symlink is never followed or truncated; embedded Markdown/HTML pictures are
  read only from inside the reading's own directory, so a document cannot embed
  an absolute path or one that climbs out with `..`; and JSON export streams the
  pretty form through a `BufWriter` instead of holding a second full copy of the
  document in memory. Flow readers were held to the same containment later: a
  page's own `<img src>` / `![](...)` path is read only from inside the
  document's directory, so an absolute path, a `..` climb or a symlink out
  names no picture. Decryption's plaintext is now charged to the
  whole-document budget before any of it is allocated (below). Left as
  follow-ups: `--pdf-password` still appears in argv (the environment remains
  the documented route), decryption still copies ciphertext before filters
  rather than charging those copies too, and `soffice` is still found by `PATH`
  (overridable with `PDFE_SOFFICE`).
- Crypt review: RC4-40/128, AES-128 and AES-256 (revisions 2–6) key derivation,
  padding, the `/Encrypt`-dictionary and object-stream exemptions, and the
  trailer-`/Root` choice hold up; `crypt_test` 5/5 at that point (6 with the
  decryption-budget test below) and the CLI `encrypted_test` 6/6 pass against
  the Poppler-verified fixtures. One gap fixed: a stream whose own
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
- The orientation check's decode is budgeted too: `OcrEngine::detect_upright`
  takes the run's `Limits` (there is no defaulting wrapper to call by accident),
  decodes the reduced detection copy through `image_parser::decode_checked`, and
  both
  CLI call sites pass `&budgets()`. A page the image budgets refuse yields that
  `LimitError` (the command line keeps the stored orientation and reads on, as it
  does for a refused page turn). New test
  `a_one_pixel_ceiling_refuses_the_orientation_decode`; suite count 1,288.
- Flow readers were held to the exporters' containment rule: a picture path
  written in an untrusted HTML or Markdown document (`local_image_path`) is
  resolved against the document's own directory and read only when the
  canonical file is still under it. An absolute path, a `..` climb or a symlink
  leading out names no picture instead of an arbitrary local file. New tests
  `an_absolute_picture_path_is_not_read_from_a_flow_page` and
  `a_picture_path_cannot_climb_out_of_the_documents_directory`; the existing
  in-directory picture tests still pass. Suite counts 1,296 / 133.
- Decryption is charged before it allocates. `SecurityHandler::plaintext_len`
  sums the payload of every stream the handler decrypts plus every string the
  file holds (stream dictionaries included); `PdfDocument::decrypt` charges
  that total to the whole-document budget, excluding the `/Encrypt` object's own
  strings, and returns on a refusal before a single plaintext buffer exists, so
  the parse reports the refusal before object streams are expanded. New test
  `a_lowered_document_budget_refuses_decryption` lowers the document ceiling to
  32 bytes against a fixture whose plaintext is 45, and confirms the default
  budget still opens it. Suite counts 1,297 / 133.
- Extraction refuses a locked file instead of reading its ciphertext.
  `PdfDocument::extract_checked` returns an error carrying the `CryptError`
  when `locked` is set, with a context naming the encryption, so a caller that
  did not check `locked` itself cannot get noise shaped like a document; the
  log of the un-fallible `extract` says the same and yields no pages. The
  command line keeps its own message, which names `--pdf-password`. New test
  `a_locked_document_is_refused_rather_than_read_as_ciphertext` opens each
  encrypted fixture with `user` (its page reads), then with `guess` and checks
  the typed refusal, the message and the empty facade. Suite counts 1,298 / 133,
  `crypt_test` 7/7.
- Streams are decrypted where they stand. `SecurityHandler::decrypt_object` no
  longer builds a fresh ciphertext-sized buffer per stream; RC4 XORs the payload
  in place, AES-CBC decrypts each 16-byte block in place and drops the IV and
  PKCS#7 padding by moving the plaintext down over them, and an Identity stream
  is left untouched rather than copied for nothing (a string's bytes are built
  once and decrypted where they lie). The payload's buffer was already charged
  to the document budget when the parser copied it out of the file, and
  decrypting in place allocates nothing more, so a file of many encrypted
  streams has no second ciphertext-sized copy to spend. New unit test
  `crypt::tests::a_stream_is_decrypted_where_it_stands` holds RC4, AES-128 and
  Identity streams to the same buffer address and the same bytes; the encrypted
  fixtures still read end to end. Suite counts 1,299 / 133, `crypt` unit tests
  7.
- Extraction fails closed on any document the security handler did not open,
  not only on a recorded `locked` reason: `extract_checked` refuses when
  `encrypted` is set and `opened_with` is `None`, carrying the `CryptError`
  when the parse recorded one and saying the pages are still ciphertext when it
  did not (a malformed `/Encrypt` whose handler never ran). A gap in the parser
  therefore cannot surface as a partial read. New test
  `extraction_fails_closed_when_no_handler_opened_the_file` marks a parsed
  plain file encrypted and checks the refusal and the empty facade; the
  wrong-password test from the previous round still covers the `locked` case.
  Suite counts 1,300 / 133, `crypt_test` 8/8.
- Decryption has a fuzz target and an aggregate test. `fuzz/fuzz_targets/crypt.rs`
  overwrites every stream payload in two committed fixtures with the input (the
  real `/Encrypt` authenticates, so `decrypt_object` runs on arbitrary
  ciphertext) and also parses a minimal PDF whose `/Encrypt` dictionary body is
  the input; every refusal must carry a `CryptError` or a `LimitError`, so an
  untyped failure is a finding. The deterministic half is
  `crypt_fuzz_test::every_mutation_is_read_or_refused_with_a_typed_error`: a
  seeded xorshift loop flips bytes in all four fixtures under three passwords
  and feeds 256 synthesized security dictionaries straight to
  `SecurityHandler::authenticate`, asserting the same property; a dictionary
  that does authenticate decrypts an arbitrary payload. The first local run
  found a real panic — a `/V 5` handler naming a legacy revision took the
  AES-256 key length into the 16-byte MD5 derivation and sliced past it — now a
  typed `Unsupported` refusal, pinned by
  `an_aes256_key_length_with_a_legacy_revision_is_refused`. Local runs:
  `cargo +nightly fuzz run crypt fuzz/corpus/crypt -- -runs=4000
  -max_total_time=45` → 302 executions, no crash (the corpus is seeded from
  `test-data/encrypted*.pdf`); no workflow was dispatched. Suite counts
  1,302 / 133, `crypt` unit tests 8.
- The document budget now counts the bytes the parser and the object-stream
  expansion actually copy. A stream payload's copy out of the file is charged
  to the whole-document total before it is made (a lowered ceiling now refuses
  the file at parse, encrypted or not); `honour_stream_lengths` reuses the
  payload's own buffer and charges only the   growth a longer `/Length` needs; and an object stream is decoded straight
  from the store rather than cloned, with its decoded contents charged before
  they are parsed. Decryption no longer charges the stream plaintext a second
  time — it decrypts in the buffer that copy already paid for — so
  `SecurityHandler::plaintext_len` counts only the string buffers decryption
  rebuilds, and the budget is one charge per buffer. New tests
  `a_stream_payload_is_charged_when_the_parser_copies_it_out_of_the_file` and
  `expanding_an_object_stream_is_charged_for_its_decoded_copy`;
  `a_lowered_document_budget_refuses_decryption` still passes, the 32-byte
  ceiling now refusing the fixture's 45-byte payload as it is copied. The
  refusal matrix (`limits_matrix_test`) and the decryption fuzz aggregate still
  pass. Suite counts 1,304 / 133.
- Encryption stated only in a cross-reference stream is detected without `/ID`.
  A PDF 1.5 file can put `/Encrypt` in an xref stream's own dictionary instead
  of a classic trailer; the parser required `/ID` in that dictionary before
  believing the entry, so a file whose only `/Encrypt` sat there without one
  parsed as if it were plain and its ciphertext was read as text. The entry now
  counts on its own — `/ID` is what a handler authenticates against, not what
  makes the bytes ciphertext — and the newest such stream wins, by file offset,
  as the newest classic trailer does. With no `/ID` the handler authenticates
  against an empty first element and refuses, so the document is refused with
  its `CryptError` instead of read. New fixture
  `test-data/xref_encrypt_no_id.pdf` and test
  `an_xref_stream_encrypt_without_an_id_is_refused_not_read`. Suite counts
  1,305 / 133, `crypt_test` 9/9.
