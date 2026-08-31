//! Drawing a `block-beta`.
//!
//! # The grid is the diagram
//!
//! `columns 3` and `a:2` are not hints about how to draw the picture — they *are* the picture. So
//! this module places the blocks on the grid the source describes and does not go near
//! [`super::lay_out_spec`]: a layered layout would rank them, and a ranked block diagram is a
//! different diagram.
//!
//! The rule is upstream's: fill a row left to right, wrap when the next block will not fit in what
//! is left of it, and give every cell in a column the width of the widest thing in that column.
//! `columns auto` (the default, `-1`) means one row.
//!
//! # The defect this is measured against
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §6-A records the crate drawing `a --> e` as a stub between
//! `a` and `b`. A link here is routed between the two blocks it *names*, wherever they landed on
//! the grid, and [`super::tests::check_endpoints_land_on_the_outline`] is what says so.

use std::collections::HashMap;

use crate::preview::mermaid::block::{BlockDiagram, Composite, Dir, Item, Node, AUTO};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

use super::band;
use super::shapes::Mark;
use super::{
    edges, normalise, shapes, svg, Curve, Diagram, Glyph, Label, PlacedCluster, PlacedEdge,
    PlacedEdgeLabel, PlacedNode, RenderError, Size, Theme,
};

/// Blank space between two cells.
pub const CELL_GAP: f64 = 12.0;

/// Blank space between a composite's frame and what is inside it.
pub const FRAME_PAD: f64 = 10.0;

/// The narrowest a cell may be.
pub const MIN_CELL: f64 = 56.0;

/// How far below the blocks a link that cannot go straight is taken.
pub const DETOUR: f64 = 22.0;

/// The shortest a row may be.
pub const MIN_ROW: f64 = 28.0;

/// Reads a mermaid block diagram and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let diagram = crate::preview::mermaid::block::parse(code)?;
    let laid = lay_out(&diagram)?;
    Ok(svg::emit(&laid, &Theme::named(theme)))
}

/// Works out where every block, frame and line goes.
pub fn lay_out(diagram: &BlockDiagram) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if diagram.items.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut clusters: Vec<PlacedCluster> = Vec::new();
    let grid = place(
        &diagram.items,
        diagram.columns,
        0,
        0.0,
        0.0,
        &mut nodes,
        &mut clusters,
    );
    let _ = grid;

    // --- the links ------------------------------------------------------------------------------
    let by_id: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let mut placed: Vec<PlacedEdge> = Vec::new();
    for edge in &diagram.edges {
        let ends = |id: &str| -> Option<(Point, Size, Glyph)> {
            if let Some(&i) = by_id.get(id) {
                return Some((nodes[i].center.clone(), nodes[i].size, nodes[i].shape));
            }
            clusters.iter().find(|c| c.id == id).map(|c| {
                (
                    c.center.clone(),
                    c.size,
                    Glyph::Flow(crate::preview::mermaid::flowchart::Shape::Rect),
                )
            })
        };
        let (Some((ac, asz, ag)), Some((bc, bsz, bg))) = (ends(&edge.from), ends(&edge.to)) else {
            continue;
        };
        // A straight line between two cells that are not neighbours runs *through* whatever is
        // between them, and so does its label. So a link whose direct route crosses another block
        // is taken round below the two it joins — which is what makes `a --> e` on a row of five
        // readable, and is the case §6-A records the crate drawing as a stub between `a` and `b`.
        let straight = vec![
            shapes::intersect(ag, ac.clone(), asz, &bc),
            shapes::intersect(bg, bc.clone(), bsz, &ac),
        ];
        let blocked = nodes
            .iter()
            .any(|n| n.id != edge.from && n.id != edge.to && crosses(&straight, n));
        let (points, elbow) = if blocked {
            (detour(&ac, asz, &bc, bsz), true)
        } else {
            (straight, false)
        };
        let (tip_start, tip_end) = edges::Tip::of_arrow(edge.arrow);
        let label = edge.label.as_deref().map(Label::measure).and_then(|l| {
            edges::arc_midpoint(&points).map(|center| PlacedEdgeLabel {
                center,
                size: Size::new(
                    l.width + super::LABEL_PAD_X * 2.0,
                    l.height + super::LABEL_PAD_Y * 2.0,
                ),
                label: l,
            })
        });
        placed.push(PlacedEdge {
            from: edge.from.clone(),
            to: edge.to.clone(),
            points,
            gaps: Vec::new(),
            tip_start,
            tip_end,
            stroke: edge.stroke,
            label,
            start_label: None,
            end_label: None,
            badge: None,
            series: None,
            // A detour is three right angles the grid asked for; a spline through them draws a
            // wide arc across the cells in between, which is the picture this language exists to
            // avoid.
            straight: elbow,
            overlay: false,
            style: None,
            curve: Curve::Basis,
            tip_matches_line: false,
        });
    }

    clusters.sort_by_key(|c| c.depth);
    let mut out = Diagram {
        nodes,
        edges: placed,
        clusters,
        ..Diagram::default()
    };
    band::add_title(&mut out, diagram.preamble.title.as_deref());
    normalise(&mut out);
    Ok(out)
}

/// Lays a grid out with its top-left corner at `(x0, y0)`, and returns the room it took.
fn place(
    items: &[Item],
    columns: i32,
    depth: usize,
    x0: f64,
    y0: f64,
    nodes: &mut Vec<PlacedNode>,
    clusters: &mut Vec<PlacedCluster>,
) -> Size {
    // How many cells a row has. `auto` is one row, which is what `-1` means upstream and what
    // makes `block-beta\n a b c` a row of three.
    let total: usize = items.iter().map(Item::width).sum();
    let per_row = if columns == AUTO {
        total.max(1)
    } else {
        (columns.max(1) as usize).max(1)
    };

    // Every item's natural size first: a composite has to be laid out to be measured, so it is
    // laid out at the origin and moved afterwards. A composite's own measurement includes the
    // band its title needs, so the frame does not grow up into the row above it.
    let mut sizes: Vec<Size> = Vec::with_capacity(items.len());
    let mut inner: Vec<Vec<PlacedNode>> = Vec::with_capacity(items.len());
    let mut inner_frames: Vec<Vec<PlacedCluster>> = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Item::Node(n) => sizes.push(node_size(n)),
            Item::Space { .. } => sizes.push(Size::new(0.0, 0.0)),
            Item::Composite(c) => {
                let mut sub_nodes = Vec::new();
                let mut sub_frames = Vec::new();
                let size = place(
                    &c.children,
                    c.columns,
                    depth + 1,
                    0.0,
                    0.0,
                    &mut sub_nodes,
                    &mut sub_frames,
                );
                sizes.push(Size::new(
                    size.w + FRAME_PAD * 2.0,
                    size.h + FRAME_PAD * 2.0 + band::title_room(composite_title(c)),
                ));
                inner.push(sub_nodes);
                inner_frames.push(sub_frames);
                continue;
            }
        }
        inner.push(Vec::new());
        inner_frames.push(Vec::new());
    }

    // The cell **width** is one number for the whole grid, so the columns line up — which is what
    // a grid is. The **height** is per row: a row holding a nested block is as tall as that block
    // and a row of plain boxes is not, so a diagram with one group in it does not become a page of
    // airy stripes.
    let cell_w = sizes
        .iter()
        .zip(items)
        .map(|(s, i)| s.w / i.width().max(1) as f64)
        .fold(MIN_CELL, f64::max);

    // Which row each item lands on, filling left to right and wrapping when the next one will not
    // fit in what is left.
    let mut of_row: Vec<(usize, usize)> = Vec::with_capacity(items.len()); // (row, column)
    let mut col = 0usize;
    let mut row = 0usize;
    for item in items {
        let span = item.width().max(1).min(per_row);
        if col + span > per_row && col > 0 {
            col = 0;
            row += 1;
        }
        of_row.push((row, col));
        col += span;
    }
    let rows = row + 1;
    let mut row_h: Vec<f64> = vec![0.0; rows];
    for (i, (r, _)) in of_row.iter().enumerate() {
        row_h[*r] = row_h[*r].max(sizes[i].h);
    }
    for h in &mut row_h {
        *h = h.max(MIN_ROW);
    }
    let row_top: Vec<f64> = (0..rows)
        .map(|r| y0 + row_h[..r].iter().sum::<f64>() + CELL_GAP * r as f64)
        .collect();

    let mut widest = 0.0_f64;
    for (i, item) in items.iter().enumerate() {
        let (r, c) = of_row[i];
        let span = item.width().max(1).min(per_row);
        let w = cell_w * span as f64 + CELL_GAP * (span - 1) as f64;
        let x = x0 + c as f64 * (cell_w + CELL_GAP);
        let center = Point::new(x + w / 2.0, row_top[r] + row_h[r] / 2.0);
        widest = widest.max(x + w - x0);

        match item {
            Item::Space { .. } => {}
            Item::Node(n) => {
                // A cell's *width* is the grid's and its height is the row's — except for a shape
                // whose proportions carry meaning. Stretching a circle to fill a `columns 1` row
                // draws an ellipse the width of the page, which says "this is an ellipse"; the
                // shapes below therefore keep the size their own label asked for, centred in
                // their cell.
                let natural = sizes[i];
                let (bw, bh) = if keeps_its_shape(n) {
                    (natural.w, natural.h)
                } else {
                    (w, natural.h.max(row_h[r] * 0.8).min(row_h[r]))
                };
                nodes.push(block_node(n, center, Size::new(bw, bh)));
            }
            Item::Composite(c2) => {
                let dx = center.x - sizes[i].w / 2.0 + FRAME_PAD;
                let dy =
                    center.y - sizes[i].h / 2.0 + FRAME_PAD + band::title_room(composite_title(c2));
                for mut n in inner[i].clone() {
                    n.center = Point::new(n.center.x + dx, n.center.y + dy);
                    nodes.push(n);
                }
                for mut f in inner_frames[i].clone() {
                    f.center = Point::new(f.center.x + dx, f.center.y + dy);
                    // **Whose frame this one is inside.** The recursion above laid `c2`'s
                    // children out and gave each of *their* nested frames a parent; whichever
                    // came back without one is a direct child of `c2`. Without this the link is
                    // never made, and a `PlacedCluster` with no parent is not a frame that
                    // merely lacks an attribute: `check_nested_clusters_sit_inside_their_parent`
                    // skips it (so nothing checks that a nested block stays in its own box) and
                    // `check_unrelated_clusters_do_not_overlap` reads the two as strangers and
                    // calls their nesting a collision. Both are §6's "green and checking
                    // nothing" shape.
                    if f.parent.is_none() {
                        f.parent = Some(c2.id.clone());
                    }
                    clusters.push(f);
                }
                clusters.push(composite_frame(
                    c2,
                    center,
                    Size::new(w, sizes[i].h.max(row_h[r])),
                    depth,
                ));
            }
        }
    }

    Size::new(
        widest.max(cell_w),
        row_h.iter().sum::<f64>() + CELL_GAP * (rows - 1) as f64,
    )
}

/// Whether a two-point line passes through a node's box.
fn crosses(line: &[Point], node: &PlacedNode) -> bool {
    let (l, t, r, b) = node.bounds();
    let (a, z) = (&line[0], &line[line.len() - 1]);
    let steps = ((a.x - z.x).hypot(a.y - z.y).ceil() as usize).clamp(1, 4000);
    (0..=steps).any(|i| {
        let f = i as f64 / steps as f64;
        let (x, y) = (a.x + f * (z.x - a.x), a.y + f * (z.y - a.y));
        x > l + 0.5 && x < r - 0.5 && y > t + 0.5 && y < b - 0.5
    })
}

/// A route below both boxes, for a link whose direct line would run through a third.
fn detour(a: &Point, asz: Size, z: &Point, zsz: Size) -> Vec<Point> {
    let under = (a.y + asz.h / 2.0).max(z.y + zsz.h / 2.0) + DETOUR;
    vec![
        Point::new(a.x, a.y + asz.h / 2.0),
        Point::new(a.x, under),
        Point::new(z.x, under),
        Point::new(z.x, z.y + zsz.h / 2.0),
    ]
}

/// One block's box.
fn block_node(n: &Node, center: Point, size: Size) -> PlacedNode {
    let label = Label::measure(&n.label);
    let (glyph, mark) = match &n.arrow {
        Some(dirs) => (
            Glyph::BlockArrow,
            Some(Mark::BlockArrow {
                left: dirs.iter().any(|d| matches!(d, Dir::Left | Dir::X)),
                right: dirs.iter().any(|d| matches!(d, Dir::Right | Dir::X)),
                up: dirs.iter().any(|d| matches!(d, Dir::Up | Dir::Y)),
                down: dirs.iter().any(|d| matches!(d, Dir::Down | Dir::Y)),
            }),
        ),
        None => (Glyph::Flow(n.shape), None),
    };
    PlacedNode {
        id: n.id.clone(),
        shape: glyph,
        center,
        size,
        label,
        panel: None,
        series: None,
        mark,
        style: None,
    }
}

/// The natural size of one block, before the grid levels it.
fn node_size(n: &Node) -> Size {
    let label = Label::measure(&n.label);
    let glyph = if n.arrow.is_some() {
        Glyph::BlockArrow
    } else {
        Glyph::Flow(n.shape)
    };
    let size = shapes::size(glyph, Size::new(label.width, label.height));
    Size::new(
        size.w / n.width.max(1) as f64 * n.width.max(1) as f64,
        size.h,
    )
}

/// The words on a composite's frame: its id when the source wrote one, nothing otherwise.
fn composite_title(c: &Composite) -> &str {
    if c.named {
        c.id.as_str()
    } else {
        ""
    }
}

/// A composite's frame.
///
/// The title band is **already inside** `size` (`place` reserved it when it measured the
/// composite), so the frame is built over the whole box rather than through `band::frame`, which
/// grows a box upwards for a title that has not been paid for.
fn composite_frame(c: &Composite, center: Point, size: Size, depth: usize) -> PlacedCluster {
    let mut frame = band::frame(
        c.id.clone(),
        "",
        center.x - size.w / 2.0,
        center.y - size.h / 2.0,
        center.x + size.w / 2.0,
        center.y + size.h / 2.0,
    );
    frame.title = Label::measure(composite_title(c));
    frame.depth = depth;
    frame
}

/// Whether a block's outline says something its proportions carry.
///
/// A rectangle stretched to fill its cell is still a rectangle; a circle stretched to fill one is
/// an ellipse, which is a different shape in this grammar. So these keep the size their own label
/// asked for and are centred in the cell instead.
fn keeps_its_shape(n: &Node) -> bool {
    use crate::preview::mermaid::flowchart::Shape;
    n.arrow.is_some()
        || matches!(
            n.shape,
            Shape::Circle | Shape::DoubleCircle | Shape::Diamond | Shape::Hexagon
        )
}
