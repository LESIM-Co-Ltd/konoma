"""sheets.{xlsx,xls,ods}: 20 sheets with Japanese/symbol names, a hidden sheet, a used range that
starts at C5, an empty sheet. empty.*: a workbook with one empty sheet."""
from lo_lib import *

NAMES = ["売上", "Sheet 2", "A&B", "O'Brien", "売上 (2026)", "経費・予算", "😀絵文字", "R&D <x>",
         "データ#7", "Q1", "Q2", "Q3", "Q4", "第十三シート", "サマリー", "メモ", "付録A", "付録B", "最後から2番目", "最後"]


def main():
    doc = new_doc()
    rename_first(doc, NAMES[0])
    for n in NAMES[1:]:
        add_sheet(doc, n)
    sheets = doc.getSheets()
    for i, n in enumerate(NAMES):
        sh = sheets.getByName(n)
        put(sh, "A1", f"シート {i + 1}: {n}")
        put(sh, "B1", i + 1)
    # sheet 4 (O'Brien): hidden. sheet 9: hidden too
    sheets.getByName("O'Brien").IsVisible = False
    sheets.getByName("データ#7").IsVisible = False
    # used range starting at C5
    off = sheets.getByName("Q1")
    put(off, "C5", "C5 から始まる")
    put(off, "D6", 66)
    # an empty sheet
    sheets.getByName("Q2").clearContents(1023)
    sheets.getByName("Q2").getCellRangeByName("A1:B1").clearContents(1023)
    paths = save(doc, "sheets")
    doc.close(True)
    finish(paths)

    doc = new_doc()
    paths = save(doc, "empty")
    doc.close(True)
    finish(paths)


main()
