# Real-world test corpus

Documents produced by other tools, used as regression fixtures by
`crates/pdf-parser/tests/real_world_test.rs` and
`crates/docx-parser/tests/real_docx_test.rs`. Hand-written fixtures did not
expose the bugs these files found.

Eleven of them are also converted by real Docling as golden references — see
`test-data/golden/README.md`.

## PDFs

| File | Source | Producer | What it exercises |
|---|---|---|---|
| `arxiv_attention.pdf` | [arXiv:1706.03762](https://arxiv.org/abs/1706.03762) "Attention Is All You Need" | pdfTeX 1.40.25, PDF 1.5 | `/ObjStm` object streams, `/Differences` ligature encodings, TJ-kerning word gaps, **figures at exactly 500 dpi** |
| `tracemonkey.pdf` | [pdf.js test corpus](https://github.com/mozilla/pdf.js/tree/master/test/pdfs) | Mac OS X Quartz, PDF 1.4 | large multi-column paper, `Td` word gaps, many fonts, a **vector-only figure** |
| `basicapi.pdf` | [pdf.js test corpus](https://github.com/mozilla/pdf.js/tree/master/test/pdfs) | TCPDF, PDF 1.6 | Type0 / Identity-H composite fonts — and **zero image XObjects**, so it must report no pictures |
| `irs_f1040.pdf` | [IRS Form 1040](https://www.irs.gov/pub/irs-pdf/f1040.pdf) | PDF 1.7 | **linearized** file with an `%%EOF` mid-file, page tree in `/ObjStm`, cp1252 punctuation, AcroForm fields, and **page-covering background rectangles that must not be called figures** |
| `dummy.pdf` | [W3C test file](https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf) | minimal, PDF 1.4 | smallest useful sanity case |
| `math_batchnorm.pdf` | [arXiv:1502.03167](https://arxiv.org/abs/1502.03167) | pdfTeX, PDF 1.4 | math-heavy paper; plots drawn as **vector paths only** |
| `math_layernorm.pdf` | [arXiv:1607.06450](https://arxiv.org/abs/1607.06450) | pdfTeX, PDF 1.4 | TeX math fonts (`CMSY`/`CMMI`/`CMEX`) whose glyph codes are not in the PDF |
| `math_seq2seq.pdf` | [arXiv:1409.3215](https://arxiv.org/abs/1409.3215) | pdfTeX, PDF 1.4 | stacked vector diagram panels |
| `math_gan.pdf` | [arXiv:1406.2661](https://arxiv.org/abs/1406.2661) | pdfTeX, PDF 1.4 | **both** raster figures and vector figure panels on the same pages |

## DOCX (OOXML WordprocessingML)

| File | Source | What it exercises |
|---|---|---|
| `VariousPictures.docx` | [Apache POI test data](https://github.com/apache/poi/tree/trunk/test-data/document) | five drawings in several formats: PNG, JPEG, **EMF**, **WMF** — the three metafiles are played into PNGs (a Santa, a cow and a wrench) |
| `drawing.docx` | Apache POI test data | 105 tables, 10 media parts, prose mixed with tables; a **CorelDRAW logo stored as a WMF behind a `w:object`** |
| `shapes-with-text.docx` | Apache POI test data | drawings that are **shapes, not images** (media count is zero) |
| `chartex.docx` | Apache POI test data | chart drawings |
| `headerPic.docx` | Apache POI test data | the only picture lives in `word/header1.xml` — a separate part |
| `calibre_demo.docx` | [calibre demo](https://calibre-ebook.com/downloads/demos/demo.docx) | headings, list items, 38 tables, 4 images |
| `math-*.docx` (13 files) | [LibreOffice OOXML test data](https://github.com/LibreOffice/core/tree/master/sw/qa/extras/ooxmlexport/data) | real Office Math (`m:oMath`) covering fractions, scripts, radicals, n-ary operators, accents, matrices, limits, overstrikes, vertical stacks, escaping, placeholders |
| `EquationWithAboveAndBelowCaption.docx` | LibreOffice OOXML test data | the binomial theorem, with a caption above and below |
| `EquationAsScientificNumbering.docx` | LibreOffice OOXML test data | equation numbering, caption label |
| `floatingtbl_with_formula.docx` | LibreOffice OOXML test data | a floating table containing formulas |
| `mathtype.docx` | LibreOffice OOXML test data | a **MathType OLE object** — binary, not OMML, so no formula text is recoverable; its **EMF preview** is the equation's only picture, and it is played into a PNG |

## Legacy `.doc` (Word 97 binary)

`equation.doc`, `two_images.doc`, `testPictures.doc` and `FloatingPictures.doc`
(Apache POI test data) are the legacy **OLE compound file** format, not an OPC
package. `crates/doc-parser/tests/real_doc_test.rs` reads them through the
container, Fib and piece table. Between them they cover an embedded OLE object
behind a field (whose instruction must not become text), two inline pictures with
captions, a `SHAPE` field with drawn objects and a page break, and a long
styles-and-fonts listing. Raster Art extraction is not implemented, so the
pictures come out as placeholders.

## Bugs these files exposed

1. **Linearized PDFs** contain an `%%EOF` *mid-file* (after the first-page
   cross-reference section), with the rest of the document following. The lexer
   stopped at the first `%%EOF` and parsed only 2 of 295 objects — so
   `irs_f1040.pdf` produced zero pages.
2. **Object streams** (`/Type /ObjStm`) hold the page tree in PDF 1.5+ files,
   so it was invisible until they were expanded.
3. **Words were concatenated**: producers separate words by repositioning
   (negative `TJ` kerning, same-line `Td`), not with space characters.
4. **Character codes were emitted raw**: cp1252 punctuation (`0x97` em dash),
   ligatures from `/Differences`, and Identity-H 2-byte codes all came out as
   control characters.
5. **`Box2` derived `Default`**, so `empty` started as `false` and the first
   `add` kept a bogus `(0, 0)` origin — stretching **every** vector figure box
   back to the page corner. Caught by the vector-figure test.
6. **The image files lied about their format**: EMF and WMF payloads were written
   with a `.png` extension. The file command now disagrees with nothing.
7. **A form's background rectangles were reported as figures**: the IRS form's
   own border covers ~86% of the page and passed the minimum-area filter. A
   maximum-area rule now rejects page furniture.
8. **Office Math operators are carried in attributes** (`<m:chr m:val="∑"/>`).
   A math tree that only collected child text silently lost every Σ, Π and ∮.
9. **A Word drawing is not always a bitmap.** Word stores a vector drawing — and the
   preview of an embedded object — as an Enhanced Metafile or a Windows Metafile: a list
   of GDI calls. Neither media type is in Docling's registry, so `VariousPictures.docx`'s
   three drawings were reported as pictures with a box and no `image`, and `drawing.docx`'s
   CorelDRAW WMF was not reported at all because the `w:object` holding it was never read
   as a picture. `crates/metafile` plays the records now and `docx-parser` carries the
   raster as a PNG (bug 50 in `progress.md`).
10. **A compound file's mini-FAT count is in sectors, not entries**
   (`crates/doc-parser/src/cfb.rs`). Reading only that many *entries* truncated the
   table and `equation.doc`'s `WordDocument` stream came back as 896 bytes instead
   of 3620, so its Fib looked truncated.

### Bugs the Docling comparison exposed

Found by diffing our JSON against real Docling conversions of these same files
(`test-data/golden/`), which is the first check that compared our *labelling* of
a document with someone else's. Together they are bug 46 in `progress.md`.

11. **Every shape was read twice.** Word writes each shape twice — the real
    DrawingML in `mc:Choice` and a VML copy for old readers in `mc:Fallback` — so
    `shapes-with-text.docx` reported each shape's text twice in a row. The
    fallback subtree is now skipped. Caught by a check that no block repeats
    itself, which needs no reference to be true.
12. **Tabs were dropped.** A `w:tab` in a run is a real tab on the page: the
    table of contents in `calibre_demo.docx` separates its title from its page
    number with one, and without it `Demonstration of DOCX support in calibre1`
    ran the two together. Tab *stops* are declared with the same element inside
    `w:pPr` and are still not text.
13. **Chart titles were missing.** A chart's title exists only in the chart part
    (`word/charts/chart1.xml`); the drawing in the body is a reference to it, so
    looking at the drawing finds no words at all. `chartex.docx`'s six titles now
    come out as captions, including the two modern chart types that write theirs
    under the `cx:` namespace and that Docling itself misses.
14. **Footnotes and endnotes were missing.** They live in `word/footnotes.xml`
    and `word/endnotes.xml`, not in `document.xml`, so nothing ever read them.
    They are reported as `footnote` items at the end of the flow, which is where
    Docling puts them too.
15. **Three equations in one paragraph became one item.** A paragraph of nothing
    but math was emitted as a single formula, so `math-nary.docx` reported one
    equation where there are three. One `m:oMath` is one equation — on the page
    and in Docling's output — so each is now its own item, and the golden
    comparison holds us to the count.
16. **A page was published that nothing was on.** A DOCX declares a paper size in
    `w:sectPr`, so we reported a single 595×842 page with no item on it. Word
    repaginates as the text is edited and no block has a page rectangle, so the
    claim could not be supported; Docling publishes no pages for a DOCX and
    neither do we now. The declared size stays a parser-level fact.

## Regenerating

```bash
cd test-data/real
# PDFs: re-download from the URLs above.
# DOCX from Apache POI:
base=https://raw.githubusercontent.com/apache/poi/trunk/test-data/document
for f in VariousPictures.docx drawing.docx shapes-with-text.docx chartex.docx \
         headerPic.docx equation.doc two_images.doc testPictures.doc FloatingPictures.doc; do
  curl -sfL "$base/$f" -o "$f"
done
# DOCX from LibreOffice:
base=https://raw.githubusercontent.com/LibreOffice/core/master/sw/qa/extras/ooxmlexport/data
for f in math-mso2k7.docx math-subscripts.docx math-matrix.docx math-rad.docx \
         math-nary.docx math-accents.docx math-lim.docx math-placeholders.docx \
         math-bold-italic.docx math-overstrike.docx math-vertical_stacks.docx \
         math-escaping.docx math-literal.docx EquationWithAboveAndBelowCaption.docx \
         EquationAsScientificNumbering.docx floatingtbl_with_formula.docx mathtype.docx; do
  curl -sfL "$base/$f" -o "$f"
done
```

Then run `cargo test --workspace` and confirm the `real_*` suites still pass.
The golden references for the new file are generated separately (see
`test-data/golden/README.md`); without one, the corpus document is still held to
the Docling schema, it is just not compared against Docling's reading of it.
A `.docx` that is really an HTML error page (a 4 KB non-ZIP) fails the ZIP
open — check new fixtures with
`python3 -c "import zipfile;zipfile.ZipFile('file.docx')"`.

## Licensing

All files are publicly distributed and used here only as local test inputs:
arXiv papers (CC-BY), Mozilla pdf.js test files (Apache-2.0), IRS form (US
government work, public domain), W3C test file, calibre demo document,
Apache POI test data (Apache-2.0), LibreOffice test data (MPL-2.0).
