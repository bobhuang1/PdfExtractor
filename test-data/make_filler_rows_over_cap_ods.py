#!/usr/bin/env python3
"""Write `test-data/filler_rows_over_cap_calc.ods` with LibreOffice.

The existing `filler_rows_calc.ods` puts 1,000 repeated empty rows between two
blocks, just under the reader's 1,024-row materialization cap. This fixture puts
**2,000** rows there instead: the run still costs no cells, so it has to be
advanced over whole rather than truncated at the cap, or the South block lands
at row 1,024 instead of the row the producer wrote.

The document is authored as flat ODF and re-saved by LibreOffice as a package,
so the fixture is what a real producer writes.

Run from the repository root:

    python3 test-data/make_filler_rows_over_cap_ods.py
"""

import pathlib
import shutil
import subprocess
import tempfile
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
    <dc:title>Filler rows over the cap</dc:title>
  </office:meta>
  <office:styles/>
  <office:automatic-styles/>
  <office:body>
    <office:spreadsheet>
      <table:table table:name="Filler">
        <table:table-column table:number-columns-repeated="2"/>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>Region</text:p></table:table-cell>
          <table:table-cell office:value-type="string"><text:p>Total</text:p></table:table-cell>
        </table:table-row>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>North</text:p></table:table-cell>
          <table:table-cell office:value-type="float" office:value="10"><text:p>10</text:p></table:table-cell>
        </table:table-row>
        <table:table-row table:number-rows-repeated="2000">
          <table:table-cell table:number-columns-repeated="2"/>
        </table:table-row>
        <table:table-row>
          <table:table-cell office:value-type="string"><text:p>South</text:p></table:table-cell>
          <table:table-cell office:value-type="float" office:value="20"><text:p>20</text:p></table:table-cell>
        </table:table-row>
      </table:table>
    </office:spreadsheet>
  </office:body>
</office:document>
"""


def main() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        source = pathlib.Path(scratch) / "filler_rows_over_cap_calc.fods"
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
        target = pathlib.Path(scratch) / "filler_rows_over_cap_calc.ods"
        if not target.is_file():
            raise SystemExit("LibreOffice wrote no filler_rows_over_cap_calc.ods")
        content = zipfile.ZipFile(target).read("content.xml")
        if b'table:name="Filler"' not in content:
            raise SystemExit("LibreOffice dropped the Filler sheet")
        if b'table:number-rows-repeated="2000"' not in content:
            raise SystemExit("LibreOffice dropped the repeated filler rows")
        destination = ROOT / "filler_rows_over_cap_calc.ods"
        shutil.copy(target, destination)
    print(f"wrote {destination}")


if __name__ == "__main__":
    main()
