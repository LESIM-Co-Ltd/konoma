//! Review instrument (ignored test): draws chosen slides and writes konoma | LibreOffice pictures.
//!
//! `G2_SLIDES="poi/themes:9,poi/themes:8"` (source/deck-stem:slide, `.odp` decks as `x-odp`;
//! `:*` = the first 12 slides, which only write `G2_FULL` files),
//! `G2_TAG=before|after`, `G2_FULL=<dir>` (SVG and full-size pictures too), output
//! `docs/render-check/g2/<tag>-<source>-<stem>-NNN.png`.
//! Run: `cargo test g2_dump -- --ignored --nocapture`.

use std::path::Path;
use std::sync::Arc;

use image::{imageops, RgbaImage};

use super::docx::pptx::load_presentation;
use super::docx::DocOptions;
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/g2";
const ALL_SLIDES: usize = 12;

fn find_deck(source: &str, stem: &str) -> Option<std::path::PathBuf> {
    let c = Path::new(CACHE);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for dir in [
        c.join("pptx-corpus").join(source),
        c.join("slide-corpus-gen"),
        root.join("samples"),
        root.join("testdata/office"),
    ] {
        for ext in ["pptx", "odp"] {
            let p = dir.join(format!("{stem}.{ext}"));
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn side_by_side(ours: Option<&RgbaImage>, reference: Option<&RgbaImage>) -> RgbaImage {
    let half = 640u32;
    let fit = |img: &RgbaImage| {
        let h = (f64::from(img.height()) * f64::from(half) / f64::from(img.width().max(1))).round()
            as u32;
        let mut bg = RgbaImage::from_pixel(half, h.max(1), image::Rgba([255, 255, 255, 255]));
        let s = imageops::resize(img, half, h.max(1), imageops::FilterType::Lanczos3);
        imageops::overlay(&mut bg, &s, 0, 0);
        bg
    };
    let (l, r) = (ours.map(fit), reference.map(fit));
    let h = l
        .as_ref()
        .map_or(0, |i| i.height())
        .max(r.as_ref().map_or(0, |i| i.height()))
        .max(100);
    let mut c = RgbaImage::from_pixel(half * 2 + 8, h, image::Rgba([120, 120, 120, 255]));
    if let Some(l) = &l {
        imageops::overlay(&mut c, l, 0, 0);
    }
    if let Some(r) = &r {
        imageops::overlay(&mut c, r, i64::from(half) + 8, 0);
    }
    c
}

#[test]
#[ignore = "reads the local corpus; writes docs/render-check/g2/"]
fn g2_dump() {
    let spec = std::env::var("G2_SLIDES").unwrap_or_default();
    let tag = std::env::var("G2_TAG").unwrap_or_else(|_| "after".into());
    let _ = std::fs::create_dir_all(OUT);
    for item in spec.split(',').filter(|s| !s.is_empty()) {
        let (deck, n) = item.rsplit_once(':').expect("source/stem:N");
        let (source, stem) = deck.split_once('/').expect("source/stem");
        let Some(path) = find_deck(source, stem) else {
            eprintln!("no deck {deck}");
            continue;
        };
        let doc = load_presentation(&path, &DocOptions::default()).expect("load");
        let media: std::collections::HashMap<String, Arc<Vec<u8>>> = doc
            .images
            .iter()
            .map(|i| (i.key.clone(), Arc::new(i.bytes.clone())))
            .collect();
        let lookup = |k: &str| media.get(k).cloned();
        let all = n == "*";
        let slides: Vec<usize> = if all {
            (1..=doc.slide_scenes.len().min(ALL_SLIDES)).collect()
        } else {
            vec![n.parse().expect("slide number or *")]
        };
        for n in slides {
            let r = sd::render_svg(&doc.slide_scenes[n - 1], &lookup);
            let ours =
                crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/s.svg"), 1280)
                    .map(|i| i.to_rgba8());
            // `G2_FULL=<dir>`: also the SVG and the full-size picture, for measuring.
            if let Ok(dir) = std::env::var("G2_FULL") {
                let _ = std::fs::create_dir_all(&dir);
                let name = format!("{source}-{stem}-{n:03}");
                let _ = std::fs::write(Path::new(&dir).join(format!("{name}.svg")), &r.svg);
                if let Some(o) = &ours {
                    let _ = o.save(Path::new(&dir).join(format!("{name}.ours.png")));
                }
            }
            if all {
                continue;
            }
            let leaf = if path.extension().is_some_and(|e| e == "odp") {
                format!("{stem}-odp")
            } else {
                stem.to_string()
            };
            let refp = Path::new(CACHE)
                .join("pptx-ref")
                .join(source)
                .join(leaf)
                .join(format!("slide-{n:03}.png"));
            let reference = image::open(&refp).ok().map(|i| i.to_rgba8());
            let c = side_by_side(ours.as_ref(), reference.as_ref());
            let out = Path::new(OUT).join(format!("{tag}-{source}-{stem}-{n:03}.png"));
            c.save(&out).unwrap();
            eprintln!(
                "wrote {} ({} items, truncated {})",
                out.display(),
                doc.slide_scenes[n - 1].items.len(),
                r.truncated
            );
        }
    }
}
