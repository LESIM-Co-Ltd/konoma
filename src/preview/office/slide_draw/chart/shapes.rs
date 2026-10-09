//! The output side of the chart drawing: a list of items with a budget, and the few primitives
//! (rectangle, ellipse, polyline / path, text) the chart is made of. All arguments are in px of the
//! chart box; the items are in EMU.

use super::super::model::*;
use super::{Stroke, MAX_DRAWN};

/// EMU of a px length (finite, bounded).
pub(super) fn e(px: f64) -> f64 {
    if px.is_finite() {
        px.clamp(-1e6, 1e6) * EMU_PER_PX
    } else {
        0.0
    }
}

/// The output of a chart under construction.
pub(super) struct Out {
    /// Chart box, px.
    pub w: f64,
    pub h: f64,
    items: Vec<Item>,
    left: usize,
    pub truncated: bool,
}

impl Out {
    pub fn new(w_px: f64, h_px: f64) -> Out {
        Out {
            w: w_px,
            h: h_px,
            items: Vec::new(),
            left: MAX_DRAWN,
            truncated: false,
        }
    }

    pub fn finish(self) -> (Vec<Item>, bool) {
        (self.items, self.truncated)
    }

    /// Takes one primitive out of the budget; `false` (and the truncation flag) once it is spent.
    pub fn spend(&mut self) -> bool {
        if self.left == 0 {
            self.truncated = true;
            false
        } else {
            self.left -= 1;
            true
        }
    }

    /// Whether the budget is spent (without spending).
    pub fn full(&self) -> bool {
        self.left == 0
    }

    pub fn push(&mut self, it: Item) {
        self.items.push(it);
    }

    /// An axis-aligned rectangle (nothing for an empty one).
    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, fill: &Fill, line: Option<&Line>) {
        if !(w.is_finite() && h.is_finite() && x.is_finite() && y.is_finite()) {
            return;
        }
        if (w.abs() < 0.01 && h.abs() < 0.01) || (!fill.is_visible() && line.is_none()) {
            return;
        }
        if !self.spend() {
            return;
        }
        let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
        let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
        let mut s = ShapeItem::new(Xfrm::rect(e(x), e(y), e(w), e(h)), Geometry::Rect);
        s.fill = fill.clone();
        s.line = line.cloned();
        self.items.push(Item::Shape(s));
    }

    pub fn ellipse(
        &mut self,
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        fill: &Fill,
        line: Option<&Line>,
    ) {
        if !(rx > 0.0 && ry > 0.0 && cx.is_finite() && cy.is_finite()) {
            return;
        }
        if !fill.is_visible() && line.is_none() {
            return;
        }
        if !self.spend() {
            return;
        }
        let mut s = ShapeItem::new(
            Xfrm::rect(e(cx - rx), e(cy - ry), e(2.0 * rx), e(2.0 * ry)),
            Geometry::Ellipse,
        );
        s.fill = fill.clone();
        s.line = line.cloned();
        self.items.push(Item::Shape(s));
    }

    /// A path of commands in absolute px; `bbox` (`x0, y0, x1, y1`) is where the shape's box goes
    /// (what a gradient fill is relative to).
    pub fn path(
        &mut self,
        cmds: &[PathCmd],
        bbox: (f64, f64, f64, f64),
        fill: &Fill,
        line: Option<&Line>,
    ) {
        if cmds.len() < 2 || (!fill.is_visible() && line.is_none()) {
            return;
        }
        if !(bbox.0.is_finite() && bbox.1.is_finite() && bbox.2.is_finite() && bbox.3.is_finite()) {
            return;
        }
        if !self.spend() {
            return;
        }
        let (ox, oy) = (bbox.0, bbox.1);
        let rel = |p: Pt| Pt::new(e(p.x - ox), e(p.y - oy));
        let cmds: Vec<PathCmd> = cmds
            .iter()
            .map(|c| match *c {
                PathCmd::MoveTo(p) => PathCmd::MoveTo(rel(p)),
                PathCmd::LineTo(p) => PathCmd::LineTo(rel(p)),
                PathCmd::QuadTo(a, b) => PathCmd::QuadTo(rel(a), rel(b)),
                PathCmd::CubicTo(a, b, c2) => PathCmd::CubicTo(rel(a), rel(b), rel(c2)),
                PathCmd::ArcTo {
                    wr,
                    hr,
                    st_deg,
                    sw_deg,
                } => PathCmd::ArcTo {
                    wr: e(wr),
                    hr: e(hr),
                    st_deg,
                    sw_deg,
                },
                PathCmd::Close => PathCmd::Close,
            })
            .collect();
        let mut s = ShapeItem::new(
            Xfrm::rect(
                e(ox),
                e(oy),
                e((bbox.2 - bbox.0).max(0.0)),
                e((bbox.3 - bbox.1).max(0.0)),
            ),
            Geometry::Paths(vec![GeomPath {
                w: 0.0,
                h: 0.0,
                fill_mode: PathFill::Norm,
                stroke: line.is_some(),
                cmds,
            }]),
        );
        s.fill = fill.clone();
        s.line = line.cloned();
        self.items.push(Item::Shape(s));
    }

    /// A polygon or polyline through `pts` (straight segments).
    pub fn poly(&mut self, pts: &[(f64, f64)], close: bool, fill: &Fill, line: Option<&Line>) {
        let pts: Vec<(f64, f64)> = pts
            .iter()
            .copied()
            .filter(|p| p.0.is_finite() && p.1.is_finite())
            .collect();
        if pts.len() < 2 {
            return;
        }
        let mut cmds = Vec::with_capacity(pts.len() + 1);
        for (i, p) in pts.iter().enumerate() {
            let pt = Pt::new(p.0, p.1);
            cmds.push(if i == 0 {
                PathCmd::MoveTo(pt)
            } else {
                PathCmd::LineTo(pt)
            });
        }
        if close {
            cmds.push(PathCmd::Close);
        }
        let bb = bbox_of(&pts);
        self.path(&cmds, bb, fill, line);
    }

    /// A smooth curve through `pts` (Catmull-Rom as cubic Beziers).
    pub fn smooth(&mut self, pts: &[(f64, f64)], fill: &Fill, line: Option<&Line>) {
        let pts: Vec<(f64, f64)> = pts
            .iter()
            .copied()
            .filter(|p| p.0.is_finite() && p.1.is_finite())
            .collect();
        if pts.len() < 3 {
            self.poly(&pts, false, fill, line);
            return;
        }
        let cmds = smooth_cmds(&pts);
        self.path(&cmds, bbox_of(&pts), fill, line);
    }

    /// A straight line.
    pub fn seg(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, line: &Line) {
        self.poly(&[(x1, y1), (x2, y2)], false, &Fill::None, Some(line));
    }
}

/// The commands of a smooth curve through the points (the first one a `MoveTo`).
pub(super) fn smooth_cmds(pts: &[(f64, f64)]) -> Vec<PathCmd> {
    let mut cmds = vec![PathCmd::MoveTo(Pt::new(pts[0].0, pts[0].1))];
    let n = pts.len();
    for i in 0..n - 1 {
        let p0 = if i == 0 { pts[0] } else { pts[i - 1] };
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = if i + 2 < n { pts[i + 2] } else { pts[n - 1] };
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        cmds.push(PathCmd::CubicTo(
            Pt::new(c1.0, c1.1),
            Pt::new(c2.0, c2.1),
            Pt::new(p2.0, p2.1),
        ));
    }
    cmds
}

pub(super) fn bbox_of(pts: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        b.0 = b.0.min(p.0);
        b.1 = b.1.min(p.1);
        b.2 = b.2.max(p.0);
        b.3 = b.3.max(p.1);
    }
    if b.0 > b.2 {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        b
    }
}

/// The outline for a stroke spec: `None` for "no line", otherwise the colour, width and dash with
/// the given defaults for what the spec leaves unset. `None` spec = the automatic line.
pub(super) fn line_of(s: Option<&Stroke>, def_color: Rgba, def_width: f64) -> Option<Line> {
    match s {
        Some(s) if s.none => None,
        Some(s) => {
            let mut l = Line::solid(
                s.width
                    .filter(|w| w.is_finite())
                    .unwrap_or(def_width)
                    .clamp(0.0, 5_000_000.0),
                s.color.unwrap_or(def_color),
            );
            l.dash = s.dash.clone();
            Some(l)
        }
        None => Some(Line::solid(def_width, def_color)),
    }
}

/// Like [`line_of`] for an element that has no outline unless one is specified.
pub(super) fn line_if_set(s: Option<&Stroke>, def_color: Rgba, def_width: f64) -> Option<Line> {
    s.and_then(|s| line_of(Some(s), def_color, def_width))
}

/// The point on a circle: angle in degrees clockwise from 12 o'clock.
pub(super) fn on_circle(cx: f64, cy: f64, r: f64, deg: f64) -> (f64, f64) {
    let a = deg.to_radians();
    (cx + r * a.sin(), cy - r * a.cos())
}

/// The point on an ellipse (radii `rx`, `ry`): angle in degrees clockwise from 12 o'clock.
pub(super) fn on_ellipse(cx: f64, cy: f64, rx: f64, ry: f64, deg: f64) -> (f64, f64) {
    let a = deg.to_radians();
    (cx + rx * a.sin(), cy - ry * a.cos())
}
