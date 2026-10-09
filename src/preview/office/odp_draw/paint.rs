//! Fills, outlines, markers and shadows of a shape from its resolved style ([`View`]).
//!
//! # Approximations (each one documented where it is made)
//!
//! * **Gradients**: `linear` is exact (angle and border); `axial` is a linear gradient mirrored
//!   about the middle (the start colour in the middle, as LibreOffice draws it); `radial` and
//!   `ellipsoid` are the model's radial gradient (a circle through the box corners, the end colour
//!   in the middle); `square` and `rectangular` are the model's rectangular gradient. The angle of
//!   an `ellipsoid`, `square` or `rectangular` gradient turns it ([`sd::GradKind::RadialRotated`],
//!   [`sd::GradKind::RectRotated`]); a `radial` one is a circle, which turning does not change.
//! * **Transparency**: `draw:opacity` is a uniform alpha; a transparency gradient
//!   (`draw:opacity-name`) of the same style, angle, border and centre as the colour gradient is
//!   multiplied into its stops, over a solid fill it becomes a gradient of one colour with
//!   varying alpha, otherwise its mean is the alpha.
//! * **Hatches** are the model's 8 x 8 patterns: the direction is snapped to
//!   horizontal / vertical / the two diagonals / their crossings, the spacing to three densities.
//! * **Bitmaps**: `stretch` and `no-repeat` are stretched; `repeat` is tiled at the stated tile
//!   size (percentages of the filled shape), or at the picture's own size at 96 dpi.
//! * **Dashes** are in multiples of the line width; a zero-length dot is one line width.
//! * **Markers** are exact in size (see [`Sb::arrow`]) but the model has no way to say "the line
//!   ends where the marker's base is": the line is trimmed by the renderer's own rule.
//! * **Shadows** are the model's outer shadow (offset, colour, opacity, blur).

use crate::preview::office::slide_draw as sd;
use sd::{Gradient, PathCmd, Pt, Rgba};

use super::styles::View;
use super::units::{angle_deg, color, number, parse_svg_path, pct, shift, view_box};
use super::*;

/// Most gradient stops kept of a `loext:gradient-stop` list.
pub(super) const MAX_GRAD_STOPS: usize = 32;
/// The width a zero-width line (`svg:stroke-width="0cm"`) is drawn at: 0.6 px, as thin as
/// LibreOffice draws it.
pub(super) const HAIRLINE_EMU: f64 = 0.6 * sd::EMU_PER_PX;
/// The start colour of LibreOffice's default gradient of a shape (and its default hatch colour):
/// its default line colour.
pub(super) const DEFAULT_GRADIENT_START: Rgba = Rgba::rgb(0x34, 0x65, 0xa4);
/// Most dash entries (pairs) of one dash style.
pub(super) const MAX_DASH_PAIRS: usize = 32;

/// A gradient as ODF states it: stops from the start to the end of the gradient.
struct Spec {
    style: String,
    angle: f64,
    border: f64,
    cx: f64,
    cy: f64,
    stops: Vec<(f64, Rgba)>,
}

fn clamp01(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn scale_color(c: Rgba, k: f64) -> Rgba {
    let f = |v: u8| (f64::from(v) * k).round().clamp(0.0, 255.0) as u8;
    Rgba {
        r: f(c.r),
        g: f(c.g),
        b: f(c.b),
        a: c.a,
    }
}

/// The alpha of a piecewise-linear stop list at `p`.
fn alpha_at(stops: &[(f64, Rgba)], p: f64) -> f64 {
    let Some(first) = stops.first() else {
        return 1.0;
    };
    if p <= first.0 {
        return first.1.a;
    }
    for w in stops.windows(2) {
        if p <= w[1].0 {
            let span = (w[1].0 - w[0].0).max(1e-12);
            let t = (p - w[0].0) / span;
            return w[0].1.a + (w[1].1.a - w[0].1.a) * t;
        }
    }
    stops.last().map_or(1.0, |s| s.1.a)
}

/// The stops of a spec in the model's order and positions for the style of the gradient.
fn model_stops(style: &str, border: f64, stops: &[(f64, Rgba)]) -> Vec<(f64, Rgba)> {
    let b = clamp01(border).min(0.99);
    let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
        return Vec::new();
    };
    let mut out: Vec<(f64, Rgba)> = Vec::new();
    match style {
        "axial" => {
            // The start colour in the middle, the end colour at both edges.
            let half = (1.0 - b) * 0.5;
            if b > 0.0 {
                out.push((0.0, last.1));
            }
            for (p, c) in stops.iter().rev() {
                out.push((0.5 - p * half, *c));
            }
            for (p, c) in stops.iter().skip(1) {
                out.push((0.5 + p * half, *c));
            }
            if b > 0.0 {
                out.push((1.0, last.1));
            }
        }
        "radial" | "ellipsoid" | "square" | "rectangular" => {
            // The end colour in the middle, the start colour outside; the border is a solid ring of
            // the start colour.
            for (p, c) in stops.iter().rev() {
                out.push(((1.0 - p) * (1.0 - b), *c));
            }
            if b > 0.0 {
                out.push((1.0, first.1));
            }
        }
        _ => {
            if b > 0.0 {
                out.push((0.0, first.1));
            }
            for (p, c) in stops {
                out.push((b + p * (1.0 - b), *c));
            }
        }
    }
    for s in &mut out {
        s.0 = clamp01(s.0);
    }
    out.truncate(MAX_GRAD_STOPS * 2 + 2);
    out
}

fn to_gradient(s: &Spec) -> Option<Gradient> {
    let stops = model_stops(&s.style, s.border, &s.stops);
    if stops.len() < 2 {
        return None;
    }
    // An ellipse or a rectangle gradient turned by its angle (counter-clockwise in ODF; the
    // model's is clockwise). A circle is the same turned.
    let turn = (-s.angle).rem_euclid(360.0);
    let turned = turn > 1e-9 && turn < 360.0 - 1e-9;
    let (kind, rect) = match s.style.as_str() {
        "ellipsoid" if turned => (
            sd::GradKind::RadialRotated { angle_deg: turn },
            (s.cx, s.cy, 1.0 - s.cx, 1.0 - s.cy),
        ),
        "square" | "rectangular" if turned => (
            sd::GradKind::RectRotated { angle_deg: turn },
            (s.cx, s.cy, 1.0 - s.cx, 1.0 - s.cy),
        ),
        "radial" | "ellipsoid" => (sd::GradKind::Radial, (s.cx, s.cy, 1.0 - s.cx, 1.0 - s.cy)),
        "square" | "rectangular" => (sd::GradKind::Rect, (s.cx, s.cy, 1.0 - s.cx, 1.0 - s.cy)),
        _ => (
            // LibreOffice measures the angle counter-clockwise from "start at the top"; the
            // model's is clockwise from "to the right".
            sd::GradKind::Linear {
                angle_deg: (90.0 - s.angle).rem_euclid(360.0),
                scaled: false,
            },
            (0.5, 0.5, 0.5, 0.5),
        ),
    };
    Some(Gradient {
        kind,
        stops,
        fill_to_rect: rect,
        rot_with_shape: true,
    })
}

/// A gradient description node (`draw:gradient`, or `draw:opacity` with `opacity`) as a spec.
fn spec_of(n: &Node, opacity: bool) -> Option<Spec> {
    let style = n.attr("style").unwrap_or("linear").trim().to_string();
    let angle = n.attr("angle").and_then(angle_deg).unwrap_or(0.0);
    let border = n
        .attr("border")
        .and_then(pct)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let cx = n.attr("cx").and_then(pct).unwrap_or(0.5).clamp(0.0, 1.0);
    let cy = n.attr("cy").and_then(pct).unwrap_or(0.5).clamp(0.0, 1.0);
    let mut stops: Vec<(f64, Rgba)> = Vec::new();
    if opacity {
        let a = |k: &str, d: f64| n.attr(k).and_then(pct).unwrap_or(d).clamp(0.0, 1.0);
        stops.push((0.0, Rgba::WHITE.with_alpha(a("start", 1.0))));
        stops.push((1.0, Rgba::WHITE.with_alpha(a("end", 1.0))));
    } else {
        for s in n.nodes().filter(|k| k.name == "gradient-stop") {
            let (Some(off), Some(c)) = (
                s.attr("offset")
                    .and_then(|o| number(o).or_else(|| pct(o)))
                    .map(clamp01),
                s.attr("color-value").and_then(color),
            ) else {
                continue;
            };
            let alpha = s
                .attr("transparency")
                .and_then(pct)
                .map_or(1.0, |t| 1.0 - t.clamp(0.0, 1.0));
            if stops.len() < MAX_GRAD_STOPS {
                stops.push((off, c.with_alpha(alpha)));
            }
        }
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        if stops.len() < 2 {
            let k = |name: &str| n.attr(name).and_then(pct).unwrap_or(1.0).max(0.0);
            let s = n.attr("start-color").and_then(color).unwrap_or(Rgba::BLACK);
            let e = n.attr("end-color").and_then(color).unwrap_or(Rgba::WHITE);
            stops = vec![
                (0.0, scale_color(s, k("start-intensity"))),
                (1.0, scale_color(e, k("end-intensity"))),
            ];
        }
    }
    Some(Spec {
        style,
        angle,
        border,
        cx,
        cy,
        stops,
    })
}

/// The 8 x 8 pattern preset that stands for a hatch.
fn hatch_preset(style: &str, rotation_deg: f64, distance_px: f64) -> &'static str {
    // 0 = horizontal, 1 = vertical, 2 = falls to the right, 3 = rises to the right (LibreOffice's
    // angles are counter-clockwise, so 45 degrees rises).
    let dir = |deg: f64| -> usize {
        let a = deg.rem_euclid(180.0);
        if !(22.5..157.5).contains(&a) {
            0
        } else if a < 67.5 {
            3
        } else if a < 112.5 {
            1
        } else {
            2
        }
    };
    let density = if distance_px <= 3.0 {
        0
    } else if distance_px <= 6.0 {
        1
    } else {
        2
    };
    let d = dir(rotation_deg);
    match (style, d, density) {
        ("double" | "triple", 0 | 1, _) => "cross",
        ("double" | "triple", _, 0) => "diagCross",
        ("double" | "triple", _, _) => "diagCross",
        (_, 0, 0) => "narHorz",
        (_, 0, 1) => "ltHorz",
        (_, 0, _) => "horz",
        (_, 1, 0) => "narVert",
        (_, 1, 1) => "ltVert",
        (_, 1, _) => "vert",
        (_, 2, 0) => "dkDnDiag",
        (_, 2, 1) => "ltDnDiag",
        (_, 2, _) => "dnDiag",
        (_, _, 0) => "dkUpDiag",
        (_, _, 1) => "ltUpDiag",
        _ => "upDiag",
    }
}

/// The renderer's effective line width for sizing arrow heads (EMU): at least 2 pt, a hairline
/// counting as one pixel.
fn effective_width(width_emu: f64) -> f64 {
    let px = width_emu / sd::EMU_PER_PX;
    let px = if px < 1.0 / 16.0 { 1.0 } else { px };
    px.max(2.0 * 96.0 / 72.0) * sd::EMU_PER_PX
}

impl Sb<'_> {
    /// The fill the style states: `None` when it states nothing, `Some(Fill::None)` for an
    /// explicit `draw:fill="none"`. `w`/`h` is the filled box (EMU), for percentage tile sizes.
    pub(super) fn fill(&mut self, v: &View, w: f64, h: f64) -> Option<sd::Fill> {
        self.fill_of(v, w, h, false)
    }

    /// [`Self::fill`] for a drawing page's background (a gradient that is not defined is another
    /// default there).
    pub(super) fn page_fill(&mut self, v: &View, w: f64, h: f64) -> Option<sd::Fill> {
        self.fill_of(v, w, h, true)
    }

    fn fill_of(&mut self, v: &View, w: f64, h: f64, page: bool) -> Option<sd::Fill> {
        let kind = v.g("fill")?.trim();
        // An opacity outside 0..100 % is a broken file (LibreOffice writes `-4900%` for some
        // imported cells): it is ignored, the fill is opaque.
        let opacity = v
            .g("opacity")
            .and_then(pct)
            .filter(|a| (0.0..=1.0).contains(a));
        let named_opacity = v
            .g("opacity-name")
            .and_then(|n| self.book.opacities.get(n))
            .and_then(|n| spec_of(n, true));
        let base = v.g("fill-color").and_then(color).unwrap_or(Rgba::BLACK);
        match kind {
            "none" => Some(sd::Fill::None),
            "solid" => {
                let c = base.with_alpha(opacity.unwrap_or(1.0));
                Some(match named_opacity {
                    Some(o) => self.solid_with_gradient_alpha(c, &o),
                    None => sd::Fill::Solid(c),
                })
            }
            "gradient" => {
                let node = v
                    .g("fill-gradient-name")
                    .and_then(|n| self.book.gradients.get(n));
                let Some(mut spec) = node.and_then(|n| spec_of(n, false)) else {
                    // A gradient that is not defined (an empty or unknown name): LibreOffice draws
                    // its own default and ignores the fill colour: linear, angle 0 (from the top
                    // to the bottom), from black to white on a page's background and from its
                    // default line blue `#3465a4` to white on a shape (measured on its renderings
                    // of both).
                    let a = opacity.unwrap_or(1.0);
                    let start = if page {
                        Rgba::BLACK
                    } else {
                        DEFAULT_GRADIENT_START
                    };
                    return Some(sd::Fill::Gradient(Gradient::linear(
                        90.0,
                        vec![(0.0, start.with_alpha(a)), (1.0, Rgba::WHITE.with_alpha(a))],
                    )));
                };
                if let Some(o) = &named_opacity {
                    let same = o.style == spec.style
                        && (o.angle - spec.angle).abs() < 1e-6
                        && (o.border - spec.border).abs() < 1e-6
                        && (o.cx - spec.cx).abs() < 1e-6
                        && (o.cy - spec.cy).abs() < 1e-6;
                    if same {
                        for s in &mut spec.stops {
                            s.1.a *= alpha_at(&o.stops, s.0);
                        }
                    } else {
                        let mean = (o.stops.first().map_or(1.0, |s| s.1.a)
                            + o.stops.last().map_or(1.0, |s| s.1.a))
                            / 2.0;
                        for s in &mut spec.stops {
                            s.1.a *= mean;
                        }
                    }
                }
                if let Some(k) = opacity {
                    for s in &mut spec.stops {
                        s.1.a *= k;
                    }
                }
                match to_gradient(&spec) {
                    Some(g) => Some(sd::Fill::Gradient(g)),
                    None => Some(sd::Fill::Solid(base)),
                }
            }
            "hatch" => {
                let node = v
                    .g("fill-hatch-name")
                    .and_then(|n| self.book.hatches.get(n));
                let (style, rot, dist, fg) = match node {
                    Some(n) => (
                        n.attr("style").unwrap_or("single").trim().to_string(),
                        n.attr("rotation").and_then(angle_deg).unwrap_or(0.0),
                        n.attr("distance")
                            .and_then(emu)
                            .map_or(36_000.0, |d| d.abs())
                            / sd::EMU_PER_PX,
                        n.attr("color").and_then(color).unwrap_or(Rgba::BLACK),
                    ),
                    // (A hatch that is not defined: thin pale-blue horizontal lines, close
                    // together, as LibreOffice draws them.)
                    None => {
                        // (With `draw:fill-hatch-solid` the background alone is drawn.)
                        if v.g("fill-hatch-solid").is_some_and(|s| s.trim() == "true") {
                            return Some(sd::Fill::Solid(base.with_alpha(opacity.unwrap_or(1.0))));
                        }
                        ("single".to_string(), 0.0, 4.0, DEFAULT_GRADIENT_START)
                    }
                };
                let solid = v.g("fill-hatch-solid").is_some_and(|s| s.trim() == "true");
                let a = opacity.unwrap_or(1.0);
                Some(sd::Fill::Pattern {
                    preset: hatch_preset(&style, rot, dist).to_string(),
                    fg: fg.with_alpha(a),
                    bg: if solid {
                        base.with_alpha(a)
                    } else {
                        Rgba::TRANSPARENT
                    },
                })
            }
            // A bitmap that is not defined (an empty or unknown name) is no fill at all on a
            // shape in LibreOffice (not the fill colour); one that is defined but whose picture
            // cannot be shown keeps the fill colour.
            "bitmap" => {
                let defined = v
                    .g("fill-image-name")
                    .is_some_and(|n| self.book.fill_images.contains_key(n));
                // (A page's background keeps the fill colour, as the decks of LibreOffice's
                // own test suite show it.)
                if !defined {
                    return Some(if page {
                        sd::Fill::Solid(base.with_alpha(opacity.unwrap_or(1.0)))
                    } else {
                        sd::Fill::None
                    });
                }
                self.bitmap_fill(v, w, h, opacity.unwrap_or(1.0))
                    .or(Some(sd::Fill::Solid(
                        base.with_alpha(opacity.unwrap_or(1.0)),
                    )))
            }
            _ => None,
        }
    }

    fn solid_with_gradient_alpha(&self, c: Rgba, o: &Spec) -> sd::Fill {
        let uniform = o
            .stops
            .iter()
            .all(|s| (s.1.a - o.stops[0].1.a).abs() < 1e-9);
        if uniform {
            let a = o.stops.first().map_or(1.0, |s| s.1.a);
            return sd::Fill::Solid(c.with_alpha(c.a * a));
        }
        let spec = Spec {
            style: o.style.clone(),
            angle: o.angle,
            border: o.border,
            cx: o.cx,
            cy: o.cy,
            stops: o
                .stops
                .iter()
                .map(|(p, s)| (*p, c.with_alpha(c.a * s.a)))
                .collect(),
        };
        match to_gradient(&spec) {
            Some(g) => sd::Fill::Gradient(g),
            None => sd::Fill::Solid(c),
        }
    }

    fn bitmap_fill(&mut self, v: &View, w: f64, h: f64, alpha: f64) -> Option<sd::Fill> {
        let node = v
            .g("fill-image-name")
            .and_then(|n| self.book.fill_images.get(n))?;
        let href = node.attr("href")?;
        let key = self.image_for_href(href)?;
        let repeat = v.g("repeat").map_or("repeat", str::trim);
        let mut img = sd::ImageFill::stretch(key.clone());
        img.alpha = alpha;
        if repeat != "repeat" {
            return Some(sd::Fill::Image(img));
        }
        let dims = self.image_dims(&key);
        let native = |px: u32| f64::from(px) * sd::EMU_PER_PX;
        let size = |attr: &str, whole: f64, nat: Option<f64>| -> Option<f64> {
            let a = v.g(attr)?.trim();
            let _ = nat;
            if let Some(p) = pct(a) {
                // A negative percentage is LibreOffice's way of writing "relative to the shape".
                return (p != 0.0).then(|| p.abs() * whole);
            }
            // A size of zero says "the picture's own size".
            emu(a).filter(|e| *e > 0.0)
        };
        let tw = size("fill-image-width", w, dims.map(|d| native(d.0)));
        let th = size("fill-image-height", h, dims.map(|d| native(d.1)));
        let sx = match (tw, dims) {
            (Some(t), Some(d)) if d.0 > 0 => t / native(d.0),
            _ => 1.0,
        };
        let sy = match (th, dims) {
            (Some(t), Some(d)) if d.1 > 0 => t / native(d.1),
            _ => 1.0,
        };
        let align = match v.g("fill-image-ref-point").map_or("", str::trim) {
            "top" => sd::RectAlign::Top,
            "top-right" => sd::RectAlign::TopRight,
            "left" => sd::RectAlign::Left,
            "center" => sd::RectAlign::Center,
            "right" => sd::RectAlign::Right,
            "bottom-left" => sd::RectAlign::BottomLeft,
            "bottom" => sd::RectAlign::Bottom,
            "bottom-right" => sd::RectAlign::BottomRight,
            _ => sd::RectAlign::TopLeft,
        };
        let tile_w = tw.or(dims.map(|d| native(d.0))).unwrap_or(0.0);
        let tile_h = th.or(dims.map(|d| native(d.1))).unwrap_or(0.0);
        let off = |attr: &str, whole: f64| v.g(attr).and_then(pct).map_or(0.0, |p| p * whole);
        img.mode = sd::ImageMode::Tile {
            sx: sx.clamp(1e-4, 1e4),
            sy: sy.clamp(1e-4, 1e4),
            tx: off("fill-image-ref-point-x", tile_w),
            ty: off("fill-image-ref-point-y", tile_h),
            align,
            flip: sd::TileFlip::None,
        };
        Some(sd::Fill::Image(img))
    }

    /// A page background that is a picture placed once (`style:repeat="no-repeat"`) at its stated
    /// size (else its own) on the page, by the reference point. A shape's fill has no such mode
    /// (it is stretched), so the page puts it on the page as a picture item.
    pub(super) fn single_background_picture(&mut self, v: &View) -> Option<sd::PictureItem> {
        if v.g("fill").map(str::trim) != Some("bitmap")
            || v.g("repeat").map(str::trim) != Some("no-repeat")
        {
            return None;
        }
        let node = v
            .g("fill-image-name")
            .and_then(|n| self.book.fill_images.get(n))?;
        let key = self.image_for_href(node.attr("href")?)?;
        let native = self.native_emu(&key);
        let (pw, ph) = self.size;
        let len = |attr: &str, whole: f64| -> Option<f64> {
            let a = v.g(attr)?.trim();
            match pct(a) {
                Some(p) => (p != 0.0).then(|| p.abs() * whole),
                None => emu(a).filter(|e| *e > 0.0),
            }
        };
        let w = len("fill-image-width", pw).or(native.map(|n| n.0))?;
        let h = len("fill-image-height", ph).or(native.map(|n| n.1))?;
        let (fx, fy) = match v.g("fill-image-ref-point").map_or("top-left", str::trim) {
            "top" => (0.5, 0.0),
            "top-right" => (1.0, 0.0),
            "left" => (0.0, 0.5),
            "center" => (0.5, 0.5),
            "right" => (1.0, 0.5),
            "bottom-left" => (0.0, 1.0),
            "bottom" => (0.5, 1.0),
            "bottom-right" => (1.0, 1.0),
            _ => (0.0, 0.0),
        };
        let xf = sd::Xfrm::rect((pw - w) * fx, (ph - h) * fy, w, h);
        Some(sd::PictureItem::new(xf, key))
    }

    /// The outline the style states (`None`: no line).
    pub(super) fn line(&mut self, v: &View) -> Option<sd::Line> {
        match v.g("stroke").map_or("none", str::trim) {
            "none" => return None,
            "solid" | "dash" => {}
            _ => return None,
        }
        let width = v
            .g("stroke-width")
            .and_then(emu)
            .map_or(0.0, |w| w.abs().min(1.0e8));
        let alpha = v.g("stroke-opacity").and_then(pct).map_or(1.0, clamp01);
        let c = v
            .g("stroke-color")
            .and_then(color)
            .unwrap_or(Rgba::BLACK)
            .with_alpha(alpha);
        // A hairline (width 0) is a thinner line than the renderer's one-pixel hairline in
        // LibreOffice: it is drawn at [`HAIRLINE_EMU`].
        let width = if width < 1.0 { HAIRLINE_EMU } else { width };
        let mut line = sd::Line::solid(width, c);
        if v.g("stroke").map(str::trim) == Some("dash") {
            line.dash = match v.g("stroke-dash").and_then(|n| self.book.dashes.get(n)) {
                Some(n) => dash_of(n, width),
                None => sd::Dash::Dash,
            };
        }
        line.cap = match v.g("stroke-linecap").map_or("", str::trim) {
            "round" => sd::Cap::Round,
            "square" => sd::Cap::Square,
            _ => sd::Cap::Flat,
        };
        line.join = match v.g("stroke-linejoin").map_or("round", str::trim) {
            "miter" => sd::Join::Miter(4.0),
            "bevel" => sd::Join::Bevel,
            _ => sd::Join::Round,
        };
        line.head = self.arrow(v, "start", width);
        line.tail = self.arrow(v, "end", width);
        Some(line)
    }

    /// The marker of one end of a line (`side` is `start` or `end`).
    ///
    /// A `draw:marker` is a path in its own `svg:viewBox` whose top-middle is the end of the line
    /// and which points "up"; `draw:marker-*-width` is the size across the line, and
    /// `draw:marker-*-center` puts the marker's middle (instead of its tip) on the end. The model's
    /// custom head has the tip at the right-middle and is sized in classes (2, 3 or 5 times the
    /// renderer's effective line width); to get the exact size the path is written in a space
    /// larger than the class box, which the renderer scales down (the tip stays at the right
    /// edge). The class is the largest whose trim of the line (0.7 of the head's length) does not
    /// exceed the marker's own length.
    fn arrow(&mut self, v: &View, side: &str, line_width: f64) -> Option<sd::Arrow> {
        let name = v.g(&format!("marker-{side}"))?;
        let node = self.book.markers.get(name)?;
        let (vx, vy, vw, vh) = view_box(node.attr("viewBox")?)?;
        let (mut cmds, _) = parse_svg_path(node.attr("d")?)?;
        shift(&mut cmds, vx, vy);
        let width = v
            .g(&format!("marker-{side}-width"))
            .and_then(emu)
            .filter(|w| *w > 0.0)
            .unwrap_or(72_000.0)
            .min(1.0e8);
        let center = v
            .g(&format!("marker-{side}-center"))
            .is_some_and(|c| c.trim() == "true");
        let unit = width / vw; // EMU per marker unit
        let length = vh * unit;
        let eff = effective_width(line_width);
        let class = [
            (sd::ArrowSize::Lg, 5.0),
            (sd::ArrowSize::Med, 3.0),
            (sd::ArrowSize::Sm, 2.0),
        ]
        .into_iter()
        .find(|(_, d)| 0.7 * d * eff <= length)
        .unwrap_or((sd::ArrowSize::Sm, 2.0));
        let wclass = (sd::ArrowSize::Sm, 2.0);
        let pw = class.1 * eff / unit;
        let ph = wclass.1 * eff / unit;
        let lead = if center { vh / 2.0 } else { 0.0 };
        let map = |p: Pt| Pt::new(pw + lead - p.y, ph / 2.0 + (p.x - vw / 2.0));
        let mapped: Vec<PathCmd> = cmds
            .iter()
            .map(|c| match *c {
                PathCmd::MoveTo(a) => PathCmd::MoveTo(map(a)),
                PathCmd::LineTo(a) => PathCmd::LineTo(map(a)),
                PathCmd::QuadTo(a, b) => PathCmd::QuadTo(map(a), map(b)),
                PathCmd::CubicTo(a, b, c) => PathCmd::CubicTo(map(a), map(b), map(c)),
                other => other,
            })
            .collect();
        Some(sd::Arrow {
            kind: sd::ArrowKind::Custom(vec![sd::GeomPath {
                w: pw,
                h: ph,
                fill_mode: sd::PathFill::Norm,
                stroke: false,
                cmds: mapped,
            }]),
            w: wclass.0,
            len: class.0,
        })
    }

    /// The drop shadow the style states.
    pub(super) fn effects(&self, v: &View) -> sd::Effects {
        let mut e = sd::Effects::default();
        if v.g("shadow").map(str::trim) != Some("visible") {
            return e;
        }
        let off = |k: &str| v.g(k).and_then(emu).map_or(0.0, |x| x.clamp(-1.0e8, 1.0e8));
        let (dx, dy) = (off("shadow-offset-x"), off("shadow-offset-y"));
        let alpha = v.g("shadow-opacity").and_then(pct).map_or(1.0, clamp01);
        let c = v
            .g("shadow-color")
            .and_then(color)
            .unwrap_or(Rgba::rgb(0x80, 0x80, 0x80))
            .with_alpha(alpha);
        let blur = v
            .g("shadow-blur")
            .and_then(emu)
            .map_or(0.0, |b| b.clamp(0.0, 1.0e8));
        e.outer_shadow = Some(sd::Shadow {
            blur_rad: blur,
            dist: dx.hypot(dy),
            dir_deg: dy.atan2(dx).to_degrees().rem_euclid(360.0),
            color: c,
            sx: 1.0,
            sy: 1.0,
            rot_with_shape: false,
        });
        e
    }
}

/// The dash of a `draw:stroke-dash` for a line of `width` EMU.
fn dash_of(n: &Node, width: f64) -> sd::Dash {
    let w = if width < 1.0 { sd::EMU_PER_PX } else { width };
    // A length is a percentage of the line width or an absolute length; 0 or none is one line width.
    let len = |k: &str| -> f64 {
        match n.attr(k).map(str::trim) {
            Some(s) => {
                if let Some(p) = pct(s) {
                    p
                } else if let Some(e) = emu(s) {
                    e / w
                } else {
                    0.0
                }
            }
            None => 0.0,
        }
    };
    let count = |k: &str| {
        n.attr(k)
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(0)
            .min(MAX_DASH_PAIRS)
    };
    let d1 = count("dots1").max(if n.attr("dots1").is_none() { 1 } else { 0 });
    let d2 = count("dots2");
    let dist = len("distance").max(0.05);
    let l1 = len("dots1-length");
    let l2 = len("dots2-length");
    let norm = |l: f64| if l <= 0.0 { 1.0 } else { l };
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    for _ in 0..d1 {
        pairs.push((norm(l1), dist));
    }
    for _ in 0..d2 {
        pairs.push((norm(l2), dist));
    }
    pairs.truncate(MAX_DASH_PAIRS);
    if pairs.is_empty() {
        sd::Dash::Dash
    } else {
        sd::Dash::Custom(pairs)
    }
}
