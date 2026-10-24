#!/usr/bin/env python3
"""Convert corpus documents with real Docling and store the reference JSON.

The references are what `our_output_agrees_with_real_docling_conversions`
compares against. They are generated here rather than committed for every
corpus document because Docling's PDF pipeline runs layout and table models on
CPU: a handful of documents is a few minutes, all of them is most of an hour.

    python regenerate.py calibre_demo.docx dummy.pdf

Needs the full `docling` package (not just docling-core), which pins its own
model versions, so the exact output depends on the Docling version:

    ~/.venvs/pdfextractor-docling/bin/python regenerate.py <names>

References generated with a docling-core older than the committed schema will
make the golden comparison noisy; regenerate them when the schema is updated.
"""

import json
import sys
from pathlib import Path

REPOSITORY = Path(__file__).resolve().parents[2]
CORPUS = REPOSITORY / "test-data" / "real"
# Hand-written fixtures live beside the corpus, not in it (`inventory.xlsx`,
# the XLSX parser's fixture; `invoice_scan.png`, the image parser's); a
# reference for one is generated the same way.
FIXTURES = REPOSITORY / "test-data"
OUT = Path(__file__).resolve().parent / "docling"


def locate(name: str) -> Path | None:
    for base in (CORPUS, FIXTURES):
        candidate = base / name
        if candidate.is_file():
            return candidate
    return None


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2

    from docling.document_converter import DocumentConverter

    OUT.mkdir(parents=True, exist_ok=True)
    converter = DocumentConverter()
    for name in argv[1:]:
        source = locate(name)
        if source is None:
            print(f"no such corpus document or fixture: {name}", file=sys.stderr)
            return 2
        print(f"converting {name} ...")
        result = converter.convert(source)
        document = result.document
        # `export_to_dict` is what Docling itself writes to `.json`.
        data = document.export_to_dict()
        target = OUT / f"{source.stem}.json"
        target.write_text(json.dumps(data, indent=2), encoding="utf-8")
        texts = len(data.get("texts", []))
        print(f"  -> {target.name}: {len(data.get('pages', {}))} page(s), {texts} text item(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
