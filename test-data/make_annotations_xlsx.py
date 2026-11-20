#!/usr/bin/env python3
"""Write `test-data/annotations.xlsx`, the XLSX comment fixture.

The workbook asks what `inventory.xlsx` does not: a comment on a cell. Both
comments name an author, because that is what the notes text records; one hangs
on a cell with words in it and the other on a cell with none, so a remark is not
lost with the text it is not part of. A second sheet carries a comment of its
own, so the reader has to follow each worksheet's comments relationship rather
than only the first sheet's.

The file is written by `openpyxl` — a real XLSX writer, not a hand-built
package — so the comments part is what a spreadsheet application writes:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_annotations_xlsx.py
"""

from pathlib import Path

from openpyxl import Workbook
from openpyxl.comments import Comment

HERE = Path(__file__).resolve().parent


def main() -> None:
    workbook = Workbook()

    review = workbook.active
    review.title = "Review"
    review.append(["Item", "Qty"])
    review.append(["Widget", 2])
    review.append(["Gadget", 1])
    review["B2"].comment = Comment("Checked against the invoice.", "Ada Lovelace")
    review["C4"].comment = Comment("An empty cell's remark.", "Grace Hopper")

    second = workbook.create_sheet("Second")
    second["A1"] = "Second sheet"
    second["A1"].comment = Comment("A remark on another sheet.", "Ada Lovelace")

    for sheet in (review, second):
        for column in ("A", "B", "C"):
            sheet.column_dimensions[column].width = 16

    target = HERE / "annotations.xlsx"
    workbook.save(target)
    print(f"wrote {target}")


if __name__ == "__main__":
    main()
