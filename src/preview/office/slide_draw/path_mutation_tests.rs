//! Tests from mutation testing of the path resolver (`path.rs`): arcs become cubic Beziers that
//! stay on their ellipse, the cancel callback is looked at every 2048th command, and degenerate
//! arcs are skipped.

use std::cell::Cell;

use super::super::model::*;
use super::super::path::{resolve_path_capped, resolve_path_polled, Seg};

fn arc_path(start: (f64, f64), wr: f64, hr: f64, st_deg: f64, sw_deg: f64) -> GeomPath {
    GeomPath {
        w: 0.0,
        h: 0.0,
        fill_mode: PathFill::None,
        stroke: true,
        cmds: vec![
            PathCmd::MoveTo(Pt::new(start.0, start.1)),
            PathCmd::ArcTo {
                wr,
                hr,
                st_deg,
                sw_deg,
            },
        ],
    }
}

fn resolve(p: &GeomPath) -> Vec<Seg> {
    resolve_path_capped(p, 0.0, 0.0, 1.0, 1.0, usize::MAX).segs
}

/// Point `t` (0..=1) of a cubic.
fn bez(p0: Pt, a: Pt, b: Pt, c: Pt, t: f64) -> Pt {
    let u = 1.0 - t;
    Pt::new(
        u * u * u * p0.x + 3.0 * u * u * t * a.x + 3.0 * u * t * t * b.x + t * t * t * c.x,
        u * u * u * p0.y + 3.0 * u * u * t * a.y + 3.0 * u * t * t * b.y + t * t * t * c.y,
    )
}

/// Walks the cubics of `segs` (after the move) and returns, for each, its points at 0, 1/4, 1/2,
/// 3/4 and 1.
fn cubic_points(segs: &[Seg]) -> Vec<[Pt; 5]> {
    let mut cur = match segs[0] {
        Seg::M(p) => p,
        ref other => panic!("{other:?}"),
    };
    let mut out = Vec::new();
    for s in &segs[1..] {
        match *s {
            Seg::C(a, b, c) => {
                out.push([
                    cur,
                    bez(cur, a, b, c, 0.25),
                    bez(cur, a, b, c, 0.5),
                    bez(cur, a, b, c, 0.75),
                    c,
                ]);
                cur = c;
            }
            ref other => panic!("{other:?}"),
        }
    }
    out
}

fn on_ellipse(p: Pt, c: (f64, f64), wr: f64, hr: f64) -> f64 {
    let v = ((p.x - c.0) / wr).powi(2) + ((p.y - c.1) / hr).powi(2);
    (v.sqrt() - 1.0).abs()
}

#[test]
fn a_quarter_circle_arc_is_one_cubic_with_the_standard_control_points() {
    // the circle of radius 100 around the origin, from 3 o'clock (0 degrees) a quarter turn
    // clockwise on screen (y down): to 6 o'clock
    let segs = resolve(&arc_path((100.0, 0.0), 100.0, 100.0, 0.0, 90.0));
    assert_eq!(segs.len(), 2);
    let k = 4.0 / 3.0 * (std::f64::consts::FRAC_PI_2 / 4.0).tan() * 100.0;
    match segs[1] {
        Seg::C(a, b, c) => {
            let near = |p: Pt, x: f64, y: f64| {
                (p.x - x).abs() < 1e-6 * 100.0 && (p.y - y).abs() < 1e-6 * 100.0
            };
            assert!(near(a, 100.0, k), "{a:?}");
            assert!(near(b, k, 100.0), "{b:?}");
            assert!(near(c, 0.0, 100.0), "{c:?}");
        }
        ref other => panic!("{other:?}"),
    }
}

#[test]
fn arcs_stay_on_their_ellipse_for_any_sweep_direction_and_radii() {
    for (wr, hr) in [(100.0, 100.0), (200.0, 50.0), (30.0, 90.0)] {
        for (st, sw) in [
            (0.0f64, 90.0f64),
            (0.0, 90.0),
            (0.0, 180.0),
            (30.0, 270.0),
            (0.0, 360.0),
            (-45.0, 100.0),
            (0.0, -90.0),
            (60.0, -200.0),
            (10.0, -360.0),
        ] {
            // the arc starts at angle `st` of the ellipse around the origin (visual angle: the
            // point is at the parametric angle atan2(wr sin, hr cos))
            let t = f64::atan2(wr * st.to_radians().sin(), hr * st.to_radians().cos());
            let start = (wr * t.cos(), hr * t.sin());
            let segs = resolve(&arc_path(start, wr, hr, st, sw));
            let pieces = cubic_points(&segs);
            assert!(!pieces.is_empty(), "{wr}x{hr} {st}/{sw}");
            assert!(
                pieces.len() <= 8,
                "{wr}x{hr} {st}/{sw}: {} pieces",
                pieces.len()
            );
            for (i, ps) in pieces.iter().enumerate() {
                for p in ps {
                    assert!(
                        on_ellipse(*p, (0.0, 0.0), wr, hr) < 2e-3,
                        "{wr}x{hr} {st}/{sw} piece {i}: {p:?} is {} off",
                        on_ellipse(*p, (0.0, 0.0), wr, hr)
                    );
                }
            }
            // the pieces join up
            for w in pieces.windows(2) {
                assert!(
                    (w[0][4].x - w[1][0].x).abs() < 1e-9 && (w[0][4].y - w[1][0].y).abs() < 1e-9
                );
            }
            // each piece sweeps at most 90 degrees of parameter: it is monotone in angle
            // and the whole arc is the asked sweep: the end is at visual angle st + sw
            let end = pieces.last().unwrap()[4];
            let want_t = f64::atan2(
                wr * (st + sw).to_radians().sin(),
                hr * (st + sw).to_radians().cos(),
            );
            let want = (wr * want_t.cos(), hr * want_t.sin());
            assert!(
                (end.x - want.0).abs() < 1e-6 * wr && (end.y - want.1).abs() < 1e-6 * hr,
                "{wr}x{hr} {st}/{sw}: ends at {end:?}, wanted {want:?}"
            );
        }
    }
}

#[test]
fn a_negative_sweep_goes_the_other_way_round() {
    // 3 o'clock, a quarter turn anticlockwise on screen: to 12 o'clock (y up is negative)
    let segs = resolve(&arc_path((100.0, 0.0), 100.0, 100.0, 0.0, -90.0));
    assert_eq!(
        cubic_points(&segs).len(),
        1,
        "a quarter turn is one piece, not a turn and a quarter"
    );
    for (sw, pieces) in [
        (90.0, 1),
        (-90.0, 1),
        (180.0, 2),
        (-180.0, 2),
        (-30.0, 1),
        (30.0, 1),
    ] {
        let n = cubic_points(&resolve(&arc_path((100.0, 0.0), 100.0, 100.0, 0.0, sw))).len();
        assert_eq!(n, pieces, "sweep {sw}");
    }
    let end = cubic_points(&segs).last().unwrap()[4];
    assert!(
        end.x.abs() < 1e-6 && (end.y + 100.0).abs() < 1e-6,
        "{end:?}"
    );
    // and a three-quarter turn: to 9 o'clock the long way round the top
    let segs = resolve(&arc_path((100.0, 0.0), 100.0, 100.0, 0.0, -270.0));
    let pieces = cubic_points(&segs);
    let end = pieces.last().unwrap()[4];
    assert!(
        (end.x - 0.0).abs() < 1e-6 && (end.y - 100.0).abs() < 1e-6,
        "{end:?}"
    );
    // its middle is at 12 o'clock side: some point has y < -90
    assert!(pieces.iter().flatten().any(|p| p.y < -90.0));
}

#[test]
fn a_whole_circle_sweep_comes_back_to_where_it_started() {
    for sw in [360.0, -360.0, 720.0] {
        let segs = resolve(&arc_path((100.0, 0.0), 100.0, 100.0, 0.0, sw));
        let pieces = cubic_points(&segs);
        let end = pieces.last().unwrap()[4];
        assert!(
            (end.x - 100.0).abs() < 1e-6 && end.y.abs() < 1e-6,
            "{sw}: {end:?}"
        );
        // a full circle takes four to eight pieces and covers all four quadrants
        assert!(
            pieces.len() >= 4 && pieces.len() <= 8,
            "{sw}: {}",
            pieces.len()
        );
        for q in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
            assert!(
                pieces
                    .iter()
                    .flatten()
                    .any(|p| p.x * q.0 > 50.0 && p.y * q.1 > 50.0),
                "{sw}: nothing in quadrant {q:?}"
            );
        }
    }
}

#[test]
fn an_arc_with_no_radius_or_no_sweep_adds_nothing_and_the_smallest_radius_still_counts() {
    let n = |wr: f64, hr: f64, sw: f64| resolve(&arc_path((10.0, 10.0), wr, hr, 0.0, sw)).len();
    assert_eq!(n(0.0, 50.0, 90.0), 1);
    assert_eq!(n(50.0, 0.0, 90.0), 1);
    assert_eq!(n(0.0, 0.0, 90.0), 1);
    assert_eq!(n(50.0, 50.0, 0.0), 1);
    assert_eq!(n(f64::NAN, 50.0, 90.0), 1);
    // exactly 1e-9 is a radius
    assert_eq!(n(1e-9, 50.0, 90.0), 2);
    assert_eq!(n(50.0, 1e-9, 90.0), 2);
    assert_eq!(n(50.0, 50.0, 90.0), 2);
    // a radius given negative is its size
    assert_eq!(n(-50.0, -50.0, 90.0), 2);
}

#[test]
fn the_cancel_callback_is_looked_at_every_2048th_command() {
    let mut cmds = vec![PathCmd::MoveTo(Pt::new(0.0, 0.0))];
    for i in 0..5000 {
        cmds.push(PathCmd::LineTo(Pt::new(i as f64, 1.0)));
    }
    let p = GeomPath {
        w: 0.0,
        h: 0.0,
        fill_mode: PathFill::None,
        stroke: true,
        cmds,
    };
    // never asked to stop: everything, looked at twice (before commands 2047 and 4095)
    let looks = Cell::new(0);
    let r = resolve_path_polled(&p, 0.0, 0.0, 1.0, 1.0, usize::MAX, &|| {
        looks.set(looks.get() + 1);
        false
    });
    assert_eq!(looks.get(), 2);
    assert_eq!(r.segs.len(), 5001);
    assert!(!r.truncated);
    // asked at once: the commands before the first look are resolved, none after
    let r = resolve_path_polled(&p, 0.0, 0.0, 1.0, 1.0, usize::MAX, &|| true);
    assert_eq!(r.segs.len(), 2047);
    assert!(r.truncated);
    // asked the second time
    let looks = Cell::new(0);
    let r = resolve_path_polled(&p, 0.0, 0.0, 1.0, 1.0, usize::MAX, &|| {
        looks.set(looks.get() + 1);
        looks.get() >= 2
    });
    assert_eq!(r.segs.len(), 4095);
    assert!(r.truncated);
    // a short path is never looked at
    let looks = Cell::new(0);
    let short = GeomPath {
        cmds: p.cmds[..2000].to_vec(),
        ..p.clone()
    };
    let _ = resolve_path_polled(&short, 0.0, 0.0, 1.0, 1.0, usize::MAX, &|| {
        looks.set(looks.get() + 1);
        true
    });
    assert_eq!(looks.get(), 0);
}
