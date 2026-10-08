//! Presentations written by a real application (LibreOffice Impress, `testdata/office/gen/gen_pptx.py`):
//! `slides.pptx` and `slides-ja.pptx` (and the same decks as `.odp`).
//! **No file here was made by Microsoft PowerPoint**: PowerPoint-specific structures are covered by
//! the hand-written packages of `tests_pptx.rs`. The files are skipped when absent.

use std::path::PathBuf;

use super::docx::pptx::*;
use super::docx::*;
use super::tests_docx::rendered;
use super::tests_pptx::check_headings;
use super::OfficeError;
use crate::i18n::Lang;

fn file(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/office")
        .join(name);
    p.exists().then_some(p)
}

fn load(name: &str, lang: Lang) -> Option<Document> {
    let p = file(name)?;
    let opts = DocOptions {
        lang,
        ..DocOptions::default()
    };
    Some(load_presentation(&p, &opts).unwrap())
}

#[test]
fn the_english_deck() {
    let Some(d) = load("slides.pptx", Lang::En) else {
        return;
    };
    let key = d.images.first().map(|i| i.key.clone()).unwrap_or_default();
    let want = [
        "## Slide 1: Quarterly review",
        "Own content only",
        "## Slide 2: Agenda \\*and\\* more",
        "- Results\n  - Revenue grew\n    - By region\n- Plans\n- \\# not a heading",
        "## Slide 3: Compare",
        "- Left one\n- Left two\n- Right one\n- Right two",
        "## Slide 4: Numbers",
        "| Name | Qty | Note |\n| --- | --- | --- |\n| A\\|B | 1 | \u{2217}x\u{2217} |\n| tail | 2 |   |",
        "## Slide 5: Figure",
        &format!("![A gradient picture]({key})"),
        "Figure 1. The caption",
        "Grouped top",
        "Grouped bottom",
        "## Slide 6: Backup (hidden)",
        "- Hidden detail",
        "> **Notes**  \n> Say this aloud.  \n> \\# Remember the numbers",
        "## Slide 7: Links",
        "See [the site](https://example.com/slides) for more.",
    ]
    .join("\n\n");
    assert_eq!(d.markdown, want);
    assert!(!d.truncated);
    assert_eq!(d.images.len(), 1);
    assert_eq!(d.slides.len(), 7);
    assert!(d.slides[5].hidden);
    assert_eq!(d.slides[1].title, "Agenda *and* more");
    check_headings(&d);
}

#[test]
fn the_japanese_deck() {
    let Some(d) = load("slides-ja.pptx", Lang::Jp) else {
        return;
    };
    for want in [
        "## スライド 1: 四半期レビュー\n\n自作の内容のみ",
        "## スライド 2: 議題と \\*その他\\*",
        "- 結果\n  - 売上が伸びた\n    - 地域別\n- 計画\n- \\# 見出しではない",
        "- 左一\n- 左二\n- 右一\n- 右二",
        "| 名前 | 数量 | 備考 |",
        "![グラデーションの画像](office-img://",
        "図1 キャプション\n\nグループ上\n\nグループ下",
        "## スライド 6: 予備 (非表示)",
        "> **ノート**  \n> ここで話す。  \n> \\# 数字を覚えておく",
        "参照: [サイト](https://example.com/slides) を見てください。",
    ] {
        assert!(d.markdown.contains(want), "{want:?} in\n{}", d.markdown);
    }
    assert!(!d.truncated);
    // The headings of this deck are in Japanese: the same check on the English wording.
    check_headings(&Document {
        slides: d.slides.clone(),
        markdown: d
            .markdown
            .replace("## スライド", "## Slide")
            .replace(" (非表示)", ""),
        ..Document::default()
    });
}

#[test]
fn the_decks_render() {
    for (name, lang) in [("slides.pptx", Lang::En), ("slides-ja.pptx", Lang::Jp)] {
        let Some(d) = load(name, lang) else {
            continue;
        };
        let shown = rendered(&d.markdown, 100).join("\n");
        assert!(!shown.contains("**"), "{shown}");
        assert!(!shown.contains("## "), "{shown}");
        assert!(
            shown.contains("the site") || shown.contains("サイト"),
            "{shown}"
        );
    }
}

/// The OpenDocument version of a deck reads to the same Markdown as the PowerPoint one (the two
/// files were made from one description by LibreOffice, `gen_pptx.py`).
#[test]
fn the_opendocument_decks_read_like_the_powerpoint_ones() {
    for (odp, pptx, lang) in [
        ("slides.odp", "slides.pptx", Lang::En),
        ("slides-ja.odp", "slides-ja.pptx", Lang::Jp),
    ] {
        let (Some(o), Some(p)) = (load(odp, lang), load(pptx, lang)) else {
            continue;
        };
        // (Only the picture's file name differs: the two applications name it differently.)
        let unnamed = |m: &str| m.replace("100000000000005000000028919738D5.png", "image1.png");
        assert_eq!(unnamed(&o.markdown), p.markdown, "{odp}");
        assert_eq!(o.slides, p.slides, "{odp}");
        assert_eq!(o.images.len(), p.images.len(), "{odp}");
        assert_eq!(o.truncated, p.truncated, "{odp}");
        check_headings(&Document {
            slides: o.slides.clone(),
            markdown: o
                .markdown
                .replace("## スライド", "## Slide")
                .replace(" (非表示)", ""),
            ..Document::default()
        });
    }
}

#[test]
fn a_real_deck_survives_every_prefix_and_a_cancel() {
    let Some(p) = file("slides.pptx") else { return };
    let bytes = std::fs::read(&p).unwrap();
    let dir = super::tests::tmp("pptxrealprefix");
    for n in (0..bytes.len()).step_by(bytes.len() / 60 + 1) {
        let q = super::tests::write(&dir, "x.pptx", &bytes[..n]);
        if let Err(OfficeError::Corrupt(m)) = load_presentation(&q, &DocOptions::default()) {
            assert!(!m.starts_with("panic"), "{m}");
        }
    }
    let c = super::Cancel::new(|| true);
    let d = load_presentation_cancellable(&p, &DocOptions::default(), Some(&c)).unwrap();
    assert!(d.truncated);
}
