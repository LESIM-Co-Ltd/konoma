"""layout.{xlsx,xls,ods}: hidden rows/columns, grouped (collapsed) rows, an auto-filter that hides
rows, merged cells, wrapped multi-line text, frozen panes and a protected sheet.

konoma does not read row/column visibility, outlines, filters or freeze panes: every row and column
is shown. The tests pin exactly that.
"""
import traceback

import uno

from lo_lib import *


def step(name, fn):
    try:
        fn()
        print("ok", name)
    except Exception:
        print("FEATURE-FAILED", name)
        traceback.print_exc()


def main():
    doc = new_doc()
    sh = rename_first(doc, "隠し")
    put(sh, "A1", "行・列")
    for c, h in enumerate(["A列", "B列(非表示)", "C列", "D列(非表示)", "E列"]):
        put(sh, f"{'ABCDE'[c]}2", h)
    for r in range(3, 13):
        for c in range(5):
            put(sh, f"{'ABCDE'[c]}{r}", f"r{r}{'abcde'[c]}")
    # hidden columns B and D, hidden rows 5 and 6
    cols = sh.getColumns()
    cols.getByIndex(1).IsVisible = False
    cols.getByIndex(3).IsVisible = False
    rows = sh.getRows()
    rows.getByIndex(4).IsVisible = False
    rows.getByIndex(5).IsVisible = False

    # grouped + collapsed rows
    g = add_sheet(doc, "グループ")
    for r in range(1, 11):
        put(g, f"A{r}", f"行{r}")
        put(g, f"B{r}", r * 10)
    g.group(uno.createUnoStruct("com.sun.star.table.CellRangeAddress", g.getRangeAddress().Sheet, 0, 2, 0, 6),
            uno.Enum("com.sun.star.table.TableOrientation", "ROWS"))
    g.group(uno.createUnoStruct("com.sun.star.table.CellRangeAddress", g.getRangeAddress().Sheet, 0, 2, 0, 6),
            uno.Enum("com.sun.star.table.TableOrientation", "ROWS"))
    try:
        g.getRows().getByIndex(2).IsVisible = True
        g.getRows().getByIndex(7).IsVisible = True
        g.hideDetail(uno.createUnoStruct("com.sun.star.table.CellRangeAddress", g.getRangeAddress().Sheet, 0, 2, 0, 6))
    except Exception:
        traceback.print_exc()

    # autofilter that hides rows
    f = add_sheet(doc, "フィルタ")
    put_rows(f, 0, 0, [["地域", "売上"]] + [[("東" if i % 2 == 0 else "西") + str(i), i * 10] for i in range(1, 11)])

    def autofilter():
        dbs = doc.DatabaseRanges
        dbs.addNewByName("範囲F", f.getCellRangeByName("A1:B11").getRangeAddress())
        db = dbs.getByName("範囲F")
        db.AutoFilter = True
        db.AutoFilter = True
        desc = db.getFilterDescriptor()
        fld = uno.createUnoStruct("com.sun.star.sheet.TableFilterField")
        fld.Field = 1
        fld.Operator = uno.Enum("com.sun.star.sheet.FilterOperator", "GREATER")
        fld.IsNumeric = True
        fld.NumericValue = 50
        desc.setFilterFields((fld,))
        db.refresh()
    step("autofilter", autofilter)

    # merges
    m = add_sheet(doc, "結合")
    put(m, "A1", "見出し(3行×4列の結合)")
    m.getCellRangeByName("A1:D3").merge(True)
    put(m, "A5", "横結合")
    m.getCellRangeByName("A5:C5").merge(True)
    put(m, "E5", "縦結合")
    m.getCellRangeByName("E5:E8").merge(True)
    put(m, "A9", "結合の隣")
    put(m, "B9", 42)
    # multi-line and long text
    w = add_sheet(doc, "改行")
    c = put(w, "A1", "1行目\n2行目\n3行目")
    c.IsTextWrapped = True
    put(w, "A2", "long " + "あいうえお" * 80)
    put(w, "A3", "tab\there and  nbsp")
    put(w, "B1", "隣のセル")
    put(w, "A4", "a   three spaces")
    put(w, "A5", "  leading two")
    put(w, "A6", "trailing  ")

    # frozen panes: needs a controller; hidden docs may not have one
    def freeze():
        ctl = doc.getCurrentController()
        ctl.setActiveSheet(sh)
        ctl.freezeAtPosition(1, 2)
    step("freeze panes", freeze)

    # protected sheet (not encrypted)
    p = add_sheet(doc, "保護")
    put(p, "A1", "保護されたシート")
    put(p, "A2", 123)
    p.protect("")

    paths = save(doc, "layout")
    doc.close(True)
    finish(paths)


main()
