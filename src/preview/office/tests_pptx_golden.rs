//! The text Markdown of the decks in the repository must not change when the drawing side of the
//! PowerPoint reader changes: the goldens in `testdata/office/golden/` were written by the reader
//! as it was before the drawing model was added. `KONOMA_WRITE_GOLDEN=1 cargo test -- --ignored
//! write_goldens` rewrites them (only to be done on purpose).

use std::path::PathBuf;

use super::docx::pptx::load_presentation;
use super::docx::DocOptions;
use crate::i18n::Lang;

const DECKS: &[(&str, &str, Lang)] = &[
    ("testdata/office/slides.pptx", "slides.en.md", Lang::En),
    ("testdata/office/slides.pptx", "slides.jp.md", Lang::Jp),
    (
        "testdata/office/slides-ja.pptx",
        "slides-ja.en.md",
        Lang::En,
    ),
    (
        "testdata/office/slides-ja.pptx",
        "slides-ja.jp.md",
        Lang::Jp,
    ),
    ("samples/sample.pptx", "sample.en.md", Lang::En),
    ("samples/sample.ja.pptx", "sample.ja.jp.md", Lang::Jp),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn markdown(deck: &str, lang: Lang) -> String {
    let opts = DocOptions {
        lang,
        ..DocOptions::default()
    };
    load_presentation(&root().join(deck), &opts)
        .unwrap()
        .markdown
}

#[test]
fn the_text_markdown_of_every_deck_is_unchanged() {
    for (deck, golden, lang) in DECKS {
        let path = root().join("testdata/office/golden").join(golden);
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("golden {}: {e}", path.display()));
        assert_eq!(markdown(deck, *lang), want, "{deck} ({golden})");
    }
}

#[test]
#[ignore]
fn write_goldens() {
    if std::env::var_os("KONOMA_WRITE_GOLDEN").is_none() {
        return;
    }
    for (deck, golden, lang) in DECKS {
        let path = root().join("testdata/office/golden").join(golden);
        std::fs::write(path, markdown(deck, *lang)).unwrap();
    }
}
