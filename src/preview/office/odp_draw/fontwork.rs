//! Fontwork (`draw:custom-shape` with `draw:text-path="true"`): the text is drawn as outlines
//! warped into the shape.
//!
//! LibreOffice lays the text out on one straight line and maps it into the shape's guide curves
//! (the sub-paths of the enhanced path). We do the same with the real glyph outlines of the face
//! the font stack resolves to (read with `skrifa`, the crate resvg reads fonts with):
//!
//! * **Two or more sub-paths** (`fontwork-plain-text`, `-wave`, `-chevron-*`, `-slant-*`,
//!   `-fade-*`, ...): the first two are the top and the bottom edge. A point of the text at
//!   `(u, v)` (`u` along the width of the line, `v` from the top to the bottom of the glyph
//!   box) lands on the straight line between the top curve and the bottom curve at the same
//!   fraction of their lengths. This is exact for those types.
//! * **One sub-path** (`fontwork-arch-*-curve`, `-circle-curve`, `-open-circle-curve`): the
//!   line follows the curve as its baseline, the glyphs standing on it; the scale is the one
//!   that makes the text as long as the curve (kept uniform, so the letters are not stretched).
//!   Approximation: LibreOffice scales the letters by the curve's own ratio; a closed curve
//!   starts where the path starts.
//! * **Approximations**: several lines are stacked in equal bands; the kerning of the face is not
//!   applied; `draw:text-path-same-letter-heights` is not read; a glyph the stack's face lacks
//!   comes from the system fallback face; the shape's fill (a gradient too) covers the shape's
//!   box, where LibreOffice's covers the text's bounds.
//! * **Budget**: [`MAX_CHARS`] characters, [`MAX_POINTS`] points per shape (over it the rest is
//!   left out and the caller sets `truncated`).

use resvg::usvg::fontdb;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};

use crate::preview::office::slide_draw as sd;
use crate::preview::svg::shared_fontdb;
use sd::path::Seg;
use sd::{GeomPath, PathCmd, PathFill, Pt};

/// Most characters warped in one shape.
pub(super) const MAX_CHARS: usize = 400;
/// Most points one warped shape may have.
pub(super) const MAX_POINTS: usize = 400_000;
/// A segment of a glyph is cut so that no piece spans more than this fraction of the text width
/// (the warp is not linear along the curve).
const MAX_STEP: f64 = 1.0 / 300.0;
/// Most pieces one segment is cut into.
const MAX_SPLIT: usize = 48;
/// The fraction of a guide's length either side of a point its direction is taken over.
const DIR_SPAN: f64 = 0.01;
/// Pieces a cubic of a guide curve is flattened into (the letters stand on it: its corners show).
const GUIDE_CUBIC_STEPS: usize = 64;
/// Pieces a glyph curve is flattened into.
const CURVE_STEPS: usize = 6;

/// A line of text to warp.
pub(super) struct TextLine {
    pub text: String,
    pub stack: Vec<String>,
    pub bold: bool,
    pub italic: bool,
}

/// The outlines of one line, in font units, y down, origin at the line's start.
struct Laid {
    /// Closed polylines.
    contours: Vec<Vec<Pt>>,
    width: f64,
    ymin: f64,
    ymax: f64,
}

struct Pen {
    out: Vec<Vec<Pt>>,
    cur: Vec<Pt>,
    dx: f64,
}

impl Pen {
    fn p(&self, x: f32, y: f32) -> Pt {
        Pt::new(self.dx + f64::from(x), -f64::from(y))
    }
    fn last(&self) -> Pt {
        self.cur.last().copied().unwrap_or_default()
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        let p = self.p(x, y);
        self.cur.push(p);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.cur.push(p);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (a, c, b) = (self.last(), self.p(cx, cy), self.p(x, y));
        for i in 1..=CURVE_STEPS {
            let t = i as f64 / CURVE_STEPS as f64;
            let m = 1.0 - t;
            self.cur.push(Pt::new(
                m * m * a.x + 2.0 * m * t * c.x + t * t * b.x,
                m * m * a.y + 2.0 * m * t * c.y + t * t * b.y,
            ));
        }
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        let (a, c1, c2, b) = (
            self.last(),
            self.p(c1x, c1y),
            self.p(c2x, c2y),
            self.p(x, y),
        );
        for i in 1..=CURVE_STEPS {
            let t = i as f64 / CURVE_STEPS as f64;
            let m = 1.0 - t;
            self.cur.push(Pt::new(
                m * m * m * a.x + 3.0 * m * m * t * c1.x + 3.0 * m * t * t * c2.x + t * t * t * b.x,
                m * m * m * a.y + 3.0 * m * m * t * c1.y + 3.0 * m * t * t * c2.y + t * t * t * b.y,
            ));
        }
    }
    fn close(&mut self) {
        if self.cur.len() > 1 {
            self.out.push(std::mem::take(&mut self.cur));
        } else {
            self.cur.clear();
        }
    }
}

/// Lays `line` out on one straight line: the glyph outlines and the advance width.
fn lay_out(line: &TextLine, budget: &mut usize) -> Option<Laid> {
    let text = line.text.trim_end();
    if text.chars().all(char::is_whitespace) {
        return None;
    }
    let metrics = sd::fonts::metrics_for(&line.stack, line.bold, line.italic)?;
    let db = shared_fontdb();
    let primary = metrics.face_id();
    let mut pen = Pen {
        out: Vec::new(),
        cur: Vec::new(),
        dx: 0.0,
    };
    let mut x = 0.0_f64;
    for c in text.chars() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let mut advance = None;
        let mut try_face = |id: fontdb::ID, pen: &mut Pen| -> bool {
            db.with_face_data(id, |data, index| {
                let Ok(font) = FontRef::from_index(data, index) else {
                    return false;
                };
                let Some(gid) = font.charmap().map(c) else {
                    return false;
                };
                if gid.to_u32() == 0 && !c.is_whitespace() {
                    return false;
                }
                let em = f64::from(
                    font.metrics(Size::unscaled(), LocationRef::default())
                        .units_per_em,
                );
                // Every face is scaled to the first face's em.
                let k = if em > 0.0 {
                    upem_of(&metrics) / em
                } else {
                    1.0
                };
                let adv = font
                    .glyph_metrics(Size::unscaled(), LocationRef::default())
                    .advance_width(gid)
                    .map_or(em * 0.5, f64::from)
                    * k;
                pen.dx = x;
                if let Some(g) = font.outline_glyphs().get(gid) {
                    let before = pen.out.len();
                    let _ = g.draw(
                        DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                        &mut *pen,
                    );
                    pen.close();
                    if (k - 1.0).abs() > 1e-9 {
                        for contour in &mut pen.out[before..] {
                            for p in contour {
                                p.x = x + (p.x - x) * k;
                                p.y *= k;
                            }
                        }
                    }
                }
                advance = Some(adv);
                true
            })
            .unwrap_or(false)
        };
        let ok = try_face(primary, &mut pen)
            || metrics
                .fallback_face(c)
                .is_some_and(|id| try_face(id, &mut pen));
        if !ok {
            // No face has it: the width of a half em, nothing drawn.
            advance = Some(upem_of(&metrics) * 0.5);
        }
        x += advance.unwrap_or(0.0);
    }
    let (mut ymin, mut ymax) = (f64::MAX, f64::MIN);
    for contour in &pen.out {
        for p in contour {
            ymin = ymin.min(p.y);
            ymax = ymax.max(p.y);
        }
    }
    if pen.out.is_empty()
        || ymax.partial_cmp(&ymin) != Some(std::cmp::Ordering::Greater)
        || x <= 0.0
    {
        return None;
    }
    Some(Laid {
        contours: pen.out,
        width: x,
        ymin,
        ymax,
    })
}

fn upem_of(m: &crate::preview::mermaid::text_metrics::TextMetrics) -> f64 {
    f64::from(m.units_per_em()).max(1.0)
}

/// A polyline with the running length at each point.
struct Poly {
    pts: Vec<Pt>,
    cum: Vec<f64>,
}

impl Poly {
    fn new(pts: Vec<Pt>) -> Option<Poly> {
        if pts.len() < 2 {
            return None;
        }
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            let l = cum.last().copied().unwrap_or(0.0) + (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
            cum.push(l);
        }
        let total = cum.last().copied().unwrap_or(0.0);
        (total.is_finite() && total > 0.0).then_some(Poly { pts, cum })
    }

    fn length(&self) -> f64 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    /// The point at the fraction `t` of the length (clamped).
    fn point(&self, t: f64) -> Pt {
        let d = t.clamp(0.0, 1.0) * self.length();
        let i = match self
            .cum
            .binary_search_by(|c| c.partial_cmp(&d).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(i) => i.min(self.pts.len() - 2),
            Err(i) => i.saturating_sub(1).min(self.pts.len() - 2),
        };
        let (a, b) = (self.pts[i], self.pts[i + 1]);
        let seg = (self.cum[i + 1] - self.cum[i]).max(1e-12);
        let k = ((d - self.cum[i]) / seg).clamp(0.0, 1.0);
        Pt::new(a.x + (b.x - a.x) * k, a.y + (b.y - a.y) * k)
    }

    /// The point at the fraction `t` and the unit direction there (taken over a short stretch,
    /// so that the corners of the polyline do not show in the letters standing on it).
    fn at(&self, t: f64) -> (Pt, Pt) {
        let t = t.clamp(0.0, 1.0);
        let (a, b) = (self.point(t - DIR_SPAN), self.point(t + DIR_SPAN));
        let l = (b.x - a.x).hypot(b.y - a.y).max(1e-12);
        (self.point(t), Pt::new((b.x - a.x) / l, (b.y - a.y) / l))
    }

    /// Runs left to right (the text reads along the curve that way).
    fn left_to_right(mut self) -> Poly {
        let (a, b) = (self.pts[0], self.pts[self.pts.len() - 1]);
        if a.x > b.x {
            self.pts.reverse();
            let total = self.length();
            let mut cum: Vec<f64> = self.cum.iter().rev().map(|c| total - c).collect();
            cum[0] = 0.0;
            self.cum = cum;
        }
        self
    }
}

/// The sub-paths of the guide geometry as polylines (box coordinates, EMU).
fn guide_polys(paths: &[GeomPath], w: f64, h: f64) -> Vec<Poly> {
    let mut out = Vec::new();
    for p in paths {
        let r = sd::path::resolve_path(p, 0.0, 0.0, w, h);
        let mut cur: Vec<Pt> = Vec::new();
        let flush = |cur: &mut Vec<Pt>, out: &mut Vec<Poly>| {
            if let Some(poly) = Poly::new(std::mem::take(cur)) {
                out.push(poly.left_to_right());
            } else {
                cur.clear();
            }
        };
        for s in &r.segs {
            match *s {
                Seg::M(q) => {
                    flush(&mut cur, &mut out);
                    cur.push(q);
                }
                Seg::L(q) => cur.push(q),
                Seg::Q(c, q) => {
                    let a = cur.last().copied().unwrap_or(q);
                    for i in 1..=CURVE_STEPS * 2 {
                        let t = i as f64 / (CURVE_STEPS * 2) as f64;
                        let m = 1.0 - t;
                        cur.push(Pt::new(
                            m * m * a.x + 2.0 * m * t * c.x + t * t * q.x,
                            m * m * a.y + 2.0 * m * t * c.y + t * t * q.y,
                        ));
                    }
                }
                Seg::C(c1, c2, q) => {
                    let a = cur.last().copied().unwrap_or(q);
                    for i in 1..=GUIDE_CUBIC_STEPS {
                        let t = i as f64 / (GUIDE_CUBIC_STEPS) as f64;
                        let m = 1.0 - t;
                        cur.push(Pt::new(
                            m * m * m * a.x
                                + 3.0 * m * m * t * c1.x
                                + 3.0 * m * t * t * c2.x
                                + t * t * t * q.x,
                            m * m * m * a.y
                                + 3.0 * m * m * t * c1.y
                                + 3.0 * m * t * t * c2.y
                                + t * t * t * q.y,
                        ));
                    }
                }
                Seg::Z => {
                    if let Some(f) = cur.first().copied() {
                        cur.push(f);
                    }
                }
            }
        }
        flush(&mut cur, &mut out);
    }
    out
}

/// How the text is placed on the guides.
enum Guides {
    /// Top and bottom curves.
    Band(Poly, Poly),
    /// One curve the text stands on.
    Baseline(Poly),
}

fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// Warps `lines` into the geometry of `eg` (a `draw:enhanced-geometry` with a text path) of a
/// shape of `w` x `h` EMU. The paths are in the shape's box (EMU, origin at its corner), without
/// a scale of their own, each one a filled and stroked closed outline. `None` when the geometry
/// has no guide curve or there is nothing to draw. The flag says a budget cut the text.
pub(super) fn warp(
    paths: &[GeomPath],
    w: f64,
    h: f64,
    lines: &[TextLine],
) -> Option<(Vec<GeomPath>, bool)> {
    let mut polys = guide_polys(paths, w, h).into_iter();
    let guides = match (polys.next(), polys.next()) {
        (Some(a), Some(b)) => Guides::Band(a, b),
        (Some(a), None) => Guides::Baseline(a),
        _ => return None,
    };
    let mut budget = MAX_CHARS;
    let laid: Vec<Option<Laid>> = lines.iter().map(|l| lay_out(l, &mut budget)).collect();
    let truncated = budget == 0;
    let n = laid.len().max(1) as f64;
    let mut cmds: Vec<PathCmd> = Vec::new();
    let mut points = 0usize;
    'lines: for (i, l) in laid.iter().enumerate() {
        let Some(l) = l else { continue };
        let band = (i as f64 / n, (i as f64 + 1.0) / n);
        for contour in &l.contours {
            let mapped = map_contour(contour, l, &guides, band, h);
            points += mapped.len();
            if points > MAX_POINTS {
                break 'lines;
            }
            for (k, p) in mapped.into_iter().enumerate() {
                cmds.push(if k == 0 {
                    PathCmd::MoveTo(p)
                } else {
                    PathCmd::LineTo(p)
                });
            }
            cmds.push(PathCmd::Close);
        }
    }
    if cmds.is_empty() {
        return None;
    }
    Some((
        vec![GeomPath {
            w: 0.0,
            h: 0.0,
            fill_mode: PathFill::Norm,
            stroke: true,
            cmds,
        }],
        truncated || points > MAX_POINTS,
    ))
}

/// One glyph contour, cut finely and mapped.
fn map_contour(contour: &[Pt], l: &Laid, g: &Guides, band: (f64, f64), box_h: f64) -> Vec<Pt> {
    let height = l.ymax - l.ymin;
    let to_uv = |p: Pt| ((p.x / l.width).clamp(-0.5, 1.5), (p.y - l.ymin) / height);
    let map = |u: f64, v: f64| -> Pt {
        match g {
            Guides::Band(top, bottom) => {
                let vv = band.0 + v * (band.1 - band.0);
                lerp(top.at(u).0, bottom.at(u).0, vv)
            }
            Guides::Baseline(curve) => {
                // Uniform scale: the text as long as the curve, its box no taller than the shape.
                let mut s = curve.length() / l.width;
                if height * s > box_h && box_h > 0.0 {
                    s = box_h / height;
                }
                let (c, d) = curve.at(u);
                // Up is to the left of the direction of travel on a y-down screen.
                let up = Pt::new(d.y, -d.x);
                // The baseline is y = 0 in the glyph's own coordinates (y down: ascenders < 0).
                let y = (v * height + l.ymin) * s;
                Pt::new(c.x + up.x * -y, c.y + up.y * -y)
            }
        }
    };
    let mut out = Vec::new();
    let n = contour.len();
    for i in 0..n {
        let a = contour[i];
        let b = contour[(i + 1) % n];
        let (ua, va) = to_uv(a);
        let (ub, vb) = to_uv(b);
        let pieces = (((ub - ua).abs() / MAX_STEP).ceil() as usize).clamp(1, MAX_SPLIT);
        for k in 0..pieces {
            let t = k as f64 / pieces as f64;
            out.push(map(ua + (ub - ua) * t, va + (vb - va) * t));
        }
    }
    out
}
