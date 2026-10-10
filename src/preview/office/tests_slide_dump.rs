//! Review instrument (ignored test): draws chosen slides of the local corpus and writes
//! "konoma | LibreOffice" pictures, a score, and (on request) the SVG and the full-size picture.
//!
//! ```text
//! SLIDE_DUMP_SLIDES="poi/customGeo:5,libreoffice/odp__16-9:1" \
//! SLIDE_DUMP_OUT=/path/to/dir \
//!   cargo test --release --bin konoma slide_dump -- --ignored --nocapture
//! ```
//!
//! Environment:
//! - `SLIDE_DUMP_SLIDES` (required): comma-separated `source/stem:N`. `source` is a folder of
//!   `NoCode/.cache/pptx-corpus/` (`poi`, `libreoffice`, `python-pptx`, ..); `stem` is the file name
//!   without `.pptx` / `.odp` (the LibreOffice-made ODP decks are named `odp__<name>`), also looked up in
//!   `slide-corpus-gen/` (self-made decks, give any `source`, e.g. `selfmade/draw-shapes:3`),
//!   `samples/` and `testdata/office/`. `N` is the slide number (1-based); `*` is the first 12
//!   slides, for which only the files of `SLIDE_DUMP_SVG` are written (no pictures, no score).
//! - `SLIDE_DUMP_OUT`: the folder of the side-by-side pictures
//!   `<tag>-<source>-<stem>-NNN.png` (konoma left, LibreOffice right, 640 px each). Default
//!   `docs/render-check/slide-dump/` of the main checkout.
//! - `SLIDE_DUMP_TAG`: the file-name prefix, e.g. `before` / `after` around a change (default `after`).
//! - `SLIDE_DUMP_SVG`: a folder; also writes `<source>-<stem>-NNN.svg` and the full-size 1280 px
//!   `<source>-<stem>-NNN.ours.png`, for measuring.
//!
//! Each picture also prints `SCORE <deck> <n> <combined>` (0 = identical to the LibreOffice
//! reference, 1 = unrelated; the same score as `tests_fidelity_rank`) and the item count.
//! The LibreOffice reference pictures are those of `docs/render-check/tools/lo_export_slides.py`
//! in `NoCode/.cache/pptx-ref/<source>/<stem>[-odp]/slide-NNN.png`; a slide without one is written
//! with an empty right half and no score.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{imageops, Rgba as Px, RgbaImage};

use super::docx::pptx::load_presentation;
use super::docx::DocOptions;
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const DEFAULT_OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/slide-dump";
/// Slides drawn for `N = *`.
const ALL_SLIDES: usize = 12;
/// Width of one half of a side-by-side picture.
const HALF: u32 = 640;
/// Width the slide is rasterized at (the reference is 1280 px wide).
const FULL_W: u32 = 1280;

/// One `source/stem:N` of `SLIDE_DUMP_SLIDES`; `slide` is `None` for `*`.
#[derive(Debug, PartialEq)]
struct Spec<'a> {
    source: &'a str,
    stem: &'a str,
    slide: Option<usize>,
}

fn parse_spec(item: &str) -> Result<Spec<'_>, String> {
    let (deck, n) = item
        .rsplit_once(':')
        .ok_or_else(|| format!("{item}: want source/stem:N"))?;
    let (source, stem) = deck
        .split_once('/')
        .ok_or_else(|| format!("{item}: want source/stem:N"))?;
    let slide = if n == "*" {
        None
    } else {
        match n.parse::<usize>() {
            Ok(v) if v >= 1 => Some(v),
            _ => return Err(format!("{item}: the slide is a number from 1, or *")),
        }
    };
    Ok(Spec {
        source,
        stem,
        slide,
    })
}

/// The deck file: the corpus folder of `source`, the self-made decks, the samples, the test data.
fn find_deck(source: &str, stem: &str) -> Option<PathBuf> {
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

fn on_white(img: &image::DynamicImage) -> RgbaImage {
    let src = img.to_rgba8();
    let mut out = RgbaImage::from_pixel(src.width(), src.height(), Px([255, 255, 255, 255]));
    imageops::overlay(&mut out, &src, 0, 0);
    out
}

fn side_by_side(ours: Option<&RgbaImage>, reference: Option<&RgbaImage>) -> RgbaImage {
    let fit = |img: &RgbaImage| {
        let h = (f64::from(img.height()) * f64::from(HALF) / f64::from(img.width().max(1))).round()
            as u32;
        imageops::resize(img, HALF, h.max(1), imageops::FilterType::Lanczos3)
    };
    let (l, r) = (ours.map(fit), reference.map(fit));
    let h = l
        .as_ref()
        .map_or(0, |i| i.height())
        .max(r.as_ref().map_or(0, |i| i.height()))
        .max(100);
    let mut c = RgbaImage::from_pixel(HALF * 2 + 8, h, Px([120, 120, 120, 255]));
    if let Some(l) = &l {
        imageops::overlay(&mut c, l, 0, 0);
    }
    if let Some(r) = &r {
        imageops::overlay(&mut c, r, i64::from(HALF) + 8, 0);
    }
    c
}

#[test]
fn a_spec_names_a_source_a_deck_and_a_slide_or_all() {
    assert_eq!(
        parse_spec("poi/customGeo:5"),
        Ok(Spec {
            source: "poi",
            stem: "customGeo",
            slide: Some(5)
        })
    );
    assert_eq!(parse_spec("libreoffice/odp__16-9:*").unwrap().slide, None);
    for bad in ["customGeo:5", "poi/customGeo", "poi/customGeo:0", "poi/x:y"] {
        assert!(parse_spec(bad).is_err(), "{bad}");
    }
}

#[test]
#[ignore = "reads the local corpus (NoCode/.cache/pptx-corpus, pptx-ref) and writes SLIDE_DUMP_OUT / SLIDE_DUMP_SVG"]
fn slide_dump() {
    let list = std::env::var("SLIDE_DUMP_SLIDES").expect("SLIDE_DUMP_SLIDES=source/stem:N,..");
    let tag = std::env::var("SLIDE_DUMP_TAG").unwrap_or_else(|_| "after".into());
    let out = PathBuf::from(std::env::var("SLIDE_DUMP_OUT").unwrap_or_else(|_| DEFAULT_OUT.into()));
    let svg_dir = std::env::var("SLIDE_DUMP_SVG").ok().map(PathBuf::from);
    std::fs::create_dir_all(&out).expect("SLIDE_DUMP_OUT");
    if let Some(d) = &svg_dir {
        std::fs::create_dir_all(d).expect("SLIDE_DUMP_SVG");
    }
    for item in list.split(',').filter(|s| !s.is_empty()) {
        let spec = parse_spec(item).unwrap_or_else(|e| panic!("{e}"));
        let Spec {
            source,
            stem,
            slide,
        } = spec;
        let Some(path) = find_deck(source, stem) else {
            eprintln!("no deck {source}/{stem}");
            continue;
        };
        let doc = load_presentation(&path, &DocOptions::default()).expect("load");
        let media: HashMap<String, Arc<Vec<u8>>> = doc
            .images
            .iter()
            .map(|i| (i.key.clone(), Arc::new(i.bytes.clone())))
            .collect();
        let lookup = |k: &str| media.get(k).cloned();
        let slides: Vec<usize> = match slide {
            Some(n) => vec![n],
            None => (1..=doc.slide_scenes.len().min(ALL_SLIDES)).collect(),
        };
        let leaf = if path.extension().is_some_and(|e| e == "odp") {
            format!("{stem}-odp")
        } else {
            stem.to_string()
        };
        for n in slides {
            let Some(scene) = doc.slide_scenes.get(n - 1) else {
                eprintln!("{source}/{stem} has {} slides", doc.slide_scenes.len());
                continue;
            };
            let r = sd::render_svg(scene, &lookup);
            let ours = crate::preview::svg::rasterize_trusted(
                r.svg.as_bytes(),
                Path::new("/s.svg"),
                FULL_W,
            )
            .map(|i| on_white(&i));
            if let Some(dir) = &svg_dir {
                let name = format!("{source}-{stem}-{n:03}");
                std::fs::write(dir.join(format!("{name}.svg")), &r.svg).expect("write svg");
                if let Some(o) = &ours {
                    o.save(dir.join(format!("{name}.ours.png")))
                        .expect("write png");
                }
            }
            if slide.is_none() {
                continue;
            }
            let reference = image::open(
                Path::new(CACHE)
                    .join("pptx-ref")
                    .join(source)
                    .join(&leaf)
                    .join(format!("slide-{n:03}.png")),
            )
            .ok()
            .map(|i| on_white(&i));
            if let (Some(a), Some(b)) = (&ours, &reference) {
                let (mad, ssim, hist, _) = super::tests_fidelity_rank::scores(a, b);
                println!(
                    "SCORE {source}/{stem} {n} {:.3}",
                    super::tests_fidelity_rank::combined(mad, ssim, hist)
                );
            }
            let file = out.join(format!("{tag}-{source}-{stem}-{n:03}.png"));
            side_by_side(ours.as_ref(), reference.as_ref())
                .save(&file)
                .expect("write picture");
            println!(
                "wrote {} ({} items, truncated {})",
                file.display(),
                scene.items.len(),
                r.truncated
            );
        }
    }
}
