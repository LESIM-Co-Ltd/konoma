//! Dumps for a human (ignored tests): every slide of the OpenDocument decks drawn by konoma, side by
//! side with the picture LibreOffice made of the same slide. They write into
//! `docs/render-check/d1/` of the main checkout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{imageops, DynamicImage, Rgba as Px, RgbaImage};

use super::super::super::pptx::load_presentation;
use super::{DocOptions, Document};
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/d1";

/// Where the images go: `ODP_DUMP_OUT` when set (so that parallel work does not overwrite another
/// dump), else [`OUT`].
fn out_dir() -> PathBuf {
    std::env::var("ODP_DUMP_OUT")
        .ok()
        .filter(|s| !s.is_empty())
        .map_or_else(|| PathBuf::from(OUT), PathBuf::from)
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn draw_all(doc: &Document, only: Option<usize>) -> Vec<Option<DynamicImage>> {
    let media: HashMap<String, Arc<Vec<u8>>> = doc
        .images
        .iter()
        .map(|i| (i.key.clone(), Arc::new(i.bytes.clone())))
        .collect();
    let lookup = |k: &str| media.get(k).cloned();
    doc.slide_scenes
        .iter()
        .take(only.unwrap_or(usize::MAX))
        .enumerate()
        .map(|(i, sc)| {
            let r = sd::render_svg(sc, &lookup);
            if let Some(dir) = selected("ODP_DUMP_SVG") {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(
                    Path::new(&dir).join(format!("slide-{:03}.svg", i + 1)),
                    &r.svg,
                );
            }
            crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/slide.svg"), 1280)
        })
        .collect()
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

fn clear_dir(dir: &Path, corpus: bool) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(rd) = std::fs::read_dir(dir) {
        for f in rd.flatten() {
            let own_name = !f.file_name().to_string_lossy().starts_with("c-");
            if f.path().extension().is_some_and(|e| e == "png") && own_name != corpus {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
}

fn dump_deck(name: &str, odp: &Path, ref_dir: &Path, out: &Path, only: Option<usize>) {
    let t = std::time::Instant::now();
    let doc = match load_presentation(odp, &DocOptions::default()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{name}: load failed: {e:?}");
            return;
        }
    };
    let drawn = draw_all(&doc, only);
    eprintln!(
        "{name}: {} slides, {:?}, truncated {}",
        doc.slides.len(),
        t.elapsed(),
        doc.truncated
    );
    for (i, img) in drawn.iter().enumerate() {
        let n = i + 1;
        let reference = ref_dir.join(format!("slide-{n:03}.png"));
        let sbs = side_by_side(
            img.as_ref(),
            reference.exists().then_some(reference.as_path()),
        );
        sbs.save(out.join(format!("{name}-{n:03}.png"))).unwrap();
    }
}

fn selected(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.is_empty())
}

#[test]
#[ignore = "writes docs/render-check/d1/*.png for a human to look at"]
fn odp_dump_own_and_selfmade() {
    let out_buf = out_dir();
    let out = out_buf.as_path();
    let only = selected("ODP_DUMP_ONLY");
    if only.is_none() {
        clear_dir(out, false);
    }
    let own = Path::new(CACHE).join("pptx-ref/own");
    let selfmade = Path::new(CACHE).join("pptx-ref/selfmade");
    let gen = Path::new(CACHE).join("slide-corpus-gen");
    let mut decks: Vec<(String, PathBuf, PathBuf)> = vec![
        (
            "sample".into(),
            root().join("samples/sample.odp"),
            own.join("sample-odp"),
        ),
        (
            "slides".into(),
            root().join("testdata/office/slides.odp"),
            own.join("slides-odp"),
        ),
        (
            "slides-ja".into(),
            root().join("testdata/office/slides-ja.odp"),
            own.join("slides-ja-odp"),
        ),
    ];
    for k in ["text", "shapes", "fills", "tables", "charts", "background"] {
        let name = format!("draw-{k}");
        decks.push((
            name.clone(),
            gen.join(format!("{name}.odp")),
            selfmade.join(format!("{name}-odp")),
        ));
    }
    for (name, odp, refd) in decks {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        dump_deck(&name, &odp, &refd, out, None);
    }
}

#[test]
#[ignore = "writes images for a human to look at: ODP_DUMP_DECK (an .odp) and ODP_DUMP_REF (the directory of the reference slides)"]
fn odp_dump_custom() {
    let (Some(deck), Some(refd)) = (selected("ODP_DUMP_DECK"), selected("ODP_DUMP_REF")) else {
        return;
    };
    let out_buf = out_dir();
    let name = Path::new(&deck)
        .file_stem()
        .map_or("custom".to_string(), |s| s.to_string_lossy().to_string());
    let _ = std::fs::create_dir_all(&out_buf);
    dump_deck(&name, Path::new(&deck), Path::new(&refd), &out_buf, None);
}

#[test]
#[ignore = "writes docs/render-check/d1/*.png for a human to look at"]
fn odp_dump_corpus() {
    let out_buf = out_dir();
    let out = out_buf.as_path();
    let only = selected("ODP_DUMP_ONLY");
    if only.is_none() {
        clear_dir(out, true);
    }
    let src = Path::new(CACHE).join("pptx-corpus/libreoffice");
    let refs = Path::new(CACHE).join("pptx-ref/libreoffice");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&src)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "odp"))
        .collect();
    files.sort();
    for f in files {
        let stem = f.file_stem().unwrap().to_string_lossy().to_string();
        let short = stem.trim_start_matches("odp__").to_string();
        if only.as_deref().is_some_and(|o| o != short) {
            continue;
        }
        dump_deck(
            &format!("c-{short}"),
            &f,
            &refs.join(format!("{stem}-odp")),
            out,
            Some(6),
        );
    }
}
