//! Tests of the MathML -> LaTeX converter (`mathml.rs`): every element's output, that RaTeX draws
//! every output, and that no input makes it panic.

use super::mathml::{fallback_text, is_math, to_latex};
use crate::preview::math::latex_to_svg;

const NS: &str = "xmlns=\"http://www.w3.org/1998/Math/MathML\"";

fn wrap(body: &str) -> String {
    format!("<math {NS}>{body}</math>")
}

fn drawable(l: &str) -> bool {
    latex_to_svg(l, true, "#000000").is_some()
}

/// The LaTeX of a body, which RaTeX must be able to draw.
#[track_caller]
fn tex(body: &str) -> String {
    let x = wrap(body);
    let l = to_latex(&x, false).unwrap_or_else(|| panic!("no LaTeX for {x}"));
    assert!(drawable(&l), "RaTeX cannot draw {l:?} (from {body})");
    l
}

#[test]
fn ratex_rejects_what_it_cannot_draw() {
    // The `drawable` gate is only worth something if RaTeX does say no.
    assert!(!drawable(r"\frac{1"));
    assert!(!drawable(r"\begin{matrix} a"));
}

#[test]
fn tokens() {
    assert_eq!(tex("<mi>x</mi>"), "x");
    assert_eq!(tex("<mn>3.14</mn>"), "3.14");
    assert_eq!(
        tex("<mi>\u{03B1}</mi><mo>+</mo><mi>\u{0393}</mi>"),
        "\\alpha +\\Gamma"
    );
    assert_eq!(tex("<mo>\u{00B1}</mo>"), "\\pm");
    assert_eq!(tex("<mo>&lt;</mo>"), "<");
    assert_eq!(tex("<mo>&amp;</mo>"), "\\&");
    assert_eq!(tex("<mo>\u{2264}</mo><mo>\u{2260}</mo>"), "\\leq \\neq");
    assert_eq!(tex("<mi>\u{221E}</mi>"), "\\infty");
    // Invisible operators draw nothing.
    assert_eq!(
        tex("<mi>f</mi><mo>\u{2061}</mo><mo>(</mo><mi>x</mi><mo>)</mo>"),
        "f(x)"
    );
    assert_eq!(tex("<mi>a</mi><mo>\u{2062}</mo><mi>b</mi>"), "ab");
    // Entities and character references in tokens.
    assert_eq!(tex("<mi>&#x3C0;</mi>"), "\\pi");
    assert_eq!(tex("<mi>x</mi>\n  <mo>=</mo>\n  <mn>1</mn>"), "x=1");
}

#[test]
fn identifiers_that_are_names() {
    assert_eq!(tex("<mi>sin</mi><mi>x</mi>"), "\\sin x");
    assert_eq!(
        tex("<mi>log</mi><mo>(</mo><mi>x</mi><mo>)</mo>"),
        "\\log (x)"
    );
    assert_eq!(tex("<mi>foo</mi>"), "\\operatorname{foo}");
    // Not a function name and not only ASCII letters: characters.
    assert_eq!(tex("<mi>\u{03B1}\u{03B2}</mi>"), "\\alpha \\beta");
}

#[test]
fn text_and_strings() {
    assert_eq!(tex("<mtext>if </mtext><mi>x</mi>"), "\\text{if }x");
    assert_eq!(tex("<mtext>\u{00A0}and\u{00A0}</mtext>"), "\\text{ and }");
    assert_eq!(tex("<mtext>a   b</mtext>"), "\\text{a b}");
    assert_eq!(tex("<mtext>100%</mtext>"), "\\text{100\\%}");
    assert_eq!(tex("<mtext>a_b {c}</mtext>"), "\\text{a\\_b \\{c\\}}");
    assert_eq!(tex("<mtext> </mtext><mi>x</mi>"), "\\ x");
    assert_eq!(tex("<ms>s</ms>"), "\\text{\"s\"}");
    assert_eq!(tex("<ms lquote=\"'\" rquote=\"'\">s</ms>"), "\\text{'s'}");
}

#[test]
fn spaces() {
    assert_eq!(
        tex("<mi>a</mi><mspace width=\"1em\"/><mi>b</mi>"),
        "a\\quad b"
    );
    assert_eq!(
        tex("<mi>a</mi><mspace width=\"2em\"/><mi>b</mi>"),
        "a\\qquad b"
    );
    assert_eq!(
        tex("<mi>a</mi><mspace width=\"0.5em\"/><mi>b</mi>"),
        r"a\; b"
    );
    assert_eq!(
        tex("<mi>a</mi><mspace width=\"0.1em\"/><mi>b</mi>"),
        "a\\, b"
    );
    assert_eq!(tex("<mi>a</mi><mspace width=\"0em\"/><mi>b</mi>"), "ab");
    assert_eq!(tex("<mi>a</mi><mspace width=\"-1em\"/><mi>b</mi>"), "ab");
    assert_eq!(tex("<mi>a</mi><mspace/><mi>b</mi>"), "a\\,b");
    assert_eq!(
        tex("<mi>a</mi><mspace width=\"10px\"/><mi>b</mi>"),
        r"a\; b"
    );
}

#[test]
fn fractions_and_roots() {
    assert_eq!(tex("<mfrac><mn>1</mn><mn>2</mn></mfrac>"), "\\frac{1}{2}");
    assert_eq!(
        tex("<mfrac><mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow><mi>c</mi></mfrac>"),
        "\\frac{a+b}{c}"
    );
    assert_eq!(
        tex("<mfrac linethickness=\"0\"><mi>n</mi><mi>k</mi></mfrac>"),
        "{{n} \\atop {k}}"
    );
    assert_eq!(
        tex("<mfrac linethickness=\"0px\"><mi>n</mi><mi>k</mi></mfrac>"),
        "{{n} \\atop {k}}"
    );
    assert_eq!(
        tex("<mfrac linethickness=\"2\"><mi>n</mi><mi>k</mi></mfrac>"),
        "\\frac{n}{k}"
    );
    assert_eq!(
        tex("<mfrac bevelled=\"true\"><mi>a</mi><mi>b</mi></mfrac>"),
        "{a}/{b}"
    );
    assert_eq!(tex("<msqrt><mi>x</mi></msqrt>"), "\\sqrt{x}");
    assert_eq!(
        tex("<msqrt><mi>x</mi><mo>+</mo><mn>1</mn></msqrt>"),
        "\\sqrt{x+1}"
    );
    assert_eq!(tex("<mroot><mi>x</mi><mn>3</mn></mroot>"), "\\sqrt[3]{x}");
    assert_eq!(tex("<mroot><mi>x</mi></mroot>"), "\\sqrt{x}");
    // Missing children are empty groups, not a panic.
    assert!(to_latex(&wrap("<mfrac><mn>1</mn></mfrac>"), false).is_some());
}

#[test]
fn scripts() {
    assert_eq!(tex("<msup><mi>x</mi><mn>2</mn></msup>"), "{x}^{2}");
    assert_eq!(tex("<msub><mi>x</mi><mn>1</mn></msub>"), "{x}_{1}");
    assert_eq!(
        tex("<msubsup><mi>x</mi><mn>1</mn><mn>2</mn></msubsup>"),
        "{x}_{1}^{2}"
    );
    // A big operator keeps its limit behaviour: no braces around it.
    assert_eq!(
        tex("<msubsup><mo>\u{222B}</mo><mn>0</mn><mi>\u{221E}</mi></msubsup><mi>f</mi>"),
        "\\int _{0}^{\\infty}f"
    );
    assert_eq!(
        tex("<msub><mo>\u{2211}</mo><mi>i</mi></msub>"),
        "\\sum _{i}"
    );
    // A function name takes its limit as a script.
    assert_eq!(tex("<msub><mi>max</mi><mi>x</mi></msub>"), "\\max _{x}");
    // Nested scripts.
    assert_eq!(
        tex("<msup><msub><mi>x</mi><mi>i</mi></msub><mn>2</mn></msup>"),
        "{{x}_{i}}^{2}"
    );
    // Prescripts and post scripts.
    assert_eq!(
        tex("<mmultiscripts><mi>X</mi><mi>a</mi><mi>b</mi><mprescripts/><mi>c</mi><mi>d</mi></mmultiscripts>"),
        "{}_{c}^{d}{X}_{a}^{b}"
    );
    assert_eq!(
        tex("<mmultiscripts><mi>X</mi><none/><mi>b</mi></mmultiscripts>"),
        "{X}^{b}"
    );
    assert_eq!(
        tex("<mmultiscripts><mi>X</mi><mprescripts/><mi>c</mi><none/></mmultiscripts>"),
        "{}_{c}{X}"
    );
}

#[test]
fn under_and_over() {
    // Limits of a sum / product / limit.
    assert_eq!(
        tex("<munderover><mo>\u{2211}</mo><mrow><mi>i</mi><mo>=</mo><mn>1</mn></mrow><mi>n</mi></munderover><msub><mi>x</mi><mi>i</mi></msub>"),
        "\\sum _{i=1}^{n}{x}_{i}"
    );
    assert_eq!(
        tex("<munder><mo>\u{220F}</mo><mi>i</mi></munder><mi>a</mi>"),
        "\\prod _{i}a"
    );
    assert_eq!(
        tex("<munder><mi>lim</mi><mrow><mi>x</mi><mo>\u{2192}</mo><mn>0</mn></mrow></munder><mi>f</mi>"),
        "\\lim _{x\\to 0}f"
    );
    assert_eq!(
        tex("<mover><mo>\u{22C3}</mo><mi>n</mi></mover>"),
        "\\bigcup ^{n}"
    );
    // Accents.
    assert_eq!(
        tex("<mover><mi>x</mi><mo>\u{02C6}</mo></mover>"),
        "\\hat{x}"
    );
    assert_eq!(tex("<mover><mi>x</mi><mo>~</mo></mover>"), "\\tilde{x}");
    assert_eq!(
        tex("<mover><mi>x</mi><mo>\u{02D9}</mo></mover>"),
        "\\dot{x}"
    );
    assert_eq!(
        tex("<mover><mi>x</mi><mo>\u{00A8}</mo></mover>"),
        "\\ddot{x}"
    );
    assert_eq!(
        tex("<mover><mi>v</mi><mo>\u{2192}</mo></mover>"),
        "\\vec{v}"
    );
    assert_eq!(
        tex("<mover><mrow><mi>A</mi><mi>B</mi></mrow><mo>\u{2192}</mo></mover>"),
        "\\overrightarrow{AB}"
    );
    assert_eq!(
        tex("<mover><mrow><mi>a</mi><mi>b</mi></mrow><mo>\u{00AF}</mo></mover>"),
        "\\overline{ab}"
    );
    assert_eq!(
        tex("<mover><mi>x</mi><mo>\u{2190}</mo></mover>"),
        "\\overleftarrow{x}"
    );
    assert_eq!(
        tex("<mover><mi>x</mi><mo>\u{2194}</mo></mover>"),
        "\\overleftrightarrow{x}"
    );
    // Lines and braces below.
    assert_eq!(
        tex("<munder><mrow><mi>a</mi><mi>b</mi></mrow><mo>_</mo></munder>"),
        "\\underline{ab}"
    );
    assert_eq!(
        tex("<munder><mrow><mi>x</mi><mo>+</mo><mi>y</mi></mrow><mo>\u{23DF}</mo></munder>"),
        "\\underbrace{x+y}"
    );
    assert_eq!(
        tex("<mover><mrow><mi>x</mi><mo>+</mo><mi>y</mi></mrow><mo>\u{23DE}</mo></mover>"),
        "\\overbrace{x+y}"
    );
    // A label on a brace: the outer element carries it as a script of the brace.
    assert_eq!(
        tex("<munder><munder><mrow><mi>x</mi></mrow><mo>\u{23DF}</mo></munder><mi>n</mi></munder>"),
        "\\underbrace{x}_{n}"
    );
    assert_eq!(
        tex("<mover><mover><mrow><mi>x</mi></mrow><mo>\u{23DE}</mo></mover><mi>n</mi></mover>"),
        "\\overbrace{x}^{n}"
    );
    // Anything else: a symbol set over / under.
    assert_eq!(
        tex("<mover><mi>x</mi><mi>a</mi></mover>"),
        "\\overset{a}{x}"
    );
    assert_eq!(
        tex("<munder><mi>x</mi><mi>a</mi></munder>"),
        "\\underset{a}{x}"
    );
    assert_eq!(
        tex("<munderover><mi>x</mi><mi>a</mi><mi>b</mi></munderover>"),
        "\\overset{b}{\\underset{a}{x}}"
    );
    // The element without its children does not panic.
    assert!(to_latex(&wrap("<munder/>"), false).is_none());
}

#[test]
fn fences() {
    assert_eq!(
        tex("<mfenced><mi>a</mi><mi>b</mi></mfenced>"),
        "\\left( a , b \\right)"
    );
    assert_eq!(
        tex("<mfenced open=\"[\" close=\"]\" separators=\";\"><mi>a</mi><mi>b</mi><mi>c</mi></mfenced>"),
        "\\left[ a ; b ; c \\right]"
    );
    assert_eq!(
        tex("<mfenced open=\"{\" close=\"}\" separators=\"\"><mi>a</mi><mi>b</mi></mfenced>"),
        "\\left\\{ a b \\right\\}"
    );
    assert_eq!(
        tex("<mfenced open=\"|\" close=\"|\"><mi>x</mi></mfenced>"),
        "\\left| x \\right|"
    );
    assert_eq!(
        tex("<mfenced open=\"\u{27E8}\" close=\"\u{27E9}\" separators=\"|\"><mi>a</mi><mi>b</mi></mfenced>"),
        "\\left\\langle a \\mid b \\right\\rangle"
    );
    // No delimiter on either side: a group.
    assert_eq!(
        tex("<mfenced open=\"\" close=\"\"><mi>a</mi></mfenced>"),
        "{a}"
    );
    // A delimiter RaTeX has no stretchy form for is a plain character.
    assert_eq!(
        tex("<mfenced open=\"\u{300C}\" close=\"\u{300D}\"><mi>a</mi></mfenced>"),
        "\u{300C}a\u{300D}"
    );
    // LibreOffice's `left ( .. right )`: stretchy fences around an mrow.
    assert_eq!(
        tex("<mrow><mo stretchy=\"true\" fence=\"true\">(</mo><mi>a</mi><mo>+</mo><mi>b</mi><mo stretchy=\"true\" fence=\"true\">)</mo></mrow>"),
        "\\left( a+b \\right)"
    );
    // Plain parentheses stay plain.
    assert_eq!(
        tex("<mrow><mo stretchy=\"false\">(</mo><mi>a</mi><mo stretchy=\"false\">)</mo></mrow>"),
        "(a)"
    );
    assert_eq!(tex("<mrow><mo>(</mo><mi>a</mi><mo>)</mo></mrow>"), "(a)");
    // A lone stretchy fence pairs with an empty one (`cases`).
    assert_eq!(
        tex("<mrow><mo stretchy=\"true\" fence=\"true\">{</mo><mtable><mtr><mtd><mi>x</mi></mtd></mtr><mtr><mtd><mi>y</mi></mtd></mtr></mtable></mrow>"),
        "\\left\\{ \\begin{matrix} x \\\\ y \\end{matrix} \\right."
    );
    assert_eq!(
        tex("<mrow><mi>a</mi><mo stretchy=\"true\" fence=\"true\">)</mo></mrow>"),
        "\\left. a \\right)"
    );
    // A table between two plain parentheses is a matrix.
    assert_eq!(
        tex("<mrow><mo>(</mo><mtable><mtr><mtd><mi>a</mi></mtd></mtr></mtable><mo>)</mo></mrow>"),
        "\\left( \\begin{matrix} a \\end{matrix} \\right)"
    );
}

#[test]
fn tables() {
    assert_eq!(
        tex("<mtable><mtr><mtd><mi>a</mi></mtd><mtd><mi>b</mi></mtd></mtr><mtr><mtd><mi>c</mi></mtd><mtd><mi>d</mi></mtd></mtr></mtable>"),
        "\\begin{matrix} a & b \\\\ c & d \\end{matrix}"
    );
    assert_eq!(
        tex("<mtable><mlabeledtr><mtd><mtext>(1)</mtext></mtd><mtd><mi>a</mi></mtd></mlabeledtr></mtable>"),
        "\\begin{matrix} a \\end{matrix}"
    );
    // A table with no rows is nothing.
    assert!(to_latex(&wrap("<mtable/>"), false).is_none());
}

#[test]
fn enclosures_and_phantoms() {
    assert_eq!(
        tex("<menclose notation=\"box\"><mi>x</mi></menclose>"),
        "\\boxed{x}"
    );
    assert_eq!(
        tex("<menclose notation=\"roundedbox\"><mi>x</mi></menclose>"),
        "\\boxed{x}"
    );
    assert_eq!(
        tex("<menclose notation=\"top\"><mi>x</mi></menclose>"),
        "\\overline{x}"
    );
    assert_eq!(
        tex("<menclose notation=\"bottom\"><mi>x</mi></menclose>"),
        "\\underline{x}"
    );
    assert_eq!(
        tex("<menclose notation=\"radical\"><mi>x</mi></menclose>"),
        "\\sqrt{x}"
    );
    assert_eq!(
        tex("<menclose notation=\"updiagonalstrike\"><mi>x</mi></menclose>"),
        "\\cancel{x}"
    );
    assert_eq!(
        tex("<menclose notation=\"downdiagonalstrike\"><mi>x</mi></menclose>"),
        "\\bcancel{x}"
    );
    assert_eq!(
        tex("<menclose notation=\"updiagonalstrike downdiagonalstrike\"><mi>x</mi></menclose>"),
        "\\xcancel{x}"
    );
    // Not drawn here: the content alone.
    assert_eq!(
        tex("<menclose notation=\"longdiv\"><mi>x</mi></menclose>"),
        "x"
    );
    assert_eq!(tex("<menclose><mi>x</mi></menclose>"), "x");
    assert_eq!(tex("<mphantom><mi>y</mi></mphantom>"), "\\phantom{y}");
}

#[test]
fn variants() {
    assert_eq!(tex("<mi mathvariant=\"bold\">v</mi>"), "\\mathbf{v}");
    assert_eq!(
        tex("<mi mathvariant=\"double-struck\">R</mi>"),
        "\\mathbb{R}"
    );
    assert_eq!(tex("<mi mathvariant=\"script\">L</mi>"), "\\mathcal{L}");
    assert_eq!(tex("<mi mathvariant=\"fraktur\">g</mi>"), "\\mathfrak{g}");
    assert_eq!(tex("<mi mathvariant=\"sans-serif\">s</mi>"), "\\mathsf{s}");
    assert_eq!(tex("<mi mathvariant=\"monospace\">m</mi>"), "\\mathtt{m}");
    assert_eq!(tex("<mi mathvariant=\"normal\">x</mi>"), "\\mathrm{x}");
    assert_eq!(
        tex("<mi mathvariant=\"bold-italic\">x</mi>"),
        "\\boldsymbol{x}"
    );
    assert_eq!(tex("<mi mathvariant=\"italic\">x</mi>"), "x");
    // A digit looks the same in every alphabet.
    assert_eq!(tex("<mn mathvariant=\"bold\">3</mn>"), "3");
    // `mstyle` passes its variant down; an element's own wins.
    assert_eq!(
        tex("<mstyle mathvariant=\"bold\"><mi>a</mi><mi mathvariant=\"normal\">b</mi></mstyle>"),
        "\\mathbf{a}\\mathrm{b}"
    );
}

#[test]
fn wrappers_are_transparent() {
    for body in [
        "<mrow><mi>x</mi></mrow>",
        "<mstyle displaystyle=\"true\"><mi>x</mi></mstyle>",
        "<mpadded width=\"1em\"><mi>x</mi></mpadded>",
        "<merror><mi>x</mi></merror>",
        "<maction actiontype=\"toggle\"><mi>x</mi><mi>y</mi></maction>",
        "<semantics><mi>x</mi><annotation encoding=\"StarMath 5.0\">x</annotation></semantics>",
        "<semantics><mrow><mi>x</mi></mrow><annotation-xml encoding=\"MathML-Content\"><ci>x</ci></annotation-xml></semantics>",
        "<unknown><mi>x</mi></unknown>",
    ] {
        assert_eq!(tex(body), "x", "{body}");
    }
}

#[test]
fn prefixed_and_unprefixed_namespaces() {
    let a = "<math:math xmlns:math=\"http://www.w3.org/1998/Math/MathML\"><math:mfrac><math:mn>1</math:mn><math:mn>2</math:mn></math:mfrac></math:math>";
    assert_eq!(to_latex(a, false).as_deref(), Some("\\frac{1}{2}"));
    assert!(is_math(a));
    // No namespace declaration at all (a fragment).
    assert_eq!(
        to_latex("<math><mi>x</mi></math>", false).as_deref(),
        Some("x")
    );
    // No `math` element: the top-level elements are the expression.
    assert_eq!(to_latex("<mi>x</mi>", false).as_deref(), Some("x"));
}

#[test]
fn libreoffice_formulas() {
    // The two formulas of testdata/office/word.odt, as LibreOffice wrote them.
    assert_eq!(
        tex("<semantics><mrow><mrow><mi>E</mi><mo stretchy=\"false\">=</mo><mi>m</mi></mrow><msup><mi>c</mi><mn>2</mn></msup></mrow><annotation encoding=\"StarMath 5.0\">E = m c^2</annotation></semantics>"),
        "E=m{c}^{2}"
    );
    assert_eq!(
        tex("<semantics><mrow><mfrac><mrow><mi>a</mi><mo stretchy=\"false\">+</mo><mi>b</mi></mrow><mi>c</mi></mfrac><mo stretchy=\"false\">=</mo><msqrt><mrow><msubsup><mi>x</mi><mn>1</mn><mn>2</mn></msubsup><mo stretchy=\"false\">+</mo><msubsup><mi>y</mi><mn>1</mn><mn>2</mn></msubsup></mrow></msqrt></mrow><annotation encoding=\"StarMath 5.0\">x</annotation></semantics>"),
        "\\frac{a+b}{c}=\\sqrt{{x}_{1}^{2}+{y}_{1}^{2}}"
    );
}

#[test]
fn same_latex_as_the_omml_converter_for_the_same_formula() {
    // The two converters share the character tables; a formula written in either markup gives the
    // same drawing. (The strings differ in spacing, so compare the RaTeX output.)
    let omml = |x: &str| {
        super::omml::to_latex(
            &format!("<m:oMath xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\">{x}</m:oMath>"),
            false,
        )
        .unwrap()
    };
    let r = |t: &str| format!("<m:r><m:t>{t}</m:t></m:r>");
    let svg = |l: &str| latex_to_svg(l, true, "#000000").unwrap();
    let pairs: Vec<(String, String)> = vec![
        (
            tex("<mfrac><mi>a</mi><mi>b</mi></mfrac>"),
            omml(&format!(
                "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
                r("a"),
                r("b")
            )),
        ),
        (
            tex("<msup><mi>x</mi><mn>2</mn></msup>"),
            omml(&format!(
                "<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>",
                r("x"),
                r("2")
            )),
        ),
        (
            tex("<msqrt><mi>x</mi></msqrt>"),
            omml(&format!(
                "<m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/><m:e>{}</m:e></m:rad>",
                r("x")
            )),
        ),
        (
            tex("<mi>\u{03B1}</mi><mo>+</mo><mi>\u{03B2}</mi>"),
            omml(&r("\u{03B1}+\u{03B2}")),
        ),
    ];
    for (a, b) in pairs {
        assert_eq!(svg(&a), svg(&b), "{a} vs {b}");
    }
}

#[test]
fn fallback_text_prefers_the_starmath_source() {
    let x = wrap("<semantics><mrow><mi>a</mi></mrow><annotation encoding=\"StarMath 5.0\">{a} over {b}</annotation></semantics>");
    assert_eq!(fallback_text(&x), "{a} over {b}");
    // Without a StarMath annotation: the tokens' characters.
    let y = wrap("<mrow><mi>a</mi><mo>+</mo><mn>1</mn></mrow>");
    assert_eq!(fallback_text(&y), "a + 1");
    // Another annotation encoding is not StarMath.
    let z = wrap("<semantics><mi>q</mi><annotation encoding=\"application/x-tex\">\\alpha</annotation></semantics>");
    assert_eq!(fallback_text(&z), "q");
    assert_eq!(fallback_text("<not xml"), "");
}

#[test]
fn is_math_tells_a_formula_from_another_object() {
    assert!(is_math(&wrap("<mi>x</mi>")));
    assert!(!is_math(
        "<office:document-content xmlns:office=\"urn:x\"><office:body/></office:document-content>"
    ));
    assert!(!is_math("not xml at all <"));
    assert!(!is_math(""));
}

#[test]
fn what_cannot_be_converted_is_none() {
    assert!(to_latex("", false).is_none());
    assert!(to_latex("<math", false).is_none());
    assert!(to_latex(&wrap(""), false).is_none());
    assert!(to_latex(&wrap("<mrow/>"), false).is_none());
    assert!(to_latex(&wrap("<mspace/>"), false).is_some());
    // Mismatched / unclosed XML.
    assert!(to_latex("<math><mi>x</math>", false).is_none());
    assert!(to_latex("<math><mi>x</mi>", false).is_none());
}

#[test]
fn limits() {
    // Deeper than the reader allows.
    let deep = format!(
        "{}<mi>x</mi>{}",
        "<mrow>".repeat(300),
        "</mrow>".repeat(300)
    );
    assert!(to_latex(&wrap(&deep), false).is_none());
    // Within the depth: fine.
    let ok = format!(
        "{}<mi>x</mi>{}",
        "<mrow>".repeat(100),
        "</mrow>".repeat(100)
    );
    assert_eq!(to_latex(&wrap(&ok), false).as_deref(), Some("x"));
    // Too many elements.
    let many = "<mi>x</mi>".repeat(100_001);
    assert!(to_latex(&wrap(&many), false).is_none());
    // Over the input length.
    let big = format!("<mtext>{}</mtext>", "a".repeat(1 << 20));
    assert!(to_latex(&wrap(&big), false).is_none());
    assert_eq!(fallback_text(&wrap(&big)), "");
    // A very long token is cut, the formula still converts.
    let long = format!("<mn>{}</mn>", "1".repeat(20_000));
    let l = to_latex(&wrap(&long), false).unwrap();
    assert!(l.len() <= 64 * 1024);
    // Many scripts are bounded.
    let ms = format!(
        "<mmultiscripts><mi>x</mi>{}</mmultiscripts>",
        "<mi>a</mi>".repeat(10_000)
    );
    let _ = to_latex(&wrap(&ms), false);
}

#[test]
fn nothing_panics_on_mutated_input() {
    let base = wrap("<semantics><mrow><munderover><mo>\u{2211}</mo><mi>i</mi><mi>n</mi></munderover><mfenced><mfrac><mi>a</mi><msqrt><mi>b</mi></msqrt></mfrac></mfenced><mtable><mtr><mtd><mi>x</mi></mtd></mtr></mtable><menclose notation=\"box\"><mi>y</mi></menclose></mrow><annotation encoding=\"StarMath 5.0\">q</annotation></semantics>");
    let bytes = base.as_bytes();
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..3000 {
        let mut v = bytes.to_vec();
        for _ in 0..=(next() % 4) {
            if v.is_empty() {
                break;
            }
            let i = (next() as usize) % v.len();
            match next() % 3 {
                0 => v[i] = (next() & 0xFF) as u8,
                1 => {
                    v.remove(i);
                }
                _ => v.insert(i, b"<>&\"/ ="[(next() % 7) as usize]),
            }
        }
        let s = String::from_utf8_lossy(&v).into_owned();
        let _ = to_latex(&s, false);
        let _ = fallback_text(&s);
        let _ = is_math(&s);
    }
}

#[test]
fn every_output_in_this_file_is_drawable_even_in_an_equation_array() {
    // A formula that mixes several features, drawn as a whole.
    let l = tex("<mrow><mi>f</mi><mo>(</mo><mi>x</mi><mo>)</mo><mo>=</mo><munderover><mo>\u{222B}</mo><mn>0</mn><mi>x</mi></munderover><mfrac><mrow><mi>sin</mi><mi>t</mi></mrow><mi>t</mi></mfrac><mi>d</mi><mi>t</mi><mo>+</mo><mroot><mi>x</mi><mn>3</mn></mroot><mo>+</mo><mrow><mo stretchy=\"true\">(</mo><mtable><mtr><mtd><mn>1</mn></mtd><mtd><mn>0</mn></mtd></mtr><mtr><mtd><mn>0</mn></mtd><mtd><mn>1</mn></mtd></mtr></mtable><mo stretchy=\"true\">)</mo></mrow></mrow>");
    assert!(l.contains("\\int") && l.contains("\\frac") && l.contains("\\left("));
}
