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

use crate::preview::mermaid::flowchart::Stroke;
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics::{FONT_FAMILY, FONT_SIZE};

use super::clusters;
use super::edges;
use super::edges::Tip;
use super::labels::Label;
use super::panel::Panel;
use super::shapes::{Glyph, Outline};
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

/// Dash pattern of the frame around one concurrent region of a state diagram, and of every
/// sequence-diagram group frame.
pub const CLUSTER_DASH: &str = "6 4";

/// Stroke width of a sequence diagram's lifeline. Thinner than an edge on purpose — see
/// [`Theme::lifeline`].
pub const LIFELINE_STROKE_WIDTH: f64 = 1.0;

/// Dash pattern of a lifeline.
pub const LIFELINE_DASH: &str = "3 4";

/// Half the diagonal of the cross drawn where a `destroy`ed participant's lifeline ends.
pub const DESTROY_HALF: f64 = 7.0;

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
    // Lifelines go between the frames and the messages: they run *inside* a group frame, and every
    // arrow that ends on one has to be drawn over the bar it lands on.
    if !diagram.lifelines.is_empty() {
        out.push_str("<g class=\"lifelines\">\n");
        for l in &diagram.lifelines {
            emit_lifeline(&mut out, l, theme);
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
            // The patch exists to keep the words readable where the line runs under them, so it
            // is drawn only when the line really does. A flowchart's label sits *on* the arc
            // midpoint and always needs one; a sequence diagram's sits in a band of its own above
            // the arrow and never does, and painting one there is a grey slab on a ground the
            // terminal is supposed to show through (§1).
            if label_meets_its_line(e, l) {
                out.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
                    num(l.center.x - l.size.w / 2.0),
                    num(l.center.y - l.size.h / 2.0),
                    num(l.size.w),
                    num(l.size.h),
                    theme.background_ref
                ));
            }
            emit_text(
                &mut out,
                &l.label,
                l.center.x,
                l.center.y,
                theme.edge_label_text,
            );
        }
        // A cardinality gets **no patch behind it**: it is placed clear of its own line rather
        // than on it, so the only thing a patch could hide is another part of the drawing.
        for l in [&e.start_label, &e.end_label].into_iter().flatten() {
            emit_text(
                &mut out,
                &l.label,
                l.center.x,
                l.center.y,
                theme.edge_label_text,
            );
        }
        // A sequence number sits *on* the shaft, so it gets a disc — a patch the shape of the
        // thing it hides.
        if let Some(b) = &e.badge {
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" stroke=\"{}\" \
                 stroke-width=\"{}\"/>\n",
                num(b.center.x),
                num(b.center.y),
                num(b.size.w.max(b.size.h) / 2.0),
                theme.node_fill,
                theme.node_stroke,
                num(clusters::STROKE_WIDTH)
            ));
            emit_text(&mut out, &b.label, b.center.x, b.center.y, theme.node_text);
        }
    }
    out.push_str("</g>\n");
    if diagram
        .clusters
        .iter()
        .any(|c| !c.title.is_blank() || !c.sections.is_empty())
    {
        out.push_str("<g class=\"cluster-labels\">\n");
        for c in &diagram.clusters {
            emit_cluster_title(&mut out, c, theme);
        }
        out.push_str("</g>\n");
    }
    out.push_str("</svg>\n");
    out
}

/// Whether an edge's own drawn line passes through its label's box.
///
/// Sampling the polyline rather than testing the waypoints: a label sitting on a long straight
/// segment is exactly the common case, and its two waypoints are nowhere near it.
fn label_meets_its_line(edge: &PlacedEdge, label: &super::PlacedEdgeLabel) -> bool {
    let (l, t, r, b) = (
        label.center.x - label.size.w / 2.0,
        label.center.y - label.size.h / 2.0,
        label.center.x + label.size.w / 2.0,
        label.center.y + label.size.h / 2.0,
    );
    let inside = |p: &Point| p.x >= l && p.x <= r && p.y >= t && p.y <= b;
    let points = edge.drawn_points();
    for w in points.windows(2) {
        let len = (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
        let steps = ((len / 2.0).ceil() as usize).clamp(1, 2000);
        for i in 0..=steps {
            let f = i as f64 / steps as f64;
            if inside(&Point::new(
                w[0].x + f * (w[1].x - w[0].x),
                w[0].y + f * (w[1].y - w[0].y),
            )) {
                return true;
            }
        }
    }
    false
}

/// One subgraph frame's rectangle. Its title is emitted separately, at the end — see the module
/// docs for why.
fn emit_cluster(out: &mut String, cluster: &PlacedCluster, theme: &Theme) {
    let (l, t, _, _) = cluster.bounds();
    // A dashed frame is a concurrent region of a state diagram's `--`: it has no title, so the
    // outline is the only thing that can say "this is one of several things happening at once".
    let dash = if cluster.dashed {
        format!(" stroke-dasharray=\"{CLUSTER_DASH}\"")
    } else {
        String::new()
    };
    out.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{r}\" ry=\"{r}\" \
         fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{dash}/>\n",
        num(l),
        num(t),
        num(cluster.size.w),
        num(cluster.size.h),
        if cluster.filled {
            theme.cluster_fill
        } else {
            "none"
        },
        theme.cluster_stroke,
        num(clusters::STROKE_WIDTH),
        r = num(clusters::CORNER_RADIUS)
    ));
    // A section rule spans the frame and is drawn with it, so a message inside the section below
    // is drawn over it rather than under it.
    let (_, _, right, _) = cluster.bounds();
    for section in &cluster.sections {
        out.push_str(&format!(
            "<line x1=\"{}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"{}\" \
             stroke-width=\"{}\" stroke-dasharray=\"{CLUSTER_DASH}\"/>\n",
            num(l),
            num(right),
            theme.cluster_stroke,
            num(clusters::STROKE_WIDTH),
            y = num(section.y)
        ));
    }
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
    // A section's own title sits just under its rule, centred like the frame's, and is drawn in
    // the same pass and for the same reason.
    for section in &cluster.sections {
        if section.title.is_blank() {
            continue;
        }
        emit_text(
            out,
            &section.title,
            cluster.center.x,
            section.y + clusters::TITLE_PAD_Y + section.title.height / 2.0,
            theme.cluster_text,
        );
    }
    if cluster.title.is_blank() {
        return;
    }
    let c = cluster.title_center();
    emit_text(out, &cluster.title, c.x, c.y, theme.cluster_text);
}

/// One node: its outline, then its label.
fn emit_node(out: &mut String, node: &PlacedNode, theme: &Theme) {
    let (cx, cy) = (node.center.x, node.center.y);
    // Which of the palette's three families of colour this node belongs to. A flowchart only
    // ever reaches the last arm, so its output is byte for byte what it was before state
    // diagrams existed.
    let (fill, stroke, text) = match node.shape {
        Glyph::Note => (theme.note_fill, theme.note_stroke, theme.note_text),
        Glyph::StateStart | Glyph::StateEnd | Glyph::Bar { .. } => {
            (theme.state_marker, theme.state_marker, theme.node_text)
        }
        _ => (theme.node_fill, theme.node_stroke, theme.node_text),
    };
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
        // A solid dot: the `[*]` an arrow leaves. No stroke — a filled mark with an outline in
        // the same colour just reads as a slightly bigger dot.
        Outline::Disc { r } => {
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{fill}\"/>\n",
                num(cx),
                num(cy),
                num(r)
            ));
        }
        // A ring around a solid core: the `[*]` an arrow enters. The gap between the two is what
        // tells the two markers apart at the size a terminal shows them at.
        Outline::Target { outer, inner } => {
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"none\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n",
                num(cx),
                num(cy),
                num(outer - NODE_STROKE_WIDTH / 2.0)
            ));
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{fill}\"/>\n",
                num(cx),
                num(cy),
                num(inner)
            ));
        }
        // `<<fork>>` / `<<join>>`: a solid bar across the flow.
        Outline::Bar { w, h } => {
            out.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{fill}\"/>\n",
                num(cx - w / 2.0),
                num(cy - h / 2.0),
                num(w),
                num(h)
            ));
        }
        // A note: a box with its top-right corner turned down, drawn as the outline plus the
        // little triangle that makes the fold read as a fold rather than as a chamfer.
        Outline::Note { w, h, fold } => {
            let (l, r) = (cx - w / 2.0, cx + w / 2.0);
            let (t, b) = (cy - h / 2.0, cy + h / 2.0);
            out.push_str(&format!(
                "<path d=\"M{l},{t} L{fx},{t} L{r},{fy} L{r},{b} L{l},{b} Z\" fill=\"{fill}\" \
                 stroke=\"{stroke}\" stroke-width=\"{sw}\"/>\n",
                l = num(l),
                t = num(t),
                r = num(r),
                b = num(b),
                fx = num(r - fold),
                fy = num(t + fold)
            ));
            out.push_str(&format!(
                "<path d=\"M{fx},{t} L{fx},{fy} L{r},{fy}\" fill=\"none\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n",
                t = num(t),
                r = num(r),
                fx = num(r - fold),
                fy = num(t + fold)
            ));
        }
        // A stick figure: head, spine, arms, legs. No box — that is the whole point of `actor`,
        // and the crate konoma replaces draws a second box here instead.
        Outline::Actor { w, h, figure_h } => {
            let top = cy - h / 2.0;
            let r = shapes::ACTOR_HEAD_RADIUS.min(figure_h / 4.0);
            let head_y = top + r + 1.0;
            let shoulder = head_y + r;
            let hip = top + figure_h * 0.68;
            let foot = top + figure_h - 1.0;
            let reach = (w / 2.0 - 2.0).min(figure_h * 0.30);
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"none\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n",
                num(cx),
                num(head_y),
                num(r)
            ));
            out.push_str(&format!(
                "<path d=\"M{cx},{shoulder} L{cx},{hip} M{al},{arms} L{ar},{arms} \
                 M{cx},{hip} L{ll},{foot} M{cx},{hip} L{lr},{foot}\" fill=\"none\" \
                 stroke=\"{stroke}\" stroke-width=\"{sw}\"/>\n",
                cx = num(cx),
                shoulder = num(shoulder),
                hip = num(hip),
                arms = num(shoulder + (hip - shoulder) * 0.35),
                al = num(cx - reach),
                ar = num(cx + reach),
                ll = num(cx - reach * 0.8),
                lr = num(cx + reach * 0.8),
                foot = num(foot)
            ));
        }
        Outline::None => {}
    }
    // A box that holds a table draws its table instead of a centred label.
    if let Some(panel) = &node.panel {
        emit_panel(out, panel, node, stroke, text);
        return;
    }
    // A titled box keeps its first line apart from the rest with a rule, which is what mermaid's
    // `SHAPE_STATE_WITH_DESC` does. The rule goes where the first line ends, so it is derived
    // from the label rather than declared: a box whose label grew a line moves it by itself.
    if node.shape == Glyph::TitledBox && node.label.lines.len() > 1 {
        let y = cy - node.label.height / 2.0 + super::labels::line_height();
        out.push_str(&format!(
            "<line x1=\"{}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"{stroke}\" \
             stroke-width=\"{}\"/>\n",
            num(cx - node.size.w / 2.0),
            num(cx + node.size.w / 2.0),
            num(clusters::STROKE_WIDTH),
            y = num(y)
        ));
    }
    emit_text(out, &node.label, cx, cy, text);
}

/// One participant's lifeline, and the activation bars on it.
///
/// The line is dashed, which mermaid's is not. On a terminal the lifelines are the longest strokes
/// on the page and a solid one competes with the messages for a reader's eye; dashing it says
/// "this is an axis" without needing a colour the terminal might be showing through.
///
/// The bars are drawn **after** the line and are opaque, so a bar covers the stretch of axis it
/// occupies — which is what an activation means.
fn emit_lifeline(out: &mut String, lifeline: &super::PlacedLifeline, theme: &Theme) {
    out.push_str(&format!(
        "<line x1=\"{x}\" y1=\"{}\" x2=\"{x}\" y2=\"{}\" stroke=\"{}\" \
         stroke-width=\"{}\" stroke-dasharray=\"{LIFELINE_DASH}\"/>\n",
        num(lifeline.top),
        num(lifeline.bottom),
        theme.lifeline,
        num(LIFELINE_STROKE_WIDTH),
        x = num(lifeline.x)
    ));
    for i in 0..lifeline.activations.len() {
        let Some((l, t, r, b)) = lifeline.activation_bounds(i) else {
            continue;
        };
        out.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" \
             stroke=\"{}\" stroke-width=\"{}\"/>\n",
            num(l),
            num(t),
            num(r - l),
            num(b - t),
            theme.activation_fill,
            theme.activation_stroke,
            num(clusters::STROKE_WIDTH)
        ));
    }
    // A `destroy`ed participant's line ends in a cross, which is the only thing that says the
    // participant stopped existing rather than simply stopped being talked to.
    if lifeline.destroyed {
        let d = DESTROY_HALF;
        out.push_str(&format!(
            "<path d=\"M{},{} L{},{} M{},{} L{},{}\" stroke=\"{}\" stroke-width=\"{}\" \
             fill=\"none\"/>\n",
            num(lifeline.x - d),
            num(lifeline.bottom - d),
            num(lifeline.x + d),
            num(lifeline.bottom + d),
            num(lifeline.x + d),
            num(lifeline.bottom - d),
            num(lifeline.x - d),
            num(lifeline.bottom + d),
            theme.line,
            num(EDGE_STROKE_WIDTH)
        ));
    }
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
    let (start_room, end_room) = edges::terminator_lengths(edge.tip_start, edge.tip_end);
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

    // The head end first, then the tail end. That order is not cosmetic: it is the order the
    // flowchart renderer emitted a `<-->`'s two heads in before terminators became per-end, and
    // the golden pins the sequence of elements.
    let n = points.len();
    let (tail_dir, tail_tip) = (&points[1], &points[0]);
    let (head_dir, head_tip) = (&points[n - 2], &points[n - 1]);
    emit_tip(out, head_dir, head_tip, edge.tip_end, theme);
    emit_tip(out, tail_dir, tail_tip, edge.tip_start, theme);
}

/// One end of a line, whatever kind of mark it carries.
fn emit_tip(out: &mut String, from: &Point, tip: &Point, kind: Tip, theme: &Theme) {
    match kind {
        Tip::None => {}
        Tip::Arrow => emit_arrow_head(out, from, tip, theme),
        Tip::Cross => emit_cross(out, from, tip, theme),
        Tip::Circle => emit_circle_end(out, from, tip, theme),
        Tip::Async => {
            let pts = edges::async_head(from, tip);
            out.push_str(&format!(
                "<polygon points=\"{}\" fill=\"{}\"/>\n",
                pts.iter()
                    .map(|p| format!("{},{}", num(p.x), num(p.y)))
                    .collect::<Vec<_>>()
                    .join(" "),
                theme.arrowhead
            ));
        }
        Tip::HollowTriangle => emit_polygon_tip(out, &edges::triangle(from, tip), theme, false),
        Tip::FilledDiamond => emit_polygon_tip(out, &edges::diamond(from, tip), theme, true),
        Tip::HollowDiamond => emit_polygon_tip(out, &edges::diamond(from, tip), theme, false),
        // A ring sitting *on* the boundary, which is how UML draws a provided interface.
        Tip::Lollipop => {
            let c = edges::back_along(from, tip, edges::LOLLIPOP_RADIUS);
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" stroke=\"{}\" \
                 stroke-width=\"{}\"/>\n",
                num(c.x),
                num(c.y),
                num(edges::LOLLIPOP_RADIUS),
                theme.node_fill,
                theme.arrowhead,
                num(NODE_STROKE_WIDTH)
            ));
        }
        Tip::ErOnlyOne => {
            emit_er_bar(out, from, tip, edges::ER_NEAR, theme);
            emit_er_bar(out, from, tip, edges::ER_FAR, theme);
        }
        Tip::ErZeroOrOne => {
            emit_er_bar(out, from, tip, edges::ER_NEAR, theme);
            emit_er_ring(out, from, tip, edges::ER_FAR, theme);
        }
        Tip::ErOneOrMore => {
            emit_crows_foot(out, from, tip, theme);
            emit_er_bar(out, from, tip, edges::ER_FAR, theme);
        }
        Tip::ErZeroOrMore => {
            emit_crows_foot(out, from, tip, theme);
            emit_er_ring(out, from, tip, edges::ER_FAR, theme);
        }
    }
}

/// A triangle or a diamond at the end of a line. `filled` picks between "this end owns the other"
/// (composition, a solid mark) and "this end is the general case" (inheritance and aggregation,
/// an outline the terminal's own ground shows through).
fn emit_polygon_tip(out: &mut String, points: &[Point], theme: &Theme, filled: bool) {
    let pts = points
        .iter()
        .map(|p| format!("{},{}", num(p.x), num(p.y)))
        .collect::<Vec<_>>()
        .join(" ");
    let fill = if filled {
        theme.arrowhead
    } else {
        theme.node_fill
    };
    out.push_str(&format!(
        "<polygon points=\"{pts}\" fill=\"{fill}\" stroke=\"{}\" stroke-width=\"{}\"/>\n",
        theme.arrowhead,
        num(NODE_STROKE_WIDTH)
    ));
}

/// The bar that means "one" on an ER relationship, drawn across the line.
fn emit_er_bar(out: &mut String, from: &Point, tip: &Point, distance: f64, theme: &Theme) {
    let (a, b) = edges::cross_bar(from, tip, distance, edges::ER_BAR_HALF);
    out.push_str(&format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" \
         stroke-width=\"{}\"/>\n",
        num(a.x),
        num(a.y),
        num(b.x),
        num(b.y),
        theme.arrowhead,
        num(EDGE_STROKE_WIDTH)
    ));
}

/// The ring that means "zero" on an ER relationship. Filled with the node colour rather than left
/// open, so the shaft it sits on does not run through the middle of it.
fn emit_er_ring(out: &mut String, from: &Point, tip: &Point, distance: f64, theme: &Theme) {
    let c = edges::back_along(from, tip, distance);
    out.push_str(&format!(
        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" stroke=\"{}\" \
         stroke-width=\"{}\"/>\n",
        num(c.x),
        num(c.y),
        num(edges::ER_RING_RADIUS),
        theme.node_fill,
        theme.arrowhead,
        num(EDGE_STROKE_WIDTH)
    ));
}

/// The crow's foot that means "many": three prongs opening onto the entity.
fn emit_crows_foot(out: &mut String, from: &Point, tip: &Point, theme: &Theme) {
    let mut d = String::new();
    for (a, b) in edges::crows_foot(from, tip) {
        d.push_str(&format!(
            "M{},{} L{},{} ",
            num(a.x),
            num(a.y),
            num(b.x),
            num(b.y)
        ));
    }
    out.push_str(&format!(
        "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"/>\n",
        d.trim_end(),
        theme.arrowhead,
        num(EDGE_STROKE_WIDTH)
    ));
}

/// The compartments inside a class box or an entity box.
///
/// Everything here is a translation of numbers [`super::panel`] already worked out, in the order
/// rules-then-text so a rule never lands on top of a word.
fn emit_panel(out: &mut String, panel: &Panel, node: &PlacedNode, stroke: &str, text: &str) {
    let (left, top, right, bottom) = node.bounds();
    for y in &panel.rules {
        out.push_str(&format!(
            "<line x1=\"{}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"{stroke}\" \
             stroke-width=\"{}\"/>\n",
            num(left),
            num(right),
            num(clusters::STROKE_WIDTH),
            y = num(top + y)
        ));
    }
    // A column rule starts under the name band — the first horizontal rule — because above it
    // there is one centred name, not a row of cells.
    let column_top = panel.rules.first().map_or(top, |y| top + y);
    for x in &panel.columns {
        out.push_str(&format!(
            "<line x1=\"{x}\" y1=\"{}\" x2=\"{x}\" y2=\"{}\" stroke=\"{stroke}\" \
             stroke-width=\"{}\"/>\n",
            num(column_top),
            num(bottom),
            num(clusters::STROKE_WIDTH),
            x = num(left + x)
        ));
    }
    for row in &panel.rows {
        for cell in &row.cells {
            emit_line_of_text(
                out,
                &cell.label,
                left + cell.x,
                top + row.y,
                text,
                cell.centered,
            );
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
/// One row of a panel: a single line, anchored at its centre or at its left edge.
///
/// `text-anchor="start"` is the whole reason a class member reads as a list rather than as a
/// column of centred fragments — `+String owner` over `+deposit(amount)` only lines up if both
/// start at the same x.
fn emit_line_of_text(out: &mut String, label: &Label, x: f64, cy: f64, fill: &str, centered: bool) {
    if label.is_blank() {
        return;
    }
    out.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"{}\" font-family=\"{}\" font-size=\"{}\" \
         fill=\"{}\">{}</text>\n",
        num(x),
        num(cy + FONT_SIZE as f64 * super::labels::BASELINE_RATIO),
        if centered { "middle" } else { "start" },
        FONT_FAMILY,
        num(FONT_SIZE as f64),
        fill,
        escape(&label.lines.join(" "))
    ));
}

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
