#!/usr/bin/env python3
"""Write `crlf_text.docx`: literal CRLF and CR line endings inside text nodes.

XML 1.0 normalizes every line ending before a document is read: a CRLF and a
lone CR each become one LF, and an existing LF is left alone. The rule holds in
ordinary character data and inside a CDATA section alike, so a reader that hands
on the event's raw text comes out with `\\r` — which is what this fixture exists
to catch. `crates/docx-parser` reads it and holds the normalized text.

Word itself writes a line break as `<w:br/>`, and python-docx converts `\\n` in a
run to the same element, so no ordinary document carries a literal CRLF inside
`<w:t>` and the corpus cannot pin this path. The two paragraphs are therefore
written with placeholder words and `word/document.xml` is rewritten afterwards
with the raw bytes — the same completion `make_svg_stretch_docx.py` uses.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_crlf_text_docx.py
"""

import zipfile
from pathlib import Path

from docx import Document

HERE = Path(__file__).resolve().parent
DOCX = HERE / "crlf_text.docx"

TEXT_TOKEN = "CRLFTEXTTOKEN"
CDATA_TOKEN = "CRLFCDATATOKEN"

# All three line-ending forms in each place: CRLF and CR have to collapse to one
# LF, and an LF that is already one has to survive as one.
TEXT_CONTENT = "first\r\nsecond\rthird\nfourth"
CDATA_CONTENT = "fifth\r\nsixth\rseventh"


def write_placeholders(path: Path) -> None:
    document = Document()
    document.add_paragraph(TEXT_TOKEN)
    document.add_paragraph(CDATA_TOKEN)
    document.save(path)


def rewrite_document_part(path: Path) -> None:
    with zipfile.ZipFile(path) as package:
        parts = {name: package.read(name) for name in package.namelist()}
    document = parts["word/document.xml"]
    for token in (TEXT_TOKEN, CDATA_TOKEN):
        assert document.count(token.encode()) == 1, f"{token} is not in the part once"
    document = document.replace(TEXT_TOKEN.encode(), TEXT_CONTENT.encode())
    # The CDATA arm carries no words python-docx could have written, so the
    # placeholder is replaced with the section itself.
    document = document.replace(
        CDATA_TOKEN.encode(), f"<![CDATA[{CDATA_CONTENT}]]>".encode()
    )
    parts["word/document.xml"] = document
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(name, data)


def main() -> None:
    write_placeholders(DOCX)
    rewrite_document_part(DOCX)
    print(f"wrote {DOCX.relative_to(HERE.parent)}")


if __name__ == "__main__":
    main()
