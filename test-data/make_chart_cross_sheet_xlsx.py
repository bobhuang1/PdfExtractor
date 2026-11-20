#!/usr/bin/env python3
"""Write `test-data/chart_cross_sheet.xlsx`, a workbook whose chart plots another sheet.

The workbook is written by `openpyxl` — a real XLSX writer, not a hand-built
package — so the chart part is the one a spreadsheet application writes:

* `Figures` holds the region names and the two quarters a chart plots.
* `Charts` holds the chart, anchored at `E2`, and a block of deliberately wrong
  `999` cells under the same header names. The chart's categories and both
  series name `Figures`, so a reader that ignored the sheet a range carries
  would plot the decoys instead.

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_chart_cross_sheet_xlsx.py

`test-data/make_sheet_drawings_ods.py` converts it to
`test-data/chart_cross_sheet_calc.ods`, the real Calc package the end-to-end
test reads.
"""

from pathlib import Path

from openpyxl import Workbook
from openpyxl.chart import BarChart, Reference

HERE = Path(__file__).resolve().parent
TARGET = HERE / "chart_cross_sheet.xlsx"


def main() -> None:
    workbook = Workbook()

    figures = workbook.active
    figures.title = "Figures"
    figures.append(["Region", "Q1", "Q2"])
    figures.append(["North", 120, 140])
    figures.append(["South", 90, 95])
    figures.append(["West", 60, 80])

    charts = workbook.create_sheet("Charts")
    # Wrong by design: the chart's ranges all name Figures, so none of these
    # cells may reach its data table.
    charts.append(["Wrong", "Wrong", "Wrong"])
    charts.append(["Wrong", 999, 999])
    charts.append(["Wrong", 999, 999])
    charts.append(["Wrong", 999, 999])

    bar = BarChart()
    bar.title = "Cross-sheet revenue"
    # The header row names the series and the first column names the categories,
    # and every reference is on Figures rather than on the sheet holding the chart.
    bar.add_data(
        Reference(figures, min_col=2, max_col=3, min_row=1, max_row=4),
        titles_from_data=True,
    )
    bar.set_categories(Reference(figures, min_col=1, min_row=2, max_row=4))
    bar.anchor = "E2"
    charts.add_chart(bar)

    workbook.save(TARGET)
    print(f"wrote {TARGET.relative_to(HERE)}")


if __name__ == "__main__":
    main()
