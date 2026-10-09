//! Tests of the chart drawing: shape counts and geometry, scaling, number formats, legends, data
//! labels, budgets and hostile input.

use super::*;

pub(super) const W: f64 = 480.0;
pub(super) const H: f64 = 300.0;

pub(super) fn emu(px: f64) -> f64 {
    px * EMU_PER_PX
}

pub(super) fn ser(name: &str, cats: &[&str], vals: &[f64]) -> Series {
    Series {
        name: Some(name.into()),
        cats: cats.iter().map(|c| c.to_string()).collect(),
        values: vals.iter().map(|v| Some(*v)).collect(),
        ..Series::default()
    }
}

pub(super) fn colored(mut s: Series, c: Rgba) -> Series {
    s.fill = Some(Fill::Solid(c));
    s
}

pub(super) fn cat_ax() -> Axis {
    Axis {
        id: 1,
        kind: AxisKind::Cat,
        cross_ax: 2,
        pos: AxisPos::Bottom,
        ..Axis::default()
    }
}

pub(super) fn val_ax() -> Axis {
    Axis {
        id: 2,
        kind: AxisKind::Val,
        cross_ax: 1,
        pos: AxisPos::Left,
        major_grid: Some(Stroke::default()),
        ..Axis::default()
    }
}

pub(super) fn grp(kind: GroupKind, series: Vec<Series>) -> ChartGroup {
    ChartGroup {
        kind,
        series,
        axis_ids: vec![1, 2],
        grouping: if kind == GroupKind::Bar {
            Grouping::Clustered
        } else {
            Grouping::Standard
        },
        ..ChartGroup::default()
    }
}

pub(super) fn model(groups: Vec<ChartGroup>) -> ChartModel {
    ChartModel {
        groups,
        axes: vec![cat_ax(), val_ax()],
        ..ChartModel::default()
    }
}

/// A model whose value axis is fixed at `0..max` with a unit of `unit`.
pub(super) fn fixed(groups: Vec<ChartGroup>, max: f64, unit: f64) -> ChartModel {
    let mut m = model(groups);
    m.axes[1].min = Some(0.0);
    m.axes[1].max = Some(max);
    m.axes[1].major_unit = Some(unit);
    m
}

pub(super) fn draw(m: &ChartModel) -> Vec<Item> {
    draw_chart(m, emu(W), emu(H)).0
}

/// Every shape, flattened out of groups.
pub(super) fn shapes(items: &[Item]) -> Vec<&ShapeItem> {
    let mut v = Vec::new();
    for it in items {
        match it {
            Item::Shape(s) => v.push(s),
            Item::Group(g) => v.extend(shapes(&g.items)),
            Item::Picture(_) => {}
        }
    }
    v
}

pub(super) fn solid(s: &ShapeItem) -> Option<Rgba> {
    match &s.fill {
        Fill::Solid(c) => Some(*c),
        _ => None,
    }
}

/// Rectangles (px: x, y, w, h) filled with exactly `c`.
pub(super) fn rects_of(items: &[Item], c: Rgba) -> Vec<(f64, f64, f64, f64)> {
    shapes(items)
        .into_iter()
        .filter(|s| s.geom == Geometry::Rect && solid(s) == Some(c) && s.text.is_none())
        .map(|s| {
            (
                s.xfrm.x / EMU_PER_PX,
                s.xfrm.y / EMU_PER_PX,
                s.xfrm.w / EMU_PER_PX,
                s.xfrm.h / EMU_PER_PX,
            )
        })
        .collect()
}

/// All text strings (one per paragraph) with the shape box (px: x, y, w, h) and rotation.
pub(super) type TextBox = (String, (f64, f64, f64, f64), f64);

pub(super) fn texts(items: &[Item]) -> Vec<TextBox> {
    let mut out = Vec::new();
    for s in shapes(items) {
        if let Some(t) = &s.text {
            let txt = t
                .paragraphs
                .iter()
                .map(|p| p.runs.iter().map(|r| r.text.clone()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            out.push((
                txt,
                (
                    s.xfrm.x / EMU_PER_PX,
                    s.xfrm.y / EMU_PER_PX,
                    s.xfrm.w / EMU_PER_PX,
                    s.xfrm.h / EMU_PER_PX,
                ),
                s.xfrm.rot_deg,
            ));
        }
    }
    out
}

pub(super) fn text_strings(items: &[Item]) -> Vec<String> {
    texts(items).into_iter().map(|t| t.0).collect()
}

/// The absolute px points of a path shape's commands (control points included, arcs excluded).
pub(super) fn path_pts(s: &ShapeItem) -> Vec<(f64, f64)> {
    let Geometry::Paths(ps) = &s.geom else {
        return Vec::new();
    };
    let (ox, oy) = (s.xfrm.x, s.xfrm.y);
    let mut v = Vec::new();
    for p in ps {
        for c in &p.cmds {
            let mut push = |q: &Pt| v.push(((ox + q.x) / EMU_PER_PX, (oy + q.y) / EMU_PER_PX));
            match c {
                PathCmd::MoveTo(q) | PathCmd::LineTo(q) => push(q),
                PathCmd::QuadTo(a, b) => {
                    push(a);
                    push(b)
                }
                PathCmd::CubicTo(a, b, c) => {
                    push(a);
                    push(b);
                    push(c)
                }
                _ => {}
            }
        }
    }
    v
}

pub(super) fn cmds(s: &ShapeItem) -> Vec<PathCmd> {
    match &s.geom {
        Geometry::Paths(ps) => ps.iter().flat_map(|p| p.cmds.clone()).collect(),
        _ => Vec::new(),
    }
}

/// Straight two-point lines `(x0, y0, x1, y1)` in px.
pub(super) fn segs(items: &[Item]) -> Vec<(f64, f64, f64, f64, Rgba)> {
    shapes(items)
        .into_iter()
        .filter_map(|s| {
            let c = cmds(s);
            if c.len() != 2 {
                return None;
            }
            let p = path_pts(s);
            let col = match &s.line.as_ref()?.fill {
                Fill::Solid(c) => *c,
                _ => return None,
            };
            Some((p[0].0, p[0].1, p[1].0, p[1].1, col))
        })
        .collect()
}

/// The plot rectangle `(x0, y0, x1, y1)`, from the horizontal gridlines.
pub(super) fn plot_rect(items: &[Item]) -> (f64, f64, f64, f64) {
    let hs: Vec<_> = segs(items)
        .into_iter()
        .filter(|s| {
            s.4 == Rgba::rgb(0x86, 0x86, 0x86) && (s.1 - s.3).abs() < 0.01 && s.2 - s.0 > 50.0
        })
        .collect();
    assert!(!hs.is_empty(), "no horizontal gridline");
    let x0 = hs.iter().map(|s| s.0.min(s.2)).fold(f64::MAX, f64::min);
    let x1 = hs.iter().map(|s| s.0.max(s.2)).fold(f64::MIN, f64::max);
    let y0 = hs.iter().map(|s| s.1).fold(f64::MAX, f64::min);
    let y1 = hs.iter().map(|s| s.1).fold(f64::MIN, f64::max);
    (x0, y0, x1, y1)
}

pub(super) const A: Rgba = Rgba::rgb(10, 20, 30);
pub(super) const B: Rgba = Rgba::rgb(40, 50, 60);
pub(super) const C: Rgba = Rgba::rgb(70, 80, 90);

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.6
}

#[test]
fn smoke() {
    let (items, trunc) = draw_chart(&ChartModel::default(), 4_000_000.0, 3_000_000.0);
    assert!(items.is_empty());
    assert!(!trunc);
}

#[test]
fn tiny_boxes_draw_nothing() {
    let m = model(vec![grp(GroupKind::Bar, vec![ser("a", &["x"], &[1.0])])]);
    assert!(draw_chart(&m, 0.0, 0.0).0.is_empty());
    assert!(draw_chart(&m, f64::NAN, 100.0).0.is_empty());
    assert!(draw_chart(&m, 10.0, 10.0).0.is_empty());
    assert!(!draw_chart(&m, f64::INFINITY, f64::INFINITY).1);
}

// ---- columns and bars ----

#[test]
fn clustered_columns_proportional() {
    let g = grp(
        GroupKind::Bar,
        vec![
            colored(ser("a", &["p", "q", "r"], &[5.0, 10.0, 20.0]), A),
            colored(ser("b", &["p", "q", "r"], &[10.0, 20.0, 5.0]), B),
        ],
    );
    let items = draw(&fixed(vec![g], 20.0, 5.0));
    let (_, py0, _, py1) = plot_rect(&items);
    let a = rects_of(&items, A);
    let b = rects_of(&items, B);
    assert_eq!((a.len(), b.len()), (3, 3));
    let unit = (py1 - py0) / 20.0;
    for (r, v) in a.iter().zip([5.0, 10.0, 20.0]) {
        assert!(approx(r.3, v * unit), "{} vs {}", r.3, v * unit);
        // All bars stand on the axis.
        assert!(approx(r.1 + r.3, py1));
    }
    for (r, v) in b.iter().zip([10.0, 20.0, 5.0]) {
        assert!(approx(r.3, v * unit));
    }
    // The series of a cluster are side by side, in order, and of the same width.
    assert!(a[0].0 + a[0].2 <= b[0].0 + 0.01);
    assert!(approx(a[0].2, b[0].2));
    assert!(a[1].0 > b[0].0);
}

#[test]
fn gap_width_and_overlap_set_the_bar_width() {
    let mk = |gap: f64, ov: f64| {
        let mut g = grp(
            GroupKind::Bar,
            vec![
                colored(ser("a", &["p", "q"], &[5.0, 5.0]), A),
                colored(ser("b", &["p", "q"], &[5.0, 5.0]), B),
            ],
        );
        g.gap_width = gap;
        g.overlap = ov;
        let items = draw(&fixed(vec![g], 10.0, 5.0));
        (rects_of(&items, A), rects_of(&items, B), plot_rect(&items))
    };
    let (a, b, pr) = mk(100.0, 0.0);
    let slot = (pr.2 - pr.0) / 2.0;
    // Two bars and a gap of one bar: slot = 3 bars.
    assert!(approx(a[0].2, slot / 3.0), "{} {}", a[0].2, slot / 3.0);
    assert!(approx(b[0].0, a[0].0 + a[0].2));
    // Full overlap: the bars coincide and the slot is 1 + gap bars.
    let (a, b, pr) = mk(50.0, 100.0);
    let slot = (pr.2 - pr.0) / 2.0;
    assert!(approx(a[0].2, slot / 1.5));
    assert!(approx(a[0].0, b[0].0));
    // Negative overlap leaves a space between the bars.
    let (a, b, _) = mk(0.0, -50.0);
    assert!(b[0].0 > a[0].0 + a[0].2 + 0.5);
}

#[test]
fn stacked_columns_add_up() {
    let cats = ["p", "q"];
    let mut g = grp(
        GroupKind::Bar,
        vec![
            colored(ser("a", &cats, &[3.0, 1.0]), A),
            colored(ser("b", &cats, &[4.0, 2.0]), B),
            colored(ser("c", &cats, &[5.0, 3.0]), C),
        ],
    );
    g.grouping = Grouping::Stacked;
    g.overlap = 100.0;
    let items = draw(&fixed(vec![g], 20.0, 5.0));
    let (_, py0, _, py1) = plot_rect(&items);
    let unit = (py1 - py0) / 20.0;
    let (a, b, c) = (
        rects_of(&items, A),
        rects_of(&items, B),
        rects_of(&items, C),
    );
    for i in 0..2 {
        // Each segment sits on the previous one.
        assert!(approx(a[i].1 + a[i].3, py1));
        assert!(approx(b[i].1 + b[i].3, a[i].1));
        assert!(approx(c[i].1 + c[i].3, b[i].1));
        // Same column.
        assert!(approx(a[i].0, b[i].0) && approx(b[i].0, c[i].0));
    }
    // The tops are at the sums.
    assert!(approx(py1 - c[0].1, 12.0 * unit));
    assert!(approx(py1 - c[1].1, 6.0 * unit));
}

#[test]
fn stacked_negative_values_go_below() {
    let mut g = grp(
        GroupKind::Bar,
        vec![
            colored(ser("a", &["p"], &[4.0]), A),
            colored(ser("b", &["p"], &[-3.0]), B),
        ],
    );
    g.grouping = Grouping::Stacked;
    let mut m = model(vec![g]);
    m.axes[1].min = Some(-5.0);
    m.axes[1].max = Some(5.0);
    m.axes[1].major_unit = Some(5.0);
    let items = draw(&m);
    let (_, py0, _, py1) = plot_rect(&items);
    let zero = (py0 + py1) / 2.0;
    let a = rects_of(&items, A)[0];
    let b = rects_of(&items, B)[0];
    assert!(approx(a.1 + a.3, zero));
    assert!(approx(b.1, zero));
    assert!(approx(b.3, 3.0 * (py1 - py0) / 10.0));
}

#[test]
fn percent_stacked_columns_fill_the_plot() {
    let cats = ["p", "q"];
    let mut g = grp(
        GroupKind::Bar,
        vec![
            colored(ser("a", &cats, &[1.0, 30.0]), A),
            colored(ser("b", &cats, &[3.0, 10.0]), B),
        ],
    );
    g.grouping = Grouping::PercentStacked;
    let items = draw(&model(vec![g]));
    let (_, py0, _, py1) = plot_rect(&items);
    let (a, b) = (rects_of(&items, A), rects_of(&items, B));
    for i in 0..2 {
        // The column reaches 100 %: the top is the top of the plot.
        assert!(approx(b[i].1, py0), "{} vs {}", b[i].1, py0);
        assert!(approx(a[i].1 + a[i].3, py1));
    }
    assert!(approx(a[0].3, 0.25 * (py1 - py0)));
    assert!(approx(a[1].3, 0.75 * (py1 - py0)));
    // The axis is labelled in percent.
    let t = text_strings(&items);
    assert!(
        t.contains(&"0%".to_string()) && t.contains(&"100%".to_string()),
        "{t:?}"
    );
}

#[test]
fn horizontal_bars_grow_to_the_right_first_category_at_the_bottom() {
    let mut g = grp(
        GroupKind::Bar,
        vec![colored(ser("a", &["p", "q"], &[5.0, 10.0]), A)],
    );
    g.bar_dir = BarDir::Bar;
    let mut m = model(vec![g]);
    m.axes[0].pos = AxisPos::Left;
    m.axes[1].pos = AxisPos::Bottom;
    m.axes[1].min = Some(0.0);
    m.axes[1].max = Some(10.0);
    m.axes[1].major_unit = Some(5.0);
    let items = draw(&m);
    let r = rects_of(&items, A);
    assert_eq!(r.len(), 2);
    // p is the lower bar and half the length of q.
    assert!(r[0].1 > r[1].1);
    assert!(approx(r[1].2, 2.0 * r[0].2));
    assert!(approx(r[0].0, r[1].0));
}

#[test]
fn reversed_category_axis_flips_the_columns() {
    let g = grp(
        GroupKind::Bar,
        vec![colored(ser("a", &["p", "q"], &[5.0, 10.0]), A)],
    );
    let mut m = fixed(vec![g], 10.0, 5.0);
    let normal = rects_of(&draw(&m), A);
    m.axes[0].reversed = true;
    let flipped = rects_of(&draw(&m), A);
    assert!(normal[0].0 < normal[1].0);
    assert!(flipped[0].0 > flipped[1].0);
    assert!(approx(normal[0].3, flipped[0].3));
}

#[test]
fn varied_colours_for_a_single_series() {
    let mut g = grp(
        GroupKind::Bar,
        vec![ser("a", &["p", "q", "r"], &[1.0, 2.0, 3.0])],
    );
    g.vary_colors = true;
    let m = fixed(vec![g], 4.0, 1.0);
    let items = draw(&m);
    let cols: Vec<Rgba> = shapes(&items)
        .into_iter()
        .filter(|s| s.geom == Geometry::Rect && s.text.is_none() && s.fill.is_visible())
        .filter_map(solid)
        .collect();
    for i in 0..3 {
        assert!(cols.contains(&layout::series_color(&m, i)), "colour {i}");
    }
}

#[test]
fn point_formats_override_the_series() {
    let mut s = colored(ser("a", &["p", "q"], &[1.0, 2.0]), A);
    s.points.push(PointFmt {
        idx: 1,
        fill: Some(Fill::Solid(B)),
        ..PointFmt::default()
    });
    let items = draw(&fixed(vec![grp(GroupKind::Bar, vec![s])], 2.0, 1.0));
    assert_eq!(rects_of(&items, A).len(), 1);
    assert_eq!(rects_of(&items, B).len(), 1);
}

#[test]
fn bars_beyond_an_explicit_maximum_are_clipped_to_the_plot() {
    let g = grp(GroupKind::Bar, vec![colored(ser("a", &["p"], &[100.0]), A)]);
    let items = draw(&fixed(vec![g], 10.0, 5.0));
    let (_, py0, _, py1) = plot_rect(&items);
    let r = rects_of(&items, A)[0];
    assert!(approx(r.1, py0) && approx(r.1 + r.3, py1));
}

#[test]
fn missing_values_leave_no_bar() {
    let mut s = colored(ser("a", &["p", "q", "r"], &[1.0, 2.0, 3.0]), A);
    s.values[1] = None;
    let items = draw(&fixed(vec![grp(GroupKind::Bar, vec![s])], 4.0, 1.0));
    assert_eq!(rects_of(&items, A).len(), 2);
}

// ---- lines, areas ----

fn polylines(items: &[Item]) -> Vec<&ShapeItem> {
    shapes(items)
        .into_iter()
        .filter(|s| {
            matches!(&s.geom, Geometry::Paths(_))
                && !s.fill.is_visible()
                && s.line
                    .as_ref()
                    .is_some_and(|l| l.fill != Fill::Solid(Rgba::rgb(0x86, 0x86, 0x86)))
        })
        .collect()
}

#[test]
fn line_series_polyline_and_markers() {
    let mut s = ser("a", &["p", "q", "r", "s"], &[1.0, 3.0, 2.0, 4.0]);
    s.line = Some(Stroke {
        color: Some(A),
        ..Stroke::default()
    });
    s.marker = Some(Marker {
        symbol: MarkerSymbol::Square,
        size_pt: Some(6.0),
        fill: Some(Fill::Solid(A)),
        ..Marker::default()
    });
    let items = draw(&fixed(vec![grp(GroupKind::Line, vec![s])], 5.0, 1.0));
    let lines = polylines(&items);
    assert_eq!(lines.len(), 1);
    assert_eq!(cmds(lines[0]).len(), 4);
    assert_eq!(rects_of(&items, A).len(), 4, "markers");
    // The line is at the points: rising, falling, rising.
    let p = path_pts(lines[0]);
    assert!(p[1].1 < p[0].1 && p[2].1 > p[1].1 && p[3].1 < p[2].1);
}

#[test]
fn line_without_markers_when_the_group_says_so() {
    let mut g = grp(GroupKind::Line, vec![ser("a", &["p", "q"], &[1.0, 2.0])]);
    g.markers = false;
    let items = draw(&fixed(vec![g], 3.0, 1.0));
    assert!(shapes(&items).iter().all(|s| s.geom != Geometry::Ellipse));
    assert_eq!(polylines(&items).len(), 1);
}

#[test]
fn smooth_line_uses_curves() {
    let mut s = ser("a", &["p", "q", "r"], &[1.0, 3.0, 2.0]);
    s.smooth = Some(true);
    let mut g = grp(GroupKind::Line, vec![s]);
    g.markers = false;
    let items = draw(&fixed(vec![g], 4.0, 1.0));
    let l = polylines(&items);
    assert!(cmds(l[0]).iter().any(|c| matches!(c, PathCmd::CubicTo(..))));
}

#[test]
fn blank_points_break_the_line_unless_spanned() {
    let mut s = ser("a", &["p", "q", "r", "s", "t"], &[1.0, 2.0, 0.0, 2.0, 3.0]);
    s.values[2] = None;
    let mut g = grp(GroupKind::Line, vec![s]);
    g.markers = false;
    let mut m = fixed(vec![g], 4.0, 1.0);
    assert_eq!(polylines(&draw(&m)).len(), 2);
    m.disp_blanks_as = DispBlanks::Span;
    let drawn = draw(&m);
    let l = polylines(&drawn);
    assert_eq!(l.len(), 1);
    assert_eq!(cmds(l[0]).len(), 4);
    m.disp_blanks_as = DispBlanks::Zero;
    let drawn = draw(&m);
    let l = polylines(&drawn);
    assert_eq!(l.len(), 1);
    assert_eq!(cmds(l[0]).len(), 5);
}

#[test]
fn stacked_line_adds_up() {
    let cats = ["p", "q"];
    let mut g = grp(
        GroupKind::Line,
        vec![ser("a", &cats, &[1.0, 1.0]), ser("b", &cats, &[2.0, 2.0])],
    );
    g.grouping = Grouping::Stacked;
    g.markers = false;
    let items = draw(&fixed(vec![g], 4.0, 1.0));
    let (_, py0, _, py1) = plot_rect(&items);
    let l = polylines(&items);
    assert_eq!(l.len(), 2);
    let u = (py1 - py0) / 4.0;
    assert!(approx(path_pts(l[0])[0].1, py1 - u));
    assert!(approx(path_pts(l[1])[0].1, py1 - 3.0 * u));
}

#[test]
fn line_points_sit_between_ticks_or_on_them() {
    let mk = |between: bool| {
        let mut g = grp(
            GroupKind::Line,
            vec![ser("a", &["p", "q", "r"], &[1.0, 2.0, 3.0])],
        );
        g.markers = false;
        let mut m = fixed(vec![g], 4.0, 1.0);
        m.axes[1].between = between;
        let items = draw(&m);
        let (px0, _, px1, _) = plot_rect(&items);
        let l = polylines(&items);
        let p = path_pts(l[0]);
        (px0, px1, p)
    };
    let (x0, x1, p) = mk(true);
    assert!(approx(p[0].0, x0 + (x1 - x0) / 6.0));
    let (x0, x1, p) = mk(false);
    assert!(approx(p[0].0, x0) && approx(p[2].0, x1));
}

#[test]
fn area_polygons_stack() {
    let cats = ["p", "q", "r"];
    let mut g = grp(
        GroupKind::Area,
        vec![
            colored(ser("a", &cats, &[1.0, 2.0, 1.0]), A),
            colored(ser("b", &cats, &[1.0, 1.0, 1.0]), B),
        ],
    );
    g.grouping = Grouping::Stacked;
    let mut m = fixed(vec![g], 4.0, 1.0);
    m.axes[1].between = false;
    let items = draw(&m);
    let polys: Vec<_> = shapes(&items)
        .into_iter()
        .filter(|s| matches!(solid(s), Some(c) if c == A || c == B))
        .collect();
    assert_eq!(polys.len(), 2);
    let (_, py0, _, py1) = plot_rect(&items);
    let u = (py1 - py0) / 4.0;
    // The second polygon's top is at 2, 3, 2 and its bottom follows the first one's top.
    let top_b: Vec<f64> = path_pts(polys[1])[..3].iter().map(|p| p.1).collect();
    assert!(approx(top_b[0], py1 - 2.0 * u));
    assert!(approx(top_b[1], py1 - 3.0 * u));
    let bottom_b: Vec<f64> = path_pts(polys[1])[3..6].iter().map(|p| p.1).collect();
    assert!(approx(bottom_b[1], py1 - 2.0 * u), "{bottom_b:?}");
}

#[test]
fn percent_area_reaches_the_top() {
    let cats = ["p", "q"];
    let mut g = grp(
        GroupKind::Area,
        vec![
            colored(ser("a", &cats, &[1.0, 3.0]), A),
            colored(ser("b", &cats, &[3.0, 1.0]), B),
        ],
    );
    g.grouping = Grouping::PercentStacked;
    let items = draw(&model(vec![g]));
    let (_, py0, _, _) = plot_rect(&items);
    let b = shapes(&items)
        .into_iter()
        .find(|s| solid(s) == Some(B))
        .unwrap();
    let top = path_pts(b)[0].1;
    assert!(approx(top, py0), "{top} vs {py0}");
}

// ---- scatter, bubble, radar ----

#[test]
fn scatter_markers_and_axis_scales() {
    let mut s = ser("y", &[], &[2.0, 4.0, 6.0]);
    s.x_values = vec![Some(10.0), Some(20.0), Some(30.0)];
    s.line = Some(Stroke {
        none: true,
        ..Stroke::default()
    });
    s.marker = Some(Marker {
        symbol: MarkerSymbol::Circle,
        size_pt: Some(7.0),
        ..Marker::default()
    });
    let mut g = grp(GroupKind::Scatter, vec![s]);
    g.scatter_style = ScatterStyle::LineMarker;
    let mut m = model(vec![g]);
    m.axes[0].kind = AxisKind::Val;
    m.axes[0].major_grid = None;
    let items = draw(&m);
    let dots = shapes(&items)
        .into_iter()
        .filter(|s| s.geom == Geometry::Ellipse)
        .count();
    assert_eq!(dots, 3);
    // No line (it was switched off) and x labels are numbers, not categories.
    assert!(polylines(&items).is_empty());
    let t = text_strings(&items);
    assert!(t.iter().any(|s| s == "20"), "{t:?}");
}

#[test]
fn scatter_lines_smooth() {
    let mut s = ser("y", &[], &[1.0, 3.0, 2.0, 4.0]);
    s.x_values = (1..=4).map(|v| Some(v as f64)).collect();
    let mut g = grp(GroupKind::Scatter, vec![s]);
    g.scatter_style = ScatterStyle::SmoothMarker;
    let mut m = model(vec![g]);
    m.axes[0].kind = AxisKind::Val;
    let items = draw(&m);
    let l = polylines(&items);
    assert_eq!(l.len(), 1);
    assert!(cmds(l[0]).iter().any(|c| matches!(c, PathCmd::CubicTo(..))));
}

#[test]
fn bubble_area_is_proportional_to_the_size() {
    let mut s = ser("y", &[], &[1.0, 2.0]);
    s.x_values = vec![Some(1.0), Some(2.0)];
    s.sizes = vec![Some(1.0), Some(4.0)];
    let g = grp(GroupKind::Bubble, vec![s]);
    let mut m = model(vec![g]);
    m.axes[0].kind = AxisKind::Val;
    let items = draw(&m);
    let mut d: Vec<f64> = shapes(&items)
        .into_iter()
        .filter(|s| s.geom == Geometry::Ellipse)
        .map(|s| s.xfrm.w / EMU_PER_PX)
        .collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(d.len(), 2);
    // Area 4:1 means diameters 2:1.
    assert!(approx(d[1], 2.0 * d[0]), "{d:?}");
}

#[test]
fn radar_draws_polygons_and_category_labels() {
    let cats = ["a", "b", "c", "d", "e"];
    let mut g = grp(
        GroupKind::Radar,
        vec![ser("s", &cats, &[3.0, 4.0, 2.0, 5.0, 1.0])],
    );
    g.axis_ids = vec![1, 2];
    let mut m = model(vec![g]);
    m.axes[1].min = Some(0.0);
    m.axes[1].max = Some(5.0);
    m.axes[1].major_unit = Some(1.0);
    let items = draw(&m);
    let closed: Vec<_> = shapes(&items)
        .into_iter()
        .filter(|s| cmds(s).last() == Some(&PathCmd::Close))
        .collect();
    // Four inner grid polygons (1..5, without 0) plus the series: 5 + 1.
    assert_eq!(closed.len(), 6);
    let t = text_strings(&items);
    for c in cats {
        assert!(t.contains(&c.to_string()));
    }
    // The series polygon has one vertex per category.
    assert!(closed.iter().any(|s| cmds(s).len() == 6));
}

#[test]
fn filled_radar_has_a_fill() {
    let cats = ["a", "b", "c"];
    let mut g = grp(
        GroupKind::Radar,
        vec![colored(ser("s", &cats, &[1.0, 2.0, 3.0]), A)],
    );
    g.radar_style = RadarStyle::Filled;
    let items = draw(&model(vec![g]));
    assert_eq!(
        shapes(&items)
            .iter()
            .filter(|s| solid(s) == Some(A))
            .count(),
        1
    );
}

// ---- pie and doughnut ----

fn arcs(s: &ShapeItem) -> Vec<(f64, f64, f64, f64)> {
    cmds(s)
        .into_iter()
        .filter_map(|c| match c {
            PathCmd::ArcTo {
                wr,
                hr,
                st_deg,
                sw_deg,
            } => Some((wr / EMU_PER_PX, hr / EMU_PER_PX, st_deg, sw_deg)),
            _ => None,
        })
        .collect()
}

pub(super) fn pie_model(vals: &[f64], kind: GroupKind) -> ChartModel {
    let cats: Vec<String> = (0..vals.len()).map(|i| format!("c{i}")).collect();
    let cats: Vec<&str> = cats.iter().map(String::as_str).collect();
    let mut g = grp(kind, vec![ser("s", &cats, vals)]);
    g.vary_colors = true;
    g.axis_ids.clear();
    ChartModel {
        groups: vec![g],
        ..ChartModel::default()
    }
}

#[test]
fn pie_slice_angles_add_up_to_a_full_turn() {
    let items = draw(&pie_model(&[1.0, 2.0, 3.0, 4.0], GroupKind::Pie));
    let wedges: Vec<_> = shapes(&items)
        .into_iter()
        .filter(|s| !arcs(s).is_empty())
        .collect();
    assert_eq!(wedges.len(), 4);
    let sweeps: Vec<f64> = wedges.iter().map(|s| arcs(s)[0].3).collect();
    assert!(approx(sweeps.iter().sum::<f64>(), 360.0));
    assert!(approx(sweeps[0], 36.0) && approx(sweeps[3], 144.0));
    // The first slice starts at 12 o'clock (DrawingML -90 degrees) and the next one where it ended.
    assert!(approx(arcs(wedges[0])[0].2, -90.0));
    assert!(approx(arcs(wedges[1])[0].2, -90.0 + 36.0));
}

#[test]
fn pie_first_slice_angle() {
    let mut m = pie_model(&[1.0, 1.0], GroupKind::Pie);
    m.groups[0].first_slice_ang = 90.0;
    let items = draw(&m);
    let w: Vec<_> = shapes(&items)
        .into_iter()
        .filter(|s| !arcs(s).is_empty())
        .collect();
    assert!(approx(arcs(w[0])[0].2, 0.0));
    assert!(approx(arcs(w[1])[0].2, 180.0));
    // A turn past 360 wraps.
    m.groups[0].first_slice_ang = 450.0;
    let w2 = draw(&m);
    let s2: Vec<_> = shapes(&w2)
        .into_iter()
        .filter(|s| !arcs(s).is_empty())
        .collect();
    assert!(approx(arcs(s2[0])[0].2, 0.0));
}

#[test]
fn pie_with_one_slice_is_a_disc_and_zeros_have_no_slice() {
    let items = draw(&pie_model(&[5.0, 0.0, -3.0], GroupKind::Pie));
    let discs = shapes(&items)
        .into_iter()
        .filter(|s| s.geom == Geometry::Ellipse)
        .count();
    assert_eq!(discs, 1);
    assert!(draw(&pie_model(&[0.0, 0.0], GroupKind::Pie)).is_empty());
}

#[test]
fn pie_explosion_moves_the_slice_and_shrinks_the_pie() {
    let plain = draw(&pie_model(&[1.0, 1.0], GroupKind::Pie));
    let mut m = pie_model(&[1.0, 1.0], GroupKind::Pie);
    m.groups[0].series[0].points.push(PointFmt {
        idx: 0,
        explosion: Some(30.0),
        ..PointFmt::default()
    });
    let exploded = draw(&m);
    let r_of = |items: &[Item]| {
        arcs(
            shapes(items)
                .into_iter()
                .find(|s| !arcs(s).is_empty())
                .unwrap(),
        )[0]
        .0
    };
    assert!(approx(r_of(&plain) / r_of(&exploded), 1.3));
    // The exploded slice's apex is away from the other's.
    let apex = |items: &[Item], i: usize| {
        let w: Vec<_> = shapes(items)
            .into_iter()
            .filter(|s| !arcs(s).is_empty())
            .collect();
        path_pts(w[i])[0]
    };
    let (a0, a1) = (apex(&exploded, 0), apex(&exploded, 1));
    let d = ((a0.0 - a1.0).powi(2) + (a0.1 - a1.1).powi(2)).sqrt();
    assert!(approx(d, 0.3 * r_of(&exploded)), "{d}");
    let (p0, p1) = (apex(&plain, 0), apex(&plain, 1));
    assert!(approx(p0.0, p1.0) && approx(p0.1, p1.1));
}

#[test]
fn doughnut_hole_and_rings() {
    let mut m = pie_model(&[1.0, 1.0], GroupKind::Doughnut);
    m.groups[0].hole_size = 60.0;
    let items = draw(&m);
    let rings: Vec<_> = shapes(&items)
        .into_iter()
        .filter(|s| !arcs(s).is_empty())
        .collect();
    assert_eq!(rings.len(), 2);
    let a = arcs(rings[0]);
    assert_eq!(a.len(), 2);
    assert!(approx(a[1].0 / a[0].0 * 100.0, 60.0), "{a:?}");
    // Inner arc runs back.
    assert!(a[1].3 < 0.0 && a[0].3 > 0.0);
    // Two series: two rings, the second further out.
    let mut m = pie_model(&[1.0, 1.0], GroupKind::Doughnut);
    let s2 = m.groups[0].series[0].clone();
    m.groups[0].series.push(s2);
    let items = draw(&m);
    let radii: Vec<f64> = shapes(&items)
        .into_iter()
        .filter(|s| !arcs(s).is_empty())
        .map(|s| arcs(s)[0].0)
        .collect();
    assert_eq!(radii.len(), 4);
    assert!(radii[2] > radii[0]);
}

// ---- axes, ticks, labels ----

#[test]
fn value_axis_labels_follow_the_scale() {
    let g = grp(GroupKind::Bar, vec![ser("a", &["p", "q"], &[6.0, 21.0])]);
    let items = draw(&model(vec![g]));
    let t = text_strings(&items);
    for want in ["0", "5", "10", "15", "20", "25"] {
        assert!(t.contains(&want.to_string()), "{want} in {t:?}");
    }
    assert!(!t.contains(&"30".to_string()));
    assert!(t.contains(&"p".to_string()) && t.contains(&"q".to_string()));
}

#[test]
fn tick_label_number_formats() {
    let mk = |vals: &[f64], code: &str| {
        let g = grp(GroupKind::Bar, vec![ser("a", &["p", "q"], vals)]);
        let mut m = model(vec![g]);
        m.axes[1].num_fmt = Some((code.to_string(), false));
        text_strings(&draw(&m))
    };
    let t = mk(&[0.2, 0.9], "0%");
    assert!(
        t.contains(&"50%".to_string()) || t.contains(&"40%".to_string()),
        "{t:?}"
    );
    assert!(t.iter().all(|s| !s.contains('.')));
    let t = mk(&[1000.0, 5000.0], "#,##0");
    assert!(
        t.contains(&"3,000".to_string()) || t.contains(&"2,000".to_string()),
        "{t:?}"
    );
    let t = mk(&[0.5, 1.5], "0.0");
    assert!(
        t.contains(&"0.5".to_string()) && t.contains(&"1.0".to_string()),
        "{t:?}"
    );
    let t = mk(&[1.0, 2.0], "0.00\"x\"");
    assert!(t.iter().any(|s| s.ends_with('x')), "{t:?}");
    // A linked format takes the series' own.
    let mut s = ser("a", &["p", "q"], &[0.25, 0.75]);
    s.format_code = Some("0%".into());
    let mut m = model(vec![grp(GroupKind::Bar, vec![s])]);
    m.axes[1].num_fmt = Some(("General".into(), true));
    assert!(text_strings(&draw(&m)).iter().any(|s| s.ends_with('%')));
}

#[test]
fn date_categories_use_the_axis_format() {
    let mut s = ser("a", &[], &[1.0, 2.0]);
    s.cats = vec!["44927".into(), "44958".into()];
    s.cat_nums = vec![Some(44927.0), Some(44958.0)];
    let mut m = model(vec![grp(GroupKind::Bar, vec![s])]);
    m.axes[0].kind = AxisKind::Date;
    m.axes[0].num_fmt = Some(("yyyy-mm-dd".into(), false));
    let t = text_strings(&draw(&m));
    assert!(t.contains(&"2023-01-01".to_string()), "{t:?}");
    assert!(t.contains(&"2023-02-01".to_string()), "{t:?}");
}

#[test]
fn long_category_labels_wrap_rotate_or_thin() {
    let cats: Vec<String> = (0..12).map(|i| format!("Category number {i}")).collect();
    let refs: Vec<&str> = cats.iter().map(String::as_str).collect();
    let g = grp(GroupKind::Bar, vec![ser("a", &refs, &[1.0; 12])]);
    let items = draw(&model(vec![g]));
    let tx = texts(&items);
    // Either rotated, or wrapped onto lines, or skipped -- but never overlapping at full size.
    let cat_labels: Vec<_> = tx.iter().filter(|t| t.0.contains("Category")).collect();
    assert!(!cat_labels.is_empty());
    assert!(cat_labels
        .iter()
        .all(|t| t.2 != 0.0 || t.0.contains('\n') || cat_labels.len() < 12));
    // Short labels stay horizontal and complete.
    let g = grp(
        GroupKind::Bar,
        vec![ser("a", &["a", "b", "c"], &[1.0, 2.0, 3.0])],
    );
    let items = draw(&model(vec![g]));
    let tx = texts(&items);
    assert!(tx
        .iter()
        .filter(|t| ["a", "b", "c"].contains(&t.0.as_str()))
        .all(|t| t.2 == 0.0));
}

#[test]
fn deleted_axes_draw_nothing() {
    let g = grp(GroupKind::Bar, vec![ser("a", &["p", "q"], &[1.0, 2.0])]);
    let mut m = model(vec![g]);
    m.axes[0].deleted = true;
    m.axes[1].deleted = true;
    m.axes[1].major_grid = None;
    let items = draw(&m);
    assert!(text_strings(&items).is_empty());
    assert!(segs(&items).is_empty());
    // The bars are still there.
    assert!(!shapes(&items).is_empty());
}

#[test]
fn tick_label_position_none_and_gridlines() {
    let g = grp(GroupKind::Bar, vec![ser("a", &["p", "q"], &[1.0, 2.0])]);
    let mut m = model(vec![g]);
    m.axes[1].tick_label_pos = TickLabelPos::None;
    let t = text_strings(&draw(&m));
    assert!(!t.contains(&"1".to_string()));
    // Major gridlines: one per tick; minor gridlines add lines in between.
    let m0 = fixed(
        vec![grp(GroupKind::Bar, vec![ser("a", &["p"], &[1.0])])],
        10.0,
        5.0,
    );
    let n_major = segs(&draw(&m0))
        .iter()
        .filter(|s| (s.1 - s.3).abs() < 0.01 && s.2 - s.0 > 50.0)
        .count();
    let mut m1 = m0.clone();
    m1.axes[1].minor_grid = Some(Stroke::default());
    let n_minor = segs(&draw(&m1))
        .iter()
        .filter(|s| (s.1 - s.3).abs() < 0.01 && s.2 - s.0 > 50.0)
        .count();
    // 3 majors + the axis line of the category axis; minors add 4 per interval of 2 = 8.
    assert!(n_minor > n_major + 4, "{n_major} {n_minor}");
}

#[test]
fn axis_crossing_at_a_value_moves_the_category_axis() {
    let g = grp(GroupKind::Bar, vec![ser("a", &["p", "q"], &[5.0, -5.0])]);
    let mut m = model(vec![g]);
    m.axes[1].min = Some(-10.0);
    m.axes[1].max = Some(10.0);
    m.axes[1].major_unit = Some(10.0);
    let items = draw(&m);
    let (_, py0, _, py1) = plot_rect(&items);
    // The category axis line sits at zero: the middle of the plot.
    let axis_line = segs(&items)
        .into_iter()
        .filter(|s| (s.1 - s.3).abs() < 0.01 && s.2 - s.0 > 50.0 && approx(s.1, (py0 + py1) / 2.0))
        .count();
    assert!(axis_line >= 1);
    // With tickLblPos=low the category labels are below the plot, not at the axis.
    let mut low = m.clone();
    low.axes[0].tick_label_pos = TickLabelPos::Low;
    let drawn = draw(&low);
    let (_, _, _, py1) = plot_rect(&drawn);
    let t = texts(&drawn);
    let p = t.iter().find(|t| t.0 == "p").unwrap();
    assert!(p.1 .1 > py1, "label y {} plot bottom {}", p.1 .1, py1);
}

#[test]
fn log_axis_places_decades_evenly() {
    let g = grp(
        GroupKind::Bar,
        vec![ser("a", &["p", "q", "r"], &[20.0, 200.0, 2000.0])],
    );
    let mut m = model(vec![g]);
    m.axes[1].log_base = Some(10.0);
    let items = draw(&m);
    let t = text_strings(&items);
    for want in ["10", "100", "1000", "10000"] {
        assert!(t.contains(&want.to_string()), "{want} in {t:?}");
    }
    let r = rects_of(&items, layout::series_color(&m, 0));
    assert_eq!(r.len(), 3);
    // Heights are 1, 2, 3 decades: equal steps.
    assert!(approx(r[1].3 - r[0].3, r[2].3 - r[1].3), "{r:?}");
}

#[test]
fn log_axis_with_non_positive_values_does_not_break() {
    let g = grp(
        GroupKind::Bar,
        vec![ser("a", &["p", "q", "r"], &[0.0, -5.0, 100.0])],
    );
    let mut m = model(vec![g]);
    m.axes[1].log_base = Some(10.0);
    let items = draw(&m);
    assert!(!items.is_empty());
    assert_all_finite(&items);
}

#[test]
fn titles_axis_titles_and_rotation() {
    let g = grp(GroupKind::Bar, vec![ser("a", &["p", "q"], &[1.0, 2.0])]);
    let mut m = model(vec![g]);
    m.title = Some(ChartText {
        text: "My chart".into(),
        ..ChartText::default()
    });
    m.axes[0].title = Some(ChartText {
        text: "Cats".into(),
        ..ChartText::default()
    });
    m.axes[1].title = Some(ChartText {
        text: "Vals".into(),
        ..ChartText::default()
    });
    let items = draw(&m);
    let tx = texts(&items);
    let title = tx.iter().find(|t| t.0 == "My chart").unwrap();
    // Centred, at the top.
    assert!(approx(title.1 .0 + title.1 .2 / 2.0, W / 2.0));
    assert!(title.1 .1 < 20.0);
    let cats = tx.iter().find(|t| t.0 == "Cats").unwrap();
    assert!(cats.1 .1 > H * 0.8 && cats.2 == 0.0);
    let vals = tx.iter().find(|t| t.0 == "Vals").unwrap();
    assert!(vals.2 == 270.0, "rot {}", vals.2);
    assert!(vals.1 .0 < 30.0);
}

#[test]
fn title_makes_room_unless_it_overlays() {
    let g = grp(GroupKind::Bar, vec![colored(ser("a", &["p"], &[1.0]), A)]);
    let mut m = fixed(vec![g], 2.0, 1.0);
    let bare = plot_rect(&draw(&m));
    m.title = Some(ChartText {
        text: "T".into(),
        ..ChartText::default()
    });
    let titled = plot_rect(&draw(&m));
    assert!(titled.1 > bare.1 + 10.0);
    m.title.as_mut().unwrap().overlay = true;
    let over = plot_rect(&draw(&m));
    assert!(approx(over.1, bare.1));
}

#[test]
fn manual_plot_layout_places_the_plot() {
    let g = grp(GroupKind::Bar, vec![colored(ser("a", &["p"], &[1.0]), A)]);
    let mut m = fixed(vec![g], 2.0, 1.0);
    m.plot_layout = Some(ManualLayout {
        x: Some(0.2),
        y: Some(0.1),
        w: Some(0.5),
        h: Some(0.6),
        x_edge: true,
        y_edge: true,
        w_edge: false,
        h_edge: false,
        inner: true,
    });
    let (x0, y0, x1, y1) = plot_rect(&draw(&m));
    assert!(approx(x0, 0.2 * W) && approx(x1, 0.7 * W), "{x0} {x1}");
    assert!(approx(y0, 0.1 * H) && approx(y1, 0.7 * H), "{y0} {y1}");
}

// ---- legend ----

fn legend_model(pos: LegendPos) -> ChartModel {
    let cats = ["p", "q"];
    let g = grp(
        GroupKind::Bar,
        vec![
            colored(ser("First", &cats, &[1.0, 2.0]), A),
            colored(ser("Second", &cats, &[2.0, 1.0]), B),
        ],
    );
    let mut m = model(vec![g]);
    m.legend = Some(Legend {
        pos,
        ..Legend::default()
    });
    m
}

#[test]
fn legend_is_placed_by_its_position() {
    for (pos, check) in [
        (LegendPos::Right, 0),
        (LegendPos::Left, 1),
        (LegendPos::Top, 2),
        (LegendPos::Bottom, 3),
        (LegendPos::TopRight, 4),
    ] {
        let items = draw(&legend_model(pos));
        let t = texts(&items);
        let f = t.iter().find(|t| t.0 == "First").expect("legend entry");
        let s = t.iter().find(|t| t.0 == "Second").expect("legend entry");
        let (x, y) = (f.1 .0, f.1 .1);
        match check {
            0 | 4 => assert!(x > 0.7 * W, "{pos:?} x {x}"),
            1 => assert!(x < 0.2 * W, "{pos:?} x {x}"),
            2 => assert!(y < 0.2 * H, "{pos:?} y {y}"),
            _ => assert!(y > 0.8 * H, "{pos:?} y {y}"),
        }
        if check == 4 {
            assert!(y < 0.2 * H);
        }
        // Right and left: entries stacked; top and bottom: side by side.
        if matches!(
            pos,
            LegendPos::Right | LegendPos::Left | LegendPos::TopRight
        ) {
            assert!(s.1 .1 != f.1 .1);
        } else {
            assert!(approx(s.1 .1, f.1 .1) && s.1 .0 > f.1 .0);
        }
    }
}

#[test]
fn legend_makes_room_and_each_series_has_a_key() {
    let with = plot_rect(&draw(&{
        let mut m = legend_model(LegendPos::Right);
        m.axes[1].min = Some(0.0);
        m.axes[1].max = Some(2.0);
        m.axes[1].major_unit = Some(1.0);
        m
    }));
    let mut m0 = legend_model(LegendPos::Right);
    m0.legend = None;
    m0.axes[1].min = Some(0.0);
    m0.axes[1].max = Some(2.0);
    m0.axes[1].major_unit = Some(1.0);
    let without = plot_rect(&draw(&m0));
    assert!(with.2 < without.2 - 30.0);
    // The keys are small squares of the series colours (besides the bars).
    let items = draw(&legend_model(LegendPos::Right));
    let keys = rects_of(&items, A)
        .into_iter()
        .filter(|r| r.2 < 15.0 && r.3 < 15.0 && r.2 > 4.0)
        .count();
    assert!(keys >= 1);
}

#[test]
fn legend_stacked_columns_list_the_last_series_first() {
    let mut m = legend_model(LegendPos::Right);
    m.groups[0].grouping = Grouping::Stacked;
    let t = texts(&draw(&m));
    let f = t.iter().find(|t| t.0 == "First").unwrap();
    let s = t.iter().find(|t| t.0 == "Second").unwrap();
    assert!(s.1 .1 < f.1 .1, "stacked columns: the top series first");
    let mut m = legend_model(LegendPos::Right);
    let t = texts(&draw(&m));
    let f = t.iter().find(|t| t.0 == "First").unwrap();
    let s = t.iter().find(|t| t.0 == "Second").unwrap();
    assert!(f.1 .1 < s.1 .1, "clustered: in order");
    m.legend.as_mut().unwrap().deleted = vec![0];
    let t = text_strings(&draw(&m));
    assert!(!t.contains(&"First".to_string()) && t.contains(&"Second".to_string()));
}

#[test]
fn pie_legend_lists_the_points() {
    let mut m = pie_model(&[1.0, 2.0, 3.0], GroupKind::Pie);
    m.legend = Some(Legend::default());
    let t = text_strings(&draw(&m));
    for c in ["c0", "c1", "c2"] {
        assert!(t.contains(&c.to_string()), "{c}");
    }
    // Series without a name get Office's placeholder.
    let mut m = legend_model(LegendPos::Right);
    m.groups[0].series[0].name = None;
    assert!(text_strings(&draw(&m)).contains(&"Series1".to_string()));
}

#[test]
fn legend_with_many_entries_stays_in_the_chart() {
    let cats = ["p"];
    let series: Vec<Series> = (0..60)
        .map(|i| ser(&format!("series {i}"), &cats, &[1.0]))
        .collect();
    let mut m = model(vec![grp(GroupKind::Bar, series)]);
    m.legend = Some(Legend::default());
    let items = draw(&m);
    for t in texts(&items) {
        assert!(t.1 .1 >= 0.0 && t.1 .1 + t.1 .3 <= H + 1.0, "{t:?}");
    }
    m.legend.as_mut().unwrap().pos = LegendPos::Bottom;
    let items = draw(&m);
    for t in texts(&items) {
        assert!(t.1 .1 >= 0.0 && t.1 .1 + t.1 .3 <= H + 1.0, "{t:?}");
    }
}

// ---- data labels ----

fn with_labels(mut m: ChartModel, dl: DataLabels) -> ChartModel {
    m.groups[0].labels = Some(dl);
    m
}

fn show_val() -> DataLabels {
    DataLabels {
        show_value: true,
        ..DataLabels::default()
    }
}

#[test]
fn data_labels_show_values_above_the_bars() {
    let g = grp(
        GroupKind::Bar,
        vec![colored(ser("a", &["p", "q"], &[5.0, 10.0]), A)],
    );
    let m = with_labels(fixed(vec![g], 10.0, 5.0), show_val());
    let items = draw(&m);
    let tx = texts(&items);
    let l5 = tx
        .iter()
        .find(|t| t.0 == "5" && t.1 .0 > 40.0 && t.2 == 0.0 && t.1 .1 < 200.0);
    assert!(l5.is_some());
    let bars = rects_of(&items, A);
    // The label of the first bar is above it (outside end).
    let lab = tx
        .iter()
        .filter(|t| t.0 == "5")
        .find(|t| t.1 .0 + t.1 .2 / 2.0 > bars[0].0 && t.1 .0 < bars[0].0 + bars[0].2)
        .unwrap();
    assert!(
        lab.1 .1 + lab.1 .3 <= bars[0].1 + 3.0,
        "label {:?} bar {:?}",
        lab.1,
        bars[0]
    );
}

#[test]
fn data_label_positions() {
    let mk = |pos: LabelPos| {
        let g = grp(GroupKind::Bar, vec![colored(ser("a", &["p"], &[10.0]), A)]);
        let mut dl = show_val();
        dl.num_fmt = Some("\"LBL\"".into());
        dl.pos = Some(pos);
        let items = draw(&with_labels(fixed(vec![g], 10.0, 5.0), dl));
        let bar = rects_of(&items, A)[0];
        let lab = texts(&items).into_iter().find(|t| t.0 == "LBL").unwrap();
        (bar, lab.1)
    };
    let (bar, lab) = mk(LabelPos::OutsideEnd);
    assert!(lab.1 + lab.3 <= bar.1 + 3.0);
    let (bar, lab) = mk(LabelPos::InsideEnd);
    assert!(lab.1 >= bar.1 - 1.0 && lab.1 + lab.3 <= bar.1 + bar.3);
    let (bar, lab) = mk(LabelPos::Center);
    assert!(approx(lab.1 + lab.3 / 2.0, bar.1 + bar.3 / 2.0));
    let (bar, lab) = mk(LabelPos::InsideBase);
    assert!(lab.1 + lab.3 <= bar.1 + bar.3 + 1.0 && lab.1 > bar.1 + bar.3 / 2.0);
}

#[test]
fn data_labels_name_category_series_and_percent() {
    let mut m = pie_model(&[1.0, 3.0], GroupKind::Pie);
    m.groups[0].labels = Some(DataLabels {
        show_percent: true,
        show_category: true,
        separator: Some("; ".into()),
        ..DataLabels::default()
    });
    let t = text_strings(&draw(&m));
    assert!(t.contains(&"c0; 25%".to_string()), "{t:?}");
    assert!(t.contains(&"c1; 75%".to_string()), "{t:?}");
    // Series name and value.
    let g = grp(GroupKind::Bar, vec![ser("Sales", &["p"], &[7.0])]);
    let m = with_labels(
        fixed(vec![g], 10.0, 5.0),
        DataLabels {
            show_series: true,
            show_value: true,
            ..DataLabels::default()
        },
    );
    assert!(text_strings(&draw(&m)).contains(&"Sales, 7".to_string()));
}

#[test]
fn per_point_label_override_and_delete() {
    let g = grp(
        GroupKind::Bar,
        vec![ser("a", &["p", "q", "r"], &[1.0, 2.0, 3.0])],
    );
    let mut dl = show_val();
    let gone = DataLabels {
        delete: true,
        ..DataLabels::default()
    };
    let custom = DataLabels {
        text: Some("custom".into()),
        ..DataLabels::default()
    };
    dl.points = vec![(1, gone), (2, custom)];
    let t = text_strings(&draw(&with_labels(fixed(vec![g], 4.0, 1.0), dl)));
    assert!(t.contains(&"custom".to_string()));
    assert_eq!(
        t.iter().filter(|s| *s == "2").count(),
        1,
        "only the axis label: {t:?}"
    );
}

#[test]
fn line_data_labels_sit_beside_the_points() {
    let g = grp(GroupKind::Line, vec![ser("a", &["p", "q"], &[2.0, 4.0])]);
    let mut dl = show_val();
    dl.num_fmt = Some("\"V\"0.0".into());
    let items = draw(&with_labels(fixed(vec![g], 5.0, 1.0), dl));
    let tx = texts(&items);
    assert_eq!(tx.iter().filter(|t| t.0.starts_with('V')).count(), 2);
}

// ---- budgets and hostile input ----

pub(super) fn assert_all_finite(items: &[Item]) {
    for s in shapes(items) {
        let x = &s.xfrm;
        for v in [x.x, x.y, x.w, x.h, x.rot_deg] {
            assert!(v.is_finite(), "non-finite xfrm in {s:?}");
        }
        if let Geometry::Paths(ps) = &s.geom {
            for p in ps {
                for c in &p.cmds {
                    let ok = match c {
                        PathCmd::MoveTo(a) | PathCmd::LineTo(a) => {
                            a.x.is_finite() && a.y.is_finite()
                        }
                        PathCmd::QuadTo(a, b) => {
                            [a, b].iter().all(|p| p.x.is_finite() && p.y.is_finite())
                        }
                        PathCmd::CubicTo(a, b, c) => {
                            [a, b, c].iter().all(|p| p.x.is_finite() && p.y.is_finite())
                        }
                        PathCmd::ArcTo {
                            wr,
                            hr,
                            st_deg,
                            sw_deg,
                        } => [wr, hr, st_deg, sw_deg].iter().all(|v| v.is_finite()),
                        PathCmd::Close => true,
                    };
                    assert!(ok, "non-finite path command {c:?}");
                }
            }
        }
    }
}

#[test]
fn hostile_numbers_never_break_the_drawing() {
    for bad in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        1e308,
        -1e308,
        1e-308,
        0.0,
    ] {
        for kind in [
            GroupKind::Bar,
            GroupKind::Line,
            GroupKind::Area,
            GroupKind::Scatter,
            GroupKind::Bubble,
            GroupKind::Radar,
            GroupKind::Pie,
            GroupKind::Doughnut,
        ] {
            let mut s = ser("a", &["p", "q", "r", "s", "t"], &[1.0, bad, 3.0, bad, -bad]);
            s.x_values = vec![Some(bad), Some(1.0), Some(2.0), Some(3.0), Some(bad)];
            s.sizes = vec![Some(bad), Some(1.0), Some(2.0), Some(-1.0), Some(0.0)];
            let mut g = grp(kind, vec![s]);
            g.gap_width = bad;
            g.overlap = bad;
            g.hole_size = bad;
            g.first_slice_ang = bad;
            let mut m = model(vec![g]);
            m.axes[1].min = Some(bad);
            m.axes[1].max = Some(bad);
            m.axes[1].major_unit = Some(bad);
            m.legend = Some(Legend::default());
            m.title = Some(ChartText {
                text: "t".into(),
                ..ChartText::default()
            });
            let (items, _) = draw_chart(&m, emu(W), emu(H));
            assert_all_finite(&items);
        }
    }
}

#[test]
fn huge_values_keep_bars_inside_the_chart() {
    let g = grp(
        GroupKind::Bar,
        vec![colored(ser("a", &["p", "q"], &[1e308, 1.0]), A)],
    );
    let items = draw(&model(vec![g]));
    assert_all_finite(&items);
    for r in rects_of(&items, A) {
        assert!(r.1 >= -1.0 && r.1 + r.3 <= H + 1.0, "{r:?}");
    }
}

#[test]
fn degenerate_data() {
    // All equal.
    let g = grp(
        GroupKind::Bar,
        vec![colored(ser("a", &["p", "q"], &[5.0, 5.0]), A)],
    );
    let items = draw(&model(vec![g]));
    let r = rects_of(&items, A);
    assert_eq!(r.len(), 2);
    assert!(approx(r[0].3, r[1].3) && r[0].3 > 10.0);
    // A single point.
    let g = grp(GroupKind::Line, vec![ser("a", &["p"], &[3.0])]);
    assert_all_finite(&draw(&model(vec![g])));
    // Empty series, no series, mismatched lengths.
    let g = grp(GroupKind::Bar, vec![Series::default()]);
    assert_all_finite(&draw(&model(vec![g])));
    assert_all_finite(&draw(&model(vec![grp(GroupKind::Bar, vec![])])));
    let mut s = ser("a", &["p", "q", "r"], &[1.0]);
    s.values.push(Some(2.0));
    s.cats.truncate(1);
    let items = draw(&fixed(
        vec![grp(GroupKind::Bar, vec![colored(s, A)])],
        3.0,
        1.0,
    ));
    assert_eq!(rects_of(&items, A).len(), 2);
    // Axes that are not in the model.
    let mut m = model(vec![grp(GroupKind::Bar, vec![ser("a", &["p"], &[1.0])])]);
    m.axes.clear();
    m.groups[0].axis_ids = vec![9, 8];
    assert_all_finite(&draw(&m));
    m.groups[0].axis_ids.clear();
    assert_all_finite(&draw(&m));
}

#[test]
fn a_hundred_thousand_points_are_cut_quickly() {
    let n = 100_000;
    let vals: Vec<f64> = (0..n).map(|i| (i % 97) as f64).collect();
    let cats: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    let mut s = Series {
        name: Some("big".into()),
        cats,
        values: vals.iter().map(|v| Some(*v)).collect(),
        ..Series::default()
    };
    s.fill = Some(Fill::Solid(A));
    for kind in [
        GroupKind::Bar,
        GroupKind::Line,
        GroupKind::Area,
        GroupKind::Scatter,
    ] {
        let m = model(vec![grp(kind, vec![s.clone()])]);
        let t = std::time::Instant::now();
        let (items, trunc) = draw_chart(&m, emu(W), emu(H));
        assert!(
            t.elapsed().as_secs() < 20,
            "{kind:?} took {:?}",
            t.elapsed()
        );
        assert!(trunc || kind != GroupKind::Bar, "{kind:?}");
        assert!(shapes(&items).len() <= MAX_DRAWN + 100, "{kind:?}");
    }
}

#[test]
fn many_series_are_capped() {
    let series: Vec<Series> = (0..1000)
        .map(|i| ser(&format!("s{i}"), &["p"], &[1.0]))
        .collect();
    let mut m = model(vec![grp(GroupKind::Bar, series)]);
    m.legend = Some(Legend::default());
    let (items, _) = draw_chart(&m, emu(W), emu(H));
    assert_all_finite(&items);
    let sanitized = sanitize(&m);
    assert_eq!(sanitized.groups[0].series.len(), MAX_SERIES);
}

#[test]
fn long_texts_are_cut() {
    let long = "x".repeat(10_000);
    let mut s = ser("a", &["p"], &[1.0]);
    s.name = Some(long.clone());
    s.cats = vec![long.clone()];
    let mut m = model(vec![grp(GroupKind::Bar, vec![s])]);
    m.title = Some(ChartText {
        text: long,
        ..ChartText::default()
    });
    m.legend = Some(Legend::default());
    for t in text_strings(&draw(&m)) {
        // (A title is also broken into lines: the breaks are not characters of the text.)
        assert!(
            t.chars().filter(|c| *c != '\n').count() <= MAX_LABEL_CHARS + 1,
            "{}",
            t.len()
        );
    }
}

#[test]
fn chart_and_plot_area_fills() {
    let g = grp(GroupKind::Bar, vec![ser("a", &["p"], &[1.0])]);
    let mut m = model(vec![g]);
    m.chart_fill = Some(Fill::Solid(A));
    m.plot_fill = Some(Fill::Solid(B));
    let items = draw(&m);
    let a = rects_of(&items, A);
    assert_eq!(a.len(), 1);
    assert!(approx(a[0].2, W) && approx(a[0].3, H));
    let b = rects_of(&items, B);
    assert_eq!(b.len(), 1);
    assert!(b[0].2 < W && b[0].0 > 0.0);
}

#[test]
fn series_palette_follows_the_theme_then_varies() {
    let m = ChartModel {
        palette: (1..=6).map(|i| Rgba::rgb(i * 10, 0, 0)).collect(),
        ..ChartModel::default()
    };
    for i in 0..6 {
        assert_eq!(
            layout::series_color(&m, i),
            Rgba::rgb((i as u8 + 1) * 10, 0, 0)
        );
    }
    // The second cycle is darker, the third lighter than the base.
    let base = layout::series_color(&m, 0);
    let darker = layout::series_color(&m, 6);
    assert!(darker.r < base.r);
    let m2 = ChartModel::default();
    assert_eq!(layout::series_color(&m2, 0), Rgba::rgb(0x44, 0x72, 0xC4));
    assert!(layout::series_color(&m2, 6).r < 0x44);
    let lighter = layout::series_color(&m2, 12);
    assert!(lighter.r >= 0x44 || lighter.g > 0x72);
    // Far past the end it still gives colours.
    let _ = layout::series_color(&m2, 10_000);
}

#[test]
fn text_style_inheritance() {
    let mut m = model(vec![grp(GroupKind::Bar, vec![ser("a", &["p"], &[1.0])])]);
    m.text.size_pt = Some(20.0);
    m.text.color = Some(A);
    m.axes[0].text.size_pt = Some(8.0);
    let items = draw(&m);
    let sizes: Vec<(String, f64, Rgba)> = shapes(&items)
        .into_iter()
        .filter_map(|s| {
            let r = &s.text.as_ref()?.paragraphs[0].runs[0];
            let c = match &r.fill {
                Fill::Solid(c) => *c,
                _ => return None,
            };
            Some((r.text.clone(), r.size_pt, c))
        })
        .collect();
    let p = sizes.iter().find(|s| s.0 == "p").unwrap();
    assert_eq!(p.1, 8.0);
    assert_eq!(p.2, A);
    let one = sizes.iter().find(|s| s.0 == "1").unwrap();
    assert_eq!(one.1, 20.0);
}

#[test]
fn rendering_the_items_makes_a_valid_svg() {
    let g = grp(GroupKind::Bar, vec![ser("a", &["p", "q"], &[1.0, 2.0])]);
    let mut m = model(vec![g]);
    m.legend = Some(Legend::default());
    m.title = Some(ChartText {
        text: "T < & >".into(),
        ..ChartText::default()
    });
    let (items, trunc) = draw_chart(&m, emu(W), emu(H));
    let scene = SlideScene {
        width: emu(W),
        height: emu(H),
        background: Fill::Solid(Rgba::WHITE),
        items,
        truncated: trunc,
    };
    let r = super::super::render_svg(&scene, &|_| None);
    assert!(!r.truncated);
    let img = crate::preview::svg::rasterize_trusted(
        r.svg.as_bytes(),
        std::path::Path::new("/c.svg"),
        480,
    )
    .expect("the SVG rasterizes");
    assert_eq!(img.width(), 480);
    // Something is drawn that is not white.
    let rgba = img.to_rgba8();
    assert!(rgba.pixels().any(|p| p.0[0] < 200));
}

use super::super::SlideScene;

#[test]
fn pie_counter_clockwise_slices_run_the_other_way_round() {
    let mut m = pie_model(&[1.0, 2.0, 3.0, 4.0], GroupKind::Pie);
    m.groups[0].counter_clockwise = true;
    let items = draw(&m);
    let wedges: Vec<_> = shapes(&items)
        .into_iter()
        .filter(|s| !arcs(s).is_empty())
        .collect();
    assert_eq!(wedges.len(), 4);
    let sweeps: Vec<f64> = wedges.iter().map(|s| arcs(s)[0].3).collect();
    assert!(approx(sweeps.iter().sum::<f64>(), 360.0));
    assert!(approx(sweeps[0], 36.0) && approx(sweeps[3], 144.0));
    // The first slice ends at 12 o'clock (its 36 degrees lie before it) and the next one ends
    // where the first one began.
    assert!(approx(arcs(wedges[0])[0].2, -90.0 - 36.0));
    assert!(approx(arcs(wedges[1])[0].2, -90.0 - 36.0 - 72.0));
    // The default is clockwise.
    assert!(!ChartGroup::default().counter_clockwise);
}

#[test]
fn pie_counter_clockwise_from_a_start_angle_and_with_explosion() {
    let mut m = pie_model(&[1.0, 1.0], GroupKind::Pie);
    m.groups[0].counter_clockwise = true;
    m.groups[0].first_slice_ang = 90.0;
    m.groups[0].series[0].explosion = Some(20.0);
    let items = draw(&m);
    let w: Vec<_> = shapes(&items)
        .into_iter()
        .filter(|s| !arcs(s).is_empty())
        .collect();
    // 3 o'clock, going back: the first slice covers 90 -> -90 degrees
    assert!(approx(arcs(w[0])[0].2, 90.0 - 180.0 - 90.0));
    assert!(approx(arcs(w[1])[0].2, 90.0 - 360.0 - 90.0));
}

#[test]
fn doughnut_counter_clockwise_slices_run_the_other_way_round() {
    let mut m = pie_model(&[1.0, 3.0], GroupKind::Doughnut);
    m.groups[0].counter_clockwise = true;
    let items = draw(&m);
    let w: Vec<_> = shapes(&items)
        .into_iter()
        .filter(|s| !arcs(s).is_empty())
        .collect();
    assert_eq!(w.len(), 2);
    // the first slice (90 degrees) ends at 12 o'clock
    assert!(approx(arcs(w[0])[0].2, -90.0 - 90.0));
    assert!(approx(arcs(w[0])[0].3, 90.0));
    assert!(approx(arcs(w[1])[0].2, -90.0 - 90.0 - 270.0));
    // a single slice is the full ring, as before
    let mut one = pie_model(&[5.0], GroupKind::Doughnut);
    one.groups[0].counter_clockwise = true;
    assert!(!draw(&one).is_empty());
}

#[test]
fn a_long_chart_title_wraps_at_four_fifths_of_the_chart_width() {
    // (Slide 8 of the deck aascu 5864: a 20 pt title of 120 characters ran off both sides.)
    let g = grp(GroupKind::Bar, vec![ser("a", &["p", "q"], &[1.0, 2.0])]);
    let mut m = model(vec![g]);
    let long = "Employment of workers with a Bachelor degree or better grew at a 2 percent to 3 percent rate over the past two decades";
    m.title = Some(ChartText {
        text: long.into(),
        style: TextStyle {
            size_pt: Some(20.0),
            bold: Some(true),
            ..TextStyle::default()
        },
        ..ChartText::default()
    });
    let items = draw(&m);
    let tx = texts(&items);
    let title = tx.iter().find(|t| t.0.contains("Employment")).unwrap();
    assert!(title.0.contains('\n'), "not wrapped: {}", title.0);
    // The same words in the same order, only the breaks differ.
    assert_eq!(title.0.replace('\n', " "), long);
    assert!(title.1 .2 <= W * 0.8 + 1.0, "box {} wide", title.1 .2);
    assert!(title.1 .0 >= 0.0 && title.1 .0 + title.1 .2 <= W);
    // A short one stays on one line; an unspaced (CJK) one breaks between characters.
    m.title.as_mut().unwrap().text = "Short".into();
    let items = draw(&m);
    assert!(texts(&items).iter().any(|t| t.0 == "Short"));
    m.title.as_mut().unwrap().text = "あ".repeat(60);
    let items = draw(&m);
    let t = texts(&items)
        .into_iter()
        .find(|t| t.0.contains('あ'))
        .unwrap();
    assert!(t.0.contains('\n'));
    assert_eq!(t.0.replace('\n', ""), "あ".repeat(60));
    assert!(t.1 .2 <= W * 0.8 + 1.0);
}
