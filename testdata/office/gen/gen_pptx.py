"""slides.pptx / slides-ja.pptx (and slides.odp): a presentation written by LibreOffice Impress, for
the pptx -> Markdown reader tests (`tests_pptx_real.rs`).

Own content only. Slides: a title slide; a title with bullets (three levels); two columns; a table;
a picture with a caption and a group; a hidden slide with speaker notes; a link (a chart could not be inserted through the API: charts are covered by hand-written packages in the tests). The
Japanese one uses Japanese text.

Run through run_job.sh (see README.md); OUT_DIR picks where the files go (default: ..).
"""
import os
import traceback

import uno

from lo_lib import pv, OUT, DESKTOP
from pngmaker import png


def step(name, fn):
    try:
        fn()
        print("ok", name)
    except Exception:
        print("FEATURE-FAILED", name)
        traceback.print_exc()


def rect(x, y, w, h):
    from com.sun.star.awt import Rectangle
    r = Rectangle()
    r.X, r.Y, r.Width, r.Height = x, y, w, h
    return r


def size(w, h):
    from com.sun.star.awt import Size
    s = Size()
    s.Width, s.Height = w, h
    return s


def point(x, y):
    from com.sun.star.awt import Point
    p = Point()
    p.X, p.Y = x, y
    return p


def shapes_of(page):
    return [page.getByIndex(i) for i in range(page.getCount())]


def kind(sh):
    return sh.ShapeType.split(".")[-1]


def set_levels(shape, texts_levels):
    """Fills an outline shape: one paragraph per (text, level)."""
    shape.setString("\n".join(t for t, _ in texts_levels))
    en = shape.getText().createEnumeration()
    i = 0
    while en.hasMoreElements():
        para = en.nextElement()
        para.NumberingLevel = texts_levels[i][1]
        i += 1


def build(ja):
    doc = DESKTOP.loadComponentFromURL("private:factory/simpress", "_blank", 0, (pv("Hidden", True),))
    t = (lambda en, jp: jp if ja else en)
    pages = doc.getDrawPages()
    work = os.environ.get("WORK_DIR", ".")

    def new_page(layout):
        pages.insertNewByIndex(pages.getCount())
        page = pages.getByIndex(pages.getCount() - 1)
        page.Layout = layout
        return page

    def title_and_body(page, title, body_levels):
        for sh in shapes_of(page):
            k = kind(sh)
            if k == "TitleTextShape":
                sh.setString(title)
            elif k == "OutlinerShape":
                set_levels(sh, body_levels)

    # ---- 1. title slide ------------------------------------------------------------------------
    def s1():
        page = pages.getByIndex(0)
        page.Layout = 0
        for sh in shapes_of(page):
            k = kind(sh)
            if k == "TitleTextShape":
                sh.setString(t("Quarterly review", "四半期レビュー"))
            elif k == "SubtitleShape":
                sh.setString(t("Own content only", "自作の内容のみ"))
    step("title slide", s1)

    # ---- 2. bullets with levels ------------------------------------------------------------------
    def s2():
        page = new_page(1)
        title_and_body(page, t("Agenda *and* more", "議題と *その他*"), [
            (t("Results", "結果"), 0),
            (t("Revenue grew", "売上が伸びた"), 1),
            (t("By region", "地域別"), 2),
            (t("Plans", "計画"), 0),
            (t("# not a heading", "# 見出しではない"), 0),
        ])
    step("bullets", s2)

    # ---- 3. two columns --------------------------------------------------------------------------
    def s3():
        page = new_page(3)
        outs = [sh for sh in shapes_of(page) if kind(sh) == "OutlinerShape"]
        for sh in shapes_of(page):
            if kind(sh) == "TitleTextShape":
                sh.setString(t("Compare", "比較"))
        outs.sort(key=lambda s: s.Position.X)
        set_levels(outs[0], [(t("Left one", "左一"), 0), (t("Left two", "左二"), 0)])
        set_levels(outs[1], [(t("Right one", "右一"), 0), (t("Right two", "右二"), 0)])
    step("two columns", s3)

    # ---- 4. a table ------------------------------------------------------------------------------
    def s4():
        page = new_page(19)
        for sh in shapes_of(page):
            if kind(sh) == "TitleTextShape":
                sh.setString(t("Numbers", "数値"))
        tbl = doc.createInstance("com.sun.star.drawing.TableShape")
        page.add(tbl)
        tbl.setPosition(point(2000, 5000))
        tbl.setSize(size(20000, 4000))
        model = tbl.getPropertyValue("Model")
        cols = model.getColumns()
        rows = model.getRows()
        while cols.getCount() < 3:
            cols.insertByIndex(cols.getCount(), 1)
        while rows.getCount() < 3:
            rows.insertByIndex(rows.getCount(), 1)
        data = [[t("Name", "名前"), t("Qty", "数量"), t("Note", "備考")],
                ["A|B", "1", "*x*"],
                [t("tail", "末尾"), "2", ""]]
        for r in range(3):
            for c in range(3):
                model.getCellByPosition(c, r).setString(data[r][c])
    step("table", s4)

    # ---- 5. picture + caption + group ---------------------------------------------------------------
    def s5():
        page = new_page(19)
        for sh in shapes_of(page):
            if kind(sh) == "TitleTextShape":
                sh.setString(t("Figure", "図"))
        path = os.path.join(work, "_slide_pic.png")
        open(path, "wb").write(png(80, 40))
        g = doc.createInstance("com.sun.star.drawing.GraphicObjectShape")
        page.add(g)
        g.GraphicURL = uno.systemPathToFileUrl(path)
        g.setPosition(point(2000, 4500))
        g.setSize(size(8000, 4000))
        g.Description = t("A gradient picture", "グラデーションの画像")
        cap = doc.createInstance("com.sun.star.drawing.TextShape")
        page.add(cap)
        cap.setPosition(point(2000, 9000))
        cap.setSize(size(8000, 1500))
        cap.setString(t("Figure 1. The caption", "図1 キャプション"))
        # a group of two text shapes, to the right
        a = doc.createInstance("com.sun.star.drawing.TextShape")
        b = doc.createInstance("com.sun.star.drawing.TextShape")
        page.add(a)
        page.add(b)
        a.setPosition(point(12000, 4500)); a.setSize(size(8000, 1500)); a.setString(t("Grouped top", "グループ上"))
        b.setPosition(point(12000, 7000)); b.setSize(size(8000, 1500)); b.setString(t("Grouped bottom", "グループ下"))
        coll = SMGR_INST("com.sun.star.drawing.ShapeCollection")
        coll.add(a)
        coll.add(b)
        page.group(coll)
    step("picture and group", s5)

    # ---- 6. hidden slide with notes -------------------------------------------------------------------
    def s6():
        page = new_page(1)
        title_and_body(page, t("Backup", "予備"), [(t("Hidden detail", "非表示の詳細"), 0)])
        page.setPropertyValue("Visible", False)
        notes = page.getNotesPage()
        for sh in shapes_of(notes):
            if kind(sh) == "NotesShape":
                sh.setString(t("Say this aloud.\n# Remember the numbers", "ここで話す。\n# 数字を覚えておく"))
    step("hidden slide with notes", s6)

    # ---- 7. a link ------------------------------------------------------------------------------------
    def s8():
        page = new_page(19)
        for sh in shapes_of(page):
            if kind(sh) == "TitleTextShape":
                sh.setString(t("Links", "リンク"))
        tb = doc.createInstance("com.sun.star.drawing.TextShape")
        page.add(tb)
        tb.setPosition(point(2000, 5000))
        tb.setSize(size(16000, 2000))
        text = tb.getText()
        cur = text.createTextCursor()
        text.insertString(cur, t("See ", "参照: "), False)
        fld = doc.createInstance("com.sun.star.text.TextField.URL")
        fld.URL = "https://example.com/slides"
        fld.Representation = t("the site", "サイト")
        text.insertTextContent(cur, fld, False)
        text.insertString(cur, t(" for more.", " を見てください。"), False)
    step("link", s8)

    name = "slides-ja" if ja else "slides"
    path = os.path.join(OUT, name + ".pptx")
    if os.path.exists(path):
        os.remove(path)
    doc.storeToURL(uno.systemPathToFileUrl(path), (pv("FilterName", "Impress MS PowerPoint 2007 XML"),))
    print("saved", path, os.path.getsize(path), "bytes")
    opath = os.path.join(OUT, name + ".odp")
    if os.path.exists(opath):
        os.remove(opath)
    doc.storeToURL(uno.systemPathToFileUrl(opath), (pv("FilterName", "impress8"),))
    print("saved", opath, os.path.getsize(opath), "bytes")
    doc.close(True)


def SMGR_INST(name):
    from lo_lib import CTX
    return CTX.ServiceManager.createInstanceWithContext(name, CTX)


def main():
    build(False)
    build(True)


main()
