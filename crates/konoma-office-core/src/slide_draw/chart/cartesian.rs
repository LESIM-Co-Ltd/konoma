//! Charts drawn against a pair of axes: column / bar, line, area, scatter and bubble charts, with
//! everything around them (axes, ticks, tick labels, gridlines, axis titles, data labels).
//!
//! The geometry is described on two axes of its own: the *category* axis (categories, or the x
//! values of a scatter chart) and the *value* axis. Positions on an axis are fractions `0..1` in the
//! drawing direction (left to right, bottom to top; a column chart's category axis is horizontal, a
//! bar chart's vertical). The plot rectangle is found by iterating: guess the space the labels and
//! titles take, scale the axes for the resulting plot size, measure the labels again, repeat.

use std::collections::HashMap;

use super::super::model::*;
use super::labels::{self, Info};
use super::layout::{
    apply_manual, draw_marker, is_cartesian, marker_of, point_color, series_color, series_fill,
    Rect, AXIS_COLOR, AXIS_W, TEXT_PT,
};
use super::scale::{LibreOpts, Opts, Scale};
use super::shapes::{line_if_set, line_of, Out};
use super::text::{line_width, measure, resolve, HAlign, RStyle, VAlign};
use super::three_d::{self, Depth};
use super::{
    Axis, AxisKind, AxisPos, BarDir, ChartGroup, ChartModel, Crosses, DispBlanks, GroupKind,
    Grouping, LabelPos, ManualLayout, PointFmt, ScatterStyle, Series, TickLabelPos, TickMark,
    MAX_CATEGORIES,
};

/// Length of a tick mark, px.
const TICK: f64 = 4.0;
/// Gap between a tick label and the tick / axis, px.
const LABEL_GAP: f64 = 3.0;
/// Gap between the labels and the axis title, px.
const TITLE_GAP: f64 = 4.0;
/// Most category labels drawn on one axis.
const MAX_CAT_LABELS: usize = 600;

/// One axis of the plot: categories or numbers.
#[derive(Debug, Clone)]
enum Dim {
    Cat {
        n: usize,
        between: bool,
        reversed: bool,
    },
    Num(Scale),
}

impl Dim {
    /// Fraction (drawing direction) of the centre of category `i`, or of the point's position.
    fn cat_frac(&self, i: usize) -> f64 {
        match self {
            Dim::Cat { n, between, .. } => self.flip(slot_pos(*n, *between, i)),
            Dim::Num(_) => 0.0,
        }
    }

    /// Fraction of a position `p` (0..1 from the first to the last category).
    fn flip(&self, p: f64) -> f64 {
        match self {
            Dim::Cat { reversed: true, .. } => 1.0 - p,
            _ => p,
        }
    }

    /// Where an axis that crosses this one with `c` sits, as a fraction.
    fn cross_frac(&self, c: Crosses) -> f64 {
        match self {
            Dim::Cat { n, .. } => match c {
                Crosses::AutoZero | Crosses::Min => self.flip(0.0),
                Crosses::Max => self.flip(1.0),
                Crosses::At(v) => self.flip(((v - 1.0) / (*n).max(1) as f64).clamp(0.0, 1.0)),
            },
            Dim::Num(s) => {
                let v = match c {
                    Crosses::AutoZero => {
                        if s.log.is_some() {
                            s.min
                        } else {
                            0.0f64.clamp(s.min, s.max)
                        }
                    }
                    Crosses::Min => s.min,
                    Crosses::Max => s.max,
                    Crosses::At(v) => v,
                };
                s.frac(v)
            }
        }
    }

    fn reversed(&self) -> bool {
        match self {
            Dim::Cat { reversed, .. } => *reversed,
            Dim::Num(s) => s.reversed,
        }
    }
}

/// Position (0..1) of the centre of slot / point `i` of `n`.
fn slot_pos(n: usize, between: bool, i: usize) -> f64 {
    if between {
        (i as f64 + 0.5) / n.max(1) as f64
    } else if n <= 1 {
        0.5
    } else {
        i as f64 / (n - 1) as f64
    }
}

/// Which side of the plot something is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Bottom,
    Top,
    Left,
    Right,
}

/// A series with its place in the chart.
#[derive(Clone, Copy)]
struct GroupRef<'a> {
    g: &'a ChartGroup,
    /// Ordinal of the group's first series among all series of the chart (colours).
    ord0: usize,
}

/// Everything about one axis pair.
struct Pair<'a> {
    cat_axis: Option<&'a Axis>,
    val_axis: Option<&'a Axis>,
    groups: Vec<GroupRef<'a>>,
    /// The category axis is horizontal (columns, lines, scatter); vertical for bar charts.
    cat_h: bool,
    cat: Dim,
    val: Dim,
    /// The value axis' scale (the same as in `val`).
    vs: Scale,
    percent: bool,
    cat_labels: Vec<String>,
    source_fmt: Option<String>,
}

/// The drawn form of one axis.
struct AxisPlan {
    horizontal: bool,
    /// Position of the axis line across the plot, as a fraction along its cross axis.
    line_f: f64,
    /// Where the tick labels are, `None` for no labels.
    label_side: Option<Side>,
    /// The labels sit outside the plot (they take room).
    outside: bool,
    labels: Vec<(f64, String)>,
    label_rot: f64,
    label_style: RStyle,
    label_w: f64,
    label_h: f64,
    major: Vec<f64>,
    minor: Vec<f64>,
    major_tick: TickMark,
    minor_tick: TickMark,
    title: Option<(String, RStyle, f64)>,
    title_side: Side,
    /// Space the axis needs outside the plot on its label side / title side.
    need_label: f64,
    need_title: f64,
    /// Half the first / last label's width (horizontal axes), which overhang the plot.
    overhang: (f64, f64),
}

#[derive(Debug, Clone, Copy, Default)]
struct Margins {
    l: f64,
    t: f64,
    r: f64,
    b: f64,
}

pub(super) fn draw(o: &mut Out, m: &ChartModel, avail: Rect, manual: Option<ManualLayout>) {
    let (w, h) = (o.w, o.h);
    // Group refs with ordinals.
    let mut refs: Vec<GroupRef> = Vec::new();
    let mut ord = 0usize;
    for g in m.groups.iter().filter(|g| is_cartesian(g)) {
        refs.push(GroupRef { g, ord0: ord });
        ord += g.series.len();
    }
    if refs.is_empty() {
        return;
    }

    // First guess of the margins, then iterate.
    let mut mg = Margins {
        l: 40.0,
        t: 8.0,
        r: 8.0,
        b: 28.0,
    };
    // The plot rectangle (the front plane of a 3-D plot, whose depth takes room at the top and on
    // one side) and the depth, when the plot is a 3-D one.
    let rect_of = |mg: &Margins| -> (Rect, Option<Depth>) {
        let mut base = Rect {
            x: avail.x + mg.l,
            y: avail.y + mg.t,
            w: (avail.w - mg.l - mg.r).max(10.0),
            h: (avail.h - mg.t - mg.b).max(10.0),
        };
        let d3 = three_d::depth(m, &base);
        if let Some(d) = &d3 {
            base = three_d::front_rect(base, d);
        }
        let r = match manual {
            Some(l) if l.inner => apply_manual(&l, base, w, h),
            _ => base,
        };
        (r, d3)
    };
    for _ in 0..3 {
        let (rect, _) = rect_of(&mg);
        let (pairs, plans) = build(m, &refs, rect);
        mg = margins(&plans, &pairs, avail);
    }
    let (rect, d3) = rect_of(&mg);
    let (pairs, plans) = build(m, &refs, rect);

    // Plot area (a 3-D plot has walls instead).
    if let Some(d) = &d3 {
        three_d::walls(o, m, &rect, d);
    } else if m.plot_fill.is_some() || m.plot_line.is_some() {
        let ln = line_if_set(m.plot_line.as_ref(), Rgba::BLACK, 9525.0);
        o.rect(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            m.plot_fill.as_ref().unwrap_or(&Fill::None),
            ln.as_ref(),
        );
    }
    // Gridlines.
    for (pi, p) in pairs.iter().enumerate() {
        for (ai, ax) in [p.cat_axis, p.val_axis].into_iter().enumerate() {
            if let Some(a) = ax {
                match &d3 {
                    Some(d) => draw_grid3(o, rect, d, a, &plans[pi][ai]),
                    None => draw_grid(o, rect, a, &plans[pi][ai]),
                }
            }
        }
    }
    // Series.
    let mut reqs: Vec<LabelReq> = Vec::new();
    for p in &pairs {
        let geo = Geo {
            r: rect,
            cat_h: p.cat_h,
            d3,
        };
        for gr in &p.groups {
            if o.full() {
                break;
            }
            match gr.g.kind {
                GroupKind::Bar => bars(o, m, p, &geo, gr, &mut reqs),
                GroupKind::Line => lines(o, m, p, &geo, gr, &mut reqs),
                GroupKind::Area => areas(o, m, p, &geo, gr),
                GroupKind::Scatter => scatter(o, m, p, &geo, gr, &mut reqs),
                GroupKind::Bubble => bubbles(o, m, p, &geo, gr, &mut reqs),
                _ => {}
            }
        }
    }
    // Axes on top of the series.
    for (pi, p) in pairs.iter().enumerate() {
        for (ai, ax) in [p.cat_axis, p.val_axis].into_iter().enumerate() {
            let deleted = ax.is_some_and(|a| a.deleted);
            if !deleted {
                draw_axis(o, m, rect, avail, ax, &plans[pi][ai]);
            }
        }
    }
    // Data labels on top of everything but titles and the legend.
    nudge_point_labels(&mut reqs);
    for r in reqs {
        r.draw(o);
    }
    // Axis titles.
    for (pi, p) in pairs.iter().enumerate() {
        for (ai, ax) in [p.cat_axis, p.val_axis].into_iter().enumerate() {
            if ax.is_some_and(|a| !a.deleted) {
                draw_axis_title(o, rect, avail, &plans[pi][ai], &mg);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Building the pairs and the axes
// ---------------------------------------------------------------------------------------------

fn find_axis(m: &ChartModel, id: i64) -> Option<&Axis> {
    m.axes.iter().find(|a| a.id == id)
}

/// The axis pairs of the chart, each with its plans (category / x axis first, value / y second).
fn build<'a>(
    m: &'a ChartModel,
    refs: &[GroupRef<'a>],
    rect: Rect,
) -> (Vec<Pair<'a>>, Vec<[AxisPlan; 2]>) {
    // Group by axis ids.
    let mut keys: Vec<(Option<i64>, Option<i64>)> = Vec::new();
    let mut members: Vec<Vec<GroupRef<'a>>> = Vec::new();
    for gr in refs {
        let (mut a, mut b) = (
            gr.g.axis_ids.first().copied(),
            gr.g.axis_ids.get(1).copied(),
        );
        // Put the value axis second whatever the order in the file.
        if let (Some(x), Some(y)) = (a, b) {
            let kx = find_axis(m, x).map(|ax| ax.kind);
            let ky = find_axis(m, y).map(|ax| ax.kind);
            if !matches!(gr.g.kind, GroupKind::Scatter | GroupKind::Bubble)
                && kx == Some(AxisKind::Val)
                && ky != Some(AxisKind::Val)
            {
                std::mem::swap(&mut a, &mut b);
            }
        }
        let key = (a, b);
        match keys.iter().position(|k| *k == key) {
            Some(i) => members[i].push(*gr),
            None => {
                keys.push(key);
                members.push(vec![*gr]);
            }
        }
    }
    let mut pairs = Vec::new();
    let mut plans = Vec::new();
    for (key, groups) in keys.into_iter().zip(members) {
        let cat_axis = key.0.and_then(|id| find_axis(m, id));
        let val_axis = key.1.and_then(|id| find_axis(m, id));
        let pair = make_pair(m, cat_axis, val_axis, groups, rect);
        let pl = plan_axes(m, &pair, rect);
        pairs.push(pair);
        plans.push(pl);
    }
    (pairs, plans)
}

fn is_xy(g: &ChartGroup) -> bool {
    matches!(g.kind, GroupKind::Scatter | GroupKind::Bubble)
}

fn make_pair<'a>(
    m: &ChartModel,
    cat_axis: Option<&'a Axis>,
    val_axis: Option<&'a Axis>,
    groups: Vec<GroupRef<'a>>,
    rect: Rect,
) -> Pair<'a> {
    let xy = groups.iter().any(|g| is_xy(g.g));
    let cat_h = !groups
        .iter()
        .any(|g| g.g.kind == GroupKind::Bar && g.g.bar_dir == BarDir::Bar);
    let (cat_len, val_len) = if cat_h {
        (rect.w, rect.h)
    } else {
        (rect.h, rect.w)
    };

    // Categories.
    let mut n = 0usize;
    let mut cat_labels: Vec<String> = Vec::new();
    let mut cat_nums: Vec<Option<f64>> = Vec::new();
    let mut cat_fmt: Option<String> = None;
    for gr in &groups {
        for s in &gr.g.series {
            n = n.max(s.cats.len()).max(s.values.len());
            if cat_labels.is_empty() && !s.cats.is_empty() {
                cat_labels = s.cats.clone();
                cat_nums = s.cat_nums.clone();
                cat_fmt = s.cat_format.clone();
            }
        }
    }
    let n = n.min(MAX_CATEGORIES);
    cat_labels.truncate(n);
    // A numeric category axis with a format of its own re-formats the numbers.
    if let Some((code, false)) = cat_axis.and_then(|a| a.num_fmt.clone()) {
        for (i, l) in cat_labels.iter_mut().enumerate() {
            if let Some(Some(v)) = cat_nums.get(i) {
                *l = labels::format_number(Some(&code), *v, m.locale_ja);
            }
        }
    } else if let Some(code) = &cat_fmt {
        let _ = code; // the reader already formatted the labels with the cache's own code
    }
    while cat_labels.len() < n {
        cat_labels.push((cat_labels.len() + 1).to_string());
    }

    // Value axis data.
    let log = val_axis.is_some_and(|a| a.log_base.is_some());
    let mut span_lo = f64::MAX;
    let mut span_hi = f64::MIN;
    let mut have = false;
    let mut percent = false;
    let mut add = |v: f64| {
        if log && v <= 0.0 {
            return;
        }
        have = true;
        span_lo = span_lo.min(v);
        span_hi = span_hi.max(v);
    };
    let mut x_lo = f64::MAX;
    let mut x_hi = f64::MIN;
    let mut x_have = false;
    for gr in &groups {
        let g = gr.g;
        let stacked = matches!(g.grouping, Grouping::Stacked | Grouping::PercentStacked)
            && matches!(g.kind, GroupKind::Bar | GroupKind::Line | GroupKind::Area);
        if stacked && g.grouping == Grouping::PercentStacked {
            percent = true;
            continue;
        }
        if stacked {
            let len = g.series.iter().map(|s| s.values.len()).max().unwrap_or(0);
            let mut pos = vec![0.0f64; len];
            let mut neg = vec![0.0f64; len];
            for s in &g.series {
                for (i, v) in s.values.iter().enumerate() {
                    if let Some(v) = v {
                        if *v >= 0.0 {
                            pos[i] += v;
                            add(pos[i]);
                        } else {
                            neg[i] += v;
                            add(neg[i]);
                        }
                    }
                }
            }
            if g.kind == GroupKind::Bar {
                add(0.0);
            }
        } else {
            for s in &g.series {
                for v in s.values.iter().flatten() {
                    add(*v);
                }
                if is_xy(g) {
                    for v in s.x_values.iter().flatten() {
                        x_have = true;
                        x_lo = x_lo.min(*v);
                        x_hi = x_hi.max(*v);
                    }
                    if s.x_values.is_empty() && !s.values.is_empty() {
                        x_have = true;
                        x_lo = x_lo.min(1.0);
                        x_hi = x_hi.max(s.values.len() as f64);
                    }
                }
            }
        }
    }
    let span = have.then_some((span_lo, span_hi));
    // LibreOffice's automatic ranges depend on the size of the tick labels.
    let libre = |axis: Option<&Axis>, y_axis: bool| {
        let st = resolve(
            m,
            &axis.map(|a| a.text.clone()).unwrap_or_default(),
            TEXT_PT,
            false,
        );
        LibreOpts {
            y_axis,
            label_h: st.line_h(),
            digit_w: line_width(&st, "0"),
        }
    };
    let val_opts = Opts {
        len_px: val_len,
        horizontal: !cat_h,
        percent,
    };
    let mut val_scale = if m.libre_scaling {
        Scale::new_libre(val_axis, span, val_opts, libre(val_axis, true))
    } else {
        Scale::new(val_axis, span, val_opts)
    };
    if percent {
        val_scale.minor = val_scale.major / 5.0;
    }
    let cat = if xy {
        let sp = x_have.then_some((x_lo, x_hi));
        let o = Opts {
            len_px: cat_len,
            horizontal: true,
            percent: false,
        };
        Dim::Num(if m.libre_scaling {
            Scale::new_libre(cat_axis, sp, o, libre(cat_axis, false))
        } else {
            Scale::new(cat_axis, sp, o)
        })
    } else {
        // Only bars force the points to sit between the ticks.
        let has_bar = groups.iter().any(|g| g.g.kind == GroupKind::Bar);
        let between = has_bar || val_axis.is_none_or(|a| a.between);
        Dim::Cat {
            n,
            between,
            reversed: cat_axis.is_some_and(|a| a.reversed),
        }
    };
    let source_fmt = groups
        .iter()
        .flat_map(|g| g.g.series.iter())
        .find_map(|s| s.format_code.clone());
    Pair {
        cat_axis,
        val_axis,
        groups,
        cat_h,
        cat,
        val: Dim::Num(val_scale.clone()),
        vs: val_scale,
        percent,
        cat_labels,
        source_fmt,
    }
}

/// The side an axis' tick labels are on.
fn label_side(
    a: Option<&Axis>,
    horizontal: bool,
    line_f: f64,
    cross_reversed: bool,
) -> Option<Side> {
    let pos = match a.map(|a| a.tick_label_pos).unwrap_or_default() {
        TickLabelPos::None => return None,
        p => p,
    };
    let (neg, posi) = if horizontal {
        (Side::Bottom, Side::Top)
    } else {
        (Side::Left, Side::Right)
    };
    Some(match pos {
        TickLabelPos::Low => {
            if cross_reversed {
                posi
            } else {
                neg
            }
        }
        TickLabelPos::High => {
            if cross_reversed {
                neg
            } else {
                posi
            }
        }
        _ => {
            let at_pos = a.is_some_and(|a| {
                matches!(
                    (horizontal, a.pos),
                    (true, AxisPos::Top) | (false, AxisPos::Right)
                )
            });
            if line_f > 0.99 || (at_pos && line_f > 0.5) {
                posi
            } else {
                neg
            }
        }
    })
}

fn plan_axes(m: &ChartModel, p: &Pair, rect: Rect) -> [AxisPlan; 2] {
    [plan_axis(m, p, rect, true), plan_axis(m, p, rect, false)]
}

/// Plans the category / x axis (`cat = true`) or the value / y axis.
fn plan_axis(m: &ChartModel, p: &Pair, rect: Rect, cat: bool) -> AxisPlan {
    let axis = if cat { p.cat_axis } else { p.val_axis };
    let dim = if cat { &p.cat } else { &p.val };
    let cross_dim = if cat { &p.val } else { &p.cat };
    let horizontal = if cat { p.cat_h } else { !p.cat_h };
    let len = if horizontal { rect.w } else { rect.h };
    let crosses = axis.map(|a| a.crosses).unwrap_or_default();
    let line_f = cross_dim.cross_frac(crosses);
    let side = label_side(axis, horizontal, line_f, cross_dim.reversed());
    let style = resolve(
        m,
        &axis.map(|a| a.text.clone()).unwrap_or_default(),
        TEXT_PT,
        false,
    );
    let explicit_rot = axis.and_then(|a| a.text.rot_deg).filter(|r| r.is_finite());

    // Tick positions and label texts.
    let mut items: Vec<(f64, String)> = Vec::new();
    let mut major: Vec<f64> = Vec::new();
    let mut minor: Vec<f64> = Vec::new();
    let mut rot = explicit_rot.unwrap_or(0.0);
    match dim {
        Dim::Num(s) => {
            let code: Option<String> = match axis.and_then(|a| a.num_fmt.clone()) {
                Some((c, false)) => Some(c),
                _ if p.percent && !cat => Some("0%".into()),
                _ => p.source_fmt.clone().filter(|_| !cat || true),
            };
            // A whole-number format would print LibreOffice's half-unit ticks (0.5, 1.5, ...)
            // as repeated integers: the numbers are written plainly then.
            let code = code
                .filter(|c| !(m.libre_scaling && s.major.fract().abs() > 1e-9 && !c.contains('.')));
            for v in s.major_ticks() {
                let f = s.frac(v);
                major.push(f);
                items.push((f, labels::format_number(code.as_deref(), v, m.locale_ja)));
            }
            minor = s.minor_ticks().into_iter().map(|v| s.frac(v)).collect();
            // Thin out labels that would overlap along the axis.
            let step = if horizontal {
                items
                    .iter()
                    .map(|(_, t)| line_width(&style, t))
                    .fold(0.0, f64::max)
                    + 8.0
            } else {
                style.line_h()
            };
            let mut keep = 1usize;
            while items.len() / keep > 1 && len / ((items.len() / keep) as f64) < step {
                keep += 1;
            }
            if keep > 1 {
                items = items
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| i % keep == 0)
                    .map(|(_, v)| v)
                    .collect();
            }
        }
        Dim::Cat { n, between, .. } => {
            let n = *n;
            let slot = if *between {
                len / n.max(1) as f64
            } else {
                len / (n.max(2) - 1) as f64
            };
            // Tick marks.
            let tick_skip = axis.map(|a| a.tick_skip).filter(|s| *s > 0).unwrap_or(1);
            if *between {
                for i in (0..=n).step_by(tick_skip).take(MAX_CAT_LABELS + 1) {
                    major.push(dim.flip(i as f64 / n.max(1) as f64));
                }
            } else {
                for i in (0..n).step_by(tick_skip).take(MAX_CAT_LABELS) {
                    major.push(dim.flip(slot_pos(n, false, i)));
                }
            }
            // Labels: horizontal text, wrapped, rotated or thinned to fit.
            let texts: Vec<&str> = p.cat_labels.iter().map(String::as_str).collect();
            let widest = texts
                .iter()
                .map(|t| line_width(&style, t))
                .fold(0.0, f64::max);
            let mut skip = axis.map(|a| a.label_skip).filter(|s| *s > 0).unwrap_or(0);
            let mut wrapped: Option<Vec<String>> = None;
            if horizontal && explicit_rot.is_none() {
                if widest + 4.0 > slot {
                    // Wrap at spaces into at most three lines of the slot's width.
                    let w2: Vec<String> = texts
                        .iter()
                        .map(|t| wrap_words(&style, t, (slot - 4.0).max(10.0)))
                        .collect();
                    let ok = w2.iter().all(|t| {
                        t.split('\n').count() <= 3
                            && t.split('\n').all(|l| line_width(&style, l) <= slot - 2.0)
                    });
                    if ok {
                        wrapped = Some(w2);
                    } else {
                        rot = -45.0;
                    }
                }
            } else if !horizontal {
                // A vertical category axis: labels are horizontal; wrap what is too wide.
                let cap = (rect.w * 0.4).max(60.0);
                if widest > cap {
                    wrapped = Some(texts.iter().map(|t| wrap_words(&style, t, cap)).collect());
                }
            }
            if skip == 0 {
                // Needed spacing along the axis per label.
                let need = if !horizontal {
                    style.line_h()
                } else if rot != 0.0 {
                    let r = rot.to_radians().sin().abs().max(0.2);
                    style.line_h() / r
                } else {
                    let w = match &wrapped {
                        Some(ws) => ws
                            .iter()
                            .flat_map(|t| t.split('\n'))
                            .map(|l| line_width(&style, l))
                            .fold(0.0, f64::max),
                        None => widest,
                    };
                    w + 4.0
                };
                skip = ((need / slot.max(1.0)).ceil() as usize).max(1);
            }
            for i in (0..n).step_by(skip.max(1)).take(MAX_CAT_LABELS) {
                let t = match &wrapped {
                    Some(w) => w[i].clone(),
                    None => p.cat_labels[i].clone(),
                };
                items.push((dim.cat_frac(i), t));
            }
            if axis.is_some_and(|a| a.minor_tick != TickMark::None) {
                // Minor ticks of a category axis: halfway between the majors.
                if *between {
                    for i in 0..n.min(MAX_CAT_LABELS) {
                        minor.push(dim.flip((i as f64 + 0.5) / n.max(1) as f64));
                    }
                }
            }
        }
    }

    // Label extents.
    let mut label_w = 0.0f64;
    let mut label_h = 0.0f64;
    for (_, t) in &items {
        let (w, h) = measure(&style, t);
        label_w = label_w.max(w);
        label_h = label_h.max(h);
    }
    let (ext_w, ext_h) = if rot != 0.0 {
        let (s, c) = rot.to_radians().sin_cos();
        (
            label_w * c.abs() + label_h * s.abs(),
            label_w * s.abs() + label_h * c.abs(),
        )
    } else {
        (label_w, label_h)
    };
    let outside = side.is_some_and(|sd| match tick_label_pos(axis) {
        TickLabelPos::Low | TickLabelPos::High => true,
        _ => match sd {
            Side::Bottom | Side::Left => line_f <= 0.01,
            Side::Top | Side::Right => line_f >= 0.99,
        },
    });
    let major_tick = axis.map(|a| a.major_tick).unwrap_or_default();
    let tick_out = matches!(major_tick, TickMark::Out | TickMark::Cross);
    let need_label = if outside {
        let extent = if horizontal { ext_h } else { ext_w };
        extent + LABEL_GAP + if tick_out { TICK } else { 0.0 }
    } else if tick_out && line_f <= 0.01 {
        TICK
    } else {
        0.0
    };

    // Title.
    let title = axis
        .and_then(|a| a.title.as_ref())
        .filter(|t| !t.text.trim().is_empty());
    let title_plan = title.map(|t| {
        let st = resolve(m, &t.style, TEXT_PT, true);
        let r = t
            .style
            .rot_deg
            .filter(|r| r.is_finite())
            .unwrap_or(if horizontal { 0.0 } else { -90.0 });
        (t.text.clone(), st, r)
    });
    let title_side = match (axis.map(|a| a.pos), horizontal, side, outside) {
        (_, true, Some(s), true) => s,
        (_, false, Some(s), true) => s,
        (Some(AxisPos::Top), true, _, _) => Side::Top,
        (Some(AxisPos::Right), false, _, _) => Side::Right,
        (_, true, _, _) => Side::Bottom,
        (_, false, _, _) => Side::Left,
    };
    let need_title = title_plan.as_ref().map_or(0.0, |(t, st, r)| {
        let (w, h) = measure(st, t);
        let (rw, rh) = {
            let (s, c) = r.to_radians().sin_cos();
            (w * c.abs() + h * s.abs(), w * s.abs() + h * c.abs())
        };
        (if horizontal { rh } else { rw }) + TITLE_GAP
    });

    // The first and last labels of a horizontal numeric axis overhang the plot.
    let overhang = if horizontal && matches!(dim, Dim::Num(_)) && !items.is_empty() {
        let first = items
            .first()
            .map_or(0.0, |(_, t)| line_width(&style, t) / 2.0);
        let last = items
            .last()
            .map_or(0.0, |(_, t)| line_width(&style, t) / 2.0);
        (first, last)
    } else {
        (0.0, 0.0)
    };

    AxisPlan {
        horizontal,
        line_f,
        label_side: side,
        outside,
        labels: items,
        label_rot: rot,
        label_style: style,
        label_w,
        label_h,
        major,
        minor,
        major_tick,
        minor_tick: axis.map(|a| a.minor_tick).unwrap_or(TickMark::None),
        title: title_plan,
        title_side,
        need_label,
        need_title,
        overhang,
    }
}

fn tick_label_pos(a: Option<&Axis>) -> TickLabelPos {
    a.map(|a| a.tick_label_pos).unwrap_or_default()
}

/// Greedy word wrap of `text` to `max_w` px; words are never broken.
fn wrap_words(st: &RStyle, text: &str, max_w: f64) -> String {
    if line_width(st, text) <= max_w {
        return text.to_string();
    }
    let mut out = String::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let cand = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if line.is_empty() || line_width(st, &cand) <= max_w {
            line = cand;
        } else {
            out.push_str(&line);
            out.push('\n');
            line = word.to_string();
        }
    }
    out.push_str(&line);
    out
}

/// The room around the plot that the axes need (labels, ticks, titles).
fn margins(plans: &[[AxisPlan; 2]], pairs: &[Pair], avail: Rect) -> Margins {
    let mut mg = Margins {
        l: 4.0,
        t: 4.0,
        r: 4.0,
        b: 4.0,
    };
    for (pl, pair) in plans.iter().zip(pairs) {
        for (ai, ap) in pl.iter().enumerate() {
            let ax = if ai == 0 {
                pair.cat_axis
            } else {
                pair.val_axis
            };
            if ax.is_none_or(|a| a.deleted) {
                continue;
            }
            // Labels.
            if let (Some(side), true) = (ap.label_side, ap.outside) {
                add_margin(&mut mg, side, ap.need_label);
            } else if ap.need_label > 0.0 {
                let side = if ap.horizontal {
                    Side::Bottom
                } else {
                    Side::Left
                };
                add_margin(&mut mg, side, ap.need_label);
            }
            // The labels of a vertical axis are centred on their ticks: half a line overhangs.
            if !ap.horizontal && ap.label_side.is_some() {
                let half = ap.label_h / 2.0;
                mg.t = mg.t.max(half + 2.0);
                mg.b = mg.b.max(half + 2.0);
            }
            if ap.horizontal && ap.label_side.is_some() {
                mg.l = mg.l.max(ap.overhang.0 + 2.0);
                mg.r = mg.r.max(ap.overhang.1 + 2.0);
                if ap.label_rot != 0.0 {
                    // A tilted label hangs to the left of its tick.
                    mg.l =
                        mg.l.max((ap.label_w * ap.label_rot.to_radians().cos().abs()) * 0.5);
                }
            }
            // Titles are stacked beyond the labels.
            if ap.need_title > 0.0 {
                // `add_margin` takes the larger of what is there; titles add to the labels.
                add_title_margin(&mut mg, ap.title_side, ap.need_title, ap);
            }
        }
    }
    // Never leave the plot smaller than a fifth of the area.
    let max_h = avail.w * 0.8;
    let max_v = avail.h * 0.8;
    mg.l = mg.l.min(max_h * 0.6);
    mg.r = mg.r.min(max_h * 0.4);
    mg.t = mg.t.min(max_v * 0.4);
    mg.b = mg.b.min(max_v * 0.6);
    mg
}

fn add_margin(mg: &mut Margins, side: Side, v: f64) {
    let slot = match side {
        Side::Left => &mut mg.l,
        Side::Right => &mut mg.r,
        Side::Top => &mut mg.t,
        Side::Bottom => &mut mg.b,
    };
    *slot = slot.max(v + 4.0);
}

fn add_title_margin(mg: &mut Margins, side: Side, v: f64, ap: &AxisPlan) {
    let labels_here = ap.outside && ap.label_side == Some(side);
    let base = if labels_here { ap.need_label } else { 0.0 };
    let slot = match side {
        Side::Left => &mut mg.l,
        Side::Right => &mut mg.r,
        Side::Top => &mut mg.t,
        Side::Bottom => &mut mg.b,
    };
    *slot = slot.max(base + v + 4.0);
}

// ---------------------------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------------------------

struct Geo {
    r: Rect,
    cat_h: bool,
    /// The depth of a 3-D plot (the groups that are 3-D use it).
    d3: Option<Depth>,
}

impl Geo {
    /// The point at category-axis fraction `c` and value-axis fraction `v`.
    fn xy(&self, c: f64, v: f64) -> (f64, f64) {
        if self.cat_h {
            (self.r.x + c * self.r.w, self.r.bottom() - v * self.r.h)
        } else {
            (self.r.x + v * self.r.w, self.r.bottom() - c * self.r.h)
        }
    }

    /// The rectangle spanning category fractions `c0..c1` and value fractions `v0..v1`.
    fn span(&self, c0: f64, c1: f64, v0: f64, v1: f64) -> Rect {
        let (x0, y0) = self.xy(c0, v0);
        let (x1, y1) = self.xy(c1, v1);
        Rect {
            x: x0.min(x1),
            y: y0.min(y1),
            w: (x1 - x0).abs(),
            h: (y1 - y0).abs(),
        }
    }
}

fn val_scale<'p>(p: &'p Pair) -> &'p Scale {
    &p.vs
}

// ---------------------------------------------------------------------------------------------
// Data labels
// ---------------------------------------------------------------------------------------------

/// A data label waiting to be drawn above the axes.
struct LabelReq {
    text: String,
    st: RStyle,
    pos: LabelPos,
    at: LabelAt,
}

enum LabelAt {
    Bar {
        r: Rect,
        vertical: bool,
        positive: bool,
    },
    Point {
        x: f64,
        y: f64,
        r: f64,
    },
}

/// The box `(left, top, right, bottom)` a label next to a point takes (the geometry of
/// [`labels::at_point`]).
fn point_label_box(r: &LabelReq) -> Option<(f64, f64, f64, f64)> {
    let LabelAt::Point { x, y, r: mr } = r.at else {
        return None;
    };
    let (tw, th) = measure(&r.st, &r.text);
    let w = tw + 6.0;
    let g = mr + 3.0;
    Some(match r.pos {
        LabelPos::Left => (x - g - w, y - th / 2.0, x - g, y + th / 2.0),
        LabelPos::Above | LabelPos::OutsideEnd | LabelPos::InsideEnd => {
            (x - w / 2.0, y - g - th, x + w / 2.0, y - g)
        }
        LabelPos::Below | LabelPos::InsideBase => (x - w / 2.0, y + g, x + w / 2.0, y + g + th),
        LabelPos::Center => (x - w / 2.0, y - th / 2.0, x + w / 2.0, y + th / 2.0),
        LabelPos::Right | LabelPos::BestFit => (x + g, y - th / 2.0, x + g + w, y + th / 2.0),
    })
}

/// Most labels of one chart that are checked against each other (the check is quadratic).
const MAX_NUDGED_LABELS: usize = 600;

/// Simple overlap avoidance for the labels of points (line, scatter, bubble): a label that would
/// cover one already placed moves up or down by whole label heights to the nearest free spot (it
/// stays where it is when there is none). Labels of bars are not moved: they sit in their bar.
fn nudge_point_labels(reqs: &mut [LabelReq]) {
    let mut placed: Vec<(f64, f64, f64, f64)> = Vec::new();
    let hit = |a: &(f64, f64, f64, f64), b: &(f64, f64, f64, f64)| {
        a.0 < b.2 - 0.5 && b.0 < a.2 - 0.5 && a.1 < b.3 - 0.5 && b.1 < a.3 - 0.5
    };
    let mut seen = 0usize;
    for r in reqs.iter_mut() {
        let Some(b0) = point_label_box(r) else {
            continue;
        };
        seen += 1;
        if seen > MAX_NUDGED_LABELS {
            break;
        }
        let h = b0.3 - b0.1;
        let mut chosen = b0;
        let mut dy_chosen = 0.0;
        // A label above its point moves further up first, one below it further down.
        let order: [f64; 5] = match r.pos {
            LabelPos::Below | LabelPos::InsideBase => [0.0, 1.0, 2.0, -1.0, -2.0],
            _ => [0.0, -1.0, -2.0, 1.0, 2.0],
        };
        for k in order {
            let dy = k * h;
            let b = (b0.0, b0.1 + dy, b0.2, b0.3 + dy);
            if !placed.iter().any(|p| hit(p, &b)) {
                chosen = b;
                dy_chosen = dy;
                break;
            }
        }
        if let LabelAt::Point { y, .. } = &mut r.at {
            *y += dy_chosen;
        }
        placed.push(chosen);
    }
}

impl LabelReq {
    fn draw(self, o: &mut Out) {
        match self.at {
            LabelAt::Bar {
                r,
                vertical,
                positive,
            } => labels::on_bar(o, &self.st, &self.text, r, self.pos, vertical, positive),
            LabelAt::Point { x, y, r } => {
                labels::at_point(o, &self.st, &self.text, x, y, r, self.pos)
            }
        }
    }
}

/// The label of point `i` of a series, if it has one.
#[allow(clippy::too_many_arguments)]
fn label_for(
    m: &ChartModel,
    p: &Pair,
    g: &ChartGroup,
    s: &Series,
    i: usize,
    value: Option<f64>,
    default_pos: LabelPos,
) -> Option<(String, RStyle, LabelPos)> {
    let dl = labels::effective(g.labels.as_ref(), s.labels.as_ref(), i)?;
    let info = Info {
        cat: p.cat_labels.get(i).map(String::as_str),
        series: s.name.as_deref(),
        value,
        percent: None,
        source_fmt: s.format_code.as_deref(),
        ja: m.locale_ja,
    };
    let t = labels::text(&dl, &info);
    if t.is_empty() {
        return None;
    }
    let st = resolve(m, &dl.style, TEXT_PT, false);
    Some((t, st, dl.pos.unwrap_or(default_pos)))
}

fn point_map(s: &Series) -> HashMap<usize, &PointFmt> {
    s.points.iter().map(|p| (p.idx, p)).collect()
}

// ---------------------------------------------------------------------------------------------
// Bars
// ---------------------------------------------------------------------------------------------

fn base_value(p: &Pair, sc: &Scale) -> f64 {
    let c = p.cat_axis.map(|a| a.crosses).unwrap_or_default();
    match c {
        Crosses::AutoZero => {
            if sc.log.is_some() {
                sc.min
            } else {
                0.0f64.clamp(sc.min, sc.max)
            }
        }
        Crosses::Min => sc.min,
        Crosses::Max => sc.max,
        Crosses::At(v) => v,
    }
}

fn bars(o: &mut Out, m: &ChartModel, p: &Pair, geo: &Geo, gr: &GroupRef, reqs: &mut Vec<LabelReq>) {
    let g = gr.g;
    let sc = val_scale(p);
    let Dim::Cat { n, .. } = p.cat else { return };
    if n == 0 || g.series.is_empty() {
        return;
    }
    let stacked = matches!(g.grouping, Grouping::Stacked | Grouping::PercentStacked);
    let percent = g.grouping == Grouping::PercentStacked;
    let k = if stacked { 1 } else { g.series.len() };
    let gap = g.gap_width / 100.0;
    let ov = if stacked { 1.0 } else { g.overlap / 100.0 };
    let denom = k as f64 - (k as f64 - 1.0) * ov + gap;
    let bw = if denom > 0.0 { 1.0 / denom } else { 1.0 };
    let start = gap * bw / 2.0;
    let base = base_value(p, sc);
    let base_f = sc.frac(base).clamp(0.0, 1.0);
    let vertical = p.cat_h;
    let maps: Vec<HashMap<usize, &PointFmt>> = g.series.iter().map(point_map).collect();
    let vary = g.vary_colors && g.series.len() == 1;

    // Per-category totals for percent stacking.
    let totals: Vec<f64> = if percent {
        (0..n)
            .map(|i| {
                g.series
                    .iter()
                    .filter_map(|s| s.values.get(i).copied().flatten())
                    .map(f64::abs)
                    .sum()
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut pos_sum = vec![0.0f64; if stacked { n } else { 0 }];
    let mut neg_sum = vec![0.0f64; if stacked { n } else { 0 }];

    for i in 0..n {
        for (j, s) in g.series.iter().enumerate() {
            let Some(Some(raw)) = s.values.get(i).copied().map(|v| v.filter(|_| true)) else {
                continue;
            };
            let v = if percent {
                let t = totals[i];
                if t > 0.0 {
                    raw / t
                } else {
                    0.0
                }
            } else {
                raw
            };
            let (a, b) = if stacked {
                if v >= 0.0 {
                    let a = pos_sum[i];
                    pos_sum[i] += v;
                    (a, pos_sum[i])
                } else {
                    let a = neg_sum[i];
                    neg_sum[i] += v;
                    (a, neg_sum[i])
                }
            } else {
                (base, v)
            };
            let fa = if stacked { sc.frac(a) } else { base_f };
            let fa = fa.clamp(0.0, 1.0);
            let fb = sc.frac(b).clamp(0.0, 1.0);
            let slot = if stacked {
                0.0
            } else {
                j as f64 * bw * (1.0 - ov)
            };
            let left = (i as f64 + start + slot) / n as f64;
            let right = left + bw / n as f64;
            let r = geo.span(p.cat.flip(left), p.cat.flip(right), fa, fb);
            if r.w < 0.01 && r.h < 0.01 {
                continue;
            }
            let pf = maps[j].get(&i).copied();
            let ord = gr.ord0 + j;
            let fill = pf
                .and_then(|pf| pf.fill.clone())
                .or_else(|| s.fill.clone().filter(|_| !vary))
                .unwrap_or_else(|| {
                    Fill::Solid(if vary {
                        point_color(m, i, n)
                    } else {
                        series_color(m, ord)
                    })
                });
            let stroke = pf.and_then(|pf| pf.line.as_ref()).or(s.line.as_ref());
            let line = line_if_set(stroke, Rgba::BLACK, 9525.0).or_else(|| {
                stroke
                    .is_none()
                    .then(|| super::layout::style_outline(m))
                    .flatten()
            });
            let r = match geo.d3.filter(|_| g.three_d) {
                Some(d) => {
                    let (thick, z0) = d.thickness(d.d);
                    three_d::bar_box(o, &d, &r, z0, thick, &fill, line.as_ref())
                }
                None => {
                    o.rect(r.x, r.y, r.w, r.h, &fill, line.as_ref());
                    r
                }
            };
            if o.full() {
                return;
            }
            let dflt = if stacked {
                LabelPos::Center
            } else {
                LabelPos::OutsideEnd
            };
            if let Some((text, st, pos)) = label_for(m, p, g, s, i, Some(raw), dflt) {
                reqs.push(LabelReq {
                    text,
                    st,
                    pos,
                    at: LabelAt::Bar {
                        r,
                        vertical,
                        positive: raw >= 0.0,
                    },
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Lines and areas
// ---------------------------------------------------------------------------------------------

/// The plotted value of each point of each series of a line or area group, with stacking applied:
/// `(lower, upper)` per point (the lower edge is the previous series' upper), `None` for a blank.
fn stacked_values(g: &ChartGroup, n: usize, blanks_zero: bool) -> Vec<Vec<Option<(f64, f64)>>> {
    let stacked = matches!(g.grouping, Grouping::Stacked | Grouping::PercentStacked);
    let percent = g.grouping == Grouping::PercentStacked;
    let totals: Vec<f64> = if percent {
        (0..n)
            .map(|i| {
                g.series
                    .iter()
                    .filter_map(|s| s.values.get(i).copied().flatten())
                    .map(f64::abs)
                    .sum()
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut acc = vec![0.0f64; if stacked { n } else { 0 }];
    let mut out = Vec::with_capacity(g.series.len());
    for s in &g.series {
        let mut row = Vec::with_capacity(n);
        for i in 0..n {
            let raw = s.values.get(i).copied().flatten();
            let raw = match raw {
                Some(v) => Some(v),
                None if blanks_zero => Some(0.0),
                None => None,
            };
            row.push(raw.map(|v| {
                let v = if percent {
                    if totals[i] > 0.0 {
                        v / totals[i]
                    } else {
                        0.0
                    }
                } else {
                    v
                };
                if stacked {
                    let lo = acc[i];
                    acc[i] += v;
                    (lo, acc[i])
                } else {
                    (0.0, v)
                }
            }));
        }
        out.push(row);
    }
    out
}

fn lines(
    o: &mut Out,
    m: &ChartModel,
    p: &Pair,
    geo: &Geo,
    gr: &GroupRef,
    reqs: &mut Vec<LabelReq>,
) {
    let g = gr.g;
    let sc = val_scale(p);
    let Dim::Cat { n, .. } = p.cat else { return };
    let stacked = matches!(g.grouping, Grouping::Stacked | Grouping::PercentStacked);
    let rows = stacked_values(g, n, m.disp_blanks_as == DispBlanks::Zero);
    let d3 = geo.d3.filter(|_| g.three_d);
    for j in series_order(g, d3.is_some()) {
        let s = &g.series[j];
        let ord = gr.ord0 + j;
        let color = s
            .line
            .as_ref()
            .and_then(|l| l.color)
            .unwrap_or_else(|| series_color(m, ord));
        let ln = line_of(s.line.as_ref(), color, 28575.0);
        // Points as positions; runs split at blanks.
        let pts: Vec<Option<(f64, f64)>> = rows[j]
            .iter()
            .enumerate()
            .map(|(i, v)| {
                v.map(|(lo, hi)| {
                    let val = if stacked { hi } else { hi - lo };
                    let (x, y) = geo.xy(p.cat.cat_frac(i), sc.frac(val).clamp(-0.5, 1.5));
                    // A 3-D line runs in its own row in depth.
                    match &d3 {
                        Some(d) => {
                            let (_, z0) = depth_row(d, g, j);
                            let (ox, oy) = d.off(z0);
                            (x + ox, y + oy)
                        }
                        None => (x, y),
                    }
                })
            })
            .collect();
        let smooth = s.smooth.unwrap_or(false);
        if let Some(ln) = &ln {
            let mut run: Vec<(f64, f64)> = Vec::new();
            let flush = |o: &mut Out, run: &mut Vec<(f64, f64)>| {
                if run.len() >= 2 {
                    if let Some(d) = &d3 {
                        let (thick, _) = depth_row(d, g, j);
                        three_d::line_ribbon(o, d, run, 0.0, thick, ln);
                    } else if smooth {
                        o.smooth(run, &Fill::None, Some(ln));
                    } else {
                        o.poly(run, false, &Fill::None, Some(ln));
                    }
                }
                run.clear();
            };
            for pt in &pts {
                match pt {
                    Some(q) => run.push(*q),
                    None if m.disp_blanks_as == DispBlanks::Span => {}
                    None => flush(o, &mut run),
                }
            }
            flush(o, &mut run);
        }
        let pmap = point_map(s);
        let mk = marker_of(m, s, None, ord, g.markers);
        let mut msize = 0.0f64;
        for (i, pt) in pts.iter().enumerate() {
            let Some((x, y)) = pt else { continue };
            let pm = pmap.get(&i).and_then(|pf| pf.marker.as_ref());
            let mk_i = if pm.is_some() {
                marker_of(m, s, pm, ord, g.markers)
            } else {
                mk
            };
            if let Some((sym, size, c)) = mk_i.filter(|_| d3.is_none()) {
                msize = msize.max(size);
                if o.full() {
                    return;
                }
                draw_marker(o, *x, *y, sym, size, c, pm.or(s.marker.as_ref()));
            }
            if let Some((text, st, pos)) = label_for(
                m,
                p,
                g,
                s,
                i,
                s.values.get(i).copied().flatten(),
                LabelPos::Right,
            ) {
                reqs.push(LabelReq {
                    text,
                    st,
                    pos,
                    at: LabelAt::Point {
                        x: *x,
                        y: *y,
                        r: msize / 2.0,
                    },
                });
            }
        }
    }
}

fn areas(o: &mut Out, m: &ChartModel, p: &Pair, geo: &Geo, gr: &GroupRef) {
    let g = gr.g;
    let sc = val_scale(p);
    let Dim::Cat { n, .. } = p.cat else { return };
    if n == 0 {
        return;
    }
    let stacked = matches!(g.grouping, Grouping::Stacked | Grouping::PercentStacked);
    let rows = stacked_values(g, n, true);
    let base_f = sc.frac(base_value(p, sc)).clamp(0.0, 1.0);
    let d3 = geo.d3.filter(|_| g.three_d);
    for j in series_order(g, d3.is_some()) {
        let s = &g.series[j];
        let ord = gr.ord0 + j;
        let fill = series_fill(m, s, ord);
        let ln = line_if_set(s.line.as_ref(), Rgba::BLACK, 9525.0).or_else(|| {
            s.line
                .is_none()
                .then(|| super::layout::style_outline(m))
                .flatten()
        });
        let mut top: Vec<(f64, f64)> = Vec::with_capacity(n);
        let mut bottom: Vec<(f64, f64)> = Vec::with_capacity(n);
        for (i, row) in rows[j].iter().enumerate().take(n) {
            let Some((lo, hi)) = *row else { continue };
            let c = p.cat.cat_frac(i);
            let (upper, lower) = if stacked { (hi, lo) } else { (hi, 0.0) };
            let fu = sc.frac(upper).clamp(0.0, 1.0);
            let fl = if stacked {
                sc.frac(lower).clamp(0.0, 1.0)
            } else {
                base_f
            };
            top.push(geo.xy(c, fu));
            bottom.push(geo.xy(c, fl));
        }
        if top.len() < 2 {
            continue;
        }
        if let Some(d) = &d3 {
            let (thick, z0) = depth_row(d, g, j);
            three_d::area_ribbon(o, d, &top, &bottom, z0, thick, &fill, ln.as_ref());
            if o.full() {
                return;
            }
            continue;
        }
        let mut poly = top;
        bottom.reverse();
        poly.extend(bottom);
        o.poly(&poly, true, &fill, ln.as_ref());
        if o.full() {
            return;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Scatter and bubble
// ---------------------------------------------------------------------------------------------

fn xy_points(p: &Pair, geo: &Geo, s: &Series) -> Vec<Option<(f64, f64, usize)>> {
    let (Dim::Num(xs), Dim::Num(ys)) = (&p.cat, &p.val) else {
        return Vec::new();
    };
    let n = s.values.len().max(s.x_values.len());
    (0..n)
        .map(|i| {
            let y = s.values.get(i).copied().flatten()?;
            let x = if s.x_values.is_empty() {
                (i + 1) as f64
            } else {
                s.x_values.get(i).copied().flatten()?
            };
            if (xs.log.is_some() && x <= 0.0) || (ys.log.is_some() && y <= 0.0) {
                return None;
            }
            let (px, py) = geo.xy(xs.frac(x).clamp(-0.5, 1.5), ys.frac(y).clamp(-0.5, 1.5));
            Some((px, py, i))
        })
        .collect()
}

fn scatter(
    o: &mut Out,
    m: &ChartModel,
    p: &Pair,
    geo: &Geo,
    gr: &GroupRef,
    reqs: &mut Vec<LabelReq>,
) {
    let g = gr.g;
    for (j, s) in g.series.iter().enumerate() {
        let ord = gr.ord0 + j;
        let pts = xy_points(p, geo, s);
        let style = g.scatter_style;
        let smooth = s.smooth.unwrap_or(matches!(
            style,
            ScatterStyle::Smooth | ScatterStyle::SmoothMarker
        ));
        let show_line = !s.line.as_ref().is_some_and(|l| l.none)
            && !matches!(style, ScatterStyle::Marker | ScatterStyle::None);
        let color = s
            .line
            .as_ref()
            .and_then(|l| l.color)
            .unwrap_or_else(|| series_color(m, ord));
        if show_line {
            if let Some(ln) = line_of(s.line.as_ref(), color, 28575.0) {
                let mut run: Vec<(f64, f64)> = Vec::new();
                for pt in &pts {
                    match pt {
                        Some((x, y, _)) => run.push((*x, *y)),
                        None if m.disp_blanks_as == DispBlanks::Span => {}
                        None => {
                            emit_run(o, &run, smooth, &ln);
                            run.clear();
                        }
                    }
                }
                emit_run(o, &run, smooth, &ln);
            }
        }
        let on = !matches!(style, ScatterStyle::Line | ScatterStyle::Smooth);
        let pmap = point_map(s);
        let mk = marker_of(m, s, None, ord, on);
        for pt in pts.iter().flatten() {
            let (x, y, i) = *pt;
            let pm = pmap.get(&i).and_then(|pf| pf.marker.as_ref());
            let mk_i = if pm.is_some() {
                marker_of(m, s, pm, ord, on)
            } else {
                mk
            };
            let mut r = 0.0;
            if let Some((sym, size, c)) = mk_i {
                if o.full() {
                    return;
                }
                r = size / 2.0;
                draw_marker(o, x, y, sym, size, c, pm.or(s.marker.as_ref()));
            }
            if let Some((text, st, pos)) = label_for(
                m,
                p,
                g,
                s,
                i,
                s.values.get(i).copied().flatten(),
                LabelPos::Right,
            ) {
                reqs.push(LabelReq {
                    text,
                    st,
                    pos,
                    at: LabelAt::Point { x, y, r },
                });
            }
        }
    }
}

fn emit_run(o: &mut Out, run: &[(f64, f64)], smooth: bool, ln: &Line) {
    if run.len() < 2 {
        return;
    }
    if smooth {
        o.smooth(run, &Fill::None, Some(ln));
    } else {
        o.poly(run, false, &Fill::None, Some(ln));
    }
}

fn bubbles(
    o: &mut Out,
    m: &ChartModel,
    p: &Pair,
    geo: &Geo,
    gr: &GroupRef,
    reqs: &mut Vec<LabelReq>,
) {
    let g = gr.g;
    let max_size = g
        .series
        .iter()
        .flat_map(|s| s.sizes.iter().flatten().copied())
        .filter(|v| *v > 0.0)
        .fold(0.0f64, f64::max);
    if max_size <= 0.0 {
        return;
    }
    let max_d = 0.25 * geo.r.w.min(geo.r.h) * (g.bubble_scale / 100.0);
    for (j, s) in g.series.iter().enumerate() {
        let ord = gr.ord0 + j;
        let pts = xy_points(p, geo, s);
        let base = match series_fill(m, s, ord) {
            Fill::Solid(c) if s.fill.is_none() => Fill::Solid(c.with_alpha(0.75)),
            f => f,
        };
        let ln = line_if_set(s.line.as_ref(), Rgba::WHITE, 9525.0);
        let pmap = point_map(s);
        let mut order: Vec<(f64, f64, usize, f64)> = pts
            .iter()
            .flatten()
            .filter_map(|(x, y, i)| {
                let sz = s.sizes.get(*i).copied().flatten()?;
                (sz > 0.0).then_some((*x, *y, *i, sz))
            })
            .collect();
        // Big bubbles first, so that small ones stay visible.
        order.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
        for (x, y, i, sz) in order {
            let frac = if g.size_is_width {
                sz / max_size
            } else {
                (sz / max_size).sqrt()
            };
            let r = max_d * frac / 2.0;
            let fill = pmap
                .get(&i)
                .and_then(|pf| pf.fill.clone())
                .unwrap_or_else(|| base.clone());
            if o.full() {
                return;
            }
            o.ellipse(x, y, r.max(0.5), r.max(0.5), &fill, ln.as_ref());
            if let Some((text, st, pos)) = label_for(
                m,
                p,
                g,
                s,
                i,
                s.values.get(i).copied().flatten(),
                LabelPos::Right,
            ) {
                reqs.push(LabelReq {
                    text,
                    st,
                    pos,
                    at: LabelAt::Point { x, y, r },
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Axes
// ---------------------------------------------------------------------------------------------

fn draw_grid(o: &mut Out, rect: Rect, a: &Axis, ap: &AxisPlan) {
    for (stroke, fracs) in [(&a.major_grid, &ap.major), (&a.minor_grid, &ap.minor)] {
        let Some(s) = stroke else { continue };
        let Some(ln) = line_of(Some(s), AXIS_COLOR, AXIS_W) else {
            continue;
        };
        for &f in fracs.iter() {
            if !(-0.001..=1.001).contains(&f) {
                continue;
            }
            if ap.horizontal {
                let x = rect.x + f * rect.w;
                o.seg(x, rect.y, x, rect.bottom(), &ln);
            } else {
                let y = rect.bottom() - f * rect.h;
                o.seg(rect.x, y, rect.right(), y, &ln);
            }
            if o.full() {
                return;
            }
        }
    }
}

/// The gridlines of an axis on the walls of a 3-D plot.
fn draw_grid3(o: &mut Out, rect: Rect, d: &Depth, a: &Axis, ap: &AxisPlan) {
    for (stroke, fracs) in [(&a.major_grid, &ap.major), (&a.minor_grid, &ap.minor)] {
        let Some(s) = stroke else { continue };
        let Some(ln) = line_of(Some(s), AXIS_COLOR, AXIS_W) else {
            continue;
        };
        for &f in fracs.iter() {
            if !(-0.001..=1.001).contains(&f) {
                continue;
            }
            three_d::grid_line(o, &rect, d, ap.horizontal, f, &ln);
            if o.full() {
                return;
            }
        }
    }
}

/// The order series are drawn in: the ones in rows of a 3-D plot go back to front (the first
/// series is in front), the others in their own order.
fn series_order(g: &ChartGroup, three_d: bool) -> Vec<usize> {
    let n = g.series.len();
    if three_d && three_d::rows_of(g) > 1 {
        (0..n).rev().collect()
    } else {
        (0..n).collect()
    }
}

/// `(thickness, z0)` of the ribbon of series `j` in a 3-D plot: the depth is shared by the rows.
fn depth_row(d: &Depth, g: &ChartGroup, j: usize) -> (f64, f64) {
    let rows = three_d::rows_of(g);
    let row = d.d / rows as f64;
    let (thick, off) = d.thickness(row);
    let r = if rows > 1 { j.min(rows - 1) } else { 0 };
    (thick, r as f64 * row + off)
}

fn draw_axis(
    o: &mut Out,
    _m: &ChartModel,
    rect: Rect,
    _avail: Rect,
    a: Option<&Axis>,
    ap: &AxisPlan,
) {
    let ln = line_of(a.and_then(|a| a.line.as_ref()), AXIS_COLOR, AXIS_W);
    let (ax, ay, bx, by) = if ap.horizontal {
        let y = rect.bottom() - ap.line_f * rect.h;
        (rect.x, y, rect.right(), y)
    } else {
        let x = rect.x + ap.line_f * rect.w;
        (x, rect.y, x, rect.bottom())
    };
    // Which way "out" points: the label side, else the side of the position.
    let out_dir = match ap.label_side.unwrap_or(ap.title_side) {
        Side::Bottom | Side::Right => 1.0,
        Side::Top | Side::Left => -1.0,
    };
    if let Some(l) = &ln {
        o.seg(ax, ay, bx, by, l);
        // Tick marks.
        for (fracs, kind, len) in [
            (&ap.major, ap.major_tick, TICK),
            (&ap.minor, ap.minor_tick, TICK * 0.6),
        ] {
            let (out, inn) = match kind {
                TickMark::None => continue,
                TickMark::Out => (len, 0.0),
                TickMark::In => (0.0, len),
                TickMark::Cross => (len, len),
            };
            for &f in fracs.iter().take(super::MAX_TICKS * 5) {
                if !(-0.001..=1.001).contains(&f) {
                    continue;
                }
                if ap.horizontal {
                    let x = rect.x + f * rect.w;
                    o.seg(x, ay - out_dir * -inn, x, ay + out_dir * out, l);
                } else {
                    let y = rect.bottom() - f * rect.h;
                    o.seg(ax + out_dir * -inn, y, ax + out_dir * out, y, l);
                }
            }
        }
    }
    // Tick labels.
    let Some(side) = ap.label_side else { return };
    let tick_out = if matches!(ap.major_tick, TickMark::Out | TickMark::Cross) {
        TICK
    } else {
        0.0
    };
    let st = &ap.label_style;
    let at_edge = matches!(
        a.map(|a| a.tick_label_pos),
        Some(TickLabelPos::Low | TickLabelPos::High)
    );
    for (f, text) in &ap.labels {
        if !(-0.001..=1.001).contains(f) {
            continue;
        }
        if ap.horizontal {
            let x = rect.x + f * rect.w;
            let base_y = if at_edge {
                match side {
                    Side::Top => rect.y,
                    _ => rect.bottom(),
                }
            } else {
                ay
            };
            let (y, av) = match side {
                Side::Top => (base_y - tick_out - LABEL_GAP, VAlign::Bottom),
                _ => (base_y + tick_out + LABEL_GAP, VAlign::Top),
            };
            if ap.label_rot == 0.0 {
                o.text(x, y, HAlign::Center, av, text, st, 0.0);
            } else {
                // The end of the text sits at the tick, the text rises (or falls) away from it.
                let ah = if ap.label_rot < 0.0 {
                    HAlign::Right
                } else {
                    HAlign::Left
                };
                let av = if side == Side::Top {
                    VAlign::Bottom
                } else {
                    VAlign::Middle
                };
                let y = if side == Side::Top {
                    y
                } else {
                    y + st.line_h() / 2.0
                };
                o.text(x, y, ah, av, text, st, ap.label_rot);
            }
        } else {
            let y = rect.bottom() - f * rect.h;
            let base_x = if at_edge {
                match side {
                    Side::Right => rect.right(),
                    _ => rect.x,
                }
            } else {
                ax
            };
            let (x, ah) = match side {
                Side::Right => (base_x + tick_out + LABEL_GAP + 1.0, HAlign::Left),
                _ => (base_x - tick_out - LABEL_GAP - 1.0, HAlign::Right),
            };
            o.text(x, y, ah, VAlign::Middle, text, st, ap.label_rot);
        }
        if o.full() {
            return;
        }
    }
}

fn draw_axis_title(o: &mut Out, rect: Rect, avail: Rect, ap: &AxisPlan, _mg: &Margins) {
    let Some((text, st, rot)) = &ap.title else {
        return;
    };
    let (w, h) = measure(st, text);
    let (s, c) = rot.to_radians().sin_cos();
    let (rw, rh) = (w * c.abs() + h * s.abs(), w * s.abs() + h * c.abs());
    if ap.horizontal {
        let cx = rect.x + rect.w / 2.0;
        let cy = match ap.title_side {
            Side::Top => avail.y + rh / 2.0,
            _ => avail.bottom() - rh / 2.0,
        };
        o.text(cx, cy, HAlign::Center, VAlign::Middle, text, st, *rot);
    } else {
        let cy = rect.y + rect.h / 2.0;
        let cx = match ap.title_side {
            Side::Right => avail.right() - rw / 2.0,
            _ => avail.x + rw / 2.0,
        };
        o.text(cx, cy, HAlign::Center, VAlign::Middle, text, st, *rot);
    }
}
