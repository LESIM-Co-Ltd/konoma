//! Regression tests for the review of the Word / OpenDocument text readers: hostile documents
//! (file reads, amplification), Markdown injection and conversion mistakes. Every size here is
//! small: a limit is proven by lowering it, never by allocating the real thing.

use super::docx::{DocOptions, PEAK_HELD};
use super::tests_docx::{conv, conv_with, md, md_styled, p, rendered, Dx, HEADING_STYLES};
use super::tests_odt::{conv as oconv, conv_with as oconv_with, para_style, sp, with_opts, Ox};
use crate::preview::markdown::process_inline_html;

/// konoma's slug of a heading (`app::md_text::github_slug`, private to the app; same rule).
fn app_slug(text: &str) -> String {
    let mut s = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() {
            s.extend(c.to_lowercase());
        } else if c == ' ' {
            s.push('-');
        } else if c == '-' || c == '_' {
            s.push(c);
        }
    }
    s
}

/// The `#anchor`s the app would register for `markdown`: konoma's own pre-passes and renderer, then
/// the slug of each rendered heading, numbered like `compute_md_anchors`.
fn rendered_anchors(markdown: &str) -> Vec<String> {
    use crate::preview::markdown::{
        heading_text, process_footnotes, process_inline_html, render_markdown, strip_front_matter,
        CodeStyle,
    };
    let body = strip_front_matter(markdown).1;
    let pre = process_inline_html(&process_footnotes(&body));
    let mut counts = std::collections::HashMap::<String, usize>::new();
    let mut out = Vec::new();
    for l in render_markdown(&pre, 200, CodeStyle::default(), "TwoDark", false) {
        let Some(t) = heading_text(&l) else { continue };
        let base = app_slug(&t);
        if base.is_empty() {
            continue;
        }
        let n = counts.entry(base.clone()).or_insert(0);
        out.push(if *n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        });
        *n += 1;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

/// Whether `markdown` parses to nothing but paragraphs (and inline markup): no heading, list,
/// quote, rule, table, code block or HTML block. The structure a text must not be able to make.
fn only_paragraphs(markdown: &str) -> bool {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    Parser::new_ext(markdown, Options::all()).all(|e| match e {
        Event::Start(t) => matches!(
            t,
            Tag::Paragraph
                | Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::Superscript
                | Tag::Subscript
        ),
        Event::Rule | Event::Html(_) | Event::TaskListMarker(_) => false,
        _ => true,
    })
}

/// How many block-level constructs of each kind `markdown` has: (headings, code blocks, tables).
fn block_counts(markdown: &str) -> (usize, usize, usize) {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    let (mut h, mut c, mut t) = (0, 0, 0);
    for e in Parser::new_ext(markdown, Options::all()) {
        match e {
            Event::Start(Tag::Heading { .. }) => h += 1,
            Event::Start(Tag::CodeBlock(_)) => c += 1,
            Event::Start(Tag::Table(_)) => t += 1,
            _ => {}
        }
    }
    (h, c, t)
}

/// The cells of each row of the first table of `markdown`.
fn table_rows(markdown: &str) -> Vec<Vec<String>> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut rows: Vec<Vec<String>> = Vec::new();
    for e in Parser::new_ext(markdown, Options::all()) {
        match e {
            Event::Start(Tag::TableHead | Tag::TableRow) => rows.push(Vec::new()),
            Event::Start(Tag::TableCell) => rows.last_mut().unwrap().push(String::new()),
            Event::Text(t) | Event::Code(t) => {
                if let Some(c) = rows.last_mut().and_then(|r| r.last_mut()) {
                    c.push_str(&t);
                }
            }
            Event::End(TagEnd::Table) => break,
            _ => {}
        }
    }
    rows
}

/// Characters that start a line of text the way a space does, but are not trimmed by the
/// escaping (they used to be trimmed *after* it).
const LEADING: [char; 6] = [
    '\u{3000}', '\u{a0}', '\u{2003}', '\u{2028}', '\u{2029}', '\u{85}',
];

/// Text that would be a block of its own at the start of a line.
const MARKERS: [&str; 9] = [
    "# t", "## t", "+ t", "1) t", "1. t", "- t", "> t", "* t", "=== ",
];

// ---------------------------------------------------------------------------------------------
// markdown injection: a text must not be able to make structure
// ---------------------------------------------------------------------------------------------

#[test]
fn a_paragraph_starting_with_unicode_space_cannot_make_a_block() {
    for lead in LEADING {
        for marker in MARKERS {
            let m = md(&p(&format!("{lead}{marker}")));
            assert!(only_paragraphs(&m), "{lead:?} {marker:?} -> {m:?}");
            // and in konoma's own renderer
            let shown = rendered(&m, 80).join("\n");
            assert!(!shown.contains('━') && !shown.contains('─'), "{shown}");
            // The same text after a line break inside the paragraph.
            let m = md(&para_br(&format!("a<w:br/>{lead}{marker}")));
            assert!(
                only_paragraphs(&m),
                "{lead:?} {marker:?} after a break -> {m:?}"
            );
        }
    }
}

fn para_br(inner: &str) -> String {
    // `<w:br/>` between runs: a run per piece.
    let mut out = String::new();
    for (i, piece) in inner.split("<w:br/>").enumerate() {
        if i > 0 {
            out.push_str("<w:r><w:br/></w:r>");
        }
        out.push_str(&format!(
            r#"<w:r><w:t xml:space="preserve">{piece}</w:t></w:r>"#
        ));
    }
    format!("<w:p>{out}</w:p>")
}

#[test]
fn a_list_item_and_a_cell_starting_with_unicode_space_cannot_make_a_block_either() {
    let numbering = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#;
    let item = format!(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t xml:space="preserve">{}# t</w:t></w:r></w:p>"#,
        '\u{3000}'
    );
    let m = conv(&Dx::new(&item).numbering(numbering)).markdown;
    assert_eq!(block_counts(&m).0, 0, "{m:?}");
    let cell = format!(
        r#"<w:tbl><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>"#,
        p(&format!("{}# t", '\u{a0}'))
    );
    let m = md(&cell);
    assert_eq!(block_counts(&m).0, 0, "{m:?}");
}

#[test]
fn an_odt_paragraph_starting_with_unicode_space_cannot_make_a_block() {
    for lead in LEADING {
        for marker in MARKERS {
            let m = oconv(&Ox::new(&format!("<text:p>{lead}{marker}</text:p>"))).markdown;
            assert!(only_paragraphs(&m), "{lead:?} {marker:?} -> {m:?}");
        }
    }
}

#[test]
fn unicode_line_separators_are_spaces() {
    // U+2028 / U+2029 / U+0085 end a line for some readers: here they are spaces.
    let m = md(&p("a\u{2028}# b\u{2029}c\u{85}d"));
    assert_eq!(m, "a # b c d");
}

#[test]
fn an_escaped_angle_bracket_is_not_a_tag_in_the_inline_html_pass() {
    for tag in ["br", "kbd", "sub", "sup", "del", "s", "strike"] {
        let src = format!("x\\<{tag}># h\\</{tag}>\n");
        assert_eq!(process_inline_html(&src), src, "{tag}");
    }
    // An escaped backslash does not escape the `<` after it: that one is a tag.
    assert!(process_inline_html("a\\\\<br>b\n").contains("a\\\\  \nb"));
    // And a plain tag still converts.
    assert!(process_inline_html("a<br>b\n").contains("a  \nb"));
    assert!(process_inline_html("<kbd>k</kbd> \\<kbd>j</kbd>\n").starts_with("`k` \\<kbd>j"));
}

#[test]
fn a_text_with_a_tag_and_a_heading_cannot_make_a_heading_through_the_whole_chain() {
    let text = "x<br># Injected<sub>2</sub><kbd>k</kbd><del>d</del>";
    for m in [
        md(&p(&text.replace('<', "&lt;"))),
        oconv(&Ox::new(&format!(
            "<text:p>{}</text:p>",
            text.replace('<', "&lt;")
        )))
        .markdown,
    ] {
        let pre = process_inline_html(&m);
        assert!(only_paragraphs(&pre), "{m:?} -> {pre:?}");
        assert_eq!(rendered_anchors(&pre), Vec::<String>::new(), "{pre:?}");
    }
}

#[test]
fn a_code_paragraph_cannot_close_its_own_fence() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/></w:style>"#;
    let code = |t: &str| {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Code"/></w:pPr><w:r><w:t xml:space="preserve">{t}</w:t></w:r></w:p>"#
        )
    };
    for fence_line in ["   ```", "  ````", " ```", "```", "    ```"] {
        let body = code("first") + &code(fence_line) + &code("# Injected") + &p("after");
        let m = conv(&Dx::new(&body).styles(styles)).markdown;
        let (h, c, _) = block_counts(&m);
        assert_eq!((h, c), (0, 1), "{fence_line:?} -> {m:?}");
        assert!(m.ends_with("after"), "{m:?}");
    }
    // odt shares the writer.
    let auto = para_style("Source_20_Code", "", None, "");
    let body = format!(
        "{}{}{}",
        sp("Source_20_Code", "first"),
        sp("Source_20_Code", "   ```"),
        sp("Source_20_Code", "# Injected")
    ) + "<text:p>after</text:p>";
    let m = oconv(&Ox::new(&body).auto(&auto)).markdown;
    assert_eq!((block_counts(&m).0, block_counts(&m).1), (0, 1), "{m:?}");
}

#[test]
fn a_heading_that_ends_in_a_hash_keeps_it() {
    let h = |t: &str| md_styled(&styled(t), HEADING_STYLES);
    assert_eq!(h("Chapter #"), "# Chapter \\#");
    assert_eq!(h("Chapter ##"), "# Chapter \\##");
    assert_eq!(h("Chapter # #"), "# Chapter # \\#");
    assert_eq!(h("C#"), "# C#");
    assert_eq!(h("#"), "# \\#");
    for t in ["Chapter #", "Chapter ##", "Chapter # #", "C#"] {
        let m = h(t);
        let shown = rendered(&m, 80).join("\n");
        assert!(shown.contains(t), "{t:?} -> {m:?} -> {shown:?}");
    }
    // odt
    let m = oconv(&Ox::new(
        r#"<text:h text:outline-level="1">Chapter #</text:h>"#,
    ))
    .markdown;
    assert_eq!(m, "# Chapter \\#");
}

fn styled(text: &str) -> String {
    super::tests_docx::styled_p("Heading1", text)
}

// ---------------------------------------------------------------------------------------------
// formulas in table cells
// ---------------------------------------------------------------------------------------------

fn cell_table(inner: &str) -> String {
    format!(
        r#"<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>"#,
        p("h1"),
        p("h2"),
        p("left"),
        inner
    )
}

#[test]
fn a_bar_in_a_formula_does_not_split_the_cell() {
    let d = |beg: &str, end: &str| {
        format!(
            r#"<m:oMath><m:d><m:dPr><m:begChr m:val="{beg}"/><m:endChr m:val="{end}"/></m:dPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:d></m:oMath>"#
        )
    };
    let sep = r#"<m:oMath><m:d><m:e><m:r><m:t>a</m:t></m:r></m:e><m:e><m:r><m:t>b</m:t></m:r></m:e></m:d></m:oMath>"#;
    let bare = r#"<m:oMath><m:r><m:t>a|b</m:t></m:r></m:oMath>"#;
    let text = r#"<m:oMath><m:r><m:rPr><m:nor/></m:rPr><m:t>a|b</m:t></m:r></m:oMath>"#;
    let cases = [
        d("|", "|"),
        d("\u{2016}", "\u{2016}"),
        sep.to_string(),
        bare.to_string(),
        text.to_string(),
    ];
    for f in cases {
        for cell in [
            para(&f),
            format!("<w:p><m:oMathPara>{f}</m:oMathPara></w:p>"),
        ] {
            let m = md(&cell_table(&cell));
            let rows = table_rows(&m);
            assert_eq!(rows.len(), 2, "{f} -> {m}");
            assert!(rows.iter().all(|r| r.len() == 2), "{f} -> {m} -> {rows:?}");
            // konoma's own cell scanner agrees (a box of 2 columns).
            let shown = rendered(&m, 80);
            let row = shown.iter().find(|l| l.contains("left")).expect("row");
            assert_eq!(row.matches('│').count(), 3, "{f} -> {row}");
        }
    }
    // Outside a cell the same formula is the same LaTeX with no bare bar.
    let m = md(&para(&d("|", "|")));
    assert!(m.contains("\\vert") && !m.contains('|'), "{m}");
}

fn para(inner: &str) -> String {
    super::tests_docx::para(inner)
}

#[test]
fn a_bar_in_an_odt_formula_does_not_split_the_cell() {
    let mml = |inner: &str| {
        format!(
            r#"<?xml version="1.0"?><math xmlns="http://www.w3.org/1998/Math/MathML"><mrow>{inner}</mrow></math>"#
        )
    };
    let frame = |dir: &str| {
        format!(
            r#"<draw:frame draw:name="O" text:anchor-type="as-char"><draw:object xlink:href="./{dir}" xlink:type="simple"/></draw:frame>"#
        )
    };
    let o1 = mml(r#"<mo stretchy="true">|</mo><mi>x</mi><mo stretchy="true">|</mo>"#);
    let o2 = mml(r#"<mfenced open="|" close="|" separators="|"><mi>a</mi><mi>b</mi></mfenced>"#);
    let o3 = mml(r#"<mi>a</mi><mo>|</mo><mi>b</mi>"#);
    let body = format!(
        "<table:table><table:table-row><table:table-cell><text:p>h1</text:p></table:table-cell><table:table-cell><text:p>h2</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell><text:p>{}</text:p></table:table-cell><table:table-cell><text:p>{}{}</text:p></table:table-cell></table:table-row></table:table>",
        frame("O1"),
        frame("O2"),
        frame("O3")
    );
    let o = Ox::new(&body)
        .object("O1", &o1)
        .object("O2", &o2)
        .object("O3", &o3);
    let m = oconv(&o).markdown;
    let rows = table_rows(&m);
    assert!(
        rows.len() == 2 && rows.iter().all(|r| r.len() == 2),
        "{m} -> {rows:?}"
    );
    // The converter wrote the commands; the net for a stray bar never had to act.
    assert!(m.contains("\\vert") && !m.contains('\u{2223}'), "{m}");
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

fn link_targets(markdown: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    Parser::new_ext(markdown, Options::ENABLE_FOOTNOTES)
        .filter_map(|e| match e {
            Event::Start(Tag::Link { dest_url, .. }) => Some(dest_url.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn an_internal_link_finds_a_heading_with_a_note_mark_or_a_formula() {
    let h = |bm: &str, inner: &str| {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="1" w:name="{bm}"/>{inner}</w:p>"#
        )
    };
    let link = |bm: &str| {
        format!(r#"<w:p><w:hyperlink w:anchor="{bm}"><w:r><w:t>go</w:t></w:r></w:hyperlink></w:p>"#)
    };
    let t = |s: &str| format!(r#"<w:r><w:t xml:space="preserve">{s}</w:t></w:r>"#);
    let body = [
        link("a"),
        link("b"),
        link("c"),
        link("d"),
        link("e"),
        h("a", &(t("Alpha") + r#"<w:r><w:footnoteReference w:id="2"/></w:r>"#)),
        h("b", &(t("Beta ") + "<m:oMath><m:r><m:t>x</m:t></m:r></m:oMath>")),
        h(
            "c",
            &(t("Gamma ")
                + "<m:oMath><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath>"
                + &t(" end")),
        ),
        h("d", &(t("Delta") + "<w:r><w:br/></w:r>" + &t("two"))),
        h("e", &(t("Eps ") + r#"<w:r><w:footnoteReference w:id="2"/></w:r>"# + &t(" ") + r#"<w:r><w:footnoteReference w:id="3"/></w:r>"#)),
    ]
    .concat();
    let notes = r#"<w:footnote w:id="2"><w:p><w:r><w:t>n</w:t></w:r></w:p></w:footnote><w:footnote w:id="3"><w:p><w:r><w:t>m</w:t></w:r></w:p></w:footnote>"#;
    let m = conv(&Dx::new(&body).styles(HEADING_STYLES).footnotes(notes)).markdown;
    let anchors = rendered_anchors(&m);
    let targets = link_targets(&m);
    assert_eq!(targets.len(), 5, "{m}");
    for tg in &targets {
        let slug = tg.trim_start_matches('#');
        assert!(
            anchors.iter().any(|a| a == slug),
            "{tg} is not among {anchors:?}\n{m}"
        );
    }
}

#[test]
fn an_odt_cross_reference_finds_a_heading_with_a_note_mark() {
    let note = r#"<text:note text:note-class="footnote"><text:note-body><text:p>n</text:p></text:note-body></text:note>"#;
    let body = format!(
        r##"<text:p><text:a xlink:href="#Alpha|outline">go</text:a></text:p><text:h text:outline-level="1">Alpha{note}</text:h>"##
    );
    let m = oconv(&Ox::new(&body)).markdown;
    let anchors = rendered_anchors(&m);
    let targets = link_targets(&m);
    assert_eq!(targets.len(), 1, "{m}");
    assert!(
        anchors.contains(&targets[0].trim_start_matches('#').to_string()),
        "{targets:?} {anchors:?}\n{m}"
    );
}
