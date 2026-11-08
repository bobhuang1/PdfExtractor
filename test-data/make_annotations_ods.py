#!/usr/bin/env python3
"""Write `test-data/annotations_calc.ods` with LibreOffice.

The workbook asks what the other Calc fixtures do not: a comment on a cell.
Both comments name an author and a time, because those are what the notes text
records; one hangs on a cell with words in it and the other on a cell with none,
so a remark is not lost with the text it is not part of.

The document is authored as flat ODF and re-saved by LibreOffice as a package,
so the reader is held to what a real producer writes — the same reason
`make_notes_odt.py` converts its flat source rather than building a package by
hand.

Run from the repository root:

    python3 test-data/make_annotations_ods.py
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
    office:mimetype="application/vnd.oasis.opendocument.spreadsheet">
  <office:styles/>
  <office:automatic-styles/>
  <office:body>
    <office:spreadsheet>
      <table:table table:name="Review">
        <table:table-column table:number-columns-repeated="3"/>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>Item</text:p></table:table-cell>
          <table:table-cell office:value-type="string"><text:p>Qty</text:p></table:table-cell>
        </table:table-row>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>Widget</text:p></table:table-cell>
          <table:table-cell office:value-type="float" office:value="2"><text:p>2</text:p>
            <office:annotation office:display="true">
              <dc:creator>Ada Lovelace</dc:creator>
              <dc:date>2024-03-05T09:15:00</dc:date>
              <text:p>Checked against the invoice.</text:p>
            </office:annotation>
          </table:table-cell>
        </table:table-row>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>Gadget</text:p></table:table-cell>
          <table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell>
        </table:table-row>
        <table:table-row>
          <table:table-cell/>
          <table:table-cell/>
          <table:table-cell>
            <office:annotation office:display="true">
              <dc:creator>Grace Hopper</dc:creator>
              <dc:date>2024-03-06T10:30:00</dc:date>
              <text:p>An empty cell's remark.</text:p>
            </office:annotation>
          </table:table-cell>
        </table:table-row>
      </table:table>
    </office:spreadsheet>
  </office:body>
</office:document>
"""


def main() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        source = pathlib.Path(scratch) / "annotations_calc.fods"
        source.write_text(FLAT, encoding="utf-8")
        subprocess.run(
            [
                "soffice",
                "--headless",
                "--convert-to",
                "ods",
                "--outdir",
                scratch,
                str(source),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        target = pathlib.Path(scratch) / "annotations_calc.ods"
        if not target.is_file():
            raise SystemExit("LibreOffice wrote no annotations_calc.ods")
        destination = ROOT / "annotations_calc.ods"
        shutil.copy(target, destination)
    print(f"wrote {destination}")


if __name__ == "__main__":
    main()
