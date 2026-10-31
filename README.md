# PDF & Office Document Extractor

A Rust document extractor that converts PDF, DOCX/DOC, XLSX, PPTX, legacy
`.ppt`, plain images (PNG, JPEG, TIFF, BMP, WebP, GIF, JPEG 2000 in its JP2
container or as a bare codestream named `.j2c`, `.j2k`, `.jpc` or `.jpf`), RTF,
HTML,
Markdown, plain text and CSV/TSV into
[Docling](https://github.com/docling-project/docling)-compatible JSON, and — with
`--to md`, `--to html`, `--to text`, `--to yaml` — into Markdown, a standalone HTML
page, plain text, or the document model as YAML, all rendered from the same
document tree. Formats nobody can read from a specification — the Microsoft Works
family, WordPerfect, legacy `.xls` and legacy `.ppt` — are read through
**LibreOffice** when it is installed, and the result is the same document tree.

**Approach:** hybrid. Commodity infrastructure is reused (DEFLATE, ZIP, XML, image
codecs, GPU API); the domain logic — PDF object model, content-stream interpretation,
layout analysis, Docling export — is implemented here.

## Status

| Area | State |
|---|---|
| PDF text extraction | ✅ works — Flate/ASCII85/LZW/RunLength/ASCIIHex, object streams, linearized files, font encodings (cp1252, `/Differences`, `/ToUnicode`, Identity-H, **and the encoding an embedded Type 1 program carries itself**, which is where a TeX symbol font keeps `∗`/`†`/`‡`), **real glyph advances from `/Widths`** |
| PDF pictures | ✅ raster images extracted with their true placement box; **vector-only figures** detected for papers that contain no image at all |
| **OCR for scanned pages** | ✅ works (Tesseract, wired into the pipeline) — languages chosen with `--ocr-lang`, page segmentation with `--psm`, and a page's own **`/Rotate` honoured**: the bitmap is turned with the page before recognition, so a scan captured sideways comes back upright, with its boxes in the displayed page's space. A scan with **nothing saying which way up it is** — no `/Rotate`, no EXIF tag, merely pixels on their side — is **found and turned upright before recognition**: Tesseract reads the page in its stored, half and (when the stored reading is weak) quarter turns, and the page is turned only when the other reading is clearly better |
| **Plain images** | ✅ works — one image is one page, in PNG, JPEG, TIFF, BMP, WebP, GIF or JPEG 2000. The page is the upright pixel grid at the resolution the file declares (PNG `pHYs`, JPEG JFIF/EXIF, TIFF `XResolution`, BMP pixels per metre; 72 dpi — one pixel per point — when it declares none, which is what a WebP gets, having no resolution field at all, and what a GIF gets, its descriptor stating a pixel aspect ratio rather than a density; a JP2 states its own in a capture-resolution box (`resc`, in samples per metre), and one that states none gets the same default; a bare JPEG 2000 codestream (`.j2c`, `.j2k`, `.jpc`, `.jpf`) has no container to hold one, so its page is the grid its `SIZ` marker states at that same 72 dpi default — and which spelling a JPEG 2000 is comes from its first bytes, a signature box or the codestream's start marker, so a `.jpf` holding a container is placed by the container's boxes), and `origin.mimetype` is the image's own type rather than a PDF's. A **GIF** or an **animated WebP** is a page per frame: the logical screen its descriptor states, or the `ANIM` canvas a WebP states, is the page's size, and every frame of an animation is a page a reader turns — a hundred-frame animation is a hundred pages, each frame decoded by this reader (composited on the screen the way a player shows it, and laid on white where it leaves the screen untouched) and recognised on its own. A **TIFF** is a page per image file directory, which is how a scanner writes a batch: a three-sheet scan is three pages, each measured from its own grid at the file's declared resolution — the sheets need not be the same size — turned upright by its own directory's orientation tag, so one sideways sheet does not turn the batch with it, and recognised on its own pixels. A camera's **EXIF orientation** is honoured: the page is the grid the tag describes and the pixels are turned before recognition, so a phone held sideways is read upright. A file with **no tag at all** is read to find its own orientation: a sideways or upside-down page is detected from the pixels and turned upright before recognition, and the page size and every line box are measured on the upright grid. An image has no text layer, so the page goes straight into OCR: Tesseract's lines are mapped from pixels into points and grouped by the same layout pass a scanned PDF uses. **A photographed-page corpus scores the reading** — eight synthetic photos (skew, uneven light, softness, one tagged sideways, one sideways and one upside down with no tag) with ground truth, scored by character and word accuracy and held to floors: see [OCR quality](#ocr-quality-on-photographed-pages) |
| **DOCX** | ✅ works — paragraphs, headings, lists with their real bullet/number and level (`numbering.xml`, incl. `w:lvlOverride` restarts and relabels), table grids, embedded images (incl. headers/footers, and a picture inside a table cell, which the cell holds: the cell becomes a Docling `RichTableCell` referencing a group under the table, and the group's picture is the cell's rather than an item standing beside the table, and a table written inside a cell, which is that cell's content the same way — read inside the outer cell instead of as a table of the body beside it) — a PNG or a JPEG arriving as itself, and every other raster encoding a package can carry (BMP, TIFF, GIF, WebP, JPEG 2000) decoded and written out as PNG, which is the one type Docling's own Word reader ever leaves a picture in, an SVG **drawn into a PNG** at the frame the picture is placed in, because a drawing is not an encoding anyone downstream can show — fitted by the drawing's own `preserveAspectRatio` (by default `xMidYMid meet`, so a frame of other proportions keeps the drawing's shape with transparent margins rather than squashing it; `none` stretches, `slice` covers and crops, and every alignment is honoured) — and a picture Word states as an SVG beside a PNG fallback is taken from its `asvg:svgBlip` drawing, not the raster — a shape whose fill is a picture is that picture too, the drawing when its fill states one — **picture bullets** (`w:numPicBullet` named by `w:lvlPicBulletId`) are pictures of their own, ahead of the item they open and at the frame the list draws them in, the drawing played when the bullet states an SVG, and the item's words carry no glyph — with their **EXIF orientation** applied, **Office Math equations** (one item per equation), **chart titles as captions**, footnotes and endnotes, tabs, **vector EMF/WMF drawings played into PNGs** (Word's own metafiles and the previews of embedded objects), and **shapes painted into PNGs** — an autoshape, a group of them, a WordArt or a `wps:wsp` becomes a picture at the size the file gives it, whether the file states it in VML (`w:pict`) or in DrawingML (`w:drawing`), where the drawing has no image part behind it; a shape's colours come from the package's theme where it names a theme slot, its gradient fill is ramped across it (VML's `v:fill type="gradient"` or DrawingML's `a:gradFill`), its shadow lands under it, and a line's arrowheads are drawn at its ends |
| **XLSX** | ✅ works — a worksheet is a page whose units are cells, the shape Docling's own XLSX backend produces: a `sheet` group per worksheet holding the worksheet's tables (one per connected block of cells, an empty row or column separating two), **merged ranges** as `row_span`/`col_span` with the covered slots not written at all, shared and inline strings, numbers, booleans, cached formula results, and **dates as the text a reader sees** (a serial with a date format is `2024-03-05 00:00:00`, not `45356`) — plus the **pictures the sheet draws**: each worksheet's own drawing part is followed through its relationships to the media parts, and every picture is placed on the cells its anchor names and written beside the JSON under the same rule a Word or PowerPoint package's is (a PNG or a JPEG as itself, anything else re-encoded as PNG), and **the charts the sheet draws**: a chart's frame names a chart part, not a media part, so each chart is read as the data it plots — its family, the categories and series numbers, the title as a caption and no image — through the same reader a deck's charts go through, with a reference that carries no cache resolved against the workbook's own cells. A hidden worksheet stays on the `invisible` content layer |
| **PPTX** | ✅ works — a slide is a page, measured in EMU at the deck's own size (`p:sldSz`), the shape Docling's own PPTX backend produces: a `chapter` group per slide holding the slide's items, each with the box its shape occupies — including **the shapes PowerPoint paints itself**: an autoshape, a connector or a group has no image part behind it, so it is drawn into a PNG at its own box, in the fill, line and theme colour the slide states (a text box with no fill and no line puts no ink down and is no picture, and a group is one picture rather than one per shape). The presets are the format's own library rather than a handful of boxes: the rounded and snipped rectangles, rings and frames, flowchart symbols, cylinders and cubes, arcs and brackets, and the bent and curved connectors are drawn from their stated geometry, with a **gradient fill** ramped across the shape, a **soft shadow** under it and an **arrowhead** of each kind at a line's ends. A text body is read paragraph by paragraph with its **runs** (bold, italic, underline, size), and a paragraph's bullet or number is resolved the way PowerPoint resolves it — paragraph, then shape, then layout, then master — so a body placeholder's lines are list items without anything being written on the slide, and an auto-numbered list counts itself. `graphicFrame` tables keep `gridSpan`/`rowSpan` with the covered slots not written at all, and a picture is the media part its slide's **relationship** names (not a guessed `image1.png`), written beside the JSON with its **EXIF orientation** applied, as PNG unless it is already a PNG or a JPEG — a BMP, a TIFF, a GIF, a WebP or a JPEG 2000 is decoded and re-encoded, and an SVG is drawn into a PNG at its frame — fitted by the drawing's own `preserveAspectRatio`, so the default `xMidYMid meet` letterboxes a frame of other proportions instead of stretching it, `none` stretches it, `slice` covers and crops it, and any alignment pins it — the `asvg:svgBlip` drawing when the picture carries one, which is how PowerPoint 2016 and later state it, rather than the PNG fallback beside it — and a shape **filled** with a picture is that picture too, at the shape's own frame, the drawing when its fill states one. A paragraph's **picture bullet** (`a:buBlip`) is a picture too: a square of the item's type size at the corner its words start from — a slide states no line boxes — with the drawing played when the bullet states one. A slide's **picture background** (`p:cSld/p:bg/p:bgPr/a:blipFill`) is not a shape in the tree and not a picture item: it is the page's `image`, written beside the JSON under the slide's name and covering the page, so a photographic backdrop is not lost with the shape tree. A hidden slide stays on the `invisible` content layer. A deck's own remarks are read too, on the **notes** layer rather than the slide's: its **speaker notes**, from the notes part the slide's relationship names (the body placeholder, read the way python-pptx reads a text frame), and its **comments**, each in a `comment_section` group of its own with the author, time and words the file records — the time read at PowerPoint's millisecond precision, so a bridged legacy deck and its native original state the same remark. A **chart** is read as the data it is: the chart part's caches are reconstructed into the picture's `tabular_chart` — series across the header row, categories down the first column, an omitted point left as a gap — classified by its family (`bar_chart`, `line_chart`, `pie_chart`, `scatter_chart`, `other_chart`), with the chart's title as its `caption` and no image, because a chart is a drawing of data rather than a picture file |
| **Microsoft Works, WordPerfect, legacy Excel, legacy PowerPoint** (`.wps`, `.wp`, `.wks`, `.wdb`, `.wpd`, `.xls`, `.ppt`) | ✅ works when **LibreOffice** is installed — there is no published specification for the Works formats, so LibreOffice's own importer (libwps, libwpd) reads them and this tool reads what it writes (`.docx`/`.xlsx`/`.csv`/`.pptx`) with its own readers. A legacy `.ppt` is a compound file whose record stream nothing here reads; Impress writes the package the PPTX reader reads, so a legacy deck comes out as **slides, text, tables, pictures, speaker notes and comments**, with its media extracted beside the JSON like a native deck's and its comments saying exactly what the native deck's do (Impress writes their times with more fractional digits; the notes text does not notice). The fixture is the same deck as the native `q3_review.pptx`, and the test holds the **whole converted document** to the native one — pages, groups, every item, label, layer, word, table cell and picture, with only the package's own identity, Impress's re-laid-out boxes and its comment ids excluded, each for a stated reason. A second, one-slide pair (`comment_times.pptx` and `comment_times.ppt`) gives its comments **non-zero milliseconds**, so the notes layer's cut of a comment's time to PowerPoint's three fractional digits is proven through the bridge — the converted deck spells `09:15:12.125000000`, the native one `09:15:12.125`, and both must read the same — and that pair is held to the **same whole-document comparison**, by the same view: it is the second deck carried across the bridge field by field, so a difference the millisecond case would not notice (a title, a paragraph, a layer) fails there too. Either document keeps the **original file's name, MIME type and hash**: the conversion is how its words were reached, not what they are. A legacy `.xls` is bridged the same way, into the XLSX reader, and its pair (`ledger.xls` and `ledger.xlsx`, one workbook written as both) is held to the **same whole-document comparison**: every sheet, its layer, every table, cell, row and column span, cached formula result and date has to agree, with only the package's identity excluded — a workbook's geometry *is* its content, so the view removes nothing else. The hidden sheet crosses on the `invisible` layer, the merged ranges keep their spans with the slots they cover absent, and the formula's cached `15` is what a reader sees on both sides. Measured on libwps' own sample corpus: **38/40 Writer files, 24/26 spreadsheets, 2/2 databases**. Without LibreOffice the run stops with a message naming it and offering the re-save route |
| **Format detection by content** | ✅ works — the bytes decide which reader runs; the extension is only a promise the run accepts. A DOCX called `report.wps` or a Word 97 file called `letter.wps` is read as what it is, and a Works file under any name is recognised as Works. `.wps` alone names three unrelated formats, so this is the only honest way to read it |
| **Layout analysis** | ✅ works — rows and runs, column gutters, reading order, paragraph merging, `title`/`section_header`/`list_item`/`caption` labels, rotated margin stamps kept in their own box, **PDF table detection**, pictures placed in reading order |
| **Page furniture and footnotes** | ✅ works — the running heads, footers and page numbers a document repeats across pages are labelled `page_header`/`page_footer` and moved to the furniture layer, and a **note at the foot of a page** is labelled `footnote` by the two marks that make one: the **rule** the page drew above it and the **marker** it opens with, which the body of that page also uses. The page's own rules are given to layout analysis as well, which is what keeps a note block out of the table list: a marker-and-text block at the foot *under a rule* is the page's notes, not a two-column grid. The encoding a TeX symbol font carries in its own program is read too, so a marker is the character the page drew rather than the Latin-1 byte its code lands on. Measured against Docling: of the 25 notes the corpus's five PDFs carry, **23 come out labelled** (all five of `arxiv_attention`'s, all five of `math_layernorm`'s, all nine of `math_seq2seq`'s, all four of `math_gan`'s); the other two are read as a paragraph because their pages draw no rule at all, and **not one is missing**. See `progress.md` |
| Docling JSON export | ✅ pages, text items, tables, pictures, provenance bboxes, furniture, body tree |
| **Markdown export** (`--to md`) | ✅ works — the same tree rendered as Markdown: headings at their depth, list markers kept, GFM tables (merged cells flattened), formulas as `$$` blocks, pictures referenced beside the document (or carried in the page as a base64 `data:` URI, or left as `<!-- image -->` where one stood, with `--image-export-mode`, that mark read back as the picture it stood for), `hyperlink`s as `[text](url)`, an ordinary block quote as `> …`, furniture omitted — and the **notes layer omitted unless `--include-notes`**, which puts a slide's notes after its content and a deck's comments beside the slide they discuss, a speaker note's paragraph as a blockquote line headed `**Notes:**` and a comment's as one headed `**Comment:**`, so remarks are never mistaken for slides, a reviewer's comment is never mistaken for the presenter, and paragraph breaks survive. A slide's `chapter` group, a comment's `comment_section` group and a workbook's `sheet` group are marked with HTML comments that render as nothing (`<!-- pdfextractor:group chapter body slide-0 -->`, `<!-- pdfextractor:group sheet body Inventory -->`), so reading the file back rebuilds the grouping — each slide's words and notes inside its own group, each comment in its own section, each worksheet's tables under their sheet, names and layers intact — instead of leaving one flat list. Golden-tested byte for byte; `pictures.md`'s picture is pinned in the two modes the goldens do not use — `embedded` carries the JPEG's own bytes, decoded from the page and compared with `test-data/images/scan.jpg` byte for byte (a re-encode would still decode, so the bytes are the assertion), and `placeholder` writes `<!-- image -->` where the picture stood, between the same two sentences; the reader reads those links back into `hyperlink`, those pictures back into picture items (a local source — or an embedded `data:` URI, which is decoded instead — is copied beside the output), those `> **Notes:**` blockquotes back onto the **notes** layer rather than into the body — so a deck's remarks stay remarks instead of becoming slides on the next pass, and a remark's own `[words](url)` comes back as its `hyperlink` rather than as brackets — and reads `$$` blocks back as formula items — both the fenced form and a formula written on one line (`$$E = mc^2$$`) — and headings the inverse of the writer the way Docling's Markdown backend does, so a round trip keeps them all — every committed golden is generated with `--include-notes`, re-read with the notes layer, and must re-render byte for byte under `every_markdown_golden_is_a_fixed_point` |
| **HTML / text / YAML export** (`--to html` / `--to text` / `--to yaml`) | ✅ works — one shared walk, three more readings: HTML keeps merged cells as native `rowspan`/`colspan` where Markdown must flatten, links as `<a href>` and a block quote as `<blockquote>`, text is reading order with line-end hyphens joined (`--text-dehyphenate false` is the off switch), YAML is the whole document model listed in reading order. The notes layer joins HTML and text with `--include-notes`, as it does Markdown — HTML marks a speaker note as `<aside class="notes">` with a bold `Notes:` label and a comment as `<aside class="notes comment">` with `Comment:`, text leaves both unmarked, and reading the HTML back puts those asides on the **notes** layer again, keeping the comment's kind (with an `<a href>` around a remark's first paragraph kept as the note's link) and an `<img>` back into a picture item — a `<picture>`'s `<source srcset>` or an `img`'s own `srcset` choosing its first *usable* candidate before falling back to `src` — a source whose `media` query asks about a viewport, colour scheme or pointer a document does not have, or whose `type` names a format it cannot carry (an AVIF, a non-image), is passed over for the next candidate — its file, or its own embedded `data:` URI, read back into the picture and copied like a Markdown one, and a bare `<!-- image -->` back into a picture the page marked but did not carry; YAML always carries it. Groups use the same invisible `<!-- pdfextractor:group … -->` markers as Markdown. Formulas are typeset as MathML — the LaTeX as a `<math>` the browser draws, with its own TeX in a `<semantics>`/`<annotation encoding="application/x-tex">` — and a formula the LaTeX reader cannot set is drawn instead: the writer lays the equation out itself and carries it as an SVG `data:` URI whose `alt` text is that LaTeX, so the equation is seen and still reads back as the formula item it came from. The reader is the writer's inverse for the rest of the same constructs — `h1` back to `title`, `h2` to a level-1 header, `<pre><code>` to `code`, an equation back out of its MathML annotation (a `<math>` with none at all falls back to its tokens), `rowspan`/`colspan` back into placed cells — one spanning cell over the slots it covers, not an empty slot beside it — and a printed list marker `<ol>` cannot spell (`1.1.`) kept in `data-marker` — and `every_golden_round_trips_through_html_with_notes` holds it there: each golden document is written as HTML with its notes, read back, and must render the same Markdown written directly from the document (pictures' run-specific file names and HTML's collapsed cell spacing excepted), then settle when that Markdown is read again. Each golden-tested byte for byte. HTML has a byte fixed point of its own as well: `every_html_golden_is_a_fixed_point` reads every committed HTML golden back with the real binary and requires the page it re-renders to be **its own bytes**, which is what makes two page-level facts true rather than merely claimed — a flow page's own picture keeps the path the page gave it (`images/scan.jpg` stays `images/scan.jpg`, so the page written from the document names the file it came with, and a name holding a space or a character HTML escapes — `&`, a quote, `<` — is written escaped in the attribute and read back whole), and a table cell is written with every run of whitespace made one space, the one spacing a page can hold and a reading give back. Text is held to the same fixed point Markdown is: `every_text_golden_is_a_fixed_point` reads every committed text golden back with the real binary, renders it again, and requires its own bytes, twice over. YAML's fixed point is the model's rather than a reading's: `every_yaml_golden_is_a_fixed_point` parses each YAML golden into a `DoclingDocument` and renders it again, which is where a key out of order, a `self_ref` written twice or a `pages` map in another order would show. Pictures follow the same `--image-export-mode` choice as Markdown |
| **Docling compatibility (golden tests)** | ✅ **verified against real Docling**: every corpus document — and the fixtures beside it — passes `docling-core`'s own model validation and tree check, and agrees with a real Docling conversion of **all 44** (37 corpus documents and the XLSX, PPTX, image, RTF, two HTML and nested-table DOCX fixtures) |
| **Box overlay preview** (`--preview`) | ✅ works — HTML page render with every text/OCR/figure box and every **layout block** drawn on it |
| CLI (file/dir, parallel, output rules, `--ocr`, `--preview`) | ✅ works |
| **Legacy `.doc`** (OLE compound file) | ✅ works — read through **LibreOffice** when it is installed, the way the Works family is: Writer reads the binary document and writes the package the DOCX reader reads whole, so headings, list markers, tables with their merges and pictures all come through — including the VML copy of a shape or a WordArt, painted when its DrawingML carries no picture — under the original file's name, `application/msword` and hash. Without LibreOffice the built-in reader still opens the file — container, Fib and piece table read directly; paragraphs, page breaks, pictures as placeholders; field instructions dropped. The fixture is a pair (`legacy_brief.doc` and `brief.docx`, by `make_legacy_doc.py`) and the test holds the **whole converted document** to the native one — every item, label, marker, level, word, link, table cell, span and picture, with only the container's identity and the image directory excluded — so the Writer side of the bridge is gated the way the deck and workbook sides are |
| **RTF / HTML / Markdown / text / CSV** | ✅ works — RTF control groups and **`\pict` pictures** (the blip's hex payload is the image: PNG and JPEG carried as their own bytes, a metafile played into a PNG at the size the document asks for, an old Word `\wbitmap`/`\dibitmap` DIB wrapped in the BMP file header RTF leaves out so the picture can actually be opened, and Word's `\nonshppict` fallback not counted twice), a **`\trowd` table rebuilt as a grid** — its `\cellx` boundaries are the columns, so rows line up even when a writer saves a merged cell as one wide cell, with `\clmgf`/`\clmrg` and `\clvmgf`/`\clvmrg` becoming column and row spans, and `\trhdr` naming the row that heads the columns — a table that marks none keeps its first row — and **headings from the stylesheet**: a paragraph under a style whose `\outlinelevel` (or `Heading N` name) says heading is a `section_header` at that depth, Word's `Title` style is the document's `title`, and everything else stays a paragraph. HTML block structure — and a page's head **`<title>`** is the document's title when the body has no `h1`, so a title that lives only in the head is read as one and written back rather than emptied; a body `h1` wins and the head's is dropped — and the head's **description** (`<meta name="description">`), **canonical URL** (`<link rel="canonical">`) and **Open Graph and Twitter card statements** (`og:` keys in `property`, `twitter:` keys in `name`, kept in the order the head stated them, because an `og:image:width` measures the image before it) are kept too, beside the model rather than in it, because Docling's schema has no field for any of them: the page they came from writes them all back into its head, Markdown writes them as YAML front matter, while the JSON and YAML never name them — Markdown headings (read at Docling's depths: `#` is the title, `##` a level-1 section header)/lists/code/tables — a table's merged cells kept as the spans their `rowspan`/`colspan` declare, one cell over the slots it covers, the reading the JSON model and the HTML writer both hold — and its **`<th>` rows kept as the header they declare**: a row of `<th>`s heads the columns wherever it sits, so a table whose caption row comes first still heads its columns in the `<th>` row that follows, while `scope="row"` and a `<th>` opening a row of `<td>`s label their own row instead — with **inline `[label](url)` links read into `hyperlink`** (angle destinations and titles included), **`![alt](source)` images and HTML `<img>` elements read into picture items** — including responsive markup, where a `<picture>`'s first `<source srcset>` candidate that its `media` and `type` leave usable is taken ahead of the `img`'s own `srcset`, with `src` as the fallback — a feature query (`(min-width: 800px)`, `(prefers-color-scheme: dark)`) cannot hold for a document with no viewport, while a bare media type (`all`, `print`) asks nothing of it, and a `type` naming a format this cannot carry (`image/avif`, or a non-image) is skipped the way a browser skips one it cannot show — the source loaded from beside the document and copied beside the output, or, when the page was written embedded, its `data:` URI's own bytes decoded back into the picture (a `data:` URI in a `srcset` is one URL: the comma joining its payload does not start another candidate) — and a quoted `**Notes:**` line read back onto the **notes** layer, RFC 4180 CSV, all decoded to blocks. Plain text states one boundary — the blank line — so a run of non-empty lines is one block: a single line is a paragraph, a longer run is kept verbatim as a `code` item (the text writer folds a paragraph's own lines onto one, so words whose breaks were kept can only be content), and a run opening with the writer's pipe grid is the table it is — which is what makes text in, text out a fixed point |
| DOCX equations as LaTeX | ✅ works — Office Math is rendered in Docling's notation: `\frac{a}{b}`, `x^{2}`, `\sqrt[3]{x+1}`, `\sum_{k=0}^{n}`, `\left(…\right)`, `\begin{matrix}…\end{matrix}`, accents as `\acute{a}`; formula items match Docling's own conversion of every DOCX fixture carrying Office Math, up to whitespace |
| Kingsoft **WPS Office legacy** files (`.wps`, `.et`, `.dps` written before 2005) | ⚠️ not read — no public specification, no public sample, and LibreOffice's Works filter does not accept them. WPS Office's current `.docx`/`.xlsx`/`.pptx` output is read normally, whatever the file is called |
| Legacy `.doc` styles, tables, raster images | ⚠️ without LibreOffice, the built-in reader needs character formatting (`PAPX`/`CHPX`), which it does not read: no headings, tables flatten to tabs, images are placeholders. With LibreOffice the bridge carries all of them, so this is the no-LibreOffice reading only |
| Flow formats (DOCX/RTF/HTML/`.doc`) page geometry | ⚠️ none — no page is reported at all (Word repaginates; Docling publishes none either); `--preview` is not produced |
| Images beyond the page | ⚠️ an image's text is OCR — `--ocr never` writes the pages with no text on them. `--preview` is not produced for an image, and a later GIF frame or TIFF sheet whose colour type this reader cannot convert is left out rather than guessed at |
| XLSX beyond cells | ⚠️ cells, merges, dates, **pictures** and **charts** (read as the data they plot) — cell comments are not read, a vector metafile picture (EMF/WMF) is placed with its box and no image, and a merged label row above a header is kept in the table instead of split out as a text item (Docling splits it) |
| PPTX beyond slides | ⚠️ slides, text bodies, tables, pictures, **charts** (their plotted data, family and title) and **the shapes a slide paints itself** (an autoshape, a connector or a group of them, drawn into a PNG at the shape's own box with the fill — including a **picture fill**, painted through the shape's outline — line, **gradient**, **soft shadow** and **arrowhead** the slide states and its theme colour — patterns, 3-D and a shape's *text* are not drawn, and a text box with no fill and no line puts no ink down and is no picture), plus **speaker notes and comments on the notes layer** — the rest is not read: **SmartArt and equations**, a comment's *anchor* (the position PowerPoint stores, which names no shape and has nowhere to go in the document model), a linked (not embedded) picture has no bytes to show, and a vector metafile (EMF/WMF) picture is placed with its box and no image; PNG, JPEG, GIF, BMP, TIFF, WebP and SVG are extracted (every picture that is not a PNG or a JPEG is written out as PNG, and an SVG is drawn into one at the frame the picture is placed in, fitted by the drawing's own `preserveAspectRatio` — letterboxed by default when that frame's proportions differ, and stretched, cropped or cornered when the drawing asks for it). Legacy `.ppt` has no reader here: it needs LibreOffice, and the deck it hands back is only as faithful as Impress' own conversion — a text box Impress re-lays out is the re-laid-out box |
| **Compare mode** (UI) | ✅ works — a reference (a file, or a folder of them — `test-data/golden/docling/`) puts every converted document beside Docling's reading of the same one: what agrees, what differs by label or wording, what only one side has, equations marked as notation, **what the reference cut finer than we did** (a reference item wholly inside one of our blocks — by its words, or, for an item with no letters in it at all, by lying among the words of a line we hold — drawn as the run it is, with the items a broken run leaves behind taken onto the row of the block that holds them, leaders and all), and the share of the reference's characters we also hold. An item with no letters in it that nothing of ours answers is **placed** — named, one line of a form per row, and **drawn page by page**: each page that carries places is drawn once above the rows with all of its items at once, so a form's leader pattern is one picture rather than one thumbnail per row. Rows content cannot pair are joined by **page and box**, so a drawing the two readings ordered differently is one row rather than one a side — where the producers publish boxes (PDFs), with the overlap that joined the row on the row itself |
| **UI** (`crates/ui`) | ✅ works — Tauri window with a file/folder picker and a settings panel. A conversion runs in the background and is **streamed**: the window lists every document as it finishes, shows a progress bar and the CLI's own log, and can **stop** the run at any time. Cross-platform, no JavaScript toolchain needed. |

See [progress.md](progress.md) for the verified check results and known limitations,
and [plan.md](plan.md) for the full roadmap.

## Requirements

- Rust 1.75+ (verified on 1.99.0)
- **OCR (required by default):** Tesseract + Leptonica development libraries, plus
  `libclang` because the bindings use `bindgen`:

  ```bash
  apt-get install -y libtesseract-dev libleptonica-dev tesseract-ocr tesseract-ocr-eng libclang-dev
  ```

  Without these, build with `--no-default-features`; OCR calls then return a clear error.
- **LibreOffice (optional):** only for the formats no reader here implements —
  Microsoft Works (`.wps`, `.wp`, `.wks`, `.wdb`), WordPerfect (`.wpd`), legacy
  Excel (`.xls`) and legacy PowerPoint (`.ppt`). `soffice` is looked up on `PATH`;
  `PDFE_SOFFICE` names it explicitly. Every other format is read without it, and nothing is ever handed to
  it that this could read itself.
- **UI (optional):** Tauri uses the system webview. On Linux that is
  `webkit2gtk-4.1` + `libsoup-3.0` + `gtk3` (plus `libclang`, already needed for
  OCR); Windows uses WebView2 and macOS uses WKWebView. Nothing else is required
  — the frontend is plain HTML/CSS/JS, so no Node or bundler is involved.

## Releases

Every `v*` tag builds and publishes a release (`.github/workflows/release.yml`),
and the tag must name the version the binaries report — `v0.1.0` for version
`0.1.0`; the workflow compares the tag with `Cargo.toml`, `tauri.conf.json` and
the `VERSION` inside every archive, and refuses to publish otherwise. A release
carries, for Linux (x86_64), Windows (x86_64) and macOS (Apple silicon):

* `pdfextractor-cli-<tag>-<target>.tar.gz` / `.zip` — the command line with its
  README and, beside it, `tessdata/eng.traineddata`: the binary looks for OCR
  data next to itself first, so every archive runs OCR on a machine with no
  Tesseract install (Windows has none to find). The Windows archive also
  carries the Tesseract DLLs beside the binary and needs nothing else. On
  Linux and macOS the Tesseract *runtime* must be installed (`sudo apt install
  libtesseract5` on Debian/Ubuntu, `brew install tesseract` on macOS) — the
  same library the build linked.
* The window's installer: `pdf-extractor-<tag>-<target>.deb` / `.AppImage` /
  `.msi` / `.dmg`. Every bundle carries the command line built beside it —
  with its DLLs on Windows and the same `tessdata` beside it on every
  platform — so an installed window converts without a separate install; the
  deb declares `libtesseract5`/`liblept5` for apt to pull, and on macOS
  conversions need `brew install tesseract`.

`SHA256SUMS.txt` covers every file of the release. The artifacts are **not
code-signed** — signing keys are not in this repository — so Gatekeeper and
SmartScreen will ask the first time you open one. The workflow also runs by
hand (`workflow_dispatch`), which builds and packages everything but
publishes nothing; that is how a change to it is tested before a tag depends
on it.

## Build and test

```bash
cargo build --workspace      # 0 warnings
cargo test  --workspace      # 970 tests
cargo clippy --workspace --all-targets -- -D warnings   # no warnings
cargo fmt --all --check      # formatted as rustfmt reads it

# The compatibility suite, held to real Docling. The schema layer always runs;
# pointing DOCLING_PY at a Python with docling-core installed adds the layer that
# loads every document with Docling's own model.
DOCLING_PY=~/.venvs/pdfextractor-docling/bin/python \
  cargo test -p pdfextractor-cli --test docling_golden_test
```

Every push runs that clippy command in CI (`.github/workflows/clippy.yml`) with
`--locked` added, over the whole workspace and every target: a lint, or a warning
only a test target reaches, fails the build instead of scrolling past in a log.
`cargo fmt --all --check` runs beside it (`.github/workflows/fmt.yml`), so formatting
drift — a hand-formatted file, a merge that kept both sides — fails the build too
instead of arriving inside the diff of whatever change comes next. And every push runs
`cargo test --workspace --locked` (`.github/workflows/test.yml`) — the suite above with
nothing filtered out: the unit tests, the corpus report, the golden suite and the
command line's own integration tests, with the native libraries the workspace links and
the OCR data the tests recognise with installed first, and LibreOffice deliberately left
out. The suite is green without it — its bridge tests return early, the corpus report
keeps the baseline entries of the legacy documents it could not measure, the compare gate
does not hold those documents to the bridge's numbers, and the golden suite compares
those four through the built-in reader — so the gate never puts the
runner's LibreOffice build between the ratchet and the numbers the baseline was measured
with. A run here is what a fresh checkout on a machine without LibreOffice gets. The
bridge is not left untested for it: `.github/workflows/bridge.yml` runs weekly with
LibreOffice installed — the re-save tests, the corpus report, the corpus comparison,
the compare gate over a legacy `.doc`, and the Works test against libwps' corpus (which
the job fetches, since no sample can be committed), the suites whose readings change
when the bridge is there — on a pinned
`ubuntu-24.04` image, because those readings depend on the LibreOffice build that writes
them and `ubuntu-latest` would move it under the ratchet by itself. The per-push suite is
pinned to the same image for the same kind of reason: its OCR floors were recorded
against the Tesseract and Leptonica that image installs, and `ubuntu-latest` migrating to
Ubuntu 26 would swap the engine under those floors — a red run on an unchanged tree whose
likeliest repair is a lower floor. Moving the image is therefore a commit that re-measures
them. The gates that hold no number a package version can move — clippy, formatting, and
the compare gate, which measures with `--ocr never` — still run on `ubuntu-latest`, so a
package that disappears on a new image fails the push that finds it.

### Compatibility with Docling

The output is checked two ways (`test-data/golden/README.md`):

* **The schema** `docling-core` generates from its own model, committed and checked in
the Rust suite: required fields, no field an item class does not allow, legal enum
spellings, resolvable references, and a hierarchy whose children agree with their
parents. Every document the CLI writes for the whole corpus has to pass.
* **Real Docling conversions** of every corpus document and every fixture, compared by
content — page count and page size, text as a character multiset, equation counts and
notation, and no block appearing twice. Text agreement is 0.999–1.000 across all of them,
the image scan included. Every Docling formula that carries text must be reproduced as
the same LaTeX up to whitespace: all 38 equations across the 14 formula-bearing
references do.

## OCR quality on photographed pages

A clean scan says whether OCR ran; it does not say how well it reads a page that
was photographed. `test-data/photos/` is a small corpus of eight synthetic
photographs — turned up to 4°, lit unevenly, two slightly soft and smaller, one
stored sideways with EXIF orientation 6, and two with **no tag at all**: one
stored sideways and one upside down, so that finding the page's orientation from
its pixels is itself measured — each with its exact text written to
`corpus.json` by the generator. The report converts every page through the real
binary and scores the recognised text by character and word edit distance:

```bash
cargo test -p pdfextractor-cli --test ocr_quality_test -- --nocapture
```

The seven cleaner pages — including the two stored sideways, tagged or not, and
the one stored upside down — are read at **100%** characters and words; the
softest photograph (smaller body text, blur, shadow and JPEG 72) holds **95.6%**
of the characters and **87.0%** of the words, and the corpus aggregate is
**99.4% / 98.3%**. The test holds each page and the weighted corpus to recorded floors and
prints the recognised text beside the reference when one falls below, so a
change to OCR, layout or the image path is a score rather than an opinion. See
[`test-data/photos/README.md`](test-data/photos/README.md).

## Usage

```bash
# Single file; output defaults to <input>.json next to the input
cargo run -p pdfextractor-cli -- -i document.pdf

# Explicit output, 4 parallel conversions
cargo run -p pdfextractor-cli -- -i document.pdf -o out.json -p 4

# Whole directory (recursive by default); output tree mirrors the input
cargo run -p pdfextractor-cli -- -i ./docs -o ./out -p 8

# OCR a scanned PDF (default: only pages with no text layer)
cargo run -p pdfextractor-cli -- -i scan.pdf --ocr auto

# A plain image is one page, read by OCR (PNG/JPEG/TIFF/BMP/WebP/GIF/JP2/J2C); EXIF
# orientation is applied, so a photo stored sideways is read upright — and a
# page with no EXIF tag is read in all its turns to find which way up it is
cargo run -p pdfextractor-cli -- -i invoice_scan.png -o invoice_scan.json

# A legacy .ppt is bridged through LibreOffice into the PPTX reader (needs soffice)
cargo run -p pdfextractor-cli -- -i deck.ppt -o deck.json

# A legacy .xls is bridged the same way, into the XLSX reader
cargo run -p pdfextractor-cli -- -i ledger.xls -o ledger.json

# Force OCR even when a text layer exists
cargo run -p pdfextractor-cli -- -i mixed.pdf --ocr always

# Skip OCR entirely
cargo run -p pdfextractor-cli -- -i doc.pdf --ocr never

# German + English recognition, one block of text per page
cargo run -p pdfextractor-cli -- -i scan.pdf --ocr-lang deu,eng --psm 6

# Write an HTML overlay of every extracted box (see "Box overlay" below)
cargo run -p pdfextractor-cli -- -i scan.pdf -o scan.json --preview

# Top level only
cargo run -p pdfextractor-cli -- -i ./docs --recursive false

# Markdown next to the JSON
cargo run -p pdfextractor-cli -- -i document.pdf --to md

# Markdown only, written to stdout
cargo run -p pdfextractor-cli -- -i document.pdf --to md -o -

# A deck's speaker notes and comments as well as its slides
cargo run -p pdfextractor-cli -- -i deck.pptx --to md --include-notes

# One self-contained Markdown file: pictures carried inside it
cargo run -p pdfextractor-cli -- -i document.pdf --to md --image-export-mode embedded
```

| Flag | Meaning |
|---|---|
| `-i, --input <PATH>` | file or directory (required unless `--list-input-formats` is given) |
| `-o, --output <PATH>` | output file (single input) or directory (directory input); defaults to beside the input. The name `-` writes the document to stdout — one input, one format |
| `--to <FMT>` | output format, repeatable: `json` (the default), `md`, `html`, `text` or `yaml`; each format is written beside the same stem, so `--to json --to md` writes `doc.json` and `doc.md` |
| `-p, --parallel <N>` | concurrent conversions, 1–1000 (clamped to 256) |
| `-r, --recursive <BOOL>` | recurse into subdirectories (default `true`) |
| `--ocr <MODE>` | `auto` (default, scans only), `always`, or `never` |
| `--ocr-lang <LANGS>` | OCR languages as comma-separated Tesseract codes (`eng,deu`), default `eng`; the language's data must be installed, or shipped beside a released binary |
| `--psm <N>` | Tesseract page segmentation mode, 0–13 (6 is one block, 7 one line, 11 sparse text); the engine's own default when absent |
| `--text-dehyphenate <BOOL>` | join words a line break split when writing `--to text` (default `true`, as pdftotext does); `--text-dehyphenate false` is the off switch — every source character is kept, at the cost of `exam- ple` style breaks |
| `--include-notes` | include the notes layer — a deck's speaker notes and comments — in the `md`, `html` and `text` outputs. Off, those three are a reading of the slides alone; JSON and YAML always carry the whole document. Markdown writes a speaker note's paragraphs as `> **Notes:**` lines and a comment's as `> **Comment:**` lines, and HTML as an `<aside class="notes">` (or `<aside class="notes comment">`) holding one `<p>` per paragraph with a bold label on the first, so an included remark is visibly not a slide, a comment is visibly not the presenter's note, and paragraph breaks are kept; plain text keeps it unmarked. Reading such a Markdown or HTML back puts the remark on the notes layer again, paragraphs and all |
| `--image-export-mode <MODE>` | how the `md` and `html` outputs treat a picture: `referenced` (the default) points at the image files written beside the document, `embedded` carries each picture's bytes in the page as a base64 `data:` URI (under the image's own media type), and `placeholder` writes `<!-- image -->` where each picture stood — the same three names Docling's flag uses. A picture with no file behind it — a chart, vector art, an unnameable media type — has nothing to point at, so `referenced` writes nothing at all for it rather than an image link to nothing, and `embedded` marks the spot, having no bytes to carry; `placeholder` marks it like any other. An `embedded` page reads back as the picture it carried: the URI is decoded with no network access, so the bytes survive a second pass instead of becoming the literal link. A `placeholder` page reads back too — the mark is the picture, an item the model holds with no image behind it, exactly what the page said — so a page written that way re-renders with its pictures in place instead of turning them into the words `<!-- image -->`, and read in the mode it was written in it is its own bytes. `chart_deck.pptx` is the fixture whose only picture is such a drawing, and it is in the goldens so the writers' behaviour for one is pinned. JSON and YAML always keep the extract's own image references, and the image files are written beside the document either way, because the extraction is shared with those formats |
| `--preview` | also write `<output>.preview.html`, a page overlay of every extracted box |
| `--preview-dpi <N>` | resolution of the preview's page backdrop (default 110) |
| `--progress` | stream one JSON record per finished document to stdout (see below) |
| `--compare <PATH>` | compare every finished document with a Docling reference (a file, or a folder of them) and exit non-zero if any falls below the floor its memory remembers (see "Compare mode") |
| `--list-input-formats` | print the input formats this build accepts as JSON — `{"input_formats": [".pdf", …], "families": [{"name": "PDF", "input_formats": [".pdf"]}, …]}` — and exit, needing no input path. The list is the one the gate itself reads, which `--help` (grouped by family) and the empty-folder refusal also print, so a window or a script reads it instead of parsing help text. `families` is the same set under the headings `--help` uses, one family per heading in the help's order and every name in exactly one of them, so a UI can build a grouped picker without knowing what a group is called — which is what the desktop window does, grouping its formats line under those headings and offering one **File…** filter per family after **All documents** |

Log records go to **stderr**; stdout is left for the output of the run itself, so
`--progress` records can be read a line at a time with nothing to filter out. Set
`RUST_LOG=info` for the log.

One environment variable matters: **`PDFE_SOFFICE`** names the LibreOffice binary
for the bridge, for a machine where it is installed somewhere `PATH` does not reach
(and for a test that must prove a file needed no conversion). `-i old.wps` on a
machine without LibreOffice fails with the reason and the alternative, never with a
parse error from a format it cannot read.

### Streaming progress

`--progress` turns a run into a stream a parent process can follow, which is what the
desktop window renders. A plan line lists the whole queue before any work starts, then
each document is reported **the moment it finishes** rather than being collected and
printed at the end:

```jsonc
{"kind":"plan","total":2,"inputs":["/docs/a.pdf","/docs/b.docx"]}
{"kind":"progress","done":1,"total":2,"input":"/docs/a.pdf","ok":true,"output":"/out/a.json"}
{"kind":"progress","done":2,"total":2,"input":"/docs/b.docx","ok":false,"error":"not a DOCX (ZIP) package: ..."}
```

Every line is a record, `done` counts up in the order documents finish, and a failure is
reported as a record rather than as silence. Without the flag the same news goes to the
log (`tracing` for successes, stderr for failures) and stdout stays empty.

Output files are written through a temporary sibling and renamed into place, so
stopping a run — or the machine losing power during one — cannot leave a half-written
`*.json` that still looks like a document.

## UI

A small Tauri window wraps the same pipeline: pick one file or one folder, set
the options, press **Convert**. It does not reimplement anything — it builds the
argument vector, runs the `pdfextractor-cli` binary with `--progress`, and draws
the stream it gets back.

```bash
cargo run -p pdfextractor-ui
```

A conversion is **never waited on**, so the window stays live throughout:

- the queue appears as soon as the run starts, one row per document;
- each row turns *converted* or *failed* as its document finishes, with the
  failure's reason attached;
- a progress bar, a document count, and the CLI's log (colour escapes removed)
  sit under it, alongside the exact command line being run;
- **Stop** kills the process at any point. Documents already written are kept —
  each one is written through a temporary file and renamed, so a stopped run
  leaves whole files or none. Closing the window stops the run too.

The frontend is plain HTML/CSS/JS in `crates/ui/dist`, loaded by the system
webview, so there is no Node build step. `dist/index.html?mock=1` loads a small
stand-in for the Tauri API (`dist/mock.js`, shipped but never referenced by the
app) that plays a scripted run in an ordinary browser, which is how the page can
be worked on without rebuilding the app. The window's permissions live in
`crates/ui/capabilities/default.json`; `core:event:allow-listen` there is what
lets the page subscribe to the stream at all.

The CLI is looked for beside the UI binary; set `PDFEXTRACTOR_CLI=/path/to/pdfextractor-cli`
to point somewhere else.

The window does not carry a list of the formats it accepts. On load it asks the
CLI — `pdfextractor-cli --list-input-formats`, the same JSON list `--help` names
and the gate enforces — prints the answer under the input box
(`Reads 33 format(s) in 7 families`) and groups it there under the same headings
as the native **File…** chooser: *PDF* with `.pdf`, *Word* with its extensions,
and so on. Both views are built from the answer's `families`, in the help's own
order, so a reader added to the command line is offered and named by the window
without an edit on that side. The chooser adds **All documents** first and then
one filter per family, so a pick that knows what it wants is not scrolling one
list of thirty-three extensions. The page's grouped list is folded under a
*Formats by family* disclosure, so the form stays short — the line above already
carries the count — and the families are there to open when a file is turned away
rather than something to scroll past before choosing one. It appears only once
the CLI has answered (a failure shows no empty disclosure), and a re-ask leaves
it open or closed as the person left it. The grouping is the CLI's own; the
window invents no family name. A CLI older than the families prints only the flat
list, and everything is shown and offered under one *Documents* heading rather
than the window refusing what a run takes. A CLI that cannot be asked says why on that
same line, clears the old families with it, and leaves the chooser unfiltered,
which is picking from no list rather than from a guessed one.

The answer is kept rather than asked per click, so **Ask again** beside that line
re-asks it: a CLI rebuilt, replaced or repaired while the window is open is then
the one the line names and the chooser filters by, with no reopening. The refresh
is one command (`refresh_input_formats`), and a failed ask is re-askable the same
way. It also re-resolves `PDFEXTRACTOR_CLI`, so pointing the variable at a new
binary takes effect on the next refresh — and on the next run, which always
resolves the binary afresh — without reopening the window. The binary itself is
named on the line under the formats (`Binary /path/to/pdfextractor-cli`, from
`cli_program`), so a moved path is visible rather than inferred from a changed
count; it is resolved with the same state the formats use and redrawn by the same
re-ask, and again when a run starts — the `conversion-started` event carries the
path that run resolved for itself, so a binary moved since the last ask is named
on the line rather than only in the command below it. `--selftest` re-asks before
it reports `formats=N`, `families="..."` and `cli="..."` — the *refresh's* count,
headings and binary, with the line then following the run's own start — so the
round trip is checked end to end with the rest of the window.

### Markdown export

`--to md` writes the same document as Markdown, beside the JSON or instead of
it (`--to md` alone writes no JSON) or on stdout with `-o -`. The rules:

- **Furniture is omitted** — a running head or page number on every page would
  interrupt the prose; it stays in the JSON. The **notes layer** is omitted the
  same way: a deck's speaker notes and comments are the deck talking about
  itself, and a reading of the slides is the slides. When `--include-notes`
  asks for them, each paragraph is a blockquote line headed `**Notes:**` — or
  `**Comment:**` when it is a reviewer's comment — so it reads as the deck
  commenting on a slide rather than as more slide, the two kinds stay apart,
  and the speaker's paragraph breaks are kept. The label is not decoration: a comment
  opens `[author: …]:`, which at the start of a line is Markdown's
  link-reference-definition syntax and would render as nothing — inside a
  blockquote as much as outside one — so every line begins with words instead,
  which is what keeps it a remark.
- **Headings** take their depth: a title is `#`, a section header sits one
  level below its own depth (`##` for a level-1 section). The reader is its
  inverse — `#` is the title, `##` a level-1 section header — which is how
  Docling's own Markdown backend reads them too, so a document written as
  Markdown and read back does not gain a level. The gate that holds every
  committed rendering to it is described below.
- **List markers are kept**: a printed `1.` starts an ordered list, a roman
  marker continues as `- i. …`, a bullet glyph becomes Markdown's own `-`.
- **Quotations stay quotations**: a paragraph the document set apart — a `>`
  line of a Markdown page, a `<blockquote>` of an HTML one — is written back as
  `> …`, not as plain prose (and the HTML export writes a `<blockquote>`
  around it). Docling has no block quote item, so the JSON and YAML report the
  paragraph Docling itself would make of it and the quoting travels beside the
  model instead of in a field Docling would reject. Consecutive quoted lines
  are one quotation and a blank line between two runs starts another, which is
  what keeps a page from changing every time it is saved; a link inside a
  quotation is still a link, split out as its own quoted paragraph the way it is
  in a paragraph.
- **Head facts become YAML front matter**: a page read from HTML keeps its
  description, canonical URL and Open Graph/Twitter cards beside the model; a
  Markdown rendering of that page starts with a YAML front matter holding
  whichever of them the page stated — `description`, `canonical`, and a `social`
  list of `key`/`content` records — so a head that is cards alone gets cards
  alone rather than an empty field invented for it. The list, rather than a
  mapping, is what keeps a repeated key and the order of an image and its width;
  the reader puts those facts back beside the document, so writing it again
  keeps the head.
  A document with no head facts gets no front matter and its bytes are unchanged.
- **Tables** are GitHub-flavoured pipe tables, padded to their columns. Merged
  cells are flattened — the spanning cell's text appears once, at its top-left
  — because Markdown has no merged cells. The row the document marks as its
  header opens the table wherever it sat, so a table whose column names lie
  under a caption row heads the names rather than an empty row; a table that
  marks no header at all gets an empty one so it still parses.
- **Formulas** become `$$ … $$` display blocks holding LaTeX in Docling's
  notation (`\frac{a}{b}`, `\sum_{k=0}^{n}`, `\sqrt[3]{x+1}`), the same
  spelling Docling's own OMML converter writes. A display block is read back as
  a formula item, so an equation survives being saved again unchanged; the
  reader accepts the fenced form and a one-line `$$…$$` block alike, and lifts
  either out of a block quote or list item into the same formula item. Inline
  `$…$` in prose stays inside its sentence: Markdown keeps the dollars, the
  HTML writer typesets the formula as MathML inside a `<span class="formula">`
  (or draws it there when the LaTeX cannot be set), and the HTML reader takes the
  TeX back out of the MathML annotation — or out of the drawing's `alt` text — and
  writes the `$…$` again. The span goes wherever the sentence does: a paragraph,
  a heading, a table cell, a list item and a quoted remark — a `**Notes:**` or
  `**Comment:**` line, which the HTML writer puts in an `<aside>` — are all
  scanned the same way, so the delimiters never reach the page. A lone or escaped
  dollar, and a price whose dollars only look paired, stay words.
- **Pictures** become `![Image](…)` pointing at the image extracted beside the
  document; a drawing with no image leaves nothing rather than a broken link.
- **Links** become `[text](url)`. The model keeps a link on the whole item — a
  PDF `/Link` annotation over the words, a DOCX `w:hyperlink` around them, an
  HTML `a` holding them — so the item's whole text is the label. An address
  Markdown would otherwise end early (a space or a bracket) is written in
  `<…>` form; code and formulas stay literal.

The output is pinned byte for byte under `test-data/golden/markdown/` for
the eighteen documents the fixtures cover — including a form, a paper, a Markdown file
read back as Markdown, **a Markdown file whose picture is a real file beside it** (the
picture is copied beside the output and referenced from there), a DOCX whose one table
merges cells (its spans flattened in Markdown, kept as `rowspan`/`colspan` in HTML —
the two readings golden-tested side by side), and a PDF whose link annotation comes out
as `[text](url)` — plus the deck whose slide and comment groups are marked in it and
**the workbook whose worksheets are**: `inventory.xlsx` pins one `sheet` group per
visible worksheet, each holding its sheet's tables, and
`a_workbook_pins_a_sheet_group_per_worksheet` reads the golden back to require the
same groups with the same tables under them. `social_cards.html` pins the front
matter in `markdown/social_cards.md`, with the description, canonical URL and every
card line in order; the fixed-point gate reads it back and requires the same bytes.
`picture_names.html` is the picture whose name has to be escaped: its file is
`images/Q3 chart & graph.gif`, the HTML golden writes
`<img src="images/Q3 chart &amp; graph.gif">`, Markdown wraps the same path in `<…>`
for the space, and the fixed-point gates re-read each spelling to the same name.
Inside that wrapper the writer also escapes `<`, `>` and a backslash, which the
angle form cannot hold otherwise; no golden carries a name with both a space and
those characters, so a unit test in the writer
(`a_picture_name_is_wrapped_and_escaped_at_once`) pins that spelling instead.
A word processor's own file is pinned as
well, in all four writers: `rtf_bitmap.rtf` is LibreOffice's export of a DOCX holding
a small bitmap, so its heading, its paragraphs and the picture it embeds are held byte
for byte in Markdown and — the same document — in HTML, text and YAML, which is where
the RTF reader meets a real exporter's font table, stylesheet and control-word soup
instead of a hand-written sample. A deliberate
change regenerates the goldens with
`UPDATE_GOLDEN_MD=1 cargo test -p pdfextractor-cli --test md_golden_test`,
and its diff is the review. A second gate, `every_markdown_golden_is_a_fixed_point`,
re-reads every committed `.md` golden with the real binary and renders it again: the
re-rendering must be the golden **byte for byte**, which is what makes the reader the
writer's inverse for every construct the goldens carry rather than one more level of
drift; a second pass proves that re-rendering is itself stable. The same gate stands
over the HTML renderings — `every_html_golden_is_a_fixed_point` re-reads every
committed `.html` golden and requires the page it re-renders to be its own bytes,
its pictures copied beside it where its own `src` resolves first — and over the text
ones. A third gate,
`every_golden_renders_embedded_to_its_referenced_pictures`, holds the pictures
instead of the words: every committed Markdown and HTML golden is rendered both
ways — `referenced`, which writes each picture beside the page, and `embedded`,
which carries its bytes in a `data:` URI — and the embedded page must carry the
referenced render's picture bytes, read back into a document that writes those
same bytes again and re-renders the embedded page unchanged — byte for byte,
`<head>` and `<title>` included, because the page's title is the document's own
rather than the name of the file it was read from. A re-encoded or dropped
picture fails it even when the count and the size are right. Every golden is
rendered `placeholder` too: that page must carry one `<!-- image -->` for every
picture the document holds — a drawing with no bytes included, which no other
mode shows — and no image syntax at all (no `_images/` path, no `data:` URI, no
`![…](…)` or `<img>`), and where every picture has a file it must be the
referenced page with each picture block swapped for the mark, so a mark cannot
land in the wrong place. Note that Docling's own CLI writes
**Markdown by default**; this one defaults to JSON, because the compare mode and
the window read JSON.

### HTML, text and YAML export

`--to html`, `--to text` and `--to yaml` are the same walk as Markdown with different
writing at its end. They combine with every other format, so
`-o doc.json --to md --to html --to text --to yaml` writes five siblings beside one
stem, and `-o -` with a single format writes that one document to stdout.

- **HTML** is a whole page — doctype, `utf-8`, the document's own first `title` item
  as its `<title>` (a document with no title gets an empty one, so the page does not
  change when it is read back under another name) — plus the page's own **description**
  and **canonical URL** when it came from a page that stated them
  (`<meta name="description" content="…">` and `<link rel="canonical" href="…">`,
  escaped as attribute values), and its **Open Graph and Twitter card statements**
  (`<meta property="og:title">`, `<meta name="twitter:card">` and every other
  `og:`/`twitter:` key), in the order the page stated them — an `og:image:width`
  stays the line after the image it measures, and a `twitter:` key goes back in
  `name` where Open Graph's goes in `property`. None of it is a block on the page and
  Docling's schema has no field for any of it, so it all travels *beside* the model
  (`DoclingDocument::html_head`) — the JSON and YAML never name it, exactly as they
  never name a quotation, while Markdown writes it as YAML front matter that reads
  back — and a page read as HTML and written as HTML keeps the head it had, byte for
  byte, while a document converted from anything else gains no statement it never
  made — and its tables are the reason it
  exists beside Markdown: a merged cell is written
  once with real `rowspan`/`colspan`, with the cells it covers *not written at all*,
  where Markdown must flatten them into empty ones. The row the document marks a
  header becomes `<thead>`/`<th>` wherever in the table it sat — the rows above it
  follow it in `<tbody>`, since a page has one place for a head — and a table that
  claims no header gets no `<thead>`, because an invented header is a claim the
  document never made. Markdown, which needs a header row before its separator, does
  the same: the marked row opens the pipe table and the rows above it follow in the
  body, so a table that names its columns under a caption row names them there,
  rather than under an empty header row. Code is
  `<pre><code>`, and **formulas are typeset**: the LaTeX the document holds becomes
  MathML inside the `<div class="formula">` (or the `<span class="formula">` an inline
  formula stands in), so a browser draws the equation without a script or a
  stylesheet, and the equation's own TeX travels beside the drawing in the MathML
  `<annotation>` — which is what the reader takes back, so a page keeps its formulas
  exactly rather than a redrawing of them. Every accent Docling writes is spelled
  the way the reader can set it before it parses — the same accent under a
  standard name (`\overbar` → `\overline`, `\ocirc` → `\mathring`, `\wideutilde`
  → `\underset{\sim}`, `\leftharpoonaccent` → `\overleftharpoon`), or the shape
  that draws it where LaTeX has no name at all (`\dddot` → `\overset{\cdots}`,
  `\ddddot` → `\overset{\cdot\cdot\cdot\cdot}`, `\vertoverlay` →
  `\overset{\mid}`, `\asteraccent` → `\overset{\ast}`, `\threeunderdot` →
  `\underset{\cdots}`, a harpoon under a letter → `\underset{\rightharpoondown}`)
  — the hook above and the candrabindu, which have no relative at all, as the
  nearest accent — while the `<annotation>` keeps Docling's spelling, so an
  Office Math equation, inline or display, is typeset rather than drawn. The
  DOCX reader names every accent Docling names, including `\annuity`,
  `\threeunderdot` and the arrows and harpoons under a letter, so such a mark
  keeps the letter it stands over. A
  formula the LaTeX reader still cannot set is **drawn**: the writer reads the
  small LaTeX subset these formulas use, lays it out on a baseline and writes a
  standalone SVG as an `<img>` `data:` URI — fractions with their rules, roots,
  scripts, accents, delimiters and matrices — with that same LaTeX as the `alt`
  text, which is where the reader takes the formula back from.
  Never a parse error where an equation belongs, and never the TeX standing in
  for the equation it describes. A picture with an
  image is an `<img>` (a drawing with no image leaves nothing rather than a broken one), a `hyperlink`
  an `<a href="…">` around the item's text, everything else `<p>` — all text and
  every address HTML-escaped. Consecutive list items become one list, `<ol>` when
  every marker printed a number, each number kept in the item's `value` so a list
  that started at `2)` still does. A `--include-notes` remark is an
  `<aside class="notes">` whose paragraph is labelled `Notes:` in bold, so
  presenter remarks sit visibly beside the slides instead of reading as slides.
- **Text** is reading order with the markup taken off: headings are their words, list
  markers the ones the document printed (`1. First`, `- i. Roman`, a bullet glyph
  collapsed to `-`), tables the same padded pipe grid as Markdown — the writer is
  shared, so the two formats can never disagree about a cell — code and formulas
  verbatim without fences or `$$`, and pictures leave nothing (a file path is not a
  reading of a drawing). Paragraph lines fold back into one, and a word broken by a
  line end is joined by default (`inter-` / `national` is `international`,
  pdftotext's rule). **`--text-dehyphenate false` is the off switch**: every source
  character is kept, at the cost of `exam- ple` style breaks. The join is a heuristic
  with no dictionary behind it — a genuine `twenty-` / `four` fuses like any other
  split — which is the case for switching it off.
- **YAML** is the whole document model, unlike the other three: pages, groups,
  furniture, the notes layer and provenance all appear, because the YAML *is* the
  document rather than a reading of it. What the walk decides is where the content sits: `texts`, `tables`
  and `pictures` are listed in the body's reading order — furniture follows the
  content instead of interleaving — with every `self_ref` and `$ref` rewritten to
  match, so the file still parses as the same document, nothing dropped. Keys are
  written in the order `--to json` writes them, so two runs of one extract are
  byte-identical.

Each format is pinned byte for byte under `test-data/golden/{html,text,yaml}/` for
the seventeen documents they cover, including `social_cards.html`: its committed HTML
golden pins the description, canonical URL and every Open Graph and Twitter card
line, escaped and in the head's order, while its text and YAML goldens pin that none
of those statements becomes body content. A deliberate change regenerates the
goldens with
`UPDATE_GOLDEN_FORMATS=1 cargo test -p pdfextractor-cli --test formats_golden_test`;
as everywhere, the diff of the goldens is the review.

A golden pins the files it names as well as its own bytes. The references in each
rendering are read back — Markdown's `![Image](…)`, HTML's `<img src>`, YAML's
`uri:` — and every local one is held to a copy committed beside the golden under
the same relative path, so a re-encoded or truncated picture fails even though the
reference alone still resolves. Nothing is listed by case: a document that starts
naming a new file is pinned by the same run, and a pinned file no golden names any
more fails instead of lingering. The shared helper lives in
`crates/cli/tests/common/mod.rs`.

### Compare mode

A second use of the same window: turn on **Compare** and give it a Docling reference.
Every document that finishes is then drawn beside the reference for it — either the
reference file itself, or, when the reference is a folder, the file named after the
document (`test-data/golden/docling/` is laid out that way). The reference is never
handed to the extractor: it is what the two readings are compared *with*, so the CLI
is never told about it.

The document's own body tree lays both sides out — `body.children`, and groups
(`#/groups/N`, which is how Docling encloses a picture area, a list or a run of
formulas) walked where they stand — and the two lists are lined up the way a diff
lines up two files: a block is paired with at most one on the other side, pairs never
cross, and a block with no counterpart stands alone on its side.

Where both producers published a box for a block — Docling's PDF backend and ours do,
the flow formats do not — that box takes part in the matching itself, not only in the
joining afterwards. The words still decide whether two blocks are a link at all, and
the boxes decide which link is taken where the words offer more than one: a pair both
readings put in the same place counts for a tenth of a link more, which is what tells
apart two pairs the words cannot (a drawing a document lists twice, in one order on
each side — only the box says which copy is which), and a pair they put in different
places counts for half a perfect match, which is what makes two of those lose to the
one pair the boxes agree about. Nothing is hidden by any of it: a pair the boxes
contradict is still one row, whose note says where each reading put the block
(`not in the same place: page 1, 8% of the smaller box shared`).

Two readings do not always agree about the *order* of the same things, either — a
drawing Docling lists after the caption we list before it — and an alignment that
respects order cannot pair both halves of that at once. The boxes decide which of
those pairs the alignment takes; what is left over is joined afterwards by **page and
box**: two blocks that stand alone, are the same kind of thing, and occupy the same
place on the same page are one thing read twice. Only leftovers are joined this way,
and each joined row says so on itself (`paired by position: page 2, 88% of the smaller
box shared`), with the total in the panel's header, because that step is a heuristic
rather than a comparison. It also needs both producers to publish boxes at all — so
two readings of a DOCX that disagree about order still show one row a side.

Docling's side of a PDF is *finer* than ours — its backend writes an item per line,
where we keep a paragraph whole — so a comparison that paired only by content would
report every one of those per-line items as text the reference has and we do not. A
reference item whose letters run from one of our word boundaries to another is
therefore recognised as **contained** rather than shown as "only the reference", and
consecutive items one of our blocks holds are drawn as the run they are: our side of
the panel stays one block per row. A piece of a word is not inside ours — the
reference's `3` is not a piece of our `1937` — so a shared digit or a shared ending is
not mistaken for one reading cut up, and text our own reading spaced out letter by
letter (a form's field labels) still matches, because each letter is a word of ours.
An item the alignment could not reach that way — the reference put something we do not
hold between it and the run, so the run broke — is moved onto the row of the block whose
words hold it, which says how many items it took; the row keeps whatever it already
disagreed about, so a wording difference is never hidden behind a containment.

That last step is also where an item with **no letters in it at all** is answered for,
which is what a filled-in form is made of: Docling writes each dotted leader and each rule
as an item of its own, so a line of `irs_f1040` whose words we hold in full ("37 38
Estimated tax penalty (see instructions) …38") is read by the reference as word fragments
with leader items between them, and every leader would otherwise stand as a line saying
the reference holds text we do not. Such an item is taken onto the row of a block that
already holds *words* of the reference — a leader standing among the fragments of a line
we read is part of that line — and the row's note says so apart from the items taken by
their words (`3 of the reference's items are wholly inside ours; 16 items with no letters
in them lie in the same place`), with the row showing the reference's items in the order
the reference wrote them. It is the one place where *where* decides, so it asks three
things: a box from both producers, a block that already holds words there, and no letters
in the item itself — an item with letters in it that are not ours is text we did not read,
and a letterless item standing alone in our block's area is the reference's own reading of
that area rather than a piece of anything of ours. Both of those are left where they were:
one-sided, unless the joining by position finds them the same box on the same page and
nothing else, in which case the row says that is what paired them.

What is left of a form after all that is the rest of its dotted leaders: items with no
letters in them at all that no block of ours holds and that nothing of ours lies along —
lines our reading produced no block for at all. Nothing can be compared with an item like
that: it has no letters to match. So it is **placed** rather than listed — a row that says
what Docling wrote and where it lies (`a dotted leader or a rule at page 5, box
50,560–300,580: an item with no letters in it at all, so there are no words to compare`),
counted in the header as *placed* instead of among the rows only the reference has,
because a panel that called punctuation its words cannot compare "text we do not have"
would be claiming to know something it does not. The items of **one line** of a form are
placed together, though, because Docling writes each run of dots of a leader as an item of
its own: one row that names how many items it stands for and the extent they cover (the
runs of dots and the rule under them of one line are one row, not three), so `irs_f1040`'s
**327** items are **25** rows — one per line of the form — rather than 327 rows that used
to read as punctuation we had lost.

A placement is *drawn* as well as named: beside the row's note stands the page the item
lies on, drawn to its own shape as the reference stated it, with the item's box outlined
on it — so which line of a form a leader stands on is seen rather than worked out from the
coordinates (which stay in the note, where a row can still be read and copied). A page
whose size the reference never states has nothing to draw the box *on*: those rows are
named like the rest and drawn nowhere.

A row's own box answers *which line*, though, not *what the form looks like*. So the
placed items are gathered the other way round as well, **page by page**: above the rows
the panel draws each page that carries a place once, with every placed item of it at once,
so a form's leader pattern is one picture rather than one thumbnail per row. That picture
is built from the items *before* the rows are cut to what the panel lists, so a long
form's page is whole even where its rows are not — and it holds items, not rows, so it is
a drawing of what the reference wrote, dot run by dot run, rather than of the lines the
rows were grouped into.

Each row carries one of seven verdicts:

| Verdict | Means |
|---|---|
| **same** | Both sides read the block the same way — same kind, same label, same words, and, where both published a box, the same place on the page |
| **notation** | Both sides found it and only its *notation* differs: an equation the two write differently (`\sum_{}^{2}x` against `∑² x`), or LaTeX that differs only in spacing. DOCX equations now match Docling's LaTeX, so this row is uncommon there; a formula read from a PDF may still be a notation row |
| **differs** | Both found it and disagree about something a reader would notice — the label (`section_header` against `paragraph`), the words, or where the block sits: two readings that found the same words in different parts of the page are a difference, and the row names both places. The row says which |
| **contained** | Both found the words and disagree only about where an item *ends*: Docling writes an item per line, we keep the paragraph whole, so the reference's item (or the run of them) lies inside one block of ours. Our side of the row is that one block and the reference's side is the run, so a paragraph the reference read as a dozen lines is one line of the panel — and a form line's dotted leaders and rules are on that row too, taken by where they lie rather than by words they do not have |
| **placed** | The reference wrote an item with no letters in it at all — a dotted leader, a rule — where our reading produced no block for the line it lies along. There is nothing to compare and nothing to miss, so the row stands for *where* the item is — its box drawn on an outline of the page it lies on, beside the row, with the items of one line standing as one row that names how many they are and the extent they cover — and the header counts it apart from the rows only the reference has. Every placed item is also drawn on its page in the per-page view above the rows, so a form's whole leader pattern is one picture |
| **only ours** / **only the reference** | One side read something there and the other did not: a table read as prose, a drawing that came out as a box with no image. Two lonely rows that share a place on a page are joined into one row instead |

The panel's header shows the same numbers the golden suite quotes — the share of the
reference's alphanumeric characters that ours also holds, counted as a multiset and
with equations excluded, because an equation's *spelling* is not evidence about
anything. Two differences of scale are shown rather than hidden: the two tools publish
their own dpi on a picture, and a drawing rasterised from its declared size lands a
point or two from Docling's render of it.

This is not the golden suite: that one decides **pass or fail** against budgets, this
one **shows where the two readings differ** so a person can look. Where they overlap
they use the same definitions (`crates/ui/src/diff.rs`). The work is bounded, so a
pathological document cannot hang the window: at most 2,000 blocks a side are aligned,
at most 2,000 rows are drawn, and each text is clipped visibly at 300 characters — with
what was left out reported in the panel.

The whole corpus can be run through the mode in one pass, worst first — which is what to
reach for after a change, to see which documents it helped and which it did not:

```bash
cargo test -p pdfextractor-ui --test corpus_report_test -- --nocapture
```

That converts all 37 corpus documents (and the two fixtures beside them) in a single
CLI run and compares each with its reference, then prints the verdict mix per document, ordered by the share of rows the
reference does not simply agree with (rows that differ + rows only we have), beside the
character-agreement figure, the rows paired by position and the rows **placed**. The
`cont` column is what the reference cut finer inside our blocks, and what is left in
`refs` after it is text we did not read — the dotted leaders of a form line *we* read go
onto that line's row, and a leader whose line our reading produced no block for is in
`plcd` rather than `refs` — one row per line of the form — because there is nothing in it
to have missed. Those rows are
listed separately rather than ranked.
[progress.md](progress.md) has the report as it stands and what it found.

The report is also a **ratchet**: the numbers it measured last time live in
`test-data/golden/compare-baseline.json`, and a document whose panel now says more
`differs`, more rows only ours, more rows only the reference, or holds fewer of the
reference's characters than that file remembers fails the test — naming itself and the
count — so a change that helps one document and quietly harms another cannot pass. The
character share counts no stamp a conversion writes: a `DATE` or `TIME` field has no text
of its own, so a legacy `.doc` read through the bridge holds the day and the moment that
conversion ran while its reference holds the day and the moment Docling's did, and
counting those digits would make the gate a reading of the calendar and the clock —
`FloatingPictures.doc` would fall through its floor on some mornings without a word of
the document changing, and `stamp_fields.doc`'s footer across a minute. Leaving out the
value does not leave out the item: a footer that lost its date is still a row that
differs or a row only the reference has, and those are ratcheted on their own. The rule
and the gate that applies it are the library's (`crates/ui/src/diff.rs` and
`gate.rs`): the panel, this report and the command line's `--compare` all hold a
document through the same Session, so there is one gate rather than three that can
drift. The
columns that are not ratcheted (`same`, `contained`, `notation`, `joined`) may move
either way, and a `differs` row cannot leave its column without landing in one of the
one-sided ones. A form's **placed rows** are ratcheted as a rate rather than a count: the
items those rows stand for and the rows they stand in are compared by cross-multiplication,
so the same 327 items coming back as 327 rows instead of 25 is refused, while fewer rows for
the same items is the improvement it is. The per-page drawing is the same rule one level up —
more pictures for the same boxes is the leader pattern answered row by row again. A rate
alone answers nothing at the limit, though, and it keeps any *proportional* fall: half the
panel in half the rows is the same ratio, and a document whose form is answered no more at
all is 0 rows for 0 items. So the **items themselves** are a floor of their own — the placed
item count may only fall on a document that placed nothing to begin with, and the failure
names it (`form item(s) placed: 327 → 160 item(s) in 12 row(s)`). Nothing else in the file can
tell "answered as text instead" from "lost", so that reading change is argued for the way
every deliberate trade is, by regenerating the baseline and reading its diff. Accept a trade on purpose with
`CORPUS_BASELINE=update cargo test -p pdfextractor-ui --test corpus_report_test`, and let
the diff of that file be the argument for it. A document whose reading goes through
LibreOffice — a legacy `.doc`, or any of the families the bridge is the only reader
for — is **left out** of a run on a machine without it, before the corpus is converted
at all, and the baseline entry is kept: the same file read through Writer and read by
the built-in piece-table reader says different things, so neither reading can be held
to the other's numbers — and a family with no reading here at all would otherwise stop
the conversion on the missing tool rather than be left out. Which documents those are is
the gate's own question, asked of each file's **bytes** rather than of the name it was
saved under: a DOCX somebody saved as `report.doc` is read here exactly as it would be
with the bridge installed, and is held.

The window holds that **same floor itself**, by the same code: `compare-baseline.json`
lives beside the references it is about, so the Compare panel finds it without being told
where it is (`test-data/golden/docling/` → `test-data/golden/compare-baseline.json`) and
measures each finished document against the numbers it remembers. A document whose items
have left the panel is **refused** rather than drawn: the panel names what fell
(`below the floor · form item(s) placed: 654 → 327 item(s) in 25 row(s)`), withholds the
rows, the page pictures and the verdicts, and says how to fix the reading or accept the
trade. How much of the ratchet a run is held to follows from the settings it was made with:
the baseline records them (`measured`: the OCR mode, and anything else worth recording),
and when the run's own settings match the remembered ones — the corpus run itself, or the
window set to `--ocr never` — **every** rule applies, `differs` rising and agreement
falling included. When they do not — the window's default `--ocr auto` against a
baseline measured with OCR off — only the floor applies, because OCR is the setting that
moves the counts measuring how the two readings *agree*, while whether a form's items
are still in the panel belongs to the reading. None of this happens without a memory
beside the reference, which is every comparison against a document of your own, and a
memory that is there but unreadable is said on the panel rather than passed over in
silence.

The panel says which of the two it was, on every document the memory has: a quiet line
under the panel's header —
`Held to the memory: every rule of the ratchet applies: this run was measured the way the
memory records (--ocr never)`, or
`Held to the memory: only the floor under a form's placed items applies: this run was
measured with --ocr auto and the memory with --ocr never, and OCR moves the counts that
measure how the two readings agree`. A refused panel carries the line too, so whether the
whole ratchet refused the document or the floor alone is read rather than guessed; a
document the memory does not have carries no line, because there was nothing to hold it
to.

A panel nobody is watching does not fail a build, so the same hold is on the command
line: `--compare` holds every finished document to the memory beside its reference,
exactly as the panel does, and **exits non-zero** when one is refused — naming the
document and what fell, so a fallen panel fails a build and not only a window:

```bash
cargo run -p pdfextractor-cli -- -i test-data/real -o /tmp/out --ocr never \
  --compare test-data/golden/docling
#   compare: test-data/golden/docling · memory: 37 document(s), measured with --ocr never
#   compare: test-data/real/EquationAsScientificNumbering.docx: 2 row(s), agreement 100.0% · at or above the whole ratchet
#   … one line per document …
#   compare: test-data/real/tracemonkey.pdf: 439 row(s), agreement 99.1% · at or above the whole ratchet
#   (exit 0 — nothing fell; a run that does fall ends)
#   error: the compare mode refused 1 document(s): …/dummy.pdf: below the floor · form item(s) placed: 5 → 0 item(s) in 0 row(s)
```

With no memory beside the reference the comparisons still print and no floor can fire;
what the mode refuses instead is a run it could not hold at all — a reference path that
is not there, a memory that is there and cannot be read, or a document with no reference
beside it — because a gate that held nothing while claiming to hold everything is worse
than no gate.

Every push runs that gate over the corpus in CI
(`.github/workflows/compare-gate.yml`): the same command over `test-data/real`,
measured `--ocr never` so every rule of the ratchet applies — a document that falls
below its floor fails the build with its name and its count in the job's log.

A document whose reading goes through the bridge is left out of a run on a machine
without one — the corpus's four legacy `.doc` files, on that runner — because the
numbers the baseline remembers are Writer's reading of those files and what the run
compared is the built-in reader's. The run decides that **before** it converts anything,
which is what puts the families the bridge is the *only* reader for under the same rule: a
legacy `.xls` or a Works `.wks` has no reading here to finish and leave out afterwards, so a
rule that waited for a converted document would have it fail the gate on the missing tool
instead. Each is named on a line of its own and counted at the end
(`not held · read through LibreOffice, and this machine has none — a reading made
without it is not the reading the memory remembers`), so the log says what was held and
what was left out rather than quietly holding less than the gate claims; the weekly
bridge gate above is where those documents are held, with LibreOffice installed. A run that
is only converting, with no `--compare`, still stops on the missing bridge and names the
re-save route.

### Smoke-testing the whole stack without a human

`--selftest [INPUT]` opens the window, proves the webview and IPC round-trip,
converts the given path through the CLI, and prints what the page actually drew;
`--selftest-ref PATH` turns Compare on for that run, so the panel is exercised too.

```bash
# Smoke-test the whole stack without a human: the window opens, the webview
# loads the frontend, the IPC round-trips, the given path is converted through
# the CLI, and the page reports back what it actually rendered.
cargo run -p pdfextractor-ui -- --selftest path/to/document.doc

# The same, stopped 2.5 seconds in, to check the stop path end to end.
cargo run -p pdfextractor-ui -- --selftest ./docs --selftest-stop 2500

# The whole stack with Compare on: one corpus document against the committed
# Docling reference for it, with the panel's own tallies reported back.
cargo run -p pdfextractor-ui -- --selftest test-data/real/math-nary.docx \
  --selftest-ref test-data/golden/docling
#   … diffs=1 diff-agreement="100.0%" diff-verdict="same=0 notation=3 differs=0 contained=0 ours-only=0 theirs-only=0 placed=0 paired-by-position=0 truncated=0" placed-drawn=0 placed-pages=0 diff-rows=3 floors=0 held=floor

# A PDF reference carries the boxes the two readings found each block at, so those
# boxes take part in the matching, the rows they leave one-sided are joined by page
# and box, and the placed leaders of a form line — one row per line, naming how many
# items it stands for and the extent they cover — are **drawn** on the page they lie on,
# reported back as the panel's own `paired-by-position` count, the `placed-drawn` boxes it
# really put on a page rather than was told about, and the `placed-pages` pictures it drew
# with every placed item of a page at once.
cargo run -p pdfextractor-ui -- --selftest test-data/real/tracemonkey.pdf \
  --selftest-ref test-data/golden/docling
#   … diff-agreement="99.1%" diff-verdict="same=25 notation=0 differs=138 contained=56 ours-only=89 theirs-only=97 placed=34 paired-by-position=60 truncated=0" placed-drawn=1 placed-pages=3 diff-rows=401 floors=0 held=floor
#   (a panel draws at most 400 rows and this document has 439, so 33 of its 34 placed
#   rows are past the end of what was drawn — the 401st row drawn is the "and N more"
#   line — and its 34 items fall on 3 pages; `test-data/real/irs_f1040.pdf`, whose 198
#   rows carry the form's 25 lines as 25 placements on **2** pages, comes back with
#   placed-drawn=25 placed-pages=2 diff-rows=198 floors=0 held=floor, and `floors` counts the
#   documents this run refused: a panel whose placed items have fallen below the count
#   the memory beside the reference remembers draws no comparison at all, and says so.
#   `held` is which rules held the panel to its memory — `floor` here, because the
#   window measures with `--ocr auto` and the memory records `--ocr never`; it reads
#   `ratchet` when the two measurements match and every rule applies)
```

What the page hands back includes `formats=N` — how many input formats the CLI
named when the page asked it again, the refresh being the ask the smoke test
exercises (the page asks on load too) — `families="..."`, the headings those
formats were drawn under, and `cli="..."`, the binary named beside them, so both
asks, the IPC, the grouped formats and the binary line are covered by the same
smoke test as the conversion.

The self-test converts into a directory of its own under the system temporary
directory, never beside the document it read, so pointing one at the corpus leaves the
corpus alone: a stray `.json` among the documents is a document with an unsupported
extension to the golden suite, and a `<stem>_images/` directory is litter of the same
kind. The run says where it wrote — the command line it prints carries the `--output` —
so what it converted is still there to look at (and left there, since that is what a
temporary directory is for):

```
selftest-started run=1 command=…/pdfextractor-cli --input …/tracemonkey.pdf --output /tmp/pdfextractor-selftest-4242/tracemonkey.json --parallel 4 --recursive true --ocr auto --progress
```

The first prints `webview-ready`, `selftest-started run=...`, then a
`selftest-rendered ...` line carrying the state **as drawn** — title, verdict,
progress bar width, one status per row, log line count — so a mistake in the page
is not mistaken for a working window. The second adds `selftest-stop ... stopped=Some(1)`
and reports a cancelled run. The last two add `diffs=...`, `diff-agreement=...`,
`diff-verdict=...`, `placed-drawn=...`, `placed-pages=...`, `diff-rows=...`, `floors=...`
and, where there is one, `floor="..."`: the comparison panels, their tallies, how many
placed boxes were actually drawn, how many per-page pictures the panel drew with all of a
page's placed items on them, how many rows it drew (0 on a document it refused), how many
documents fell through the floor their memory remembers, and what that floor said — all as
the page drew them. All of them exit 0 if the stack is healthy.

## Test documents

The extractor is regression-tested against real documents produced by other tools
(`test-data/real/`, see its [README](test-data/real/README.md) for provenance):

| Document | Producer | Exercises |
|---|---|---|
| arXiv paper | pdfTeX | object streams, ligature encodings, kerning word gaps, 500 dpi figures |
| pdf.js `tracemonkey` | Quartz | multi-column paper, `Td` word gaps, a vector-only figure |
| pdf.js `basicapi` | TCPDF | Identity-H composite fonts, no images at all |
| IRS Form 1040 | PDF 1.7 | linearized file, cp1252 punctuation, forms, page-covering background |
| 4 arXiv papers | pdfTeX | math-heavy text, vector diagrams, raster + vector figures |
| 16 OMML `math*.docx` | LibreOffice/Word | real Office Math: fractions, radicals, Σ/∏/∮, matrices, accents, limits |
| `mathtype.docx` | Word + MathType | an OLE equation object (binary, not OMML) |
| `VariousPictures.docx` | Word | PNG, JPEG, EMF and WMF drawings — the three metafiles are played into PNGs |
| `drawing.docx` | Word | 105 tables mixed with prose |
| `headerPic.docx` | Word | a picture that lives only in the header part |
| `calibre_demo.docx` | Word | headings, lists, 38 tables, footnotes, a table of contents with tabs |
| `chartex.docx` | Word | six charts whose titles exist only in the chart parts |
| `shapes-with-text.docx` | Word | shapes, not images — each written twice by Word |
| 4 legacy `.doc` | Word 97 | OLE container, piece table, field instructions, inline pictures |

Run the whole corpus at once:

```bash
cargo run -p pdfextractor-cli -- -i test-data/real -o /tmp/real -p 4
```

Eleven of them also have a real Docling conversion committed beside them, which the
golden suite compares against; see [test-data/golden/README.md](test-data/golden/README.md).

**Microsoft Works has no committed sample.** libwps' OSS-Fuzz corpus is the only
public set of real Works documents, and it states no licence for its files — a
fixture this repository cannot license is one it cannot hold. Fetch it and point
`PDFE_WORKS_SAMPLES` at the directory the archive unpacks to — 40 real Writer files,
three of which the test converts — to run it:

```bash
curl -sfL -o wps.zip "https://downloads.sourceforge.net/project/libwps/corpus/wpsfuzzer_seed_corpus.zip"
echo "95c919a7506f6be30d90c100df6f5fa29bcf9e75a1bbef7a509de65b42e5aa5a  wps.zip" | sha256sum -c -
unzip -oq wps.zip -d works-wps
PDFE_WORKS_SAMPLES=$PWD/works-wps/wps cargo test -p pdfextractor-cli
```

The weekly bridge gate (`.github/workflows/bridge.yml`) runs that fetch itself before
its tests, pinned to that sha256, so the Works test asserts there every week without a
byte of the corpus entering the repository. Without LibreOffice on `PATH`,
that test says so and returns instead of failing: the bridge needs a converter, and a
machine without one is a supported machine.

## Layout

```
crates/
├── core/          # Docling types + document builder
├── pdf-parser/    # lexer, object model, structural parser, content interpreter,
│                  # image decoding, font encodings, stream filters, pictures/figures
├── docx-parser/   # OPC package, WordprocessingML, images, VML drawings, OMML equations
├── xlsx-parser/   # OPC package, SpreadsheetML: sheets, shared strings, styles, merges
├── pptx-parser/   # OPC package, PresentationML: slides, text bodies/runs, tables, media
├── drawingml/     # the shape painter both office readers use: geometry, fill (solid
│                  # or gradient), line with its arrowheads, shadow, theme colours,
│                  # and the PNG a shape is drawn into
├── image-parser/  # plain images: format, pixel size, resolution and EXIF orientation
├── doc-parser/    # legacy OLE compound files, Word Fib + piece table
├── text-parser/   # RTF, HTML, Markdown/text, CSV/TSV
├── picture-formats/ # the one list of picture formats: media type ↔ extension
├── formats/       # what a file actually is (the bytes, not the name), and the
│                  # one family with no reader here: the LibreOffice bridge. Asked
│                  # by the run, the `--compare` gate and the corpus report alike
├── layout/        # rows/runs, XY-cut columns, reading order, labels, tables
├── ocr-engine/    # Tesseract OCR (required, default-on); TSV word/line boxes
├── json-exporter/ # Docling JSON serialization
├── cli/           # command line interface over the parsers and the gate
└── ui/            # Tauri window (static frontend in ui/dist) over the CLI
test-data/
├── hello.pdf, compressed.pdf, scanned.pdf   # hand-made fixtures
├── inventory.xlsx                           # workbook fixture (make_inventory_xlsx.py)
├── worksheet_pictures.xlsx                  # workbook that draws pictures (make_pictures_xlsx.py)
├── shape_deck.pptx                          # shapes PowerPoint paints itself (make_shape_deck.py)
├── background_deck.pptx                     # a slide with a picture background (make_background_deck.py)
├── master_background_deck.pptx              # a picture background the slide master states
│                                            #   (make_master_background_deck.py)
├── layout_background_deck.pptx              # a picture background the slide layout states,
│                                            #   with the master's own beside it
│                                            #   (make_layout_background_deck.py)
├── layout_colour_background_deck.pptx       # a colour background the slide layout states,
│                                            #   which hides the master's picture
│                                            #   (make_layout_colour_background_deck.py)
├── preset_deck.pptx                         # one shape per preset outline (make_preset_deck.py)
├── q3_review.pptx                           # deck fixture (make_deck_pptx.py)
├── legacy_deck.ppt                          # legacy deck fixture (make_legacy_ppt.py)
├── comment_times.pptx, comment_times.ppt    # comment-time bridge pair (make_comment_times.py)
├── ledger.xlsx, ledger.xls                  # workbook bridge pair (make_legacy_xls.py)
├── brief.docx, legacy_brief.doc             # document bridge pair (make_legacy_doc.py)
├── stamp_fields.doc                         # a footer of DATE and TIME fields, stamped
│                                            #   afresh by whichever conversion runs it
│                                            #   (make_legacy_doc.py)
├── vml_shapes.docx                          # VML shapes, WordArt, gradients, a shadow, an
│                                            #   arrow, and the focused and focus-sized fills
│                                            #   (make_vml_shapes.py)
├── drawingml_shapes.docx                    # DrawingML shapes (make_drawingml_shapes.py)
├── shaded_deck.pptx                         # a gradient, a shadow and arrowheads
│                                            #   (make_shaded_deck.py)
├── svg_blip_pictures.docx, .pptx            # an SVG stated as an svgBlip (make_svg_blip_pictures.py)
├── svg_embed_pictures.docx, .pptx           # the same document with the SVG as the picture's
│                                            #   own part, LibreOffice's way (same script)
├── svg_stretch_picture.docx                 # an SVG stating preserveAspectRatio="none" in a
│                                            #   frame of other proportions (make_svg_stretch_docx.py)
├── mc_svg_pictures.docx, .pptx              # an SVG in an mc:Choice, VML/raster copy in
│                                            #   mc:Fallback (make_mc_svg_pictures.py)
├── mc_fallback_pictures.docx, .pptx         # pictures stated only in mc:Fallback: the
│                                            #   body's, a cell's, a nested table's cell's,
│                                            #   a text box's and a header's in the DOCX,
│                                            #   and the slide's and a group's in the PPTX
│                                            #   (make_mc_fallback_pictures.py)
├── blip_fill_shapes.docx, .pptx             # shapes filled with a picture, SVG in the
│                                            #   blip (make_blip_fill_shapes.py)
├── picture_bullets.docx, .pptx              # list items whose bullets are pictures,
│                                            #   one of them an SVG (make_picture_bullets.py)
├── invoice_scan.png                         # image fixture (make_scan_images.py)
├── nested_table.docx                        # a table inside a table's cell
│                                            #   (make_nested_table_docx.py)
├── images/        # the same scan as JPEG, TIFF, BMP, WebP, GIF and JPEG 2000,
│                  #   a second JPEG 2000 stating 150 dpi in its `resc` box, the
│                  #   first one's bare codestream (`scan.j2c`, and `scan.jpc` for
│                  #   the same bytes under the other name), the container under
│                  #   `.jpf`, a three-frame
│                  #   animation (`turning.gif`, make_animation_gif.py), the same
│                  #   strip as an animated WebP (`turning.webp`), the same
│                  #   strip stored on its side with EXIF orientation 6
│                  #   (`turning_turned.webp`, make_animation_webp.py), and two
│                  #   three-sheet scanned
│                  #   batches, upright and turned (`pages.tiff`,
│                  #   `pages_turned.tiff`, make_multipage_tiff.py), and the
│                  #   drawing the stretch fixture states (`stretch.svg`,
│                  #   make_svg_stretch_docx.py)
├── real/          # corpus from other producers (see its README)
└── golden/        # the docling-core schema, real Docling conversions, docling-core check
docs/              # schema and format notes
```

## Box overlay

`--preview` writes a self-contained HTML file next to the JSON showing every box
that was extracted, drawn on the page it came from — so extraction quality is
visible rather than inferred from numbers. It is the quickest way to spot a
mis-scaled OCR box, a figure that was missed, or a text run whose box is in the
wrong place.

```bash
cargo run -p pdfextractor-cli -- -i test-data/scanned.pdf -o scan.json --preview
# then open scan.preview.html
```

Boxes are colour-coded by where they came from (text layer, OCR, vector figure,
image) and a second layer draws the **layout blocks** — one dotted box per element
the analysis produced, labelled `title`, `section_header`, `list_item`, `caption`
or `table`. Checkboxes isolate one kind, so you can compare what the page says with
what the block structure claims. The recognised text is drawn inside each box at the
size the box implies, so a box that is too wide or too narrow is obvious; toggling
**show text** off fades the page render back in for a clean side-by-side. A linked
sidebar lets you hover a row to highlight its box and click to scroll to it, so a
long page is navigable rather than a wall of boxes.

A page backdrop is used when one exists. Scanned pages already have their image
extracted, and for text pages `pdftoppm` is used **when it happens to be
installed** — rendering is best-effort and preview-only, so the extractor itself
never depends on an external renderer. Without one the HTML still works: it
reconstructs the page from the extracted text at its true coordinates and says so
in the header.

Flow-based formats get no overlay: DOCX, legacy `.doc`, RTF, HTML, Markdown and
text carry no page geometry, so blocks have no rectangle to draw on. The CLI says
so and skips the preview for them instead of writing an empty one. A workbook is
skipped for the other reason: its pages are cell indices, not a rendering, so
there is no page image to draw a box on.

## Reading order and structure

Text runs are grouped into the document a reader perceives: rows are cut into runs
at column gutters, whitespace bands split the page recursively (columns first),
runs merge into paragraphs, and each block is labelled from its size, position and
literal prefix. A run of rows with aligned short cells becomes a table. Pictures are
inserted where they appear rather than appended, and list markers and heading numbers
are kept in `marker`/`level` fields instead of the text.

Each item class carries only the structure it has, because Docling rejects a field an
item does not allow: a heading keeps its numbering depth in `level` and its number folded
back into the text (which is where Docling keeps it), a list item keeps the bullet or
number the page printed in `marker` together with `enumerated`, and a title has neither.

```jsonc
{ "label": "title",          "text": "Attention Is All You Need" }
{ "label": "section_header", "text": "3.1 Encoder and Decoder Stacks", "level": 2 }
{ "label": "list_item",      "text": "first thing", "marker": "\u2022", "enumerated": false }
{ "label": "paragraph",      "text": "the paper", "hyperlink": "https://arxiv.org/abs/1706.03762" }
```

A text item that goes somewhere keeps that target in `hyperlink`: a PDF `/Link` annotation
over its box, a DOCX `w:hyperlink` around its words, an HTML `a` with an `href` inside
them. A link splits its sentence the way a reader sees the link as its own thing — the
words before, the link's words carrying the URL, the words after, as separate items — and
the field is left out entirely for text that goes nowhere. Internal PDF jumps become
`#page=N`; a named DOCX anchor or an unresolved PDF named destination names no URL and
leaves its words plain rather than inventing one.

Blocks are ordered top-to-bottom and, on a multi-column page, column by column. On the
real corpus this yields 2,180 text items and 66 tables, among them 8 titles, 183 section
headers, 84 list items, 40 equations and 26 footnotes - where the parser alone produced one
item per text run, in drawing order.

A Word table cell can hold more than words. A picture a cell states is kept on the cell
rather than left in the body beside the table: the builder places the cell's pictures in
an `unspecified` group whose parent is the table, lists that group among the table's
children, and points the cell's `ref` at it — Docling's own `RichTableCell`, the shape
its ODF reader writes for a cell holding an item. A cell with no picture writes no `ref`
and stays the plain `TableCell` it was. The picture has no provenance, because a Word
cell is flow content with no page rectangle of its own. Both writers follow the cell:
Markdown puts the image inside its pipe row and HTML inside its `<td>`/`<th>`, so a
picture that is the cell's is not written as a block beside the table. A table a cell
holds is the cell's the same way: the nested `w:tbl` goes into that group rather than
becoming an item of the body beside the outer table, so the cell that holds it
references the group the nested table sits in and the nested table's own cells are rich
cells in turn — the shape Docling's own Word reader writes for a table in a cell. All
three writers follow the cell there too: Markdown, HTML and plain text fold the nested
table's cells' words — and the pictures those cells hold — into the outer cell after
its own words, which is what Docling's Markdown export does with one. HTML says it as
words rather than markup on purpose: this reader reads a `<table>` inside a `<td>` as
rows of the table around it, so writing one would be a page that does not read back to
the document it came from. A vertically
merged cell is one cell: a picture or a table its continuation row states travels to
the cell that opened the merge, which is the cell written to the grid, so being stated
in the second row of a merge does not lose it. A sheet's cell and a slide's table cell still hold text
only — their pictures are anchored on the page, not drawn in a cell — so neither is a
rich cell.

## Pictures and equations

Extracted raster images are written to a `<output-stem>_images/` directory next to the
JSON and referenced from each picture item, with the MIME type, pixel size, and the
resolution implied by the box the document drew them into. A picture a **flow page**
(HTML or Markdown) already named beside itself keeps the page's own relative path
instead — `![…](images/scan.jpg)` is still `images/scan.jpg` beside the output —
because the page named that file, and a page written from the document has to name
it again to be the page it was. A picture the page carried in a `data:` URI, or named
in a way this cannot spell (a remote URL, an absolute path, a name needing escapes, an
extension that is not the bytes' own), is written under `<stem>_images/img<N>.<ext>`
like any extracted picture. Figures drawn as pure vector
paths — common in scientific papers — have no pixels to extract, so they are reported as
picture items with a placement box and no `image`. OCR boxes for a figure are mapped
through that same placement box.

Which media types a picture can be is one table — `crates/picture-formats` — and
every side reads it: a Word or PowerPoint package names an embedded part by the same
list the CLI writes a picture file under and the flow readers accept an `<img>`
`type` for, so a format added there is carried out of a package and onto disk without
a second edit. The CLI's accepted *input* extensions are that table filtered by the
encodings the image reader turns into a page, and the `no supported documents (…)`
message, the `Accepted input formats:` block at the end of `--help` and the JSON of
`--list-input-formats` are generated from the same list, so none of them can name an
extension the run refuses or miss one it accepts. The help prints that block
**grouped by family** — PDF, Word, Excel, PowerPoint, Works/WordPerfect, text and
markup, then the picture encodings with each format's aliases beside its own name
(`.jpg/.jpeg`) — and marks with `*` the names the LibreOffice bridge reads, which a
machine without LibreOffice cannot take. The refusal message keeps the flat list, so
a script never has to know the grouping; the JSON carries both, the flat list under
`input_formats` for a caller that only filters and the same set under `families` —
the help's headings, in the help's order, each holding its names in their plain
spelling (`.jpg` and `.jpeg` apart, not `*`-marked) — for a picker that offers
groups. One arrangement of families feeds both, so the JSON cannot group a name
under a heading the help does not use. A picture on that table that
is not a page — an SVG, a Windows metafile — is refused as an input with a message
that says so,
names its media type and offers the picture formats that *are* pages — a GIF among
them, its screen being a page and its frames the pages — so a format the tool writes
out is never
reported as an unknown one. A folder holding only such pictures is not an empty
folder, and its refusal says so: `no supported documents (…)` goes on to name the
pictures the walk passed over — counted, the first six shown — so a folder of
drawings does not read as a folder with nothing in it. A name
the run does not accept is answered from the bytes before the name is left to speak
alone: `report.xyz` holding a PDF is refused with `the bytes are a PDF, which this
tool reads: rename the file to .pdf`, and the same is done for a package, an image,
a flow file, or a format this knows and cannot read yet (an EPUB is named as itself,
with no rename promised, because none would help) — only a stranger whose bytes say
nothing keeps the bare `unsupported extension:` line. A GIF and an SVG are both on that list, and both are proved from the
package side rather than only from a flow file: `test-data/gif_svg_pictures.docx` and
`gif_svg_pictures.pptx` each embed `images/chart.gif` and `images/diagram.svg`, and
one end-to-end test holds each conversion to two pictures — the GIF at the 60 × 36 its
header states, decoded and written beside the JSON as the PNG every office picture is,
and the drawing at the 72 × 43 points the package places it in, played at 144 dpi and
written as a 144 × 86 PNG with the diagram's pale panel and blue stroke on it — and a
render that names both files.

The SVG is a drawing rather than an encoding of pixels, so it is *played*: a whole
graphics language cannot be carried in an `image` reference and be seen. The rendering
is `pdfextractor-svg`, which resvg drives; what this project decides is the size — the
frame the package places the picture in, at the same 144 dpi an EMF or WMF in the same
package is played at, or the drawing's own viewport when the package states no box — and
the background, which is the drawing's own: an SVG is transparent unless it paints a
background, and a white page would put a box around a picture on a coloured slide. Text
is drawn in the host's fonts, since an SVG names families rather than glyphs, and a
family the host lacks is substituted rather than dropping the words. A drawing that does
not parse, or paints nothing at all, keeps its markup and its type: a box with nothing
in it is worse than the drawing the file did state.

The SVG in those packages is written into them by hand, because neither office library
can place one, so a second test asks an office application to accept it: headless
LibreOffice opens each fixture and writes it back, and what it writes must keep the
drawing as its own part byte for byte, still declared `image/svg` and still reached by
a relationship, with a raster drawn from its `<rect>` and `<polyline>` beside it as the
fallback Word carries. A part LibreOffice cannot read is dropped or rasterises to
nothing, so the check fails rather than passing on the file's mere presence. It skips
where LibreOffice is not installed, like the other bridge tests.

The same re-save asks the same question of the other fixture pictures. The BMP/TIFF
pair comes back with both drawings as media parts of their own bytes — the TIFF under
the `.tif` name LibreOffice gives it — each reached by a relationship and declared
`image/bmp` and `image/tiff`; and the metafile fixtures, `VariousPictures.docx` and
`drawing.docx` from the corpus, come back with every one of their EMFs and WMFs byte for
byte, reached and declared. The WebP/JPEG 2000 pair is the uneven one: the chart comes
back drawn as a 60 × 36 raster with its three bar colours on it, while the diagram is
gone, because LibreOffice has no JPEG 2000 filter at all — a standalone `.jp2` is not a
picture it can open either. The test says so rather than hiding it: the diagram is the
half of that fixture only this reader reads, and a LibreOffice that gains the filter
fails the assertion, which is the moment to say it has. The `mc_svg_pictures` pair comes
back in the application's own form of the same picture: the drawing copied through byte
for byte, the `asvg:svgBlip` extension naming it, and a raster it drew from the markup
beside it — both reached and declared. Its second picture, whose choice names a drawing
the package does not hold, is dropped rather than shown from its orange fallback, so this
re-save says nothing about the case a fallback exists for; the test states that, and
fails if a later LibreOffice keeps it. The `mc_fallback_pictures` pair states its
pictures only in an `mc:Fallback`, with no `mc:Choice` at all — the body's, and in the
DOCX a table's cell's, a cell's in a table nested in another cell, a text box's and a
header's; the slide's and a group's in the PPTX — and the application reads all of them:
its DOCX export keeps an `mc:Fallback` branch for each (now with a choice beside it)
whose `v:imagedata` still names the picture, the cell's inside the cell and the text
box's inside the box, and it writes the header's into the two header parts it makes for
the section; its PPTX export writes each fallback as a `p:pic`, the group's inside its
own `p:grpSp`. Either way the 60 × 36 orange rasters are in the re-saved package,
declared and reached. The `svg_embed_pictures` pair says the same thing
in the application's own spelling — the drawing is the picture's media part, its
`r:embed`, with no raster beside it — and it comes back the same way: that drawing copied
through byte for byte and a raster drawn from it beside it, both reached and declared.
LibreOffice writes the two EMFs
of `VariousPictures.docx` back under `.wmf` names while keeping their bytes, so the parts
are matched by content
and each declaration is read from the name the application gave the part. `mathtype.docx`
is not in that check: its only drawing is a MathType OLE object's preview, which
LibreOffice's export drops whole, so a re-save can say nothing about that part — the
conversion test is where the preview is proven.

The pair Word writes is asked the same question: `svg_blip_pictures.docx` and
`svg_blip_pictures.pptx` put the PNG fallback on the picture's `a:blip` and the drawing
in `asvg:svgBlip`; LibreOffice reads the extension, copies the drawing through and writes
a raster of it as the blip's fallback, and this reader still takes the drawing alone. The
oriented pair — `oriented_picture.docx` and `oriented_picture.pptx`, holding a JPEG whose
stored pixels are sideways and whose EXIF tag turns them — comes back with the JPEG's
bytes, tag included, under the `.jpeg` name the application gives it. A Word picture
bullet is only half kept: the painted raster survives as the numbering part's
`v:imagedata`, while the drawn bullet is dropped; PowerPoint keeps both and rasterises
the drawing at 41 × 25. The PowerPoint half of the picture-fill pair keeps both fills —
the SVG copied through and the orange raster unchanged — though its export adds explicit
line and shadow to the shapes, so this reader paints them into canvases about 13 pixels
larger; LibreOffice refuses the DOCX half outright. The `worksheet_pictures` workbook
keeps the four media parts it has filters for — BMP, GIF, PNG and JPEG — byte for byte,
with both sheets' drawings still pointing at them, and drops the last two, the two
spellings of a JPEG 2000 the fifth and sixth anchors name: the bare codestream `.jpc`
and the `.jp2` container beside it. There is no JPEG 2000 import filter, so a workbook a
spreadsheet application re-saves loses both pictures whole, whichever spelling the
encoder chose. The real-world `headerPic.docx` has its only picture in a header part, and
LibreOffice keeps it there — under a second header as well, so this reader takes the
original 80 × 80 picture and its copy. A slide's picture background is asked the same
question: `background_deck.pptx`
keeps its first slide's `p:bg` as a background — the `a:blipFill` still naming the
photograph, with no `p:pic` promoted beside it — and the second slide still states none,
so the JPEG comes back byte for byte in a part the package declares and a relationship
reaches, and this reader takes the same 620 × 350 page image from the deck the
application wrote. `master_background_deck.pptx` asks it of the inheritance: no slide of
its three states a picture background — the photograph is the slide master's, behind
the first two slides by inheritance and stopped at the third by the colour it states
itself — and the application writes the deck back the same way, no `p:bg` on either
inheriting slide, the master still holding the `a:blipFill` that names the photograph,
and the third slide still holding its colour. The photograph comes through byte for
byte there too, and this reader takes the same 620 × 350 page image from the first two
pages of the re-saved deck and none from the third — a reading only the inherited
background can give, since the re-saved slides name no picture themselves.

Each re-saved package is converted by this tool as well, so the bridge covers our own
reading of what the application wrote and not only the application's acceptance of the
fixture. The BMP/TIFF pair and `VariousPictures.docx` come back picture for picture, at
the same sizes and with the same pixels. The SVG pair keeps the GIF's pixels, and its
drawing is the picture both before and after the re-save: the application copies the
markup through byte for byte, so the two renderings are the same pixels at the same
frame — the fallback raster the application draws counts as a part of the drawing, not
as a third picture. The WebP/JPEG 2000 pair comes back one
picture short, exactly the half the application drops, with the chart's pixels unchanged
under the PNG that replaces the WebP. The `mc_svg_pictures` pair comes back one picture
short too, for the application's own reason: the drawing is the picture before and after
the re-save, the raster beside it a part rather than a second picture, and the picture
whose choice named a missing part is gone with it. The `mc_fallback_pictures` pair comes
back picture for picture: the fallbacks' orange rasters at their 60 × 36 grid, with their
pixels, whether the application kept them as fallback branches (DOCX, the cell's still in
its cell, the text box's in its box) or as the slide's pictures (PPTX, the group's still
in its group). The DOCX's header picture is the one exception, and it is the
application's: LibreOffice writes the header into two header parts, so this reader takes
the header's picture twice, and the test allows exactly that one copy and holds it to the
picture the fixture states. Every other picture is read by both, and the reader places
it where its container stands, between the sentences that frame it. The `svg_embed_pictures` pair comes
back picture for picture: one drawing before and one after, at the same 144 × 86 frame
with byte-identical pixels, the fallback the application adds beside it again a part
rather than a second picture. The Word-spelled SVG pair comes back as the same one
drawing at 144 × 86, and the oriented pair as the same one upright 1240 × 1754 picture.
The picture bullets come back as one painted bullet in the DOCX and two in the PPTX, the
drawn one at the 41 × 25 the application rasterised it to. The picture fills come back as
two pictures — the drawing and the orange raster — in the larger canvases the
application's added line and shadow produce, and the worksheet's five pictures (four on
the first sheet, the BMP again on the second) at their fixture sizes and pixels. The
header picture comes back twice at 80 × 80 with the fixture's pixels, the copy stated
rather than counted as the same picture once. And `drawing.docx` comes back with the four
drawings the application copied through, the one it re-drew (its 137 × 77 logo, written
back at 136 × 76) and four rasters it makes of rules that the fixture states as `o:hr`
shapes and this reader reports as no picture at all — so the one picture that stops
matching is the one the application itself redrew, which is stated rather than smoothed
over.

Word 2016 and later write that pair the other way round: the PNG fallback in the
picture's `a:blip`, the drawing in an `asvg:svgBlip` extension beside it. A reader that
stops at `a:blip` reports an SVG saved by Word as its raster twin — a picture of the
wrong thing, at a resolution the file never chose. Both office readers therefore read
the extension and prefer it: the drawing is the picture the file asks for, and the
fallback is only what an old reader shows. The two forms are proved separately — the
`gif_svg_pictures` pair puts the SVG where LibreOffice keeps it (the picture's own
`r:embed`, as its re-save does), and `test-data/svg_blip_pictures.docx` and
`svg_blip_pictures.pptx` put it where Word keeps it: a grey PNG fallback part, the
drawing beside it, and the extension naming it. One end-to-end test holds each
conversion to a picture drawn from the drawing — 144 × 86 at the frame the document
places it in, with the diagram's panel and stroke on it — after checking the fixture
still carries the extension and the fallback it sits beside — a package that lost its `svgBlip` would otherwise pass by testing the
raster alone. A `svgBlip` naming something that is not a drawing, or a part the package
does not hold, leaves the fallback as the picture, which is what the fallback is for.
The two spellings are the same picture, so the script writes a twin of each fixture —
`test-data/svg_embed_pictures.docx` and `svg_embed_pictures.pptx`, the same sentences
and the same 40 × 24 drawing, with the SVG as the picture's own `r:embed` and no raster
beside it — and one end-to-end test converts both variants of each format under one name
and holds the two JSON documents to equality, and the two HTML renders too. The input's
`binary_hash` is the one field that must differ, because the bytes do: if it did not, the
test would be comparing a package with itself.

A drawing states its own viewport too, and a frame need not share its proportions. The
root's `preserveAspectRatio` decides how the drawing meets a frame of another shape: the
default `xMidYMid meet` scales both axes alike, centres the drawing and leaves the rest
transparent, while `none` scales each axis on its own so the drawing fills the frame.
Every other SVG fixture here states no attribute, so they all ask the default;
`test-data/svg_stretch_picture.docx` asks `none`, placing a 40 × 24 drawing split red on
the left and blue on the right in a 2.5 × 0.5 in frame — five times as wide as it is tall,
where the drawing is five to three. Read as the default fit, the drawing would be a third
of the frame's width with 120 pixels of transparent margin at either side; read
as stated, it is the whole 360 × 72 frame, red on the left half and blue on the right. One
end-to-end test first holds the package to a drawing that states the fit it is about — a
fixture that lost the attribute would otherwise pass by looking like a default fit — then
converts it and holds the written PNG, 360 × 72 at 144 dpi, to the ink at both margins
and both corners, which only the stretch puts there.

A converter that keeps an older Word or PowerPoint happy states the picture the other
way round as well: the drawing in an `mc:AlternateContent`'s `mc:Choice`, and in
`mc:Fallback` the copy that application would have shown — a `w:pict` with a VML
`v:imagedata` in Word, a second `p:pic` in PowerPoint. The fallback is not a second
picture, so both readers take the choice and read the fallback only when the choice
names a part the package does not hold, which is the one case a fallback exists for.
`test-data/mc_svg_pictures.docx` and `mc_svg_pictures.pptx` carry two such pictures:
the first states `images/diagram.svg` in its choice and a grey box in its fallback, the
second names a drawing that is not in the package, so its fallback — an orange 60 × 36
raster — is the picture the document means. One end-to-end test checks each fixture
still states both branches before it converts, then holds the conversion to exactly two
pictures: the drawing played at its 40 × 24 frame, so a `image/png` of 80 × 48, and the
fallback's pixels as a PNG at 60 × 36, with the sentences around them read once each.

A converter can also state the picture only for an older reader: `mc:AlternateContent`
with no `mc:Choice`, only `mc:Fallback`. The fallback is then the picture, not a copy
beside one. `test-data/mc_fallback_pictures.docx` and `mc_fallback_pictures.pptx` carry
60 × 36 orange rasters that way, each with a sentence before and after it: in the body,
and in the DOCX inside a table's cell, a cell of a table nested in another table's cell, a
VML text box, and a header part — the last read as furniture — and on the slide inside a
group. One end-to-end test holds each package to that shape — the fallback branches and no
choice, the body's picture outside every container, and each container's picture inside
it — then converts it to the orange rasters at 60 × 36 with each one read where its
container stands. The DOCX-specific test follows the same picture through the nested
table, the text box and the header: the two tables' cells are rich cells whose groups
hold their pictures, the outer table's cell references the group holding the nested
table (so the nested table is read inside the cell that holds it, not as a table of the
body), the text box's picture stands in the body between its sentences, and the
header's is furniture after the header's words. The re-save test asks LibreOffice
to open both packages and records that every fallback survives — as branches in the DOCX
(the cells' still in their cells, the text box's in its box, the header's in a header
part) and as the slide's pictures in the PPTX (the group's still in its group).

A picture can also be a shape's fill rather than a picture of its own: an `a:blipFill` in
a `p:sp`'s or a `wps:wsp`'s properties names its media with an `a:blip`, and an SVG in
that fill is stated the way a picture states one — the raster on the blip, the drawing in
the `asvg:svgBlip` extension beside it. A fill is part of the shape that holds it, so
both readers paint the shape rather than handing over the fill's picture: the drawing —
or the raster when there is no drawing — is decoded and drawn *through* the shape's own
outline, stretched across its frame, cropped by whatever `a:srcRect` states, turned with
the shape, with the shape's line over it and its words still the document's text. What
leaves the reader is one picture of what the page shows, at the shape's own size — not
the bare image standing where the shape is — which is what both readers reported before:
PowerPoint left such a shape with a box and then with the fill's raw picture. An SVG fill
is played at the fill's frame, the same 144 dpi every drawing in a package is played at;
a raster is stretched, so a 60 × 36 image in a 60 × 36 point shape comes out at 120 × 72
pixels. `test-data/blip_fill_shapes.docx` and `blip_fill_shapes.pptx` hold two filled
shapes: one filled with `images/diagram.svg` behind a grey raster fallback and carrying
the words `Inside the fill.`, and one filled with an orange 60 × 36 raster that states no
drawing; one end-to-end test holds each conversion to the drawing painted into the
shape's rounded rectangle (288 × 173, with the frame's corner the page rather than the
panel), the raster painted into its shape (120 × 72, its corner likewise), and one
reading of each sentence.
A bullet can be a picture rather than a character — `w:numPicBullet`, named by the
level's `w:lvlPicBulletId`, in Word; `a:buBlip` on the paragraph in PowerPoint. It is
the one part of a list item that is a drawing, and it used to be the one picture an
office package lost whole: the item was reported with an empty marker and nothing where
the picture was. Both readers now report it as the picture it is, ahead of the words it
opens, which carry no glyph of their own. A bullet stated as an SVG is played like any
other drawing, at the size the bullet is drawn in: Word's `wp:extent` (18 points in the
fixture is a 36 x 36 PNG at 144 dpi) or the paragraph's type size in PowerPoint (24
points is 48 x 48). A Word bullet's image is a part of `numbering.xml` rather than of the
document, so it is named from that part's own `.rels` — the document's `rId1` is the
numbering part itself, and a reader that looks only there loses the bullet.
`calibre_demo.docx` in `test-data/real` is the real-world case: a 9-point GIF bullet
become a 12 x 12 PNG at 96 dpi, and a list item whose `•` marker is gone because the
picture is the marker. A slide states no line boxes, so a slide bullet's square sits at
the corner the shape's text starts from — where the first item's marker is drawn, and as
close as the file comes to saying where the rest are. `test-data/picture_bullets.docx`
and `picture_bullets.pptx` each hold two such bullets, one the drawing behind a grey
raster fallback and one a plain raster; one end-to-end test holds each conversion to the
drawing's pixels at the bullet's own size, the raster's pixels unchanged, two list items
with no glyph, and a render naming both PNGs.

A background is the other place a slide keeps a picture without a `p:pic`: PowerPoint
writes a photographic backdrop in `p:cSld/p:bg/p:bgPr/a:blipFill`, outside the shape
tree, naming a media part through the slide's own relationships. A reader that walks
only the tree loses the picture whole, so the page now keeps it: the picture is written
beside the JSON under the slide's name and recorded as that page's `image`, not as a
picture item in the slide's group. `test-data/background_deck.pptx`
has two slides — the first backed by `images/scan.jpg`, the second with no `p:bg` — and
one end-to-end test holds the first page to a 620 × 350 `image/jpeg` at the 62 dpi its
620 pixels across the 10-inch page imply, the second to no image, both sentences to
their text, and `pictures` to none.

A slide that states no `p:bg` inherits the one behind it: from its layout first, then
from the layout's master. That is where a deck usually keeps a photographic backdrop —
one picture behind every slide — so a reader that looked at the slide's own part alone
would come back with bare pages for a deck that was all photograph. This reader resolves
the chain slide → layout → master and takes the first background one of them states,
reading its `a:blipFill` through that part's own relationships; a `p:bg` stating a
colour, a gradient or a theme fill (`p:bgRef`) is a background but not a picture, and the
chain stops there rather than reaching past it to a picture behind.
`test-data/master_background_deck.pptx` is the fixture: three slides, the photograph in
the master, the first two slides stating nothing of their own, the third a colour — held
by one test of the parser and one end-to-end test to the first two pages' 620 × 350
image and the third page's none, with no picture item anywhere.

`test-data/layout_background_deck.pptx` asks the question in the middle of that chain:
the master states one photograph (`images/scan.webp`) and the blank layout states
another (`images/scan.jpg`), so its first two slides, which state nothing, are backed by
the layout's JPEG rather than the master's WebP — the nearer level is the one behind the
page — and the third, which states a colour, by neither. The two pictures are the same
scan in two encodings on purpose: a reading that reached straight through to the master
would carry the other bytes and the other media type. One test of the parser holds the
first two slides to the layout's bytes at the full slide box and the third to no
background, and one end-to-end test holds the first two pages' page images to the same
620 × 350 `image/jpeg` at 62 dpi with the layout's own bytes written beside the JSON and
the third page's to nothing, no picture item anywhere.

`test-data/layout_colour_background_deck.pptx` puts a colour at that level instead: the
master states a photograph (`images/scan.webp`) and the blank layout a colour, so the
first two slides, which state nothing, have no backdrop at all — the photograph is in
the same package and must not be reached past a part that states a background, because a
colour is a background and not a picture. The third slide states a picture of its own
(`images/scan.jpg`), the nearest level of all, so its page is that JPEG: the two bare
pages say the layout stopped the chain, not that the reading lost backgrounds. One test
of the parser and one end-to-end test hold it there.

A GIF arriving as the input file is an animation rather than one picture, and every
frame of it is a page: `image-parser`'s `gif_frames` decodes them all, compositing each
on the logical screen the way a player shows it and laying it on white where it leaves
the screen untouched, and the CLI writes one page per frame — the screen's size at the
72 dpi a GIF's missing resolution means — with each frame's words read from its own
pixels. That is a deliberate departure from Docling, whose own backend opens a GIF with
Pillow and reads the first frame only: a page that exists in the file is not dropped for
being later than the first. `test-data/images/turning.gif` is three frames that each say
their own name, and one end-to-end test holds the conversion to three pages of the
screen's size with each page showing its own frame's words and no other's.

A WebP arriving as the input file is an animation too when its header says so:
`webp_frames` reads every `ANMF` frame and composites it on the `ANIM` canvas, and the
CLI writes one page per frame at that canvas's size, 72 dpi because WebP states no
resolution. A still WebP is the one page it always was.
`test-data/images/turning.webp` is the three-frame fixture, and one end-to-end test
holds its conversion to three pages with each page's own words and no other's.
An animation can carry an EXIF orientation tag as a still can, and the tag belongs
to the file as the `ANIM` canvas does: every frame was stored the same way, so
`turning_turned.webp` — the same three pages stored a quarter turn anticlockwise on
a 160 × 480 canvas, with tag 6 saying to turn them back — is three upright 480 × 160
pages, each with its own words and no other's. A reader that applied the tag to the
frame it measured and not to the rest would report page one upright and pages two
and three on their side; a second end-to-end test holds every page to the upright
grid, to its own lines and to line boxes that are horizontal, which only upright
pixels give. The reader reports the stored grid and the tag, so the turn is applied
to the whole strip by the caller rather than to one frame by the parser.

A TIFF is the same story in the shape a scanner writes: its chain of image file
directories is a batch of sheets, and `image-parser`'s `tiff_frames` walks it, giving
each sheet as its own PNG at its own grid. A three-directory TIFF is three pages, each
measured from its own pixels at the file's declared resolution — a batch's sheets need
not be the same size, so a page is not the first page's box with different words on it —
and each recognised on its own pixels. Each directory's orientation tag is its own
statement too: a sheet tagged 6 is turned 90° clockwise, the next sheet tagged 1 stays
as stored, and the first sheet's tag is never applied to the rest.
`test-data/images/pages.tiff` is three sheets written that way, the last a different
shape, each saying its own name; one end-to-end
test holds the conversion to three pages at those three sizes with each page showing its
own words and no other's, and a single-directory TIFF is still the one page it always
was. `test-data/images/pages_turned.tiff` is the same batch fed the other way up: its
directories are tagged 1, 6 and 8, so the second and third sheets are stored a quarter
turn each way and are 160 × 480 until their own tag is applied — every page is 480 × 160
once turned. A second end-to-end test converts it with the recogniser on and holds each
page to the upright grid, to its own two lines (never another page's) and to line boxes
that are horizontal and inside the page, which is what only upright pixels give.

A WebP and a JPEG 2000 are the other way round: this project decodes both, but Docling
cannot name either in an `image` reference, so a package embedding one has to hand on a
PNG. `test-data/webp_jp2_pictures.docx` and `webp_jp2_pictures.pptx` each embed
`images/chart.webp`, `images/diagram.jp2` and `images/diagram.jpc`, and one end-to-end
test holds each conversion to three `image/png` pictures at the 60 × 36, 40 × 24 and
36 × 22 grids their encodings state, with a real PNG beside the JSON whose bytes are
exactly what the same picture transcodes to on its own — the drawing's pixels, not a
blank — and a render naming all three files. The `.jpc` is a bare codestream, with no
container to hold an `ihdr`, so its grid can only have come from its `SIZ` marker.
Neither office library can place any of the encodings, so those parts are assembled the
same way the SVG's is, and an office application is asked about them: LibreOffice opens
both fixtures, keeps the sentences around the pictures, and draws the WebP as a 60 × 36
PNG of its own bar colours — but it has no JPEG 2000 filter, so both JPEG 2000 parts are
dropped, and the re-save test states that rather than leaving it unsaid.

An embedded picture can be stored the way the camera wrote it: a phone saves a portrait
page as a landscape grid and says in EXIF how to turn it back. Word and PowerPoint
honour that tag when they display the picture, but the package keeps the stored pixels,
so a reader that wrote them out would produce the sideways picture no user ever saw.
The DOCX and PPTX readers turn such a picture upright before it leaves the parser: the
bytes beside the JSON are re-encoded upright (a JPEG stays a JPEG at quality 90,
everything else becomes a lossless PNG), the `size` the model reports is the upright
one, and the tag is gone because the pixels no longer need it. A picture whose tag says
it is already upright is not touched — no decode, no re-encode.

A DOCX drawing is the exception that is no longer an exception: Word stores a vector
drawing — and the preview of an embedded object such as a MathType equation — as an
Enhanced Metafile (`image/emf`) or a Windows Metafile (`image/wmf`), neither of which can
be named in an `image` reference. `crates/metafile` plays their GDI records into a raster
and the drawing is carried as a PNG at the size the document asks for, so it is a picture
with pixels rather than a box with no image. Text inside such a drawing is drawn in a
built-in 5×7 font, because a metafile names a font and fetches its outlines from the
system; the glyph positions, sizes and colours are the file's.

Such a record can embed a bitmap (`STRETCHDIBITS`, `DIBSTRETCHBLT`), and an old Word
RTF can carry the same `BITMAPINFO` with no file header in front of it. The DIB header
has one parser, in `crates/metafile`: the renderer reads where the palette and the
pixels are from it, and so does the RTF reader's BMP wrapper, so the two cannot count
a `biClrUsed`, a mask stored outside a 40-byte header or a core header's three-byte
palette entries differently.

Two committed RTFs hold that to a file a reader can open, both showing
`test-data/images/rtf_bitmap.bmp`: `word_bitmap.rtf` is in Word's own shape —
`\pict\wbitmap0` and the DIB's hex, no `BITMAPFILEHEADER` — and its conversion must
write a BMP beside the JSON that is the source file byte for byte;
`rtf_bitmap.rtf` is LibreOffice's own output from a DOCX holding the same bitmap,
and its picture must come out readable with the source's pixels. The second is a
real word processor's file, which is also why the first exists: LibreOffice spells
every embedded raster `\pngblip`, so a word-processor-written RTF cannot carry a
DIB on a machine that has no Word. `test-data/make_rtf_fixtures.py` writes both and
says so.

A Word file can also state a shape rather than a picture: an older file — and the
VML copy beside a modern DrawingML shape — keeps autoshapes, groups and WordArt in
`w:pict` markup, with no image part behind them at all. Word paints those, so they
are painted here: a shape's box, oval, rounded box, line, polyline or its own `path`
(with the preset's `@` formulas resolved) is filled and stroked at 144 dpi, a group's
`coordsize` places its children, and a WordArt's `v:textpath` string is drawn —
stretched across its shape, as WordArt stretches it — in the same built-in 5×7 font,
because a WordArt names a font family and no glyphs. The words inside a shape's
textbox are the document's own text and stay where they were read; only a WordArt's
string, which exists nowhere else, is painted. A VML autoshape is shaded by its own
markup too: a `v:fill type="gradient"` (or `gradientRadial`) is a two-colour ramp
across the shape rather than its first colour painted flat, with `opacity`/`opacity2`,
`angle` and `focusposition` read, a linear ramp's `focus` moving the middle of the
blend (so 50% puts the second colour in the middle with the first at both ends, and
100% reverses the ramp) and a radial ramp's `focussize` sizing the inner region its
first stop fills; a `v:shadow on="t"` is the silhouette moved by its
`offset` and laid down at its `opacity` under the ink; and a `v:stroke`
`startarrow`/`endarrow` is a block, classic, oval, diamond or open head, sized from
the line's own width by `*arrowwidth` and `*arrowlength`, with the stroke stopped at
its base.

A modern file states the same kinds of shape in DrawingML instead: a `wps:wsp` inside
`w:drawing`, with its outline in `a:prstGeom` (or its own `a:custGeom` path), its box
in `a:xfrm` and its colours in `wps:style`'s `a:fillRef`/`a:lnRef` — theme slots such
as `accent1`, resolved against the package's `word/theme/theme1.xml`, with their
`shade`/`tint` applied. LibreOffice writes every shape that way, and Word does
whenever it needs no compatibility copy, so a reader that looked only for an image
relationship left those shapes out of the document entirely. They are painted here
too: a shape is placed by its transform (mirrored and turned as the transform says),
a `wpg:wgp` group maps its children's coordinate space onto its frame, boxes, ovals,
polygons, stars, arrows, chevrons and callouts are filled and stroked from the file's
own colours, and a shape's own `a:custGeom` path is read — line and curve segments
alike — from its `a:moveTo`, `a:lnTo`, `a:cubicBezTo` and `a:close` commands.

The preset library is the one Office states rather than a handful of boxes: rounded and
snipped rectangles whose corners are treated one by one, rings and frames (a hole is
a contour inside another, under the alternate fill rule), the mathematical signs,
brackets and braces, stars of 12 to 32 points, the flowchart symbols — a sort is a
decision's diamond with its bar drawn as a line of the drawing, in the shape's line
colour rather than its fill, so the two presets are not the same picture — cylinders,
cubes and bevels — whose rim and front edges are lines of the drawing, stroked but
never filled — the arrows with a bend or a notch, the cloud, heart, moon, sun, smiley
and tear, the arcs, pies, chords and block arcs, and the straight, bent and curved
connectors. A preset outside it is still no picture rather than a box put in its
place.

A shape is shaded the way the file states, not flushed to one colour. An
`a:gradFill` is the ramp it names: its stops at their own positions and opacities,
running along the `a:lin` angle — taken in the shape's own proportions, or at its
true angle for `scaled="0"` — or out from the middle of the shape for an `a:path`
(`circle`, `rect`, `shape`), so a wide box fades wider than it does tall. An
`a:effectLst`'s `a:outerShdw` is the silhouette of the shape, moved by `dist` along
`dir`, softened over `blurRad` and laid down at its own `a:alpha` before the shape is
painted, so the ink reaches past the frame and the canvas grows to hold it. And a
line's `a:headEnd`/`a:tailEnd` is a head at that end: a triangle, a stealth notched
back into its base, a diamond, an oval, or the open arrow's two strokes, sized from
the line's own width (`sm`/`med`/`lg` being 2×, 3× and 5× of it), with the stroke
stopped at the head's base so a concave head shows the surface behind it rather than
the line running through it.

The VML copy beside a DrawingML shape is read only when the DrawingML gave no picture — a
shape this does not paint, or a text box with no ink — so a modern file's shapes are
not reported twice, and a Word horizontal rule (`o:hr`) is a paragraph's line rather
than a picture. Patterns, 3-D, effects other than an outer shadow (a glow, a
reflection, a bevel) and the words inside a shape are not drawn.

The same painter draws a **PowerPoint slide's** shapes, because the body is the same
DrawingML: a `p:sp` states its box in `a:xfrm`, its outline in `a:prstGeom`, its
colours in `p:spPr` or in `p:style`'s `a:fillRef`/`a:lnRef` against the master's theme
— which the reader follows from the slide's layout to its master and there to
`ppt/theme/theme1.xml` — and its words in a `p:txBody` that is never painted. A
connector is the one shape whose frame has no height (`a:ext cy="0"` for a line that
does not rise), so its box is its stroke rather than its frame; a group (`p:grpSp`) is
painted whole, one picture whose children are mapped into the group's frame; and the
whole of it lives in `crates/drawingml`, so a shape is read the same way whichever
format wrote it.

Every picture a Word, PowerPoint or Excel package carries leaves it as a PNG unless it
is already a PNG or a JPEG. That is the shape Docling's own office readers produce:
they open each picture with Pillow and write the frames back as PNG, so `image/bmp`,
`image/tiff` and `image/gif` are types their model never holds — and a BMP, a TIFF or
a GIF would otherwise be the one picture in a conversion that is not the kind a scan
or a photograph is stored in. The two encodings a camera or a scanner already writes
travel untouched; everything else that decodes is re-encoded losslessly, at the pixelgrid its own header states. An SVG is not decoded here, so it keeps its own type and
bytes and its size still comes from its root tag.

A workbook's picture is found the way the format says it is: the worksheet part names a
drawing part, that drawing's own relationship part names the media part each anchor
reaches, and the anchor — two cells, a cell and a size, or a position in EMU — says where
the picture sits. Its box is that rectangle in cells, the same units the sheet's tables
are placed in, offsets inside a cell included, so a picture narrower than a column is
the fraction it is and a picture above a table is read before it. A `graphicFrame`
anchor is not a picture: it names no `a:blip`, so it is read as what it frames. A
chart frame names a **chart part**, and a chart part is a drawing of data rather than
an image — the same part a deck's chart is, read by the same reader. So a workbook's
chart reaches the document the way a deck's does: a `picture` item classified as the
chart family it plots (`bar_chart`, `line_chart`, `pie_chart`, `scatter_chart`,
`other_chart` from its first plot), with the numbers it plots as its
`tabular_chart.chart_data` table — a header row naming the series over one column
each, the categories down the first column, and one row per category, gaps left as
empty cells — and the chart's own title as a `caption` item the picture points back
at. No image is written for it: what a chart says is the data, not pixels of it. The
chart part may carry its numbers in caches (`c:numCache`, which Excel and PowerPoint
write) or only in the references (`'Quarterly'!$B$2:$B$4`, which a scripted writer
leaves), so a reference with no cache is resolved against the workbook's own cells —
its own sheet's or another sheet's — down a column and across a row, as a spreadsheet
fills one. A reference nothing can follow (a defined name, another workbook) leaves
the chart with what it carried rather than inventing numbers. An embedded object — an
OLE frame — is still nothing this reads.

A Markdown file names its pictures, or — when it was written with `--image-export-mode
embedded` — carries their bytes in a `data:` URI, and an HTML page's `<img src>` is the
same two cases; both readers therefore hand one loader the same `source`. A third case
carries nothing: a page written with `--image-export-mode placeholder` says only that a
picture stood here, in the comment `<!-- image -->`, and both readers take that mark as
the picture — an item with no image behind it — rather than as the words it is spelled
with. Any other comment is invisible, as a browser renders it; the writer's own mark is
the one string that means a picture. A path is
resolved against the document's own directory: the destination is percent-decoded
(`a%20shot.png` names `a shot.png`) and read, then copied beside the output as a picture
with the media type the bytes or the extension name and the pixel size the format
declares. A `data:` URI is decoded instead — base64, the form both writers use, or the
percent-encoded payload RFC 2397 also allows — its declared type checked against the
carried list, and its bytes measured and copied exactly as a file's are, so an embedded
render reads back as the picture it carried instead of its literal syntax. A format
whose pixels are not decoded here — a GIF, an SVG — is still a picture the document
named, so it is copied as it is, and its size is the one its own bytes state: a GIF's
header, an SVG's `width`/`height` or `viewBox`. That is what lets every format Docling
can name in an `image` travel — **PNG, JPEG, GIF, BMP, TIFF and SVG** each survive a
conversion and an HTML round trip with their bytes unchanged, where before the two that
are not decoded here lost their `image` and with it the page's `<img>`. A format Docling
cannot name is no longer a box with no `image` either: a **WebP or a JPEG 2000** picture —
from a flow file, a `data:` URI, a Word or PowerPoint package, or a workbook — is
decoded and written out as a PNG before it reaches the model, the same choice a Word
metafile already forces, so the document names a file a reader and Docling can both open.
A format whose pixels this cannot decode keeps its own type and bytes, and a metafile
outside a Word package is still carried as a picture with its box and no `image`. A
source that answers to neither — a remote
URL, a `data:` URI whose declared type is not a carried picture or whose payload does
not decode, a path that is not there — stays the literal `![alt](source)` words the file
contains: a converter must not fetch the network, and a picture with nothing behind it
is worse than the text. A picture inside a sentence splits it the way a link does — the
words before, the picture, the words after — which is what lets Markdown in, Markdown
out keep its pictures as well as its links.

An RTF picture is the same picture in a third spelling. `\pict` carries the image as
hex inside the document, under a blip control word that says what encoding it is —
`\pngblip`, `\jpegblip`, `\emfblip`, `\wmetafile` — and the reader turns those digits
back into bytes, hands them to the same flow loader a Markdown source reaches as a
`data:` URI under the blip's own type, and the conversion writes the image beside the
JSON with the Markdown naming it where the picture stood. A metafile blip is played
into a PNG at the `\picwgoal`/`\pichgoal` size the document asks for, as a DOCX
drawing is; a `\pict` whose blip names no encodable type is left out rather than
carried as a file nothing can open. Word writes a picture twice — `\shppict` for
readers that know it, `\nonshppict` as the fallback — and only the first is read, so
one picture stays one picture.

An RTF table is a grid, not a run of tabs. `\trowd` opens a row and states its
cells' right edges in `\cellxN`; `\cell` ends a cell and `\row` ends the row, so
the pair becomes a Docling table with the rows and columns the document printed.
The columns are the distinct boundaries across the whole table rather than a
count of cells in one row: a cell whose right edge is three boundaries along
covers three columns, which is how LibreOffice saves a merged cell without
writing `\clmgf` at all, and how a four-cell row stays four columns under a row
that merged two of them. `\clmgf`/`\clmrg` and `\clvmgf`/`\clvmrg` are read where
they do appear, as column and row spans. `\trhdr` is the table's own statement
of which row is its header, so a table that labels a group above its column
names heads the names rather than the group: the rows it marks outrank the
first-row default, and a table that marks none keeps that default. The reading
is gated on a word processor's own output rather than on hand-written samples:
`test-data/merged_table.rtf` is LibreOffice's export of a brief whose title, two
heading depths and merged table are held cell by cell, span included. A
paragraph under a style the `\stylesheet` defines with an `\outlinelevel` — or
names `Heading N` — is a `section_header` at that depth, and Word's `Title`
style is the document's `title`; the stylesheet is read before the body, so its
definitions never become page text.

DOCX equations (Office Math) are converted to LaTeX in the notation Docling uses:
`\left(x+a\right)^{n}=\sum_{k=0}^{n}\left(\genfrac{}{}{0pt}{}{n}{k}\right)x^{k}a^{n-k}`,
`\sqrt[3]{x+1}`, `\lim_{x \text{ \textrightarrow } 1}x`. The mapping covers fractions
(all four OMML styles), scripts and pre-scripts, delimiters, radicals, n-ary operators
with their limits, integrals, matrices, accents and bars, braces, function names,
limits, equation arrays and Greek/operator symbols. It is checked against Docling's own
conversion of the `math*` fixtures; a construct Docling does not name is passed through
its children rather than guessed.

## OCR

OCR is a required capability and is enabled by default. For scanned pages it extracts the
embedded image XObject, runs Tesseract over it, and emits line-level text items whose
bounding boxes are scaled into PDF points.

The backend is Tesseract via `leptess`. Word geometry comes from Tesseract's TSV output and
is grouped into lines. To build without Tesseract (the workspace then compiles, but OCR
returns an error):

```bash
cargo build -p pdfextractor-ocr-engine --no-default-features
```
