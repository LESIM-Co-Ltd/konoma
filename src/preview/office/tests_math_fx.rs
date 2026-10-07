//! Regression tests for the formula review of the Word / OpenDocument text readers: what the
//! converters (`omml.rs`, `mathml.rs`) and the places that write `$...$` (`docx.rs`, `odt.rs`) give
//! the Markdown renderer. Every LaTeX here must be drawn by RaTeX, and every `$...$` must be one
//! the renderer's own scanner (`collect_math_exprs`) lifts as a formula.

use super::mathml;
use super::omml::to_latex;
use super::tests_docx::{md, para, run};
use super::tests_odt::{conv as oconv, p as op, Ox};
use crate::preview::markdown::collect_math_exprs;
use crate::preview::math::latex_to_svg;

const NS: &str = "xmlns=\"http://www.w3.org/1998/Math/MathML\"";

fn drawn(l: &str) -> bool {
    latex_to_svg(l, true, "#000000").is_some()
}

fn r(t: &str) -> String {
    format!("<m:r><m:t xml:space=\"preserve\">{t}</m:t></m:r>")
}
fn rp(props: &str, t: &str) -> String {
    format!("<m:r><m:rPr>{props}</m:rPr><m:t xml:space=\"preserve\">{t}</m:t></m:r>")
}
fn om(inner: &str) -> String {
    format!("<m:oMath>{inner}</m:oMath>")
}

/// The LaTeX of an `oMath` body; it must be drawable (`to_latex` only answers drawable LaTeX).
#[track_caller]
fn tl(inner: &str) -> String {
    let l = to_latex(&om(inner), false).unwrap_or_else(|| panic!("no LaTeX for {inner}"));
    assert!(drawn(&l), "RaTeX cannot draw {l:?}");
    l
}

fn ml(body: &str) -> String {
    let x = format!("<math {NS}>{body}</math>");
    let l = mathml::to_latex(&x, false).unwrap_or_else(|| panic!("no LaTeX for {body}"));
    assert!(drawn(&l), "RaTeX cannot draw {l:?}");
    l
}

/// A fragment ready for `$...$` / a group: no white space at the ends and no lone `\` at the end
/// (which would escape the `}` or `$` after it).
#[track_caller]
fn assert_tidy(l: &str) {
    assert_eq!(l, l.trim(), "white space at an end of {l:?}");
    let slashes = l.bytes().rev().take_while(|&b| b == b'\\').count();
    assert_eq!(slashes % 2, 0, "{l:?} ends in an escaping backslash");
}

// ---------------------------------------------------------------------------------------------
// 1. trailing white space
// ---------------------------------------------------------------------------------------------

#[test]
fn trailing_white_space_does_not_break_the_formula() {
    let nbsp = "\u{00A0}";
    for sp in [" ", "  ", nbsp, "\u{2003}", " \u{00A0}"] {
        let cases: Vec<(&str, String)> = vec![
            // The whole expression ends in a space.
            ("whole", r(&format!("x{sp}"))),
            // A numerator / denominator / script / limit / radicand / delimiter body / matrix
            // cell / equation-array row / limit base ends in a space.
            (
                "num",
                format!(
                    "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
                    r(&format!("a{sp}")),
                    r("b")
                ),
            ),
            (
                "den",
                format!(
                    "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
                    r("a"),
                    r(&format!("b{sp}"))
                ),
            ),
            (
                "sup",
                format!(
                    "<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>",
                    r("x"),
                    r(&format!("n{sp}"))
                ),
            ),
            (
                "sub",
                format!(
                    "<m:sSub><m:e>{}</m:e><m:sub>{}</m:sub></m:sSub>",
                    r("x"),
                    r(&format!("n{sp}"))
                ),
            ),
            (
                "nary",
                format!(
                    "<m:nary><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:nary>",
                    r("0"),
                    r("1"),
                    r(&format!("x dx{sp}"))
                ),
            ),
            (
                "rad",
                format!("<m:rad><m:e>{}</m:e></m:rad>", r(&format!("x{sp}"))),
            ),
            (
                "delim",
                format!("<m:d><m:e>{}</m:e></m:d>", r(&format!("x{sp}"))),
            ),
            (
                "matrix",
                format!(
                    "<m:m><m:mr><m:e>{}</m:e><m:e>{}</m:e></m:mr></m:m>",
                    r(&format!("a{sp}")),
                    r("b")
                ),
            ),
            (
                "eqArr",
                format!("<m:eqArr><m:e>{}</m:e></m:eqArr>", r(&format!("a=b{sp}"))),
            ),
            (
                "limLow",
                format!(
                    "<m:limLow><m:e>{}</m:e><m:lim>{}</m:lim></m:limLow>",
                    r(&format!("x{sp}")),
                    r("y")
                ),
            ),
        ];
        for (name, inner) in cases {
            let l = tl(&inner);
            assert_tidy(&l);
            // The structure survives (the whole expression did not fall back to its characters).
            match name {
                "num" | "den" => assert!(l.contains(r"\frac"), "{name}: {l}"),
                "nary" => assert!(l.contains(r"\int"), "{name}: {l}"),
                "rad" => assert!(l.contains(r"\sqrt"), "{name}: {l}"),
                "delim" => assert!(l.contains(r"\left"), "{name}: {l}"),
                "matrix" => assert!(l.contains("matrix"), "{name}: {l}"),
                "eqArr" => assert!(l.contains("aligned"), "{name}: {l}"),
                _ => {}
            }
        }
    }
}

#[test]
fn a_trailing_space_is_kept_as_a_space_not_dropped_to_a_lone_backslash() {
    // `x` + space: the space is `\ `, and what follows it is an empty group.
    let l = tl(&r("x "));
    assert!(l.starts_with('x') && l.contains(r"\ "), "{l}");
}

#[test]
fn trailing_white_space_in_mathml_does_not_break_the_formula() {
    let l = ml("<mfrac><mrow><mi>a</mi><mtext> </mtext></mrow><mi>b</mi></mfrac>");
    assert!(l.contains(r"\frac"), "{l}");
    assert_tidy(&l);
    let l = ml("<mi>x</mi><mtext> </mtext>");
    assert_tidy(&l);
    let l = ml("<msqrt><mi>x</mi><mtext>\u{00A0}</mtext></msqrt>");
    assert!(l.contains(r"\sqrt"), "{l}");
    let l = ml("<mfenced><mi>x</mi><mtext> </mtext></mfenced>");
    assert!(l.contains(r"\left"), "{l}");
    let l = ml("<mtable><mtr><mtd><mi>a</mi><mtext> </mtext></mtd></mtr></mtable>");
    assert!(l.contains("matrix"), "{l}");
    let l = ml("<menclose notation=\"box\"><mi>x</mi><mtext> </mtext></menclose>");
    assert!(l.contains(r"\boxed"), "{l}");
}

#[test]
fn a_formula_ending_in_a_space_is_a_formula_in_a_document() {
    let m = md(&para(&format!(
        "{}{}",
        run("see "),
        om(&format!(
            "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
            r("a "),
            r("b")
        ))
    )));
    let found = collect_math_exprs(&m);
    assert_eq!(found.len(), 1, "{m}");
    assert!(found[0].0.contains(r"\frac"), "{m}");
}

// ---------------------------------------------------------------------------------------------
// 2. borderBox
// ---------------------------------------------------------------------------------------------

fn bb(pr: &str) -> String {
    format!(
        "<m:borderBox><m:borderBoxPr>{pr}</m:borderBoxPr><m:e>{}</m:e></m:borderBox>",
        r("x")
    )
}
fn on(name: &str) -> String {
    format!("<m:{name} m:val=\"1\"/>")
}
const HIDE_ALL: [&str; 4] = ["hideTop", "hideBot", "hideLeft", "hideRight"];
fn hide_all() -> String {
    HIDE_ALL.iter().map(|h| on(h)).collect()
}

#[test]
fn a_border_box_is_a_box_unless_its_sides_are_hidden() {
    assert_eq!(tl(&bb("")), r"\boxed{x}");
    // No side drawn: the content alone (or its strike lines).
    assert_eq!(tl(&bb(&hide_all())), "x");
    assert_eq!(tl(&bb(&(hide_all() + &on("strikeBLTR")))), r"\cancel{x}");
    assert_eq!(tl(&bb(&(hide_all() + &on("strikeTLBR")))), r"\bcancel{x}");
    assert_eq!(
        tl(&bb(&(hide_all() + &on("strikeBLTR") + &on("strikeTLBR")))),
        r"\xcancel{x}"
    );
    assert_eq!(tl(&bb(&(hide_all() + &on("strikeH")))), r"\sout{x}");
    // A strike inside a box that still has its sides.
    assert_eq!(tl(&bb(&on("strikeBLTR"))), r"\boxed{\cancel{x}}");
    // Some sides.
    assert_eq!(
        tl(&bb(&(on("hideBot") + &on("hideLeft") + &on("hideRight")))),
        r"\overline{x}"
    );
    assert_eq!(
        tl(&bb(&(on("hideTop") + &on("hideLeft") + &on("hideRight")))),
        r"\underline{x}"
    );
    assert_eq!(
        tl(&bb(&(on("hideTop") + &on("hideBot")))),
        r"\left\vert x \right\vert"
    );
    assert_eq!(
        tl(&bb(&(on("hideTop") + &on("hideBot") + &on("hideRight")))),
        r"\left\vert x \right."
    );
    // `m:val="0"` is off.
    assert_eq!(
        tl(&bb(r#"<m:hideTop m:val="0"/><m:hideBot m:val="false"/>"#)),
        r"\boxed{x}"
    );
}

#[test]
fn a_menclose_is_drawn_like_the_same_border_box() {
    let enc = |n: &str| ml(&format!("<menclose notation=\"{n}\"><mi>x</mi></menclose>"));
    assert_eq!(enc("box"), r"\boxed{x}");
    assert_eq!(enc("updiagonalstrike"), r"\cancel{x}");
    assert_eq!(enc("downdiagonalstrike"), r"\bcancel{x}");
    assert_eq!(enc("updiagonalstrike downdiagonalstrike"), r"\xcancel{x}");
    assert_eq!(enc("horizontalstrike"), r"\sout{x}");
    assert_eq!(enc("box updiagonalstrike"), r"\boxed{\cancel{x}}");
    assert_eq!(enc("top"), r"\overline{x}");
    assert_eq!(enc("bottom"), r"\underline{x}");
    assert_eq!(enc("top bottom"), r"\underline{\overline{x}}");
    assert_eq!(enc("left right"), r"\left\vert x \right\vert");
    assert_eq!(enc("left"), r"\left\vert x \right.");
    assert_eq!(enc("longdiv"), "x");
    assert_eq!(enc("radical"), r"\sqrt{x}");
}

// ---------------------------------------------------------------------------------------------
// 3. the characters shown for a formula that cannot be drawn
// ---------------------------------------------------------------------------------------------

fn none_latex(_: &str, _: bool) -> Option<String> {
    None
}

/// What a docx shows for the formula `inner` when it cannot be converted to LaTeX.
fn shown(inner: &str) -> String {
    use super::docx::DocOptions;
    use super::tests_docx::{conv_with, Dx};
    let opts = DocOptions {
        math: none_latex,
        ..DocOptions::default()
    };
    conv_with(&Dx::new(&para(&om(inner))), &opts)
        .unwrap()
        .markdown
        .replace('\\', "")
}

#[test]
fn an_undrawn_formula_keeps_its_structure_as_text() {
    let x = r("x");
    // A fraction keeps its bar, a script its mark.
    assert_eq!(
        shown(&format!(
            "<m:f><m:num>{}</m:num><m:den><m:sSup><m:e>{x}</m:e><m:sup>{}</m:sup></m:sSup></m:den></m:f>",
            r("1"),
            r("2")
        )),
        "(1)/(x^2)"
    );
    // The operator character and its limits.
    assert_eq!(
        shown(&format!(
            "<m:nary><m:naryPr><m:chr m:val=\"\u{2211}\"/></m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:nary>",
            r("i=1"),
            r("n"),
            r("a")
        )),
        "\u{2211}_(i=1)^n a"
    );
    // A root.
    assert_eq!(
        shown(&format!(
            "<m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/><m:e>{x}</m:e></m:rad>"
        )),
        "\u{221A}(x)"
    );
    // A matrix.
    assert_eq!(
        shown(&format!(
            "<m:m><m:mr><m:e>{}</m:e><m:e>{}</m:e></m:mr><m:mr><m:e>{}</m:e><m:e>{}</m:e></m:mr></m:m>",
            r("a"),
            r("b"),
            r("c"),
            r("d")
        )),
        "[a, b; c, d]"
    );
    // A subscript of several characters and the delimiters.
    assert_eq!(
        shown(&format!(
            "<m:d><m:e><m:sSub><m:e>{x}</m:e><m:sub>{}</m:sub></m:sSub></m:e></m:d>",
            r("i+1")
        )),
        "(x_(i+1))"
    );
}

#[test]
fn an_undrawn_mathml_formula_keeps_its_structure_as_text() {
    let t = |body: &str| mathml::fallback_text(&format!("<math {NS}>{body}</math>"));
    assert_eq!(
        t("<mfrac><mn>1</mn><msup><mi>x</mi><mn>2</mn></msup></mfrac>"),
        "(1)/(x^2)"
    );
    assert_eq!(
        t("<munderover><mo>\u{2211}</mo><mrow><mi>i</mi><mo>=</mo><mn>1</mn></mrow><mi>n</mi></munderover>"),
        "\u{2211}_(i = 1)^n"
    );
    assert_eq!(t("<msqrt><mi>x</mi></msqrt>"), "\u{221A}(x)");
    assert_eq!(
        t("<mtable><mtr><mtd><mi>a</mi></mtd><mtd><mi>b</mi></mtd></mtr><mtr><mtd><mi>c</mi></mtd><mtd><mi>d</mi></mtd></mtr></mtable>"),
        "[a, b; c, d]"
    );
    // The StarMath source is still preferred when there is one.
    assert_eq!(
        t("<semantics><mi>x</mi><annotation encoding=\"StarMath 5.0\">{a} over {b}</annotation></semantics>"),
        "{a} over {b}"
    );
}

// ---------------------------------------------------------------------------------------------
// 4. an empty `m:chr`
// ---------------------------------------------------------------------------------------------

#[test]
fn an_empty_chr_is_no_character_not_the_default() {
    let nary = |chr: &str| {
        format!(
            "<m:nary><m:naryPr>{chr}</m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:nary>",
            r("a"),
            r("b"),
            r("x")
        )
    };
    // Absent: the integral. Present and empty: no operator.
    assert!(tl(&nary("")).starts_with(r"\int"));
    let l = tl(&nary(r#"<m:chr m:val=""/>"#));
    assert!(!l.contains(r"\int"), "{l}");
    assert!(l.contains("_{a}") && l.contains("^{b}"), "{l}");
    // An accent and a group character with no character are the content alone.
    assert_eq!(
        tl(&format!(
            r#"<m:acc><m:accPr><m:chr m:val=""/></m:accPr><m:e>{}</m:e></m:acc>"#,
            r("x")
        )),
        "{x}"
    );
    assert_eq!(
        tl(&format!(
            r#"<m:groupChr><m:groupChrPr><m:chr m:val=""/></m:groupChrPr><m:e>{}</m:e></m:groupChr>"#,
            r("x")
        )),
        "{x}"
    );
    // Absent chr keeps the defaults.
    assert_eq!(
        tl(&format!("<m:acc><m:e>{}</m:e></m:acc>", r("x"))),
        r"\hat{x}"
    );
}

// ---------------------------------------------------------------------------------------------
// 5. a prime in a superscript
// ---------------------------------------------------------------------------------------------

#[test]
fn a_prime_in_a_superscript_is_not_raised_twice() {
    for prime in ["\u{2032}", "'"] {
        let l = tl(&format!(
            "<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>",
            r("f"),
            r(prime)
        ));
        assert_eq!(l, r"{f}^{\prime}");
    }
    assert_eq!(
        tl(&format!(
            "<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>",
            r("f"),
            r("\u{2033}")
        )),
        r"{f}^{\prime\prime}"
    );
    // With more in the superscript, and in a sub-superscript.
    assert_eq!(
        tl(&format!(
            "<m:sSubSup><m:e>{}</m:e><m:sub>{}</m:sub><m:sup>{}</m:sup></m:sSubSup>",
            r("f"),
            r("1"),
            r("\u{2032}2")
        )),
        r"{f}_{1}^{\prime 2}"
    );
    // A prime that is a plain character after a letter stays `'`.
    assert_eq!(tl(&r("f\u{2032}")), "f'");
    // MathML: `msup` with a prime operator.
    assert_eq!(
        ml("<msup><mi>f</mi><mo>\u{2032}</mo></msup>"),
        r"{f}^{\prime}"
    );
    assert_eq!(
        ml("<msubsup><mi>f</mi><mn>1</mn><mo>\u{2032}</mo></msubsup>"),
        r"{f}_{1}^{\prime}"
    );
}

// ---------------------------------------------------------------------------------------------
// 6. a `$` that is a formula to the renderer
// ---------------------------------------------------------------------------------------------

#[test]
fn a_digit_after_a_formula_does_not_make_it_a_price() {
    let f = om(&r("x"));
    for after in ["5", "5 apples", "0.5", "99"] {
        let m = md(&para(&format!("{}{}{}", run("a "), f, run(after))));
        let found = collect_math_exprs(&m);
        assert_eq!(found.len(), 1, "{m:?}");
        assert_eq!(found[0], ("x".to_string(), false), "{m:?}");
    }
    // Two formulas in one paragraph: both are lifted (one that is read as a price stops the other).
    let m = md(&para(&format!(
        "{}{}{}{}",
        om(&r("x")),
        run("5 and "),
        om(&r("y")),
        run("7")
    )));
    let found: Vec<String> = collect_math_exprs(&m).into_iter().map(|f| f.0).collect();
    assert_eq!(found, ["x", "y"], "{m:?}");
    // The text itself is unchanged: a zero-width space is the only addition.
    assert_eq!(m.replace('\u{200B}', ""), "$x$5 and $y$7");
    // A formula right before a digit-led text with a style mark has no digit after the `$`.
    let m = md(&para(&format!(
        "{}{}",
        om(&r("x")),
        "<w:r><w:rPr><w:b/></w:rPr><w:t>5</w:t></w:r>"
    )));
    assert_eq!(collect_math_exprs(&m).len(), 1, "{m:?}");
    // No guard where none is needed.
    assert_eq!(md(&para(&format!("{}{}", om(&r("x")), run(" 5")))), "$x$ 5");
}

#[test]
fn a_digit_after_an_odt_formula_does_not_make_it_a_price() {
    let mml = format!("<math {NS}><mi>x</mi></math>");
    let frame = r#"<draw:frame draw:name="Object1" text:anchor-type="as-char"><draw:object xlink:href="./Object 1" xlink:type="simple"/><draw:image xlink:href="./ObjectReplacements/Object 1"/></draw:frame>"#;
    let o = Ox::new(&op(&format!("{frame}5 and {frame}7"))).object("Object 1", &mml);
    let m = oconv(&o).markdown;
    let found = collect_math_exprs(&m);
    assert_eq!(found.len(), 2, "{m:?}");
}

#[test]
fn a_formula_never_ends_in_a_lone_backslash_in_a_document() {
    // The whole expression ends in a space: `$...\ $` would be `\$`, an escaped dollar.
    let m = md(&para(&format!("{}{}", om(&r("x ")), run(" z"))));
    let found = collect_math_exprs(&m);
    assert_eq!(found.len(), 1, "{m:?}");
    assert_tidy(&found[0].0);
}

// ---------------------------------------------------------------------------------------------
// 7. the first character of a display formula
// ---------------------------------------------------------------------------------------------

#[test]
fn a_display_formula_cannot_be_turned_into_another_block() {
    for first in [
        ">", "-", "--", "---", "+", "=", "<", ":", "[", "!", "1.", "12)",
    ] {
        let m = md(&para(&format!(
            "<m:oMathPara>{}</m:oMathPara>",
            om(&r(&format!("{first} x")
                .replace('<', "&lt;")
                .replace('>', "&gt;")))
        )));
        let found = collect_math_exprs(&m);
        assert_eq!(found.len(), 1, "first `{first}`: {m:?}");
        assert!(found[0].1, "display: {m:?}");
        assert!(drawn(&found[0].0), "{:?}", found[0].0);
    }
    // Nothing is added to a formula that does not need it.
    let m = md(&para(&format!(
        "<m:oMathPara>{}</m:oMathPara>",
        om(&r("a-b"))
    )));
    assert_eq!(m, "$$\na-b\n$$");
    let m = md(&para(&format!(
        "<m:oMathPara>{}</m:oMathPara>",
        om(&r("1.5x"))
    )));
    assert_eq!(m, "$$\n1.5x\n$$");
}

// ---------------------------------------------------------------------------------------------
// 8. an escaped `$`
// ---------------------------------------------------------------------------------------------

#[test]
fn a_dollar_sign_in_a_formula_is_not_a_reason_to_show_text() {
    let m = md(&para(&format!(
        "{}{}{}",
        run("cost "),
        om(&r("$5")),
        run(" ok")
    )));
    assert_eq!(m, "cost $\\$5$ ok");
    let found = collect_math_exprs(&m);
    assert_eq!(found, [("\\$5".to_string(), false)], "{m:?}");
    // In a display formula too.
    let m = md(&para(&format!(
        "<m:oMathPara>{}</m:oMathPara>",
        om(&r("a$b"))
    )));
    assert_eq!(
        collect_math_exprs(&m),
        [("a\\$b".to_string(), true)],
        "{m:?}"
    );
    // And in OpenDocument.
    let mml = format!("<math {NS}><mi>$</mi><mn>5</mn></math>");
    let frame = r#"<draw:frame draw:name="Object1" text:anchor-type="as-char"><draw:object xlink:href="./Object 1" xlink:type="simple"/><draw:image xlink:href="./ObjectReplacements/Object 1"/></draw:frame>"#;
    let m = oconv(&Ox::new(&op(&format!("a {frame} b"))).object("Object 1", &mml)).markdown;
    assert_eq!(collect_math_exprs(&m).len(), 1, "{m:?}");
}

#[test]
fn only_an_unescaped_dollar_is_a_bare_dollar() {
    use super::omml::has_bare_dollar;
    assert!(!has_bare_dollar(r"a"));
    assert!(!has_bare_dollar(r"\$"));
    assert!(!has_bare_dollar(r"\$ \$"));
    assert!(has_bare_dollar("$"));
    assert!(has_bare_dollar(r"\\$"));
    assert!(!has_bare_dollar(r"\\\$"));
    assert!(has_bare_dollar(r"\$$"));
    assert!(has_bare_dollar(r"a $ b"));
    // A trailing lone backslash does not panic.
    assert!(!has_bare_dollar("a\\"));
}

// ---------------------------------------------------------------------------------------------
// 9. `^` and `~`
// ---------------------------------------------------------------------------------------------

#[test]
fn a_caret_and_a_tilde_are_themselves() {
    // In text (m:nor) they were spaces.
    assert_eq!(tl(&rp("<m:nor/>", "a^b~c")), r"\text{a\^{}b\~{}c}");
    // In a math run they were `\wedge` and `\sim`.
    assert_eq!(tl(&r("a^b")), r"a\^{}b");
    assert_eq!(tl(&r("a~b")), r"a\~{}b");
    // The real operators are still the operators.
    assert_eq!(tl(&r("a\u{2227}b")), r"a\wedge b");
    assert_eq!(tl(&r("a\u{223C}b")), r"a\sim b");
    assert_eq!(ml("<mi>a</mi><mo>^</mo><mi>b</mi>"), r"a\^{}b");
}

// ---------------------------------------------------------------------------------------------
// 10. bold
// ---------------------------------------------------------------------------------------------

#[test]
fn bold_reaches_greek_letters_digits_and_alphabets() {
    let b = |t: &str| tl(&rp(r#"<m:sty m:val="b"/>"#, t));
    let bi = |t: &str| tl(&rp(r#"<m:sty m:val="bi"/>"#, t));
    assert_eq!(b("v"), r"\mathbf{v}");
    // `\mathbf` leaves a lower-case Greek letter light.
    assert_eq!(b("\u{03B1}"), r"\boldsymbol{\alpha }");
    assert_eq!(b("v\u{03B1}2"), r"\mathbf{v}\boldsymbol{\alpha }\mathbf{2}");
    // Digits are bold too.
    assert_eq!(b("12"), r"\mathbf{12}");
    assert_eq!(b("3,14"), r"\mathbf{3{,}14}");
    assert_eq!(bi("v"), r"\boldsymbol{v}");
    assert_eq!(bi("12"), r"\boldsymbol{12}");
    assert_eq!(bi("2\u{03B1}"), r"\boldsymbol{2\alpha }");
    // A bold alphabet.
    let scr = |s: &str, sty: &str| {
        tl(&rp(
            &format!(r#"<m:scr m:val="{s}"/><m:sty m:val="{sty}"/>"#),
            "A",
        ))
    };
    assert_eq!(scr("script", "p"), r"\mathcal{A}");
    assert_eq!(scr("script", "b"), r"\boldsymbol{\mathcal{A}}");
    assert_eq!(scr("fraktur", "b"), r"\boldsymbol{\mathfrak{A}}");
    assert_eq!(scr("double-struck", "bi"), r"\boldsymbol{\mathbb{A}}");
    // Plain and italic are unchanged.
    assert_eq!(tl(&rp(r#"<m:sty m:val="p"/>"#, "ab")), r"\mathrm{ab}");
    assert_eq!(tl(&r("ab")), "ab");
}

// ---------------------------------------------------------------------------------------------
// 11. the base of a limit
// ---------------------------------------------------------------------------------------------

#[test]
fn the_base_of_a_limit_is_a_function_name_only_when_it_is_one() {
    let lim = |kind: &str, base: &str| {
        tl(&format!(
            "<m:{kind}><m:e>{base}</m:e><m:lim>{}</m:lim></m:{kind}>",
            r("y")
        ))
    };
    // Two variables stay two italic letters (they were an upright word).
    assert_eq!(lim("limLow", &r("ab")), r"\underset{y}{ab}");
    assert_eq!(lim("limUpp", &r("ab")), r"\overset{y}{ab}");
    // A function name is still one.
    assert_eq!(lim("limLow", &r("lim")), r"\lim_{y}");
    assert_eq!(lim("limLow", &r("max")), r"\max_{y}");
    // Nested elements in the base do not turn their letters into function names.
    let nested = lim(
        "limLow",
        &format!(
            "<m:sSub><m:e>{}</m:e><m:sub>{}</m:sub></m:sSub><m:bar><m:e>{}</m:e></m:bar>",
            r("x"),
            r("ab"),
            r("cd")
        ),
    );
    assert!(!nested.contains("operatorname"), "{nested}");
    // In an `m:func` name the base is a function name (unchanged).
    let f = tl(&format!(
        "<m:func><m:fName><m:limLow><m:e>{}</m:e><m:lim>{}</m:lim></m:limLow></m:fName><m:e>{}</m:e></m:func>",
        r("max"),
        r("x"),
        r("f")
    ));
    assert!(f.contains(r"\max"), "{f}");
}

// ---------------------------------------------------------------------------------------------
// 12. decimal commas
// ---------------------------------------------------------------------------------------------

#[test]
fn a_decimal_comma_takes_no_space() {
    assert_eq!(tl(&r("3,14")), "3{,}14");
    assert_eq!(tl(&r("1,000,000")), "1{,}000{,}000");
    // A comma between letters, after a digit only, or before one only is a separator.
    assert_eq!(tl(&r("a,b")), "a,b");
    assert_eq!(tl(&r("1,a")), "1,a");
    assert_eq!(tl(&r("a,1")), "a,1");
    assert_eq!(tl(&r("1, 2")), r"1,\ 2");
    assert_eq!(ml("<mn>3,14</mn>"), "3{,}14");
    assert_eq!(ml("<mi>a</mi><mo>,</mo><mi>b</mi>"), "a,b");
}

// ---------------------------------------------------------------------------------------------
// a backtick is no markup inside a formula
// ---------------------------------------------------------------------------------------------

#[test]
fn a_backtick_in_a_formula_is_not_markdown_code() {
    let l = tl(&r("a`b"));
    assert!(!l.contains('`'), "{l}");
    assert!(drawn(&l));
}
