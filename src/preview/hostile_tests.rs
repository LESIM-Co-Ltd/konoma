//! Hostile-input tests for the image paths: a crafted SVG / GIF / PNG must be refused quickly
//! (`None`, the same as "can not preview") instead of aborting, hanging or exhausting memory, and
//! the legitimate neighbours of every limit must keep working.
//!
//! Each hostile case here was reproduced against the code before the limits existed:
//! the deeply nested SVGs abort the whole test binary with a stack overflow (so they are
//! the signal that the guard is gone), the GIF with a 65535 x 65535 logical screen allocates
//! 17 GB, the filter files run for tens of seconds.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::preview::svg::{intrinsic_size_bytes, rasterize_trusted as rasterize_bytes};
use crate::preview::svg_guard::{MAX_SVG_BYTES, MAX_XML_DEPTH};

/// Generous wall-clock bound for "refused immediately". Refusals take milliseconds; the hostile
/// files used to take 14-25 s or never finish.
const QUICK: Duration = Duration::from_secs(5);

fn svg(w: u32, h: u32, body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{body}</svg>"#
    )
}

fn nested(open: &str, close: &str, n: usize, inner: &str) -> String {
    format!("{}{}{}", open.repeat(n), inner, close.repeat(n))
}

/// Rasterize and require that it is refused within `QUICK`.
fn refused(svg: &str) {
    refused_within(svg, QUICK);
}

fn refused_within(svg: &str, limit: Duration) {
    let t = Instant::now();
    let r = rasterize_bytes(svg.as_bytes(), Path::new("hostile.svg"), 4096);
    let took = t.elapsed();
    assert!(r.is_none(), "a hostile SVG must not render");
    assert!(took < limit, "refusal took {took:?}");
}

fn drawn(svg: &str) -> image::DynamicImage {
    rasterize_bytes(svg.as_bytes(), Path::new("ok.svg"), 800).expect("a legitimate SVG must draw")
}

// ---- nesting depth: stack overflow (abort) ---------------------------------------------------

#[test]
fn deeply_nested_translucent_groups_are_refused_not_aborted() {
    // 1000 levels of <g opacity>: a stack overflow (SIGABRT) before the guard.
    refused(&svg(
        100,
        100,
        &nested(
            r#"<g opacity="0.9">"#,
            "</g>",
            1000,
            r#"<rect width="50" height="50"/>"#,
        ),
    ));
}

#[test]
fn deeply_nested_plain_groups_are_refused_not_aborted() {
    refused(&svg(
        100,
        100,
        &nested("<g>", "</g>", 1000, r#"<rect width="50" height="50"/>"#),
    ));
}

#[test]
fn deeply_nested_svg_elements_are_refused_not_aborted() {
    refused(&svg(
        100,
        100,
        &nested("<svg>", "</svg>", 900, r#"<rect width="50" height="50"/>"#),
    ));
}

#[test]
fn absurdly_deep_nesting_is_refused_without_recursing() {
    // Our own checks must not recurse on the document either.
    refused(&svg(100, 100, &nested("<g>", "</g>", 200_000, "")));
}

#[test]
fn nesting_limit_boundary_is_exact() {
    // <svg> is level 1 and the <rect> inside the groups is the deepest element, so MAX_XML_DEPTH - 2
    // groups reach exactly the limit.
    let at = svg(
        100,
        100,
        &nested(
            "<g>",
            "</g>",
            MAX_XML_DEPTH - 2,
            r#"<rect width="50" height="50"/>"#,
        ),
    );
    let img = drawn(&at);
    assert_eq!(img.width(), 800, "drawn at the normal size");
    let over = svg(
        100,
        100,
        &nested(
            "<g>",
            "</g>",
            MAX_XML_DEPTH - 1,
            r#"<rect width="50" height="50"/>"#,
        ),
    );
    refused(&over);
    // The size query (UI thread) reads the root element and nothing else, so it has no opinion on
    // how deep the rest is: a refused document still has a size, and the drawing refuses it.
    assert_eq!(intrinsic_size_bytes(over.as_bytes()), Some((100, 100)));
}

#[test]
fn a_document_at_the_depth_limit_draws_on_a_two_megabyte_stack() {
    // The thread konoma draws inline images on has the default 2 MiB stack; the guard hands deep
    // documents to a large one. Run the call on exactly such a thread.
    let doc = svg(
        100,
        100,
        &nested(
            r#"<g opacity="0.9">"#,
            "</g>",
            MAX_XML_DEPTH - 2,
            r#"<rect width="50" height="50"/>"#,
        ),
    );
    let ok = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || rasterize_bytes(doc.as_bytes(), Path::new("t.svg"), 800).is_some())
        .unwrap()
        .join()
        .unwrap();
    assert!(
        ok,
        "the deepest accepted document must draw on a 2 MiB stack"
    );
}

#[test]
fn an_svg_over_the_size_cap_is_refused() {
    let pad = " ".repeat(MAX_SVG_BYTES);
    let doc = svg(10, 10, &format!("{pad}<rect width=\"5\" height=\"5\"/>"));
    refused(&doc);
}

// ---- <use> expansion --------------------------------------------------------------------------

fn use_bomb(levels: usize) -> String {
    let mut body = String::from(r#"<defs><rect id="u0" width="2" height="2"/>"#);
    for i in 0..levels {
        body += &format!(
            r##"<g id="u{}"><use href="#u{i}"/><use href="#u{i}" x="1"/></g>"##,
            i + 1
        );
    }
    body += &format!(r##"</defs><use href="#u{levels}"/>"##);
    svg(100, 100, &body)
}

#[test]
fn an_exponential_use_chain_is_refused() {
    // 22 levels = 4 million elements; converting took 4 s (debug) to 14 s before the guard, which
    // answers from the XML alone.
    refused_within(&use_bomb(22), Duration::from_secs(1));
}

#[test]
fn a_moderate_use_chain_still_draws() {
    // 8 levels = 256 rectangles: an ordinary repeated symbol.
    let img = drawn(&use_bomb(8));
    assert_eq!(img.width(), 800);
}

#[test]
fn use_reference_cycles_do_not_hang_the_check() {
    let doc = svg(
        50,
        50,
        r##"<g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g><use href="#a"/>"##,
    );
    let t = Instant::now();
    let _ = rasterize_bytes(doc.as_bytes(), Path::new("c.svg"), 200);
    assert!(t.elapsed() < QUICK);
}

// ---- filters and layers: CPU and memory ------------------------------------------------------

#[test]
fn turbulence_with_thousands_of_octaves_is_refused() {
    refused(&svg(
        4096,
        4096,
        r#"<filter id="f" x="0" y="0" width="100%" height="100%"><feTurbulence baseFrequency="0.01" numOctaves="2000"/></filter><rect width="4096" height="4096" filter="url(#f)"/>"#,
    ));
}

#[test]
fn morphology_with_a_huge_radius_is_refused() {
    refused(&svg(
        4096,
        4096,
        r#"<filter id="f" x="0" y="0" width="100%" height="100%"><feMorphology operator="dilate" radius="5000"/></filter><rect width="4000" height="4000" fill="red" filter="url(#f)"/>"#,
    ));
}

#[test]
fn a_blur_over_a_huge_filter_region_is_refused() {
    refused(&svg(
        4096,
        4096,
        r#"<filter id="f" x="-1000%" y="-1000%" width="2100%" height="2100%"><feGaussianBlur stdDeviation="100000"/></filter><rect width="4000" height="4000" fill="red" filter="url(#f)"/>"#,
    ));
}

#[test]
fn a_few_hundred_nested_filters_are_refused() {
    // One blur filter applied by 200 nested groups on a 4096 px canvas: 634 MB and 20+ s before.
    refused(&svg(
        4096,
        4096,
        &format!(
            r#"<filter id="f"><feGaussianBlur stdDeviation="3"/></filter>{}"#,
            nested(
                r##"<g filter="url(#f)">"##,
                "</g>",
                200,
                r#"<rect width="4000" height="4000" fill="red"/>"#
            )
        ),
    ));
}

#[test]
fn many_full_size_layers_on_a_big_canvas_are_refused() {
    // 100 nested translucent groups on a 4096 px canvas = 100 live 64 MB layers (6.4 GB).
    refused(&svg(
        4096,
        4096,
        &nested(
            r#"<g opacity="0.9">"#,
            "</g>",
            100,
            r#"<rect width="4096" height="4096"/>"#,
        ),
    ));
}

#[test]
fn the_same_nesting_on_a_small_canvas_still_draws() {
    // The layer budget is memory, not a count: 30 layers of 100 x 100 px are about 1 MB.
    let img = drawn(&svg(
        100,
        100,
        &nested(
            r#"<g opacity="0.9">"#,
            "</g>",
            30,
            r#"<rect width="100" height="100"/>"#,
        ),
    ));
    assert_eq!(img.width(), 800);
}

#[test]
fn ordinary_filters_still_draw() {
    // A drop-shadow style blur and a soft-edge morphology on a typical icon.
    let img = drawn(&svg(
        64,
        64,
        r##"<filter id="s"><feGaussianBlur stdDeviation="2"/><feOffset dx="1" dy="1"/></filter>
            <filter id="m"><feMorphology operator="erode" radius="1"/></filter>
            <filter id="t"><feTurbulence baseFrequency="0.05" numOctaves="3"/></filter>
            <rect x="8" y="8" width="30" height="30" filter="url(#s)"/>
            <rect x="30" y="30" width="20" height="20" filter="url(#m)"/>
            <rect width="20" height="20" filter="url(#t)"/>"##,
    ));
    assert_eq!((img.width(), img.height()), (800, 800));
}

#[test]
fn an_image_href_to_a_device_file_does_not_hang() {
    // `/dev/zero` exists and "reads" forever: the resolver must only open bounded regular files.
    let t = Instant::now();
    let _ = rasterize_bytes(
        svg(
            10,
            10,
            r#"<image href="/dev/zero" width="10" height="10"/>"#,
        )
        .as_bytes(),
        Path::new("x.svg"),
        200,
    );
    assert!(t.elapsed() < QUICK);
}

#[test]
fn embedded_svg_images_go_through_the_same_checks() {
    use base64_lite::encode;
    let inner = use_bomb(22);
    let outer = svg(
        100,
        100,
        &format!(
            r#"<image width="100" height="100" href="data:image/svg+xml;base64,{}"/>"#,
            encode(inner.as_bytes())
        ),
    );
    let t = Instant::now();
    let _ = rasterize_bytes(outer.as_bytes(), Path::new("o.svg"), 200);
    assert!(
        t.elapsed() < QUICK,
        "an SVG hidden in a data URL must not bypass the checks"
    );
}

/// Just enough base64 for the test above (no new dependency).
mod base64_lite {
    pub fn encode(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in data.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 {
                T[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if c.len() > 2 {
                T[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }
}

// ---- legitimate SVGs: the limits must be far away --------------------------------------------

/// The deepest nesting and the most elements konoma's own generators produce, over the corpus the
/// repo already keeps. The limits are several times these.
#[test]
fn legitimate_generated_svgs_stay_far_below_the_limits() {
    use crate::preview::markdown::mermaid_to_svg_flow;
    let mut worst_depth = 0usize;
    let diagrams = [
        "flowchart LR\n A[Start] --> B{Choice}\n B -->|yes| C[Do]\n B -->|no| D[Skip]\n C --> E[End]\n D --> E\n subgraph S\n  F --> G\n end\n E --> F\n",
        "sequenceDiagram\n A->>B: hi\n B-->>A: hello\n loop every minute\n  A->>B: ping\n end\n",
        "stateDiagram-v2\n [*] --> A\n A --> B\n state B {\n  [*] --> X\n  X --> Y\n }\n",
        "classDiagram\n class A {\n  +int x\n }\n A <|-- B\n",
        "gantt\n title t\n section s\n a :a1, 2024-01-01, 3d\n b :after a1, 2d\n",
        "pie title P\n \"a\" : 3\n \"b\" : 4\n",
    ];
    for code in diagrams {
        let Some(svg) = mermaid_to_svg_flow(code, "default", "basis", "splines") else {
            continue;
        };
        worst_depth = worst_depth.max(depth_of(&svg));
        assert!(
            rasterize_bytes(svg.as_bytes(), Path::new("m.svg"), 800).is_some(),
            "a mermaid diagram must still draw: {code}"
        );
    }
    for latex in [
        r"E = mc^2",
        r"\frac{a}{b} + \sqrt{x^2 + y^2}",
        r"\sum_{i=0}^{n} \frac{1}{i!}",
        r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
        r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}",
    ] {
        let svg = crate::preview::math::latex_to_svg(latex, true, "#d0d0d0").expect("math svg");
        worst_depth = worst_depth.max(depth_of(&svg));
        assert!(
            rasterize_bytes(svg.as_bytes(), Path::new("m.svg"), 1024).is_some(),
            "math must still draw: {latex}"
        );
    }
    eprintln!("deepest generated SVG nesting: {worst_depth}");
    assert!(
        worst_depth * 3 < MAX_XML_DEPTH,
        "generated SVG nests {worst_depth} deep; the limit {MAX_XML_DEPTH} must stay well above"
    );
}

fn depth_of(svg: &str) -> usize {
    let mut depth = 0usize;
    let mut max = 0usize;
    let b = svg.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' && i + 1 < b.len() {
            match b[i + 1] {
                b'/' => depth = depth.saturating_sub(1),
                b'!' | b'?' => {}
                _ => {
                    let end = svg[i..].find('>').map_or(b.len(), |e| i + e);
                    if !svg[i..end].ends_with('/') {
                        depth += 1;
                        max = max.max(depth);
                    }
                }
            }
        }
        i += 1;
    }
    max
}

#[test]
fn the_bundled_sample_svgs_still_draw_at_their_usual_size() {
    let mut n = 0;
    for entry in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("samples"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("svg") {
            continue;
        }
        let data = std::fs::read(&p).unwrap();
        let img = rasterize_bytes(&data, &p, 800)
            .unwrap_or_else(|| panic!("{} must still draw", p.display()));
        assert!(img.width().max(img.height()) >= 800, "{}", p.display());
        n += 1;
    }
    assert!(n > 0, "the samples directory has SVGs to check");
}

// ---- raster images ---------------------------------------------------------------------------

/// A valid GIF: a 1x1 frame repeated `frames` times on a `canvas` logical screen.
fn gif(canvas: (u16, u16), frames: usize) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&canvas.0.to_le_bytes());
    out.extend_from_slice(&canvas.1.to_le_bytes());
    out.extend_from_slice(&[0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
    for _ in 0..frames {
        out.extend_from_slice(&[0x21, 0xf9, 0x04, 0x00, 0x0a, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
        out.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);
    }
    out.push(0x3b);
    out
}

#[test]
fn a_gif_with_a_huge_logical_screen_is_refused_before_allocating() {
    use crate::preview::image::decode_gif_bytes_inline;
    for side in [65535u16, 30000] {
        let t = Instant::now();
        let r = decode_gif_bytes_inline(&gif((side, side), 2));
        assert!(r.is_none(), "{side}x{side} logical screen must be refused");
        assert!(t.elapsed() < QUICK, "{side}: took {:?}", t.elapsed());
    }
}

#[test]
fn a_gif_just_inside_and_just_outside_the_canvas_limit() {
    use crate::preview::image::decode_gif_bytes_inline;
    // 36,000,000 px: 6000 x 6000 is the limit. (Decoding a frame that large takes a minute in a
    // debug build, so the inside of the limit is checked on the limit function and the outside on a
    // real GIF, where refusal is immediate.)
    use crate::preview::image::gif_canvas_within_limit;
    assert!(
        gif_canvas_within_limit((6000, 6000)),
        "6000 x 6000 is a legitimate (large) GIF"
    );
    assert!(
        !gif_canvas_within_limit((6000, 6001)),
        "one row more is over the limit"
    );
    assert!(decode_gif_bytes_inline(&gif((6000, 6001), 2)).is_none());
    assert!(
        decode_gif_bytes_inline(&gif((800, 600), 2)).is_some(),
        "an ordinary GIF decodes"
    );
}

#[test]
fn an_ordinary_animated_gif_still_decodes_with_all_frames() {
    use crate::preview::image::decode_gif_bytes_inline;
    let (frames, canvas) = decode_gif_bytes_inline(&gif((64, 48), 7)).expect("decodes");
    assert_eq!(frames.len(), 7);
    assert_eq!(canvas, (64, 48));
}

#[test]
fn too_many_gif_frames_fall_back_to_a_still() {
    use crate::preview::image::decode_gif_bytes_inline;
    assert!(decode_gif_bytes_inline(&gif((8, 8), 5_001)).is_none());
}

/// A PNG header that declares `w` x `h` with no pixel data behind it.
pub(crate) fn png_header_only(w: u32, h: u32) -> Vec<u8> {
    fn chunk(t: &[u8; 4], d: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&(d.len() as u32).to_be_bytes());
        c.extend_from_slice(t);
        c.extend_from_slice(d);
        let mut crc = crc32(t);
        crc = crc32_update(crc, d);
        c.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
        c
    }
    fn crc32(d: &[u8]) -> u32 {
        crc32_update(0xffff_ffff, d)
    }
    fn crc32_update(mut c: u32, d: &[u8]) -> u32 {
        for &b in d {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        c
    }
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &ihdr));
    out.extend(chunk(b"IDAT", &[0x78, 0x01]));
    out
}

#[test]
fn a_still_image_over_the_pixel_limit_is_refused_from_its_header() {
    use crate::preview::image::decode_static_bytes;
    for (w, h) in [(60_000u32, 60_000u32), (40_000, 1), (12_300, 12_300)] {
        let t = Instant::now();
        assert!(
            decode_static_bytes(&png_header_only(w, h)).is_none(),
            "{w}x{h}"
        );
        assert!(t.elapsed() < QUICK);
    }
}

#[test]
fn a_real_png_decodes_and_the_capped_decode_shrinks_only_when_needed() {
    use crate::preview::image::{decode_static, decode_static_capped};
    let dir = crate::test_support::unique_tmp("konoma_hostile_png");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("big.png");
    image::RgbImage::from_pixel(500, 250, image::Rgb([10, 20, 30]))
        .save(&p)
        .unwrap();
    let full = decode_static(&p).expect("decodes");
    assert_eq!((full.width(), full.height()), (500, 250));
    let capped = decode_static_capped(&p, 400).expect("decodes");
    assert_eq!(capped.width(), 400, "long side is the cap");
    assert_eq!(capped.height(), 200, "aspect ratio is kept");
    let small = decode_static_capped(&p, 500).expect("decodes");
    assert_eq!(
        (small.width(), small.height()),
        (500, 250),
        "within the cap = untouched"
    );
}

#[test]
fn the_decode_gate_blocks_until_memory_is_returned() {
    use crate::preview::image::DecodeGate;
    use std::sync::atomic::{AtomicBool, Ordering};
    static GATE: DecodeGate = DecodeGate::new(100);
    static SECOND_RAN: AtomicBool = AtomicBool::new(false);
    let first = GATE.reserve(80);
    assert_eq!(GATE.in_use(), 80);
    let t = std::thread::spawn(|| {
        let _r = GATE.reserve(50); // 80 + 50 > 100: must wait for `first`
        SECOND_RAN.store(true, Ordering::SeqCst);
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !SECOND_RAN.load(Ordering::SeqCst),
        "must wait while the pool is full"
    );
    drop(first);
    t.join().unwrap();
    assert!(SECOND_RAN.load(Ordering::SeqCst));
    assert_eq!(GATE.in_use(), 0, "everything is returned");
}

#[test]
fn the_decode_gate_lets_an_oversized_request_run_alone_and_never_waits_on_itself() {
    use crate::preview::image::DecodeGate;
    static GATE: DecodeGate = DecodeGate::new(100);
    let big = GATE.reserve(10_000);
    assert_eq!(GATE.in_use(), 100, "clamped to the budget");
    let nested = GATE.reserve(50); // same thread: must not deadlock
    drop(nested);
    drop(big);
    assert_eq!(GATE.in_use(), 0);
}

/// Calibration for `svg_guard::MAX_FILTER_WORK` (run with `--release --ignored --nocapture`):
/// times resvg on filters directly, bypassing the guard, so the per-pixel weights and the budget can
/// be checked against real cost.
#[test]
#[ignore = "calibration; prints timings"]
fn calibrate_filter_costs() {
    use resvg::{tiny_skia, usvg};
    let time = |label: &str, prim: &str, side: u32| {
        let doc = svg(
            side,
            side,
            &format!(
                r#"<filter id="f" x="0" y="0" width="100%" height="100%">{prim}</filter><rect width="{side}" height="{side}" fill="red" filter="url(#f)"/>"#
            ),
        );
        let tree = usvg::Tree::from_str(&doc, &usvg::Options::default()).unwrap();
        let mut pm = tiny_skia::Pixmap::new(side, side).unwrap();
        let t = Instant::now();
        resvg::render(&tree, tiny_skia::Transform::identity(), &mut pm.as_mut());
        let secs = t.elapsed().as_secs_f64();
        let px = f64::from(side) * f64::from(side);
        println!(
            "{label:32} side {side}: {secs:.3}s  = {:.2e} px/s",
            px / secs
        );
    };
    for side in [1024u32, 2048] {
        time("blur std 3", r#"<feGaussianBlur stdDeviation="3"/>"#, side);
        time(
            "blur std 200",
            r#"<feGaussianBlur stdDeviation="200"/>"#,
            side,
        );
        time("offset", r#"<feOffset dx="3" dy="3"/>"#, side);
        time(
            "turbulence octaves 1",
            r#"<feTurbulence baseFrequency="0.01" numOctaves="1"/>"#,
            side,
        );
        time(
            "turbulence octaves 4",
            r#"<feTurbulence baseFrequency="0.01" numOctaves="4"/>"#,
            side,
        );
        time(
            "morphology r 2",
            r#"<feMorphology operator="dilate" radius="2"/>"#,
            side,
        );
        time(
            "morphology r 8",
            r#"<feMorphology operator="dilate" radius="8"/>"#,
            side,
        );
        time(
            "morphology r 32",
            r#"<feMorphology operator="dilate" radius="32"/>"#,
            side,
        );
        time(
            "lighting",
            r#"<feDiffuseLighting><feDistantLight azimuth="45" elevation="45"/></feDiffuseLighting>"#,
            side,
        );
    }
}

// ---- budget boundaries (the tree check alone: no rendering, so they are fast in debug) -------

fn within_budget(doc: &str, scale: f32) -> bool {
    crate::preview::svg_guard::load_tree(doc.as_bytes(), None, |tree| {
        Ok(crate::preview::svg_guard::check_render_budget(&tree, scale))
    })
    .expect("the document parses")
}

fn filtered_rect(side: u32, prim: &str) -> String {
    svg(
        side,
        side,
        &format!(
            r#"<filter id="f" x="0" y="0" width="100%" height="100%">{prim}</filter><rect width="{side}" height="{side}" filter="url(#f)"/>"#
        ),
    )
}

#[test]
fn morphology_radius_boundary_on_an_800_px_canvas() {
    // 800 x 800 px = 640,000 px; budget 6e8 => 937 units/px => radius 28 passes, 29 does not.
    let m = |r: u32| filtered_rect(800, &format!(r#"<feMorphology radius="{r}"/>"#));
    assert!(within_budget(&m(28), 1.0), "radius 28 is the last allowed");
    assert!(!within_budget(&m(29), 1.0), "radius 29 is over");
}

#[test]
fn turbulence_octave_boundary_on_an_800_px_canvas() {
    let t = |n: u32| {
        filtered_rect(
            800,
            &format!(r#"<feTurbulence baseFrequency="0.01" numOctaves="{n}"/>"#),
        )
    };
    assert!(
        within_budget(&t(371), 1.0),
        "371 octaves is the last allowed"
    );
    assert!(!within_budget(&t(372), 1.0), "372 is over");
    assert!(within_budget(&t(8), 1.0), "typical use is nowhere near");
}

#[test]
fn live_layer_memory_boundary_at_the_4096_canvas() {
    // Each nested translucent group holds a 4096 x 4096 x 4 B = 64 MiB layer; the budget is 1 GiB.
    let layers = |n: usize| {
        svg(
            4096,
            4096,
            &nested(
                r#"<g opacity="0.9">"#,
                "</g>",
                n,
                r#"<rect width="4096" height="4096"/>"#,
            ),
        )
    };
    assert!(
        within_budget(&layers(15), 1.0),
        "15 live layers (960 MiB, plus the group's own) fit"
    );
    assert!(!within_budget(&layers(17), 1.0), "17 do not");
}

#[test]
fn a_huge_embedded_raster_is_refused_without_decoding_it() {
    // An <image> whose PNG header declares 12,000 x 12,000 (resvg would allocate 576 MB for it).
    let png = png_header_only(12_000, 12_000);
    let b64 = base64_lite::encode(&png);
    let doc = svg(
        100,
        100,
        &format!(r#"<image width="100" height="100" href="data:image/png;base64,{b64}"/>"#),
    );
    // usvg only reads the header to size the image; the guard refuses it before any decode.
    assert!(
        !within_budget(&doc, 1.0),
        "a 144-megapixel embedded image is over the 64 MP budget"
    );
    let t = Instant::now();
    assert!(rasterize_bytes(doc.as_bytes(), Path::new("e.svg"), 200).is_none());
    assert!(t.elapsed() < QUICK);
}
