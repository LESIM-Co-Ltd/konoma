//! Pie and doughnut charts.
//!
//! A pie uses the first series of its group; a doughnut draws one ring per series (the first series
//! innermost). Slices start at `firstSliceAng` degrees clockwise from 12 o'clock and run clockwise;
//! negative and blank values are zero. A slice with an explosion is moved out along its bisector
//! and the whole pie is shrunk so that the exploded slices stay inside the plot. Doughnut slices
//! are not exploded.

use super::super::model::*;
use super::labels::{self, Info};
use super::layout::{point_color, style_outline, Rect, TEXT_PT};
use super::shapes::{line_if_set, on_circle, on_ellipse, Out};
use super::text::{line_width, resolve, HAlign, VAlign};
use super::three_d;
use super::{ChartGroup, ChartModel, GroupKind, LabelPos, PointFmt};

pub(super) fn draw(o: &mut Out, m: &ChartModel, rect: Rect) {
    let Some(g) = m
        .groups
        .iter()
        .find(|g| matches!(g.kind, GroupKind::Pie | GroupKind::Doughnut))
    else {
        return;
    };
    if g.series.is_empty() {
        return;
    }
    let st_probe = resolve(m, &super::TextStyle::default(), TEXT_PT, false);
    let max_exp_early = if g.kind == GroupKind::Pie {
        g.series
            .first()
            .map(|s| {
                s.explosion
                    .into_iter()
                    .chain(s.points.iter().filter_map(|p| p.explosion))
                    .fold(0.0f64, f64::max)
            })
            .unwrap_or(0.0)
            .clamp(0.0, 400.0)
    } else {
        0.0
    };
    // Room for labels outside the pie is made only when some label ends up there.
    let r_probe = ((rect.w.min(rect.h) / 2.0 - 4.0) / (1.0 + max_exp_early / 100.0)).max(4.0);
    let outside_labels = g.kind == GroupKind::Pie
        && g.series.first().is_some_and(|s| {
            let n = s.values.len().max(s.cats.len());
            let (vals, total) = slices(&s.values, n);
            total > 0.0
                && vals.iter().enumerate().any(|(i, v)| {
                    *v > 0.0
                        && slice_label(m, g, s, i, *v, total, 360.0 * v / total, r_probe)
                            .is_some_and(|l| is_outside(l.2))
                })
        });
    let margin = if outside_labels {
        st_probe.line_h() + 14.0
    } else {
        4.0
    };
    let max_exp = max_exp_early;
    let tilt = (g.kind == GroupKind::Pie && g.three_d).then(|| three_d::pie_tilt(m));
    let r_full = match tilt {
        // A tilted pie is as wide as it is high times `squash`, plus its thickness.
        Some(t) => ((rect.w / 2.0 - margin)
            .min((rect.h - 2.0 * margin) / (2.0 * t.squash + t.thick)))
        .max(4.0),
        None => (rect.w.min(rect.h) / 2.0 - margin).max(4.0),
    };
    let radius = r_full / (1.0 + max_exp / 100.0);
    let (cx, mut cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    if let Some(t) = tilt {
        // The top face sits half the thickness above the middle of the plot.
        cy -= t.thick * radius / 2.0;
    }

    match g.kind {
        GroupKind::Pie => pie(o, m, g, cx, cy, radius, &rect, tilt),
        _ => doughnut(o, m, g, cx, cy, r_full),
    }
}

/// The values of a series as slice sizes (negatives and blanks are 0) and their total.
fn slices(values: &[Option<f64>], n: usize) -> (Vec<f64>, f64) {
    let v: Vec<f64> = (0..n)
        .map(|i| values.get(i).copied().flatten().unwrap_or(0.0).max(0.0))
        .collect();
    let t = v.iter().sum();
    (v, t)
}

fn point_fill(
    m: &ChartModel,
    g: &ChartGroup,
    s: &super::Series,
    pf: Option<&PointFmt>,
    i: usize,
) -> Fill {
    pf.and_then(|p| p.fill.clone()).unwrap_or_else(|| {
        if g.vary_colors || g.series.len() > 1 && g.kind == GroupKind::Doughnut {
            Fill::Solid(point_color(m, i, s.values.len().max(s.cats.len())))
        } else {
            s.fill
                .clone()
                .unwrap_or_else(|| Fill::Solid(point_color(m, 0, 1)))
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn pie(
    o: &mut Out,
    m: &ChartModel,
    g: &ChartGroup,
    cx: f64,
    cy: f64,
    radius: f64,
    rect: &Rect,
    tilt: Option<three_d::Tilt>,
) {
    let squash = tilt.map_or(1.0, |t| t.squash);
    let mut slices3: Vec<three_d::Slice> = Vec::new();
    let s = &g.series[0];
    let n = s.values.len().max(s.cats.len());
    let (vals, total) = slices(&s.values, n);
    if total <= 0.0 {
        return;
    }
    let map: std::collections::HashMap<usize, &PointFmt> =
        s.points.iter().map(|p| (p.idx, p)).collect();
    let mut a0 = g.first_slice_ang.rem_euclid(360.0);
    let mut reqs: Vec<(String, super::text::RStyle, LabelPos, f64, f64, f64, f64)> = Vec::new();
    for (i, v) in vals.iter().enumerate() {
        if *v <= 0.0 {
            continue;
        }
        let sweep = 360.0 * v / total;
        // (Counter-clockwise: the slice is the one that ends at the running angle.)
        let a_s = if g.counter_clockwise { a0 - sweep } else { a0 };
        let mid = a_s + sweep / 2.0;
        let pf = map.get(&i).copied();
        let exp = pf
            .and_then(|p| p.explosion)
            .or(s.explosion)
            .unwrap_or(0.0)
            .clamp(0.0, 400.0)
            / 100.0
            * radius;
        let (ox, oy) = if exp > 0.0 {
            let p = on_circle(0.0, 0.0, exp, mid);
            (p.0, p.1 * squash)
        } else {
            (0.0, 0.0)
        };
        let (px, py) = (cx + ox, cy + oy);
        let fill = point_fill(m, g, s, pf, i);
        let stroke = pf.and_then(|p| p.line.as_ref()).or(s.line.as_ref());
        let line = line_if_set(stroke, Rgba::WHITE, 9525.0)
            .or_else(|| stroke.is_none().then(|| style_outline(m)).flatten());
        if tilt.is_some() {
            slices3.push(three_d::Slice {
                a_s,
                sweep,
                cx: px,
                cy: py,
                fill: fill.clone(),
                line: line.clone(),
                exploded: exp > 0.0,
            });
        } else if sweep >= 359.99 {
            o.ellipse(px, py, radius, radius, &fill, line.as_ref());
        } else {
            let start = on_circle(px, py, radius, a_s);
            let cmds = [
                PathCmd::MoveTo(Pt::new(px, py)),
                PathCmd::LineTo(Pt::new(start.0, start.1)),
                PathCmd::ArcTo {
                    wr: radius,
                    hr: radius,
                    st_deg: a_s - 90.0,
                    sw_deg: sweep,
                },
                PathCmd::Close,
            ];
            o.path(
                &cmds,
                (px - radius, py - radius, px + radius, py + radius),
                &fill,
                line.as_ref(),
            );
        }
        if o.full() {
            return;
        }
        if let Some((text, st, pos)) = slice_label(m, g, s, i, *v, total, sweep, radius) {
            reqs.push((text, st, pos, px, py, radius, mid));
        }
        a0 += if g.counter_clockwise { -sweep } else { sweep };
    }
    if let Some(t) = tilt {
        three_d::draw_pie(o, slices3, radius, radius * t.squash, t.thick * radius);
        if o.full() {
            return;
        }
    }
    let mut outside: Vec<OutsideLabel> = Vec::new();
    for (text, st, pos, px, py, r, mid) in reqs {
        if is_outside(pos) {
            outside.push(OutsideLabel {
                text,
                st,
                cx: px,
                cy: py,
                r,
                mid,
                squash,
            });
        } else {
            place(o, &text, &st, pos, px, py, r, 0.0, mid, squash);
        }
    }
    place_outside(o, outside, rect);
}

/// A label outside its slice, waiting to be placed (see [`place_outside`]).
struct OutsideLabel {
    text: String,
    st: super::text::RStyle,
    /// The centre of the slice (moved by its explosion) and the radius.
    cx: f64,
    cy: f64,
    r: f64,
    /// The angle of the slice's bisector, degrees clockwise from 12 o'clock.
    mid: f64,
    /// Height of the pie's outline over its width (`1.0` for a flat pie, less for a tilted 3-D one).
    squash: f64,
}

/// A label that moved this far (px) from its natural place gets a leader line to its slice.
const LEADER_MIN_MOVE: f64 = 3.0;
/// A label that moved this far (px) sideways from its natural place gets a leader line too.
const LEADER_MIN_MOVE_X: f64 = 6.0;
/// The leader line of a moved label (Office draws them in a mid gray).
const LEADER_GRAY: u8 = 0x86;

/// Places the labels outside a pie in two columns, one on each side of it (the right one for
/// slices whose bisector points right of 12 o'clock, the left one for the others).
///
/// A label starts at its slice's bisector just outside the rim. On each side the labels are then
/// spread vertically ([`spread`]): the ones that would cover each other are stacked in a block
/// centred on where they wanted to be, and the blocks are kept inside the plot's height, so a
/// crowd of small slices fans out along the whole side instead of piling up at one end. Every
/// label then hugs the rim at its own height (it starts at the rim point of its nearest edge, so
/// it never covers the pie).
///
/// A label that moved gets a gray leader line from its slice's rim to the edge of its box that
/// faces the pie. To keep a leader line from ever passing through the text of another label, the
/// labels that a leader line reaches past (those whose boxes lie within the height the line spans)
/// line up in one column together with the labels that moved, at the outermost position any of
/// them hugs: every box of such a group then lies outside every line of the group.
fn place_outside(o: &mut Out, labels: Vec<OutsideLabel>, rect: &Rect) {
    struct Placed {
        l: OutsideLabel,
        right: bool,
        /// Where the label's text starts (the end of the text on the left side).
        x: f64,
        x_nat: f64,
        y_nat: f64,
        y: f64,
        w: f64,
        h: f64,
        /// The point on the slice's rim the leader line starts from.
        rim: (f64, f64),
    }
    impl Placed {
        fn moved(&self) -> bool {
            (self.y - self.y_nat).abs() > LEADER_MIN_MOVE
                || (self.x - self.x_nat).abs() > LEADER_MIN_MOVE_X
        }
        /// The vertical extent of the label and its leader line.
        fn span(&self) -> (f64, f64) {
            let (a, b) = (self.y - self.h / 2.0, self.y + self.h / 2.0);
            if self.moved() {
                (a.min(self.rim.1), b.max(self.rim.1))
            } else {
                (a, b)
            }
        }
    }
    let mut items: Vec<Placed> = Vec::with_capacity(labels.len());
    for l in labels {
        let gap = l.r + 4.0;
        let (ax, ay) = on_ellipse(l.cx, l.cy, gap, gap * l.squash, l.mid);
        let dy = -l.mid.to_radians().cos();
        let (tw, th) = super::text::measure(&l.st, &l.text);
        // The vertical centre of the label as `place` would anchor it.
        let yc = if dy < -0.25 {
            ay - th / 2.0
        } else if dy > 0.25 {
            ay + th / 2.0
        } else {
            ay
        };
        let rim = on_ellipse(l.cx, l.cy, l.r, l.r * l.squash, l.mid);
        items.push(Placed {
            right: l.mid.to_radians().sin() >= 0.0,
            x: ax,
            x_nat: ax,
            y_nat: yc,
            y: yc,
            w: tw + 6.0,
            h: th,
            rim,
            l,
        });
    }
    let (top, bottom) = (rect.y + 1.0, rect.y + rect.h - 1.0);
    for side in [true, false] {
        let mut idx: Vec<usize> = (0..items.len())
            .filter(|&i| items[i].right == side)
            .collect();
        idx.sort_by(|&a, &b| items[a].y_nat.total_cmp(&items[b].y_nat));
        let want: Vec<f64> = idx.iter().map(|&i| items[i].y_nat).collect();
        let hs: Vec<f64> = idx.iter().map(|&i| items[i].h).collect();
        for (k, y) in spread(&want, &hs, top, bottom).into_iter().enumerate() {
            items[idx[k]].y = y;
        }
    }
    // Hug the rim: the box starts where the rim is at its edge nearest to the pie's centre line.
    for it in &mut items {
        let rim = it.l.r + 4.0;
        let ry = rim * it.l.squash;
        let (t, b) = (it.y - it.h / 2.0 - it.l.cy, it.y + it.h / 2.0 - it.l.cy);
        let d = if t <= 0.0 && b >= 0.0 {
            0.0
        } else {
            t.abs().min(b.abs())
        };
        let reach = if ry > 0.0 && d < ry {
            rim * (1.0 - (d / ry) * (d / ry)).sqrt()
        } else {
            0.0
        };
        it.x = it.l.cx + if it.right { reach } else { -reach };
        // Unmoved labels keep their natural anchor (the two agree to within a pixel or two).
        if !it.moved() {
            it.x = it.x_nat;
        }
    }
    // Line up the labels a leader line reaches past with the labels that moved.
    for side in [true, false] {
        for _ in 0..4 {
            let mut spans: Vec<(f64, f64)> = items
                .iter()
                .filter(|it| it.right == side && it.moved())
                .map(Placed::span)
                .collect();
            if spans.is_empty() {
                break;
            }
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut groups: Vec<(f64, f64)> = Vec::new();
            for sp in spans {
                match groups.last_mut() {
                    Some(g) if sp.0 <= g.1 => g.1 = g.1.max(sp.1),
                    _ => groups.push(sp),
                }
            }
            let mut changed = false;
            for g in groups {
                let members: Vec<usize> = (0..items.len())
                    .filter(|&i| {
                        let it = &items[i];
                        it.right == side && it.y + it.h / 2.0 > g.0 && it.y - it.h / 2.0 < g.1
                    })
                    .collect();
                let col = members
                    .iter()
                    .map(|&i| items[i].x)
                    .fold(None, |a: Option<f64>, x| {
                        Some(match a {
                            Some(a) if side => a.max(x),
                            Some(a) => a.min(x),
                            None => x,
                        })
                    });
                if let Some(col) = col {
                    for i in members {
                        if (items[i].x - col).abs() > 1e-9 {
                            items[i].x = col;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    // Keep the text inside the plot's width when there is room.
    let (lo, hi) = (rect.x, rect.x + rect.w);
    for it in &mut items {
        if it.right && it.x + it.w > hi {
            it.x = (hi - it.w).max(it.l.cx);
        } else if !it.right && it.x - it.w < lo {
            it.x = (lo + it.w).min(it.l.cx);
        }
    }
    let leader = Line::solid(9525.0, Rgba::rgb(LEADER_GRAY, LEADER_GRAY, LEADER_GRAY));
    for it in &items {
        let ha = if it.right {
            HAlign::Left
        } else {
            HAlign::Right
        };
        if it.moved() {
            // To the edge of the label's box that faces the pie.
            let lx = if it.right { it.x - 1.0 } else { it.x + 1.0 };
            o.seg(it.rim.0, it.rim.1, lx, it.y, &leader);
        }
        o.text(it.x, it.y, ha, VAlign::Middle, &it.l.text, &it.l.st, 0.0);
    }
}

/// Centres (in the order of `want`, which is ascending) of boxes of heights `hs` that are as near
/// to `want` as they can be without overlapping and inside `top..=bottom`. Boxes that collide form
/// a block that is stacked in order and placed so that its boxes are, on average, as near as
/// possible to where they wanted to be (blocks that run into each other merge); a block is kept
/// inside the limits (when the boxes are taller than the room they overflow at the bottom).
pub(super) fn spread(want: &[f64], hs: &[f64], top: f64, bottom: f64) -> Vec<f64> {
    struct Block {
        first: usize,
        end: usize,
        top: f64,
        h: f64,
    }
    let n = want.len().min(hs.len());
    let mut blocks: Vec<Block> = Vec::new();
    let fit = |b: &mut Block, want: &[f64], hs: &[f64]| {
        let mut off = 0.0;
        let mut sum = 0.0;
        for k in b.first..b.end {
            sum += want[k] - hs[k] / 2.0 - off;
            off += hs[k];
        }
        let mean = sum / (b.end - b.first) as f64;
        b.top = mean.min(bottom - b.h).max(top);
    };
    for i in 0..n {
        let mut b = Block {
            first: i,
            end: i + 1,
            top: 0.0,
            h: hs[i],
        };
        fit(&mut b, want, hs);
        blocks.push(b);
        while blocks.len() >= 2 {
            let k = blocks.len();
            if blocks[k - 2].top + blocks[k - 2].h <= blocks[k - 1].top + 1e-9 {
                break;
            }
            let Some(last) = blocks.pop() else { break };
            if let Some(prev) = blocks.last_mut() {
                prev.end = last.end;
                prev.h += last.h;
                fit(prev, want, hs);
            }
        }
    }
    let mut out = vec![0.0; n];
    for b in &blocks {
        let mut y = b.top;
        for k in b.first..b.end {
            out[k] = y + hs[k] / 2.0;
            y += hs[k];
        }
    }
    out
}

fn is_outside(p: LabelPos) -> bool {
    matches!(
        p,
        LabelPos::OutsideEnd
            | LabelPos::BestFit
            | LabelPos::Left
            | LabelPos::Right
            | LabelPos::Above
            | LabelPos::Below
    )
}

/// The label of slice `i` (text, style, position), if it has one. Best fit is inside when the text
/// fits the slice at 0.65 of the radius, outside otherwise.
#[allow(clippy::too_many_arguments)]
fn slice_label(
    m: &ChartModel,
    g: &ChartGroup,
    s: &super::Series,
    i: usize,
    v: f64,
    total: f64,
    sweep: f64,
    radius: f64,
) -> Option<(String, super::text::RStyle, LabelPos)> {
    let dl = labels::effective(g.labels.as_ref(), s.labels.as_ref(), i)?;
    let info = Info {
        cat: s.cats.get(i).map(String::as_str),
        series: s.name.as_deref(),
        value: Some(v),
        percent: Some(v / total),
        source_fmt: s.format_code.as_deref(),
        ja: m.locale_ja,
    };
    let text = labels::text(&dl, &info);
    if text.is_empty() {
        return None;
    }
    let st = resolve(m, &dl.style, TEXT_PT, false);
    let pos = match dl.pos {
        Some(LabelPos::BestFit) | None => {
            let chord = 2.0 * 0.65 * radius * (sweep.to_radians() / 2.0).sin();
            let widest = text
                .split('\n')
                .map(|l| line_width(&st, l))
                .fold(0.0, f64::max);
            if sweep < 180.0 && widest < chord * 0.95 && st.line_h() < 0.5 * radius {
                LabelPos::Center
            } else {
                LabelPos::OutsideEnd
            }
        }
        Some(p) => p,
    };
    Some((text, st, pos))
}

/// Places a label of a slice of the circle (centre, radius, ring) at angle `mid`.
#[allow(clippy::too_many_arguments)]
fn place(
    o: &mut Out,
    text: &str,
    st: &super::text::RStyle,
    pos: LabelPos,
    cx: f64,
    cy: f64,
    r: f64,
    r_in: f64,
    mid: f64,
    squash: f64,
) {
    // The point at radius `rr` of the slice's bisector (on the tilted ellipse for a 3-D pie).
    let on_circle = |cx: f64, cy: f64, rr: f64, mid: f64| on_ellipse(cx, cy, rr, rr * squash, mid);
    match pos {
        LabelPos::OutsideEnd
        | LabelPos::BestFit
        | LabelPos::Left
        | LabelPos::Right
        | LabelPos::Above
        | LabelPos::Below => {
            let (x, y) = on_circle(cx, cy, r + 4.0, mid);
            let dx = (mid.to_radians()).sin();
            let dy = -(mid.to_radians()).cos();
            let ha = if dx > 0.25 {
                HAlign::Left
            } else if dx < -0.25 {
                HAlign::Right
            } else {
                HAlign::Center
            };
            let va = if dy < -0.25 {
                VAlign::Bottom
            } else if dy > 0.25 {
                VAlign::Top
            } else {
                VAlign::Middle
            };
            o.text(x, y, ha, va, text, st, 0.0);
        }
        LabelPos::InsideEnd => {
            let (x, y) = on_circle(cx, cy, r * 0.85, mid);
            o.text(x, y, HAlign::Center, VAlign::Middle, text, st, 0.0);
        }
        LabelPos::InsideBase => {
            let (x, y) = on_circle(cx, cy, (r_in + (r - r_in) * 0.2).max(r * 0.2), mid);
            o.text(x, y, HAlign::Center, VAlign::Middle, text, st, 0.0);
        }
        LabelPos::Center => {
            let (x, y) = on_circle(cx, cy, (r_in + r) / 2.0, mid);
            let rr = if r_in == 0.0 {
                r * 0.65
            } else {
                (r_in + r) / 2.0
            };
            let (x, y) = if r_in == 0.0 {
                on_circle(cx, cy, rr, mid)
            } else {
                (x, y)
            };
            o.text(x, y, HAlign::Center, VAlign::Middle, text, st, 0.0);
        }
    }
}

fn doughnut(o: &mut Out, m: &ChartModel, g: &ChartGroup, cx: f64, cy: f64, radius: f64) {
    let rings = g.series.len();
    let hole = radius * g.hole_size / 100.0;
    let thick = (radius - hole) / rings as f64;
    for (k, s) in g.series.iter().enumerate() {
        let n = s.values.len().max(s.cats.len());
        let (vals, total) = slices(&s.values, n);
        if total <= 0.0 {
            continue;
        }
        let r_in = hole + k as f64 * thick;
        let r_out = r_in + thick;
        let map: std::collections::HashMap<usize, &PointFmt> =
            s.points.iter().map(|p| (p.idx, p)).collect();
        let mut a0 = g.first_slice_ang.rem_euclid(360.0);
        for (i, v) in vals.iter().enumerate() {
            if *v <= 0.0 {
                continue;
            }
            let sweep = 360.0 * v / total;
            let a_s = if g.counter_clockwise { a0 - sweep } else { a0 };
            let pf = map.get(&i).copied();
            let fill = point_fill(m, g, s, pf, i);
            let stroke = pf.and_then(|p| p.line.as_ref()).or(s.line.as_ref());
            let line = line_if_set(stroke, Rgba::WHITE, 9525.0)
                .or_else(|| stroke.is_none().then(|| style_outline(m)).flatten());
            if sweep >= 359.99 {
                // A full ring: two circles, the hole cut out by an even-odd-free trick: draw the
                // outer disc, then the inner one in the plot's colour is not possible, so draw two
                // half rings.
                ring_slice(o, cx, cy, r_in, r_out, a_s, 180.0, &fill, line.as_ref());
                ring_slice(
                    o,
                    cx,
                    cy,
                    r_in,
                    r_out,
                    a_s + 180.0,
                    179.99,
                    &fill,
                    line.as_ref(),
                );
            } else {
                ring_slice(o, cx, cy, r_in, r_out, a_s, sweep, &fill, line.as_ref());
            }
            if o.full() {
                return;
            }
            if let Some(dl) = labels::effective(g.labels.as_ref(), s.labels.as_ref(), i) {
                let info = Info {
                    cat: s.cats.get(i).map(String::as_str),
                    series: s.name.as_deref(),
                    value: Some(*v),
                    percent: Some(v / total),
                    source_fmt: s.format_code.as_deref(),
                    ja: m.locale_ja,
                };
                let text = labels::text(&dl, &info);
                if !text.is_empty() {
                    let st = resolve(m, &dl.style, TEXT_PT, false);
                    place(
                        o,
                        &text,
                        &st,
                        LabelPos::Center,
                        cx,
                        cy,
                        r_out,
                        r_in,
                        a_s + sweep / 2.0,
                        1.0,
                    );
                }
            }
            a0 += if g.counter_clockwise { -sweep } else { sweep };
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn ring_slice(
    o: &mut Out,
    cx: f64,
    cy: f64,
    r_in: f64,
    r_out: f64,
    a0: f64,
    sweep: f64,
    fill: &Fill,
    line: Option<&Line>,
) {
    let s = on_circle(cx, cy, r_out, a0);
    let e_in = on_circle(cx, cy, r_in, a0 + sweep);
    let cmds = [
        PathCmd::MoveTo(Pt::new(s.0, s.1)),
        PathCmd::ArcTo {
            wr: r_out,
            hr: r_out,
            st_deg: a0 - 90.0,
            sw_deg: sweep,
        },
        PathCmd::LineTo(Pt::new(e_in.0, e_in.1)),
        PathCmd::ArcTo {
            wr: r_in,
            hr: r_in,
            st_deg: a0 + sweep - 90.0,
            sw_deg: -sweep,
        },
        PathCmd::Close,
    ];
    o.path(
        &cmds,
        (cx - r_out, cy - r_out, cx + r_out, cy + r_out),
        fill,
        line,
    );
}
