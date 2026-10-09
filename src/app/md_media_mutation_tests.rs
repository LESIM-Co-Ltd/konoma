//! Tests added from mutation testing of `md_media.rs`: the kitty id families, which picture keys
//! can be made again after eviction, and the loader attachments.

use super::*;
use crate::config::Config;
use crate::test_support::unique_tmp;
use std::collections::HashMap;

fn full() -> MdEncodeKey {
    MdEncodeKey::Full { cols: 10, rows: 5 }
}
fn clip() -> MdEncodeKey {
    MdEncodeKey::Clip {
        cols: 10,
        full_rows: 5,
        row_off: 1,
        vis_rows: 3,
    }
}

#[test]
fn kitty_ids_are_fixed_per_path_slot_and_family() {
    let mut ids: HashMap<PathBuf, [Vec<u32>; 3]> = HashMap::new();
    let a = Path::new("a.png");
    // not a kitty terminal: no id, and nothing remembered
    assert_eq!(kitty_id_for(&mut ids, a, &full(), 0, false), None);
    assert!(ids.is_empty());
    // a kitty terminal: a real id (0 means "unspecified" to kitty), the same one every time
    let f0 = kitty_id_for(&mut ids, a, &full(), 0, true).expect("id");
    assert_ne!(f0, 0);
    assert_eq!(kitty_id_for(&mut ids, a, &full(), 0, true), Some(f0));
    // another slot of the same family is another id, and growing the family keeps the first
    let f1 = kitty_id_for(&mut ids, a, &full(), 1, true).expect("id");
    assert_ne!(f1, f0);
    assert_eq!(kitty_id_for(&mut ids, a, &full(), 0, true), Some(f0));
    assert_eq!(kitty_id_for(&mut ids, a, &full(), 1, true), Some(f1));
    assert_eq!(ids[a][0], vec![f0, f1], "exactly slot + 1 ids exist");
    // asking for a later slot first creates every slot before it
    let c2 = kitty_id_for(&mut ids, a, &clip(), 2, true).expect("id");
    assert_eq!(ids[a][1].len(), 3);
    assert_eq!(ids[a][1][2], c2);
    // the clip family does not share ids with the full family
    assert!(!ids[a][0].contains(&c2));
    assert!(ids[a][1].iter().all(|i| !ids[a][0].contains(i)));
    // another picture has its own ids
    let b = kitty_id_for(&mut ids, Path::new("b.png"), &full(), 0, true).expect("id");
    assert_ne!(b, f0);
    assert_ne!(b, f1);
}

#[test]
fn only_pictures_that_can_be_made_again_may_be_evicted() {
    use crate::preview::markdown::{math_url, mermaid_fence_url};
    // a file on disk and a picture of the open Word document can both be rebuilt
    assert!(is_rebuildable_md_key(Path::new("/tmp/pic.png")));
    assert!(is_rebuildable_md_key(Path::new(
        "office-img://abc/image1.png"
    )));
    // diagrams, formulas and media-diff pictures exist nowhere else
    assert!(!is_rebuildable_md_key(Path::new(&mermaid_fence_url(
        "graph LR\nA-->B"
    ))));
    assert!(!is_rebuildable_md_key(Path::new(&math_url("x^2", false))));
    assert!(!is_rebuildable_md_key(Path::new("media-diff://old/0")));
}

#[test]
fn slide_pictures_are_drawn_at_the_frame_size_between_the_floor_and_the_cap() {
    use crate::app::office_doc::{DocPicture, LoadedDocument, PictureSource};
    use std::sync::{atomic::AtomicBool, atomic::AtomicU32, Arc};
    let dir = unique_tmp("konoma_mdm_slide_px");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let slide = "office-img://deck/slide-1";
    let mut pictures = HashMap::new();
    pictures.insert(
        slide.to_string(),
        DocPicture {
            source: PictureSource::Slide {
                scene: Arc::new(Default::default()),
                media: Arc::new(HashMap::new()),
                render_truncated: Arc::new(AtomicBool::new(false)),
                raster_cap: Arc::new(AtomicU32::new(0)),
            },
            dims: Some((1280, 720)),
            raster: false,
        },
    );
    pictures.insert(
        "office-img://deck/photo.png".to_string(),
        DocPicture {
            source: PictureSource::Bytes(Arc::new(vec![1, 2, 3])),
            dims: None,
            raster: true,
        },
    );
    app.document = Some(Box::new(LoadedDocument {
        markdown: String::new(),
        picture_markdown: String::new(),
        truncated: false,
        render_truncated: Arc::new(AtomicBool::new(false)),
        pictures,
        slides: Vec::new(),
    }));
    // no image backend yet: the cell size is unknown
    assert_eq!(app.slide_raster_px(slide, 10, 10), None);
    let picker = ratatui_image::picker::Picker::halfblocks();
    let font = picker.font_size();
    let (fw, fh) = (u32::from(font.width.max(1)), u32::from(font.height.max(1)));
    app.picker = Some(picker);
    app.cfg.ui.svg_max_px = 100;
    // the wider of the frame's two sides, each in its own cell size
    assert_eq!(
        app.slide_raster_px(slide, 400, 2),
        Some((400 * fw).max(2 * fh))
    );
    assert_eq!(
        app.slide_raster_px(slide, 2, 150),
        Some((2 * fw).max(150 * fh))
    );
    assert!(
        150 * fh > 2 * fw && 400 * fw > 2 * fh,
        "both sides were told apart"
    );
    // never below the configured size, never above the cap
    assert_eq!(app.slide_raster_px(slide, 1, 1), Some(100.max(fw).max(fh)));
    assert_eq!(
        app.slide_raster_px(slide, 5000, 5000),
        Some(SLIDE_RASTER_MAX_PX)
    );
    app.cfg.ui.svg_max_px = 500;
    assert_eq!(app.slide_raster_px(slide, 1, 1), Some(500.max(fw).max(fh)));
    // anything but a slide of the open presentation: nothing
    assert_eq!(
        app.slide_raster_px("office-img://deck/photo.png", 10, 10),
        None
    );
    assert_eq!(app.slide_raster_px("office-img://deck/other", 10, 10), None);
    assert_eq!(app.slide_raster_px("/tmp/slide-1.png", 10, 10), None);
}

#[test]
fn the_raster_cap_of_a_slide_is_unknown_until_it_was_drawn() {
    use crate::app::office_doc::{DocPicture, LoadedDocument, PictureSource};
    use std::sync::{atomic::AtomicBool, atomic::AtomicU32, atomic::Ordering, Arc};
    let dir = unique_tmp("konoma_mdm_slide_cap");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let cap = Arc::new(AtomicU32::new(0));
    let mut pictures = HashMap::new();
    pictures.insert(
        "office-img://deck/slide-1".to_string(),
        DocPicture {
            source: PictureSource::Slide {
                scene: Arc::new(Default::default()),
                media: Arc::new(HashMap::new()),
                render_truncated: Arc::new(AtomicBool::new(false)),
                raster_cap: cap.clone(),
            },
            dims: Some((1280, 720)),
            raster: false,
        },
    );
    pictures.insert(
        "office-img://deck/photo.png".to_string(),
        DocPicture {
            source: PictureSource::Bytes(Arc::new(vec![1])),
            dims: None,
            raster: true,
        },
    );
    assert_eq!(
        app.slide_raster_cap("office-img://deck/slide-1"),
        None,
        "no document"
    );
    app.document = Some(Box::new(LoadedDocument {
        markdown: String::new(),
        picture_markdown: String::new(),
        truncated: false,
        render_truncated: Arc::new(AtomicBool::new(false)),
        pictures,
        slides: Vec::new(),
    }));
    let url = "office-img://deck/slide-1";
    assert_eq!(app.slide_raster_cap(url), None, "0 = not drawn yet");
    cap.store(1, Ordering::Relaxed);
    assert_eq!(app.slide_raster_cap(url), Some(1));
    cap.store(3000, Ordering::Relaxed);
    assert_eq!(app.slide_raster_cap(url), Some(3000));
    assert_eq!(app.slide_raster_cap("office-img://deck/photo.png"), None);
    assert_eq!(app.slide_raster_cap("office-img://deck/other"), None);
}

#[test]
fn a_reloaded_document_keeps_only_the_pictures_it_still_has() {
    use crate::app::office_doc::{DocPicture, LoadedDocument, PictureSource};
    use std::sync::{atomic::AtomicBool, Arc};
    let dir = unique_tmp("konoma_mdm_land");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let path = dir.join("d.docx");
    app.tab.preview_kind = Some(crate::app::PreviewKind::Document(path.clone()));
    app.tab.preview_path = Some(path);
    let key = |s: &str| PathBuf::from(s);
    for k in [
        "/some/file.png",
        "office-img://d/keep",
        "office-img://d/gone",
    ] {
        app.md_image_cache.insert(key(k), MdImgEntry::default());
    }
    let mut pictures = HashMap::new();
    pictures.insert(
        "office-img://d/keep".to_string(),
        DocPicture {
            source: PictureSource::Bytes(Arc::new(vec![1])),
            dims: None,
            raster: true,
        },
    );
    app.land_document(Box::new(LoadedDocument {
        markdown: "x\n".into(),
        picture_markdown: String::new(),
        truncated: false,
        render_truncated: Arc::new(AtomicBool::new(false)),
        pictures,
        slides: Vec::new(),
    }));
    assert!(
        app.md_image_cache.contains_key(&key("/some/file.png")),
        "not a document picture"
    );
    assert!(app.md_image_cache.contains_key(&key("office-img://d/keep")));
    assert!(!app.md_image_cache.contains_key(&key("office-img://d/gone")));
}

#[test]
fn attaching_the_kitty_loader_keeps_its_sender() {
    let dir = unique_tmp("konoma_mdm_kitty");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    assert!(app.kitty_tx.is_none());
    let (tx, _rx) = std::sync::mpsc::channel();
    app.attach_kitty_loader(tx);
    assert!(app.kitty_tx.is_some());
}
