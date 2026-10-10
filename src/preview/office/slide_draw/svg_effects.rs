//! Shape effects of the SVG writer: outer shadow, glow, reflection, soft edge and inner shadow.
//!
//! The painted shape (fills and outlines, without its text) is written once into `<defs>` as a
//! `<g id>` and every layer is a `<use>` of it: the layers under the shape (reflection, outer
//! shadow, glow) each have a filter that keeps only their own part, and the shape itself is drawn
//! with the soft-edge and inner-shadow filters when it has them. Everything is in px of the slide.
//!
//! # Approximations (DrawingML gives no pixel recipe; these are close to what PowerPoint shows)
//!
//! * `blurRad` is the radius of the blur; the Gaussian's standard deviation is half of it.
//! * Shadow and reflection scale (`sx`, `sy`) are applied about the bottom centre of the shape
//!   (`algn="b"`, the one perspective shadows and reflections use; the model carries no `algn`).
//!   A shadow with `rotWithShape="0"` keeps its direction on the slide when the shape is rotated.
//! * The glow's colour (with its alpha) holds out to 0.4 of `rad` beyond the outline and falls
//!   smoothly to nothing at 1.5 `rad`: that is the extent LibreOffice draws (it dilates by half
//!   the radius and blurs by half; measured on its rendering of an 11 pt glow, which is gone
//!   after about 25 px of 14.7). LibreOffice's glow is also stronger (60 % where the alpha is
//!   40 %); the alpha is used as written. A dilated copy would
//!   be exact, but a morphology filter costs the square of its radius (the render budget
//!   refuses a glow of any size worth drawing), so the shape's alpha is blurred (deviation: half
//!   the radius) and mapped through `GLOW_FALLOFF`, which turns the Gaussian's edge
//!   profile into that plateau-and-fall profile.
//! * The soft edge is the shape's alpha blurred (deviation: two thirds of the radius) and mapped
//!   through `2 a - 1`: transparent at the outline, opaque about one radius in.
//! * The reflection is the shape mirrored about its bottom edge (the transform of `sx`/`sy`, then
//!   moved by `dist` along `dir`), faded from `start_alpha` at `start_pos` to `end_alpha` at
//!   `end_pos` of its height.

use super::*;

/// Alpha of a glow by the blurred alpha `a = i / 100` of the shape (the input is first scaled by
/// 5 and clamped, so 21 values cover `a` in 0..0.2 and everything above is full strength).
/// The blur's deviation is half the glow radius, so a straight edge has `a = Phi(-z)` at
/// `z` deviations outside it; the value is 1 up to `z = 0.8` (0.4 of the radius) and falls
/// as a smoothstep to 0 at `z = 3` (1.5 radii): `out = smoothstep(1 - (z - 0.8) / 2.2)`.
const GLOW_FALLOFF: [f64; 21] = [
    0.0, 0.224, 0.396, 0.513, 0.601, 0.671, 0.728, 0.775, 0.815, 0.848, 0.877, 0.902, 0.923, 0.94,
    0.955, 0.968, 0.978, 0.986, 0.992, 0.996, 0.999,
];

// Work per device pixel of each filter written here, in the drawing process's own units
// (`svg_guard`: a Gaussian blur is 12, any other primitive 3): the primitives of the filter added up.
/// Outer shadow: blur, flood, composite.
const UNITS_SHADOW: f64 = 12.0 + 3.0 + 3.0;
/// Glow: blur, two transfers, flood, composite.
const UNITS_GLOW: f64 = 12.0 + 3.0 + 3.0 + 3.0 + 3.0;
/// Soft edge: blur, transfer, composite.
const UNITS_SOFT_EDGE: f64 = 12.0 + 3.0 + 3.0;
/// Inner shadow: flood, composite, blur, offset, composite, merge.
const UNITS_INNER_SHADOW: f64 = 3.0 + 3.0 + 12.0 + 3.0 + 3.0 + 3.0;
/// Reflection blur.
const UNITS_BLUR: f64 = 12.0;

/// Largest padding of a filter region around the shape's box, px.
const MAX_REGION_PAD: f64 = 600.0;
/// Largest filter radius or blur deviation written, px (the filters are costly beyond this).
const MAX_FX_PX: f64 = 200.0;

fn px_clamped(emu: f64) -> f64 {
    if emu.is_finite() {
        (emu / EMU_PER_PX).clamp(-MAX_FX_PX * 10.0, MAX_FX_PX * 10.0)
    } else {
        0.0
    }
}

fn fnum(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

impl W<'_> {
    /// Whether any effect of `fx` is drawn.
    pub(super) fn has_effects(fx: &Effects) -> bool {
        fx.outer_shadow.is_some_and(|s| s.color.a > 0.0)
            || fx.inner_shadow.is_some_and(|s| s.color.a > 0.0)
            || fx.glow.is_some_and(|g| g.color.a > 0.0 && g.rad > 0.0)
            || fx.soft_edge.is_some_and(|r| r > 0.0)
            || fx.reflection.is_some()
    }

    /// How many times a shape with effects `fx` paints what it contains: once for itself and
    /// once more for each layer drawn under it (shadow, glow, reflection). Pictures inside it are
    /// decoded that many times by the drawing process.
    pub(super) fn fx_layers(fx: &Effects) -> u64 {
        if !Self::has_effects(fx) {
            return 1;
        }
        1 + u64::from(fx.outer_shadow.is_some_and(|s| s.color.a > 0.0))
            + u64::from(fx.glow.is_some_and(|g| g.color.a > 0.0 && g.rad > 0.0))
            + u64::from(fx.reflection.is_some())
    }

    /// Writes the painted shape `content` (markup in slide coordinates) with its effects. `bx` is
    /// the shape's box, `x` its transform (for the shadow direction), `acc_rot` the rotation of
    /// its ancestors.
    pub(super) fn with_effects(
        &mut self,
        content: &str,
        bx: Bx,
        fx: &Effects,
        x: &Xfrm,
        acc_rot: f64,
    ) {
        if !Self::has_effects(fx) || self.plain {
            self.flush_cur(1.0);
            self.body.push_str(content);
            return;
        }
        let (body_at, defs_at) = (self.body.len(), self.defs.len());
        let gid = self.id("fx");
        let _ = write!(self.defs, r#"<g id="{gid}">{content}</g>"#);
        let (bxx, bxy, bw, bh) = bx;
        let (l, t, w, h) = (
            bxx / EMU_PER_PX,
            bxy / EMU_PER_PX,
            bw / EMU_PER_PX,
            bh / EMU_PER_PX,
        );
        let (ax, ay) = (l + w / 2.0, t + h); // bottom centre
                                             // The work of a filter the way the drawing process counts it: its region (never more than
                                             // 25 canvases) in device px, times the work per pixel of its primitives (`units`).
        let work = Cell::new(0.0f64);
        let k2 = self.raster_scale * self.raster_scale;
        let layer_max = 25.0 * self.slide_px.0 * self.slide_px.1;
        let region = |pad: f64, units: f64| -> String {
            let pad = pad.clamp(0.0, MAX_REGION_PAD) + 2.0;
            work.set(work.get() + ((w + 2.0 * pad) * (h + 2.0 * pad)).min(layer_max) * k2 * units);
            format!(
                r#"filterUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" color-interpolation-filters="sRGB""#,
                num(l - pad),
                num(t - pad),
                num(w + 2.0 * pad),
                num(h + 2.0 * pad)
            )
        };

        // 1. reflection (under everything)
        if let Some(r) = fx.reflection {
            self.reflection_layer(&gid, &r, (l, t, w, h), (ax, ay), &region);
        }
        // 2. outer shadow
        if let Some(sh) = fx.outer_shadow.filter(|s| s.color.a > 0.0) {
            let blur = px_clamped(sh.blur_rad).max(0.0);
            let sigma = (blur / 2.0).min(MAX_FX_PX);
            let dist = px_clamped(sh.dist);
            let dir = fnum(sh.dir_deg).to_radians();
            let (mut dx, mut dy) = (dist * dir.cos(), dist * dir.sin());
            if !sh.rot_with_shape {
                // The direction is on the slide: undo the shape's own turn and mirroring.
                let rot = (acc_rot + fnum(x.rot_deg)).to_radians();
                let (c, s) = (rot.cos(), rot.sin());
                let (lx, ly) = (dx * c + dy * s, -dx * s + dy * c);
                dx = if x.flip_h { -lx } else { lx };
                dy = if x.flip_v { -ly } else { ly };
            }
            let (sx, sy) = (
                if sh.sx.is_finite() {
                    sh.sx.clamp(-20.0, 20.0)
                } else {
                    1.0
                },
                if sh.sy.is_finite() {
                    sh.sy.clamp(-20.0, 20.0)
                } else {
                    1.0
                },
            );
            let fid = self.id("sh");
            let _ = write!(
                self.defs,
                r#"<filter id="{fid}" {}><feGaussianBlur in="SourceAlpha" stdDeviation="{}" result="b"/><feFlood flood-color="{}" flood-opacity="{}"/><feComposite in2="b" operator="in"/></filter>"#,
                region(blur * 3.0 + dist.abs() + h * sy.abs(), UNITS_SHADOW),
                num(sigma),
                hex(sh.color),
                num(alpha_of(sh.color)),
            );
            // (Blur and tint in the filter, offset and scale by the transform.)
            let _ = write!(
                self.body,
                r##"<use href="#{gid}" transform="translate({} {}) scale({} {}) translate({} {})" filter="url(#{fid})"/>"##,
                num(ax + dx),
                num(ay + dy),
                num(sx),
                num(sy),
                num(-ax),
                num(-ay),
            );
        }
        // 3. glow
        if let Some(g) = fx.glow.filter(|g| g.color.a > 0.0 && g.rad > 0.0) {
            let sigma = (px_clamped(g.rad) / 2.0).clamp(0.0, MAX_FX_PX);
            let table: Vec<String> = GLOW_FALLOFF.iter().map(|v| num(*v)).collect();
            let fid = self.id("gl");
            let _ = write!(
                self.defs,
                r#"<filter id="{fid}" {}><feGaussianBlur in="SourceAlpha" stdDeviation="{}" result="b"/><feComponentTransfer in="b" result="s"><feFuncA type="linear" slope="5"/></feComponentTransfer><feComponentTransfer in="s" result="d"><feFuncA type="table" tableValues="{}"/></feComponentTransfer><feFlood flood-color="{}" flood-opacity="{}"/><feComposite in2="d" operator="in"/></filter>"#,
                region(sigma * 4.0, UNITS_GLOW),
                num(sigma),
                table.join(" "),
                hex(g.color),
                num(alpha_of(g.color)),
            );
            let _ = write!(self.body, r##"<use href="#{gid}" filter="url(#{fid})"/>"##);
        }
        // 4. the shape itself: soft edge, then inner shadow
        let mut inner = String::new();
        let mut layer = format!(r##"<use href="#{gid}"/>"##);
        if let Some(rad) = fx.soft_edge.filter(|r| *r > 0.0) {
            let r = px_clamped(rad).clamp(0.0, 2.0 * MAX_FX_PX);
            let fid = self.id("se");
            let _ = write!(
                self.defs,
                r#"<filter id="{fid}" {}><feGaussianBlur in="SourceAlpha" stdDeviation="{}" result="b"/><feComponentTransfer in="b" result="e"><feFuncA type="linear" slope="2" intercept="-1"/></feComponentTransfer><feComposite in="SourceGraphic" in2="e" operator="in"/></filter>"#,
                region(r, UNITS_SOFT_EDGE),
                num((r * 2.0 / 3.0).min(MAX_FX_PX)),
            );
            layer = format!(r##"<use href="#{gid}" filter="url(#{fid})"/>"##);
        }
        if let Some(sh) = fx.inner_shadow.filter(|s| s.color.a > 0.0) {
            let blur = px_clamped(sh.blur_rad).max(0.0);
            let dist = px_clamped(sh.dist);
            let dir = fnum(sh.dir_deg).to_radians();
            let fid = self.id("is");
            let _ = write!(
                self.defs,
                r#"<filter id="{fid}" {}><feFlood flood-color="{}" flood-opacity="{}" result="c"/><feComposite in="c" in2="SourceAlpha" operator="out" result="i"/><feGaussianBlur in="i" stdDeviation="{}" result="b"/><feOffset in="b" dx="{}" dy="{}" result="o"/><feComposite in="o" in2="SourceAlpha" operator="in" result="s"/><feMerge><feMergeNode in="SourceGraphic"/><feMergeNode in="s"/></feMerge></filter>"#,
                region(blur * 2.0 + dist.abs(), UNITS_INNER_SHADOW),
                hex(sh.color),
                num(alpha_of(sh.color)),
                num((blur / 2.0).min(MAX_FX_PX)),
                num(dist * dir.cos()),
                num(dist * dir.sin()),
            );
            inner = fid;
        }
        if inner.is_empty() {
            self.body.push_str(&layer);
        } else {
            let _ = write!(self.body, r##"<g filter="url(#{inner})">{layer}</g>"##);
        }
        // The drawing process refuses a picture whose filters add up to too much work (see
        // `svg_guard`) and spends time in proportion to it. The work of this shape is known only
        // once its filters are written: if it takes the slide past the allowance they are taken
        // back and the shape is drawn plain (the check is on the total *with* this shape, not on
        // what came before it: one shape with every effect can use up the whole allowance).
        if self.filter_work + work.get() > MAX_FILTER_WORK {
            self.body.truncate(body_at);
            self.defs.truncate(defs_at);
            self.truncated = true;
            self.flush_cur(1.0);
            self.body.push_str(content);
            return;
        }
        self.filter_work += work.get();
        // The effects draw the shape again in each of their layers.
        let layers = Self::fx_layers(fx) as f64;
        self.flush_cur(layers);
        let sc = cost::scan(content);
        self.feat.els += sc.els * (layers - 1.0);
        self.feat.segs += sc.segs * (layers - 1.0);
    }

    /// The mirrored, faded copy of the shape.
    fn reflection_layer(
        &mut self,
        gid: &str,
        r: &Reflection,
        (l, t, w, h): (f64, f64, f64, f64),
        (ax, ay): (f64, f64),
        region: &dyn Fn(f64, f64) -> String,
    ) {
        let dist = px_clamped(r.dist);
        let dir = fnum(r.dir_deg).to_radians();
        let (dx, dy) = (dist * dir.cos(), dist * dir.sin());
        let (sx, sy) = (
            if r.sx.is_finite() {
                r.sx.clamp(-20.0, 20.0)
            } else {
                1.0
            },
            if r.sy.is_finite() {
                r.sy.clamp(-20.0, 20.0)
            } else {
                1.0
            },
        );
        let blur = px_clamped(r.blur_rad).max(0.0);
        // The fade runs over the shape's own height, from its bottom edge upwards in the shape's
        // coordinates (the mirrored copy shows it from its top edge downwards).
        let gr = self.id("rg");
        let mk = self.id("rm");
        let clamp01 = |v: f64| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let (sp, ep) = (clamp01(r.start_pos), clamp01(r.end_pos.max(r.start_pos)));
        let (sa, ea) = (clamp01(r.start_alpha), clamp01(r.end_alpha));
        let _ = write!(
            self.defs,
            r##"<linearGradient id="{gr}" gradientUnits="userSpaceOnUse" x1="0" y1="{}" x2="0" y2="{}"><stop offset="{}" stop-color="#fff" stop-opacity="{}"/><stop offset="{}" stop-color="#fff" stop-opacity="{}"/></linearGradient><mask id="{mk}" maskUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}"><rect x="{}" y="{}" width="{}" height="{}" fill="url(#{gr})"/></mask>"##,
            num(t + h),
            num(t),
            num(sp),
            num(sa),
            num(ep.max(sp + 1e-6)),
            num(ea),
            num(l - 10.0),
            num(t - 10.0),
            num(w + 20.0),
            num(h + 20.0),
            num(l - 10.0),
            num(t - 10.0),
            num(w + 20.0),
            num(h + 20.0),
        );
        let filter = if blur > 0.0 {
            let fid = self.id("rb");
            let _ = write!(
                self.defs,
                r#"<filter id="{fid}" {}><feGaussianBlur stdDeviation="{}"/></filter>"#,
                region(blur * 3.0 + h * sy.abs() + dist.abs(), UNITS_BLUR),
                num((blur / 2.0).min(MAX_FX_PX)),
            );
            format!(r##" filter="url(#{fid})""##)
        } else {
            String::new()
        };
        // (The mask is in the user space of the element, i.e. the shape's own coordinates; the
        // transform mirrors both.)
        let _ = write!(
            self.body,
            r##"<g transform="translate({} {}) scale({} {}) translate({} {})"{filter}><use href="#{gid}" mask="url(#{mk})"/></g>"##,
            num(ax + dx),
            num(ay + dy),
            num(sx),
            num(sy),
            num(-ax),
            num(-ay),
        );
    }
}
