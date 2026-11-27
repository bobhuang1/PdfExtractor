#!/usr/bin/env python3
"""Write `vector_text.pdf`: a page whose only text is drawn as vector art.

The invoice's lines are laid out here as ordinary text and then converted by
Ghostscript with `-dNoOutputFonts`, which draws every glyph as filled paths: the
page that comes out has **no font objects and no text-showing operators**, and
no raster image either. Read as it stands, the extractor finds nothing on it;
the page's words exist only as ink shapes, which is what a producer that
outlines its text (or a CAD/illustration export) leaves behind.

It is the fixture for vector-page rasterising: the page is rendered through
`pdftoppm` and recognised, so the flat page reads its four lines again.

`vector_text_figure.pdf` is the same idea with the lines set 16 points apart, so
the glyph clusters overlap and the parser groups the whole block into one
figure. That is what a real export does: outlining a 14-page paper with the same
Ghostscript command gave six or seven "figures" on every page — its paragraphs
and headings — and a page framed that way used to be refused by the rasteriser
because it counted a figure as a picture. The fixture pins the case: the page
has no text and one figure, and `--ocr-vector-pages` still reads its four lines.

Ghostscript stamps the output with the time it ran, so the bytes are normalised
here — the creation and modification dates are pinned, in the trailer and in the
XMP packet Ghostscript 10 writes, the document UUID and file identifier are
zeroed; all to the same width, so the cross-reference offsets still hold — and a regenerated file is
byte for byte the committed one.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_vector_text_pdf.py
"""

import re
import shutil
import subprocess
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TARGET = HERE / "vector_text.pdf"
FIGURE_TARGET = HERE / "vector_text_figure.pdf"

WIDTH, HEIGHT = 612, 792

#: The invoice's lines, and the font size each is set at.
LINES = [
    (72, 700, 26, "ACME SUPPLIES"),
    (72, 640, 17, "Invoice 12345"),
    (72, 600, 17, "Total Due 42.50"),
    (72, 560, 14, "Thank you for your business."),
]

#: The same lines set 16 points apart: at 17 points the glyph boxes touch, so
#: the parser's figure clustering merges the block into one box.
DENSE_LINES = [
    (72, 700, 17, "ACME SUPPLIES"),
    (72, 684, 17, "Invoice 12345"),
    (72, 668, 17, "Total Due 42.50"),
    (72, 652, 17, "Thank you for your business."),
]


def pdf(objects: list[bytes]) -> bytes:
    """Assemble the objects into a file with a classic xref table."""
    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += f"{number} 0 obj\n".encode()
        out += body
        out += b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n".encode()
    out += b"0000000000 65535 f \n"
    for offset in offsets:
        out += f"{offset:010d} 00000 n \n".encode()
    out += (
        f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\n"
        f"startxref\n{xref}\n%%EOF\n"
    ).encode()
    return bytes(out)


def content(lines: list[tuple[int, int, int, str]] = LINES) -> bytes:
    stream = "".join(
        f"BT /F1 {size} Tf {x} {y} Td ({text}) Tj ET\n"
        for x, y, size, text in lines
    )
    body = stream.encode("latin-1")
    return f"<< /Length {len(body)} >>\nstream\n".encode() + body + b"\nendstream"


def text_pdf(lines: list[tuple[int, int, int, str]] = LINES) -> bytes:
    return pdf(
        [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            (
                f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {WIDTH} {HEIGHT}]"
                " /Resources << /Font << /F1 4 0 R >> >>"
                " /Contents 5 0 R >>"
            ).encode(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica"
            b" /Encoding /WinAnsiEncoding >>",
            content(lines),
        ]
    )


def outlined(source: bytes) -> bytes:
    """`source` with every glyph drawn as a path instead of shown as text."""
    encoder = shutil.which("gs")
    if not encoder:
        raise SystemExit("vector_text.pdf needs Ghostscript (gs)")
    with tempfile.TemporaryDirectory() as directory:
        directory = Path(directory)
        text = directory / "text.pdf"
        flat = directory / "flat.pdf"
        text.write_bytes(source)
        result = subprocess.run(
            [
                encoder,
                "-q",
                "-dNOPAUSE",
                "-dBATCH",
                "-dSAFER",
                "-sDEVICE=pdfwrite",
                "-dNoOutputFonts",
                "-dCompatibilityLevel=1.4",
                "-dAutoRotatePages=/None",
                f"-sOutputFile={flat}",
                str(text),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        if result.returncode != 0:
            raise SystemExit(
                "Ghostscript could not outline the page: "
                + result.stderr.decode(errors="replace")
            )
        data = flat.read_bytes()

    # The date strings and the identifier Ghostscript stamps in are the only
    # run-to-run difference. Both are replaced in place — the same width, so
    # every cross-reference offset still points at its object. Ghostscript 10
    # also writes the dates into an XMP packet, which the trailer's `D:` form
    # does not cover.
    data = re.sub(rb"D:\d{14}", b"D:20260101000000", data)
    data = re.sub(
        rb"(<xmp:(?:CreateDate|ModifyDate)>)[^<]*(</xmp:(?:CreateDate|ModifyDate)>)",
        rb"\g<1>2026-01-01T00:00:00+00:00\g<2>",
        data,
    )
    data = re.sub(
        rb"uuid:[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}",
        b"uuid:00000000-0000-0000-0000-000000000000",
        data,
    )
    data = re.sub(rb"<[0-9A-Fa-f]{32}>", b"<" + b"0" * 32 + b">", data)
    return data


def main() -> None:
    TARGET.write_bytes(outlined(text_pdf(LINES)))
    print(f"wrote {TARGET} ({TARGET.stat().st_size} bytes)")
    FIGURE_TARGET.write_bytes(outlined(text_pdf(DENSE_LINES)))
    print(f"wrote {FIGURE_TARGET} ({FIGURE_TARGET.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
