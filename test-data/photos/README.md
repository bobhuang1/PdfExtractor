# Photographed pages — the OCR quality corpus

A clean scan proves the pipeline can read a page that was already a picture of
text. These eight files are photographed pages: turned a few degrees, lit
unevenly, and — on two of them — soft and small enough that recognition is not
perfect. Three are stored the wrong way round: one says so in an EXIF tag, one
is sideways with no tag at all, and one is upside down with no tag either — the
last two can only be found by reading the pixels. They exist so that OCR
quality has a number, and a floor under it, that a change has to move.

| File | Page | Condition |
|---|---|---|
| `01_flat.png` | ACME invoice | square to the frame, even light, sharp |
| `02_skew.png` | City Library hours | 2.5° clockwise, even light, sharp |
| `03_backlit.png` | Harbour Logistics memo | 4° counter-clockwise, diagonal fall-off |
| `04_shadow.png` | ACME invoice | 3°, vignette and corner shadow, slight blur |
| `05_photo.jpg` | City Library hours | 4°, fall-off and shadow, softer, JPEG 72 |
| `06_rotated.jpg` | ACME invoice | held sideways: pixels stored 90° counter-clockwise, EXIF orientation 6 |
| `07_sideways.jpg` | ACME invoice | pixels stored 90° counter-clockwise, **no EXIF tag**: the orientation has to be found from the page |
| `08_upside_down.png` | City Library hours | pixels stored upside down, **no EXIF tag**: the half turn has to be found from the page |

All eight are A4 pages at 150 dpi, so the page size the reader reports is the
same for each. `06_rotated.jpg` and `07_sideways.jpg` are stored as a
1754 × 1240 landscape grid, the way a phone held on its side writes it;
`06_rotated.jpg`'s EXIF tag says to turn it back, while `07_sideways.jpg` has no
tag and its pixels are all the file says. `08_upside_down.png` keeps the
portrait grid and turns the text round instead. The page and the text of all
three are measured upright.

`corpus.json` is the ground truth: generated with the images, one entry per page
with the exact lines it was drawn from and the condition it was photographed
under. The images and the reference cannot drift apart because one script writes
both.

Everything is written by [`make_photo_corpus.py`](../make_photo_corpus.py):

```bash
~/.venvs/pdfextractor-docling/bin/python test-data/make_photo_corpus.py
```

Regeneration is deterministic — the lighting masks are computed and Pillow's
rotation and blur are — so identical bytes come back until the script changes.

## The report

`crates/cli/tests/ocr_quality_test.rs` converts every page through the real
binary with `--ocr auto`, compares the recognised text with `corpus.json`, and
prints the scores:

```bash
cargo test -p pdfextractor-cli --test ocr_quality_test -- --nocapture
```

* **characters** — one minus the character edit distance over the reference's
  length: the share of the page's characters the reading holds, where an
  insertion, deletion or substitution is one error;
* **words** — the same over whitespace-split tokens, after both sides are
  lower-cased and whitespace is collapsed. A lost space merges two tokens, which
  costs two word edits (one token disappears, another absorbs it).

The aggregate is weighted by the reference's length rather than averaged, so a
short page cannot outvote a long one.

Measured with the corpus as committed:

```text
01_flat.png        100.0%   100.0%
02_skew.png        100.0%   100.0%
03_backlit.png     100.0%   100.0%
04_shadow.png      100.0%   100.0%
05_photo.jpg        95.6%    87.0%
06_rotated.jpg     100.0%   100.0%
07_sideways.jpg    100.0%   100.0%
08_upside_down.png 100.0%   100.0%
aggregate           99.4%    98.3%
```

The test's floors sit a margin below those numbers (98/98 on the clean pages,
92/82 on the hardest one, 96/92 over the corpus). `a_sideways_camera_scan_is_read_upright`,
`a_sideways_scan_with_no_exif_tag_is_read_upright` and
`an_upside_down_scan_with_no_exif_tag_is_read_upright` hold the three turned
pages to a stronger reading still: each page is A4 portrait (595.2 × 841.92 pt) and every
line of it comes back upright and inside the page — which is only true if
recognition saw upright pixels measured on the upright grid. The untagged pair
is also checked to carry no `Exif` marker, so the test cannot pass because of a
tag the fixture happened to grow. A change that costs the clean pages a couple of
characters, or the hard page several points, fails and prints what was read
beside what the page says. The floors are in the test rather than here so that
the accepted numbers and the thing holding the build to them are in one place.
