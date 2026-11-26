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

Ghostscript stamps the output with the time it ran, so the bytes are normalised
here — the creation and modification dates are pinned and the file identifier is
zeroed, both to the same width so the cross-reference offsets still hold — and a
regenerated file is byte for byte the committed one.

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

WIDTH, HEIGHT = 612, 792

#: The invoice's lines, and the font size each is set at.
LINES = [
    (72, 700, 26, "ACME SUPPLIES"),
    (72, 640, 17, "Invoice 12345"),
    (72, 600, 17, "Total Due 42.50"),
    (72, 560, 14, "Thank you for your business."),
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


def content() -> bytes:
    stream = "".join(
        f"BT /F1 {size} Tf {x} {y} Td ({text}) Tj ET\n"
        for x, y, size, text in LINES
    )
    body = stream.encode("latin-1")
    return f"<< /Length {len(body)} >>\nstream\n".encode() + body + b"\nendstream"


def text_pdf() -> bytes:
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
            content(),
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
    # every cross-reference offset still points at its object.
    data = re.sub(rb"D:\d{14}", b"D:20260101000000", data)
    data = re.sub(rb"<[0-9A-Fa-f]{32}>", b"<" + b"0" * 32 + b">", data)
    return data


def main() -> None:
    TARGET.write_bytes(outlined(text_pdf()))
    print(f"wrote {TARGET} ({TARGET.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
