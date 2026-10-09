//! Dumps for a human (ignored tests): every slide of the decks drawn by konoma, side by side with
//! the picture LibreOffice made of the same slide. They write into `docs/render-check/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use image::{imageops, DynamicImage, Rgba as Px, RgbaImage};

use super::super::super::{load_presentation, DocOptions, Document};
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/pptx-a";
const OUT_CORPUS: &str = "/Users/shuhei/work/konoma/docs/render-check/pptx-a-corpus";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Draws every slide of a loaded deck as a 1280 px wide picture (`None` for one that does not
/// rasterize).
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
        .map(|sc| {
            let r = sd::render_svg(sc, &lookup);
            crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/slide.svg"), 1280)
        })
        .collect()
}

/// `konoma | reference`, equally high.
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

fn clear_dir(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(rd) = std::fs::read_dir(dir) {
        for f in rd.flatten() {
            if f.path().extension().is_some_and(|e| e == "png") {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
}

fn dump_deck(name: &str, pptx: &Path, ref_dir: &Path, out: &Path, only: Option<usize>) {
    let t = Instant::now();
    let doc = match load_presentation(pptx, &DocOptions::default()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{name}: load failed: {e:?}");
            return;
        }
    };
    let load = t.elapsed();
    let drawn = draw_all(&doc, only);
    eprintln!(
        "{name}: {} slides, load {:?}, draw+raster {:?}, truncated {}",
        doc.slides.len(),
        load,
        t.elapsed() - load,
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

#[test]
#[ignore = "writes docs/render-check/pptx-a/*.png for a human to look at"]
fn pptx_a_dump_render_check() {
    let out = Path::new(OUT);
    clear_dir(out);
    let own = Path::new(CACHE).join("pptx-ref/own");
    let selfmade = Path::new(CACHE).join("pptx-ref/selfmade");
    let gen = Path::new(CACHE).join("slide-corpus-gen");
    for (name, pptx, refd) in [
        (
            "sample",
            root().join("samples/sample.pptx"),
            own.join("sample"),
        ),
        (
            "sample.ja",
            root().join("samples/sample.ja.pptx"),
            own.join("sample.ja"),
        ),
        (
            "slides",
            root().join("testdata/office/slides.pptx"),
            own.join("slides"),
        ),
        (
            "slides-ja",
            root().join("testdata/office/slides-ja.pptx"),
            own.join("slides-ja"),
        ),
    ] {
        dump_deck(name, &pptx, &refd, out, None);
    }
    for k in ["text", "shapes", "fills", "tables", "charts", "background"] {
        let name = format!("draw-{k}");
        dump_deck(
            &name,
            &gen.join(format!("{name}.pptx")),
            &selfmade.join(&name),
            out,
            None,
        );
    }
}

/// The PowerPoint-made decks of the corpus whose pictures are put next to LibreOffice's.
const PICKS: &[(&str, &str)] = &[
    ("poi", "2411-Performance_Up.pptx"),
    ("poi", "backgrounds.pptx"),
    ("poi", "60810.pptx"),
    ("poi", "alterman_security.pptx"),
    ("poi", "aptia.pptx"),
    ("poi", "WithMaster.pptx"),
    ("poi", "SampleShow.pptx"),
    ("poi", "54542_cropped_bitmap.pptx"),
    ("poi", "63200.pptx"),
    ("poi", "bug62513.pptx"),
];

fn corpus_files() -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    for src in ["poi", "python-pptx", "libreoffice"] {
        let Ok(rd) = std::fs::read_dir(Path::new(CACHE).join("pptx-corpus").join(src)) else {
            continue;
        };
        for f in rd.flatten() {
            let p = f.path();
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "pptx" | "pptm" | "potx" | "ppsx") {
                v.push((src.to_string(), p));
            }
        }
    }
    v.sort();
    v
}

#[test]
#[ignore = "reads the local PowerPoint corpus; writes docs/render-check/pptx-a-corpus/*.png"]
fn pptx_a_corpus_run() {
    let files = corpus_files();
    assert!(!files.is_empty(), "no corpus at {CACHE}/pptx-corpus");
    let (mut ok, mut failed, mut panics, mut slow) = (0, 0, 0, Vec::new());
    let t0 = Instant::now();
    for (src, p) in &files {
        let t = Instant::now();
        let r = load_presentation(p, &DocOptions::default());
        let dt = t.elapsed();
        match r {
            Ok(doc) => {
                ok += 1;
                assert_eq!(doc.slide_scenes.len(), doc.slides.len(), "{}", p.display());
                // Drawing a few slides must not panic either.
                let _ = draw_all(&doc, Some(1));
            }
            Err(e) => {
                let msg = format!("{e:?}");
                if msg.contains("panic") {
                    panics += 1;
                    eprintln!(
                        "PANIC {src}/{}: {msg}",
                        p.file_name().unwrap().to_string_lossy()
                    );
                } else {
                    failed += 1;
                }
            }
        }
        if dt.as_secs_f64() > 1.0 {
            slow.push((p.file_name().unwrap().to_string_lossy().to_string(), dt));
        }
    }
    eprintln!(
        "corpus: {} files, {ok} loaded, {failed} failed cleanly, {panics} panics, total {:?}; slow loads (>1s): {slow:?}",
        files.len(),
        t0.elapsed()
    );
    assert_eq!(panics, 0);

    let out = Path::new(OUT_CORPUS);
    clear_dir(out);
    for (src, file) in PICKS {
        let p = Path::new(CACHE).join("pptx-corpus").join(src).join(file);
        let stem = file.rsplit_once('.').map_or(*file, |(s, _)| s);
        let refd = Path::new(CACHE).join("pptx-ref").join(src).join(stem);
        dump_deck(&format!("{src}__{stem}"), &p, &refd, out, Some(3));
    }
}
