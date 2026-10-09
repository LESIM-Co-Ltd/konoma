//! `SlideScene` -> SVG.
//!
//! The picture is written in **px at 96 dpi** (the model's EMU divided by 9525); the root has
//! `viewBox="0 0 W H"` with `width`/`height` equal to it. Everything user-supplied (text, font
//! names, preset names, image keys) goes through [`esc`] before it reaches an attribute or a text
//! node; numbers go through [`num`], which never prints NaN or infinity.
//!
//! # What is approximated (all documented where it happens)
//!
//! * **Gradients**: linear gradients use the exact DrawingML gradient line (the line through the
//!   box centre at the angle, long enough that the corners get the end colours; `scaled`
//!   angles are applied in the unit square). `Rect` gradients are concentric rectangles from the
//!   focus rectangle to the box edges (four linear-gradient wedges in a `<pattern>`, see
//!   `rect_gradient`). `Radial` is an SVG radial gradient centred on the focus rectangle; `Path`
//!   (`path="shape"`) has no SVG equivalent and is drawn as the same radial gradient (the colours
//!   run the right way, the iso-lines are ellipses instead of the shape's outline).
//!   `rot_with_shape = false` is ignored (the gradient turns with the shape).
//! * **Patterns**: 8 x 8 px tiles drawn from per-pixel rules (see `patterns`: the 54 bitmaps
//!   measured from PowerPoint, not scaled with the shape). Unknown presets are the 50 % pattern.
//! * **Compound lines**: `Dbl` and `Tri` are exact (a mask cuts the gaps out of one wide stroke);
//!   `ThickThin` and `ThinThick` are asymmetric and are drawn like `Dbl`.
//! * **Arrow heads**: sized `sm/med/lg = 2/3/5 x` the line width (never less than 2 pt of width,
//!   so heads on hairlines stay visible) for both width and length, the way LibreOffice and
//!   PowerPoint's dialogs describe them; the line is shortened under a filled head when its last
//!   segment is straight.
//! * **Effects** (outer shadow, glow, reflection, soft edge, inner shadow): see `svg_effects`; the
//!   painted shape is defined once and every layer is a filtered `<use>` of it.
//! * **Fill modes** `lighten`/`darken` mix the fill colour 40 % (`...Less`: 20 %) towards white
//!   or black.
//! * **Pictures**: PNG, JPEG, GIF and WebP are embedded as they are; BMP and TIFF are decoded and
//!   re-embedded as PNG; SVG is embedded as `image/svg+xml`. EMF / WMF are converted to SVG
//!   ([`super::metafile`]) and embedded the same way; a metafile that cannot be converted and
//!   anything unrecognised become a light grey placeholder box.
//! * Tiled images ignore the crop.
//!
//! # Budgets
//!
//! [`MAX_SVG_TEXT_BYTES`] bounds the markup (embedded image data excluded) and
//! [`MAX_EMBEDDED_IMAGE_BYTES`] the image bytes embedded in one slide. When either is reached the
//! remaining items are not drawn and the result is marked truncated.

use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::Arc;

use base64::Engine as _;

use super::color::mix;
use super::model::*;
use super::path::{self, Resolved, Seg};
use super::patterns::pattern_pixels;
use super::text::{self, BulletDraw, Frag, Frame, ASCENT, LINE_HEIGHT};

#[path = "svg_effects.rs"]
mod effects;

/// Largest SVG markup written for one slide, in bytes, not counting embedded image data.
pub const MAX_SVG_TEXT_BYTES: usize = 16 * 1024 * 1024;
/// Largest total of embedded image bytes (before base64) in one slide.
pub const MAX_EMBEDDED_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// Deepest group nesting that is drawn.
pub const MAX_GROUP_DEPTH: usize = 64;
/// Largest picture, in pixels per side, that BMP / TIFF conversion decodes.
const MAX_CONVERT_SIDE: u32 = 8192;

/// The MIME types of what [`sniff`] recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sniffed {
    Png,
    Jpeg,
    Gif,
    Webp,
    Svg,
    Bmp,
    Tiff,
    Emf,
    Wmf,
    Unknown,
}

/// Recognises an image format from its first bytes.
pub(crate) fn sniff(b: &[u8]) -> Sniffed {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Sniffed::Png
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Sniffed::Jpeg
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Sniffed::Gif
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Sniffed::Webp
    } else if b.starts_with(b"BM") && b.len() > 14 {
        Sniffed::Bmp
    } else if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        Sniffed::Tiff
    } else if b.len() > 44 && b[0..4] == [1, 0, 0, 0] && &b[40..44] == b" EMF" {
        Sniffed::Emf
    } else if b.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A])
        || (b.len() > 18 && matches!(b[0..4], [1, 0, 9, 0] | [2, 0, 9, 0]))
    {
        Sniffed::Wmf
    } else {
        let head = &b[..b.len().min(1024)];
        let s = String::from_utf8_lossy(head);
        let t = s.trim_start_matches('\u{feff}').trim_start();
        if (t.starts_with("<?xml")
            || t.starts_with("<svg")
            || t.starts_with("<!--")
            || t.starts_with("<!DOCTYPE svg"))
            && s.contains("<svg")
        {
            Sniffed::Svg
        } else {
            Sniffed::Unknown
        }
    }
}

/// XML-escapes text for attribute values and text nodes, and drops characters XML cannot carry.
pub(crate) fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            '\t' | '\n' | '\r' => o.push(' '),
            c if (c as u32) < 0x20 || matches!(c as u32, 0x7F..=0x9F | 0xFFFE | 0xFFFF) => {}
            c => o.push(c),
        }
    }
    o
}

/// A finite number with at most three decimals.
pub(crate) fn num(v: f64) -> String {
    let v = if v.is_finite() {
        v.clamp(-1e7, 1e7)
    } else {
        0.0
    };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// EMU to px, printed.
fn px(v: f64) -> String {
    num(v / EMU_PER_PX)
}

fn hex(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

fn alpha_of(c: Rgba) -> f64 {
    if c.a.is_finite() {
        c.a.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Box in EMU: x, y, w, h.
type Bx = (f64, f64, f64, f64);

#[derive(Clone, Copy, Default)]
struct Ctx {
    /// Accumulated mirroring of the ancestors (horizontal, vertical).
    flip: (bool, bool),
    /// Accumulated rotation of the ancestors in degrees (screen angle).
    rot: f64,
    depth: usize,
}

/// Widest stroke (px) that is drawn without anti-aliasing when it is axis-aligned.
const CRISP_MAX_PX: f64 = 1.5;

/// Whether a rotation (degrees) leaves the horizontal and vertical directions as they are.
fn screen_axis_aligned(rot_deg: f64) -> bool {
    rot_deg.is_finite() && ((rot_deg / 90.0) - (rot_deg / 90.0).round()).abs() < 1e-6
}

/// Whether every segment is a horizontal or vertical line.
fn axis_aligned_segs(segs: &[Seg]) -> bool {
    let mut cur: Option<Pt> = None;
    for s in segs {
        match s {
            Seg::M(p) => cur = Some(*p),
            Seg::L(p) => {
                let Some(c) = cur else {
                    return false;
                };
                if (c.x - p.x).abs() > 1e-6 && (c.y - p.y).abs() > 1e-6 {
                    return false;
                }
                cur = Some(*p);
            }
            Seg::Z => {}
            Seg::Q(..) | Seg::C(..) => return false,
        }
    }
    true
}

struct W<'a> {
    body: String,
    defs: String,
    next_id: usize,
    embedded_len: usize,
    embedded_raw: usize,
    truncated: bool,
    media: &'a dyn Fn(&str) -> Option<Arc<Vec<u8>>>,
}

/// Renders a scene; see the module documentation.
pub(super) fn render(
    scene: &SlideScene,
    media: &dyn Fn(&str) -> Option<Arc<Vec<u8>>>,
) -> super::Rendered {
    let (sw, sh) = (clamp_dim(scene.width), clamp_dim(scene.height));
    let mut w = W {
        body: String::new(),
        defs: String::new(),
        next_id: 0,
        embedded_len: 0,
        embedded_raw: 0,
        truncated: scene.truncated,
        media,
    };
    // Background.
    let bg_box: Bx = (0.0, 0.0, sw, sh);
    let d = path_d(&path::rect_segs(0.0, 0.0, sw, sh));
    match &scene.background {
        Fill::None => {
            let _ = write!(
                w.body,
                r##"<rect width="{}" height="{}" fill="#ffffff"/>"##,
                px(sw),
                px(sh)
            );
        }
        f => w.paint(&d, f, bg_box, "", PathFill::Norm),
    }
    for item in &scene.items {
        if w.over_budget() {
            w.truncated = true;
            break;
        }
        w.item(item, Ctx::default());
    }
    let mut svg = String::with_capacity(w.body.len() + w.defs.len() + 256);
    let _ = write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 {} {}" width="{}" height="{}">"#,
        px(sw),
        px(sh),
        px(sw),
        px(sh)
    );
    if !w.defs.is_empty() {
        svg.push_str("<defs>");
        svg.push_str(&w.defs);
        svg.push_str("</defs>");
    }
    svg.push_str(&w.body);
    svg.push_str("</svg>");
    super::Rendered {
        svg,
        truncated: w.truncated,
    }
}

fn clamp_dim(v: f64) -> f64 {
    // 1 px .. 100 000 px; a slide is 1280 x 720 px at 96 dpi
    if v.is_finite() {
        v.clamp(EMU_PER_PX, 100_000.0 * EMU_PER_PX)
    } else {
        13.333 * EMU_PER_IN
    }
}

fn path_d(segs: &[Seg]) -> String {
    let mut d = String::with_capacity(segs.len() * 24);
    let p = |q: Pt| format!("{} {}", px(q.x), px(q.y));
    for s in segs {
        match *s {
            Seg::M(a) => {
                let _ = write!(d, "M{} ", p(a));
            }
            Seg::L(a) => {
                let _ = write!(d, "L{} ", p(a));
            }
            Seg::Q(a, b) => {
                let _ = write!(d, "Q{} {} ", p(a), p(b));
            }
            Seg::C(a, b, c) => {
                let _ = write!(d, "C{} {} {} ", p(a), p(b), p(c));
            }
            Seg::Z => d.push_str("Z "),
        }
    }
    d.truncate(d.trim_end().len());
    d
}

fn has_transform(x: &Xfrm) -> bool {
    (x.rot_deg.is_finite() && x.rot_deg.rem_euclid(360.0) != 0.0) || x.flip_h || x.flip_v
}

/// `transform` attribute value for a box's rotation and flips about its centre.
fn xfrm_transform(x: &Xfrm) -> String {
    let (cx, cy) = (x.x + x.w / 2.0, x.y + x.h / 2.0);
    let mut t = format!("translate({} {})", px(cx), px(cy));
    let rot = if x.rot_deg.is_finite() {
        x.rot_deg
    } else {
        0.0
    };
    if rot.rem_euclid(360.0) != 0.0 {
        let _ = write!(t, " rotate({})", num(rot));
    }
    if x.flip_h || x.flip_v {
        let _ = write!(
            t,
            " scale({} {})",
            if x.flip_h { -1 } else { 1 },
            if x.flip_v { -1 } else { 1 }
        );
    }
    let _ = write!(t, " translate({} {})", px(-cx), px(-cy));
    t
}

impl<'a> W<'a> {
    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn text_len(&self) -> usize {
        (self.body.len() + self.defs.len()).saturating_sub(self.embedded_len)
    }

    fn over_budget(&self) -> bool {
        self.text_len() > MAX_SVG_TEXT_BYTES || self.embedded_raw > MAX_EMBEDDED_IMAGE_BYTES
    }

    // ----- items -----------------------------------------------------------------------------

    fn item(&mut self, item: &Item, cx: Ctx) {
        match item {
            Item::Shape(s) => self.shape(s, cx),
            Item::Picture(p) => self.picture(p, cx),
            Item::Group(g) => self.group(g, cx),
        }
    }

    fn open_g(&mut self, x: &Xfrm) -> bool {
        if has_transform(x) {
            let _ = write!(self.body, r#"<g transform="{}">"#, xfrm_transform(x));
            true
        } else {
            false
        }
    }

    fn group(&mut self, g: &GroupItem, cx: Ctx) {
        if cx.depth >= MAX_GROUP_DEPTH {
            self.truncated = true;
            return;
        }
        let x = &g.xfrm;
        let (cw, ch) = (
            if g.child_ext.0.is_finite() && g.child_ext.0.abs() > 1e-9 {
                g.child_ext.0
            } else {
                x.w
            },
            if g.child_ext.1.is_finite() && g.child_ext.1.abs() > 1e-9 {
                g.child_ext.1
            } else {
                x.h
            },
        );
        let sx = if cw.abs() > 1e-9 { x.w / cw } else { 1.0 };
        let sy = if ch.abs() > 1e-9 { x.h / ch } else { 1.0 };
        let (ox, oy) = (
            if g.child_off.0.is_finite() {
                g.child_off.0
            } else {
                0.0
            },
            if g.child_off.1.is_finite() {
                g.child_off.1
            } else {
                0.0
            },
        );
        let mut t = String::new();
        if has_transform(x) {
            t.push_str(&xfrm_transform(x));
            t.push(' ');
        }
        // child space -> group box: (p - off) * scale + box origin, in px
        let _ = write!(
            t,
            "translate({} {}) scale({} {}) translate({} {})",
            px(x.x),
            px(x.y),
            num(if sx.is_finite() {
                sx.clamp(-1e6, 1e6)
            } else {
                1.0
            }),
            num(if sy.is_finite() {
                sy.clamp(-1e6, 1e6)
            } else {
                1.0
            }),
            px(-ox),
            px(-oy)
        );
        let _ = write!(self.body, r#"<g transform="{t}">"#);
        let mirrored_before = cx.flip;
        let rot_sign = if mirrored_before.0 != mirrored_before.1 {
            -1.0
        } else {
            1.0
        };
        let inner = Ctx {
            flip: (cx.flip.0 ^ x.flip_h, cx.flip.1 ^ x.flip_v),
            rot: cx.rot
                + rot_sign
                    * if x.rot_deg.is_finite() {
                        x.rot_deg
                    } else {
                        0.0
                    },
            depth: cx.depth + 1,
        };
        for it in &g.items {
            if self.over_budget() {
                self.truncated = true;
                break;
            }
            self.item(it, inner);
        }
        self.body.push_str("</g>");
    }

    fn shape(&mut self, s: &ShapeItem, cx: Ctx) {
        let x = &s.xfrm;
        let bx: Bx = (x.x, x.y, x.w, x.h);
        let opened = self.open_g(x);
        let axis = screen_axis_aligned(cx.rot + x.rot_deg);
        let (paths, trunc) = path::resolve_geometry(&s.geom, x);
        self.truncated |= trunc;
        let start = self.body.len();
        let filled_geom = !matches!(s.geom, Geometry::Line);
        // All the fills first, then all the outlines: a later path's fill must not cover an
        // earlier path's outline (`chartPlus` / `chartX` draw their cross first, the box second).
        if filled_geom && s.fill.is_visible() {
            for (r, mode, _) in &paths {
                if *mode != PathFill::None {
                    self.paint(&path_d(&r.segs), &s.fill, bx, "", *mode);
                }
            }
        }
        if let Some(l) = &s.line {
            for (r, _, stroke) in &paths {
                if *stroke {
                    self.stroke(r, l, bx, axis);
                }
            }
        }
        let content = self.body.split_off(start);
        self.with_effects(&content, bx, &s.effects, x, cx.rot);
        if let Some(t) = &s.text {
            self.text_body(t, s.text_rect, x, cx);
        }
        if opened {
            self.body.push_str("</g>");
        }
    }

    // ----- fills -----------------------------------------------------------------------------

    fn adjust(c: Rgba, mode: PathFill) -> Rgba {
        match mode {
            PathFill::Lighten => mix(c, Rgba::WHITE.with_alpha(c.a), 0.4),
            PathFill::LightenLess => mix(c, Rgba::WHITE.with_alpha(c.a), 0.2),
            PathFill::Darken => mix(c, Rgba::BLACK.with_alpha(c.a), 0.4),
            PathFill::DarkenLess => mix(c, Rgba::BLACK.with_alpha(c.a), 0.2),
            _ => c,
        }
    }

    /// Writes `<path d=.../>` filled with `fill`. `extra` is appended to the path's attributes.
    fn paint(&mut self, d: &str, fill: &Fill, bx: Bx, extra: &str, mode: PathFill) {
        match fill {
            Fill::None => {}
            Fill::Image(img) => self.image_fill(d, img, bx),
            other => {
                if let Some(attr) = self.paint_attr(other, bx, "fill", mode) {
                    let _ = write!(
                        self.body,
                        r#"<path d="{d}" {attr} fill-rule="evenodd"{extra}/>"#
                    );
                }
            }
        }
    }

    /// The paint of `fill` as an attribute string (`fill="..."` plus an opacity attribute), or
    /// `None` when nothing is painted. `attr` is `fill` or `stroke`.
    fn paint_attr(&mut self, fill: &Fill, bx: Bx, attr: &str, mode: PathFill) -> Option<String> {
        let op = |c: Rgba| {
            let a = alpha_of(c);
            if a < 1.0 {
                format!(r#" {attr}-opacity="{}""#, num(a))
            } else {
                String::new()
            }
        };
        match fill {
            Fill::None => None,
            Fill::Solid(c) => {
                if c.a <= 0.0 {
                    return None;
                }
                let c = Self::adjust(*c, mode);
                Some(format!(r#"{attr}="{}"{}"#, hex(c), op(c)))
            }
            Fill::Gradient(g) => {
                if g.stops.is_empty() {
                    return None;
                }
                if g.stops.len() == 1 {
                    return self.paint_attr(&Fill::Solid(g.stops[0].1), bx, attr, mode);
                }
                let id = self.gradient(g, bx, mode);
                Some(format!(r#"{attr}="url(#{id})""#))
            }
            Fill::Pattern { preset, fg, bg } => {
                let id = self.pattern(preset, Self::adjust(*fg, mode), Self::adjust(*bg, mode));
                Some(format!(r#"{attr}="url(#{id})""#))
            }
            Fill::Image(_) => {
                // Not expressible as a paint (a line drawn with a picture): a mid grey.
                Some(format!(r##"{attr}="#808080""##))
            }
        }
    }

    fn gradient(&mut self, g: &Gradient, bx: Bx, mode: PathFill) -> String {
        let id = self.id("gr");
        let mut stops: Vec<(f64, Rgba)> = g
            .stops
            .iter()
            .map(|(p, c)| {
                (
                    if p.is_finite() {
                        p.clamp(0.0, 1.0)
                    } else {
                        0.0
                    },
                    Self::adjust(*c, mode),
                )
            })
            .collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut stop_xml = String::new();
        for (p, c) in &stops {
            let _ = write!(
                stop_xml,
                r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
                num(*p),
                hex(*c),
                num(alpha_of(*c))
            );
        }
        let (x, y, w, h) = bx;
        match g.kind {
            GradKind::Linear { angle_deg, scaled } => {
                let a = if angle_deg.is_finite() {
                    angle_deg.to_radians()
                } else {
                    0.0
                };
                let (c, s) = (a.cos(), a.sin());
                if scaled {
                    let half = (c.abs() + s.abs()) / 2.0;
                    let _ = write!(
                        self.defs,
                        r#"<linearGradient id="{id}" gradientUnits="objectBoundingBox" x1="{}" y1="{}" x2="{}" y2="{}">{stop_xml}</linearGradient>"#,
                        num(0.5 - c * half),
                        num(0.5 - s * half),
                        num(0.5 + c * half),
                        num(0.5 + s * half),
                    );
                } else {
                    let (cxm, cym) = (x + w / 2.0, y + h / 2.0);
                    let half = (w * c).abs() / 2.0 + (h * s).abs() / 2.0;
                    let _ = write!(
                        self.defs,
                        r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}">{stop_xml}</linearGradient>"#,
                        px(cxm - c * half),
                        px(cym - s * half),
                        px(cxm + c * half),
                        px(cym + s * half),
                    );
                }
            }
            GradKind::Rect => self.rect_gradient(&id, g, &stop_xml),
            GradKind::Radial | GradKind::Path => {
                let (l, t, r, b) = g.fill_to_rect;
                let f = |v: f64| {
                    if v.is_finite() {
                        v.clamp(0.0, 1.0)
                    } else {
                        0.5
                    }
                };
                let (l, t, r, b) = (f(l), f(t), f(r), f(b));
                let (fx, fy) = ((l + (1.0 - r)) / 2.0, (t + (1.0 - b)) / 2.0);
                // The circle is centred on the focus rectangle and reaches the last stop at half the
                // box diagonal (the circle through the corners of a centred focus). Measured on
                // LibreOffice's rendering of a dark-theme deck: the grey falls to black at
                // 0.6 of the width from a top-right focus, i.e. at 600 px of 960x720, not at the
                // farthest corner.
                let rad = w.hypot(h) / 2.0;
                let _ = write!(
                    self.defs,
                    r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" cx="{}" cy="{}" fx="{}" fy="{}" r="{}">{stop_xml}</radialGradient>"#,
                    px(x + fx * w),
                    px(y + fy * h),
                    px(x + fx * w),
                    px(y + fy * h),
                    px(rad.max(0.01)),
                );
            }
        }
        id
    }

    /// A rectangular gradient (`path="rect"`): concentric rectangles from the focus rectangle
    /// (`fillToRect`, the first stop) out to the box edges (the last stop), as PowerPoint draws
    /// it. SVG has no such gradient, so it is a `<pattern>` over the box (in bounding-box units)
    /// of four linear-gradient wedges, one per side, cut along the diagonals from the box corners
    /// to the focus rectangle's corners, over an elliptical-gradient underlay that hides the
    /// anti-aliased seams between the wedges.
    fn rect_gradient(&mut self, id: &str, g: &Gradient, stop_xml: &str) {
        let f = |v: f64| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.5
            }
        };
        let (l, t, r, b) = g.fill_to_rect;
        let (fl, ft) = (f(l), f(t));
        let (fr, fb) = ((1.0 - f(r)).max(fl), (1.0 - f(b)).max(ft));
        let first = g.stops.first().map_or(Rgba::BLACK, |s| s.1);
        let mut inner = String::new();
        let (cx, cy) = ((fl + fr) / 2.0, (ft + fb) / 2.0);
        let _ = write!(
            self.defs,
            r#"<radialGradient id="{id}u" gradientUnits="userSpaceOnUse" cx="{}" cy="{}" fx="{}" fy="{}" r="{}">{stop_xml}</radialGradient>"#,
            num(cx),
            num(cy),
            num(cx),
            num(cy),
            num(cx.max(1.0 - cx).hypot(cy.max(1.0 - cy)).max(0.01)),
        );
        let _ = write!(inner, r#"<rect width="1" height="1" fill="url(#{id}u)"/>"#);
        // (side, vector x1 y1 x2 y2, wedge polygon)
        type Wedge = ([f64; 4], [(f64, f64); 4]);
        let wedges: [(&str, Wedge); 4] = [
            (
                "l",
                (
                    [fl, 0.0, 0.0, 0.0],
                    [(0.0, 0.0), (fl, ft), (fl, fb), (0.0, 1.0)],
                ),
            ),
            (
                "t",
                (
                    [0.0, ft, 0.0, 0.0],
                    [(0.0, 0.0), (1.0, 0.0), (fr, ft), (fl, ft)],
                ),
            ),
            (
                "r",
                (
                    [fr, 0.0, 1.0, 0.0],
                    [(1.0, 0.0), (1.0, 1.0), (fr, fb), (fr, ft)],
                ),
            ),
            (
                "b",
                (
                    [0.0, fb, 0.0, 1.0],
                    [(0.0, 1.0), (fl, fb), (fr, fb), (1.0, 1.0)],
                ),
            ),
        ];
        for (side, (v, poly)) in wedges {
            let len = (v[2] - v[0]).abs() + (v[3] - v[1]).abs();
            if len < 1e-6 {
                continue;
            }
            let _ = write!(
                self.defs,
                r#"<linearGradient id="{id}{side}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}">{stop_xml}</linearGradient>"#,
                num(v[0]),
                num(v[1]),
                num(v[2]),
                num(v[3]),
            );
            let pts: Vec<String> = poly
                .iter()
                .map(|(x, y)| format!("{} {}", num(*x), num(*y)))
                .collect();
            let _ = write!(
                inner,
                r#"<path d="M{} Z" fill="url(#{id}{side})"/>"#,
                pts.join(" L")
            );
        }
        // The focus rectangle itself is the first colour.
        if fr > fl && fb > ft {
            let _ = write!(
                inner,
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" fill-opacity="{}"/>"#,
                num(fl),
                num(ft),
                num(fr - fl),
                num(fb - ft),
                hex(first),
                num(alpha_of(first)),
            );
        }
        let _ = write!(
            self.defs,
            r#"<pattern id="{id}" patternUnits="objectBoundingBox" patternContentUnits="objectBoundingBox" width="1" height="1">{inner}</pattern>"#
        );
    }

    fn pattern(&mut self, preset: &str, fg: Rgba, bg: Rgba) -> String {
        let id = self.id("pt");
        let mut fgd = String::new();
        let px_on = pattern_pixels(preset);
        for (yy, row) in px_on.iter().enumerate() {
            let mut xx = 0;
            while xx < 8 {
                if row[xx] {
                    let start = xx;
                    while xx < 8 && row[xx] {
                        xx += 1;
                    }
                    let _ = write!(fgd, "M{start} {yy}h{}v1h-{}z", xx - start, xx - start);
                } else {
                    xx += 1;
                }
            }
        }
        let _ = write!(
            self.defs,
            r#"<pattern id="{id}" patternUnits="userSpaceOnUse" width="8" height="8"><rect width="8" height="8" fill="{}" fill-opacity="{}"/><path d="{fgd}" fill="{}" fill-opacity="{}" shape-rendering="crispEdges"/></pattern>"#,
            hex(bg),
            num(alpha_of(bg)),
            hex(fg),
            num(alpha_of(fg)),
        );
        id
    }

    // ----- images ----------------------------------------------------------------------------

    /// The data URI of the image `key`, or `None` when it is missing, unusable or over budget.
    fn embed(&mut self, key: &str) -> Option<(String, Sniffed)> {
        let bytes = (self.media)(key)?;
        if bytes.is_empty() {
            return None;
        }
        let kind = sniff(&bytes);
        let (mime, data): (&str, Vec<u8>) = match kind {
            Sniffed::Png => ("image/png", bytes.to_vec()),
            Sniffed::Jpeg => ("image/jpeg", bytes.to_vec()),
            Sniffed::Gif => ("image/gif", bytes.to_vec()),
            Sniffed::Webp => ("image/webp", bytes.to_vec()),
            Sniffed::Svg => ("image/svg+xml", bytes.to_vec()),
            Sniffed::Bmp | Sniffed::Tiff => ("image/png", to_png(&bytes)?),
            Sniffed::Emf | Sniffed::Wmf => {
                let m = super::metafile::to_svg(&bytes)?;
                self.truncated |= m.truncated;
                ("image/svg+xml", m.svg.into_bytes())
            }
            Sniffed::Unknown => return None,
        };
        if self.embedded_raw + data.len() > MAX_EMBEDDED_IMAGE_BYTES {
            self.truncated = true;
            return None;
        }
        self.embedded_raw += data.len();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        let uri = format!("data:{mime};base64,{b64}");
        self.embedded_len += uri.len();
        Some((uri, kind))
    }

    /// Natural pixel size of the image `key` (for tiling), 96 dpi.
    fn natural_size(&self, key: &str) -> Option<(f64, f64)> {
        let bytes = (self.media)(key)?;
        match sniff(&bytes) {
            Sniffed::Svg => {
                crate::preview::svg::intrinsic_size_bytes(&bytes).map(|(w, h)| (w as f64, h as f64))
            }
            Sniffed::Emf | Sniffed::Wmf => {
                super::metafile::to_svg(&bytes).map(|m| (m.width_px, m.height_px))
            }
            Sniffed::Unknown => None,
            _ => {
                let r = image::ImageReader::new(Cursor::new(&bytes[..]))
                    .with_guessed_format()
                    .ok()?;
                r.into_dimensions().ok().map(|(w, h)| (w as f64, h as f64))
            }
        }
    }

    /// An image drawn into the area `(x, y, w, h)` honouring `crop` and `fill_rect`, clipped by
    /// `clip` (a clipPath id) when given.
    #[allow(clippy::too_many_arguments)]
    fn image_el(
        &mut self,
        uri: &str,
        area: Bx,
        crop: Rect4,
        fill_rect: Rect4,
        alpha: f64,
        clip: Option<&str>,
    ) {
        let (x, y, w, h) = area;
        let f = |v: f64| if v.is_finite() { v } else { 0.0 };
        let (fl, ft, fr, fb) = (
            f(fill_rect.0),
            f(fill_rect.1),
            f(fill_rect.2),
            f(fill_rect.3),
        );
        let (ax, ay, aw, ah) = (
            x + w * fl,
            y + h * ft,
            w * (1.0 - fl - fr),
            h * (1.0 - ft - fb),
        );
        let (cl, ct, cr, cb) = (f(crop.0), f(crop.1), f(crop.2), f(crop.3));
        let kw = (1.0 - cl - cr).max(0.001);
        let kh = (1.0 - ct - cb).max(0.001);
        let (fw, fh) = (aw / kw, ah / kh);
        let (ix, iy) = (ax - cl * fw, ay - ct * fh);
        let a = if alpha.is_finite() {
            alpha.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let op = if a < 1.0 {
            format!(r#" opacity="{}""#, num(a))
        } else {
            String::new()
        };
        let clip_attr = clip
            .map(|c| format!(r#" clip-path="url(#{c})""#))
            .unwrap_or_default();
        let _ = write!(
            self.body,
            r#"<image x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="none" href="{uri}"{op}{clip_attr}/>"#,
            px(ix),
            px(iy),
            px(fw),
            px(fh),
        );
    }

    fn clip_def(&mut self, d: &str) -> String {
        let id = self.id("cp");
        let _ = write!(
            self.defs,
            r#"<clipPath id="{id}"><path d="{d}"/></clipPath>"#
        );
        id
    }

    fn placeholder(&mut self, d: &str, area: Bx) {
        let (x, y, w, h) = area;
        let _ = write!(
            self.body,
            r##"<path d="{d}" fill="#e6e6e6" stroke="#bfbfbf" stroke-width="1"/><path d="M{} {}L{} {}M{} {}L{} {}" stroke="#bfbfbf" stroke-width="1" fill="none"/>"##,
            px(x),
            px(y),
            px(x + w),
            px(y + h),
            px(x + w),
            px(y),
            px(x),
            px(y + h),
        );
    }

    fn image_fill(&mut self, d: &str, img: &ImageFill, bx: Bx) {
        match img.mode {
            ImageMode::Stretch { fill_rect } => {
                let Some((uri, _)) = self.embed(&img.key) else {
                    self.placeholder(d, bx);
                    return;
                };
                let clip = self.clip_def(d);
                self.image_el(&uri, bx, img.crop, fill_rect, img.alpha, Some(&clip));
            }
            ImageMode::Tile {
                sx,
                sy,
                tx,
                ty,
                align,
                flip,
            } => {
                let Some((nw, nh)) = self.natural_size(&img.key) else {
                    self.placeholder(d, bx);
                    return;
                };
                let Some((uri, _)) = self.embed(&img.key) else {
                    self.placeholder(d, bx);
                    return;
                };
                let (x, y, w, h) = bx;
                let s = |v: f64| {
                    if v.is_finite() && v > 0.0 {
                        v.min(1000.0)
                    } else {
                        1.0
                    }
                };
                let (tw, th) = ((nw * s(sx)).max(1.0), (nh * s(sy)).max(1.0));
                let (fx, fy) = match align {
                    RectAlign::TopLeft => (0.0, 0.0),
                    RectAlign::Top => (0.5, 0.0),
                    RectAlign::TopRight => (1.0, 0.0),
                    RectAlign::Left => (0.0, 0.5),
                    RectAlign::Center => (0.5, 0.5),
                    RectAlign::Right => (1.0, 0.5),
                    RectAlign::BottomLeft => (0.0, 1.0),
                    RectAlign::Bottom => (0.5, 1.0),
                    RectAlign::BottomRight => (1.0, 1.0),
                };
                let ox = x + w * fx - tw * EMU_PER_PX * fx + if tx.is_finite() { tx } else { 0.0 };
                let oy = y + h * fy - th * EMU_PER_PX * fy + if ty.is_finite() { ty } else { 0.0 };
                let pid = self.id("pi");
                let (fh, fv) = match flip {
                    TileFlip::None => (false, false),
                    TileFlip::X => (true, false),
                    TileFlip::Y => (false, true),
                    TileFlip::Xy => (true, true),
                };
                let (pw, ph) = (
                    tw * if fh { 2.0 } else { 1.0 },
                    th * if fv { 2.0 } else { 1.0 },
                );
                let mut cells = String::new();
                for iy in 0..(if fv { 2 } else { 1 }) {
                    for ix in 0..(if fh { 2 } else { 1 }) {
                        let (mx, my) = (ix == 1, iy == 1);
                        let t = format!(
                            "translate({} {}) scale({} {})",
                            num(if mx { tw * 2.0 } else { 0.0 }),
                            num(if my { th * 2.0 } else { 0.0 }),
                            if mx { -1 } else { 1 },
                            if my { -1 } else { 1 }
                        );
                        let _ = write!(
                            cells,
                            r#"<image x="{}" y="0" width="{}" height="{}" preserveAspectRatio="none" href="{uri}" transform="{t}"/>"#,
                            num(0.0),
                            num(tw),
                            num(th),
                        );
                    }
                }
                // The cells above are placed by their own transform; for the unflipped case the
                // transform is the identity.
                let _ = write!(
                    self.defs,
                    r#"<pattern id="{pid}" patternUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}">{cells}</pattern>"#,
                    px(ox),
                    px(oy),
                    num(pw),
                    num(ph),
                );
                let op = if img.alpha.is_finite() && img.alpha < 1.0 {
                    format!(r#" fill-opacity="{}""#, num(img.alpha.clamp(0.0, 1.0)))
                } else {
                    String::new()
                };
                let _ = write!(self.body, r#"<path d="{d}" fill="url(#{pid})"{op}/>"#);
            }
        }
    }

    // ----- pictures --------------------------------------------------------------------------

    fn picture(&mut self, p: &PictureItem, _cx: Ctx) {
        let x = &p.xfrm;
        let bx: Bx = (x.x, x.y, x.w, x.h);
        let opened = self.open_g(x);
        let (paths, trunc) = path::resolve_geometry(&p.geom, x);
        self.truncated |= trunc;
        let d: String = paths
            .iter()
            .map(|(r, _, _)| path_d(&r.segs))
            .collect::<Vec<_>>()
            .join(" ");
        let start = self.body.len();
        match self.embed(&p.image.key) {
            Some((uri, _)) => {
                let fill_rect = match p.image.mode {
                    ImageMode::Stretch { fill_rect } => fill_rect,
                    ImageMode::Tile { .. } => (0.0, 0.0, 0.0, 0.0),
                };
                let c = p.image.crop;
                let plain_rect = matches!(p.geom, Geometry::Rect)
                    && c == (0.0, 0.0, 0.0, 0.0)
                    && fill_rect == (0.0, 0.0, 0.0, 0.0);
                if plain_rect {
                    self.image_el(&uri, bx, c, fill_rect, p.image.alpha, None);
                } else {
                    let clip = self.clip_def(&d);
                    self.image_el(&uri, bx, c, fill_rect, p.image.alpha, Some(&clip));
                }
            }
            None => self.placeholder(&d, bx),
        }
        if let Some(l) = &p.line {
            if let Some((r, _, _)) = paths.first() {
                self.stroke(r, l, bx, screen_axis_aligned(_cx.rot + x.rot_deg));
            }
        }
        let content = self.body.split_off(start);
        self.with_effects(&content, bx, &p.effects, x, _cx.rot);
        if opened {
            self.body.push_str("</g>");
        }
    }

    // ----- lines -----------------------------------------------------------------------------

    /// `axis`: the shape's axes are the slide's (no rotation but a quarter turn), so a path with
    /// only horizontal and vertical segments is axis-aligned on the slide too.
    fn stroke(&mut self, r: &Resolved, l: &Line, bx: Bx, axis: bool) {
        let Some(paint) = self.paint_attr(&l.fill, bx, "stroke", PathFill::Norm) else {
            return;
        };
        let w_px = if l.width.is_finite() && l.width > 0.0 {
            (l.width / EMU_PER_PX).clamp(0.0, 10_000.0)
        } else {
            0.0
        };
        // PowerPoint draws every line at least one device pixel wide (a width of 0 included).
        let w_px = w_px.max(1.0);
        let mut segs = r.segs.clone();
        let closed = segs.iter().any(|s| matches!(s, Seg::Z));
        // Arrow heads (open paths only).
        let mut heads = String::new();
        if !closed {
            let color = line_color(&l.fill);
            let min_w = 2.0 * 96.0 / 72.0;
            let eff = w_px.max(min_w) * EMU_PER_PX;
            if let Some(a) = &l.tail {
                if let Some((p, dir)) = path::end_tangent(&segs) {
                    heads.push_str(&arrow_path(a, p, dir, eff, w_px * EMU_PER_PX, color));
                    if filled_head(a) {
                        path::trim_end(&mut segs, arrow_len(a, eff) * 0.7);
                    }
                }
            }
            if let Some(a) = &l.head {
                if let Some((p, dir)) = path::start_tangent(&segs) {
                    heads.push_str(&arrow_path(
                        a,
                        p,
                        Pt::new(-dir.x, -dir.y),
                        eff,
                        w_px * EMU_PER_PX,
                        color,
                    ));
                    if filled_head(a) {
                        path::trim_start(&mut segs, arrow_len(a, eff) * 0.7);
                    }
                }
            }
        }
        let d = path_d(&segs);
        let cap = match l.cap {
            Cap::Flat => "butt",
            Cap::Round => "round",
            Cap::Square => "square",
        };
        let join = match l.join {
            Join::Round => "round".to_string(),
            Join::Bevel => "bevel".to_string(),
            Join::Miter(m) => format!(
                r#"miter" stroke-miterlimit="{}"#,
                num(if m.is_finite() {
                    m.clamp(1.0, 100.0)
                } else {
                    8.0
                })
            ),
        };
        let dash = dash_array(&l.dash, w_px, l.cap != Cap::Flat);
        let dash_attr = dash
            .map(|d| format!(r#" stroke-dasharray="{d}""#))
            .unwrap_or_default();
        // PowerPoint draws a line of one pixel or less as one crisp device pixel; without
        // anti-aliasing an axis-aligned thin stroke is a single row or column, not two grey ones
        // (a stroke centred on a pixel boundary).
        let crisp = if axis && w_px <= CRISP_MAX_PX && axis_aligned_segs(&segs) {
            r#" shape-rendering="crispEdges""#
        } else {
            ""
        };
        let base = format!(
            r#"fill="none" {paint} stroke-linecap="{cap}" stroke-linejoin="{join}"{dash_attr}{crisp}"#
        );
        match l.compound {
            Compound::Sng => {
                let _ = write!(
                    self.body,
                    r#"<path d="{d}" {base} stroke-width="{}"/>"#,
                    num(w_px)
                );
            }
            c => {
                let mid = self.id("mk");
                let (gap, line) = match c {
                    Compound::Tri => (0.6, Some(0.2)),
                    _ => (1.0 / 3.0, None),
                };
                let mut mask = format!(
                    r##"<path d="{d}" fill="none" stroke="#fff" stroke-width="{}"/><path d="{d}" fill="none" stroke="#000" stroke-width="{}"/>"##,
                    num(w_px),
                    num(w_px * gap)
                );
                if let Some(lw) = line {
                    let _ = write!(
                        mask,
                        r##"<path d="{d}" fill="none" stroke="#fff" stroke-width="{}"/>"##,
                        num(w_px * lw)
                    );
                }
                let (x, y, w, h) = bx;
                let _ = write!(
                    self.defs,
                    r#"<mask id="{mid}" maskUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}">{mask}</mask>"#,
                    px(x - 4.0 * w.abs() - 1e6),
                    px(y - 4.0 * h.abs() - 1e6),
                    px(9.0 * w.abs() + 2e6),
                    px(9.0 * h.abs() + 2e6),
                );
                let _ = write!(
                    self.body,
                    r#"<path d="{d}" {base} stroke-width="{}" mask="url(#{mid})"/>"#,
                    num(w_px)
                );
            }
        }
        self.body.push_str(&heads);
    }

    // ----- text ------------------------------------------------------------------------------

    fn text_body(&mut self, t: &TextBody, text_rect: Option<Rect4>, x: &Xfrm, cx: Ctx) {
        let (l, tp, r, b) = text_rect.unwrap_or((0.0, 0.0, x.w, x.h));
        let f = |v: f64| if v.is_finite() { v } else { 0.0 };
        let (il, it, ir, ib) = (f(t.insets.0), f(t.insets.1), f(t.insets.2), f(t.insets.3));
        let (rx, ry) = (x.x + f(l) + il, x.y + f(tp) + it);
        let (rw, rh) = (
            (f(r) - f(l) - il - ir).max(0.0),
            (f(b) - f(tp) - it - ib).max(0.0),
        );
        let (wpx, hpx) = (rw / EMU_PER_PX, rh / EMU_PER_PX);
        if t.paragraphs.is_empty() {
            return;
        }
        let lay = text::layout(t, wpx, hpx);
        self.truncated |= lay.truncated;
        if lay.lines.is_empty() && lay.bullets.is_empty() {
            return;
        }
        let (ox, oy) = (rx / EMU_PER_PX, ry / EMU_PER_PX);
        let (ccx, ccy) = (ox + wpx / 2.0, oy + hpx / 2.0);
        // Net mirroring of the text: undone, and flipped text is turned over (PowerPoint).
        let ph = cx.flip.0 ^ x.flip_h;
        let pv = cx.flip.1 ^ x.flip_v;
        let sx = if ph ^ pv { -1 } else { 1 };
        let acc = cx.rot
            + if cx.flip.0 != cx.flip.1 { -1.0 } else { 1.0 }
                * if x.rot_deg.is_finite() {
                    x.rot_deg
                } else {
                    0.0
                };
        let mut rot = if t.rot_deg.is_finite() {
            t.rot_deg
        } else {
            0.0
        };
        if t.upright {
            rot -= acc;
        }
        let mut tf = format!("translate({} {})", num(ccx), num(ccy));
        if sx == -1 {
            tf.push_str(" scale(-1 1)");
        }
        if rot.rem_euclid(360.0) != 0.0 {
            let _ = write!(tf, " rotate({})", num(rot));
        }
        let _ = write!(tf, " translate({} {})", num(-ccx), num(-ccy));
        let _ = write!(tf, " translate({} {})", num(ox), num(oy));
        match lay.frame {
            Frame::Normal => {}
            Frame::Rot90 => {
                let _ = write!(tf, " translate({} 0) rotate(90)", num(wpx));
            }
            Frame::Rot270 => {
                let _ = write!(tf, " translate(0 {}) rotate(-90)", num(hpx));
            }
        }
        let _ = write!(self.body, r#"<g transform="{tf}">"#);
        // highlights and decorations come from the fragments
        for line in &lay.lines {
            for fr in &line.frags {
                self.frag_back(fr);
            }
        }
        for bd in &lay.bullets {
            match bd {
                BulletDraw::Text(fr) => {
                    self.text_line(std::slice::from_ref(fr));
                    self.frag_decor(fr);
                }
                BulletDraw::Picture { image, x, y, size } => {
                    if let Some((uri, _)) = self.embed(&image.key) {
                        let _ = write!(
                            self.body,
                            r#"<image x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="xMidYMid meet" href="{uri}"/>"#,
                            num(*x),
                            num(*y),
                            num(*size),
                            num(*size)
                        );
                    }
                }
            }
        }
        for line in &lay.lines {
            if self.over_budget() {
                self.truncated = true;
                break;
            }
            self.text_line(&line.frags);
        }
        for line in &lay.lines {
            for fr in &line.frags {
                self.frag_decor(fr);
            }
        }
        self.body.push_str("</g>");
    }

    fn frag_back(&mut self, f: &Frag) {
        if let Some(h) = f.style.highlight {
            if h.a > 0.0 && !f.rot90 {
                let _ = write!(
                    self.body,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" fill-opacity="{}"/>"#,
                    num(f.x),
                    num(f.y - ASCENT * f.style.size_px),
                    num(f.width),
                    num(LINE_HEIGHT * f.style.size_px),
                    hex(h),
                    num(alpha_of(h)),
                );
            }
        }
    }

    fn frag_decor(&mut self, f: &Frag) {
        if f.rot90 || f.text.trim().is_empty() {
            return;
        }
        let s = f.style.size_px;
        let th = (s / 16.0).max(1.0);
        let col = hex(f.style.color);
        let op = num(alpha_of(f.style.color));
        let bar = |w: &mut Self, y: f64| {
            let _ = write!(
                w.body,
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{col}" fill-opacity="{op}"/>"#,
                num(f.x),
                num(y),
                num(f.width),
                num(th),
            );
        };
        match f.style.underline {
            Underline::None => {}
            Underline::Single => bar(self, f.y + 0.1 * s),
            Underline::Double => {
                bar(self, f.y + 0.08 * s);
                bar(self, f.y + 0.08 * s + 2.0 * th);
            }
        }
        match f.style.strike {
            Strike::None => {}
            Strike::Single => bar(self, f.y - 0.3 * s),
            Strike::Double => {
                bar(self, f.y - 0.3 * s - th);
                bar(self, f.y - 0.3 * s + th);
            }
        }
    }

    fn text_line(&mut self, frags: &[Frag]) {
        if frags.is_empty() {
            return;
        }
        // Rotated fragments need a transform of their own; everything else shares one <text>.
        let mut open = false;
        for f in frags {
            if f.rot90 {
                if open {
                    self.body.push_str("</text>");
                    open = false;
                }
                let _ = write!(
                    self.body,
                    r#"<text xml:space="preserve" transform="rotate(90 {} {})">"#,
                    num(f.x),
                    num(f.y)
                );
                self.tspan(f);
                self.body.push_str("</text>");
                continue;
            }
            if !open {
                self.body.push_str(r#"<text xml:space="preserve">"#);
                open = true;
            }
            self.tspan(f);
        }
        if open {
            self.body.push_str("</text>");
        }
    }

    fn tspan(&mut self, f: &Frag) {
        let s = &f.style;
        let _ = write!(
            self.body,
            r#"<tspan x="{}" y="{}" font-family="{}" font-size="{}""#,
            num(f.x),
            num(f.y),
            esc(&s.family),
            num(s.size_px),
        );
        if s.bold {
            self.body.push_str(r#" font-weight="700""#);
        }
        if s.italic {
            self.body.push_str(r#" font-style="italic""#);
        }
        let _ = write!(self.body, r#" fill="{}""#, hex(s.color));
        if alpha_of(s.color) < 1.0 {
            let _ = write!(self.body, r#" fill-opacity="{}""#, num(alpha_of(s.color)));
        }
        if s.spacing_px != 0.0 {
            let _ = write!(self.body, r#" letter-spacing="{}""#, num(s.spacing_px));
        }
        let _ = write!(self.body, ">{}</tspan>", esc(&f.text));
    }
}

fn line_color(f: &Fill) -> Rgba {
    match f {
        Fill::Solid(c) => *c,
        Fill::Gradient(g) => g.stops.first().map(|s| s.1).unwrap_or(Rgba::BLACK),
        Fill::Pattern { fg, .. } => *fg,
        _ => Rgba::rgb(128, 128, 128),
    }
}

fn dash_array(d: &Dash, w_px: f64, round_caps: bool) -> Option<String> {
    let units: Vec<f64> = match d {
        Dash::Solid => return None,
        Dash::Dot => vec![1.0, 3.0],
        Dash::Dash => vec![4.0, 3.0],
        Dash::LgDash => vec![8.0, 3.0],
        Dash::DashDot => vec![4.0, 3.0, 1.0, 3.0],
        Dash::LgDashDot => vec![8.0, 3.0, 1.0, 3.0],
        Dash::LgDashDotDot => vec![8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
        Dash::SysDash => vec![3.0, 1.0],
        Dash::SysDot => vec![1.0, 1.0],
        Dash::SysDashDot => vec![3.0, 1.0, 1.0, 1.0],
        Dash::SysDashDotDot => vec![3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        Dash::Custom(v) => {
            let u: Vec<f64> = v
                .iter()
                .take(16)
                .flat_map(|(a, b)| [*a, *b])
                .map(|x| {
                    if x.is_finite() {
                        x.clamp(0.0, 1000.0)
                    } else {
                        0.0
                    }
                })
                .collect();
            if u.iter().sum::<f64>() <= 0.0 {
                return None;
            }
            u
        }
    };
    // With round / square caps each dash grows by half a width at both ends: take it out of the
    // dash and put it into the gap so the pattern keeps its period.
    let v: Vec<String> = units
        .chunks(2)
        .flat_map(|c| {
            let (a, b) = (c[0], c.get(1).copied().unwrap_or(0.0));
            let (a, b) = if round_caps {
                ((a - 1.0).max(0.01), b + 1.0)
            } else {
                (a.max(0.01), b.max(0.01))
            };
            [num(a * w_px), num(b * w_px)]
        })
        .collect();
    Some(v.join(" "))
}

fn arrow_dim(s: ArrowSize) -> f64 {
    match s {
        ArrowSize::Sm => 2.0,
        ArrowSize::Med => 3.0,
        ArrowSize::Lg => 5.0,
    }
}

fn arrow_len(a: &Arrow, eff_emu: f64) -> f64 {
    arrow_dim(a.len) * eff_emu
}

fn filled_head(a: &Arrow) -> bool {
    !matches!(a.kind, ArrowKind::Arrow)
}

/// The SVG for one arrow head with its tip at `tip`, pointing along `dir` (a unit vector), sized
/// from the effective line width `eff` (EMU); `line_w` is the real stroke width (EMU).
fn arrow_path(a: &Arrow, tip: Pt, dir: Pt, eff: f64, line_w: f64, color: Rgba) -> String {
    let (len, wid) = (arrow_dim(a.len) * eff, arrow_dim(a.w) * eff);
    // local frame: tip at the origin, head extends towards -u, width along v
    let (ux, uy) = (dir.x, dir.y);
    let (vx, vy) = (-dir.y, dir.x);
    let pt = |u: f64, v: f64| Pt::new(tip.x + ux * u + vx * v, tip.y + uy * u + vy * v);
    let fill = format!(
        r#"fill="{}" fill-opacity="{}""#,
        hex(color),
        num(alpha_of(color))
    );
    match &a.kind {
        ArrowKind::Triangle => {
            let d = path_d(&[
                Seg::M(pt(0.0, 0.0)),
                Seg::L(pt(-len, -wid / 2.0)),
                Seg::L(pt(-len, wid / 2.0)),
                Seg::Z,
            ]);
            format!(r#"<path d="{d}" {fill}/>"#)
        }
        ArrowKind::Stealth => {
            let d = path_d(&[
                Seg::M(pt(0.0, 0.0)),
                Seg::L(pt(-len, -wid / 2.0)),
                Seg::L(pt(-len * 0.7, 0.0)),
                Seg::L(pt(-len, wid / 2.0)),
                Seg::Z,
            ]);
            format!(r#"<path d="{d}" {fill}/>"#)
        }
        ArrowKind::Diamond => {
            let d = path_d(&[
                Seg::M(pt(0.0, 0.0)),
                Seg::L(pt(-len / 2.0, -wid / 2.0)),
                Seg::L(pt(-len, 0.0)),
                Seg::L(pt(-len / 2.0, wid / 2.0)),
                Seg::Z,
            ]);
            format!(r#"<path d="{d}" {fill}/>"#)
        }
        ArrowKind::Oval => {
            let c = pt(-len / 2.0, 0.0);
            let (rx, ry) = (len / 2.0, wid / 2.0);
            let ang = uy.atan2(ux).to_degrees();
            format!(
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" transform="rotate({} {} {})" {fill}/>"#,
                px(c.x),
                px(c.y),
                px(rx),
                px(ry),
                num(ang),
                px(c.x),
                px(c.y)
            )
        }
        ArrowKind::Arrow => {
            let d = path_d(&[
                Seg::M(pt(-len, -wid / 2.0)),
                Seg::L(pt(0.0, 0.0)),
                Seg::L(pt(-len, wid / 2.0)),
            ]);
            format!(
                r#"<path d="{d}" fill="none" stroke="{}" stroke-opacity="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                hex(color),
                num(alpha_of(color)),
                px(line_w.max(EMU_PER_PX)),
            )
        }
        ArrowKind::Custom(paths) => {
            let mut out = String::new();
            for p in paths.iter().take(8) {
                let (pw, ph) = (
                    if p.w > 0.0 { p.w } else { 1.0 },
                    if p.h > 0.0 { p.h } else { 1.0 },
                );
                // path space -> head frame: x in 0..pw maps to -len..0, y in 0..ph centred on 0
                let r = path::resolve_path(
                    &GeomPath {
                        w: pw,
                        h: ph,
                        ..p.clone()
                    },
                    0.0,
                    0.0,
                    len,
                    wid,
                );
                let segs: Vec<Seg> = r
                    .segs
                    .iter()
                    .map(|s| {
                        let m = |q: Pt| pt(q.x - len, q.y - wid / 2.0);
                        match *s {
                            Seg::M(a) => Seg::M(m(a)),
                            Seg::L(a) => Seg::L(m(a)),
                            Seg::Q(a, b) => Seg::Q(m(a), m(b)),
                            Seg::C(a, b, c) => Seg::C(m(a), m(b), m(c)),
                            Seg::Z => Seg::Z,
                        }
                    })
                    .collect();
                let _ = write!(out, r#"<path d="{}" {fill}/>"#, path_d(&segs));
            }
            out
        }
    }
}

/// Re-encodes a BMP or TIFF as PNG with decoding limits; `None` when it cannot be decoded.
fn to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_CONVERT_SIDE);
    limits.max_image_height = Some(MAX_CONVERT_SIDE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}
