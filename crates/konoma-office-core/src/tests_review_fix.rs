//! Regression tests for the review of the Word / OpenDocument text readers: hostile documents
//! (file reads, amplification), Markdown injection and conversion mistakes. Every size here is
//! small: a limit is proven by lowering it, never by allocating the real thing.
// Compiled outside `cfg(test)` only for the host's tests (feature `test-hooks`), where the
// `#[test]` functions are gone and what only they used is unused.
#![cfg_attr(
    not(test),
    allow(unused_imports, dead_code, unused_variables, unused_mut)
)]

use super::docx::{DocOptions, PEAK_HELD};
use super::tests_docx::{conv, conv_with, md, p, Dx};
use super::tests_odt::{conv as oconv, conv_with as oconv_with, with_opts, Ox};

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// markdown injection: a text must not be able to make structure
// ---------------------------------------------------------------------------------------------

#[test]
fn unicode_line_separators_are_spaces() {
    // U+2028 / U+2029 / U+0085 end a line for some readers: here they are spaces.
    let m = md(&p("a\u{2028}# b\u{2029}c\u{85}d"));
    assert_eq!(m, "a # b c d");
}

// ---------------------------------------------------------------------------------------------
// formulas in table cells
// ---------------------------------------------------------------------------------------------

pub fn para(inner: &str) -> String {
    super::tests_docx::para(inner)
}

#[test]
fn the_converters_write_no_bare_bar() {
    use super::omml::{drawable, to_latex};
    let f = |chr: &str| {
        format!(
            r#"<m:oMath><m:d><m:dPr><m:begChr m:val="{chr}"/><m:endChr m:val="{chr}"/></m:dPr><m:e><m:r><m:t>x|y</m:t></m:r></m:e></m:d></m:oMath>"#
        )
    };
    for chr in ["|", "\u{2016}"] {
        let l = to_latex(&f(chr), false).unwrap();
        assert!(!l.contains('|'), "{l}");
        assert!(drawable(&l), "{l}");
    }
    let t = to_latex(
        r#"<m:oMath><m:r><m:rPr><m:nor/></m:rPr><m:t>p|q</m:t></m:r></m:oMath>"#,
        false,
    )
    .unwrap();
    assert!(!t.contains('|') && drawable(&t), "{t}");
}

// ---------------------------------------------------------------------------------------------
// formulas: size of what is laid out, cancellation
// ---------------------------------------------------------------------------------------------

#[test]
fn a_formula_too_large_to_lay_out_is_shown_as_characters() {
    use super::omml::{drawable, to_latex, MAX_DRAWABLE_BYTES};
    let ok = format!(
        "<m:oMath><m:r><m:t>{}</m:t></m:r></m:oMath>",
        "x".repeat(100)
    );
    assert!(to_latex(&ok, false).is_some());
    let big = format!(
        "<m:oMath><m:r><m:t>{}</m:t></m:r></m:oMath>",
        "x".repeat(MAX_DRAWABLE_BYTES + 10)
    );
    assert_eq!(to_latex(&big, false), None);
    assert!(!drawable(&"x".repeat(MAX_DRAWABLE_BYTES + 1)));
    // Through the document: the characters stay.
    let m = md(&para(&big));
    assert!(m.starts_with("xxxx") && !m.contains('$'), "{}", &m[..20]);
    // The answer is the same the second time (it is remembered).
    assert!(drawable("x^2") && drawable("x^2"));
    assert!(!drawable(r"\frac{1") && !drawable(r"\frac{1"));
}

static DOCX_FORMULAS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static DOCX_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static ODT_FORMULAS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static ODT_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn docx_math_then_cancel(_: &str, _: bool) -> Option<String> {
    DOCX_FORMULAS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    DOCX_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
    Some("x".into())
}

fn odt_math_then_cancel(_: &str, _: bool) -> Option<String> {
    ODT_FORMULAS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    ODT_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
    Some("x".into())
}

#[test]
fn a_cancelled_load_tries_no_more_formulas_even_inside_one_block() {
    use std::sync::atomic::Ordering::SeqCst;
    let f = "<m:oMath><m:r><m:t>x</m:t></m:r></m:oMath>";
    let body = para(&f.repeat(5));
    let cancel = super::Cancel::new(|| DOCX_CANCEL.load(SeqCst));
    let opts = DocOptions {
        math: docx_math_then_cancel,
        ..DocOptions::default()
    };
    let dir = super::tests::tmp("fixmath");
    let path = super::tests::write(&dir, "t.docx", &Dx::new(&body).bytes());
    let d = super::docx::load_document_cancellable(&path, &opts, Some(&cancel)).unwrap();
    assert_eq!(DOCX_FORMULAS.load(SeqCst), 1, "{}", d.markdown);

    let mml = r#"<?xml version="1.0"?><math xmlns="http://www.w3.org/1998/Math/MathML"><mi>x</mi></math>"#;
    let frame = |dir: &str| {
        format!(
            r#"<draw:frame draw:name="O" text:anchor-type="as-char"><draw:object xlink:href="./{dir}" xlink:type="simple"/></draw:frame>"#
        )
    };
    let o = Ox::new(&format!(
        "<text:p>{}{}{}</text:p>",
        frame("A"),
        frame("B"),
        frame("C")
    ))
    .object("A", mml)
    .object("B", mml)
    .object("C", mml);
    let opts = DocOptions {
        mathml: odt_math_then_cancel,
        ..DocOptions::default()
    };
    let dir = super::tests::tmp("fixmathodt");
    let path = super::tests::write(&dir, "t.odt", &o.bytes());
    let cancel = super::Cancel::new(|| ODT_CANCEL.load(SeqCst));
    super::docx::load_document_cancellable(&path, &opts, Some(&cancel)).unwrap();
    assert_eq!(ODT_FORMULAS.load(SeqCst), 1);
    let _ = (
        conv_with as fn(&Dx, &DocOptions) -> _,
        oconv_with as fn(&Ox, &DocOptions) -> _,
    );
}

// ---------------------------------------------------------------------------------------------
// amplification: a few bytes of XML must not become a lot of memory
// ---------------------------------------------------------------------------------------------

fn lvl_xml(fmt: &str, text: &str, start: u64) -> String {
    format!(
        r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="{start}"/><w:numFmt w:val="{fmt}"/><w:lvlText w:val="{text}"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#
    )
}

fn item(text: &str) -> String {
    super::tests_docx::num_p(1, 0, text)
}

#[test]
fn a_huge_level_text_makes_a_short_label() {
    // 2,048 x `%1`, each a ten-digit ideographic number: tens of kilobytes a paragraph before.
    let text = "%1".repeat(2048);
    let numbering = lvl_xml("ideographDigital", &text, 999_999_999);
    let body = item("a") + &item("b") + &item("");
    let m = conv(&Dx::new(&body).numbering(&numbering)).markdown;
    assert!(m.len() < 1000, "{} bytes", m.len());
    assert!(m.contains("a") && m.contains("b"));
    // A normal label is untouched.
    let m = conv(&Dx::new(&item("a")).numbering(&lvl_xml("decimal", "%1.", 3))).markdown;
    assert_eq!(m, "3. a");
}

#[test]
fn a_level_is_looked_up_without_a_copy() {
    use super::docx_styles::Numbering;
    use std::borrow::Cow;
    let xml = format!(
        r#"<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{}</w:numbering>"#,
        lvl_xml("decimal", "%1.", 1)
    );
    let n = Numbering::parse(std::io::Cursor::new(xml.as_bytes())).unwrap();
    assert!(
        matches!(n.level(1, 0, 0), Cow::Borrowed(_)),
        "a defined level"
    );
    // A level the abstract numbering does not define is a default, made once on demand.
    assert_eq!(n.level(1, 0, 5).text, "%6.");
    let long = "%1".repeat(4000);
    let xml = format!(
        r#"<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{}</w:numbering>"#,
        lvl_xml("decimal", &long, 1)
    );
    let n = Numbering::parse(std::io::Cursor::new(xml.as_bytes())).unwrap();
    assert!(n.level(1, 0, 0).text.chars().count() <= 64);
    assert!(
        super::docx_styles::render_label(&long, |_| "123456789012345".into())
            .chars()
            .count()
            <= 200
    );
}

#[test]
fn code_paragraphs_waiting_for_the_end_of_their_run_are_within_the_budget() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/></w:style>"#;
    let line = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Code"/></w:pPr><w:r><w:t>{}</w:t></w:r></w:p>"#,
        "c".repeat(100)
    );
    let body = line.repeat(300) + &p("after");
    let opts = DocOptions {
        max_markdown_bytes: 4000,
        ..DocOptions::default()
    };
    PEAK_HELD.with(|p| p.set(0));
    let d = conv_with(&Dx::new(&body).styles(styles), &opts).unwrap();
    let peak = PEAK_HELD.with(|p| p.get());
    assert!(peak <= 4000, "held {peak} bytes");
    assert!(
        d.truncated && d.markdown.len() <= 4000,
        "{}",
        d.markdown.len()
    );
    assert!(!d.markdown.contains("after"));
}

#[test]
fn note_definitions_waiting_for_the_end_of_the_document_are_within_the_budget() {
    let note = |id: usize| {
        format!(
            r#"<w:footnote w:id="{id}"><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:footnote>"#,
            "n".repeat(3000)
        )
    };
    let refs: String = (1..=60)
        .map(|i| format!(r#"<w:r><w:footnoteReference w:id="{i}"/></w:r>"#))
        .collect();
    let notes: String = (1..=60).map(note).collect();
    let opts = DocOptions {
        max_markdown_bytes: 10_000,
        ..DocOptions::default()
    };
    PEAK_HELD.with(|p| p.set(0));
    let d = conv_with(&Dx::new(&para(&refs)).footnotes(&notes), &opts).unwrap();
    let peak = PEAK_HELD.with(|p| p.get());
    assert!(peak <= 10_000, "held {peak} bytes");
    assert!(d.truncated && d.markdown.len() <= 10_000);
    // odt
    let note = format!(
        r#"<text:note text:note-class="footnote"><text:note-body><text:p>{}</text:p></text:note-body></text:note>"#,
        "n".repeat(3000)
    );
    let o = Ox::new(&format!("<text:p>{}</text:p>", note.repeat(60)));
    PEAK_HELD.with(|p| p.set(0));
    let d = oconv_with(&o, &opts).unwrap();
    let peak = PEAK_HELD.with(|p| p.get());
    assert!(peak <= 10_000, "held {peak} bytes");
    assert!(d.truncated && d.markdown.len() <= 10_000);
}

fn odt_table(row_attrs: &str, cell_attrs: &str, text: &str) -> Ox {
    Ox::new(&format!(
        r#"<table:table><table:table-row table:number-rows-repeated="1"><table:table-cell><text:p>head</text:p></table:table-cell></table:table-row><table:table-row {row_attrs}><table:table-cell {cell_attrs}><text:p>{text}</text:p></table:table-cell></table:table-row></table:table><text:p>after</text:p>"#
    ))
}

#[test]
fn odt_repeated_cells_and_rows_are_not_copied_past_the_budget() {
    let opts = with_opts(|o| o.max_markdown_bytes = 5000);
    // 1,000 copies of a cell in one row.
    PEAK_HELD.with(|p| p.set(0));
    let d = oconv_with(
        &odt_table("", r#"table:number-columns-repeated="1000""#, "0123456789"),
        &opts,
    )
    .unwrap();
    let peak = PEAK_HELD.with(|p| p.get());
    assert!(peak <= 5000, "held {peak} bytes");
    assert!(d.truncated && d.markdown.len() <= 5000);
    assert!(!d.markdown.is_empty(), "what fits is shown");
    // 10,000 copies of a row.
    PEAK_HELD.with(|p| p.set(0));
    let long = "r".repeat(97);
    let d = oconv_with(
        &odt_table(r#"table:number-rows-repeated="10000""#, "", &long),
        &opts,
    )
    .unwrap();
    let peak = PEAK_HELD.with(|p| p.get());
    assert!(peak <= 5000, "held {peak} bytes");
    assert!(d.truncated && d.markdown.len() <= 5000);
    assert!(
        d.markdown.matches(&long).count() >= 10,
        "what fits is shown"
    );
    // Both at once.
    PEAK_HELD.with(|p| p.set(0));
    let d = oconv_with(
        &odt_table(
            r#"table:number-rows-repeated="10000""#,
            r#"table:number-columns-repeated="1000""#,
            &long,
        ),
        &opts,
    )
    .unwrap();
    assert!(PEAK_HELD.with(|p| p.get()) <= 5000);
    assert!(d.truncated);
    // A table that fits is not touched.
    let d = oconv(&odt_table(r#"table:number-rows-repeated="3""#, "", "x"));
    assert!(!d.truncated, "{}", d.markdown);
    assert_eq!(d.markdown.matches("| x |").count(), 3);
}

#[test]
fn attributes_count_against_the_block_budget() {
    // Twenty elements with a 3,000-byte attribute each: no text at all, 60 KB of attributes.
    let attr = "A".repeat(3000);
    let runs = format!(r#"<w:r><w:t>x</w:t><w:foo w:val="{attr}"/></w:r>"#).repeat(20);
    let opts = DocOptions {
        max_block_text_bytes: 20_000,
        ..DocOptions::default()
    };
    let d = conv_with(&Dx::new(&para(&runs)), &opts).unwrap();
    assert!(d.truncated, "{:?}", d.markdown);
    // The same under a roomy budget converts.
    let d = conv(&Dx::new(&para(&runs)));
    assert!(!d.truncated);
    assert_eq!(d.markdown, "x".repeat(20));
}

// ---------------------------------------------------------------------------------------------
// odt: a deletion range that ends in something that is not shown
// ---------------------------------------------------------------------------------------------

const DELETED: &str = r#"<text:tracked-changes><text:changed-region text:id="c1"><text:deletion/></text:changed-region></text:tracked-changes>"#;
const START: &str = r#"<text:change-start text:change-id="c1"/>"#;
const END: &str = r#"<text:change-end text:change-id="c1"/>"#;

fn hidden_style() -> String {
    r#"<style:style style:name="Hid" style:family="paragraph"><style:text-properties text:display="none"/></style:style><style:style style:name="HidSpan" style:family="text"><style:text-properties text:display="none"/></style:style>"#.to_string()
}

#[test]
fn a_deletion_range_may_end_in_a_hidden_paragraph_span_or_section() {
    let cases = [
        // the end is in a paragraph the style hides
        format!(
            r#"{DELETED}<text:p>a{START}x</text:p><text:p text:style-name="Hid">y{END}</text:p><text:p>after</text:p>"#
        ),
        // in a span the style hides
        format!(
            r#"{DELETED}<text:p>a{START}x</text:p><text:p><text:span text:style-name="HidSpan">y{END}</text:span>z</text:p><text:p>after</text:p>"#
        ),
        // in a section that is not shown
        format!(
            r#"{DELETED}<text:p>a{START}x</text:p><text:section text:display="none"><text:p>y{END}</text:p></text:section><text:p>after</text:p>"#
        ),
    ];
    for body in cases {
        let m = oconv(&Ox::new(&body).auto(&hidden_style())).markdown;
        assert!(
            m.starts_with('a') && m.ends_with("after"),
            "{body}\n -> {m:?}"
        );
        assert!(!m.contains('x'), "{m:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// heading anchors
// ---------------------------------------------------------------------------------------------
