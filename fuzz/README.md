# Fuzz targets for untrusted inputs

These targets are built with [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
and libFuzzer, so they live outside the product workspace and are not part of
`cargo test --workspace`.

| Target | Reaches |
|---|---|
| `pdf` | PDF object parsing, page tree, decoded streams, image XObjects, page limit and parser deadline. |
| `zip` | The shared ZIP/XML gate, then DOCX, XLSX, PPTX, ODT, ODS or ODP according to the package's own parts. |
| `xml` | XML size, nesting depth, DOCTYPE/entity rejection and the XML deadline. |
| `image` | Image header limits, guarded decode, GIF/WebP/TIFF frame budgets and image deadlines. |

Seed them with the committed fixtures; the real corpus is useful too:

```sh
mkdir -p fuzz/corpus/pdf fuzz/corpus/zip fuzz/corpus/image fuzz/corpus/xml
cp test-data/*.pdf fuzz/corpus/pdf/
# The committed crash reproducers seed the pdf target as well, so a run starts
# from the input shape that took the PNG encoder down instead of rediscovering
# it. They are ignored by `.gitignore` only as corpus copies; the originals
# under `fuzz/artifacts/pdf/` are tracked.
cp fuzz/artifacts/pdf/* fuzz/corpus/pdf/
cp test-data/*.docx test-data/*.xlsx test-data/*.pptx test-data/*.odt fuzz/corpus/zip/ 2>/dev/null || true
cp test-data/*.png test-data/*.jpg test-data/*.tif fuzz/corpus/image/ 2>/dev/null || true
printf '<a><b/></a>' > fuzz/corpus/xml/basic.xml
```

Run one target for a bounded time:

```sh
cargo +nightly fuzz run pdf -- -max_total_time=60
cargo +nightly fuzz run zip -- -max_total_time=60
cargo +nightly fuzz run xml -- -max_total_time=60
cargo +nightly fuzz run image -- -max_total_time=60
```

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
