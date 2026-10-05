"""pivot.{xlsx,xls,ods}: source data + several DataPilot (pivot) tables."""
import json
import os

import uno
from com.sun.star.table import CellAddress, CellRangeAddress

from lo_lib import *

REGIONS = ["北海道", "関東", "関西", "九州"]
PRODUCTS = ["りんご", "みかん", "Pen & Ink <A>", "ぶどう"]
MONTHS = ["1月", "2月", "3月", "4月", "5月", "6月"]
HEADER = ["地域", "商品", "月", "チャネル", "売上"]


def source_rows():
    rows = []
    for ri, r in enumerate(REGIONS):
        for pi, p in enumerate(PRODUCTS):
            for mi, m in enumerate(MONTHS):
                sales = 100 + (ri * 7 + pi * 13 + mi * 5) * 10 + (ri * pi + mi) % 7
                ch = "店舗" if (ri + pi + mi) % 2 == 0 else "Web"
                rows.append([r, p, m, ch, sales])
    return rows


def expected(rows):
    cell, sub, prod_total, month_total = {}, {}, {}, {}
    grand = 0
    for r, p, m, ch, s in rows:
        cell[f"{r}|{p}|{m}"] = cell.get(f"{r}|{p}|{m}", 0) + s
        sub[r] = sub.get(r, 0) + s
        month_total[m] = month_total.get(m, 0) + s
        grand += s
    # per (region, product) row totals across months; average by product x channel
    rp = {}
    for r, p, m, ch, s in rows:
        rp[f"{r}|{p}"] = rp.get(f"{r}|{p}", 0) + s
    pc = {}
    for r, p, m, ch, s in rows:
        pc.setdefault(f"{p}|{ch}", []).append(s)
    avg = {k: sum(v) / len(v) for k, v in pc.items()}
    by_ch, by_prod = {}, {}
    for r, p, m, ch, s in rows:
        by_ch.setdefault(ch, []).append(s)
        by_prod.setdefault(p, []).append(s)
    count = {}
    for r, p, m, ch, s in rows:
        count[f"{r}|{ch}"] = count.get(f"{r}|{ch}", 0) + 1
    return {
        "avg_by_channel": {k: sum(v) / len(v) for k, v in by_ch.items()},
        "avg_by_product": {k: sum(v) / len(v) for k, v in by_prod.items()},
        "avg_all": sum(s for *_, s in rows) / len(rows),
        "count_by_region_channel": count,
        "rows": len(rows),
        "regions": REGIONS, "products": PRODUCTS, "months": MONTHS,
        "cell": cell, "region_subtotal": sub, "region_product_total": rp,
        "month_total": month_total, "grand": grand, "avg_by_product_channel": avg,
        "kanto_only_grand": sum(s for r, p, m, ch, s in rows if r == "関東"),
    }


def make_pivot(doc, src_sheet, dest_sheet, name, dest_col, dest_row, rows_f, cols_f, data_f, func, pages=(), current=None, subtotal_outer=True):
    from com.sun.star.sheet.DataPilotFieldOrientation import ROW, COLUMN, DATA, PAGE
    from com.sun.star.sheet.GeneralFunction import SUM, AVERAGE, COUNT
    funcs = {"SUM": SUM, "AVERAGE": AVERAGE, "COUNT": COUNT}
    n = len(source_rows_cache)
    src = CellRangeAddress()
    src.Sheet = src_sheet.getRangeAddress().Sheet
    src.StartColumn, src.StartRow, src.EndColumn, src.EndRow = 0, 0, len(HEADER) - 1, n
    dps = dest_sheet.getDataPilotTables()
    desc = dps.createDataPilotDescriptor()
    desc.setSourceRange(src)
    fields = desc.getDataPilotFields()
    for i, h in enumerate(HEADER):
        f = fields.getByIndex(i)
        if h in rows_f:
            f.Orientation = ROW
        elif h in cols_f:
            f.Orientation = COLUMN
        elif h in pages:
            f.Orientation = PAGE
        elif h == data_f:
            f.Orientation = DATA
            f.Function = funcs[func]
    if subtotal_outer and len(rows_f) > 1:
        outer = fields.getByIndex(HEADER.index(rows_f[0]))
        outer.Subtotals = (SUM,)
    desc.ColumnGrand = True
    desc.RowGrand = True
    dest = CellAddress()
    dest.Sheet = dest_sheet.getRangeAddress().Sheet
    dest.Column, dest.Row = dest_col, dest_row
    dps.insertNewByName(name, dest, desc)
    t = dps.getByName(name)
    if current:
        pf = t.getDataPilotFields().getByIndex(HEADER.index(pages[0]))
        pf.setPropertyValue("SelectedPage", current)
        items = pf.getItems()
        for i in range(items.getCount()):
            it = items.getByIndex(i)
            it.IsHidden = it.getName() != current
        t.refresh()
    a = t.getOutputRange()
    return [a.StartColumn, a.StartRow, a.EndColumn, a.EndRow]


source_rows_cache = source_rows()


def main():
    doc = new_doc()
    data = rename_first(doc, "元データ")
    put_rows(data, 0, 0, [HEADER] + source_rows_cache)
    pv_sheet = add_sheet(doc, "ピボット")
    put(pv_sheet, "A1", "地域×商品×月 売上 (SUM)")
    ranges = {}
    ranges["main"] = make_pivot(doc, data, pv_sheet, "売上集計", 0, 2, ["地域", "商品"], ["月"], "売上", "SUM")
    # Second pivot on the SAME sheet, to the right: average by product x channel.
    ranges["same_sheet_avg"] = make_pivot(doc, data, pv_sheet, "平均", 12, 2, ["商品"], ["チャネル"], "売上", "AVERAGE")
    # Separate sheet with a page (filter) field set to one region.
    pg = add_sheet(doc, "ページ付き")
    ranges["page"] = make_pivot(doc, data, pg, "関東のみ", 0, 3, ["商品"], ["月"], "売上", "SUM", pages=("地域",), current="関東")
    # A third sheet: a pivot with only a row field and a count.
    cn = add_sheet(doc, "件数")
    ranges["count"] = make_pivot(doc, data, cn, "件数", 0, 1, ["地域"], ["チャネル"], "売上", "COUNT")
    exp = expected(source_rows_cache)
    exp["ranges"] = ranges
    with open(os.path.join(EXPECTED, "pivot.expected.json"), "w", encoding="utf-8") as f:
        json.dump(exp, f, ensure_ascii=False, indent=1)
    paths = save(doc, "pivot")
    doc.close(True)
    finish(paths)


if __name__ == "__main__":
    os.makedirs(EXPECTED, exist_ok=True)
    main()
