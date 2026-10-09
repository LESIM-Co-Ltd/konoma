//! Dumps for a human (ignored tests) of the shape/effect work: the self-made and sample decks and
//! a corpus selection, konoma beside LibreOffice's picture. They write into
//! `docs/render-check/e3/`. `E3_ONLY=<substring,...>` limits the decks, `E3_CORPUS=src/file.pptx,...`
//! picks corpus decks, `E3_SLIDES=<n>` the slides drawn per corpus deck.

use std::path::Path;

use super::dump::{clear_dir, dump_deck, root, CACHE};

const OUT: &str = "/Users/shuhei/work/konoma/docs/render-check/e3";

fn wanted(name: &str) -> bool {
    std::env::var("E3_ONLY").map_or(true, |f| f.split(',').any(|p| name.contains(p)))
}

#[test]
#[ignore = "writes docs/render-check/e3/*.png for a human to look at"]
fn e3_dump_decks() {
    let out = Path::new(OUT);
    if std::env::var("E3_ONLY").is_err() && std::env::var("E3_CORPUS").is_err() {
        clear_dir(out);
    }
    let _ = std::fs::create_dir_all(out);
    let own = Path::new(CACHE).join("pptx-ref/own");
    let selfmade = Path::new(CACHE).join("pptx-ref/selfmade");
    let gen = Path::new(CACHE).join("slide-corpus-gen");
    let corpus_only = std::env::var("E3_CORPUS").is_ok() && std::env::var("E3_ONLY").is_err();
    if !corpus_only {
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
            if wanted(name) {
                dump_deck(name, &pptx, &refd, out, None);
            }
        }
        for k in ["text", "shapes", "fills", "background", "tables", "charts"] {
            let name = format!("draw-{k}");
            if wanted(&name) {
                dump_deck(
                    &name,
                    &gen.join(format!("{name}.pptx")),
                    &selfmade.join(&name),
                    out,
                    None,
                );
            }
        }
    }
    if let Ok(list) = std::env::var("E3_CORPUS") {
        let n: usize = std::env::var("E3_SLIDES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4);
        for item in list.split(',').filter(|s| !s.is_empty()) {
            let Some((src, file)) = item.split_once('/') else {
                continue;
            };
            let p = Path::new(CACHE).join("pptx-corpus").join(src).join(file);
            let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
            let refd = Path::new(CACHE).join("pptx-ref").join(src).join(stem);
            dump_deck(&format!("{src}__{stem}"), &p, &refd, out, Some(n));
        }
    }
}

/// `E3_SVG=<pptx path>` and `E3_SVG_SLIDE=<n>` (1-based): writes the slide's SVG to
/// `E3_SVG_OUT`, for looking at the markup.
#[test]
#[ignore = "writes one SVG for a human to look at"]
fn e3_dump_svg() {
    use super::super::super::{load_presentation, DocOptions};
    let Ok(path) = std::env::var("E3_SVG") else {
        return;
    };
    let n: usize = std::env::var("E3_SVG_SLIDE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let Ok(out) = std::env::var("E3_SVG_OUT") else {
        return;
    };
    let doc = load_presentation(Path::new(&path), &DocOptions::default()).expect("loads");
    let media: std::collections::HashMap<String, std::sync::Arc<Vec<u8>>> = doc
        .images
        .iter()
        .map(|i| (i.key.clone(), std::sync::Arc::new(i.bytes.clone())))
        .collect();
    let r = crate::preview::office::slide_draw::render_svg(&doc.slide_scenes[n - 1], &|k| {
        media.get(k).cloned()
    });
    std::fs::write(out, r.svg).unwrap();
}

/// A slide of the features of this work in one picture (vertical text, autofit, symbol bullets,
/// effects, rectangular gradient): `docs/render-check/e3/synthetic-1.png`.
#[test]
#[ignore = "writes docs/render-check/e3/synthetic-1.png for a human to look at"]
fn e3_dump_synthetic() {
    use super::{para, shape, xf, D, RECT};
    let fill = |c: &str| format!(r#"<a:solidFill><a:srgbClr val="{c}"/></a:solidFill>"#);
    let ln = r#"<a:ln w="9525"><a:solidFill><a:srgbClr val="334455"/></a:solidFill></a:ln>"#;
    let boxed = |x: i64, y: i64, w: i64, h: i64, extra: &str, body_pr: &str, paras: &str| {
        shape(
            "",
            &format!("{}{RECT}{}{extra}", xf(x, y, w, h), fill("DDE8F5")),
            body_pr,
            "",
            paras,
            "",
        )
    };
    let e = 9525;
    let mut shapes = String::new();
    // vertical Japanese
    shapes += &boxed(
        40 * e,
        30 * e,
        150 * e,
        330 * e,
        ln,
        r#"<a:bodyPr vert="eaVert" anchor="ctr" anchorCtr="1"><a:noAutofit/></a:bodyPr>"#,
        &para("縦書き、テスト。「かっこ」ーー（丸）…終"),
    );
    shapes += &boxed(
        210 * e,
        30 * e,
        110 * e,
        330 * e,
        ln,
        r#"<a:bodyPr vert="eaVert"><a:noAutofit/></a:bodyPr>"#,
        &para("日本語の縦書き。次の行へ、続く"),
    );
    // autofit without stored scales
    shapes += &boxed(
        340 * e,
        30 * e,
        260 * e,
        120 * e,
        ln,
        r#"<a:bodyPr><a:normAutofit/></a:bodyPr>"#,
        &para(&"Autofit shrinks this long text until it fits the box. ".repeat(6)),
    );
    // symbol bullets
    let bu = |font: &str, ch: &str, txt: &str| {
        format!(
            r#"<a:p><a:pPr marL="342900" indent="-342900"><a:buFont typeface="{font}" charset="2"/><a:buChar char="{ch}"/></a:pPr><a:r><a:rPr lang="en-US" sz="1600"/><a:t>{txt}</a:t></a:r></a:p>"#
        )
    };
    shapes += &boxed(
        340 * e,
        170 * e,
        260 * e,
        190 * e,
        ln,
        "",
        &(bu("Wingdings", "&#xF06C;", "Wingdings circle")
            + &bu("Wingdings", "&#xF0A7;", "Wingdings small square")
            + &bu("Wingdings", "&#xF0D8;", "Wingdings arrowhead")
            + &bu("Wingdings", "&#xF0FC;", "Wingdings check")
            + &bu("Symbol", "&#xF02D;", "Symbol minus")
            + &bu("Symbol", "&#xF0B7;", "Symbol bullet")
            + &bu("Wingdings 3", "&#xF07D;", "Wingdings 3 triangle")),
    );
    // effects
    let fx = |x: i64, y: i64, label: &str, eff: &str| {
        boxed(
            x * e,
            y * e,
            110 * e,
            60 * e,
            &format!("<a:effectLst>{eff}</a:effectLst>"),
            "",
            &para(label),
        )
    };
    shapes += &fx(
        640,
        40,
        "glow",
        r#"<a:glow rad="139700"><a:srgbClr val="FFC000"><a:alpha val="60000"/></a:srgbClr></a:glow>"#,
    );
    shapes += &fx(780, 40, "softEdge", r#"<a:softEdge rad="127000"/>"#);
    shapes += &fx(
        640,
        140,
        "reflection",
        r#"<a:reflection blurRad="6350" stA="50000" endA="300" endPos="55000" dist="25400" dir="5400000" sy="-100000" algn="bl" rotWithShape="0"/>"#,
    );
    shapes += &fx(
        780,
        140,
        "innerShdw",
        r#"<a:innerShdw blurRad="114300" dist="38100" dir="2700000"><a:srgbClr val="000000"><a:alpha val="60000"/></a:srgbClr></a:innerShdw>"#,
    );
    shapes += &fx(
        640,
        260,
        "outer",
        r#"<a:outerShdw blurRad="76200" dist="63500" dir="2700000" algn="tl" rotWithShape="0"><a:srgbClr val="000000"><a:alpha val="50000"/></a:srgbClr></a:outerShdw>"#,
    );
    // rectangular gradient
    shapes += &shape(
        "",
        &format!(
            r#"{}{RECT}<a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="1F4E9C"/></a:gs><a:gs pos="100000"><a:srgbClr val="F0A030"/></a:gs></a:gsLst><a:path path="rect"><a:fillToRect l="50000" t="50000" r="50000" b="50000"/></a:path></a:gradFill>"#,
            xf(780 * e, 260 * e, 110 * e, 60 * e)
        ),
        "",
        "",
        "",
        "",
    );
    let mut d = D::new(&shapes);
    d.size = (12_192_000, 6_858_000);
    let doc = d.load();
    let img = super::dump::draw_all(&doc, Some(1))
        .remove(0)
        .expect("rasterizes");
    let out = Path::new(OUT);
    let _ = std::fs::create_dir_all(out);
    img.save(out.join("synthetic-1.png")).unwrap();
}
