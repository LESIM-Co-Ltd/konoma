//! Dump for a human (ignored test): the slides with tables of the self-made deck and of every
//! corpus deck that has tables, konoma's picture on the left and LibreOffice's on the right, into
//! `docs/render-check/e1/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{imageops, DynamicImage, Rgba as Px, RgbaImage};

use crate::preview::office::docx::pptx::{load_presentation, DocOptions, Document};
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/e1";

fn draw(doc: &Document, n: usize) -> Option<DynamicImage> {
    let media: HashMap<String, Arc<Vec<u8>>> = doc
        .images
        .iter()
        .map(|i| (i.key.clone(), Arc::new(i.bytes.clone())))
        .collect();
    let lookup = |k: &str| media.get(k).cloned();
    let r = sd::render_svg(doc.slide_scenes.get(n)?, &lookup);
    crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/slide.svg"), 1280)
}

fn side_by_side(ours: Option<&DynamicImage>, reference: Option<&Path>) -> RgbaImage {
    let left = ours.map(|i| i.to_rgba8());
    let right = reference
        .and_then(|p| image::open(p).ok())
        .map(|i| i.to_rgba8());
    let h = left
        .as_ref()
        .map(|i| i.height())
        .or_else(|| right.as_ref().map(|i| i.height()))
        .unwrap_or(720);
    let scale = |img: RgbaImage| -> RgbaImage {
        let w =
            (f64::from(img.width()) * f64::from(h) / f64::from(img.height().max(1))).round() as u32;
        imageops::resize(&img, w.max(1), h, imageops::FilterType::Lanczos3)
    };
    let right = right.map(scale);
    let lw = left.as_ref().map_or(640, |i| i.width());
    let rw = right.as_ref().map_or(640, |i| i.width());
    let mut canvas = RgbaImage::from_pixel(lw + rw + 12, h, Px([120, 120, 120, 255]));
    if let Some(l) = &left {
        imageops::overlay(&mut canvas, l, 0, 0);
    }
    if let Some(r) = &right {
        imageops::overlay(&mut canvas, r, i64::from(lw) + 12, 0);
    }
    canvas
}

/// The slides (0-based) whose text view holds a table.
fn table_slides(doc: &Document) -> Vec<usize> {
    let mut out = Vec::new();
    let mut idx: isize = -1;
    let mut seen_table = false;
    for line in doc.markdown.lines() {
        if line.starts_with("## ") {
            if idx >= 0 && seen_table {
                out.push(idx as usize);
            }
            idx += 1;
            seen_table = false;
        } else if line.starts_with("| ") || line.starts_with("|-") {
            seen_table = true;
        }
    }
    if idx >= 0 && seen_table {
        out.push(idx as usize);
    }
    out
}

fn dump_deck(name: &str, pptx: &Path, ref_dir: &Path, max: usize) -> usize {
    let Ok(doc) = load_presentation(pptx, &DocOptions::default()) else {
        eprintln!("{name}: load failed");
        return 0;
    };
    if doc.slide_scenes.len() != doc.slides.len() {
        eprintln!("{name}: no scenes");
        return 0;
    }
    let mut n = 0;
    for s in table_slides(&doc).into_iter().take(max) {
        let img = draw(&doc, s);
        let reference = ref_dir.join(format!("slide-{:03}.png", s + 1));
        let sbs = side_by_side(
            img.as_ref(),
            reference.exists().then_some(reference.as_path()),
        );
        sbs.save(Path::new(OUT).join(format!("{name}-{:03}.png", s + 1)))
            .unwrap();
        n += 1;
    }
    n
}

#[test]
#[ignore = "writes docs/render-check/e1/*.png for a human to look at"]
fn tables_dump_render_check() {
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
    dump_deck(
        "draw-tables",
        &cache.join("slide-corpus-gen/draw-tables.pptx"),
        &cache.join("pptx-ref/selfmade/draw-tables"),
        20,
    );
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cache.join("pptx-corpus/index.json")).unwrap())
            .unwrap();
    let only = std::env::var("E1_ONLY").ok();
    for e in index.as_array().unwrap() {
        if e["tables"] != true || e["status"] != "ok" {
            continue;
        }
        let file = e["file"].as_str().unwrap();
        let (src, f) = file.split_once('/').unwrap();
        let stem = f.rsplit_once('.').map_or(f, |(s, _)| s);
        if only.as_deref().is_some_and(|o| !stem.contains(o)) {
            continue;
        }
        let p: PathBuf = cache.join("pptx-corpus").join(file);
        let short: String = stem.chars().take(40).collect();
        let n = dump_deck(
            &format!("{src}__{short}"),
            &p,
            &cache.join("pptx-ref").join(src).join(stem),
            4,
        );
        eprintln!("{file}: {n} slides");
    }
}
