#!/usr/bin/env python3
"""Write `test-data/booleans_calc.ods` with LibreOffice.

The other Calc fixtures have booleans only as part of a larger workbook. This
one isolates two cell shapes a reader must get right: a boolean whose displayed
words are Calc's `TRUE`/`FALSE` (while its value is Docling's `True`/`False`),
and rows that end before the table's last column, leaving trailing empty cells
that are padding rather than cells.

The document is authored as flat ODF and re-saved by LibreOffice as a package,
so the fixture is what a real producer writes.

Run from the repository root:

    python3 test-data/make_booleans_ods.py
"""

import pathlib
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent

FLAT = """<?xml version="1.0" encoding="UTF-8"?>
<office:document
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
    xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    office:version="1.3"
    office:mimetype="application/vnd.oasis.opendocument.spreadsheet">
  <office:meta>
    <dc:title>Boolean flags and trailing blanks</dc:title>
  </office:meta>
  <office:styles/>
  <office:automatic-styles/>
  <office:body>
    <office:spreadsheet>
      <table:table table:name="Flags">
        <table:table-column table:number-columns-repeated="4"/>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>Item</text:p></table:table-cell>
          <table:table-cell office:value-type="string"><text:p>Active</text:p></table:table-cell>
          <table:table-cell office:value-type="string"><text:p>Archived</text:p></table:table-cell>
          <table:table-cell office:value-type="string"><text:p>Checked</text:p></table:table-cell>
        </table:table-row>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>Alpha</text:p></table:table-cell>
          <table:table-cell office:value-type="boolean" office:boolean-value="true"><text:p>TRUE</text:p></table:table-cell>
          <table:table-cell office:value-type="boolean" office:boolean-value="false"><text:p>FALSE</text:p></table:table-cell>
          <table:table-cell/>
        </table:table-row>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>Beta</text:p></table:table-cell>
          <table:table-cell office:value-type="boolean" office:boolean-value="false"><text:p>FALSE</text:p></table:table-cell>
          <table:table-cell office:value-type="boolean" office:boolean-value="true"><text:p>TRUE</text:p></table:table-cell>
          <table:table-cell/>
        </table:table-row>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>Total</text:p></table:table-cell>
          <table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell>
          <table:table-cell/>
          <table:table-cell/>
        </table:table-row>
      </table:table>
    </office:spreadsheet>
  </office:body>
</office:document>
"""

TABLE = "{urn:oasis:names:tc:opendocument:xmlns:table:1.0}"
OFFICE = "{urn:oasis:names:tc:opendocument:xmlns:office:1.0}"
TEXT = "{urn:oasis:names:tc:opendocument:xmlns:text:1.0}"


def check_content(content: bytes) -> None:
    """Require the saved package to hold the shapes this fixture is for."""
    root = ET.fromstring(content)
    for table in root.iter(f"{TABLE}table"):
        if table.get(f"{TABLE}name") != "Flags":
            continue
        for row in table.findall(f"{TABLE}table-row"):
            cells = row.findall(f"{TABLE}table-cell")
            texts = ["".join(cell.itertext()).strip() for cell in cells]
            if "Total" in texts:
                # The row ends with two empty cells before Calc's filler,
                # which LibreOffice saves as one repeated empty cell.
                assert texts[:2] == ["Total", "1"], texts[:8]
                trailing = sum(
                    int(cell.get(f"{TABLE}number-columns-repeated", "1"))
                    for cell in cells[2:]
                )
                assert trailing >= 2, texts[:8]
                return
    raise SystemExit("LibreOffice dropped the trailing empty cells")


def main() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        source = pathlib.Path(scratch) / "booleans_calc.fods"
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
        target = pathlib.Path(scratch) / "booleans_calc.ods"
        if not target.is_file():
            raise SystemExit("LibreOffice wrote no booleans_calc.ods")
        content = zipfile.ZipFile(target).read("content.xml")
        for displayed, value in ((b"TRUE", b"true"), (b"FALSE", b"false")):
            if f"<text:p>{displayed.decode()}</text:p>".encode() not in content:
                raise SystemExit(f"LibreOffice dropped displayed {displayed.decode()}")
            if f'office:boolean-value="{value.decode()}"'.encode() not in content:
                raise SystemExit(f"LibreOffice dropped boolean value {value.decode()}")
        check_content(content)
        destination = ROOT / "booleans_calc.ods"
        shutil.copy(target, destination)
    print(f"wrote {destination}")


if __name__ == "__main__":
    main()
