//! Drawing an `architecture-beta`.
//!
//! # This one is placed on a grid, and that is a deliberate departure
//!
//! Stage 5b's brief expected this language to go through [`super::lay_out_spec`] with the other
//! node-and-edge kinds. It does not, and the reason is the grammar: an edge here names a **side**
//! at each end (`db:R --> L:server`), and a side is a statement about *where the other box is*.
//! dagre has one rank direction for the whole graph and no way to honour "this one is to the
//! right of that one" — so a layered layout would read the ports, place the boxes somewhere else,
//! and draw lines that contradict what the author wrote.
//!
//! So the placement is a **breadth-first walk of the edges**: the first service sits at the
//! origin, and every edge puts its other end one cell along in the direction its ports name. A
//! cell that is taken is stepped past, which is the only decision this module makes that the
//! source did not.
//!
//! `align row` / `align column` are read and applied as a final pass over the members they name.
//!
//! # A group is a frame
//!
//! Which means the cluster invariants apply, exactly as they do for a C4 boundary.

use std::collections::{BTreeMap, HashMap};

use crate::preview::mermaid::architecture::{Architecture, Side};
use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

use super::band;
use super::{
    normalise, shapes, svg, Diagram, Glyph, Label, PlacedCluster, PlacedEdge, PlacedNode,
    RenderError, Size, Theme, Tip,
};

/// Distance between two cells, centre to centre, across.
pub const CELL_W: f64 = 132.0;

/// The same, down.
pub const CELL_H: f64 = 84.0;

/// The narrowest gap left between two boxes in neighbouring cells.
///
/// [`CELL_W`] and [`CELL_H`] are the *minimum* pitch, not the answer: a service whose name is
/// wider than the pitch would otherwise be drawn over the box beside it, which is what happened
/// the first time a long label reached the corpus.
pub const CELL_GAP: f64 = 20.0;

/// Blank space between a group frame and what is inside it.
pub const GROUP_PAD: f64 = 12.0;

/// Side of a junction's dot.
pub const JUNCTION: f64 = 8.0;

/// Reads a mermaid architecture diagram and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let arch = crate::preview::mermaid::architecture::parse(code)?;
    let diagram = lay_out(&arch)?;
    Ok(svg::emit(&diagram, &Theme::named(theme)))
}

/// Works out where every service, junction, group and edge goes.
pub fn lay_out(arch: &Architecture) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if arch.services.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    let cells = place_on_grid(arch);
    // **Every box is measured before any of them is placed.** The grid used to step by a fixed
    // `CELL_W`, which says a box is never wider than 132px — and a service whose name is longer
    // than that was drawn straight over its neighbour, with the line between them ending inside
    // the other box. The pitch is now whatever the two boxes on either side of a gap need, with
    // `CELL_W` / `CELL_H` as the floor, so the grid still reads as a grid when the labels are
    // short and stops colliding when they are not.
    let boxes: Vec<(Glyph, Label, Size)> = arch.services.iter().map(measure).collect();
    let sizes: Vec<Size> = boxes.iter().map(|(_, _, size)| *size).collect();
    let xs = axis(&cells, &sizes, true);
    let ys = axis(&cells, &sizes, false);

    let mut nodes: Vec<PlacedNode> = Vec::new();
    for (i, s) in arch.services.iter().enumerate() {
        let (col, row) = cells[i];
        let center = Point::new(xs[&col], ys[&row]);
        let (glyph, label, size) = boxes[i].clone();
        nodes.push(PlacedNode {
            id: s.id.clone(),
            shape: glyph,
            center,
            size,
            label,
            panel: None,
            series: None,
            mark: None,
        });
        let _ = s;
    }

    // --- the edges ------------------------------------------------------------------------------
    let by_id: HashMap<&str, usize> = arch
        .services
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let mut edges: Vec<PlacedEdge> = Vec::new();
    for e in &arch.edges {
        let (Some(&a), Some(&b)) = (by_id.get(e.from.as_str()), by_id.get(e.to.as_str())) else {
            continue;
        };
        let (from, to) = (&nodes[a], &nodes[b]);
        // Each end leaves by the side the source named, so the line agrees with the ports even
        // where the grid could not put the two boxes exactly opposite each other.
        let start = port(from, e.from_side);
        let end = port(to, e.to_side);
        // One elbow, turned the way the *sending* port faces, so a line out of a right-hand port
        // starts by going right.
        let mut points = vec![start.clone()];
        if (start.x - end.x).abs() > 0.5 && (start.y - end.y).abs() > 0.5 {
            points.push(match e.from_side {
                Side::Left | Side::Right => Point::new(end.x, start.y),
                Side::Top | Side::Bottom => Point::new(start.x, end.y),
            });
        }
        points.push(end);
        edges.push(PlacedEdge {
            from: e.from.clone(),
            to: e.to.clone(),
            points,
            tip_start: if e.from_arrow { Tip::Arrow } else { Tip::None },
            tip_end: if e.to_arrow { Tip::Arrow } else { Tip::None },
            stroke: Stroke::Normal,
            label: None,
            start_label: None,
            end_label: None,
            badge: None,
            series: None,
            // The elbow is the port directions, drawn: a spline through it would round the corner
            // off and cross the cell beside it.
            straight: true,
            overlay: false,
        });
        if !e.title.trim().is_empty() {
            let label = Label::measure(&e.title);
            let mid = edges.last().expect("just pushed").points.clone();
            if let Some(center) = super::edges::arc_midpoint(&mid) {
                let size = Size::new(
                    label.width + super::LABEL_PAD_X * 2.0,
                    label.height + super::LABEL_PAD_Y * 2.0,
                );
                edges.last_mut().expect("just pushed").label = Some(super::PlacedEdgeLabel {
                    center,
                    size,
                    label,
                });
            }
        }
    }

    // --- the group frames -----------------------------------------------------------------------
    //
    // **Deepest first**, and a frame is grown around the *frames* of the groups inside it as well
    // as around its own boxes. Both halves of that are the fix to one defect: the frames used to
    // be built in declaration order around a group's transitive services, with the padding scaled
    // by depth — and scaled the wrong way, so a nested group took a **wider** margin than the
    // group holding it and its frame came out around the outside of its own parent's. A frame has
    // to clear the border and the title band of every frame it holds, and the only thing that
    // knows how much room those need is the child frame itself, so it is built first and taken
    // whole.
    let mut clusters: Vec<PlacedCluster> = Vec::new();
    let mut order: Vec<usize> = (0..arch.groups.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(depth_of(arch, &arch.groups[*i].id)));
    for gi in order {
        let group = &arch.groups[gi];
        let (mut l, mut t, mut r, mut b) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let mut grow = |(ml, mt, mr, mb): (f64, f64, f64, f64)| {
            l = l.min(ml);
            t = t.min(mt);
            r = r.max(mr);
            b = b.max(mb);
        };
        for (i, s) in arch.services.iter().enumerate() {
            if s.group.as_deref() == Some(group.id.as_str()) {
                grow(nodes[i].bounds());
            }
        }
        for c in &clusters {
            if c.parent.as_deref() == Some(group.id.as_str()) {
                grow(c.bounds());
            }
        }
        // A group holding neither a service nor another group's frame has nothing to draw round.
        if l > r {
            continue;
        }
        let mut frame = band::frame(
            group.id.clone(),
            &group.title,
            l - GROUP_PAD,
            t - GROUP_PAD,
            r + GROUP_PAD,
            b + GROUP_PAD,
        );
        frame.parent = group.parent.clone();
        frame.depth = depth_of(arch, &group.id);
        clusters.push(frame);
    }
    clusters.sort_by_key(|c| c.depth);

    let mut diagram = Diagram {
        nodes,
        edges,
        clusters,
        ..Diagram::default()
    };
    band::add_title(&mut diagram, arch.preamble.title.as_deref());
    normalise(&mut diagram);
    Ok(diagram)
}

/// How many groups a group is inside.
fn depth_of(arch: &Architecture, id: &str) -> usize {
    let mut at = arch
        .groups
        .iter()
        .find(|g| g.id == id)
        .and_then(|g| g.parent.as_deref());
    let mut n = 0usize;
    for _ in 0..arch.groups.len() + 1 {
        let Some(parent) = at else { break };
        n += 1;
        at = arch
            .groups
            .iter()
            .find(|g| g.id == parent)
            .and_then(|g| g.parent.as_deref());
    }
    n
}

/// Where a line leaves a box by the side the source named.
fn port(node: &PlacedNode, side: Side) -> Point {
    let (l, t, r, b) = node.bounds();
    match side {
        Side::Left => Point::new(l, node.center.y),
        Side::Right => Point::new(r, node.center.y),
        Side::Top => Point::new(node.center.x, t),
        Side::Bottom => Point::new(node.center.x, b),
    }
}

/// What one service is drawn as, and how big.
///
/// Separated from the placing because the placing needs the sizes: a cell is as big as what is
/// in it, and nothing knows that until every label has been measured.
fn measure(s: &crate::preview::mermaid::architecture::Service) -> (Glyph, Label, Size) {
    if s.junction {
        return (
            Glyph::ChartPoint,
            Label::measure(""),
            Size::new(JUNCTION, JUNCTION),
        );
    }
    let text = if s.title.trim().is_empty() {
        s.id.clone()
    } else {
        s.title.clone()
    };
    let label = Label::measure(&text);
    let glyph = Glyph::Flow(Shape::RoundedRect);
    let size = shapes::size(glyph, Size::new(label.width, label.height));
    (glyph, label, size)
}

/// Where each occupied column (`across`) or row has its centre line.
///
/// Two neighbouring lines are whichever is further apart: the fixed pitch, or half of each
/// side's widest box plus [`CELL_GAP`]. A cell nobody occupies still costs the fixed pitch,
/// because a gap in the grid is a gap the walk deliberately stepped over.
fn axis(cells: &[(i32, i32)], sizes: &[Size], across: bool) -> HashMap<i32, f64> {
    let pitch = if across { CELL_W } else { CELL_H };
    let extent = |s: &Size| if across { s.w } else { s.h };
    let key = |c: &(i32, i32)| if across { c.0 } else { c.1 };

    let mut widest: BTreeMap<i32, f64> = BTreeMap::new();
    for (i, c) in cells.iter().enumerate() {
        let at = widest.entry(key(c)).or_insert(0.0);
        *at = at.max(extent(&sizes[i]));
    }
    let mut out: HashMap<i32, f64> = HashMap::new();
    let mut coordinate = 0.0_f64;
    let mut previous: Option<(i32, f64)> = None;
    for (&k, &size) in &widest {
        if let Some((last_k, last_size)) = previous {
            let steps = (k - last_k) as f64;
            coordinate += (pitch * steps).max((last_size + size) / 2.0 + CELL_GAP);
        }
        out.insert(k, coordinate);
        previous = Some((k, size));
    }
    out
}

/// The grid cell of every service, from the ports its edges name.
///
/// **Group by group.** The services of one group are placed together, in a band of rows of their
/// own, before the next group starts — because a group is drawn as a frame around its members, and
/// two groups whose members interleave draw two frames that overlap, which is the same picture as
/// one group.
///
/// Within a band it is a breadth-first walk of the edges: the first service sits at the band's
/// origin, and every edge puts its other end one cell along in the direction its ports name. A
/// cell that is already taken is stepped past, which is the only decision this function makes that
/// the source did not.
fn place_on_grid(arch: &Architecture) -> Vec<(i32, i32)> {
    let n = arch.services.len();
    let index: HashMap<&str, usize> = arch
        .services
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let mut cell: Vec<Option<(i32, i32)>> = vec![None; n];
    let mut taken: Vec<(i32, i32)> = Vec::new();
    let mut next_free_row = 0i32;

    // The bands: one per group, in declaration order, then everything in no group.
    let mut bands: Vec<Vec<usize>> = Vec::new();
    for group in &arch.groups {
        let members: Vec<usize> = (0..n)
            .filter(|i| arch.services[*i].group.as_deref() == Some(group.id.as_str()))
            .collect();
        if !members.is_empty() {
            bands.push(members);
        }
    }
    let loose: Vec<usize> = (0..n)
        .filter(|i| {
            arch.services[*i]
                .group
                .as_deref()
                .is_none_or(|g| !arch.groups.iter().any(|x| x.id == g))
        })
        .collect();
    if !loose.is_empty() {
        bands.push(loose);
    }

    for (b, band) in bands.iter().enumerate() {
        // A blank row between two bands. A group's frame reaches a padding and a title band past
        // its members, so two bands packed against each other draw two frames that touch — which
        // `check_unrelated_clusters_do_not_overlap` catches and which reads as one group.
        if b > 0 {
            next_free_row += 1;
        }
        // The band's own row, and where the next service nothing has connected yet goes on it.
        // **Across, not down**: the members of a group are siblings, so a group with no internal
        // edges reads as a row of boxes rather than as a column, and two groups joined by an edge
        // then sit one above the other — which is what the edge said.
        let base_row = next_free_row;
        let mut next_seed_col = 0i32;
        for &seed in band {
            if cell[seed].is_some() {
                continue;
            }
            let mut at = (next_seed_col, base_row);
            while taken.contains(&at) {
                at.0 += 1;
            }
            next_seed_col = at.0 + 1;
            taken.push(at);
            cell[seed] = Some(at);
            next_free_row = next_free_row.max(at.1 + 1);

            let mut queue = vec![seed];
            while let Some(from) = queue.pop() {
                let here = cell[from].expect("placed");
                for e in &arch.edges {
                    // Which way each end lies from the other. `db:L -- R:server` leaves `db` by
                    // its **left**, so `server` is to the left of `db`; and it arrives at
                    // `server`'s **right**, so `db` is to the right of `server`. Both steps are
                    // the side written at the end being *left behind*, not its opposite — which
                    // would mirror the whole diagram against its own source.
                    for (a, b, side) in [(&e.from, &e.to, e.from_side), (&e.to, &e.from, e.to_side)]
                    {
                        if index.get(a.as_str()) != Some(&from) {
                            continue;
                        }
                        let Some(&other) = index.get(b.as_str()) else {
                            continue;
                        };
                        // Only inside this band: a line to another group is drawn, not obeyed.
                        if cell[other].is_some() || !band.contains(&other) {
                            continue;
                        }
                        let step = side.step();
                        let got = claim((here.0 + step.0, here.1 + step.1), step, &mut taken, n);
                        cell[other] = Some(got);
                        next_free_row = next_free_row.max(got.1 + 1);
                        queue.push(other);
                    }
                }
            }
        }
    }

    let mut out: Vec<(i32, i32)> = cell.iter().map(|c| c.unwrap_or((0, 0))).collect();
    apply_alignments(arch, &index, &mut out);
    out
}

/// The first free cell at or beyond `at`, stepping in `step`.
fn claim(at: (i32, i32), step: (i32, i32), taken: &mut Vec<(i32, i32)>, n: usize) -> (i32, i32) {
    let mut c = at;
    // The guard is on the count: a zero step — which no side gives — would otherwise spin.
    for _ in 0..(n as i32 + 2) {
        if !taken.contains(&c) {
            break;
        }
        if step == (0, 0) {
            c.0 += 1;
        } else {
            c = (c.0 + step.0, c.1 + step.1);
        }
    }
    taken.push(c);
    c
}

/// `align row a b c` / `align column a b c`, applied after the walk.
fn apply_alignments(arch: &Architecture, index: &HashMap<&str, usize>, cells: &mut [(i32, i32)]) {
    for a in &arch.alignments {
        let members: Vec<usize> = a
            .members
            .iter()
            .filter_map(|m| index.get(m.as_str()).copied())
            .collect();
        let Some(&first) = members.first() else {
            continue;
        };
        for &m in &members {
            if a.row {
                cells[m].1 = cells[first].1;
            } else {
                cells[m].0 = cells[first].0;
            }
        }
        // …and then spread them along the other axis so they do not land on each other.
        let mut used: Vec<i32> = Vec::new();
        for &m in &members {
            let axis = if a.row { 0 } else { 1 };
            let mut v = if axis == 0 { cells[m].0 } else { cells[m].1 };
            while used.contains(&v) {
                v += 1;
            }
            used.push(v);
            if axis == 0 {
                cells[m].0 = v;
            } else {
                cells[m].1 = v;
            }
        }
    }
}
