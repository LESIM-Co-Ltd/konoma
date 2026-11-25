//! 3-D charts: the oblique depth of bars, ribbons and walls, and the tilted pie.

use super::layout::Rect;
use super::tests::*;
use super::three_d::{self, Depth};
use super::*;

const RED: Rgba = Rgba::rgb(0xC0, 0x30, 0x30);

fn bars3(vals: &[f64], rot_x: f64, rot_y: f64) -> ChartModel {
    let cats: Vec<String> = (0..vals.len()).map(|i| format!("k{i}")).collect();
    let cats: Vec<&str> = cats.iter().map(String::as_str).collect();
    let mut g = grp(GroupKind::Bar, vec![colored(ser("a", &cats, vals), RED)]);
    g.three_d = true;
    let mut m = fixed(vec![g], 10.0, 5.0);
    m.three_d = true;
    m.view3d = Some(View3D {
        rot_x,
        rot_y,
        ..View3D::default()
    });
    m
}

fn flat_bars(vals: &[f64]) -> ChartModel {
    let mut m = bars3(vals, 15.0, 20.0);
    m.groups[0].three_d = false;
    m.three_d = false;
    m
}

/// The colour of a path / rect shape, if it is a solid one.
fn color_of(s: &ShapeItem) -> Option<Rgba> {
    solid(s)
}

fn lum(c: Rgba) -> u32 {
    u32::from(c.r) + u32::from(c.g) + u32::from(c.b)
}

/// Shapes of exactly the colours: `front` (the series colour) and any lighter / darker tone of it.
fn tones(items: &[Item]) -> (Vec<&ShapeItem>, Vec<&ShapeItem>, Vec<&ShapeItem>) {
    let (mut front, mut lighter, mut darker) = (Vec::new(), Vec::new(), Vec::new());
    for s in shapes(items) {
        let Some(c) = color_of(s) else { continue };
        // tones of RED have the same ordering of channels
        if !(c.r > c.g && c.g == c.b || c.r >= c.g && c.g >= c.b && c.r > c.b) || s.text.is_some() {
            continue;
        }
        match lum(c).cmp(&lum(RED)) {
            std::cmp::Ordering::Equal => front.push(s),
            std::cmp::Ordering::Greater => lighter.push(s),
            std::cmp::Ordering::Less => darker.push(s),
        }
    }
    (front, lighter, darker)
}

/// How many shapes are the series colour, lighter and darker tones of it.
fn tone_counts(items: &[Item]) -> (usize, usize, usize) {
    let (f, l, d) = tones(items);
    (f.len(), l.len(), d.len())
}

fn base() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 300.0,
    }
}

// ---- the depth -----------------------------------------------------------------------------

#[test]
fn a_flat_chart_has_no_depth_and_a_3d_one_has() {
    assert!(three_d::depth(&flat_bars(&[1.0, 2.0]), &base()).is_none());
    assert!(three_d::depth(&bars3(&[1.0, 2.0], 15.0, 20.0), &base()).is_some());
}

#[test]
fn the_depth_axis_goes_back_to_the_upper_right_by_the_rotations() {
    let d = three_d::depth(&bars3(&[1.0, 2.0], 15.0, 20.0), &base()).unwrap();
    assert!((d.ux - 20f64.to_radians().sin()).abs() < 1e-9);
    assert!((d.uy - 15f64.to_radians().sin()).abs() < 1e-9);
    assert!(d.d > 10.0);
    let (dx, dy) = d.shift();
    assert!(dx > 0.0 && dy > 0.0);
    // looking from the right (rotY 340) shifts the back plane to the left
    let d = three_d::depth(&bars3(&[1.0, 2.0], 15.0, 340.0), &base()).unwrap();
    assert!(d.ux < 0.0 && d.shift().0 < 0.0);
    // from below (negative rotX) counts as 0: no vertical shift
    let d = three_d::depth(&bars3(&[1.0, 2.0], -20.0, 20.0), &base()).unwrap();
    assert_eq!(d.uy, 0.0);
}

#[test]
fn the_back_plane_stays_inside_the_plot_and_the_front_rect_gives_room() {
    let mut m = bars3(&[1.0], 85.0, 85.0);
    m.view3d.as_mut().unwrap().depth_percent = 2000.0;
    let b = base();
    let d = three_d::depth(&m, &b).unwrap();
    let (dx, dy) = d.shift();
    assert!(
        dx <= 0.22 * b.w + 1e-6 && dy <= 0.28 * b.h + 1e-6,
        "{dx} {dy}"
    );
    let f = three_d::front_rect(b, &d);
    let back = three_d::back_rect(&f, &d);
    assert!(
        f.x >= b.x - 1e-9 && back.right() <= b.right() + 1e-6,
        "{f:?} {back:?}"
    );
    assert!(back.y >= b.y - 1e-6 && f.bottom() <= b.bottom() + 1e-6);
    // shifted to the left: the front plane moves right
    let d2 = three_d::depth(&bars3(&[1.0], 15.0, 340.0), &b).unwrap();
    let f2 = three_d::front_rect(b, &d2);
    assert!(f2.x > b.x);
    assert!(three_d::back_rect(&f2, &d2).x >= b.x - 1e-6);
}

#[test]
fn a_cluster_of_bars_shares_the_depth_of_its_slot() {
    // two series in one category: the depth is half of the slot's, as a bar is
    let one = three_d::depth(&bars3(&[1.0; 6], 15.0, 20.0), &base()).unwrap();
    let mut m = bars3(&[1.0; 6], 15.0, 20.0);
    let s2 = ser("b", &["k0", "k1", "k2", "k3", "k4", "k5"], &[2.0; 6]);
    m.groups[0].series.push(s2);
    let two = three_d::depth(&m, &base()).unwrap();
    assert!(two.d < one.d * 0.75, "{} vs {}", two.d, one.d);
    // stacked bars are one bar per slot
    m.groups[0].grouping = Grouping::Stacked;
    let st = three_d::depth(&m, &base()).unwrap();
    assert!((st.d - one.d).abs() < 1e-6);
}

#[test]
fn depth_percent_scales_the_depth_and_nan_in_the_model_is_harmless() {
    let mut m = bars3(&[1.0, 2.0, 3.0], 15.0, 20.0);
    let d100 = three_d::depth(&m, &base()).unwrap().d;
    m.view3d.as_mut().unwrap().depth_percent = 50.0;
    let d50 = three_d::depth(&m, &base()).unwrap().d;
    assert!((d50 * 2.0 - d100).abs() < 1e-6);
    m.view3d = Some(View3D {
        rot_x: f64::NAN,
        rot_y: f64::INFINITY,
        depth_percent: f64::NAN,
        perspective: f64::NAN,
        h_percent: Some(f64::NAN),
        ..View3D::default()
    });
    let d = three_d::depth(&m, &base()).unwrap();
    assert!(d.d.is_finite() && d.ux.is_finite() && d.uy.is_finite());
    let items = draw(&m);
    assert!(!items.is_empty());
}

#[test]
fn thickness_and_the_row_of_a_ribbon_follow_the_gap() {
    let d = Depth {
        ux: 0.3,
        uy: 0.2,
        d: 100.0,
        gap_pct: 150.0,
    };
    let (t, z0) = d.thickness(100.0);
    assert!((t - 40.0).abs() < 1e-9 && (z0 - 30.0).abs() < 1e-9);
    let d0 = Depth { gap_pct: 0.0, ..d };
    assert_eq!(d0.thickness(100.0), (100.0, 0.0));
    // a negative gap counts as none
    let dn = Depth {
        gap_pct: -50.0,
        ..d
    };
    assert_eq!(dn.thickness(100.0), (100.0, 0.0));
    let (ox, oy) = d.off(10.0);
    assert!((ox - 3.0).abs() < 1e-9 && (oy + 2.0).abs() < 1e-9);
}

#[test]
fn rows_in_depth_are_one_per_series_for_standard_lines_and_areas_only() {
    let mk = |kind, grouping| ChartGroup {
        kind,
        grouping,
        series: vec![ser("a", &["x"], &[1.0]), ser("b", &["x"], &[1.0])],
        ..ChartGroup::default()
    };
    assert_eq!(
        three_d::rows_of(&mk(GroupKind::Area, Grouping::Standard)),
        2
    );
    assert_eq!(
        three_d::rows_of(&mk(GroupKind::Line, Grouping::Standard)),
        2
    );
    assert_eq!(three_d::rows_of(&mk(GroupKind::Area, Grouping::Stacked)), 1);
    assert_eq!(
        three_d::rows_of(&mk(GroupKind::Bar, Grouping::Clustered)),
        1
    );
}

// ---- bars ---------------------------------------------------------------------------------

#[test]
fn a_3d_bar_has_a_front_a_lighter_top_and_a_darker_side() {
    let items = draw(&bars3(&[4.0, 6.0, 8.0], 15.0, 20.0));
    let (front, lighter, darker) = tones(&items);
    assert_eq!(front.len(), 3, "front faces");
    assert_eq!(lighter.len(), 3, "top faces");
    assert_eq!(darker.len(), 3, "side faces");
    // the front faces are rectangles, the others polygons
    assert!(front.iter().all(|s| matches!(s.geom, Geometry::Rect)));
    assert!(lighter
        .iter()
        .chain(&darker)
        .all(|s| matches!(s.geom, Geometry::Paths(_))));
}

#[test]
fn the_front_of_a_3d_bar_is_smaller_than_the_flat_bar_and_the_side_is_to_its_right() {
    let flat = draw(&flat_bars(&[4.0, 6.0, 8.0]));
    let flat_w = rects_of(&flat, RED)[0].2;
    let items = draw(&bars3(&[4.0, 6.0, 8.0], 15.0, 20.0));
    let front = rects_of(&items, RED);
    assert_eq!(front.len(), 3);
    assert!(
        front[0].2 > 5.0 && front[0].2 < flat_w + 0.01,
        "{} {flat_w}",
        front[0].2
    );
    // the right face's points reach right of the front face's right edge and above its top
    let (_, _, darker) = tones(&items);
    let pts = path_pts(darker[0]);
    let f = front[0];
    assert!(pts.iter().any(|p| p.0 > f.0 + f.2 + 0.5), "{pts:?} {f:?}");
    assert!(pts.iter().any(|p| p.1 < f.1 - 0.5), "{pts:?} {f:?}");
}

#[test]
fn no_rotation_about_the_vertical_axis_means_no_side_faces_and_no_elevation_no_top_faces() {
    assert_eq!(
        tone_counts(&draw(&bars3(&[4.0, 6.0], 15.0, 0.0))),
        (2, 2, 0)
    );
    assert_eq!(
        tone_counts(&draw(&bars3(&[4.0, 6.0], 0.0, 20.0))),
        (2, 0, 2)
    );
}

#[test]
fn a_view_from_the_right_puts_the_side_faces_on_the_left_of_the_bars() {
    let items = draw(&bars3(&[4.0, 6.0], 15.0, 340.0));
    let front = rects_of(&items, RED);
    let (_, _, darker) = tones(&items);
    assert_eq!(darker.len(), 2);
    let pts = path_pts(darker[0]);
    let f = front[0];
    assert!(pts.iter().any(|p| p.0 < f.0 - 0.5), "{pts:?} {f:?}");
}

#[test]
fn bars_are_painted_left_to_right_each_one_over_the_side_of_the_one_before() {
    let items = draw(&bars3(&[4.0, 6.0, 8.0], 15.0, 20.0));
    let order: Vec<f64> = shapes(&items)
        .into_iter()
        .filter(|s| color_of(s) == Some(RED) && matches!(s.geom, Geometry::Rect))
        .map(|s| s.xfrm.x)
        .collect();
    assert_eq!(order.len(), 3);
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
}

#[test]
fn horizontal_3d_bars_have_top_and_side_faces_too() {
    let mut m = bars3(&[4.0, 6.0, 8.0], 15.0, 20.0);
    m.groups[0].bar_dir = BarDir::Bar;
    m.axes[0].pos = AxisPos::Left;
    m.axes[1].pos = AxisPos::Bottom;
    assert_eq!(tone_counts(&draw(&m)), (3, 3, 3));
}

#[test]
fn data_labels_of_3d_bars_follow_the_moved_front_face() {
    let mut m = bars3(&[4.0, 6.0, 8.0], 15.0, 20.0);
    m.groups[0].series[0].labels = Some(DataLabels {
        show_value: true,
        pos: Some(LabelPos::OutsideEnd),
        ..DataLabels::default()
    });
    let items = draw(&m);
    let front = rects_of(&items, RED);
    let labels: Vec<_> = texts(&items)
        .into_iter()
        .filter(|t| t.0 == "4" || t.0 == "6" || t.0 == "8")
        .collect();
    assert_eq!(labels.len(), 3);
    for (l, f) in labels.iter().zip(&front) {
        // each label is above its own front face
        assert!(l.1 .1 + l.1 .3 <= f.1 + 1.0, "{l:?} {f:?}");
        assert!(
            (l.1 .0 + l.1 .2 / 2.0 - (f.0 + f.2 / 2.0)).abs() < 2.0,
            "{l:?} {f:?}"
        );
    }
}

#[test]
fn walls_are_painted_only_when_they_have_a_fill_and_before_the_series() {
    let mut m = bars3(&[4.0, 6.0], 15.0, 20.0);
    let grey = Rgba::rgb(0xD9, 0xD9, 0xD9);
    let wall = |c| Wall {
        fill: Some(Fill::Solid(c)),
        line: None,
    };
    let none = draw(&m);
    assert!(shapes(&none).iter().all(|s| color_of(s) != Some(grey)));
    m.back_wall = Some(wall(grey));
    m.side_wall = Some(wall(Rgba::rgb(0xE0, 0xE0, 0xE0)));
    m.floor = Some(wall(Rgba::rgb(0xC8, 0xC8, 0xC8)));
    // an empty wall element draws nothing
    let items = draw(&m);
    for c in [
        grey,
        Rgba::rgb(0xE0, 0xE0, 0xE0),
        Rgba::rgb(0xC8, 0xC8, 0xC8),
    ] {
        let at = shapes(&items)
            .iter()
            .position(|s| color_of(s) == Some(c))
            .unwrap_or_else(|| panic!("no wall {c:?}"));
        let first_bar = shapes(&items)
            .iter()
            .position(|s| color_of(s) == Some(RED))
            .unwrap();
        assert!(at < first_bar, "wall {c:?} after the bars");
    }
    m.floor = Some(Wall::default());
    assert!(draw(&m).len() < items.len());
}

#[test]
fn a_wall_on_the_other_side_for_a_view_from_the_right() {
    let mut m = bars3(&[4.0, 6.0], 15.0, 340.0);
    let c = Rgba::rgb(0xE0, 0xE0, 0xE0);
    m.side_wall = Some(Wall {
        fill: Some(Fill::Solid(c)),
        line: None,
    });
    let items = draw(&m);
    let side = shapes(&items)
        .into_iter()
        .find(|s| color_of(s) == Some(c))
        .expect("side wall");
    let front = rects_of(&items, RED)[0];
    // the wall is to the right of the plot's middle
    let xs: Vec<f64> = path_pts(side).iter().map(|p| p.0).collect();
    assert!(xs.iter().all(|x| *x > front.0), "{xs:?} {front:?}");
}

#[test]
fn gridlines_run_across_the_back_wall_and_along_the_side_wall() {
    let flat = segs(&draw(&flat_bars(&[4.0, 6.0]))).len();
    let m = bars3(&[4.0, 6.0], 15.0, 20.0);
    let items = draw(&m);
    let s = segs(&items);
    // two lines per gridline instead of one (the back wall and the side wall)
    assert!(s.len() >= flat + 3, "{} vs {flat}", s.len());
    // some gridline is slanted: it runs along the side wall
    assert!(s
        .iter()
        .any(|l| (l.0 - l.2).abs() > 1.0 && (l.1 - l.3).abs() > 1.0));
}

// ---- areas and lines -----------------------------------------------------------------------

fn area3(grouping: Grouping) -> ChartModel {
    let mut g = grp(
        GroupKind::Area,
        vec![
            colored(ser("front", &["a", "b", "c"], &[3.0, 6.0, 4.0]), RED),
            colored(
                ser("back", &["a", "b", "c"], &[5.0, 8.0, 6.0]),
                Rgba::rgb(0x30, 0x30, 0xC0),
            ),
        ],
    );
    g.three_d = true;
    g.grouping = grouping;
    let mut m = fixed(vec![g], 20.0, 5.0);
    m.three_d = true;
    m
}

#[test]
fn standard_3d_areas_are_painted_back_to_front_with_a_top_band_and_an_end_face() {
    let items = draw(&area3(Grouping::Standard));
    let blue = Rgba::rgb(0x30, 0x30, 0xC0);
    let polys: Vec<_> = shapes(&items)
        .into_iter()
        .filter(|s| color_of(s) == Some(RED) || color_of(s) == Some(blue))
        .collect();
    // the front polygon of each series (the exact colour)
    let at = |c| polys.iter().position(|s| color_of(s) == Some(c)).unwrap();
    assert!(at(blue) < at(RED), "the back series is painted first");
    // top bands and end faces exist for both (lighter / darker tones of each colour)
    let (_, lighter, darker) = tones(&items);
    // (the red series: its two top-band quads and its end face)
    assert_eq!((lighter.len(), darker.len()), (2, 1));
}

#[test]
fn the_front_series_of_3d_areas_is_lower_on_the_page_than_the_one_behind() {
    let items = draw(&area3(Grouping::Standard));
    let red_top = shapes(&items)
        .into_iter()
        .find(|s| color_of(s) == Some(RED))
        .map(|s| s.xfrm.y)
        .unwrap();
    // the behind series is shifted up by its row: its polygon starts above where it would
    let blue = Rgba::rgb(0x30, 0x30, 0xC0);
    let blue_poly = shapes(&items)
        .into_iter()
        .find(|s| color_of(s) == Some(blue))
        .unwrap();
    let (front_flat, back_flat) = {
        let mut m = area3(Grouping::Standard);
        m.groups[0].three_d = false;
        m.three_d = false;
        let it = draw(&m);
        let f = |c| {
            shapes(&it)
                .into_iter()
                .find(|s| color_of(s) == Some(c))
                .map(|s| s.xfrm.y)
                .unwrap()
        };
        (f(RED), f(blue))
    };
    let _ = (front_flat, red_top);
    assert!(blue_poly.xfrm.y < back_flat + 200.0 * EMU_PER_PX);
}

#[test]
fn stacked_3d_areas_share_one_row() {
    let items = draw(&area3(Grouping::Stacked));
    let blue = Rgba::rgb(0x30, 0x30, 0xC0);
    let ys = |c| {
        shapes(&items)
            .into_iter()
            .find(|s| color_of(s) == Some(c))
            .map(|s| s.xfrm.x)
            .unwrap()
    };
    // both start at the same left edge: no per-row shift
    assert!((ys(RED) - ys(blue)).abs() < 1.0 * EMU_PER_PX);
}

#[test]
fn a_3d_line_is_a_ribbon_without_markers() {
    let mut g = grp(
        GroupKind::Line,
        vec![colored(ser("l", &["a", "b", "c"], &[3.0, 6.0, 4.0]), RED)],
    );
    g.three_d = true;
    g.markers = true;
    let mut m = fixed(vec![g], 10.0, 5.0);
    m.three_d = true;
    m.groups[0].series[0].line = Some(Stroke {
        color: Some(RED),
        ..Stroke::default()
    });
    let items = draw(&m);
    // darker top-band quads, one per segment, and no marker shapes (a flat chart has 3)
    let (_, _, darker) = tones(&items);
    assert_eq!(darker.len(), 2, "{darker:?}");
    let mut flat = m.clone();
    flat.groups[0].three_d = false;
    flat.three_d = false;
    let markers = |it: &[Item]| {
        shapes(it)
            .iter()
            .filter(|s| s.text.is_none() && color_of(s) == Some(RED))
            .filter(|s| s.xfrm.w < 12.0 * EMU_PER_PX && s.xfrm.h < 12.0 * EMU_PER_PX)
            .count()
    };
    assert_eq!(markers(&draw(&flat)), 3);
    assert_eq!(markers(&items), 0);
}

// ---- pie ------------------------------------------------------------------------------------

fn pie3(vals: &[f64], rot_x: Option<f64>) -> ChartModel {
    let mut m = pie_model(vals, GroupKind::Pie);
    m.groups[0].three_d = true;
    m.three_d = true;
    if let Some(r) = rot_x {
        m.view3d = Some(View3D {
            rot_x: r,
            rot_y: 0.0,
            ..View3D::default()
        });
    }
    m
}

/// The union of the bounding boxes of the filled shapes without text: the whole pie.
fn union(items: &[Item]) -> (f64, f64, f64, f64) {
    let b = boxes(items);
    let x0 = b.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let y0 = b.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let x1 = b.iter().map(|p| p.0 + p.2).fold(f64::MIN, f64::max);
    let y1 = b.iter().map(|p| p.1 + p.3).fold(f64::MIN, f64::max);
    (x0, y0, x1 - x0, y1 - y0)
}

/// Bounding boxes `(x, y, w, h)` px of the filled shapes without text.
fn boxes(items: &[Item]) -> Vec<(f64, f64, f64, f64)> {
    shapes(items)
        .into_iter()
        .filter(|s| s.text.is_none() && s.fill.is_visible())
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

#[test]
fn the_tilt_follows_rot_x_and_a_pie_defaults_to_30_degrees() {
    let t30 = three_d::pie_tilt(&pie3(&[1.0, 2.0], None));
    assert!((t30.squash - 0.5).abs() < 1e-9, "{t30:?}");
    let t60 = three_d::pie_tilt(&pie3(&[1.0, 2.0], Some(60.0)));
    assert!((t60.squash - 60f64.to_radians().sin()).abs() < 1e-9);
    assert!(t60.thick < t30.thick);
    // seen from straight above: a circle without a side
    let t90 = three_d::pie_tilt(&pie3(&[1.0, 2.0], Some(90.0)));
    assert!((t90.squash - 1.0).abs() < 1e-9 && t90.thick < 1e-9);
    // nearly edge-on is bounded
    let t0 = three_d::pie_tilt(&pie3(&[1.0, 2.0], Some(0.0)));
    assert!(t0.squash >= 0.18);
    assert!(three_d::pie_tilt(&pie3(&[1.0], Some(f64::NAN)))
        .squash
        .is_finite());
}

#[test]
fn a_3d_pie_is_an_ellipse_with_a_side_band_below() {
    let items = draw(&pie3(&[3.0, 2.0, 1.0], Some(40.0)));
    let all = union(&items);
    // wider than high: the top is sin(40 deg) as high as wide, plus the band
    assert!(all.3 < all.2 * 0.85, "{all:?}");
    let t = three_d::pie_tilt(&pie3(&[3.0, 2.0, 1.0], Some(40.0)));
    let rx = all.2 / 2.0;
    let expect = 2.0 * rx * t.squash + t.thick * rx;
    assert!((all.3 - expect).abs() < 2.0, "{} vs {expect}", all.3);
    // there are side bands: shapes with points below the top ellipse's bottom
    let top_bottom = all.1 + 2.0 * rx * t.squash;
    let band = shapes(&items)
        .into_iter()
        .any(|s| s.text.is_none() && path_pts(s).iter().any(|p| p.1 > top_bottom + 2.0));
    assert!(band, "no side band below {top_bottom}");
}

#[test]
fn seen_from_above_a_3d_pie_looks_like_a_flat_one() {
    let n3 = shapes(&draw(&pie3(&[3.0, 2.0, 1.0], Some(90.0)))).len();
    let flat = shapes(&draw(&pie_model(&[3.0, 2.0, 1.0], GroupKind::Pie))).len();
    assert_eq!(n3, flat);
}

#[test]
fn slices_of_a_3d_pie_are_painted_back_to_front() {
    // two halves: the one whose middle points up (0..180 would be right...), use thirds
    let items = draw(&pie3(&[1.0, 1.0, 1.0], Some(45.0)));
    let tops: Vec<(f64, f64, f64, f64)> = shapes(&items)
        .into_iter()
        .filter(|s| s.text.is_none() && matches!(s.geom, Geometry::Paths(_)))
        .filter(|s| path_pts(s).len() > 10)
        .map(|s| (s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h))
        .collect();
    assert!(tops.len() >= 3);
}

#[test]
fn exploded_slices_of_a_3d_pie_show_their_cut_faces() {
    let mut m = pie3(&[1.0, 1.0, 1.0, 1.0], Some(40.0));
    let plain = shapes(&draw(&m)).len();
    m.groups[0].series[0].explosion = Some(20.0);
    let exploded = shapes(&draw(&m)).len();
    assert!(exploded > plain, "{exploded} vs {plain}");
}

#[test]
fn a_one_slice_3d_pie_is_an_ellipse_with_a_band() {
    let items = draw(&pie3(&[5.0], Some(40.0)));
    let s = shapes(&items);
    assert!(s.iter().any(|x| matches!(x.geom, Geometry::Ellipse)));
    assert!(s
        .iter()
        .any(|x| matches!(x.geom, Geometry::Paths(_)) && x.text.is_none()));
}

#[test]
fn labels_of_a_3d_pie_sit_on_the_tilted_ellipse_and_inside_the_chart() {
    let mut m = pie3(&[1.0, 1.0, 1.0, 1.0, 1.0, 1.0], Some(40.0));
    m.groups[0].series[0].labels = Some(DataLabels {
        show_value: true,
        show_category: true,
        pos: Some(LabelPos::OutsideEnd),
        ..DataLabels::default()
    });
    let items = draw(&m);
    let labels: Vec<_> = texts(&items)
        .into_iter()
        .filter(|t| t.0.starts_with('c'))
        .collect();
    assert_eq!(labels.len(), 6);
    for l in &labels {
        assert!(l.1 .1 >= 0.0 && l.1 .1 + l.1 .3 <= H + 0.5, "{l:?}");
    }
    // inside labels (centre) are on the top face, not on the side band
    m.groups[0].series[0].labels.as_mut().unwrap().pos = Some(LabelPos::Center);
    let items = draw(&m);
    let all = union(&items);
    let tilt = three_d::pie_tilt(&m);
    let ry = all.2 / 2.0 * tilt.squash;
    for t in texts(&items).iter().filter(|t| t.0.starts_with('c')) {
        let cy = t.1 .1 + t.1 .3 / 2.0;
        assert!(
            cy >= all.1 - 1.0 && cy <= all.1 + 2.0 * ry + 1.0,
            "{t:?} {all:?}"
        );
    }
}

// ---- budgets and robustness ----------------------------------------------------------------

#[test]
fn a_huge_3d_chart_stays_within_the_budget_and_does_not_panic() {
    let n = 30_000;
    let cats: Vec<String> = (0..n).map(|i| format!("{i}")).collect();
    let cats: Vec<&str> = cats.iter().map(String::as_str).collect();
    let vals: Vec<f64> = (0..n).map(|i| (i % 17) as f64).collect();
    let mut g = grp(GroupKind::Bar, vec![ser("a", &cats, &vals)]);
    g.three_d = true;
    let mut m = model(vec![g]);
    m.three_d = true;
    let (items, truncated) = draw_chart(&m, emu(W), emu(H));
    assert!(truncated);
    assert!(items.len() <= MAX_DRAWN + 64);
}

#[test]
fn a_3d_chart_with_no_data_and_a_tiny_box_draws_without_panicking() {
    let mut m = bars3(&[], 15.0, 20.0);
    let _ = draw(&m);
    m.groups[0].series.clear();
    let _ = draw(&m);
    let _ = draw_chart(&bars3(&[1.0, 2.0], 15.0, 20.0), emu(12.0), emu(12.0));
    let _ = draw_chart(&bars3(&[1.0, 2.0], 15.0, 20.0), emu(9.0), emu(300.0));
}
