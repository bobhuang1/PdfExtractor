#!/usr/bin/env python3
"""Write `test-data/merged_table.rtf`, the merged-table RTF fixture.

The document is a brief: a title, a sentence of context, a `Heading 1` over a
table, a sentence under it, then a `Heading 2` and one more sentence. The table
merges both ways — a region runs down two rows, and the year's total is one cell
across both quarters — so a reading that loses a span loses content with it.

Everything in it is a real word processor's spelling rather than a script's:

* **The merges are LibreOffice's.** A horizontally merged cell is written as one
  cell ending at the far boundary, with no `\\clmgf` at all — the shape the reader
  has to infer a span from — while the vertical merge is stated with
  `\\clvmgf`/`\\clvmrg`. Both spellings have to agree with the DOCX reading of the
  same table, which is why the fixture is not hand-written RTF.
* **The headings are LibreOffice's too.** Its RTF export states no
  `\\outlinelevel`; it names its styles `Heading N` and `Title` in the
  `\\stylesheet`, which is the other way the reader recognises a heading, at the
  depth the name gives.
* **The header row is a default, not a marker.** The source DOCX marks the first
  row `w:tblHeader`, and LibreOffice's RTF export drops that, so the fixture says
  nothing about a header row and is read with the first-row fallback. What a
  `\\trhdr` marker does is covered by hand-written RTF in the reader's own tests,
  where the marker is the thing under test.

Regenerate with:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_merged_table_rtf.py

Needs LibreOffice (`soffice` on `PATH`, or `PDFE_SOFFICE` naming it) and
python-docx, both only to write the fixture: the readers under test never see
this script, and they read the committed bytes.

Two runs do not produce the same bytes — LibreOffice orders its font and colour
tables by whatever its hash tables happen to do — but they produce the same
document: converting both was checked to give identical JSON apart from the
file name and its `binary_hash`. So a regenerated fixture is worth diffing the
output of, even though its own diff is noise.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TARGET = HERE / "merged_table.rtf"
NAME = "merged_table"

# Region | Q1 | Q2, with the two halves of the year under the quarters.
CELLS = [
    ["Region", "Q1", "Q2"],
    ["North", "120", "140"],
    ["", "H1", "H2"],
    ["Total", "250", ""],
]


def soffice() -> str:
    explicit = os.environ.get("PDFE_SOFFICE")
    if explicit:
        return explicit
    found = shutil.which("soffice") or shutil.which("libreoffice")
    if not found:
        sys.exit("no soffice on PATH and PDFE_SOFFICE is not set")
    return found


def document():
    """The brief, as python-docx writes it before LibreOffice converts it."""
    from docx import Document
    from docx.oxml import OxmlElement

    brief = Document()
    brief.add_paragraph("Quarterly Brief", style="Title")
    brief.add_paragraph(
        "Revenue is grouped by region; the quarter cells merge across the year."
    )

    brief.add_heading("Revenue by region", level=1)
    table = brief.add_table(rows=len(CELLS), cols=len(CELLS[0]))
    table.style = "Table Grid"
    for row, words in enumerate(CELLS):
        for column, text in enumerate(words):
            table.cell(row, column).text = text

    # The header row repeats across pages: a marker LibreOffice's RTF export
    # does not carry, which is why the fixture's header row is the fallback.
    table.rows[0]._tr.get_or_add_trPr().append(OxmlElement("w:tblHeader"))

    # "North" runs down both halves of the year, and the total is one cell
    # across its two quarters.
    table.cell(1, 0).merge(table.cell(2, 0))
    table.cell(3, 1).merge(table.cell(3, 2))

    brief.add_paragraph("Figures in thousands.")
    brief.add_heading("Notes", level=2)
    brief.add_paragraph("All figures are unaudited.")
    return brief


def main() -> None:
    try:
        import docx  # noqa: F401
    except ImportError:
        sys.exit("python-docx is needed; run this with the Docling venv's python")

    with tempfile.TemporaryDirectory(prefix="pdfe-rtf-table-") as scratch:
        scratch = Path(scratch)
        source = scratch / f"{NAME}.docx"
        document().save(source)
        out = scratch / "out"
        out.mkdir()
        profile = scratch / "profile"
        result = subprocess.run(
            [
                soffice(),
                "--headless",
                "--norestore",
                f"-env:UserInstallation=file://{profile}",
                "--convert-to",
                "rtf",
                "--outdir",
                str(out),
                str(source),
            ],
            capture_output=True,
            text=True,
        )
        produced = out / f"{NAME}.rtf"
        if result.returncode != 0 or not produced.is_file():
            sys.exit(
                "LibreOffice could not write the fixture: "
                f"{result.stderr.strip() or result.stdout.strip() or 'no diagnostic'}"
            )
        shutil.copyfile(produced, TARGET)
    print(f"wrote {TARGET} ({TARGET.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
