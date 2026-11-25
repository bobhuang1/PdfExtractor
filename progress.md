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
  follow-ups: `--pdf-password` appeared in argv (it is now refused; the
  environment and `--pdf-password-file` are the routes), the filters now read
  the stored payload instead of copying it before decoding, and `soffice` was
  found by `PATH` (now a fixed install path or `PDFE_SOFFICE`, never `PATH`).
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
- Decryption is charged before it allocates. The handler's plaintext length
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
  `SecurityHandler::rebuilt_string_bytes` counts only the string buffers
  decryption rebuilds, and the budget is one charge per buffer. The filter chain hands
  back the stored payload as a borrow when no filter could read it, so the
  fallback copy is charged before it is owned, not after. New tests
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
- Picture containment holds the read to the file it checked. Both places that
  read a picture named beside a document — the Markdown/HTML writers embedding
  it as a `data:` URI, and the command line reading a flow picture back — used
  to canonicalize the path, confirm it sat under the document's directory, and
  then open the same name again. A symlink swapped in between resolution and
  open escaped containment because only the earlier check was believed. Both now
  open inside the check: the canonical path is stat'ed (a path that has become a
  symlink is refused), the handle is opened, and its identity (`dev`/`ino`, or
  `volume serial`/`file index` on Windows) is compared with the file the
  canonical path named. `read_input` likewise reads the handle it measured
  rather than reopening the name. New unit tests
  `a_picture_is_embedded_only_from_the_readings_own_directory` and
  `a_handle_is_held_to_the_file_the_path_names` (md-exporter) and
  `a_flow_picture_is_opened_only_from_the_documents_own_directory` (cli).
  Suite counts 1,308 / 133.
- Non-ASCII passwords are written the way the file's revision writes them.
  `SecurityHandler::authenticate`, and with it `parse_with_password` and
  `parse_with_limits_and_password`, take the password as text (`&str`) rather
  than bytes: a revision 4 or older file stores it in PDFDocEncoding and a
  revision 6 file stores UTF-8 after SASLprep (RFC 4013 — B.1 dropped, C.1.2
  mapped to a space, then NFKC; the prohibition and bidi checks are not
  repeated, because refusing text no prepared password can hold and rejecting a
  wrong password are the same answer). Revision 5 stays UTF-8 as typed, and the
  127-byte truncation applies after preparation. A password the encoding cannot
  write is refused as a wrong password instead of being hashed as the terminal's
  UTF-8. New qpdf fixtures (11.9.0, `test-data/make_encrypted_unicode.sh`):
  `encrypted_pdfdoc.pdf` (revision 4, `Grüße`) and
  `encrypted_r6_saslprep.pdf` (revision 6, composed `päss`). New tests
  `a_pdfdocencoding_password_opens_a_revision_four_file` and
  `an_r6_password_is_prepared_with_saslprep` (parser), the CLI's
  `a_non_ascii_password_is_encoded_for_the_files_revision`, and unit tests for
  the encoding table and RFC 4013's own examples. Suite counts 1,313 / 133.
- A cross-reference stream's encryption is read from its shape as well as its
  type. Detection keyed on `/Type /XRef`, so a PDF 1.5 file whose `/Encrypt`
  sat in an xref stream dictionary without the type — a repaired or
  hand-written file, `--object-streams` output edited by another tool — parsed
  with `encrypted` false and its page read as plaintext. Detection now asks
  nothing of the dictionary but the entry: a stream that names `/Encrypt` is a
  trailer, whether or not it says `/Type /XRef` or carries `/W` and `/Size`, so
  a file that lost them is still refused rather than read. With no `/ID` the
  handler cannot authenticate, and the document comes back with its
  `CryptError` instead of text. The `/Root` lookup reads the catalog from a
  stream dictionary the same way. New fixture
  `test-data/xref_encrypt_no_type.pdf` and test
  `an_untyped_xref_stream_encrypt_is_refused_not_read`. Suite counts 1,314 / 133.
- The same rule is now exercised on a whole file, not only a rebuilt one.
  Detection must not depend on `/Type /XRef`: a stream that names `/Encrypt` is
  a trailer, and a valid file can lose the marker while `startxref` and every
  offset stay intact. The earlier fixture was hand-made around a lost xref
  (`startxref 0`), so it only proved the repair path. qpdf 11.9.0 wrote
  `test-data/encrypted_xref_no_type.pdf` with object streams, making its xref
  stream the trailer; `/Type /XRef` was then blanked in place at equal length,
  so what is left is a whole file with only the marker missing. Test
  `a_valid_xref_stream_without_its_type_is_refused_not_read` checks that
  `startxref` names the stream object and `/Encrypt` sits in its dictionary,
  that the ciphertext is refused without a password, and that the same bytes
  open with the file's own password — so the refusal is the encryption seen,
  not an unindexable file. `test-data/make_xref_no_type.sh` regenerates it.
  Suite counts 1,343.

### 2026-11-22 — full review of the workspace

Read the workspace end to end after the security-handler rounds, exercising the command line as
a user runs it. Three defects were found and fixed, one suspicion was rejected on the evidence,
and two repository gates were already red.

- **`-o -` piped into `head` panicked** (`failed printing to stdout: Broken pipe`) and exited
  101 after the first screenful. Every stdout write now goes through `write_stdout`, which treats
  `BrokenPipe` as the end of the stream; `--progress` records do the same, so a parent that stops
  reading cannot stop the run. Test
  `a_consumer_that_stops_reading_ends_the_stream_without_a_panic` (a corpus JSON far larger than
  a pipe buffer, its reader dropped after 64 bytes).
- **`-i report.pdf -o out` with `out` an existing folder** wrote `out.json` beside the folder
  instead of `out/report.json` inside it, and a path ending in a separator did the same. Both are
  a destination folder now. Test `an_existing_output_folder_receives_the_document`.
- **A symlink back to its own folder** was followed until the kernel's 40-symlink limit: 41
  conversions of one document and a nested copy of the output tree. The walk remembers resolved
  directories, so an alias and its target are one folder. Test `a_symlink_cycle_is_walked_once`
  (Unix).
- **The LibreOffice profile URI was built by concatenation** (`file://{path}`), which is a URI
  only when the path is a Unix absolute path: a Windows temporary directory with a space in it
  (`C:\Users\John Smith\…`) or a `#` anywhere produced one LibreOffice cannot open. `profile_url`
  writes `file:///` + forward slashes + percent-encoding; unit test
  `a_profile_path_is_a_file_uri`. The Unix spelling is unchanged byte for byte.
- **`-o -` leaves extracted pictures beside the input**, which is where the default output would
  have gone. That is now said on stderr when the document has pictures — a document without
  any leaves no folder, and `embedded` writes none (the review found the gap the hard way: a probe left three
  PNGs in `test-data/real/arxiv_attention_images/`, hidden from `git status` by `*_images/`, and
  only `tracked_fixtures_test` noticed). The README's `-o` row and a new **Exit status** note
  document both it and the `3` a budget refusal exits with.
- **Rejected as a defect:** `-i notes.md --to md` rewrites the Markdown in place. Six `cli_test`
  cases depend on that normalization, the document is read whole before the first write, and
  every writer replaces its file through a temporary, so a failed write leaves the source whole.
  Left as it is, and said so in `convert_one`.
- Repository gates that were red on the committed tree:
  - `./scripts/test.sh --self-test` failed: `test-data/make_encrypted_unicode.sh` is a generator,
    but the coverage audit exempted only `*.py`. The rule is the `make_*` name now, and the
    synthetic coverage case adds a shell generator.
  - The encrypted fixtures from the handler rounds had no `scripts/fixture-owners.tsv` rows, so
    `--path` fell back to the full suite; they do now.
  - `plan.md` said the branch was `origin/main` plus four commits; it is eight.
- `test-data/xref_encrypt_no_type.pdf` was missing from the index, so a fresh checkout's
  `crypt_test` would fail. It went in with the `/Encrypt` round, together with its
  `scripts/fixture-owners.tsv` row: the runner's self-test refuses a row for a file that is not
  tracked, which is how this surfaced.
- Checks: `cli_test` **136 passed, 0 failed**; `documentation_test` 4/4 with the counts at
  1,318 / 136; `pdfextractor-formats` suite green; `tracked_fixtures_test` green;
  `./scripts/test.sh --self-test` green; `cargo fmt --all --check` clean; the workspace clippy
  run (`-D warnings`) clean; full workspace run **95 targets, 94 green** — the one failure was
  the probe litter above, and the target re-ran green once it was removed.

### 2026-11-23 — quick-xml 0.42, workspace lints and the scheduled gates

- `quick-xml` 0.36.2 → 0.42.0 in the workspace and in `fuzz/`: `plist`/`tauri` already
  resolved 0.42, so the tree keeps one copy. The nine reader files that moved to the new API:
  `QName` converted at the byte boundary, `normalized_value(XmlVersion::Implicit1_0)` in
  place of the deprecated `unescape_value` (12 sites), text and CDATA raw, and
  `Event::GeneralRef` arms so a reference between two runs is still text — one shared
  `limits::push_reference` instead of eight copies, with an undeclared entity keeping
  its spelling.
- EOL normalization restored: 17 text/CDATA sites read through
  `xml_content(XmlVersion::Implicit1_0)` (docx body and chart title, pptx, xlsx, odt,
  ods, odp, the ods chart and the chart reader). The fixture `test-data/crlf_text.docx`,
  written by `test-data/make_crlf_text_docx.py`, carries CRLF, a bare CR and an LF in a `<w:t>` and
  CRLF/CR in a CDATA section; `crlf_in_text_and_cdata_is_normalized_to_lf` fails if the
  event bytes are read directly (verified by temporarily reverting the docx arms).
- `unsafe_code = "forbid"` in `[workspace.lints.rust]`, every one of the 24 members opting
  in with `[lints] workspace = true`; neither `crates/` nor `fuzz/src` contains `unsafe`.
- Toolchain: `rust-toolchain.toml` pins 1.99.0 (clippy, rustfmt), README states the 1.92
  minimum and the 1.99.0 build, and both release jobs install the pin explicitly — the
  toolchain action publishes no per-version tag, so the version goes to its `toolchain`
  input. Since 2026-11-25 the release legs and `fmt.yml` read the channel out of
  `rust-toolchain.toml` instead of repeating it.
- The dependency gate: `.github/workflows/deny.yml` (Mondays 05:23 UTC, dispatch,
  cargo-deny 0.22.0, advisory-db cache) with `deny.toml` (`all-features`, licence allow
  list, duplicate versions and wildcards warn). Its first runs found the zip advisories
  (eight ids, named by id in `deny.toml`) and one cookie advisory: `zip` is 9.8.2 with only `deflate` enabled and `cookie` is
  0.18.3, so the gate is green without an ignore list. `ZipFile::name()` being fallible is
  threaded through the limits crate and the readers (an undecodable name skips the entry;
  the package gate reports it as `LimitError::Zip`).
- The fuzz smoke: `.github/workflows/fuzz.yml` (Mondays 04:43 UTC, dispatch, a `seconds`
  bound of 1–600, seven targets, corpus seeding and caches, findings uploaded for 30 days).
  The timeout is 110 minutes so the 600 s maximum can finish, and a failing target no longer
  skips the ones after it: the step collects the failures, runs the whole list, then fails.
- Documentation: README's CI prose covers both gates and its `test-data` tree lists the CRLF
  fixture; `fuzz/README.md` gained *The scheduled smoke*; `plan.md`'s crate count (24) is
  held by `documentation_test`, and its stale local-commit count is corrected to eleven.
- Checks: clippy (`-D warnings`) and `fmt --check` clean; limits, chart and the six office
  readers' suites green; `cli_test` 137 passed; `documentation_test` green with 1,320;
  `cargo deny check advisories bans licenses` green. The full workspace suite was not
  re-run, and no workflow was dispatched.
- PDF outlines as a heading signal. Measured first: six of the nine corpus PDFs carry
  outlines (89 entries); 67 already agreed with a `section_header`, 15 sat in an
  unsplit paragraph, 7 were absent from the extracted text. `PdfDocument::outline()`
  reads titles, nesting and both inline and named (`/Dests`, `/Names /Dests`)
  destinations, which is what five of the six files use; layout takes per-page
  `HeadingHint`s, matches on letters and digits at a line boundary, splits a heading
  off a merged paragraph and sets its level from the bookmark depth. Corpus headings
  158 -> 167 (seq2seq appendix `A.1`-`B.2` at levels 2/3; one batchnorm heading no
  longer carries the body line that followed it), no change on the four outline-less
  PDFs. Pinned by `test-data/outline_headings.pdf` (four 10 pt headings only the
  outline names, direct and named destinations, nested one level;
  `make_outline_headings.py`) and `test-data/golden/markdown/outline_headings.md`
  (`##`/`###` hierarchy). `md_golden_test`, `tracked_fixtures_test`, layout tests,
  clippy and fmt green.
- OCR engine reuse, measured first. `crates/ocr-engine/benches/engine_cost.rs` (std
  clock, `harness = false`, `cargo bench -p pdfextractor-ocr-engine --bench
  engine_cost`) times construction and one-page recognition, then ten documents
  with a fresh engine against ten through one. Release medians on `01_flat.png`:
  construction 173-176 ms, one page 458-476 ms; fresh documents 593-667 ms,
  worker engine 468-503 ms (-15% to -30% across runs). The CLI now keeps a
  thread-local engine per Rayon worker, keyed by language and psm; PDF and image
  readers borrow it instead of constructing one per document, and non-OCR formats
  never start Tesseract. The file-size ceiling is still checked before `detect`
  reads the input. End-to-end debug batch (16 scans, `-p 4`): ~37.0 s before,
  ~35.9 s after, byte-identical output. Checks: `pdfextractor-ocr-engine` 5
  passed, CLI `scan` 3, `image` 6, `ocr_languages` 1, bench run, clippy/fmt.

### 2026-11-24 — pipeline extraction

- **The conversion is a library.** `crates/pipeline` (`pdfextractor-pipeline`)
  now holds `build_document`, `build_document_with_format`, every reader dispatch
  and its helpers, the OCR worker cache, picture extraction, the preview writer,
  `relabel` and the run's budgets (`limits`/`set_limits`). It takes
  `ConvertOptions` instead of clap's `Args`, so an embedder converts without
  spawning the binary.
- **The CLI keeps its edges.** Argument parsing, the input walk, output writers,
  compare/redline, the extension gate and its refusals stay in `main.rs`; the
  gate calls the library after the name is accepted. `crates/cli/src/preview.rs`
  moved into the crate.
- **Checks.** `cli_test` unchanged: **140 passed, 0 failed** (the parity proof;
  Works case skipped without `PDFE_WORKS_SAMPLES`). Pipeline unit tests **23
  passed**, CLI binary unit tests **12 passed**, `documentation_test` **4 passed**
  with the workspace count at 25; focused clippy `-D warnings` clean and
  `cargo fmt --all --check` clean. The full workspace suite was not re-run.

### 2026-11-25 — the toolchain in every gate, and the zip decision recorded

- **Every cargo job installs the pin, and says which.** rustup's automatic install of
  a missing toolchain is deprecated, so the pin is fetched by a step that either
  succeeds or fails on its own line. Clippy, both test jobs, both compare-gate jobs,
  bridge and deny name `toolchain: 1.99.0` (with `clippy`/`rustfmt` components where the
  job uses them). `fmt.yml` and the two release legs read the channel out of
  `rust-toolchain.toml`, pass it to the action's `toolchain` input and log it: the
  install stays explicit, and a bumped pin cannot leave the formatter or a release on
  the old one. `fmt.yml` had been installing floating stable that `rust-toolchain.toml`
  then overrode. Fuzz stays on nightly (`@nightly`, `cargo +nightly`), which never
  resolves the pin. No job, matrix leg or path filter was added; the install is the
  cached one.
- **The zip advisories are documented, not ignored.** `deny.toml` names them by id
  with their patched floors; the zip entry below records the eight ids and why none can
  apply. They are absent from `ignore` on purpose: an ignore cargo-deny never meets
  would silence a real hit if the dependency moved back.
- **The smoke, as it now stands.** Seven targets, a 110-minute job timeout so a manual
  600 s maximum can finish, findings kept 30 days, and a failing target does not skip
  the rest. README's CI prose was corrected to those numbers and `fuzz/README.md`
  already matched.
- **Checks.** `cargo deny check advisories bans licenses` green with `chrono` the only
  ignore; `cargo test -p pdfextractor-cli --test cli_test` 140 passed; the zip-reading
  crates' suites green; `documentation_test` green with the suite at 1,352 then (1,358
  after the six CRLF tests added later) and the workspace at 25 crates; focused clippy
  `-D warnings` and `cargo fmt --all --check` clean. The full workspace suite ran
  separately at that point: 1,347 passed with the corpus ratchet's two heading-driven
  regressions still open.

### 2026-11-25 — zip 2.4.2 → 9.8.2

- **One version, not two.** `zip` moved to 9.8.2 in the workspace and in `fuzz/`;
  `cargo tree -d` reports no duplicate, so the readers and the fuzzer build the same
  copy.
- **Deflate only.** `default-features = false, features = ["deflate"]` leaves out the
  bzip2/LZMA/XZ/Zstd/PPMd/Deflate64 decoders and the WinZip AES path — code none of
  the readers calls, and their licences with it.
- **A fallible entry name.** `ZipFile::name()` returns a `Result` in this release, so
  an entry whose name cannot be decoded is skipped rather than panicking, and the
  package gate reports the same condition as `LimitError::Zip`; every reader goes
  through that one wrapper instead of repeating the rule.
- **The weekly gate is green without an ignore.** The extraction advisories the first
  `cargo deny` run found — `extract()`, `ZipStreamReader`, symlink targets, AES — are
  all paths this workspace never calls, and 9.8.2 sits above every patched floor the
  database states for them (the highest is `>= 9.0.5`). `deny.toml` names all eight ids
  with those floors instead of ignoring them, and `cargo deny check advisories bans
  licenses` ends `advisories ok, bans ok, licenses ok`.
- **So no zip id sits in `ignore`, and none should be added.** An ignore is only right
  for an advisory the tree actually matches; these no longer match, and cargo-deny
  reports an ignore it never met, so one here would make the gate noise today and go
  on silencing a real hit if `zip` ever moved back below a floor. The ids live in the
  comment block beside `ignore` for that reason — read it before re-adding one.
- **Checks.** The zip-reading crates (`docx`, `xlsx`, `odt`, `ods`, `odp`, `pptx`,
  `metafile`, `limits`) green, and the full workspace suite ran to completion with the
  oracle live: 1,351 passed then, the only failure the corpus ratchet's two
  heading-driven regressions (the six CRLF tests added afterwards bring the workspace
  listing to 1,358).

### 2026-11-25 — WS8 P2: the CLI's manifest trimmed to what it names

- **Nine readers moved to `[dev-dependencies]`.** `pdf-parser`, `docx-parser`,
  `odt-parser`, `ods-parser`, `odp-parser`, `doc-parser`, `xlsx-parser`,
  `pptx-parser` and `text-parser` are reached by `limits_matrix_test` and the
  fixture suites directly; `src/` names none of them, because every document is
  converted through `pdfextractor-pipeline`. They now compile for the tests only.
- **Four dependencies deleted.** `chart`, `svg`, `layout` and `ocr-engine` were
  named nowhere under `crates/cli` — the pipeline carries them — so they left the
  manifest rather than being moved.
- **The `ocr` feature now forwards one hop.** It used to name
  `pdfextractor-ocr-engine/tesseract` as well as `pdfextractor-pipeline/ocr`; with
  the engine gone from the manifest it forwards to the pipeline only, and the
  pipeline still enables the same engine feature. `--no-default-features` still
  builds a binary with no Tesseract to link.
- **The cut is visible in the graph.** The CLI's normal edges
  (`cargo tree --no-default-features --edges normal --depth 1`) are ten workspace
  crates with no reader; the readers remain only below `pdfextractor-pipeline`.
- **Checks.** `cargo build -p pdfextractor-cli --no-default-features` and
  `--all-targets --no-default-features` green (the test-only paths still compile),
  the default build green (the single-hop forward works), `cli_test` 140 passed —
  unchanged — and `documentation_test` 4 passed. The workspace listing stays 1,360.

### 2026-11-25 — the corpus ratchet's outline-round delta accepted, with the argument

- **What moved.** The outline round changed two corpus documents. `math_batchnorm`:
  `4.2.1 Accelerating BN Networks` is split off the paragraph it was glued to (the
  reference holds it as its own `section_header`). `math_seq2seq`: nine paragraphs the
  document's own outline names become `section_header`, and `B.1 PARAMETER
  INITIALIZATION` is split off the line of arrows that follows it.
- **Why the counters moved.** Both splits are what the reference does. `math_batchnorm`
  ends with 2 fewer rows, one more `same`, 2 fewer `oursOnly`, 3 fewer `theirsOnly` and
  3 more `differs` — the `differs` are a pairing shift on page 6, where both readings
  garble the same two columns, so items that were one-sided now pair. `math_seq2seq`
  gains one `oursOnly`: the arrow line, a figure the reference carries no text for.
- **Decision.** Accepted, not "fixed": the reading is closer to the reference in both
  documents, no reference item was lost, and nothing was read differently — the split
  itself is the whole delta. Rewriting it would mean undoing correct structure to hold
  a counter. The baseline was regenerated with `CORPUS_BASELINE=update` and its `note`
  records this argument, so the diff of `test-data/golden/compare-baseline.json` is the
  record a reviewer reads beside the change.
- **Checks.** `cargo test -p pdfextractor-ui --test corpus_report_test` green (11
  passed) against the regenerated baseline; the diff touches those two documents only.
