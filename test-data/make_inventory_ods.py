#!/usr/bin/env python3
"""Write `test-data/inventory_calc.ods` with LibreOffice from `test-data/inventory.xlsx`.

The fixture is a Calc workbook rather than a hand-written package: the reader
must be held to what a real producer writes. `inventory.xlsx` already carries
three sheets (one hidden), a merge, an empty row that splits a sheet into two
data regions, numbers, a boolean, a date and a formula whose cached value is
what a reader sees, so converting it gives every case the ODS reader reads.

The file's stem is `inventory_calc`, not `inventory`, so it can have a Docling
reference of its own beside `inventory.xlsx`'s: the references and the ratchet
are keyed by stem, and two same-stem documents cannot both hold a row there.

Run from the repository root:

    python3 test-data/make_inventory_ods.py
"""

import pathlib
import shutil
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent


def main() -> None:
    source = ROOT / "inventory.xlsx"
    if not source.is_file():
        raise SystemExit(f"no {source}")
    with tempfile.TemporaryDirectory() as scratch:
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
        target = pathlib.Path(scratch) / "inventory.ods"
        if not target.is_file():
            raise SystemExit("LibreOffice wrote no inventory.ods")
        shutil.copy(target, ROOT / "inventory_calc.ods")
    print(f"wrote {ROOT / 'inventory_calc.ods'}")


if __name__ == "__main__":
    main()
