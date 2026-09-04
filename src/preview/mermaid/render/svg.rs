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
//! Drawing order is frames, then edges, then nodes, then **data paths**, then edge labels, then
//! frame titles. Nodes cover the lines that run under them and labels cover the line they belong
//! to, which is what makes a label readable without an opaque patch big enough to hide the
//! diagram.
//!
//! A data path is the one exception, and it is one an edge declares for itself
//! ([`PlacedEdge::overlay`]): an xy chart's plot drawn under the bars disappears behind the tall
//! ones. Reading that off an edge's `series` instead — which is a *colour* — put a git graph's
//! branch lines over its commit ids and struck the words through.
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

/// How opaque a Sankey ribbon is.
///
/// Flows cross, and an opaque band hides whatever runs under it — which for a Sankey is exactly
/// the comparison the diagram exists to make. Low enough that two crossing bands read as two, high
/// enough that one band on the terminal's own ground still reads as a band.
pub const RIBBON_OPACITY: f64 = 0.45;

/// The path of a pie slice of radius `r`, from `start` to `start + sweep` degrees clockwise from
/// twelve o'clock.
///
/// A full turn is drawn as a circle rather than as an arc: SVG's elliptical arc command cannot
/// express 360°, because its start and end points coincide and the renderer draws nothing at all.
/// A one-slice pie is the commonest chart there is, so this is not a corner case.
pub fn wedge_path(cx: f64, cy: f64, r: f64, start: f64, sweep: f64) -> String {
    let sweep = sweep.clamp(0.0, 360.0);
    if sweep >= 360.0 - 1e-9 {
        return format!(
            "M{cx},{top} A{r},{r} 0 1 1 {cx2},{bottom} A{r},{r} 0 1 1 {cx3},{top2} Z",
            cx = num(cx),
            cx2 = num(cx),
            cx3 = num(cx),
            top = num(cy - r),
            top2 = num(cy - r),
            bottom = num(cy + r),
            r = num(r)
        );
    }
    let at = |deg: f64| {
        let a = (deg - 90.0).to_radians();
        (cx + r * a.cos(), cy + r * a.sin())
    };
    let (x0, y0) = at(start);
    let (x1, y1) = at(start + sweep);
    let large = usize::from(sweep > 180.0);
    format!(
        "M{},{} L{},{} A{r},{r} 0 {large} 1 {},{} Z",
        num(cx),
        num(cy),
        num(x0),
        num(y0),
        num(x1),
        num(y1),
        r = num(r)
    )
}

/// The vertices of a regular `sides`-gon of radius `r`, first vertex at twelve o'clock.
fn radial_polygon(cx: f64, cy: f64, r: f64, sides: usize) -> String {
    (0..sides)
        .map(|k| {
            let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * k as f64 / sides as f64;
            format!("{},{}", num(cx + r * a.cos()), num(cy + r * a.sin()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

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
    // Every edge that is *not* a data path. That is every edge in every diagram kind before
    // stage 5, so the group is byte for byte what it was; a chart's axes, ticks and grid lines
    // join it, and they belong under the marks for the same reason a flowchart's lines belong
    // under its boxes.
    out.push_str("<g class=\"edges\">\n");
    for e in diagram.edges.iter().filter(|e| !e.overlay) {
        emit_edge(&mut out, e, theme);
    }
    out.push_str("</g>\n<g class=\"nodes\">\n");
    for n in &diagram.nodes {
        emit_node(&mut out, n, theme);
    }
    out.push_str("</g>\n");
    // A data path goes **over** the marks; every other line goes under them. Which of the two a
    // line is is asked of the line itself ([`PlacedEdge::overlay`]) rather than read off its
    // `series`, because `series` says what colour it takes and nothing about depth — a git graph's
    // lane line carries one, and inferring depth from it drew the lane over the commit ids and
    // struck the words through. Omitted entirely when there is no data path, so a diagram that has
    // none is unchanged.
    if diagram.edges.iter().any(|e| e.overlay) {
        out.push_str("<g class=\"series\">\n");
        for e in diagram.edges.iter().filter(|e| e.overlay) {
            emit_edge(&mut out, e, theme);
        }
        out.push_str("</g>\n");
    }
    out.push_str("<g class=\"edge-labels\">\n");
    for e in &diagram.edges {
        if let Some(l) = &e.label {
            // A palette carrying `Tokens` draws an edge label smaller than the body text it
            // labels, and the patch shrinks with it (`Label::resized`'s own doc: the layout's box
            // is an input to the routing and stays where it is; what has to match is the words and
            // the patch behind them).
            let resized = theme
                .tokens
                .map(|t| l.label.resized(t.edge_label_font_size));
            let drawn = resized.as_ref().unwrap_or(&l.label);
            // Without `Tokens` the patch is exactly the box the layout placed — the same rectangle
            // every diagram konoma has ever drawn, including the hand-built ones the emit golden
            // pins, whose `size` is not derived from the label at all.
            let patch = match &resized {
                Some(d) => super::Size::new(
                    d.width + super::LABEL_PAD_X * 2.0,
                    d.height + super::LABEL_PAD_Y * 2.0,
                ),
                None => l.size,
            };
            // The patch exists to keep the words readable where the line runs under them, so it
            // is drawn only when the line really does. A flowchart's label sits *on* the arc
            // midpoint and always needs one; a sequence diagram's sits in a band of its own above
            // the arrow and never does, and painting one there is a grey slab on a ground the
            // terminal is supposed to show through (§1).
            if label_meets_its_line(e, l) {
                out.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
                    num(l.center.x - patch.w / 2.0),
                    num(l.center.y - patch.h / 2.0),
                    num(patch.w),
                    num(patch.h),
                    theme.background_ref
                ));
            }
            // `class`/`:::`/`linkStyle`'s own `color:` (or, under `konoma-orthogonal`, §10-3 item
            // 6's derived colour — `mod.rs`'s own doc on filling `style.text` from the downstream
            // node's lightened stroke) wins over the theme default, the same "most specific
            // instruction wins" rule `emit_node`/`emit_edge`'s own line colour already follow — this
            // was the one place in the cascade `edge.style` reached every field but this one.
            // A palette carrying `Tokens` extends that one step further (`ink_follows_line`): a
            // line the source coloured with a `linkStyle`/`class` but gave no `color:` to draws
            // its words in the line's own colour, under either routing — the reference's
            // "辺・矢尻・ラベル文字…を同色で揃える", stated as a property of the palette rather than
            // of the router the way `tip_matches_line` states it.
            let label_color = e
                .style
                .as_ref()
                .and_then(|s| {
                    s.text.as_deref().or_else(|| {
                        theme
                            .tokens
                            .filter(|t| t.ink_follows_line)
                            .and(s.stroke.as_deref())
                    })
                })
                .unwrap_or(theme.edge_label_text);
            emit_text(&mut out, drawn, l.center.x, l.center.y, label_color);
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

/// Outline colour of one frame.
///
/// A composite state's frame (`title_strip`) and a flowchart `subgraph`'s are the same colour in
/// every palette that carries no [`super::theme::Tokens`] — which is every `[ui] mermaid_theme`
/// value — and two different greys in the one that does: the reference draws a subgraph as a faint
/// dashed outline and a composite state as a slightly brighter solid one, so that a box holding a
/// *state machine* does not read as the same kind of thing as a box grouping some nodes.
fn frame_stroke(cluster: &PlacedCluster, theme: &Theme) -> &'static str {
    match theme.tokens {
        Some(t) if cluster.title_strip => t.composite_stroke,
        _ => theme.cluster_stroke,
    }
}

/// One subgraph frame's rectangle. Its title is emitted separately, at the end — see the module
/// docs for why.
fn emit_cluster(out: &mut String, cluster: &PlacedCluster, theme: &Theme) {
    let (l, t, _, _) = cluster.bounds();
    // A dashed frame is a concurrent region of a state diagram's `--`: it has no title, so the
    // outline is the only thing that can say "this is one of several things happening at once".
    //
    // A palette carrying `Tokens` dashes an ordinary `subgraph` frame too, and in its own pattern:
    // the reference draws a subgraph as a dashed outline with nothing behind it, so the dash is
    // the only thing left saying "this is a frame". A composite state's own frame (`title_strip`)
    // stays solid there — its strip says it instead.
    let dash_pattern = theme.tokens.map_or(CLUSTER_DASH, |t| t.cluster_dash);
    let dashed = cluster.dashed || (theme.tokens.is_some() && !cluster.title_strip);
    let dash = if dashed {
        format!(" stroke-dasharray=\"{dash_pattern}\"")
    } else {
        String::new()
    };
    // §10-5 S2: a composite state's frame under `konoma-orthogonal` is never filled — the title
    // strip below carries the only fill this frame gets, and `read_clusters`'s own generic
    // construction leaves `filled: true` unconditionally (it serves a flowchart subgraph too, and
    // has no way to tell the two apart), so this is a drawing-time override rather than a change
    // to what `read_clusters` builds — the same "layout untouched, only the picture changes" split
    // the rest of `konoma-orthogonal` keeps.
    let filled =
        cluster.filled && !cluster.title_strip && theme.tokens.is_none_or(|t| t.cluster_filled);
    out.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{r}\" ry=\"{r}\" \
         fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{dash}/>\n",
        num(l),
        num(t),
        num(cluster.size.w),
        num(cluster.size.h),
        if filled { theme.cluster_fill } else { "none" },
        frame_stroke(cluster, theme),
        num(clusters::STROKE_WIDTH),
        r = num(theme
            .tokens
            .map_or(clusters::CORNER_RADIUS, |t| t.frame_radius))
    ));
    // §10-5 S2's own title strip: a filled band across the frame's own top edge, down to exactly
    // where `PlacedCluster::title_center`'s own formula already reserved room for the title
    // (`top + TITLE_PAD_Y*2 + title.height` — read off the same two quantities `title_center`
    // uses, not a fixed 24px, so the strip always matches whatever room the shared frame-growth
    // machinery actually reserved rather than risking a mismatch against it), with a 1px rule
    // dividing it from the body below. Skipped for a blank title (an untitled concurrent region,
    // `dashed` already says what that is) — there is no room reserved for one to draw a strip in.
    if cluster.title_strip && !cluster.title.is_blank() {
        let (_, _, right, _) = cluster.bounds();
        let strip_bottom = t + clusters::TITLE_PAD_Y * 2.0 + cluster.title.height;
        out.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
            num(l),
            num(t),
            num(right - l),
            num(strip_bottom - t),
            // `theme.node_fill`, not `cluster_fill`: the design reference's own token list
            // (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 5) gives the strip the identical
            // "ノード塗り" value, not the frame's own (different) fill — the strip is meant to
            // read as a small header bar, the same weight as a node, not as a tinted patch of the
            // frame's own interior.
            theme.node_fill
        ));
        out.push_str(&format!(
            "<line x1=\"{}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"{}\" stroke-width=\"1\"/>\n",
            num(l),
            num(right),
            theme
                .tokens
                .map_or(theme.cluster_stroke, |t| t.composite_rule),
            y = num(strip_bottom)
        ));
    }
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
    if cluster.title_strip {
        // §10-5 S2: left-aligned inside the strip `emit_cluster` already filled, at the same
        // 16px inset `clusters::TITLE_PAD_X` gives a subgraph's own left margin (reused rather
        // than a new constant — it is exactly "how far a frame's contents sit from its own left
        // edge" already, which is what a strip title's left inset means too).
        let (l, t, _, _) = cluster.bounds();
        let strip_bottom = t + clusters::TITLE_PAD_Y * 2.0 + cluster.title.height;
        emit_anchored_title(
            out,
            &cluster.title,
            l + clusters::TITLE_PAD_X,
            (t + strip_bottom) / 2.0,
            theme
                .tokens
                .map_or(theme.cluster_text, |t| t.composite_text),
            STRIP_FONT_SIZE,
        );
        return;
    }
    // A palette with `Tokens` puts a `subgraph`'s title in the frame's own top-left corner, small
    // and quiet, instead of centred on the top edge — inside the very band `rebuild_frames` /
    // `fit_titles` already reserved for it (`title.height + 2 * TITLE_PAD_Y` deep, at least
    // `title.width + TITLE_PAD_X` wide), so a smaller, left-anchored rendering of the same words
    // cannot reach a member.
    if let Some(tokens) = theme.tokens.filter(|t| t.title_left_aligned) {
        let (l, t, _, _) = cluster.bounds();
        emit_anchored_title(
            out,
            &cluster.title,
            l + clusters::TITLE_PAD_X,
            t + clusters::TITLE_PAD_Y + cluster.title.height / 2.0,
            theme.cluster_text,
            tokens.title_font_size,
        );
        return;
    }
    let c = cluster.title_center();
    emit_text(out, &cluster.title, c.x, c.y, theme.cluster_text);
}

/// §10-5 S2's own strip title size, and the size the design reference draws a composite state's
/// title at (`docs/render-check/zz-design-4c-wrap.html`: `font-size="11"`).
const STRIP_FONT_SIZE: f64 = 11.0;

/// A frame's title, left-anchored and drawn smaller than the body: §10-5 S2's composite-state
/// strip, and — for a palette carrying [`super::theme::Tokens`] — an ordinary subgraph's title
/// too. `docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 5 gives both a size a point or two under the
/// body's, the same relationship a node's label and an edge's label already have.
///
/// Drawn at that smaller size purely as a rendering choice: the `Label` this reads
/// (`cluster.title`) was measured at the ordinary body size (`read_clusters`'s own
/// `Label::measure`, shared with an ordinary subgraph — §10-5's own "枠のポート/退避則/拡大則は
/// 通常ノードと完全に同一" keeps the frame's own width/height growth untouched), so the band this
/// sits inside is always at least as roomy as this smaller rendering needs, never tighter.
fn emit_anchored_title(
    out: &mut String,
    label: &Label,
    x: f64,
    cy: f64,
    fill: &str,
    font_size: f64,
) {
    if label.is_blank() {
        return;
    }
    out.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"start\" font-family=\"{}\" font-size=\"{}\" \
         fill=\"{}\">{}</text>\n",
        num(x),
        num(cy + font_size * super::labels::BASELINE_RATIO),
        FONT_FAMILY,
        num(font_size),
        fill,
        escape(&label.lines.join(" "))
    ));
}

/// Corner radius of one rectangular node, `shaped` being what [`shapes::outline`] worked out.
///
/// A palette carrying [`super::theme::Tokens`] draws **an ordinary box** — a flowchart's `A[…]`
/// and `A(…)`, and a state diagram's own state boxes — at one radius of its own, which is what
/// makes every box in the reference drawings read as coming from one hand. Deliberately keyed off
/// the node's *glyph* rather than off the radius `shapes` chose, so the two shapes that are square
/// on purpose stay square: a class box and an ER entity, whose sharp corner is the only thing
/// telling a compartmented box from a subgraph frame at a glance. A stadium (radius `h/2`) and a
/// circle are not `Outline::Rect` in the first place and never reach here.
fn node_radius(node: &PlacedNode, theme: &Theme, shaped: f64) -> f64 {
    let ordinary_box = matches!(
        node.shape,
        Glyph::Flow(crate::preview::mermaid::flowchart::Shape::Rect)
            | Glyph::Flow(crate::preview::mermaid::flowchart::Shape::RoundedRect)
            | Glyph::TitledBox
    );
    match theme.tokens {
        Some(t) if ordinary_box => t.node_radius,
        _ => shaped,
    }
}

/// One node: its outline, then its label.
fn emit_node(out: &mut String, node: &PlacedNode, theme: &Theme) {
    let (cx, cy) = (node.center.x, node.center.y);
    // Which of the palette's three families of colour this node belongs to. A flowchart only
    // ever reaches the last arm, so its output is byte for byte what it was before state
    // diagrams existed.
    let (mut fill, mut stroke, mut text) = match node.shape {
        Glyph::Note => (theme.note_fill, theme.note_stroke, theme.note_text),
        Glyph::StateStart | Glyph::StateEnd | Glyph::Bar { .. } => {
            (theme.state_marker, theme.state_marker, theme.node_text)
        }
        // A chart's furniture: an outline in the axis colour with nothing behind it, because §1
        // forbids painting over the terminal.
        Glyph::PlotFrame | Glyph::Graticule => ("none", theme.axis, theme.node_text),
        // Words and nothing else. The colour depends on what they are drawn *on*: a tick label
        // sits on the terminal, a percentage sits on its own wedge.
        Glyph::ChartLabel => (
            "none",
            "none",
            if node.series.is_some() {
                theme.series_text
            } else {
                theme.node_text
            },
        ),
        // A datum. Its fill is its series, and the text on it is the one colour every series
        // colour was chosen to carry.
        Glyph::Wedge | Glyph::ChartBar | Glyph::ChartPoint | Glyph::Ribbon => (
            node.series.map_or(theme.node_fill, |i| theme.series(i)),
            theme.node_stroke,
            theme.series_text,
        ),
        // …and the general rule the four above are the special cases of: **a node that carries a
        // series is drawn in that series' colour**, whatever its outline. That is what makes a
        // timeline's events, a kanban board's cards, a journey's faces and a git graph's commits
        // tell their group apart at a glance — and it is why a `series` on a node is never
        // decoration: it is the one thing that says which group the node belongs to.
        //
        // Nothing that existed before stage 5b reaches this arm: every node with a series in the
        // four graph languages and the seven charts is one of the four kinds above, which the
        // checked-in goldens confirm by not moving.
        _ if node.series.is_some() => (
            node.series.map_or(theme.node_fill, |i| theme.series(i)),
            theme.node_stroke,
            theme.series_text,
        ),
        _ => (theme.node_fill, theme.node_stroke, theme.node_text),
    };
    // `classDef` / `class` / `:::` / `style` win over every rule above, theme and series alike —
    // they are the most specific instruction the source gave, and konoma's SVG carries no
    // stylesheet to express that any other way (`style::cascade`'s module docs).
    let mut sw = NODE_STROKE_WIDTH;
    // `tint_class_fill`: a source that colour-codes a node by its *outline* alone still gets the
    // reference's "薄い塗り" behind it, derived from that outline (`style::tint`). Only when the
    // source named no `fill:` of its own — an explicit fill is the more specific instruction, and
    // that is what the reference's own `2a` writes.
    let tinted: Option<String> = match (&node.style, theme.tokens) {
        (Some(s), Some(t)) if t.tint_class_fill && s.fill.is_none() => {
            s.stroke.as_deref().and_then(super::style::tint)
        }
        _ => None,
    };
    if let Some(v) = &tinted {
        fill = v.as_str();
    }
    if let Some(s) = &node.style {
        if let Some(v) = s.fill.as_deref() {
            fill = v;
        }
        if let Some(v) = s.stroke.as_deref() {
            stroke = v;
        }
        if let Some(v) = s.text.as_deref() {
            text = v;
        }
        if let Some(v) = s.stroke_width {
            sw = v;
        }
    }
    let sw = num(sw);
    match shapes::outline(node.shape, node.size, node.mark) {
        Outline::Rect { w, h, r } => {
            let r = node_radius(node, theme, r);
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
        // A pie slice. Drawn as an explicit path rather than as a stroked arc so that the
        // straight edges are part of the same shape as the curve — an arc with two separate
        // radii leaves a seam at every boundary once the terminal shows through the joins.
        Outline::Wedge { r, start, sweep } => {
            out.push_str(&format!(
                "<path d=\"{}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{sw}\"/>\n",
                wedge_path(cx, cy, r, start, sweep)
            ));
        }
        // A Sankey flow: two cubics, one along each edge of the band, closed at the ends.
        // Translucent, because flows cross and an opaque one hides whatever it passes over —
        // which for a Sankey is the very comparison the diagram exists to make.
        Outline::Ribbon {
            w,
            left_top,
            left_bottom,
            right_top,
            right_bottom,
        } => {
            let (l, r) = (cx - w / 2.0, cx + w / 2.0);
            let mid = (l + r) / 2.0;
            out.push_str(&format!(
                "<path d=\"M{l},{lt} C{mid},{lt} {mid},{rt} {r},{rt} L{r},{rb} \
                 C{mid},{rb} {mid},{lb} {l},{lb} Z\" fill=\"{fill}\" \
                 fill-opacity=\"{RIBBON_OPACITY}\" stroke=\"none\"/>\n",
                l = num(l),
                r = num(r),
                mid = num(mid),
                lt = num(cy + left_top),
                lb = num(cy + left_bottom),
                rt = num(cy + right_top),
                rb = num(cy + right_bottom),
            ));
        }
        // A radar chart's background. The rings and the spokes are one element because they are
        // one thing to a reader — the scale — and drawing them apart would let a mutation delete
        // half of it and leave a picture that still looks deliberate.
        Outline::Graticule {
            r,
            rings,
            spokes,
            polygon,
        } => {
            for i in 1..=rings.max(1) {
                let rr = r * i as f64 / rings.max(1) as f64;
                if polygon && spokes >= 3 {
                    let pts = radial_polygon(cx, cy, rr, spokes);
                    out.push_str(&format!(
                        "<polygon points=\"{pts}\" fill=\"none\" stroke=\"{stroke}\" \
                         stroke-width=\"{}\"/>\n",
                        num(clusters::STROKE_WIDTH)
                    ));
                } else {
                    out.push_str(&format!(
                        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"none\" \
                         stroke=\"{stroke}\" stroke-width=\"{}\"/>\n",
                        num(cx),
                        num(cy),
                        num(rr),
                        num(clusters::STROKE_WIDTH)
                    ));
                }
            }
            let mut d = String::new();
            for k in 0..spokes {
                let a =
                    -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * k as f64 / spokes as f64;
                d.push_str(&format!(
                    "M{},{} L{},{} ",
                    num(cx),
                    num(cy),
                    num(cx + r * a.cos()),
                    num(cy + r * a.sin())
                ));
            }
            if !d.is_empty() {
                out.push_str(&format!(
                    "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{}\"/>\n",
                    d.trim_end(),
                    num(clusters::STROKE_WIDTH)
                ));
            }
        }
        // A mindmap cloud: the box's outline with a bump in the middle of each side, drawn as
        // four quadratic curves so the bumps are part of the same closed path as the corners.
        Outline::Cloud { w, h } => {
            let (l, r) = (cx - w / 2.0, cx + w / 2.0);
            let (t, b) = (cy - h / 2.0, cy + h / 2.0);
            out.push_str(&format!(
                "<path d=\"M{l},{cy} Q{l},{t} {cx},{t} Q{r},{t} {r},{cy} Q{r},{b} {cx},{b} \
                 Q{l},{b} {l},{cy} Z\" fill=\"{fill}\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n",
                l = num(l),
                r = num(r),
                t = num(t),
                b = num(b),
                cx = num(cx),
                cy = num(cy),
            ));
        }
        // A mindmap node written with no brackets: a rule under the words and nothing else.
        Outline::Underline { w, h } => {
            let y = cy + h / 2.0;
            out.push_str(&format!(
                "<path d=\"M{},{y} L{},{y}\" stroke=\"{stroke}\" stroke-width=\"{sw}\" \
                 fill=\"none\"/>\n",
                num(cx - w / 2.0),
                num(cx + w / 2.0),
                y = num(y)
            ));
        }
        // A user journey's score. The eyes are fixed and the mouth is the score: a smile at 5, a
        // straight line at 3, a frown at 1 — so the *shape of the curve* carries the number, and
        // a mutation that moves the score draws a different face rather than a different colour.
        Outline::Face { r, score } => {
            let s = score.clamp(1.0, 5.0);
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{fill}\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n",
                num(cx),
                num(cy),
                num(r)
            ));
            for dx in [-r * 0.35, r * 0.35] {
                out.push_str(&format!(
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{text}\"/>\n",
                    num(cx + dx),
                    num(cy - r * 0.25),
                    num(r * 0.12)
                ));
            }
            // The control point runs from well below the mouth line (a smile) to well above it
            // (a frown) as the score falls.
            let lift = (s - 3.0) / 2.0 * r * 0.9;
            out.push_str(&format!(
                "<path d=\"M{},{y} Q{},{qy} {},{y}\" fill=\"none\" stroke=\"{text}\" \
                 stroke-width=\"{sw}\"/>\n",
                num(cx - r * 0.45),
                num(cx),
                num(cx + r * 0.45),
                y = num(cy + r * 0.25),
                qy = num(cy + r * 0.25 + lift)
            ));
        }
        // A reverted commit: the dot, and a cross through it.
        Outline::Crossed { r } => {
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{fill}\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n",
                num(cx),
                num(cy),
                num(r)
            ));
            let d = r * 0.6;
            out.push_str(&format!(
                "<path d=\"M{},{} L{},{} M{},{} L{},{}\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\" fill=\"none\"/>\n",
                num(cx - d),
                num(cy - d),
                num(cx + d),
                num(cy + d),
                num(cx - d),
                num(cy + d),
                num(cx + d),
                num(cy - d)
            ));
        }
        // A git graph's tag: the ticket, then the hole punched through it. The hole is filled
        // with the colour an edge label's backing patch uses — the palette's own ground — rather
        // than left unpainted, because the ticket is filled and a hole that showed the fill
        // through it would not be a hole. (It is a mark a few pixels across, not a page
        // background, so §1's rule against painting over the terminal is not in play.)
        Outline::Tag {
            w,
            h,
            point,
            tip,
            hole,
        } => {
            let pts = shapes::tag_points(w, h, point, tip)
                .into_iter()
                .map(|p| format!("{},{}", num(cx + p.x), num(cy + p.y)))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!(
                "<polygon points=\"{pts}\" fill=\"{fill}\" stroke=\"{stroke}\" \
                 stroke-width=\"{sw}\"/>\n"
            ));
            let at = shapes::tag_hole_center(w, point);
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" stroke=\"{stroke}\" \
                 stroke-width=\"{}\"/>\n",
                num(cx + at.x),
                num(cy + at.y),
                num(hole),
                theme.background_ref,
                num(NODE_STROKE_WIDTH / 2.0)
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

    let normal_width = theme.tokens.map_or(EDGE_STROKE_WIDTH, |t| t.edge_width);
    let dotted_dash = theme.tokens.map_or(DOTTED_DASH, |t| t.dotted_dash);
    let (mut width, mut dash) = match edge.stroke {
        Stroke::Thick => (THICK_STROKE_WIDTH, None),
        Stroke::Dotted => (normal_width, Some(dotted_dash)),
        // A `x-- text -->` whose two halves disagree is `INVALID` in mermaid; it still has a line.
        Stroke::Normal | Stroke::Invalid => (normal_width, None),
        Stroke::Invisible => return,
    };
    // A chart's line plot and a radar chart's curve are drawn in their own series colour; every
    // other edge in every diagram kind is `theme.line`, exactly as before this field existed.
    let mut line = edge.series.map_or(theme.line, |i| theme.series(i));
    // `class` / `:::` / `linkStyle` win over both the theme and `~~~`/`===`/`-.-`'s own defaults —
    // the most specific instruction the source gave (`style::cascade_edge`'s module docs).
    if let Some(s) = &edge.style {
        if let Some(v) = s.stroke.as_deref() {
            line = v;
        }
        if let Some(v) = s.stroke_width {
            width = v;
        }
        if let Some(v) = s.dash.as_deref() {
            dash = Some(v);
        }
    }
    // A data path is drawn straight and a route is drawn smooth — see `PlacedEdge::drawn_points`
    // and `edges::polyline_path`. `series` is the thing that tells them apart, because a series is
    // exactly what makes a line a series. Everything else draws in `edge.curve` (mermaid's
    // `flowchart.curve` / `linkStyle interpolate`; `Curve::Basis` — the only value any diagram
    // kind but a flowchart's own edges ever carries — reproduces `curve_basis_path` exactly).
    let path_of = |pts: &[Point]| -> String {
        if edge.series.is_some() || edge.straight {
            edges::polyline_path(pts)
        } else {
            edge.curve.path(pts)
        }
    };
    // §10-1 item 4's 12px crossing gap (`PlacedEdge::gaps`'s own doc) — empty for every edge
    // before stage 5 existed and empty still for anything that never crosses another edge, which
    // is what keeps this the same single `<path>` it always was in that case.
    let pieces: Vec<Vec<Point>> = if edge.gaps.is_empty() {
        vec![trimmed]
    } else {
        edges::split_at_gaps(&trimmed, &edge.gaps)
    };
    for piece in &pieces {
        if piece.len() < 2 {
            continue;
        }
        out.push_str(&format!(
            "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"{}/>\n",
            path_of(piece),
            line,
            num(width),
            dash.map_or(String::new(), |d| format!(" stroke-dasharray=\"{d}\""))
        ));
    }

    // The head end first, then the tail end. That order is not cosmetic: it is the order the
    // flowchart renderer emitted a `<-->`'s two heads in before terminators became per-end, and
    // the golden pins the sequence of elements.
    //
    // `tip_color`: mermaid's own convention (every diagram kind before `[ui] mermaid_routing`
    // existed) is one fixed `theme.arrowhead` regardless of the line's own colour — that is
    // `PlacedEdge::tip_matches_line`'s `false` case, and it is what every edge still draws.
    // `true` — only a flowchart's own edge, only under `"konoma-orthogonal"` — instead paints the
    // terminal mark in `line`, the exact string just drawn the path in: the theme default, unless
    // `class`/`:::`/`linkStyle` overrode it above (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item
    // 5, "辺・矢尻・ラベル文字・ノード枠を同色で揃える").
    //
    // A palette carrying `Tokens` says the same thing as a property of the *palette* rather than
    // of the routing (`ink_follows_line`), so a line that a `linkStyle` coloured keeps its own
    // colour right through its terminal mark whichever router drew it.
    let tip_color = if edge.tip_matches_line || theme.tokens.is_some_and(|t| t.ink_follows_line) {
        line
    } else {
        theme.arrowhead
    };
    let n = points.len();
    let (tail_dir, tail_tip) = (&points[1], &points[0]);
    let (head_dir, head_tip) = (&points[n - 2], &points[n - 1]);
    emit_tip(
        out,
        head_dir,
        head_tip,
        edge.tip_end,
        tip_color,
        normal_width,
        theme,
    );
    emit_tip(
        out,
        tail_dir,
        tail_tip,
        edge.tip_start,
        tip_color,
        normal_width,
        theme,
    );
}

/// One end of a line, whatever kind of mark it carries.
///
/// `color` is the ink every terminal-specific "arrowhead" colour below draws in —
/// `theme.arrowhead` for every edge except a flowchart's own orthogonal-routed one (see
/// `emit_edge`'s own doc on `tip_color`). `theme` is kept alongside it only for the handful of
/// marks — `Lollipop`'s ring, a hollow `Tip::HollowTriangle`/`Tip::HollowDiamond`, an ER "zero"
/// ring — whose *interior* is cut out in the node's own ground colour, which is never the line's
/// business to override.
fn emit_tip(
    out: &mut String,
    from: &Point,
    tip: &Point,
    kind: Tip,
    color: &str,
    width: f64,
    theme: &Theme,
) {
    match kind {
        Tip::None => {}
        Tip::Arrow => emit_arrow_head(out, from, tip, color),
        Tip::Cross => emit_cross(out, from, tip, color, width),
        Tip::Circle => emit_circle_end(out, from, tip, color),
        Tip::Async => {
            let pts = edges::async_head(from, tip);
            out.push_str(&format!(
                "<polygon points=\"{}\" fill=\"{}\"/>\n",
                pts.iter()
                    .map(|p| format!("{},{}", num(p.x), num(p.y)))
                    .collect::<Vec<_>>()
                    .join(" "),
                color
            ));
        }
        Tip::HollowTriangle => {
            emit_polygon_tip(out, &edges::triangle(from, tip), theme, color, false)
        }
        Tip::FilledDiamond => emit_polygon_tip(out, &edges::diamond(from, tip), theme, color, true),
        Tip::HollowDiamond => {
            emit_polygon_tip(out, &edges::diamond(from, tip), theme, color, false)
        }
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
                color,
                num(NODE_STROKE_WIDTH)
            ));
        }
        Tip::ErOnlyOne => {
            emit_er_bar(out, from, tip, edges::ER_NEAR, color, width);
            emit_er_bar(out, from, tip, edges::ER_FAR, color, width);
        }
        Tip::ErZeroOrOne => {
            emit_er_bar(out, from, tip, edges::ER_NEAR, color, width);
            emit_er_ring(out, from, tip, edges::ER_FAR, theme, color, width);
        }
        Tip::ErOneOrMore => {
            emit_crows_foot(out, from, tip, color, width);
            emit_er_bar(out, from, tip, edges::ER_FAR, color, width);
        }
        Tip::ErZeroOrMore => {
            emit_crows_foot(out, from, tip, color, width);
            emit_er_ring(out, from, tip, edges::ER_FAR, theme, color, width);
        }
    }
}

/// A triangle or a diamond at the end of a line. `filled` picks between "this end owns the other"
/// (composition, a solid mark) and "this end is the general case" (inheritance and aggregation,
/// an outline the terminal's own ground shows through).
fn emit_polygon_tip(out: &mut String, points: &[Point], theme: &Theme, color: &str, filled: bool) {
    let pts = points
        .iter()
        .map(|p| format!("{},{}", num(p.x), num(p.y)))
        .collect::<Vec<_>>()
        .join(" ");
    let fill = if filled { color } else { theme.node_fill };
    out.push_str(&format!(
        "<polygon points=\"{pts}\" fill=\"{fill}\" stroke=\"{}\" stroke-width=\"{}\"/>\n",
        color,
        num(NODE_STROKE_WIDTH)
    ));
}

/// The bar that means "one" on an ER relationship, drawn across the line.
fn emit_er_bar(
    out: &mut String,
    from: &Point,
    tip: &Point,
    distance: f64,
    color: &str,
    width: f64,
) {
    let (a, b) = edges::cross_bar(from, tip, distance, edges::ER_BAR_HALF);
    out.push_str(&format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" \
         stroke-width=\"{}\"/>\n",
        num(a.x),
        num(a.y),
        num(b.x),
        num(b.y),
        color,
        num(width)
    ));
}

/// The ring that means "zero" on an ER relationship. Filled with the node colour rather than left
/// open, so the shaft it sits on does not run through the middle of it.
fn emit_er_ring(
    out: &mut String,
    from: &Point,
    tip: &Point,
    distance: f64,
    theme: &Theme,
    color: &str,
    width: f64,
) {
    let c = edges::back_along(from, tip, distance);
    out.push_str(&format!(
        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" stroke=\"{}\" \
         stroke-width=\"{}\"/>\n",
        num(c.x),
        num(c.y),
        num(edges::ER_RING_RADIUS),
        theme.node_fill,
        color,
        num(width)
    ));
}

/// The crow's foot that means "many": three prongs opening onto the entity.
fn emit_crows_foot(out: &mut String, from: &Point, tip: &Point, color: &str, width: f64) {
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
        color,
        num(width)
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
fn emit_arrow_head(out: &mut String, from: &Point, tip: &Point, color: &str) {
    let pts = edges::arrow_head(from, tip);
    out.push_str(&format!(
        "<polygon points=\"{}\" fill=\"{}\"/>\n",
        pts.iter()
            .map(|p| format!("{},{}", num(p.x), num(p.y)))
            .collect::<Vec<_>>()
            .join(" "),
        color
    ));
}

/// The `--x` terminator: two strokes crossing just short of the boundary point.
///
/// Set back by its own half-diagonal rather than centred on the boundary. Nodes are drawn over
/// the edges (so a line running under a box disappears under it), and a cross centred on the
/// boundary loses its inner half to the node's fill — which reads as a `>`, not an `x`. Seen on a
/// real render, not reasoned about.
fn emit_cross(out: &mut String, from: &Point, tip: &Point, color: &str, width: f64) {
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
        color,
        num(width)
    ));
}

/// The `--o` terminator: a filled disc sitting just inside the boundary point.
fn emit_circle_end(out: &mut String, from: &Point, tip: &Point, color: &str) {
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
        color
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
        num(label.font_size),
        fill
    ));
    for (i, line) in label.lines.iter().enumerate() {
        let dy = if i == 0 { 0.0 } else { label.line_height() };
        out.push_str(&format!(
            "<tspan x=\"{}\" dy=\"{}\">{}</tspan>",
            num(cx),
            num(dy),
            escape(line)
        ));
    }
    out.push_str("</text>\n");
}
