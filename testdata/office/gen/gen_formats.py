"""formats.{xlsx,xls,ods} and date1904.{xlsx,xls,ods}: number formats of every family."""
import datetime

import uno

from lo_lib import *

D = datetime.date


def serial(d, base=D(1899, 12, 30)):
    return (d - base).days


CASES = [
    # (label, kind, value, format code)
    ("和暦(令和)", "n", serial(D(2026, 10, 5)), '[$-411]ggge"年"m"月"d"日"'),
    ("和暦(平成1年)", "n", serial(D(1989, 1, 8)), '[$-411]ggge"年"m"月"d"日"'),
    ("和暦(昭和)", "n", serial(D(1980, 4, 29)), '[$-411]gggee"年"mm"月"dd"日"'),
    ("西暦 日本語曜日", "n", serial(D(2026, 10, 5)), 'yyyy"年"mm"月"dd"日" (aaa)'),
    ("英語曜日・月名", "n", serial(D(2026, 10, 5)), "dddd, mmmm d, yyyy"),
    ("ISO 日付", "n", serial(D(2026, 10, 5)), "yyyy-mm-dd"),
    ("24 時間超 [h]:mm:ss", "n", 1.5, "[h]:mm:ss"),
    ("100.25 日 [h]:mm", "n", 100.25, "[h]:mm"),
    ("経過分 [mm]:ss", "n", 0.0425, "[mm]:ss"),
    ("時刻 AM/PM", "n", 0.5625, "h:mm AM/PM"),
    ("時刻 小数秒", "n", 0.000694444, "mm:ss.00"),
    ("負数の赤", "n", -1234.5, "#,##0;[Red]-#,##0"),
    ("負数の括弧(赤)", "n", -1234.5, "#,##0.00_);[Red](#,##0.00)"),
    ("正数", "n", 1234.5, "#,##0;[Red]-#,##0"),
    ("ゼロ区画", "n", 0, '0.00;-0.00;"ゼロ"'),
    ("分数 # ?/?", "n", 0.75, "# ?/?"),
    ("分数 帯分数", "n", 3.14159, "# ??/??"),
    ("指数 0.00E+00", "n", 12345.678, "0.00E+00"),
    ("指数 ##0.0E+0", "n", 12345.678, "##0.0E+0"),
    ("指数 微小", "n", 0.000123, "0.00E+00"),
    ("通貨 円", "n", 1500, "[$¥-411]#,##0"),
    ("通貨 円 負", "n", -1500, "[$¥-411]#,##0;[Red]-[$¥-411]#,##0"),
    ("通貨 ユーロ", "n", 1234.5, "[$€-407] #,##0.00"),
    ("通貨 ドル", "n", 1234.5, "[$$-409]#,##0.00"),
    ("通貨 ウォン", "n", 1234567, "[$₩-412]#,##0"),
    ("通貨 ポンド", "n", 99.5, "[$£-809]#,##0.00"),
    ("パーセント", "n", 0.256, "0.0%"),
    ("千単位", "n", 1234567, '#,##0,"K"'),
    ("条件付き", "n", 12345, '[>=10000]0.0,"万";0'),
    ("色と条件", "n", -5, "[Blue][>0]0;[Red][<0]-0;0"),
    ("ゼロ詰め", "n", 123, "000000"),
    ("電話番号", "n", 9012345678, "000-0000-0000"),
    ("テキスト書式 @", "s", "00123", "@"),
    ("文字列に書式", "s", "あいう", '"【"@"】"'),
    ("会計 _ と *", "n", 1234.5, '_(* #,##0.00_);_(* (#,##0.00);_(* "-"??_);_(@_)'),
    ("丸め 0.5 → 1", "n", 0.5, "0"),
    ("丸め 2.5 → 3", "n", 2.5, "0"),
    ("丸め 0.285 小数2", "n", 0.285, "0.00"),
    ("大きい数 General", "n", 123456789012, "General"),
    ("小数 General", "n", 1 / 3, "General"),
    ("日付+時刻", "n", serial(D(2026, 10, 5)) + 0.75, "yyyy/mm/dd hh:mm:ss"),
    ("通し日付 0", "n", 0, "yyyy-mm-dd"),
    ("1900 閏年 60", "n", 60, "yyyy-mm-dd"),
]


def fill(sh, doc, cases, base=D(1899, 12, 30)):
    put(sh, "A1", "説明")
    put(sh, "B1", "表示")
    put(sh, "C1", "書式コード")
    for i, (label, kind, value, code) in enumerate(cases, start=2):
        put(sh, f"A{i}", label)
        c = sh.getCellRangeByName(f"B{i}")
        if kind == "s":
            # the text format must be set before the string for '@' to apply as text
            c.NumberFormat = number_format(doc, code)
            c.setString(value)
        else:
            c.setValue(float(value))
            c.NumberFormat = number_format(doc, code)
        put(sh, f"C{i}", code)


def main():
    doc = new_doc()
    sh = rename_first(doc, "書式")
    fill(sh, doc, CASES)
    paths = save(doc, "formats")
    doc.close(True)
    finish(paths)

    doc = new_doc()
    doc.NullDate = uno.createUnoStruct("com.sun.star.util.Date", 1, 1, 1904)
    sh = rename_first(doc, "1904")
    base = D(1904, 1, 1)
    cases = [
        ("1904 基準の日付", "n", serial(D(2026, 10, 5), base), "yyyy-mm-dd"),
        ("1904 基準 0", "n", 0, "yyyy-mm-dd"),
        ("1904 基準 1462", "n", 1462, "yyyy-mm-dd"),
        ("1904 基準 日付時刻", "n", serial(D(2026, 10, 5), base) + 0.5, "yyyy-mm-dd hh:mm"),
        ("1904 基準 経過", "n", 1.5, "[h]:mm"),
    ]
    fill(sh, doc, cases, base)
    paths = save(doc, "date1904")
    doc.close(True)
    finish(paths)


main()
