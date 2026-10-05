"""formulas.{xlsx,xls,ods}: cross-sheet references, named ranges, array formulas, error values,
volatile functions, text/boolean/date results. konoma shows the value the file saved."""
import traceback

import uno

from lo_lib import *


def main():
    doc = new_doc()
    inp = rename_first(doc, "入力")
    put_rows(inp, 0, 0, [["品目", "数量", "単価"],
                         ["りんご", 3, 120], ["みかん", 5, 80], ["ぶどう", 2, 450], ["Pen & Ink", 10, 35]])
    calc = add_sheet(doc, "計算")
    # named range over the quantities, and one over a single cell
    cell0 = inp.getCellRangeByName("A1").getCellAddress()
    doc.NamedRanges.addNewByName("数量範囲", "$入力.$B$2:$B$5", cell0, 0)
    doc.NamedRanges.addNewByName("税率", "$入力.$E$1", cell0, 0)
    put(inp, "D1", "税率")
    put(inp, "E1", 0.1)
    rows = [
        ("A1", "名前付き範囲の合計"), ("B1", "=SUM(数量範囲)"),
        ("A2", "シート間参照"), ("B2", "='入力'.B2*'入力'.C2"),
        ("A3", "合計金額"), ("B3", "=SUMPRODUCT('入力'.B2:B5;'入力'.C2:C5)"),
        ("A4", "税込"), ("B4", "=ROUND(B3*(1+税率);0)"),
        ("A5", "文字列結合"), ("B5", '="合計: "&TEXT(B3;"#,##0")&" 円"'),
        ("A6", "ブール"), ("B6", "=B3>1000"),
        ("A7", "検索"), ("B7", "=VLOOKUP(\"みかん\";'入力'.A2:C5;3;0)"),
        ("A8", "IF 日本語"), ("B8", '=IF(B3>1500;"高い";"安い")'),
        ("A9", "日付"), ("B9", "=DATE(2026;10;5)"),
        ("A10", "0.1+0.2"), ("B10", "=0.1+0.2"),
        ("A11", "巨大"), ("B11", "=10^300"),
        ("A12", "微小"), ("B12", "=10^-300"),
        ("A13", "文字列を返す空"), ("B13", '=""'),
        ("A14", "REPT"), ("B14", '=REPT("あ";5)'),
    ]
    for a, v in rows:
        put(calc, a, v)
    # errors
    errs = add_sheet(doc, "エラー")
    for i, (label, f) in enumerate([("#DIV/0!", "=1/0"), ("#N/A", "=NA()"), ("#NAME?", "=FOOBAR(1)"),
                                    ("#VALUE!", '="a"+1'), ("#REF!", "=#REF!"), ("#NUM!", "=SQRT(-1)"),
                                    ("#NULL!", "=SUM(A1 B1)"), ("伝播", "=1+C1")], start=1):
        put(errs, f"A{i}", label)
        put(errs, f"B{i}", f)
    # array formulas
    arr = add_sheet(doc, "配列")
    put_rows(arr, 0, 0, [[1, 10], [2, 20], [3, 30]])
    arr.getCellRangeByName("D1").setArrayFormula("=SUM(A1:A3*B1:B3)")
    arr.getCellRangeByName("D3:D5").setArrayFormula("=A1:A3*B1:B3")
    put(arr, "C1", "SUMPRODUCT 相当")
    put(arr, "C3", "要素ごとの積")
    # 2x2 matrix product
    put_rows(arr, 5, 0, [[1, 2], [3, 4]])
    arr.getCellRangeByName("H1:I2").setArrayFormula("=MMULT(F1:G2;F1:G2)")
    # volatile
    vol = add_sheet(doc, "揮発")
    put(vol, "A1", "=NOW()")
    put(vol, "A2", "=TODAY()")
    put(vol, "A3", "=RAND()")
    put(vol, "A4", "=NOW()-NOW()")
    doc.calculateAll()
    paths = save(doc, "formulas")
    doc.close(True)
    finish(paths)


main()
