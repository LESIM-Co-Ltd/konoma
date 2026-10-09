//! Temporary review instrument (ignored): side-by-side dumps of chosen corpus slides.
//! `G1_SLIDES="poi__alterman_security:10,..." G1_OUT=<dir> cargo test --release --lib g1_dump -- --ignored`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{imageops, Rgba as Px, RgbaImage};

use super::docx::pptx::load_presentation;
use super::docx::DocOptions;
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";

fn find_deck(name: &str) -> Option<(PathBuf, PathBuf)> {
    let (src, stem) = name.split_once("__")?;
    if src == "selfmade" {
        let p = Path::new(CACHE).join(format!("slide-corpus-gen/{stem}.pptx"));
        return Some((p, Path::new(CACHE).join("pptx-ref/selfmade").join(stem)));
    }
    let txt = std::fs::read_to_string(Path::new(CACHE).join("pptx-corpus/index.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    for it in v.as_array()? {
        let (Some(s), Some(f)) = (it["source"].as_str(), it["file"].as_str()) else {
            continue;
        };
        let p = Path::new(CACHE).join("pptx-corpus").join(f);
        if s == src && p.file_stem().and_then(|x| x.to_str()) == Some(stem) {
            return Some((p, Path::new(CACHE).join("pptx-ref").join(s).join(stem)));
        }
    }
    None
}

fn on_white(img: &image::DynamicImage) -> RgbaImage {
    let src = img.to_rgba8();
    let mut out = RgbaImage::from_pixel(src.width(), src.height(), Px([255, 255, 255, 255]));
    imageops::overlay(&mut out, &src, 0, 0);
    out
}

#[test]
#[ignore = "reads the local corpus; writes G1_OUT"]
fn g1_dump() {
    let list = std::env::var("G1_SLIDES").unwrap_or_default();
    let out = PathBuf::from(std::env::var("G1_OUT").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    for ent in list.split(',').filter(|s| !s.is_empty()) {
        let (deck, n) = ent.rsplit_once(':').unwrap();
        let n: usize = n.parse().unwrap();
        let Some((path, refd)) = find_deck(deck) else {
            eprintln!("no deck {deck}");
            continue;
        };
        let doc = load_presentation(&path, &DocOptions::default()).unwrap();
        let media: HashMap<String, Arc<Vec<u8>>> = doc
            .images
            .iter()
            .map(|i| (i.key.clone(), Arc::new(i.bytes.clone())))
            .collect();
        let sc = &doc.slide_scenes[n - 1];
        let lookup = |k: &str| media.get(k).cloned();
        let r = sd::render_svg(sc, &lookup);
        if std::env::var("G1_SVG").is_ok() {
            std::fs::write(out.join(format!("{deck}-{n}.svg")), &r.svg).unwrap();
        }
        let ours =
            crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/s.svg"), 1280)
                .map(|i| on_white(&i));
        let reference = image::open(refd.join(format!("slide-{n:03}.png")))
            .ok()
            .map(|i| on_white(&i));
        if let (Some(a), Some(b)) = (&ours, &reference) {
            let (mad, ssim, hist, _) = super::tests_fidelity_rank::scores(a, b);
            println!(
                "SCORE {deck} {n} {:.3}",
                super::tests_fidelity_rank::combined(mad, ssim, hist)
            );
        }
        const HALF: u32 = 640;
        let fit = |img: &RgbaImage| {
            let h = (f64::from(img.height()) * f64::from(HALF) / f64::from(img.width().max(1)))
                .round() as u32;
            imageops::resize(img, HALF, h.max(1), imageops::FilterType::Lanczos3)
        };
        let (l, rr) = (ours.as_ref().map(fit), reference.as_ref().map(fit));
        let h = l
            .as_ref()
            .map_or(0, |i| i.height())
            .max(rr.as_ref().map_or(0, |i| i.height()))
            .max(300);
        let mut c = RgbaImage::from_pixel(HALF * 2 + 8, h, Px([120, 120, 120, 255]));
        if let Some(l) = &l {
            imageops::overlay(&mut c, l, 0, 0);
        }
        if let Some(r) = &rr {
            imageops::overlay(&mut c, r, i64::from(HALF) + 8, 0);
        }
        c.save(out.join(format!("{deck}-{n}.png"))).unwrap();
    }
}
