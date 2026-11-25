//! What the XML readers do with bytes that are not valid UTF-8, and with parts that are not
//! UTF-8 at all (UTF-16, Latin-1). quick-xml 0.42 refuses invalid UTF-8 where 0.41 passed the bytes
//! through and konoma decoded them lossily; every reader is built on `LossyUtf8` (see
//! `lossy_utf8.rs`) so that a part with one stray byte is still shown, with U+FFFD where the byte
//! was.

use std::io::{Cursor, Read};

use super::chart_xml::{parse_chart, ChartEnv};
use super::docx::pptx::load_presentation;
use super::docx::*;
use super::fmt_ods;
use super::fmt_ods::Styles;
use super::tests::load;
use super::tests::{deflated, tmp, write};
use super::tests_docx::Dx;
use super::tests_odp as od;
use super::tests_ods::content_xml as ods_content;
use super::tests_odt::Ox;
use super::tests_pptx::{pa, sl, tb, Px, I};

const M: &str = "ZQZQZQZQ";

/// The text a marker is replaced with, per variant.
const VARIANTS: &[(&str, &[u8])] = &[
    ("ok", b"fine"),
    ("stray", b"a\xFFb"),
    ("cut", b"a\xE3\x81"),
    ("latin1", b"caf\xE9"),
    ("multi", b"x\xF0\x9F\x98\xFFy\xC0\xAFz"),
];

fn replace(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

/// Rewrites every entry of a zip with `f(name, bytes)`.
fn mutate(pkg: &[u8], f: impl Fn(&str, Vec<u8>) -> Vec<u8>) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(Cursor::new(pkg.to_vec())).unwrap();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..z.len() {
        let mut e = z.by_index(i).unwrap();
        let mut b = Vec::new();
        e.read_to_end(&mut b).unwrap();
        let name = String::from_utf8_lossy(e.name_raw()).into_owned();
        entries.push((name.clone(), f(&name, b)));
    }
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    deflated(&refs)
}

fn with_text(pkg: &[u8], text: &[u8]) -> Vec<u8> {
    mutate(pkg, |_, b| replace(&b, M.as_bytes(), text))
}

/// The part `which` written as UTF-16 (little endian, BOM) with a matching declaration.
fn utf16(pkg: &[u8], which: &str) -> Vec<u8> {
    mutate(pkg, |name, b| {
        if name != which {
            return replace(&b, M.as_bytes(), b"hello");
        }
        let s = String::from_utf8(replace(&b, M.as_bytes(), "héllo 日本".as_bytes())).unwrap();
        let s = s.replace("encoding=\"UTF-8\"", "encoding=\"UTF-16\"");
        let mut out = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out
    })
}

/// The part `which` declared as ISO-8859-1 and written so (an `é` is the single byte E9).
fn latin1(pkg: &[u8], which: &str) -> Vec<u8> {
    mutate(pkg, |name, b| {
        if name != which {
            return replace(&b, M.as_bytes(), b"hello");
        }
        let s = String::from_utf8(replace(&b, M.as_bytes(), "héllo".as_bytes())).unwrap();
        let s = s.replace("encoding=\"UTF-8\"", "encoding=\"ISO-8859-1\"");
        s.chars().map(|c| c as u32 as u8).collect()
    })
}

fn doc_of(bytes: &[u8], ext: &str) -> String {
    let dir = tmp("xmlbytes");
    let p = write(&dir, &format!("t.{ext}"), bytes);
    let r = match ext {
        "docx" | "odt" => load_document(&p, &DocOptions::default()),
        _ => load_presentation(&p, &DocOptions::default()),
    };
    match r {
        Ok(d) => format!("OK {:?}", d.markdown),
        Err(e) => format!("ERR {e:?}"),
    }
}

fn book_of(bytes: &[u8], ext: &str) -> String {
    let dir = tmp("xmlbytes_book");
    let p = write(&dir, &format!("t.{ext}"), bytes);
    match load(&p) {
        Ok(wb) => format!("OK {:?}", wb.sheets),
        Err(e) => format!("ERR {e:?}"),
    }
}

fn docx_pkg() -> Vec<u8> {
    Dx::new(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="H1"/></w:pPr><w:r><w:t>{M}</w:t></w:r></w:p><w:p><w:r><w:t>{M}&amp;&#233;</w:t></w:r></w:p>"#
    ))
    .styles(&format!(
        r#"<w:style w:type="paragraph" w:styleId="H1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="X"><w:name w:val="{M}"/></w:style>"#
    ))
    .footnotes(&format!(
        r#"<w:footnote w:id="1"><w:p><w:r><w:t>{M}</w:t></w:r></w:p></w:footnote>"#
    ))
    .numbering(&format!(
        r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="{M}"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#
    ))
    .bytes()
}

fn xlsx_pkg() -> Vec<u8> {
    Pkg::new()
        .styles(super::tests::styles_xml(&[], &[0, 14]))
        .shared(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><t>{M}</t></si></sst>"#
        ))
        .sheet(
            &format!("S{M}"),
            "visible",
            super::tests::sheet_xml(
                &format!(
                    r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>{M}</t></is></c><c r="C1"><v>12.5</v></c><c r="D1" t="str"><f>1</f><v>{M}</v></c></row><row r="2"><c r="A2" t="e"><v>#N/A</v></c><c r="B2"><v>7</v></c></row>"#
                ),
                "",
            ),
        )
        .bytes()
}

use super::tests::Pkg;

fn pptx_pkg() -> Vec<u8> {
    Px::new(vec![sl(&tb(I, I, 4 * I, I, &pa(M)))]).bytes()
}

fn odt_pkg() -> Vec<u8> {
    Ox::new(&format!(
        r#"<text:p>{M}</text:p><text:p><text:a xlink:href="{M}">link</text:a></text:p>"#
    ))
    .bytes()
}

fn odp_pkg() -> Vec<u8> {
    od::Op::new(&od::page(&od::title(M))).bytes()
}

fn ods_content_with(text: &[u8]) -> Vec<u8> {
    let auto = format!(
        r#"<number:number-style style:name="N{M}"><number:number number:decimal-places="2" number:min-integer-digits="1"/></number:number-style><style:style style:name="c" style:family="table-cell" style:data-style-name="N{M}"/>"#
    );
    replace(ods_content(&auto, "").as_bytes(), M.as_bytes(), text)
}

fn chart_xml() -> Vec<u8> {
    format!(
        r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:plotArea><c:barChart><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:f>x</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>{M}</c:v></c:pt></c:strCache></c:strRef></c:tx><c:cat><c:strLit><c:ptCount val="1"/><c:pt idx="0"><c:v>{M}</c:v></c:pt></c:strLit></c:cat><c:val><c:numLit><c:ptCount val="1"/><c:pt idx="0"><c:v>2</c:v></c:pt></c:numLit></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#
    )
    .into_bytes()
}

fn chart_of(xml: &[u8]) -> String {
    fn resolver(_: &super::docx_xml::Node) -> Option<super::slide_draw::Rgba> {
        None
    }
    let env = ChartEnv {
        resolve_color: &resolver,
        palette: &[],
        minor_font: None,
        major_font: None,
    };
    match parse_chart(xml, &env) {
        Ok(c) => format!("OK {:?}", c.model),
        Err(e) => format!("ERR {e:?}"),
    }
}

fn lossy(text: &[u8]) -> String {
    String::from_utf8_lossy(text).into_owned()
}

/// Every reader keeps a part that holds a stray byte, with U+FFFD where the byte was, in text
/// and in attribute values (a sheet name, a style name, a link target): the result is what quick-xml
/// 0.41 gave. Without `LossyUtf8` the whole part fails with an "invalid utf-8" error.
#[test]
fn a_stray_byte_is_a_replacement_character_in_every_reader() {
    for (vn, text) in VARIANTS.iter().filter(|(n, _)| *n != "ok") {
        let l = lossy(text);
        assert!(l.contains('\u{FFFD}'), "{vn}");
        assert_eq!(
            doc_of(&with_text(&docx_pkg(), text), "docx"),
            format!("OK {:?}", format!("# {l}\n\n{l}&é")),
            "docx {vn}"
        );
        assert_eq!(
            doc_of(&with_text(&pptx_pkg(), text), "pptx"),
            format!("OK {:?}", format!("## Slide 1\n\n{l}")),
            "pptx {vn}"
        );
        assert_eq!(
            doc_of(&with_text(&odt_pkg(), text), "odt"),
            format!("OK {:?}", format!("{l}\n\nlink")),
            "odt {vn}"
        );
        assert_eq!(
            doc_of(&with_text(&odp_pkg(), text), "odp"),
            format!("OK {:?}", format!("## Slide 1: {l}")),
            "odp {vn}"
        );
        let book = book_of(&with_text(&xlsx_pkg(), text), "xlsx");
        assert!(book.starts_with("OK "), "xlsx {vn}: {book}");
        assert!(
            book.contains(&format!("name: {:?}", format!("S{l}"))),
            "xlsx {vn}: {book}"
        );
        assert_eq!(
            book.matches(&format!("Text({l:?})")).count(),
            3,
            "xlsx {vn}: shared string, inline string and formula text"
        );
        let chart = chart_of(&replace(&chart_xml(), M.as_bytes(), text));
        assert!(chart.starts_with("OK "), "chart {vn}: {chart}");
        assert!(
            chart.contains(&format!("text: {l:?}")),
            "chart {vn}: {chart}"
        );
    }
}

/// A single part in another encoding is read the way quick-xml 0.41 read it (its bytes are not
/// UTF-8, so what is not valid is a replacement character) and never fails the document.
#[test]
fn a_part_declared_latin1_is_read_with_replacement_characters() {
    let h = "h\u{FFFD}llo";
    assert_eq!(
        doc_of(&latin1(&docx_pkg(), "word/document.xml"), "docx"),
        format!("OK {:?}", format!("# {h}\n\n{h}&é"))
    );
    assert_eq!(
        doc_of(&latin1(&pptx_pkg(), "ppt/slides/slide1.xml"), "pptx"),
        format!("OK {:?}", format!("## Slide 1\n\n{h}"))
    );
    assert_eq!(
        doc_of(&latin1(&odt_pkg(), "content.xml"), "odt"),
        format!("OK {:?}", format!("{h}\n\nlink"))
    );
    assert_eq!(
        doc_of(&latin1(&odp_pkg(), "content.xml"), "odp"),
        format!("OK {:?}", format!("## Slide 1: {h}"))
    );
    for part in ["xl/sharedStrings.xml", "xl/workbook.xml"] {
        let book = book_of(&latin1(&xlsx_pkg(), part), "xlsx");
        assert!(book.starts_with("OK "), "{part}: {book}");
        assert!(
            book.contains(&format!("Text({h:?})")) || book.contains(&format!("name: \"S{h}\"")),
            "{part}: {book}"
        );
    }
}

/// UTF-16 parts get the answer they had before the upgrade (unsupported / no text), and no panic.
///
/// quick-xml's `encoding` feature IS on in this build (calamine enables it and cargo unifies the
/// features of the shared 0.42 copy; `cargo tree -i quick-xml@0.42.0 -e features`), but it does not
/// make `Reader::from_reader` decode anything: in 0.42 only the separate `DecodingReader` adapter
/// transcodes, and konoma never uses it. With the feature on, `Reader` only records the encoding it
/// detects from a BOM (`detect_encoding`); with it off it only strips a UTF-8 BOM. Either way a
/// UTF-16 part reaches the reader as bytes that are not UTF-8: before `LossyUtf8` existed this
/// build failed on them ("invalid utf-8", seen by bypassing the adapter), and with it the BOM
/// and the NUL-interleaved text come out as replacement characters / NULs exactly as with 0.41,
/// whose own copy had no `encoding` feature at all. So these expectations do not depend on the
/// feature being on or off; they depend on nothing but `LossyUtf8` sitting in front of the reader.
#[test]
fn a_utf16_part_gets_the_answer_it_had_before() {
    for (pkg, ext, part) in [
        (docx_pkg(), "docx", "word/document.xml"),
        (odt_pkg(), "odt", "content.xml"),
        (odp_pkg(), "odp", "content.xml"),
    ] {
        assert_eq!(doc_of(&utf16(&pkg, part), ext), "ERR Unsupported", "{ext}");
    }
    assert_eq!(
        doc_of(&utf16(&pptx_pkg(), "ppt/slides/slide1.xml"), "pptx"),
        format!("OK {:?}", "## Slide 1")
    );
    let sheet = book_of(&utf16(&xlsx_pkg(), "xl/worksheets/sheet1.xml"), "xlsx");
    assert!(sheet.contains("nrows: 0, ncols: 0"), "{sheet}");
    assert_eq!(
        book_of(&utf16(&xlsx_pkg(), "xl/workbook.xml"), "xlsx"),
        "OK []"
    );
}

/// Texts whose reading depends on the reader's own rules (line ends, entities, character
/// references, CDATA, a byte order mark, a DTD).
const TEXTS: &[(&str, &str)] = &[
    ("crlf", "a\r\nb\rc"),
    ("char refs", "a&#10;b&#x41;&#233;&#0;&#xD800;&#x110000;"),
    ("entities", "&amp;&lt;&gt;&quot;&apos;"),
    ("undefined", "x&nope;y&#;z&;w"),
    ("cdata", "<![CDATA[p\r\nq<>&]]>r"),
    ("amp bare", "a & b &"),
];

fn entity_cases() -> Vec<String> {
    let mut lines = Vec::new();
    for (n, t) in TEXTS {
        let d = Dx::new(&format!(
            r#"<w:p><w:r><w:t xml:space="preserve">{t}</w:t></w:r></w:p>"#
        ))
        .bytes();
        lines.push(format!("docx|{n}|{}", doc_of(&d, "docx")));
        let o = Ox::new(&format!(r#"<text:p>{t}</text:p>"#)).bytes();
        lines.push(format!("odt|{n}|{}", doc_of(&o, "odt")));
        let p = Px::new(vec![sl(&tb(I, I, 4 * I, I, &pa(t)))]).bytes();
        lines.push(format!("pptx|{n}|{}", doc_of(&p, "pptx")));
        let x = Pkg::new()
            .sheet(
                "S",
                "visible",
                super::tests::sheet_xml(
                    &format!(r#"<row r="1"><c r="A1" t="inlineStr"><is><t>{t}</t></is></c></row>"#),
                    "",
                ),
            )
            .bytes();
        lines.push(format!("xlsx|{n}|{}", book_of(&x, "xlsx")));
    }
    // Attribute values.
    for (n, a) in [
        ("attr nl", "a&#10;b&#9;c"),
        ("attr undefined", "a&nope;b"),
        ("attr crlf", "a\r\nb"),
    ] {
        let o = Ox::new(&format!(
            r#"<text:p><text:a xlink:href="{a}">l</text:a></text:p>"#
        ))
        .bytes();
        lines.push(format!("odt|{n}|{}", doc_of(&o, "odt")));
        let x = Pkg::new()
            .sheet(
                &format!("S{a}"),
                "visible",
                super::tests::sheet_xml(r#"<row r="1"><c r="A1"><v>1</v></c></row>"#, ""),
            )
            .bytes();
        lines.push(format!("xlsx|{n}|{}", book_of(&x, "xlsx")));
    }
    // A UTF-8 byte order mark in front of the part, and a DTD with an entity.
    let bom = |pkg: Vec<u8>, part: &'static str| {
        mutate(&pkg, move |name, b| {
            if name == part {
                let mut v = vec![0xEF, 0xBB, 0xBF];
                v.extend(b);
                v
            } else {
                b
            }
        })
    };
    lines.push(format!(
        "docx|bom|{}",
        doc_of(
            &bom(
                Dx::new("<w:p><w:r><w:t>hi</w:t></w:r></w:p>").bytes(),
                "word/document.xml"
            ),
            "docx"
        )
    ));
    lines.push(format!(
        "odt|bom|{}",
        doc_of(
            &bom(Ox::new("<text:p>hi</text:p>").bytes(), "content.xml"),
            "odt"
        )
    ));
    for (n, dtd) in [
        ("dtd", r#"<!DOCTYPE d [<!ENTITY e "EXPANDED">]>"#),
        (
            "dtd bomb",
            r#"<!DOCTYPE d [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">]>"#,
        ),
    ] {
        let d = Dx::new("<w:p><w:r><w:t>x&e;y&b;z</w:t></w:r></w:p>").bytes();
        let d = mutate(&d, |name, b| {
            if name == "word/document.xml" {
                let s = String::from_utf8(b).unwrap();
                s.replacen("?><w:document", &format!("?>{dtd}<w:document"), 1)
                    .into_bytes()
            } else {
                b
            }
        });
        lines.push(format!("docx|{n}|{}", doc_of(&d, "docx")));
    }
    lines
}

/// What the readers make of line ends, references, CDATA, a byte order mark and a DTD stays what
/// quick-xml 0.41 gave (the expected values were taken from a 0.41 build). **A DTD-defined entity
/// is never expanded**: `&e;` stays text, so there is no billion-laughs path.
#[test]
fn references_line_ends_and_dtd_entities_read_as_before() {
    let lines = entity_cases();
    let get = |k: &str| -> &str {
        lines
            .iter()
            .find_map(|l| l.strip_prefix(&format!("{k}|")))
            .unwrap_or_else(|| panic!("no case {k}"))
    };
    for k in ["docx|dtd", "docx|dtd bomb"] {
        assert_eq!(get(k), r#"OK "x\\&e;y\\&b;z""#, "{k}");
        assert!(
            !get(k).contains("EXPANDED") && !get(k).contains("aaaa"),
            "{k}"
        );
    }
    assert_eq!(get("docx|crlf"), r#"OK "a b c""#);
    assert_eq!(
        get("docx|char refs"),
        r#"OK "a bAé\\&#0;\\&#xD800;\\&#x110000;""#
    );
    assert_eq!(get("docx|entities"), r#"OK "&\\<>\"'""#);
    assert_eq!(get("docx|undefined"), r#"OK "x\\&nope;y\\&#;z&;w""#);
    assert_eq!(get("docx|cdata"), r#"OK "p  q\\<>&r""#);
    assert_eq!(get("odt|cdata"), r#"OK "p q\\<>&r""#);
    assert_eq!(get("docx|bom"), r#"OK "hi""#);
    assert_eq!(get("odt|bom"), r#"OK "hi""#);
    assert!(get("docx|amp bare").starts_with("ERR Corrupt(\"xml: ill-formed document"));
    assert!(get("xlsx|attr nl").contains(r#"name: "Sa\nb\tc""#));
    assert!(get("xlsx|attr crlf").contains(r#"name: "Sa b""#));
    assert!(get("xlsx|attr undefined").contains(r#"name: "Sa&nope;b""#));
    assert!(get("xlsx|cdata").contains(r#"Text("p\nq<>&r")"#));
}

/// The ods format pass (`fmt_ods`) reads `content.xml` itself, after `calamine` has: a stray byte in
/// a style name must not fail it either.
#[test]
fn the_ods_format_pass_survives_a_stray_byte() {
    for (vn, text) in VARIANTS {
        let xml = ods_content_with(text);
        let mut styles = Styles::default();
        let r = fmt_ods::parse_styles(xml.as_slice(), &mut styles, true);
        assert!(r.is_ok(), "{vn}: {r:?}");
    }
}
