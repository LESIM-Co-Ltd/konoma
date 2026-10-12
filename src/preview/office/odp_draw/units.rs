//! Small value parsers of the OpenDocument drawing reader: percentages, angles, colours, plain
//! numbers, the `svg:d` path syntax and the affine matrix of a `draw:transform`.

use crate::preview::office::slide_draw as sd;
use sd::{PathCmd, Pt, Rgba};

/// Longest `svg:d` / `draw:points` string that is parsed (bytes). More: no geometry.
pub(super) const MAX_PATH_BYTES: usize = 1 << 20;
/// Most path commands one `svg:d` may produce (arcs count as the cubics they become).
pub(super) const MAX_PATH_CMDS: usize = 100_000;
/// Largest absolute number kept of a path (viewBox units); larger values are clamped.
const MAX_COORD: f64 = 1e12;

/// `50%` -> `0.5` (also `0.5` plain is not a percentage: `None`).
pub(super) fn pct(v: &str) -> Option<f64> {
    let n: f64 = v.trim().strip_suffix('%')?.trim().parse().ok()?;
    n.is_finite().then_some(n / 100.0)
}

/// A plain finite number.
pub(super) fn number(v: &str) -> Option<f64> {
    let n: f64 = v.trim().parse().ok()?;
    n.is_finite().then_some(n)
}

/// An ODF angle in degrees: `30deg`, `0.5rad`, `100grad`, or a plain integer which is tenths of a
/// degree (the ODF 1.1 form LibreOffice still reads and writes for some attributes).
pub(super) fn angle_deg(v: &str) -> Option<f64> {
    let v = v.trim();
    let (num, mul) = if let Some(n) = v.strip_suffix("deg") {
        (n, 1.0)
    } else if let Some(n) = v.strip_suffix("grad") {
        (n, 0.9)
    } else if let Some(n) = v.strip_suffix("rad") {
        (n, 180.0 / std::f64::consts::PI)
    } else {
        (v, 0.1)
    };
    let n: f64 = num.trim().parse().ok()?;
    let d = n * mul;
    d.is_finite().then_some(d % 360.0)
}

/// `#rrggbb` (also `#rgb`) to a colour; `None` for `transparent` and anything else.
pub(super) fn color(v: &str) -> Option<Rgba> {
    let v = v.trim();
    let h = v.strip_prefix('#')?;
    if !h.is_ascii() {
        return None;
    }
    match h.len() {
        6 => Rgba::from_hex(h),
        3 => {
            let d = |i: usize| u8::from_str_radix(&h[i..=i], 16).ok().map(|x| x * 17);
            Some(Rgba::rgb(d(0)?, d(1)?, d(2)?))
        }
        _ => None,
    }
}

/// The relative luminance (0..1, sRGB without gamma: good enough to pick black or white text).
pub(super) fn luminance(c: Rgba) -> f64 {
    (0.2126 * f64::from(c.r) + 0.7152 * f64::from(c.g) + 0.0722 * f64::from(c.b)) / 255.0
}

// ---------------------------------------------------------------------------------------------
// affine matrices
// ---------------------------------------------------------------------------------------------

/// `[a b c d e f]`: `x' = a x + c y + e`, `y' = b x + d y + f`.
pub(super) type Mat = [f64; 6];

pub(super) const IDENTITY: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m * n` (apply `n` first).
pub(super) fn mul(m: Mat, n: Mat) -> Mat {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

pub(super) fn apply(m: Mat, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// The matrix of a `draw:transform` list. SVG's syntax with one difference: ODF's `rotate` (and
/// `skewX` / `skewY`) angles run **counter-clockwise** (LibreOffice writes `rotate (0.349)` for a
/// shape turned 20 degrees counter-clockwise, and the operations apply **in the order they are written**, the
/// first one to the shape first: `rotate (a) translate (x y)` turns the shape about its own
/// origin (its top-left corner) and then moves that corner to `(x, y)`. Checked against
/// LibreOffice's own renderings of the self-made decks.
pub(super) fn draw_transform(list: &str) -> Option<Mat> {
    let mut m = IDENTITY;
    let mut rest = list.trim();
    let mut ops = 0;
    while !rest.is_empty() {
        ops += 1;
        if ops > 16 {
            return None;
        }
        let open = rest.find('(')?;
        let close = rest.find(')')?;
        if close < open {
            return None;
        }
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let args: Vec<&str> = rest[open + 1..close]
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|a| !a.is_empty())
            .collect();
        let len = |a: &str| super::emu(a);
        let op = match (name, args.len()) {
            ("translate", 1 | 2) => {
                let tx = len(args[0])?;
                let ty = args.get(1).map_or(Some(0.0), |a| len(a))?;
                [1.0, 0.0, 0.0, 1.0, tx, ty]
            }
            ("scale", 1 | 2) => {
                let sx = number(args[0])?;
                let sy = args.get(1).map_or(Some(sx), |a| number(a))?;
                [sx, 0.0, 0.0, sy, 0.0, 0.0]
            }
            ("rotate", 1) => {
                let (s, c) = (-number(args[0])?).sin_cos();
                [c, s, -s, c, 0.0, 0.0]
            }
            ("skewX", 1) => [1.0, 0.0, (-number(args[0])?).tan(), 1.0, 0.0, 0.0],
            ("skewY", 1) => [1.0, (-number(args[0])?).tan(), 0.0, 1.0, 0.0, 0.0],
            ("matrix", 6) => [
                number(args[0])?,
                number(args[1])?,
                number(args[2])?,
                number(args[3])?,
                len(args[4])?,
                len(args[5])?,
            ],
            _ => return None,
        };
        // The operations apply in the order they are written (the first one to the shape first).
        m = mul(op, m);
        rest = rest[close + 1..].trim_start_matches([',', ' ', '\t', '\n', '\r']);
    }
    m.iter().all(|v| v.is_finite()).then_some(m)
}

/// The box `(x, y, w, h)` (in the coordinates the matrix is applied to) as a model box under `m`.
///
/// The model box can hold a rotation, flips and a scale: the matrix is split into
/// `rotate * scale(sx, sy)` about the box centre (`sy < 0` is a vertical flip). **A shear cannot be
/// held and is dropped** (LibreOffice writes `skewX` with a value of 1e-17 for plain shapes; a real
/// skew is drawn as the rotation and scale of the matrix's first column). `None` when the matrix
/// collapses the box.
pub(super) fn box_under(m: Mat, x: f64, y: f64, w: f64, h: f64) -> Option<sd::Xfrm> {
    let sx = m[0].hypot(m[1]);
    if !(sx.is_finite() && sx > 1e-12) {
        return None;
    }
    let det = m[0] * m[3] - m[1] * m[2];
    let sy = det / sx;
    if !(sy.is_finite() && sy.abs() > 1e-12) {
        return None;
    }
    let rot = m[1].atan2(m[0]).to_degrees();
    let (cx, cy) = apply(m, x + w / 2.0, y + h / 2.0);
    let (nw, nh) = (w * sx, h * sy.abs());
    let rot = if rot.abs() < 1e-9 { 0.0 } else { rot };
    Some(sd::Xfrm {
        x: cx - nw / 2.0,
        y: cy - nh / 2.0,
        w: nw,
        h: nh,
        rot_deg: rot,
        flip_h: false,
        flip_v: sy < 0.0,
    })
}

// ---------------------------------------------------------------------------------------------
// svg path data
// ---------------------------------------------------------------------------------------------

struct Scan<'a> {
    b: &'a [u8],
    i: usize,
}

impl Scan<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.b.len()
            && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b',')
        {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_sep();
        self.b.get(self.i).copied()
    }

    /// The next number, or `None` at a command letter / the end / garbage.
    fn num(&mut self) -> Option<f64> {
        self.skip_sep();
        let s = self.i;
        let mut j = s;
        if matches!(self.b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let mut digits = false;
        while matches!(self.b.get(j), Some(c) if c.is_ascii_digit()) {
            j += 1;
            digits = true;
        }
        if self.b.get(j) == Some(&b'.') {
            j += 1;
            while matches!(self.b.get(j), Some(c) if c.is_ascii_digit()) {
                j += 1;
                digits = true;
            }
        }
        if !digits {
            return None;
        }
        if matches!(self.b.get(j), Some(b'e' | b'E')) {
            let mut k = j + 1;
            if matches!(self.b.get(k), Some(b'+' | b'-')) {
                k += 1;
            }
            if matches!(self.b.get(k), Some(c) if c.is_ascii_digit()) {
                while matches!(self.b.get(k), Some(c) if c.is_ascii_digit()) {
                    k += 1;
                }
                j = k;
            }
        }
        let v: f64 = std::str::from_utf8(&self.b[s..j]).ok()?.parse().ok()?;
        self.i = j;
        Some(v.clamp(-MAX_COORD, MAX_COORD))
    }

    /// An arc flag (a single `0` / `1`, which SVG lets touch the next number).
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        match self.b.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Some(false)
            }
            Some(b'1') => {
                self.i += 1;
                Some(true)
            }
            _ => None,
        }
    }
}

/// The cubic Beziers of an SVG endpoint arc (`A rx ry rot large sweep x y`) from `p0`.
fn arc_cubics(
    p0: Pt,
    rx: f64,
    ry: f64,
    rot_deg: f64,
    large: bool,
    sweep: bool,
    p1: Pt,
) -> Vec<PathCmd> {
    if (p0.x - p1.x).abs() < 1e-12 && (p0.y - p1.y).abs() < 1e-12 {
        return Vec::new();
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-12 || ry < 1e-12 {
        return vec![PathCmd::LineTo(p1)];
    }
    let phi = rot_deg.to_radians();
    let (sp, cp) = phi.sin_cos();
    let dx = (p0.x - p1.x) / 2.0;
    let dy = (p0.y - p1.y) / 2.0;
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lam = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lam > 1.0 {
        let s = lam.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut co = if den.abs() < 1e-300 {
        0.0
    } else {
        (num / den).max(0.0).sqrt()
    };
    if large == sweep {
        co = -co;
    }
    let cxp = co * rx * y1 / ry;
    let cyp = -co * ry * x1 / rx;
    let cx = cp * cxp - sp * cyp + (p0.x + p1.x) / 2.0;
    let cy = sp * cxp + cp * cyp + (p0.y + p1.y) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let d = ux * vx + uy * vy;
        let l = ux.hypot(uy) * vx.hypot(vy);
        let mut a = (d / l.max(1e-300)).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let th1 = ang(1.0, 0.0, (x1 - cxp) / rx, (y1 - cyp) / ry);
    let mut dth = ang(
        (x1 - cxp) / rx,
        (y1 - cyp) / ry,
        (-x1 - cxp) / rx,
        (-y1 - cyp) / ry,
    );
    if !sweep && dth > 0.0 {
        dth -= std::f64::consts::TAU;
    } else if sweep && dth < 0.0 {
        dth += std::f64::consts::TAU;
    }
    let n = ((dth.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize).clamp(1, 4);
    let step = dth / n as f64;
    let t = 4.0 / 3.0 * (step / 4.0).tan();
    let mut out = Vec::with_capacity(n);
    let pt = |a: f64| -> (f64, f64) {
        let (s, c) = a.sin_cos();
        (rx * c, ry * s)
    };
    let to_user = |x: f64, y: f64| Pt::new(cp * x - sp * y + cx, sp * x + cp * y + cy);
    let mut a = th1;
    for k in 0..n {
        let b = a + step;
        let (x0, y0) = pt(a);
        let (x3, y3) = pt(b);
        let (s0, c0) = a.sin_cos();
        let (s3, c3) = b.sin_cos();
        let c1 = to_user(x0 - t * rx * s0, y0 + t * ry * c0);
        let c2 = to_user(x3 + t * rx * s3, y3 - t * ry * c3);
        let end = if k + 1 == n { p1 } else { to_user(x3, y3) };
        out.push(PathCmd::CubicTo(c1, c2, end));
        a = b;
    }
    out
}

/// Parses SVG path data into absolute commands. Returns the commands and whether the command
/// budget [`MAX_PATH_CMDS`] cut the path; `None` for data that is over [`MAX_PATH_BYTES`], has no
/// drawing command, or does not start with a move.
pub(super) fn parse_svg_path(d: &str) -> Option<(Vec<PathCmd>, bool)> {
    if d.len() > MAX_PATH_BYTES {
        return None;
    }
    let mut s = Scan {
        b: d.as_bytes(),
        i: 0,
    };
    let mut out: Vec<PathCmd> = Vec::new();
    let mut cur = Pt::new(0.0, 0.0);
    let mut start = cur;
    let mut last_c: Option<Pt> = None; // the last cubic control point (for S)
    let mut last_q: Option<Pt> = None; // the last quad control point (for T)
    let mut cmd = 0u8;
    let mut truncated = false;
    let mut drew = false;
    while let Some(c) = s.peek() {
        if c.is_ascii_alphabetic() {
            cmd = c;
            s.i += 1;
            if cmd == b'z' || cmd == b'Z' {
                out.push(PathCmd::Close);
                cur = start;
                last_c = None;
                last_q = None;
                continue;
            }
        } else if cmd == 0 {
            return None;
        } else if cmd == b'z' || cmd == b'Z' {
            // numbers after a close: SVG says error; stop
            break;
        }
        if out.len() >= MAX_PATH_CMDS {
            truncated = true;
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        let (ox, oy) = if rel { (cur.x, cur.y) } else { (0.0, 0.0) };
        let mut ok = true;
        match up {
            b'M' => {
                match (s.num(), s.num()) {
                    (Some(x), Some(y)) => {
                        cur = Pt::new(x + ox, y + oy);
                        start = cur;
                        out.push(PathCmd::MoveTo(cur));
                        // further pairs are implicit line-tos
                        cmd = if rel { b'l' } else { b'L' };
                    }
                    _ => ok = false,
                }
                last_c = None;
                last_q = None;
            }
            b'L' => match (s.num(), s.num()) {
                (Some(x), Some(y)) => {
                    cur = Pt::new(x + ox, y + oy);
                    out.push(PathCmd::LineTo(cur));
                    drew = true;
                    last_c = None;
                    last_q = None;
                }
                _ => ok = false,
            },
            b'H' => match s.num() {
                Some(x) => {
                    cur = Pt::new(x + ox, cur.y);
                    out.push(PathCmd::LineTo(cur));
                    drew = true;
                    last_c = None;
                    last_q = None;
                }
                None => ok = false,
            },
            b'V' => match s.num() {
                Some(y) => {
                    cur = Pt::new(cur.x, y + oy);
                    out.push(PathCmd::LineTo(cur));
                    drew = true;
                    last_c = None;
                    last_q = None;
                }
                None => ok = false,
            },
            b'C' => {
                let v: Vec<Option<f64>> = (0..6).map(|_| s.num()).collect();
                if v.iter().all(Option::is_some) {
                    let g = |i: usize| v[i].unwrap_or(0.0);
                    let c1 = Pt::new(g(0) + ox, g(1) + oy);
                    let c2 = Pt::new(g(2) + ox, g(3) + oy);
                    let e = Pt::new(g(4) + ox, g(5) + oy);
                    out.push(PathCmd::CubicTo(c1, c2, e));
                    drew = true;
                    cur = e;
                    last_c = Some(c2);
                    last_q = None;
                } else {
                    ok = false;
                }
            }
            b'S' => {
                let v: Vec<Option<f64>> = (0..4).map(|_| s.num()).collect();
                if v.iter().all(Option::is_some) {
                    let g = |i: usize| v[i].unwrap_or(0.0);
                    let c1 = match last_c {
                        Some(p) => Pt::new(2.0 * cur.x - p.x, 2.0 * cur.y - p.y),
                        None => cur,
                    };
                    let c2 = Pt::new(g(0) + ox, g(1) + oy);
                    let e = Pt::new(g(2) + ox, g(3) + oy);
                    out.push(PathCmd::CubicTo(c1, c2, e));
                    drew = true;
                    cur = e;
                    last_c = Some(c2);
                    last_q = None;
                } else {
                    ok = false;
                }
            }
            b'Q' => {
                let v: Vec<Option<f64>> = (0..4).map(|_| s.num()).collect();
                if v.iter().all(Option::is_some) {
                    let g = |i: usize| v[i].unwrap_or(0.0);
                    let c1 = Pt::new(g(0) + ox, g(1) + oy);
                    let e = Pt::new(g(2) + ox, g(3) + oy);
                    out.push(PathCmd::QuadTo(c1, e));
                    drew = true;
                    cur = e;
                    last_q = Some(c1);
                    last_c = None;
                } else {
                    ok = false;
                }
            }
            b'T' => match (s.num(), s.num()) {
                (Some(x), Some(y)) => {
                    let c1 = match last_q {
                        Some(p) => Pt::new(2.0 * cur.x - p.x, 2.0 * cur.y - p.y),
                        None => cur,
                    };
                    let e = Pt::new(x + ox, y + oy);
                    out.push(PathCmd::QuadTo(c1, e));
                    drew = true;
                    cur = e;
                    last_q = Some(c1);
                    last_c = None;
                }
                _ => ok = false,
            },
            b'A' => {
                let rx = s.num();
                let ry = s.num();
                let rot = s.num();
                let large = s.flag();
                let sweep = s.flag();
                let x = s.num();
                let y = s.num();
                match (rx, ry, rot, large, sweep, x, y) {
                    (Some(rx), Some(ry), Some(rot), Some(l), Some(sw), Some(x), Some(y)) => {
                        let e = Pt::new(x + ox, y + oy);
                        let cubics = arc_cubics(cur, rx, ry, rot, l, sw, e);
                        if cubics.is_empty() {
                            // coincident points: SVG omits the arc
                        } else {
                            out.extend(cubics);
                            drew = true;
                        }
                        cur = e;
                        last_c = None;
                        last_q = None;
                    }
                    _ => ok = false,
                }
            }
            _ => ok = false,
        }
        if !ok {
            // Unknown command or a short argument list: the path ends here (as a browser's
            // error handling does: what was drawn so far stays).
            break;
        }
    }
    if !matches!(out.first(), Some(PathCmd::MoveTo(_))) || !drew {
        return None;
    }
    Some((out, truncated))
}

/// `draw:points` (`x,y x,y ...`, viewBox units) as a polyline or polygon. `None` for fewer than two
/// points or data over [`MAX_PATH_BYTES`] / [`MAX_PATH_CMDS`].
pub(super) fn parse_points(s: &str, close: bool) -> Option<Vec<PathCmd>> {
    if s.len() > MAX_PATH_BYTES {
        return None;
    }
    let mut sc = Scan {
        b: s.as_bytes(),
        i: 0,
    };
    let mut pts: Vec<Pt> = Vec::new();
    while let (Some(x), Some(y)) = (sc.num(), sc.num()) {
        pts.push(Pt::new(x, y));
        if pts.len() > MAX_PATH_CMDS {
            return None;
        }
    }
    if pts.len() < 2 {
        return None;
    }
    let mut out = Vec::with_capacity(pts.len() + 1);
    for (i, p) in pts.iter().enumerate() {
        out.push(if i == 0 {
            PathCmd::MoveTo(*p)
        } else {
            PathCmd::LineTo(*p)
        });
    }
    if close {
        out.push(PathCmd::Close);
    }
    Some(out)
}

/// `svg:viewBox="x y w h"`.
pub(super) fn view_box(s: &str) -> Option<(f64, f64, f64, f64)> {
    let v: Vec<f64> = s
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<f64>().ok().filter(|n| n.is_finite()))
        .collect::<Option<Vec<_>>>()?;
    if v.len() != 4 || !(v[2] > 0.0 && v[3] > 0.0) {
        return None;
    }
    Some((v[0], v[1], v[2], v[3]))
}

/// Moves a command list by `(-dx, -dy)` (to make a viewBox's origin the path space's origin).
pub(super) fn shift(cmds: &mut [PathCmd], dx: f64, dy: f64) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let m = |p: &mut Pt| {
        p.x -= dx;
        p.y -= dy;
    };
    for c in cmds {
        match c {
            PathCmd::MoveTo(a) | PathCmd::LineTo(a) => m(a),
            PathCmd::QuadTo(a, b) => {
                m(a);
                m(b);
            }
            PathCmd::CubicTo(a, b, c) => {
                m(a);
                m(b);
                m(c);
            }
            PathCmd::ArcTo { .. } | PathCmd::Close => {}
        }
    }
}
