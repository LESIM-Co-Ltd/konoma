//! Tests from mutation testing of the pie and doughnut drawing (`pie.rs`): the geometry of the
//! slices (radius, centre, angles, explosion) and the colours, from the rules in the module
//! documentation.

use super::super::pie::draw as draw_pie;
use super::super::shapes::Out;
use super::*;
use crate::preview::office::slide_draw::chart::layout::Rect;

fn pie_items(m: &ChartModel, rect: Rect) -> Vec<Item> {
    let mut o = Out::new(rect.x + rect.w, rect.y + rect.h);
    draw_pie(&mut o, m, rect);
    o.finish().0
}

fn rect_400x300() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 300.0,
    }
}

/// The arcs `(wr, hr, start, sweep)` of a shape's paths, radii in px.
fn arcs(s: &ShapeItem) -> Vec<(f64, f64, f64, f64)> {
    let Geometry::Paths(paths) = &s.geom else {
        return Vec::new();
    };
    paths
        .iter()
        .flat_map(|p| p.cmds.iter())
        .filter_map(|c| match c {
            PathCmd::ArcTo {
                wr,
                hr,
                st_deg,
                sw_deg,
            } => Some((wr / EMU_PER_PX, hr / EMU_PER_PX, *st_deg, *sw_deg)),
            _ => None,
        })
        .collect()
}

/// The wedges (shapes with arcs) in drawing order.
fn wedges(items: &[Item]) -> Vec<&ShapeItem> {
    shapes(items)
        .into_iter()
        .filter(|s| !arcs(s).is_empty())
        .collect()
}

fn wedge_sweeps(items: &[Item]) -> Vec<(f64, f64)> {
    wedges(items)
        .iter()
        .map(|s| {
            let a = arcs(s)[0];
            (a.2, a.3)
        })
        .collect()
}

#[test]
fn a_pie_fills_the_plot_less_four_pixels_and_sits_in_its_middle() {
    let m = pie_model(&[1.0, 1.0, 2.0], GroupKind::Pie);
    let items = pie_items(&m, rect_400x300());
    let (cx, cy, r) = circle(&items);
    assert!(
        (cx - 200.0).abs() < 1e-6 && (cy - 150.0).abs() < 1e-6,
        "{cx},{cy}"
    );
    assert!((r - 146.0).abs() < 1e-6, "{r}");
    // a plot that is not at the origin
    let items = pie_items(
        &m,
        Rect {
            x: 50.0,
            y: 20.0,
            w: 200.0,
            h: 300.0,
        },
    );
    let (cx, cy, r) = circle(&items);
    assert!(
        (cx - 150.0).abs() < 1e-6 && (cy - 170.0).abs() < 1e-6,
        "{cx},{cy}"
    );
    assert!((r - 96.0).abs() < 1e-6, "{r}");
    // never smaller than 4 px
    let items = pie_items(
        &m,
        Rect {
            x: 0.0,
            y: 0.0,
            w: 3.0,
            h: 3.0,
        },
    );
    assert!((circle(&items).2 - 4.0).abs() < 1e-6);
}

#[test]
fn slices_run_clockwise_from_the_first_slice_angle_in_proportion_to_their_values() {
    let m = pie_model(&[1.0, 3.0], GroupKind::Pie);
    let sweeps = wedge_sweeps(&pie_items(&m, rect_400x300()));
    // arcs start at (angle - 90): 12 o'clock is -90
    assert_eq!(sweeps.len(), 2);
    assert!(
        (sweeps[0].0 - -90.0).abs() < 1e-9 && (sweeps[0].1 - 90.0).abs() < 1e-9,
        "{sweeps:?}"
    );
    assert!(
        (sweeps[1].0 - 0.0).abs() < 1e-9 && (sweeps[1].1 - 270.0).abs() < 1e-9,
        "{sweeps:?}"
    );
    // first slice at 30 degrees, and 390 (the same)
    for first in [30.0, 390.0, -330.0] {
        let mut m = pie_model(&[1.0, 3.0], GroupKind::Pie);
        m.groups[0].first_slice_ang = first;
        let sweeps = wedge_sweeps(&pie_items(&m, rect_400x300()));
        assert!((sweeps[0].0 - -60.0).abs() < 1e-9, "{first}: {sweeps:?}");
        assert!((sweeps[1].0 - 30.0).abs() < 1e-9, "{first}: {sweeps:?}");
    }
    // counter-clockwise: the first slice ends where the running angle starts
    let mut m = pie_model(&[1.0, 3.0], GroupKind::Pie);
    m.groups[0].counter_clockwise = true;
    let sweeps = wedge_sweeps(&pie_items(&m, rect_400x300()));
    assert!(
        (sweeps[0].0 - -180.0).abs() < 1e-9 && (sweeps[0].1 - 90.0).abs() < 1e-9,
        "{sweeps:?}"
    );
    assert!(
        (sweeps[1].0 - -450.0).abs() < 1e-9 && (sweeps[1].1 - 270.0).abs() < 1e-9,
        "{sweeps:?}"
    );
    // blanks, zeros and negatives take no angle
    let m = pie_model(&[2.0, -5.0, 0.0, 2.0], GroupKind::Pie);
    let sweeps = wedge_sweeps(&pie_items(&m, rect_400x300()));
    assert_eq!(sweeps.len(), 2);
    assert!((sweeps[0].1 - 180.0).abs() < 1e-9 && (sweeps[1].1 - 180.0).abs() < 1e-9);
}

#[test]
fn a_single_slice_is_a_whole_ellipse_and_no_slices_draw_nothing() {
    let items = pie_items(&pie_model(&[5.0], GroupKind::Pie), rect_400x300());
    assert!(wedges(&items).is_empty());
    let (cx, cy, r) = circle(&items);
    assert!((cx - 200.0).abs() < 1e-6 && (cy - 150.0).abs() < 1e-6 && (r - 146.0).abs() < 1e-6);
    // a slice of 359.995 degrees is whole, one of 359.9 is not
    let items = pie_items(
        &pie_model(&[1_000_000.0, 1.0], GroupKind::Pie),
        rect_400x300(),
    );
    assert_eq!(wedges(&items).len(), 1, "the tiny one only");
    let items = pie_items(&pie_model(&[1000.0, 1.0], GroupKind::Pie), rect_400x300());
    assert_eq!(wedges(&items).len(), 2);
    assert!(pie_items(&pie_model(&[0.0, 0.0], GroupKind::Pie), rect_400x300()).is_empty());
    let mut m = pie_model(&[1.0], GroupKind::Pie);
    m.groups[0].series.clear();
    assert!(pie_items(&m, rect_400x300()).is_empty());
}

#[test]
fn an_exploded_slice_moves_out_along_its_bisector_and_the_pie_shrinks_to_make_room() {
    // series explosion of 50 %: the pie is 1 / 1.5 of the room, the slices move 0.5 radius
    let mut m = pie_model(&[1.0, 1.0], GroupKind::Pie);
    m.groups[0].series[0].explosion = Some(50.0);
    let items = pie_items(&m, rect_400x300());
    let r = (150.0 - 4.0) / 1.5;
    let ws = wedges(&items);
    assert_eq!(ws.len(), 2);
    // the first slice is the right half (12 to 6 o'clock): moved right by 0.5 r
    let first = circle(&items);
    assert!((first.2 - r).abs() < 1e-6, "{first:?}");
    assert!((first.0 - (200.0 + 0.5 * r)).abs() < 1e-6, "{first:?}");
    assert!((first.1 - 150.0).abs() < 1e-6, "{first:?}");
    // the second the other way
    let s2 = ws[1];
    let (x, w) = (s2.xfrm.x / EMU_PER_PX, s2.xfrm.w / EMU_PER_PX);
    assert!(((x + w / 2.0) - (200.0 - 0.5 * r)).abs() < 1e-6);
    // one point exploded alone
    let mut m = pie_model(&[1.0, 1.0], GroupKind::Pie);
    m.groups[0].series[0].points.push(PointFmt {
        idx: 1,
        explosion: Some(100.0),
        ..PointFmt::default()
    });
    let items = pie_items(&m, rect_400x300());
    let r = (150.0 - 4.0) / 2.0;
    let ws = wedges(&items);
    let (x, w) = (ws[0].xfrm.x / EMU_PER_PX, ws[0].xfrm.w / EMU_PER_PX);
    assert!(
        ((x + w / 2.0) - 200.0).abs() < 1e-6,
        "the other slice stays"
    );
    assert!((w / 2.0 - r).abs() < 1e-6);
    let (x, w) = (ws[1].xfrm.x / EMU_PER_PX, ws[1].xfrm.w / EMU_PER_PX);
    assert!(
        ((x + w / 2.0) - (200.0 - r)).abs() < 1e-6,
        "moved by a whole radius"
    );
    // an explosion beyond 400 % counts as 400
    let mut m = pie_model(&[1.0, 1.0], GroupKind::Pie);
    m.groups[0].series[0].explosion = Some(900.0);
    let r = circle(&pie_items(&m, rect_400x300())).2;
    assert!((r - 146.0 / 5.0).abs() < 1e-6, "{r}");
}

#[test]
fn a_doughnut_does_not_explode() {
    let mut m = pie_model(&[1.0, 1.0], GroupKind::Doughnut);
    m.groups[0].series[0].explosion = Some(50.0);
    let plain = pie_model(&[1.0, 1.0], GroupKind::Doughnut);
    let a = pie_items(&m, rect_400x300());
    let b = pie_items(&plain, rect_400x300());
    assert_eq!(a, b);
}

#[test]
fn labels_outside_a_pie_make_room_for_themselves() {
    let plain = pie_model(&[1.0, 1.0, 2.0], GroupKind::Pie);
    let r_plain = circle(&pie_items(&plain, rect_400x300())).2;
    let mut m = pie_model(&[1.0, 1.0, 2.0], GroupKind::Pie);
    m.groups[0].series[0].labels = Some(DataLabels {
        show_value: true,
        pos: Some(LabelPos::OutsideEnd),
        ..DataLabels::default()
    });
    let r_out = circle(&pie_items(&m, rect_400x300())).2;
    let line_h = super::super::text::resolve(
        &m,
        &super::super::TextStyle::default(),
        super::super::layout::TEXT_PT,
        false,
    )
    .line_h();
    assert!((r_plain - 146.0).abs() < 1e-6);
    assert!(
        (r_out - (150.0 - (line_h + 14.0))).abs() < 1e-6,
        "{r_out} for line height {line_h}"
    );
    // labels inside the slices need no room
    let mut m = pie_model(&[1.0, 1.0, 2.0], GroupKind::Pie);
    m.groups[0].series[0].labels = Some(DataLabels {
        show_value: true,
        pos: Some(LabelPos::Center),
        ..DataLabels::default()
    });
    assert!((circle(&pie_items(&m, rect_400x300())).2 - 146.0).abs() < 1e-6);
}

#[test]
fn slice_colours_follow_the_point_the_series_and_the_vary_colours_flag() {
    let fill_of = |items: &[Item], i: usize| solid(wedges(items)[i]).expect("a solid wedge");
    let red = Rgba::rgb(255, 0, 0);
    let blue = Rgba::rgb(0, 0, 255);
    // vary colours: one colour per point from the palette
    let m = pie_model(&[1.0, 1.0, 1.0], GroupKind::Pie);
    let items = pie_items(&m, rect_400x300());
    let colours: Vec<Rgba> = (0..3).map(|i| fill_of(&items, i)).collect();
    assert_eq!(colours[0], super::super::layout::point_color(&m, 0, 3));
    assert_eq!(colours[1], super::super::layout::point_color(&m, 1, 3));
    assert_eq!(colours[2], super::super::layout::point_color(&m, 2, 3));
    assert!(colours[0] != colours[1] && colours[1] != colours[2]);
    // not varying: every slice has the series colour
    let mut m = pie_model(&[1.0, 1.0, 1.0], GroupKind::Pie);
    m.groups[0].vary_colors = false;
    m.groups[0].series[0].fill = Some(Fill::Solid(red));
    let items = pie_items(&m, rect_400x300());
    assert!((0..3).all(|i| fill_of(&items, i) == red));
    // not varying and no series colour: the first palette colour
    m.groups[0].series[0].fill = None;
    let items = pie_items(&m, rect_400x300());
    let first = super::super::layout::point_color(&m, 0, 1);
    assert!((0..3).all(|i| fill_of(&items, i) == first));
    // a point's own fill wins in every case
    m.groups[0].series[0].points.push(PointFmt {
        idx: 1,
        fill: Some(Fill::Solid(blue)),
        ..PointFmt::default()
    });
    let items = pie_items(&m, rect_400x300());
    assert_eq!(fill_of(&items, 1), blue);
    assert_eq!(fill_of(&items, 0), first);
    // a doughnut with several series varies its colours although the flag is off; with one
    // series it follows the flag
    let mut one = pie_model(&[1.0, 1.0], GroupKind::Doughnut);
    one.groups[0].vary_colors = false;
    one.groups[0].series[0].fill = Some(Fill::Solid(red));
    let items = pie_items(&one, rect_400x300());
    assert_eq!(fill_of(&items, 0), red);
    assert_eq!(fill_of(&items, 1), red);
    let mut two = one.clone();
    let second = two.groups[0].series[0].clone();
    two.groups[0].series.push(second);
    let items = pie_items(&two, rect_400x300());
    assert!(fill_of(&items, 0) != fill_of(&items, 1));
    assert_eq!(
        fill_of(&items, 0),
        super::super::layout::point_color(&two, 0, 2)
    );
}
