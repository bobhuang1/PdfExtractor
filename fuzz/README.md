# Fuzz targets for untrusted inputs

These targets are built with [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
and libFuzzer, so they live outside the product workspace and are not part of
`cargo test --workspace`.

| Target | Reaches |
|---|---|
| `pdf` | PDF object parsing, page tree, decoded streams, image XObjects, page limit and parser deadline; a second pass opens the same input with a password derived from it, so the security handler's key derivation and decryption run too. |
| `zip` | The shared ZIP/XML gate, then DOCX, XLSX, PPTX, ODT, ODS or ODP according to the package's own parts. |
| `xml` | XML size, nesting depth, DOCTYPE/entity rejection and the XML deadline. |
| `image` | Image header limits, guarded decode, GIF/WebP/TIFF frame budgets and image deadlines. |
| `budgets` | All four gates above at once, under `Limits` read from the input's own first 16 bytes instead of `FUZZ_*`: ceilings small enough to sit under a real document, so the refusal paths are fuzzed too. |
| `crypt` | Encrypted PDFs: the committed fixtures with their stream payloads overwritten by the input (the real `/Encrypt` authenticates, so the handler decrypts mutated ciphertext), and a minimal PDF whose `/Encrypt` dictionary body is the input. Every refusal must carry a `CryptError` or a `LimitError`; an untyped failure or a panic is the finding. |
| `encrypted` | Encrypted PDFs under budgets and a password the input derives: the 16-byte header is `Limits` (as in `budgets`), the next byte is a password length and those bytes are the password, and the rest is the body. The body is spliced into a minimal PDF's `/Encrypt` dictionary, and also cut into a well-formed `/V 5 /R 6` handler's `/O`, `/U`, `/OE` and `/UE`, so malformed dictionaries and algorithm 2.B, the revision-6 password hash, are exercised on fuzzed salts and password. An extraction refusal must be typed. |

Seed them with the committed fixtures; the real corpus is useful too:

```sh
mkdir -p fuzz/corpus/pdf fuzz/corpus/zip fuzz/corpus/image fuzz/corpus/xml \
  fuzz/corpus/crypt fuzz/corpus/budgets fuzz/corpus/encrypted
cp test-data/*.pdf fuzz/corpus/pdf/
cp test-data/encrypted*.pdf fuzz/corpus/crypt/
# The committed crash reproducers seed the pdf target as well, so a run starts
# from the input shape that took the PNG encoder down instead of rediscovering
# it. They are ignored by `.gitignore` only as corpus copies; the originals
# under `fuzz/artifacts/pdf/` are tracked.
cp fuzz/artifacts/pdf/* fuzz/corpus/pdf/
cp test-data/*.docx test-data/*.xlsx test-data/*.pptx test-data/*.odt fuzz/corpus/zip/ 2>/dev/null || true
cp test-data/*.png test-data/*.jpg test-data/*.tif fuzz/corpus/image/ 2>/dev/null || true
printf '<a><b/></a>' > fuzz/corpus/xml/basic.xml
# `budgets` reads its ceilings from the first 16 bytes, so a headered fixture
# seeds a real document and the widest derived budgets in one input.
mkdir -p fuzz/corpus/budgets
{ printf '\377\377\377\377\377\377\377\377\377\377\377\377\377\377\377\377'; cat test-data/hello.pdf; } > fuzz/corpus/budgets/hello.pdf
{ printf '\377\377\377\377\377\377\377\377\377\377\377\377\377\377\377\377'; cat test-data/brief.docx; } > fuzz/corpus/budgets/brief.docx
# `encrypted` reads the same 16-byte header, then a password length and that
# many password bytes; the rest is the body its two PDFs are built from.
mkdir -p fuzz/corpus/encrypted
{ printf '\377\377\377\377\377\377\377\377\377\377\377\377\377\377\377\377'; printf '\003'; printf 'user'; cat test-data/encrypted_aes256.pdf; } > fuzz/corpus/encrypted/aes256.pdf
```

Run one target for a bounded time:

```sh
cargo +nightly fuzz run pdf -- -max_total_time=60
cargo +nightly fuzz run zip -- -max_total_time=60
cargo +nightly fuzz run xml -- -max_total_time=60
cargo +nightly fuzz run image -- -max_total_time=60
cargo +nightly fuzz run budgets -- -max_total_time=60
cargo +nightly fuzz run crypt -- -max_total_time=60
cargo +nightly fuzz run encrypted -- -max_total_time=60
```

## The scheduled smoke

`.github/workflows/fuzz.yml` runs all seven targets for a bounded time every
Monday (04:43 UTC), and by hand from the Actions page with a `seconds` input —
60 by default, and 1–600, so a mistyped value cannot hold a runner for hours.
It is not part of a push: the smoke's value is that the targets still build,
still take their seeds and find nothing immediate in a short pass, and a longer
run does not prove more than that.

The job is a clean checkout, so it seeds the corpus with the commands above
(nothing under `fuzz/corpus` is committed) and restores the corpus cache first,
so a run continues from what earlier ones grew instead of starting from the
fixtures alone. It then runs, once per target:

```sh
cargo +nightly fuzz run <target> fuzz/corpus/<target> -- \
  -max_total_time=60 -timeout=30 -rss_limit_mb=4096
```

`-timeout=30` stops one input that hangs the target and `-rss_limit_mb=4096`
one that allocates without bound; both are the job's bounds rather than a
target's, so what they turn up is a finding like any other. A finding lands
under `fuzz/artifacts/<target>/` and the job uploads that directory
(`fuzz-artifacts-<run id>`, kept 30 days) when a target fails, so it can be
downloaded and replayed with the command above rather than reconstructed from
the log. A target that fails does not skip the ones after it: the step runs the
whole list, then fails, so every target still takes its seeds and every finding
the run produced is kept. Reproducing the smoke locally is the seeding recipe,
the corpus cache of your own working tree, and one run per target.

## Budgets read from the input

`budgets` does not read `FUZZ_*`. It takes its `Limits` from the input's own
first 16 bytes (`pdfextractor_fuzz::limits_from`): each byte is scaled into a
range that sits under a real document — input 0–64 KiB, PDF objects 0–64, tokens
0–4096, nesting 0–8, decoded stream 0–1 MiB, ZIP 0–16 entries, XML 0–64 KiB and
0–16 deep, image 0–65,536 pixels, document total 0–4 MiB, the three clocks 0–100
ms — and the rest of the input is the document handed to the PDF, ZIP, XML and
image readers. The totals and the clocks share a header byte with the size they
bound, because sixteen bytes cannot give each of the twenty-two budgets one.

A run therefore fuzzes the refusal side of every ceiling with no environment set:
the mutation that grows a document also moves the budget that governs it. A zero
header is a valid input and refuses everything, which is a boundary the scale is
meant to reach; an input shorter than the header keeps the product defaults and
is read whole.

`encrypted` reads the same header for its budgets, then a one-byte password
length and that many password bytes; the remaining body is spliced into a
minimal `/Encrypt` dictionary and cut into a revision-6 handler's `/O`, `/U`,
`/OE` and `/UE`. Its budgets therefore govern the revision-6 hash's reach as
well as the parse, and the password is fuzzed with the file. The four format targets keep their `FUZZ_*` recipe, where a
lowered pass is a deliberate, repeatable measurement rather than a mutation.

A finding is written under `fuzz/artifacts/<target>/`; reproduce it with
`cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>`.

The corpus and new findings are machine-generated, so they stay out of the
repository (see `.gitignore`); the seeding commands above rebuild the corpus
from fixtures that are committed. A finding that becomes a regression test is
the exception: the two PDF crashes are committed under `fuzz/artifacts/pdf/`
and replayed by `crates/pdf-parser/tests/fuzz_regressions_test.rs`, so the case
stays covered without a nightly toolchain. The seeding step above also copies
those reproducers into `fuzz/corpus/pdf/`, so a later `cargo +nightly fuzz run
pdf` begins from the shape that crashed even before the fuzzer mutates it.

## Lowered budgets

The targets build their `Limits` from the environment, so a run can be lowered
into the refusal paths without a rebuild. Unset variables keep the product
default.

| Variable | Budget |
|---|---|
| `FUZZ_MAX_INPUT_BYTES`, `FUZZ_MAX_PAGES`, `FUZZ_MAX_FLOW_BLOCKS` | shared input, page and block ceilings |
| `FUZZ_MAX_DOCUMENT_BYTES` | whole-document total every decoded PDF stream and image frame is charged to |
| `FUZZ_MAX_PDF_OBJECTS`, `FUZZ_MAX_DECODED_STREAM_BYTES` | PDF object and decoded-stream ceilings |
| `FUZZ_MAX_ZIP_ENTRIES`, `FUZZ_MAX_ZIP_ENTRY_BYTES` | package entry ceilings |
| `FUZZ_MAX_XML_BYTES`, `FUZZ_MAX_XML_DEPTH` | XML size and nesting |
| `FUZZ_MAX_IMAGE_PIXELS`, `FUZZ_MAX_IMAGE_FRAMES` | image grid, the picture transcode and the frame stack |
| `FUZZ_TIMEOUT_MS`, `FUZZ_OCR_TIMEOUT_MS` | parser and OCR deadlines |

`FUZZ_REPORT_REFUSALS=1` prints every budget refusal with its stage, which is
how the corpus pass shows what the lowered budgets actually refused:

```sh
FUZZ_REPORT_REFUSALS=1 FUZZ_MAX_INPUT_BYTES=65536 FUZZ_TIMEOUT_MS=200 \
  cargo +nightly fuzz run pdf fuzz/corpus/pdf -- -runs=2000 -max_total_time=15
```

### The lowered passes used so far

These are the settings the lowered runs used, kept here so the refusal paths can
be reached again without rediscovering which ceiling catches which input. Each
is a corpus pass: the seed corpus holds multi-part packages, XML parts and
multi-frame stacks, so a ceiling low enough to sit under them is hit on a seed
and the fuzzer then mutates around the refusal.

Run them from the repository root, which is where `cargo fuzz` finds `fuzz/` and
where the corpus paths below resolve. From inside `fuzz/` the same path names
`fuzz/fuzz/corpus/xml`, and the target stops with `ERROR: The required directory
"fuzz/corpus/xml" does not exist`.

| Pass | Environment | Comes back as |
|---|---|---|
| 16-entry ZIP | `FUZZ_MAX_ZIP_ENTRIES=16` | `TooManyZipEntries`, walking the package |
| depth-8 XML | `FUZZ_MAX_XML_DEPTH=8` | `XmlTooDeep`, from the shared XML gate |
| 512-byte XML | `FUZZ_MAX_XML_BYTES=512` | `XmlTooLarge`, from the shared XML gate |
| 2-frame image | `FUZZ_MAX_IMAGE_FRAMES=2` | `TooManyImageFrames`, from the GIF/WebP/TIFF readers |

```sh
# 16-entry ZIP: a package with more parts than the ceiling is refused before
# the second part's reader runs.
FUZZ_REPORT_REFUSALS=1 FUZZ_MAX_ZIP_ENTRIES=16 \
  cargo +nightly fuzz run zip fuzz/corpus/zip -- -runs=2000 -max_total_time=15

# Depth-8 XML: the gate is shared, so it runs in front of every package part.
FUZZ_REPORT_REFUSALS=1 FUZZ_MAX_XML_DEPTH=8 \
  cargo +nightly fuzz run xml fuzz/corpus/xml -- -runs=2000 -max_total_time=15

# 512-byte XML: a part larger than the ceiling is refused before it is parsed.
FUZZ_REPORT_REFUSALS=1 FUZZ_MAX_XML_BYTES=512 \
  cargo +nightly fuzz run xml fuzz/corpus/xml -- -runs=2000 -max_total_time=15

# 2-frame image: the third frame of a stack is the refusal, so a GIF or TIFF
# with a frame or two in the corpus reaches it on the first seed.
FUZZ_REPORT_REFUSALS=1 FUZZ_MAX_IMAGE_FRAMES=2 \
  cargo +nightly fuzz run image fuzz/corpus/image -- -runs=2000 -max_total_time=15
```

The XML ceilings also apply to the `zip` target, because every package part goes
through the same gate: `FUZZ_MAX_XML_BYTES=512` or `FUZZ_MAX_XML_DEPTH=8` on
`cargo +nightly fuzz run zip` is the package-side spelling of the two XML passes.

A refusal printed by these passes is the budget working, not a finding: the
targets return the `LimitError` and keep going, and `FUZZ_REPORT_REFUSALS=1` is
what makes it visible. A crash, a hang or a panic is the finding.

The product tests (`cargo test -p pdfextractor-limits`, the `limits_test`
binaries in `pdf-parser` and `image-parser`) are the deterministic half: they
build a ZIP bomb, a deeply nested XML part, a giant image header, a page tree
over budget, an over-size Flate stream and a zero-deadline parser, and assert
each is refused by the budget rather than read.
