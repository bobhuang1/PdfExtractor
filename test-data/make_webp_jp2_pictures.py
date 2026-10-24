#!/usr/bin/env python3
"""Write `webp_jp2_pictures.docx`, `webp_jp2_pictures.pptx` and their pictures.

A WebP and a JPEG 2000 are encodings this project decodes, but neither can be
named in a Docling `image` reference: `ImageRef` is validated against Docling's
own registry, which has no entry for `image/webp` or `image/jp2`. A Word or
PowerPoint package that embeds one therefore has to hand on a PNG, which is what
these fixtures are for — the office readers must transcode them, not leave a box
with no image, and the file written beside the JSON must be a real PNG of the
picture's own pixels.

The JPEG 2000 is carried in both spellings a scanner may write: the JP2
container, and the bare codestream named `.jpc`. The extension alone does not
state which one a part is — the readers sniff the first bytes — so a package
holding both is the place that rule is held in an office document rather than in
a standalone image.

Each package carries the same three pictures in the same order, with a sentence
before and after them:

* `images/chart.webp` — a 60 × 36 bar chart, lossless WebP, so its pixels are
  the drawing's and a transcoded PNG can be held to them;
* `images/diagram.jp2` — a 40 × 24 line drawing, lossless JPEG 2000, whose only
  statement of its size is the JP2 `ihdr` box;
* `images/diagram.jpc` — a 36 × 22 boxed graph, the same lossless encoding as a
  bare codestream, whose `SIZ` marker states its grid because it has no
  container to hold an `ihdr`.

Neither office library can place either encoding: python-docx rejects both with
`UnrecognizedImageError`, and python-pptx raises `unsupported image format`. The
pictures are therefore written into the saved package the way Word and
PowerPoint store a part they cannot themselves have placed there — as ordinary
pictures the library does write (a tiny PNG stub), followed by a rewrite
that renames the media part, replaces its bytes, points every relationship at
the new name and declares the extension's content type. The part names are
asserted, so a library that renames them fails in the generator rather than
silently turning the fixture back into a PNG.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_webp_jp2_pictures.py

The libraries are used only to write the fixtures; the readers under test never
see this script, and they read the committed bytes.

An office application is asked whether the assembled packages are ones it
accepts, which is the one question the pictures' hand placement raises:

    cargo test -p pdfextractor-cli --test cli_test \
        libreoffice_opens_and_resaves_the_webp_and_jp2_fixture_packages

opens both files with headless LibreOffice and writes them back, where
LibreOffice is installed, and holds what it wrote to the sentences around the
pictures and to the one raster it redrew: the chart at its own 60 × 36 grid with
the three bars' colours on it and the WebP's bytes nowhere in the package. The
JPEG 2000s are the half that comes back only as this project's reading —
LibreOffice has no JPEG 2000 import filter at all — and the test asserts that
absence for both spellings, so a LibreOffice that gains the filter fails it
instead of leaving the note to rot.
"""

import struct
import zipfile
from io import BytesIO
from pathlib import Path

from docx import Document
from docx.shared import Inches
from PIL import Image, ImageDraw
from pptx import Presentation
from pptx.util import Inches as PptxInches

HERE = Path(__file__).resolve().parent
IMAGES = HERE / "images"
WEBP = IMAGES / "chart.webp"
JP2 = IMAGES / "diagram.jp2"
JPC = IMAGES / "diagram.jpc"
DOCX = HERE / "webp_jp2_pictures.docx"
PPTX = HERE / "webp_jp2_pictures.pptx"

WEBP_SIZE = (60, 36)
JP2_SIZE = (40, 24)
JPC_SIZE = (36, 22)

# Three PNGs written only so the office library has something to place: every
# part is replaced before the fixture is saved. They differ in size because
# python-docx and python-pptx store a picture part once per file, so pictures of
# the same stub would share one part — and the fixture needs one part per
# encoding.
STUBS = [
    HERE / ".webp-jp2-stub-1.png",
    HERE / ".webp-jp2-stub-2.png",
    HERE / ".webp-jp2-stub-3.png",
]


def write_pictures() -> None:
    IMAGES.mkdir(exist_ok=True)

    # Lossless, so a transcoded PNG is the drawing's own pixels rather than a
    # decode of a lossy encode.
    chart = Image.new("RGB", WEBP_SIZE, (232, 238, 247))
    draw = ImageDraw.Draw(chart)
    for left, colour in [(4, "#2f6db5"), (22, "#6da33a"), (38, "#e0b23b")]:
        draw.rectangle([(left, 6), (left + 12, 29)], fill=colour)
    chart.save(WEBP, lossless=True)

    diagram = Image.new("RGB", JP2_SIZE, (232, 238, 247))
    draw = ImageDraw.Draw(diagram)
    draw.line([(4, 19), (14, 11), (22, 15), (36, 5)], fill="#2f6db5", width=2)
    diagram.save(JP2, irreversible=False)

    # The same encoding without its container. Pillow writes a JP2 even when
    # the name says `.jpc`, so the codestream is taken out of the box tree: a
    # bare codestream is what a scanner calls `.jpc`, and its `SIZ` marker is
    # the only place its grid is stated.
    graph = Image.new("RGB", JPC_SIZE, (232, 238, 247))
    draw = ImageDraw.Draw(graph)
    draw.rectangle([(3, 4), (32, 17)], outline="#c0392b", width=1)
    draw.line([(6, 14), (12, 8), (18, 13), (24, 6), (30, 11)], fill="#c0392b", width=2)
    buffer = BytesIO()
    graph.save(buffer, format="JPEG2000", irreversible=False)
    JPC.write_bytes(jp2_codestream(buffer.getvalue()))

    Image.new("RGB", (1, 1), "white").save(STUBS[0])
    Image.new("RGB", (2, 2), "white").save(STUBS[1])
    Image.new("RGB", (3, 3), "white").save(STUBS[2])


def write_docx(path: Path) -> None:
    document = Document()
    document.add_paragraph("Before the pictures.")
    document.add_picture(str(STUBS[0]), width=Inches(1.5), height=Inches(0.9))
    document.add_picture(str(STUBS[1]), width=Inches(1.0), height=Inches(0.6))
    document.add_picture(str(STUBS[2]), width=Inches(0.9), height=Inches(0.55))
    document.add_paragraph("After the pictures.")
    document.save(path)


def write_pptx(path: Path) -> None:
    presentation = Presentation()
    slide = presentation.slides.add_slide(presentation.slide_layouts[6])  # blank

    before = slide.shapes.add_textbox(
        PptxInches(0.8), PptxInches(0.4), PptxInches(4.0), PptxInches(0.4)
    )
    before.text_frame.text = "Before the pictures."

    slide.shapes.add_picture(
        str(STUBS[0]),
        PptxInches(0.8),
        PptxInches(1.0),
        width=PptxInches(1.5),
        height=PptxInches(0.9),
    )
    slide.shapes.add_picture(
        str(STUBS[1]),
        PptxInches(0.8),
        PptxInches(2.2),
        width=PptxInches(1.0),
        height=PptxInches(0.6),
    )
    slide.shapes.add_picture(
        str(STUBS[2]),
        PptxInches(0.8),
        PptxInches(3.0),
        width=PptxInches(0.9),
        height=PptxInches(0.55),
    )

    after = slide.shapes.add_textbox(
        PptxInches(0.8), PptxInches(3.9), PptxInches(4.0), PptxInches(0.4)
    )
    after.text_frame.text = "After the pictures."

    presentation.save(path)


def replace_stubs(path: Path) -> None:
    """Rename each stub's media part, and point every relationship at it.

    The office libraries name the parts by order (`image1.png` for the picture
    added first, `image2.png` for the second, `image3.png` for the third). That
    order is asserted rather than guessed: if a future library names them
    differently, the fixture is rebuilt deliberately instead of silently keeping
    a PNG where the test says WebP or JPEG 2000.
    """
    replacements = [
        ("image1.png", "image1.webp", WEBP.read_bytes(), "webp", "image/webp"),
        ("image2.png", "image2.jp2", JP2.read_bytes(), "jp2", "image/jp2"),
        ("image3.png", "image3.jpc", JPC.read_bytes(), "jpc", "image/jp2"),
    ]

    with zipfile.ZipFile(path) as package:
        parts = {name: package.read(name) for name in package.namelist()}

    for stub, renamed, payload, extension, content_type in replacements:
        media = [name for name in parts if name.endswith("/" + stub)]
        assert len(media) == 1, f"{path}: expected one {stub}, found {media}"
        media = media[0]
        new_part = media[: -len(stub)] + renamed

        parts[new_part] = payload
        del parts[media]

        # Only relationships name a media part; the drawing itself refers to the
        # relationship, not to the file.
        for name in [name for name in parts if name.endswith(".rels")]:
            text = parts[name].decode("utf-8")
            parts[name] = text.replace(stub, renamed).encode("utf-8")

        types = parts["[Content_Types].xml"].decode("utf-8")
        marker = f'Extension="{extension}"'
        if marker not in types:
            anchor = '<Default Extension="rels"'
            assert anchor in types, f"{path}: no rels default to insert beside"
            types = types.replace(
                anchor,
                f'<Default Extension="{extension}" ContentType="{content_type}"/>'
                + anchor,
            )
        parts["[Content_Types].xml"] = types.encode("utf-8")

    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(name, data)


def jp2_codestream(jp2: bytes) -> bytes:
    """The codestream a JP2 wraps: its `jp2c` box's payload, byte for byte.

    A bare codestream has no signature box, no `ftyp` and no `jp2h`, so no
    `ihdr` and no `resc` either: its `SIZ` marker states the grid, and nothing
    in a codestream states a density. This is the picture without the container,
    which is the form a scanner writes under `.jpc`.
    """
    pos = 0
    while pos + 8 <= len(jp2):
        length, kind = struct.unpack(">I4s", jp2[pos : pos + 8])
        header = 8
        if length == 1:
            length = struct.unpack(">Q", jp2[pos + 8 : pos + 16])[0]
            header = 16
        elif length == 0:
            length = len(jp2) - pos
        if kind == b"jp2c":
            return jp2[pos + header : pos + length]
        pos += length
    raise ValueError("the JP2 carries no jp2c box")


def main() -> None:
    write_pictures()
    write_docx(DOCX)
    write_pptx(PPTX)
    replace_stubs(DOCX)
    replace_stubs(PPTX)
    for stub in STUBS:
        stub.unlink()
    for path in (WEBP, JP2, JPC, DOCX, PPTX):
        print(f"wrote {path} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
