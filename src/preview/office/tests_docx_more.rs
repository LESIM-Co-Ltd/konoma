//! Word reader tests: pictures, notes, links, fields, text boxes, content controls, tracked
//! changes, formulas, escaping.

use std::cell::RefCell;

use super::docx::{DocOptions, Document};
use super::tests_docx::*;
use super::workbook::Cancel;
use super::OfficeError;

// ---------------------------------------------------------------------------------------------
// pictures
// ---------------------------------------------------------------------------------------------

fn drawing(rid: &str, descr: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="{descr}"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="{rid}"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

fn with_image(body: &str, name: &str, bytes: Vec<u8>) -> Dx {
    Dx::new(body)
        .rel("rId9", "image", &format!("media/{name}"), false)
        .media(name, bytes)
}

#[test]
fn a_picture_becomes_an_image_with_a_synthetic_url_and_its_bytes() {
    let d = conv(&with_image(
        &para(&drawing("rId9", "A red square")),
        "image1.png",
        tiny_png(1),
    ));
    assert_eq!(d.images.len(), 1);
    let img = &d.images[0];
    assert_eq!(img.bytes, tiny_png(1));
    assert_eq!(img.name, "image1.png");
    assert!(
        img.key.starts_with("office-img://") && img.key.ends_with("/image1.png"),
        "{}",
        img.key
    );
    let hash = &img.key["office-img://".len()..img.key.len() - "/image1.png".len()];
    assert_eq!(hash.len(), 12);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(d.markdown, format!("![A red square]({})", img.key));
}

#[test]
fn identical_pictures_in_two_parts_are_one_image() {
    let body = para(&(drawing("rIdA", "a") + &drawing("rIdB", "b")));
    let d = Dx::new(&body)
        .rel("rIdA", "image", "media/x/image1.png", false)
        .rel("rIdB", "image", "media/y/image1.png", false)
        .part("word/media/x/image1.png", &tiny_png(7))
        .part("word/media/y/image1.png", &tiny_png(7));
    let r = conv(&d);
    assert_eq!(r.images.len(), 1);
    assert_eq!(r.markdown.matches(&r.images[0].key).count(), 2);
}

#[test]
fn the_same_bytes_give_the_same_key_and_different_bytes_a_different_one() {
    let a = conv(&with_image(
        &para(&drawing("rId9", "")),
        "image1.png",
        tiny_png(1),
    ));
    let b = conv(&with_image(
        &para(&drawing("rId9", "")),
        "image1.png",
        tiny_png(1),
    ));
    let c = conv(&with_image(
        &para(&drawing("rId9", "")),
        "image1.png",
        tiny_png(2),
    ));
    assert_eq!(a.images[0].key, b.images[0].key);
    assert_ne!(a.images[0].key, c.images[0].key);
}

#[test]
fn a_picture_used_twice_is_kept_once() {
    let d = conv(&with_image(
        &(para(&drawing("rId9", "one")) + &para(&drawing("rId9", "two"))),
        "image1.png",
        tiny_png(1),
    ));
    assert_eq!(d.images.len(), 1);
    assert_eq!(d.markdown.matches("office-img://").count(), 2);
}

#[test]
fn alt_text_comes_from_descr_then_title_and_is_made_safe() {
    let title = r#"<w:r><w:drawing><wp:inline><wp:docPr id="1" name="P" title="The title"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId9"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#;
    let d = conv(&with_image(&para(title), "image1.png", tiny_png(1)));
    assert!(d.markdown.starts_with("![The title]("), "{}", d.markdown);
    let nasty = para(&drawing("rId9", "a [b](c) *d* `e` &lt;f&gt; ]"));
    let d = conv(&with_image(&nasty, "image1.png", tiny_png(1)));
    let alt = &d.markdown[2..d.markdown.find("](").unwrap()];
    assert!(
        !alt.contains(['[', ']', '(', ')', '*', '`', '<', '>']),
        "{alt}"
    );
    assert!(alt.contains('b') && alt.contains('d'), "{alt}");
    assert_eq!(d.images.len(), 1);
}

#[test]
fn a_vml_picture_is_read_too() {
    let pict = r#"<w:r><w:pict><v:shape alt="vml alt"><v:imagedata r:id="rId9" o:title="t"/></v:shape></w:pict></w:r>"#;
    let d = conv(&with_image(&para(pict), "image1.jpeg", tiny_png(3)));
    assert_eq!(d.images.len(), 1);
    assert!(d.markdown.starts_with("![vml alt]("), "{}", d.markdown);
}

#[test]
fn pictures_inside_alternate_content_are_taken_once() {
    let ac = format!(
        r#"<w:r><mc:AlternateContent><mc:Choice Requires="wps">{}</mc:Choice><mc:Fallback><w:pict><v:shape><v:imagedata r:id="rId9"/></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r>"#,
        drawing("rId9", "choice")
            .replace("<w:r>", "")
            .replace("</w:r>", "")
    );
    let d = conv(&with_image(&para(&ac), "image1.png", tiny_png(1)));
    assert_eq!(d.markdown.matches("![").count(), 1, "{}", d.markdown);
    assert!(d.markdown.starts_with("![choice]("));
}

#[test]
fn an_embedded_object_without_a_picture_leaves_a_mark() {
    let obj = |inner: &str| format!(r#"<w:r><w:object>{inner}</w:object></w:r>"#);
    let d = conv(&Dx::new(&para(&obj(
        r#"<o:OLEObject Type="Embed" ProgID="Excel.Sheet.12" r:id="rId9"/>"#,
    ))));
    assert_eq!(d.markdown, "\\[object: Excel.Sheet.12]");
    let d = conv(&Dx::new(&para(&obj(r#"<o:OLEObject r:id="rId9"/>"#))));
    assert_eq!(d.markdown, "\\[object]");
    // With its preview picture the picture is what shows.
    let d = conv(&with_image(
        &para(&obj(
            r#"<v:shape><v:imagedata r:id="rId9"/></v:shape><o:OLEObject ProgID="Excel.Sheet.12" r:id="rId8"/>"#,
        )),
        "image1.png",
        tiny_png(1),
    ));
    assert!(d.markdown.starts_with("![]("), "{}", d.markdown);
}

#[test]
fn an_external_or_missing_or_unsupported_picture_is_a_placeholder() {
    // linked (never fetched)
    let linked = r#"<w:r><w:drawing><wp:inline><wp:docPr id="1" name="P" descr="remote"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:link="rId5"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#;
    let d = conv(&Dx::new(&para(linked)).rel("rId5", "image", "http://example.com/x.png", true));
    assert_eq!(d.markdown, "\\[remote]");
    assert!(d.images.is_empty());
    // a relationship id that is not there
    let d = conv(&Dx::new(&para(&drawing("rId404", ""))));
    assert_eq!(d.markdown, "\\[image]");
    // a format konoma cannot draw
    let d = conv(&with_image(
        &para(&drawing("rId9", "chart")),
        "image1.emf",
        vec![1, 2, 3, 4],
    ));
    assert_eq!(d.markdown, "\\[chart]");
    assert!(d.images.is_empty());
    // an internal rel that names a part that is not in the package
    let d =
        conv(&Dx::new(&para(&drawing("rId9", ""))).rel("rId9", "image", "media/none.png", false));
    assert_eq!(d.markdown, "\\[image]");
}

#[test]
fn an_embedded_picture_whose_relationship_is_marked_external_is_not_fetched() {
    let d = conv(
        &Dx::new(&para(&drawing("rId9", "x")))
            .rel("rId9", "image", "media/image1.png", true)
            .media("image1.png", tiny_png(1)),
    );
    assert!(d.images.is_empty());
}

#[test]
fn picture_budgets_stop_with_a_placeholder_and_truncated() {
    let mut d = Dx::new("");
    let mut body = String::new();
    for i in 0..4u8 {
        d = d
            .rel(
                &format!("rI{i}"),
                "image",
                &format!("media/i{i}.png"),
                false,
            )
            .media(&format!("i{i}.png"), tiny_png(i + 1));
        body += &para(&drawing(&format!("rI{i}"), &format!("n{i}")));
    }
    d.body = body;
    let opts = DocOptions {
        max_images: 2,
        ..DocOptions::default()
    };
    let r = conv_with(&d, &opts).unwrap();
    assert_eq!(r.images.len(), 2);
    assert!(r.truncated);
    assert!(
        r.markdown.contains("\\[n2]") && r.markdown.contains("\\[n3]"),
        "{}",
        r.markdown
    );

    let opts = DocOptions {
        max_image_bytes: 10,
        ..DocOptions::default()
    };
    let r = conv_with(&d, &opts).unwrap();
    assert!(r.images.is_empty() && r.truncated);

    let opts = DocOptions {
        max_total_image_bytes: 50,
        ..DocOptions::default()
    };
    let r = conv_with(&d, &opts).unwrap();
    assert!(r.images.len() == 2 && r.truncated, "{}", r.images.len());
}

#[test]
fn a_picture_in_a_table_cell_and_konoma_draws_it() {
    let cell = format!("<w:tc>{}</w:tc>", para(&drawing("rId9", "fig")));
    let d = conv(&with_image(
        &format!("<w:tbl><w:tr>{cell}</w:tr></w:tbl>"),
        "image1.png",
        tiny_png(1),
    ));
    assert!(
        d.markdown.starts_with("| ![fig](office-img://"),
        "{}",
        d.markdown
    );
    let lines = rendered(&d.markdown, 40);
    assert!(lines.iter().any(|l| l.contains("fig")), "{lines:?}");
}

#[test]
fn a_drawing_without_a_picture_shows_its_description_only() {
    let chart = r#"<w:r><w:drawing><wp:inline><wp:docPr id="1" name="Chart 1" descr="sales chart"/><a:graphic><a:graphicData uri="chart"><c:chart xmlns:c="x" r:id="rId3"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#;
    assert_eq!(md(&para(chart)), "\\[sales chart]");
    let none = r#"<w:r><w:drawing><wp:inline><wp:docPr id="1" name="Chart 1"/></wp:inline></w:drawing></w:r>"#;
    assert_eq!(md(&(p("a") + &para(none) + &p("b"))), "a\n\nb");
}

// ---------------------------------------------------------------------------------------------
// footnotes and endnotes
// ---------------------------------------------------------------------------------------------

fn note(id: i64, text: &str) -> String {
    format!(
        r#"<w:footnote w:id="{id}"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> {text}</w:t></w:r></w:p></w:footnote>"#
    )
}

const SEPARATORS: &str = r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#;

fn fn_ref(id: i64) -> String {
    format!(r#"<w:r><w:footnoteReference w:id="{id}"/></w:r>"#)
}

#[test]
fn footnotes_become_numbered_references_and_definitions() {
    let body = para(&(run("first") + &fn_ref(2) + &run(" second") + &fn_ref(5)));
    let d = conv(&Dx::new(&body).footnotes(
        &(SEPARATORS.to_string() + &note(2, "two") + &note(5, "five") + &note(9, "unused")),
    ));
    assert_eq!(d.markdown, "first[^1] second[^2]\n\n[^1]: two\n[^2]: five");
    assert_eq!(d.notes, 2);
    let v = visible(&d.markdown);
    assert!(v.contains("first¹ second²"), "{v}");
    assert!(v.contains("1. two") && v.contains("2. five"), "{v}");
    assert!(!v.contains("unused"));
}

#[test]
fn references_are_numbered_in_order_of_use_and_a_repeated_one_keeps_its_number() {
    let body = para(&(fn_ref(7) + &fn_ref(3) + &fn_ref(7)));
    let d = conv(&Dx::new(&body).footnotes(&(note(3, "three") + &note(7, "seven"))));
    assert_eq!(d.markdown, "[^1][^2][^1]\n\n[^1]: seven\n[^2]: three");
}

#[test]
fn endnotes_share_the_numbering() {
    let body =
        para(&(run("a") + &fn_ref(1) + &run("b") + r#"<w:r><w:endnoteReference w:id="1"/></w:r>"#));
    let en = r#"<w:endnote w:id="1"><w:p><w:r><w:endnoteRef/></w:r><w:r><w:t xml:space="preserve"> end text</w:t></w:r></w:p></w:endnote>"#;
    let d = conv(&Dx::new(&body).footnotes(&note(1, "foot text")).endnotes(en));
    assert_eq!(d.markdown, "a[^1]b[^2]\n\n[^1]: foot text\n[^2]: end text");
}

#[test]
fn a_note_with_several_paragraphs_and_formatting_keeps_each_paragraph_on_its_own_line() {
    let n = r#"<w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> first </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r></w:p><w:p><w:r><w:t>second</w:t></w:r></w:p></w:footnote>"#;
    let d = conv(&Dx::new(&para(&(run("x") + &fn_ref(1)))).footnotes(n));
    assert_eq!(d.markdown, "x[^1]\n\n[^1]: first **bold**\\\n    second");
}

#[test]
fn a_hyperlink_in_a_note_uses_the_notes_own_relationships() {
    let n = r#"<w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:hyperlink r:id="rIdN"><w:r><w:t>site</w:t></w:r></w:hyperlink></w:p></w:footnote>"#;
    let mut d = Dx::new(&para(&(run("x") + &fn_ref(1)))).footnotes(n);
    d.footnote_rels.push((
        "rIdN".into(),
        "hyperlink".into(),
        "https://example.org/n".into(),
        true,
    ));
    assert_eq!(
        conv(&d).markdown,
        "x[^1]\n\n[^1]: [site](https://example.org/n)"
    );
}

#[test]
fn a_reference_with_no_note_text_still_has_a_definition() {
    let d = conv(&Dx::new(&para(&(run("x") + &fn_ref(4)))).footnotes(SEPARATORS));
    // No such note in the part: the reference is kept; nothing defines it.
    assert_eq!(d.markdown, "x[^1]");
    let d = conv(&Dx::new(&para(&(run("x") + &fn_ref(4)))).footnotes(&note(4, "")));
    assert_eq!(d.markdown, "x[^1]\n\n[^1]: \u{2014}");
}

#[test]
fn a_reference_without_a_notes_part_is_kept_as_a_mark() {
    assert_eq!(md(&para(&(run("x") + &fn_ref(1)))), "x[^1]");
}

#[test]
fn footnote_text_with_markup_is_escaped() {
    let d = conv(
        &Dx::new(&para(&(run("x") + &fn_ref(1))))
            .footnotes(&note(1, "1. not a list *star* [^1] $5")),
    );
    let v = visible(&d.markdown);
    assert!(
        v.contains("1. 1. not a list *star* [^1] $5") || v.contains("1. not a list *star*"),
        "{v}"
    );
    assert!(v.contains("*star*") && v.contains("$5"), "{v}");
}

#[test]
fn references_inside_a_note_and_inside_a_deleted_run_are_not_numbered() {
    let n = r#"<w:footnote w:id="1"><w:p><w:r><w:t>see</w:t></w:r><w:r><w:footnoteReference w:id="2"/></w:r></w:p></w:footnote>"#;
    let d = conv(
        &Dx::new(&para(
            &(fn_ref(1) + "<w:del><w:r><w:footnoteReference w:id=\"3\"/></w:r></w:del>"),
        ))
        .footnotes(&(n.to_string() + &note(2, "two") + &note(3, "three"))),
    );
    assert_eq!(d.markdown, "[^1]\n\n[^1]: see");
}

#[test]
fn the_note_budget_stops_numbering() {
    let body = para(&(fn_ref(1) + &fn_ref(2) + &fn_ref(3)));
    let opts = DocOptions {
        max_notes: 2,
        ..DocOptions::default()
    };
    let d = conv_with(
        &Dx::new(&body).footnotes(&(note(1, "a") + &note(2, "b") + &note(3, "c"))),
        &opts,
    )
    .unwrap();
    assert!(d.truncated);
    assert_eq!(d.notes, 2);
    assert!(!d.markdown.contains("[^3]"));
}

#[test]
fn a_literal_bracket_caret_in_the_text_does_not_collide_with_a_real_note() {
    let body = para(&(run("literal [^1] text") + &fn_ref(1)));
    let d = conv(&Dx::new(&body).footnotes(&note(1, "real")));
    let v = visible(&d.markdown);
    assert!(v.contains("literal [^1] text") && v.contains('¹'), "{v}");
    assert!(v.contains("1. real"), "{v}");
}

// ---------------------------------------------------------------------------------------------
// hyperlinks and fields
// ---------------------------------------------------------------------------------------------

fn link(rid: &str, text: &str) -> String {
    format!(
        r#"<w:hyperlink r:id="{rid}"><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:hyperlink>"#
    )
}

fn with_link(body: &str, target: &str) -> Dx {
    Dx::new(body).rel("rId7", "hyperlink", target, true)
}

#[test]
fn an_external_hyperlink() {
    let d = conv(&with_link(
        &para(&(run("go ") + &link("rId7", "there") + &run("!"))),
        "https://example.com/a",
    ));
    assert_eq!(d.markdown, "go [there](https://example.com/a)!");
}

#[test]
fn a_link_target_is_percent_encoded_where_it_could_end_the_link() {
    let d = conv(&with_link(
        &para(&link("rId7", "t")),
        "https://example.com/a (b)/日本?x=1&y=2#f",
    ));
    assert_eq!(
        d.markdown,
        "[t](https://example.com/a%20%28b%29/%E6%97%A5%E6%9C%AC?x=1&y=2#f)"
    );
}

#[test]
fn only_web_and_mail_schemes_become_links() {
    for (target, linked) in [
        ("http://a.example/", true),
        ("HTTPS://a.example/", true),
        ("mailto:a@example.com", true),
        ("tel:+81312345678", true),
        ("ftp://a.example/", true),
        ("file:///etc/passwd", false),
        ("javascript:alert(1)", false),
        ("smb://server/share", false),
        ("../other.docx", false),
        ("", false),
    ] {
        let d = conv(&with_link(&para(&link("rId7", "t")), target));
        if linked {
            assert!(d.markdown.starts_with("[t]("), "{target}: {}", d.markdown);
        } else {
            assert_eq!(d.markdown, "t", "{target}");
        }
    }
}

#[test]
fn link_text_keeps_its_formatting_and_brackets_do_not_end_it() {
    let inner = r#"<w:hyperlink r:id="rId7"><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r><w:r><w:t xml:space="preserve"> and [b] c</w:t></w:r></w:hyperlink>"#;
    let d = conv(&with_link(&para(inner), "https://e.example/"));
    assert_eq!(
        d.markdown,
        "[**bold** and &#91;b&#93; c](https://e.example/)"
    );
    assert!(visible(&d.markdown).contains("bold and [b] c"));
}

#[test]
fn a_link_with_no_text_is_dropped_and_a_link_without_target_is_plain_text() {
    let d = conv(&with_link(
        &para(&(run("a") + "<w:hyperlink r:id=\"rId7\"/>" + &run("b"))),
        "https://e.example/",
    ));
    assert_eq!(d.markdown, "ab");
    let d = conv(&Dx::new(&para(&link("rIdNone", "plain"))));
    assert_eq!(d.markdown, "plain");
}

#[test]
fn a_link_around_a_picture_keeps_the_picture() {
    let inner = format!(
        r#"<w:hyperlink r:id="rId7">{}</w:hyperlink>"#,
        drawing("rId9", "logo")
    );
    let d = conv(
        &with_link(&para(&inner), "https://e.example/")
            .rel("rId9", "image", "media/l.png", false)
            .media("l.png", tiny_png(1)),
    );
    assert!(
        d.markdown.starts_with("[![logo](office-img://")
            && d.markdown.ends_with("](https://e.example/)"),
        "{}",
        d.markdown
    );
}

#[test]
fn an_internal_link_goes_to_the_heading_holding_the_bookmark() {
    let body = para(r#"<w:hyperlink w:anchor="_Toc9"><w:r><w:t>jump</w:t></w:r></w:hyperlink>"#)
        + &para(
            r#"<w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="1" w:name="_Toc9"/><w:r><w:t>Chapter One</w:t></w:r><w:bookmarkEnd w:id="1"/>"#,
        );
    let m = md_styled(&body, HEADING_STYLES);
    assert_eq!(m, "[jump](#chapter-one)\n\n# Chapter One");
}

#[test]
fn links_to_headings_with_equal_text_follow_the_duplicate_numbering() {
    let h = |bm: &str| {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="1" w:name="{bm}"/><w:r><w:t>Same</w:t></w:r></w:p>"#
        )
    };
    let l = |bm: &str| {
        format!(
            r#"<w:p><w:hyperlink w:anchor="{bm}"><w:r><w:t>to {bm}</w:t></w:r></w:hyperlink></w:p>"#
        )
    };
    let m = md_styled(&(h("a") + &h("b") + &l("a") + &l("b")), HEADING_STYLES);
    assert!(
        m.contains("[to a](#same)") && m.contains("[to b](#same-1)"),
        "{m}"
    );
}

#[test]
fn a_japanese_heading_slug_keeps_the_characters_unencoded() {
    // konoma matches `#anchor` to heading slugs without percent-decoding.
    let body = r#"<w:p><w:hyperlink w:anchor="bm"><w:r><w:t>へ</w:t></w:r></w:hyperlink></w:p><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="1" w:name="bm"/><w:r><w:t>はじめに 1</w:t></w:r></w:p>"#;
    let m = md_styled(body, HEADING_STYLES);
    assert!(m.starts_with("[へ](#はじめに-1)\n\n# はじめに 1"), "{m}");
}

#[test]
fn a_link_to_a_bookmark_that_is_not_a_heading_falls_back_to_the_name() {
    let body = para(r#"<w:hyperlink w:anchor="Some Mark"><w:r><w:t>x</w:t></w:r></w:hyperlink>"#);
    assert_eq!(md(&body), "[x](#some-mark)");
}

#[test]
fn a_simple_hyperlink_field_and_a_complex_one_with_an_anchor() {
    let simple = r#"<w:p><w:fldSimple w:instr=" HYPERLINK &quot;https://f.example/x&quot; "><w:r><w:t>simple</w:t></w:r></w:fldSimple></w:p>"#;
    assert_eq!(md(simple), "[simple](https://f.example/x)");
    let complex = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK \l "Target" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>complex</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    let body = complex.to_string()
        + r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="1" w:name="Target"/><w:r><w:t>Where</w:t></w:r></w:p>"#;
    assert_eq!(
        md_styled(&body, HEADING_STYLES),
        "[complex](#where)\n\n# Where"
    );
}

#[test]
fn a_hyperlink_field_with_a_url_and_switches() {
    let f = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> HYPERLINK "https://x.example/p" \o "tip" \t "_blank" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>text</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    assert_eq!(md(f), "[text](https://x.example/p)");
}

#[test]
fn a_field_shows_its_stored_result_and_never_its_instruction() {
    let f = r#"<w:p><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>3</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> of </w:t></w:r><w:fldSimple w:instr=" NUMPAGES "><w:r><w:t>9</w:t></w:r></w:fldSimple></w:p>"#;
    assert_eq!(md(f), "Page 3 of 9");
}

#[test]
fn nested_fields_and_a_field_with_no_result() {
    let f = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> IF </w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>HIDDEN-INNER</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>outer result</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> FOO </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>.</w:t></w:r></w:p>"#;
    assert_eq!(md(f), "outer result.");
}

#[test]
fn a_table_of_contents_keeps_its_entries_and_their_links() {
    let entry = |bm: &str, title: &str, page: &str| {
        format!(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK \l "{bm}" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{title}</w:t></w:r><w:r><w:tab/></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGEREF {bm} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{page}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
        )
    };
    let toc = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> TOC \o "1-3" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p>"#.to_string()
        + &entry("_Toc1", "Intro", "1")
        + &entry("_Toc2", "Body", "5")
        + r#"<w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    let h = |bm: &str, t: &str| {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="1" w:name="{bm}"/><w:r><w:t>{t}</w:t></w:r></w:p>"#
        )
    };
    let m = md_styled(
        &(toc + &h("_Toc1", "Intro") + &h("_Toc2", "Body")),
        HEADING_STYLES,
    );
    let nb = "\u{a0}\u{a0}\u{a0}\u{a0}";
    assert_eq!(
        m,
        format!("[Intro{nb}1](#intro)\n\n[Body{nb}5](#body)\n\n# Intro\n\n# Body")
    );
}

#[test]
fn a_link_field_open_at_the_end_of_a_paragraph_does_not_leak_into_the_next() {
    let f = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> HYPERLINK "https://x.example/" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>linked</w:t></w:r></w:p><w:p><w:r><w:t>after</w:t></w:r></w:p>"#;
    assert_eq!(md(f), "[linked](https://x.example/)\n\nafter");
}

#[test]
fn an_unbalanced_field_end_is_ignored() {
    let f = r#"<w:p><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>ok</w:t></w:r></w:p>"#;
    assert_eq!(md(f), "ok");
}

// ---------------------------------------------------------------------------------------------
// text boxes, alternate content, content controls
// ---------------------------------------------------------------------------------------------

fn txbx(text: &str) -> String {
    format!("<w:txbxContent><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:txbxContent>")
}

#[test]
fn a_text_box_is_placed_after_its_paragraph_once() {
    let ac = format!(
        r#"<w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:anchor><wp:docPr id="2" name="Text Box"/><a:graphic><a:graphicData><wps:wsp><wps:txbx>{b}</wps:txbx></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape><v:textbox>{b}</v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r>"#,
        b = txbx("boxed")
    );
    let m = md(&para(&(ac + &run("host"))));
    assert_eq!(m, "host\n\nboxed");
}

#[test]
fn a_vml_only_text_box_and_a_fallback_without_a_choice() {
    let vml = format!(
        r#"<w:r><w:pict><v:shape><v:textbox>{}</v:textbox></v:shape></w:pict></w:r>"#,
        txbx("vml box")
    );
    assert_eq!(md(&para(&(run("h") + &vml))), "h\n\nvml box");
    let only_fallback = format!(
        r#"<w:r><mc:AlternateContent><mc:Fallback><w:pict><v:textbox>{}</v:textbox></w:pict></mc:Fallback></mc:AlternateContent></w:r>"#,
        txbx("fb")
    );
    assert_eq!(md(&para(&only_fallback)), "fb");
}

#[test]
fn a_text_box_with_several_paragraphs_and_a_text_box_in_a_cell() {
    let two = r#"<w:txbxContent><w:p><w:r><w:t>l1</w:t></w:r></w:p><w:p><w:r><w:t>l2</w:t></w:r></w:p></w:txbxContent>"#;
    let vml =
        format!(r#"<w:r><w:pict><v:shape><v:textbox>{two}</v:textbox></v:shape></w:pict></w:r>"#);
    assert_eq!(md(&para(&vml)), "l1\n\nl2");
    let cell = format!("<w:tc>{}</w:tc>", para(&(run("c") + &vml)));
    assert!(md(&format!("<w:tbl><w:tr>{cell}</w:tr></w:tbl>")).starts_with("| c<br>l1<br>l2 |"));
}

#[test]
fn alternate_content_at_the_body_level_takes_a_choice_it_understands() {
    let ac = |requires: &str| {
        format!(
            r#"<mc:AlternateContent><mc:Choice Requires="{requires}">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
            p("choice"),
            p("fallback")
        )
    };
    for r in ["wps", "wpg", "w14", "w15 wp14", "a14 m", ""] {
        assert_eq!(md(&ac(r)), "choice", "{r:?}");
    }
    // A namespace the reader cannot read (chart extensions, ink, anything unknown): the Fallback.
    for r in ["x", "cx1", "p14", "wps cx1", "w14 x"] {
        assert_eq!(md(&ac(r)), "fallback", "{r:?}");
    }
}

#[test]
fn the_next_choice_is_taken_when_the_first_is_not_readable_and_no_fallback_means_nothing() {
    let two = format!(
        r#"<mc:AlternateContent><mc:Choice Requires="cx1">{}</mc:Choice><mc:Choice Requires="wps">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
        p("first"),
        p("second"),
        p("fallback")
    );
    assert_eq!(md(&two), "second");
    let none = format!(
        r#"{}<mc:AlternateContent><mc:Choice Requires="cx1">{}</mc:Choice></mc:AlternateContent>"#,
        p("before"),
        p("lost")
    );
    assert_eq!(md(&none), "before");
}

#[test]
fn content_controls_at_block_and_inline_level_and_other_wrappers() {
    let block = format!(
        "<w:sdt><w:sdtPr><w:alias w:val=\"x\"/></w:sdtPr><w:sdtContent>{}{}</w:sdtContent></w:sdt>",
        p("one"),
        p("two")
    );
    assert_eq!(md(&block), "one\n\ntwo");
    let inline = para(
        &(run("a ")
            + "<w:sdt><w:sdtPr/><w:sdtContent><w:r><w:t>inside</w:t></w:r></w:sdtContent></w:sdt>"
            + &run(" b")),
    );
    assert_eq!(md(&inline), "a inside b");
    let wrappers = para("<w:smartTag><w:r><w:t>s</w:t></w:r></w:smartTag><w:customXml><w:r><w:t>c</w:t></w:r></w:customXml><w:r><w:t>d</w:t></w:r>");
    assert_eq!(md(&wrappers), "scd");
    let outer = "<w:customXml><w:p><w:r><w:t>cx</w:t></w:r></w:p></w:customXml>".to_string()
        + "<w:smartTag><w:p><w:r><w:t>st</w:t></w:r></w:p></w:smartTag>";
    assert_eq!(md(&outer), "cx\n\nst");
}

#[test]
fn unknown_elements_are_ignored_not_walked() {
    let body = para(&(run("a") + "<w:foo><w:r><w:t>secret</w:t></w:r></w:foo>" + &run("b")));
    assert_eq!(md(&body), "ab");
}

// ---------------------------------------------------------------------------------------------
// tracked changes, comments, headers
// ---------------------------------------------------------------------------------------------

#[test]
fn tracked_insertions_are_shown_and_deletions_are_not() {
    let body = para(
        &(run("keep ")
            + r#"<w:ins w:id="1" w:author="a"><w:r><w:t xml:space="preserve">added </w:t></w:r></w:ins>"#
            + r#"<w:del w:id="2" w:author="a"><w:r><w:delText xml:space="preserve">removed </w:delText></w:r></w:del>"#
            + &run("end")),
    );
    assert_eq!(md(&body), "keep added end");
}

#[test]
fn moves_show_the_destination_only() {
    let body = para(
        &(r#"<w:moveFrom w:id="1"><w:r><w:t>from-here </w:t></w:r></w:moveFrom>"#.to_string()
            + &run("mid ")
            + r#"<w:moveTo w:id="2"><w:r><w:t>to-here</w:t></w:r></w:moveTo>"#),
    );
    assert_eq!(md(&body), "mid to-here");
}

#[test]
fn a_deleted_paragraph_mark_joins_the_next_paragraph() {
    let a = r#"<w:p><w:pPr><w:rPr><w:del w:id="1"/></w:rPr></w:pPr><w:r><w:t xml:space="preserve">joined </w:t></w:r></w:p>"#;
    assert_eq!(
        md(&(a.to_string() + &p("with next") + &p("after"))),
        "joined with next\n\nafter"
    );
    // the joined paragraph takes the following paragraph's role
    assert_eq!(
        md_styled(
            &(a.to_string() + &styled_p("Heading1", "title")),
            HEADING_STYLES
        ),
        "# joined title"
    );
    // nothing follows: it stands alone; a table follows: it stands before it
    assert_eq!(md(a), "joined");
    let t = "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";
    assert_eq!(md(&(a.to_string() + t)), "joined\n\n| c |\n| --- |");
}

#[test]
fn a_paragraph_that_was_entirely_deleted_leaves_nothing() {
    let a = r#"<w:p><w:pPr><w:rPr><w:del w:id="1"/></w:rPr></w:pPr><w:del w:id="2"><w:r><w:delText>gone</w:delText></w:r></w:del></w:p>"#;
    assert_eq!(md(&(p("a") + a + &p("b"))), "a\n\nb");
}

#[test]
fn comments_and_their_anchors_are_not_shown() {
    let body = para(
        &(run("text ")
            + r#"<w:commentRangeStart w:id="0"/>"#
            + &run("anchored")
            + r#"<w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r>"#),
    );
    let comments = format!(
        r#"<?xml version="1.0"?><w:comments {}><w:comment w:id="0" w:author="x"><w:p><w:r><w:t>SECRET COMMENT</w:t></w:r></w:p></w:comment></w:comments>"#,
        ns()
    );
    let d = conv(
        &Dx::new(&body)
            .rel("rIdC", "comments", "comments.xml", false)
            .part("word/comments.xml", comments.as_bytes()),
    );
    assert_eq!(d.markdown, "text anchored");
}

#[test]
fn headers_and_footers_are_not_shown() {
    let body = p("body")
        + r#"<w:sectPr><w:headerReference w:type="default" r:id="rIdH"/><w:footerReference w:type="default" r:id="rIdF"/></w:sectPr>"#;
    let part = |t: &str| {
        format!(
            r#"<?xml version="1.0"?><w:hdr {}><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:hdr>"#,
            ns()
        )
    };
    let d = conv(
        &Dx::new(&body)
            .rel("rIdH", "header", "header1.xml", false)
            .rel("rIdF", "footer", "footer1.xml", false)
            .part("word/header1.xml", part("RUNNING HEAD").as_bytes())
            .part("word/footer1.xml", part("RUNNING FOOT").as_bytes()),
    );
    assert_eq!(d.markdown, "body");
}

// ---------------------------------------------------------------------------------------------
// formulas
// ---------------------------------------------------------------------------------------------

thread_local! {
    static SEEN: RefCell<Vec<(String, bool)>> = const { RefCell::new(Vec::new()) };
}

fn fake_latex(fragment: &str, display: bool) -> Option<String> {
    SEEN.with(|s| s.borrow_mut().push((fragment.to_string(), display)));
    Some(format!("X_{}", u8::from(display)))
}

fn none_latex(_: &str, _: bool) -> Option<String> {
    None
}

fn dollar_latex(_: &str, _: bool) -> Option<String> {
    Some("a $ b".into())
}

fn omml(text: &str) -> String {
    format!("<m:oMath><m:r><m:t>{text}</m:t></m:r></m:oMath>")
}

fn with_math(f: fn(&str, bool) -> Option<String>) -> DocOptions {
    DocOptions {
        math: f,
        ..DocOptions::default()
    }
}

#[test]
fn an_inline_formula_is_dollar_latex_and_gets_its_raw_xml() {
    SEEN.with(|s| s.borrow_mut().clear());
    let d = conv_with(
        &Dx::new(&para(&(run("a ") + &omml("x") + &run(" b")))),
        &with_math(fake_latex),
    )
    .unwrap();
    assert_eq!(d.markdown, "a $X_0$ b");
    assert_eq!((d.math_total, d.math_latex), (1, 1));
    let seen = SEEN.with(|s| s.borrow().clone());
    assert_eq!(seen.len(), 1);
    assert!(!seen[0].1);
    assert!(
        seen[0].0.starts_with("<m:oMath xmlns:m=\""),
        "{}",
        seen[0].0
    );
    assert!(
        seen[0].0.contains("<m:r><m:t>x</m:t></m:r>") && seen[0].0.ends_with("</m:oMath>"),
        "{}",
        seen[0].0
    );
}

#[test]
fn a_display_formula_is_its_own_block() {
    let body = para(
        &(run("before") + &format!("<m:oMathPara>{}</m:oMathPara>", omml("y")) + &run("after")),
    );
    let d = conv_with(&Dx::new(&body), &with_math(fake_latex)).unwrap();
    assert_eq!(d.markdown, "before\n\n$$\nX_1\n$$\n\nafter");
    assert!(visible(&d.markdown).contains("$$ X_1 $$"));
}

#[test]
fn a_formula_alone_in_its_paragraph() {
    let body = para(&format!("<m:oMathPara>{}</m:oMathPara>", omml("y")));
    assert_eq!(
        conv_with(&Dx::new(&body), &with_math(fake_latex))
            .unwrap()
            .markdown,
        "$$\nX_1\n$$"
    );
    let inline_only = para(&omml("y"));
    assert_eq!(
        conv_with(&Dx::new(&inline_only), &with_math(fake_latex))
            .unwrap()
            .markdown,
        "$X_0$"
    );
}

#[test]
fn a_formula_that_cannot_be_converted_shows_its_characters() {
    let body = para(
        &(run("v = ") + &omml("a+b") + &format!("<m:oMathPara>{}</m:oMathPara>", omml("c*d"))),
    );
    let d = conv_with(&Dx::new(&body), &with_math(none_latex)).unwrap();
    assert_eq!(d.markdown, "v = a+b\n\nc\\*d");
    assert_eq!((d.math_total, d.math_latex), (2, 0));
}

#[test]
fn the_default_converter_is_the_real_omml_converter() {
    let d = conv(&Dx::new(&para(&omml("q"))));
    assert_eq!(d.markdown, "$q$");
    assert_eq!((d.math_total, d.math_latex), (1, 1));
}

#[test]
fn latex_with_a_dollar_or_an_oversized_formula_falls_back_to_characters() {
    let d = conv_with(&Dx::new(&para(&omml("m"))), &with_math(dollar_latex)).unwrap();
    assert_eq!(d.markdown, "m");
    let opts = DocOptions {
        max_math_xml: 20,
        math: fake_latex,
        ..DocOptions::default()
    };
    let d = conv_with(&Dx::new(&para(&omml("big"))), &opts).unwrap();
    assert_eq!(d.markdown, "big");
}

#[test]
fn a_formula_in_a_table_cell_is_inline_and_one_inside_a_deleted_run_is_not_counted() {
    let cell = format!(
        "<w:tc>{}</w:tc>",
        para(&format!("<m:oMathPara>{}</m:oMathPara>", omml("z")))
    );
    let d = conv_with(
        &Dx::new(&format!("<w:tbl><w:tr>{cell}</w:tr></w:tbl>")),
        &with_math(fake_latex),
    )
    .unwrap();
    assert!(d.markdown.starts_with("| $X_1$ |"), "{}", d.markdown);
    let del = para(&format!("<w:del>{}</w:del>", omml("gone")));
    let d = conv_with(&Dx::new(&del), &with_math(fake_latex)).unwrap();
    assert_eq!(d.markdown, "");
    assert_eq!(d.math_total, 0);
}

// ---------------------------------------------------------------------------------------------
// escaping: what the renderer shows is what the document said
// ---------------------------------------------------------------------------------------------

fn shown(text: &str) -> String {
    // The text as one paragraph, through the reader and konoma's renderer.
    let d = conv(&Dx::new(&p(&text
        .replace('&', "&amp;")
        .replace('<', "&lt;"))));
    visible(&d.markdown).replace('\u{200B}', "")
}

#[test]
fn inline_markup_characters_in_text_are_shown_literally() {
    for t in [
        "*star*",
        "**bold**",
        "a * b * c",
        "_under_",
        "snake_case_name",
        "__dunder__",
        "`code`",
        "``dbl``",
        "[link](http://example.com)",
        "![img](x.png)",
        "[^1]",
        "[x]",
        "[ ]",
        "<b>tag</b>",
        "<div>",
        "<!-- c -->",
        "$5",
        "$a$",
        "$$b$$",
        "a|b",
        "~~s~~",
        "~x~",
        "^up^",
        "back\\slash",
        "\\*",
        "&amp;",
        "&#35;",
        "&copy;",
        "a:b:c",
        ":smile:",
        "{#id}",
        "<http://x.com>",
        "x \\[a\\] y",
        "x \\(a\\) y",
        "50% off",
        "a & b",
        "(a) [b] {c}",
        "**",
        "*",
        "_",
        "``",
        "~~~",
        "[",
        "]",
        "![",
        "](",
        "<",
        ">",
    ] {
        let line = format!("before {t} after");
        assert_eq!(shown(&line), line, "{t:?}");
    }
}

#[test]
fn block_markup_at_the_start_of_a_paragraph_is_shown_literally() {
    for t in [
        "# heading",
        "## h2",
        "#hash",
        "- item",
        "+ item",
        "* item",
        "1. one",
        "1) one",
        "10. ten",
        "2023. in the year",
        "> quote",
        "=== ",
        "| a | b |",
        "```fence",
        "~~~",
        "- [ ] task",
        "[ ] box",
        "- - -",
        "<div>x</div>",
        "[x]: http://ref",
        "***bold*** start",
        "Term\\",
        "1.5 million",
        "3) c",
        "-5 degrees",
        "+1 plus",
    ] {
        assert_eq!(shown(t), t.trim_end(), "{t:?}");
    }
}

#[test]
fn a_paragraph_that_is_only_a_rule_looking_run_stays_text() {
    for t in ["---", "***", "___", "- - -", "* * *", "-----"] {
        assert_eq!(shown(t), t, "{t:?}");
    }
}

#[test]
fn leading_spaces_do_not_make_an_indented_code_block() {
    let m = md(&p("      indented text"));
    assert_eq!(m, "indented text");
}

#[test]
fn a_front_matter_looking_start_is_text() {
    let d = conv(&Dx::new(
        &(p("---") + &p("title: x") + &p("---") + &p("body")),
    ));
    let v = visible(&d.markdown);
    assert!(v.contains("title: x") && v.contains("body"), "{v}");
}

#[test]
fn table_cells_show_markup_characters_literally_apart_from_look_alikes() {
    for (t, want) in [
        ("*star*", "\u{2217}star\u{2217}"),
        ("a*b*c", "a\u{2217}b\u{2217}c"),
        ("`code`", "\u{02CB}code\u{02CB}"),
        ("~~s~~", "\u{223C}\u{223C}s\u{223C}\u{223C}"),
        ("[l](u)", "[l](u)"),
        ("[x]", "[x]"),
        ("a|b", "a|b"),
        ("<b>x</b>", "<b>x</b>"),
        ("<br>", "<br>"),
        ("[^1]", "[^1]"),
        ("$5 and $6", "$5 and $6"),
        ("2 * 3", "2 * 3"),
        ("*", "*"),
        ("under_score_x", "under_score_x"),
        ("back\\slash", "back\\slash"),
        ("# not heading", "# not heading"),
    ] {
        let cell = format!(
            "<w:tc>{}</w:tc>",
            p(&t.replace('&', "&amp;").replace('<', "&lt;"))
        );
        let d = conv(&Dx::new(&format!(
            "<w:tbl><w:tr><w:tc>{}</w:tc></w:tr><w:tr>{cell}</w:tr></w:tbl>",
            p("h")
        )));
        let lines = rendered(&d.markdown, 80);
        let row = lines
            .iter()
            .find(|l| l.starts_with('│') && !l.contains(" h "))
            .unwrap_or_else(|| panic!("{t}: {lines:?}"));
        let got = row
            .trim_matches(|c| c == '│' || c == ' ')
            .replace('\u{200B}', "");
        assert_eq!(got, want, "{t:?}\n{d:?}");
    }
}

#[test]
fn a_literal_html_tag_in_a_cell_is_not_converted_by_the_inline_html_pass() {
    let cell = format!(
        "<w:tc>{}</w:tc>",
        p("press &lt;kbd&gt;Ctrl&lt;/kbd&gt; x&lt;br&gt;y")
    );
    let d = conv(&Dx::new(&format!(
        "<w:tbl><w:tr><w:tc>{}</w:tc></w:tr><w:tr>{cell}</w:tr></w:tbl>",
        p("h")
    )));
    let lines = rendered(&d.markdown, 80).join("\n").replace('\u{200B}', "");
    assert!(
        lines.contains("<kbd>Ctrl</kbd>") && lines.contains("x<br>y"),
        "{lines}"
    );
}

#[test]
fn bold_text_with_markup_characters_inside() {
    let body = para(&runp("<w:b/>", "a*b and `c`"));
    let m = md(&body);
    assert_eq!(visible(&m), "a*b and `c`");
}

#[test]
fn text_in_a_heading_is_escaped_too() {
    let m = md_styled(&styled_p("Heading1", "C# and *x* and [y]"), HEADING_STYLES);
    assert_eq!(visible(&m).lines().next().unwrap(), "C# and *x* and [y]");
}

// ---------------------------------------------------------------------------------------------
// budgets
// ---------------------------------------------------------------------------------------------

fn many(n: usize) -> String {
    (0..n)
        .map(|i| p(&format!("paragraph number {i}")))
        .collect()
}

#[test]
fn the_byte_budget_cuts_at_a_paragraph_and_says_so() {
    let opts = DocOptions {
        max_markdown_bytes: 300,
        ..DocOptions::default()
    };
    let d = conv_with(&Dx::new(&many(100)), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.len() <= 300, "{}", d.markdown.len());
    assert!(d.markdown.ends_with(|c: char| c.is_ascii_digit()));
    assert!(d
        .markdown
        .starts_with("paragraph number 0\n\nparagraph number 1"));
    let last = d.markdown.rsplit("\n\n").next().unwrap();
    assert!(last.starts_with("paragraph number ") && last.len() >= "paragraph number 0".len());
}

#[test]
fn the_line_budget_counts_markdown_lines() {
    let opts = DocOptions {
        max_markdown_lines: 20,
        ..DocOptions::default()
    };
    let d = conv_with(&Dx::new(&many(100)), &opts).unwrap();
    assert!(d.truncated);
    assert!(
        d.markdown.lines().count() <= 20,
        "{}",
        d.markdown.lines().count()
    );
    assert!(d.markdown.lines().count() >= 10);
}

#[test]
fn footnotes_still_fit_when_the_body_filled_the_budget() {
    let opts = DocOptions {
        max_markdown_bytes: 2000,
        max_markdown_lines: 100,
        ..DocOptions::default()
    };
    let body = para(&(run("start") + &fn_ref(1))) + &many(500);
    let d = conv_with(&Dx::new(&body).footnotes(&note(1, "kept note")), &opts).unwrap();
    assert!(d.truncated);
    assert!(
        d.markdown.ends_with("[^1]: kept note"),
        "{}",
        &d.markdown[d.markdown.len().saturating_sub(80)..]
    );
    assert!(d.markdown.len() <= 2000);
}

#[test]
fn one_huge_paragraph_shows_its_beginning() {
    let opts = DocOptions {
        max_markdown_bytes: 1000,
        ..DocOptions::default()
    };
    let d = conv_with(&Dx::new(&p(&"word ".repeat(10_000))), &opts).unwrap();
    assert!(d.truncated);
    assert!(
        d.markdown.starts_with("word word") && d.markdown.len() <= 1000 && d.markdown.len() > 500,
        "{}",
        d.markdown.len()
    );
}

#[test]
fn a_block_over_the_node_budget_stops_the_conversion_but_keeps_what_came_before() {
    let opts = DocOptions {
        max_block_nodes: 30,
        ..DocOptions::default()
    };
    let big = para(&run("x").repeat(50));
    let d = conv_with(&Dx::new(&(p("kept") + &big + &p("never"))), &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.markdown, "kept");
}

#[test]
fn a_block_over_the_text_budget_is_refused_the_same_way() {
    let opts = DocOptions {
        max_block_text_bytes: 100,
        ..DocOptions::default()
    };
    let d = conv_with(&Dx::new(&(p("kept") + &p(&"y".repeat(500)))), &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.markdown, "kept");
}

#[test]
fn the_block_count_budget_stops_a_body_of_empty_paragraphs() {
    let opts = DocOptions {
        max_blocks: 10,
        ..DocOptions::default()
    };
    let body = "<w:p/>".repeat(1000) + &p("never reached");
    let d = conv_with(&Dx::new(&body), &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.markdown, "");
}

#[test]
fn cancellation_stops_the_conversion() {
    let dir = super::tests::tmp("docx_cancel");
    let path = super::tests::write(&dir, "c.docx", &Dx::new(&many(50)).bytes());
    let cancel = Cancel::new(|| true);
    let d = super::docx::load_document_cancellable(&path, &DocOptions::default(), Some(&cancel))
        .unwrap();
    assert!(d.truncated);
    assert_eq!(d.markdown, "");
    let never = Cancel::new(|| false);
    let d = super::docx::load_document_cancellable(&path, &DocOptions::default(), Some(&never))
        .unwrap();
    assert!(!d.truncated && d.markdown.lines().count() > 50);
}

#[test]
fn a_whole_document_that_fits_is_not_truncated() {
    let d: Document = conv(&Dx::new(&many(20)));
    assert!(!d.truncated);
    assert_eq!(d.markdown.matches("paragraph number").count(), 20);
}

// ---------------------------------------------------------------------------------------------
// the package
// ---------------------------------------------------------------------------------------------

fn load_bytes(name: &str, bytes: &[u8]) -> Result<Document, OfficeError> {
    let dir = super::tests::tmp("docx_pkg");
    let path = super::tests::write(&dir, name, bytes);
    super::docx::load_document(&path, &DocOptions::default())
}

#[test]
fn not_a_package_is_corrupt_empty_is_corrupt_and_a_workbook_is_unsupported() {
    assert!(matches!(
        load_bytes("a.docx", b"hello this is not a zip"),
        Err(OfficeError::Corrupt(_))
    ));
    assert!(matches!(
        load_bytes("a.docx", b""),
        Err(OfficeError::Corrupt(_))
    ));
    let xlsx = super::tests::deflated(&[("xl/workbook.xml", b"<workbook/>")]);
    assert_eq!(
        load_bytes("a.docx", &xlsx).unwrap_err(),
        OfficeError::Unsupported
    );
    let empty_zip = super::tests::deflated(&[("readme.txt", b"x")]);
    assert_eq!(
        load_bytes("a.docx", &empty_zip).unwrap_err(),
        OfficeError::Unsupported
    );
}

fn cfb_with(stream: &str) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    cf.create_stream(stream)
        .unwrap()
        .write_all(b"data")
        .unwrap();
    cf.flush().unwrap();
    cf.into_inner().into_inner()
}

#[test]
fn a_password_protected_document_is_encrypted_and_a_legacy_doc_is_unsupported() {
    assert_eq!(
        load_bytes("a.docx", &cfb_with("/EncryptedPackage")).unwrap_err(),
        OfficeError::Encrypted
    );
    assert_eq!(
        load_bytes("a.docx", &cfb_with("/EncryptionInfo")).unwrap_err(),
        OfficeError::Encrypted
    );
    assert_eq!(
        load_bytes("a.doc", &cfb_with("/WordDocument")).unwrap_err(),
        OfficeError::Unsupported
    );
    assert_eq!(
        load_bytes("a.docx", &cfb_with("/Workbook")).unwrap_err(),
        OfficeError::Unsupported
    );
}

#[test]
fn the_main_part_is_the_one_the_package_relationships_name() {
    let doc = Dx::new(&p("renamed")).document_xml();
    let root = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL_BASE}/officeDocument" Target="/word/main2.xml"/></Relationships>"#
    );
    let bytes = super::tests::deflated(&[
        ("_rels/.rels", root.as_bytes()),
        ("word/main2.xml", doc.as_bytes()),
    ]);
    assert_eq!(load_bytes("a.docx", &bytes).unwrap().markdown, "renamed");
}

#[test]
fn part_names_are_matched_without_regard_to_case_and_the_usual_name_is_the_fallback() {
    let doc = Dx::new(&p("fallback")).document_xml();
    let bytes = super::tests::deflated(&[("WORD/Document.XML", doc.as_bytes())]);
    assert_eq!(load_bytes("a.docx", &bytes).unwrap().markdown, "fallback");
}

#[test]
fn every_word_extension_is_read_the_same() {
    for ext in ["docx", "docm", "dotx", "dotm", "DOCX"] {
        assert_eq!(
            load_bytes(&format!("a.{ext}"), &Dx::new(&p("same")).bytes())
                .unwrap()
                .markdown,
            "same",
            "{ext}"
        );
    }
}

#[test]
fn a_document_with_no_body_is_empty() {
    let xml = format!(r#"<?xml version="1.0"?><w:document {}/>"#, ns());
    let bytes = super::tests::deflated(&[("word/document.xml", xml.as_bytes())]);
    let d = load_bytes("a.docx", &bytes).unwrap();
    assert_eq!(d.markdown, "");
    assert!(!d.truncated);
}

#[test]
fn damaged_xml_is_corrupt_deep_xml_is_refused_and_damaged_side_parts_cost_only_themselves() {
    let cut = format!(
        r#"<?xml version="1.0"?><w:document {}><w:body><w:p><w:r><w:t>x"#,
        ns()
    );
    let bytes = super::tests::deflated(&[("word/document.xml", cut.as_bytes())]);
    assert!(matches!(
        load_bytes("a.docx", &bytes),
        Err(OfficeError::Corrupt(_))
    ));

    let deep = format!(
        r#"<w:document {}><w:body>{}{}</w:body></w:document>"#,
        ns(),
        "<w:sdt>".repeat(300),
        "</w:sdt>".repeat(300)
    );
    let bytes = super::tests::deflated(&[("word/document.xml", deep.as_bytes())]);
    assert_eq!(
        load_bytes("a.docx", &bytes).unwrap_err(),
        OfficeError::TooLarge { what: "xml depth" }
    );

    // styles / numbering / notes / rels parts that are not XML: the document still reads
    let mut d =
        Dx::new(&(styled_p("Heading1", "t") + &num_p(1, 0, "n") + &para(&(run("x") + &fn_ref(1)))));
    d.styles = Some("<<<not xml".into());
    d.numbering = Some("<<<not xml".into());
    d.footnotes = Some("<<<not xml".into());
    let r = conv(&d);
    // (the heading still comes from the style id's own spelling)
    assert!(
        r.markdown.starts_with("# t\n\nn\n\nx[^1]"),
        "{}",
        r.markdown
    );
}

#[test]
fn a_package_over_the_limits_is_refused_before_reading() {
    let opts = DocOptions {
        limits: super::container::Limits {
            max_total_bytes: 1000,
            ..super::container::Limits::default()
        },
        ..DocOptions::default()
    };
    let big = Dx::new(&many(200));
    assert_eq!(
        conv_with(&big, &opts).unwrap_err(),
        OfficeError::TooLarge { what: "package" }
    );
    let opts = DocOptions {
        limits: super::container::Limits {
            max_file_bytes: 100,
            ..super::container::Limits::default()
        },
        ..DocOptions::default()
    };
    assert_eq!(
        conv_with(&big, &opts).unwrap_err(),
        OfficeError::TooLarge { what: "file" }
    );
}

#[test]
fn a_missing_file_is_an_io_error() {
    let r = super::docx::load_document(
        std::path::Path::new("/nonexistent/none.docx"),
        &DocOptions::default(),
    );
    assert!(matches!(r, Err(OfficeError::Io(_))));
}

#[test]
fn numbering_functions() {
    use super::docx_styles::format_number as f;
    assert_eq!(f("decimal", 12), "12");
    assert_eq!(f("lowerRoman", 14), "xiv");
    assert_eq!(f("upperRoman", 1994), "MCMXCIV");
    assert_eq!(f("upperRoman", 4000), "4000");
    assert_eq!(f("lowerLetter", 1), "a");
    assert_eq!(f("upperLetter", 27), "AA");
    assert_eq!(f("ideographDigital", 2025), "二〇二五");
    assert_eq!(f("japaneseCounting", 100), "百");
    assert_eq!(f("japaneseCounting", 111), "百十一");
    assert_eq!(f("japaneseCounting", 2024), "二千二十四");
    assert_eq!(f("japaneseLegal", 12), "拾弐");
    assert_eq!(f("aiueo", 46), "ン");
    assert_eq!(f("aiueo", 47), "アア");
    assert_eq!(f("iroha", 1), "イ");
    assert_eq!(f("decimalEnclosedCircle", 20), "⑳");
    assert_eq!(f("decimalEnclosedCircle", 21), "㉑");
    assert_eq!(f("decimalEnclosedCircle", 51), "51");
    assert_eq!(f("ordinal", 11), "11th");
    assert_eq!(f("ordinal", 22), "22nd");
    assert_eq!(f("decimalZero", 5), "05");
    assert_eq!(f("unheardOf", 5), "5");
    assert_eq!(f("lowerLetter", 0), "0");
}
