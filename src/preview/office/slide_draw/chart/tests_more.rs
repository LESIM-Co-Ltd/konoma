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
