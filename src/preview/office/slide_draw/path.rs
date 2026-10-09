//! Geometry to path segments: `arcTo`, path-space scaling, and the helpers the line drawing needs
//! (end tangents, trimming for arrow heads).
//!
//! Segments are in **EMU, slide coordinates** (the box origin is already added); the SVG writer
//! divides by [`super::model::EMU_PER_PX`] when it formats numbers.

use super::model::{GeomPath, Geometry, PathCmd, Pt, Xfrm};

/// A resolved path segment (absolute coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    M(Pt),
    L(Pt),
    Q(Pt, Pt),
    C(Pt, Pt, Pt),
    Z,
}

/// Commands per path after which the rest is dropped (the result says so).
pub const MAX_PATH_CMDS: usize = 200_000;

/// The result of resolving a path.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub segs: Vec<Seg>,
    /// The command limit was hit.
    pub truncated: bool,
}

fn finite(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(-1e12, 1e12)
    } else {
        0.0
    }
}

/// Appends the cubic Bezier segments of an elliptical arc given in the ellipse's parametric angle
/// (radians), centre `c` and radii `rx`/`ry`. The arc is split in pieces of at most 90 degrees.
fn arc_cubics(
    out: &mut Vec<Seg>,
    c: Pt,
    rx: f64,
    ry: f64,
    t1: f64,
    dt: f64,
    map: &dyn Fn(Pt) -> Pt,
) {
    let n = ((dt.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize).clamp(1, 4);
    let step = dt / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let at = |t: f64| Pt::new(c.x + rx * t.cos(), c.y + ry * t.sin());
    let d = |t: f64| Pt::new(-rx * t.sin(), ry * t.cos());
    for i in 0..n {
        let a = t1 + step * i as f64;
        let b = a + step;
        let (pa, pb, da, db) = (at(a), at(b), d(a), d(b));
        out.push(Seg::C(
            map(Pt::new(pa.x + k * da.x, pa.y + k * da.y)),
            map(Pt::new(pb.x - k * db.x, pb.y - k * db.y)),
            map(pb),
        ));
    }
}

/// The parametric angle of the ellipse point seen at visual angle `deg` (DrawingML's angles are
/// visual: 45 degrees points at the corner of the bounding box of a stretched ellipse).
fn param_angle(deg: f64, wr: f64, hr: f64) -> f64 {
    let a = deg.to_radians();
    (wr * a.sin()).atan2(hr * a.cos())
}

/// Resolves one custom-geometry path inside the box `x`, `y`, `w`, `h`.
pub fn resolve_path(p: &GeomPath, x: f64, y: f64, w: f64, h: f64) -> Resolved {
    let sx = if p.w > 0.0 && p.w.is_finite() {
        w / p.w
    } else {
        1.0
    };
    let sy = if p.h > 0.0 && p.h.is_finite() {
        h / p.h
    } else {
        1.0
    };
    let map = |q: Pt| Pt::new(x + finite(q.x) * sx, y + finite(q.y) * sy);
    let mut out = Resolved::default();
    let mut cur = Pt::new(0.0, 0.0); // current point in path space
    let mut start = cur;
    for (i, cmd) in p.cmds.iter().enumerate() {
        if i >= MAX_PATH_CMDS {
            out.truncated = true;
            break;
        }
        match *cmd {
            PathCmd::MoveTo(q) => {
                cur = Pt::new(finite(q.x), finite(q.y));
                start = cur;
                out.segs.push(Seg::M(map(cur)));
            }
            PathCmd::LineTo(q) => {
                cur = Pt::new(finite(q.x), finite(q.y));
                out.segs.push(Seg::L(map(cur)));
            }
            PathCmd::QuadTo(a, b) => {
                let b = Pt::new(finite(b.x), finite(b.y));
                out.segs.push(Seg::Q(map(a), map(b)));
                cur = b;
            }
            PathCmd::CubicTo(a, b, c) => {
                let c = Pt::new(finite(c.x), finite(c.y));
                out.segs.push(Seg::C(map(a), map(b), map(c)));
                cur = c;
            }
            PathCmd::ArcTo {
                wr,
                hr,
                st_deg,
                sw_deg,
            } => {
                let (wr, hr) = (finite(wr).abs(), finite(hr).abs());
                let (st, sw) = (finite(st_deg), finite(sw_deg).clamp(-3600.0, 3600.0));
                if wr < 1e-9 || hr < 1e-9 || sw == 0.0 {
                    continue;
                }
                let t1 = param_angle(st, wr, hr);
                let c = Pt::new(cur.x - wr * t1.cos(), cur.y - hr * t1.sin());
                let t2 = param_angle(st + sw, wr, hr);
                let tau = std::f64::consts::TAU;
                // The parametric sweep that ends at the same visual angle and has the sign of `sw`.
                let mut dt = (t2 - t1).rem_euclid(tau);
                if sw < 0.0 {
                    dt -= tau;
                    if dt == -tau {
                        dt = 0.0;
                    }
                }
                // whole turns
                let turns = (sw.abs() / 360.0).floor();
                if turns >= 1.0 {
                    dt += turns * tau * sw.signum();
                }
                // an exact multiple of 360 degrees sweeps whole circles
                if (sw.abs() % 360.0) < 1e-9 && sw != 0.0 {
                    dt = sw.signum() * turns * tau;
                }
                // split long sweeps in turns of at most 180 degrees so the 90-degree pieces stay
                // a bounded count
                let pieces = ((dt.abs() / std::f64::consts::PI).ceil() as usize).clamp(1, 20);
                let step = dt / pieces as f64;
                for j in 0..pieces {
                    arc_cubics(&mut out.segs, c, wr, hr, t1 + step * j as f64, step, &map);
                }
                let te = t1 + dt;
                cur = Pt::new(c.x + wr * te.cos(), c.y + hr * te.sin());
            }
            PathCmd::Close => {
                out.segs.push(Seg::Z);
                cur = start;
            }
        }
    }
    out
}

/// A rectangle as segments.
pub fn rect_segs(x: f64, y: f64, w: f64, h: f64) -> Vec<Seg> {
    vec![
        Seg::M(Pt::new(x, y)),
        Seg::L(Pt::new(x + w, y)),
        Seg::L(Pt::new(x + w, y + h)),
        Seg::L(Pt::new(x, y + h)),
        Seg::Z,
    ]
}

/// The ellipse inscribed in the box, as four cubics.
pub fn ellipse_segs(x: f64, y: f64, w: f64, h: f64) -> Vec<Seg> {
    let (cx, cy, rx, ry) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
    let mut v = vec![Seg::M(Pt::new(cx + rx, cy))];
    arc_cubics(
        &mut v,
        Pt::new(cx, cy),
        rx,
        ry,
        0.0,
        std::f64::consts::TAU,
        &|p| p,
    );
    v.push(Seg::Z);
    v
}

/// Resolves a geometry for a box. A `Paths` geometry yields one entry per path with its
/// `(fill_mode, stroke)`; the other kinds yield one filled and stroked entry.
pub fn resolve_geometry(
    g: &Geometry,
    xf: &Xfrm,
) -> (Vec<(Resolved, super::model::PathFill, bool)>, bool) {
    use super::model::PathFill;
    let (x, y, w, h) = (xf.x, xf.y, xf.w, xf.h);
    match g {
        Geometry::Rect => (
            vec![(
                Resolved {
                    segs: rect_segs(x, y, w, h),
                    truncated: false,
                },
                PathFill::Norm,
                true,
            )],
            false,
        ),
        Geometry::Ellipse => (
            vec![(
                Resolved {
                    segs: ellipse_segs(x, y, w, h),
                    truncated: false,
                },
                PathFill::Norm,
                true,
            )],
            false,
        ),
        Geometry::Line => (
            vec![(
                Resolved {
                    segs: vec![Seg::M(Pt::new(x, y)), Seg::L(Pt::new(x + w, y + h))],
                    truncated: false,
                },
                PathFill::None,
                true,
            )],
            false,
        ),
        Geometry::Paths(ps) => {
            let mut trunc = false;
            let v = ps
                .iter()
                .map(|p| {
                    let r = resolve_path(p, x, y, w, h);
                    trunc |= r.truncated;
                    (r, p.fill_mode, p.stroke)
                })
                .collect();
            (v, trunc)
        }
    }
}

/// The last point of a segment list and the direction (unit vector) the path arrives with, or
/// `None` for an empty / degenerate path.
pub fn end_tangent(segs: &[Seg]) -> Option<(Pt, Pt)> {
    let mut prev = None::<Pt>;
    let mut last = None::<(Pt, Pt)>; // (end, direction)
    let mut sub_start = Pt::default();
    for s in segs {
        let (end, from) = match *s {
            Seg::M(p) => {
                prev = Some(p);
                sub_start = p;
                continue;
            }
            Seg::L(p) => (p, prev),
            Seg::Q(c, p) => (p, Some(c)),
            Seg::C(_, c2, p) => (p, Some(c2)),
            Seg::Z => (sub_start, prev),
        };
        if let Some(f) = from {
            let (dx, dy) = (end.x - f.x, end.y - f.y);
            let len = dx.hypot(dy);
            if len > 1e-9 {
                last = Some((end, Pt::new(dx / len, dy / len)));
            }
        }
        prev = Some(end);
    }
    last
}

/// The first point of a segment list and the direction the path leaves with (a unit vector
/// pointing *into* the path).
pub fn start_tangent(segs: &[Seg]) -> Option<(Pt, Pt)> {
    let mut first = None::<Pt>;
    for s in segs {
        match *s {
            Seg::M(p) => {
                if first.is_none() {
                    first = Some(p);
                }
            }
            Seg::L(p) | Seg::Q(p, _) | Seg::C(p, _, _) => {
                let f = first?;
                let (dx, dy) = (p.x - f.x, p.y - f.y);
                let len = dx.hypot(dy);
                if len > 1e-9 {
                    return Some((f, Pt::new(dx / len, dy / len)));
                }
                // degenerate control point: look at the next one
                if let Seg::C(_, c2, e) = *s {
                    for q in [c2, e] {
                        let (dx, dy) = (q.x - f.x, q.y - f.y);
                        let len = dx.hypot(dy);
                        if len > 1e-9 {
                            return Some((f, Pt::new(dx / len, dy / len)));
                        }
                    }
                }
            }
            Seg::Z => {}
        }
    }
    None
}

/// Moves the end of the last straight segment back by `by` (never more than 90 % of its length).
/// Curves are left alone. Used so a thick line does not poke out beyond an arrow head's tip.
pub fn trim_end(segs: &mut [Seg], by: f64) {
    let n = segs.len();
    if n < 2 {
        return;
    }
    if let (Seg::M(a) | Seg::L(a), Seg::L(b)) = (segs[n - 2], segs[n - 1]) {
        let len = (b.x - a.x).hypot(b.y - a.y);
        if len > 1e-9 {
            let k = by.min(len * 0.9) / len;
            segs[n - 1] = Seg::L(Pt::new(b.x - (b.x - a.x) * k, b.y - (b.y - a.y) * k));
        }
    }
}

/// Like [`trim_end`] at the start of the path.
pub fn trim_start(segs: &mut [Seg], by: f64) {
    if segs.len() < 2 {
        return;
    }
    if let (Seg::M(a), Seg::L(b)) = (segs[0], segs[1]) {
        let len = (b.x - a.x).hypot(b.y - a.y);
        if len > 1e-9 {
            let k = by.min(len * 0.9) / len;
            segs[0] = Seg::M(Pt::new(a.x + (b.x - a.x) * k, a.y + (b.y - a.y) * k));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Pt {
        Pt::new(x, y)
    }

    fn end_of(r: &Resolved) -> Pt {
        match *r.segs.last().unwrap() {
            Seg::M(p) | Seg::L(p) | Seg::Q(_, p) | Seg::C(_, _, p) => p,
            Seg::Z => panic!("ends with Z"),
        }
    }

    fn near(a: Pt, b: Pt) {
        assert!(
            (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6,
            "{a:?} != {b:?}"
        );
    }

    fn arc(wr: f64, hr: f64, st: f64, sw: f64, from: Pt) -> Resolved {
        let path = GeomPath {
            cmds: vec![
                PathCmd::MoveTo(from),
                PathCmd::ArcTo {
                    wr,
                    hr,
                    st_deg: st,
                    sw_deg: sw,
                },
            ],
            ..Default::default()
        };
        resolve_path(&path, 0.0, 0.0, 1000.0, 1000.0)
    }

    #[test]
    fn quarter_circle_clockwise() {
        // start at the right-most point of a circle centred (50, 50), radius 50; 0 -> 90 degrees
        // sweeps (clockwise on screen) to the bottom point
        let r = arc(50.0, 50.0, 0.0, 90.0, p(100.0, 50.0));
        assert_eq!(r.segs.len(), 2);
        near(end_of(&r), p(50.0, 100.0));
    }

    #[test]
    fn half_circle_and_negative_sweeps() {
        let r = arc(50.0, 50.0, 0.0, 180.0, p(100.0, 50.0));
        near(end_of(&r), p(0.0, 50.0));
        // counter-clockwise from the right point goes up first and ends at the top after 90
        let r = arc(50.0, 50.0, 0.0, -90.0, p(100.0, 50.0));
        near(end_of(&r), p(50.0, 0.0));
        let r = arc(50.0, 50.0, 0.0, -180.0, p(100.0, 50.0));
        near(end_of(&r), p(0.0, 50.0));
        // start at the top (270 degrees), sweep +90 -> right
        let r = arc(50.0, 50.0, 270.0, 90.0, p(50.0, 0.0));
        near(end_of(&r), p(100.0, 50.0));
        // start at the left (180), sweep -90 -> up/top
        let r = arc(50.0, 50.0, 180.0, -90.0, p(0.0, 50.0));
        near(end_of(&r), p(50.0, 100.0));
        // ... and the same from the left with +90 goes through the top
        let r = arc(50.0, 50.0, 180.0, 90.0, p(0.0, 50.0));
        near(end_of(&r), p(50.0, 0.0));
    }

    #[test]
    fn full_and_multiple_turns_return_to_start() {
        let r = arc(50.0, 50.0, 0.0, 360.0, p(100.0, 50.0));
        near(end_of(&r), p(100.0, 50.0));
        assert_eq!(r.segs.len(), 1 + 4);
        let r = arc(50.0, 50.0, 0.0, -360.0, p(100.0, 50.0));
        near(end_of(&r), p(100.0, 50.0));
        let r = arc(50.0, 50.0, 90.0, 720.0, p(50.0, 100.0));
        near(end_of(&r), p(50.0, 100.0));
        // 270 degrees
        let r = arc(50.0, 50.0, 0.0, 270.0, p(100.0, 50.0));
        near(end_of(&r), p(50.0, 0.0));
    }

    #[test]
    fn arc_points_lie_on_the_ellipse() {
        // elliptical, non-trivial start angle: visual 45 degrees on a 100x50 ellipse points at the
        // corner direction of its bounding box
        let (wr, hr) = (100.0, 50.0);
        let t1 = param_angle(45.0, wr, hr);
        let from = p(wr * t1.cos() + 200.0, hr * t1.sin() + 100.0);
        let r = arc(wr, hr, 45.0, 200.0, from);
        let c = p(200.0, 100.0);
        for s in &r.segs[1..] {
            if let Seg::C(_, _, e) = *s {
                let v = ((e.x - c.x) / wr).powi(2) + ((e.y - c.y) / hr).powi(2);
                assert!((v - 1.0).abs() < 1e-9, "{v}");
            }
        }
        // the visual end angle is 245 degrees
        let e = end_of(&r);
        let vis = (e.y - c.y).atan2(e.x - c.x).to_degrees().rem_euclid(360.0);
        let want = 245f64;
        // convert: visual angle of the ellipse point
        let want_param = param_angle(want, wr, hr);
        let wp = p(c.x + wr * want_param.cos(), c.y + hr * want_param.sin());
        near(e, wp);
        assert!((vis - want).abs() < 1e-6, "{vis}");
    }

    #[test]
    fn path_space_scaling() {
        let path = GeomPath {
            w: 100.0,
            h: 50.0,
            cmds: vec![
                PathCmd::MoveTo(p(0.0, 0.0)),
                PathCmd::LineTo(p(100.0, 50.0)),
                PathCmd::Close,
            ],
            ..Default::default()
        };
        let r = resolve_path(&path, 10.0, 20.0, 1000.0, 400.0);
        assert_eq!(r.segs[0], Seg::M(p(10.0, 20.0)));
        assert_eq!(r.segs[1], Seg::L(p(1010.0, 420.0)));
        assert_eq!(r.segs[2], Seg::Z);
        // w = h = 0 means "the path is already in box units"
        let path = GeomPath {
            cmds: vec![PathCmd::MoveTo(p(5.0, 6.0)), PathCmd::LineTo(p(7.0, 8.0))],
            ..Default::default()
        };
        let r = resolve_path(&path, 100.0, 200.0, 1000.0, 400.0);
        assert_eq!(r.segs[1], Seg::L(p(107.0, 208.0)));
    }

    #[test]
    fn arc_in_a_scaled_path_scales_with_it() {
        // a circle of radius 50 in a 100x100 path space stretched to a 200x100 box is an ellipse
        let path = GeomPath {
            w: 100.0,
            h: 100.0,
            cmds: vec![
                PathCmd::MoveTo(p(100.0, 50.0)),
                PathCmd::ArcTo {
                    wr: 50.0,
                    hr: 50.0,
                    st_deg: 0.0,
                    sw_deg: 180.0,
                },
            ],
            ..Default::default()
        };
        let r = resolve_path(&path, 0.0, 0.0, 200.0, 100.0);
        near(end_of(&r), p(0.0, 50.0));
        // the top of the arc (after the first quarter) is at x = 100
        if let Seg::C(_, _, e) = r.segs[1] {
            near(e, p(100.0, 100.0));
        } else {
            panic!()
        }
    }

    #[test]
    fn degenerate_and_hostile_commands() {
        let path = GeomPath {
            cmds: vec![
                PathCmd::MoveTo(p(f64::NAN, f64::INFINITY)),
                PathCmd::ArcTo {
                    wr: 0.0,
                    hr: 5.0,
                    st_deg: 0.0,
                    sw_deg: 90.0,
                },
                PathCmd::ArcTo {
                    wr: 5.0,
                    hr: 5.0,
                    st_deg: f64::NAN,
                    sw_deg: f64::NAN,
                },
                PathCmd::ArcTo {
                    wr: 5.0,
                    hr: 5.0,
                    st_deg: 0.0,
                    sw_deg: 1e12,
                },
                PathCmd::LineTo(p(1e300, -1e300)),
            ],
            ..Default::default()
        };
        let r = resolve_path(&path, 0.0, 0.0, 10.0, 10.0);
        for s in &r.segs {
            let pts: Vec<Pt> = match *s {
                Seg::M(a) | Seg::L(a) => vec![a],
                Seg::Q(a, b) => vec![a, b],
                Seg::C(a, b, c) => vec![a, b, c],
                Seg::Z => vec![],
            };
            for q in pts {
                assert!(q.x.is_finite() && q.y.is_finite());
            }
        }
        // a huge command list is cut
        let big = GeomPath {
            cmds: vec![PathCmd::LineTo(p(1.0, 1.0)); MAX_PATH_CMDS + 10],
            ..Default::default()
        };
        let r = resolve_path(&big, 0.0, 0.0, 1.0, 1.0);
        assert!(r.truncated);
        assert_eq!(r.segs.len(), MAX_PATH_CMDS);
    }

    #[test]
    fn rect_ellipse_and_line_segments() {
        let xf = Xfrm::rect(10.0, 20.0, 100.0, 50.0);
        let (v, _) = resolve_geometry(&Geometry::Rect, &xf);
        assert_eq!(v[0].0.segs.len(), 5);
        let (v, _) = resolve_geometry(&Geometry::Ellipse, &xf);
        near(
            match v[0].0.segs[0] {
                Seg::M(p) => p,
                _ => panic!(),
            },
            p(110.0, 45.0),
        );
        let (v, _) = resolve_geometry(&Geometry::Line, &xf);
        assert_eq!(v[0].0.segs[1], Seg::L(p(110.0, 70.0)));
    }

    #[test]
    fn tangents_and_trimming() {
        let mut s = vec![
            Seg::M(p(0.0, 0.0)),
            Seg::L(p(10.0, 0.0)),
            Seg::L(p(10.0, 10.0)),
        ];
        let (e, d) = end_tangent(&s).unwrap();
        near(e, p(10.0, 10.0));
        near(d, p(0.0, 1.0));
        let (b, d) = start_tangent(&s).unwrap();
        near(b, p(0.0, 0.0));
        near(d, p(1.0, 0.0));
        trim_end(&mut s, 4.0);
        assert_eq!(s[2], Seg::L(p(10.0, 6.0)));
        trim_end(&mut s, 1000.0);
        // never more than 90 % of what is left of the segment (6 px -> 0.6)
        match s[2] {
            Seg::L(q) => near(q, p(10.0, 0.6)),
            _ => panic!(),
        }
        let mut s = vec![Seg::M(p(0.0, 0.0)), Seg::L(p(10.0, 0.0))];
        trim_start(&mut s, 3.0);
        assert_eq!(s[0], Seg::M(p(3.0, 0.0)));
        assert!(end_tangent(&[]).is_none());
        assert!(start_tangent(&[Seg::M(p(1.0, 1.0))]).is_none());
        // a zero-length last segment falls back to the previous direction
        let s = vec![
            Seg::M(p(0.0, 0.0)),
            Seg::L(p(5.0, 0.0)),
            Seg::L(p(5.0, 0.0)),
        ];
        near(end_tangent(&s).unwrap().1, p(1.0, 0.0));
    }
}
