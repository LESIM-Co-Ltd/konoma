//! A human-readable check of every preset: konoma (left) next to LibreOffice (right).
//!
//! The reference is a self-made deck (`NoCode/.cache/slide-corpus-gen/draw-shapes.pptx`) that lays
//! the presets out 24 to a slide, and LibreOffice's rendering of it. This test reads the deck,
//! draws the same shapes with [`super::preset`], and writes side-by-side images.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;

use super::super::model::*;
use super::super::render_svg;
use super::{preset, preset_names};
use crate::preview::office::docx_xml::{read_element, Budget, Node, Tree};
use crate::preview::office::fmt_xlsx::XmlReader;

const DECK: &str = "/Users/shuhei/work/NoCode/.cache/slide-corpus-gen/draw-shapes.pptx";
const REF_DIR: &str = "/Users/shuhei/work/NoCode/.cache/pptx-ref/selfmade/draw-shapes";
const OUT_DIR: &str = "/Users/shuhei/work/konoma/docs/render-check/presets";
const SLIDE_W: f64 = 10_080_625.0;
const SLIDE_H: f64 = 5_670_550.0;

fn parse(xml: &[u8]) -> Node {
    let mut rd = XmlReader::new(xml);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let (e, empty) = match rd.read_event_into(&mut buf).unwrap() {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof => panic!("empty part"),
            _ => continue,
        };
        let mut b = Budget::new(2_000_000, 64 << 20);
        match read_element(&mut rd, &e, empty, &mut b).unwrap() {
            Tree::Ok(n) => return n,
            Tree::TooBig => panic!("too big"),
        }
    }
}

fn color_of(n: Option<&Node>) -> Option<Rgba> {
    let c = n?.child("solidFill")?.child("srgbClr")?.attr("val")?;
    Rgba::from_hex(c)
}

fn num(n: &Node, a: &str, d: f64) -> f64 {
    n.attr(a).and_then(|v| v.parse().ok()).unwrap_or(d)
}

/// One slide of the deck as a scene (presets drawn by the engine), plus the preset names on it.
fn scene_of(slide: &Node, names: &mut Vec<String>) -> SlideScene {
    let tree = slide
        .child("cSld")
        .and_then(|c| c.child("spTree"))
        .expect("spTree");
    let mut items = Vec::new();
    for sp in tree.nodes().filter(|n| n.name == "sp") {
        let Some(sppr) = sp.child("spPr") else {
            continue;
        };
        let (Some(xf), Some(prst)) = (sppr.child("xfrm"), sppr.child("prstGeom")) else {
            continue;
        };
        let (off, ext) = (xf.child("off").unwrap(), xf.child("ext").unwrap());
        let (x, y, w, h) = (
            num(off, "x", 0.0),
            num(off, "y", 0.0),
            num(ext, "cx", 0.0),
            num(ext, "cy", 0.0),
        );
        let name = prst.attr("prst").unwrap_or("rect").to_string();
        let adj: Vec<(String, f64)> = prst
            .child("avLst")
            .map(|a| {
                a.nodes()
                    .filter(|g| g.name == "gd")
                    .filter_map(|g| {
                        let v = g.attr("fmla")?.strip_prefix("val ")?.trim().parse().ok()?;
                        Some((g.attr("name")?.to_string(), v))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut xfrm = Xfrm::rect(x, y, w, h);
        xfrm.rot_deg = num(xf, "rot", 0.0) / 60_000.0;
        xfrm.flip_h = xf.attr("flipH") == Some("1");
        xfrm.flip_v = xf.attr("flipV") == Some("1");
        let g = preset(&name, &adj, w, h).unwrap_or_else(|| panic!("preset {name}"));
        let mut s = ShapeItem::new(xfrm, Geometry::Paths(g.paths));
        s.text_rect = g.text_rect;
        if let Some(c) = color_of(Some(sppr)) {
            s.fill = Fill::Solid(c);
        }
        if let Some(ln) = sppr.child("ln") {
            if let Some(c) = color_of(Some(ln)) {
                s.line = Some(Line::solid(num(ln, "w", 0.0), c));
            }
        }
        if let Some(tb) = sp.child("txBody") {
            let bp = tb.child("bodyPr");
            let inset = |a: &str, d: f64| bp.map_or(d, |b| num(b, a, d));
            let mut body = TextBody {
                insets: (
                    inset("lIns", 91_440.0),
                    inset("tIns", 45_720.0),
                    inset("rIns", 91_440.0),
                    inset("bIns", 45_720.0),
                ),
                ..TextBody::default()
            };
            for p in tb.nodes().filter(|p| p.name == "p") {
                let mut para = Paragraph::default();
                for r in p.nodes().filter(|r| r.name == "r") {
                    let t = r.child("t").map(Node::text).unwrap_or_default();
                    let rpr = r.child("rPr");
                    let mut run = Run::text(t, rpr.map_or(18.0, |x| num(x, "sz", 1800.0) / 100.0));
                    run.bold = rpr.is_some_and(|x| x.attr("b") == Some("1"));
                    run.font.latin = Some("Arial".into());
                    para.runs.push(run);
                }
                body.paragraphs.push(para);
            }
            if body.paragraphs.iter().any(|p| !p.runs.is_empty()) {
                s.text = Some(body);
            }
        }
        if !(name == "rect" && s.text.is_some()) {
            names.push(name);
        }
        items.push(Item::Shape(s));
    }
    SlideScene {
        width: SLIDE_W,
        height: SLIDE_H,
        background: Fill::Solid(Rgba::WHITE),
        underlay: Vec::new(),
        items,
        truncated: false,
    }
}

fn raster(scene: &SlideScene) -> image::RgbaImage {
    let r = render_svg(scene, &|_| None);
    crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/slide.svg"), 1280)
        .expect("the slide rasterizes")
        .to_rgba8()
}

fn side_by_side(left: &image::RgbaImage, right: Option<&image::RgbaImage>) -> image::RgbaImage {
    let (w, h) = (1280u32, 720u32);
    let mut out = image::RgbaImage::from_pixel(w * 2 + 8, h, image::Rgba([255, 255, 255, 255]));
    for x in w..w + 8 {
        for y in 0..h {
            out.put_pixel(x, y, image::Rgba([200, 0, 0, 255]));
        }
    }
    for (x, y, p) in left.enumerate_pixels() {
        if x < w && y < h {
            let a = p.0[3] as f32 / 255.0;
            let mix = |c: u8| (c as f32 * a + 255.0 * (1.0 - a)) as u8;
            out.put_pixel(
                x,
                y,
                image::Rgba([mix(p.0[0]), mix(p.0[1]), mix(p.0[2]), 255]),
            );
        }
    }
    if let Some(r) = right {
        for (x, y, p) in r.enumerate_pixels() {
            if x < w && y < h {
                out.put_pixel(x + w + 8, y, *p);
            }
        }
    }
    out
}

#[test]
#[ignore = "writes docs/render-check/presets/*.png for a human to look at"]
fn presets_against_libreoffice() {
    let dir = Path::new(OUT_DIR);
    let _ = std::fs::create_dir_all(dir);
    if let Ok(rd) = std::fs::read_dir(dir) {
        for f in rd.flatten() {
            let _ = std::fs::remove_file(f.path());
        }
    }
    let mut zip = zip::ZipArchive::new(std::fs::File::open(DECK).expect("the deck")).unwrap();
    let mut shown: HashSet<String> = HashSet::new();
    for i in 1..=8 {
        let mut xml = Vec::new();
        zip.by_name(&format!("ppt/slides/slide{i}.xml"))
            .unwrap()
            .read_to_end(&mut xml)
            .unwrap();
        let mut names = Vec::new();
        let sc = scene_of(&parse(&xml), &mut names);
        shown.extend(names);
        let left = raster(&sc);
        let right = image::open(format!("{REF_DIR}/slide-{i:03}.png"))
            .ok()
            .map(|i| i.to_rgba8());
        side_by_side(&left, right.as_ref())
            .save(dir.join(format!("presets-{i:02}.png")))
            .unwrap();
    }
    // presets the deck does not show: konoma only, same grid
    let missing: Vec<&str> = preset_names()
        .into_iter()
        .filter(|n| !shown.contains(*n))
        .collect();
    eprintln!("presets not in the reference deck: {missing:?}");
    for (page, chunk) in missing.chunks(24).enumerate() {
        let mut items = Vec::new();
        for (k, name) in chunk.iter().enumerate() {
            let (col, row) = ((k % 6) as f64, (k / 6) as f64);
            let (x, y) = (288_000.0 + col * 1_548_000.0, 612_000.0 + row * 1_080_000.0);
            let g = preset(name, &[], 1_008_000.0, 720_000.0).unwrap();
            let mut s = ShapeItem::new(
                Xfrm::rect(x, y, 1_008_000.0, 720_000.0),
                Geometry::Paths(g.paths),
            );
            s.fill = Fill::Solid(Rgba::from_hex("CFE2F3").unwrap());
            s.line = Some(Line::solid(0.0, Rgba::from_hex("1F4E79").unwrap()));
            items.push(Item::Shape(s));
            let mut label = ShapeItem::new(
                Xfrm::rect(x - 108_000.0, y + 720_000.0, 1_224_000.0, 216_000.0),
                Geometry::Rect,
            );
            let mut run = Run::text(*name, 8.0);
            run.font.latin = Some("Arial".into());
            label.text = Some(TextBody {
                insets: (90_000.0, 45_000.0, 90_000.0, 45_000.0),
                paragraphs: vec![Paragraph {
                    runs: vec![run],
                    ..Paragraph::default()
                }],
                ..TextBody::default()
            });
            items.push(Item::Shape(label));
        }
        let sc = SlideScene {
            width: SLIDE_W,
            height: SLIDE_H,
            background: Fill::Solid(Rgba::WHITE),
            underlay: Vec::new(),
            items,
            truncated: false,
        };
        side_by_side(&raster(&sc), None)
            .save(dir.join(format!("presets-extra-{}.png", page + 1)))
            .unwrap();
    }
}
