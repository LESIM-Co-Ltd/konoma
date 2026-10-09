//! Review instrument (ignored test): ranks every slide of every local corpus deck by how far
//! konoma's drawing is from the LibreOffice reference picture.
//!
//! Run: `cargo test --release --lib fidelity_rank -- --ignored --nocapture`
//!
//! Output: `NoCode/.cache/fidelity-rank/{ranking.csv,summary.md}` and the side-by-side pictures of the
//! worst slides under `docs/render-check/rank/` (konoma left, LibreOffice right).
//!
//! Scores, all on 320 px wide pictures (reference aspect; ours is stretched to it, flag `aspect`
//! when the two differ by more than 2 %), composited on white:
//! - `mad`: mean absolute luminance difference, 0..1.
//! - `ssim`: mean SSIM over non-overlapping 8x8 luminance windows (C1 = 0.01^2, C2 = 0.03^2), -1..1.
//! - `hist`: half the L1 distance of the joint 8x8x8 RGB histograms, 0..1 (wrong colours/backgrounds).
//! - `combined = 0.35 * min(1, mad / 0.25) + 0.45 * (1 - max(0, ssim)) + 0.20 * min(1, hist / 0.5)`,
//!   0 = identical, 1 = unrelated. A slide konoma fails to rasterize scores 1.0 (flag `raster-fail`).
//!
//! Timing is `render_svg` + the trusted rasterizer in this process; the app draws in a child process
//! (`svg_proc`), which adds process start and pipe transfer on top.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use image::{imageops, DynamicImage, Rgba as Px, RgbaImage};

use super::docx::{DocOptions, Document};
use super::docx::pptx::load_presentation;
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const OUT_DIR: &str = "/Users/shuhei/work/NoCode/.cache/fidelity-rank";
const SBS_DIR: &str = "/Users/shuhei/work/konoma/docs/render-check/rank";
/// Slides per deck that are drawn and scored.
const MAX_SLIDES: usize = 12;
/// How many of the worst slides get a side-by-side picture.
const WORST: usize = 80;
const SCORE_W: u32 = 320;

struct Deck {
    name: String,
    path: PathBuf,
    ref_dir: PathBuf,
}

struct Row {
    deck: String,
    path: PathBuf,
    ref_dir: PathBuf,
    slide: usize,
    mad: f64,
    ssim: f64,
    hist: f64,
    combined: f64,
    scene_truncated: bool,
    deck_truncated: bool,
    ms: f64,
    svg_bytes: usize,
    items: usize,
    flags: Vec<&'static str>,
}

fn count_items(items: &[sd::Item]) -> usize {
    items
        .iter()
        .map(|i| match i {
            sd::Item::Group(g) => 1 + count_items(&g.items),
            _ => 1,
        })
        .sum()
}

fn media_of(doc: &Document) -> HashMap<String, Arc<Vec<u8>>> {
    doc.images
        .iter()
        .map(|i| (i.key.clone(), Arc::new(i.bytes.clone())))
        .collect()
}

fn on_white(img: &DynamicImage) -> RgbaImage {
    let src = img.to_rgba8();
    let mut out = RgbaImage::from_pixel(src.width(), src.height(), Px([255, 255, 255, 255]));
    imageops::overlay(&mut out, &src, 0, 0);
    for p in out.pixels_mut() {
        p.0[3] = 255;
    }
    out
}

fn small(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    imageops::resize(img, w, h, imageops::FilterType::Triangle)
}

fn luma(img: &RgbaImage) -> Vec<f64> {
    img.pixels()
        .map(|p| {
            (0.299 * f64::from(p.0[0]) + 0.587 * f64::from(p.0[1]) + 0.114 * f64::from(p.0[2]))
                / 255.0
        })
        .collect()
}

fn ssim(a: &[f64], b: &[f64], w: usize, h: usize) -> f64 {
    const C1: f64 = 0.0001;
    const C2: f64 = 0.0009;
    let (mut sum, mut n) = (0.0, 0usize);
    let mut y = 0;
    while y + 8 <= h {
        let mut x = 0;
        while x + 8 <= w {
            let (mut ma, mut mb) = (0.0, 0.0);
            for j in 0..8 {
                for i in 0..8 {
                    ma += a[(y + j) * w + x + i];
                    mb += b[(y + j) * w + x + i];
                }
            }
            ma /= 64.0;
            mb /= 64.0;
            let (mut va, mut vb, mut cov) = (0.0, 0.0, 0.0);
            for j in 0..8 {
                for i in 0..8 {
                    let da = a[(y + j) * w + x + i] - ma;
                    let db = b[(y + j) * w + x + i] - mb;
                    va += da * da;
                    vb += db * db;
                    cov += da * db;
                }
            }
            va /= 63.0;
            vb /= 63.0;
            cov /= 63.0;
            sum += ((2.0 * ma * mb + C1) * (2.0 * cov + C2))
                / ((ma * ma + mb * mb + C1) * (va + vb + C2));
            n += 1;
            x += 8;
        }
        y += 8;
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f64
    }
}

fn hist(img: &RgbaImage) -> Vec<f64> {
    let mut h = vec![0.0; 512];
    for p in img.pixels() {
        let i = (usize::from(p.0[0]) >> 5) * 64
            + (usize::from(p.0[1]) >> 5) * 8
            + (usize::from(p.0[2]) >> 5);
        h[i] += 1.0;
    }
    let n = img.pixels().len().max(1) as f64;
    h.iter_mut().for_each(|v| *v /= n);
    h
}

/// `(mad, ssim, hist, aspect_differs)` of ours against the reference.
fn scores(ours: &RgbaImage, reference: &RgbaImage) -> (f64, f64, f64, bool) {
    let h = ((f64::from(SCORE_W) * f64::from(reference.height())
        / f64::from(reference.width().max(1)))
    .round() as u32)
        .max(8);
    let ra = f64::from(ours.width()) / f64::from(ours.height().max(1));
    let rb = f64::from(reference.width()) / f64::from(reference.height().max(1));
    let aspect = (ra / rb - 1.0).abs() > 0.02;
    let a = small(ours, SCORE_W, h);
    let b = small(reference, SCORE_W, h);
    let (la, lb) = (luma(&a), luma(&b));
    let mad = la.iter().zip(&lb).map(|(x, y)| (x - y).abs()).sum::<f64>() / la.len().max(1) as f64;
    let s = ssim(&la, &lb, SCORE_W as usize, h as usize);
    let hd = hist(&a)
        .iter()
        .zip(hist(&b))
        .map(|(x, y)| (x - y).abs())
        .sum::<f64>()
        / 2.0;
    (mad, s, hd, aspect)
}

fn combined(mad: f64, ssim: f64, hist: f64) -> f64 {
    0.35 * (mad / 0.25).min(1.0) + 0.45 * (1.0 - ssim.max(0.0)) + 0.20 * (hist / 0.5).min(1.0)
}

/// Where LibreOffice's pictures of a deck are.
fn ref_dir_for(source: &str, path: &Path) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let odp = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("odp"));
    let leaf = if odp {
        format!("{stem}-odp")
    } else {
        stem.to_string()
    };
    Path::new(CACHE).join("pptx-ref").join(source).join(leaf)
}

fn collect_decks() -> Vec<Deck> {
    let mut v = Vec::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Ok(txt) = std::fs::read_to_string(Path::new(CACHE).join("pptx-corpus/index.json")) {
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(&txt)
        {
            for it in items {
                if it["status"].as_str() != Some("ok") {
                    continue;
                }
                let (Some(src), Some(file)) = (it["source"].as_str(), it["file"].as_str()) else {
                    continue;
                };
                let p = Path::new(CACHE).join("pptx-corpus").join(file);
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                let odp = p.extension().is_some_and(|e| e == "odp");
                v.push(Deck {
                    name: format!("{src}__{stem}{}", if odp { "-odp" } else { "" }),
                    ref_dir: ref_dir_for(src, &p),
                    path: p,
                });
            }
        }
    }
    let mut extra: Vec<(&str, PathBuf)> = Vec::new();
    for (tag, dir) in [
        ("selfmade", Path::new(CACHE).join("slide-corpus-gen")),
        ("own", root.join("samples")),
        ("own", root.join("testdata/office")),
    ] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for f in rd.flatten() {
                let p = f.path();
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if matches!(ext, "pptx" | "odp") && (tag != "selfmade" || name.starts_with("draw-"))
                {
                    extra.push((tag, p));
                }
            }
        }
    }
    extra.sort();
    for (tag, p) in extra {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        let odp = p.extension().is_some_and(|e| e == "odp");
        v.push(Deck {
            name: format!("{tag}__{stem}{}", if odp { "-odp" } else { "" }),
            ref_dir: ref_dir_for(tag, &p),
            path: p,
        });
    }
    v
}

fn load(path: &Path) -> Result<Document, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        load_presentation(path, &DocOptions::default())
    })) {
        Ok(Ok(d)) => Ok(d),
        Ok(Err(e)) => Err(format!("{e:?}")),
        Err(_) => Err("panic".into()),
    }
}

type Cached = (PathBuf, Document, HashMap<String, Arc<Vec<u8>>>);
type Drawn = (Option<RgbaImage>, f64, usize, bool, usize);

/// Draws one slide: `(rgba, ms, svg bytes, scene truncated, item count)`.
fn draw(doc: &Document, i: usize, media: &HashMap<String, Arc<Vec<u8>>>) -> Drawn {
    let sc = &doc.slide_scenes[i];
    let lookup = |k: &str| media.get(k).cloned();
    let t = Instant::now();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let r = sd::render_svg(sc, &lookup);
        let img =
            crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/slide.svg"), 1280);
        (r, img)
    }));
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    match r {
        Ok((r, img)) => (
            img.map(|i| on_white(&i)),
            ms,
            r.svg.len(),
            r.truncated,
            count_items(&sc.items),
        ),
        Err(_) => (None, ms, 0, false, count_items(&sc.items)),
    }
}

fn sbs(ours: Option<&RgbaImage>, reference: Option<&RgbaImage>) -> RgbaImage {
    const HALF: u32 = 640;
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
        .max(360);
    let mut c = RgbaImage::from_pixel(HALF * 2 + 8, h, Px([120, 120, 120, 255]));
    if let Some(l) = &l {
        imageops::overlay(&mut c, l, 0, 0);
    }
    if let Some(r) = &r {
        imageops::overlay(&mut c, r, i64::from(HALF) + 8, 0);
    }
    c
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore = "reads the local corpus and LibreOffice references; writes fidelity-rank outputs and docs/render-check/rank/"]
fn fidelity_rank_whole_corpus() {
    let decks = collect_decks();
    assert!(!decks.is_empty(), "no decks under {CACHE}");
    let mut rows: Vec<Row> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut unref: Vec<String> = Vec::new();
    let t0 = Instant::now();
    for d in &decks {
        let doc = match load(&d.path) {
            Ok(x) => x,
            Err(e) => {
                let has_ref = d.ref_dir.join("slide-001.png").exists();
                failures.push((
                    d.name.clone(),
                    format!(
                        "{e} (LibreOffice reference {})",
                        if has_ref { "exists" } else { "absent" }
                    ),
                ));
                continue;
            }
        };
        if !d.ref_dir.join("slide-001.png").exists() {
            unref.push(d.name.clone());
            continue;
        }
        let media = media_of(&doc);
        for i in 0..doc.slide_scenes.len().min(MAX_SLIDES) {
            let (img, ms, svg_bytes, trunc, items) = draw(&doc, i, &media);
            let mut flags = Vec::new();
            let scene_truncated = trunc || doc.slide_scenes[i].truncated;
            if scene_truncated {
                flags.push("scene-truncated");
            }
            if doc.truncated {
                flags.push("deck-truncated");
            }
            let refp = d.ref_dir.join(format!("slide-{:03}.png", i + 1));
            let Some(reference) = image::open(&refp).ok().map(|x| on_white(&x)) else {
                continue;
            };
            let (mad, s, hd, comb) = match &img {
                Some(a) => {
                    let (mad, s, hd, asp) = scores(a, &reference);
                    if asp {
                        flags.push("aspect");
                    }
                    (mad, s, hd, combined(mad, s, hd))
                }
                None => {
                    flags.push("raster-fail");
                    (1.0, 0.0, 1.0, 1.0)
                }
            };
            rows.push(Row {
                deck: d.name.clone(),
                path: d.path.clone(),
                ref_dir: d.ref_dir.clone(),
                slide: i + 1,
                mad,
                ssim: s,
                hist: hd,
                combined: comb,
                scene_truncated,
                deck_truncated: doc.truncated,
                ms,
                svg_bytes,
                items,
                flags,
            });
        }
    }
    rows.sort_by(|a, b| b.combined.total_cmp(&a.combined));

    let out = Path::new(OUT_DIR);
    let _ = std::fs::create_dir_all(out);
    let mut csv = String::from(
        "rank,deck,slide,combined,mad,ssim,hist,scene_truncated,deck_truncated,render_ms,svg_bytes,items,flags\n",
    );
    for (n, r) in rows.iter().enumerate() {
        csv.push_str(&format!(
            "{},{},{},{:.4},{:.4},{:.4},{:.4},{},{},{:.1},{},{},{}\n",
            n + 1,
            r.deck.replace(',', "_"),
            r.slide,
            r.combined,
            r.mad,
            r.ssim,
            r.hist,
            r.scene_truncated,
            r.deck_truncated,
            r.ms,
            r.svg_bytes,
            r.items,
            r.flags.join("|")
        ));
    }
    std::fs::write(out.join("ranking.csv"), csv).unwrap();

    // Side-by-side pictures of the worst slides (decks reloaded one at a time).
    let sbs_dir = Path::new(SBS_DIR);
    let _ = std::fs::create_dir_all(sbs_dir);
    if let Ok(rd) = std::fs::read_dir(sbs_dir) {
        for f in rd.flatten() {
            if f.path().extension().is_some_and(|e| e == "png") {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
    let mut cache: Option<Cached> = None;
    for (n, r) in rows.iter().take(WORST).enumerate() {
        if cache.as_ref().map(|c| &c.0) != Some(&r.path) {
            cache = load(&r.path).ok().map(|d| {
                let m = media_of(&d);
                (r.path.clone(), d, m)
            });
        }
        let Some((_, doc, media)) = &cache else {
            continue;
        };
        let (img, ..) = draw(doc, r.slide - 1, media);
        let reference = image::open(r.ref_dir.join(format!("slide-{:03}.png", r.slide)))
            .ok()
            .map(|x| on_white(&x));
        let name = format!("{:03}-{}-{:02}.png", n + 1, r.deck, r.slide);
        let _ = sbs(img.as_ref(), reference.as_ref()).save(sbs_dir.join(name));
    }

    // Summary.
    let mut c: Vec<f64> = rows.iter().map(|r| r.combined).collect();
    c.sort_by(f64::total_cmp);
    let mut md = String::new();
    md.push_str("# Slide fidelity ranking\n\n");
    md.push_str(&format!(
        "{} decks listed, {} slides scored (first {MAX_SLIDES} per deck), {} decks konoma failed to load, {} decks without a LibreOffice reference, total {:.0?}.\n\n",
        decks.len(),
        rows.len(),
        failures.len(),
        unref.len(),
        t0.elapsed()
    ));
    md.push_str("Score: `combined = 0.35*min(1,mad/0.25) + 0.45*(1-max(0,ssim)) + 0.20*min(1,hist/0.5)` (0 identical, 1 unrelated); see the module doc in `src/preview/office/tests_fidelity_rank.rs`.\n\n");
    md.push_str("## Distribution of `combined`\n\n| p10 | p25 | p50 | p75 | p90 | p95 | p99 | max |\n|---|---|---|---|---|---|---|---|\n");
    md.push_str(&format!(
        "| {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} |\n\n",
        percentile(&c, 0.1),
        percentile(&c, 0.25),
        percentile(&c, 0.5),
        percentile(&c, 0.75),
        percentile(&c, 0.9),
        percentile(&c, 0.95),
        percentile(&c, 0.99),
        percentile(&c, 1.0)
    ));
    md.push_str("Histogram (combined, bins of 0.1): ");
    for b in 0..10 {
        let n = c
            .iter()
            .filter(|v| (**v * 10.0).floor().min(9.0) as usize == b)
            .count();
        md.push_str(&format!(
            "[{:.1}-{:.1}) {} ; ",
            b as f64 / 10.0,
            (b + 1) as f64 / 10.0,
            n
        ));
    }
    md.push_str("\n\nFlag counts: ");
    for f in ["scene-truncated", "deck-truncated", "aspect", "raster-fail"] {
        md.push_str(&format!(
            "{f} {} ; ",
            rows.iter().filter(|r| r.flags.contains(&f)).count()
        ));
    }
    md.push_str("\n\n## Decks konoma failed to load\n\n");
    for (n, e) in &failures {
        md.push_str(&format!("- {n}: {e}\n"));
    }
    md.push_str("\n## Decks without LibreOffice reference (not scored)\n\n");
    for n in &unref {
        md.push_str(&format!("- {n}\n"));
    }
    md.push_str("\n## Slowest 20 (render_svg + rasterize, in process)\n\n| deck | slide | ms | svg bytes | items |\n|---|---|---|---|---|\n");
    let mut by_ms: Vec<&Row> = rows.iter().collect();
    by_ms.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    for r in by_ms.iter().take(20) {
        md.push_str(&format!(
            "| {} | {} | {:.0} | {} | {} |\n",
            r.deck, r.slide, r.ms, r.svg_bytes, r.items
        ));
    }
    md.push_str("\n## Largest 20 SVGs\n\n| deck | slide | svg bytes | ms | items |\n|---|---|---|---|---|\n");
    by_ms.sort_by_key(|r| std::cmp::Reverse(r.svg_bytes));
    for r in by_ms.iter().take(20) {
        md.push_str(&format!(
            "| {} | {} | {} | {:.0} | {} |\n",
            r.deck, r.slide, r.svg_bytes, r.ms, r.items
        ));
    }
    md.push_str(&format!(
        "\n## Worst {WORST}\n\n| # | deck | slide | combined | mad | ssim | hist | flags |\n|---|---|---|---|---|---|---|---|\n"
    ));
    for (n, r) in rows.iter().take(WORST).enumerate() {
        md.push_str(&format!(
            "| {} | {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {} |\n",
            n + 1,
            r.deck,
            r.slide,
            r.combined,
            r.mad,
            r.ssim,
            r.hist,
            r.flags.join(" ")
        ));
    }
    std::fs::write(out.join("summary.md"), md).unwrap();
    eprintln!(
        "fidelity-rank: {} slides scored in {:.0?}; see {OUT_DIR}",
        rows.len(),
        t0.elapsed()
    );
}
