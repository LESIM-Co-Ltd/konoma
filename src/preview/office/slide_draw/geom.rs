//! DrawingML geometry engine: the guide-formula evaluator, preset shapes and custom geometry.
//!
//! Format-neutral (the readers hand in names, numbers and `a:custGeom` nodes; they get
//! [`GeomPath`]s in box coordinates back) and defensive: nothing here panics, every number that
//! leaves is finite and within [`MAX_VALUE`], and every count is budgeted.
//!
//! # Guide formulas (ECMA-376 Part 1, 20.1.9.11 `gd`)
//!
//! A formula is `operator operand operand operand`; an operand is a guide name or an integer
//! literal. Angles are in 60000ths of a degree. A guide may refer to the built-in guides (`w`, `h`,
//! `wd2`, `ss`, `cd4`, ...), to adjust values and to **earlier** guides only (document order).
//!
//! | operator | result |
//! |---|---|
//! | `*/ x y z` | `x * y / z` |
//! | `+- x y z` | `x + y - z` |
//! | `+/ x y z` | `(x + y) / z` |
//! | `?: x y z` | `y` if `x > 0`, else `z` |
//! | `abs x` | `\|x\|` |
//! | `at2 x y` | `atan2(y, x)` as an angle (signed, -180..180 degrees: the preset formulas add a full turn themselves where they need 0..360) |
//! | `cat2 x y z` | `x * cos(atan2(z, y))` |
//! | `sat2 x y z` | `x * sin(atan2(z, y))` |
//! | `cos x y` / `sin x y` / `tan x y` | `x * cos(y)` / `x * sin(y)` / `x * tan(y)`, `y` an angle |
//! | `max x y` / `min x y` | the larger / smaller |
//! | `mod x y z` | `sqrt(x^2 + y^2 + z^2)` |
//! | `pin x y z` | `y` limited to `x ..= z` (`x` if `y < x`, else `z` if `y > z`, else `y`) |
//! | `sqrt x` | square root |
//! | `val x` | `x` |
//!
//! ## What happens on bad input (the specification is silent; PowerPoint does not report)
//!
//! * Division by zero (`*/`, `+/` with `z == 0`) gives `0`.
//! * `sqrt` of a negative number gives `0`.
//! * An unknown operator, an unknown name, an empty formula or a missing operand gives `0`.
//! * Every literal and every result is limited to `±`[`MAX_VALUE`] (a result of `NaN` is `0`), so
//!   `tan` near a right angle and products of huge numbers stay finite. What leaves the engine
//!   (points, radii, angles, the text rectangle, connection sites) is limited to
//!   `±`[`MAX_COORD`].
//! * A guide that refers to a *later* guide sees `0` (it is not defined yet).

use std::cell::Cell;
use std::collections::HashMap;

use super::model::{GeomPath, PathCmd, PathFill, Pt, Rect4};

/// Most guides (adjust values plus guides) one geometry may define; more and the geometry is
/// refused (`None`): half a guide list gives a wrong shape, not a smaller one. The largest preset
/// has about a hundred.
pub const MAX_GUIDES: usize = 4096;
/// Most paths one geometry may have; the rest are dropped and [`ShapeGeom::truncated`] is set.
pub const MAX_PATHS: usize = 256;
/// Most path commands in all the paths of one geometry together; the rest are dropped and
/// [`ShapeGeom::truncated`] is set. (A freeform scribble has thousands of points, not millions.)
pub const MAX_COMMANDS: usize = 100_000;
/// The magnitude every literal and every formula result is limited to. Guides are intermediate
/// values (a preset squares and multiplies coordinates: `q5 = rO * rO * dO * dO` is about
/// `1e28` for an ordinary slide), so the limit is far above any coordinate; it only has to keep
/// a chain of operations from reaching infinity.
pub const MAX_VALUE: f64 = 1e100;
/// The magnitude of everything that leaves the engine: coordinates, radii, angles (degrees) and
/// the box size. Equal to the limit the path resolver applies to coordinates.
pub const MAX_COORD: f64 = 1e12;

/// A pair of operands (an `x` and a `y`: guide names or literals).
pub type Xy = (String, String);

/// A connection site: where a connector may attach, and the direction it leaves in.
pub type Connection = (Pt, f64);

/// One path command with its operands still unevaluated.
#[derive(Debug, Clone, PartialEq)]
pub enum CmdSpec {
    Move(Xy),
    Line(Xy),
    Quad(Xy, Xy),
    Cubic(Xy, Xy, Xy),
    /// `a:arcTo`: radii and angles (60000ths of a degree).
    Arc {
        wr: String,
        hr: String,
        st_ang: String,
        sw_ang: String,
    },
    Close,
}

/// One `a:path`.
#[derive(Debug, Clone, PartialEq)]
pub struct PathSpec {
    /// The path's own coordinate space (`0` = the box on that axis).
    pub w: f64,
    pub h: f64,
    pub fill: PathFill,
    pub stroke: bool,
    /// `extrusionOk` (3-D extrusion; the flat renderer does not use it).
    pub extrusion_ok: bool,
    pub cmds: Vec<CmdSpec>,
}

impl Default for PathSpec {
    fn default() -> Self {
        PathSpec {
            w: 0.0,
            h: 0.0,
            fill: PathFill::Norm,
            stroke: true,
            extrusion_ok: true,
            cmds: Vec::new(),
        }
    }
}

/// A connection site with unevaluated operands.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnSpec {
    /// Angle (60000ths of a degree).
    pub ang: String,
    pub pos: Xy,
}

/// A geometry as a file describes it: `a:custGeom`, or one entry of the preset definitions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CustomGeomSpec {
    /// `avLst`: adjust values `(name, formula)` -- formulas, normally `val N`.
    pub adjusts: Vec<(String, String)>,
    /// `gdLst`: guides `(name, formula)`, in evaluation order.
    pub guides: Vec<(String, String)>,
    /// `rect`: the text rectangle as `[l, t, r, b]` operands.
    pub text_rect: Option<[String; 4]>,
    pub connections: Vec<ConnSpec>,
    pub paths: Vec<PathSpec>,
    /// The reader dropped paths or commands over the budgets.
    pub truncated: bool,
}

/// An evaluated geometry, in the coordinates of its box (EMU, origin top left).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShapeGeom {
    pub paths: Vec<GeomPath>,
    /// The text rectangle `(l, t, r, b)`; `None` = the whole box.
    pub text_rect: Option<Rect4>,
    /// Connection sites: position and direction in degrees.
    pub connections: Vec<Connection>,
    /// Paths or commands were dropped over [`MAX_PATHS`] / [`MAX_COMMANDS`].
    pub truncated: bool,
}

// ---------------------------------------------------------------------------------------------
// formula evaluation
// ---------------------------------------------------------------------------------------------

/// Limits a value that leaves the engine to `±`[`MAX_COORD`].
fn out(v: f64) -> f64 {
    v.clamp(-MAX_COORD, MAX_COORD)
}

fn clamp(v: f64) -> f64 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-MAX_VALUE, MAX_VALUE)
    }
}

/// A literal operand: an integer (the specification) or, leniently, a plain decimal number.
fn parse_literal(tok: &str) -> Option<f64> {
    let first = *tok.as_bytes().first()?;
    if !(first.is_ascii_digit() || matches!(first, b'-' | b'+' | b'.')) {
        return None;
    }
    if let Ok(i) = tok.parse::<i64>() {
        return Some(i as f64);
    }
    if tok
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+'))
    {
        return tok.parse::<f64>().ok().filter(|v| v.is_finite());
    }
    None
}

/// Guide values by name.
pub(super) struct Scope<'a> {
    vals: HashMap<&'a str, f64>,
    /// Operands that were neither a known name nor a number.
    unknown: Cell<usize>,
}

/// The built-in guides for a box of `w` x `h`.
fn builtins(w: f64, h: f64) -> Vec<(&'static str, f64)> {
    let (ss, ls) = (w.min(h), w.max(h));
    let mut v = vec![
        ("l", 0.0),
        ("t", 0.0),
        ("r", w),
        ("b", h),
        ("w", w),
        ("h", h),
        ("hc", w / 2.0),
        ("vc", h / 2.0),
        ("ss", ss),
        ("ls", ls),
        ("cd2", 10_800_000.0),
        ("cd4", 5_400_000.0),
        ("cd8", 2_700_000.0),
        ("3cd4", 16_200_000.0),
        ("3cd8", 8_100_000.0),
        ("5cd8", 13_500_000.0),
        ("7cd8", 18_900_000.0),
    ];
    let fractions: [(&'static str, &'static str, &'static str, f64); 9] = [
        ("wd2", "hd2", "", 2.0),
        ("wd3", "hd3", "", 3.0),
        ("wd4", "hd4", "", 4.0),
        ("wd5", "hd5", "", 5.0),
        ("wd6", "hd6", "", 6.0),
        ("wd8", "hd8", "ssd8", 8.0),
        ("wd10", "hd10", "", 10.0),
        ("wd12", "hd12", "", 12.0),
        ("wd32", "hd32", "ssd32", 32.0),
    ];
    for (wn, hn, sn, d) in fractions {
        v.push((wn, w / d));
        v.push((hn, h / d));
        if !sn.is_empty() {
            v.push((sn, ss / d));
        }
    }
    for (n, d) in [("ssd2", 2.0), ("ssd4", 4.0), ("ssd6", 6.0), ("ssd16", 16.0)] {
        v.push((n, ss / d));
    }
    v
}

impl<'a> Scope<'a> {
    pub(super) fn new(w: f64, h: f64) -> Scope<'a> {
        let mut vals = HashMap::new();
        for (n, v) in builtins(w, h) {
            vals.insert(n, v);
        }
        Scope {
            vals,
            unknown: Cell::new(0),
        }
    }

    pub(super) fn define(&mut self, name: &'a str, v: f64) {
        self.vals.insert(name, clamp(v));
    }

    pub(super) fn unknown(&self) -> usize {
        self.unknown.get()
    }

    /// The value of an operand: a defined name, a literal, or `0` (counted as unknown).
    pub(super) fn operand(&self, tok: &str) -> f64 {
        if let Some(v) = self.vals.get(tok) {
            return *v;
        }
        if let Some(v) = parse_literal(tok) {
            return clamp(v);
        }
        self.unknown.set(self.unknown.get() + 1);
        0.0
    }

    /// Evaluates one formula (`operator operand ...`).
    pub(super) fn eval(&self, fmla: &str) -> f64 {
        let mut it = fmla.split_ascii_whitespace();
        let Some(op) = it.next() else {
            return 0.0;
        };
        let mut arg = || it.next().map_or(0.0, |t| self.operand(t));
        let deg = |a: f64| (a / 60_000.0).to_radians();
        let r = match op {
            "val" => arg(),
            "*/" => {
                let (x, y, z) = (arg(), arg(), arg());
                if z == 0.0 {
                    0.0
                } else {
                    x * y / z
                }
            }
            "+-" => {
                let (x, y, z) = (arg(), arg(), arg());
                x + y - z
            }
            "+/" => {
                let (x, y, z) = (arg(), arg(), arg());
                if z == 0.0 {
                    0.0
                } else {
                    (x + y) / z
                }
            }
            "?:" => {
                let (x, y, z) = (arg(), arg(), arg());
                if x > 0.0 {
                    y
                } else {
                    z
                }
            }
            "abs" => arg().abs(),
            "at2" => {
                let (x, y) = (arg(), arg());
                y.atan2(x).to_degrees() * 60_000.0
            }
            "cat2" => {
                let (x, y, z) = (arg(), arg(), arg());
                x * z.atan2(y).cos()
            }
            "sat2" => {
                let (x, y, z) = (arg(), arg(), arg());
                x * z.atan2(y).sin()
            }
            "cos" => {
                let (x, y) = (arg(), arg());
                x * deg(y).cos()
            }
            "sin" => {
                let (x, y) = (arg(), arg());
                x * deg(y).sin()
            }
            "tan" => {
                let (x, y) = (arg(), arg());
                x * deg(y).tan()
            }
            "max" => {
                let (x, y) = (arg(), arg());
                x.max(y)
            }
            "min" => {
                let (x, y) = (arg(), arg());
                x.min(y)
            }
            "mod" => {
                let (x, y, z) = (arg(), arg(), arg());
                (x * x + y * y + z * z).sqrt()
            }
            "pin" => {
                let (x, y, z) = (arg(), arg(), arg());
                if y < x {
                    x
                } else if y > z {
                    z
                } else {
                    y
                }
            }
            "sqrt" => {
                let x = arg();
                if x < 0.0 {
                    0.0
                } else {
                    x.sqrt()
                }
            }
            _ => {
                self.unknown.set(self.unknown.get() + 1);
                0.0
            }
        };
        clamp(r)
    }
}

/// Evaluates one formula against the built-in guides of a `w` x `h` box and the extra `guides`
/// (name, value). For tests; the geometry code uses [`Scope`] directly.
#[cfg(test)]
pub fn eval_formula(fmla: &str, w: f64, h: f64, guides: &[(&str, f64)]) -> f64 {
    let mut s = Scope::new(sane_size(w), sane_size(h));
    for (n, v) in guides {
        s.define(n, *v);
    }
    s.eval(fmla)
}

fn sane_size(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, MAX_COORD)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------------------------
// evaluating a geometry
// ---------------------------------------------------------------------------------------------

impl CustomGeomSpec {
    /// Evaluates the geometry for a `w` x `h` box (EMU). `overrides` replace adjust values by name
    /// (a non-finite override is ignored). `None` when there are more than [`MAX_GUIDES`] guides.
    pub fn eval(&self, w: f64, h: f64, overrides: &[(String, f64)]) -> Option<ShapeGeom> {
        self.eval_counting(w, h, overrides).map(|(g, _)| g)
    }

    /// [`eval`](Self::eval), also returning how many operands were unknown names / operators
    /// (each read as `0`). A bundled preset must have none.
    pub(super) fn eval_counting(
        &self,
        w: f64,
        h: f64,
        overrides: &[(String, f64)],
    ) -> Option<(ShapeGeom, usize)> {
        if self.adjusts.len() + self.guides.len() > MAX_GUIDES {
            return None;
        }
        let (w, h) = (sane_size(w), sane_size(h));
        let mut sc = Scope::new(w, h);
        for (name, fmla) in &self.adjusts {
            let v = match overrides.iter().find(|(n, v)| n == name && v.is_finite()) {
                Some((_, v)) => *v,
                None => sc.eval(fmla),
            };
            sc.define(name, v);
        }
        for (name, fmla) in &self.guides {
            let v = sc.eval(fmla);
            sc.define(name, v);
        }
        let pt = |xy: &Xy| Pt::new(out(sc.operand(&xy.0)), out(sc.operand(&xy.1)));
        let text_rect = self.text_rect.as_ref().map(|r| {
            (
                out(sc.operand(&r[0])),
                out(sc.operand(&r[1])),
                out(sc.operand(&r[2])),
                out(sc.operand(&r[3])),
            )
        });
        let connections = self
            .connections
            .iter()
            .take(MAX_PATHS)
            .map(|c| (pt(&c.pos), out(sc.operand(&c.ang) / 60_000.0)))
            .collect();
        let mut truncated = self.truncated;
        let mut budget = MAX_COMMANDS;
        let mut paths = Vec::new();
        for p in &self.paths {
            if paths.len() >= MAX_PATHS {
                truncated = true;
                break;
            }
            let mut cmds = Vec::with_capacity(p.cmds.len().min(budget));
            for c in &p.cmds {
                if budget == 0 {
                    truncated = true;
                    break;
                }
                budget -= 1;
                cmds.push(match c {
                    CmdSpec::Move(a) => PathCmd::MoveTo(pt(a)),
                    CmdSpec::Line(a) => PathCmd::LineTo(pt(a)),
                    CmdSpec::Quad(a, b) => PathCmd::QuadTo(pt(a), pt(b)),
                    CmdSpec::Cubic(a, b, c) => PathCmd::CubicTo(pt(a), pt(b), pt(c)),
                    CmdSpec::Arc {
                        wr,
                        hr,
                        st_ang,
                        sw_ang,
                    } => PathCmd::ArcTo {
                        wr: out(sc.operand(wr)),
                        hr: out(sc.operand(hr)),
                        st_deg: out(sc.operand(st_ang) / 60_000.0),
                        sw_deg: out(sc.operand(sw_ang) / 60_000.0),
                    },
                    CmdSpec::Close => PathCmd::Close,
                });
            }
            paths.push(GeomPath {
                w: sane_size(p.w),
                h: sane_size(p.h),
                fill_mode: p.fill,
                stroke: p.stroke,
                cmds,
            });
        }
        let geom = ShapeGeom {
            paths,
            text_rect,
            connections,
            truncated,
        };
        Some((geom, sc.unknown()))
    }
}

// ---------------------------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------------------------

/// A preset shape (`a:prstGeom prst="..."`) for a `w` x `h` box (EMU). `adj` overrides the
/// preset's adjust values by name (`adj`, `adj1`, ...; the raw numbers of `a:gd fmla="val N"`).
/// `None` for a name that is not one of the presets.
pub fn preset(name: &str, adj: &[(String, f64)], w: f64, h: f64) -> Option<ShapeGeom> {
    super::geom_xml::preset_spec(name)?.eval(w, h, adj)
}

/// A custom geometry (`a:custGeom`) for a `w` x `h` box (EMU). `None` over [`MAX_GUIDES`].
pub fn custom(spec: &CustomGeomSpec, w: f64, h: f64) -> Option<ShapeGeom> {
    spec.eval(w, h, &[])
}

// The readers (a later task) are the only callers; until they land only the tests use it.
#[allow(unused_imports)]
pub use super::geom_xml::custom_from_xml;

/// The names of the adjust values of preset `name` (`adj`, `adj1`, ...), in the order the
/// definition lists them; `None` for a name that is not a preset. The OpenDocument reader maps the
/// positional `draw:modifiers` of an `ooxml-*` shape onto these.
pub fn preset_adjust_names(name: &str) -> Option<Vec<String>> {
    Some(
        super::geom_xml::preset_spec(name)?
            .adjusts
            .iter()
            .map(|(n, _)| n.clone())
            .collect(),
    )
}

/// The names of all the presets, sorted (the tests walk them).
#[cfg(test)]
pub fn preset_names() -> Vec<&'static str> {
    super::geom_xml::preset_names()
}

#[cfg(test)]
#[path = "geom_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "geom_dump_tests.rs"]
mod dump_tests;
