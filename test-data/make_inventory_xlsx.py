#!/usr/bin/env python3
"""Write `test-data/inventory.xlsx`, the XLSX parser's fixture.

The workbook is written by `openpyxl` — a real XLSX writer, not a hand-built
package — so the file the parser reads is one a spreadsheet application could
have produced, and the script that writes it is the fixture's provenance:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_inventory_xlsx.py

What it holds, one case per worksheet:

* `Inventory` — a header row, shared strings and numbers, a boolean, a
  column-spanning merge (`B5:C5`), an empty row that splits the sheet into two
  data regions, and a formula whose cached value (Excel writes both) is what a
  reader sees: `=SUM(B2:B3)` is `15`.
* `Notes` — a small second table, and a date cell: the value a reader sees is
  `2024-03-05 00:00:00`, not the serial number the file stores.
* `Hidden` — a hidden sheet, which Docling keeps as an `invisible` content
  layer rather than dropping.

The values are fixed, so the file is byte-reproducible apart from the ZIP
timestamps `openpyxl` writes (`core.xml` and the member times); the test suite
pins the bytes of the parse, never of the container.
"""

from datetime import date
from pathlib import Path

from openpyxl import Workbook
from openpyxl.utils import get_column_letter
from openpyxl.worksheet.datavalidation import DataValidation

HERE = Path(__file__).resolve().parent


def main() -> None:
    workbook = Workbook()

    inventory = workbook.active
    inventory.title = "Inventory"
    inventory.append(["Item", "Qty", "Price", "In stock"])
    inventory.append(["Widget", 12, 1.5, True])
    inventory.append(["Gadget", 3, 22.75, False])
    inventory.append([])  # an empty row: the sheet's two data regions
    inventory["B5"] = "Shipping included"
    inventory.merge_cells("B5:C5")
    inventory["A6"] = "Total"
    inventory["B6"] = "=SUM(B2:B3)"

    notes = workbook.create_sheet("Notes")
    notes.append(["note", "value"])
    notes.append(["owner", "Ada"])
    # A real date, not a string: the file stores a serial number and a number
    # format, and the text a reader sees is the date.
    notes.append(["updated", date(2024, 3, 5)])
    notes["B3"].number_format = "yyyy-mm-dd"

    hidden = workbook.create_sheet("Hidden")
    hidden.append(["internal"])
    hidden.append([42])
    hidden.sheet_state = "hidden"

    # Column widths make the fixture look like a spreadsheet rather than a
    # row of identical columns; they are not read by the parser (a cell's
    # width is not text), so this only keeps the file honest to its name.
    for sheet in (inventory, notes):
        for column in range(1, 5):
            sheet.column_dimensions[get_column_letter(column)].width = 16

    # A data validation is part of a real sheet, and its XML lives in the
    # worksheet part beside the cells: a parser that assumed `sheetData` is the
    # only child would trip over the siblings this adds.
    validation = DataValidation(type="list", formula1='"yes,no"', allow_blank=True)
    inventory.add_data_validation(validation)
    validation.add("D2:D6")

    workbook.save(HERE / "inventory.xlsx")
    print(f"wrote {HERE / 'inventory.xlsx'}")


if __name__ == "__main__":
    main()
