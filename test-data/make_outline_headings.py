#!/usr/bin/env python3
"""Write `outline_headings.pdf`, the fixture for outline-derived headings.

The page is deliberately plain: one 10 pt Courier face throughout, no numbering
in the text, and four standalone heading lines. The only thing that says those
lines are headings is the document's `/Outlines` tree, so a conversion that
recovers them is reading the outline rather than the font size. The tree also
nests `Scope` under `Introduction` and `Details` under `Method`, which pins the
levels: 1 for the top-level entries and 2 for their children.

The file is generated with the standard library only and is checked in, so the
test does not need this script at run time.
"""

from pathlib import Path

BODY = (
    "This body line is long enough to read as a full paragraph line in this fixture."
)
BODY = BODY.ljust(80)

LINES = [
    (680, "1. Introduction", 10),
    (662, BODY, 10),
    (648, BODY, 10),
    (620, "I. Scope", 10),
    (602, BODY, 10),
    (584, BODY, 10),
    (556, "2. Method", 10),
    (538, BODY, 10),
    (524, BODY, 10),
    (496, "II. Details", 10),
    (478, BODY, 10),
    (464, BODY, 10),
]


def content_stream() -> bytes:
    out = ["BT"]
    for y, text, size in LINES:
        escaped = text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
        out.append(f"/F1 {size} Tf 1 0 0 1 72 {y} Tm ({escaped}) Tj")
    out.append("ET")
    return ("\n".join(out) + "\n").encode("latin-1")


def obj(body: bytes) -> bytes:
    return body


def main() -> None:
    stream = content_stream()
    objects: list[bytes] = [
        # 1 catalog
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R "
        b"/Names << /Dests 11 0 R >> /PageMode /UseOutlines >>",
        # 2 page tree
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        # 3 page
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
        b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
        # 4 font
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
        # 5 content
        b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"endstream",
        # 6 outline root
        b"<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 4 >>",
        # 7 Introduction (child 8)
        b"<< /Title (1. Introduction) /Parent 6 0 R /First 8 0 R /Last 8 0 R "
        b"/Count 1 /Next 9 0 R /Dest [3 0 R /XYZ 0 680 0] >>",
        # 8 Scope, reached through the name tree below
        b"<< /Title (I. Scope) /Parent 7 0 R /Dest (scope) >>",
        # 9 Method (child 10)
        b"<< /Title (2. Method) /Parent 6 0 R /Prev 7 0 R /First 10 0 R "
        b"/Last 10 0 R /Count 1 /Dest [3 0 R /XYZ 0 556 0] >>",
        # 10 Details, reached through the name tree below
        b"<< /Title (II. Details) /Parent 9 0 R /Dest (details) >>",
        # 11 named destinations: a leaf of the catalog's `/Names /Dests` tree
        b"<< /Names [(scope) [3 0 R /XYZ 0 620 0] "
        b"(details) [3 0 R /XYZ 0 496 0]] >>",
    ]

    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0]
    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += f"{number} 0 obj\n".encode() + obj(body) + b"\nendobj\n"

    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n".encode()
    out += b"0000000000 65535 f \n"
    for offset in offsets[1:]:
        out += f"{offset:010d} 00000 n \n".encode()
    out += (
        f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\n"
        f"startxref\n{xref}\n%%EOF\n"
    ).encode()

    target = Path(__file__).with_name("outline_headings.pdf")
    target.write_bytes(out)
    print(f"wrote {target} ({len(out)} bytes)")


if __name__ == "__main__":
    main()
