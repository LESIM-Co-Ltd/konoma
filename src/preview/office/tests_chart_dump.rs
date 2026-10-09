//! `#[ignore]` dump of every chart of the local corpora next to the LibreOffice reference, for
//! looking at (`cargo test chart_dump -- --ignored`). Needs files under `NoCode/.cache` that are
//! not in the repository; the images go to `docs/render-check/charts/` of the main tree.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::chart_xml::{parse_chart, ChartEnv};
use super::docx_xml::Node;
use super::slide_draw::color::{apply_mods, preset_color, system_color, ColorMod};
use super::slide_draw::{chart::draw_chart, render_svg, Fill, Item, Rgba, SlideScene};

const CACHE: &str = "/Users/shuhei/work/NoCode/.cache";
const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/charts";

fn read(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut f = zip.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

/// The text between `<tag ...` and the first `>` after it, from `from`.
fn tag_at<'a>(s: &'a str, tag: &str, from: usize) -> Option<(usize, &'a str)> {
    let i = s[from..].find(&format!("<{tag}"))? + from;
    let j = s[i..].find('>')? + i;
    Some((i, &s[i..=j]))
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let k = format!(" {name}=\"");
    let i = tag.find(&k)? + k.len();
    let j = tag[i..].find('"')? + i;
    Some(&tag[i..j])
}

struct Theme {
    colors: Vec<(String, Rgba)>,
    minor: Option<String>,
    major: Option<String>,
}

fn theme_of(zip: &mut zip::ZipArchive<std::fs::File>, part: &str) -> Theme {
    let t = read(zip, part).unwrap_or_default();
    let mut colors = Vec::new();
    for name in [
        "dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5",
        "accent6", "hlink", "folHlink",
    ] {
        let open = format!("<a:{name}>");
        if let Some(i) = t.find(&open) {
            let rest = &t[i + open.len()..];
            let c = if let Some((_, tag)) = tag_at(rest, "a:srgbClr", 0) {
                attr(tag, "val").and_then(Rgba::from_hex)
            } else if let Some((_, tag)) = tag_at(rest, "a:sysClr", 0) {
                attr(tag, "lastClr")
                    .and_then(Rgba::from_hex)
                    .or_else(|| attr(tag, "val").and_then(system_color))
            } else {
                None
            };
            if let Some(c) = c {
                colors.push((name.to_string(), c));
            }
        }
    }
    let font = |which: &str| {
        let open = format!("<a:{which}>");
        let i = t.find(&open)?;
        let (_, tag) = tag_at(&t[i..], "a:latin", 0)?;
        attr(tag, "typeface").map(str::to_string)
    };
    Theme {
        colors,
        minor: font("minorFont"),
        major: font("majorFont"),
    }
}

fn resolver(theme: &Theme) -> impl Fn(&Node) -> Option<Rgba> + '_ {
    move |n: &Node| {
        let v = n.attr("val")?;
        let mut base = match n.name.as_str() {
            "srgbClr" => Rgba::from_hex(v)?,
            "prstClr" => preset_color(v)?,
            "sysClr" => n
                .attr("lastClr")
                .and_then(Rgba::from_hex)
                .or_else(|| system_color(v))?,
            "schemeClr" => {
                let key = match v {
                    "tx1" => "dk1",
                    "bg1" => "lt1",
                    "tx2" => "dk2",
                    "bg2" => "lt2",
                    o => o,
                };
                theme.colors.iter().find(|(k, _)| k == key)?.1
            }
            _ => return None,
        };
        let mut mods = Vec::new();
        for c in n.nodes() {
            let f = c
                .attr("val")
                .and_then(|x| x.parse::<f64>().ok())
                .map(|x| x / 100_000.0);
            if let Some(f) = f {
                match c.name.as_str() {
                    "lumMod" => mods.push(ColorMod::LumMod(f)),
                    "lumOff" => mods.push(ColorMod::LumOff(f)),
                    "tint" => mods.push(ColorMod::Tint(f)),
                    "shade" => mods.push(ColorMod::Shade(f)),
                    "alpha" => mods.push(ColorMod::Alpha(f)),
                    "satMod" => mods.push(ColorMod::SatMod(f)),
                    _ => {}
                }
            }
        }
        base = apply_mods(base, &mods);
        Some(base)
    }
}

/// A chart on a slide: its part, the frame (EMU) and the slide's index.
struct Found {
    part: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    slide: usize,
    theme: String,
}

/// slide -> layout -> master -> theme, through the relationships.
fn theme_part(zip: &mut zip::ZipArchive<std::fs::File>, slide_rels: &str, slide: &str) -> String {
    fn target(rels: &str, kind: &str, base: &str) -> Option<String> {
        let mut f = 0;
        while let Some((j, rt)) = tag_at(rels, "Relationship ", f) {
            f = j + 1;
            if attr(rt, "Type").is_some_and(|t| t.ends_with(kind)) {
                let t = attr(rt, "Target")?;
                let mut parts: Vec<&str> = base.split('/').collect();
                parts.pop();
                for seg in t.split('/') {
                    match seg {
                        ".." => {
                            parts.pop();
                        }
                        "." => {}
                        s => parts.push(s),
                    }
                }
                return Some(parts.join("/"));
            }
        }
        None
    }
    let rels_of = |zip: &mut zip::ZipArchive<std::fs::File>, part: &str| {
        let (d, f) = part.rsplit_once('/').unwrap_or(("", part));
        read(zip, &format!("{d}/_rels/{f}.rels")).unwrap_or_default()
    };
    let fallback = "ppt/theme/theme1.xml".to_string();
    let Some(layout) = target(slide_rels, "slideLayout", slide) else {
        return fallback;
    };
    let lr = rels_of(zip, &layout);
    let Some(master) = target(&lr, "slideMaster", &layout) else {
        return fallback;
    };
    let mr = rels_of(zip, &master);
    target(&mr, "theme", &master).unwrap_or(fallback)
}

fn charts_of(zip: &mut zip::ZipArchive<std::fs::File>) -> (Vec<Found>, f64, f64) {
    let pres = read(zip, "ppt/presentation.xml").unwrap_or_default();
    let (sw, sh) = tag_at(&pres, "p:sldSz", 0)
        .map(|(_, t)| {
            (
                attr(t, "cx")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(9144000.0),
                attr(t, "cy")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(6858000.0),
            )
        })
        .unwrap_or((9144000.0, 6858000.0));
    // Slide order: presentation.xml.rels + sldIdLst.
    let prels = read(zip, "ppt/_rels/presentation.xml.rels").unwrap_or_default();
    let mut order: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some((i, tag)) = tag_at(&pres, "p:sldId ", from) {
        from = i + 1;
        let Some(rid) = attr(tag, "r:id") else {
            continue;
        };
        let mut f2 = 0;
        while let Some((j, rt)) = tag_at(&prels, "Relationship ", f2) {
            f2 = j + 1;
            if attr(rt, "Id") == Some(rid) {
                if let Some(t) = attr(rt, "Target") {
                    order.push(format!(
                        "ppt/{}",
                        t.trim_start_matches('/').trim_start_matches("ppt/")
                    ));
                }
            }
        }
    }
    let mut out = Vec::new();
    for (si, slide) in order.iter().enumerate() {
        let Some(xml) = read(zip, slide) else {
            continue;
        };
        let relname = {
            let (dir, file) = slide.rsplit_once('/').unwrap_or(("", slide));
            format!("{dir}/_rels/{file}.rels")
        };
        let rels = read(zip, &relname).unwrap_or_default();
        let theme = theme_part(zip, &rels, slide);
        let mut from = 0;
        while let Some(i) = xml[from..].find("<p:graphicFrame") {
            let i = i + from;
            let end = xml[i..]
                .find("</p:graphicFrame>")
                .map_or(xml.len(), |e| e + i);
            from = end.max(i + 1);
            let frame = &xml[i..end];
            let Some((_, ctag)) =
                tag_at(frame, "c:chart ", 0).or_else(|| tag_at(frame, "c:chart/", 0))
            else {
                continue;
            };
            let Some(rid) = attr(ctag, "r:id") else {
                continue;
            };
            let Some((_, off)) = tag_at(frame, "a:off", 0) else {
                continue;
            };
            let Some((_, ext)) = tag_at(frame, "a:ext ", 0) else {
                continue;
            };
            let num = |t: &str, k: &str| {
                attr(t, k)
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            };
            let mut target = None;
            let mut f2 = 0;
            while let Some((j, rt)) = tag_at(&rels, "Relationship ", f2) {
                f2 = j + 1;
                if attr(rt, "Id") == Some(rid) {
                    target = attr(rt, "Target").map(str::to_string);
                }
            }
            let Some(t) = target else { continue };
            let part = if let Some(rest) = t.strip_prefix("../") {
                format!("ppt/{rest}")
            } else {
                t.trim_start_matches('/').to_string()
            };
            out.push(Found {
                part,
                x: num(off, "x"),
                y: num(off, "y"),
                w: num(ext, "cx"),
                h: num(ext, "cy"),
                slide: si + 1,
                theme: theme.clone(),
            });
        }
    }
    (out, sw, sh)
}

fn dump_deck(pptx: &Path, refdir: &Path, tag: &str) -> usize {
    let Ok(file) = std::fs::File::open(pptx) else {
        return 0;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return 0;
    };
    let (found, sw, _sh) = charts_of(&mut zip);
    let mut n = 0;
    for (k, f) in found.iter().enumerate() {
        let Some(xml) = read(&mut zip, &f.part) else {
            continue;
        };
        let theme = theme_of(&mut zip, &f.theme);
        let accents: Vec<Rgba> = (1..=6)
            .filter_map(|i| {
                theme
                    .colors
                    .iter()
                    .find(|(k, _)| *k == format!("accent{i}"))
                    .map(|x| x.1)
            })
            .collect();
        let res = resolver(&theme);
        let env = ChartEnv {
            resolve_color: &res,
            palette: &accents,
            minor_font: theme.minor.as_deref(),
            major_font: theme.major.as_deref(),
        };
        let parsed = match parse_chart(xml.as_bytes(), &env) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{tag} {}: parse error {e:?}", f.part);
                continue;
            }
        };
        let (items, trunc): (Vec<Item>, bool) = draw_chart(&parsed.model, f.w, f.h);
        let scene = SlideScene {
            width: f.w,
            height: f.h,
            background: Fill::Solid(Rgba::WHITE),
            underlay: Vec::new(),
            items,
            truncated: trunc,
        };
        let r = render_svg(&scene, &|_| None);
        // The reference, cropped to the frame.
        let refpng = refdir.join(format!("slide-{:03}.png", f.slide));
        let refimg = image::open(&refpng).ok().map(|i| i.to_rgba8());
        let scale = refimg.as_ref().map_or(1.0, |i| i.width() as f64 / sw);
        let (cw, ch) = (
            (f.w * scale).round().max(8.0) as u32,
            (f.h * scale).round().max(8.0) as u32,
        );
        let mine =
            crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/c.svg"), cw)
                .map(|i| i.to_rgba8());
        let mut canvas =
            image::RgbaImage::from_pixel(cw * 2 + 12, ch, image::Rgba([200, 200, 200, 255]));
        if let Some(m) = &mine {
            image::imageops::overlay(&mut canvas, m, 0, 0);
        }
        if let Some(ri) = &refimg {
            let (rx, ry) = ((f.x * scale) as i64, (f.y * scale) as i64);
            let crop = image::imageops::crop_imm(
                ri,
                rx.max(0) as u32,
                ry.max(0) as u32,
                cw.min(ri.width().saturating_sub(rx.max(0) as u32)),
                ch.min(ri.height().saturating_sub(ry.max(0) as u32)),
            )
            .to_image();
            image::imageops::overlay(&mut canvas, &crop, (cw + 12) as i64, 0);
        }
        let name = format!("{tag}-s{}-{}.png", f.slide, k + 1);
        let _ = canvas.save(PathBuf::from(OUT).join(name));
        n += 1;
        if trunc || parsed.truncated || parsed.approximated {
            eprintln!(
                "{tag} {}: truncated={} parse-truncated={} approximated={}",
                f.part, trunc, parsed.truncated, parsed.approximated
            );
        }
    }
    n
}

#[test]
#[ignore]
fn chart_dump() {
    let _ = std::fs::remove_dir_all(OUT);
    std::fs::create_dir_all(OUT).unwrap();
    let mut total = 0;
    // Self-made deck.
    total += dump_deck(
        Path::new(&format!("{CACHE}/slide-corpus-gen/draw-charts.pptx")),
        Path::new(&format!("{CACHE}/pptx-ref/selfmade/draw-charts")),
        "own",
    );
    // PowerPoint-made decks with charts.
    let index =
        std::fs::read_to_string(format!("{CACHE}/pptx-corpus/INDEX.md")).unwrap_or_default();
    for line in index.lines() {
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        if cols.len() < 8 || cols[5] != "x" {
            continue;
        }
        let (src, file) = (cols[1], cols[2]);
        if !file.ends_with(".pptx") && !file.ends_with(".pptm") {
            continue;
        }
        let stem = file.rsplit_once('.').map_or(file, |s| s.0);
        let short: String = stem
            .chars()
            .rev()
            .take(24)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        total += dump_deck(
            Path::new(&format!("{CACHE}/pptx-corpus/{src}/{file}")),
            Path::new(&format!("{CACHE}/pptx-ref/{src}/{stem}")),
            &format!("{src}-{short}"),
        );
    }
    eprintln!("dumped {total} charts to {OUT}");
    assert!(total > 0);
}
