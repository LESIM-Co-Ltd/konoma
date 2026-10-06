"""objects.{xlsx,xls,ods}: chart, picture, shape, comment, conditional format, validation, hyperlink.

The cells around the objects must read normally; the objects themselves are not read by konoma.
"""
import os
import traceback

import uno

from lo_lib import *
from pngmaker import png


def step(name, fn):
    try:
        fn()
        print("ok", name)
    except Exception:
        print("FEATURE-FAILED", name)
        traceback.print_exc()


def rect(x, y, w, h):
    r = uno.createUnoStruct("com.sun.star.awt.Rectangle")
    r.X, r.Y, r.Width, r.Height = x, y, w, h
    return r


def main():
    doc = new_doc()
    sh = rename_first(doc, "売上")
    rows = [["月", "売上", "前年", "備考"],
            ["1月", 120, 100, "好調 \U0001F600"],
            ["2月", 150, 130, "Tom & Jerry <b>"],
            ["3月", 90, 110, "低調"],
            ["4月", 200, 160, "セール"],
            ["5月", 175, 150, ""]]
    put_rows(sh, 0, 0, rows)
    put(sh, "A30", "グラフの下の行")
    put(sh, "B30", "=SUM(B2:B6)")

    def chart():
        addr = sh.getCellRangeByName("A1:C6").getRangeAddress()
        sh.getCharts().addNewByName("グラフ1", rect(8000, 1000, 9000, 6000), (addr,), True, True)
    step("chart", chart)

    def picture():
        path = os.path.join(os.environ.get("WORK_DIR", "."), "_pic.png")
        open(path, "wb").write(png())
        g = doc.createInstance("com.sun.star.drawing.GraphicObjectShape")
        sh.getDrawPage().add(g)
        g.GraphicURL = uno.systemPathToFileUrl(path)
        g.Position = uno.createUnoStruct("com.sun.star.awt.Point", 8000, 8000)
        g.Size = uno.createUnoStruct("com.sun.star.awt.Size", 3000, 2000)
    step("picture", picture)

    def shape():
        r = doc.createInstance("com.sun.star.drawing.RectangleShape")
        sh.getDrawPage().add(r)
        r.Position = uno.createUnoStruct("com.sun.star.awt.Point", 12000, 8000)
        r.Size = uno.createUnoStruct("com.sun.star.awt.Size", 4000, 2000)
        r.setString("図形のテキスト")
    step("shape", shape)

    def comment():
        a = sh.getCellRangeByName("B2").getCellAddress()
        sh.getAnnotations().insertNew(a, "コメント: 先月比 +20%\n二行目")
        a = sh.getCellRangeByName("D4").getCellAddress()
        sh.getAnnotations().insertNew(a, "備考のコメント")
    step("comment", comment)

    def condfmt():
        rng = sh.getCellRangeByName("B2:B6")
        cf = rng.ConditionalFormat
        op = uno.Enum("com.sun.star.sheet.ConditionOperator", "GREATER")
        cf.addNew((pv("Operator", op), pv("Formula1", "150"), pv("StyleName", "Good")))
        rng.ConditionalFormat = cf
        # color scale / data bar are not expressible through the legacy API; the above is enough
    step("conditional format", condfmt)

    def validation():
        rng = sh.getCellRangeByName("E2:E6")
        v = rng.Validation
        v.Type = uno.Enum("com.sun.star.sheet.ValidationType", "LIST")
        v.setFormula1('"りんご";"みかん";"ぶどう"')
        rng.Validation = v
        put(sh, "E1", "選択")
        put(sh, "E2", "みかん")
    step("validation list", validation)

    def links():
        put(sh, "G1", '=HYPERLINK("https://example.com/";"例のリンク")')
        cell = sh.getCellRangeByName("G2")
        f = doc.createInstance("com.sun.star.text.TextField.URL")
        f.URL = "https://example.org/"
        f.Representation = "URL フィールド"
        cell.insertTextContent(cell.createTextCursor(), f, False)
    step("hyperlinks", links)

    # A second sheet holding only an image-less drawing: a text box (not a cell)
    s2 = add_sheet(doc, "図形のみ")
    put(s2, "A1", "セルは A1 だけ")

    def box():
        r = doc.createInstance("com.sun.star.drawing.TextShape")
        s2.getDrawPage().add(r)
        r.Position = uno.createUnoStruct("com.sun.star.awt.Point", 3000, 3000)
        r.Size = uno.createUnoStruct("com.sun.star.awt.Size", 5000, 2000)
        r.setString("テキストボックス")
    step("text box", box)

    paths = save(doc, "objects")
    doc.close(True)
    finish(paths)


main()
