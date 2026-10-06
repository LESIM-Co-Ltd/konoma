"""encrypted.{xlsx,xls,ods} (password 'konoma'), and widesheet.xlsx. (The 120,000-row "big" sheet is generated inside the tests.)"""
from lo_lib import *

PW = "konoma"


def main():
    doc = new_doc()
    sh = rename_first(doc, "秘密")
    put(sh, "A1", "暗号化されたセル")
    put(sh, "A2", 42)
    paths = save(doc, "encrypted", password=PW)
    doc.close(True)
    # ground truth for the encrypted files is not dumped (konoma refuses them)

    doc = new_doc()
    sh = rename_first(doc, "広い")
    for c in range(0, 300):
        sh.getCellByPosition(c, 0).setValue(float(c))
    sh.getCellByPosition(300, 1).setString("桁 KN")
    paths = save(doc, "widesheet", exts=("xlsx",))
    doc.close(True)
    finish(paths)


main()
