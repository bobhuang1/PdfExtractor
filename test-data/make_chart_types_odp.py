#!/usr/bin/env python3
"""Write `test-data/chart_types_impress.odp` with LibreOffice.

The fixture is a real Impress package whose chart objects isolate what the
ODP reader's inline tests describe: a pie, a line with two series, and a chart
with no `local-table`. The last object is made by deleting the cached table
from LibreOffice's own export, so the rest of the object is still what a
producer writes.

The Docling reference beside it is generated from this file, not written by
hand:

    ~/.venvs/pdfextractor-docling/bin/python test-data/make_chart_types_odp.py
    cd test-data/golden
    ~/.venvs/pdfextractor-docling/bin/python regenerate.py chart_types_impress.odp

Run from the repository root. `python-pptx` and LibreOffice's `soffice` are
needed to build the fixture.
"""

import pathlib
import re
import shutil
import subprocess
import tempfile
import zipfile

from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.enum.chart import XL_CHART_TYPE
from pptx.enum.text import PP_ALIGN
from pptx.util import Inches, Pt

ROOT = pathlib.Path(__file__).resolve().parent
TARGET = ROOT / "chart_types_impress.odp"


def add_title(slide, text: str) -> None:
    title = slide.shapes.title
    title.left = Inches(0.45)
    title.top = Inches(0.25)
    title.width = Inches(12.4)
    title.height = Inches(0.8)
    frame = title.text_frame
    frame.text = text
    paragraph = frame.paragraphs[0]
    paragraph.alignment = PP_ALIGN.LEFT
    run = paragraph.runs[0]
    run.font.size = Pt(30)
    run.font.bold = True


def add_caption(slide, left: float, text: str) -> None:
    box = slide.shapes.add_textbox(Inches(left), Inches(6.35), Inches(4.1), Inches(0.55))
    frame = box.text_frame
    frame.text = text
    frame.paragraphs[0].runs[0].font.size = Pt(14)


def add_chart(slide, kind, left: float, data: CategoryChartData, title: str):
    chart = slide.shapes.add_chart(
        kind, Inches(left), Inches(1.2), Inches(4.1), Inches(4.9), data
    ).chart
    chart.has_title = True
    chart.chart_title.text_frame.text = title
    chart.has_legend = True
    chart.legend.include_in_layout = False
    return chart


def main() -> None:
    presentation = Presentation()
    presentation.slide_width = Inches(13.333)
    presentation.slide_height = Inches(7.5)
    slide = presentation.slides.add_slide(presentation.slide_layouts[5])
    add_title(slide, "Chart families")

    pie = CategoryChartData()
    pie.categories = ["North America", "Europe", "Asia Pacific"]
    pie.add_series("Revenue", (120, 98, 74))
    add_chart(slide, XL_CHART_TYPE.PIE, 0.35, pie, "Revenue by region")
    add_caption(slide, 0.45, "Pie chart with a local table")

    line = CategoryChartData()
    line.categories = ["Q2", "Q3", "Q4"]
    line.add_series("North America", (120, 145, 150))
    line.add_series("Europe", (98, 121, 130))
    add_chart(slide, XL_CHART_TYPE.LINE_MARKERS, 4.55, line, "Quarterly trend")
    add_caption(slide, 4.65, "Line chart with two series")

    uncached = CategoryChartData()
    uncached.categories = ["North America", "Europe", "Asia Pacific"]
    uncached.add_series("Revenue", (120, 98, 74))
    add_chart(slide, XL_CHART_TYPE.COLUMN_CLUSTERED, 8.75, uncached, "No cached data")
    add_caption(slide, 8.85, "Chart without a local table")

    with tempfile.TemporaryDirectory() as scratch:
        source = pathlib.Path(scratch) / "chart_types.pptx"
        presentation.save(source)
        subprocess.run(
            [
                "soffice",
                "--headless",
                "--convert-to",
                "odp",
                "--outdir",
                scratch,
                str(source),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        converted = pathlib.Path(scratch) / "chart_types.odp"
        if not converted.is_file():
            raise SystemExit("LibreOffice wrote no chart_types.odp")
        target = pathlib.Path(scratch) / TARGET.name
        strip_local_table(converted, target)
        shutil.copy(target, TARGET)
    print(f"wrote {TARGET}")


def strip_local_table(source: pathlib.Path, target: pathlib.Path) -> None:
    """Delete the cached table from the fixture's bar-chart object.

    LibreOffice exports every chart with a `local-table`. The bar object is the
    one this fixture leaves without one, so its ranges are pointed at a `data`
    table that is not there: the chart still has the ranges it would draw, but
    the data behind them was not cached.
    """
    with zipfile.ZipFile(source) as archive:
        entries = [(info, archive.read(info.filename)) for info in archive.infolist()]

    table = re.compile(
        r'<table:table\b[^>]*table:name="local-table".*?</table:table>', re.DOTALL
    )
    changed = False
    rewritten = []
    for info, data in entries:
        if info.filename.startswith("Object ") and info.filename.endswith("/content.xml"):
            text = data.decode("utf-8")
            if 'chart:class="chart:bar"' in text and "local-table" in text:
                text = table.sub("", text).replace("local-table.", "data.")
                changed = True
                data = text.encode("utf-8")
        rewritten.append((info, data))
    if not changed:
        raise SystemExit("no bar-chart object with a local-table was found")

    target.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(target, "w") as archive:
        for info, data in rewritten:
            if info.filename == "mimetype":
                archive.writestr(info, data, compress_type=zipfile.ZIP_STORED)
            else:
                archive.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED)


if __name__ == "__main__":
    main()
