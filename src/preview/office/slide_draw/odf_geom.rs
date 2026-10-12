//! OpenDocument custom-shape geometry (`draw:enhanced-geometry`) to drawing paths.
//!
//! ODF draws a custom shape with a path in the coordinate space of `svg:viewBox`
//! (`draw:enhanced-path`, ODF 1.3 section 19.140) whose numbers may be literals, modifiers (`$n`,
//! from `draw:modifiers`) or named equations (`?name`, from `draw:equation`). This module
//! evaluates the equations and turns the path into [`GeomPath`]s whose `w` / `h` are the viewBox
//! size, so the SVG writer scales them to the shape box exactly as it does for DrawingML custom
//! geometry. It knows nothing about the rest of the shape (position, style, text): see
//! [`enhanced_geometry`].
//!
//! # Equations (`draw:formula`)
//!
//! Numbers, `$n`, `?name`, the constants `pi left top right bottom xstretch ystretch hasstroke
//! hasfill width height logwidth logheight`, the operators `+ - * /` (usual precedence, unary
//! `-` / `+`, parentheses) and the functions `abs sqrt sin cos tan atan atan2 min max if`.
//!
//! * **Angles are radians.** `sin`, `cos`, `tan` take radians and `atan`, `atan2` return radians;
//!   the shapes LibreOffice writes convert explicitly (`10800*cos($0 *(pi/180))`,
//!   `atan2(?f1 ,?f0 )/(pi/180)`). Checked on the OpenDocument files of LibreOffice's own test
//!   suite (`block-arc`, `round-callout`, `sun`, `star8`).
//! * `atan2(y, x)` has the C argument order (the first argument is the ordinate).
//! * `if(a, b, c)` is `b` when `a > 0`, otherwise `c`. All three are evaluated.
//! * **Division by zero, a NaN or an infinity is `0`**, and every value is clamped to
//!   +-[`MAX_ABS`]. An unknown name, an unknown function, a wrong argument count or a syntax error
//!   makes the whole equation `0`.
//! * `?name` may refer forward or backward. Each equation is evaluated at most once (memoised);
//!   a reference to an equation that is being evaluated (a cycle) is `0`, and a chain of
//!   references deeper than [`MAX_REF_DEPTH`] ends in `0` (not memoised).
//! * `left` / `top` / `right` / `bottom` are the edges of the viewBox, `width` / `height` its
//!   size, `logwidth` / `logheight` the shape size in 1/100 mm (360 EMU), `xstretch` / `ystretch`
//!   the `draw:path-stretchpoint-x/y` (0 when absent), `hasstroke` / `hasfill` are always `1`
//!   (the caller does not tell). The stretch points are used only through these constants; the
//!   path itself is scaled uniformly with the box.
//!
//! # Paths (`draw:enhanced-path`)
//!
//! | cmd | parameters | meaning |
//! |-----|------------|---------|
//! | `M` | `(x y)+` | move to (every pair moves) |
//! | `L` | `(x y)+` | line to |
//! | `C` | `(x1 y1 x2 y2 x y)+` | cubic Bezier |
//! | `Q` | `(x1 y1 x y)+` | quadratic Bezier |
//! | `Z` | | close the sub-polygon |
//! | `N` | | end the sub-path: a new [`GeomPath`] starts |
//! | `F` / `S` | | the sub-path is not filled / not stroked |
//! | `X` / `Y` | `(x y)+` | elliptical quadrant from the current point to `(x, y)`; `X` leaves along the x axis, `Y` along the y axis |
//! | `T` / `U` | `(cx cy rx ry a0 a1)+` | ellipse arc by angles in degrees, `T` is joined to the current point with a line, `U` starts a new polygon |
//! | `A` / `B` | `(x1 y1 x2 y2 x3 y3 x4 y4)+` | arc of the ellipse in the box `x1 y1 x2 y2`, **counter-clockwise** from the ray through `(x3, y3)` to the ray through `(x4, y4)`; `A` is joined to the current point |
//! | `W` / `V` | same | the same, **clockwise** |
//! | `G` | `(wR hR stAng swAng)+` | LibreOffice extension, only in `drawooo:enhanced-path`: DrawingML `arcTo` (degrees) |
//!
//! * A command with more parameters than it needs repeats (`L 0 0 10 10 20 0` is three line-tos);
//!   trailing parameters that do not fill a whole repeat are dropped.
//! * A drawing command with no current point (the first one in a sub-path) starts at its first
//!   point.
//! * Arcs become cubic Beziers (at most four per arc, each under 90 degrees): the ODF arc is given
//!   by a box and two points, not from the current point as DrawingML's `arcTo` is, so [`PathCmd::ArcTo`]
//!   would not be exact. Only `G` is already an `arcTo` and is emitted as [`PathCmd::ArcTo`].
//! * `A` `B` `W` `V` use the **parametric** angle of the start and end points (the point where the
//!   ray from the centre through the given point meets the ellipse). Two coincident points give a
//!   whole ellipse.
//! * `T` / `U` angles are degrees, the parametric angle, increasing clockwise on screen, from the
//!   start to the end; an end before the start is reached by going on, and 360 degrees or more is a
//!   whole ellipse. LibreOffice also writes 16.16 fixed point (`0 23592960` = 0 to 360): values
//!   beyond 3600 are divided by 65536. Only whole ellipses (`0 360`) appear in the files checked, so
//!   the direction of a partial `T` / `U` is **not verified**.
//! * `drawooo:enhanced-path`, when present, is the exact path of a shape LibreOffice imported from
//!   OOXML (`draw:enhanced-path` is then a stub); it is preferred.
//!
//! `svg:viewBox`: the origin is subtracted (output coordinates start at 0). Without a viewBox the
//! space is 21600 x 21600 (LibreOffice's default). A viewBox of size 0
//! (`0 0 0 0`, as LibreOffice writes for its `ooxml-*` shapes whose equations use `logwidth`)
//! means the shape size in 1/100 mm.
//!
//! `draw:mirror-horizontal` / `draw:mirror-vertical` flip the paths and the text area about the
//! middle of the viewBox.
//!
//! # LibreOffice writes the path for most of its shapes, but not for the `ooxml-*` ones
//!
//! On the 56 `.odp` files of the LibreOffice test corpus plus `draw-shapes.odp` (290
//! `draw:enhanced-geometry`), 105 carry a path; the other 185 are `ooxml-*` preset shapes that name
//! the preset only (`draw:type="ooxml-rightArrow"`): the caller has to draw those from the preset
//! name (the PowerPoint preset table). [`enhanced_geometry`] returns `None` for them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

use super::model::{GeomPath, PathCmd, PathFill, Pt, Rect4};
use crate::preview::office::docx_xml::{local_of, Node};

/// Most `draw:equation`s a shape may have (real presets have under 200). More: no geometry.
pub const MAX_EQUATIONS: usize = 4096;
/// Longest formula (bytes) that is evaluated; a longer one is `0`.
pub const MAX_FORMULA_BYTES: usize = 2048;
/// Deepest parenthesis / function-call nesting inside one formula; deeper is a syntax error (`0`).
pub const MAX_EXPR_DEPTH: usize = 48;
/// Longest chain of `?name` references followed; the end of a longer chain is `0`.
pub const MAX_REF_DEPTH: usize = 256;
/// Most bytes of an `enhanced-path`. More: no geometry.
pub const MAX_PATH_BYTES: usize = 1 << 20;
/// Longest single path parameter (bytes). Longer: no geometry.
const MAX_TOKEN_BYTES: usize = 128;
/// Most commands plus parameters in one path. More: no geometry.
pub const MAX_PATH_TOKENS: usize = 200_000;
/// Most [`PathCmd`]s produced (all sub-paths together). More: no geometry.
pub const MAX_COMMANDS: usize = 100_000;
/// Most sub-paths (`N`). More: no geometry.
pub const MAX_SUBPATHS: usize = 4096;
/// Most `draw:modifiers` read.
pub const MAX_MODIFIERS: usize = 1024;
/// Every value is clamped to +-this.
pub const MAX_ABS: f64 = 1e12;
/// The side of the viewBox when a shape has none (LibreOffice's own default; its `mso-spt100`
/// pie is written without one).
const DEFAULT_VIEW_BOX: f64 = 21600.0;
/// EMU per 1/100 mm.
const EMU_PER_HMM: f64 = 360.0;
/// The Bezier control distance of a quarter ellipse.
const KAPPA: f64 = 0.552_284_749_830_793_4;

fn sane(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(-MAX_ABS, MAX_ABS)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------------------------
// Equations
// ---------------------------------------------------------------------------------------------

struct Consts {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    xstretch: f64,
    ystretch: f64,
    width: f64,
    height: f64,
    logwidth: f64,
    logheight: f64,
}

#[derive(Clone, Copy)]
enum Memo {
    Todo,
    Busy,
    Done(f64),
}

/// The equations, modifiers and constants of one shape.
struct Env<'a> {
    names: HashMap<&'a str, usize>,
    formulas: Vec<&'a str>,
    memo: RefCell<Vec<Memo>>,
    mods: Vec<f64>,
    k: Consts,
}

impl<'a> Env<'a> {
    fn named(&self, name: &str, depth: usize) -> f64 {
        match self.names.get(name) {
            Some(&i) => self.equation(i, depth),
            None => 0.0,
        }
    }

    fn equation(&self, i: usize, depth: usize) -> f64 {
        match self.memo.borrow()[i] {
            Memo::Done(v) => return v,
            Memo::Busy => return 0.0,
            Memo::Todo => {}
        }
        if depth >= MAX_REF_DEPTH {
            return 0.0;
        }
        self.memo.borrow_mut()[i] = Memo::Busy;
        let v = self.eval(self.formulas[i], depth + 1);
        self.memo.borrow_mut()[i] = Memo::Done(v);
        v
    }

    /// Evaluates one formula (or one path parameter). Any error is `0`.
    fn eval(&self, s: &str, depth: usize) -> f64 {
        if s.len() > MAX_FORMULA_BYTES {
            return 0.0;
        }
        let mut p = Parser {
            s: s.as_bytes(),
            i: 0,
            env: self,
            depth,
            nest: 0,
        };
        let v = p.expr().and_then(|v| {
            p.ws();
            if p.i == p.s.len() {
                Ok(v)
            } else {
                Err(())
            }
        });
        v.map_or(0.0, sane)
    }
}

struct Parser<'e, 'a> {
    s: &'e [u8],
    i: usize,
    env: &'e Env<'a>,
    /// Depth of `?name` references being followed.
    depth: usize,
    /// Current parenthesis nesting.
    nest: usize,
}

type Res = Result<f64, ()>;

impl Parser<'_, '_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }

    fn expr(&mut self) -> Res {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.i += 1;
                    v = sane(v + self.term()?);
                }
                Some(b'-') => {
                    self.i += 1;
                    v = sane(v - self.term()?);
                }
                _ => return Ok(v),
            }
        }
    }

    fn term(&mut self) -> Res {
        let mut v = self.unary()?;
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.i += 1;
                    v = sane(v * self.unary()?);
                }
                Some(b'/') => {
                    self.i += 1;
                    let r = self.unary()?;
                    v = if r == 0.0 { 0.0 } else { sane(v / r) };
                }
                _ => return Ok(v),
            }
        }
    }

    fn unary(&mut self) -> Res {
        let mut neg = false;
        loop {
            match self.peek() {
                Some(b'-') => {
                    neg = !neg;
                    self.i += 1;
                }
                Some(b'+') => self.i += 1,
                _ => break,
            }
        }
        let v = self.primary()?;
        Ok(if neg { -v } else { v })
    }

    fn word(&mut self) -> &str {
        let st = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[st..self.i]).unwrap_or("")
    }

    fn primary(&mut self) -> Res {
        let c = self.peek().ok_or(())?;
        match c {
            b'(' => {
                self.i += 1;
                let v = self.nested(|p| p.expr())?;
                if self.peek() != Some(b')') {
                    return Err(());
                }
                self.i += 1;
                Ok(v)
            }
            b'0'..=b'9' | b'.' => self.number(),
            b'?' => {
                self.i += 1;
                let name = self.word().to_string();
                if name.is_empty() {
                    return Err(());
                }
                Ok(self.env.named(&name, self.depth))
            }
            b'$' => {
                self.i += 1;
                let st = self.i;
                while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                }
                let n: usize = std::str::from_utf8(&self.s[st..self.i])
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .ok_or(())?;
                Ok(self.env.mods.get(n).copied().unwrap_or(0.0))
            }
            c if c.is_ascii_alphabetic() => {
                let name = self.word().to_ascii_lowercase();
                if self.peek() == Some(b'(') {
                    self.i += 1;
                    let args = self.nested(|p| p.args())?;
                    call(&name, &args)
                } else {
                    constant(&self.env.k, &name)
                }
            }
            _ => Err(()),
        }
    }

    /// Runs `f` one nesting level deeper.
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, ()>) -> Result<T, ()> {
        if self.nest >= MAX_EXPR_DEPTH {
            return Err(());
        }
        self.nest += 1;
        let r = f(self);
        self.nest -= 1;
        r
    }

    /// The comma-separated arguments up to and including the closing parenthesis.
    fn args(&mut self) -> Result<Vec<f64>, ()> {
        let mut out = Vec::new();
        if self.peek() == Some(b')') {
            self.i += 1;
            return Ok(out);
        }
        loop {
            out.push(self.expr()?);
            if out.len() > 3 {
                return Err(());
            }
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b')') => {
                    self.i += 1;
                    return Ok(out);
                }
                _ => return Err(()),
            }
        }
    }

    fn number(&mut self) -> Res {
        let st = self.i;
        let digits = |p: &mut Self| {
            while p.s.get(p.i).is_some_and(u8::is_ascii_digit) {
                p.i += 1;
            }
        };
        digits(self);
        if self.s.get(self.i) == Some(&b'.') {
            self.i += 1;
            digits(self);
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            let mut j = self.i + 1;
            if matches!(self.s.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if self.s.get(j).is_some_and(u8::is_ascii_digit) {
                self.i = j;
                digits(self);
            }
        }
        std::str::from_utf8(&self.s[st..self.i])
            .ok()
            .and_then(|t| t.parse::<f64>().ok())
            .ok_or(())
    }
}

fn constant(k: &Consts, name: &str) -> Res {
    Ok(match name {
        "pi" => PI,
        "left" => k.left,
        "top" => k.top,
        "right" => k.right,
        "bottom" => k.bottom,
        "xstretch" => k.xstretch,
        "ystretch" => k.ystretch,
        "hasstroke" | "hasfill" => 1.0,
        "width" => k.width,
        "height" => k.height,
        "logwidth" => k.logwidth,
        "logheight" => k.logheight,
        _ => return Err(()),
    })
}

fn call(name: &str, a: &[f64]) -> Res {
    let v = match (name, a) {
        ("abs", [x]) => x.abs(),
        ("sqrt", [x]) => x.sqrt(),
        ("sin", [x]) => x.sin(),
        ("cos", [x]) => x.cos(),
        ("tan", [x]) => x.tan(),
        ("atan", [x]) => x.atan(),
        ("atan2", [y, x]) => y.atan2(*x),
        ("min", [x, y]) => x.min(*y),
        ("max", [x, y]) => x.max(*y),
        ("if", [c, t, f]) => {
            if *c > 0.0 {
                *t
            } else {
                *f
            }
        }
        _ => return Err(()),
    };
    Ok(sane(v))
}

// ---------------------------------------------------------------------------------------------
// Path
// ---------------------------------------------------------------------------------------------

enum Tok {
    Cmd(u8),
    Arg(f64),
}

/// Splits a path into commands and evaluated parameters. `None`: over a budget.
fn tokenize(env: &Env, s: &str) -> Option<Vec<Tok>> {
    if s.len() > MAX_PATH_BYTES {
        return None;
    }
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() || c == b',' {
            i += 1;
            continue;
        }
        if c.is_ascii_uppercase() {
            out.push(Tok::Cmd(c));
        } else {
            let st = i;
            if matches!(c, b'-' | b'+') {
                i += 1;
            }
            match b.get(i) {
                Some(b'?' | b'$') => {
                    i += 1;
                    while b
                        .get(i)
                        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
                    {
                        i += 1;
                    }
                }
                Some(b'0'..=b'9' | b'.') => {
                    while b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                        i += 1;
                    }
                    if matches!(b.get(i), Some(b'e' | b'E')) {
                        let mut j = i + 1;
                        if matches!(b.get(j), Some(b'+' | b'-')) {
                            j += 1;
                        }
                        if b.get(j).is_some_and(u8::is_ascii_digit) {
                            i = j;
                            while b.get(i).is_some_and(u8::is_ascii_digit) {
                                i += 1;
                            }
                        }
                    }
                }
                // not a parameter (a stray character): skipped
                _ => {
                    i = st + 1;
                    continue;
                }
            }
            if i - st > MAX_TOKEN_BYTES {
                return None;
            }
            out.push(Tok::Arg(env.eval(&s[st..i], 0)));
            if out.len() > MAX_PATH_TOKENS {
                return None;
            }
            continue;
        }
        i += 1;
        if out.len() > MAX_PATH_TOKENS {
            return None;
        }
    }
    Some(out)
}

struct Builder {
    vw: f64,
    vh: f64,
    paths: Vec<GeomPath>,
    cmds: Vec<PathCmd>,
    fill_none: bool,
    stroke: bool,
    cur: Option<Pt>,
    start: Pt,
    total: usize,
    over: bool,
}

impl Builder {
    fn push(&mut self, c: PathCmd) {
        if self.total >= MAX_COMMANDS {
            self.over = true;
            return;
        }
        self.total += 1;
        self.cmds.push(c);
    }

    fn finish_sub(&mut self) {
        if !self.cmds.is_empty() {
            if self.paths.len() >= MAX_SUBPATHS {
                self.over = true;
            } else {
                self.paths.push(GeomPath {
                    w: self.vw,
                    h: self.vh,
                    fill_mode: if self.fill_none {
                        PathFill::None
                    } else {
                        PathFill::Norm
                    },
                    stroke: self.stroke,
                    cmds: std::mem::take(&mut self.cmds),
                });
            }
        }
        self.cmds.clear();
        self.fill_none = false;
        self.stroke = true;
        self.cur = None;
    }

    fn move_to(&mut self, p: Pt) {
        self.push(PathCmd::MoveTo(p));
        self.cur = Some(p);
        self.start = p;
    }

    fn line_to(&mut self, p: Pt) {
        if self.cur.is_none() {
            self.move_to(p);
        } else {
            self.push(PathCmd::LineTo(p));
            self.cur = Some(p);
        }
    }

    /// The current point, starting the sub-path at `p` when there is none.
    fn cur_or(&mut self, p: Pt) -> Pt {
        if self.cur.is_none() {
            self.move_to(p);
        }
        self.cur.unwrap_or(p)
    }

    /// An elliptical arc from parametric angle `t0` sweeping `dt` (radians), joined to the
    /// current point by a line (`join`) or starting a new polygon.
    fn ellipse_arc(&mut self, c: Pt, rx: f64, ry: f64, t0: f64, dt: f64, join: bool) {
        let at = |t: f64| Pt::new(c.x + rx * t.cos(), c.y + ry * t.sin());
        if join {
            self.line_to(at(t0));
        } else {
            self.move_to(at(t0));
        }
        if dt == 0.0 {
            return;
        }
        let n = ((dt.abs() / (PI / 2.0)).ceil() as usize).clamp(1, 4);
        let step = dt / n as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        for i in 0..n {
            let a = t0 + step * i as f64;
            let b = a + step;
            let (pa, pb) = (at(a), at(b));
            let da = Pt::new(-rx * a.sin(), ry * a.cos());
            let db = Pt::new(-rx * b.sin(), ry * b.cos());
            self.push(PathCmd::CubicTo(
                Pt::new(pa.x + k * da.x, pa.y + k * da.y),
                Pt::new(pb.x - k * db.x, pb.y - k * db.y),
                pb,
            ));
        }
        self.cur = Some(at(t0 + dt));
    }

    fn run(&mut self, c: u8, a: &[f64]) {
        match c {
            b'M' => {
                for ch in a.as_chunks::<2>().0.iter() {
                    self.move_to(Pt::new(ch[0], ch[1]));
                }
            }
            b'L' => {
                for ch in a.as_chunks::<2>().0.iter() {
                    self.line_to(Pt::new(ch[0], ch[1]));
                }
            }
            b'C' => {
                for ch in a.as_chunks::<6>().0.iter() {
                    let (c1, c2, e) = (
                        Pt::new(ch[0], ch[1]),
                        Pt::new(ch[2], ch[3]),
                        Pt::new(ch[4], ch[5]),
                    );
                    self.cur_or(c1);
                    self.push(PathCmd::CubicTo(c1, c2, e));
                    self.cur = Some(e);
                }
            }
            b'Q' => {
                for ch in a.as_chunks::<4>().0.iter() {
                    let (c1, e) = (Pt::new(ch[0], ch[1]), Pt::new(ch[2], ch[3]));
                    self.cur_or(c1);
                    self.push(PathCmd::QuadTo(c1, e));
                    self.cur = Some(e);
                }
            }
            b'Z' => {
                if self.cur.is_some() {
                    self.push(PathCmd::Close);
                    self.cur = Some(self.start);
                }
            }
            b'N' => self.finish_sub(),
            b'F' => self.fill_none = true,
            b'S' => self.stroke = false,
            b'X' | b'Y' => {
                for ch in a.as_chunks::<2>().0.iter() {
                    let e = Pt::new(ch[0], ch[1]);
                    let s = self.cur_or(e);
                    let (c1, c2) = if c == b'X' {
                        (
                            Pt::new(s.x + KAPPA * (e.x - s.x), s.y),
                            Pt::new(e.x, e.y + KAPPA * (s.y - e.y)),
                        )
                    } else {
                        (
                            Pt::new(s.x, s.y + KAPPA * (e.y - s.y)),
                            Pt::new(e.x + KAPPA * (s.x - e.x), e.y),
                        )
                    };
                    self.push(PathCmd::CubicTo(c1, c2, e));
                    self.cur = Some(e);
                }
            }
            b'T' | b'U' => {
                for ch in a.as_chunks::<6>().0.iter() {
                    let (rx, ry) = (ch[2].abs(), ch[3].abs());
                    if rx < 1e-9 || ry < 1e-9 {
                        continue;
                    }
                    let (mut a0, mut a1) = (ch[4], ch[5]);
                    if a0.abs().max(a1.abs()) > 3600.0 {
                        a0 /= 65536.0;
                        a1 /= 65536.0;
                    }
                    let raw = a1 - a0;
                    let deg = if raw.abs() >= 360.0 {
                        360.0 * raw.signum()
                    } else if raw < 0.0 {
                        raw + 360.0
                    } else {
                        raw
                    };
                    self.ellipse_arc(
                        Pt::new(ch[0], ch[1]),
                        rx,
                        ry,
                        a0.to_radians(),
                        deg.to_radians(),
                        c == b'T',
                    );
                }
            }
            b'A' | b'B' | b'W' | b'V' => {
                for ch in a.as_chunks::<8>().0.iter() {
                    let (cx, cy) = ((ch[0] + ch[2]) / 2.0, (ch[1] + ch[3]) / 2.0);
                    let (rx, ry) = ((ch[2] - ch[0]).abs() / 2.0, (ch[3] - ch[1]).abs() / 2.0);
                    let end = Pt::new(ch[6], ch[7]);
                    let join = matches!(c, b'A' | b'W');
                    if rx < 1e-9 || ry < 1e-9 {
                        if join {
                            self.line_to(end);
                        } else {
                            self.move_to(end);
                        }
                        continue;
                    }
                    let ts = ((ch[5] - cy) / ry).atan2((ch[4] - cx) / rx);
                    let te = ((ch[7] - cy) / ry).atan2((ch[6] - cx) / rx);
                    let clockwise = matches!(c, b'W' | b'V');
                    let fwd = if clockwise { te - ts } else { ts - te }.rem_euclid(TAU);
                    let mag = if fwd < 1e-9 || fwd > TAU - 1e-9 {
                        TAU
                    } else {
                        fwd
                    };
                    let dt = if clockwise { mag } else { -mag };
                    self.ellipse_arc(Pt::new(cx, cy), rx, ry, ts, dt, join);
                }
            }
            b'G' => {
                for ch in a.as_chunks::<4>().0.iter() {
                    let Some(cur) = self.cur else { continue };
                    let (wr, hr) = (ch[0].abs(), ch[1].abs());
                    let (st, sw) = (ch[2], ch[3].clamp(-3600.0, 3600.0));
                    if wr < 1e-9 || hr < 1e-9 || sw == 0.0 {
                        continue;
                    }
                    self.push(PathCmd::ArcTo {
                        wr,
                        hr,
                        st_deg: st,
                        sw_deg: sw,
                    });
                    // the end point: same parametrisation as `path::resolve_path`
                    let par =
                        |deg: f64| (wr * deg.to_radians().sin()).atan2(hr * deg.to_radians().cos());
                    let (t1, t2) = (par(st), par(st + sw));
                    let centre = Pt::new(cur.x - wr * t1.cos(), cur.y - hr * t1.sin());
                    self.cur = Some(Pt::new(centre.x + wr * t2.cos(), centre.y + hr * t2.sin()));
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

/// Attribute `local` of the node, preferring the `drawooo:` prefix over any other.
fn attr_pref<'n>(node: &'n Node, local: &str) -> Option<&'n str> {
    let mut found = None;
    for (k, v) in &node.attrs {
        if local_of(k) == local {
            if k.starts_with("drawooo:") {
                return Some(v);
            }
            found.get_or_insert(v.as_str());
        }
    }
    found
}

/// The shading LibreOffice gives the sub-paths of some of its own shapes (a lighter top for the
/// can, a darker side for the cube, ...). The file does not say so: the shading belongs to the
/// `draw:type`, so it is a table here, by sub-path index (`N`-separated). The bevels with a
/// generated type name (`col-...`: the octagon and diamond bevels) cannot be told apart from any
/// other shape and stay unshaded.
fn shading(draw_type: Option<&str>) -> &'static [PathFill] {
    use PathFill::{DarkenLess as Dl, LightenLess as Ll, Norm as N};
    match draw_type {
        Some("can") => &[N, PathFill::Lighten],
        Some("cube") => &[N, Ll, Dl],
        Some("paper") => &[N, Dl],
        Some("quad-bevel") => &[N, Ll, PathFill::Darken, Dl, Ll],
        Some("smiley") => &[N, Dl, Dl],
        Some("vertical-scroll" | "horizontal-scroll") => &[N, Dl, Dl],
        _ => &[],
    }
}

fn numbers(s: &str, max: usize) -> Vec<f64> {
    s.split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .take(max)
        .map(|t| {
            t.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .unwrap_or(0.0)
        })
        .collect()
}

fn is_true(v: Option<&str>) -> bool {
    v.is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
}

/// The geometry of a `draw:enhanced-geometry` element for a shape of `w` x `h` EMU: the paths
/// (each `GeomPath { w, h }` = the viewBox size, so the writer scales them to the box) and the
/// first text area in box coordinates (EMU, `0..w` / `0..h`), both with the mirror flags applied.
///
/// `None` when there is nothing to draw from: no `draw:enhanced-path` (`draw:type` only: the
/// `ooxml-*` presets, see the module documentation), a viewBox of size 0 with no shape size to take it from, a path that is empty or
/// over a budget ([`MAX_EQUATIONS`], [`MAX_PATH_BYTES`], [`MAX_PATH_TOKENS`], [`MAX_COMMANDS`],
/// [`MAX_SUBPATHS`]). Never panics, whatever the numbers.
pub fn enhanced_geometry(node: &Node, w: f64, h: f64) -> Option<(Vec<GeomPath>, Option<Rect4>)> {
    let path = attr_pref(node, "enhanced-path")?.trim();
    if path.is_empty() {
        return None;
    }
    let (w, h) = (sane(w), sane(h));
    // no (or a malformed) viewBox: the default coordinate space of 21600 x 21600
    let vb = node
        .attr("viewBox")
        .map(|v| numbers(v, 4))
        .filter(|v| v.len() == 4)
        .unwrap_or_else(|| vec![0.0, 0.0, DEFAULT_VIEW_BOX, DEFAULT_VIEW_BOX]);
    let (vx, vy, mut vw, mut vh) = (vb[0], vb[1], vb[2], vb[3]);
    let (vx, vy) = if vw > 0.0 && vh > 0.0 && vw.abs() < MAX_ABS && vh.abs() < MAX_ABS {
        (sane(vx), sane(vy))
    } else {
        // a viewBox of size 0: the shape size in 1/100 mm
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        vw = w / EMU_PER_HMM;
        vh = h / EMU_PER_HMM;
        (0.0, 0.0)
    };

    let mut formulas = Vec::new();
    let mut names = HashMap::new();
    for e in node.nodes().filter(|n| n.name == "equation") {
        if formulas.len() >= MAX_EQUATIONS {
            return None;
        }
        let (Some(name), Some(f)) = (e.attr("name"), e.attr("formula")) else {
            continue;
        };
        names.entry(name).or_insert(formulas.len());
        formulas.push(f);
    }
    let stretch = |n: &str| {
        node.attr(n)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map_or(0.0, sane)
    };
    let env = Env {
        memo: RefCell::new(vec![Memo::Todo; formulas.len()]),
        names,
        formulas,
        mods: node
            .attr("modifiers")
            .map(|m| numbers(m, MAX_MODIFIERS))
            .unwrap_or_default(),
        k: Consts {
            left: vx,
            top: vy,
            right: sane(vx + vw),
            bottom: sane(vy + vh),
            xstretch: stretch("path-stretchpoint-x"),
            ystretch: stretch("path-stretchpoint-y"),
            width: vw,
            height: vh,
            logwidth: w / EMU_PER_HMM,
            logheight: h / EMU_PER_HMM,
        },
    };
    // Evaluate every equation once, in order: the memo is then complete and the cost bounded.
    for i in 0..env.formulas.len() {
        env.equation(i, 0);
    }

    let toks = tokenize(&env, path)?;
    let mut b = Builder {
        vw,
        vh,
        paths: Vec::new(),
        cmds: Vec::new(),
        fill_none: false,
        stroke: true,
        cur: None,
        start: Pt::new(0.0, 0.0),
        total: 0,
        over: false,
    };
    let mut i = 0;
    while i < toks.len() {
        let Tok::Cmd(c) = toks[i] else {
            i += 1; // parameters before any command
            continue;
        };
        let mut j = i + 1;
        let mut args = Vec::new();
        while let Some(Tok::Arg(v)) = toks.get(j) {
            args.push(*v);
            j += 1;
        }
        b.run(c, &args);
        if b.over {
            return None;
        }
        i = j;
    }
    b.finish_sub();
    if b.over || b.paths.is_empty() {
        return None;
    }

    for (gp, fill) in b.paths.iter_mut().zip(shading(node.attr("type"))) {
        if gp.fill_mode == PathFill::Norm {
            gp.fill_mode = *fill;
        }
    }

    let mh = is_true(node.attr("mirror-horizontal"));
    let mv = is_true(node.attr("mirror-vertical"));
    let fix = |p: Pt| {
        let (mut x, mut y) = (sane(p.x - vx), sane(p.y - vy));
        if mh {
            x = vw - x;
        }
        if mv {
            y = vh - y;
        }
        Pt::new(x, y)
    };
    for gp in &mut b.paths {
        for c in &mut gp.cmds {
            *c = match *c {
                PathCmd::MoveTo(p) => PathCmd::MoveTo(fix(p)),
                PathCmd::LineTo(p) => PathCmd::LineTo(fix(p)),
                PathCmd::QuadTo(a, e) => PathCmd::QuadTo(fix(a), fix(e)),
                PathCmd::CubicTo(a, c2, e) => PathCmd::CubicTo(fix(a), fix(c2), fix(e)),
                PathCmd::ArcTo {
                    wr,
                    hr,
                    mut st_deg,
                    mut sw_deg,
                } => {
                    if mh {
                        st_deg = 180.0 - st_deg;
                        sw_deg = -sw_deg;
                    }
                    if mv {
                        st_deg = -st_deg;
                        sw_deg = -sw_deg;
                    }
                    PathCmd::ArcTo {
                        wr,
                        hr,
                        st_deg,
                        sw_deg,
                    }
                }
                PathCmd::Close => PathCmd::Close,
            };
        }
    }

    let text = node.attr("text-areas").and_then(|t| {
        let v: Vec<f64> = t
            .split_ascii_whitespace()
            .take(4)
            .map(|tok| env.eval(tok, 0))
            .collect();
        if v.len() != 4 {
            return None;
        }
        let (l, t0) = (fix(Pt::new(v[0], v[1])), fix(Pt::new(v[2], v[3])));
        let (sx, sy) = (w / vw, h / vh);
        let (x0, x1) = (l.x.min(t0.x) * sx, l.x.max(t0.x) * sx);
        let (y0, y1) = (l.y.min(t0.y) * sy, l.y.max(t0.y) * sy);
        Some((sane(x0), sane(y0), sane(x1), sane(y1)))
    });
    Some((b.paths, text))
}

#[cfg(test)]
#[path = "odf_geom_survey.rs"]
mod survey;
#[cfg(test)]
#[path = "odf_geom_tests.rs"]
mod tests;
