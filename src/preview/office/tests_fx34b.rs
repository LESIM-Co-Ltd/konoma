//! Tests added with the fourth review of the Word / OpenDocument text preview: the line-start
//! guards inside `<sup>`/`<sub>`, the bound on reading chart parts, per-character font slots of
//! `w:rFonts`, the deletion carry of OpenDocument, legacy form fields across paragraphs, and a row
//! that only skips grid columns. The comment on each test says what it catches.

use super::docx::DocOptions;
use super::tests_docx::*;
use super::tests_odt as od;
use std::time::Instant;

const SUP: &str = r#"<w:vertAlign w:val="superscript"/>"#;
const SUB: &str = r#"<w:vertAlign w:val="subscript"/>"#;

fn xe(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The first marker of every block kind a Markdown line can start with.
const STARTERS: [&str; 10] = [
    "# Heading injected",
    "> quoted",
    "- item",
    "+ plus",
    "* star",
    "1. one",
    "---",
    "***",
    "___",
    "=== eq",
];

/// What the reader sees must hold the text itself, as text: no heading, quote, list or rule.
fn assert_literal(m: &str, t: &str, what: &str) {
    let v = visible(m).replace('\u{200B}', "");
    assert!(
        v.contains(t),
        "{what}: `{t}` is not shown as text\nMD: {m:?}\nVIS: {v:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// 1. line-start guards under <sup> / <sub>
// ---------------------------------------------------------------------------------------------

/// The renderer strips `<sup>`/`<sub>` before it reads blocks, so the first character inside a tag
/// at a line start begins the line (catches `esc(core, false)`: `<sup># x</sup>` became a heading,
/// `<sup>- x</sup>` a list, `<sup>***</sup>` a rule).
#[test]
fn text_in_sup_or_sub_at_a_line_start_is_not_taken_for_a_block() {
    for t in STARTERS {
        for (tag, rpr) in [("sup", SUP), ("sub", SUB)] {
            let m = md(&para(&runp(rpr, &xe(t))));
            assert_literal(&m, t, &format!("{tag} at the start"));
            // After a line break inside the run, and after a break between runs.
            let m = md(&para(&format!(
                "<w:r><w:rPr>{rpr}</w:rPr><w:t>a</w:t><w:br/><w:t>{}</w:t></w:r>",
                xe(t)
            )));
            assert_literal(&m, t, &format!("{tag} after a break"));
            let m = md(&para(
                &(run("a") + "<w:r><w:br/></w:r>" + &runp(rpr, &xe(t))),
            ));
            assert_literal(&m, t, &format!("{tag} after a break run"));
        }
    }
}

/// The same guard for the first paragraph of a note (a different line start).
#[test]
fn text_in_sup_at_a_note_start_is_not_taken_for_a_block() {
    for t in ["# Heading injected", "- item", "***"] {
        let note = format!(
            r#"<w:footnote w:id="1"><w:p><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p></w:footnote>"#,
            xe(t)
        );
        let _ = note;
        let note = format!(
            r#"<w:footnote w:id="1"><w:p><w:r><w:rPr>{SUP}</w:rPr><w:t xml:space="preserve">{}</w:t></w:r></w:p></w:footnote>"#,
            xe(t)
        );
        let d = Dx::new(&para(
            &(run("body") + r#"<w:r><w:footnoteReference w:id="1"/></w:r>"#),
        ))
        .footnotes(&note);
        let m = conv(&d).markdown;
        assert_literal(&m, t, "note start");
    }
}

/// OpenDocument writes a raise as `style:text-position`; it goes through the same emitter.
#[test]
fn odt_raised_text_at_a_line_start_is_not_taken_for_a_block() {
    let auto = od::text_style("T1", r#"style:text-position="super 58%""#, None)
        + &od::text_style("T2", r#"style:text-position="sub 58%""#, None);
    for t in STARTERS {
        for s in ["T1", "T2"] {
            let m = od::md_auto(&od::p(&od::span(s, &xe(t))), &auto);
            assert_literal(&m, t, &format!("odt {s}"));
            let m = od::md_auto(
                &od::p(&format!("a<text:line-break/>{}", od::span(s, &xe(t)))),
                &auto,
            );
            assert_literal(&m, t, &format!("odt {s} after a break"));
        }
    }
}

/// A bare `<sup>` keeps its punctuation inside the tag: only emphasis markers move it out (catches
/// the `!marks.is_empty()` gates on the punctuation move being dropped).
#[test]
fn punctuation_stays_inside_a_bare_sup() {
    assert_eq!(
        md(&para(&(run("10") + &runp(SUP, "-6") + &run("m")))),
        "10<sup>-6</sup>m"
    );
    assert_eq!(
        md(&para(&(run("x") + &runp(SUP, "(1)") + &run("y")))),
        "x<sup>(1)</sup>y"
    );
    assert_eq!(
        md(&para(&(run("a") + &runp(SUB, "(i,j)") + &run("b")))),
        "a<sub>(i,j)</sub>b"
    );
}

// ---------------------------------------------------------------------------------------------
// 2. charts: a part is read once, and a document reads a bounded total
// ---------------------------------------------------------------------------------------------

fn chart_ref(rid: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline><wp:docPr id="1" name="G 1"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="{rid}"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

/// A chart part of about 480 KB whose title stands first.
fn big_chart(title: &str) -> String {
    big_chart_body(&format!(
        r#"<c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>{title}</a:t></a:r></a:p></c:rich></c:tx></c:title>"#
    ))
}

/// A big chart part whose `c:chart` starts with `head`.
fn big_chart_body(head: &str) -> String {
    let junk = "<c:x/>".repeat(80_000);
    format!(
        r#"<?xml version="1.0"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart>{head}<c:plotArea>{junk}</c:plotArea></c:chart></c:chartSpace>"#
    )
}

/// A chart drawn many times is one part: it is read once, so a document of references to one big
/// part costs one read (catches the part being opened and scanned at every reference: 100
/// references took 9 s).
#[test]
fn a_chart_drawn_many_times_is_read_once() {
    let n = 150;
    let body = para(&chart_ref("rIdC").repeat(n));
    let d = Dx::new(&body)
        .rel("rIdC", "chart", "charts/chart1.xml", false)
        .part("word/charts/chart1.xml", big_chart_body("").as_bytes());
    let t = Instant::now();
    let m = conv(&d).markdown;
    let took = t.elapsed();
    // (A part with no title is scanned to its end: the worst case.)
    assert_eq!(m.matches("chart").count(), n, "{m}");
    eprintln!("took {took:?}");
    assert!(took.as_secs_f64() < 5.0, "took {took:?}");
}

/// The answer for a part is kept: every drawing of a titled chart shows the title, also past the
/// bytes a document may read (catches the cache being dropped: only the first few were titled).
#[test]
fn every_drawing_of_one_chart_shows_its_title() {
    let n = 60;
    let d = Dx::new(&para(&chart_ref("rIdC").repeat(n)))
        .rel("rIdC", "chart", "charts/chart1.xml", false)
        .part("word/charts/chart1.xml", big_chart("Sales").as_bytes());
    let m = conv(&d).markdown;
    assert_eq!(m.matches("chart: Sales").count(), n, "{m}");
}

/// Many different big chart parts: a document reads only so many bytes of them (the first ones are
/// titled, the rest are a plain `chart`), whatever the count (catches a bound that is per part
/// only: 2000 parts took over a minute).
#[test]
fn the_chart_parts_of_a_document_are_read_up_to_a_total_bound() {
    let n = 40;
    let mut body = String::new();
    let mut d = Dx::new("");
    for i in 0..n {
        body.push_str(&para(&chart_ref(&format!("rIdC{i}"))));
        d = d
            .rel(
                &format!("rIdC{i}"),
                "chart",
                &format!("charts/chart{i}.xml"),
                false,
            )
            .part(
                &format!("word/charts/chart{i}.xml"),
                big_chart(&format!("T{i}")).as_bytes(),
            );
    }
    d.body = body;
    let t = Instant::now();
    let m = conv(&d).markdown;
    let took = t.elapsed();
    let titled = m.matches("chart: T").count();
    assert!((1..=12).contains(&titled), "{titled} titled\n{m}");
    assert_eq!(
        m.matches("chart").count(),
        n,
        "every chart is still marked\n{m}"
    );
    assert!(m.contains("chart: T0"), "{m}");
    eprintln!("took {took:?}");
    assert!(took.as_secs_f64() < 6.0, "took {took:?}");
}

/// Small charts are all titled: the total bound is far above what real documents use.
#[test]
fn a_hundred_small_charts_all_keep_their_titles() {
    let small = |t: &str| {
        format!(
            r#"<?xml version="1.0"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>{t}</a:t></a:r></a:p></c:rich></c:tx></c:title></c:chart></c:chartSpace>"#
        )
    };
    let mut body = String::new();
    let mut d = Dx::new("");
    for i in 0..100 {
        body.push_str(&para(&chart_ref(&format!("rIdC{i}"))));
        d = d
            .rel(
                &format!("rIdC{i}"),
                "chart",
                &format!("charts/chart{i}.xml"),
                false,
            )
            .part(
                &format!("word/charts/chart{i}.xml"),
                small(&format!("T{i}")).as_bytes(),
            );
    }
    d.body = body;
    let m = conv(&d).markdown;
    for i in 0..100 {
        assert!(m.contains(&format!("chart: T{i}")), "T{i}\n{m}");
    }
}

// ---------------------------------------------------------------------------------------------
// 3. fonts: each character takes the slot Word uses for it
// ---------------------------------------------------------------------------------------------

fn rfonts(attrs: &str, text: &str) -> String {
    format!(r#"<w:r><w:rPr><w:rFonts {attrs}/></w:rPr><w:t>{text}</w:t></w:r>"#)
}

/// A symbol font in a slot a character does not use leaves it alone (catches "any slot names a
/// symbol font, so the whole run is symbols": `abc` in Calibri turned into `αβχ` for a `cs` slot
/// of Symbol, and `abc` into `♋♌♍` for an `eastAsia` slot of Wingdings).
#[test]
fn a_symbol_font_in_another_slot_does_not_change_latin_letters() {
    for slot in ["cs", "eastAsia"] {
        for font in ["Symbol", "Wingdings"] {
            let a = format!(r#"w:ascii="Calibri" w:hAnsi="Calibri" w:{slot}="{font}""#);
            assert_eq!(md(&para(&rfonts(&a, "abc"))), "abc", "{slot} {font}");
        }
    }
    // A run's `hAnsi` slot is not the one ASCII letters use.
    let a = r#"w:ascii="Calibri" w:hAnsi="Wingdings""#;
    assert_eq!(md(&para(&rfonts(a, "abc"))), "abc");
}

/// ASCII letters take the `ascii` slot (the symbol case stays as it was).
#[test]
fn ascii_letters_take_the_ascii_slot() {
    let a = r#"w:ascii="Symbol" w:hAnsi="Calibri""#;
    assert_eq!(md(&para(&rfonts(a, "abc"))), "\u{3b1}\u{3b2}\u{3c7}");
    let a = r#"w:ascii="Wingdings" w:hAnsi="Wingdings""#;
    assert_eq!(md(&para(&rfonts(a, "l"))), "\u{25CF}");
}

/// A private-use code (`U+F0xx`, how Word stores a symbol-font character) takes the `ascii` slot
/// (a style may name only that one), and the `hAnsi` slot when `ascii` is not set.
#[test]
fn a_private_use_symbol_code_takes_the_ascii_slot() {
    let a = r#"w:ascii="Wingdings" w:hAnsi="Calibri""#;
    assert_eq!(md(&para(&rfonts(a, "\u{F06C}"))), "\u{25CF}");
    let a = r#"w:ascii="Calibri" w:hAnsi="Wingdings""#;
    // `ascii` is a plain font: a private-use code shows that a symbol stood there.
    assert_eq!(md(&para(&rfonts(a, "\u{F06C}"))), "\u{2022}");
    let a = r#"w:hAnsi="Wingdings""#;
    assert_eq!(md(&para(&rfonts(a, "\u{F06C}"))), "\u{25CF}");
}

/// East Asian characters take `eastAsia`; the `hint` picks it for the characters both scripts use.
#[test]
fn east_asian_text_takes_the_east_asia_slot() {
    let a = r#"w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Wingdings""#;
    // A Wingdings `eastAsia` slot maps the code of a kana only when it is a code; kana are not
    // codes of the font, so they stay (they are U+30xx, above 0xFF).
    assert_eq!(md(&para(&rfonts(a, "\u{3042}"))), "\u{3042}");
    // `°` (U+00B0) follows the hint: Wingdings code 0xB0 is a symbol, with the hint; plain without.
    let with_hint =
        r#"w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Wingdings" w:hint="eastAsia""#;
    let plain = r#"w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Wingdings""#;
    assert_eq!(md(&para(&rfonts(plain, "\u{B0}"))), "\u{B0}");
    assert_ne!(md(&para(&rfonts(with_hint, "\u{B0}"))), "\u{B0}");
}

/// A theme font replaces the named font of its slot, and is never a symbol font.
#[test]
fn a_theme_font_is_not_a_symbol_font() {
    let a =
        r#"w:ascii="Symbol" w:asciiTheme="minorHAnsi" w:hAnsi="Symbol" w:hAnsiTheme="minorHAnsi""#;
    assert_eq!(md(&para(&rfonts(a, "abc"))), "abc");
}

/// A run's slot overrides the style's for that slot only (the others stay the style's).
#[test]
fn a_run_overrides_the_font_slot_by_slot() {
    let styles = r#"<w:style w:type="character" w:styleId="W"><w:name w:val="W"/><w:rPr><w:rFonts w:ascii="Wingdings" w:hAnsi="Wingdings"/></w:rPr></w:style>"#;
    let r = |rpr: &str, t: &str| runp(&format!(r#"<w:rStyle w:val="W"/>{rpr}"#), t);
    // `\u{B7}` is a `hAnsi` character: what a run in Wingdings shows for it.
    let sym = md(&para(&rfonts(
        r#"w:ascii="Wingdings" w:hAnsi="Wingdings""#,
        "\u{B7}",
    )));
    assert_ne!(sym, "\u{B7}");
    // The run changes only `ascii`: `l` is plain; `\u{B7}` is still the style's.
    let body = para(&r(r#"<w:rFonts w:ascii="Calibri"/>"#, "l\u{B7}"));
    assert_eq!(md_styled(&body, styles), format!("l{sym}"));
    // The run changes only `hAnsi`: `\u{B7}` is plain; `l` is still the style's.
    let body = para(&r(r#"<w:rFonts w:hAnsi="Calibri"/>"#, "l\u{B7}"));
    assert_eq!(md_styled(&body, styles), "\u{25CF}\u{B7}");
}

// ---------------------------------------------------------------------------------------------
// 3b. the bare edge of the private-use range
// ---------------------------------------------------------------------------------------------

/// The last code of the private-use symbol block (`U+F0FF`) is a symbol too (catches the range
/// ending one short).
#[test]
fn the_last_private_use_symbol_code_is_shown_as_a_symbol() {
    assert_eq!(md(&para(&run("\u{F0FF}"))), "\u{2022}");
    assert_eq!(md(&para(&run("\u{F020}"))), "\u{2022}");
    assert_eq!(md(&para(&run("\u{F01F}"))), "\u{F01F}");
}

// ---------------------------------------------------------------------------------------------
// 4. OpenDocument deletion carry
// ---------------------------------------------------------------------------------------------

/// A deletion across very many paragraphs reads in time proportional to their number (catches the
/// whole carry being scanned at each paragraph: quadratic).
#[test]
fn a_deletion_over_many_paragraphs_is_linear() {
    let regions = r#"<text:tracked-changes text:track-changes="false"><text:changed-region xml:id="d1" text:id="d1"><text:deletion><office:change-info><dc:creator>A</dc:creator></office:change-info><text:p/></text:deletion></text:changed-region></text:tracked-changes>"#;
    let n = 30_000;
    // The first paragraph puts `n` segments in the carry; the `n` empty ones after it each pass
    // it on.
    let body = format!(
        "{regions}<text:p>{}<text:change-start text:change-id=\"d1\"/></text:p>{}<text:p><text:change-end text:change-id=\"d1\"/>end</text:p>",
        "<text:line-break/>".repeat(n),
        "<text:p/>".repeat(n)
    );
    let t = Instant::now();
    let m = od::conv(&od::Ox::new(&body)).markdown;
    let took = t.elapsed();
    assert!(m.contains("end"), "{}", m.len());
    eprintln!("took {took:?}");
    assert!(took.as_secs_f64() < 4.0, "took {took:?}");
}

/// What the carry holds still decides whether a lone formula is a display one: a deletion that
/// carries text into the formula's paragraph keeps it inline.
#[test]
fn a_formula_after_carried_text_stays_inline() {
    let regions = r#"<text:tracked-changes text:track-changes="false"><text:changed-region xml:id="d1" text:id="d1"><text:deletion><office:change-info><dc:creator>A</dc:creator></office:change-info><text:p/></text:deletion></text:changed-region></text:tracked-changes>"#;
    let body = format!(
        "{regions}<text:p>before <text:change-start text:change-id=\"d1\"/></text:p><text:p><text:change-end text:change-id=\"d1\"/>after</text:p>"
    );
    let m = od::md(&body);
    assert!(m.contains("before") && m.contains("after"), "{m}");
}

// ---------------------------------------------------------------------------------------------
// 5. legacy form fields
// ---------------------------------------------------------------------------------------------

fn ff_begin(ffdata: &str) -> String {
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="F1"/><w:enabled/>{ffdata}</w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMTEXT </w:instrText></w:r>"#
    )
}
const SEP: &str = r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#;
const END: &str = r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
const DEFAULT: &str = r#"<w:textInput><w:default w:val="DEF"/></w:textInput>"#;

/// A field with no result shows its default; one with a result shows the result, whether the field
/// is in one paragraph or spans several (catches `result_from`, a position in one paragraph, being
/// compared with the length of another).
#[test]
fn a_form_field_shows_its_default_only_without_a_result() {
    // No result at all.
    assert_eq!(md(&para(&(ff_begin(DEFAULT) + END))), "DEF");
    assert_eq!(md(&para(&(ff_begin(DEFAULT) + SEP + END))), "DEF");
    // A result in the paragraph.
    assert_eq!(
        md(&para(&(ff_begin(DEFAULT) + SEP + &run("typed") + END))),
        "typed"
    );
    // A result that began in an earlier, longer paragraph (more segments before the end than the
    // closing paragraph holds).
    let body = para(&(ff_begin(DEFAULT) + SEP + &run("a") + &run("b") + &run("c") + &run("d")))
        + &para(&(run("e") + END));
    let m = md(&body);
    assert!(!m.contains("DEF"), "{m}");
    // A result that is all in an earlier paragraph; the closing paragraph is empty.
    let body = para(&(ff_begin(DEFAULT) + SEP + &run("kept"))) + &para(END);
    let m = md(&body);
    assert!(m.contains("kept") && !m.contains("DEF"), "{m}");
    // No result in either paragraph: the default.
    let body = para(&(ff_begin(DEFAULT) + SEP)) + &para(END);
    assert!(md(&body).contains("DEF"));
}

/// Hidden text (`w:vanish`) hides the field's form data too.
#[test]
fn a_hidden_form_field_shows_no_default() {
    let hidden = r#"<w:r><w:rPr><w:vanish/></w:rPr><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="F1"/><w:enabled/><w:textInput><w:default w:val="DEF"/></w:textInput></w:ffData></w:fldChar></w:r>"#;
    let end_hidden = r#"<w:r><w:rPr><w:vanish/></w:rPr><w:fldChar w:fldCharType="end"/></w:r>"#;
    let m = md(&para(&(run("a ") + hidden + SEP + &run("b ") + END)));
    assert!(!m.contains("DEF"), "{m}");
    // Hidden only at the end mark.
    let m = md(&para(&(run("a ") + &ff_begin(DEFAULT) + end_hidden)));
    assert!(!m.contains("DEF"), "{m}");
    // Shown when nothing hides it.
    let m = md(&para(&(run("a ") + &ff_begin(DEFAULT) + END)));
    assert!(m.contains("DEF"), "{m}");
}

// ---------------------------------------------------------------------------------------------
// 6. table rows that only skip columns; the budget of skipped columns
// ---------------------------------------------------------------------------------------------

/// A row of no cells is no row, even when `w:gridBefore` skips columns: it does not widen the
/// table (catches the empty 64-column row).
#[test]
fn a_row_with_no_cells_and_a_grid_before_is_dropped() {
    let tbl = format!(
        r#"<w:tbl><w:tr>{}{}</w:tr><w:tr><w:trPr><w:gridBefore w:val="64"/></w:trPr></w:tr></w:tbl>"#,
        "<w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc>",
        "<w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc>",
    );
    assert_eq!(md(&tbl), "| a | b |\n| --- | --- |");
}

/// The columns a row skips count against the table's cell budget (catches the `cells += before`
/// being dropped: rows that only skip columns were free).
#[test]
fn skipped_grid_columns_count_against_the_cell_budget() {
    let row = r#"<w:tr><w:trPr><w:gridBefore w:val="10"/></w:trPr><w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr>"#;
    let tbl = format!("<w:tbl>{}</w:tbl>", row.repeat(6));
    let opts = DocOptions {
        max_table_cells: 25,
        ..DocOptions::default()
    };
    let d = conv_with(&Dx::new(&tbl), &opts).unwrap();
    assert!(d.truncated, "{}", d.markdown);
    assert!(d.markdown.matches('x').count() <= 2, "{}", d.markdown);
}

// ---------------------------------------------------------------------------------------------
// 7. a no-break space in a formula
// ---------------------------------------------------------------------------------------------

/// A no-break space in a formula is a LaTeX space (`\ `): left bare it would be dropped by the
/// formula check and the formula shown as text (catches the NBSP arm being removed).
#[test]
fn a_no_break_space_in_a_formula_is_a_latex_space() {
    let o = "<m:oMath><m:r><m:t>a\u{a0}b</m:t></m:r></m:oMath>";
    let m = md(&para(&(run("x ") + o + &run(" y"))));
    assert!(m.contains(r"$a\ b$"), "{m}");
}
