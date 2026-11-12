#!/usr/bin/env python3
"""Write the ODS picture and chart fixtures with LibreOffice.

Three XLSX fixtures already hold what the ODS reader must follow:

* `worksheet_pictures.xlsx` draws six pictures on `Pictures` — a BMP, a GIF, a
  PNG, a JPEG, and two JPEG 2000s — and one on `Summary`. LibreOffice keeps the
  four raster encodings on the first sheet and the BMP on the second; its JPEG
  2000 import filter drops the two codestreams, leaving frames with no image.
* `chart_sheet.xlsx` draws a bar chart on `Quarterly` and a line chart on
  `Trend`. LibreOffice writes each as an embedded object, `Object N/content.xml`,
  with the chart's cell ranges in the object and a replacement image beside it.
* `chart_cross_sheet.xlsx` draws a bar chart on `Charts` whose categories and
  series name cells on `Figures`, so the chart has to follow the sheet each
  range carries rather than read the sheet it is anchored on.

The `.ods` files are real Calc packages, not hand-written XML, so the reader is
held to the markup a producer writes. Their stems are `_calc` so they cannot
collide with the XLSX fixtures' rows in the Docling references, which are keyed
by stem.

Run from the repository root:

    python3 test-data/make_sheet_drawings_ods.py
"""

import pathlib
import shutil
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent

FIXTURES = [
    ("worksheet_pictures.xlsx", "worksheet_pictures_calc.ods"),
    ("chart_sheet.xlsx", "chart_sheet_calc.ods"),
    ("chart_cross_sheet.xlsx", "chart_cross_sheet_calc.ods"),
]


def main() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        for source_name, target_name in FIXTURES:
            source = ROOT / source_name
            if not source.is_file():
                raise SystemExit(f"no {source}")
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
            target = pathlib.Path(scratch) / f"{source.stem}.ods"
            if not target.is_file():
                raise SystemExit(f"LibreOffice wrote no {target.name}")
            shutil.copy(target, ROOT / target_name)
            print(f"wrote {ROOT / target_name}")


if __name__ == "__main__":
    main()
