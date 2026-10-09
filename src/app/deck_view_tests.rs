//! A presentation's slide pictures: the box a slide gets (`slide_cells`), the size a scene reports
//! (`slide_px`), and how a converted deck turns into pictures (`LoadedDocument::from_document`).
//! The views, `R` and the drawing on the decode thread are covered end to end in
//! `e2e_tests/deck_view.rs`.

use super::office_doc::{LoadedDocument, PictureSource};
use super::*;
use crate::preview::office::docx::pptx::SlideInfo;
use crate::preview::office::docx::{DocImage, Document};
use crate::preview::office::slide_draw::{Fill, Rgba, SlideScene};

// ---- the box of a slide --------------------------------------------------------------------

#[test]
fn a_slide_takes_the_full_width_when_the_height_allows() {
    // 16:9 in 10x20 px cells: 96 columns are 960 px wide, so 540 px = 27 rows.
    assert_eq!(slide_cells(960, 540, 10, 20, 96, 60), (96, 27));
    // The same slide, twice as large in pixels, is the same box (vector: any size fits the width).
    assert_eq!(slide_cells(1920, 1080, 10, 20, 96, 60), (96, 27));
    // A small slide is enlarged to the width.
    assert_eq!(slide_cells(96, 54, 10, 20, 96, 60), (96, 27));
}

#[test]
fn a_slide_is_capped_by_the_height_and_narrows_with_it() {
    let (cols, rows) = slide_cells(960, 540, 10, 20, 96, 10);
    assert_eq!(rows, 10);
    // 10 rows are 200 px tall; at 16:9 that is 355.6 px = 36 columns.
    assert_eq!(cols, 36);
    // Exactly at the limit: the width wins, nothing narrows.
    assert_eq!(slide_cells(960, 540, 10, 20, 96, 27), (96, 27));
    assert_eq!(slide_cells(960, 540, 10, 20, 96, 26).1, 26);
}

#[test]
fn a_slide_keeps_its_aspect_in_pixels_whatever_the_cell_shape() {
    for (fw, fh) in [(8u16, 16u16), (10, 20), (7, 14), (12, 12), (9, 25)] {
        for (pw, ph) in [(960u32, 540u32), (1440, 1080), (540, 960), (500, 500)] {
            for max_rows in [4u16, 12, 30, 80] {
                let (cols, rows) = slide_cells(pw, ph, fw, fh, 100, max_rows);
                assert!(rows <= max_rows && cols <= 100, "{fw}x{fh} {pw}x{ph}");
                let box_aspect = (cols as f64 * fw as f64) / (rows as f64 * fh as f64);
                let slide_aspect = pw as f64 / ph as f64;
                // Within the rounding of one cell on the shorter side.
                let tol = 1.0 / rows.min(cols).max(1) as f64 * 1.5;
                assert!(
                    (box_aspect / slide_aspect - 1.0).abs() <= tol,
                    "{fw}x{fh} {pw}x{ph} max {max_rows}: {cols}x{rows} {box_aspect} vs {slide_aspect}"
                );
            }
        }
    }
}

#[test]
fn a_tall_slide_is_limited_by_the_rows() {
    // Portrait: the rows run out long before the width does.
    let (cols, rows) = slide_cells(540, 960, 10, 20, 96, 20);
    assert_eq!(rows, 20);
    assert!(cols < 96, "{cols}");
}

#[test]
fn a_slide_box_survives_tiny_and_huge_terminals_and_zero_inputs() {
    // One column, one row: still a box.
    assert_eq!(slide_cells(960, 540, 10, 20, 1, 1), (1, 1));
    // Nothing to fit in: treated as one column and one row, never zero.
    assert_eq!(slide_cells(960, 540, 10, 20, 0, 0), (1, 1));
    // A zero-sized slide or cell is not a division by zero.
    for (pw, ph, fw, fh) in [
        (0, 0, 10, 20),
        (0, 540, 10, 20),
        (960, 0, 10, 20),
        (960, 540, 0, 0),
    ] {
        let (c, r) = slide_cells(pw, ph, fw, fh, 80, 30);
        assert!(
            c >= 1 && r >= 1 && c <= 80 && r <= 30,
            "{pw}x{ph} {fw}x{fh}"
        );
    }
    // The largest terminal u16 can describe.
    let (c, r) = slide_cells(960, 540, 10, 20, u16::MAX, u16::MAX);
    assert_eq!(c, u16::MAX);
    assert!(r >= 1);
    // An extremely wide slide is at least one row tall; an extremely tall one at least one column.
    assert_eq!(slide_cells(u32::MAX, 1, 10, 20, 80, 30).1, 1);
    assert_eq!(slide_cells(1, u32::MAX, 10, 20, 80, 30), (1, 30));
}

// ---- the size a scene reports ----------------------------------------------------------------

fn scene_of(w: f64, h: f64) -> SlideScene {
    SlideScene {
        width: w,
        height: h,
        background: Fill::Solid(Rgba::WHITE),
        underlay: Vec::new(),
        items: Vec::new(),
        truncated: false,
    }
}

#[test]
fn a_slide_is_sized_in_pixels_from_emu() {
    assert_eq!(
        office_doc::slide_px(&scene_of(9_144_000.0, 5_143_500.0)),
        Some((960, 540))
    );
    // A 4:3 slide.
    assert_eq!(
        office_doc::slide_px(&scene_of(9_144_000.0, 6_858_000.0)),
        Some((960, 720))
    );
}

#[test]
fn a_slide_without_a_usable_size_has_none() {
    for (w, h) in [
        (0.0, 5_143_500.0),
        (9_144_000.0, 0.0),
        (-9_144_000.0, 5_143_500.0),
        (f64::NAN, 5_143_500.0),
        (9_144_000.0, f64::NAN),
        (f64::INFINITY, 5_143_500.0),
        (9_144_000.0, f64::NEG_INFINITY),
        (4000.0, 5_143_500.0), // under half a pixel
    ] {
        assert_eq!(office_doc::slide_px(&scene_of(w, h)), None, "{w} x {h}");
    }
}

#[test]
fn a_huge_slide_is_clamped_not_wrapped() {
    assert_eq!(
        office_doc::slide_px(&scene_of(1e30, 1e30)),
        Some((u32::MAX, u32::MAX))
    );
}

// ---- how a converted deck becomes pictures ------------------------------------------------

fn slides(n: usize) -> Vec<SlideInfo> {
    (1..=n)
        .map(|number| SlideInfo {
            number,
            title: format!("T{number}"),
            hidden: false,
        })
        .collect()
}

fn doc(n: usize, scenes: usize, keys: usize, picture_md: &str) -> Document {
    Document {
        markdown: "## Slide 1: T1\n".into(),
        slides: slides(n),
        slide_scenes: (0..scenes)
            .map(|_| scene_of(9_144_000.0, 5_143_500.0))
            .collect(),
        slide_keys: (1..=keys)
            .map(|k| format!("office-img://aaaaaaaaaaaa/slide-{k}.svg"))
            .collect(),
        picture_markdown: picture_md.into(),
        ..Document::default()
    }
}

#[test]
fn a_consistent_deck_gets_a_picture_per_slide_sized_from_its_scene() {
    let d = LoadedDocument::from_document(doc(3, 3, 3, "## Slide 1: T1\n\n![a](x)\n"));
    assert_eq!(d.picture_markdown, "## Slide 1: T1\n\n![a](x)\n");
    assert_eq!(d.pictures.len(), 3);
    for k in 1..=3 {
        let p = &d.pictures[&format!("office-img://aaaaaaaaaaaa/slide-{k}.svg")];
        assert!(matches!(p.source, PictureSource::Slide { .. }));
        assert_eq!(p.dims, Some((960, 540)));
        assert!(!p.raster, "a slide is a vector drawing");
    }
}

#[test]
fn a_deck_whose_reader_did_not_fill_everything_keeps_only_its_text() {
    for (n, scenes, keys, md) in [
        (3, 2, 3, "x"), // a scene short
        (3, 3, 2, "x"), // a key short
        (3, 4, 3, "x"), // a scene too many
        (3, 3, 3, ""),  // no picture Markdown
        (3, 0, 0, "x"), // nothing drawn yet
        (0, 3, 3, "x"), // not a deck at all
    ] {
        let d = LoadedDocument::from_document(doc(n, scenes, keys, md));
        assert_eq!(d.picture_markdown, "", "{n} {scenes} {keys} {md:?}");
        assert!(
            d.pictures
                .values()
                .all(|p| !matches!(p.source, PictureSource::Slide { .. })),
            "{n} {scenes} {keys} {md:?}"
        );
        assert_eq!(d.markdown, "## Slide 1: T1\n", "the text view is untouched");
    }
}

#[test]
fn slide_scenes_resolve_images_through_a_snapshot_of_the_documents_pictures() {
    let mut document = doc(2, 2, 2, "## Slide 1: T1\n");
    document.images.push(DocImage {
        key: "office-img://aaaaaaaaaaaa/p.png".into(),
        bytes: vec![1, 2, 3],
        name: "p.png".into(),
    });
    let d = LoadedDocument::from_document(document);
    let bytes = match &d.pictures["office-img://aaaaaaaaaaaa/p.png"].source {
        PictureSource::Bytes(b) => b.clone(),
        PictureSource::Slide { .. } => panic!("a file is bytes"),
    };
    for k in 1..=2 {
        match &d.pictures[&format!("office-img://aaaaaaaaaaaa/slide-{k}.svg")].source {
            PictureSource::Slide { media, .. } => {
                let got = media.get("office-img://aaaaaaaaaaaa/p.png").unwrap();
                // The snapshot shares the document's bytes, it does not copy them.
                assert!(Arc::ptr_eq(got, &bytes), "slide {k}");
                assert!(media.get("office-img://aaaaaaaaaaaa/slide-1.svg").is_none());
            }
            PictureSource::Bytes(_) => panic!("a slide has no bytes"),
        }
    }
}

#[test]
fn a_word_document_has_no_slide_pictures() {
    let d = LoadedDocument::from_document(Document {
        markdown: "Hello\n".into(),
        ..Document::default()
    });
    assert!(d.picture_markdown.is_empty() && d.pictures.is_empty() && d.slides.is_empty());
}
