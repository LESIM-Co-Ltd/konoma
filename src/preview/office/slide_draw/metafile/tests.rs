//! Test builders (hand-assembled EMF / WMF byte streams), pixel helpers, the integration with the
//! slide writer, and the local corpus dump.

use std::path::Path;

use super::*;

// ----- byte builders -------------------------------------------------------------------------

/// `COLORREF` of an RGB triple, as the word the records carry.
pub(super) fn rgb(r: u8, g: u8, b: u8) -> i64 {
    r as i64 | (g as i64) << 8 | (b as i64) << 16
}

/// The IEEE bits of a float as a record word.
pub(super) fn fl(v: f32) -> i64 {
    v.to_bits() as i64
}

/// Words (truncated to 32 bits) as little-endian bytes.
pub(super) fn words(v: &[i64]) -> Vec<u8> {
    v.iter().flat_map(|x| (*x as u32).to_le_bytes()).collect()
}

/// An EMF under construction: header fields are written by [`Emf::finish`].
pub(super) struct Emf {
    pub recs: Vec<u8>,
    pub count: u32,
    /// Picture size in device pixels (96 dpi: 960 px = 254 mm).
    pub size: (i32, i32),
    /// Overrides for the frame (0.01 mm); `None` = derived from `size`.
    pub frame: Option<[i32; 4]>,
    pub bounds: Option<[i32; 4]>,
}

impl Emf {
    pub fn new(w: i32, h: i32) -> Emf {
        Emf {
            recs: Vec::new(),
            count: 1,
            size: (w, h),
            frame: None,
            bounds: None,
        }
    }

    /// A record whose payload is whole words.
    pub fn r(&mut self, ty: u32, payload: &[i64]) -> &mut Emf {
        self.raw(ty, &words(payload))
    }

    /// A record with an arbitrary payload (padded to a word boundary).
    pub fn raw(&mut self, ty: u32, payload: &[u8]) -> &mut Emf {
        let mut p = payload.to_vec();
        while !p.len().is_multiple_of(4) {
            p.push(0);
        }
        self.recs.extend_from_slice(&ty.to_le_bytes());
        self.recs
            .extend_from_slice(&((p.len() + 8) as u32).to_le_bytes());
        self.recs.extend_from_slice(&p);
        self.count += 1;
        self
    }

    pub fn brush(&mut self, idx: u32, style: u32, color: i64, hatch: u32) -> &mut Emf {
        self.r(39, &[idx as i64, style as i64, color, hatch as i64])
    }

    pub fn solid(&mut self, idx: u32, color: i64) -> &mut Emf {
        self.brush(idx, 0, color, 0)
    }

    pub fn pen(&mut self, idx: u32, style: u32, width: i32, color: i64) -> &mut Emf {
        self.r(38, &[idx as i64, style as i64, width as i64, 0, color])
    }

    pub fn select(&mut self, idx: u32) -> &mut Emf {
        self.r(37, &[idx as i64])
    }

    pub fn stock(&mut self, n: u32) -> &mut Emf {
        self.select(0x8000_0000 | n)
    }

    /// Red brush selected, no pen: the common start of a fill test.
    pub fn fill_with(&mut self, color: i64) -> &mut Emf {
        self.solid(1, color).select(1).stock(8)
    }

    pub fn rect(&mut self, l: i32, t: i32, r: i32, b: i32) -> &mut Emf {
        self.r(43, &[l as i64, t as i64, r as i64, b as i64])
    }

    pub fn finish(&self) -> Vec<u8> {
        let (w, h) = self.size;
        let mut recs = self.recs.clone();
        // EMR_EOF.
        recs.extend_from_slice(&14u32.to_le_bytes());
        recs.extend_from_slice(&20u32.to_le_bytes());
        recs.extend_from_slice(&[0; 12]);
        let frame = self.frame.unwrap_or([
            0,
            0,
            (w as f64 * 26.4583).round() as i32,
            (h as f64 * 26.4583).round() as i32,
        ]);
        let bounds = self.bounds.unwrap_or([0, 0, w - 1, h - 1]);
        let mut out = Vec::new();
        let hd = |v: &[i64]| words(v);
        out.extend(hd(&[1, 88]));
        out.extend(hd(&bounds.map(|v| v as i64)));
        out.extend(hd(&frame.map(|v| v as i64)));
        out.extend(hd(&[0x464D_4520, 0x10000, 0, (self.count + 1) as i64]));
        out.extend(hd(&[0])); // handles + reserved
        out.extend(hd(&[0, 0, 0])); // description + palette
        out.extend(hd(&[960, 960, 254, 254])); // device, millimetres
        assert_eq!(out.len(), 88);
        let total = (88 + recs.len()) as u32;
        out[48..52].copy_from_slice(&total.to_le_bytes());
        out.extend(recs);
        out
    }
}

/// A WMF under construction (placeable, 96 units per inch: 1 unit = 1 px).
pub(super) struct Wmf {
    pub recs: Vec<u8>,
    pub bbox: [i16; 4],
    pub placeable: bool,
}

impl Wmf {
    pub fn new(w: i16, h: i16) -> Wmf {
        Wmf {
            recs: Vec::new(),
            bbox: [0, 0, w, h],
            placeable: true,
        }
    }

    /// A record of 16-bit parameters.
    pub fn r(&mut self, func: u16, params: &[i32]) -> &mut Wmf {
        let bytes: Vec<u8> = params
            .iter()
            .flat_map(|p| (*p as i16).to_le_bytes())
            .collect();
        self.raw(func, &bytes)
    }

    pub fn raw(&mut self, func: u16, payload: &[u8]) -> &mut Wmf {
        let mut p = payload.to_vec();
        if !p.len().is_multiple_of(2) {
            p.push(0);
        }
        let words = (6 + p.len()) / 2;
        self.recs.extend_from_slice(&(words as u32).to_le_bytes());
        self.recs.extend_from_slice(&func.to_le_bytes());
        self.recs.extend_from_slice(&p);
        self
    }

    pub fn brush(&mut self, style: u16, color: i64, hatch: u16) -> &mut Wmf {
        let mut p = style.to_le_bytes().to_vec();
        p.extend((color as u32).to_le_bytes());
        p.extend(hatch.to_le_bytes());
        self.raw(0x02FC, &p)
    }

    pub fn pen(&mut self, style: u16, width: i16, color: i64) -> &mut Wmf {
        let mut p = style.to_le_bytes().to_vec();
        p.extend(width.to_le_bytes());
        p.extend(0i16.to_le_bytes());
        p.extend((color as u32).to_le_bytes());
        self.raw(0x02FA, &p)
    }

    pub fn select(&mut self, idx: i32) -> &mut Wmf {
        self.r(0x012D, &[idx])
    }

    pub fn delete(&mut self, idx: i32) -> &mut Wmf {
        self.r(0x01F0, &[idx])
    }

    /// Window origin and extent the way a typical file sets them.
    pub fn window(&mut self, w: i32, h: i32) -> &mut Wmf {
        self.r(0x0103, &[8]);
        self.r(0x020B, &[0, 0]);
        self.r(0x020C, &[h, w])
    }

    /// bottom, right, top, left order.
    pub fn rect(&mut self, l: i32, t: i32, r: i32, b: i32) -> &mut Wmf {
        self.r(0x041B, &[b, r, t, l])
    }

    pub fn finish(&self) -> Vec<u8> {
        let mut recs = self.recs.clone();
        recs.extend_from_slice(&3u32.to_le_bytes());
        recs.extend_from_slice(&0u16.to_le_bytes());
        let mut out = Vec::new();
        if self.placeable {
            out.extend(0x9AC6_CDD7u32.to_le_bytes());
            out.extend(0u16.to_le_bytes());
            for v in self.bbox {
                out.extend(v.to_le_bytes());
            }
            out.extend(96u16.to_le_bytes());
            out.extend(0u32.to_le_bytes());
            out.extend(0u16.to_le_bytes());
        }
        out.extend(1u16.to_le_bytes());
        out.extend(9u16.to_le_bytes());
        out.extend(0x0300u16.to_le_bytes());
        out.extend((((18 + recs.len()) / 2) as u32).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(recs);
        out
    }
}

// ----- pixels --------------------------------------------------------------------------------

/// Rasterizes an SVG with the project's rasterizer, `max_px` on the long side.
pub(super) fn raster_svg(svg: &str, max_px: u32) -> image::RgbaImage {
    crate::preview::svg::rasterize_trusted(svg.as_bytes(), Path::new("/m.svg"), max_px)
        .unwrap_or_else(|| panic!("the SVG does not rasterize:\n{svg}"))
        .to_rgba8()
}

pub(super) fn conv(bytes: &[u8]) -> MetaSvg {
    to_svg(bytes).expect("converts")
}

/// The picture drawn 100 px on its long side.
pub(super) fn draw(bytes: &[u8]) -> image::RgbaImage {
    raster_svg(&conv(bytes).svg, 100)
}

pub(super) fn at(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
    img.get_pixel(x.min(img.width() - 1), y.min(img.height() - 1))
        .0
}

pub(super) fn is_clear(p: [u8; 4]) -> bool {
    p[3] < 16
}

pub(super) fn is_red(p: [u8; 4]) -> bool {
    p[3] > 200 && p[0] > 200 && p[1] < 60 && p[2] < 60
}

pub(super) fn is_blue(p: [u8; 4]) -> bool {
    p[3] > 200 && p[2] > 200 && p[0] < 60 && p[1] < 60
}

pub(super) fn is_green(p: [u8; 4]) -> bool {
    p[3] > 200 && p[1] > 150 && p[0] < 60 && p[2] < 60
}

pub(super) fn is_dark(p: [u8; 4]) -> bool {
    p[3] > 128 && p[0] < 80 && p[1] < 80 && p[2] < 80
}

/// Any visible mostly-dark pixel (a one-pixel line straddles two pixels at half coverage).
pub(super) fn is_inkish(p: [u8; 4]) -> bool {
    p[3] > 40 && p[0] < 100 && p[1] < 100 && p[2] < 100
}

pub(super) fn is_bluish(p: [u8; 4]) -> bool {
    p[3] > 40 && p[2] > 150 && p[0] < 100 && p[1] < 100
}

pub(super) fn is_near(p: [u8; 4], c: [u8; 3], tol: i32) -> bool {
    p[3] > 200 && (0..3).all(|i| (p[i] as i32 - c[i] as i32).abs() <= tol)
}

/// Number of pixels in the image for which `f` holds.
pub(super) fn count(img: &image::RgbaImage, f: impl Fn([u8; 4]) -> bool) -> usize {
    img.pixels().filter(|p| f(p.0)).count()
}

// ----- the API -------------------------------------------------------------------------------

#[test]
fn neither_emf_nor_wmf_is_refused() {
    assert!(to_svg(b"").is_none());
    assert!(to_svg(b"not a metafile at all, just text").is_none());
    assert!(to_svg(&[0u8; 200]).is_none());
    assert!(to_svg(b"\x89PNG\r\n\x1a\n").is_none());
}

#[test]
fn emf_svg_is_a_self_contained_document() {
    let mut e = Emf::new(200, 100);
    e.fill_with(rgb(255, 0, 0)).rect(10, 10, 60, 60);
    let m = conv(&e.finish());
    assert!(m.svg.starts_with("<svg "), "{}", m.svg);
    assert!(m.svg.ends_with("</svg>"));
    assert!(m.svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(!m.svg.contains("NaN") && !m.svg.contains("inf"));
    assert!(!m.truncated);
    // 200 x 100 px at 96 dpi.
    assert!((m.width_px - 200.0).abs() < 0.5, "{}", m.width_px);
    assert!((m.height_px - 100.0).abs() < 0.5, "{}", m.height_px);
}

#[test]
fn emf_view_box_is_the_frame_in_device_units() {
    let mut e = Emf::new(300, 150);
    e.fill_with(rgb(255, 0, 0)).rect(0, 0, 10, 10);
    let svg = conv(&e.finish()).svg;
    let vb = svg
        .split("viewBox=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let v: Vec<f64> = vb.split(' ').map(|s| s.parse().unwrap()).collect();
    assert!(v[0].abs() < 0.1 && v[1].abs() < 0.1);
    assert!(
        (v[2] - 300.0).abs() < 0.2 && (v[3] - 150.0).abs() < 0.2,
        "{vb}"
    );
}

#[test]
fn emf_without_a_frame_uses_the_bounds() {
    let mut e = Emf::new(80, 40);
    e.frame = Some([0, 0, 0, 0]);
    e.bounds = Some([10, 20, 89, 59]);
    e.fill_with(rgb(0, 0, 255)).rect(10, 20, 90, 60);
    let m = conv(&e.finish());
    let vb = m
        .svg
        .split("viewBox=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_eq!(vb, "10 20 80 40");
    let img = raster_svg(&m.svg, 80);
    assert!(is_blue(at(&img, 40, 20)));
}

#[test]
fn a_wide_picture_keeps_its_ratio_when_drawn() {
    let mut e = Emf::new(200, 50);
    e.fill_with(rgb(255, 0, 0)).rect(0, 0, 200, 50);
    let img = raster_svg(&conv(&e.finish()).svg, 200);
    assert!(
        (199..=202).contains(&img.width()) && (49..=52).contains(&img.height()),
        "{}x{}",
        img.width(),
        img.height()
    );
    assert!(is_red(at(&img, 5, 5)) && is_red(at(&img, 195, 45)));
}

#[test]
fn wmf_placeable_view_box_and_size() {
    let mut w = Wmf::new(300, 150);
    w.window(300, 150);
    w.brush(0, rgb(255, 0, 0), 0).select(0);
    w.rect(0, 0, 300, 150);
    let m = conv(&w.finish());
    assert!(m.svg.contains("viewBox=\"0 0 300 150\""), "{}", m.svg);
    assert!((m.width_px - 300.0).abs() < 0.5 && (m.height_px - 150.0).abs() < 0.5);
}

// ----- the slide writer ----------------------------------------------------------------------

mod in_slide {
    use super::super::super::tests::{at as spx, raster, scene};
    use super::super::super::*;
    use super::*;
    use std::sync::Arc;

    fn media_of(bytes: Vec<u8>) -> impl Fn(&str) -> Option<Arc<Vec<u8>>> {
        let b = Arc::new(bytes);
        move |_| Some(b.clone())
    }

    fn picture_scene(key: &str) -> SlideScene {
        let x = Xfrm::rect(
            100.0 * 9525.0,
            50.0 * 9525.0,
            200.0 * 9525.0,
            100.0 * 9525.0,
        );
        scene(vec![Item::Picture(PictureItem::new(x, key))])
    }

    #[test]
    fn an_emf_picture_is_drawn_not_a_placeholder() {
        let mut e = Emf::new(100, 50);
        e.fill_with(rgb(255, 0, 0)).rect(0, 0, 100, 50);
        let s = picture_scene("m.emf");
        let r = render_svg(&s, &media_of(e.finish()));
        assert!(r.svg.contains("data:image/svg+xml;base64,"), "{}", r.svg);
        assert!(!r.svg.contains("#e6e6e6"), "no placeholder");
        let img = raster(&r);
        // The slide scales: pixel scale is 1 here (960 px wide slide).
        let p = spx(&img, 200, 100);
        assert!(p[0] > 200 && p[1] < 60 && p[2] < 60, "{p:?}");
    }

    #[test]
    fn a_wmf_picture_is_drawn() {
        let mut w = Wmf::new(100, 50);
        w.window(100, 50);
        w.brush(0, rgb(0, 0, 255), 0).select(0);
        w.rect(0, 0, 100, 50);
        let s = picture_scene("m.wmf");
        let r = render_svg(&s, &media_of(w.finish()));
        assert!(r.svg.contains("data:image/svg+xml;base64,"));
        let img = raster(&r);
        let p = spx(&img, 200, 100);
        assert!(p[2] > 200 && p[0] < 60, "{p:?}");
    }

    #[test]
    fn a_picture_stretches_to_its_box_not_its_aspect() {
        // A square metafile placed in a 2:1 box fills the box.
        let mut e = Emf::new(100, 100);
        e.fill_with(rgb(255, 0, 0)).rect(0, 0, 100, 100);
        let r = render_svg(&picture_scene("m.emf"), &media_of(e.finish()));
        let img = raster(&r);
        let (a, b) = (spx(&img, 105, 55), spx(&img, 295, 145));
        assert!(
            a[0] > 200 && a[1] < 60 && b[0] > 200 && b[1] < 60,
            "{a:?} {b:?}"
        );
    }

    #[test]
    fn a_broken_metafile_stays_a_placeholder() {
        // EMF signature but nothing usable.
        let mut bad = vec![0u8; 100];
        bad[0] = 1;
        bad[40..44].copy_from_slice(b" EMF");
        let r = render_svg(&picture_scene("m.emf"), &media_of(bad));
        assert!(r.svg.contains("#e6e6e6"), "{}", r.svg);
    }

    #[test]
    fn an_emf_plus_only_picture_stays_a_placeholder() {
        let mut e = Emf::new(100, 100);
        let mut c = words(&[12]);
        c.extend(b"EMF+\0\0\0\0");
        e.raw(70, &c);
        let r = render_svg(&picture_scene("m.emf"), &media_of(e.finish()));
        assert!(r.svg.contains("#e6e6e6"));
    }
}

// ----- the local corpus ----------------------------------------------------------------------

/// Converts every metafile of the local PowerPoint corpus, rasterizes it and writes it next to
/// LibreOffice's rendering of the same file. Local only: the corpus is not in the repository.
///
/// `cargo test -- --ignored dump_metafile_corpus`; needs the files extracted to
/// `NoCode/.cache/slide-c2-work/meta` and (optionally) LibreOffice's PNGs in `.../lo`.
#[test]
#[ignore]
fn dump_metafile_corpus() {
    let meta = Path::new("/Users/shuhei/work/NoCode/.cache/slide-c2-work/meta");
    let lo = Path::new("/Users/shuhei/work/NoCode/.cache/slide-c2-work/lo");
    let out = Path::new("/Users/shuhei/work/konoma/docs/render-check/metafile");
    if let Ok(rd) = std::fs::read_dir(out) {
        for f in rd.flatten() {
            let _ = std::fs::remove_file(f.path());
        }
    }
    std::fs::create_dir_all(out).unwrap();
    let mut names: Vec<_> = std::fs::read_dir(meta)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    names.sort();
    let mut report = String::new();
    for p in names {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if !(name.ends_with(".emf") || name.ends_with(".wmf")) {
            continue;
        }
        let bytes = std::fs::read(&p).unwrap();
        let t = std::time::Instant::now();
        let m = to_svg(&bytes);
        let ms = t.elapsed().as_millis();
        let Some(m) = m else {
            report.push_str(&format!("{name}: NONE ({} bytes)\n", bytes.len()));
            continue;
        };
        let _ = std::fs::write(
            Path::new("/Users/shuhei/work/NoCode/.cache/slide-c2-work/svg")
                .join(format!("{name}.svg")),
            &m.svg,
        );
        let ours =
            crate::preview::svg::rasterize_trusted(m.svg.as_bytes(), Path::new("/m.svg"), 400)
                .map(|i| i.to_rgba8());
        let theirs = image::open(lo.join(name.rsplit_once('.').unwrap().0.to_string() + ".png"))
            .ok()
            .map(|i| i.to_rgba8());
        report.push_str(&format!(
            "{name}: {} bytes, svg {} bytes, {:.0}x{:.0} px, {ms} ms{}\n",
            bytes.len(),
            m.svg.len(),
            m.width_px,
            m.height_px,
            if m.truncated { ", TRUNCATED" } else { "" }
        ));
        let Some(ours) = ours else {
            report.push_str("   does not rasterize\n");
            continue;
        };
        let h = 400u32;
        let tw = |i: &image::RgbaImage| {
            ((i.width() as f64 * h as f64 / i.height() as f64) as u32).max(1)
        };
        let ours = image::imageops::resize(
            &ours,
            tw(&ours).min(800),
            h,
            image::imageops::FilterType::Triangle,
        );
        let theirs = theirs.map(|t| {
            image::imageops::resize(
                &t,
                tw(&t).min(800),
                h,
                image::imageops::FilterType::Triangle,
            )
        });
        let w2 = theirs.as_ref().map_or(0, |t| t.width());
        let mut canvas = image::RgbaImage::from_pixel(
            ours.width() + w2 + 30,
            h + 20,
            image::Rgba([200, 200, 255, 255]),
        );
        let paste = |canvas: &mut image::RgbaImage, img: &image::RgbaImage, x0: u32| {
            for (x, y, p) in img.enumerate_pixels() {
                let a = p.0[3] as f32 / 255.0;
                let bg = if (x / 8 + y / 8) % 2 == 0 {
                    255.0
                } else {
                    230.0
                };
                let m = |c: u8| (c as f32 * a + bg * (1.0 - a)) as u8;
                canvas.put_pixel(
                    x0 + x,
                    y + 10,
                    image::Rgba([m(p.0[0]), m(p.0[1]), m(p.0[2]), 255]),
                );
            }
        };
        paste(&mut canvas, &ours, 10);
        if let Some(t) = &theirs {
            paste(&mut canvas, t, 20 + ours.width());
        }
        canvas
            .save(out.join(format!("{}.png", name.rsplit_once('.').unwrap().0)))
            .unwrap();
    }
    std::fs::write(out.join("REPORT.txt"), &report).unwrap();
    println!("{report}");
}
