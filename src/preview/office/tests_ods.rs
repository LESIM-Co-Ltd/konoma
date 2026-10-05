//! Tests for the ods format pass (`fmt_ods`): the style -> Excel format code translation, the
//! cell/column/row style resolution, repeat expansion, error cells, encryption and the memory
//! budget. Real LibreOffice output is in `tests.rs` (`real_ods_*`); everything here is built from
//! hand-written XML so each rule is pinned on its own.

use super::container::Limits;
use super::fmt_ods::{self, Styles};
use super::numfmt::{self, Value};
use super::tests::{
    assert_inner_does_not_panic, deflated, load, load_with, mutate, read_parts, small_limits, tmp,
    write, xorshift,
};
use super::*;
use crate::test_support::sample_path_or_skip;

const NS: &str = concat!(
    r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" "#,
    r#"xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" "#,
    r#"xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" "#,
    r#"xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" "#,
    r#"xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" "#,
    r#"xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" "#,
    r#"xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0" "#,
    r#"xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0""#
);

// ---------------------------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------------------------

fn content_xml(auto: &str, tables: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {NS} office:version="1.3"><office:automatic-styles>{auto}</office:automatic-styles><office:body><office:spreadsheet>{tables}</office:spreadsheet></office:body></office:document-content>"#
    )
}

fn ods_bytes(content: &str, styles: Option<&str>, manifest: Option<&str>) -> Vec<u8> {
    let mut entries: Vec<(&str, Vec<u8>)> = vec![
        (
            "mimetype",
            b"application/vnd.oasis.opendocument.spreadsheet".to_vec(),
        ),
        ("content.xml", content.as_bytes().to_vec()),
    ];
    if let Some(s) = styles {
        entries.push(("styles.xml", s.as_bytes().to_vec()));
    }
    // calamine insists on a manifest (it looks for encryption there).
    let m = manifest.unwrap_or(MANIFEST_PLAIN);
    entries.push(("META-INF/manifest.xml", m.as_bytes().to_vec()));
    let refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, c)| (*n, c.as_slice())).collect();
    deflated(&refs)
}

fn table(name: &str, rows: &str) -> String {
    format!(r#"<table:table table:name="{name}">{rows}</table:table>"#)
}

fn hidden_table(name: &str, rows: &str) -> String {
    format!(r#"<table:table table:name="{name}" table:style-name="taHidden">{rows}</table:table>"#)
}

const HIDDEN_STYLE: &str = r#"<style:style style:name="taHidden" style:family="table"><style:table-properties table:display="false"/></style:style>"#;

fn row(cells: &str) -> String {
    format!("<table:table-row>{cells}</table:table-row>")
}

fn float(style: &str, v: &str) -> String {
    let st = if style.is_empty() {
        String::new()
    } else {
        format!(r#" table:style-name="{style}""#)
    };
    format!(
        r#"<table:table-cell{st} office:value-type="float" office:value="{v}"><text:p>{v}</text:p></table:table-cell>"#
    )
}

fn string(text: &str) -> String {
    format!(
        r#"<table:table-cell office:value-type="string"><text:p>{text}</text:p></table:table-cell>"#
    )
}

fn date(style: &str, v: &str) -> String {
    let st = if style.is_empty() {
        String::new()
    } else {
        format!(r#" table:style-name="{style}""#)
    };
    format!(
        r#"<table:table-cell{st} office:value-type="date" office:date-value="{v}"><text:p>{v}</text:p></table:table-cell>"#
    )
}

/// A data style `N` with the given body, and a cell style `c` pointing at it.
fn data_style(kind: &str, body: &str) -> String {
    format!(
        r#"<number:{kind} style:name="N">{body}</number:{kind}><style:style style:name="c" style:family="table-cell" style:data-style-name="N"/>"#
    )
}

/// The format code a cell style `c` resolves to, from automatic styles written as `auto`.
fn code_of(auto: &str, cell_style: &str) -> Option<String> {
    let xml = content_xml(auto, "");
    let mut styles = Styles::default();
    fmt_ods::parse_styles(xml.as_bytes(), &mut styles, true).unwrap();
    styles.code_of_cell_style(cell_style)
}

fn code(kind: &str, body: &str) -> String {
    code_of(&data_style(kind, body), "c").unwrap()
}

const NUM_2: &str = r#"<number:number number:decimal-places="2" number:min-integer-digits="1" number:grouping="true"/>"#;
const NUM_0: &str = r#"<number:number number:decimal-places="0" number:min-integer-digits="1" number:grouping="true"/>"#;

fn render(code: &str, v: f64) -> String {
    numfmt::format_value(code, Value::Number(v), &numfmt::Options::default())
}

fn sheet0(wb: &Workbook) -> &Sheet {
    &wb.sheets[0]
}

fn load_ods_xml(name: &str, auto: &str, tables: &str) -> Workbook {
    let dir = tmp(name);
    let p = write(
        &dir,
        "t.ods",
        &ods_bytes(&content_xml(auto, tables), None, None),
    );
    load(&p).unwrap()
}

// ---------------------------------------------------------------------------------------------
// translation: number styles
// ---------------------------------------------------------------------------------------------

#[test]
fn number_style_grouping_decimals_and_min_integer_digits() {
    let n = |attrs: &str| code("number-style", &format!("<number:number {attrs}/>"));
    assert_eq!(
        n(r#"number:decimal-places="2" number:min-integer-digits="1" number:grouping="true""#),
        "#,##0.00"
    );
    assert_eq!(
        n(r#"number:decimal-places="0" number:min-integer-digits="1""#),
        "0"
    );
    assert_eq!(
        n(r#"number:decimal-places="1" number:min-integer-digits="3""#),
        "000.0"
    );
    assert_eq!(
        n(r#"number:decimal-places="0" number:min-integer-digits="5" number:grouping="true""#),
        "00,000"
    );
    assert_eq!(
        n(r#"number:decimal-places="0" number:min-integer-digits="1" number:grouping="true""#),
        "#,##0"
    );
    // Optional decimals (ODF 1.3): 1 required, 3 optional.
    assert_eq!(
        n(
            r#"number:decimal-places="4" number:min-decimal-places="1" number:min-integer-digits="1""#
        ),
        "0.0###"
    );
    // A style without decimal-places is "as many as needed".
    assert_eq!(n(r#"number:min-integer-digits="1""#), "General");
    assert_eq!(
        n(r#"number:min-integer-digits="1" number:grouping="true""#),
        "#,##0.##########"
    );
    // Absurd widths are clamped, never allocated.
    let c = n(r#"number:decimal-places="99999999" number:min-integer-digits="99999999""#);
    assert!(c.len() < 100, "{c}");
}

#[test]
fn number_style_display_factor_is_trailing_commas() {
    let n = |f: &str| {
        code(
            "number-style",
            &format!(
                r#"<number:number number:decimal-places="0" number:min-integer-digits="1" number:display-factor="{f}"/>"#
            ),
        )
    };
    assert_eq!(n("1000"), "0,");
    assert_eq!(n("1000000"), "0,,");
    assert_eq!(n("500"), "0", "not a power of 1000: ignored");
    assert_eq!(n("1"), "0");
    assert_eq!(render(&n("1000"), 1_234_000.0), "1234");
}

#[test]
fn scientific_and_fraction_styles() {
    assert_eq!(
        code(
            "number-style",
            r#"<number:scientific-number number:decimal-places="2" number:min-integer-digits="1" number:min-exponent-digits="2"/>"#
        ),
        "0.00E+00"
    );
    assert_eq!(
        render("0.00E+00", 12345.0),
        "1.23E+04",
        "the engine reads what we write"
    );
    let f = |attrs: &str| code("number-style", &format!("<number:fraction {attrs}/>"));
    assert_eq!(
        f(
            r#"number:min-integer-digits="1" number:min-numerator-digits="1" number:min-denominator-digits="1""#
        ),
        "0 ?/?"
    );
    assert_eq!(
        f(
            r#"number:min-integer-digits="1" number:min-numerator-digits="1" number:min-denominator-digits="1" number:denominator-value="8""#
        ),
        "0 ?/8"
    );
    // LibreOffice writes `min-integer-digits="0"` for `# ?/?`: the integer part is there, optional.
    assert_eq!(
        f(
            r#"number:min-integer-digits="0" number:min-numerator-digits="2" number:min-denominator-digits="2""#
        ),
        "# ??/??"
    );
    // No attribute: an improper fraction.
    assert_eq!(
        f(r#"number:min-numerator-digits="2" number:min-denominator-digits="2""#),
        "??/??"
    );
    assert_eq!(render("# ?/?", 1.5), "1 1/2");
    assert_eq!(render("0 ?/?", 0.75), "0 3/4");
}

#[test]
fn percentage_style_keeps_the_percent_operator() {
    let c = code(
        "percentage-style",
        r#"<number:number number:decimal-places="1" number:min-integer-digits="1"/><number:text>%</number:text>"#,
    );
    assert_eq!(c, "0.0%");
    assert_eq!(render(&c, 0.256), "25.6%");
    // Text around the percent sign: only `%` is the operator.
    let c = code(
        "percentage-style",
        r#"<number:number number:decimal-places="0" number:min-integer-digits="1"/><number:text> of total%</number:text>"#,
    );
    assert_eq!(c, "0 \"of\" \"total\"%");
    assert_eq!(render(&c, 0.5), "50 of total%");
}

#[test]
fn percent_sign_in_a_plain_number_style_is_a_literal() {
    let c = code(
        "number-style",
        &format!("{NUM_0}<number:text>%</number:text>"),
    );
    assert_eq!(c, "#,##0\"%\"");
    assert_eq!(render(&c, 5.0), "5%", "no scaling by 100");
}

#[test]
fn currency_style_symbol_before_and_after() {
    let c = code(
        "currency-style",
        &format!(
            r#"<number:currency-symbol number:language="ja" number:country="JP">¥</number:currency-symbol>{NUM_0}"#
        ),
    );
    assert_eq!(c, "\"¥\"#,##0");
    assert_eq!(render(&c, 1500.0), "¥1,500");
    let c = code(
        "currency-style",
        &format!(
            r#"{NUM_2}<number:text> </number:text><number:currency-symbol>€</number:currency-symbol>"#
        ),
    );
    assert_eq!(c, "#,##0.00 \"€\"");
    assert_eq!(render(&c, 1234.5), "1,234.50 €");
    // An empty symbol element adds nothing.
    let c = code(
        "currency-style",
        &format!(r#"<number:currency-symbol/>{NUM_0}"#),
    );
    assert_eq!(c, "#,##0");
}

#[test]
fn literal_text_is_quoted_and_quotes_are_escaped() {
    // Safe separators stay bare, everything else is quoted, `"` is backslash-escaped.
    let c = code(
        "number-style",
        &format!("{NUM_0}<number:text> kg (net)</number:text>"),
    );
    assert_eq!(c, "#,##0 \"kg\" (\"net\")");
    assert_eq!(render(&c, 12.0), "12 kg (net)");
    let c = code(
        "number-style",
        &format!("<number:text>a&quot;b&amp;c</number:text>{NUM_0}"),
    );
    assert_eq!(c, "\"a\"\\\"\"b&c\"#,##0");
    assert_eq!(render(&c, 7.0), "a\"b&c7");
    // CJK and entities in a character reference.
    let c = code(
        "number-style",
        &format!("{NUM_0}<number:text>&#x5186;</number:text>"),
    );
    assert_eq!(c, "#,##0\"円\"");
    // A line break cannot live in a one-line code.
    let c = code(
        "number-style",
        &format!("{NUM_0}<number:text>a\nb</number:text>"),
    );
    assert_eq!(c, "#,##0\"ab\"");
}

#[test]
fn text_and_boolean_styles() {
    let c = code(
        "text-style",
        "<number:text>Name: </number:text><number:text-content/>",
    );
    assert_eq!(c, "\"Name\": @");
    assert_eq!(
        numfmt::format_value(&c, Value::Text("Ada"), &numfmt::Options::default()),
        "Name: Ada"
    );
    assert_eq!(code("boolean-style", "<number:boolean/>"), "General");
    // A style with no parts at all is General.
    assert_eq!(code("number-style", ""), "General");
}

// ---------------------------------------------------------------------------------------------
// translation: date and time styles
// ---------------------------------------------------------------------------------------------

#[test]
fn date_style_elements() {
    let c = code(
        "date-style",
        r#"<number:year number:style="long"/><number:text>-</number:text><number:month number:style="long"/><number:text>-</number:text><number:day number:style="long"/>"#,
    );
    assert_eq!(c, "yyyy-mm-dd");
    let c = code(
        "date-style",
        r#"<number:year/><number:text>/</number:text><number:month/><number:text>/</number:text><number:day/>"#,
    );
    assert_eq!(c, "yy/m/d");
    let c = code(
        "date-style",
        r#"<number:day-of-week number:style="long"/><number:text>, </number:text><number:month number:textual="true" number:style="long"/><number:text> </number:text><number:day/>"#,
    );
    assert_eq!(c, "dddd, mmmm d");
    assert_eq!(render(&c, 46297.0), "Friday, October 2");
    let c = code(
        "date-style",
        r#"<number:day-of-week/><number:text> </number:text><number:month number:textual="true"/>"#,
    );
    assert_eq!(c, "ddd mmm");
    assert_eq!(render(&c, 46297.0), "Fri Oct");
    // Quarter and week-of-year have no Excel code: dropped.
    let c = code(
        "date-style",
        r#"<number:year number:style="long"/><number:quarter/><number:week-of-year/>"#,
    );
    assert_eq!(c, "yyyy");
}

#[test]
fn japanese_era_date_style() {
    let c = code(
        "date-style",
        r#"<number:era number:calendar="gengou" number:style="long"/><number:year number:calendar="gengou"/><number:text>年</number:text><number:month/><number:text>月</number:text><number:day/><number:text>日</number:text>"#,
    );
    assert_eq!(c, "ggge\"年\"m\"月\"d\"日\"");
    assert_eq!(render(&c, 46297.0), "令和8年10月2日");
    // Short era name and two-digit era year.
    let c = code(
        "date-style",
        r#"<number:era number:calendar="gengou"/><number:year number:calendar="gengou" number:style="long"/>"#,
    );
    assert_eq!(c, "ggee");
}

#[test]
fn time_style_elements() {
    let c = code(
        "time-style",
        r#"<number:hours number:style="long"/><number:text>:</number:text><number:minutes number:style="long"/><number:text>:</number:text><number:seconds number:style="long"/>"#,
    );
    assert_eq!(c, "hh:mm:ss");
    assert_eq!(render(&c, 0.5 + 5.0 / 1440.0 + 9.0 / 86400.0), "12:05:09");
    // Fractional seconds.
    let c = code(
        "time-style",
        r#"<number:minutes number:style="long"/><number:text>:</number:text><number:seconds number:style="long" number:decimal-places="2"/>"#,
    );
    assert_eq!(c, "mm:ss.00");
    // 12-hour clock.
    let c = code(
        "time-style",
        r#"<number:hours/><number:text>:</number:text><number:minutes number:style="long"/><number:text> </number:text><number:am-pm/>"#,
    );
    assert_eq!(c, "h:mm AM/PM");
    assert_eq!(render(&c, 0.75), "6:00 PM");
    // Elapsed time (no truncation on overflow).
    let c = code(
        "time-style",
        r#"<number:hours number:style="long" number:truncate-on-overflow="false"/><number:text>:</number:text><number:minutes number:style="long"/><number:text>:</number:text><number:seconds number:style="long"/>"#,
    );
    assert_eq!(c, "[hh]:mm:ss");
    assert_eq!(render(&c, 25.5 / 24.0), "25:30:00");
}

// ---------------------------------------------------------------------------------------------
// translation: conditional sections and colours
// ---------------------------------------------------------------------------------------------

fn currency_with_map(cond: &str) -> String {
    format!(
        r##"<number:currency-style style:name="NP" style:volatile="true"><number:currency-symbol>¥</number:currency-symbol>{NUM_0}</number:currency-style>
           <number:currency-style style:name="N"><style:text-properties fo:color="#ff0000"/><number:text>-</number:text><number:currency-symbol>¥</number:currency-symbol>{NUM_0}<style:map style:condition="{cond}" style:apply-style-name="NP"/></number:currency-style>
           <style:style style:name="c" style:family="table-cell" style:data-style-name="N"/>"##
    )
}

#[test]
fn negative_numbers_in_red_are_the_two_section_form() {
    // LibreOffice's pattern: the main style is the negative form, the map sends >= 0 to the plain one.
    let c = code_of(&currency_with_map("value()&gt;=0"), "c").unwrap();
    assert_eq!(c, "\"¥\"#,##0;[Red]-\"¥\"#,##0");
    assert_eq!(render(&c, 1500.0), "¥1,500");
    assert_eq!(render(&c, -1500.0), "-¥1,500", "one minus sign, not two");
    assert_eq!(render(&c, 0.0), "¥0");
}

#[test]
fn map_conditions_cover_the_common_shapes() {
    // < 0 -> applied style: main stays the first section.
    let auto = format!(
        r#"<number:number-style style:name="NN"><number:text>-</number:text>{NUM_0}</number:number-style>
           <number:number-style style:name="N">{NUM_0}<style:map style:condition="value()&lt;0" style:apply-style-name="NN"/></number:number-style>
           <style:style style:name="c" style:family="table-cell" style:data-style-name="N"/>"#
    );
    assert_eq!(code_of(&auto, "c").unwrap(), "#,##0;-#,##0");
    // > 0 and < 0 with the main style for zero: three sections.
    let auto = format!(
        r#"<number:number-style style:name="NP">{NUM_0}</number:number-style>
           <number:number-style style:name="NN"><number:text>-</number:text>{NUM_0}</number:number-style>
           <number:number-style style:name="N"><number:text>zero</number:text><style:map style:condition="value()&gt;0" style:apply-style-name="NP"/><style:map style:condition="value()&lt;0" style:apply-style-name="NN"/></number:number-style>
           <style:style style:name="c" style:family="table-cell" style:data-style-name="N"/>"#
    );
    let c = code_of(&auto, "c").unwrap();
    assert_eq!(c, "#,##0;-#,##0;\"zero\"");
    assert_eq!(render(&c, 0.0), "zero");
    assert_eq!(render(&c, 3.0), "3");
    // >= 0 and < 0.
    let auto = auto
        .replace("value()&gt;0", "value()&gt;=0")
        .replace("<number:text>zero</number:text>", "");
    assert_eq!(code_of(&auto, "c").unwrap(), "#,##0;-#,##0");
    // Any other numeric condition keeps an explicit bracket.
    let c = code_of(&currency_with_map("value()&gt;100"), "c").unwrap();
    assert_eq!(c, "[>100]\"¥\"#,##0;[Red]-\"¥\"#,##0");
    let c = code_of(&currency_with_map("value() != 5"), "c").unwrap();
    assert_eq!(c, "[<>5]\"¥\"#,##0;[Red]-\"¥\"#,##0");
    // A condition that is not a numeric comparison is ignored (main section only).
    let c = code_of(&currency_with_map("is-true-boolean()"), "c").unwrap();
    assert_eq!(c, "[Red]-\"¥\"#,##0");
    // A map to a style that does not exist is dropped.
    let c = code_of(
        &currency_with_map("value()&gt;=0")
            .replace("apply-style-name=\"NP\"", "apply-style-name=\"Missing\""),
        "c",
    );
    assert_eq!(c.unwrap(), "[Red]-\"¥\"#,##0");
}

#[test]
fn map_cycles_terminate() {
    let auto = format!(
        r#"<number:number-style style:name="N">{NUM_0}<style:map style:condition="value()&gt;=0" style:apply-style-name="N"/></number:number-style>
           <style:style style:name="c" style:family="table-cell" style:data-style-name="N"/>"#
    );
    let c = code_of(&auto, "c").unwrap();
    assert!(c.contains(';') && c.len() < 200, "{c}");
}

#[test]
fn colours_map_to_excel_names_only_for_the_exact_hues() {
    for (hex, name) in [
        ("#ff0000", "[Red]"),
        ("#0000FF", "[Blue]"),
        ("#00ff00", "[Green]"),
    ] {
        let c = code(
            "number-style",
            &format!(r##"<style:text-properties fo:color="{hex}"/>{NUM_0}"##),
        );
        assert_eq!(c, format!("{name}#,##0"));
    }
    let c = code(
        "number-style",
        &format!(r##"<style:text-properties fo:color="#123456"/>{NUM_0}"##),
    );
    assert_eq!(c, "#,##0");
}

#[test]
fn cell_style_inheritance_and_cycles() {
    let auto = format!(
        r#"<number:number-style style:name="N">{NUM_2}</number:number-style>
           <style:style style:name="base" style:family="table-cell" style:data-style-name="N"/>
           <style:style style:name="mid" style:family="table-cell" style:parent-style-name="base"/>
           <style:style style:name="leaf" style:family="table-cell" style:parent-style-name="mid"/>
           <style:style style:name="own" style:family="table-cell" style:parent-style-name="base" style:data-style-name="Other"/>
           <style:style style:name="loopA" style:family="table-cell" style:parent-style-name="loopB"/>
           <style:style style:name="loopB" style:family="table-cell" style:parent-style-name="loopA"/>
           <style:style style:name="col" style:family="table-column" style:data-style-name="N"/>"#
    );
    assert_eq!(code_of(&auto, "leaf").unwrap(), "#,##0.00");
    assert_eq!(code_of(&auto, "mid").unwrap(), "#,##0.00");
    // The style's own data-style-name wins over its parent's, even if that name is unknown.
    assert_eq!(code_of(&auto, "own"), None);
    assert_eq!(code_of(&auto, "loopA"), None, "a parent cycle ends");
    assert_eq!(code_of(&auto, "col"), None, "only table-cell styles count");
    assert_eq!(code_of(&auto, "nope"), None);
}

#[test]
fn styles_xml_and_content_xml_styles_combine() {
    // The data style lives in styles.xml, the cell style in content.xml.
    let styles = format!(
        r#"<?xml version="1.0"?><office:document-styles {NS}><office:styles><number:number-style style:name="NShared">{NUM_2}</number:number-style></office:styles></office:document-styles>"#
    );
    let auto = r#"<style:style style:name="ce1" style:family="table-cell" style:data-style-name="NShared"/>"#;
    let content = content_xml(auto, &table("S", &row(&float("ce1", "1234.5"))));
    let dir = tmp("ods_styles_part");
    let p = write(&dir, "t.ods", &ods_bytes(&content, Some(&styles), None));
    let wb = load(&p).unwrap();
    assert_eq!(sheet0(&wb).display(0, 0), "1,234.50");
}

// ---------------------------------------------------------------------------------------------
// the loader: cells, repeats, defaults, fallbacks
// ---------------------------------------------------------------------------------------------

#[test]
fn a_negative_amount_through_the_whole_pipeline() {
    let auto = currency_with_map("value()&gt;=0");
    let rows = row(&format!(
        "{}{}{}",
        float("c", "1500"),
        float("c", "-1500"),
        float("c", "0")
    ));
    let wb = load_ods_xml("ods_neg", &auto, &table("S", &rows));
    let s = sheet0(&wb);
    assert_eq!(s.display(0, 0), "¥1,500");
    assert_eq!(s.display(0, 1), "-¥1,500");
    assert_eq!(s.display(0, 2), "¥0");
}

#[test]
fn repeated_cells_and_rows_are_expanded_with_their_style() {
    let auto = data_style(
        "percentage-style",
        r#"<number:number number:decimal-places="0" number:min-integer-digits="1"/><number:text>%</number:text>"#,
    );
    let cell = r#"<table:table-cell table:number-columns-repeated="3" table:style-name="c" office:value-type="percentage" office:value="0.5"><text:p>50%</text:p></table:table-cell>"#;
    let rows = format!(
        r#"<table:table-row table:number-rows-repeated="2">{cell}</table:table-row>{}"#,
        row(&string("end"))
    );
    let wb = load_ods_xml("ods_repeat", &auto, &table("S", &rows));
    let s = sheet0(&wb);
    for r in 0..2 {
        for c in 0..3 {
            assert_eq!(s.display(r, c), "50%", "({r},{c})");
        }
        assert!(s.cell(r, 3).is_none());
    }
    assert_eq!(s.display(2, 0), "end");
    assert_eq!((s.nrows, s.ncols), (3, 3));
}

#[test]
fn leading_empty_repeats_shift_the_style_lookup_correctly() {
    // Empty cells repeated before a styled one: the style must land on the right column.
    let auto = data_style("number-style", NUM_2);
    let rows = row(&format!(
        r#"<table:table-cell table:number-columns-repeated="4"/>{}{}"#,
        float("c", "5"),
        float("", "5")
    ));
    let wb = load_ods_xml("ods_lead", &auto, &table("S", &rows));
    let s = sheet0(&wb);
    assert_eq!(s.display(0, 4), "5.00");
    assert_eq!(s.display(0, 5), "5");
    // And rows: empty rows repeated before.
    let rows = format!(
        r#"<table:table-row table:number-rows-repeated="3"><table:table-cell/></table:table-row>{}"#,
        row(&float("c", "5"))
    );
    let wb = load_ods_xml("ods_lead2", &auto, &table("S", &rows));
    assert_eq!(sheet0(&wb).display(3, 0), "5.00");
}

#[test]
fn column_default_styles_apply_to_unstyled_cells_only() {
    let auto = format!(
        "{}{}",
        r#"<number:date-style style:name="NW"><number:era number:calendar="gengou" number:style="long"/><number:year number:calendar="gengou"/><number:text>年</number:text><number:month/><number:text>月</number:text><number:day/><number:text>日</number:text></number:date-style>
           <style:style style:name="ceW" style:family="table-cell" style:data-style-name="NW"/>"#,
        r#"<number:date-style style:name="NI"><number:year number:style="long"/><number:text>-</number:text><number:month number:style="long"/><number:text>-</number:text><number:day number:style="long"/></number:date-style>
           <style:style style:name="ceI" style:family="table-cell" style:data-style-name="NI"/>"#
    );
    let cols = r#"<table:table-column table:number-columns-repeated="2" table:default-cell-style-name="ceW"/><table:table-column/>"#;
    let rows = row(&format!(
        "{}{}{}",
        date("", "2026-10-02"),
        date("ceI", "2026-10-02"),
        date("", "2026-10-02")
    ));
    let wb = load_ods_xml("ods_coldef", &auto, &table("S", &format!("{cols}{rows}")));
    let s = sheet0(&wb);
    assert_eq!(s.display(0, 0), "令和8年10月2日", "column default");
    assert_eq!(
        s.display(0, 1),
        "2026-10-02",
        "the cell's own style beats the column"
    );
    // Third column: no default, no style -> LibreOffice-like ISO fallback.
    assert_eq!(s.display(0, 2), "2026-10-02");
}

#[test]
fn row_default_style_beats_the_column_default() {
    let auto = format!(
        "{}{}",
        data_style("number-style", NUM_2),
        r#"<number:number-style style:name="N0"><number:number number:decimal-places="0" number:min-integer-digits="1"/></number:number-style>
           <style:style style:name="c0" style:family="table-cell" style:data-style-name="N0"/>"#
    );
    let cols = r#"<table:table-column table:default-cell-style-name="c0"/>"#;
    let rows = format!(
        r#"<table:table-row table:default-cell-style-name="c">{}</table:table-row>{}"#,
        float("", "5"),
        row(&float("", "5"))
    );
    let wb = load_ods_xml("ods_rowdef", &auto, &table("S", &format!("{cols}{rows}")));
    let s = sheet0(&wb);
    assert_eq!(s.display(0, 0), "5.00");
    assert_eq!(s.display(1, 0), "5");
}

#[test]
fn typed_cells_without_any_style_get_a_readable_default() {
    let rows = row(&format!(
        r#"{}<table:table-cell office:value-type="date" office:date-value="2026-10-02T13:05:09"><text:p/></table:table-cell><table:table-cell office:value-type="time" office:time-value="PT25H30M00S"><text:p/></table:table-cell><table:table-cell office:value-type="percentage" office:value="0.256"><text:p/></table:table-cell>{}"#,
        date("", "2026-10-02"),
        float("", "0.1")
    ));
    let wb = load_ods_xml("ods_default", "", &table("S", &rows));
    let s = sheet0(&wb);
    assert_eq!(s.display(0, 0), "2026-10-02");
    assert_eq!(s.display(0, 1), "2026-10-02 13:05:09");
    assert_eq!(s.display(0, 2), "25:30:00");
    assert_eq!(s.display(0, 3), "25.60%");
    assert_eq!(s.display(0, 4), "0.1");
}

#[test]
fn a_styled_cell_does_not_get_the_default_even_when_the_style_is_general() {
    // A style that resolves to General is still "styled": no ISO fallback.
    let auto = data_style(
        "number-style",
        r#"<number:number number:min-integer-digits="1"/>"#,
    );
    let wb = load_ods_xml(
        "ods_styled_general",
        &auto,
        &table("S", &row(&float("c", "0.256"))),
    );
    assert_eq!(sheet0(&wb).display(0, 0), "0.256");
}

#[test]
fn durations_and_dates_use_the_iso_conversion() {
    let auto = data_style(
        "time-style",
        r#"<number:hours number:style="long"/><number:text>:</number:text><number:minutes number:style="long"/>"#,
    );
    let rows = row(
        r#"<table:table-cell table:style-name="c" office:value-type="time" office:time-value="PT13H05M09S"><text:p/></table:table-cell>"#,
    );
    let wb = load_ods_xml("ods_time", &auto, &table("S", &rows));
    assert_eq!(sheet0(&wb).display(0, 0), "13:05");
}

#[test]
fn booleans_and_strings_keep_their_text() {
    let rows = row(&format!(
        r#"{}<table:table-cell office:value-type="boolean" office:boolean-value="true"><text:p>TRUE</text:p></table:table-cell><table:table-cell office:value-type="boolean" office:boolean-value="false"><text:p>FALSE</text:p></table:table-cell>"#,
        string("a &amp; b")
    ));
    let wb = load_ods_xml("ods_bool", "", &table("S", &rows));
    let s = sheet0(&wb);
    assert_eq!(s.display(0, 0), "a & b");
    assert_eq!(s.display(0, 1), "TRUE");
    assert_eq!(s.display(0, 2), "FALSE");
}

// ---------------------------------------------------------------------------------------------
// error cells
// ---------------------------------------------------------------------------------------------

fn error_cell(text: &str, extra: &str) -> String {
    format!(
        r#"<table:table-cell{extra} table:formula="of:=1/0" office:value-type="string" office:string-value="" calcext:value-type="error"><text:p>{text}</text:p></table:table-cell>"#
    )
}

#[test]
fn libreoffice_error_cells_become_error_values() {
    let rows = row(&format!(
        "{}{}{}{}{}{}",
        error_cell("#DIV/0!", ""),
        error_cell("#N/A", ""),
        error_cell("Err:502", ""),
        error_cell("Err:532", ""),
        error_cell("Err:525", ""),
        error_cell("#REF!", r#" table:number-columns-repeated="2""#),
    ));
    let wb = load_ods_xml("ods_err", "", &table("S", &rows));
    let s = sheet0(&wb);
    let got: Vec<&str> = (0..7).map(|c| s.display(0, c)).collect();
    assert_eq!(
        got,
        ["#DIV/0!", "#N/A", "#VALUE!", "#DIV/0!", "#NAME?", "#REF!", "#REF!"]
    );
    for c in 0..7 {
        assert_eq!(
            s.cell(0, c).unwrap().cell_type(),
            CellType::Error,
            "col {c}"
        );
    }
}

#[test]
fn an_error_cell_keeps_its_row_and_column() {
    // Errors recorded per (row, col) must not leak onto neighbours.
    let rows = format!(
        "{}{}",
        row(&format!("{}{}", string("x"), error_cell("#NUM!", ""))),
        row(&format!("{}{}", error_cell("#NULL!", ""), string("y")))
    );
    let wb = load_ods_xml("ods_err_pos", "", &table("S", &rows));
    let s = sheet0(&wb);
    assert_eq!(s.display(0, 0), "x");
    assert_eq!(s.display(0, 1), "#NUM!");
    assert_eq!(s.display(1, 0), "#NULL!");
    assert_eq!(s.display(1, 1), "y");
    assert_eq!(s.cell(0, 0).unwrap().cell_type(), CellType::Text);
}

// ---------------------------------------------------------------------------------------------
// encryption
// ---------------------------------------------------------------------------------------------

const MANIFEST_PLAIN: &str = r#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#;

const MANIFEST_ENCRYPTED: &str = r#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml" manifest:size="4096"><manifest:encryption-data manifest:checksum-type="SHA1/1K" manifest:checksum="AAAA"><manifest:algorithm manifest:algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" manifest:initialisation-vector="AAAA"/><manifest:key-derivation manifest:key-derivation-name="PBKDF2" manifest:salt="AAAA" manifest:iteration-count="100000"/></manifest:encryption-data></manifest:file-entry></manifest:manifest>"#;

#[test]
fn an_encrypted_manifest_is_encrypted_and_a_plain_one_is_not() {
    let dir = tmp("ods_enc");
    let content = content_xml("", &table("S", &row(&string("hello"))));
    let p = write(
        &dir,
        "e.ods",
        &ods_bytes(&content, None, Some(MANIFEST_ENCRYPTED)),
    );
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted);
    let p = write(
        &dir,
        "p.ods",
        &ods_bytes(&content, None, Some(MANIFEST_PLAIN)),
    );
    assert_eq!(sheet0(&load(&p).unwrap()).display(0, 0), "hello");
}

#[test]
fn encryption_wins_over_a_corrupt_content_part() {
    // Real encrypted ods has unreadable content.xml: the user must see "encrypted", not "corrupt".
    let dir = tmp("ods_enc2");
    let p = write(
        &dir,
        "e.ods",
        &ods_bytes(
            "\u{1}\u{2}garbage not xml <<<",
            None,
            Some(MANIFEST_ENCRYPTED),
        ),
    );
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted);
}

#[test]
fn manifest_detection_is_not_fooled_by_text_or_prefixes() {
    assert!(fmt_ods::manifest_is_encrypted(MANIFEST_ENCRYPTED.as_bytes()).unwrap());
    assert!(!fmt_ods::manifest_is_encrypted(MANIFEST_PLAIN.as_bytes()).unwrap());
    // The words in a comment or attribute value are not an element.
    let sneaky = r#"<m:manifest xmlns:m="x"><!-- encryption-data --><m:file-entry m:full-path="encryption-data"/></m:manifest>"#;
    assert!(!fmt_ods::manifest_is_encrypted(sneaky.as_bytes()).unwrap());
    // Any prefix works.
    let other =
        r#"<m:manifest xmlns:m="x"><m:file-entry><m:encryption-data/></m:file-entry></m:manifest>"#;
    assert!(fmt_ods::manifest_is_encrypted(other.as_bytes()).unwrap());
    assert!(fmt_ods::manifest_is_encrypted(&b"<a><b></a>"[..]).is_err());
}

// ---------------------------------------------------------------------------------------------
// the memory budget (calamine densifies every table while opening)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_huge_repeat_of_a_value_cell_is_refused_without_expanding_it() {
    let rows = r#"<table:table-row table:number-rows-repeated="1000000"><table:table-cell table:number-columns-repeated="16384" office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row>"#;
    let dir = tmp("ods_bomb");
    let p = write(
        &dir,
        "b.ods",
        &ods_bytes(&content_xml("", &table("S", rows)), None, None),
    );
    let t = std::time::Instant::now();
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge {
            what: "sheet cells"
        }
    );
    assert!(
        t.elapsed().as_secs() < 5,
        "refusal must not expand the repeat"
    );
    // The pass itself (not only the loader) refuses it.
    let r = fmt_ods::read(&p, &Limits::default());
    assert!(matches!(r, Err(OfficeError::TooLarge { .. })), "{r:?}");
}

#[test]
fn repeats_that_overflow_u64_are_clamped_not_wrapped() {
    let rows = r#"<table:table-row table:number-rows-repeated="18446744073709551615"><table:table-cell table:number-columns-repeated="18446744073709551615" office:value-type="float" office:value="1"/></table:table-row>"#;
    let dir = tmp("ods_u64");
    let p = write(
        &dir,
        "b.ods",
        &ods_bytes(&content_xml("", &table("S", rows)), None, None),
    );
    assert!(matches!(load(&p), Err(OfficeError::TooLarge { .. })));
    // An unparsable repeat: calamine rejects the file; our pass (which counts it as 1) must not
    // panic and the loader reports a clean error.
    let rows = r#"<table:table-row table:number-rows-repeated="many"><table:table-cell table:number-columns-repeated="-3" office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row>"#;
    let p = write(
        &dir,
        "c.ods",
        &ods_bytes(&content_xml("", &table("S", rows)), None, None),
    );
    assert!(fmt_ods::read(&p, &Limits::default()).is_ok());
    assert!(matches!(load(&p), Err(OfficeError::Corrupt(_))));
}

#[test]
fn a_far_corner_made_of_empty_repeats_is_refused_by_the_bounding_box() {
    // One value at A1, a million empty rows, then one value at column 16,001: the box is
    // 1M x 16,001 cells (calamine would allocate ~500 GB) although only two cells hold values.
    let rows = format!(
        r#"{}<table:table-row table:number-rows-repeated="1000000"><table:table-cell table:number-columns-repeated="16384"/></table:table-row><table:table-row><table:table-cell table:number-columns-repeated="16000"/>{}</table:table-row>"#,
        row(&float("", "1")),
        float("", "2")
    );
    let dir = tmp("ods_corner");
    let p = write(
        &dir,
        "c.ods",
        &ods_bytes(&content_xml("", &table("S", &rows)), None, None),
    );
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

#[test]
fn rows_that_each_start_wide_are_refused_by_the_read_cost() {
    // Each physical row has one value at column 1,000, so calamine builds 1,001 cells per row
    // while the bounding box is a single column: 11 rows are 11,011 > the 10,000 budget.
    let one = format!(
        r#"<table:table-row><table:table-cell table:number-columns-repeated="1000"/>{}</table:table-row>"#,
        float("", "1")
    );
    let dir = tmp("ods_cost");
    let limits = Limits {
        max_cols: 2000,
        ..small_limits()
    };
    let ok = write(
        &dir,
        "ok.ods",
        &ods_bytes(&content_xml("", &table("S", &one.repeat(9))), None, None),
    );
    assert!(load_with(&ok, limits).is_ok(), "9 rows fit the budget");
    let p = write(
        &dir,
        "bad.ods",
        &ods_bytes(&content_xml("", &table("S", &one.repeat(11))), None, None),
    );
    assert_eq!(
        load_with(&p, limits).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

#[test]
fn calamine_opens_hidden_tables_too_so_a_hidden_bomb_is_refused() {
    let rows = r#"<table:table-row table:number-rows-repeated="1000000"><table:table-cell table:number-columns-repeated="16384" office:value-type="float" office:value="1"/></table:table-row>"#;
    let tables = format!(
        "{}{}",
        table("Visible", &row(&string("ok"))),
        hidden_table("Hidden", rows)
    );
    let dir = tmp("ods_hidden_bomb");
    let p = write(
        &dir,
        "h.ods",
        &ods_bytes(&content_xml(HIDDEN_STYLE, &tables), None, None),
    );
    assert!(matches!(load(&p), Err(OfficeError::TooLarge { .. })));
}

#[test]
fn rows_beyond_excels_last_row_are_not_counted() {
    // 2M repeats are capped at 1,048,576 rows like calamine, and a single column of values over
    // that many rows is exactly at the limit: refused with the small default? No: 1M x 1 < 16M.
    let rows = r#"<table:table-row table:number-rows-repeated="2000000"><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row>"#;
    let dir = tmp("ods_rowcap");
    let p = write(
        &dir,
        "r.ods",
        &ods_bytes(&content_xml("", &table("S", rows)), None, None),
    );
    let sf = fmt_ods::read(&p, &Limits::default()).unwrap();
    let sf = &sf.sheets["S"];
    assert_eq!(sf.value_cells, 1_048_576);
    assert_eq!(sf.bbox, Some((0, 0, 1_048_575, 0)));
    // The loader then truncates to the row cap.
    let wb = load(&p).unwrap();
    assert!(sheet0(&wb).rows_truncated);
}

#[test]
fn a_dense_legitimate_table_passes_with_exact_counts() {
    let rows = row(&format!("{}{}", float("", "1"), float("", "2"))).repeat(3);
    let dir = tmp("ods_counts");
    let p = write(
        &dir,
        "c.ods",
        &ods_bytes(&content_xml("", &table("S", &rows)), None, None),
    );
    let fm = fmt_ods::read(&p, &Limits::default()).unwrap();
    let sf = &fm.sheets["S"];
    assert_eq!(sf.value_cells, 6);
    assert_eq!(sf.bbox, Some((0, 0, 2, 1)));
    assert_eq!(sf.read_cost, 6);
}

#[test]
fn nested_tables_do_not_confuse_the_pass() {
    // A table inside a cell (a drawing's embedded table) is not a sheet and its rows are not ours.
    let nested = r#"<table:table-cell office:value-type="string"><text:p>outer</text:p><table:table table:name="Inner"><table:table-row><table:table-cell office:value-type="float" office:value="9"/></table:table-row></table:table></table:table-cell>"#;
    let rows = format!("{}{}", row(nested), row(&string("below")));
    let dir = tmp("ods_nested");
    let p = write(
        &dir,
        "n.ods",
        &ods_bytes(&content_xml("", &table("S", &rows)), None, None),
    );
    let fm = fmt_ods::read(&p, &Limits::default()).unwrap();
    assert!(!fm.sheets.contains_key("Inner"));
    assert_eq!(fm.sheets["S"].value_cells, 2);
}

// ---------------------------------------------------------------------------------------------
// broken input
// ---------------------------------------------------------------------------------------------

#[test]
fn broken_content_xml_is_corrupt_not_a_panic() {
    let dir = tmp("ods_broken");
    for (i, body) in ["<office:document-content", "<a><b></a>"]
        .iter()
        .enumerate()
    {
        let p = write(&dir, &format!("b{i}.ods"), &ods_bytes(body, None, None));
        let r = std::panic::catch_unwind(|| fmt_ods::read(&p, &Limits::default()));
        assert!(r.is_ok(), "pass panicked on {body:?}");
        let r2 = assert_inner_does_not_panic(&p, body);
        assert!(r2.is_err(), "{body:?}: {r2:?}");
    }
    // No XML at all is an empty workbook (calamine and our pass agree).
    let p = write(&dir, "plain.ods", &ods_bytes("plain text", None, None));
    assert!(load(&p).unwrap().sheets.is_empty());
    // A table without a name is skipped by both readers rather than failing.
    let wb = load_ods_xml(
        "ods_noname",
        "",
        &format!(
            r#"<table:table><table:table-row/></table:table>{}"#,
            table("S", &row(&string("a")))
        ),
    );
    assert_eq!(wb.sheets.len(), 1);
}

#[test]
fn every_truncation_of_a_synthetic_ods_is_handled() {
    let content = content_xml(
        &data_style("number-style", NUM_2),
        &table("S", &row(&float("c", "1"))),
    );
    let bytes = ods_bytes(&content, None, Some(MANIFEST_PLAIN));
    let dir = tmp("ods_trunc");
    for n in 0..bytes.len() {
        let p = write(&dir, "t.ods", &bytes[..n]);
        let r = assert_inner_does_not_panic(&p, &format!("truncation at {n}"));
        assert!(r.is_err(), "a {n}-byte prefix of {} loaded", bytes.len());
    }
}

#[test]
fn mutated_ods_parts_never_panic_in_konomas_pass() {
    let Some(sample) = sample_path_or_skip("sample.ods") else {
        return;
    };
    let parts = read_parts(&sample);
    let dir = tmp("ods_mut");
    let mut seed = 0xA5A5_1234_5678_9ABCu64;
    for round in 0..200u64 {
        let mut m = parts.clone();
        for _ in 0..(1 + round % 3) {
            let which = (xorshift(&mut seed) as usize) % m.len();
            let name = m[which].0.clone();
            if name.ends_with(".xml") {
                let kind = xorshift(&mut seed);
                mutate(&mut m[which].1, &mut seed, kind);
            }
        }
        let refs: Vec<(&str, &[u8])> = m.iter().map(|(n, c)| (n.as_str(), c.as_slice())).collect();
        let p = write(&dir, "m.ods", &deflated(&refs));
        let r = std::panic::catch_unwind(|| fmt_ods::read(&p, &small_limits()));
        assert!(r.is_ok(), "the ods pass panicked in round {round}");
        let _ = load_with(&p, small_limits());
    }
}

#[test]
fn hostile_style_tables_are_bounded() {
    // Thousands of styles and a data style with thousands of parts must neither hang nor blow up.
    let mut auto = String::new();
    for i in 0..3000 {
        auto += &format!(
            r#"<number:number-style style:name="N{i}">{NUM_0}</number:number-style><style:style style:name="c{i}" style:family="table-cell" style:data-style-name="N{i}"/>"#
        );
    }
    auto += r#"<number:number-style style:name="Big">"#;
    for _ in 0..5000 {
        auto += "<number:text>x</number:text>";
    }
    auto += r#"</number:number-style><style:style style:name="cb" style:family="table-cell" style:data-style-name="Big"/>"#;
    let c = code_of(&auto, "cb").unwrap();
    assert!(c.len() < 5000, "parts are capped: {}", c.len());
    assert_eq!(code_of(&auto, "c2999").unwrap(), "#,##0");
}

// ---------------------------------------------------------------------------------------------
// every ods style kind reaches the engine: one table-driven pass over load_workbook
// ---------------------------------------------------------------------------------------------

#[test]
fn every_data_style_kind_end_to_end() {
    let styles: &[(&str, &str, &str, &str, &str)] = &[
        // (kind, body, value attrs, expected text, label)
        (
            "number-style",
            NUM_2,
            r#"office:value-type="float" office:value="1234.5""#,
            "1,234.50",
            "number",
        ),
        (
            "percentage-style",
            r#"<number:number number:decimal-places="1" number:min-integer-digits="1"/><number:text>%</number:text>"#,
            r#"office:value-type="percentage" office:value="0.256""#,
            "25.6%",
            "percentage",
        ),
        (
            "currency-style",
            r#"<number:currency-symbol>$</number:currency-symbol><number:number number:decimal-places="2" number:min-integer-digits="1" number:grouping="true"/>"#,
            r#"office:value-type="currency" office:currency="USD" office:value="1234.5""#,
            "$1,234.50",
            "currency",
        ),
        (
            "date-style",
            r#"<number:day/><number:text>.</number:text><number:month/><number:text>.</number:text><number:year number:style="long"/>"#,
            r#"office:value-type="date" office:date-value="2026-10-02""#,
            "2.10.2026",
            "date",
        ),
        (
            "time-style",
            r#"<number:hours/><number:text>h </number:text><number:minutes number:style="long"/><number:text>m</number:text>"#,
            r#"office:value-type="time" office:time-value="PT13H05M00S""#,
            "13h 05m",
            "time",
        ),
        (
            "number-style",
            r#"<number:scientific-number number:decimal-places="1" number:min-integer-digits="1" number:min-exponent-digits="2"/>"#,
            r#"office:value-type="float" office:value="12345""#,
            "1.2E+04",
            "scientific",
        ),
    ];
    let mut auto = String::new();
    let mut cells = String::new();
    for (i, (kind, body, attrs, _, _)) in styles.iter().enumerate() {
        auto += &format!(
            r#"<number:{kind} style:name="N{i}">{body}</number:{kind}><style:style style:name="c{i}" style:family="table-cell" style:data-style-name="N{i}"/>"#
        );
        cells += &format!(
            r#"<table:table-cell table:style-name="c{i}" {attrs}><text:p/></table:table-cell>"#
        );
    }
    let wb = load_ods_xml("ods_kinds", &auto, &table("S", &row(&cells)));
    let s = sheet0(&wb);
    for (i, (_, _, _, want, label)) in styles.iter().enumerate() {
        assert_eq!(s.display(0, i), *want, "{label}");
    }
}

// ---------------------------------------------------------------------------------------------
// memory: formula-only cells and the text budget
// ---------------------------------------------------------------------------------------------

#[test]
fn a_formula_only_cell_in_a_far_corner_is_refused_like_a_value_cell() {
    let far = format!(
        r#"{}<table:table-row table:number-rows-repeated="1048570"/><table:table-row><table:table-cell table:number-columns-repeated="16383"/><table:table-cell table:formula="of:=1"/></table:table-row>"#,
        row(r#"<table:table-cell table:formula="of:=1"/>"#)
    );
    let dir = tmp("ods_formula_far");
    let p = write(
        &dir,
        "f.ods",
        &ods_bytes(&content_xml("", &table("S", &far)), None, None),
    );
    let t = std::time::Instant::now();
    assert!(matches!(load(&p), Err(OfficeError::TooLarge { .. })));
    assert!(t.elapsed().as_secs() < 5);
    // The pass itself reports the box (formula-only cells included).
    let one = ods_bytes(
        &content_xml(
            "",
            &table("S", &row(r#"<table:table-cell table:formula="of:=1"/>"#)),
        ),
        None,
        None,
    );
    let p = write(&dir, "one.ods", &one);
    let fm = fmt_ods::read(&p, &Limits::default()).unwrap();
    assert_eq!(fm.sheets["S"].value_cells, 1);
}

#[test]
fn a_repeated_long_string_is_refused_by_the_text_budget() {
    // One 10 KB string repeated across 200 cells: 2 MB of text from a ~12 KB file (the budget
    // is 1 MiB here).
    let big = "t".repeat(10_000);
    let cell = |reps: u32| {
        format!(
            r#"<table:table-cell office:value-type="string" table:number-columns-repeated="{reps}"><text:p>{big}</text:p></table:table-cell>"#
        )
    };
    let limits = Limits {
        max_cols: 1000,
        max_rows: 1000,
        max_sheet_cells: 100_000,
        ..small_limits()
    };
    let dir = tmp("ods_amplify");
    let p = write(
        &dir,
        "a.ods",
        &ods_bytes(&content_xml("", &table("S", &row(&cell(200)))), None, None),
    );
    assert_eq!(
        load_with(&p, limits).unwrap_err(),
        OfficeError::TooLarge { what: "text" }
    );
    // Rows repeated count too.
    let rows = format!(
        r#"<table:table-row table:number-rows-repeated="200">{}</table:table-row>"#,
        cell(1)
    );
    let p = write(
        &dir,
        "r.ods",
        &ods_bytes(&content_xml("", &table("S", &rows)), None, None),
    );
    assert_eq!(
        load_with(&p, limits).unwrap_err(),
        OfficeError::TooLarge { what: "text" }
    );
    // A smaller repeat loads.
    let p = write(
        &dir,
        "ok.ods",
        &ods_bytes(&content_xml("", &table("S", &row(&cell(50)))), None, None),
    );
    let wb = load_with(&p, limits).unwrap();
    assert_eq!(wb.sheets[0].display(0, 49).len(), 10_000);
}

#[test]
fn string_value_attributes_and_formula_text_count_and_numbers_do_not() {
    let limits = Limits {
        max_text_bytes: 5_000,
        ..small_limits()
    };
    let dir = tmp("ods_text_kinds");
    // `office:string-value` of 100 bytes x 100 repeats = 10 KB > 5 KB.
    let sv = format!(
        r#"<table:table-cell office:value-type="string" office:string-value="{}" table:number-columns-repeated="100"/>"#,
        "s".repeat(100)
    );
    let p = write(
        &dir,
        "s.ods",
        &ods_bytes(&content_xml("", &table("S", &row(&sv))), None, None),
    );
    assert_eq!(
        load_with(
            &p,
            Limits {
                max_cols: 1000,
                ..limits
            }
        )
        .unwrap_err(),
        OfficeError::TooLarge { what: "text" }
    );
    // 5,000 repeated numeric cells (with their `<text:p>` display text) are not text.
    let n = r#"<table:table-cell office:value-type="float" office:value="1" table:number-columns-repeated="500"><text:p>1</text:p></table:table-cell>"#;
    let p = write(
        &dir,
        "n.ods",
        &ods_bytes(&content_xml("", &table("S", &row(n))), None, None),
    );
    assert!(load_with(
        &p,
        Limits {
            max_cols: 1000,
            ..limits
        }
    )
    .is_ok());
}

// ---------------------------------------------------------------------------------------------
// mutation survivors: translation details and exact budget boundaries
// ---------------------------------------------------------------------------------------------

#[test]
fn elapsed_time_units_are_bracketed_and_the_seconds_decimals_have_a_cap() {
    // ODF 1.2 19.363 `number:truncate-on-overflow="false"`: the unit is an elapsed total.
    let t = |body: &str| code("time-style", body);
    for (el, short, long) in [
        ("hours", "[h]", "[hh]"),
        ("minutes", "[m]", "[mm]"),
        ("seconds", "[s]", "[ss]"),
    ] {
        assert_eq!(
            t(&format!(
                r#"<number:{el} number:truncate-on-overflow="false"/>"#
            )),
            short
        );
        assert_eq!(
            t(&format!(
                r#"<number:{el} number:style="long" number:truncate-on-overflow="false"/>"#
            )),
            long
        );
        // Truncating (the default) is the plain unit.
        assert_eq!(
            t(&format!(r#"<number:{el}/>"#)),
            short.trim_matches(['[', ']'])
        );
    }
    assert_eq!(render("[m]", 0.5), "720");
    assert_eq!(render("[mm]", 5.0 / 1440.0), "05");
    // Seconds decimals: that many zeros, at most 6.
    let s = |d: &str| t(&format!(r#"<number:seconds number:decimal-places="{d}"/>"#));
    assert_eq!(s("0"), "s");
    assert_eq!(s("1"), "s.0");
    assert_eq!(s("3"), "s.000");
    assert_eq!(s("4"), "s.0000");
    assert_eq!(s("6"), "s.000000");
    assert_eq!(s("7"), "s.000000");
    assert_eq!(s("99999"), "s.000000");
}

#[test]
fn scientific_style_defaults_and_clamps() {
    let sci = |attrs: &str| {
        code(
            "number-style",
            &format!("<number:scientific-number {attrs}/>"),
        )
    };
    // No attribute at all: 2 decimals, 1 integer digit, 2 exponent digits (the shape of 1.23E+04).
    assert_eq!(sci(""), "0.00E+00");
    assert_eq!(render(&sci(""), 12345.0), "1.23E+04");
    // Each default is independent of the others.
    assert_eq!(sci(r#"number:decimal-places="0""#), "0E+00");
    assert_eq!(sci(r#"number:min-integer-digits="3""#), "000.00E+00");
    assert_eq!(sci(r#"number:min-exponent-digits="3""#), "0.00E+000");
    // Clamps: integer digits 1..=30, exponent digits 1..=5, decimals at most 30.
    assert_eq!(sci(r#"number:min-integer-digits="0""#), "0.00E+00");
    assert_eq!(sci(r#"number:min-exponent-digits="0""#), "0.00E+0");
    assert_eq!(sci(r#"number:min-exponent-digits="99""#), "0.00E+00000");
    assert_eq!(
        sci(r#"number:min-integer-digits="99""#),
        format!("{}.00E+00", "0".repeat(30))
    );
    assert_eq!(
        sci(r#"number:decimal-places="99""#),
        format!("0.{}E+00", "0".repeat(30))
    );
}

#[test]
fn number_style_decimals_and_integer_digits_are_capped_at_30() {
    let n = |attrs: &str| code("number-style", &format!("<number:number {attrs}/>"));
    assert_eq!(
        n(r#"number:decimal-places="30" number:min-integer-digits="1""#),
        format!("0.{}", "0".repeat(30))
    );
    assert_eq!(
        n(r#"number:decimal-places="31" number:min-integer-digits="1""#),
        format!("0.{}", "0".repeat(30))
    );
    assert_eq!(
        n(r#"number:decimal-places="0" number:min-integer-digits="30""#),
        "0".repeat(30)
    );
    assert_eq!(
        n(r#"number:decimal-places="0" number:min-integer-digits="31""#),
        "0".repeat(30)
    );
    assert_eq!(
        n(r#"number:decimal-places="0" number:min-integer-digits="0""#),
        "0",
        "at least one integer digit"
    );
}

#[test]
fn xml_entities_in_literal_text_are_decoded() {
    // The five predefined entities, and numeric character references.
    let lit = |t: &str| {
        code(
            "number-style",
            &format!("<number:text>{t}</number:text>{NUM_0}"),
        )
    };
    assert_eq!(lit("a&lt;b"), "\"a<b\"#,##0");
    assert_eq!(lit("a&gt;b"), "\"a>b\"#,##0");
    assert_eq!(lit("a&amp;b"), "\"a&b\"#,##0");
    assert_eq!(lit("a&apos;b"), "\"a'b\"#,##0");
    assert_eq!(lit("a&quot;b"), "\"a\"\\\"\"b\"#,##0");
    assert_eq!(lit("&#x20AC;"), "\"€\"#,##0");
    assert_eq!(lit("&#8364;"), "\"€\"#,##0");
}

#[test]
fn only_the_first_two_maps_of_a_style_shape_the_code() {
    // Three maps: the code uses the first two (a format code has at most 3 numeric sections).
    let auto = format!(
        r#"<number:number-style style:name="NP">{NUM_0}</number:number-style>
           <number:number-style style:name="NN"><number:text>-</number:text>{NUM_0}</number:number-style>
           <number:number-style style:name="NX"><number:text>x</number:text>{NUM_0}</number:number-style>
           <number:number-style style:name="N"><number:text>zero</number:text><style:map style:condition="value()&gt;0" style:apply-style-name="NP"/><style:map style:condition="value()&lt;0" style:apply-style-name="NN"/><style:map style:condition="value()=5" style:apply-style-name="NX"/></number:number-style>
           <style:style style:name="c" style:family="table-cell" style:data-style-name="N"/>"#
    );
    assert_eq!(code_of(&auto, "c").unwrap(), "#,##0;-#,##0;\"zero\"");
}

#[test]
fn a_date_cell_with_a_general_data_style_shows_the_serial_not_the_iso_default() {
    // Design choice (documented in `finish_cell`): a cell that *has* a style is never given the
    // ISO fallback, even when its data style translates to General, so the date is the number
    // General makes of it. (An unstyled date cell shows `2026-10-02`, see
    // `typed_cells_without_any_style_get_a_readable_default`.)
    let auto = data_style(
        "number-style",
        r#"<number:number number:min-integer-digits="1"/>"#,
    );
    let rows = row(&format!(
        r#"{}<table:table-cell table:style-name="c" office:value-type="time" office:time-value="PT12H00M00S"><text:p/></table:table-cell>"#,
        date("c", "2026-10-02")
    ));
    let wb = load_ods_xml("ods_styled_general_date", &auto, &table("S", &rows));
    let s = sheet0(&wb);
    assert_eq!(s.display(0, 0), "46297");
    assert_eq!(s.display(0, 1), "0.5");
    // The same cells without a style do get the readable defaults.
    let rows = row(&format!(
        r#"{}<table:table-cell office:value-type="time" office:time-value="PT12H00M00S"><text:p/></table:table-cell>"#,
        date("", "2026-10-02")
    ));
    let wb = load_ods_xml("ods_unstyled_date", "", &table("S", &rows));
    assert_eq!(sheet0(&wb).display(0, 0), "2026-10-02");
    assert_eq!(sheet0(&wb).display(0, 1), "12:00:00");
    // A *column* default style counts as a style as well.
    let cols = r#"<table:table-column table:default-cell-style-name="c"/>"#;
    let rows = row(&date("", "2026-10-02"));
    let wb = load_ods_xml(
        "ods_column_general_date",
        &auto,
        &table("S", &format!("{cols}{rows}")),
    );
    assert_eq!(sheet0(&wb).display(0, 0), "46297");
}

/// Reads one table and returns its `SheetFormats` under `limits`.
fn read_table(
    name: &str,
    rows: &str,
    limits: &Limits,
) -> Result<fmt_xlsx::XlsxFormats, OfficeError> {
    let dir = tmp(name);
    let p = write(
        &dir,
        "t.ods",
        &ods_bytes(&content_xml("", &table("S", rows)), None, None),
    );
    fmt_ods::read(&p, limits)
}

#[test]
fn row_repeats_stop_at_the_last_row_counting_the_rows_before_them() {
    // 1,048,570 rows of nothing, then a row repeated 100 times: only the 6 rows that are left
    // (1,048,570..=1,048,575) exist, like calamine.
    let rows = r#"<table:table-row table:number-rows-repeated="1048570"><table:table-cell/></table:table-row><table:table-row table:number-rows-repeated="100"><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row>"#;
    let fm = read_table("ods_rows_left", rows, &Limits::default()).unwrap();
    let sf = &fm.sheets["S"];
    assert_eq!(sf.value_cells, 6);
    assert_eq!(sf.bbox, Some((1_048_570, 0, 1_048_575, 0)));
}

#[test]
fn rows_past_the_last_row_are_ignored_not_counted() {
    // The first repeat uses up all 1,048,576 rows; a later row has no room at all and must not
    // count, widen the bounding box or underflow.
    let rows = r#"<table:table-row table:number-rows-repeated="1048576"><table:table-cell/></table:table-row><table:table-row><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row><table:table-row table:number-rows-repeated="5"><table:table-cell office:value-type="float" office:value="2"><text:p>2</text:p></table:table-cell></table:table-row>"#;
    let fm = read_table("ods_rows_none_left", rows, &Limits::default()).unwrap();
    let sf = &fm.sheets["S"];
    assert_eq!(sf.value_cells, 0);
    assert_eq!(sf.bbox, None);
    assert_eq!(sf.bbox_area(), 0);
}

#[test]
fn the_ods_budgets_are_exact_at_their_boundaries() {
    let limits = |max: u64| Limits {
        max_dense_cells: max,
        max_cols: 2000,
        ..Limits::default()
    };
    let too_large_area = || OfficeError::TooLarge { what: "sheet area" };
    let too_large_cells = || OfficeError::TooLarge {
        what: "sheet cells",
    };

    // value cells: 10 values in a row against a budget of 10 (the box and the read cost are 10
    // as well, so this one is exactly full on all three).
    let ten = row(&float("", "1").repeat(10));
    assert!(read_table("ods_cells_ok", &ten, &limits(10)).is_ok());
    assert_eq!(
        read_table("ods_cells_over", &ten, &limits(9)).unwrap_err(),
        too_large_cells()
    );

    // bounding box: two values at A1 and CV100 span 100 x 100 = 10,000 cells while only two
    // cells hold values and the widest row reads 100 cells.
    let corner = format!(
        r#"{}<table:table-row table:number-rows-repeated="98"><table:table-cell/></table:table-row><table:table-row><table:table-cell table:number-columns-repeated="99"/>{}</table:table-row>"#,
        row(&float("", "1")),
        float("", "2")
    );
    let fm = read_table("ods_box_ok", &corner, &limits(10_000)).unwrap();
    assert_eq!(fm.sheets["S"].bbox_area(), 10_000);
    assert_eq!(fm.sheets["S"].value_cells, 2);
    assert_eq!(
        read_table("ods_box_over", &corner, &limits(9_999)).unwrap_err(),
        too_large_area()
    );

    // read cost: 10 rows that each read 1,000 cells (a value at column 1,000) against 10,000.
    let wide = format!(
        r#"<table:table-row><table:table-cell table:number-columns-repeated="999"/>{}</table:table-row>"#,
        float("", "1")
    );
    let ten_rows = wide.repeat(10);
    let fm = read_table("ods_cost_ok", &ten_rows, &limits(10_000)).unwrap();
    assert_eq!(fm.sheets["S"].read_cost, 10_000);
    assert_eq!(fm.sheets["S"].bbox_area(), 10);
    assert_eq!(
        read_table("ods_cost_over", &ten_rows, &limits(9_999)).unwrap_err(),
        too_large_area()
    );
}

#[test]
fn a_format_code_over_255_characters_is_dropped_and_255_is_kept() {
    // The data style is `"<text>"0.0`: 2 quotes + the text + `0.0`. Total 255 / 256 characters.
    let style = |n: usize| {
        data_style(
            "number-style",
            &format!(
                r#"<number:text>{}</number:text><number:number number:decimal-places="1" number:min-integer-digits="1"/>"#,
                "t".repeat(n)
            ),
        )
    };
    let load_one = |name: &str, n: usize| {
        let wb = load_ods_xml(name, &style(n), &table("S", &row(&float("c", "0.5"))));
        (sheet0(&wb).display(0, 0).to_string(), wb.formats.clone())
    };
    // `"ttt…"0.0` is 2 + n + 3 characters: n = 250 -> 255, n = 251 -> 256.
    let (shown, formats) = load_one("ods_code_keep", 250);
    assert!(
        shown.starts_with("ttt") && shown.ends_with("0.5"),
        "{shown}"
    );
    assert!(formats
        .iter()
        .any(|f| matches!(f, NumFmtRef::Custom(c) if c.chars().count() == 255)));
    let (shown, formats) = load_one("ods_code_drop", 251);
    assert_eq!(shown, "0.5", "256 characters is not a format Excel wrote");
    // And it is not held: the workbook's format table never carries a code over the limit.
    assert!(
        formats
            .iter()
            .all(|f| !matches!(f, NumFmtRef::Custom(c) if c.chars().count() > 255)),
        "{formats:?}"
    );
}

#[test]
fn only_the_sheet_asked_for_is_taken_out_of_the_workbook() {
    let dir = tmp("ods_one");
    let p = write(
        &dir,
        "t.ods",
        &ods_bytes(
            &content_xml(
                "",
                &format!(
                    "{}{}",
                    table("A", &row(&float("", "1"))),
                    table("B", &row(&float("", "2")))
                ),
            ),
            None,
            None,
        ),
    );
    let wb = load_workbook_sheet(&p, &LoadOptions::default(), 1).unwrap();
    assert_eq!(wb.loaded_index(), Some(1));
    assert_eq!(wb.sheets[1].display(0, 0), "2");
    assert!(!wb.sheets[0].loaded);
}

/// `calamine` keeps every table's matrix once the file is open, so the area budget is for the
/// workbook, not for each table: tables that are each fine can still be too many together.
#[test]
fn the_area_budget_is_for_the_whole_workbook_not_each_table() {
    let limits = small_limits(); // 10,000 cells
    let wide = |cols: u32| {
        row(&format!(
            r#"<table:table-cell table:number-columns-repeated="{cols}" office:value-type="float" office:value="1"/>"#
        ))
    };
    let book = |sizes: &[u32]| {
        let tables: String = sizes
            .iter()
            .enumerate()
            .map(|(i, &c)| table(&format!("T{i}"), &wide(c)))
            .collect();
        ods_bytes(&content_xml("", &tables), None, None)
    };
    let dir = tmp("ods_total_area");
    let one = write(&dir, "one.ods", &book(&[9000]));
    assert!(
        fmt_ods::read(&one, &limits).is_ok(),
        "one table under the budget"
    );
    // 5,000 + 5,000 is exactly the budget; one cell more is over it.
    let exact = write(&dir, "exact.ods", &book(&[5000, 5000]));
    assert!(
        fmt_ods::read(&exact, &limits).is_ok(),
        "exactly 10,000 in total"
    );
    let over = write(&dir, "over.ods", &book(&[5000, 5001]));
    assert_eq!(
        fmt_ods::read(&over, &limits).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
    // Many small tables: each is nothing, together they are 12,000 cells. Hidden ones count.
    let many = write(&dir, "many.ods", &book(&[1000; 12]));
    assert_eq!(
        load_with(&many, limits).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
    let tables = format!(
        "{}{}",
        table("A", &wide(6000)),
        hidden_table("B", &wide(6000))
    );
    let hid = write(
        &dir,
        "hid.ods",
        &ods_bytes(&content_xml(HIDDEN_STYLE, &tables), None, None),
    );
    assert_eq!(
        load_with(&hid, limits).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

/// The tables are summed one by one: two tables with one name are two matrices in memory.
#[test]
fn tables_that_share_a_name_are_all_counted_in_the_workbook_total() {
    let limits = small_limits(); // 10,000 cells
    let wide = |cols: u32| {
        row(&format!(
            r#"<table:table-cell table:number-columns-repeated="{cols}" office:value-type="float" office:value="1"/>"#
        ))
    };
    let tables = format!("{}{}", table("S", &wide(6000)), table("S", &wide(6000)));
    let dir = tmp("ods_same_name_area");
    let p = write(
        &dir,
        "same.ods",
        &ods_bytes(&content_xml("", &tables), None, None),
    );
    assert_eq!(
        fmt_ods::read(&p, &limits).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

// ---------------------------------------------------------------------------------------------
// elapsed time, embedded text, engineering notation, fractions
// ---------------------------------------------------------------------------------------------

/// A time style with the given attributes on the style and the given children.
fn time_code(style_attrs: &str, body: &str) -> String {
    let auto = format!(
        r#"<number:time-style style:name="N" {style_attrs}>{body}</number:time-style><style:style style:name="c" style:family="table-cell" style:data-style-name="N"/>"#
    );
    code_of(&auto, "c").unwrap()
}

const HMS: &str = r#"<number:hours/><number:text>:</number:text><number:minutes number:style="long"/><number:text>:</number:text><number:seconds number:style="long"/>"#;

#[test]
fn the_elapsed_flag_on_the_time_style_unbounds_its_first_unit_only() {
    // LibreOffice writes `truncate-on-overflow="false"` on the style, never on the children.
    let c = time_code(r#"number:truncate-on-overflow="false""#, HMS);
    assert_eq!(c, "[h]:mm:ss");
    assert_eq!(render(&c, 1.5), "36:00:00");
    // The first unit present is the unbounded one, whichever it is.
    let c = time_code(
        r#"number:truncate-on-overflow="false""#,
        r#"<number:minutes number:style="long"/><number:text>:</number:text><number:seconds number:style="long"/>"#,
    );
    assert_eq!(c, "[mm]:ss");
    assert_eq!(render(&c, 61.2 / 1440.0), "61:12");
    let c = time_code(
        r#"number:truncate-on-overflow="false""#,
        r#"<number:seconds number:style="long"/>"#,
    );
    assert_eq!(c, "[ss]");
    // A text before the first unit does not use it up.
    let c = time_code(
        r#"number:truncate-on-overflow="false""#,
        r#"<number:text>T=</number:text><number:hours number:style="long"/><number:text>:</number:text><number:minutes number:style="long"/>"#,
    );
    assert_eq!(c, "\"T=\"[hh]:mm");
    // Not elapsed: `false` is the only value that turns it on.
    for attrs in ["", r#"number:truncate-on-overflow="true""#] {
        assert_eq!(time_code(attrs, HMS), "h:mm:ss", "{attrs}");
    }
    // The old per-element spelling still works.
    let c = time_code(
        "",
        r#"<number:hours number:truncate-on-overflow="false"/><number:text>:</number:text><number:minutes number:style="long"/>"#,
    );
    assert_eq!(c, "[h]:mm");
}

#[test]
fn the_elapsed_flag_does_not_leak_into_the_next_style() {
    // Two styles in one file: the second is an ordinary clock time.
    let auto = format!(
        r#"<number:time-style style:name="E" number:truncate-on-overflow="false">{HMS}</number:time-style><number:time-style style:name="C">{HMS}</number:time-style><style:style style:name="e" style:family="table-cell" style:data-style-name="E"/><style:style style:name="c" style:family="table-cell" style:data-style-name="C"/>"#
    );
    assert_eq!(code_of(&auto, "e").unwrap(), "[h]:mm:ss");
    assert_eq!(code_of(&auto, "c").unwrap(), "h:mm:ss");
}

#[test]
fn an_elapsed_duration_cell_goes_through_the_whole_pipeline() {
    let auto = format!(
        r#"<number:time-style style:name="N" number:truncate-on-overflow="false">{HMS}</number:time-style><style:style style:name="c" style:family="table-cell" style:data-style-name="N"/>"#
    );
    let rows = row(
        r#"<table:table-cell table:style-name="c" office:value-type="time" office:time-value="PT36H00M00S"><text:p/></table:table-cell><table:table-cell table:style-name="c" office:value-type="float" office:value="100.25"><text:p/></table:table-cell>"#,
    );
    let wb = load_ods_xml("ods_elapsed", &auto, &table("S", &rows));
    assert_eq!(sheet0(&wb).display(0, 0), "36:00:00");
    assert_eq!(sheet0(&wb).display(0, 1), "2406:00:00");
}

fn number_code(body: &str) -> String {
    code("number-style", body)
}

#[test]
fn embedded_text_is_inserted_between_the_digits() {
    // 090-1234-5678: positions count the digits to the right of the text.
    let c = number_code(
        r#"<number:number number:decimal-places="0" number:min-integer-digits="11"><number:embedded-text number:position="8">-</number:embedded-text><number:embedded-text number:position="4">-</number:embedded-text></number:number>"#,
    );
    assert_eq!(c, "000-0000-0000");
    assert_eq!(render(&c, 9_012_345_678.0), "090-1234-5678");
    // Position 0: after the last digit (before the decimals).
    let c = number_code(
        r#"<number:number number:decimal-places="1" number:min-integer-digits="1"><number:embedded-text number:position="0">m</number:embedded-text></number:number>"#,
    );
    assert_eq!(c, "0\"m\".0");
    assert_eq!(render(&c, 5.0), "5m.0");
    // Past the leftmost digit: in front of all of them.
    let c = number_code(
        r#"<number:number number:decimal-places="0" number:min-integer-digits="2"><number:embedded-text number:position="9">#</number:embedded-text></number:number>"#,
    );
    assert_eq!(c, "\"#\"00");
    // Next to grouping and other elements of the style.
    let c = code(
        "currency-style",
        r#"<number:currency-symbol>¥</number:currency-symbol><number:number number:decimal-places="0" number:min-integer-digits="4" number:grouping="true"><number:embedded-text number:position="2">/</number:embedded-text></number:number>"#,
    );
    assert_eq!(c, "\"¥\"0,0/00");
    // A text with nothing in it, and an embedded text outside a number, change nothing.
    assert_eq!(
        number_code(
            r#"<number:number number:min-integer-digits="1" number:decimal-places="0"><number:embedded-text number:position="1"></number:embedded-text></number:number>"#
        ),
        "0"
    );
    assert_eq!(
        number_code(r#"<number:embedded-text number:position="1">x</number:embedded-text>"#),
        "General"
    );
}

#[test]
fn embedded_texts_never_run_away() {
    let texts: String = (0..500)
        .map(|i| format!(r#"<number:embedded-text number:position="{i}">x</number:embedded-text>"#))
        .collect();
    let c = number_code(&format!(
        r#"<number:number number:decimal-places="0" number:min-integer-digits="3">{texts}</number:number>"#
    ));
    assert!(c.len() < 2000, "{}", c.len());
    // A position that is not a number is 0.
    let c = number_code(
        r#"<number:number number:decimal-places="0" number:min-integer-digits="2"><number:embedded-text number:position="-1">x</number:embedded-text><number:embedded-text number:position="9999999999999999999999">y</number:embedded-text></number:number>"#,
    );
    assert!(
        c.starts_with('"') || c.ends_with('"') || c.contains("00"),
        "{c}"
    );
}

#[test]
fn engineering_notation_follows_the_exponent_interval() {
    let sci = |attrs: &str| {
        code(
            "number-style",
            &format!("<number:scientific-number {attrs}/>"),
        )
    };
    assert_eq!(
        sci(
            r#"number:decimal-places="1" number:min-integer-digits="1" number:min-exponent-digits="1" number:exponent-interval="3""#
        ),
        "##0.0E+0"
    );
    assert_eq!(
        render("##0.0E+0", 12345.0),
        "12.3E+3",
        "the engine reads what we write"
    );
    assert_eq!(
        sci(
            r#"number:decimal-places="2" number:min-integer-digits="1" number:exponent-interval="2""#
        ),
        "#0.00E+00"
    );
    // The interval never makes the integer part narrower than its minimum.
    assert_eq!(
        sci(
            r#"number:decimal-places="0" number:min-integer-digits="3" number:min-exponent-digits="2" number:exponent-interval="1""#
        ),
        "000E+00"
    );
    // No interval: the usual notation.
    assert_eq!(
        sci(
            r#"number:decimal-places="2" number:min-integer-digits="1" number:min-exponent-digits="2""#
        ),
        "0.00E+00"
    );
    // The sign is forced unless the file says it is not.
    assert_eq!(
        sci(
            r#"number:decimal-places="1" number:min-integer-digits="1" number:min-exponent-digits="2" number:forced-exponent-sign="false""#
        ),
        "0.0E-00"
    );
    assert_eq!(
        sci(
            r#"number:decimal-places="1" number:min-integer-digits="1" number:min-exponent-digits="2" number:forced-exponent-sign="true""#
        ),
        "0.0E+00"
    );
    // A huge interval is bounded.
    let c = sci(r#"number:decimal-places="1" number:exponent-interval="999999""#);
    assert!(c.len() < 80, "{c}");
}

#[test]
#[allow(clippy::approx_constant)]
fn fractions_have_an_integer_part_when_the_style_says_so() {
    let f = |attrs: &str| code("number-style", &format!("<number:fraction {attrs}/>"));
    // LibreOffice: `# ??/??`.
    let mixed = f(
        r#"number:min-integer-digits="0" number:min-numerator-digits="2" number:min-denominator-digits="2" number:max-denominator-value="99""#,
    );
    assert_eq!(mixed, "# ??/??");
    assert_eq!(render(&mixed, 3.14159), "3 14/99");
    assert_eq!(render(&mixed, 0.75), "  3/4 ");
    // LibreOffice: `# ?/?` shows no integer for a value below 1 but keeps the slot.
    let one = f(
        r#"number:min-integer-digits="0" number:min-numerator-digits="1" number:min-denominator-digits="1" number:max-denominator-value="9""#,
    );
    assert_eq!(one, "# ?/?");
    assert_eq!(render(&one, 0.75), " 3/4");
    // `?/?`: no `min-integer-digits` at all, an improper fraction.
    let improper = f(
        r#"number:min-numerator-digits="2" number:min-denominator-digits="2" number:max-denominator-value="99""#,
    );
    assert_eq!(improper, "??/??");
    assert_eq!(render(&improper, 3.14159), "311/99");
    // The largest denominator allowed widens a too-narrow placeholder.
    assert_eq!(
        f(
            r#"number:min-integer-digits="0" number:min-numerator-digits="1" number:min-denominator-digits="1" number:max-denominator-value="999""#
        ),
        "# ?/???"
    );
    // A fixed denominator.
    assert_eq!(
        f(
            r#"number:min-integer-digits="0" number:min-numerator-digits="1" number:denominator-value="8""#
        ),
        "# ?/8"
    );
}

// ---------------------------------------------------------------------------------------------
// the ods calendar (null date, no 1900 leap-year bug)
// ---------------------------------------------------------------------------------------------

fn date_style_auto(body: &str) -> String {
    data_style("date-style", body)
}

const YMD: &str = r#"<number:year number:style="long"/><number:text>-</number:text><number:month number:style="long"/><number:text>-</number:text><number:day number:style="long"/>"#;
const YMD_DAY: &str = r#"<number:year number:style="long"/><number:text>-</number:text><number:month number:style="long"/><number:text>-</number:text><number:day number:style="long"/><number:text> </number:text><number:day-of-week number:style="long"/>"#;

fn null_date(iso: &str) -> String {
    format!(
        r#"<table:calculation-settings><table:null-date table:value-type="date" table:date-value="{iso}"/></table:calculation-settings>"#
    )
}

#[test]
fn ods_dates_use_the_ods_calendar_not_excels() {
    let auto = date_style_auto(YMD_DAY);
    let rows = row(&format!(
        "{}{}{}{}{}{}",
        float("c", "0"),
        float("c", "-1"),
        float("c", "1"),
        float("c", "61"),
        date("c", "1899-12-30"),
        date("c", "1800-01-01"),
    ));
    let wb = load_ods_xml("ods_cal", &auto, &table("S", &rows));
    let s = sheet0(&wb);
    // Excel would show `1900-01-00` for 0, `########` below it and be a day off before 1900-03-01.
    assert_eq!(s.display(0, 0), "1899-12-30 Saturday");
    assert_eq!(s.display(0, 1), "1899-12-29 Friday");
    assert_eq!(s.display(0, 2), "1899-12-31 Sunday");
    assert_eq!(s.display(0, 3), "1900-03-01 Thursday");
    assert_eq!(s.display(0, 4), "1899-12-30 Saturday");
    assert_eq!(s.display(0, 5), "1800-01-01 Wednesday");
}

#[test]
fn ods_dates_around_1900_are_exact() {
    // Every day from 1899-12-25 to 1900-03-05 keeps its own date (Excel's 1900 bug shifts them).
    let auto = date_style_auto(YMD);
    let days = [
        "1899-12-25",
        "1899-12-31",
        "1900-01-01",
        "1900-01-31",
        "1900-02-01",
        "1900-02-27",
        "1900-02-28",
        "1900-03-01",
        "1900-03-02",
        "1900-03-05",
    ];
    let rows = row(&days.iter().map(|d| date("c", d)).collect::<String>());
    let wb = load_ods_xml("ods_cal1900", &auto, &table("S", &rows));
    for (i, d) in days.iter().enumerate() {
        assert_eq!(sheet0(&wb).display(0, i), *d);
    }
}

#[test]
fn a_number_with_a_date_format_counts_from_the_null_date() {
    let auto = date_style_auto(YMD);
    // Default null date 1899-12-30; 46300 days later.
    let wb = load_ods_xml("ods_nd0", &auto, &table("S", &row(&float("c", "46300"))));
    assert_eq!(sheet0(&wb).display(0, 0), "2026-10-05");
    // A null date of 1904-01-01: numbers count from it, ISO dates stay absolute.
    let tables = format!(
        "{}{}",
        null_date("1904-01-01"),
        table(
            "S",
            &row(&format!(
                "{}{}{}",
                float("c", "0"),
                float("c", "1462"),
                date("c", "2026-10-05")
            ))
        )
    );
    let wb = load_ods_xml("ods_nd1904", &auto, &tables);
    assert_eq!(sheet0(&wb).display(0, 0), "1904-01-01");
    assert_eq!(sheet0(&wb).display(0, 1), "1908-01-02");
    assert_eq!(sheet0(&wb).display(0, 2), "2026-10-05");
    // Any other null date works the same.
    let tables = format!(
        "{}{}",
        null_date("2000-01-01"),
        table("S", &row(&float("c", "1")))
    );
    let wb = load_ods_xml("ods_nd2000", &auto, &tables);
    assert_eq!(sheet0(&wb).display(0, 0), "2000-01-02");
}

#[test]
fn a_broken_null_date_falls_back_to_the_default() {
    let auto = date_style_auto(YMD);
    for bad in ["", "garbage", "2000-13-01", "2000-01"] {
        let tables = format!("{}{}", null_date(bad), table("S", &row(&float("c", "0"))));
        let wb = load_ods_xml("ods_ndbad", &auto, &tables);
        assert_eq!(sheet0(&wb).display(0, 0), "1899-12-30", "{bad:?}");
    }
}

#[test]
fn ods_calendar_edges_do_not_panic() {
    let auto = date_style_auto(YMD);
    let cells: String = [
        "1e300", "-1e300", "-1e9", "1e9", "2958465", "2958466", "-693593", "-693594", "NaN",
    ]
    .iter()
    .map(|v| float("c", v))
    .collect();
    let wb = load_ods_xml("ods_caledge", &auto, &table("S", &row(&cells)));
    let s = sheet0(&wb);
    // 0001-01-01 is the first day, 9999-12-31 the last; outside is a row of hashes.
    assert_eq!(s.display(0, 6), "0001-01-01");
    assert_eq!(s.display(0, 7), "########");
    assert_eq!(s.display(0, 0), "########");
    assert_eq!(s.display(0, 1), "########");
    assert_eq!(s.display(0, 3), "########");
    // A negative number with a time format is not a date.
    let auto = data_style(
        "time-style",
        r#"<number:hours number:style="long"/><number:text>:</number:text><number:minutes number:style="long"/>"#,
    );
    let wb = load_ods_xml("ods_negtime", &auto, &table("S", &row(&float("c", "-0.5"))));
    assert_eq!(sheet0(&wb).display(0, 0), "########");
}

// ---------------------------------------------------------------------------------------------
// text with tabs and line breaks
// ---------------------------------------------------------------------------------------------

fn text_cell(attrs: &str, body: &str) -> String {
    format!(r#"<table:table-cell office:value-type="string" {attrs}>{body}</table:table-cell>"#)
}

#[test]
fn tabs_and_line_breaks_in_a_cell_are_kept() {
    let rows = row(&format!(
        "{}{}{}{}",
        text_cell("", r#"<text:p>a<text:tab/>b</text:p>"#),
        text_cell("", r#"<text:p>one<text:line-break/>two</text:p>"#),
        text_cell(
            "",
            r#"<text:p>x<text:s text:c="3"/>y<text:tab/>z</text:p><text:p>second</text:p><text:p/><text:p>fourth</text:p>"#
        ),
        text_cell(
            "",
            r#"<text:p><text:span>in <text:a>a link</text:a></text:span><text:tab/>end</text:p>"#
        ),
    ));
    let wb = load_ods_xml("ods_tabs", "", &table("S", &rows));
    let s = sheet0(&wb);
    assert_eq!(s.display(0, 0), "a\tb");
    assert_eq!(s.display(0, 1), "one\ntwo");
    assert_eq!(s.display(0, 2), "x   y\tz\nsecond\n\nfourth");
    assert_eq!(s.display(0, 3), "in a link\tend");
}

#[test]
fn a_cell_without_tabs_is_left_to_calamine() {
    // The pass rebuilds nothing unless a tab or a line break is there: spaces, paragraphs and
    // entities read the way calamine reads them.
    let rows = row(&format!(
        "{}{}",
        text_cell(
            "",
            r#"<text:p>a<text:s/>b<text:s text:c="2"/>c &amp; &lt;d&gt;</text:p><text:p>next</text:p>"#
        ),
        text_cell("", r#"<text:p>plain</text:p>"#),
    ));
    let wb = load_ods_xml("ods_notabs", "", &table("S", &rows));
    assert_eq!(sheet0(&wb).display(0, 0), "a b  c & <d>\nnext");
    assert_eq!(sheet0(&wb).display(0, 1), "plain");
}

#[test]
fn a_comment_in_the_cell_is_not_its_text() {
    let ann = r#"<office:annotation><dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">me</dc:creator><text:p>note<text:tab/>x</text:p></office:annotation>"#;
    let rows = row(&text_cell(
        "",
        &format!(r#"{ann}<text:p>a<text:tab/>b</text:p>"#),
    ));
    let wb = load_ods_xml("ods_ann", "", &table("S", &rows));
    assert_eq!(sheet0(&wb).display(0, 0), "a\tb");
}

#[test]
fn a_string_value_attribute_is_the_text_and_is_not_rebuilt() {
    let rows = row(&text_cell(
        r#"office:string-value="from attribute""#,
        r#"<text:p>shown<text:tab/>text</text:p>"#,
    ));
    let wb = load_ods_xml("ods_strval", "", &table("S", &rows));
    assert_eq!(sheet0(&wb).display(0, 0), "from attribute");
}

#[test]
fn repeated_cells_with_a_tab_are_all_repaired() {
    let cell = text_cell(
        r#"table:number-columns-repeated="3""#,
        r#"<text:p>a<text:tab/>b</text:p>"#,
    );
    let rows = format!(
        r#"<table:table-row table:number-rows-repeated="2">{cell}</table:table-row>{}"#,
        row(&string("z"))
    );
    let wb = load_ods_xml("ods_tabrep", "", &table("S", &rows));
    let s = sheet0(&wb);
    for r in 0..2 {
        for c in 0..3 {
            assert_eq!(s.display(r, c), "a\tb", "({r},{c})");
        }
    }
    assert_eq!(s.display(2, 0), "z");
}

#[test]
fn tabs_in_cells_after_empty_ones_land_on_the_right_cell() {
    let rows = format!(
        r#"<table:table-row table:number-rows-repeated="5"><table:table-cell table:number-columns-repeated="1024"/></table:table-row>{}"#,
        row(&format!(
            r#"<table:table-cell table:number-columns-repeated="3"/>{}"#,
            text_cell("", r#"<text:p>t<text:tab/>t</text:p>"#)
        ))
    );
    let wb = load_ods_xml("ods_tabpos", "", &table("S", &rows));
    assert_eq!(sheet0(&wb).display(5, 3), "t\tt");
}

#[test]
fn a_huge_space_count_is_counted_against_the_text_budget() {
    // calamine allocates `text:c` spaces; the budget must see them before it does.
    let rows = row(&text_cell(
        "",
        r#"<text:p>a<text:s text:c="4000000000"/>b</text:p>"#,
    ));
    let dir = tmp("ods_hugespace");
    let p = write(
        &dir,
        "s.ods",
        &ods_bytes(&content_xml("", &table("S", &rows)), None, None),
    );
    assert_eq!(
        load_with(&p, small_limits()).unwrap_err(),
        OfficeError::TooLarge { what: "text" }
    );
}

#[test]
fn a_very_long_cell_with_a_tab_is_not_collected_twice() {
    let long = "x".repeat((1 << 20) + 10);
    let rows = row(&text_cell(
        "",
        &format!("<text:p>{long}<text:tab/>y</text:p>"),
    ));
    let wb = load_ods_xml("ods_longtab", "", &table("S", &rows));
    // Over the repair limit: left as calamine read it (still the whole text, minus the tab).
    let d = sheet0(&wb).display(0, 0);
    assert!(d.starts_with("xxx") && d.ends_with("xy"), "{}", d.len());
}

// ---------------------------------------------------------------------------------------------
// merged ranges
// ---------------------------------------------------------------------------------------------

fn merged(cs: u32, rs: u32, inner: &str) -> String {
    format!(
        r#"<table:table-cell table:number-columns-spanned="{cs}" table:number-rows-spanned="{rs}" office:value-type="string"><text:p>{inner}</text:p></table:table-cell>"#
    )
}

fn covered(n: u32) -> String {
    format!(r#"<table:covered-table-cell table:number-columns-repeated="{n}"/>"#)
}

fn merges_of(name: &str, tables: &str) -> Vec<MergeRange> {
    let wb = load_ods_xml(name, "", tables);
    let mut m = wb.sheets[0].merges.clone();
    m.sort_by_key(|r| (r.row0, r.col0));
    m
}

fn mr(row0: u32, col0: u32, row1: u32, col1: u32) -> MergeRange {
    MergeRange {
        row0,
        col0,
        row1,
        col1,
    }
}

#[test]
fn merged_ranges_are_read_from_the_spans() {
    // A1:C2, then (after its covered cells) D2:D4.
    let rows = format!(
        "<table:table-row>{}{}</table:table-row><table:table-row>{}{}</table:table-row>{}",
        merged(3, 2, "a"),
        covered(2),
        covered(3),
        merged(1, 3, "b"),
        row(&string("plain")),
    );
    let m = merges_of("ods_merge", &table("S", &rows));
    assert_eq!(m, [mr(0, 0, 1, 2), mr(1, 3, 3, 3)]);
    // A span of 1 x 1 is not a merge, and neither is a missing or garbled attribute.
    let rows = row(&format!(
        "{}{}{}",
        merged(1, 1, "x"),
        r#"<table:table-cell table:number-columns-spanned="zz" office:value-type="string"><text:p>y</text:p></table:table-cell>"#,
        r#"<table:table-cell table:number-columns-spanned="0" table:number-rows-spanned="0" office:value-type="string"><text:p>y</text:p></table:table-cell>"#,
    ));
    assert!(merges_of("ods_nomerge", &table("S", &rows)).is_empty());
}

#[test]
fn an_empty_merged_cell_is_a_merge_too() {
    let rows = row(
        r#"<table:table-cell table:number-columns-spanned="2" table:number-rows-spanned="1"/><table:covered-table-cell/>"#,
    );
    assert_eq!(
        merges_of("ods_emptymerge", &table("S", &rows)),
        [mr(0, 0, 0, 1)]
    );
}

#[test]
fn merged_ranges_follow_repeats_and_are_clamped() {
    // A spanned cell repeated: one range per repetition.
    let rows = r#"<table:table-row table:number-rows-repeated="2"><table:table-cell table:number-columns-repeated="2" table:number-columns-spanned="2"/><table:covered-table-cell/></table:table-row>"#;
    let m = merges_of("ods_mergerep", &table("S", rows));
    assert_eq!(
        m,
        [
            mr(0, 0, 0, 1),
            mr(0, 1, 0, 2),
            mr(1, 0, 1, 1),
            mr(1, 1, 1, 2)
        ]
    );
    // A span past the last column / row stops at Excel's limit; leading empty rows and cells
    // shift the range.
    let rows = r#"<table:table-row table:number-rows-repeated="3"/><table:table-row><table:table-cell table:number-columns-repeated="2"/><table:table-cell table:number-columns-spanned="4294967295" table:number-rows-spanned="4294967295"/></table:table-row>"#;
    assert_eq!(
        merges_of("ods_mergeclamp", &table("S", rows)),
        [mr(3, 2, 1_048_575, 16_383)]
    );
}

#[test]
fn merged_ranges_are_capped_and_stay_with_their_table() {
    // 1,000 x 200 repeated spanned cells (200,000) are more than the cap.
    let cell = r#"<table:table-cell table:number-columns-repeated="200" table:number-columns-spanned="2"/>"#;
    let rows =
        format!(r#"<table:table-row table:number-rows-repeated="1000">{cell}</table:table-row>"#);
    let wb = load_ods_xml("ods_mergecap", "", &table("S", &rows));
    assert_eq!(wb.sheets[0].merges.len(), 100_000);
    // Each table keeps its own.
    let tables = format!(
        "{}{}",
        table("A", &row(&merged(2, 2, "a"))),
        table("B", &row(&string("b")))
    );
    let dir = tmp("ods_mergetables");
    let p = write(
        &dir,
        "m.ods",
        &ods_bytes(&content_xml("", &tables), None, None),
    );
    let wb = load(&p).unwrap();
    assert_eq!(wb.sheets[0].merges, [mr(0, 0, 1, 1)]);
    assert!(wb.sheets[1].merges.is_empty());
}

// ---------------------------------------------------------------------------------------------
// encrypted packages
// ---------------------------------------------------------------------------------------------

const MANIFEST_PACKAGE: &str = r#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.4"><manifest:file-entry manifest:full-path="encrypted-package" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet" manifest:size="6409"><manifest:encryption-data><manifest:algorithm manifest:algorithm-name="http://www.w3.org/2009/xmlenc11#aes256-gcm"/></manifest:encryption-data></manifest:file-entry></manifest:manifest>"#;

fn package(mime: &[u8], manifest: &str, with_package: bool) -> Vec<u8> {
    let mut entries: Vec<(&str, &[u8])> = Vec::new();
    if !mime.is_empty() {
        entries.push(("mimetype", mime));
    }
    if with_package {
        entries.push(("encrypted-package", b"\x00\x01\x02 not a zip"));
    }
    entries.push(("META-INF/manifest.xml", manifest.as_bytes()));
    deflated(&entries)
}

const SHEET_MIME: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";

#[test]
fn a_single_package_encrypted_ods_is_encrypted_not_unsupported() {
    let dir = tmp("ods_pkg");
    let p = write(&dir, "e.ods", &package(SHEET_MIME, MANIFEST_PACKAGE, true));
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted);
    // A template is a spreadsheet too, and trailing newline / spaces in the mimetype are fine.
    let p = write(
        &dir,
        "t.ods",
        &package(
            b"application/vnd.oasis.opendocument.spreadsheet-template\n",
            MANIFEST_PACKAGE,
            true,
        ),
    );
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted);
    // The manifest alone says so when the package entry has another name.
    let p = write(&dir, "m.ods", &package(SHEET_MIME, MANIFEST_PACKAGE, false));
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted);
}

#[test]
fn an_encrypted_text_or_presentation_document_is_still_not_ours() {
    let dir = tmp("ods_pkg_other");
    for mime in [
        &b"application/vnd.oasis.opendocument.text"[..],
        b"application/vnd.oasis.opendocument.presentation",
        b"",
        b"application/zip",
    ] {
        let p = write(&dir, "x.ods", &package(mime, MANIFEST_PACKAGE, true));
        assert_eq!(
            load(&p).unwrap_err(),
            OfficeError::Unsupported,
            "{}",
            String::from_utf8_lossy(mime)
        );
    }
    // A spreadsheet without content and without any sign of encryption is not "encrypted".
    let p = write(&dir, "n.ods", &package(SHEET_MIME, MANIFEST_PLAIN, false));
    assert_eq!(load(&p).unwrap_err(), OfficeError::Unsupported);
    // A broken manifest does not turn into a panic or an "encrypted".
    let p = write(&dir, "b.ods", &package(SHEET_MIME, "<a><b></a>", false));
    assert_eq!(load(&p).unwrap_err(), OfficeError::Unsupported);
}

#[test]
fn an_old_style_encrypted_ods_keeps_its_content_part_and_is_encrypted() {
    // Per-entry encryption (LibreOffice before 25, OpenOffice): `content.xml` is there, unreadable,
    // and the manifest has the `encryption-data`.
    let dir = tmp("ods_old_enc");
    let p = write(
        &dir,
        "o.ods",
        &ods_bytes("\u{1}\u{2} cipher text", None, Some(MANIFEST_ENCRYPTED)),
    );
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted);
}
