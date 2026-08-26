//! What the four **banded** languages share — `journey`, `timeline`, `gantt` and `kanban`.
//!
//! None of them is a layered graph and none of them is a data chart. What they have in common is
//! their shape: things laid out in order along one axis, grouped into bands that are drawn as
//! frames. Two pieces of that are identical in all four, and they are here rather than copied:
//!
//! * **a band is a [`PlacedCluster`]**, which is what makes the six cluster invariants apply to a
//!   gantt section and a kanban column without a line being written for them;
//! * **the title goes above everything, frames included.** [`super::chart::add_title`] measures
//!   nodes and edges, which is right for a chart and wrong here: a band's frame is what reaches
//!   highest, and a title placed off the nodes alone lands inside it.

use crate::preview::mermaid::layout::Point;

use super::chart::{label_node, TITLE_GAP};
use super::{clusters, Diagram, Label, PlacedCluster, Size};

/// How much room a band's title needs above its contents.
///
/// A caller that stacks its bands **down the page** — a gantt's sections, a top-to-bottom
/// timeline's — has to reserve this between one band and the next, or the lower band's title
/// grows up into the band above it and the two frames overlap.
pub fn title_room(title: &str) -> f64 {
    let label = Label::measure(title);
    if label.is_blank() {
        0.0
    } else {
        label.height + clusters::TITLE_PAD_Y * 2.0
    }
}

/// Builds a band's frame, grown upwards to hold its title clear of what is inside it.
///
/// `(l, t, r, b)` is the room the band's *contents* need; the frame is that plus a title band on
/// top when there is a title. Returning the frame rather than pushing it keeps the caller's
/// ordering — outermost first — visible where the caller can see it.
pub fn frame(id: String, title: &str, l: f64, t: f64, r: f64, b: f64) -> PlacedCluster {
    let label = Label::measure(title);
    let t = t - title_room(title);
    PlacedCluster {
        id,
        title: label,
        center: Point::new((l + r) / 2.0, (t + b) / 2.0),
        size: Size::new(r - l, b - t),
        parent: None,
        depth: 0,
        dashed: false,
        filled: true,
        sections: Vec::new(),
    }
}

/// Puts the diagram's title above **everything**, frames included.
pub fn add_title(diagram: &mut Diagram, title: Option<&str>) {
    let Some(text) = title else { return };
    let label = Label::measure(text);
    if label.is_blank() {
        return;
    }
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let mut max_x = f64::NEG_INFINITY;
    let mut grow = |l: f64, t: f64, r: f64| {
        min_x = min_x.min(l);
        min_y = min_y.min(t);
        max_x = max_x.max(r);
    };
    for n in &diagram.nodes {
        let (l, t, r, _) = n.bounds();
        grow(l, t, r);
    }
    for c in &diagram.clusters {
        let (l, t, r, _) = c.bounds();
        grow(l, t, r);
    }
    for e in &diagram.edges {
        for p in &e.points {
            grow(p.x, p.y, p.x);
        }
    }
    if !min_x.is_finite() {
        return;
    }
    let center = Point::new(
        (min_x + max_x) / 2.0,
        min_y - TITLE_GAP - label.height / 2.0,
    );
    if let Some(n) = label_node("chart#title", label, center, None) {
        diagram.nodes.push(n);
    }
}
