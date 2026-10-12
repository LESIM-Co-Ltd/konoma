//! Local-only checks of the ODF geometry on real files (`#[ignore]`d: they read files that are not
//! in the repository, and write images into `docs/render-check/odf-geom/` of the main checkout).
//!
//! * `survey`: counts the custom shapes of the OpenDocument corpus, how many carry a path, and
//!   which `draw:type`s do not.
//! * `compare_with_libreoffice`: draws the custom shapes of some decks through
//!   [`render_svg`](super::super::render_svg) and the project's rasterizer, and writes pages with
//!   konoma on the left and LibreOffice's own rendering on the right.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::super::{
    render_svg, Fill, Geometry, Item, Line, Rgba, ShapeItem, SlideScene, Xfrm, EMU_PER_PX,
};
use super::*;
use crate::preview::office::docx_xml::{read_element, Budget, Tree};
use crate::preview::office::fmt_xlsx::XmlReader;
use quick_xml::events::Event;

const CORPUS: &str = "/Users/shuhei/work/NoCode/.cache/pptx-corpus/libreoffice";
const GEN: &str = "/Users/shuhei/work/NoCode/.cache/slide-corpus-gen/draw-shapes.odp";
const REF_LO: &str = "/Users/shuhei/work/NoCode/.cache/pptx-ref/libreoffice";
const REF_SELF: &str = "/Users/shuhei/work/NoCode/.cache/pptx-ref/selfmade/draw-shapes-odp";
const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/odf-geom";

fn zip_part(odp: &Path, name: &str) -> Option<Node> {
    let mut z = zip::ZipArchive::new(File::open(odp).ok()?).ok()?;
    let mut buf = Vec::new();
    z.by_name(name).ok()?.read_to_end(&mut buf).ok()?;
    let mut rd = XmlReader::new(buf.as_slice());
    let mut b = Vec::new();
    loop {
        let (e, empty) = match rd.read_event_into(&mut b).ok()? {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof => return None,
            _ => continue,
        };
        let mut budget = Budget::odf(50_000_000, 1 << 30);
        return match read_element(&mut rd, &e, empty, &mut budget).ok()? {
            Tree::Ok(n) => Some(n),
            Tree::TooBig => None,
        };
    }
}

fn walk<'a>(n: &'a Node, f: &mut dyn FnMut(&'a Node)) {
    f(n);
    for k in n.nodes() {
        walk(k, f);
    }
}

fn odps() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(CORPUS)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "odp"))
        .collect();
    v.sort();
    v.push(PathBuf::from(GEN));
    v
}

fn geometry_of(shape: &Node) -> Option<&Node> {
    shape.child("enhanced-geometry")
}

#[test]
#[ignore = "reads the local OpenDocument corpus"]
fn survey() {
    let (mut files, mut shapes, mut with_path, mut drawooo, mut ok) = (0, 0, 0, 0, 0);
    let mut missing: BTreeMap<String, usize> = BTreeMap::new();
    let mut failed = Vec::new();
    let mut cmd_letters: BTreeMap<char, usize> = BTreeMap::new();
    for odp in odps() {
        let Some(root) = zip_part(&odp, "content.xml") else {
            println!("unreadable: {}", odp.display());
            continue;
        };
        files += 1;
        walk(&root, &mut |n| {
            if n.name != "custom-shape" {
                return;
            }
            let Some(g) = geometry_of(n) else { return };
            shapes += 1;
            let ty = g.attr("type").unwrap_or("-").to_string();
            let path = g.attrs.iter().find(|(k, _)| k.ends_with("enhanced-path"));
            if g.attrs
                .iter()
                .any(|(k, _)| k.starts_with("drawooo:enhanced-path"))
            {
                drawooo += 1;
            }
            let Some((_, p)) = path else {
                *missing.entry(ty).or_default() += 1;
                return;
            };
            with_path += 1;
            for c in p.chars().filter(char::is_ascii_uppercase) {
                *cmd_letters.entry(c).or_default() += 1;
            }
            // a nominal 5 x 3 cm shape when the real size cannot be read
            if enhanced_geometry(g, 5.0 * 360_000.0, 3.0 * 360_000.0).is_some() {
                ok += 1;
            } else {
                failed.push((odp.file_name().unwrap().to_string_lossy().into_owned(), ty));
            }
        });
    }
    println!("files {files}  custom shapes {shapes}  with draw:enhanced-path {with_path}  (drawooo: {drawooo})  evaluated {ok}");
    println!("without a path: {} shapes", missing.values().sum::<usize>());
    for (t, n) in &missing {
        println!("  {n:3} {t}");
    }
    println!("path command letters: {cmd_letters:?}");
    println!("evaluation returned None: {failed:?}");
    assert!(
        failed.is_empty(),
        "every shape with a path must evaluate: {failed:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Side by side with LibreOffice
// ---------------------------------------------------------------------------------------------

/// A length in EMU (`2.8cm`, `10mm`, `0.5in`, `12pt`, `3px`).
fn len_emu(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let v: f64 = s[..split].parse().ok()?;
    let per = match &s[split..] {
        "cm" => 360_000.0,
        "mm" => 36_000.0,
        "in" => 914_400.0,
        "pt" => 12_700.0,
        "px" => EMU_PER_PX,
        _ => return None,
    };
    Some(v * per)
}

fn hex(s: &str) -> Option<Rgba> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// The graphic properties of every named and automatic style, parents resolved.
struct Styles {
    props: HashMap<String, Vec<(String, String)>>,
    parent: HashMap<String, String>,
}

impl Styles {
    fn read(roots: &[&Node]) -> Styles {
        let mut s = Styles {
            props: HashMap::new(),
            parent: HashMap::new(),
        };
        for r in roots {
            walk(r, &mut |n| {
                if n.name == "style" && n.attr("family") == Some("graphic") {
                    let Some(name) = n.attr("name") else { return };
                    if let Some(p) = n.attr("parent-style-name") {
                        s.parent.insert(name.to_string(), p.to_string());
                    }
                    if let Some(g) = n.child("graphic-properties") {
                        s.props.insert(name.to_string(), g.attrs.clone());
                    }
                }
            });
        }
        s
    }

    fn get(&self, style: &str, key: &str) -> Option<String> {
        let mut cur = Some(style);
        for _ in 0..10 {
            let c = cur?;
            if let Some(a) = self.props.get(c) {
                if let Some((_, v)) = a
                    .iter()
                    .find(|(k, _)| k.ends_with(key) && k.split(':').nth(1) == Some(key))
                {
                    return Some(v.clone());
                }
            }
            cur = self.parent.get(c).map(String::as_str);
        }
        None
    }
}

struct Page {
    items: Vec<Item>,
    labels: Vec<(f64, f64, String)>,
    no_path: Vec<String>,
}

fn page_items(page: &Node, st: &Styles) -> Page {
    let mut out = Page {
        items: Vec::new(),
        labels: Vec::new(),
        no_path: Vec::new(),
    };
    walk(page, &mut |n| {
        let (x, y, w, h) = (
            n.attr("x").and_then(len_emu),
            n.attr("y").and_then(len_emu),
            n.attr("width").and_then(len_emu),
            n.attr("height").and_then(len_emu),
        );
        // a `translate (x y)` transform stands in for x / y (rotation is not applied here)
        let (mut x, mut y) = (x, y);
        if let Some(t) = n.attr("transform") {
            if let Some(i) = t.find("translate") {
                let inner = t[i..]
                    .trim_start_matches("translate")
                    .trim()
                    .trim_matches(|c| c == '(' || c == ')');
                let mut it = inner.split_whitespace();
                x = it.next().and_then(len_emu);
                y = it.next().and_then(len_emu);
            }
        }
        let (Some(x), Some(y), Some(w), Some(h)) = (x, y, w, h) else {
            return;
        };
        if n.name == "frame" {
            let text: String = {
                let mut t = String::new();
                walk(n, &mut |m| {
                    if m.name == "p" || m.name == "span" {
                        t.push_str(&m.text());
                    }
                });
                t
            };
            if !text.trim().is_empty() {
                out.labels.push((x, y, text.trim().to_string()));
            }
            return;
        }
        if n.name != "custom-shape" {
            return;
        }
        let Some(g) = geometry_of(n) else { return };
        let ty = g.attr("type").unwrap_or("-").to_string();
        let Some((paths, _text)) = enhanced_geometry(g, w, h) else {
            out.no_path.push(ty);
            return;
        };
        let style = n.attr("style-name").unwrap_or("standard");
        let fill = match st.get(style, "fill").as_deref() {
            Some("none") => Fill::None,
            _ => Fill::Solid(
                st.get(style, "fill-color")
                    .and_then(|c| hex(&c))
                    .unwrap_or(Rgba::rgb(0x72, 0x9f, 0xcf)),
            ),
        };
        let line = match st.get(style, "stroke").as_deref() {
            Some("none") => None,
            _ => Some(Line::solid(
                (st.get(style, "stroke-width")
                    .and_then(|v| len_emu(&v))
                    .unwrap_or(0.0))
                .max(9525.0 * 0.5),
                st.get(style, "stroke-color")
                    .and_then(|c| hex(&c))
                    .unwrap_or(Rgba::rgb(0x34, 0x65, 0xa4)),
            )),
        };
        let mut s = ShapeItem::new(Xfrm::rect(x, y, w, h), Geometry::Paths(paths));
        s.fill = fill;
        s.line = line;
        out.items.push(Item::Shape(s));
    });
    out
}

fn svg_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn load_png(p: &Path) -> Option<image::RgbaImage> {
    image::open(p).ok().map(|i| i.to_rgba8())
}

/// konoma | LibreOffice, side by side. Returns the number of shapes drawn and without a path.
fn compare(odp: &Path, ref_dir: &Path, stem: &str) -> (usize, usize) {
    let content = zip_part(odp, "content.xml").expect("content.xml");
    let styles = zip_part(odp, "styles.xml").expect("styles.xml");
    let st = Styles::read(&[&styles, &content]);
    let mut layouts: HashMap<String, (f64, f64)> = HashMap::new();
    let mut master_layout: HashMap<String, String> = HashMap::new();
    for root in [&styles, &content] {
        walk(root, &mut |n| {
            if n.name == "page-layout" {
                if let (Some(name), Some(pp)) = (n.attr("name"), n.child("page-layout-properties"))
                {
                    if let (Some(w), Some(h)) = (
                        pp.attr("page-width").and_then(len_emu),
                        pp.attr("page-height").and_then(len_emu),
                    ) {
                        layouts.insert(name.to_string(), (w, h));
                    }
                }
            }
            if n.name == "master-page" {
                if let (Some(name), Some(l)) = (n.attr("name"), n.attr("page-layout-name")) {
                    master_layout.insert(name.to_string(), l.to_string());
                }
            }
        });
    }
    let mut pages = Vec::new();
    walk(&content, &mut |n| {
        if n.name == "page" && n.attr("master-page-name").is_some() {
            pages.push(n);
        }
    });
    let (mut drawn, mut missing) = (0, 0);
    for (i, page) in pages.iter().enumerate() {
        let (sw, sh) = page
            .attr("master-page-name")
            .and_then(|m| master_layout.get(m))
            .and_then(|l| layouts.get(l))
            .copied()
            .expect("page size");
        let p = page_items(page, &st);
        drawn += p.items.len();
        missing += p.no_path.len();
        let scene = SlideScene {
            width: sw,
            height: sh,
            background: Fill::Solid(Rgba::WHITE),
            underlay: Vec::new(),
            items: p.items,
            truncated: false,
        };
        let mut svg = render_svg(&scene, &|_| None).svg;
        let mut extra = String::new();
        for (x, y, t) in &p.labels {
            extra.push_str(&format!(
                r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="9" fill="#000">{}</text>"##,
                x / EMU_PER_PX + 2.0,
                y / EMU_PER_PX + 10.0,
                svg_escape(t)
            ));
        }
        if let Some(i) = svg.rfind("</svg>") {
            svg.insert_str(i, &extra);
        }
        let Some(mine) =
            crate::preview::svg::rasterize_trusted(svg.as_bytes(), Path::new("/slide.svg"), 1280)
        else {
            panic!("page {} does not rasterize", i + 1);
        };
        let mine = mine.to_rgba8();
        let theirs = load_png(&ref_dir.join(format!("slide-{:03}.png", i + 1)));
        let (tw, th) = theirs
            .as_ref()
            .map_or((mine.width(), mine.height()), |t| (t.width(), t.height()));
        let left = image::imageops::resize(&mine, tw, th, image::imageops::FilterType::Lanczos3);
        let mut canvas =
            image::RgbaImage::from_pixel(tw * 2 + 12, th, image::Rgba([200, 60, 60, 255]));
        image::imageops::replace(&mut canvas, &left, 0, 0);
        if let Some(t) = &theirs {
            image::imageops::replace(&mut canvas, t, i64::from(tw) + 12, 0);
        }
        let out = Path::new(OUT).join(format!("{stem}-s{:02}.png", i + 1));
        canvas.save(&out).expect("save");
        if !p.no_path.is_empty() {
            println!("{stem} s{}: no path for {:?}", i + 1, p.no_path);
        }
    }
    (drawn, missing)
}

#[test]
#[ignore = "reads the local corpus, writes into docs/render-check/odf-geom of the main checkout"]
fn compare_with_libreoffice() {
    std::fs::create_dir_all(OUT).unwrap();
    for e in std::fs::read_dir(OUT).unwrap().flatten() {
        if e.path().extension().is_some_and(|x| x == "png") {
            std::fs::remove_file(e.path()).unwrap();
        }
    }
    let jobs: Vec<(PathBuf, PathBuf, &str)> = vec![
        (
            Path::new(CORPUS).join("odp__shapes-test.odp"),
            Path::new(REF_LO).join("odp__shapes-test-odp"),
            "shapes-test",
        ),
        (
            Path::new(CORPUS).join("odp__preset-shapes-export.odp"),
            Path::new(REF_LO).join("odp__preset-shapes-export-odp"),
            "preset-shapes-export",
        ),
        (PathBuf::from(GEN), PathBuf::from(REF_SELF), "draw-shapes"),
    ];
    for (odp, r, stem) in jobs {
        let (drawn, missing) = compare(&odp, &r, stem);
        println!("{stem}: {drawn} shapes drawn, {missing} without a path");
    }
}
