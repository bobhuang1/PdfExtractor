#!/usr/bin/env python3
"""Write `test-data/notes_odt.odt` with LibreOffice.

The second Writer fixture asks what `brief_odt.odt` does not: a footnote, whose
text Docling reports as body content at the end of the document, and a table
inside a table cell, which a reader has to place rather than lose.

The document is authored as flat ODF and re-saved by LibreOffice as a package,
so the reader is held to what a real producer writes (the same reason
`make_brief_odt.py` converts `brief.docx` rather than writing a package). The
flat source is written by hand because the corpus has no DOCX with a nested
table and python-docx cannot author a footnote.

Run from the repository root:

    python3 test-data/make_notes_odt.py
"""

import pathlib
import shutil
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent

FLAT = """<?xml version="1.0" encoding="UTF-8"?>
<office:document
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
    xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    office:version="1.3"
    office:mimetype="application/vnd.oasis.opendocument.text">
  <office:meta>
    <dc:title>Notes and a nested table</dc:title>
  </office:meta>
  <office:styles/>
  <office:automatic-styles/>
  <office:body>
    <office:text>
      <text:h text:outline-level="1">Notes and nesting</text:h>
      <text:p>A note hangs off this sentence<text:note text:note-class="footnote" text:id="ftn1"><text:note-citation>1</text:note-citation><text:note-body><text:p>The footnote's own text.</text:p></text:note-body></text:note> and the paragraph continues after it.</text:p>
      <table:table table:name="Outer">
        <table:table-column table:number-columns-repeated="2"/>
        <table:table-row>
          <table:table-cell office:value-type="string">
            <text:p>Outer left</text:p>
          </table:table-cell>
          <table:table-cell office:value-type="string">
            <table:table table:name="Inner">
              <table:table-column table:number-columns-repeated="2"/>
              <table:table-row>
                <table:table-cell office:value-type="string"><text:p>Inner A1</text:p></table:table-cell>
                <table:table-cell office:value-type="string"><text:p>Inner B1</text:p></table:table-cell>
              </table:table-row>
              <table:table-row>
                <table:table-cell office:value-type="string"><text:p>Inner A2</text:p></table:table-cell>
                <table:table-cell office:value-type="string"><text:p>Inner B2</text:p></table:table-cell>
              </table:table-row>
            </table:table>
          </table:table-cell>
        </table:table-row>
      </table:table>
      <text:p>A closing paragraph.</text:p>
    </office:text>
  </office:body>
</office:document>
"""


def main() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        source = pathlib.Path(scratch) / "notes_odt.fodt"
        source.write_text(FLAT, encoding="utf-8")
        subprocess.run(
            [
                "soffice",
                "--headless",
                "--convert-to",
                "odt",
                "--outdir",
                scratch,
                str(source),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        target = pathlib.Path(scratch) / "notes_odt.odt"
        if not target.is_file():
            raise SystemExit("LibreOffice wrote no notes_odt.odt")
        destination = ROOT / "notes_odt.odt"
        shutil.copy(target, destination)
    print(f"wrote {destination}")


if __name__ == "__main__":
    main()
