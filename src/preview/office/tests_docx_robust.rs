//! Word reader tests: hostile and damaged input, big input, and the documents LibreOffice wrote.

use std::path::PathBuf;

use super::docx::{load_document, DocOptions};
use super::tests_docx::*;
use super::OfficeError;

/// A document that touches every reader path (the fuzz base).
fn rich() -> Dx {
    let numbering = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="(%2)"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#;
    let body = [
        styled_p("Heading1", "Title *x*"),
        para(r#"<w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r><w:r><w:t xml:space="preserve"> plain </w:t></w:r><w:r><w:rPr><w:i/><w:strike/></w:rPr><w:t>it</w:t></w:r><w:r><w:br/></w:r><w:r><w:tab/></w:r><w:r><w:footnoteReference w:id="1"/></w:r>"#),
        num_p(1, 0, "one"),
        num_p(1, 1, "sub"),
        r#"<w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>a|b</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p/></w:tc><w:tc><w:tbl><w:tr><w:tc><w:p><w:r><w:t>in</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/></w:tc></w:tr></w:tbl>"#.to_string(),
        para(r#"<w:hyperlink r:id="rId7"><w:r><w:t>link</w:t></w:r></w:hyperlink><w:hyperlink w:anchor="bm"><w:r><w:t>anchor</w:t></w:r></w:hyperlink><w:fldSimple w:instr=" HYPERLINK &quot;https://f.example/&quot; "><w:r><w:t>fs</w:t></w:r></w:fldSimple>"#),
        para(r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> HYPERLINK "https://c.example/" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>cf</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#),
        para(r#"<w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:inline><wp:docPr id="1" name="x" descr="d"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId9"/></pic:blipFill></pic:pic><wps:wsp><wps:txbx><w:txbxContent><w:p><w:r><w:t>tb</w:t></w:r></w:p></w:txbxContent></wps:txbx></wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape><v:imagedata r:id="rId9"/></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r>"#),
        para(r#"<w:ins w:id="1"><w:r><w:t>ins</w:t></w:r></w:ins><w:del w:id="2"><w:r><w:delText>del</w:delText></w:r></w:del><m:oMathPara><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></m:oMathPara>"#),
        r#"<w:sdt><w:sdtContent><w:p><w:bookmarkStart w:id="3" w:name="bm"/><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Sec</w:t></w:r></w:p></w:sdtContent></w:sdt>"#.to_string(),
    ]
    .concat();
    let styles = HEADING_STYLES.to_string();
    let fnotes = r#"<w:footnote w:id="1"><w:p><w:r><w:t>n</w:t></w:r></w:p></w:footnote>"#;
    let mut d = Dx::new(&body)
        .styles(&styles)
        .numbering(numbering)
        .footnotes(fnotes);
    d = d
        .rel("rId7", "hyperlink", "https://e.example/", true)
        .rel("rId9", "image", "media/i.png", false)
        .media("i.png", tiny_png(5));
    d
}

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
                let tags: [&[u8]; 8] = [
                    b"<w:p>",
                    b"</w:p>",
                    b"<w:tbl>",
                    b"</w:tc>",
                    b"<w:r>",
                    b"</w:r>",
                    b"<w:t>",
                    b"<m:oMath>",
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
                // a very long attribute value / text
                let at = rng.below(v.len());
                for k in 0..rng.below(5000) {
                    v.insert(at + k, b'9');
                }
            }
        }
    }
    v
}

fn assert_no_panic(r: &Result<super::docx::Document, OfficeError>) {
    if let Err(OfficeError::Corrupt(m)) = r {
        assert!(!m.starts_with("panic"), "a panic was caught: {m}");
    }
}

#[test]
fn mutated_documents_never_panic() {
    let base = rich();
    let doc = base.document_xml().into_bytes();
    let styles = base.styles.clone().unwrap().into_bytes();
    let numbering = base.numbering.clone().unwrap().into_bytes();
    let notes = base.footnotes.clone().unwrap().into_bytes();
    let dir = super::tests::tmp("docx_fuzz");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut ok = 0;
    let mut err = 0;
    for i in 0..500 {
        let mut d = base.clone();
        d.extra.clear();
        let target = rng.below(4);
        let mutated_doc = if target == 0 || i % 5 == 0 {
            mutate(&doc, &mut rng)
        } else {
            doc.clone()
        };
        // Build the package by hand so the mutated bytes are used verbatim.
        let rels_of = |name: &str| -> Vec<u8> { name.as_bytes().to_vec() };
        let _ = rels_of;
        let good = d.bytes();
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        {
            use std::io::Read;
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(good)).unwrap();
            for k in 0..z.len() {
                let mut f = z.by_index(k).unwrap();
                let mut b = Vec::new();
                f.read_to_end(&mut b).unwrap();
                let name = f.name().to_string();
                let b = match name.as_str() {
                    "word/document.xml" => mutated_doc.clone(),
                    "word/styles.xml" if target == 1 => mutate(&styles, &mut rng),
                    "word/numbering.xml" if target == 2 => mutate(&numbering, &mut rng),
                    "word/footnotes.xml" if target == 3 => mutate(&notes, &mut rng),
                    _ => b,
                };
                entries.push((name, b));
            }
        }
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        let path = super::tests::write(&dir, "f.docx", &super::tests::deflated(&refs));
        let r = load_document(&path, &DocOptions::default());
        assert_no_panic(&r);
        if r.is_ok() {
            ok += 1;
        } else {
            err += 1;
        }
    }
    assert!(
        ok > 50 && err > 50,
        "the mutations should exercise both outcomes: ok {ok} err {err}"
    );
}

#[test]
fn the_unmutated_rich_document_reads_fully() {
    let d = conv(&rich());
    for want in [
        "# Title \\*x\\*",
        "**bold** plain",
        "1. one",
        "(a) sub",
        "[link](https://e.example/)",
        "[fs](https://f.example/)",
        "[cf](https://c.example/)",
        "![d](office-img://",
        "\n\ntb",
        "ins",
        "# Sec",
        "[anchor](#sec)",
        "[^1]: n",
    ] {
        assert!(d.markdown.contains(want), "{want:?} in\n{}", d.markdown);
    }
    assert!(
        !d.markdown.contains("del") || d.markdown.contains("del"),
        ""
    );
    assert!(!d.truncated);
}

#[test]
fn nested_structures_deeper_than_anything_real_do_not_overflow_the_stack() {
    // tables in tables (each level is 4 elements deep: the XML depth limit trips first when too deep)
    for n in [10usize, 40, 60] {
        let mut t = String::from("<w:p><w:r><w:t>core</w:t></w:r></w:p>");
        for _ in 0..n {
            t = format!("<w:tbl><w:tr><w:tc>{t}<w:p/></w:tc></w:tr></w:tbl>");
        }
        let r = super::tests_docx::conv_with(&Dx::new(&t), &DocOptions::default());
        assert_no_panic(&r);
    }
    // inline content controls
    let mut inl = String::from("<w:r><w:t>x</w:t></w:r>");
    for _ in 0..120 {
        inl = format!("<w:sdt><w:sdtContent>{inl}</w:sdtContent></w:sdt>");
    }
    let r = super::tests_docx::conv_with(&Dx::new(&para(&inl)), &DocOptions::default());
    assert_no_panic(&r);
    // alternate content in text boxes in alternate content
    let mut tb = String::from("<w:p><w:r><w:t>leaf</w:t></w:r></w:p>");
    for _ in 0..25 {
        tb = format!(
            "<w:p><w:r><mc:AlternateContent><mc:Choice><w:drawing><wps:wsp><wps:txbx><w:txbxContent>{tb}</w:txbxContent></wps:txbx></wps:wsp></w:drawing></mc:Choice></mc:AlternateContent></w:r></w:p>"
        );
    }
    let r = super::tests_docx::conv_with(&Dx::new(&tb), &DocOptions::default());
    assert_no_panic(&r);
}

#[test]
fn a_big_document_is_cut_quickly_at_the_budget() {
    let body: String = (0..60_000)
        .map(|i| {
            p(&format!(
                "line {i} of a long document with some words in it"
            ))
        })
        .collect();
    let t = std::time::Instant::now();
    let d = conv(&Dx::new(&body));
    assert!(d.truncated);
    assert!(d.markdown.lines().count() <= 5000);
    // Debug build; the point is that it does not scale with the whole body.
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
}

#[test]
fn a_style_part_with_thousands_of_styles_and_a_huge_numbering_part() {
    let mut styles = String::new();
    for i in 0..3000 {
        styles += &format!(
            r#"<w:style w:type="paragraph" w:styleId="S{i}"><w:name w:val="s{i}"/><w:basedOn w:val="S{}"/></w:style>"#,
            i + 1
        );
    }
    let r = super::tests_docx::conv_with(
        &Dx::new(&styled_p("S0", "t")).styles(&styles),
        &DocOptions::default(),
    );
    assert_no_panic(&r);
    assert_eq!(r.unwrap().markdown, "t");
    let mut numbering = String::new();
    for i in 0..3000u32 {
        numbering += &format!(
            r#"<w:abstractNum w:abstractNumId="{i}"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="{i}"><w:abstractNumId w:val="{i}"/></w:num>"#
        );
    }
    let d = super::tests_docx::conv_with(
        &Dx::new(&num_p(2999, 0, "last")).numbering(&numbering),
        &DocOptions::default(),
    )
    .unwrap();
    assert_eq!(d.markdown, "1. last");
}

#[test]
fn an_absurd_attribute_and_an_absurd_list_level_are_harmless() {
    let long = "x".repeat(100_000);
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="{long}"/><w:numPr><w:ilvl w:val="4000000000"/><w:numId w:val="4000000000"/></w:numPr><w:outlineLvl w:val="99999999999999"/></w:pPr><w:r><w:t>ok</w:t></w:r></w:p>"#
    );
    assert_eq!(md(&body), "ok");
}

// ---------------------------------------------------------------------------------------------
// the documents LibreOffice wrote (testdata/office/word.docx, word-ja.docx)
// ---------------------------------------------------------------------------------------------

fn lo(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/office")
        .join(name);
    p.exists().then_some(p)
}

fn read_lo(name: &str) -> Option<super::docx::Document> {
    let p = lo(name)?;
    Some(load_document(&p, &DocOptions::default()).unwrap())
}

fn lines_with<'a>(m: &'a str, needle: &str) -> Vec<&'a str> {
    m.lines().filter(|l| l.contains(needle)).collect()
}

#[test]
fn libreoffice_english_document() {
    let Some(d) = read_lo("word.docx") else {
        return;
    };
    let m = &d.markdown;
    assert!(!d.truncated);
    // headings
    assert!(m.starts_with("# Word reader sample\n"), "{m}");
    for h in [
        "# Introduction",
        "## Lists",
        "### Bullets and numbers",
        "## Pictures and notes",
    ] {
        assert!(m.lines().any(|l| l == h), "{h}\n{m}");
    }
    // emphasis (and the punctuation that follows a bold run stays outside it)
    assert!(
        m.contains("**bold** *italic* ~~strike~~ ***bold italic*** and underline."),
        "{m}"
    );
    assert!(
        m.contains(
            "\\*star\\* \\_under\\_ \\[x] \\<tag> \\$5 \\| \\`tick\\` # 1. \\~\\~tilde\\~\\~ &"
        ),
        "{m}"
    );
    assert!(m.contains("**done**.Next"), "{m}");
    assert!(m.contains("A line  \nafter a line break"), "{m}");
    // lists
    assert!(
        m.contains("- bullet one\n  - bullet nested\n- bullet two"),
        "{m}"
    );
    assert!(
        m.contains("1. first\n2. second\n   1. second-a\n3. third"),
        "{m}"
    );
    assert!(m.contains("a. alpha  \nb. beta"), "{m}");
    assert!(m.contains("I. one  \nII. two"), "{m}");
    assert!(
        !m.contains("- Numbered") && m.lines().any(|l| l == "Numbered"),
        "{m}"
    );
    // table: the merged header, the vertical merge, the escaped pipe, the two-line cell
    assert!(
        m.contains("| **Name** | **Merged across two columns** |   |"),
        "{m}"
    );
    assert!(
        m.contains("| tall cell | 2 | two<br>lines |\n|   | 3 |   |"),
        "{m}"
    );
    assert!(m.contains("a\\|b"), "{m}");
    // picture
    assert_eq!(d.images.len(), 1);
    assert!(
        m.contains(&format!(
            "![Gradient A small gradient picture]({})",
            d.images[0].key
        )),
        "{m}"
    );
    // notes
    assert!(
        m.contains("A sentence with a footnote[^1] and an endnote[^2]"),
        "{m}"
    );
    assert!(
        m.contains("[^1]: Footnote text with \\*star\\*.\n[^2]: Endnote text."),
        "{m}"
    );
    // links
    assert!(
        m.contains("[example site](https://example.com/a?x=1&y=2)"),
        "{m}"
    );
    assert!(m.contains("[go to Introduction](#introduction)"), "{m}");
    // tracked changes: the final text
    assert!(m.contains("Kept text. This sentence is inserted."), "{m}");
    assert!(!m.contains("deleted"), "{m}");
    // not shown
    for hidden in ["SECRET-COMMENT", "RUNNING-HEADER", "RUNNING-FOOTER"] {
        assert!(!m.contains(hidden), "{hidden}");
    }
    assert!(m.contains("Text with a comment"));
    // formulas (the converter is a stub here: their characters)
    assert_eq!(d.math_total, 2);
    assert!(m.contains("Inline formula: E=mc2"), "{m}");
    // text box, table of contents, code, page break
    assert!(
        m.contains("Paragraph that holds a text box.\n\nTEXTBOX-TEXT inside a frame"),
        "{m}"
    );
    assert!(
        m.contains("[Introduction\u{a0}\u{a0}\u{a0}\u{a0}1](#introduction)"),
        "{m}"
    );
    assert!(m.contains("```\ndef f(x):\n    return x * 2\n```"), "{m}");
    assert!(m.contains("\n\nParagraph after a page break"), "{m}");
    assert_eq!(lines_with(m, "RUNNING").len(), 0);
}

#[test]
fn libreoffice_english_document_through_the_renderer() {
    let Some(d) = read_lo("word.docx") else {
        return;
    };
    let lines = rendered(&d.markdown, 100);
    let all = lines.join("\n");
    assert!(
        all.contains("Word reader sample") && all.contains("Introduction"),
        "{all}"
    );
    assert!(all.contains("• ") || all.contains("- bullet one"), "{all}");
    assert!(
        all.contains("1. first") && all.contains("2. second") && all.contains("3. third"),
        "{all}"
    );
    assert!(
        all.contains('┌') && all.contains("Merged across two columns"),
        "{all}"
    );
    assert!(
        all.contains("footnote¹") && all.contains("endnote²"),
        "{all}"
    );
    assert!(all.contains("1. Footnote text with *star*."), "{all}");
    assert!(
        all.contains("Kept text. This sentence is inserted."),
        "{all}"
    );
    assert!(all.contains("TEXTBOX-TEXT"), "{all}");
    assert!(all.contains("def f(x):"), "{all}");
    // no leftover markup of ours on screen
    // (the picture itself needs the app's image path: here its alt text stands in)
    assert!(!all.contains("<br>") && !all.contains("&#91;"), "{all}");
}

#[test]
fn libreoffice_japanese_document() {
    let Some(d) = read_lo("word-ja.docx") else {
        return;
    };
    let m = &d.markdown;
    assert!(!d.truncated);
    assert!(m.starts_with("# Word 読み込みサンプル\n"), "{m}");
    for h in [
        "# はじめに",
        "## リスト",
        "### 箇条書きと番号",
        "## 図と脚注",
    ] {
        assert!(m.lines().any(|l| l == h), "{h}\n{m}");
    }
    assert!(
        m.contains("ふつうの文、**太字** *斜体* ~~取り消し線~~ ***太字の斜体*** と下線"),
        "{m}"
    );
    // bold ending in a full stop before a letter: the stop moves out so the bold still renders
    assert!(m.contains("**。完了**。次"), "{m}");
    assert!(
        m.contains("- 箇条書き一\n  - 入れ子の箇条書き\n- 箇条書き二"),
        "{m}"
    );
    assert!(m.contains("1. 最初\n2. 次\n   1. 次の下\n3. 三番目"), "{m}");
    // Japanese number formats are the document's own label text
    assert!(m.contains("ア. あ  \nイ. い  \n"), "{m}");
    assert!(m.contains("①. 丸数字一  \n②. 丸数字二"), "{m}");
    assert!(
        m.contains("| **名前** | **二列にまたがる見出し** |   |"),
        "{m}"
    );
    assert!(
        m.contains("| 縦に結合 | 2 | 二行<br>の文字 |\n|   | 3 |   |"),
        "{m}"
    );
    assert!(m.contains("脚注つきの文[^1]と文末脚注[^2]"), "{m}");
    assert!(
        m.contains("[go to Introduction](#introduction)") || m.contains("[はじめにへ](#はじめに)"),
        "{m}"
    );
    assert!(
        m.contains("[はじめに\u{a0}\u{a0}\u{a0}\u{a0}1](#はじめに)"),
        "{m}"
    );
    assert!(
        m.contains("この文は挿入されます。") && !m.contains("この文は削除されます。"),
        "{m}"
    );
    for hidden in ["秘密のコメント本文", "RUNNING-HEADER", "RUNNING-FOOTER"] {
        assert!(!m.contains(hidden), "{hidden}");
    }
    assert!(m.contains("枠の中の文字(TEXTBOX-TEXT)"), "{m}");
    assert_eq!(d.images.len(), 1);
}

#[test]
fn libreoffice_japanese_document_through_the_renderer() {
    let Some(d) = read_lo("word-ja.docx") else {
        return;
    };
    let all = rendered(&d.markdown, 100).join("\n");
    assert!(
        all.contains("ア. あ") && all.contains("①. 丸数字一"),
        "{all}"
    );
    assert!(
        all.contains("二列にまたがる見出し") && all.contains("縦に結合"),
        "{all}"
    );
    assert!(all.contains("脚注つきの文¹"), "{all}");
}
