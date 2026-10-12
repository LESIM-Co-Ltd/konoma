//! Moved from `konoma-office-core/src/tests_review_fix.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::{conv, md, md_styled, p, rendered, Dx, HEADING_STYLES};
use super::tests_odt::{conv as oconv, para_style, sp, Ox};
use crate::preview::markdown::process_inline_html;
pub use konoma_office_core::tests_review_fix::*;

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

/// Characters that start a line of text the way a space does, but are not trimmed by the
/// escaping (they used to be trimmed *after* it).
const LEADING: [char; 6] = [
    '\u{3000}', '\u{a0}', '\u{2003}', '\u{2028}', '\u{2029}', '\u{85}',
];

/// Text that would be a block of its own at the start of a line.
const MARKERS: [&str; 9] = [
    "# t", "## t", "+ t", "1) t", "1. t", "- t", "> t", "* t", "=== ",
];

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

fn styled(text: &str) -> String {
    super::tests_docx::styled_p("Heading1", text)
}

fn cell_table(inner: &str) -> String {
    format!(
        r#"<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>"#,
        p("h1"),
        p("h2"),
        p("left"),
        inner
    )
}
