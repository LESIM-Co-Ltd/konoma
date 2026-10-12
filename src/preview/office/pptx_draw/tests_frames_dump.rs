//! Ignored dump for a human: slides with charts or SmartArt drawn by konoma, side by side with
//! the picture LibreOffice made of the same slide (`docs/render-check/e2/`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use image::{imageops, RgbaImage};

use super::super::{load_presentation, DocOptions};
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/e2";
/// Most slides of one deck that are put in the dump.
const PER_DECK: usize = 4;

fn side_by_side(ours: Option<RgbaImage>, reference: &Path) -> RgbaImage {
    let right = image::open(reference).ok().map(|i| i.to_rgba8());
    let h = ours.as_ref().map_or(720, |i| i.height());
    let right = right.map(|img| {
        let w =
            (f64::from(img.width()) * f64::from(h) / f64::from(img.height().max(1))).round() as u32;
        imageops::resize(&img, w.max(1), h, imageops::FilterType::Lanczos3)
    });
    let lw = ours.as_ref().map_or(640, |i| i.width());
    let rw = right.as_ref().map_or(640, |i| i.width());
    let mut canvas = RgbaImage::from_pixel(lw + rw + 12, h, image::Rgba([120, 120, 120, 255]));
    if let Some(l) = &ours {
        imageops::overlay(&mut canvas, l, 0, 0);
    }
    if let Some(r) = &right {
        imageops::overlay(&mut canvas, r, i64::from(lw) + 12, 0);
    }
    canvas
}

fn has_group(sc: &sd::SlideScene) -> bool {
    sc.items.iter().any(|i| matches!(i, sd::Item::Group(_)))
}

#[test]
#[ignore = "reads the local corpora; writes docs/render-check/e2/*.png"]
fn frames_dump_render_check() {
    let out = Path::new(OUT);
    let _ = std::fs::create_dir_all(out);
    if let Ok(rd) = std::fs::read_dir(out) {
        for f in rd.flatten() {
            if f.path().extension().is_some_and(|e| e == "png") {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
    let cache = Path::new(CACHE);
    let mut decks: Vec<(String, std::path::PathBuf, std::path::PathBuf)> = vec![(
        "draw-charts".into(),
        cache.join("slide-corpus-gen/draw-charts.pptx"),
        cache.join("pptx-ref/selfmade/draw-charts"),
    )];
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cache.join("pptx-corpus/index.json")).unwrap())
            .unwrap();
    for e in index.as_array().unwrap() {
        if !(e["charts"].as_bool().unwrap_or(false) || e["smartart"].as_bool().unwrap_or(false)) {
            continue;
        }
        let file = e["file"].as_str().unwrap();
        if !file.ends_with(".pptx") {
            continue;
        }
        let (src, name) = file.split_once('/').unwrap();
        let stem = name.trim_end_matches(".pptx");
        decks.push((
            format!("{src}__{stem}"),
            cache.join("pptx-corpus").join(file),
            cache.join("pptx-ref").join(src).join(stem),
        ));
    }
    for (name, pptx, refd) in decks {
        let Ok(doc) = load_presentation(&pptx, &DocOptions::default()) else {
            eprintln!("{name}: load failed");
            continue;
        };
        let media: HashMap<String, Arc<Vec<u8>>> = doc
            .images
            .iter()
            .map(|i| (i.key.clone(), Arc::new(i.bytes.clone())))
            .collect();
        let lookup = |k: &str| media.get(k).cloned();
        let small = doc.slide_scenes.len() <= PER_DECK;
        let mut shown = 0;
        for (i, sc) in doc.slide_scenes.iter().enumerate() {
            if (!small && !has_group(sc)) || shown >= PER_DECK {
                continue;
            }
            shown += 1;
            let r = sd::render_svg(sc, &lookup);
            let ours = crate::preview::svg::rasterize_trusted(
                r.svg.as_bytes(),
                Path::new("/slide.svg"),
                1280,
            )
            .map(|d| d.to_rgba8());
            let n = i + 1;
            let img = side_by_side(ours, &refd.join(format!("slide-{n:03}.png")));
            img.save(out.join(format!("{name}-{n:03}.png"))).unwrap();
        }
        eprintln!("{name}: {} slides, {shown} shown", doc.slide_scenes.len());
    }
}
