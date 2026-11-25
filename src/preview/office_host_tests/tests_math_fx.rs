//! Moved from `konoma-office-core/src/tests_math_fx.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::{md, para, run};
use super::tests_odt::{conv as oconv, p as op, Ox};
use crate::preview::markdown::collect_math_exprs;
pub use konoma_office_core::tests_math_fx::*;

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
