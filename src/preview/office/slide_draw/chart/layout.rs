//! The frame of a chart: the chart and plot area fills, the title, the legend, and handing the rest
//! of the box to the plot drawing (cartesian, pie / doughnut, radar). Also the colour of series and
//! points, which the legend and the plots share.

use super::super::color::{apply_mods, hsl_to_rgb, rgb_to_hsl, ColorMod};
use super::super::model::*;
use super::shapes::{line_if_set, line_of, Out};
use super::text::{measure, resolve, HAlign, RStyle, VAlign};
use super::{
    cartesian, pie, radar, ChartGroup, ChartModel, GroupKind, Grouping, Legend, LegendPos,
    ManualLayout, Marker, MarkerSymbol, Series, MAX_LEGEND_ENTRIES,
};

/// A rectangle in px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

/// Space around the chart's edge, px.
const PAD: f64 = 8.0;
/// Default title size (pt) and weight when the chart says nothing (Office 2007 style 2).
const TITLE_PT: f64 = 18.0;
/// Default text size (pt).
pub(super) const TEXT_PT: f64 = 10.0;

/// The theme accents Office uses when the model has no palette.
const DEFAULT_ACCENTS: [Rgba; 6] = [
    Rgba::rgb(0x44, 0x72, 0xC4),
    Rgba::rgb(0xED, 0x7D, 0x31),
    Rgba::rgb(0xA5, 0xA5, 0xA5),
    Rgba::rgb(0xFF, 0xC0, 0x00),
    Rgba::rgb(0x5B, 0x9B, 0xD5),
    Rgba::rgb(0x70, 0xAD, 0x47),
];

/// Number of series the automatic colours are spread over (monochrome styles).
fn series_total(m: &ChartModel) -> usize {
    m.groups
        .iter()
        .filter(|g| !matches!(g.kind, GroupKind::Pie | GroupKind::Doughnut))
        .map(|g| g.series.len())
        .sum::<usize>()
        .max(1)
}

/// The automatic colour of the `i`-th series.
pub(super) fn series_color(m: &ChartModel, i: usize) -> Rgba {
    point_color(m, i, series_total(m))
}

/// The automatic colour of the `i`-th of `n` series or points (see the module documentation of
/// [`super`] for the styles).
pub(super) fn point_color(m: &ChartModel, i: usize, n: usize) -> Rgba {
    let accent = |k: usize| {
        if m.palette.is_empty() {
            DEFAULT_ACCENTS[k % 6]
        } else {
            m.palette[k % m.palette.len().clamp(1, 6)]
        }
    };
    let kind = if m.style == 0 {
        2
    } else {
        (m.style as usize - 1) % 8 + 1
    };
    if kind != 2 {
        let base = if kind == 1 {
            Rgba::rgb(0x7F, 0x7F, 0x7F)
        } else {
            accent(kind - 3)
        };
        if n <= 1 {
            return base;
        }
        let t = (i.min(n - 1)) as f64 / (n - 1) as f64;
        let (h, s, l) = rgb_to_hsl(base.r, base.g, base.b);
        let (l0, l1) = (l * 0.7, l + (1.0 - l) * 0.65);
        let (r, g, b) = hsl_to_rgb(h, s * (1.0 - 0.55 * t), l0 + (l1 - l0) * t);
        return Rgba::rgb(r, g, b);
    }
    let base = accent(i);
    let mods: &[ColorMod] = match (i / 6).min(5) {
        0 => &[],
        1 => &[ColorMod::LumMod(0.6)],
        2 => &[ColorMod::LumMod(0.8), ColorMod::LumOff(0.2)],
        3 => &[ColorMod::LumMod(0.6), ColorMod::LumOff(0.4)],
        4 => &[ColorMod::LumMod(0.5)],
        _ => &[ColorMod::LumMod(0.7), ColorMod::LumOff(0.3)],
    };
    apply_mods(base, mods)
}

/// The automatic axis / gridline colour.
pub(super) const AXIS_COLOR: Rgba = Rgba::rgb(0x86, 0x86, 0x86);
/// The automatic axis / gridline width: 0.75 pt.
pub(super) const AXIS_W: f64 = 9525.0;

/// The first colour of a fill, for places that need a single colour.
pub(super) fn fill_color(f: &Fill) -> Option<Rgba> {
    match f {
        Fill::Solid(c) => Some(*c),
        Fill::Gradient(g) => g.stops.first().map(|s| s.1),
        Fill::Pattern { fg, .. } => Some(*fg),
        _ => None,
    }
}

/// What kind of key a legend entry shows.
#[derive(Debug, Clone, PartialEq)]
enum Key {
    Box(Fill, Option<Line>),
    Line(Option<Line>, Option<(MarkerSymbol, f64, Rgba)>),
}

struct Entry {
    text: String,
    key: Key,
}

/// The series' own fill, or the automatic colour of position `i`.
/// The outline a filled series element (bar, area, slice) has when the file gives it none: the
/// Office 2007 chart styles 9 to 16 outline them in white (a thin white line separates the
/// segments of a stacked column; checked against LibreOffice's rendering of `c:style 12` in
/// `poi/aascu.org_hbcu_leadershipsummit_cooper_.pptx`, which applies the same style table). Every
/// other style leaves them unoutlined; a `c:spPr/a:ln` of the element always wins.
pub(super) fn style_outline(m: &ChartModel) -> Option<Line> {
    (9..=16)
        .contains(&m.style)
        .then(|| Line::solid(9525.0, Rgba::WHITE))
}

pub(super) fn series_fill(m: &ChartModel, s: &Series, i: usize) -> Fill {
    s.fill
        .clone()
        .unwrap_or_else(|| Fill::Solid(series_color(m, i)))
}

/// The marker symbol that automatic markers cycle through (Excel 2007).
pub(super) fn auto_symbol(i: usize) -> MarkerSymbol {
    const CYCLE: [MarkerSymbol; 9] = [
        MarkerSymbol::Diamond,
        MarkerSymbol::Square,
        MarkerSymbol::Triangle,
        MarkerSymbol::X,
        MarkerSymbol::Star,
        MarkerSymbol::Circle,
        MarkerSymbol::Dash,
        MarkerSymbol::Dot,
        MarkerSymbol::Plus,
    ];
    CYCLE[i % 9]
}

/// The symbol, size (px) and colour of a series' marker, or `None` for no marker. `on` is whether
/// the group shows markers unless told otherwise.
pub(super) fn marker_of(
    m: &ChartModel,
    s: &Series,
    mk: Option<&Marker>,
    i: usize,
    on: bool,
) -> Option<(MarkerSymbol, f64, Rgba)> {
    let mk = mk.or(s.marker.as_ref());
    let symbol = match mk.map(|k| k.symbol) {
        Some(MarkerSymbol::None) => return None,
        Some(MarkerSymbol::Auto) | None => {
            if !on {
                return None;
            }
            auto_symbol(i)
        }
        Some(sym) => sym,
    };
    let size = mk
        .and_then(|k| k.size_pt)
        .filter(|v| v.is_finite())
        .unwrap_or(5.0)
        .clamp(2.0, 72.0)
        * 96.0
        / 72.0;
    let color = mk
        .and_then(|k| k.fill.as_ref())
        .and_then(fill_color)
        .or_else(|| mk.and_then(|k| k.line.color))
        .or_else(|| s.line.as_ref().and_then(|l| l.color))
        .unwrap_or_else(|| series_color(m, i));
    Some((symbol, size, color))
}

/// Draws one marker centred at `(x, y)`.
pub(super) fn draw_marker(
    o: &mut Out,
    x: f64,
    y: f64,
    sym: MarkerSymbol,
    size: f64,
    color: Rgba,
    custom: Option<&Marker>,
) {
    let r = size / 2.0;
    let fill = custom
        .and_then(|k| k.fill.clone())
        .unwrap_or(Fill::Solid(color));
    let outline = match custom {
        Some(k) => line_if_set(Some(&k.line), fill_color(&fill).unwrap_or(color), 9525.0),
        None => None,
    };
    let stroke = Line::solid(1.5 * 9525.0, color);
    match sym {
        MarkerSymbol::Circle => o.ellipse(x, y, r, r, &fill, outline.as_ref()),
        MarkerSymbol::Square => o.rect(x - r, y - r, size, size, &fill, outline.as_ref()),
        MarkerSymbol::Diamond => o.poly(
            &[(x, y - r), (x + r, y), (x, y + r), (x - r, y)],
            true,
            &fill,
            outline.as_ref(),
        ),
        MarkerSymbol::Triangle => o.poly(
            &[(x, y - r), (x + r, y + r), (x - r, y + r)],
            true,
            &fill,
            outline.as_ref(),
        ),
        MarkerSymbol::X => {
            o.seg(x - r, y - r, x + r, y + r, &stroke);
            o.seg(x - r, y + r, x + r, y - r, &stroke);
        }
        MarkerSymbol::Plus => {
            o.seg(x - r, y, x + r, y, &stroke);
            o.seg(x, y - r, x, y + r, &stroke);
        }
        MarkerSymbol::Star => {
            o.seg(x - r, y - r, x + r, y + r, &stroke);
            o.seg(x - r, y + r, x + r, y - r, &stroke);
            o.seg(x, y - r, x, y + r, &stroke);
        }
        MarkerSymbol::Dash => o.rect(x - r, y - r / 3.0, size, 2.0 * r / 3.0, &fill, None),
        MarkerSymbol::Dot => o.rect(x - r / 2.0, y - r / 2.0, r, r, &fill, None),
        MarkerSymbol::Auto | MarkerSymbol::None => {}
    }
}

// ---------------------------------------------------------------------------------------------
// The whole chart
// ---------------------------------------------------------------------------------------------

pub(super) fn draw(o: &mut Out, m: &ChartModel) {
    let (w, h) = (o.w, o.h);
    if let Some(f) = &m.chart_fill {
        let ln = line_if_set(m.chart_line.as_ref(), Rgba::BLACK, 9525.0);
        o.rect(0.0, 0.0, w, h, f, ln.as_ref());
    } else if let Some(s) = &m.chart_line {
        let ln = line_if_set(Some(s), Rgba::BLACK, 9525.0);
        o.rect(0.0, 0.0, w, h, &Fill::None, ln.as_ref());
    }

    let mut avail = Rect {
        x: PAD,
        y: PAD,
        w: w - 2.0 * PAD,
        h: h - 2.0 * PAD,
    };

    // Title: makes room unless it overlays the plot.
    let title = m.title.as_ref().filter(|t| !t.text.trim().is_empty());
    let mut title_draw: Option<(f64, f64, RStyle, String)> = None;
    if let Some(t) = title {
        let st = resolve(m, &t.style, TITLE_PT, true);
        let (tw, th) = measure(&st, &t.text);
        let (x, y) = match t.layout {
            Some(l) if l.x.is_some() || l.y.is_some() => (
                l.x.unwrap_or(0.0).clamp(0.0, 1.0) * w,
                l.y.unwrap_or(0.0).clamp(0.0, 1.0) * h,
            ),
            _ => ((w - tw) / 2.0, PAD),
        };
        if !t.overlay && t.layout.is_none() {
            let used = y + th + 4.0 - avail.y;
            avail.y += used.max(0.0);
            avail.h -= used.max(0.0);
        }
        title_draw = Some((x, y, st, t.text.clone()));
    }

    // Legend.
    let mut legend_draw: Option<LegendPlan> = None;
    if let Some(l) = &m.legend {
        let entries = legend_entries(m, l);
        if !entries.is_empty() {
            let plan = plan_legend(m, l, entries, &avail, w, h, title_draw.as_ref());
            if !l.overlay && l.layout.is_none() {
                match l.pos {
                    LegendPos::Right | LegendPos::TopRight => {
                        let used = plan.box_.w + PAD;
                        avail.w -= used;
                    }
                    LegendPos::Left => {
                        let used = plan.box_.w + PAD;
                        avail.x += used;
                        avail.w -= used;
                    }
                    LegendPos::Top => {
                        let used = plan.box_.h + 4.0;
                        avail.y += used;
                        avail.h -= used;
                    }
                    LegendPos::Bottom => {
                        let used = plan.box_.h + 4.0;
                        avail.h -= used;
                    }
                }
            }
            legend_draw = Some(plan);
        }
    }
    avail.w = avail.w.max(10.0);
    avail.h = avail.h.max(10.0);

    // The plot.
    let plot_kind = plot_kind(m);
    let manual = m.plot_layout;
    match plot_kind {
        PlotKind::Pie => pie::draw(o, m, outer_rect(manual, avail, w, h)),
        PlotKind::Radar => radar::draw(o, m, outer_rect(manual, avail, w, h)),
        PlotKind::Cartesian => cartesian::draw(o, m, avail, manual),
    }

    if let Some((x, y, st, text)) = title_draw {
        let (tw, _) = measure(&st, &text);
        // Multi-line titles are centred on their own box.
        o.text(
            x + tw / 2.0,
            y,
            HAlign::Center,
            VAlign::Top,
            &text,
            &st,
            0.0,
        );
    }
    if let Some(p) = legend_draw {
        draw_legend(o, m, &p);
    }
}

/// Which family of plot the chart is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlotKind {
    Cartesian,
    Pie,
    Radar,
}

fn plot_kind(m: &ChartModel) -> PlotKind {
    let any = |k: GroupKind| m.groups.iter().any(|g| g.kind == k);
    if any(GroupKind::Pie) || any(GroupKind::Doughnut) {
        PlotKind::Pie
    } else if any(GroupKind::Radar) && !m.groups.iter().any(is_cartesian) {
        PlotKind::Radar
    } else {
        PlotKind::Cartesian
    }
}

pub(super) fn is_cartesian(g: &ChartGroup) -> bool {
    !matches!(
        g.kind,
        GroupKind::Pie | GroupKind::Doughnut | GroupKind::Radar
    )
}

/// The plot rectangle of a plot without axes (pie, radar): the manual layout, or what is left.
fn outer_rect(manual: Option<ManualLayout>, avail: Rect, w: f64, h: f64) -> Rect {
    match manual {
        Some(l) => apply_manual(&l, avail, w, h),
        None => avail,
    }
}

/// A manual layout applied to a default rectangle (fractions of the chart box).
pub(super) fn apply_manual(l: &ManualLayout, dflt: Rect, w: f64, h: f64) -> Rect {
    let f = |v: Option<f64>| v.filter(|v| v.is_finite());
    let x = match f(l.x) {
        Some(v) if l.x_edge => v * w,
        Some(v) => dflt.x + v * w,
        None => dflt.x,
    };
    let y = match f(l.y) {
        Some(v) if l.y_edge => v * h,
        Some(v) => dflt.y + v * h,
        None => dflt.y,
    };
    let rw = match f(l.w) {
        Some(v) if l.w_edge => v * w - x,
        Some(v) => v * w,
        None => dflt.w,
    };
    let rh = match f(l.h) {
        Some(v) if l.h_edge => v * h - y,
        Some(v) => v * h,
        None => dflt.h,
    };
    Rect {
        x,
        y,
        w: rw.clamp(8.0, w.max(8.0)),
        h: rh.clamp(8.0, h.max(8.0)),
    }
}

// ---------------------------------------------------------------------------------------------
// Legend
// ---------------------------------------------------------------------------------------------

struct LegendPlan {
    box_: Rect,
    entries: Vec<Entry>,
    /// `(x, y)` of each entry's left / vertical centre, px.
    at: Vec<(f64, f64)>,
    style: RStyle,
    key_w: f64,
    fill: Option<Fill>,
    line: Option<Line>,
}

fn legend_entries(m: &ChartModel, l: &Legend) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut ordinal = 0usize;
    let mut reverse = false;
    for g in &m.groups {
        match g.kind {
            GroupKind::Pie | GroupKind::Doughnut if g.vary_colors => {
                if let Some(s) = g.series.first() {
                    let n = s.values.len().max(s.cats.len());
                    for i in 0..n {
                        if out.len() >= MAX_LEGEND_ENTRIES {
                            break;
                        }
                        let pf = s.points.iter().find(|p| p.idx == i);
                        let fill = pf
                            .and_then(|p| p.fill.clone())
                            .unwrap_or_else(|| Fill::Solid(point_color(m, i, n)));
                        let line =
                            pf.and_then(|p| line_if_set(p.line.as_ref(), Rgba::WHITE, 9525.0));
                        out.push(Entry {
                            text: s
                                .cats
                                .get(i)
                                .cloned()
                                .unwrap_or_else(|| (i + 1).to_string()),
                            key: Key::Box(fill, line),
                        });
                    }
                }
            }
            _ => {
                if matches!(g.grouping, Grouping::Stacked | Grouping::PercentStacked)
                    && matches!(g.kind, GroupKind::Area)
                    || (matches!(g.grouping, Grouping::Stacked | Grouping::PercentStacked)
                        && g.kind == GroupKind::Bar
                        && g.bar_dir == super::BarDir::Col)
                {
                    reverse = true;
                }
                for s in &g.series {
                    if out.len() >= MAX_LEGEND_ENTRIES {
                        break;
                    }
                    let text = s
                        .name
                        .clone()
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| format!("Series{}", ordinal + 1));
                    let key = match g.kind {
                        GroupKind::Line | GroupKind::Scatter | GroupKind::Radar => {
                            let color = s
                                .line
                                .as_ref()
                                .and_then(|l| l.color)
                                .unwrap_or_else(|| series_color(m, ordinal));
                            let has_line = !matches!(g.kind, GroupKind::Scatter)
                                || s.line.as_ref().is_none_or(|l| !l.none);
                            let line = if has_line && !s.line.as_ref().is_some_and(|l| l.none) {
                                line_of(s.line.as_ref(), color, 28575.0)
                            } else {
                                None
                            };
                            let on = match g.kind {
                                GroupKind::Line => g.markers,
                                GroupKind::Scatter => !matches!(
                                    g.scatter_style,
                                    super::ScatterStyle::Line | super::ScatterStyle::Smooth
                                ),
                                _ => g.radar_style == super::RadarStyle::Marker,
                            };
                            Key::Line(line, marker_of(m, s, None, ordinal, on))
                        }
                        GroupKind::Bubble => Key::Box(series_fill(m, s, ordinal), None),
                        _ => Key::Box(
                            series_fill(m, s, ordinal),
                            line_if_set(s.line.as_ref(), Rgba::BLACK, 9525.0),
                        ),
                    };
                    out.push(Entry { text, key });
                    ordinal += 1;
                }
            }
        }
    }
    if reverse
        && matches!(
            l.pos,
            LegendPos::Right | LegendPos::Left | LegendPos::TopRight
        )
    {
        out.reverse();
    }
    let deleted = &l.deleted;
    if !deleted.is_empty() {
        let mut i = 0usize;
        out.retain(|_| {
            let keep = !deleted.contains(&i);
            i += 1;
            keep
        });
    }
    out
}

fn plan_legend(
    m: &ChartModel,
    l: &Legend,
    entries: Vec<Entry>,
    avail: &Rect,
    w: f64,
    h: f64,
    title: Option<&(f64, f64, RStyle, String)>,
) -> LegendPlan {
    let style = resolve(m, &l.style, TEXT_PT, false);
    let key_w = if entries.iter().any(|e| matches!(e.key, Key::Line(..))) {
        style.size_px() * 2.4
    } else {
        style.size_px() * 0.9
    };
    let gap = style.size_px() * 0.4;
    let row_h = style.line_h() + 2.0;
    let widths: Vec<f64> = entries
        .iter()
        .map(|e| key_w + gap + measure(&style, &e.text).0 + 2.0)
        .collect();
    let vertical = matches!(
        l.pos,
        LegendPos::Right | LegendPos::Left | LegendPos::TopRight
    );
    let pad = 4.0;
    let mut at: Vec<(f64, f64)> = Vec::with_capacity(entries.len());
    let (bw, bh);
    if vertical {
        let max_h = (avail.h).max(row_h);
        let rows_fit = (((max_h - 2.0 * pad) / row_h).floor() as usize).max(1);
        let n = entries.len().min(rows_fit);
        let wmax = widths.iter().copied().fold(0.0, f64::max);
        bw = wmax + 2.0 * pad;
        bh = n as f64 * row_h + 2.0 * pad;
        for i in 0..entries.len() {
            at.push((pad, pad + (i as f64 + 0.5) * row_h));
        }
    } else {
        let max_w = avail.w.max(40.0);
        // Greedy rows.
        let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
        let mut cur = 2.0 * pad;
        for (i, wd) in widths.iter().enumerate() {
            let add = wd
                + if rows.last().is_some_and(|r| r.is_empty()) {
                    0.0
                } else {
                    gap * 2.0
                };
            if cur + add > max_w && !rows.last().is_none_or(|r| r.is_empty()) {
                rows.push(Vec::new());
                cur = 2.0 * pad;
            }
            let add = wd
                + if rows.last().is_some_and(|r| r.is_empty()) {
                    0.0
                } else {
                    gap * 2.0
                };
            cur += add;
            if let Some(r) = rows.last_mut() {
                r.push(i);
            }
        }
        // At most as many rows as fit a third of the chart.
        let max_rows = (((h / 3.0 - 2.0 * pad) / row_h).floor() as usize).max(1);
        rows.truncate(max_rows);
        let row_w: Vec<f64> = rows
            .iter()
            .map(|r| {
                r.iter().map(|&i| widths[i]).sum::<f64>()
                    + gap * 2.0 * (r.len().saturating_sub(1)) as f64
            })
            .collect();
        bw = row_w.iter().copied().fold(0.0, f64::max) + 2.0 * pad;
        bh = rows.len() as f64 * row_h + 2.0 * pad;
        at = vec![(f64::NAN, f64::NAN); entries.len()];
        for (ri, r) in rows.iter().enumerate() {
            let mut x = pad + (bw - 2.0 * pad - row_w[ri]) / 2.0;
            for &i in r {
                at[i] = (x, pad + (ri as f64 + 0.5) * row_h);
                x += widths[i] + gap * 2.0;
            }
        }
    }
    let bw = bw.min(w);
    let bh = bh.min(h);
    // Position.
    let title_bottom = title.map(|t| t.1 + measure(&t.2, &t.3).1).unwrap_or(PAD);
    let top_of_free = title_bottom.max(PAD);
    let (x, y) = match l.layout {
        Some(lay) if lay.x.is_some() || lay.y.is_some() => {
            let r = apply_manual(
                &lay,
                Rect {
                    x: avail.x,
                    y: avail.y,
                    w: bw,
                    h: bh,
                },
                w,
                h,
            );
            (r.x, r.y)
        }
        _ => match l.pos {
            LegendPos::Right => (
                w - PAD - bw,
                top_of_free + ((h - PAD - top_of_free) - bh) / 2.0,
            ),
            LegendPos::Left => (PAD, top_of_free + ((h - PAD - top_of_free) - bh) / 2.0),
            LegendPos::TopRight => (w - PAD - bw, top_of_free + 2.0),
            LegendPos::Top => ((w - bw) / 2.0, top_of_free + 2.0),
            LegendPos::Bottom => ((w - bw) / 2.0, h - PAD - bh),
        },
    };
    let n_drawn = if vertical {
        (((bh - 2.0 * pad) / row_h).floor() as usize).max(1)
    } else {
        entries.len()
    };
    let mut entries = entries;
    if vertical && entries.len() > n_drawn {
        entries.truncate(n_drawn);
        at.truncate(n_drawn);
    }
    LegendPlan {
        box_: Rect { x, y, w: bw, h: bh },
        entries,
        at,
        style,
        key_w,
        fill: l.fill.clone(),
        // A legend outline needs a colour of its own: `a:ln` without one is the automatic
        // outline, which the chart styles leave off.
        line: line_if_set(
            l.line.as_ref().filter(|s| s.color.is_some()),
            Rgba::BLACK,
            9525.0,
        ),
    }
}

fn draw_legend(o: &mut Out, _m: &ChartModel, p: &LegendPlan) {
    let b = p.box_;
    if p.fill.is_some() || p.line.is_some() {
        o.rect(
            b.x,
            b.y,
            b.w,
            b.h,
            p.fill.as_ref().unwrap_or(&Fill::None),
            p.line.as_ref(),
        );
    }
    let gap = p.style.size_px() * 0.4;
    for (e, &(ex, ey)) in p.entries.iter().zip(&p.at) {
        if !ex.is_finite() {
            continue;
        }
        let x = b.x + ex;
        let y = b.y + ey;
        let ks = p.style.size_px() * 0.8;
        match &e.key {
            Key::Box(f, ln) => {
                o.rect(
                    x + (p.key_w - ks) / 2.0,
                    y - ks / 2.0,
                    ks,
                    ks,
                    f,
                    ln.as_ref(),
                );
            }
            Key::Line(ln, mk) => {
                if let Some(l) = ln {
                    o.seg(x, y, x + p.key_w, y, l);
                }
                if let Some((sym, size, color)) = mk {
                    let size = size.min(p.style.size_px());
                    draw_marker(o, x + p.key_w / 2.0, y, *sym, size, *color, None);
                }
            }
        }
        o.text(
            x + p.key_w + gap,
            y,
            HAlign::Left,
            VAlign::Middle,
            &e.text,
            &p.style,
            0.0,
        );
    }
}
