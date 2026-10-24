#!/usr/bin/env python3
"""Write `test-data/chart_sheet.xlsx`, a workbook whose sheets draw charts.

The workbook is written by `openpyxl` — a real XLSX writer, not a hand-built
package — so the chart parts the parser meets are the ones a spreadsheet
application puts in the file: an `xl/drawings/drawing1.xml` per sheet with an
`xdr:graphicFrame` anchor, the frame's `a:graphicData` holding a `c:chart`
element that names the part, and the chart part itself under `xl/charts/` with
its `c:strCache`/`c:numCache` caches carrying the categories and the plotted
numbers:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_chart_sheet_xlsx.py

What it holds, and what each chart is there to prove:

* `Quarterly` — a header row and three rows of numbers, and a bar chart anchored
  at `E2` whose two series are named by the header cells (`Q1`, `Q2`) and whose
  categories are the three regions. It has a title, so the picture it becomes
  has a caption.
* `Trend` — one series over the same three regions as a line chart anchored at
  `D2`, and no title: a chart that says nothing about itself is still a picture
  of its data, and a workbook has to read the second sheet's chart, not only the
  first.

The numbers are fixed, so the file is byte-reproducible apart from the ZIP
timestamps `openpyxl` writes; the test suite pins the parse, never the container.
"""

from pathlib import Path

from openpyxl import Workbook
from openpyxl.chart import BarChart, LineChart, Reference

HERE = Path(__file__).resolve().parent
TARGET = HERE / "chart_sheet.xlsx"


def main() -> None:
    workbook = Workbook()

    quarterly = workbook.active
    quarterly.title = "Quarterly"
    quarterly.append(["Region", "Q1", "Q2"])
    quarterly.append(["North", 120, 140])
    quarterly.append(["South", 90, 95])
    quarterly.append(["West", 60, 80])

    bar = BarChart()
    bar.title = "Revenue by region"
    # The first row names the series; the rows under it are the numbers, and the
    # first column is what they are plotted against.
    bar.add_data(
        Reference(quarterly, min_col=2, max_col=3, min_row=1, max_row=4),
        titles_from_data=True,
    )
    bar.set_categories(Reference(quarterly, min_col=1, min_row=2, max_row=4))
    bar.anchor = "E2"
    quarterly.add_chart(bar)

    trend = workbook.create_sheet("Trend")
    trend.append(["Region", "Q1"])
    trend.append(["North", 120])
    trend.append(["South", 90])
    trend.append(["West", 60])

    line = LineChart()
    line.add_data(
        Reference(trend, min_col=2, min_row=1, max_row=4),
        titles_from_data=True,
    )
    line.set_categories(Reference(trend, min_col=1, min_row=2, max_row=4))
    line.anchor = "D2"
    trend.add_chart(line)

    workbook.save(TARGET)
    print(f"wrote {TARGET.relative_to(HERE)}")


if __name__ == "__main__":
    main()
