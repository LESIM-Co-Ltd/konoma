//! More chart drawing tests: colour styles, secondary axes, markers, tilted labels.

use super::tests::*;
use super::*;

#[test]
fn monochrome_styles_ramp_from_dark_to_light() {
    let mut m = ChartModel {
        palette: vec![Rgba::rgb(0xA3, 0x26, 0x36), Rgba::rgb(1, 2, 3)],
        groups: vec![grp(
            GroupKind::Bar,
            (0..4)
                .map(|i| ser(&format!("s{i}"), &["a"], &[1.0]))
                .collect(),
        )],
        ..ChartModel::default()
    };
    m.style = 3;
    let cols: Vec<Rgba> = (0..4).map(|i| layout::series_color(&m, i)).collect();
    let lum = |c: &Rgba| c.r as u32 + c.g as u32 + c.b as u32;
    assert!(
        lum(&cols[0]) < lum(&cols[1])
            && lum(&cols[1]) < lum(&cols[2])
            && lum(&cols[2]) < lum(&cols[3])
    );
    // Style 4 uses the second accent; 11 is style 3 again (every eighth).
    m.style = 4;
    assert_ne!(layout::series_color(&m, 0), cols[0]);
    m.style = 11;
    assert_eq!(layout::series_color(&m, 0), cols[0]);
    // A single series gets the accent itself; style 2 stays colourful.
    m.groups[0].series.truncate(1);
    m.style = 3;
    assert_eq!(layout::series_color(&m, 0), Rgba::rgb(0xA3, 0x26, 0x36));
    m.style = 2;
    assert_eq!(layout::series_color(&m, 0), Rgba::rgb(0xA3, 0x26, 0x36));
    m.groups[0].series.push(ser("t", &["a"], &[1.0]));
    assert_eq!(layout::series_color(&m, 1), Rgba::rgb(1, 2, 3));
}

#[test]
fn secondary_axis_has_its_own_scale_on_the_right() {
    let bars = grp(
        GroupKind::Bar,
        vec![colored(ser("a", &["p", "q"], &[10.0, 20.0]), A)],
    );
    let mut line = grp(
        GroupKind::Line,
        vec![ser("b", &["p", "q"], &[1000.0, 2000.0])],
    );
    line.axis_ids = vec![3, 4];
    line.markers = false;
    let mut m = model(vec![bars, line]);
    m.axes.push(Axis {
        id: 3,
        kind: AxisKind::Cat,
        cross_ax: 4,
        deleted: true,
        ..Axis::default()
    });
    m.axes.push(Axis {
        id: 4,
        kind: AxisKind::Val,
        cross_ax: 3,
        pos: AxisPos::Right,
        crosses: Crosses::Max,
        ..Axis::default()
    });
    let items = draw(&m);
    let t = texts(&items);
    // Labels of both scales; the second ones are right of the plot.
    let right = t
        .iter()
        .find(|t| t.0 == "2000" || t.0 == "2500")
        .expect("secondary labels");
    let left = t
        .iter()
        .find(|t| t.0 == "20" || t.0 == "25")
        .expect("primary labels");
    assert!(
        right.1 .0 > W * 0.7 && left.1 .0 < W * 0.3,
        "{right:?} {left:?}"
    );
}

#[test]
fn every_marker_symbol_draws_something() {
    for sym in [
        MarkerSymbol::Circle,
        MarkerSymbol::Square,
        MarkerSymbol::Diamond,
        MarkerSymbol::Triangle,
        MarkerSymbol::TriangleDown,
        MarkerSymbol::TriangleLeft,
        MarkerSymbol::TriangleRight,
        MarkerSymbol::Bowtie,
        MarkerSymbol::Sandglass,
        MarkerSymbol::VBar,
        MarkerSymbol::X,
        MarkerSymbol::Star,
        MarkerSymbol::Dot,
        MarkerSymbol::Dash,
        MarkerSymbol::Plus,
    ] {
        let mut s = ser("a", &["p", "q"], &[1.0, 2.0]);
        s.marker = Some(Marker {
            symbol: sym,
            size_pt: Some(8.0),
            ..Marker::default()
        });
        s.line = Some(Stroke {
            none: true,
            ..Stroke::default()
        });
        let items = draw(&fixed(vec![grp(GroupKind::Line, vec![s])], 3.0, 1.0));
        let grid = Fill::Solid(Rgba::rgb(0x86, 0x86, 0x86));
        let n = shapes(&items)
            .iter()
            .filter(|s| s.line.as_ref().is_some_and(|l| l.fill != grid) || solid(s).is_some())
            .count();
        assert!(n >= 2, "{sym:?}: {n}");
    }
}

#[test]
fn tilted_labels_hang_off_their_ticks() {
    let cats: Vec<String> = (0..10)
        .map(|i| format!("a long category label {i}"))
        .collect();
    let refs: Vec<&str> = cats.iter().map(String::as_str).collect();
    let mut m = model(vec![grp(GroupKind::Bar, vec![ser("a", &refs, &[1.0; 10])])]);
    m.axes[0].text.rot_deg = Some(-45.0);
    let items = draw(&m);
    let tx = texts(&items);
    let tilted: Vec<_> = tx.iter().filter(|t| t.0.starts_with("a long")).collect();
    assert!(!tilted.is_empty());
    assert!(
        tilted.iter().all(|t| (t.2 - 315.0).abs() < 0.01),
        "{:?}",
        tilted[0]
    );
    // Nothing leaves the chart box.
    for t in &tx {
        assert!(t.1 .0 + t.1 .2 > 0.0 && t.1 .1 + t.1 .3 > 0.0 && t.1 .0 < W && t.1 .1 < H);
    }
}

#[test]
fn the_axis_title_stays_left_of_the_tick_labels() {
    let g = grp(GroupKind::Bar, vec![ser("a", &["p"], &[1.0])]);
    let mut m = model(vec![g]);
    m.axes[1].title = Some(ChartText {
        text: "Vertical".into(),
        ..ChartText::default()
    });
    let tx = texts(&draw(&m));
    let title = tx.iter().find(|t| t.0 == "Vertical").unwrap();
    let label = tx.iter().find(|t| t.0 == "1").unwrap();
    // The rotated title's box is centred left of the labels.
    let title_right = title.1 .0 + title.1 .2 / 2.0 + title.1 .3 / 2.0;
    assert!(title_right <= label.1 .0 + 4.0, "{title:?} {label:?}");
}

// ---- legend frame, style outlines, scale minimum, label placement ----

/// Whether any shape of the drawing has the given outline colour (width ignored).
fn has_outline(items: &[Item], c: Rgba) -> bool {
    shapes(items)
        .iter()
        .any(|s| s.line.as_ref().is_some_and(|l| l.fill == Fill::Solid(c)))
}

#[test]
fn a_legend_outline_without_a_colour_is_not_drawn_but_a_coloured_one_is() {
    let mut m = model(vec![grp(
        GroupKind::Bar,
        vec![ser("First", &["a"], &[1.0]), ser("Second", &["a"], &[2.0])],
    )]);
    m.legend = Some(Legend {
        line: Some(Stroke::default()),
        ..Legend::default()
    });
    // `a:ln` with no fill of its own: the automatic outline, which is none.
    assert!(!has_outline(&draw(&m), Rgba::BLACK));
    m.legend.as_mut().unwrap().line = Some(Stroke {
        color: Some(Rgba::rgb(0x12, 0x34, 0x56)),
        ..Stroke::default()
    });
    assert!(has_outline(&draw(&m), Rgba::rgb(0x12, 0x34, 0x56)));
    // Explicitly none.
    m.legend.as_mut().unwrap().line = Some(Stroke {
        none: true,
        ..Stroke::default()
    });
    let items = draw(&m);
    assert!(!has_outline(&items, Rgba::BLACK));
}

#[test]
fn styles_9_to_16_outline_stacked_segments_in_white_unless_the_file_says_otherwise() {
    let mk = |style: u8, line: Option<Stroke>| {
        let mut s = ser("a", &["x", "y"], &[3.0, 4.0]);
        let mut s2 = ser("b", &["x", "y"], &[1.0, 2.0]);
        s.line = line.clone();
        s2.line = line;
        let mut g = grp(GroupKind::Bar, vec![s, s2]);
        g.grouping = Grouping::Stacked;
        let mut m = model(vec![g]);
        m.style = style;
        draw(&m)
    };
    for style in [9, 12, 16] {
        assert!(has_outline(&mk(style, None), Rgba::WHITE), "style {style}");
    }
    for style in [0, 1, 2, 8, 17, 18, 48] {
        assert!(!has_outline(&mk(style, None), Rgba::WHITE), "style {style}");
    }
    // The file's own `a:ln` wins (no line at all included).
    let none = Stroke {
        none: true,
        ..Stroke::default()
    };
    assert!(!has_outline(&mk(12, Some(none)), Rgba::WHITE));
    let red = Stroke {
        color: Some(Rgba::rgb(255, 0, 0)),
        ..Stroke::default()
    };
    let items = mk(12, Some(red));
    assert!(has_outline(&items, Rgba::rgb(255, 0, 0)));
}

#[test]
fn styles_9_to_16_outline_pie_slices_and_areas_too() {
    let mut m = pie_model(&[1.0, 2.0], GroupKind::Pie);
    m.style = 10;
    assert!(has_outline(&draw(&m), Rgba::WHITE));
    m.style = 2;
    assert!(!has_outline(&draw(&m), Rgba::WHITE));
    let mut a = model(vec![grp(
        GroupKind::Area,
        vec![ser("a", &["x", "y"], &[1.0, 2.0])],
    )]);
    a.style = 14;
    assert!(has_outline(&draw(&a), Rgba::WHITE));
}

#[test]
fn data_in_the_top_sixth_starts_the_axis_a_twentieth_of_the_range_below_the_data() {
    // Peltier: min = first major unit <= Ymin - (Ymax - Ymin) / 20.
    let (mn, mx, u) = scale::auto_range(100.0, 102.0, 6.0);
    assert!(
        mn <= 100.0 - 2.0 / 20.0 && mn > 100.0 - 2.0 / 20.0 - u,
        "{mn} {u}"
    );
    assert!(mx >= 102.0);
    // A wide spread still starts at zero; mirrored for negative data.
    assert_eq!(scale::auto_range(6.0, 21.0, 6.0).0, 0.0);
    let (mn, mx, u) = scale::auto_range(-102.0, -100.0, 6.0);
    assert!(
        mx >= -100.0 + 2.0 / 20.0 && mx < -100.0 + 2.0 / 20.0 + u + 1e-9,
        "{mx} {u}"
    );
    assert!(mn <= -102.0);
}

/// Do any two of the boxes (x, y, w, h) overlap?
fn any_overlap(boxes: &[(f64, f64, f64, f64)]) -> bool {
    for (i, a) in boxes.iter().enumerate() {
        for b in &boxes[i + 1..] {
            if a.0 < b.0 + b.2 - 0.5
                && b.0 < a.0 + a.2 - 0.5
                && a.1 < b.1 + b.3 - 0.5
                && b.1 < a.1 + a.3 - 0.5
            {
                return true;
            }
        }
    }
    false
}

fn pie_with_labels(vals: &[f64]) -> ChartModel {
    let mut m = pie_model(vals, GroupKind::Pie);
    m.groups[0].series[0].labels = Some(DataLabels {
        show_value: true,
        show_category: true,
        pos: Some(LabelPos::BestFit),
        ..DataLabels::default()
    });
    m
}

#[test]
fn small_pie_slices_next_to_each_other_do_not_have_overlapping_labels_and_get_leader_lines() {
    // Six slices of one percent in a row: their labels would stack on one another.
    let m = pie_with_labels(&[60.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 34.0]);
    let items = draw(&m);
    let t = texts(&items);
    let labels: Vec<_> = t
        .iter()
        .filter(|t| t.0.starts_with('c'))
        .map(|t| t.1)
        .collect();
    assert_eq!(labels.len(), 8, "{t:?}");
    assert!(!any_overlap(&labels), "{labels:?}");
    // At least one label was moved and is joined to its slice by a gray line.
    let gray = Rgba::rgb(0x86, 0x86, 0x86);
    assert!(segs(&items).iter().any(|s| s.4 == gray), "no leader line");
    // Everything stays inside the chart.
    for l in &labels {
        assert!(l.1 >= 0.0 && l.1 + l.3 <= H + 0.5, "{l:?}");
    }
}

#[test]
fn pie_labels_that_do_not_collide_get_no_leader_line() {
    let m = pie_with_labels(&[1.0, 1.0, 1.0, 1.0]);
    let items = draw(&m);
    let gray = Rgba::rgb(0x86, 0x86, 0x86);
    assert!(!segs(&items).iter().any(|s| s.4 == gray));
}

#[test]
fn labels_of_points_that_would_cover_each_other_are_moved_apart() {
    // Two series on top of each other: every point's label sits at the same spot.
    let mk = || {
        let mut a = ser("a", &["p", "q"], &[5.0, 6.0]);
        let mut b = ser("b", &["p", "q"], &[5.0, 6.0]);
        for s in [&mut a, &mut b] {
            s.labels = Some(DataLabels {
                show_value: true,
                pos: Some(LabelPos::Above),
                ..DataLabels::default()
            });
        }
        let mut g = grp(GroupKind::Line, vec![a, b]);
        g.markers = true;
        model(vec![g])
    };
    let items = draw(&mk());
    let labels: Vec<_> = texts(&items)
        .into_iter()
        .filter(|t| (t.0 == "5" || t.0 == "6") && t.1 .0 > 20.0)
        .map(|t| t.1)
        .collect();
    assert_eq!(labels.len(), 4, "{labels:?}");
    assert!(!any_overlap(&labels), "{labels:?}");
}

#[test]
fn centred_pie_labels_near_the_top_do_not_overlap_each_other_or_the_pie_rim() {
    let m = pie_with_labels(&[40.0, 24.0, 14.0, 9.0, 5.0, 4.0, 3.0, 1.0]);
    let items = draw(&m);
    let labels: Vec<_> = texts(&items)
        .iter()
        .filter(|t| t.0.starts_with('c'))
        .map(|t| t.1)
        .collect();
    assert_eq!(labels.len(), 8);
    assert!(!any_overlap(&labels), "{labels:?}");
}

#[test]
fn point_labels_below_their_point_move_down_not_up() {
    let mk = |pos: LabelPos| {
        let mut a = ser("a", &["p", "q"], &[5.0, 6.0]);
        let mut b = ser("b", &["p", "q"], &[5.0, 6.0]);
        for s in [&mut a, &mut b] {
            s.labels = Some(DataLabels {
                show_value: true,
                pos: Some(pos),
                ..DataLabels::default()
            });
        }
        let mut g = grp(GroupKind::Line, vec![a, b]);
        g.markers = true;
        let items = draw(&model(vec![g]));
        let mut ys: Vec<f64> = texts(&items)
            .into_iter()
            .filter(|t| t.0 == "5" && t.1 .0 > 20.0)
            .map(|t| t.1 .1)
            .collect();
        ys.sort_by(f64::total_cmp);
        ys
    };
    let above = mk(LabelPos::Above);
    let below = mk(LabelPos::Below);
    assert_eq!(above.len(), 2);
    assert_eq!(below.len(), 2);
    // The unmoved label sits at the same place; the other one is one label height away on the
    // side that leads away from the point.
    assert!(above[0] < below[0] - 10.0, "{above:?} {below:?}");
    assert!(below[1] > above[1] + 10.0, "{above:?} {below:?}");
}

#[test]
fn triangle_markers_point_the_way_their_symbol_says() {
    for (sym, dx, dy) in [
        (MarkerSymbol::Triangle, 0.0, -1.0),
        (MarkerSymbol::TriangleDown, 0.0, 1.0),
        (MarkerSymbol::TriangleLeft, -1.0, 0.0),
        (MarkerSymbol::TriangleRight, 1.0, 0.0),
    ] {
        let mut s = ser("a", &["p", "q"], &[1.0, 2.0]);
        s.marker = Some(Marker {
            symbol: sym,
            size_pt: Some(12.0),
            ..Marker::default()
        });
        s.line = Some(Stroke {
            none: true,
            ..Stroke::default()
        });
        let items = draw(&fixed(vec![grp(GroupKind::Line, vec![s])], 3.0, 1.0));
        let tris: Vec<Vec<(f64, f64)>> = shapes(&items)
            .iter()
            .filter(|s| solid(s).is_some())
            .map(|s| path_pts(s))
            .filter(|p| p.len() == 3)
            .collect();
        assert_eq!(tris.len(), 2, "{sym:?}: one triangle per point");
        for t in tris {
            // the apex is the vertex farthest from the middle of the other two
            let (apex, base) = (0..3)
                .map(|i| {
                    let (a, b) = (t[(i + 1) % 3], t[(i + 2) % 3]);
                    let m = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                    (t[i], m)
                })
                .max_by(|x, y| {
                    let d = |p: &((f64, f64), (f64, f64))| (p.0 .0 - p.1 .0).hypot(p.0 .1 - p.1 .1);
                    d(x).total_cmp(&d(y))
                })
                .unwrap();
            let along = (apex.0 - base.0) * dx + (apex.1 - base.1) * dy;
            assert!(along > 1.0, "{sym:?}: {t:?}");
        }
    }
}

#[test]
fn libre_scaling_changes_the_automatic_ranges_of_a_scatter_chart() {
    let mut s = ser("y", &[], &[1.0, 2.0, 3.0, 4.0, 9.8]);
    s.x_values = (1..=5).map(|v| Some(f64::from(v))).collect();
    let mut g = grp(GroupKind::Scatter, vec![s]);
    g.scatter_style = ScatterStyle::LineMarker;
    let mut m = model(vec![g]);
    m.axes[0].kind = AxisKind::Val;
    let excel = text_strings(&draw(&m));
    m.libre_scaling = true;
    let libre = text_strings(&draw(&m));
    // LibreOffice: x from 0.5 to 5.5 in half units, y to 11 (the data reach 98 % of 10)
    for want in ["0.5", "1.5", "5.5", "11"] {
        assert!(libre.iter().any(|t| t == want), "{want}: {libre:?}");
    }
    assert!(!excel.iter().any(|t| t == "5.5"), "{excel:?}");
}
