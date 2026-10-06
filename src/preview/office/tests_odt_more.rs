//! Tests of the OpenDocument text reader (`odt.rs`): lists and their numbering, formulas, the
//! package (kind, encryption, damage), limits and hostile input.

use super::docx::*;
use super::tests::{deflated, tmp, write};
use super::tests_docx::{tiny_png, visible};
use super::tests_odt::*;
use super::{Cancel, OfficeError};

// ---------------------------------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------------------------------

/// A numbered list style: `(format, prefix, suffix)` per level.
fn nstyle(name: &str, levels: &[(&str, &str, &str)]) -> String {
    let mut s = format!(r#"<text:list-style style:name="{name}">"#);
    for (i, (f, pre, suf)) in levels.iter().enumerate() {
        s += &format!(
            r#"<text:list-level-style-number text:level="{}" style:num-format="{f}" style:num-prefix="{pre}" style:num-suffix="{suf}"/>"#,
            i + 1
        );
    }
    s + "</text:list-style>"
}

fn bstyle(name: &str) -> String {
    format!(
        r#"<text:list-style style:name="{name}"><text:list-level-style-bullet text:level="1" text:bullet-char="•"/><text:list-level-style-bullet text:level="2" text:bullet-char="◦"/></text:list-style>"#
    )
}

fn li(text: &str) -> String {
    format!("<text:list-item><text:p>{text}</text:p></text:list-item>")
}

fn lst(style: &str, items: &str) -> String {
    format!(r#"<text:list text:style-name="{style}">{items}</text:list>"#)
}

fn lst_attrs(style: &str, attrs: &str, items: &str) -> String {
    format!(r#"<text:list text:style-name="{style}" {attrs}>{items}</text:list>"#)
}

// ---------------------------------------------------------------------------------------------
// bullets and decimal lists (real Markdown lists)
// ---------------------------------------------------------------------------------------------

#[test]
fn bullets_nest() {
    let auto = bstyle("B");
    let body = lst(
        "B",
        &(li("a")
            + &format!(
                "<text:list-item><text:p>b</text:p><text:list>{}{}</text:list></text:list-item>",
                li("b1"),
                li("b2")
            )
            + &li("c")),
    );
    assert_eq!(md_auto(&body, &auto), "- a\n- b\n  - b1\n  - b2\n- c");
}

#[test]
fn a_list_without_a_style_or_with_a_bullet_or_image_level_is_a_bullet_list() {
    assert_eq!(
        md(&format!("<text:list>{}{}</text:list>", li("a"), li("b"))),
        "- a\n- b"
    );
    let auto = r#"<text:list-style style:name="Img"><text:list-level-style-image text:level="1"/></text:list-style>"#;
    assert_eq!(md_auto(&lst("Img", &li("a")), auto), "- a");
    // A style the document does not define, and a level the style does not define.
    assert_eq!(md(&lst("Missing", &li("a"))), "- a");
    let auto = nstyle("N", &[("1", "", ".")]);
    let deep = "<text:list-item><text:p>x</text:p><text:list><text:list-item><text:p>y</text:p></text:list-item></text:list></text:list-item>".to_string();
    assert_eq!(md_auto(&lst("N", &deep), &auto), "1. x\n   - y");
}

#[test]
fn a_decimal_list_is_a_real_numbered_list() {
    let auto = nstyle("N", &[("1", "", "."), ("1", "", ".")]);
    let body = lst("N", &(li("one") + &li("two") + &li("three")));
    assert_eq!(md_auto(&body, &auto), "1. one\n2. two\n3. three");
}

#[test]
fn numbers_restart_per_nesting_level() {
    let auto = nstyle("N", &[("1", "", "."), ("1", "", ".")]);
    let sub = |t: &str, a: &str, b: &str| {
        format!(
            "<text:list-item><text:p>{t}</text:p><text:list>{}{}</text:list></text:list-item>",
            li(a),
            li(b)
        )
    };
    let body = lst("N", &(sub("A", "a1", "a2") + &sub("B", "b1", "b2")));
    assert_eq!(
        md_auto(&body, &auto),
        "1. A\n   1. a1\n   2. a2\n2. B\n   1. b1\n   2. b2"
    );
}

#[test]
fn start_values_come_from_the_level_and_from_an_item() {
    let auto = r#"<text:list-style style:name="N"><text:list-level-style-number text:level="1" style:num-format="1" style:num-suffix="." text:start-value="5"/></text:list-style>"#;
    assert_eq!(
        md_auto(&lst("N", &(li("a") + &li("b"))), auto),
        "5. a\n6. b"
    );
    let auto = nstyle("N", &[("1", "", ".")]);
    let body = lst(
        "N",
        &(li("a")
            + r#"<text:list-item text:start-value="10"><text:p>b</text:p></text:list-item>"#
            + &li("c")),
    );
    assert_eq!(md_auto(&body, &auto), "1. a\n10. b\n11. c");
}

#[test]
fn a_list_can_continue_the_one_before_it() {
    let auto = nstyle("N", &[("1", "", ".")]);
    // Continued by style (`continue-numbering`) after a paragraph.
    let body = lst("N", &(li("a") + &li("b")))
        + &p("between")
        + &lst_attrs(
            "N",
            r#"text:continue-numbering="true""#,
            &(li("c") + &li("d")),
        );
    assert_eq!(md_auto(&body, &auto), "1. a\n2. b\n\nbetween\n\n3. c\n4. d");
    // Without it the numbering restarts.
    let body = lst("N", &(li("a") + &li("b"))) + &p("between") + &lst("N", &(li("c") + &li("d")));
    assert_eq!(md_auto(&body, &auto), "1. a\n2. b\n\nbetween\n\n1. c\n2. d");
}

#[test]
fn a_list_continues_another_by_its_id() {
    let auto = nstyle("N", &[("1", "", ".")]) + &nstyle("M", &[("1", "", ".")]);
    let body = lst_attrs("N", r#"xml:id="l1""#, &(li("a") + &li("b")))
        + &lst_attrs("M", r#"xml:id="l2""#, &li("other"))
        + &lst_attrs("N", r#"text:continue-list="l1""#, &li("c"))
        + &lst_attrs("N", r#"text:continue-list="nope""#, &li("fresh"));
    let m = md_auto(&body, &auto);
    assert!(m.contains("3. c"), "{m}");
    assert!(m.contains("1. fresh"), "{m}");
    assert!(m.contains("1. other"), "{m}");
}

#[test]
fn two_lists_next_to_each_other_stay_two_lists() {
    let auto = nstyle("N", &[("1", "", ".")]) + &bstyle("B");
    let body = lst("N", &(li("a") + &li("b"))) + &lst("N", &li("c"));
    assert_eq!(md_auto(&body, &auto), "1. a\n2. b\n\n<!-- -->\n\n1. c");
    // A bullet list after a numbered one needs no separator.
    let body = lst("N", &li("a")) + &lst("B", &li("b"));
    assert_eq!(md_auto(&body, &auto), "1. a\n- b");
}

#[test]
fn a_list_header_is_text_and_does_not_count() {
    let auto = nstyle("N", &[("1", "", ".")]);
    let body = lst(
        "N",
        &("<text:list-header><text:p>Header text</text:p></text:list-header>".to_string()
            + &li("one")
            + &li("two")),
    );
    assert_eq!(md_auto(&body, &auto), "Header text\n\n1. one\n2. two");
}

#[test]
fn an_item_with_several_paragraphs_keeps_them_in_the_item() {
    let auto = nstyle("N", &[("1", "", ".")]);
    let body = lst(
        "N",
        "<text:list-item><text:p>first</text:p><text:p>second</text:p><text:p>third</text:p></text:list-item><text:list-item><text:p>next</text:p></text:list-item>",
    );
    assert_eq!(
        md_auto(&body, &auto),
        "1. first  \n   second  \n   third\n2. next"
    );
}

#[test]
fn an_item_that_begins_with_a_nested_list_still_takes_its_number() {
    let auto = nstyle("N", &[("1", "", "."), ("a", "", ".")]);
    let body = lst(
        "N",
        &(li("one")
            + &format!(
                "<text:list-item><text:list>{}</text:list></text:list-item>",
                li("deep")
            )
            + &li("three")),
    );
    // The nested level shows its own label, so it is text between the two real items.
    assert_eq!(
        md_auto(&body, &auto),
        "1. one\n\n\u{a0}\u{a0}a. deep\n\n3. three"
    );
}

#[test]
fn list_text_is_escaped_like_any_other_text() {
    let auto = nstyle("N", &[("1", "", ".")]);
    assert_eq!(
        md_auto(&lst("N", &li("a*b_c [d] | e")), &auto),
        "1. a\\*b_c \\[d] \\| e"
    );
}

#[test]
fn emphasis_inside_list_items() {
    let auto = nstyle("N", &[("1", "", ".")]) + &text_style("B", r#"fo:font-weight="bold""#, None);
    let body = lst("N", &li(&format!("{} tail", span("B", "head"))));
    assert_eq!(md_auto(&body, &auto), "1. **head** tail");
}

// ---------------------------------------------------------------------------------------------
// every other number format: the label the document shows
// ---------------------------------------------------------------------------------------------

/// The first three labels of a list in `format`.
fn labels(format: &str, prefix: &str, suffix: &str) -> Vec<String> {
    let auto = nstyle("N", &[(format, prefix, suffix)]);
    let body = lst("N", &(li("x") + &li("x") + &li("x")));
    md_auto(&body, &auto)
        .split("  \n")
        .map(|l| l.split(" x").next().unwrap_or("").to_string())
        .collect()
}

#[test]
fn number_formats() {
    let cases: &[(&str, [&str; 3])] = &[
        ("a", ["a.", "b.", "c."]),
        ("A", ["A.", "B.", "C."]),
        ("i", ["i.", "ii.", "iii."]),
        ("I", ["I.", "II.", "III."]),
        ("ア, イ, ウ, ...", ["ア.", "イ.", "ウ."]),
        ("イ, ロ, ハ, ...", ["イ.", "ロ.", "ハ."]),
        ("あ, い, う, ...", ["あ.", "い.", "う."]),
        ("い, ろ, は, ...", ["い.", "ろ.", "は."]),
        ("①, ②, ③, ...", ["①.", "②.", "③."]),
        ("一, 二, 三, ...", ["一.", "二.", "三."]),
        ("壱, 弐, 参, ...", ["壱.", "弐.", "参."]),
        ("１, ２, ３, ...", ["１.", "２.", "３."]),
        ("甲, 乙, 丙, ...", ["甲.", "乙.", "丙."]),
        ("子, 丑, 寅, ...", ["子.", "丑.", "寅."]),
        ("01", ["01\\.", "02\\.", "03\\."]),
        ("☆", ["1.", "2.", "3."]),
    ];
    for (fmt, want) in cases {
        let got = labels(fmt, "", ".");
        // The decimal fallback of an unknown format is a real list: its items are one per line.
        if *fmt == "☆" {
            let auto = nstyle("N", &[(fmt, "", ".")]);
            let m = md_auto(&lst("N", &(li("x") + &li("x") + &li("x"))), &auto);
            assert_eq!(m, "1. x\n2. x\n3. x", "{fmt}");
            continue;
        }
        assert_eq!(got, *want, "{fmt}");
    }
}

#[test]
fn prefix_and_suffix_are_part_of_the_label() {
    assert_eq!(labels("1", "第", "条"), ["第1条", "第2条", "第3条"]);
    assert_eq!(labels("a", "(", ")"), ["(a)", "(b)", "(c)"]);
    assert_eq!(labels("i", "", ")"), ["i)", "ii)", "iii)"]);
    // A decimal number that does not read `N.` is a label too (Markdown could not show it).
    assert_eq!(labels("1", "", ")"), ["1\\)", "2\\)", "3\\)"]);
    assert_eq!(labels("1", "[", "]"), ["\\[1]", "\\[2]", "\\[3]"]);
    // No suffix at all: the bare number.
    assert_eq!(labels("1", "", ""), ["1", "2", "3"]);
}

#[test]
fn letters_carry_or_repeat_by_num_letter_sync() {
    let n = 28;
    let items: String = (0..n).map(|_| li("x")).collect();
    let mk = |sync: &str| {
        format!(
            r#"<text:list-style style:name="L"><text:list-level-style-number text:level="1" style:num-format="a" style:num-suffix="." style:num-letter-sync="{sync}"/></text:list-style>"#
        )
    };
    let carry = md_auto(&lst("L", &items), &mk("false"));
    let lines: Vec<&str> = carry.split("  \n").collect();
    assert_eq!(lines[25], "z. x");
    assert_eq!(lines[26], "aa. x");
    assert_eq!(lines[27], "ab. x");
    let sync = md_auto(&lst("L", &items), &mk("true"));
    let lines: Vec<&str> = sync.split("  \n").collect();
    assert_eq!(lines[26], "aa. x");
    assert_eq!(lines[27], "bb. x");
}

#[test]
fn display_levels_shows_the_numbers_of_the_levels_above() {
    let auto = r#"<text:list-style style:name="N"><text:list-level-style-number text:level="1" style:num-format="1" style:num-suffix="."/><text:list-level-style-number text:level="2" style:num-format="1" text:display-levels="2"/><text:list-level-style-number text:level="3" style:num-format="a" text:display-levels="3" style:num-suffix=")"/></text:list-style>"#;
    let body = lst(
        "N",
        &format!(
            "<text:list-item><text:p>A</text:p><text:list>{}<text:list-item><text:p>B</text:p><text:list>{}{}</text:list></text:list-item></text:list></text:list-item>",
            li("A1"),
            li("B1"),
            li("B2")
        ),
    );
    let m = md_auto(&body, auto);
    assert_eq!(
        m,
        "1. A\n\n\u{a0}\u{a0}1.1 A1  \n\u{a0}\u{a0}1.2 B  \n\u{a0}\u{a0}\u{a0}\u{a0}1.2.a) B1  \n\u{a0}\u{a0}\u{a0}\u{a0}1.2.b) B2"
    );
}

#[test]
fn the_libreoffice_label_format_extension_is_used_when_present() {
    let auto = r#"<text:list-style style:name="N"><text:list-level-style-number text:level="1" style:num-format="1" loext:num-list-format="Art. %1%"/><text:list-level-style-number text:level="2" style:num-format="a" loext:num-list-format="%1%-%2%)" style:num-suffix="!"/></text:list-style>"#;
    let body = lst(
        "N",
        &format!(
            "<text:list-item><text:p>x</text:p><text:list>{}</text:list></text:list-item>{}",
            li("y"),
            li("z")
        ),
    );
    let m = md_auto(&body, auto);
    assert!(m.contains("Art. 1 x"), "{m}");
    assert!(m.contains("1-a) y"), "{m}");
    assert!(m.contains("Art. 2 z"), "{m}");
    // The same format that equals the plain `N.` stays a real list.
    let auto = r#"<text:list-style style:name="N"><text:list-level-style-number text:level="1" style:num-format="1" loext:num-list-format="%1%."/></text:list-style>"#;
    assert_eq!(
        md_auto(&lst("N", &(li("a") + &li("b"))), auto),
        "1. a\n2. b"
    );
    // A malformed placeholder is kept as text.
    let auto = r#"<text:list-style style:name="N"><text:list-level-style-number text:level="1" style:num-format="1" loext:num-list-format="%x% %99% 100%"/></text:list-style>"#;
    let m = md_auto(&lst("N", &li("a")), auto);
    assert!(m.contains("a"), "{m}");
}

#[test]
fn an_empty_format_shows_no_number_and_a_level_without_a_number_is_a_paragraph() {
    let auto = r#"<text:list-style style:name="N"><text:list-level-style-number text:level="1" style:num-format=""/></text:list-style>"#;
    assert_eq!(md_auto(&lst("N", &(li("a") + &li("b"))), auto), "a\n\nb");
}

#[test]
fn a_numbered_item_in_a_numbered_list_of_the_formats_above_nests_with_its_own_label() {
    let auto = nstyle("N", &[("I", "", "."), ("a", "(", ")"), ("1", "", ".")]);
    let body = lst(
        "N",
        &format!(
            "<text:list-item><text:p>top</text:p><text:list><text:list-item><text:p>mid</text:p><text:list>{}</text:list></text:list-item></text:list></text:list-item>",
            li("low")
        ),
    );
    let m = md_auto(&body, &auto);
    assert!(m.starts_with("I. top"), "{m}");
    assert!(m.contains("\u{a0}\u{a0}(a) mid"), "{m}");
    assert!(m.contains("1. low"), "{m}");
}

#[test]
fn a_heading_inside_a_list_item_takes_the_item_number() {
    let auto = nstyle("N", &[("1", "", ".")]);
    let body = lst(
        "N",
        r#"<text:list-item><text:h text:outline-level="2">Chapter</text:h></text:list-item><text:list-item><text:h text:outline-level="2">Next</text:h></text:list-item>"#,
    );
    assert_eq!(md_auto(&body, &auto), "## 1\\. Chapter\n\n## 2\\. Next");
}

#[test]
fn lists_inside_cells_and_notes_are_flat() {
    let auto = nstyle("N", &[("1", "", ".")]);
    let body = format!(
        r#"<text:p>x<text:note text:note-class="footnote"><text:note-body>{}</text:note-body></text:note></text:p>"#,
        lst("N", &(li("a") + &li("b")))
    );
    assert_eq!(md_auto(&body, &auto), "x[^1]\n\n[^1]: 1\\. a\\\n    2\\. b");
}

#[test]
fn a_very_long_list_is_cut_at_the_line_budget() {
    let auto = nstyle("N", &[("1", "", ".")]);
    let items: String = (0..6_000).map(|i| li(&format!("item {i}"))).collect();
    let d = conv(&Ox::new(&lst("N", &items)).auto(&auto));
    assert!(d.truncated);
    assert!(d.markdown.lines().count() <= 5_000);
}

#[test]
fn deeply_nested_lists_do_not_overflow_the_stack() {
    let auto = bstyle("B");
    let mut body = li("core");
    for _ in 0..120 {
        body = format!(
            "<text:list-item><text:p>x</text:p><text:list>{body}</text:list></text:list-item>"
        );
    }
    let d = conv(&Ox::new(&lst("B", &body)).auto(&auto));
    assert!(d.markdown.starts_with("- x"));
}

// ---------------------------------------------------------------------------------------------
// formulas
// ---------------------------------------------------------------------------------------------

const MML: &str = r#"<?xml version="1.0" encoding="UTF-8"?><math xmlns="http://www.w3.org/1998/Math/MathML" display="block"><semantics><mrow><mi>E</mi><mo stretchy="false">=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow><annotation encoding="StarMath 5.0">E = m c^2</annotation></semantics></math>"#;

fn formula_frame(href: &str) -> String {
    format!(
        r#"<draw:frame draw:name="Object1" text:anchor-type="as-char"><draw:object xlink:href="{href}" xlink:type="simple"/><draw:image xlink:href="./ObjectReplacements/Object 1"/></draw:frame>"#
    )
}

fn with_formula(body: &str) -> Ox {
    Ox::new(body).object("Object 1", MML)
}

#[test]
fn a_formula_in_a_sentence_is_inline_math() {
    let o = with_formula(&p(&format!(
        "Energy {} is conserved.",
        formula_frame("./Object 1")
    )));
    let d = conv(&o);
    assert_eq!(d.markdown, "Energy $E=m{c}^{2}$ is conserved.");
    assert_eq!((d.math_total, d.math_latex), (1, 1));
}

#[test]
fn a_paragraph_that_is_only_a_formula_is_display_math() {
    for body in [
        p(&formula_frame("./Object 1")),
        p(&format!("  {}  ", formula_frame("./Object 1"))),
        p(&format!(
            "<text:tab/>{}<text:s/>",
            formula_frame("./Object 1")
        )),
    ] {
        assert_eq!(
            conv(&with_formula(&body)).markdown,
            "$$\nE=m{c}^{2}\n$$",
            "{body}"
        );
    }
}

#[test]
fn two_formulas_in_a_paragraph_and_a_formula_with_a_number_stay_inline() {
    let f = formula_frame("./Object 1");
    assert_eq!(
        conv(&with_formula(&p(&format!("{f} and {f}")))).markdown,
        "$E=m{c}^{2}$ and $E=m{c}^{2}$"
    );
    assert_eq!(
        conv(&with_formula(&p(&format!("{f}<text:tab/>(1)")))).markdown,
        "$E=m{c}^{2}$\u{a0}\u{a0}\u{a0}\u{a0}(1)"
    );
    assert_eq!(
        conv(&with_formula(&p(&format!("{f}{f}")))).markdown,
        "$E=m{c}^{2}$$E=m{c}^{2}$"
    );
}

#[test]
fn a_formula_in_a_table_cell_a_heading_a_note_and_a_list() {
    let f = formula_frame("./Object 1");
    let body = format!(
        r#"<text:h text:outline-level="1">H {f}</text:h><table:table><table:table-row><table:table-cell><text:p>{f}</text:p></table:table-cell></table:table-row></table:table><text:p>n<text:note text:note-class="footnote"><text:note-body><text:p>{f}</text:p></text:note-body></text:note></text:p><text:list><text:list-item><text:p>{f}</text:p></text:list-item></text:list>"#
    );
    let m = conv(&with_formula(&body)).markdown;
    assert!(m.contains("# H $E=m{c}^{2}$"), "{m}");
    assert!(m.contains("| $E=m{c}^{2}$ |"), "{m}");
    assert!(m.contains("[^1]: $E=m{c}^{2}$"), "{m}");
    assert!(m.contains("- $E=m{c}^{2}$"), "{m}");
}

#[test]
fn the_object_path_may_be_written_in_the_usual_ways() {
    for href in ["./Object 1", "Object 1", "./Object 1/", "Object 1/"] {
        let m = conv(&with_formula(&p(&format!("a {} b", formula_frame(href))))).markdown;
        assert_eq!(m, "a $E=m{c}^{2}$ b", "{href}");
    }
}

#[test]
fn a_formula_that_cannot_be_converted_shows_its_starmath_source() {
    fn none(_: &str, _: bool) -> Option<String> {
        None
    }
    let opts = with_opts(|o| o.mathml = none);
    let o = with_formula(&p(&format!("x {} y", formula_frame("./Object 1"))));
    let d = conv_with(&o, &opts).unwrap();
    assert_eq!(d.markdown, "x E = m c\\^2 y");
    assert_eq!((d.math_total, d.math_latex), (1, 0));
    // A display formula that cannot be converted is a paragraph of its source.
    let o = with_formula(&p(&formula_frame("./Object 1")));
    let d = conv_with(&o, &opts).unwrap();
    assert_eq!(d.markdown, "E = m c\\^2");
}

#[test]
fn a_formula_whose_latex_would_break_the_markdown_is_shown_as_text() {
    fn dollar(_: &str, _: bool) -> Option<String> {
        Some("a$b".to_string())
    }
    let opts = with_opts(|o| o.mathml = dollar);
    let o = with_formula(&p(&format!("x {} y", formula_frame("./Object 1"))));
    let d = conv_with(&o, &opts).unwrap();
    assert!(!d.markdown.contains("$a"), "{}", d.markdown);
    assert_eq!(d.markdown, "x E = m c\\^2 y");
}

#[test]
fn an_object_that_is_not_a_formula_is_a_placeholder_or_its_picture() {
    let chart = r#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"><office:body><office:chart/></office:body></office:document-content>"#;
    let o = Ox::new(&p(r#"a <draw:frame><draw:object xlink:href="./Object 1"/><svg:title>Sales chart</svg:title></draw:frame> b"#))
    .object("Object 1", chart);
    assert_eq!(conv(&o).markdown, "a \\[Sales chart] b");
    // With a replacement picture it can show, that is shown.
    let o = Ox::new(&p(
        r#"<draw:frame><draw:object xlink:href="./Object 1"/><draw:image xlink:href="Pictures/r.png"/><svg:desc>chart picture</svg:desc></draw:frame>"#,
    ))
    .object("Object 1", chart)
    .picture("r.png", 3);
    let d = conv(&o);
    assert_eq!(d.images.len(), 1);
    assert!(
        d.markdown.starts_with("![chart picture]("),
        "{}",
        d.markdown
    );
    // A replacement that is not a picture konoma reads (LibreOffice's metafile) is a placeholder.
    let o = Ox::new(&p(&formula_frame("./Object 7"))).object("Object 7", chart);
    assert_eq!(conv(&o).markdown, "\\[image]");
}

#[test]
fn a_missing_object_or_an_odd_path_is_a_placeholder() {
    for href in [
        "./Object 9",
        "../Object 1",
        "/Object 1",
        "https://e.example/o",
        "",
    ] {
        let o = with_formula(&p(&format!("a {} b", formula_frame(href))));
        assert_eq!(conv(&o).markdown, "a \\[image] b", "{href:?}");
    }
    let o = Ox::new(&p(r#"<draw:frame><draw:object/></draw:frame>"#));
    assert_eq!(conv(&o).markdown, "\\[image]");
}

#[test]
fn a_formula_over_the_size_limit_and_the_object_budget() {
    let opts = with_opts(|o| o.max_math_xml = 100);
    let o = with_formula(&p(&format!("a {} b", formula_frame("./Object 1"))));
    let d = conv_with(&o, &opts).unwrap();
    assert_eq!(d.markdown, "a \\[formula] b");
    assert_eq!((d.math_total, d.math_latex), (1, 0));

    let opts = with_opts(|o| o.max_math_objects = 2);
    let body: String = (0..5)
        .map(|_| p(&format!("a {} b", formula_frame("./Object 1"))))
        .collect();
    let d = conv_with(&with_formula(&body), &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.math_total, 2);
    assert_eq!(d.markdown.matches("$E=m").count(), 2, "{}", d.markdown);
}

#[test]
fn the_formula_text_of_a_real_libreoffice_document_is_converted() {
    let Some(p) = super::tests_odt::lo_path("word.odt") else {
        return;
    };
    let d = load_document(&p, &DocOptions::default()).unwrap();
    assert!(d.markdown.contains("Inline formula: $E=m{c}^{2}$ and a fraction $\\frac{a+b}{c}=\\sqrt{{x}_{1}^{2}+{y}_{1}^{2}}$"));
}

// ---------------------------------------------------------------------------------------------
// the package
// ---------------------------------------------------------------------------------------------

#[test]
fn a_template_and_the_other_text_kinds_are_read() {
    for mime in [
        "application/vnd.oasis.opendocument.text",
        "application/vnd.oasis.opendocument.text-template",
        "application/vnd.oasis.opendocument.text-master",
        "application/vnd.oasis.opendocument.text-web",
    ] {
        let d = conv(&Ox::new(&p("hello")).mime(mime));
        assert_eq!(d.markdown, "hello", "{mime}");
    }
    // A trailing newline in `mimetype` is tolerated.
    let d = conv(&Ox::new(&p("hello")).mime("application/vnd.oasis.opendocument.text\n"));
    assert_eq!(d.markdown, "hello");
}

#[test]
fn another_opendocument_kind_is_not_a_text() {
    for mime in [
        "application/vnd.oasis.opendocument.spreadsheet",
        "application/vnd.oasis.opendocument.presentation",
        "application/vnd.oasis.opendocument.graphics",
        "application/vnd.oasis.opendocument.formula",
        "application/zip",
        "",
    ] {
        let r = conv_with(&Ox::new(&p("x")).mime(mime), &DocOptions::default());
        assert_eq!(r.unwrap_err(), OfficeError::Unsupported, "{mime:?}");
    }
}

#[test]
fn the_body_must_be_a_text() {
    let ods = r#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"><office:body><office:spreadsheet/></office:body></office:document-content>"#;
    let o = Ox::new("");
    let mut e = o.entries();
    for (n, b) in &mut e {
        if n == "content.xml" {
            *b = ods.as_bytes().to_vec();
        }
    }
    let r = load_from(&e);
    assert_eq!(r.unwrap_err(), OfficeError::Unsupported);

    let wrong_root = br#"<?xml version="1.0"?><html/>"#;
    let mut e = o.entries();
    for (n, b) in &mut e {
        if n == "content.xml" {
            *b = wrong_root.to_vec();
        }
    }
    assert_eq!(load_from(&e).unwrap_err(), OfficeError::Unsupported);
}

fn load_from(entries: &[(String, Vec<u8>)]) -> Result<Document, OfficeError> {
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let dir = tmp("odt_pkg");
    let path = write(&dir, "t.odt", &deflated(&refs));
    load_document(&path, &DocOptions::default())
}

#[test]
fn a_package_with_no_content_is_corrupt_and_an_empty_text_is_empty() {
    let o = Ox::new("");
    let e: Vec<_> = o
        .entries()
        .into_iter()
        .filter(|(n, _)| n != "content.xml")
        .collect();
    assert!(matches!(load_from(&e), Err(OfficeError::Corrupt(_))));
    assert_eq!(conv(&Ox::new("")).markdown, "");
    let mut e = o.entries();
    for (n, b) in &mut e {
        if n == "content.xml" {
            *b = br#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"><office:body><office:text/></office:body></office:document-content>"#.to_vec();
        }
    }
    assert_eq!(load_from(&e).unwrap().markdown, "");
}

#[test]
fn a_package_without_mimetype_is_not_an_opendocument() {
    let o = Ox::new(&p("x"));
    let e: Vec<_> = o
        .entries()
        .into_iter()
        .filter(|(n, _)| n != "mimetype")
        .collect();
    assert_eq!(load_from(&e).unwrap_err(), OfficeError::Unsupported);
}

#[test]
fn encrypted_documents_are_reported_as_encrypted() {
    let enc_manifest = r#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"><manifest:encryption-data manifest:checksum-type="SHA1/1K"><manifest:algorithm manifest:algorithm-name="Blowfish CFB"/></manifest:encryption-data></manifest:file-entry></manifest:manifest>"#;
    // Older packages: content.xml is there, its bytes are encrypted.
    let r = conv_with(
        &Ox::new(&p("x")).manifest(enc_manifest),
        &DocOptions::default(),
    );
    assert_eq!(r.unwrap_err(), OfficeError::Encrypted);
    // LibreOffice 25: one `encrypted-package` entry, mimetype in the clear.
    let o = Ox::new("");
    let mut e: Vec<_> = o
        .entries()
        .into_iter()
        .filter(|(n, _)| n != "content.xml")
        .collect();
    e.push(("encrypted-package".into(), vec![1, 2, 3, 4]));
    assert_eq!(load_from(&e).unwrap_err(), OfficeError::Encrypted);
    // A plain package is not.
    assert!(conv_with(&Ox::new(&p("x")), &DocOptions::default()).is_ok());
}

#[test]
fn a_damaged_styles_part_costs_the_styles_not_the_document() {
    let auto = text_style("B", r#"fo:font-weight="bold""#, None);
    let o = Ox::new(&p(&format!("a {} b", span("B", "bold"))))
        .auto(&auto)
        .styles("<<<broken");
    assert_eq!(conv(&o).markdown, "a **bold** b");
    // And no styles part at all.
    let o = Ox::new(&p("x"));
    assert_eq!(conv(&o).markdown, "x");
}

#[test]
fn not_a_zip_and_a_truncated_zip_are_corrupt() {
    let dir = tmp("odt_bad");
    let path = write(&dir, "t.odt", b"this is not a zip file");
    assert!(matches!(
        load_document(&path, &DocOptions::default()),
        Err(OfficeError::Corrupt(_))
    ));
    let good = Ox::new(&p("x")).bytes();
    let path = write(&dir, "u.odt", &good[..good.len() / 2]);
    assert!(load_document(&path, &DocOptions::default()).is_err());
    let path = write(&dir, "e.odt", b"");
    assert!(matches!(
        load_document(&path, &DocOptions::default()),
        Err(OfficeError::Corrupt(_))
    ));
    assert!(matches!(
        load_document(&dir.join("missing.odt"), &DocOptions::default()),
        Err(OfficeError::Io(_))
    ));
}

#[test]
fn a_docx_is_still_read_as_a_docx_and_neither_depends_on_the_extension() {
    // The extension is only a hint: an odt saved as `.docx` and the reverse both read.
    let dir = tmp("odt_ext");
    let path = write(&dir, "renamed.docx", &Ox::new(&p("from odt")).bytes());
    assert_eq!(
        load_document(&path, &DocOptions::default())
            .unwrap()
            .markdown,
        "from odt"
    );
    let path = write(
        &dir,
        "renamed.odt",
        &super::tests_docx::conv_bytes("<w:p><w:r><w:t>from docx</w:t></w:r></w:p>"),
    );
    assert_eq!(
        load_document(&path, &DocOptions::default())
            .unwrap()
            .markdown,
        "from docx"
    );
}

// ---------------------------------------------------------------------------------------------
// limits and hostile input
// ---------------------------------------------------------------------------------------------

#[test]
fn xml_nested_too_deep_is_refused() {
    let deep = format!(
        "{}x{}",
        "<text:span>".repeat(300),
        "</text:span>".repeat(300)
    );
    let r = conv_with(&Ox::new(&p(&deep)), &DocOptions::default());
    assert_eq!(r.unwrap_err(), OfficeError::TooLarge { what: "xml depth" });
    // Nesting within the limit reads (and the deep recursion is guarded).
    let ok = format!(
        "{}x{}",
        "<text:span>".repeat(150),
        "</text:span>".repeat(150)
    );
    assert_eq!(conv(&Ox::new(&p(&ok))).markdown, "x");
}

#[test]
fn a_package_over_the_size_limits_is_refused_before_it_is_read() {
    let opts = with_opts(|o| o.limits.max_part_bytes = 1_000);
    let big = p(&"word ".repeat(2_000));
    assert_eq!(
        conv_with(&Ox::new(&big), &opts).unwrap_err(),
        OfficeError::TooLarge { what: "entry" }
    );
    let opts = with_opts(|o| o.limits.max_entries = 2);
    assert_eq!(
        conv_with(&Ox::new(&p("x")), &opts).unwrap_err(),
        OfficeError::TooLarge { what: "entries" }
    );
    let opts = with_opts(|o| o.limits.max_file_bytes = 100);
    assert_eq!(
        conv_with(&Ox::new(&p("x")), &opts).unwrap_err(),
        OfficeError::TooLarge { what: "file" }
    );
}

#[test]
fn a_forged_zip_that_inflates_far_beyond_what_it_declares_is_refused() {
    // A deflate stream of zeros declared as 5 bytes: the inflated size is what counts.
    let opts = with_opts(|o| o.limits.max_part_bytes = 100_000);
    let zeros = vec![b' '; 5_000_000];
    let o = Ox::new(&p("x")).part("Pictures/bomb.bin", &zeros);
    assert_eq!(
        conv_with(&o, &opts).unwrap_err(),
        OfficeError::TooLarge { what: "entry" }
    );
}

#[test]
fn the_markdown_budget_cuts_the_body_and_says_so() {
    let opts = with_opts(|o| o.max_markdown_bytes = 2_000);
    let body: String = (0..500)
        .map(|i| p(&format!("paragraph number {i}")))
        .collect();
    let d = conv_with(&Ox::new(&body), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.len() <= 2_000);
    assert!(d.markdown.contains("paragraph number 0"));
    // The line budget.
    let opts = with_opts(|o| o.max_markdown_lines = 50);
    let d = conv_with(&Ox::new(&body), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.lines().count() <= 50);
}

#[test]
fn one_huge_paragraph_still_shows_its_beginning() {
    let opts = with_opts(|o| o.max_markdown_bytes = 5_000);
    let d = conv_with(&Ox::new(&p(&"word ".repeat(100_000))), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.starts_with("word word"));
    assert!(d.markdown.len() <= 5_000);
}

#[test]
fn the_block_budget_stops_a_body_of_empty_paragraphs() {
    let opts = with_opts(|o| o.max_blocks = 100);
    let body = "<text:p/>".repeat(1_000) + &p("late");
    let d = conv_with(&Ox::new(&body), &opts).unwrap();
    assert!(d.truncated);
    assert!(!d.markdown.contains("late"));
}

#[test]
fn a_block_over_the_node_or_text_budget_stops_the_conversion() {
    let opts = with_opts(|o| o.max_block_nodes = 50);
    let spans: String = (0..200).map(|i| span("X", &i.to_string())).collect();
    let body = p("first") + &p(&spans) + &p("never");
    let d = conv_with(&Ox::new(&body), &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.markdown, "first");
    let opts = with_opts(|o| o.max_block_text_bytes = 100);
    let d = conv_with(
        &Ox::new(&(p("first") + &p(&"x".repeat(1_000)) + &p("never"))),
        &opts,
    )
    .unwrap();
    assert!(d.truncated);
    assert_eq!(d.markdown, "first");
}

#[test]
fn cancelling_stops_the_conversion_early() {
    let cancel = Cancel::new(|| true);
    let body: String = (0..100).map(|i| p(&format!("p{i}"))).collect();
    let dir = tmp("odt_cancel");
    let path = write(&dir, "t.odt", &Ox::new(&body).bytes());
    let d = load_document_cancellable(&path, &DocOptions::default(), Some(&cancel)).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.len() < 20);
}

#[test]
fn a_hundred_thousand_paragraphs_are_cut_quickly() {
    let body: String = (0..100_000).map(|i| p(&format!("paragraph {i}"))).collect();
    let t = std::time::Instant::now();
    let d = conv(&Ox::new(&body));
    assert!(d.truncated);
    assert!(d.markdown.lines().count() <= 5_000);
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
}

#[test]
fn an_entity_bomb_is_not_expanded() {
    let content = r#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">]><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:text><text:p>&c;&c;&c;</text:p></office:text></office:body></office:document-content>"#;
    let o = Ox::new("");
    let mut e = o.entries();
    for (n, b) in &mut e {
        if n == "content.xml" {
            *b = content.as_bytes().to_vec();
        }
    }
    if let Ok(d) = load_from(&e) {
        assert!(d.markdown.len() < 1_000, "{}", d.markdown.len());
    }
}

// ---------------------------------------------------------------------------------------------
// mutated input never panics
// ---------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(src: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut v = src.to_vec();
    for _ in 0..=rng.below(4) {
        if v.is_empty() {
            break;
        }
        match rng.below(7) {
            0 => {
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
            1 => {
                let i = rng.below(v.len());
                let n = 1 + rng.below(30);
                v.drain(i..(i + n).min(v.len()));
            }
            2 => {
                let i = rng.below(v.len());
                let n = 1 + rng.below(60);
                let chunk: Vec<u8> = v[i..(i + n).min(v.len())].to_vec();
                let at = rng.below(v.len());
                for (k, b) in chunk.into_iter().enumerate() {
                    v.insert(at + k, b);
                }
            }
            3 => {
                let n = rng.below(v.len());
                v.truncate(n);
            }
            4 => {
                let tags: [&[u8]; 10] = [
                    b"<text:p>",
                    b"</text:p>",
                    b"<table:table>",
                    b"</table:table-cell>",
                    b"<text:list>",
                    b"</text:list-item>",
                    b"<text:span>",
                    b"<draw:frame>",
                    b"<text:note>",
                    b"<text:change-start text:change-id=\"c1\"/>",
                ];
                let at = rng.below(v.len());
                let t = tags[rng.below(tags.len())];
                for (k, b) in t.iter().enumerate() {
                    v.insert(at + k, *b);
                }
            }
            5 => {
                let i = rng.below(v.len());
                v[i] = [b'<', b'>', b'&', b'"', b'/', 0, 0xFF][rng.below(7)];
            }
            _ => {
                let at = rng.below(v.len());
                for k in 0..rng.below(5000) {
                    v.insert(at + k, b'9');
                }
            }
        }
    }
    v
}

/// A document that touches every reader path.
fn rich() -> Ox {
    let auto = nstyle("N", &[("1", "", "."), ("a", "(", ")")])
        + &bstyle("B")
        + &text_style(
            "T1",
            r#"fo:font-weight="bold" style:text-line-through-style="solid""#,
            None,
        )
        + &para_style("H", "", None, r#" style:default-outline-level="1""#)
        + &regions_xml();
    let f = formula_frame("./Object 1");
    let body = format!(
        r##"{}<text:h text:style-name="H" text:outline-level="1"><text:bookmark-start text:name="bm"/>Title *x*</text:h><text:p>a {} b<text:s text:c="3"/><text:tab/>c<text:line-break/>d<text:note text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>note</text:p></text:note-body></text:note></text:p>{}{}<table:table><table:table-row><table:table-cell table:number-columns-spanned="2"><text:p>a|b</text:p></table:table-cell><table:covered-table-cell/></table:table-row><table:table-row table:number-rows-repeated="3"><table:table-cell><table:table><table:table-row><table:table-cell><text:p>in</text:p></table:table-cell></table:table-row></table:table></table:table-cell><table:table-cell table:number-columns-repeated="2"/></table:table-row></table:table><text:p><text:a xlink:href="https://e.example/">link</text:a><text:a xlink:href="#bm">anchor</text:a><text:a xlink:href="#Title *x*|outline">x</text:a></text:p><text:p><draw:frame><draw:image xlink:href="Pictures/i.png"/><svg:desc>d</svg:desc></draw:frame>{f}<draw:frame><draw:text-box><text:p>tb</text:p></draw:text-box></draw:frame></text:p><text:section><text:p>sec</text:p></text:section><text:p>x<text:change-start text:change-id="d1"/>del<text:change-end text:change-id="d1"/><office:annotation><text:p>c</text:p></office:annotation></text:p><text:table-of-content><text:index-body><text:p>toc</text:p></text:index-body></text:table-of-content>"##,
        regions_body(),
        span("T1", "bold"),
        lst("N", &(li("one") + "<text:list-item><text:p>two</text:p><text:list><text:list-item><text:p>sub</text:p></text:list-item></text:list></text:list-item>")),
        lst("B", &li("bullet")),
    );
    Ox::new(&body)
        .auto(&auto)
        .styles(&para_style("Quote", "", None, ""))
        .picture("i.png", 1)
        .object("Object 1", MML)
}

fn regions_xml() -> String {
    String::new()
}

fn regions_body() -> String {
    r#"<text:tracked-changes><text:changed-region xml:id="d1" text:id="d1"><text:deletion><text:p>DELETED</text:p></text:deletion></text:changed-region></text:tracked-changes>"#.to_string()
}

#[test]
fn the_unmutated_rich_document_reads_fully() {
    let d = conv(&rich());
    for want in [
        "# Title \\*x\\*",
        "a ~~**bold**~~",
        "b   ",
        "[^1]",
        "1. one",
        "(a) sub",
        "- bullet",
        "[link](https://e.example/)",
        "[anchor](#title-x)",
        "![d](office-img://",
        "$E=m{c}^{2}$",
        "sec",
        "toc",
        "[^1]: note",
    ] {
        assert!(d.markdown.contains(want), "{want:?} in\n{}", d.markdown);
    }
    assert!(!d.markdown.contains("DELETED") && !d.markdown.contains("del\n"));
}

#[test]
fn mutated_documents_never_panic() {
    let base = rich();
    let content = base.content_xml().into_bytes();
    let styles = format!(
        r#"<?xml version="1.0"?><office:document-styles {}><office:styles>{}</office:styles></office:document-styles>"#,
        ns(),
        nstyle("S", &[("1", "", ".")])
    )
    .into_bytes();
    let dir = tmp("odt_fuzz");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut ok, mut err) = (0, 0);
    for i in 0..600 {
        let mut e = base.entries();
        for (n, b) in &mut e {
            match n.as_str() {
                "content.xml" => {
                    if i % 4 != 3 {
                        *b = mutate(&content, &mut rng);
                    }
                }
                "Object 1/content.xml" if i % 4 == 3 => *b = mutate(MML.as_bytes(), &mut rng),
                _ => {}
            }
        }
        if i % 7 == 0 {
            e.retain(|(n, _)| n != "styles.xml");
            e.push(("styles.xml".into(), mutate(&styles, &mut rng)));
        }
        let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        let path = write(&dir, "f.odt", &deflated(&refs));
        match load_document(&path, &DocOptions::default()) {
            Ok(_) => ok += 1,
            Err(OfficeError::Corrupt(m)) => {
                assert!(!m.starts_with("panic"), "a panic was caught: {m}");
                err += 1;
            }
            Err(_) => err += 1,
        }
    }
    assert!(
        ok > 50 && err > 50,
        "both outcomes should be exercised: ok {ok} err {err}"
    );
}

#[test]
fn an_arbitrary_list_of_attribute_values_never_panics() {
    // Numeric attributes with hostile values.
    let auto = r#"<text:list-style style:name="N"><text:list-level-style-number text:level="-1" style:num-format="1"/><text:list-level-style-number text:level="1" style:num-format="1" text:start-value="99999999999999999999" text:display-levels="99999"/><text:list-level-style-number text:level="2" style:num-format="a" text:start-value="-5"/></text:list-style>"#;
    let body = lst(
        "N",
        r#"<text:list-item text:start-value="-3"><text:p>a</text:p><text:list><text:list-item text:start-value="4294967296"><text:p>b</text:p></text:list-item></text:list></text:list-item><text:list-item text:start-value="x"><text:p>c</text:p></text:list-item>"#,
    ) + r#"<table:table><table:table-row table:number-rows-repeated="-1" table:visibility="x"><table:table-cell table:number-columns-repeated="999999999999999999999"/></table:table-row></table:table>"#
        + r#"<text:p>a<text:s text:c="-1"/>b<text:s text:c="x"/></text:p>"#;
    let _ = conv(&Ox::new(&body).auto(auto));
}

#[test]
fn a_picture_with_bytes_that_are_not_an_image_still_converts() {
    let o = Ox::new(&p(
        r#"<draw:frame><draw:image xlink:href="Pictures/x.png"/></draw:frame>"#,
    ))
    .part("Pictures/x.png", b"not really a png");
    let d = conv(&o);
    assert_eq!(d.images.len(), 1);
    let _ = tiny_png(0);
    let _ = visible("x");
}

#[test]
fn a_formula_in_a_heading_keeps_its_braces_and_a_literal_brace_is_still_escaped() {
    // odt
    let f = formula_frame("./Object 1");
    let body = format!(r#"<text:h text:outline-level="1">H {f} {{#id}}</text:h>"#);
    let m = conv(&with_formula(&body)).markdown;
    assert_eq!(m, "# H $E=m{c}^{2}$ \\{#id\\}");
    // docx: the same shared writer.
    use super::tests_docx::{md_styled, HEADING_STYLES};
    let body = r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t xml:space="preserve">H </w:t></w:r><m:oMath><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath><w:r><w:t xml:space="preserve"> {#id}</w:t></w:r></w:p>"#;
    let m = md_styled(body, HEADING_STYLES);
    assert_eq!(m, "# H ${x}^{2}$ \\{#id\\}");
}
