//! Ignored dump for a human: chosen slides drawn by konoma next to the LibreOffice picture.
//! `F3_DECKS="file:ref-dir:slides,..."` (file relative to the cache dir, ref-dir relative to
//! `pptx-ref`, slides like `1+3`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use image::{imageops, RgbaImage};

use super::super::{load_presentation, DocOptions};
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/f3";

#[test]
#[ignore = "reads the local corpora; writes docs/render-check/f3/*.png"]
fn f3_dump() {
    let out = Path::new(OUT);
    let _ = std::fs::create_dir_all(out);
    let cache = Path::new(CACHE);
    let spec = std::env::var("F3_DECKS").unwrap_or_default();
    for item in spec.split(',').filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = item.split(':').collect();
        let (file, refd, slides) = (parts[0], parts[1], parts[2]);
        let pptx = cache.join(file);
        let doc = load_presentation(&pptx, &DocOptions::default()).expect("load");
        let media: HashMap<String, Arc<Vec<u8>>> = doc
            .images
            .iter()
            .map(|i| (i.key.clone(), Arc::new(i.bytes.clone())))
            .collect();
        let lookup = |k: &str| media.get(k).cloned();
        let stem = pptx.file_stem().unwrap().to_string_lossy().to_string();
        let stem: String = stem.chars().take(24).collect();
        for n in slides.split('+').filter_map(|s| s.parse::<usize>().ok()) {
            let Some(sc) = doc.slide_scenes.get(n - 1) else {
                continue;
            };
            let r = sd::render_svg(sc, &lookup);
            if std::env::var("F3_SVG").is_ok() {
                std::fs::write(out.join(format!("{stem}-{n:03}.svg")), &r.svg).unwrap();
            }
            let ours = crate::preview::svg::rasterize_trusted(
                r.svg.as_bytes(),
                Path::new("/slide.svg"),
                std::env::var("F3_WIDTH")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1280),
            )
            .map(|d| d.to_rgba8())
            .expect("raster");
            let reff = cache
                .join("pptx-ref")
                .join(refd)
                .join(format!("slide-{n:03}.png"));
            let right = image::open(&reff).expect("ref").to_rgba8();
            let h = ours.height();
            let w = (f64::from(right.width()) * f64::from(h) / f64::from(right.height())) as u32;
            let right = imageops::resize(&right, w, h, imageops::FilterType::Lanczos3);
            let mut c = RgbaImage::from_pixel(ours.width() + w + 12, h, image::Rgba([120; 4]));
            imageops::overlay(&mut c, &ours, 0, 0);
            imageops::overlay(&mut c, &right, i64::from(ours.width()) + 12, 0);
            c.save(out.join(format!("{stem}-{n:03}.png"))).unwrap();
        }
    }
}
