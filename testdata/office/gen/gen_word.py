"""word.docx / word-ja.docx (and word.odt / word-ja.odt, the same documents saved as OpenDocument): a Word document written by LibreOffice (the only real word processor on
the machine this was built on), for the docx -> Markdown reader tests.

Own content only. Both documents hold the same things (headings, emphasis, bullets and numbered
lists, a table with merged cells, a picture, a footnote and an endnote, hyperlinks, tracked
changes, a comment, header and footer, a formula, a text box, a table of contents, a line break and
a page break); the Japanese one uses Japanese text and Japanese list numbering.

Run through run_job.sh (see README.md); OUT_DIR picks where the files go (default: ..).
"""
import os
import traceback

import uno
from com.sun.star.text.ControlCharacter import LINE_BREAK, PARAGRAPH_BREAK

from lo_lib import pv, OUT, DESKTOP
from pngmaker import png


def step(name, fn):
    try:
        fn()
        print("ok", name)
    except Exception:
        print("FEATURE-FAILED", name)
        traceback.print_exc()


def numbering_type(name):
    return uno.getConstantByName("com.sun.star.style.NumberingType." + name)


class Writer:
    def __init__(self, doc):
        self.doc = doc
        self.text = doc.getText()
        self.cur = self.text.createTextCursor()

    def para(self, s, style="Standard"):
        self.cur.ParaStyleName = style
        self.cur.setPropertyValue("NumberingStyleName", "")
        self.text.insertString(self.cur, s, False)
        self.text.insertControlCharacter(self.cur, PARAGRAPH_BREAK, False)
        self.cur.ParaStyleName = "Standard"
        self.cur.setPropertyValue("NumberingStyleName", "")

    def run(self, s, **props):
        # Japanese text takes the "Asian" variants of weight and posture.
        for k in list(props):
            if k in ("CharWeight", "CharPosture"):
                props[k + "Asian"] = props[k]
        for k, v in props.items():
            self.cur.setPropertyValue(k, v)
        self.text.insertString(self.cur, s, False)
        for k in props:
            self.cur.setPropertyToDefault(k)

    def link(self, s, url):
        """Text `s` as a hyperlink to `url` (the attribute is set on a range, so the text that
        follows is not part of the link)."""
        start = self.cur.getStart()
        self.text.insertString(self.cur, s, False)
        rng = self.text.createTextCursorByRange(start)
        rng.gotoRange(self.cur.getEnd(), True)
        rng.HyperLinkURL = url

    def end_para(self):
        self.text.insertControlCharacter(self.cur, PARAGRAPH_BREAK, False)

    def list_item(self, s, style, level=0):
        self.cur.ParaStyleName = "List Paragraph" if self._has_para_style("List Paragraph") else "Standard"
        self.cur.NumberingStyleName = style
        self.cur.NumberingLevel = level
        self.text.insertString(self.cur, s, False)
        self.text.insertControlCharacter(self.cur, PARAGRAPH_BREAK, False)

    def end_list(self):
        self.cur.setPropertyValue("NumberingStyleName", "")
        self.cur.setPropertyValue("NumberingLevel", 0)
        self.cur.ParaStyleName = "Standard"

    def _has_para_style(self, name):
        return self.doc.getStyleFamilies().getByName("ParagraphStyles").hasByName(name)


def make_list_style(doc, name, types, texts=None):
    """A numbering (list) style whose level n has NumberingType types[n] and prefix/suffix."""
    styles = doc.getStyleFamilies().getByName("NumberingStyles")
    if styles.hasByName(name):
        return
    st = doc.createInstance("com.sun.star.style.NumberingStyle")
    styles.insertByName(name, st)
    rules = st.NumberingRules
    for lvl, (ntype, prefix, suffix) in enumerate(types):
        props = list(rules.getByIndex(lvl))
        out = []
        for p in props:
            if p.Name == "NumberingType":
                p = pv("NumberingType", numbering_type(ntype) if isinstance(ntype, str) else ntype)
            elif p.Name == "Prefix":
                p = pv("Prefix", prefix)
            elif p.Name == "Suffix":
                p = pv("Suffix", suffix)
            elif p.Name == "ParentNumbering":
                p = pv("ParentNumbering", 1)
            out.append(p)
        uno.invoke(rules, "replaceByIndex", (lvl, uno.Any("[]com.sun.star.beans.PropertyValue", tuple(out))))
    st.NumberingRules = rules


def build(ja):
    doc = DESKTOP.loadComponentFromURL("private:factory/swriter", "_blank", 0, (pv("Hidden", True),))
    w = Writer(doc)
    t = (lambda en, jp: jp if ja else en)

    # ---- list styles (before use) -------------------------------------------------------------
    def list_styles():
        make_list_style(doc, "K Decimal", [("ARABIC", "", "."), ("ARABIC", "", ".")])
        make_list_style(doc, "K Letters", [("CHARS_LOWER_LETTER", "", ")"), ("CHARS_LOWER_LETTER", "(", ")")])
        make_list_style(doc, "K Roman", [("ROMAN_UPPER", "", "."), ("ROMAN_LOWER", "", ".")])
        make_list_style(doc, "K Aiueo", [("AIU_FULLWIDTH_JA", "", "."), ("IROHA_FULLWIDTH_JA", "", ")")])
        make_list_style(doc, "K Circle", [("CIRCLE_NUMBER", "", ""), ("NUMBER_LOWER_ZH", "", "")])
        make_list_style(doc, "K Article", [("ARABIC", "第", "条"), ("ARABIC", "第", "項")])
    step("list styles", list_styles)

    # ---- title and headings -------------------------------------------------------------------
    w.para(t("Word reader sample", "Word 読み込みサンプル"), "Title")
    w.para(t("Introduction", "はじめに"), "Heading 1")
    w.run(t("Plain text, ", "ふつうの文、"))
    w.run(t("bold", "太字"), CharWeight=150.0)
    w.run(" ")
    w.run(t("italic", "斜体"), CharPosture=uno.Enum("com.sun.star.awt.FontSlant", "ITALIC"))
    w.run(" ")
    w.run(t("strike", "取り消し線"), CharStrikeout=1)
    w.run(" ")
    w.run(t("bold italic", "太字の斜体"), CharWeight=150.0, CharPosture=uno.Enum("com.sun.star.awt.FontSlant", "ITALIC"))
    w.run(t(" and underline", " と下線"), CharUnderline=1)
    w.run(t(". Literal marks: *star* _under_ [x] <tag> $5 | `tick` # 1. ~~tilde~~ & ", "。記号そのまま: *star* _under_ [x] <tag> $5 | `tick` # 1. ~~tilde~~ & "))
    w.run(t("done.", "。完了。"), CharWeight=150.0)
    w.run(t("Next", "次"))
    w.end_para()
    w.text.insertString(w.cur, t("A line", "一行目"), False)
    w.text.insertControlCharacter(w.cur, LINE_BREAK, False)
    w.text.insertString(w.cur, t("after a line break", "改行のあと"), False)
    w.end_para()

    w.para(t("Lists", "リスト"), "Heading 2")
    w.para(t("Bullets and numbers", "箇条書きと番号"), "Heading 3")

    def lists():
        bullet = "Bullet •" if doc.getStyleFamilies().getByName("NumberingStyles").hasByName("Bullet •") else "List 1"
        w.list_item(t("bullet one", "箇条書き一"), bullet, 0)
        w.list_item(t("bullet nested", "入れ子の箇条書き"), bullet, 1)
        w.list_item(t("bullet two", "箇条書き二"), bullet, 0)
        w.end_list()
        w.para(t("Numbered", "番号つき"))
        w.list_item(t("first", "最初"), "K Decimal", 0)
        w.list_item(t("second", "次"), "K Decimal", 0)
        w.list_item(t("second-a", "次の下"), "K Decimal", 1)
        w.list_item(t("third", "三番目"), "K Decimal", 0)
        w.end_list()
        w.para(t("Letters", "英字"))
        w.list_item("alpha", "K Letters", 0)
        w.list_item("beta", "K Letters", 0)
        w.list_item("beta one", "K Letters", 1)
        w.end_list()
        w.para(t("Roman", "ローマ数字"))
        w.list_item("one", "K Roman", 0)
        w.list_item("two", "K Roman", 0)
        w.list_item("two a", "K Roman", 1)
        w.end_list()
        if ja:
            w.para("日本語の番号")
            w.list_item("あ", "K Aiueo", 0)
            w.list_item("い", "K Aiueo", 0)
            w.list_item("いろは", "K Aiueo", 1)
            w.list_item("う", "K Aiueo", 0)
            w.end_list()
            w.list_item("丸数字一", "K Circle", 0)
            w.list_item("丸数字二", "K Circle", 0)
            w.end_list()
            w.list_item("条文一", "K Article", 0)
            w.list_item("条文二", "K Article", 0)
            w.end_list()
    step("lists", lists)

    # ---- table with merged cells --------------------------------------------------------------
    def table():
        tbl = doc.createInstance("com.sun.star.text.TextTable")
        tbl.initialize(4, 3)
        w.text.insertTextContent(w.cur, tbl, False)
        tbl.getCellByName("A1").setString(t("Name", "名前"))
        tbl.getCellByName("B1").setString(t("Merged across two columns", "二列にまたがる見出し"))
        tbl.getCellByName("A2").setString(t("row 2", "二行目"))
        tbl.getCellByName("B2").setString("1")
        tbl.getCellByName("C2").setString("a|b *x*")
        tbl.getCellByName("A3").setString(t("tall cell", "縦に結合"))
        tbl.getCellByName("B3").setString("2")
        tbl.getCellByName("C3").setString(t("two\nlines", "二行\nの文字"))
        tbl.getCellByName("B4").setString("3")
        tbl.getCellByName("C4").setString("")
        # merge B1:C1 and A3:A4
        c = tbl.createCursorByCellName("B1")
        c.goRight(1, True)
        c.mergeRange()
        c = tbl.createCursorByCellName("A3")
        c.goDown(1, True)
        c.mergeRange()
        w.cur.gotoEnd(False)
    step("table", table)

    w.para(t("Pictures and notes", "図と脚注"), "Heading 2")

    # ---- picture ------------------------------------------------------------------------------
    def picture():
        path = os.path.join(os.environ.get("WORK_DIR", "."), "_word_pic.png")
        open(path, "wb").write(png(80, 40))
        g = doc.createInstance("com.sun.star.text.TextGraphicObject")
        g.GraphicURL = uno.systemPathToFileUrl(path)
        g.AnchorType = uno.Enum("com.sun.star.text.TextContentAnchorType", "AS_CHARACTER")
        g.Width = 4000
        g.Height = 2000
        g.Description = t("A small gradient picture", "小さなグラデーションの図")
        g.Title = t("Gradient", "グラデーション")
        w.text.insertTextContent(w.cur, g, False)
        w.end_para()
    step("picture", picture)

    # ---- footnote and endnote -----------------------------------------------------------------
    def notes():
        w.run(t("A sentence with a footnote", "脚注つきの文"))
        fn = doc.createInstance("com.sun.star.text.Footnote")
        w.text.insertTextContent(w.cur, fn, False)
        fn.getText().setString(t("Footnote text with *star*.", "脚注の本文(*star* を含む)。"))
        w.run(t(" and an endnote", "と文末脚注"))
        en = doc.createInstance("com.sun.star.text.Endnote")
        w.text.insertTextContent(w.cur, en, False)
        en.getText().setString(t("Endnote text.", "文末脚注の本文。"))
        w.end_para()
    step("notes", notes)

    # ---- hyperlinks ---------------------------------------------------------------------------
    def links():
        w.run(t("External link: ", "外部リンク: "))
        w.link(t("example site", "例のサイト"), "https://example.com/a?x=1&y=2")
        w.run(t(" and a link to a heading: ", " 見出しへのリンク: "))
        w.link(t("go to Introduction", "はじめにへ"), "#" + t("Introduction", "はじめに") + "|outline")
        w.run(t(" end", " 終わり"))
        w.end_para()
    step("links", links)

    # ---- tracked changes ----------------------------------------------------------------------
    def changes():
        w.run(t("Kept text. ", "残る文。"))
        start = w.cur.getStart()
        w.run(t("This sentence is deleted. ", "この文は削除されます。"))
        end = w.cur.getEnd()
        doc.RecordChanges = True
        rng = w.text.createTextCursorByRange(start)
        rng.gotoRange(end, True)
        rng.setString("")
        w.run(t("This sentence is inserted.", "この文は挿入されます。"))
        doc.RecordChanges = False
        w.end_para()
    step("tracked changes", changes)

    # ---- comment ------------------------------------------------------------------------------
    def comment():
        w.run(t("Text with a comment", "コメントつきの文"))
        ann = doc.createInstance("com.sun.star.text.textfield.Annotation")
        ann.setPropertyValue("Content", t("SECRET-COMMENT body", "秘密のコメント本文"))
        ann.setPropertyValue("Author", "Tester")
        w.text.insertTextContent(w.cur, ann, False)
        w.end_para()
    step("comment", comment)

    # ---- formula ------------------------------------------------------------------------------
    def formula():
        w.run(t("Inline formula: ", "数式: "))
        obj = doc.createInstance("com.sun.star.text.TextEmbeddedObject")
        obj.CLSID = "078B7ABA-54FC-457F-8551-6147e776a997"
        obj.AnchorType = uno.Enum("com.sun.star.text.TextContentAnchorType", "AS_CHARACTER")
        w.text.insertTextContent(w.cur, obj, False)
        obj.getEmbeddedObject().Formula = "E = m c^2"
        w.run(t(" and a fraction ", " 分数 "))
        obj2 = doc.createInstance("com.sun.star.text.TextEmbeddedObject")
        obj2.CLSID = "078B7ABA-54FC-457F-8551-6147e776a997"
        obj2.AnchorType = uno.Enum("com.sun.star.text.TextContentAnchorType", "AS_CHARACTER")
        w.text.insertTextContent(w.cur, obj2, False)
        obj2.getEmbeddedObject().Formula = "{a + b} over {c} = sqrt {x_1 ^2 + y_1 ^2}"
        w.end_para()
    step("formula", formula)

    # ---- text box -----------------------------------------------------------------------------
    def textbox():
        w.run(t("Paragraph that holds a text box.", "テキストボックスを持つ段落。"))
        fr = doc.createInstance("com.sun.star.text.TextFrame")
        fr.setSize(uno.createUnoStruct("com.sun.star.awt.Size", 6000, 1500))
        fr.AnchorType = uno.Enum("com.sun.star.text.TextContentAnchorType", "AT_PARAGRAPH")
        w.text.insertTextContent(w.cur, fr, False)
        fr.getText().setString(t("TEXTBOX-TEXT inside a frame", "枠の中の文字(TEXTBOX-TEXT)"))
        w.end_para()
    step("text box", textbox)

    # ---- code paragraph, page break -----------------------------------------------------------
    w.para("def f(x):\n    return x * 2", "Preformatted Text")

    def page_break():
        w.cur.setPropertyValue("BreakType", uno.Enum("com.sun.star.style.BreakType", "PAGE_BEFORE"))
        w.para(t("Paragraph after a page break", "改ページのあとの段落"))
        w.cur.setPropertyToDefault("BreakType")
    step("page break", page_break)

    # ---- header and footer --------------------------------------------------------------------
    def header_footer():
        ps = doc.getStyleFamilies().getByName("PageStyles").getByName("Standard")
        ps.HeaderIsOn = True
        ps.HeaderText.setString(t("RUNNING-HEADER", "ヘッダー文字(RUNNING-HEADER)"))
        ps.FooterIsOn = True
        ps.FooterText.setString(t("RUNNING-FOOTER", "フッター文字(RUNNING-FOOTER)"))
    step("header and footer", header_footer)

    # ---- table of contents (inserted at the top, after the title) ------------------------------
    def toc():
        idx = doc.createInstance("com.sun.star.text.ContentIndex")
        idx.CreateFromOutline = True
        idx.Title = t("Contents", "目次")
        c = w.text.createTextCursor()
        c.gotoStart(False)
        c.gotoNextParagraph(False)  # after the title
        w.text.insertTextContent(c, idx, False)
        idx.update()
    step("table of contents", toc)

    name = "word-ja" if ja else "word"
    path = os.path.join(OUT, name + ".docx")
    if os.path.exists(path):
        os.remove(path)
    doc.storeToURL(uno.systemPathToFileUrl(path), (pv("FilterName", "MS Word 2007 XML"),))
    print("saved", path, os.path.getsize(path), "bytes")
    # The same document as OpenDocument text, for the odt reader (and the docx / odt comparison).
    opath = os.path.join(OUT, name + ".odt")
    if os.path.exists(opath):
        os.remove(opath)
    doc.storeToURL(uno.systemPathToFileUrl(opath), (pv("FilterName", "writer8"),))
    print("saved", opath, os.path.getsize(opath), "bytes")
    doc.close(True)


def main():
    build(False)
    build(True)


main()
