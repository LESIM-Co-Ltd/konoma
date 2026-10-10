//! 3-D charts: bars, lines and areas in a box with depth, and the tilted pie.
//!
//! The drawing is a deliberately small *oblique* projection, not a real 3-D renderer. A 3-D
//! cartesian plot keeps its front plane as an ordinary rectangle (so the axes, tick labels and data
//! labels are drawn by the 2-D code on that rectangle) and the depth axis goes back to the upper
//! right (or upper left) by a fixed screen vector: `(sin rotY, sin rotX)` per pixel of depth, so a
//! chart seen from the left and above (`rotY` 20, `rotX` 15: Office's default) shows the left wall,
//! the floor, and the right and top faces of every bar. The faces are painted back to front: the
//! back wall, the side wall, the floor and their gridlines first; then the series, each bar as its
//! right (or left) face, its top face and its front face; the front face has the series colour, the
//! top one a lighter and the side one a darker tone of it.
//!
//! Approximations (deliberate; documented in [`super`]): perspective (`perspective`) and the height
//! percentage (`hPercent`) are not used, `rAngAx = 0` is drawn with the same oblique projection as
//! `rAngAx = 1` (so the front plane is never skewed), a negative `rotX` (a view from below) is
//! drawn as `0`, 3-D columns with a *standard* grouping (series in depth rows) are drawn as
//! clustered columns, and bar shapes other than the box (cylinder, cone, pyramid) are boxes.
//! Areas and lines of a standard grouping get one depth row per series, the first series in front
//! (as in Office); stacked ones share one row.
//!
//! A 3-D pie is its circle seen from above at `rotX` degrees: an ellipse `sin rotX` as high as it
//! is wide, with a side band of `0.22 cos rotX` of the radius (times `depthPercent`) below it; the
//! slices are painted back to front, an exploded slice also shows its cut faces.

use super::super::color::mix;
use super::super::model::*;
use super::layout::{fill_color, Rect};
use super::shapes::{on_ellipse, Out};
use super::{ChartGroup, ChartModel, GroupKind, Grouping, View3D, Wall};

/// Degrees between two points of an ellipse drawn as a polyline.
const ARC_STEP_DEG: f64 = 4.0;
/// Most points of one ellipse arc.
const MAX_ARC_POINTS: usize = 120;
/// The depth never shifts the back plane by more than this fraction of the plot's width / height.
const MAX_SHIFT_X: f64 = 0.22;
const MAX_SHIFT_Y: f64 = 0.28;
/// How much darker the side faces and lighter the top faces are than the front.
const SIDE_SHADE: f64 = 0.28;
const TOP_LIGHTEN: f64 = 0.18;
/// Thickness of a 3-D pie, as a fraction of its radius at `rotX` 0 and `depthPercent` 100.
const PIE_THICK: f64 = 0.22;
/// Flattest a tilted pie is drawn (height over width).
const MIN_SQUASH: f64 = 0.18;

/// The view of the chart with every number finite and in its range (a model may hold anything).
pub(super) fn view(m: &ChartModel) -> View3D {
    clean(m.view3d.unwrap_or_default())
}

fn clean(v: View3D) -> View3D {
    let d = View3D::default();
    let f = |x: f64, lo: f64, hi: f64, dflt: f64| {
        if x.is_finite() {
            x.clamp(lo, hi)
        } else {
            dflt
        }
    };
    View3D {
        rot_x: f(v.rot_x, -90.0, 90.0, d.rot_x),
        rot_y: f(v.rot_y, -360.0, 720.0, d.rot_y),
        r_ang_ax: v.r_ang_ax,
        perspective: f(v.perspective, 0.0, 240.0, d.perspective),
        depth_percent: f(v.depth_percent, 20.0, 2000.0, d.depth_percent),
        h_percent: v.h_percent.filter(|h| h.is_finite()),
    }
}

/// The depth axis of a 3-D cartesian plot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Depth {
    /// Screen vector of one pixel of depth: right and up (up positive).
    pub ux: f64,
    pub uy: f64,
    /// Total depth, px.
    pub d: f64,
    /// `gapDepth`, percent.
    pub gap_pct: f64,
}

impl Depth {
    /// The screen offset (x right, y down) of the point `z` px back.
    pub fn off(&self, z: f64) -> (f64, f64) {
        (z * self.ux, -z * self.uy)
    }

    /// How far the back plane is shifted: `(dx, dy)`, `dy` up positive.
    pub fn shift(&self) -> (f64, f64) {
        (self.d * self.ux, self.d * self.uy)
    }

    /// Depth of a bar / ribbon in a row of `row` px, and where it starts inside the row.
    pub fn thickness(&self, row: f64) -> (f64, f64) {
        let t = row / (1.0 + self.gap_pct.max(0.0) / 100.0);
        (t, (row - t) / 2.0)
    }
}

/// The cartesian groups that are drawn in 3-D.
fn groups3(m: &ChartModel) -> impl Iterator<Item = &ChartGroup> {
    m.groups.iter().filter(|g| {
        g.three_d && matches!(g.kind, GroupKind::Bar | GroupKind::Line | GroupKind::Area)
    })
}

/// Rows in depth of one group: a standard line or area group has one per series.
pub(super) fn rows_of(g: &ChartGroup) -> usize {
    match (g.kind, g.grouping) {
        (GroupKind::Line | GroupKind::Area, Grouping::Standard) => g.series.len().max(1),
        _ => 1,
    }
}

/// The depth of the plot whose full box (front plane and depth) is `base`, or `None` when the chart
/// has no 3-D cartesian group.
pub(super) fn depth(m: &ChartModel, base: &Rect) -> Option<Depth> {
    let mut any = false;
    let mut rows = 1usize;
    let mut cats = 1usize;
    // Bars of a cluster side by side: a bar is a fraction of its category's width.
    let mut per_slot = 1usize;
    let mut bars_only = true;
    let mut horizontal = false;
    let mut gap = 150.0;
    for g in groups3(m) {
        any = true;
        rows = rows.max(rows_of(g));
        bars_only &= g.kind == GroupKind::Bar;
        horizontal |= g.kind == GroupKind::Bar && g.bar_dir == super::BarDir::Bar;
        gap = g.gap_depth;
        if g.kind == GroupKind::Bar
            && !matches!(g.grouping, Grouping::Stacked | Grouping::PercentStacked)
        {
            per_slot = per_slot.max(g.series.len());
        }
        for s in &g.series {
            cats = cats.max(s.values.len().max(s.cats.len()));
        }
    }
    if !any {
        return None;
    }
    let v = view(m);
    let ux = v.rot_y.to_radians().sin();
    let uy = v.rot_x.clamp(0.0, 90.0).to_radians().sin();
    let pct = v.depth_percent / 100.0;
    let mut d = if bars_only {
        let slot = if horizontal { base.h } else { base.w } / cats.max(1) as f64;
        slot / per_slot.max(1) as f64 * pct
    } else {
        0.16 * base.w.min(base.h) * (rows as f64).sqrt() * pct
    };
    // Keep the shifted back plane inside the plot.
    if ux.abs() > 1e-6 {
        d = d.min(MAX_SHIFT_X * base.w / ux.abs());
    }
    if uy > 1e-6 {
        d = d.min(MAX_SHIFT_Y * base.h / uy);
    }
    if !d.is_finite() || d < 0.0 {
        d = 0.0;
    }
    Some(Depth {
        ux,
        uy,
        d,
        gap_pct: gap,
    })
}

/// The front plane inside `base` once the shifted back plane has its room.
pub(super) fn front_rect(base: Rect, d: &Depth) -> Rect {
    let (dx, dy) = d.shift();
    Rect {
        x: base.x + (-dx).max(0.0),
        y: base.y + dy,
        w: (base.w - dx.abs()).max(10.0),
        h: (base.h - dy).max(10.0),
    }
}

/// The back plane: the front rectangle moved back by the whole depth.
pub(super) fn back_rect(f: &Rect, d: &Depth) -> Rect {
    let (dx, dy) = d.shift();
    Rect {
        x: f.x + dx,
        y: f.y - dy,
        w: f.w,
        h: f.h,
    }
}

fn side_color(c: Rgba) -> Rgba {
    mix(c, Rgba::BLACK.with_alpha(c.a), SIDE_SHADE)
}

fn top_color(c: Rgba) -> Rgba {
    mix(c, Rgba::WHITE.with_alpha(c.a), TOP_LIGHTEN)
}

/// A face: a closed polygon with a hairline of its own colour so adjacent faces leave no seam.
fn face(o: &mut Out, pts: &[(f64, f64)], c: Rgba) {
    let ln = Line::solid(6350.0, c);
    o.poly(pts, true, &Fill::Solid(c), Some(&ln));
}

/// The walls and the floor, behind everything: only the ones the chart gives a fill.
pub(super) fn walls(o: &mut Out, m: &ChartModel, f: &Rect, d: &Depth) {
    let b = back_rect(f, d);
    let (dx, _) = d.shift();
    let paint = |o: &mut Out, w: &Option<Wall>, pts: &[(f64, f64)]| {
        let Some(w) = w else { return };
        let fill = w.fill.clone().unwrap_or(Fill::None);
        let ln = super::shapes::line_if_set(w.line.as_ref(), Rgba::BLACK, 9525.0);
        if fill.is_visible() || ln.is_some() {
            o.poly(pts, true, &fill, ln.as_ref());
        }
    };
    paint(
        o,
        &m.back_wall,
        &[
            (b.x, b.y),
            (b.right(), b.y),
            (b.right(), b.bottom()),
            (b.x, b.bottom()),
        ],
    );
    // The side wall is on the side the back plane is shifted away from.
    if dx >= 0.0 {
        paint(
            o,
            &m.side_wall,
            &[(f.x, f.y), (b.x, b.y), (b.x, b.bottom()), (f.x, f.bottom())],
        );
    } else {
        paint(
            o,
            &m.side_wall,
            &[
                (f.right(), f.y),
                (b.right(), b.y),
                (b.right(), b.bottom()),
                (f.right(), f.bottom()),
            ],
        );
    }
    paint(
        o,
        &m.floor,
        &[
            (f.x, f.bottom()),
            (f.right(), f.bottom()),
            (b.right(), b.bottom()),
            (b.x, b.bottom()),
        ],
    );
}

/// One gridline at fraction `f` of an axis, drawn on the walls: `vertical` gridlines belong to the
/// horizontal axis. They run across the back wall and then along the side wall (vertical ones:
/// along the floor).
pub(super) fn grid_line(o: &mut Out, front: &Rect, d: &Depth, vertical: bool, f: f64, ln: &Line) {
    let b = back_rect(front, d);
    let (dx, _) = d.shift();
    if vertical {
        let x = front.x + f * front.w;
        let bx = b.x + f * b.w;
        o.seg(bx, b.y, bx, b.bottom(), ln);
        o.seg(x, front.bottom(), bx, b.bottom(), ln);
    } else {
        let y = front.bottom() - f * front.h;
        let by = b.bottom() - f * b.h;
        o.seg(b.x, by, b.right(), by, ln);
        let sx = if dx >= 0.0 { front.x } else { front.right() };
        let bx = if dx >= 0.0 { b.x } else { b.right() };
        o.seg(sx, y, bx, by, ln);
    }
}

/// A bar of the 2-D rectangle `r` (the front plane), `z0` px back and `thick` px deep, with its
/// side and top faces. Returns the front face as drawn (moved back by `z0`).
#[allow(clippy::too_many_arguments)]
pub(super) fn bar_box(
    o: &mut Out,
    d: &Depth,
    r: &Rect,
    z0: f64,
    thick: f64,
    fill: &Fill,
    line: Option<&Line>,
) -> Rect {
    let (ox, oy) = d.off(z0);
    let (fx, fy) = (r.x + ox, r.y + oy);
    let front = Rect {
        x: fx,
        y: fy,
        w: r.w,
        h: r.h,
    };
    let (ex, ey) = d.off(thick);
    if let Some(c) = fill_color(fill) {
        if ex > 0.01 {
            face(
                o,
                &[
                    (fx + r.w, fy),
                    (fx + r.w + ex, fy + ey),
                    (fx + r.w + ex, fy + r.h + ey),
                    (fx + r.w, fy + r.h),
                ],
                side_color(c),
            );
        } else if ex < -0.01 {
            face(
                o,
                &[
                    (fx, fy),
                    (fx + ex, fy + ey),
                    (fx + ex, fy + r.h + ey),
                    (fx, fy + r.h),
                ],
                side_color(c),
            );
        }
        if ey < -0.01 {
            face(
                o,
                &[
                    (fx, fy),
                    (fx + r.w, fy),
                    (fx + r.w + ex, fy + ey),
                    (fx + ex, fy + ey),
                ],
                top_color(c),
            );
        }
    }
    o.rect(fx, fy, r.w, r.h, fill, line);
    front
}

/// An area ribbon: the front polygon (`top` left to right, then `bottom` right to left as
/// `poly`), `z0` px back and `thick` px deep, with its top band and its end face.
#[allow(clippy::too_many_arguments)]
pub(super) fn area_ribbon(
    o: &mut Out,
    d: &Depth,
    top: &[(f64, f64)],
    bottom: &[(f64, f64)],
    z0: f64,
    thick: f64,
    fill: &Fill,
    line: Option<&Line>,
) {
    if top.len() < 2 || bottom.len() != top.len() {
        return;
    }
    let (ox, oy) = d.off(z0);
    let (ex, ey) = d.off(thick);
    let mv = |p: &(f64, f64)| (p.0 + ox, p.1 + oy);
    let t: Vec<(f64, f64)> = top.iter().map(mv).collect();
    let b: Vec<(f64, f64)> = bottom.iter().map(mv).collect();
    if let Some(c) = fill_color(fill) {
        // End face: at the right end when the depth goes right, else at the left end.
        if ex.abs() > 0.01 {
            let k = if ex > 0.0 { t.len() - 1 } else { 0 };
            face(
                o,
                &[
                    t[k],
                    (t[k].0 + ex, t[k].1 + ey),
                    (b[k].0 + ex, b[k].1 + ey),
                    b[k],
                ],
                side_color(c),
            );
        }
        // Top band, one quad per segment.
        if ey < -0.01 {
            for w in t.windows(2) {
                face(
                    o,
                    &[
                        w[0],
                        w[1],
                        (w[1].0 + ex, w[1].1 + ey),
                        (w[0].0 + ex, w[0].1 + ey),
                    ],
                    top_color(c),
                );
            }
        }
    }
    let mut poly = t;
    poly.extend(b.iter().rev().copied());
    o.poly(&poly, true, fill, line);
}

/// A line ribbon (a 3-D line): `pts` is one run of the front line, drawn `z0` px back with the
/// top band `thick` px deep and the line itself in front.
pub(super) fn line_ribbon(
    o: &mut Out,
    d: &Depth,
    pts: &[(f64, f64)],
    z0: f64,
    thick: f64,
    ln: &Line,
) {
    if pts.len() < 2 {
        return;
    }
    let (ox, oy) = d.off(z0);
    let (ex, ey) = d.off(thick);
    let p: Vec<(f64, f64)> = pts.iter().map(|q| (q.0 + ox, q.1 + oy)).collect();
    if let Some(c) = fill_color(&ln.fill) {
        if ey < -0.01 || ex.abs() > 0.01 {
            for w in p.windows(2) {
                face(
                    o,
                    &[
                        w[0],
                        w[1],
                        (w[1].0 + ex, w[1].1 + ey),
                        (w[0].0 + ex, w[0].1 + ey),
                    ],
                    side_color(c),
                );
            }
        }
    }
    o.poly(&p, false, &Fill::None, Some(ln));
}

// ---------------------------------------------------------------------------------------------
// Pie
// ---------------------------------------------------------------------------------------------

/// The tilt of a 3-D pie: height over width of its top, and the thickness over the radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Tilt {
    pub squash: f64,
    pub thick: f64,
}

/// The tilt of the 3-D pie of `m` (`rotX` defaults to 30 for a pie).
pub(super) fn pie_tilt(m: &ChartModel) -> Tilt {
    // A pie's own default elevation is 30 degrees.
    let v = m.view3d.map(clean);
    let rot_x = v.map_or(30.0, |v| v.rot_x).clamp(0.0, 90.0);
    let pct = v.map_or(100.0, |v| v.depth_percent) / 100.0;
    Tilt {
        squash: rot_x.to_radians().sin().max(MIN_SQUASH),
        thick: (PIE_THICK * rot_x.to_radians().cos() * pct).clamp(0.0, 0.6),
    }
}

/// One slice of a 3-D pie.
pub(super) struct Slice {
    /// Start angle and sweep, degrees clockwise from 12 o'clock.
    pub a_s: f64,
    pub sweep: f64,
    /// Centre of the slice's top (moved by its explosion).
    pub cx: f64,
    pub cy: f64,
    pub fill: Fill,
    pub line: Option<Line>,
    pub exploded: bool,
}

fn arc_pts(cx: f64, cy: f64, rx: f64, ry: f64, a0: f64, a1: f64) -> Vec<(f64, f64)> {
    let n = (((a1 - a0).abs() / ARC_STEP_DEG).ceil() as usize).clamp(1, MAX_ARC_POINTS);
    (0..=n)
        .map(|i| on_ellipse(cx, cy, rx, ry, a0 + (a1 - a0) * i as f64 / n as f64))
        .collect()
}

/// Draws the slices of a tilted pie (`rx` wide, `ry` high, `t` thick), back to front.
pub(super) fn draw_pie(o: &mut Out, mut slices: Vec<Slice>, rx: f64, ry: f64, t: f64) {
    let key = |s: &Slice| (s.a_s + s.sweep / 2.0).to_radians().cos();
    slices.sort_by(|a, b| key(b).total_cmp(&key(a)));
    for s in &slices {
        if o.full() {
            return;
        }
        let a_e = s.a_s + s.sweep;
        let base = fill_color(&s.fill);
        let side = base.map(side_color);
        if let (Some(side), true) = (side, t > 0.05) {
            // The cut faces of an exploded slice.
            if s.exploded && s.sweep < 359.99 {
                for (theta, is_end) in [(s.a_s, false), (a_e, true)] {
                    let sn = theta.to_radians().sin();
                    if (is_end && sn > 0.01) || (!is_end && sn < -0.01) {
                        let r = on_ellipse(s.cx, s.cy, rx, ry, theta);
                        face(
                            o,
                            &[(s.cx, s.cy), r, (r.0, r.1 + t), (s.cx, s.cy + t)],
                            side,
                        );
                    }
                }
            }
            // The band round the front half of the rim (angles 90..270).
            for shift in [-360.0, 0.0, 360.0] {
                let lo = (s.a_s + shift).max(90.0);
                let hi = (a_e + shift).min(270.0);
                if hi > lo + 1e-6 {
                    let mut pts = arc_pts(s.cx, s.cy, rx, ry, lo, hi);
                    let back: Vec<(f64, f64)> = pts.iter().rev().map(|p| (p.0, p.1 + t)).collect();
                    pts.extend(back);
                    face(o, &pts, side);
                }
            }
        }
        // The top.
        if s.sweep >= 359.99 {
            o.ellipse(s.cx, s.cy, rx, ry, &s.fill, s.line.as_ref());
        } else {
            let mut pts = vec![(s.cx, s.cy)];
            pts.extend(arc_pts(s.cx, s.cy, rx, ry, s.a_s, a_e));
            o.poly(&pts, true, &s.fill, s.line.as_ref());
        }
    }
}
