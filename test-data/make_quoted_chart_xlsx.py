#!/usr/bin/env python3
"""Write `test-data/chart_quoted_sheet.xlsx`, a chart whose sheet name needs quoting.

The workbook is written by `openpyxl`, a real XLSX writer:

* `Quarterly Totals` holds the region names and the two quarters the chart plots.
* `Dashboard` holds the chart, anchored at `E2`, and a block of deliberately
  wrong `999` cells. Every category and series range names `Quarterly Totals`;
  because that name has a space, LibreOffice writes each range with a quoted
  sheet name, the spelling the ODS reader has to resolve.

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_quoted_chart_xlsx.py

`test-data/make_sheet_drawings_ods.py` converts it to
`test-data/chart_quoted_sheet_calc.ods`, the real Calc package the end-to-end
test reads.
"""

from pathlib import Path

from openpyxl import Workbook
from openpyxl.chart import BarChart, Reference

HERE = Path(__file__).resolve().parent
TARGET = HERE / "chart_quoted_sheet.xlsx"


def main() -> None:
    workbook = Workbook()

    totals = workbook.active
    totals.title = "Quarterly Totals"
    totals.append(["Region", "Q1", "Q2"])
    totals.append(["North", 120, 140])
    totals.append(["South", 90, 95])
    totals.append(["West", 60, 80])

    dashboard = workbook.create_sheet("Dashboard")
    # Wrong by design: the chart's ranges all name Quarterly Totals.
    dashboard.append(["Wrong", "Wrong", "Wrong"])
    dashboard.append(["Wrong", 999, 999])
    dashboard.append(["Wrong", 999, 999])
    dashboard.append(["Wrong", 999, 999])

    bar = BarChart()
    bar.title = "Quoted ranges"
    bar.add_data(
        Reference(totals, min_col=2, max_col=3, min_row=1, max_row=4),
        titles_from_data=True,
    )
    bar.set_categories(Reference(totals, min_col=1, min_row=2, max_row=4))
    bar.anchor = "E2"
    dashboard.add_chart(bar)

    workbook.save(TARGET)
    print(f"wrote {TARGET.relative_to(HERE)}")


if __name__ == "__main__":
    main()
