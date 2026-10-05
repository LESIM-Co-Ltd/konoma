"""Shared helpers for driving a headless LibreOffice through UNO.

Run with LibreOffice's bundled Python (see README.md). Everything here is generic: connect,
create a document, save in the three formats, and dump what LibreOffice itself displays for a
saved file (the ground truth the Rust tests compare konoma's grid against).
"""
import json
import os
import sys

import uno
from com.sun.star.beans import PropertyValue

PORT = int(os.environ.get("LO_PORT", "2917"))
OUT = os.environ.get("OUT_DIR", os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
OUT = os.path.abspath(OUT)
EXPECTED = os.path.join(OUT, "expected")

FILTERS = {
    "xlsx": "Calc MS Excel 2007 XML",
    "xls": "MS Excel 97",
    "ods": "calc8",
}


def pv(name, value):
    p = PropertyValue()
    p.Name = name
    p.Value = value
    return p


def connect():
    local = uno.getComponentContext()
    if os.environ.get("LO_INPROC"):
        # Running as a macro inside the soffice process (the bundled Python binary cannot be
        # launched on this machine): the component context is already the office's.
        smgr = local.ServiceManager
        return local, smgr, smgr.createInstanceWithContext("com.sun.star.frame.Desktop", local)
    resolver = local.ServiceManager.createInstanceWithContext(
        "com.sun.star.bridge.UnoUrlResolver", local
    )
    ctx = resolver.resolve(
        f"uno:socket,host=127.0.0.1,port={PORT};urp;StarOffice.ComponentContext"
    )
    smgr = ctx.ServiceManager
    desktop = smgr.createInstanceWithContext("com.sun.star.frame.Desktop", ctx)
    return ctx, smgr, desktop


CTX, SMGR, DESKTOP = connect()


def new_doc():
    return DESKTOP.loadComponentFromURL("private:factory/scalc", "_blank", 0, (pv("Hidden", True),))


def open_doc(path, password=None):
    props = [pv("Hidden", True)]
    if password:
        props.append(pv("Password", password))
    return DESKTOP.loadComponentFromURL(uno.systemPathToFileUrl(path), "_blank", 0, tuple(props))


def sheet(doc, name_or_idx):
    sheets = doc.getSheets()
    if isinstance(name_or_idx, int):
        return sheets.getByIndex(name_or_idx)
    return sheets.getByName(name_or_idx)


def add_sheet(doc, name):
    sheets = doc.getSheets()
    sheets.insertNewByName(name, sheets.getCount())
    return sheets.getByName(name)


def rename_first(doc, name):
    doc.getSheets().getByIndex(0).setName(name)
    return doc.getSheets().getByIndex(0)


def put(sh, addr, value):
    """Set a cell by A1 address. str starting with '=' is a formula; other str is text."""
    c = sh.getCellRangeByName(addr)
    if isinstance(value, str):
        if value.startswith("="):
            c.setFormula(value)
        else:
            c.setString(value)
    elif isinstance(value, bool):
        c.setValue(1.0 if value else 0.0)
    else:
        c.setValue(float(value))
    return c


def put_rows(sh, top_left_col, top_row, rows):
    """Write a rectangular list of rows (str/number) at (col,row) zero-based."""
    for r, row in enumerate(rows):
        for c, v in enumerate(row):
            if v is None:
                continue
            cell = sh.getCellByPosition(top_left_col + c, top_row + r)
            if isinstance(v, str):
                if v.startswith("="):
                    cell.setFormula(v)
                else:
                    cell.setString(v)
            else:
                cell.setValue(float(v))


def number_format(doc, code, locale=None):
    nf = doc.getNumberFormats()
    loc = uno.createUnoStruct("com.sun.star.lang.Locale")
    if locale:
        loc.Language, loc.Country = locale
    else:
        loc.Language, loc.Country = "en", "US"
    k = nf.queryKey(code, loc, False)
    if k == -1:
        k = nf.addNew(code, loc)
    return k


def save(doc, stem, exts=("xlsx", "xls", "ods"), password=None):
    """Store `doc` as <OUT>/<stem>.<ext> for each ext."""
    paths = []
    for ext in exts:
        path = os.path.join(OUT, f"{stem}.{ext}")
        if os.path.exists(path):
            os.remove(path)
        props = [pv("FilterName", FILTERS[ext])]
        if password:
            props.append(pv("Password", password))
        doc.storeToURL(uno.systemPathToFileUrl(path), tuple(props))
        paths.append(path)
        print("saved", path, os.path.getsize(path), "bytes")
    return paths


def dump_displayed(path, password=None, max_rows=400):
    """What LibreOffice itself shows for the saved file: {sheet: {hidden, grid}} (strings)."""
    doc = open_doc(path, password)
    out = {}
    sheets = doc.getSheets()
    for i in range(sheets.getCount()):
        sh = sheets.getByIndex(i)
        cur = sh.createCursor()
        cur.gotoEndOfUsedArea(False)
        addr = cur.getRangeAddress()
        er = min(addr.EndRow, max_rows - 1)
        ec = addr.EndColumn
        grid = []
        for r in range(er + 1):
            row = []
            for c in range(ec + 1):
                row.append(sh.getCellByPosition(c, r).getString())
            while row and row[-1] == "":
                row.pop()
            grid.append(row)
        while grid and not grid[-1]:
            grid.pop()
        out[sh.getName()] = {"visible": bool(sh.IsVisible), "grid": grid}
    doc.close(True)
    return out


def write_expected(path, password=None, max_rows=400):
    os.makedirs(EXPECTED, exist_ok=True)
    data = dump_displayed(path, password, max_rows)
    name = os.path.basename(path) + ".json"
    with open(os.path.join(EXPECTED, name), "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=0)


def finish(paths, **kw):
    for p in paths:
        write_expected(p, **kw)
