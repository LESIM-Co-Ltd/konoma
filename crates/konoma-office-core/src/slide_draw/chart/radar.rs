//! Radar charts: the categories are spokes, clockwise from 12 o'clock; the value axis runs along
//! the first spoke. Grid polygons are drawn when the value axis has major gridlines. Blank points
//! are zero.

use super::super::model::*;
use super::labels::{self, Info};
use super::layout::{
    draw_marker, marker_of, series_color, series_fill, Rect, AXIS_COLOR, AXIS_W, TEXT_PT,
};
use super::scale::{Opts, Scale};
use super::shapes::{line_of, on_circle, Out};
use super::text::{line_width, measure, resolve, HAlign, VAlign};
use super::{ChartGroup, ChartModel, GroupKind, LabelPos, RadarStyle, MAX_CATEGORIES};

pub(super) fn draw(o: &mut Out, m: &ChartModel, rect: Rect) {
    let groups: Vec<(&ChartGroup, usize)> = {
        let mut ord = 0;
        m.groups
            .iter()
            .map(|g| {
                let r = (g, ord);
                ord += g.series.len();
                r
            })
            .filter(|(g, _)| g.kind == GroupKind::Radar)
            .collect()
    };
    let Some(&(g0, _)) = groups.first() else {
        return;
    };
    let n = groups
        .iter()
        .flat_map(|(g, _)| g.series.iter())
        .map(|s| s.cats.len().max(s.values.len()))
        .max()
        .unwrap_or(0)
        .min(MAX_CATEGORIES);
    if n < 3 {
        return;
    }
    let cats: Vec<String> = groups
        .iter()
        .flat_map(|(g, _)| g.series.iter())
        .find(|s| !s.cats.is_empty())
        .map(|s| {
            (0..n)
                .map(|i| {
                    s.cats
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| (i + 1).to_string())
                })
                .collect()
        })
        .unwrap_or_else(|| (1..=n).map(|i| i.to_string()).collect());
    let val_axis = g0
        .axis_ids
        .get(1)
        .and_then(|id| m.axes.iter().find(|a| a.id == *id))
        .or_else(|| m.axes.iter().find(|a| a.kind == super::AxisKind::Val));
    let cat_style = resolve(
        m,
        &m.axes
            .iter()
            .find(|a| a.kind == super::AxisKind::Cat)
            .map(|a| a.text.clone())
            .unwrap_or_default(),
        TEXT_PT,
        false,
    );
    let widest = cats
        .iter()
        .map(|c| line_width(&cat_style, c))
        .fold(0.0, f64::max);
    let lh = cat_style.line_h();
    let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let r = (rect.w / 2.0 - widest - 8.0)
        .min(rect.h / 2.0 - lh - 6.0)
        .max(8.0);

    // The scale.
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    for (g, _) in &groups {
        for v in g.series.iter().flat_map(|s| s.values.iter().flatten()) {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
    }
    let span = (lo <= hi).then_some((lo, hi));
    let sc = Scale::new(
        val_axis,
        span,
        Opts {
            len_px: r,
            horizontal: false,
            percent: false,
        },
    );
    let ang = |i: usize| 360.0 * i as f64 / n as f64;

    // Grid polygons and spokes.
    let grid = val_axis.and_then(|a| a.major_grid.as_ref());
    let grid_line = grid.and_then(|g| line_of(Some(g), AXIS_COLOR, AXIS_W));
    let ticks = sc.major_ticks();
    if let Some(l) = &grid_line {
        for v in ticks.iter().skip(1) {
            let rr = r * sc.frac(*v).clamp(0.0, 1.0);
            let pts: Vec<(f64, f64)> = (0..n).map(|i| on_circle(cx, cy, rr, ang(i))).collect();
            o.poly(&pts, true, &Fill::None, Some(l));
        }
    }
    let spoke = Line::solid(AXIS_W, AXIS_COLOR);
    for i in 0..n {
        let p = on_circle(cx, cy, r, ang(i));
        o.seg(cx, cy, p.0, p.1, &spoke);
        if o.full() {
            return;
        }
    }

    // Series.
    let mut reqs: Vec<(String, super::text::RStyle, f64, f64)> = Vec::new();
    for (g, ord0) in &groups {
        let style = g.radar_style;
        for (j, s) in g.series.iter().enumerate() {
            let ord = ord0 + j;
            let pts: Vec<(f64, f64)> = (0..n)
                .map(|i| {
                    let v = s.values.get(i).copied().flatten().unwrap_or(0.0);
                    on_circle(cx, cy, r * sc.frac(v).clamp(0.0, 1.0), ang(i))
                })
                .collect();
            let color = s
                .line
                .as_ref()
                .and_then(|l| l.color)
                .unwrap_or_else(|| series_color(m, ord));
            if style == RadarStyle::Filled {
                let fill = series_fill(m, s, ord);
                let ln = super::shapes::line_if_set(s.line.as_ref(), color, 9525.0);
                o.poly(&pts, true, &fill, ln.as_ref());
            } else if let Some(ln) = line_of(s.line.as_ref(), color, 28575.0) {
                o.poly(&pts, true, &Fill::None, Some(&ln));
            }
            if style == RadarStyle::Marker {
                if let Some((sym, size, c)) = marker_of(m, s, None, ord, true) {
                    for p in &pts {
                        if o.full() {
                            return;
                        }
                        draw_marker(o, p.0, p.1, sym, size, c, s.marker.as_ref());
                    }
                }
            }
            for (i, p) in pts.iter().enumerate() {
                if let Some(dl) = labels::effective(g.labels.as_ref(), s.labels.as_ref(), i) {
                    let info = Info {
                        cat: cats.get(i).map(String::as_str),
                        series: s.name.as_deref(),
                        value: s.values.get(i).copied().flatten(),
                        percent: None,
                        source_fmt: s.format_code.as_deref(),
                        ja: m.locale_ja,
                    };
                    let t = labels::text(&dl, &info);
                    if !t.is_empty() {
                        reqs.push((t, resolve(m, &dl.style, TEXT_PT, false), p.0, p.1));
                    }
                }
            }
        }
    }
    for (t, st, x, y) in reqs {
        labels::at_point(o, &st, &t, x, y, 3.0, LabelPos::Right);
    }

    // Value labels along the first spoke, category labels around.
    if val_axis.is_some_and(|a| !a.deleted && a.tick_label_pos != super::TickLabelPos::None) {
        let st = resolve(
            m,
            &val_axis.map(|a| a.text.clone()).unwrap_or_default(),
            TEXT_PT,
            false,
        );
        let code = val_axis.and_then(|a| match &a.num_fmt {
            Some((c, false)) => Some(c.clone()),
            _ => None,
        });
        let code = code.or_else(|| {
            groups
                .iter()
                .flat_map(|(g, _)| g.series.iter())
                .find_map(|s| s.format_code.clone())
        });
        for v in &ticks {
            let y = cy - r * sc.frac(*v).clamp(0.0, 1.0);
            let t = labels::format_number(code.as_deref(), *v, m.locale_ja);
            o.text(cx - 4.0, y, HAlign::Right, VAlign::Middle, &t, &st, 0.0);
        }
    }
    for (i, c) in cats.iter().enumerate() {
        let a = ang(i);
        let (x, y) = on_circle(cx, cy, r + 6.0, a);
        let dx = a.to_radians().sin();
        let dy = -a.to_radians().cos();
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
        let _ = measure(&cat_style, c);
        o.text(x, y, ha, va, c, &cat_style, 0.0);
        if o.full() {
            return;
        }
    }
}
