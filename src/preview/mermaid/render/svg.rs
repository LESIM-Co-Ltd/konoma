//! Turning a [`Diagram`] into SVG text.
//!
//! The constraints come from `docs/FEATURE-MERMAID-RENDERER.md` §1, and each of them is something
//! resvg was measured doing rather than something the spec implies:
//!
//! * **`width`/`height` in px plus a `viewBox`.** Without a viewBox usvg falls back to 100×100.
//! * **No opaque background.** konoma composites the diagram onto the terminal, so a background
//!   rect turns every diagram into a white (or grey) card that ignores the user's theme.
//! * **No `<foreignObject>`.** resvg drops the element whole, taking the label with it. Every
//!   label here is a `<text>` with `<tspan>` children.
//! * **No `var(--x)`.** usvg does not resolve CSS custom properties: a `fill` becomes black and a
//!   `stroke` disappears. Every colour is written out as a literal hex.
//! * **One font family, the one that was measured with.** `text_metrics::FONT_FAMILY` is
//!   `sans-serif` because that is one of the five generics usvg understands; a stack containing
//!   `system-ui` or `-apple-system` is searched as *named* families and, when they all miss, usvg
//!   quietly falls back to Serif — a face nothing measured.
//!
//! Drawing order is frames, then edges, then nodes, then edge labels, then frame titles. Nodes
//! cover the lines that run under them and labels cover the line they belong to, which is what
//! makes a label readable without an opaque patch big enough to hide the diagram.
//!
//! The frames are split across that order on purpose. Their rectangles go **first**, behind
//! everything, so a frame reads as ground rather than as another box; mermaid does the same. Their
//! titles go **last**, which mermaid does not — see [`emit_cluster_title`].

use crate::preview::mermaid::flowchart::{Arrow, Stroke};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics::{FONT_FAMILY, FONT_SIZE};

use super::clusters;
use super::edges;
use super::labels::Label;
use super::shapes::Outline;
use super::theme::Theme;
use super::{shapes, Diagram, PlacedCluster, PlacedEdge, PlacedNode};

/// Stroke width of a node's outline.
pub const NODE_STROKE_WIDTH: f64 = 1.5;

/// Stroke width of an ordinary edge.
pub const EDGE_STROKE_WIDTH: f64 = 2.0;

/// Stroke width of a `===` edge.
pub const THICK_STROKE_WIDTH: f64 = 3.5;

/// Dash pattern of a `-.-` edge.
pub const DOTTED_DASH: &str = "4 4";

/// A number as SVG writes it: at most three decimals, no trailing zeros, no negative zero.
///
/// Three decimals is well under a rasterised pixel at any zoom konoma reaches (`SvgReraster` caps
/// at 4096px on the long side) and keeps the golden files stable against the last bits of a f64.
pub fn num(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    let s = format!("{r}");
    if s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

/// XML-escapes text and attribute content.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Renders a laid-out diagram.
pub fn emit(diagram: &Diagram, theme: &Theme) -> String {
    let mut out = String::with_capacity(1024 + diagram.nodes.len() * 256);
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">\n",
        w = num(diagram.width),
        h = num(diagram.height)
    ));
    // §4-2: the background is a colour the palette derives from, never a paint. Emitted so the
    // document says out loud that it is transparent rather than leaving it to a default.
    out.push_str(&format!(
        "<rect width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
        num(diagram.width),
        num(diagram.height),
        theme.background_paint
    ));

    // The frame groups are left out entirely when a chart has no blocks, rather than emitted
    // empty. That is what lets the corpus golden say something worth saying: a diagram without a
    // `subgraph` comes out of stage 1d byte for byte as it came out of stage 1c.
    if !diagram.clusters.is_empty() {
        out.push_str("<g class=\"clusters\">\n");
        for c in &diagram.clusters {
            emit_cluster(&mut out, c, theme);
        }
        out.push_str("</g>\n");
    }
    out.push_str("<g class=\"edges\">\n");
    for e in &diagram.edges {
        emit_edge(&mut out, e, theme);
    }
    out.push_str("</g>\n<g class=\"nodes\">\n");
    for n in &diagram.nodes {
        emit_node(&mut out, n, theme);
    }
    out.push_str("</g>\n<g class=\"edge-labels\">\n");
    for e in &diagram.edges {
        if let Some(l) = &e.label {
            out.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
                num(l.center.x - l.size.w / 2.0),
                num(l.center.y - l.size.h / 2.0),
                num(l.size.w),
                num(l.size.h),
                theme.background_ref
            ));
            emit_text(
                &mut out,
                &l.label,
                l.center.x,
                l.center.y,
                theme.edge_label_text,
            );
        }
    }
    out.push_str("</g>\n");
    if diagram.clusters.iter().any(|c| !c.title.is_blank()) {
        out.push_str("<g class=\"cluster-labels\">\n");
        for c in &diagram.clusters {
            emit_cluster_title(&mut out, c, theme);
        }
        out.push_str("</g>\n");
    }
    out.push_str("</svg>\n");
    out
}

/// One subgraph frame's rectangle. Its title is emitted separately, at the end — see the module
/// docs for why.
fn emit_cluster(out: &mut String, cluster: &PlacedCluster, theme: &Theme) {
    let (l, t, _, _) = cluster.bounds();
    out.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{r}\" ry=\"{r}\" \
         fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>\n",
        num(l),
        num(t),
        num(cluster.size.w),
        num(cluster.size.h),
        theme.cluster_fill,
        theme.cluster_stroke,
        num(clusters::STROKE_WIDTH),
        r = num(clusters::CORNER_RADIUS)
    ));
}

/// One subgraph title, drawn over everything else and with **nothing behind it**.
///
/// Both halves of that were decided by looking at the result rather than reasoned about, because
/// a line entering a block from above crosses the frame's top edge exactly where the title sits:
///
/// * drawn *under* the edges (mermaid's order) the line runs over the words;
/// * drawn over the edges on a patch of the frame's fill — the treatment an edge label gets — the
///   words are clean but the line is **cut in two**, and the arrow head below the title reads as
///   belonging to nothing;
/// * drawn over the edges with no patch, the line stays whole and the title is still legible,
///   because a stroke crossing a word costs far less than a word costs a stroke.
fn emit_cluster_title(out: &mut String, cluster: &PlacedCluster, theme: &Theme) {
    if cluster.title.is_blank() {
        return;
    }
    let c = cluster.title_center();
    emit_text(out, &cluster.title, c.x, c.y, theme.cluster_text);
}

/// One node: its outline, then its label.
fn emit_node(out: &mut String, node: &PlacedNode, theme: &Theme) {
    let (cx, cy) = (node.center.x, node.center.y);
    let fill = theme.node_fill;
    let stroke = theme.node_stroke;
    let sw = num(NODE_STROKE_WIDTH);
    match shapes::outline(node.shape, node.size) {
        Outline::Rect { w, h, r } => {
            out.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" ry=\"{}\" \
                 fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{sw}\"/>\n",
                num(cx - w / 2.0),
                num(cy - h / 2.0),
                num(w),
                num(h),
                num(r),
                num(r)
            ));
        }
        Outline::Circle { r } => {
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{fill}\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n",
                num(cx),
                num(cy),
                num(r)
            ));
        }
        Outline::DoubleCircle { outer, inner } => {
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{fill}\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n",
                num(cx),
                num(cy),
                num(outer)
            ));
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"none\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n",
                num(cx),
                num(cy),
                num(inner)
            ));
        }
        Outline::Subroutine { w, h, inset } => {
            out.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{fill}\" \
                 stroke=\"{stroke}\" stroke-width=\"{sw}\"/>\n",
                num(cx - w / 2.0),
                num(cy - h / 2.0),
                num(w),
                num(h)
            ));
            for side in [-1.0_f64, 1.0] {
                let x = cx + side * (w / 2.0 - inset);
                out.push_str(&format!(
                    "<line x1=\"{x}\" y1=\"{y0}\" x2=\"{x}\" y2=\"{y1}\" stroke=\"{stroke}\" \
                     stroke-width=\"{sw}\"/>\n",
                    x = num(x),
                    y0 = num(cy - h / 2.0),
                    y1 = num(cy + h / 2.0)
                ));
            }
        }
        Outline::Cylinder { w, h, ry } => {
            let (l, r) = (cx - w / 2.0, cx + w / 2.0);
            let (y0, y1) = (cy - h / 2.0 + ry, cy + h / 2.0 - ry);
            let rx = w / 2.0;
            out.push_str(&format!(
                "<path d=\"M{l},{y0} A{rx},{ry} 0 0 1 {r},{y0} L{r},{y1} A{rx},{ry} 0 0 1 \
                 {l},{y1} Z\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{sw}\"/>\n",
                l = num(l),
                r = num(r),
                y0 = num(y0),
                y1 = num(y1),
                rx = num(rx),
                ry = num(ry)
            ));
            out.push_str(&format!(
                "<path d=\"M{l},{y0} A{rx},{ry} 0 0 0 {r},{y0}\" fill=\"none\" \
                 stroke=\"{stroke}\" stroke-width=\"{sw}\"/>\n",
                l = num(l),
                r = num(r),
                y0 = num(y0),
                rx = num(rx),
                ry = num(ry)
            ));
        }
        Outline::Polygon(points) => {
            let pts = points
                .iter()
                .map(|p| format!("{},{}", num(cx + p.x), num(cy + p.y)))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!(
                "<polygon points=\"{pts}\" fill=\"{fill}\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n"
            ));
        }
        Outline::None => {}
    }
    emit_text(out, &node.label, cx, cy, theme.node_text);
}

/// One edge: the curve, then whatever sits at its ends.
fn emit_edge(out: &mut String, edge: &PlacedEdge, theme: &Theme) {
    // `~~~` is a layout constraint that draws nothing at all.
    if edge.stroke == Stroke::Invisible {
        return;
    }
    let points = edge.drawn_points();
    if points.len() < 2 {
        return;
    }
    let (start_room, end_room) = edges::terminator_lengths(edge.arrow);
    let trimmed = edges::trim_start(&edges::trim_end(&points, end_room), start_room);

    let (width, dash) = match edge.stroke {
        Stroke::Thick => (THICK_STROKE_WIDTH, None),
        Stroke::Dotted => (EDGE_STROKE_WIDTH, Some(DOTTED_DASH)),
        // A `x-- text -->` whose two halves disagree is `INVALID` in mermaid; it still has a line.
        Stroke::Normal | Stroke::Invalid => (EDGE_STROKE_WIDTH, None),
        Stroke::Invisible => return,
    };
    out.push_str(&format!(
        "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"{}/>\n",
        edges::curve_basis_path(&trimmed),
        theme.line,
        num(width),
        dash.map_or(String::new(), |d| format!(" stroke-dasharray=\"{d}\""))
    ));

    let n = points.len();
    let (tail_dir, tail_tip) = (&points[1], &points[0]);
    let (head_dir, head_tip) = (&points[n - 2], &points[n - 1]);
    match edge.arrow {
        Arrow::None | Arrow::Invalid => {}
        Arrow::Point => emit_arrow_head(out, head_dir, head_tip, theme),
        Arrow::Cross => emit_cross(out, head_dir, head_tip, theme),
        Arrow::Circle => emit_circle_end(out, head_dir, head_tip, theme),
        Arrow::DoublePoint => {
            emit_arrow_head(out, head_dir, head_tip, theme);
            emit_arrow_head(out, tail_dir, tail_tip, theme);
        }
        Arrow::DoubleCross => {
            emit_cross(out, head_dir, head_tip, theme);
            emit_cross(out, tail_dir, tail_tip, theme);
        }
        Arrow::DoubleCircle => {
            emit_circle_end(out, head_dir, head_tip, theme);
            emit_circle_end(out, tail_dir, tail_tip, theme);
        }
    }
}

/// A filled triangle whose tip is exactly on the shape's boundary.
fn emit_arrow_head(out: &mut String, from: &Point, tip: &Point, theme: &Theme) {
    let pts = edges::arrow_head(from, tip);
    out.push_str(&format!(
        "<polygon points=\"{}\" fill=\"{}\"/>\n",
        pts.iter()
            .map(|p| format!("{},{}", num(p.x), num(p.y)))
            .collect::<Vec<_>>()
            .join(" "),
        theme.arrowhead
    ));
}

/// The `--x` terminator: two strokes crossing just short of the boundary point.
///
/// Set back by its own half-diagonal rather than centred on the boundary. Nodes are drawn over
/// the edges (so a line running under a box disappears under it), and a cross centred on the
/// boundary loses its inner half to the node's fill — which reads as a `>`, not an `x`. Seen on a
/// real render, not reasoned about.
fn emit_cross(out: &mut String, from: &Point, tip: &Point, theme: &Theme) {
    let d = edges::CROSS_HALF;
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = dx.hypot(dy);
    let (ux, uy) = if len < 1e-9 {
        (1.0, 0.0)
    } else {
        (dx / len, dy / len)
    };
    let (cx, cy) = (tip.x - ux * d, tip.y - uy * d);
    out.push_str(&format!(
        "<path d=\"M{},{} L{},{} M{},{} L{},{}\" stroke=\"{}\" stroke-width=\"{}\" \
         fill=\"none\"/>\n",
        num(cx - d),
        num(cy - d),
        num(cx + d),
        num(cy + d),
        num(cx + d),
        num(cy - d),
        num(cx - d),
        num(cy + d),
        theme.arrowhead,
        num(EDGE_STROKE_WIDTH)
    ));
}

/// The `--o` terminator: a filled disc sitting just inside the boundary point.
fn emit_circle_end(out: &mut String, from: &Point, tip: &Point, theme: &Theme) {
    let r = edges::CIRCLE_RADIUS;
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = dx.hypot(dy);
    let (ux, uy) = if len < 1e-9 {
        (1.0, 0.0)
    } else {
        (dx / len, dy / len)
    };
    out.push_str(&format!(
        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>\n",
        num(tip.x - ux * r),
        num(tip.y - uy * r),
        num(r),
        theme.arrowhead
    ));
}

/// A label, centred on `(cx, cy)`, one `<tspan>` per line.
///
/// `text-anchor="middle"` does the horizontal centring, so the emitted markup does not depend on
/// konoma and resvg agreeing about the text's width — but the box around it does, which is what
/// the measurement instrument in `tests` checks.
fn emit_text(out: &mut String, label: &Label, cx: f64, cy: f64, fill: &str) {
    if label.is_blank() {
        return;
    }
    out.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-family=\"{}\" font-size=\"{}\" \
         fill=\"{}\">",
        num(cx),
        num(label.baseline(cy, 0)),
        FONT_FAMILY,
        num(FONT_SIZE as f64),
        fill
    ));
    for (i, line) in label.lines.iter().enumerate() {
        let dy = if i == 0 {
            0.0
        } else {
            super::labels::line_height()
        };
        out.push_str(&format!(
            "<tspan x=\"{}\" dy=\"{}\">{}</tspan>",
            num(cx),
            num(dy),
            escape(line)
        ));
    }
    out.push_str("</text>\n");
}
