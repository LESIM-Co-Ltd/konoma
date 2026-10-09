//! Ignored dump for a human: slides with charts or SmartArt drawn by konoma, side by side with
//! the picture LibreOffice made of the same slide (`docs/render-check/f2/`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use image::{imageops, RgbaImage};

use super::super::{load_presentation, DocOptions};
use crate::preview::office::slide_draw as sd;

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/f2";
/// Most slides of one deck that are put in the dump.
const PER_DECK: usize = 10;

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
#[ignore = "reads the local corpora; writes docs/render-check/f2/*.png"]
fn f2_dump_render_check() {
    let out_dir = std::env::var("F2_OUT").unwrap_or_else(|_| OUT.to_string());
    let out = Path::new(&out_dir);
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

/// Synthetic charts for the data-label placement (pie best fit with leader lines, overlapping
/// point labels): `docs/render-check/f2/synth-*.png`.
#[test]
#[ignore = "writes docs/render-check/f2/synth-*.png"]
fn f2_synthetic_label_charts() {
    use crate::preview::office::chart_xml::{parse_chart, ChartEnv};
    use crate::preview::office::slide_draw::chart::draw_chart;
    use crate::preview::office::slide_draw::{Fill, Rgba, SlideScene};

    let out_dir = std::env::var("F2_OUT").unwrap_or_else(|_| OUT.to_string());
    let out = Path::new(&out_dir);
    let _ = std::fs::create_dir_all(out);
    let ns = r#"xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;
    let cats = [
        "Asia",
        "Europe",
        "Africa",
        "N. America",
        "S. America",
        "Oceania",
        "Antarctica",
        "Arctic",
        "Other",
        "More",
    ];
    let cache = |n: usize, vals: &[f64]| {
        let c: String = (0..n)
            .map(|i| {
                format!(
                    r#"<c:pt idx="{i}"><c:v>{}</c:v></c:pt>"#,
                    cats[i % cats.len()]
                )
            })
            .collect();
        let v: String = vals
            .iter()
            .enumerate()
            .map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#))
            .collect();
        format!(
            r#"<c:cat><c:strRef><c:f>c</c:f><c:strCache><c:ptCount val="{n}"/>{c}</c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>v</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="{n}"/>{v}</c:numCache></c:numRef></c:val>"#
        )
    };
    let lbl = |pos: &str, cat: &str| {
        format!(
            r#"<c:dLbls><c:dLblPos val="{pos}"/><c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="{cat}"/><c:showSerName val="0"/><c:showPercent val="0"/></c:dLbls>"#
        )
    };
    let ser = |idx: usize, name: &str, extra: &str, vals: &[f64]| {
        format!(
            r#"<c:ser><c:idx val="{idx}"/><c:order val="{idx}"/><c:tx><c:strRef><c:f>x</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>{name}</c:v></c:pt></c:strCache></c:strRef></c:tx>{extra}{}</c:ser>"#,
            cache(vals.len(), vals)
        )
    };
    let pie = format!(
        r#"<c:chartSpace {ns}><c:chart><c:plotArea><c:pieChart><c:varyColors val="1"/>{}<c:firstSliceAng val="0"/></c:pieChart></c:plotArea><c:legend><c:legendPos val="r"/></c:legend></c:chart></c:chartSpace>"#,
        ser(
            0,
            "Share",
            &lbl("bestFit", "1"),
            &[40.0, 24.0, 14.0, 9.0, 5.0, 4.0, 3.0, 1.0]
        )
    );
    let pie_tight = format!(
        r#"<c:chartSpace {ns}><c:chart><c:plotArea><c:pieChart><c:varyColors val="1"/>{}<c:firstSliceAng val="300"/></c:pieChart></c:plotArea></c:chart></c:chartSpace>"#,
        ser(
            0,
            "Share",
            &lbl("outEnd", "1"),
            &[30.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 62.0]
        )
    );
    let two_lines = format!(
        r#"<c:chartSpace {ns}><c:chart><c:plotArea><c:lineChart><c:grouping val="standard"/>{}{}<c:marker val="1"/><c:axId val="1"/><c:axId val="2"/></c:lineChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:crossAx val="1"/></c:valAx></c:plotArea><c:legend><c:legendPos val="b"/></c:legend></c:chart></c:chartSpace>"#,
        ser(0, "A", &lbl("t", "0"), &[5.0, 6.0, 7.0, 6.5, 8.0]),
        ser(1, "B", &lbl("t", "0"), &[5.1, 6.1, 6.9, 6.6, 8.1])
    );
    let accents: Vec<Rgba> = vec![
        Rgba::rgb(0x44, 0x72, 0xC4),
        Rgba::rgb(0xED, 0x7D, 0x31),
        Rgba::rgb(0xA5, 0xA5, 0xA5),
        Rgba::rgb(0xFF, 0xC0, 0x00),
        Rgba::rgb(0x5B, 0x9B, 0xD5),
        Rgba::rgb(0x70, 0xAD, 0x47),
    ];
    let res = |n: &crate::preview::office::docx_xml::Node| -> Option<Rgba> {
        match n.name.as_str() {
            "srgbClr" => Rgba::from_hex(n.attr("val")?),
            _ => None,
        }
    };
    let env = ChartEnv {
        resolve_color: &res,
        palette: &accents,
        minor_font: Some("Calibri"),
        major_font: Some("Calibri"),
    };
    for (name, xml) in [
        ("pie", pie),
        ("pie-tight", pie_tight),
        ("two-lines", two_lines),
    ] {
        let parsed = parse_chart(xml.as_bytes(), &env).expect("parses");
        let (w, h) = (6_000_000.0, 4_000_000.0);
        let (items, _) = draw_chart(&parsed.model, w, h);
        let scene = SlideScene {
            width: w,
            height: h,
            background: Fill::Solid(Rgba::WHITE),
            underlay: Vec::new(),
            items,
            truncated: false,
        };
        let r = sd::render_svg(&scene, &|_| None);
        let img =
            crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/s.svg"), 1100)
                .expect("rasterizes")
                .to_rgba8();
        img.save(out.join(format!("synth-{name}.png"))).unwrap();
    }
}
