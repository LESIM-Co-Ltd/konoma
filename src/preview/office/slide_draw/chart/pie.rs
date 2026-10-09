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
use super::shapes::{line_if_set, on_circle, Out};
use super::text::{line_width, resolve, HAlign, VAlign};
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
    let r_full = (rect.w.min(rect.h) / 2.0 - margin).max(4.0);
    let radius = r_full / (1.0 + max_exp / 100.0);
    let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);

    match g.kind {
        GroupKind::Pie => pie(o, m, g, cx, cy, radius, &rect),
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

fn pie(o: &mut Out, m: &ChartModel, g: &ChartGroup, cx: f64, cy: f64, radius: f64, rect: &Rect) {
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
        let mid = a0 + sweep / 2.0;
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
            (p.0, p.1)
        } else {
            (0.0, 0.0)
        };
        let (px, py) = (cx + ox, cy + oy);
        let fill = point_fill(m, g, s, pf, i);
        let stroke = pf.and_then(|p| p.line.as_ref()).or(s.line.as_ref());
        let line = line_if_set(stroke, Rgba::WHITE, 9525.0)
            .or_else(|| stroke.is_none().then(|| style_outline(m)).flatten());
        if sweep >= 359.99 {
            o.ellipse(px, py, radius, radius, &fill, line.as_ref());
        } else {
            let start = on_circle(px, py, radius, a0);
            let cmds = [
                PathCmd::MoveTo(Pt::new(px, py)),
                PathCmd::LineTo(Pt::new(start.0, start.1)),
                PathCmd::ArcTo {
                    wr: radius,
                    hr: radius,
                    st_deg: a0 - 90.0,
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
        a0 += sweep;
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
            });
        } else {
            place(o, &text, &st, pos, px, py, r, 0.0, mid);
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
}

/// A label that moved this far (px) from its natural place gets a leader line to its slice.
const LEADER_MIN_MOVE: f64 = 3.0;
/// The leader line of a moved label (Office draws them in a mid gray).
const LEADER_GRAY: u8 = 0x86;

/// Places the labels outside a pie like Office's best fit: each one starts at its slice's
/// bisector just outside the rim; on each side of the pie the labels are then pushed apart
/// vertically so that none covers another (and kept inside the chart's box), and a label that had to
/// move gets a leader line from the slice's rim to the label.
fn place_outside(o: &mut Out, labels: Vec<OutsideLabel>, rect: &Rect) {
    struct Placed {
        l: OutsideLabel,
        ha: HAlign,
        x: f64,
        y_nat: f64,
        y: f64,
        w: f64,
        h: f64,
        right: bool,
    }
    let mut items: Vec<Placed> = Vec::with_capacity(labels.len());
    for l in labels {
        let (x, y) = on_circle(l.cx, l.cy, l.r + 4.0, l.mid);
        let dx = l.mid.to_radians().sin();
        let dy = -l.mid.to_radians().cos();
        let ha = if dx > 0.25 {
            HAlign::Left
        } else if dx < -0.25 {
            HAlign::Right
        } else {
            HAlign::Center
        };
        let (tw, th) = super::text::measure(&l.st, &l.text);
        let rim = l.r + 4.0;
        // The vertical centre of the label as `place` would anchor it.
        let mut yc = if dy < -0.25 {
            y - th / 2.0
        } else if dy > 0.25 {
            y + th / 2.0
        } else {
            y
        };
        // A label centred above (below) the pie must clear the rim along its whole width, not
        // only at its anchor: the highest (lowest) point of the rim under it is the limit.
        if ha == HAlign::Center && dy.abs() > 0.25 {
            let (x0, x1) = (x - (tw + 6.0) / 2.0 - l.cx, x + (tw + 6.0) / 2.0 - l.cx);
            let d = if x0 <= 0.0 && x1 >= 0.0 {
                0.0
            } else {
                x0.abs().min(x1.abs())
            };
            let edge = (rim * rim - d * d).max(0.0).sqrt();
            yc = if dy < 0.0 {
                yc.min(l.cy - edge - th / 2.0)
            } else {
                yc.max(l.cy + edge + th / 2.0)
            };
        }
        items.push(Placed {
            ha,
            x,
            y_nat: yc,
            y: yc,
            w: tw + 6.0,
            h: th,
            right: dx >= 0.0,
            l,
        });
    }
    let (top, bottom) = (rect.y + 1.0, rect.y + rect.h - 1.0);
    for side in [true, false] {
        let mut idx: Vec<usize> = (0..items.len())
            .filter(|&i| items[i].right == side && items[i].ha != HAlign::Center)
            .collect();
        idx.sort_by(|&a, &b| items[a].y.total_cmp(&items[b].y));
        // Downwards: no label starts above the end of the one before it.
        let mut edge = top;
        for &i in &idx {
            let h = items[i].h;
            items[i].y = items[i].y.max(edge + h / 2.0);
            edge = items[i].y + h / 2.0;
        }
        // Upwards from the bottom edge, when the last one went past it.
        let mut edge = bottom;
        for &i in idx.iter().rev() {
            let h = items[i].h;
            items[i].y = items[i].y.min(edge - h / 2.0);
            edge = items[i].y - h / 2.0;
        }
    }
    // The labels centred above or below the pie (nearly straight up or down from the centre)
    // that overlap in width are stacked outwards, away from the pie.
    let mut centred: Vec<usize> = (0..items.len())
        .filter(|&i| items[i].ha == HAlign::Center)
        .collect();
    centred.sort_by(|&a, &b| items[a].x.total_cmp(&items[b].x));
    for (k, &i) in centred.iter().enumerate() {
        let up = items[i].y < items[i].l.cy;
        for &j in &centred[..k] {
            let (a, b) = (&items[i], &items[j]);
            let wide = (a.x - b.x).abs() < (a.w + b.w) / 2.0 - 0.5;
            let tall = (a.y - b.y).abs() < (a.h + b.h) / 2.0 - 0.5;
            if wide && tall && (items[j].y < items[j].l.cy) == up {
                let h = (a.h + b.h) / 2.0;
                items[i].y = if up { b.y - h } else { b.y + h };
            }
        }
    }
    let leader = Line::solid(9525.0, Rgba::rgb(LEADER_GRAY, LEADER_GRAY, LEADER_GRAY));
    for it in &mut items {
        // A label that moved along the rim keeps clear of the pie: it sits at least where the rim
        // is at the nearest point of its box.
        if (it.y - it.y_nat).abs() > LEADER_MIN_MOVE && it.ha != HAlign::Center {
            let rim = it.l.r + 4.0;
            let (top, bot) = (it.y - it.h / 2.0 - it.l.cy, it.y + it.h / 2.0 - it.l.cy);
            let d = if top <= 0.0 && bot >= 0.0 {
                0.0
            } else {
                top.abs().min(bot.abs())
            };
            let reach = (rim * rim - d * d).max(0.0).sqrt();
            let from_centre = (it.x - it.l.cx).abs().max(reach);
            it.x = it.l.cx + if it.right { from_centre } else { -from_centre };
        }
    }
    for it in &items {
        if (it.y - it.y_nat).abs() > LEADER_MIN_MOVE {
            let (ex, ey) = on_circle(it.l.cx, it.l.cy, it.l.r, it.l.mid);
            // To the edge of the label's box that faces the pie.
            let (lx, ly) = match it.ha {
                HAlign::Left => (it.x - 1.0, it.y),
                HAlign::Right => (it.x + 1.0, it.y),
                HAlign::Center => (
                    it.x,
                    if it.y > it.l.cy {
                        it.y - it.h / 2.0
                    } else {
                        it.y + it.h / 2.0
                    },
                ),
            };
            o.seg(ex, ey, lx, ly, &leader);
        }
        o.text(it.x, it.y, it.ha, VAlign::Middle, &it.l.text, &it.l.st, 0.0);
    }
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
) {
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
            let pf = map.get(&i).copied();
            let fill = point_fill(m, g, s, pf, i);
            let stroke = pf.and_then(|p| p.line.as_ref()).or(s.line.as_ref());
            let line = line_if_set(stroke, Rgba::WHITE, 9525.0)
                .or_else(|| stroke.is_none().then(|| style_outline(m)).flatten());
            if sweep >= 359.99 {
                // A full ring: two circles, the hole cut out by an even-odd-free trick: draw the
                // outer disc, then the inner one in the plot's colour is not possible, so draw two
                // half rings.
                ring_slice(o, cx, cy, r_in, r_out, a0, 180.0, &fill, line.as_ref());
                ring_slice(
                    o,
                    cx,
                    cy,
                    r_in,
                    r_out,
                    a0 + 180.0,
                    179.99,
                    &fill,
                    line.as_ref(),
                );
            } else {
                ring_slice(o, cx, cy, r_in, r_out, a0, sweep, &fill, line.as_ref());
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
                        a0 + sweep / 2.0,
                    );
                }
            }
            a0 += sweep;
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
