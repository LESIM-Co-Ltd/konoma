//! What stage 1c is checked with.
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §6 sets the terms, because konoma has twice been fooled by
//! a green suite that was measuring nothing (a diff harness that had quietly become a
//! self-comparison; a golden that pinned a function production did not call). So the tests here
//! are built around three questions that konoma cannot answer by itself:
//!
//! 1. **Does resvg draw the text at the width konoma measured?** [`box_width_matches_resvg`] puts
//!    the emitted SVG through usvg with konoma's own font database and compares the box konoma
//!    sized against `Text::bounding_box().width()`. That is konoma versus *resvg's real shaper*,
//!    so it cannot decay into konoma versus konoma.
//! 2. **Is the geometry sound?** The `invariant_*` tests state the five properties §6 lists —
//!    boxes do not overlap, edges do not cross a box's inside, a label sits on its own line and on
//!    no box, an arrow tip lands on the target's boundary, the viewBox holds everything — over a
//!    corpus built from the *specification's* cases (`§2-2`'s shapes, strokes, directions), not
//!    from a list of bugs already found.
//! 3. **Does it still come out the same?** The golden is the SVG text (never a PNG: §6 notes
//!    resvg 0.48 swapped its whole text stack), and it goes through [`super::render`], the
//!    function production will call.
//!
//! # Why the golden masks its numbers
//!
//! Every coordinate in a diagram descends from a label width, and a label width comes from the
//! system's sans-serif face — Helvetica here, DejaVu Sans on the Linux CI box. A byte-exact
//! end-to-end golden would therefore be a golden of *this machine's fonts*, and would fail on CI
//! for a reason that has nothing to do with the renderer. So the corpus golden masks numeric
//! attribute values and pins everything else: which elements are emitted, in what order, with
//! which colours, which path commands, and which text. The numbers are pinned separately, by
//! [`emit_golden`] (which builds a diagram by hand, so no font is involved) and by the arithmetic
//! tests over the shape formulas.
//!
//! That the mask really is font-independent was measured rather than assumed: multiplying every
//! measured label width by 0.5, 0.85, 1.15, 1.3 and 2.0 — far past the gap between Helvetica and
//! DejaVu Sans — leaves `mermaid_render.snap` byte-identical. Ranks, orders and dummy-node counts
//! come from the graph's shape, not from its labels' widths, so the emitted element sequence does
//! not move with the font.

use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};

use resvg::usvg;

use super::clusters;
use super::edges;
use super::labels::Label;
use super::shapes::{self, Glyph, Size};
use super::svg::num;
use super::theme::{self, Theme};
use super::{
    lay_out, lay_out_curve, lay_out_flow, orthogonal, render, render_curve, render_flow, Curve,
    Diagram, PlacedCluster, PlacedEdge, PlacedNode, RenderError, Tip, MARGIN,
};
use crate::preview::mermaid::flowchart::{parse, Arrow, Shape, Stroke};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;
use crate::preview::svg::shared_fontdb;

// ---------------------------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------------------------

/// The diagrams every structural test runs over.
///
/// Chosen from the *cases in the specification* rather than from past bugs (memory
/// `corpus-from-spec-not-from-bugs`): each of §2-2's classic shapes, each stroke and arrow
/// spelling, both label notations, all four directions, multi-line labels, CJK, a long edge that
/// skips ranks, a cycle, a self-loop, and a subgraph — which stage 1c must survive without
/// drawing (§7).
pub(super) const CORPUS: &[(&str, &str)] = &[
    (
        "branch",
        "flowchart TD\n  A[Start] --> B{Ready?}\n  B -->|yes| C([Ship])\n  B -->|no| D[Fix it]\n  D --> B",
    ),
    (
        "shapes",
        "flowchart TD\n  a[rect] --> b(round)\n  b --> c([stadium])\n  c --> d[[sub]]\n  \
         d --> e[(store)]\n  e --> f((circle))\n  f --> g{diamond}\n  g --> h{{hex}}\n  \
         h --> i>odd]\n  i --> j[/lean/]\n  j --> k[\\lean back\\]\n  k --> l[/trap\\]\n  \
         l --> m[\\inv trap/]",
    ),
    (
        "strokes",
        "flowchart LR\n  A --- B\n  B -.-> C\n  C ==> D\n  D --o E\n  E --x F\n  A <--> F\n  \
         C ~~~ E",
    ),
    (
        "cjk",
        "flowchart TD\n  A[ツリー] -->|Enter| B{種別を解決}\n  B -->|画像| C[全画面プレビュー]\n  \
         B -->|テキスト| D[窓読み]\n  D --> A",
    ),
    (
        "long-edge",
        "flowchart TD\n  A --> B --> C --> D --> E\n  A --> E\n  E --> B\n  C --> A",
    ),
    (
        "left-right",
        "flowchart LR\n  Parse -- tokens --> Layout -- boxes --> Draw\n  Draw --> Parse",
    ),
    (
        "bottom-top",
        "flowchart BT\n  leaf1 --> mid\n  leaf2 --> mid\n  mid --> root",
    ),
    (
        "right-left",
        "flowchart RL\n  A[one] --> B[two]\n  B --> C[three]",
    ),
    (
        "multiline",
        "flowchart TD\n  A[first line<br>second line<br>third] --> B[\"one\\ntwo\"]",
    ),
    (
        "self-loop",
        "flowchart TD\n  A[retry] --> A\n  A --> B[done]",
    ),
    (
        "subgraph",
        "flowchart TD\n  subgraph one [Group]\n    A --> B\n  end\n  B --> C\n  C --> A",
    ),
    (
        "subgraph-nested",
        "flowchart TD\n  subgraph outer [Outer]\n    subgraph inner [Inner]\n      A --> B\n             end\n    B --> C\n  end\n  C --> D",
    ),
    (
        "subgraph-siblings",
        "flowchart LR\n  subgraph left [Read]\n    A --> B\n  end\n  subgraph right [Write]\n           C --> D\n  end\n  B --> C",
    ),
    (
        // A block sitting between two nodes that have nothing to do with it: `X --> Y` spans the
        // same ranks the frame does, and it is dagre's compound layout (`parentDummyChains`) that
        // keeps its waypoints out of the box rather than anything this renderer does afterwards.
        "subgraph-bypass",
        "flowchart LR\n  subgraph one [Middle]\n    A --> B\n  end\n  X --> Y\n  X --> A\n           B --> Y",
    ),
    (
        "subgraph-endpoint",
        "flowchart LR\n  subgraph one [First]\n    A --> B\n  end\n  subgraph two [Second]\n           C --> D\n  end\n  one --> two\n  E --> one",
    ),
    (
        "subgraph-direction",
        "flowchart LR\n  subgraph one [Steps]\n    direction TB\n    A --> B --> C\n  end\n           one --> D",
    ),
    (
        "subgraph-cjk",
        "flowchart TD\n  subgraph proc [プレビュー解決]\n    A[種別] --> B[レンダラ]\n  end\n           B --> C[全画面表示]",
    ),
    (
        "subgraph-long-title",
        "flowchart TD\n  subgraph one [resolve the preview kind and delegate it]\n    A --> B\n           end\n  subgraph two [b]\n    C --> D\n  end\n  B --> C",
    ),
    (
        // A two-line title is the case that makes a frame grow upward: the band dagre leaves
        // above the topmost rank is one `ranksep`/2, which fits one line of text and not two.
        "subgraph-tall-title",
        "flowchart TD\n  subgraph one [Resolve the kind<br>and delegate]\n    A --> B\n  end\n           B --> C",
    ),
    (
        "subgraph-untitled",
        "flowchart TD\n  subgraph\n    A --> B\n  end\n  B --> C",
    ),
    (
        "amp-chain",
        "flowchart LR\n  A & B --> C & D\n  C --> E",
    ),
    (
        "length",
        "flowchart TD\n  A ---> B\n  A --> C\n  B --> D\n  C --> D",
    ),
    (
        // `linkStyle` naming several indices at once, in the exact shape a real diagram uses it
        // (`~/work/Ergora/doc/spec/05-system-architecture.md`'s six colour-coded groups of edges
        // — the source of the 2026-08-28 regression `docs/STATUS.md` records). Before that fix,
        // `mask_numbers` would have shown this case's colour drift — `fill`/`stroke` are two of
        // the few attributes it does *not* mask — but no corpus case exercised `linkStyle` at
        // all, which is exactly how the regression reached a release unnoticed.
        "link-style-multi-index",
        "flowchart TD\n  A --> B\n  B --> C\n  C --> D\n  D --> E\n  \
         linkStyle 0,1,2 stroke:#1f6feb\n  linkStyle 3 stroke:#d4a017",
    ),
    (
        // `classDef` + `:::` + a node's own `style` together, so the golden pins the whole
        // cascade at once: `default` colours every node, `:::hot` overrides the one it names, and
        // `style` on top of that overrides `:::hot` again.
        "classdef-and-style-cascade",
        "flowchart TD\n  classDef default fill:#223,stroke:#556\n  \
         classDef hot fill:#f9f,stroke:#a00\n  A:::hot --> B\n  style A fill:#0f0",
    ),
];

/// Label strings the measuring instrument runs over. §6 names these classes explicitly: CJK,
/// emoji, text with kerning pairs, thin glyphs, long runs and mixtures.
const LABELS: &[(&str, &str)] = &[
    ("ascii", "Start"),
    ("kerning", "AVATAR To Wa"),
    ("thin", "illicit lilli"),
    ("cjk", "全画面プレビュー"),
    ("cjk-punct", "開始、処理。「確認」"),
    ("emoji", "build 🚀 ship"),
    (
        "long",
        "resolve the preview kind and hand it to the right renderer",
    ),
    (
        "mixed",
        "konoma のプレビュー preview を全画面 fullscreen で",
    ),
    ("digits", "0123456789"),
    ("one", "x"),
    // usvg folds runs of whitespace *before* it shapes, so a label written with two spaces is
    // drawn one space narrower than its bytes suggest. Measuring the bytes sizes a box the text
    // does not fill, and every coordinate downstream of that box moves with it. Stage 4 met this
    // for real on a sequence diagram's `loop  Every minute`; `docs/STATUS.md` recorded it as a
    // hole in every kind's measurement, so each kind's instrument now carries a case of it.
    ("double-space", "loop  Every minute"),
    ("edge-space", "  padded out  "),
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn laid_out(src: &str) -> Diagram {
    let chart = parse(src).unwrap_or_else(|e| panic!("corpus source must parse: {e}"));
    lay_out(&chart).unwrap_or_else(|e| panic!("corpus source must lay out: {e}"))
}

/// [`laid_out`], with every edge's curve resolved from `curve` (`ui.mermaid_curve`'s raw
/// string) the way [`lay_out_curve`] resolves it.
fn laid_out_curve(src: &str, curve: &str) -> Diagram {
    let chart = parse(src).unwrap_or_else(|e| panic!("corpus source must parse: {e}"));
    lay_out_curve(&chart, curve).unwrap_or_else(|e| panic!("corpus source must lay out: {e}"))
}

/// [`laid_out_curve`], with edges additionally routed by `routing` (`ui.mermaid_routing`'s raw
/// string) the way [`lay_out_flow`] resolves it.
fn laid_out_flow(src: &str, curve: &str, routing: &str) -> Diagram {
    let chart = parse(src).unwrap_or_else(|e| panic!("corpus source must parse: {e}"));
    lay_out_flow(&chart, curve, routing)
        .unwrap_or_else(|e| panic!("corpus source must lay out: {e}"))
}

/// A [`PlacedNode`] with a plain rectangle glyph and a blank label — a hand-built layout input for
/// the stage 2-4 unit tests below, the same role `orthogonal.rs`'s own private `node` helper plays
/// for that module's tests (kept separate rather than shared: the two test modules build different
/// things from a node — this file also needs a full [`GraphSpec`] elsewhere, which
/// `orthogonal.rs`'s helper has no reason to know about).
fn placed_node(id: &str, cx: f64, cy: f64, w: f64, h: f64) -> PlacedNode {
    PlacedNode {
        id: id.to_string(),
        shape: Glyph::default(),
        center: Point::new(cx, cy),
        size: Size::new(w, h),
        label: Label::measure(""),
        panel: None,
        series: None,
        mark: None,
        style: None,
    }
}

pub(super) fn tree_of(svg: &str) -> usvg::Tree {
    let opt = usvg::Options {
        fontdb: shared_fontdb(),
        ..usvg::Options::default()
    };
    usvg::Tree::from_data(svg.as_bytes(), &opt).expect("emitted SVG parses")
}

/// Every `<text>` in the tree, as the width resvg actually laid it out at.
pub(super) fn text_widths(group: &usvg::Group, out: &mut Vec<f32>) {
    for node in group.children() {
        match node {
            usvg::Node::Text(t) => out.push(t.bounding_box().width()),
            usvg::Node::Group(g) => text_widths(g, out),
            _ => {}
        }
    }
}

/// Every `<text>` in the tree, as the string it holds and the width resvg laid it out at.
///
/// The content is what makes an instrument precise: a document usually has several kinds of label
/// in it, and comparing the widest one would only ever measure the widest kind.
pub(super) fn measured_texts(group: &usvg::Group, out: &mut Vec<(String, f64)>) {
    for node in group.children() {
        match node {
            usvg::Node::Text(t) => {
                let s: String = t.chunks().iter().map(|c| c.text().to_string()).collect();
                out.push((s, t.bounding_box().width() as f64));
            }
            usvg::Node::Group(g) => measured_texts(g, out),
            _ => {}
        }
    }
}

/// Every drawn path's bounding box (fill geometry, stroke excluded).
pub(super) fn path_boxes(group: &usvg::Group, out: &mut Vec<usvg::Rect>) {
    for node in group.children() {
        match node {
            usvg::Node::Path(p) => out.push(p.bounding_box()),
            usvg::Node::Group(g) => path_boxes(g, out),
            _ => {}
        }
    }
}

/// Squared distance from `p` to the segment `a`-`b`.
pub(super) fn dist_to_segment(p: &Point, a: &Point, b: &Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return (p.x - a.x).hypot(p.y - a.y);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}

pub(super) fn dist_to_polyline(p: &Point, line: &[Point]) -> f64 {
    line.windows(2)
        .map(|w| dist_to_segment(p, &w[0], &w[1]))
        .fold(f64::INFINITY, f64::min)
}

/// The boundary of a placed node, as a closed polygon in absolute coordinates.
///
/// Deliberately built from [`shapes::polygon`] and the bounding box rather than from
/// [`shapes::intersect`]: the invariant "the arrow tip is on the boundary" has to be checked
/// against the outline, not against the same function that produced the point.
pub(super) fn boundary(node: &PlacedNode) -> Vec<Point> {
    let (w, h) = (node.size.w, node.size.h);
    let (cx, cy) = (node.center.x, node.center.y);
    let local = shapes::polygon(node.shape, node.size);
    if !local.is_empty() {
        return local
            .into_iter()
            .map(|p| Point::new(cx + p.x, cy + p.y))
            .collect();
    }
    match node.shape {
        // Sampled finely enough that "on the boundary" is decided by geometry, not by the sample
        // count: 256 segments on a circle of radius 100 deviate by under 0.008px.
        // Both `[*]` markers are circles too — a `stateEnd`'s ring is what an edge stops on, and
        // measuring it against a bounding box instead lets a line end up to `r(1 - 1/√2)` inside
        // the shape without the invariant noticing.
        Glyph::Flow(Shape::Circle)
        | Glyph::Flow(Shape::DoubleCircle)
        | Glyph::StateStart
        | Glyph::StateEnd => {
            let r = w / 2.0;
            (0..256)
                .map(|i| {
                    let t = std::f64::consts::TAU * i as f64 / 256.0;
                    Point::new(cx + r * t.cos(), cy + r * t.sin())
                })
                .collect()
        }
        _ => vec![
            Point::new(cx - w / 2.0, cy - h / 2.0),
            Point::new(cx + w / 2.0, cy - h / 2.0),
            Point::new(cx + w / 2.0, cy + h / 2.0),
            Point::new(cx - w / 2.0, cy + h / 2.0),
        ],
    }
}

/// [`shapes::size`] for one of a flowchart's own shapes, which is what the arithmetic tests are
/// written about. The renderer measures every node through [`Glyph`]; wrapping it here keeps the
/// formulas readable as formulas.
fn flow_size(shape: Shape, label: Size) -> Size {
    shapes::size(Glyph::Flow(shape), label)
}

/// Distance from `p` to a closed polygon's boundary.
pub(super) fn dist_to_boundary(p: &Point, poly: &[Point]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| dist_to_segment(p, &poly[i], &poly[(i + 1) % n]))
        .fold(f64::INFINITY, f64::min)
}

/// Even-odd point-in-polygon.
pub(super) fn inside(p: &Point, poly: &[Point]) -> bool {
    let n = poly.len();
    let mut hit = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (&poly[i], &poly[j]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x {
                hit = !hit;
            }
        }
        j = i;
    }
    hit
}

/// How far inside the shape `p` lies, or 0 when it is outside.
pub(super) fn depth(p: &Point, poly: &[Point]) -> f64 {
    if inside(p, poly) {
        dist_to_boundary(p, poly)
    } else {
        0.0
    }
}

/// The subgraph tree of a source, rebuilt from the parser's own output.
///
/// The cluster invariants need to know who is inside what, and the [`Diagram`] deliberately does
/// not carry membership — a frame is geometry. Re-deriving it from [`parse`] keeps the question
/// ("is this node a member of that block?") answered by the parser, which stage 1b tested, rather
/// than by the layout code under test here.
fn tree_of_src(src: &str) -> clusters::Tree {
    let chart = parse(src).expect("parses");
    let ids: HashSet<String> = chart.nodes.iter().map(|n| n.id.clone()).collect();
    clusters::Tree::build(&chart, |id| ids.contains(id))
}

/// Axis-aligned overlap of two rectangles given as `(l, t, r, b)`.
pub(super) fn rect_overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> (f64, f64) {
    (a.2.min(b.2) - a.0.max(b.0), a.3.min(b.3) - a.1.max(b.1))
}

/// How far `inner` pokes out of `outer`, as the worst of the four sides. Negative means contained.
pub(super) fn escapes(inner: (f64, f64, f64, f64), outer: (f64, f64, f64, f64)) -> f64 {
    (outer.0 - inner.0)
        .max(outer.1 - inner.1)
        .max(inner.2 - outer.2)
        .max(inner.3 - outer.3)
}

/// The deepest a polyline reaches inside a rectangle, sampling every half pixel. 0 when it never
/// enters. Sampling rather than testing the vertices is what catches a segment that cuts a corner.
pub(super) fn polyline_depth_in_rect(line: &[Point], rect: (f64, f64, f64, f64)) -> f64 {
    let depth_at = |p: &Point| {
        let d = (p.x - rect.0)
            .min(rect.2 - p.x)
            .min(p.y - rect.1)
            .min(rect.3 - p.y);
        d.max(0.0)
    };
    let mut worst: f64 = 0.0;
    for w in line.windows(2) {
        let len = (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
        let steps = (len / 0.5).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let p = Point::new(
                w[0].x + t * (w[1].x - w[0].x),
                w[0].y + t * (w[1].y - w[0].y),
            );
            worst = worst.max(depth_at(&p));
        }
    }
    worst
}

/// Axis-aligned overlap of two node boxes, as `(dx, dy)` — both positive means they intersect.
pub(super) fn box_overlap(a: &PlacedNode, b: &PlacedNode) -> (f64, f64) {
    let (al, at, ar, ab) = a.bounds();
    let (bl, bt, br, bb) = b.bounds();
    (ar.min(br) - al.max(bl), ab.min(bb) - at.max(bt))
}

/// Masks the value of every numeric attribute, leaving structure, colours and text intact.
///
/// `M12.5,4 L20,4` becomes `M#,# L#,#`, so the golden pins which path commands are emitted in
/// which order without pinning coordinates that descend from the system font.
pub(super) fn mask_numbers(svg: &str) -> String {
    const NUMERIC: &[&str] = &[
        "x",
        "y",
        "x1",
        "y1",
        "x2",
        "y2",
        "width",
        "height",
        "rx",
        "ry",
        "cx",
        "cy",
        "r",
        "d",
        "points",
        "stroke-width",
        "dy",
        "font-size",
        "stroke-dasharray",
        "viewBox",
    ];
    let numeric: HashSet<&str> = NUMERIC.iter().copied().collect();
    let mut out = String::with_capacity(svg.len());
    let bytes: Vec<char> = svg.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        // Find `name="` where `name` is a numeric attribute.
        if bytes[i] == '=' && i + 1 < bytes.len() && bytes[i + 1] == '"' {
            let mut start = i;
            while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == '-')
            {
                start -= 1;
            }
            let name: String = bytes[start..i].iter().collect();
            if numeric.contains(name.as_str()) {
                out.push_str("=\"");
                let mut j = i + 2;
                let mut in_number = false;
                while j < bytes.len() && bytes[j] != '"' {
                    let c = bytes[j];
                    if c.is_ascii_digit() || c == '.' || (c == '-' && !in_number) {
                        if !in_number {
                            out.push('#');
                            in_number = true;
                        }
                    } else {
                        in_number = false;
                        out.push(c);
                    }
                    j += 1;
                }
                out.push('"');
                i = j + 1;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// The columns of an emitted SVG that a routing regression would actually move: every `d` (a
/// path's shape, with its numbers masked the same way [`mask_numbers`] masks them — font-dependent
/// label widths must not make this signature drift between Helvetica here and DejaVu Sans on
/// Linux CI, the same reason `corpus_golden` masks numbers at all) and every literal `fill`/
/// `stroke` (colour, which `mask_numbers` never touches — see the `link-style-multi-index` corpus
/// comment for why that's deliberate). One attribute per line, document order preserved, so a
/// change in bend count, sign pattern, command letter, or colour still moves this string even
/// though the exact coordinates are masked away.
pub(super) fn routing_signature(svg: &str) -> String {
    let masked = mask_numbers(svg);
    let mut out = String::new();
    let chars: Vec<char> = masked.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '=' && i + 1 < chars.len() && chars[i + 1] == '"' {
            let mut start = i;
            while start > 0 && (chars[start - 1].is_ascii_alphanumeric() || chars[start - 1] == '-')
            {
                start -= 1;
            }
            let name: String = chars[start..i].iter().collect();
            if matches!(name.as_str(), "d" | "fill" | "stroke") {
                let mut j = i + 2;
                while j < chars.len() && chars[j] != '"' {
                    j += 1;
                }
                let value: String = chars[i + 2..j].iter().collect();
                out.push_str(&name);
                out.push('=');
                out.push_str(&value);
                out.push('\n');
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

pub(super) fn snapshot_path(name: &str) -> PathBuf {
    FsPath::new(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots")
        .join(format!("{name}.snap"))
}

/// Compares against the checked-in snapshot, following konoma's existing convention:
/// `KONOMA_UPDATE_SNAPSHOTS=1` rewrites the file, and a missing file skips (a build from the
/// published crate has no `snapshots/`, which is excluded).
pub(super) fn assert_snapshot(name: &str, actual: &str) {
    let path = snapshot_path(name);
    if std::env::var_os("KONOMA_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().expect("snapshot dir")).expect("create snapshots/");
        std::fs::write(&path, actual).expect("write snapshot");
        eprintln!("mermaid snapshot: wrote {}", path.display());
        return;
    }
    let Ok(expected) = std::fs::read_to_string(&path) else {
        eprintln!(
            "mermaid snapshot: {} not found — skipping (published-crate build?); \
             regenerate with `KONOMA_UPDATE_SNAPSHOTS=1 cargo test`",
            path.display()
        );
        return;
    };
    if expected == actual {
        return;
    }
    let (el, al): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), actual.lines().collect());
    let mut i = 0;
    while i < el.len().min(al.len()) && el[i] == al[i] {
        i += 1;
    }
    let lo = i.saturating_sub(2);
    panic!(
        "mermaid snapshot {name} drifted at line {i}\n--- expected ---\n{}\n--- actual ---\n{}\n\
         Re-run with KONOMA_UPDATE_SNAPSHOTS=1 and inspect the diff if this is intentional.",
        el[lo..(i + 3).min(el.len())].join("\n"),
        al[lo..(i + 3).min(al.len())].join("\n"),
    );
}

// ---------------------------------------------------------------------------------------------
// 1. The main instrument: what konoma measured vs what resvg drew
// ---------------------------------------------------------------------------------------------

/// **The instrument §6 asks for.** A one-node diagram is emitted, put through usvg with konoma's
/// own font database, and the box konoma sized is compared against the width resvg laid the text
/// out at. The relation is exact and declared: a rectangle is the label plus `padding * 4`
/// (`squareRect.ts` doubles `padding` for `labelPaddingX`, `drawRect.ts` doubles it again).
///
/// This is konoma against **resvg's real shaper**, not against another copy of konoma, so it
/// cannot silently become a self-comparison — the failure mode that made the markdown diff
/// harness report "468/468 identical" while every paragraph was broken.
#[test]
fn box_width_matches_resvg() {
    if !text_metrics::fonts_available() {
        eprintln!("no sans-serif face — skipping (the renderer refuses to draw here too)");
        return;
    }
    let expected_padding = shapes::PADDING * 4.0;
    for (name, label) in LABELS {
        let src = format!("flowchart TD\n  A[\"{label}\"]");
        let diagram = laid_out(&src);
        let svg = render(&src, "dark").expect("renders");
        let node = &diagram.nodes[0];

        let mut widths = Vec::new();
        text_widths(tree_of(&svg).root(), &mut widths);
        assert_eq!(
            widths.len(),
            1,
            "{name}: exactly one <text> should survive usvg (a dropped one means the font \
             resolution konoma measured with is not the one resvg drew with)"
        );
        let drawn = widths[0] as f64;
        let slack = node.size.w - drawn;
        assert!(
            (slack - expected_padding).abs() <= 1.0,
            "{name} {label:?}: box is {} wide, resvg drew the text {} wide → {} of padding, \
             but the shape declares {}",
            num(node.size.w),
            num(drawn),
            num(slack),
            num(expected_padding)
        );

        // …and the box konoma sized is the box it emitted.
        let mut boxes = Vec::new();
        path_boxes(tree_of(&svg).root(), &mut boxes);
        assert!(
            boxes
                .iter()
                .any(|b| (b.width() as f64 - node.size.w).abs() < 0.01 && b.x() > 0.0),
            "{name}: no drawn path has the width the model declares ({})",
            num(node.size.w)
        );
    }
}

/// The same comparison for an edge label, which is measured by the same code but sized by a
/// different rule (a small pad, not a shape's padding).
#[test]
fn edge_label_box_matches_resvg() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "flowchart TD\n  A[a] -->|全画面プレビュー| B[b]";
    let diagram = laid_out(src);
    let svg = render(src, "dark").expect("renders");
    let label = diagram.edges[0]
        .label
        .as_ref()
        .expect("edge carries a label");

    let mut widths = Vec::new();
    text_widths(tree_of(&svg).root(), &mut widths);
    // Two node labels and the edge label; the edge label is the widest here.
    let drawn = widths.iter().copied().fold(0.0_f32, f32::max) as f64;
    let slack = label.size.w - drawn;
    assert!(
        (slack - super::LABEL_PAD_X * 2.0).abs() <= 1.0,
        "edge label box {} vs resvg's {} → {} of padding, declared {}",
        num(label.size.w),
        num(drawn),
        num(slack),
        num(super::LABEL_PAD_X * 2.0)
    );
}

/// A multi-line label is as tall as its line count says, and as wide as its widest line — which
/// is what makes `<br>` change a box's shape rather than just its text.
#[test]
fn multiline_label_is_measured_per_line() {
    if !text_metrics::fonts_available() {
        return;
    }
    let one = Label::measure("short");
    let three = Label::measure("short\nconsiderably longer line\nmid");
    assert_eq!(three.lines.len(), 3);
    assert!((three.height - one.height * 3.0).abs() < 1e-9);
    assert!(three.width > one.width);
    assert!((three.width - Label::measure("considerably longer line").width).abs() < 1e-9);
}

// ---------------------------------------------------------------------------------------------
// 2. Geometric invariants (§6's supporting list)
// ---------------------------------------------------------------------------------------------

/// (1) No two node boxes overlap.
pub(super) fn check_nodes_do_not_overlap(name: &str, d: &Diagram) {
    for i in 0..d.nodes.len() {
        for j in (i + 1)..d.nodes.len() {
            let (dx, dy) = box_overlap(&d.nodes[i], &d.nodes[j]);
            assert!(
                dx <= 0.01 || dy <= 0.01,
                "{name}: {} and {} overlap by {}x{}",
                d.nodes[i].id,
                d.nodes[j].id,
                num(dx),
                num(dy)
            );
        }
    }
}

#[test]
fn invariant_nodes_do_not_overlap() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_nodes_do_not_overlap(name, &laid_out(src));
    }
}

/// (2) No edge passes through the inside of a shape.
///
/// Checked against each node's **outline**, not its bounding box: a diamond's box is far bigger
/// than the diamond, and an edge leaving one legitimately crosses the box's corner. Sampling the
/// polyline every half pixel and asking how deep inside each sample lies makes the check
/// independent of `shapes::intersect`, which is the function under test.
///
/// **An edge that names a block is exempt**, for the reason spelled out on
/// [`check_edges_keep_out_of_foreign_frames`] and recorded by
/// [`an_edge_that_names_a_block_is_not_kept_clear_of_its_surroundings`]: dagre was handed a
/// stand-in leaf instead of the endpoint the author wrote, and the line is only cut back to the
/// frame afterwards, so nothing constrained the route to clear the boxes beside it.
///
/// **A note's connector is exempt** for a different reason, recorded by
/// [`super::state_tests::a_notes_connector_can_cross_a_state_in_a_crowded_diagram`]: a note is
/// placed on the side the author named rather than where a clear line to it exists, and on a
/// crowded diagram those are not the same place. It is a straight line between two boxes, not a
/// route, so there is nothing here for the layout to have got wrong.
pub(super) fn check_edges_stay_out_of_shapes(name: &str, d: &Diagram) {
    let outlines: Vec<Vec<Point>> = d.nodes.iter().map(boundary).collect();
    for e in &d.edges {
        // See the doc comment. A frame in this diagram bearing the endpoint's name is exactly
        // "this edge was routed between stand-ins"; an end drawn as a note is exactly "this line
        // was not routed at all".
        if d.cluster(&e.from).is_some() || d.cluster(&e.to).is_some() {
            continue;
        }
        let is_note = |id: &str| d.node(id).is_some_and(|n| n.shape == Glyph::Note);
        if is_note(&e.from) || is_note(&e.to) {
            continue;
        }
        let line = e.drawn_points();
        for w in line.windows(2) {
            let len = (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
            let steps = ((len * 2.0).ceil() as usize).clamp(1, 4000);
            for s in 0..=steps {
                let t = s as f64 / steps as f64;
                let p = Point::new(
                    w[0].x + t * (w[1].x - w[0].x),
                    w[0].y + t * (w[1].y - w[0].y),
                );
                for (node, poly) in d.nodes.iter().zip(&outlines) {
                    let inside_by = depth(&p, poly);
                    assert!(
                        inside_by <= 1.0,
                        "{name}: edge {}->{} runs {}px inside {} at ({}, {})",
                        e.from,
                        e.to,
                        num(inside_by),
                        node.id,
                        num(p.x),
                        num(p.y)
                    );
                }
            }
        }
    }
}

#[test]
fn invariant_edges_stay_out_of_shapes() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_edges_stay_out_of_shapes(name, &laid_out(src));
    }
}

/// (3) An edge label sits **on its own line** and on no node.
///
/// The first half is what distinguishes re-placing the label at the arc midpoint from leaving it
/// where dagre put it: dagre's position is offset from the line by `labeloffset` because its
/// default `labelpos` is `r`.
pub(super) fn check_labels_ride_their_edge(name: &str, d: &Diagram) {
    for e in &d.edges {
        let Some(label) = &e.label else { continue };
        let off = dist_to_polyline(&label.center, &e.points);
        assert!(
            off <= 0.5,
            "{name}: label {:?} on {}->{} sits {}px off its own line",
            label.label.lines.join(" "),
            e.from,
            e.to,
            num(off)
        );
        for n in &d.nodes {
            let (nl, nt, nr, nb) = n.bounds();
            let (ll, lt, lr, lb) = (
                label.center.x - label.size.w / 2.0,
                label.center.y - label.size.h / 2.0,
                label.center.x + label.size.w / 2.0,
                label.center.y + label.size.h / 2.0,
            );
            let (dx, dy) = (nr.min(lr) - nl.max(ll), nb.min(lb) - nt.max(lt));
            assert!(
                dx <= 0.01 || dy <= 0.01,
                "{name}: label {:?} overlaps node {} by {}x{}",
                label.label.lines.join(" "),
                n.id,
                num(dx),
                num(dy)
            );
        }
    }
}

#[test]
fn invariant_labels_ride_their_edge_and_clear_every_node() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_labels_ride_their_edge(name, &laid_out(src));
    }
}

/// (4) Every edge starts and ends **on** the outline of the shape it touches.
///
/// This is the property that makes a branch come out of a diamond's slanted side. Comparing
/// against a densely sampled outline rather than against `shapes::intersect` keeps the check
/// honest: replacing the shape intersection with a bounding-box one has to fail here.
///
/// An end that names a *block* has no outline; it lands on a frame, which
/// [`invariant_an_edge_that_names_a_block_stops_on_its_frame`] checks instead.
pub(super) fn check_endpoints_land_on_the_outline(name: &str, d: &Diagram) {
    for e in &d.edges {
        let placed = |id: &str| match d.node(id) {
            Some(n) => Some(n),
            None if d.cluster(id).is_some() => None,
            None => panic!(
                "{name}: edge {}->{} names a node that is not placed",
                e.from, e.to
            ),
        };
        let (tail, head) = (placed(&e.from), placed(&e.to));
        let first = e.points.first().expect("edge has points");
        let last = e.points.last().expect("edge has points");
        for (label, p, node) in [("start", first, tail), ("end", last, head)]
            .into_iter()
            .filter_map(|(l, p, n)| n.map(|n| (l, p, n)))
        {
            let off = dist_to_boundary(p, &boundary(node));
            assert!(
                off <= 0.05,
                "{name}: {label} of {}->{} is {}px off {}'s outline",
                e.from,
                e.to,
                num(off),
                node.id
            );
        }
    }
}

#[test]
fn invariant_endpoints_land_on_the_outline() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_endpoints_land_on_the_outline(name, &laid_out(src));
    }
}

/// (5) The viewBox contains every node, every waypoint and every label.
pub(super) fn check_view_box_contains_everything(name: &str, d: &Diagram) {
    let check = |x: f64, y: f64, what: &str| {
        assert!(
            x >= -0.01 && y >= -0.01 && x <= d.width + 0.01 && y <= d.height + 0.01,
            "{name}: {what} at ({}, {}) is outside the {}x{} viewBox",
            num(x),
            num(y),
            num(d.width),
            num(d.height)
        );
    };
    for n in &d.nodes {
        let (l, t, r, b) = n.bounds();
        check(l, t, &n.id);
        check(r, b, &n.id);
    }
    for e in &d.edges {
        for p in e.drawn_points() {
            check(p.x, p.y, "waypoint");
        }
        if let Some(l) = &e.label {
            check(
                l.center.x - l.size.w / 2.0,
                l.center.y - l.size.h / 2.0,
                "label",
            );
            check(
                l.center.x + l.size.w / 2.0,
                l.center.y + l.size.h / 2.0,
                "label",
            );
        }
    }
    // …and the margin is really there.
    let left = d
        .nodes
        .iter()
        .map(|n| n.bounds().0)
        .fold(f64::INFINITY, f64::min);
    assert!(
        left >= MARGIN - 0.01,
        "{name}: drawing starts left of the margin"
    );
}

#[test]
fn invariant_view_box_contains_everything() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_view_box_contains_everything(name, &laid_out(src));
    }
}

// ---------------------------------------------------------------------------------------------
// 4b. Cluster invariants (§6, stage 1d)
// ---------------------------------------------------------------------------------------------

/// (6) A frame holds every one of its members.
///
/// This is the whole promise of a subgraph: the reader is being told that these nodes belong
/// together. A frame that clips a member says something false about the diagram.
pub(super) fn check_clusters_hold_their_members(name: &str, d: &Diagram, tree: &clusters::Tree) {
    for c in &d.clusters {
        let frame = c.bounds();
        for n in &d.nodes {
            if !tree.touches(&n.id, &c.id) {
                continue;
            }
            let out = escapes(n.bounds(), frame);
            assert!(
                out <= 0.01,
                "{name}: node {} pokes {out:.2}px out of frame {}",
                n.id,
                c.id
            );
        }
    }
}

/// (6b) A frame holds none of what is *not* its member — the converse of (6), above.
///
/// §10-5 part-3 item 1's own invariant ("ノードは所属クラスターの枠内・非所属ノードは枠外"): a
/// node with no membership in a cluster (`clusters::Tree::touches` false) must not overlap that
/// cluster's own frame at all. Nothing before this checked the converse of (6) — every existing
/// cluster check only ever asked "does a member ever escape its own frame", never "does a
/// *stranger* ever land inside one" — which is exactly the gap `zz-design-4a`'s end marker fell
/// through under `konoma-orthogonal`'s own tighter node spacing
/// (`orthogonal::clear_foreign_cluster_overlaps`'s own doc has the dagre/nodesep reasoning).
pub(super) fn check_foreign_nodes_stay_out_of_clusters(
    name: &str,
    d: &Diagram,
    tree: &clusters::Tree,
) {
    for c in &d.clusters {
        let frame = c.bounds();
        for n in &d.nodes {
            if tree.touches(&n.id, &c.id) {
                continue;
            }
            let (dx, dy) = rect_overlap(n.bounds(), frame);
            assert!(
                dx <= 0.01 || dy <= 0.01,
                "{name}: node {} (not a member) overlaps frame {} by {dx:.2}x{dy:.2}px",
                n.id,
                c.id
            );
        }
    }
}

#[test]
fn invariant_clusters_hold_their_members() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_clusters_hold_their_members(name, &laid_out(src), &tree_of_src(src));
    }
}

/// (7) A nested frame is inside the frame that contains it.
pub(super) fn check_nested_clusters_sit_inside_their_parent(name: &str, d: &Diagram) {
    for c in &d.clusters {
        let Some(parent) = c.parent.as_deref().and_then(|p| d.cluster(p)) else {
            continue;
        };
        let out = escapes(c.bounds(), parent.bounds());
        assert!(
            out <= 0.01,
            "{name}: frame {} pokes {out:.2}px out of its parent {}",
            c.id,
            parent.id
        );
    }
}

#[test]
fn invariant_nested_clusters_sit_inside_their_parent() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_nested_clusters_sit_inside_their_parent(name, &laid_out(src));
    }
}

/// (8) Two frames that are not one inside the other do not overlap.
///
/// Stated over every pair rather than over siblings only: `left` and `right` in `subgraph-
/// siblings` share a parent, but `inner` and an unrelated top-level block do not, and the two
/// boxes overlapping would be just as wrong.
pub(super) fn check_unrelated_clusters_do_not_overlap(name: &str, d: &Diagram) {
    let nested = |d: &Diagram, a: &PlacedCluster, b: &PlacedCluster| {
        let mut cur = a.parent.clone();
        while let Some(p) = cur {
            if p == b.id {
                return true;
            }
            cur = d.cluster(&p).and_then(|c| c.parent.clone());
        }
        false
    };
    for (i, a) in d.clusters.iter().enumerate() {
        for b in d.clusters.iter().skip(i + 1) {
            if nested(d, a, b) || nested(d, b, a) {
                continue;
            }
            let (dx, dy) = rect_overlap(a.bounds(), b.bounds());
            assert!(
                dx <= 0.01 || dy <= 0.01,
                "{name}: frames {} and {} overlap by {dx:.2}x{dy:.2}px",
                a.id,
                b.id
            );
        }
    }
}

#[test]
fn invariant_unrelated_clusters_do_not_overlap() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_unrelated_clusters_do_not_overlap(name, &laid_out(src));
    }
}

/// (9) A line does not run through a frame it has no business being in.
///
/// The distinction the test turns on is [`clusters::Tree::touches`]: an edge with one end inside
/// a block *must* cross that block's border, and clipping it there would cut the line off from
/// its own node. An edge with neither end under the block has no reason to be inside it at all —
/// dagre's compound layout keeps it out (that is what `parentDummyChains` is for), and this is
/// what proves the clusters really were handed to dagre rather than drawn on afterwards.
///
/// **An edge that *names* a block is exempt, and the exemption is a known gap, not a rule.**
/// dagre cannot route to a compound parent, so such an edge is laid out against a stand-in leaf
/// inside the block ([`clusters::Tree::anchor`]) and cut back to the frame only when it is drawn.
/// dagre therefore never saw the endpoints the reader wrote, and nothing constrains the route it
/// chose between the two stand-ins to stay out of a *third* block that happens to sit between
/// them. [`an_edge_that_names_a_block_is_not_kept_clear_of_its_surroundings`] records both halves of
/// the reason it is `#[ignore]`d: it is a property of the vendored layout that predates state
/// diagrams, not something stage 2 introduced, and papering over it here would turn a bug into a
/// specification.
pub(super) fn check_edges_keep_out_of_foreign_frames(
    name: &str,
    d: &Diagram,
    tree: &clusters::Tree,
) {
    for e in &d.edges {
        // See the doc comment: an edge whose own endpoint is a block was routed between two
        // stand-ins, so the layout was never asked the question this invariant poses.
        if tree.contains(&e.from) || tree.contains(&e.to) {
            continue;
        }
        for c in &d.clusters {
            if tree.touches(&e.from, &c.id) || tree.touches(&e.to, &c.id) {
                continue;
            }
            let depth = polyline_depth_in_rect(&e.drawn_points(), c.bounds());
            assert!(
                depth <= 0.5,
                "{name}: edge {} -> {} runs {depth:.2}px into frame {}",
                e.from,
                e.to,
                c.id
            );
        }
    }
}

#[test]
fn invariant_edges_keep_out_of_frames_they_do_not_belong_to() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_edges_keep_out_of_foreign_frames(name, &laid_out(src), &tree_of_src(src));
    }
}

/// (10) An edge that names a block starts or ends **on that block's frame**, and never inside it.
///
/// `one --> two` is legal mermaid (§2-2) and dagre cannot route it — a compound parent has no
/// rank. The line is laid out against a member and then cut back to the boundary, which is the
/// only thing that makes the arrow point at the box rather than at some node the author never
/// mentioned.
pub(super) fn check_edge_that_names_a_block_stops_on_its_frame(
    name: &str,
    d: &Diagram,
    tree: &clusters::Tree,
) {
    for e in &d.edges {
        for (end, id) in [("tail", &e.from), ("head", &e.to)] {
            if !tree.contains(id) {
                continue;
            }
            let c = d
                .cluster(id)
                .unwrap_or_else(|| panic!("{name}: edge names block {id} but no frame was placed"));
            let frame = c.bounds();
            let point = if end == "tail" {
                e.points.first()
            } else {
                e.points.last()
            }
            .expect("an edge has points");
            let on_edge = (point.x - frame.0)
                .abs()
                .min((point.x - frame.2).abs())
                .min((point.y - frame.1).abs())
                .min((point.y - frame.3).abs());
            assert!(
                on_edge <= 0.01,
                "{name}: the {end} of {} -> {} is {on_edge:.2}px off frame {id}",
                e.from,
                e.to
            );
            let depth = polyline_depth_in_rect(&e.drawn_points(), frame);
            assert!(
                depth <= 0.5,
                "{name}: {} -> {} is drawn {depth:.2}px inside frame {id}",
                e.from,
                e.to
            );
        }
    }
}

#[test]
fn invariant_an_edge_that_names_a_block_stops_on_its_frame() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_edge_that_names_a_block_stops_on_its_frame(name, &laid_out(src), &tree_of_src(src));
    }
}

/// (11) A title is inside its own frame and on top of nothing.
///
/// Both halves matter and they pull in opposite directions: pushing the title further in to clear
/// the members eventually pushes it onto them from the other side, and the only reason there is
/// room at all is that dagre leaves a band above the topmost rank. A frame that had to grow to
/// make that band big enough is exactly the case this catches.
pub(super) fn check_cluster_titles(name: &str, d: &Diagram, tree: &clusters::Tree) {
    for c in &d.clusters {
        if c.title.is_blank() {
            continue;
        }
        let t = c.title_center();
        let title = (
            t.x - c.title.width / 2.0,
            t.y - c.title.height / 2.0,
            t.x + c.title.width / 2.0,
            t.y + c.title.height / 2.0,
        );
        let out = escapes(title, c.bounds());
        assert!(
            out <= 0.01,
            "{name}: the title of {} pokes {out:.2}px out of its own frame",
            c.id
        );
        for n in &d.nodes {
            if !tree.touches(&n.id, &c.id) {
                continue;
            }
            let (dx, dy) = rect_overlap(title, n.bounds());
            assert!(
                dx <= 0.01 || dy <= 0.01,
                "{name}: the title of {} sits on node {} ({dx:.2}x{dy:.2}px)",
                c.id,
                n.id
            );
        }
        for other in &d.clusters {
            if other.parent.as_deref() != Some(c.id.as_str()) {
                continue;
            }
            let (dx, dy) = rect_overlap(title, other.bounds());
            assert!(
                dx <= 0.01 || dy <= 0.01,
                "{name}: the title of {} sits on the nested frame {} ({dx:.2}x{dy:.2}px)",
                c.id,
                other.id
            );
        }
    }
}

/// (12) Every row of a compartmented box is **inside the box**, and every rule that separates the
/// rows lies on the box.
///
/// The one thing a [`Panel`](super::panel::Panel) can get wrong that nothing else notices: it is
/// the only geometry in the renderer that is stated in coordinates *relative to a node*, so a
/// sizing mistake shows up as text hanging out of a class box rather than as an overlap the other
/// invariants would catch. Both halves are needed — a panel sized too small clips its own rows,
/// and a rule computed from the wrong height draws across the diagram.
pub(super) fn check_panels_stay_inside_their_box(name: &str, d: &Diagram) {
    for n in &d.nodes {
        let Some(panel) = &n.panel else { continue };
        assert!(
            (panel.size.w - n.size.w).abs() < 0.01 && (panel.size.h - n.size.h).abs() < 0.01,
            "{name}: {} is {}x{} but its panel is {}x{}",
            n.id,
            num(n.size.w),
            num(n.size.h),
            num(panel.size.w),
            num(panel.size.h)
        );
        for (l, t, r, b) in panel.cell_bounds() {
            assert!(
                l >= -0.01 && t >= -0.01 && r <= n.size.w + 0.01 && b <= n.size.h + 0.01,
                "{name}: a row of {} runs from ({}, {}) to ({}, {}), outside its {}x{} box",
                n.id,
                num(l),
                num(t),
                num(r),
                num(b),
                num(n.size.w),
                num(n.size.h)
            );
        }
        for y in &panel.rules {
            assert!(
                *y >= -0.01 && *y <= n.size.h + 0.01,
                "{name}: a rule of {} is at y={} in a box {} tall",
                n.id,
                num(*y),
                num(n.size.h)
            );
        }
        for x in &panel.columns {
            assert!(
                *x >= -0.01 && *x <= n.size.w + 0.01,
                "{name}: a column rule of {} is at x={} in a box {} wide",
                n.id,
                num(*x),
                num(n.size.w)
            );
        }
    }
}

#[test]
fn invariant_cluster_titles_stay_in_the_frame_and_off_the_members() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        check_cluster_titles(name, &laid_out(src), &tree_of_src(src));
    }
}

/// **A known gap, recorded rather than papered over.** An edge that *names a block* is laid out
/// between stand-in leaves, so nothing keeps its route clear of what happens to lie beside it: it
/// can cross a third block, and it can graze a node.
///
/// dagre cannot route to a compound parent — the ranking phase only ever sees leaves — so
/// [`clusters::Tree::anchor`] hands it a descendant instead, and `edges::route` cuts the result
/// back to the frame only when it is drawn. Between those two steps the endpoint the author wrote
/// does not exist as far as the layout is concerned. mermaid solves it the same way and has the
/// same gap.
///
/// Measured on **flowcharts** on purpose. Both halves reproduce identically in state diagrams
/// (`composite-siblings` and `direction-lr` in [`super::state_tests::CASES`]), so this is a
/// property of stage 1's re-anchoring rather than something stage 2 brought in — which is the
/// difference between a limitation to record and a regression to fix. Closing it means
/// constraining the route, which is a change to how a cluster endpoint reaches dagre, not a
/// tolerance to widen. `#[ignore]`d with the reason rather than deleted, following the convention
/// konoma used for the nested `<details>` ordinal bug: the test says what *should* be true, and
/// it fails for as long as it is not.
#[test]
#[ignore = "known gap: an edge naming a block is routed between stand-in leaves, so neither a third block between them nor a node beside them is avoided. Reproduces in mermaid too; needs a change to how a cluster endpoint reaches dagre."]
fn an_edge_that_names_a_block_is_not_kept_clear_of_its_surroundings() {
    if !text_metrics::fonts_available() {
        return;
    }
    // (a) the route crosses a third block that sits between the two it joins.
    let src = "flowchart TD\n  Z --> one\n  one --> two\n  one --> three\n  subgraph one [First]\n    a --> b\n  end\n  subgraph two [Second]\n    c --> d\n  end\n  subgraph three [Third]\n    e --> f\n  end";
    let d = laid_out(src);
    let tree = tree_of_src(src);
    for e in &d.edges {
        for c in &d.clusters {
            if tree.touches(&e.from, &c.id) || tree.touches(&e.to, &c.id) {
                continue;
            }
            let depth = polyline_depth_in_rect(&e.drawn_points(), c.bounds());
            assert!(
                depth <= 0.5,
                "edge {} -> {} runs {depth:.2}px into frame {}",
                e.from,
                e.to,
                c.id
            );
        }
    }

    // (b) the route grazes a node standing beside the frame it leaves.
    let src = "flowchart LR\n  Z --> A\n  A --> B\n  B --> C\n  B --> D\n  subgraph B [B]\n    a --> b\n  end";
    let d = laid_out(src);
    let outlines: Vec<Vec<Point>> = d.nodes.iter().map(boundary).collect();
    for e in &d.edges {
        for p in e.drawn_points() {
            for (node, poly) in d.nodes.iter().zip(&outlines) {
                let inside_by = depth(&p, poly);
                assert!(
                    inside_by <= 1.0,
                    "edge {} -> {} runs {}px inside {}",
                    e.from,
                    e.to,
                    num(inside_by),
                    node.id
                );
            }
        }
    }
}

/// Every block the author wrote gets a frame, and every frame is a block the author wrote.
///
/// The second half is the one that matters: a frame around a group nobody declared is a lie about
/// the diagram, and it is what a bug in the tree-building would produce.
#[test]
fn every_block_that_holds_a_node_gets_exactly_one_frame() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        let d = laid_out(src);
        let tree = tree_of_src(src);
        let drawn: HashSet<&str> = d.clusters.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            drawn.len(),
            d.clusters.len(),
            "{name}: a frame is drawn twice"
        );
        let declared: HashSet<&str> = tree.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(drawn, declared, "{name}: frames and blocks disagree");
    }
}

/// Everything the author wrote is on the page: one shape per node, and one line per edge — an
/// edge that names a block included, now that a block is something a line can end on. A dropped
/// node is how a renderer quietly lies about a diagram.
#[test]
fn every_node_and_drawable_edge_survives() {
    for (name, src) in CORPUS {
        let chart = parse(src).expect("parses");
        let tree = tree_of_src(src);
        let d = laid_out(src);
        assert_eq!(d.nodes.len(), chart.nodes.len(), "{name}: node count");
        let drawable = chart
            .edges
            .iter()
            .filter(|e| {
                let end = |id: &str| chart.node(id).is_some() || tree.contains(id);
                end(&e.from) && end(&e.to)
            })
            .count();
        assert_eq!(d.edges.len(), drawable, "{name}: edge count");
    }
}

// ---------------------------------------------------------------------------------------------
// 3. The shape formulas, stated as arithmetic
// ---------------------------------------------------------------------------------------------

/// A label size with no font behind it, so these tests say the same thing on every machine.
const L: Size = Size { w: 100.0, h: 20.0 };

/// mermaid's `question.ts`: `w = label + padding`, `h = label + padding`, and the drawn square's
/// side is **`w + h`**, not `w`.
///
/// This is the formula that matters most to a reader: sizing a decision box like a rectangle puts
/// its branch labels on top of it. A mutation to `s = w` has to fail here.
#[test]
fn diamond_side_is_the_sum_of_both_padded_axes() {
    let s = flow_size(Shape::Diamond, L);
    let expected = (L.w + shapes::PADDING) + (L.h + shapes::PADDING);
    assert_eq!(s.w, expected);
    assert_eq!(s.h, expected);
    // …and it really is a square standing on its corner.
    let p = shapes::polygon(Glyph::Flow(Shape::Diamond), s);
    assert_eq!(p.len(), 4);
    assert_eq!((p[0].x, p[0].y), (0.0, -expected / 2.0));
    assert_eq!((p[1].x, p[1].y), (expected / 2.0, 0.0));
}

/// `squareRect.ts` -> `drawRect.ts`: `labelPaddingX` is `padding * 2` and `drawRect` doubles it,
/// while `labelPaddingY` is `padding * 1` and is doubled too — so a rectangle is four paddings
/// wider and two paddings taller than its label.
#[test]
fn rect_padding_is_four_by_two() {
    let s = flow_size(Shape::Rect, L);
    assert_eq!(s.w, L.w + shapes::PADDING * 4.0);
    assert_eq!(s.h, L.h + shapes::PADDING * 2.0);
}

/// `stadium.ts`: the height gets **one** padding, not two, and the width carries a quarter of the
/// height for the two caps.
#[test]
fn stadium_height_takes_a_single_padding() {
    let s = flow_size(Shape::Stadium, L);
    let h = L.h + shapes::PADDING;
    assert_eq!(s.h, h);
    assert_eq!(s.w, L.w + h / 4.0 + shapes::PADDING);
    // The outline is a rectangle whose corner radius closes the ends into semicircles.
    assert_eq!(
        shapes::outline(Glyph::Flow(Shape::Stadium), s, None),
        shapes::Outline::Rect {
            w: s.w,
            h: s.h,
            r: s.h / 2.0
        }
    );
}

/// `circle.ts` sizes the ring from the label's **width alone**, so a circle is always round.
#[test]
fn circle_radius_follows_the_label_width() {
    let s = flow_size(Shape::Circle, L);
    assert_eq!(s.w, s.h);
    assert_eq!(s.w, L.w + shapes::PADDING * 2.0);
}

/// `hexagon.ts`: the slanted ends are `h/4` wide each, so the box is half a height wider than the
/// padded label.
#[test]
fn hexagon_ends_are_a_quarter_of_its_height() {
    let s = flow_size(Shape::Hexagon, L);
    let h = L.h + shapes::PADDING;
    assert_eq!(s.h, h);
    assert_eq!(s.w, L.w + h / 2.0 + shapes::PADDING);
    let p = shapes::polygon(Glyph::Flow(Shape::Hexagon), s);
    assert_eq!(p.len(), 6);
    assert_eq!(p[0].x, -s.w / 2.0 + h / 4.0);
}

/// `trapezoid.ts` takes one padding per axis while `invertedTrapezoid.ts` doubles both. The
/// asymmetry is upstream's; recording it here stops it being "tidied up" into a bug.
#[test]
fn trapezoid_and_inverted_trapezoid_pad_differently() {
    let up = flow_size(Shape::Trapezoid, L);
    let down = flow_size(Shape::InvTrapezoid, L);
    assert_eq!(up.h, L.h + shapes::PADDING);
    assert_eq!(down.h, L.h + shapes::PADDING * 2.0);
    assert_eq!(up.w, L.w + shapes::PADDING + up.h);
    assert_eq!(down.w, L.w + shapes::PADDING * 2.0 + down.h);
    // The wide side is the bottom for one and the top for the other.
    let (a, b) = (
        shapes::polygon(Glyph::Flow(Shape::Trapezoid), up),
        shapes::polygon(Glyph::Flow(Shape::InvTrapezoid), down),
    );
    assert!(
        a[0].y > 0.0 && a[0].x == -up.w / 2.0,
        "trapezoid is wide at the bottom"
    );
    assert!(
        b[2].y < 0.0 && b[2].x == down.w / 2.0,
        "inv trapezoid is wide at the top"
    );
}

/// **The label fits inside the shape that was sized for it.**
///
/// Stated over the outline, not the bounding box, because that is where sizing goes wrong: a
/// diamond sized `s = w` instead of `s = w + h` still has a perfectly reasonable bounding box and
/// still puts its text outside the rhombus. Checking the four corners of the label rectangle is
/// what turns "the formula is the one mermaid uses" into "the diagram reads".
///
/// Circles are the documented exception: `circle.ts` derives the radius from the label's **width
/// alone**, so a tall label overflows upstream too. konoma keeps mermaid's formula here (see
/// `shapes::size`) and records the limitation rather than quietly diverging.
#[test]
fn every_shape_holds_the_label_it_was_sized_for() {
    for shape in [
        Shape::Rect,
        Shape::RoundedRect,
        Shape::Stadium,
        Shape::Subroutine,
        Shape::Cylinder,
        Shape::Diamond,
        Shape::Hexagon,
        Shape::Odd,
        Shape::Trapezoid,
        Shape::InvTrapezoid,
        Shape::LeanRight,
        Shape::LeanLeft,
        Shape::Text,
    ] {
        let size = flow_size(shape, L);
        let node = PlacedNode {
            id: format!("{shape:?}"),
            shape: Glyph::Flow(shape),
            center: Point::new(0.0, 0.0),
            size,
            label: Label {
                lines: vec!["x".to_string()],
                width: L.w,
                height: L.h,
            },
            panel: None,
            series: None,
            mark: None,
            style: None,
        };
        let outline = boundary(&node);
        for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let corner = Point::new(sx * L.w / 2.0, sy * L.h / 2.0);
            assert!(
                inside(&corner, &outline),
                "{shape:?}: a {}x{} label reaches ({}, {}), which is outside the {}x{} outline",
                num(L.w),
                num(L.h),
                num(corner.x),
                num(corner.y),
                num(size.w),
                num(size.h)
            );
        }
    }
    // What mermaid does guarantee for a circle: the label's width fits across it.
    for shape in [Shape::Circle, Shape::DoubleCircle] {
        let size = flow_size(shape, L);
        assert!(size.w >= L.w, "{shape:?}: the label is wider than the ring");
    }
}

/// A diamond's edges leave from its slanted sides, which is the whole point of intersecting the
/// outline instead of the bounding box. A point due east of the centre leaves at the right
/// corner; a point to the north-east leaves part-way along the upper-right side, strictly inside
/// the bounding box's corner.
#[test]
fn diamond_intersection_uses_the_slanted_sides() {
    let s = Size::new(100.0, 100.0);
    let c = Point::new(0.0, 0.0);
    let east = shapes::intersect(
        Glyph::Flow(Shape::Diamond),
        c.clone(),
        s,
        &Point::new(500.0, 0.0),
    );
    assert!((east.x - 50.0).abs() < 1e-9 && east.y.abs() < 1e-9);

    let ne = shapes::intersect(
        Glyph::Flow(Shape::Diamond),
        c,
        s,
        &Point::new(500.0, -500.0),
    );
    // On the side |x| + |y| = 50, and well inside the box corner (50, -50).
    assert!(
        (ne.x.abs() + ne.y.abs() - 50.0).abs() < 1e-9,
        "on the slanted side"
    );
    assert!(
        ne.x < 49.0 && ne.y > -49.0,
        "not at the bounding box's corner"
    );
}

// ---------------------------------------------------------------------------------------------
// 4. Edge geometry
// ---------------------------------------------------------------------------------------------

/// d3's `curveBasis` — mermaid's default. It touches the first and last point and **no** other:
/// the rest are control points. Pinning the exact `d` for three known points fixes the whole
/// construction, ends included (d3 draws a straight sixth of the first and last segment).
#[test]
fn curve_basis_matches_d3() {
    let pts = [
        Point::new(0.0, 0.0),
        Point::new(60.0, 0.0),
        Point::new(60.0, 60.0),
    ];
    assert_eq!(
        edges::curve_basis_path(&pts),
        "M0,0L10,0C20,0 40,0 50,10C60,20 60,40 60,50L60,60"
    );
    // Two points are just a line; one is just a move.
    assert_eq!(edges::curve_basis_path(&pts[..2]), "M0,0L60,0");
    assert_eq!(edges::curve_basis_path(&pts[..1]), "M0,0");
    // Collinear points (vertical, horizontal) and a duplicated point — this section's own
    // `VERTICAL`/`HORIZONTAL`/`DUPLICATE` module docs have the reason these three shapes are
    // swept over every curve, `basis` included even though it was never the one that broke.
    let vertical = [
        Point::new(100.0, 50.0),
        Point::new(100.0, 120.0),
        Point::new(100.0, 190.0),
    ];
    assert_eq!(
        edges::curve_basis_path(&vertical),
        "M100,50L100,61.667C100,73.333 100,96.667 100,120\
         C100,143.333 100,166.667 100,178.333L100,190"
    );
    let horizontal = [
        Point::new(50.0, 100.0),
        Point::new(120.0, 100.0),
        Point::new(190.0, 100.0),
    ];
    assert_eq!(
        edges::curve_basis_path(&horizontal),
        "M50,100L61.667,100C73.333,100 96.667,100 120,100\
         C143.333,100 166.667,100 178.333,100L190,100"
    );
    let duplicate = [
        Point::new(0.0, 0.0),
        Point::new(40.0, 40.0),
        Point::new(40.0, 40.0),
        Point::new(80.0, 80.0),
    ];
    assert_eq!(
        edges::curve_basis_path(&duplicate),
        "M0,0L6.667,6.667C13.333,13.333 26.667,26.667 33.333,33.333\
         C40,40 40,40 46.667,46.667C53.333,53.333 66.667,66.667 73.333,73.333L80,80"
    );
}

/// `edges.js`'s `fixCorners` rounds a right angle off with a radius of 5, replacing the corner
/// with three points. A bend that is too shallow to matter is left alone.
#[test]
fn fix_corners_rounds_right_angles_only() {
    let square = [
        Point::new(0.0, 0.0),
        Point::new(0.0, 100.0),
        Point::new(100.0, 100.0),
    ];
    let fixed = edges::fix_corners(&square);
    assert_eq!(fixed.len(), 5, "the corner becomes three points");
    assert!(
        fixed[1].y < 100.0 && fixed[3].x > 0.0,
        "the replacement points step back from the corner"
    );
    // A corner whose arms are shorter than the 5px test is not a corner.
    let tiny = [
        Point::new(0.0, 0.0),
        Point::new(0.0, 4.0),
        Point::new(4.0, 4.0),
    ];
    assert_eq!(edges::fix_corners(&tiny).len(), 3);
    // A straight line has nothing to round.
    let straight = [
        Point::new(0.0, 0.0),
        Point::new(0.0, 50.0),
        Point::new(0.0, 100.0),
    ];
    assert_eq!(edges::fix_corners(&straight).len(), 3);
}

/// The label goes at the half-way point **by arc length**, so uneven waypoint spacing does not
/// drag it towards the crowded end. mermaid's `traverseEdge`.
#[test]
fn arc_midpoint_is_measured_by_length_not_by_waypoint() {
    // Three waypoints, but the second sits at 90% of the length: a "middle waypoint" rule would
    // answer (90, 0) where the real midpoint is (50, 0).
    let line = [
        Point::new(0.0, 0.0),
        Point::new(90.0, 0.0),
        Point::new(100.0, 0.0),
    ];
    let mid = edges::arc_midpoint(&line).expect("has a midpoint");
    assert!((mid.x - 50.0).abs() < 1e-9 && mid.y.abs() < 1e-9);
    assert_eq!(edges::arc_midpoint(&[]), None);
}

/// An arrow head is `ARROW_LENGTH` long, points along the last segment, and has its tip exactly
/// on the point it was given — the point clipping put on the shape's outline.
#[test]
fn arrow_head_points_along_the_last_segment() {
    let head = edges::arrow_head(&Point::new(0.0, 0.0), &Point::new(0.0, 100.0));
    assert_eq!((head[0].x, head[0].y), (0.0, 100.0));
    assert!((head[1].y - (100.0 - edges::ARROW_LENGTH)).abs() < 1e-9);
    assert!((head[1].x - head[2].x).abs() - edges::ARROW_HALF_WIDTH * 2.0 < 1e-9);
}

/// Every flowchart arrow spelling puts a mark on **exactly the ends it names**, and nothing on
/// the end it does not.
///
/// Until this test existed the whole arrow family was guarded by the two goldens alone, and a
/// mutation proved what that is worth: turning `Tip::of_arrow(Arrow::Point)` from
/// `(Tip::None, Tip::Arrow)` into `(Tip::Arrow, Tip::Arrow)` — a one-way arrow quietly growing a
/// second head — reddened `emit_golden` and `corpus_golden` and **not one named test**. That is
/// the shape `docs/STATUS.md` records as having cost konoma twice: behaviour a golden alone holds
/// is lost silently the next time the golden is regenerated. The class and ER families already
/// had their own version of this check
/// ([`super::er_tests::a_crows_foot_is_drawn_at_the_end_that_asked_for_it`]); the flowchart family
/// did not.
///
/// Stated twice on purpose. Once about the **model**, which end asked for which mark, and once
/// about the **drawing**, how many of each mark actually reach the page — `svg::emit_edge` decides
/// for itself what a `Tip` looks like, so the two can disagree.
#[test]
fn a_flowchart_arrow_marks_only_the_ends_that_spelled_a_mark() {
    if !text_metrics::fonts_available() {
        return;
    }
    // (what the author wrote, what it parses to, the mark at the tail, the mark at the head).
    let cases: &[(&str, Arrow, Tip, Tip)] = &[
        ("---", Arrow::None, Tip::None, Tip::None),
        ("-->", Arrow::Point, Tip::None, Tip::Arrow),
        ("--x", Arrow::Cross, Tip::None, Tip::Cross),
        ("--o", Arrow::Circle, Tip::None, Tip::Circle),
        ("<-->", Arrow::DoublePoint, Tip::Arrow, Tip::Arrow),
        ("x--x", Arrow::DoubleCross, Tip::Cross, Tip::Cross),
        ("o--o", Arrow::DoubleCircle, Tip::Circle, Tip::Circle),
        // The two halves of an infix link disagreeing (§2-6 rule 6). mermaid calls it `INVALID`
        // and konoma draws the shaft with nothing on either end, rather than guessing.
        ("x-- t -->", Arrow::Invalid, Tip::None, Tip::None),
    ];

    // The table covers the whole enum: a new `Arrow` variant makes `covered` fail to compile, and
    // a variant left out of the table fails here.
    fn covered(a: Arrow) -> bool {
        match a {
            Arrow::None
            | Arrow::Point
            | Arrow::Cross
            | Arrow::Circle
            | Arrow::DoublePoint
            | Arrow::DoubleCross
            | Arrow::DoubleCircle
            | Arrow::Invalid => true,
        }
    }
    let spelled: HashSet<Arrow> = cases.iter().map(|(_, a, _, _)| *a).collect();
    assert_eq!(spelled.len(), cases.len(), "an arrow is spelled twice");
    for a in &spelled {
        assert!(covered(*a));
    }

    // How many marks of each kind the emitted document actually carries. Both nodes are `[]`
    // rectangles, so a `<polygon>` can only be an arrow head and a `<circle>` can only be a `--o`
    // disc; a `--x` is the one `<path>` whose `d` carries a second `M` (a curve never does).
    let drawn = |svg: &str| -> (usize, usize, usize) {
        let heads = svg.matches("<polygon").count();
        let crosses = svg
            .lines()
            .filter(|l| l.starts_with("<path") && l.matches('M').count() > 1)
            .count();
        let discs = svg.matches("<circle").count();
        (heads, crosses, discs)
    };
    let wanted = |tip: Tip| -> (usize, usize, usize) {
        match tip {
            Tip::Arrow => (1, 0, 0),
            Tip::Cross => (0, 1, 0),
            Tip::Circle => (0, 0, 1),
            _ => (0, 0, 0),
        }
    };

    for (spelling, arrow, tip_start, tip_end) in cases {
        let src = format!("flowchart LR\n  A[a] {spelling} B[b]\n");
        let chart = parse(&src).unwrap_or_else(|e| panic!("{spelling}: {e}"));
        assert_eq!(chart.edges.len(), 1, "{spelling}: one edge");
        assert_eq!(chart.edges[0].arrow, *arrow, "{spelling}: parsed arrow");

        let d = laid_out(&src);
        assert_eq!(d.edges.len(), 1, "{spelling}: one edge survives layout");
        let e = &d.edges[0];
        assert_eq!(
            (e.tip_start, e.tip_end),
            (*tip_start, *tip_end),
            "{spelling}: the marks are on the wrong ends"
        );

        let svg = render(&src, "dark").expect("renders");
        let (heads, crosses, discs) = drawn(&svg);
        let (ws, wc, wd) = wanted(*tip_start);
        let (we, wcc, wdd) = wanted(*tip_end);
        assert_eq!(
            (heads, crosses, discs),
            (ws + we, wc + wcc, wd + wdd),
            "{spelling}: {heads} arrow heads, {crosses} crosses and {discs} discs were drawn for \
             ({tip_start:?}, {tip_end:?})"
        );
    }
}

/// The line is pulled back so the arrow head has room, but never past the point of having no
/// direction left.
#[test]
fn trimming_leaves_a_line_with_a_direction() {
    let line = [Point::new(0.0, 0.0), Point::new(0.0, 100.0)];
    let cut = edges::trim_end(&line, edges::ARROW_LENGTH);
    assert!((cut.last().unwrap().y - 91.0).abs() < 1e-9);
    let stub = [Point::new(0.0, 0.0), Point::new(0.0, 3.0)];
    assert_eq!(edges::trim_end(&stub, edges::ARROW_LENGTH), stub.to_vec());
}

/// `~~~` is a layout constraint that draws nothing, and it must still have moved the layout.
#[test]
fn invisible_edge_constrains_but_does_not_draw() {
    let with = laid_out("flowchart TD\n  A[aaa]\n  B[bbb]\n  A ~~~ B");
    let without = laid_out("flowchart TD\n  A[aaa]\n  B[bbb]");
    assert!(
        with.height > without.height,
        "the invisible edge should have pushed B onto its own rank"
    );
    let svg = render("flowchart TD\n  A[aaa]\n  B[bbb]\n  A ~~~ B", "dark").expect("renders");
    assert_eq!(
        svg.matches("<path").count(),
        0,
        "an invisible edge emits no path"
    );
}

// ---------------------------------------------------------------------------------------------
// 5. The SVG contract (§1) and the round trip through resvg
// ---------------------------------------------------------------------------------------------

/// §1's constraints, each of which is a thing resvg was measured doing rather than a style rule.
#[test]
fn emitted_svg_obeys_the_renderer_contract() {
    for (name, src) in CORPUS {
        for theme_name in ["dark", "light", "classic", "forest", "neutral"] {
            let svg = render(src, theme_name).expect("renders");
            assert!(
                svg.contains("viewBox=\""),
                "{name}/{theme_name}: without a viewBox usvg falls back to 100x100"
            );
            assert!(svg.contains("width=\"") && svg.contains("height=\""));
            assert!(
                !svg.contains("foreignObject"),
                "{name}/{theme_name}: resvg drops <foreignObject> whole, label and all"
            );
            assert!(
                !svg.contains("var(--"),
                "{name}/{theme_name}: usvg cannot resolve CSS variables — fills go black"
            );
            assert!(
                !svg.contains("<style") && !svg.contains("class=\"node default\""),
                "{name}/{theme_name}: nothing may depend on a stylesheet"
            );
            assert!(
                svg.contains("font-family=\"sans-serif\""),
                "{name}/{theme_name}: the drawing font must be the one that was measured"
            );
            // The only full-canvas rect is the transparent one.
            let opaque_background = svg
                .lines()
                .any(|l| l.starts_with("<rect width=") && !l.contains("fill=\"none\""));
            assert!(
                !opaque_background,
                "{name}/{theme_name}: an opaque background turns the diagram into a card"
            );
        }
    }
}

/// The output goes through konoma's own rasteriser, which is the thing that will actually draw it.
#[test]
fn every_corpus_diagram_rasterises() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CORPUS {
        let svg = render(src, "dark").expect("renders");
        let img = crate::preview::svg::rasterize_bytes(svg.as_bytes(), FsPath::new("m.svg"), 600)
            .unwrap_or_else(|| panic!("{name}: konoma's resvg could not rasterise the output"));
        assert!(img.width() > 0 && img.height() > 0, "{name}: empty raster");
    }
}

/// A diagram that draws nothing at all would rasterise to a transparent rectangle, which is
/// exactly the failure §6 calls out in the current crate (`graph TD` alone becoming a 16x16
/// transparent SVG blown up to full width). Some ink has to reach the pixmap.
#[test]
fn a_rendered_diagram_actually_has_ink_in_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    let svg = render("flowchart TD\n  A[Start] --> B{Ready?}", "dark").expect("renders");
    let img = crate::preview::svg::rasterize_bytes(svg.as_bytes(), FsPath::new("m.svg"), 400)
        .expect("rasterises");
    let opaque = img.to_rgba8().pixels().filter(|p| p.0[3] > 32).count();
    assert!(
        opaque > 500,
        "only {opaque} pixels were drawn — the diagram is effectively blank"
    );
}

// ---------------------------------------------------------------------------------------------
// 6. Themes (§4)
// ---------------------------------------------------------------------------------------------

/// Every colour is a normalised lowercase hex, and the background is never painted.
#[test]
fn every_theme_colour_is_normalised_hex() {
    for t in theme::ALL {
        assert_eq!(
            t.background_paint, "none",
            "{}: the background is a reference colour, never a paint (§4-2)",
            t.name
        );
        for (field, value) in [
            ("background_ref", t.background_ref),
            ("node_fill", t.node_fill),
            ("node_stroke", t.node_stroke),
            ("node_text", t.node_text),
            ("line", t.line),
            ("arrowhead", t.arrowhead),
            ("edge_label_text", t.edge_label_text),
            ("cluster_fill", t.cluster_fill),
            ("cluster_stroke", t.cluster_stroke),
            ("cluster_text", t.cluster_text),
            ("state_marker", t.state_marker),
            ("note_fill", t.note_fill),
            ("note_stroke", t.note_stroke),
            ("note_text", t.note_text),
        ] {
            assert!(
                theme::parse_hex(value).is_some(),
                "{}.{field} = {value:?} is not a normalised #rrggbb",
                t.name
            );
        }
    }
}

/// **§4-1's decision, as a test.** konoma never paints a background, so a light theme's diagram
/// can end up composited on a dark terminal. mermaid's own `forest` resolves its line colour to
/// `#000000` (`invert(background)` on line 31 overwrites the `green` on line 17) and vanishes
/// there. Every light-series palette here must stay legible against **both** grounds; the dark
/// palette only has to answer for the dark one.
#[test]
fn lines_survive_being_composited_on_either_ground() {
    const FLOOR: f64 = 2.2;
    for t in theme::ALL {
        // `state_marker` is here and not with the fills below because it is the one *filled*
        // shape with nothing behind it: the `[*]` dots and the fork bars are solid marks on the
        // terminal itself, so a palette that picked a dark one would make them vanish outright
        // rather than merely go faint.
        for (field, colour) in [
            ("line", t.line),
            ("arrowhead", t.arrowhead),
            ("state_marker", t.state_marker),
        ] {
            let on_black = Theme::contrast(colour, "#000000");
            assert!(
                on_black >= 3.0,
                "{}.{field} = {colour} has contrast {:.2} on black — it disappears on a dark \
                 terminal (this is what happens to mermaid's forest)",
                t.name,
                on_black
            );
            if t.name != "dark" {
                let on_white = Theme::contrast(colour, "#ffffff");
                assert!(
                    on_white >= FLOOR,
                    "{}.{field} = {colour} has contrast {:.2} on white",
                    t.name,
                    on_white
                );
            }
        }
        // Node text has to read against the fill it is drawn on, whatever the terminal does.
        assert!(
            Theme::contrast(t.node_text, t.node_fill) >= 4.0,
            "{}: node text {} on fill {} has contrast {:.2}",
            t.name,
            t.node_text,
            t.node_fill,
            Theme::contrast(t.node_text, t.node_fill)
        );
        // A note's text is drawn on the note's own fill, which is opaque, so this is the same
        // question as node text and gets the same floor.
        assert!(
            Theme::contrast(t.note_text, t.note_fill) >= 4.0,
            "{}: note text {} on fill {} has contrast {:.2}",
            t.name,
            t.note_text,
            t.note_fill,
            Theme::contrast(t.note_text, t.note_fill)
        );
        // An edge label is drawn on the reference colour, not on the terminal.
        assert!(
            Theme::contrast(t.edge_label_text, t.background_ref) >= 4.0,
            "{}: edge label text {} on {} has contrast {:.2}",
            t.name,
            t.edge_label_text,
            t.background_ref,
            Theme::contrast(t.edge_label_text, t.background_ref)
        );
    }
}

/// The five names konoma promises, plus mermaid's own spellings — and an unknown value is `dark`,
/// silently, which is the behaviour `mermaid_to_svg` has today (§1).
#[test]
fn theme_names_resolve_the_way_the_config_promises() {
    assert_eq!(Theme::named("dark").name, "dark");
    assert_eq!(Theme::named("light").name, "light");
    assert_eq!(Theme::named("modern").name, "light");
    assert_eq!(Theme::named("classic").name, "classic");
    assert_eq!(Theme::named("mermaid").name, "classic");
    assert_eq!(Theme::named("forest").name, "forest");
    assert_eq!(Theme::named("neutral").name, "neutral");
    assert_eq!(Theme::named("").name, "dark");
    assert_eq!(Theme::named("Dracula").name, "dark");
}

/// A theme changes colours and nothing else.
///
/// Structurally guaranteed here — [`lay_out`] never sees a theme — but worth a test anyway,
/// because the bug it guards against did happen: when each theme measured with its own font, the
/// label boxes changed size per theme and an edge took a detour ring (reported from a real
/// terminal on 2026-07-17; §4-3). The comparison strips every `fill`/`stroke` colour and keeps
/// the rest, so a palette that quietly moved a coordinate cannot pass.
#[test]
fn themes_change_colours_but_never_geometry() {
    let src = "flowchart TD\n  A[Start] -->|go| B{Ready?}\n  B --> C((end))";
    let geometry_of = |s: &str| {
        s.lines()
            .map(|l| {
                l.split_whitespace()
                    .filter(|w| !w.starts_with("fill=") && !w.starts_with("stroke=\""))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let base = geometry_of(&render(src, "dark").expect("renders"));
    for name in ["light", "classic", "forest", "neutral"] {
        assert_eq!(
            base,
            geometry_of(&render(src, name).expect("renders")),
            "theme {name} moved something"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 7. Degrading rather than lying (§6)
// ---------------------------------------------------------------------------------------------

/// A diagram of another kind must not be drawn. The crate konoma ships turns `venn-beta` into
/// three boxes labelled "venn", "beta" and "sets" — a picture of something nobody wrote.
#[test]
fn another_diagram_kind_is_refused() {
    assert!(matches!(
        render("venn-beta\n  sets: [a, b]", "dark"),
        Err(RenderError::Parse(_))
    ));
    assert!(matches!(
        render("sequenceDiagram\n  A->>B: hi", "dark"),
        Err(RenderError::Parse(_))
    ));
}

/// A header with no body is not a diagram either. The current crate answers `graph TD` with a
/// 16x16 transparent SVG that gets stretched to full width.
#[test]
fn a_flowchart_with_no_nodes_is_refused() {
    assert!(matches!(
        render("graph TD", "dark"),
        Err(RenderError::Parse(_))
    ));
}

/// A block is a frame, not a box: it is drawn, but it is not a node and it holds no label of its
/// own beyond the title. Its members stay ordinary nodes with ordinary edges.
#[test]
fn a_block_is_a_frame_and_not_a_node() {
    let d =
        laid_out("flowchart TD\n  subgraph one [Group]\n    A --> B\n  end\n  B --> C\n  C --> A");
    assert_eq!(d.nodes.len(), 3, "every member is still drawn");
    assert_eq!(d.edges.len(), 3);
    assert!(d.node("one").is_none(), "the block is not a node");
    let frame = d.cluster("one").expect("the block is a frame");
    assert_eq!(frame.title.lines, vec!["Group".to_string()]);
    assert_eq!(frame.parent, None);
    assert_eq!(frame.depth, 0);
}

/// An edge that names a block is drawn, and it points at the frame.
///
/// Stage 1c dropped it: the parser removes a subgraph id from the node list, so there was no
/// shape to attach the line to. Stage 1d re-anchors it onto a member for the layout and cuts it
/// back to the frame for the drawing.
#[test]
fn an_edge_can_name_a_block() {
    let d = laid_out(
        "flowchart TD\n  subgraph one [Group]\n    A --> B\n  end\n  one --> C\n  D --> one",
    );
    assert!(d.node("one").is_none(), "the block is still not a node");
    let named: Vec<&PlacedEdge> = d
        .edges
        .iter()
        .filter(|e| e.from == "one" || e.to == "one")
        .collect();
    assert_eq!(named.len(), 2, "both edges onto the block survive");
    for e in named {
        assert!(e.points.len() >= 2, "{} -> {} has a line", e.from, e.to);
    }
}

/// A block that holds no node at all is not drawn, and an edge that names it is dropped.
///
/// dagre gives a parent its rectangle through the border nodes it hangs off its children; a
/// parent with no children never gets any, and reading a box back from it would produce a frame
/// at the origin with no size. Dropping it is the honest answer.
#[test]
fn an_empty_block_is_not_drawn() {
    let d = laid_out("flowchart TD\n  subgraph hollow [Nothing]\n  end\n  A --> B\n  hollow --> A");
    assert!(d.clusters.is_empty(), "no frame for an empty block");
    assert_eq!(d.edges.len(), 1, "the edge that named it is dropped");
    assert_eq!(d.nodes.len(), 2);
}

/// A `direction` inside a block is read by the parser and **deliberately not applied**.
///
/// See [`super::lay_out`]'s note: dagre has one `rankdir` per layout, honouring a per-block one
/// means laying that block out in a graph of its own, and that is the change mermaid shipped in
/// 11.16.0 and took back in 11.17.0 because the arrows *between* blocks broke. Pinning it as a
/// test rather than as a comment means a future stage that changes its mind has to say so here.
#[test]
fn a_block_direction_does_not_move_anything() {
    let with = laid_out(
        "flowchart LR\n  subgraph one [Steps]\n    direction TB\n    A --> B\n  end\n  B --> C",
    );
    let without = laid_out("flowchart LR\n  subgraph one [Steps]\n    A --> B\n  end\n  B --> C");
    assert_eq!(with.nodes, without.nodes);
    assert_eq!(with.clusters, without.clusters);
}

/// Errors carry the parser's reason. §8 notes the current crate throws these away, losing
/// messages as useful as "unknown participant 'API' at line 3".
#[test]
fn errors_say_what_was_wrong() {
    let e = render("venn-beta", "dark").unwrap_err();
    assert!(
        e.to_string().contains("venn-beta"),
        "the message should name what it found: {e}"
    );
}

// ---------------------------------------------------------------------------------------------
// 8. Goldens
// ---------------------------------------------------------------------------------------------

/// The regression net over the whole corpus, **through [`render`]** — the function stage 1e will
/// wire into production, not a stand-in for it (§6, after konoma was once caught pinning a golden
/// on a function nothing called).
///
/// Numeric attribute values are masked; see the module docs for why (every coordinate descends
/// from the system font, so a byte-exact end-to-end golden would be a golden of this machine).
/// What is pinned is everything else: which elements, in what order, with which colours, which
/// path commands, and which text.
#[test]
fn corpus_golden() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut out = String::new();
    for (name, src) in CORPUS {
        out.push_str(&format!("=== {name} ===\n"));
        out.push_str(&mask_numbers(&render(src, "dark").expect("renders")));
        out.push('\n');
    }
    assert_snapshot("mermaid_render", &out);
}

/// A diagram assembled by hand, so **no font is involved and every number is pinned exactly**.
///
/// This is the other half of the golden: the corpus golden pins structure over real sources, this
/// one pins arithmetic — number formatting, the outline paths of all fifteen shapes, the curve,
/// the arrow heads, the dash patterns and the label patch.
#[test]
fn emit_golden() {
    let d = synthetic_diagram();
    let mut out = String::new();
    for t in theme::ALL {
        out.push_str(&format!("=== {} ===\n", t.name));
        out.push_str(&super::svg::emit(&d, t));
        out.push('\n');
    }
    assert_snapshot("mermaid_emit", &out);
}

/// One node of every shape and one edge of every stroke/arrow spelling, at coordinates chosen by
/// hand. Nothing here is measured, so it renders identically on every machine.
fn synthetic_diagram() -> Diagram {
    let label = |text: &str, w: f64| Label {
        lines: text.split('\n').map(str::to_string).collect(),
        width: w,
        height: text.split('\n').count() as f64 * super::labels::line_height(),
    };
    let shapes_in_order = [
        Shape::Rect,
        Shape::RoundedRect,
        Shape::Stadium,
        Shape::Subroutine,
        Shape::Cylinder,
        Shape::Circle,
        Shape::DoubleCircle,
        Shape::Diamond,
        Shape::Hexagon,
        Shape::Odd,
        Shape::Trapezoid,
        Shape::InvTrapezoid,
        Shape::LeanRight,
        Shape::LeanLeft,
        Shape::Text,
    ];
    let mut nodes = Vec::new();
    for (i, shape) in shapes_in_order.into_iter().enumerate() {
        let text = if i == 0 { "two\nlines" } else { "label" };
        let l = label(text, 40.0);
        let size = flow_size(shape, Size::new(l.width, l.height));
        nodes.push(PlacedNode {
            id: format!("n{i}"),
            shape: Glyph::Flow(shape),
            center: Point::new(
                120.0 + (i % 5) as f64 * 200.0,
                80.0 + (i / 5) as f64 * 160.0,
            ),
            size,
            label: l,
            panel: None,
            series: None,
            mark: None,
            style: None,
        });
    }

    let arrows = [
        (Arrow::Point, Stroke::Normal),
        (Arrow::None, Stroke::Normal),
        (Arrow::Cross, Stroke::Dotted),
        (Arrow::Circle, Stroke::Thick),
        (Arrow::DoublePoint, Stroke::Normal),
        (Arrow::DoubleCross, Stroke::Normal),
        (Arrow::DoubleCircle, Stroke::Normal),
        (Arrow::Invalid, Stroke::Invalid),
        (Arrow::Point, Stroke::Invisible),
    ];
    let mut edges = Vec::new();
    for (i, (arrow, stroke)) in arrows.into_iter().enumerate() {
        let y = 420.0 + i as f64 * 30.0;
        let points = vec![
            Point::new(60.0, y),
            Point::new(200.0, y),
            Point::new(200.0, y + 20.0),
            Point::new(380.0, y + 20.0),
        ];
        let l = label("via", 20.0);
        edges.push(PlacedEdge {
            from: format!("n{i}"),
            to: format!("n{}", i + 1),
            label: (i % 2 == 0).then(|| super::PlacedEdgeLabel {
                center: edges::arc_midpoint(&points).expect("midpoint"),
                size: Size::new(
                    l.width + super::LABEL_PAD_X * 2.0,
                    l.height + super::LABEL_PAD_Y * 2.0,
                ),
                label: l,
            }),
            points,
            gaps: Vec::new(),
            tip_start: super::Tip::of_arrow(arrow).0,
            tip_end: super::Tip::of_arrow(arrow).1,
            stroke,
            start_label: None,
            end_label: None,
            badge: None,
            series: None,
            straight: false,
            overlay: false,
            style: None,
            curve: Curve::Basis,
            tip_matches_line: false,
        });
    }

    // Two frames, one inside the other, at coordinates chosen by hand: the outer one titled on
    // two lines (the case that makes a frame grow) and the inner one untitled (the case that
    // emits a rect and no text at all).
    let clusters = vec![
        PlacedCluster {
            id: "outer".to_string(),
            title: label("Outer frame\nwith two lines", 130.0),
            center: Point::new(300.0, 620.0),
            size: Size::new(360.0, 160.0),
            parent: None,
            depth: 0,
            dashed: false,
            filled: true,
            sections: Vec::new(),
            title_strip: false,
        },
        PlacedCluster {
            id: "inner".to_string(),
            title: label("", 0.0),
            center: Point::new(320.0, 645.0),
            size: Size::new(200.0, 90.0),
            parent: Some("outer".to_string()),
            depth: 1,
            dashed: false,
            filled: true,
            sections: Vec::new(),
            title_strip: false,
        },
    ];

    Diagram {
        width: 1040.0,
        height: 720.0,
        nodes,
        edges,
        clusters,
        lifelines: Vec::new(),
    }
}

/// The number formatter, which every coordinate goes through. Three decimals, no trailing zeros,
/// and no `-0` (which is a legal float and an ugly attribute).
#[test]
fn numbers_are_written_the_same_way_every_time() {
    assert_eq!(num(12.0), "12");
    assert_eq!(num(12.5), "12.5");
    assert_eq!(num(1.0 / 3.0), "0.333");
    assert_eq!(num(-0.0001), "0");
    assert_eq!(num(-12.25), "-12.25");
}

/// Text that would otherwise break the document is escaped, in both content and attributes.
#[test]
fn label_text_is_escaped() {
    let svg = super::svg::escape("a < b & c > \"d\"");
    assert_eq!(svg, "a &lt; b &amp; c &gt; &quot;d&quot;");
    if text_metrics::fonts_available() {
        let out = render(
            "flowchart TD\n  A[\"a &lt; b\"] --> B[\"x &amp; y\"]",
            "dark",
        )
        .expect("renders");
        assert!(!out.contains("a < b"), "a raw < would break the document");
        assert!(usvg::Tree::from_data(
            out.as_bytes(),
            &usvg::Options {
                fontdb: shared_fontdb(),
                ..usvg::Options::default()
            }
        )
        .is_ok());
    }
}

/// Sources chosen to be awkward rather than realistic. konoma's rule is that the renderer runs on
/// a worker thread wrapped in `catch_unwind` (§1) — which is a safety net, not a licence — so
/// each of these has to come back with a diagram or an error, never a panic and never a diagram
/// with a NaN in it.
#[test]
fn awkward_sources_produce_a_diagram_or_an_error_and_never_a_panic() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut wide = String::from("flowchart LR\n");
    for i in 0..120 {
        wide.push_str(&format!("  n{i} --> n{}\n", i + 1));
    }
    wide.push_str("  n0 --> n120\n  n120 --> n0\n");

    // Twelve blocks, each inside the last. mermaid's own `adjustClustersAndEdges` gives up past a
    // depth of ten; konoma's tree walk is bounded by the number of blocks instead, so this asks
    // whether that bound is really there.
    let mut deep = String::from("flowchart TD\n");
    for i in 0..12 {
        deep.push_str(&format!("  subgraph s{i} [Level {i}]\n"));
    }
    deep.push_str("  A --> B\n");
    for _ in 0..12 {
        deep.push_str("  end\n");
    }
    deep.push_str("  s0 --> C\n  C --> s11\n");

    let cases: &[&str] = &[
        "",
        "   \n\n  ",
        "flowchart TD",
        "flowchart TD\n  A[\"\"]",
        "flowchart TD\n  A[\"   \"] --> B[\"\"]",
        "flowchart TD\n  A -->|| B",
        "flowchart TD\n  A --> A",
        "flowchart LR\n  A[\"<script>&amp;\"] --> B[\"a > b\"]",
        "flowchart TD\n  A[\"一\"] --> B[\"🎉\"]",
        "flowchart TD\n  %% only a comment\n  A --> B",
        "flowchart TD\n  classDef hot fill:#f9f\n  A:::hot --> B",
        "flowchart TD\n  A@{ shape: cyl, label: \"store\" } --> B",
        // Blocks, in the shapes a person would not write on purpose.
        "flowchart TD\n  subgraph one\n  end\n  A --> B",
        "flowchart TD\n  subgraph one\n  end\n  one --> one",
        "flowchart TD\n  subgraph one [Group]\n    A --> B\n  end\n  one --> A",
        "flowchart TD\n  subgraph one [Group]\n    A --> B\n  end\n  one --> one",
        "flowchart LR\n  subgraph one\n    subgraph two\n      subgraph three\n        A\n               end\n    end\n  end\n  one --> three",
        "flowchart TD\n  subgraph one [\"\"]\n    A\n  end\n  A --> B",
        "flowchart TD\n  A --> one\n  subgraph one [Declared after the edge]\n    B --> C\n  end",
        &deep,
        &wide,
    ];
    for src in cases {
        match render(src, "dark") {
            Err(_) => {}
            Ok(svg) => {
                assert!(!svg.contains("NaN"), "NaN reached the document for {src:?}");
                assert!(
                    !svg.contains("inf"),
                    "an infinity reached the document for {src:?}"
                );
                let d = laid_out(src);
                assert!(
                    d.width.is_finite() && d.height.is_finite() && d.width > 0.0 && d.height > 0.0,
                    "{src:?}: {}x{} is not a drawable size",
                    num(d.width),
                    num(d.height)
                );
                assert!(
                    crate::preview::svg::rasterize_bytes(svg.as_bytes(), FsPath::new("m.svg"), 300)
                        .is_some(),
                    "{src:?}: the output did not rasterise"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 9. `classDef` / `class` / `:::` / `style` / `linkStyle` (docs/STATUS.md, 2026-08-28 regression)
// ---------------------------------------------------------------------------------------------
//
// The bug this section pins: `flowchart/parser.rs` always kept `class_defs` and `link_styles` —
// only so `classDef hot fill:#f9f` could not become a phantom node (§2-3) — and nothing
// downstream ever read what they said. `mask_numbers` (this file's module docs) hides
// `stroke-width`/`stroke-dasharray` but **not** `fill`/`stroke`, so a colour regression here would
// have shown up in `corpus_golden` if the corpus had a case for it; it did not (`CORPUS` has no
// `classDef` source), which is exactly how this shipped unnoticed. These tests read
// [`PlacedNode::style`]/[`PlacedEdge::style`] directly rather than relying on a golden, so a future
// regression fails by name instead of by an unrelated-looking snapshot diff.

/// `classDef default` colours every node automatically, and must **not** leak onto an edge that
/// carries neither a `class` of its own nor any `linkStyle` — caught while regenerating the
/// corpus golden for this section: [`super::style::cascade_edge`] used to start from
/// [`super::style::cascade`], which begins every element at `classDef default`, and an edge is
/// not a node.
#[test]
fn classdef_default_colours_nodes_but_not_an_untouched_edge() {
    let d = laid_out("flowchart TD\n  classDef default fill:#223,stroke:#556\n  A --> B");
    assert_eq!(
        d.node("A")
            .unwrap()
            .style
            .as_ref()
            .unwrap()
            .stroke
            .as_deref(),
        Some("#556"),
        "classDef default must still colour every node"
    );
    assert!(
        d.edges[0].style.is_none(),
        "the edge names neither a class nor a linkStyle, so classDef default must not reach it"
    );

    let svg = render(
        "flowchart TD\n  classDef default fill:#223,stroke:#556\n  A --> B",
        "dark",
    )
    .expect("renders");
    assert_eq!(
        svg.matches("stroke=\"#556\"").count(),
        2,
        "exactly the two node rects (A and B) may carry classDef default's stroke, never the edge's path: {svg}"
    );
    assert!(
        svg.contains(&format!("stroke=\"{}\"", theme::DARK.line)),
        "the edge's path must still draw in the theme's own line colour: {svg}"
    );
}

/// `classDef` + `class` colours the node it names, and leaves every other node at the theme's own
/// colour.
#[test]
fn classdef_and_class_colour_the_node_they_name() {
    let d =
        laid_out("flowchart TD\n  classDef hot fill:#f9f,stroke:#a00\n  A --> B\n  class A hot");
    let a = d
        .node("A")
        .expect("A exists")
        .style
        .as_ref()
        .expect("A has a style");
    assert_eq!(a.fill.as_deref(), Some("#f9f"));
    assert_eq!(a.stroke.as_deref(), Some("#a00"));
    assert!(
        d.node("B").expect("B exists").style.is_none(),
        "class hot named only A, so B must keep the theme's own colour"
    );

    let svg = render(
        "flowchart TD\n  classDef hot fill:#f9f,stroke:#a00\n  A --> B\n  class A hot",
        "dark",
    )
    .expect("renders");
    assert!(
        svg.contains("fill=\"#f9f\""),
        "the fill reaches the SVG: {svg}"
    );
}

/// `:::hot` at the declaration does what a separate `class` statement does.
#[test]
fn triple_colon_is_the_same_as_a_class_statement() {
    let d = laid_out("flowchart TD\n  classDef hot fill:#f9f\n  A:::hot --> B");
    let a = d
        .node("A")
        .expect("A exists")
        .style
        .as_ref()
        .expect("A has a style");
    assert_eq!(a.fill.as_deref(), Some("#f9f"));
    assert!(d.node("B").expect("B exists").style.is_none());
}

/// A node's own `style` statement is the last step of the cascade, so it wins over a `classDef`
/// the node also carries — D2's "後のものが前を上書きする".
#[test]
fn a_nodes_own_style_wins_over_its_classdef() {
    let d =
        laid_out("flowchart TD\n  classDef hot fill:#f9f\n  A:::hot --> B\n  style A fill:#00ff00");
    let a = d
        .node("A")
        .expect("A exists")
        .style
        .as_ref()
        .expect("A has a style");
    assert_eq!(
        a.fill.as_deref(),
        Some("#00ff00"),
        "the node's own `style` statement must be the final word, not the classDef"
    );
}

/// `linkStyle <idx>` touches only the edge at that index into [`Flowchart::edges`] — proof the
/// index-to-edge mapping is right, not just that *an* edge got styled.
#[test]
fn link_style_by_index_touches_only_that_edge() {
    let d = laid_out("flowchart TD\n  A --> B\n  B --> C\n  C --> D\n  linkStyle 1 stroke:#1f6feb");
    assert_eq!(d.edges.len(), 3, "three edges: A->B, B->C, C->D");
    assert!(d.edges[0].style.is_none(), "A->B (index 0) is untouched");
    assert_eq!(
        d.edges[1]
            .style
            .as_ref()
            .expect("B->C (index 1) has a style")
            .stroke
            .as_deref(),
        Some("#1f6feb")
    );
    assert!(d.edges[2].style.is_none(), "C->D (index 2) is untouched");
}

/// `linkStyle 0,1,2 stroke:#1f6feb` — the user's own reported shape
/// (`~/work/Ergora/doc/spec/05-system-architecture.md`), where several indices share one
/// declaration. Every named edge gets it and the edge left out does not.
#[test]
fn link_style_with_multiple_indices_reaches_every_named_edge() {
    let d = laid_out(
        "flowchart TD\n  A --> B\n  B --> C\n  C --> D\n  D --> E\n  \
         linkStyle 0,1,2 stroke:#1f6feb",
    );
    assert_eq!(d.edges.len(), 4);
    for i in 0..3 {
        assert_eq!(
            d.edges[i]
                .style
                .as_ref()
                .unwrap_or_else(|| panic!("edge {i} has a style"))
                .stroke
                .as_deref(),
            Some("#1f6feb"),
            "edge {i} is named by `linkStyle 0,1,2`"
        );
    }
    assert!(
        d.edges[3].style.is_none(),
        "edge 3 (D->E) was not named, and must be untouched"
    );
}

/// `linkStyle default` colours every edge, and a specific index still overrides it — the fixed
/// pipeline order [`super::style::cascade_edge`]'s docs describe, independent of which the source
/// physically wrote first.
#[test]
fn link_style_default_colours_every_edge_and_an_index_still_overrides_it() {
    let d = laid_out(
        "flowchart TD\n  A --> B\n  B --> C\n  linkStyle 0 stroke:#e11\n  \
         linkStyle default stroke:#39d",
    );
    assert_eq!(
        d.edges[0].style.as_ref().unwrap().stroke.as_deref(),
        Some("#e11"),
        "edge 0's own index is more specific than `default`, even written first"
    );
    assert_eq!(
        d.edges[1].style.as_ref().unwrap().stroke.as_deref(),
        Some("#39d"),
        "edge 1 has no index of its own, so `default` reaches it"
    );
}

/// `stroke-width` is one of [`mask_numbers`]' masked attributes, so the golden cannot see whether
/// it actually reached the SVG — this asserts on the unmasked string directly.
#[test]
fn link_style_stroke_width_reaches_the_unmasked_svg() {
    let svg = render(
        "flowchart TD\n  A --> B\n  linkStyle 0 stroke-width:4px",
        "dark",
    )
    .expect("renders");
    assert!(
        svg.contains("stroke-width=\"4\""),
        "the declared width must reach the path's own attribute, unmasked: {svg}"
    );
}

// --- the four tests above this line all read `Diagram`/`PlacedEdge::style` — the *model* the
// cascade resolves into. Every one of them is blind to a defect in the last step, `svg.rs`
// applying that field when it writes the `<path>` element: the regression this whole section
// exists for was reported *from that exact step* (`linkStyle` colours that reached `PlacedEdge`
// correctly and never reached the SVG the terminal draws). The tests below read the emitted
// string instead of the model, so a defect confined to `svg::emit_edge` — the model right, the
// drawing wrong — fails one of these rather than passing every model-level test in the file.

/// `linkStyle <idx> stroke:#…` reaches the one path element for that edge, in the raw SVG text,
/// and the other edges' paths keep the theme's own line colour.
#[test]
fn link_style_single_index_stroke_reaches_the_svg_and_only_that_edges_path() {
    let svg = render(
        "flowchart TD\n  A --> B\n  B --> C\n  C --> D\n  linkStyle 1 stroke:#1f6feb",
        "dark",
    )
    .expect("renders");
    assert_eq!(
        svg.matches("stroke=\"#1f6feb\"").count(),
        1,
        "exactly one edge (index 1) was named: {svg}"
    );
    assert_eq!(
        svg.matches(&format!("stroke=\"{}\"", theme::DARK.line))
            .count(),
        2,
        "the other two edges must still draw in the theme's own line colour: {svg}"
    );
}

/// `linkStyle 0,1,2 stroke:#…` — several indices sharing one declaration, the user's own reported
/// shape (`~/work/Ergora/doc/spec/05-system-architecture.md`) — reaches every named edge's path in
/// the raw SVG text, and the edge left out does not get it.
#[test]
fn link_style_multiple_indices_stroke_reaches_every_named_edges_path() {
    let svg = render(
        "flowchart TD\n  A --> B\n  B --> C\n  C --> D\n  D --> E\n  \
         linkStyle 0,1,2 stroke:#1f6feb",
        "dark",
    )
    .expect("renders");
    assert_eq!(
        svg.matches("stroke=\"#1f6feb\"").count(),
        3,
        "three edges (0, 1, 2) were named: {svg}"
    );
    assert_eq!(
        svg.matches(&format!("stroke=\"{}\"", theme::DARK.line))
            .count(),
        1,
        "the fourth edge (D->E) was not named and must keep the theme's line colour: {svg}"
    );
}

/// `linkStyle default` colours every edge's path in the raw SVG text, and an indexed `linkStyle`
/// still overrides it there too — the same fixed pipeline order the model-level
/// [`link_style_default_colours_every_edge_and_an_index_still_overrides_it`] states, checked at
/// the one step that test cannot see.
#[test]
fn link_style_default_then_an_index_reach_the_svg_as_two_distinct_colours() {
    let svg = render(
        "flowchart TD\n  A --> B\n  B --> C\n  linkStyle 0 stroke:#e11\n  \
         linkStyle default stroke:#39d",
        "dark",
    )
    .expect("renders");
    assert_eq!(
        svg.matches("stroke=\"#e11\"").count(),
        1,
        "edge 0's own index must win over `default`, even written first: {svg}"
    );
    assert_eq!(
        svg.matches("stroke=\"#39d\"").count(),
        1,
        "edge 1 has no index of its own, so `default` must reach its path: {svg}"
    );
}

/// An unparsable colour is dropped, not substituted with black — [`super::style::paint`]'s whole
/// reason for existing (`UiConfig::math_color`'s "blank equation" precedent). The node must still
/// draw, in the theme's own colour, and the document must still be valid SVG.
#[test]
fn an_invalid_color_falls_back_to_the_theme_instead_of_black() {
    let d = laid_out("flowchart TD\n  classDef bad fill:notacolor\n  A:::bad --> B");
    assert!(
        d.node("A").unwrap().style.is_none(),
        "the only declared field failed to validate, so there is nothing to override with"
    );

    let svg = render(
        "flowchart TD\n  classDef bad fill:notacolor\n  A:::bad --> B",
        "dark",
    )
    .expect("renders");
    assert!(
        svg.contains(theme::DARK.node_fill),
        "A must draw in the theme's node colour, not vanish or turn black: {svg}"
    );
    assert!(
        !svg.contains("notacolor"),
        "the bad literal must never reach the SVG document"
    );
    // `tree_of` itself `.expect()`s a successful parse, so simply calling it is the assertion:
    // the document is still valid SVG usvg can read.
    tree_of(&svg);
}

// ---------------------------------------------------------------------------------------------
// 10. `curve` (`[ui] mermaid_curve` / `linkStyle ... interpolate`)
// ---------------------------------------------------------------------------------------------
//
// mermaid's most-requested flowchart feature is a straight/right-angle line instead of the
// default spline (mermaid-js#2817, 129 reactions; mermaid-js#2549, 75 reactions; both open 4+
// years). The unconditional requirement this section exists to pin: `render_curve(..., "basis")`
// — and so `render` itself, which is defined as exactly that call — must draw byte-for-byte what
// it drew before `curve` existed. `corpus_golden` staying green already implies this; these tests
// state the reason directly, the way §9 states its own bug by name instead of by an
// unrelated-looking snapshot diff.

/// D5's invariant, stated directly: the default curve changes nothing. `render` is `render_curve`
/// called with `"basis"` (`mod.rs`'s own definition), so this is really pinning that the
/// delegation stays what it is — but a future edit could still break it by, say, giving `render`
/// its own copy of the pipeline that drifts from `render_curve`'s.
#[test]
fn basis_curve_is_byte_identical_to_render() {
    for (name, src) in CORPUS {
        let via_curve =
            render_curve(src, "dark", "basis").unwrap_or_else(|e| panic!("{name}: {e}"));
        let via_render = render(src, "dark").unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            via_curve, via_render,
            "{name}: render_curve(..., \"basis\") must match render() exactly"
        );
    }
}

/// `Curve::Basis` still rounds right-angle corners off (`edges::fix_corners`) — today's look,
/// unchanged. Segment lengths are past `fix_corners`' short-corner guard (`CORNER_RADIUS`'s own
/// module docs use 10px as the threshold), so the corner really is rounded, not left alone by a
/// guard this test failed to clear.
#[test]
fn basis_curve_still_rounds_corners() {
    let raw = vec![
        Point::new(0.0, 0.0),
        Point::new(0.0, 40.0),
        Point::new(40.0, 40.0),
    ];
    let edge = bent_edge(raw.clone(), Curve::Basis);
    assert_eq!(edge.drawn_points(), edges::fix_corners(&raw));
    assert_ne!(
        edge.drawn_points(),
        raw,
        "a real right angle must actually be rounded under Curve::Basis"
    );
}

/// `Curve::Linear` still runs `fix_corners`, the same as every curve but `Rounded`
/// (`Curve::rounds_corners`'s own doc has the versioned evidence). **This test used to assert the
/// opposite** — that `linear` kept a sharp right angle — on the reasoning that `linear`/`step*`
/// are chosen *because* they draw a sharp angle, so rounding one off would undo the shape asked
/// for. That reasoning was never checked against upstream, and it was wrong: `mermaid@11.4.1` and
/// `mermaid@11.12.0` (both released versions) call `fixCorners` unconditionally in `edges.js`, no
/// curve-type check at all, so a real `linear` flowchart on GitHub has this exact corner softened
/// too. This crate's acceptance test is "the same source reads the same picture GitHub draws"
/// (this session's own brief), so the fixed expectation is the one that matches that picture, not
/// the one that looked most consistent with `linear`'s own name.
#[test]
fn linear_curve_also_rounds_corners() {
    let raw = vec![
        Point::new(0.0, 0.0),
        Point::new(0.0, 40.0),
        Point::new(40.0, 40.0),
    ];
    let edge = bent_edge(raw.clone(), Curve::Linear);
    assert_eq!(
        edge.drawn_points(),
        edges::fix_corners(&raw),
        "Curve::Linear must run fix_corners, matching every released mermaid version"
    );
    assert_ne!(
        edge.drawn_points(),
        raw,
        "a real right angle must actually be rounded under Curve::Linear too"
    );
    // `Curve::Linear.path` itself is still an unmodified `polyline_path` — straight segments
    // through whatever points it is given. What changed is which points it is given: the
    // fix_corners-rounded ones (`drawn_points`), not the raw waypoints. `polyline_path` drawing a
    // straight line between two rounded-corner points is exactly how `fixCorners` + `curveLinear`
    // reads upstream too — the corner becomes a short straight chamfer, not a smooth arc.
    assert_eq!(
        Curve::Linear.path(&edge.drawn_points()),
        edges::polyline_path(&edge.drawn_points())
    );
}

/// `Curve::Step` also runs `fix_corners` — same evidence, same fix as `Curve::Linear`'s own test
/// just above. Checked separately because `step`'s own shape (an explicit right-angle transition
/// mid-segment) makes it easy to assume, wrongly, that `fixCorners` would be redundant on it; it
/// is not — `fixCorners` acts on the edge's *waypoints*, before `step_path` ever sees them, so a
/// real right-angle waypoint (not `step`'s own synthetic transition point) is softened exactly
/// like it is for every other non-`Rounded` curve.
#[test]
fn step_curve_also_rounds_corners() {
    let raw = vec![
        Point::new(0.0, 0.0),
        Point::new(0.0, 40.0),
        Point::new(40.0, 40.0),
    ];
    let edge = bent_edge(raw.clone(), Curve::Step);
    assert_eq!(
        edge.drawn_points(),
        edges::fix_corners(&raw),
        "Curve::Step must run fix_corners, matching every released mermaid version"
    );
    assert_ne!(edge.drawn_points(), raw);
}

/// `Curve::Rounded` is the one curve `PlacedEdge::drawn_points` does **not** run `fix_corners`
/// for — it rounds every corner itself, at its own radius (`rounded_path`), so pre-rounding would
/// round the same corner twice. The regression this guards: a mutation that made `Rounded` *also*
/// run `fix_corners` (double-rounding) must be caught here, not just inferred from
/// `Curve::rounds_corners`'s own boolean.
#[test]
fn rounded_curve_does_not_also_run_fix_corners() {
    let raw = vec![
        Point::new(0.0, 0.0),
        Point::new(0.0, 40.0),
        Point::new(40.0, 40.0),
    ];
    let edge = bent_edge(raw.clone(), Curve::Rounded);
    assert_eq!(
        edge.drawn_points(),
        raw,
        "Curve::Rounded must not run fix_corners — it rounds corners itself"
    );
}

/// The three step variants draw three different shapes for the same waypoints — the regression
/// guard against copy-pasting one branch over another. Values are worked out from d3-shape's own
/// `Step` class (`edges.rs`'s `step_path` docs): `stepBefore` goes vertical first, `stepAfter`
/// goes horizontal first, `step` splits at the midpoint.
#[test]
fn step_variants_differ_from_each_other_and_match_d3() {
    let pts = vec![Point::new(0.0, 0.0), Point::new(40.0, 60.0)];
    let step = Curve::Step.path(&pts);
    let before = Curve::StepBefore.path(&pts);
    let after = Curve::StepAfter.path(&pts);

    assert_ne!(step, before, "step and stepBefore must draw differently");
    assert_ne!(step, after, "step and stepAfter must draw differently");
    assert_ne!(
        before, after,
        "stepBefore and stepAfter must draw differently"
    );

    let m = |x: f64, y: f64| format!("M{},{}", num(x), num(y));
    let l = |x: f64, y: f64| format!("L{},{}", num(x), num(y));
    assert_eq!(
        before,
        format!("{}{}{}", m(0.0, 0.0), l(0.0, 60.0), l(40.0, 60.0)),
        "stepBefore: vertical leg first, at the segment's starting x"
    );
    assert_eq!(
        after,
        format!("{}{}{}", m(0.0, 0.0), l(40.0, 0.0), l(40.0, 60.0)),
        "stepAfter: horizontal leg first, at the segment's ending x"
    );
    assert_eq!(
        step,
        format!(
            "{}{}{}{}",
            m(0.0, 0.0),
            l(20.0, 0.0),
            l(20.0, 60.0),
            l(40.0, 60.0)
        ),
        "step: the transition sits at the segment's midpoint x, and the line still reaches p1"
    );
}

/// `linkStyle <n> interpolate <curve>` reaches exactly the edge it names, and no other — the
/// wiring `flowchart/model.rs` used to say konoma does not have (§2-5, now implemented).
#[test]
fn link_style_interpolate_affects_only_that_edge() {
    let d = laid_out_curve(
        "flowchart TD\n  A --> B\n  C --> D\n  linkStyle 0 interpolate linear",
        "basis",
    );
    assert_eq!(d.edges[0].curve, Curve::Linear);
    assert_eq!(d.edges[1].curve, Curve::Basis);
}

/// An edge's own `linkStyle interpolate` wins over the chart-wide `[ui] mermaid_curve` default —
/// the priority order D4 requires, stated as the strongest case: the two disagree outright.
#[test]
fn link_style_interpolate_overrides_the_chart_wide_default() {
    let d = laid_out_curve(
        "flowchart TD\n  A --> B\n  linkStyle 0 interpolate linear",
        "step",
    );
    assert_eq!(
        d.edges[0].curve,
        Curve::Linear,
        "the edge's own interpolate must win over ui.mermaid_curve"
    );
}

/// `linkStyle default interpolate` sets the chart-wide default for every edge that names no
/// index of its own, exactly the way `style::cascade_edge` already treats `linkStyle default`
/// styles — and an indexed `linkStyle` still wins over it, the same cascade order.
#[test]
fn link_style_default_interpolate_applies_unless_an_index_overrides_it() {
    let d = laid_out_curve(
        "flowchart TD\n  A --> B\n  C --> D\n  linkStyle default interpolate linear\n  \
         linkStyle 1 interpolate step",
        "basis",
    );
    assert_eq!(
        d.edges[0].curve,
        Curve::Linear,
        "linkStyle default interpolate applies to an edge with no index override"
    );
    assert_eq!(
        d.edges[1].curve,
        Curve::Step,
        "linkStyle 1's own interpolate must still win over linkStyle default"
    );
}

/// An unrecognised curve name falls back to `Curve::Basis` rather than erroring, both directly
/// through `Curve::parse` and through the whole config → render pipeline. Never crashes.
///
/// All 13 of mermaid's own `flowchart.curve` values are implemented as of this test (see the
/// "eight newly implemented curves" section below), so "a real d3 curve this crate does not
/// implement" is no longer an example this test can give — `curveBasisClosed` stands in instead:
/// a real d3-shape curve name, just not one mermaid's `flowchart.curve` schema ever accepts.
#[test]
fn unknown_curve_falls_back_to_basis_without_crashing() {
    assert_eq!(
        Curve::parse("basisClosed"),
        Curve::Basis,
        "a real d3-shape curve, but not one of mermaid's flowchart.curve values"
    );
    assert_eq!(
        Curve::parse("Basis"),
        Curve::Basis,
        "wrong case — Curve::parse is case-sensitive, matching mermaid's own spellings"
    );
    assert_eq!(
        Curve::parse("nonsense"),
        Curve::Basis,
        "not a curve name at all"
    );
    assert_eq!(Curve::parse(""), Curve::Basis, "empty string");

    let d = laid_out_curve("flowchart TD\n  A --> B", "basisClosed");
    assert_eq!(d.edges[0].curve, Curve::Basis);

    let svg = render_curve("flowchart TD\n  A --> B", "dark", "nonsense")
        .expect("an unknown curve must still render, as Curve::Basis");
    assert!(svg.contains("<svg"));
}

// ---------------------------------------------------------------------------------------------
// 10-b. The eight curves implemented after the five above: d3-shape's `natural`, `cardinal`,
// `catmullRom`, `monotoneX`, `monotoneY`, `bumpX`, `bumpY`, and mermaid's own `rounded`.
// ---------------------------------------------------------------------------------------------
//
// Every `_matches_d3` test below pins geometry generated by actually running the real npm
// `d3-shape` package (v3, the version mermaid itself depends on) through a small Node context
// object that records `moveTo`/`lineTo`/`bezierCurveTo` calls in exactly the string convention
// this module's own `push_move`/`push_line`/`push_bezier` use (comma inside a coordinate pair,
// space between pairs, three-decimal rounding via the same formula as `svg::num`) — not hand-
// derived, and not `curve_basis_matches_d3`'s approach of working the control-point algebra out
// by hand, because a spline family this size (five distinct state machines, one of them with an
// epsilon-gated weighting term) is exactly where hand arithmetic is most likely to be the thing
// that is wrong. The three point sets are chosen on purpose: `RIGHT_ANGLE` and `FOUR_POINT` have
// evenly-spaced waypoints, under which centripetal Catmull-Rom (`catmullRom`) and the classic
// Cardinal spline (`cardinal`) are mathematically the same curve — so `UNEVEN` (unequal segment
// lengths) is the set that actually exercises `catmullRom`'s own epsilon-gated weighting and
// tells the two apart; without it a `cardinal`/`catmullRom` copy-paste bug would pass silently.
//
// `VERTICAL`/`HORIZONTAL`/`DUPLICATE` exist because none of the three sets above ever has two
// consecutive points share a coordinate, and that is exactly the shape that broke `monotoneX` in
// production: `flowchart TD` is konoma's *default* direction, under which an ordinary vertical
// edge is three points with the same x — `slope3`'s `dx` is `0` for that pair, which upstream
// handles (its `Math.min` propagates the resulting `NaN` through to a `0` fallback) and this
// crate's first port of it did not (`js_min`'s own doc has the full mechanism). `VERTICAL` is
// exactly that shape; `HORIZONTAL` is its mirror (the case `monotoneY`'s reflected axis turns
// into the same failure); `DUPLICATE` is the more general version — two *identical* consecutive
// points, which is what an axis-aligned run degenerates to when dagre's own dedup does not catch
// it. All three were mechanically swept over every one of the 13 curves (not guessed at from the
// bug report) — `Curve::path` never emits `NaN`/`Infinity` for any of them, pinned separately by
// `no_curve_ever_emits_nan_or_infinite_coordinates` below.

const RIGHT_ANGLE: [(f64, f64); 3] = [(0.0, 0.0), (60.0, 0.0), (60.0, 60.0)];
const FOUR_POINT: [(f64, f64); 4] = [(0.0, 0.0), (40.0, 0.0), (40.0, 40.0), (80.0, 40.0)];
const UNEVEN: [(f64, f64); 4] = [(0.0, 0.0), (10.0, 0.0), (40.0, 30.0), (90.0, 35.0)];
/// Three collinear points sharing an x — an ordinary vertical edge under `flowchart TD`,
/// konoma's default direction. The exact shape that broke `monotoneX` in production.
const VERTICAL: [(f64, f64); 3] = [(100.0, 50.0), (100.0, 120.0), (100.0, 190.0)];
/// `VERTICAL`'s mirror — three collinear points sharing a y, the shape that breaks `monotoneY`'s
/// reflected axis the same way `VERTICAL` breaks `monotoneX`'s own.
const HORIZONTAL: [(f64, f64); 3] = [(50.0, 100.0), (120.0, 100.0), (190.0, 100.0)];
/// Two consecutive points at the exact same coordinate — the more general version of
/// `VERTICAL`/`HORIZONTAL`'s "a segment has zero length" degeneracy, on a diagonal run instead of
/// an axis-aligned one.
const DUPLICATE: [(f64, f64); 4] = [(0.0, 0.0), (40.0, 40.0), (40.0, 40.0), (80.0, 80.0)];

fn pts(raw: &[(f64, f64)]) -> Vec<Point> {
    raw.iter().map(|&(x, y)| Point::new(x, y)).collect()
}

#[test]
fn curve_natural_matches_d3() {
    assert_eq!(
        edges::curve_natural_path(&pts(&RIGHT_ANGLE)),
        "M0,0C25,-5 50,-10 60,0C70,10 65,35 60,60"
    );
    assert_eq!(
        edges::curve_natural_path(&pts(&FOUR_POINT)),
        "M0,0C17.778,-4.444 35.556,-8.889 40,0C44.444,8.889 35.556,31.111 40,40\
         C44.444,48.889 62.222,44.444 80,40"
    );
    assert_eq!(
        edges::curve_natural_path(&pts(&UNEVEN)),
        "M0,0C2,-3.222 4,-6.444 10,0C16,6.444 26,22.556 40,30C54,37.444 72,36.222 90,35"
    );
    assert_eq!(
        edges::curve_natural_path(&pts(&VERTICAL)),
        "M100,50C100,73.333 100,96.667 100,120C100,143.333 100,166.667 100,190"
    );
    assert_eq!(
        edges::curve_natural_path(&pts(&HORIZONTAL)),
        "M50,100C73.333,100 96.667,100 120,100C143.333,100 166.667,100 190,100"
    );
    assert_eq!(
        edges::curve_natural_path(&pts(&DUPLICATE)),
        "M0,0C17.778,17.778 35.556,35.556 40,40C44.444,44.444 35.556,35.556 40,40\
         C44.444,44.444 62.222,62.222 80,80"
    );
    // Two points are a line; one is a bare move; zero is empty — the shared degenerate cases.
    assert_eq!(
        edges::curve_natural_path(&pts(&RIGHT_ANGLE[..2])),
        "M0,0L60,0"
    );
    assert_eq!(edges::curve_natural_path(&pts(&RIGHT_ANGLE[..1])), "M0,0");
    assert_eq!(edges::curve_natural_path(&[]), "");
}

#[test]
fn curve_cardinal_matches_d3() {
    assert_eq!(
        edges::curve_cardinal_path(&pts(&RIGHT_ANGLE)),
        "M0,0C0,0 50,-10 60,0C70,10 60,60 60,60"
    );
    assert_eq!(
        edges::curve_cardinal_path(&pts(&FOUR_POINT)),
        "M0,0C0,0 33.333,-6.667 40,0C46.667,6.667 33.333,33.333 40,40\
         C46.667,46.667 80,40 80,40"
    );
    assert_eq!(
        edges::curve_cardinal_path(&pts(&UNEVEN)),
        "M0,0C0,0 3.333,-5 10,0C16.667,5 26.667,24.167 40,30C53.333,35.833 90,35 90,35"
    );
    assert_eq!(
        edges::curve_cardinal_path(&pts(&VERTICAL)),
        "M100,50C100,50 100,96.667 100,120C100,143.333 100,190 100,190"
    );
    assert_eq!(
        edges::curve_cardinal_path(&pts(&HORIZONTAL)),
        "M50,100C50,100 96.667,100 120,100C143.333,100 190,100 190,100"
    );
    assert_eq!(
        edges::curve_cardinal_path(&pts(&DUPLICATE)),
        "M0,0C0,0 33.333,33.333 40,40C46.667,46.667 33.333,33.333 40,40\
         C46.667,46.667 80,80 80,80"
    );
    assert_eq!(
        edges::curve_cardinal_path(&pts(&RIGHT_ANGLE[..2])),
        "M0,0L60,0"
    );
    assert_eq!(edges::curve_cardinal_path(&pts(&RIGHT_ANGLE[..1])), "M0,0");
    assert_eq!(edges::curve_cardinal_path(&[]), "");
}

#[test]
fn curve_catmull_rom_matches_d3() {
    assert_eq!(
        edges::curve_catmull_rom_path(&pts(&RIGHT_ANGLE)),
        "M0,0C0,0 50,-10 60,0C70,10 60,60 60,60"
    );
    assert_eq!(
        edges::curve_catmull_rom_path(&pts(&FOUR_POINT)),
        "M0,0C0,0 33.333,-6.667 40,0C46.667,6.667 33.333,33.333 40,40\
         C46.667,46.667 80,40 80,40"
    );
    // The one point set that actually tells `catmullRom` apart from `cardinal` — see this
    // section's own module doc.
    assert_eq!(
        edges::curve_catmull_rom_path(&pts(&UNEVEN)),
        "M0,0C0,0 6.169,-1.587 10,0C17.89,3.268 27.455,24.055 40,30C53.653,36.47 90,35 90,35"
    );
    assert_ne!(
        edges::curve_catmull_rom_path(&pts(&UNEVEN)),
        edges::curve_cardinal_path(&pts(&UNEVEN)),
        "catmullRom and cardinal must differ once waypoints are unevenly spaced"
    );
    assert_eq!(
        edges::curve_catmull_rom_path(&pts(&VERTICAL)),
        "M100,50C100,50 100,96.667 100,120C100,143.333 100,190 100,190"
    );
    assert_eq!(
        edges::curve_catmull_rom_path(&pts(&HORIZONTAL)),
        "M50,100C50,100 96.667,100 120,100C143.333,100 190,100 190,100"
    );
    assert_eq!(
        edges::curve_catmull_rom_path(&pts(&DUPLICATE)),
        "M0,0C0,0 40,40 40,40C40,40 40,40 40,40C40,40 80,80 80,80"
    );
    assert_eq!(
        edges::curve_catmull_rom_path(&pts(&RIGHT_ANGLE[..2])),
        "M0,0L60,0"
    );
    assert_eq!(
        edges::curve_catmull_rom_path(&pts(&RIGHT_ANGLE[..1])),
        "M0,0"
    );
    assert_eq!(edges::curve_catmull_rom_path(&[]), "");
}

#[test]
fn curve_monotone_x_matches_d3() {
    assert_eq!(
        edges::curve_monotone_x_path(&pts(&RIGHT_ANGLE)),
        "M0,0C20,0 40,0 60,0C60,0 60,60 60,60"
    );
    assert_eq!(
        edges::curve_monotone_x_path(&pts(&FOUR_POINT)),
        "M0,0C13.333,0 26.667,0 40,0C40,0 40,40 40,40C53.333,40 66.667,40 80,40"
    );
    assert_eq!(
        edges::curve_monotone_x_path(&pts(&UNEVEN)),
        "M0,0C3.333,0 6.667,0 10,0C20,0 30,28 40,30C56.667,33.333 73.333,34.167 90,35"
    );
    // The exact bug: three collinear points sharing an x (an ordinary vertical edge under
    // `flowchart TD`, konoma's default direction) made `slope3`'s `p` become `NaN`
    // (`Infinity * 0 - Infinity * 0`) — upstream's `Math.min` propagates that `NaN` through to a
    // `0` fallback (a flat tangent), which is the finite, correct answer below. This crate's
    // first port used `f64::min`, which *ignores* a `NaN` operand instead of propagating it
    // (`js_min`'s own doc has the full mechanism), so `slope3` returned `Infinity` and the
    // resulting bezier control points were `NaN` — an invalid SVG path whose line silently did
    // not draw at all (found by rendering a real `curve: monotoneX` flowchart and looking at it).
    assert_eq!(
        edges::curve_monotone_x_path(&pts(&VERTICAL)),
        "M100,50C100,50 100,120 100,120C100,120 100,190 100,190"
    );
    assert_eq!(
        edges::curve_monotone_x_path(&pts(&HORIZONTAL)),
        "M50,100C73.333,100 96.667,100 120,100C143.333,100 166.667,100 190,100"
    );
    assert_eq!(
        edges::curve_monotone_x_path(&pts(&DUPLICATE)),
        "M0,0C13.333,13.333 26.667,26.667 40,40C53.333,53.333 66.667,66.667 80,80"
    );
    assert_eq!(
        edges::curve_monotone_x_path(&pts(&RIGHT_ANGLE[..2])),
        "M0,0L60,0"
    );
    assert_eq!(
        edges::curve_monotone_x_path(&pts(&RIGHT_ANGLE[..1])),
        "M0,0"
    );
    assert_eq!(edges::curve_monotone_x_path(&[]), "");
}

#[test]
fn curve_monotone_y_matches_d3() {
    assert_eq!(
        edges::curve_monotone_y_path(&pts(&RIGHT_ANGLE)),
        "M0,0C0,0 60,0 60,0C60,20 60,40 60,60"
    );
    assert_eq!(
        edges::curve_monotone_y_path(&pts(&FOUR_POINT)),
        "M0,0C0,0 40,0 40,0C40,13.333 40,26.667 40,40C40,40 80,40 80,40"
    );
    assert_eq!(
        edges::curve_monotone_y_path(&pts(&UNEVEN)),
        "M0,0C0,0 10,0 10,0C30,10 20,20 40,30C43.333,31.667 66.667,33.333 90,35"
    );
    assert_ne!(
        edges::curve_monotone_x_path(&pts(&UNEVEN)),
        edges::curve_monotone_y_path(&pts(&UNEVEN)),
        "monotoneX and monotoneY must draw differently — the whole point of the reflected axis"
    );
    // `VERTICAL`'s mirror: `monotoneY`'s reflected axis hits the same degeneracy on a
    // *horizontal* run instead of a vertical one — see `curve_monotone_x_matches_d3`'s own
    // comment on the exact bug this pins the fix for.
    assert_eq!(
        edges::curve_monotone_y_path(&pts(&VERTICAL)),
        "M100,50C100,73.333 100,96.667 100,120C100,143.333 100,166.667 100,190"
    );
    assert_eq!(
        edges::curve_monotone_y_path(&pts(&HORIZONTAL)),
        "M50,100C50,100 120,100 120,100C120,100 190,100 190,100"
    );
    assert_eq!(
        edges::curve_monotone_y_path(&pts(&DUPLICATE)),
        "M0,0C13.333,13.333 26.667,26.667 40,40C53.333,53.333 66.667,66.667 80,80"
    );
    assert_eq!(
        edges::curve_monotone_y_path(&pts(&RIGHT_ANGLE[..2])),
        "M0,0L60,0"
    );
    assert_eq!(
        edges::curve_monotone_y_path(&pts(&RIGHT_ANGLE[..1])),
        "M0,0"
    );
    assert_eq!(edges::curve_monotone_y_path(&[]), "");
}

#[test]
fn curve_bump_x_matches_d3() {
    assert_eq!(
        edges::curve_bump_x_path(&pts(&RIGHT_ANGLE)),
        "M0,0C30,0 30,0 60,0C60,0 60,60 60,60"
    );
    assert_eq!(
        edges::curve_bump_x_path(&pts(&FOUR_POINT)),
        "M0,0C20,0 20,0 40,0C40,0 40,40 40,40C60,40 60,40 80,40"
    );
    assert_eq!(
        edges::curve_bump_x_path(&pts(&UNEVEN)),
        "M0,0C5,0 5,0 10,0C25,0 25,30 40,30C65,30 65,35 90,35"
    );
    // Unlike every other curve in this module, `bumpX` draws a curve — not a straight `L` — even
    // between just two points: d3-shape's own `Bump.point` has no `case 2` that falls back to a
    // line (`edges.rs`'s own `curve_bump_x_path` doc).
    assert_eq!(
        edges::curve_bump_x_path(&pts(&RIGHT_ANGLE[..2])),
        "M0,0C30,0 30,0 60,0"
    );
    assert_eq!(
        edges::curve_bump_x_path(&pts(&VERTICAL)),
        "M100,50C100,50 100,120 100,120C100,120 100,190 100,190"
    );
    assert_eq!(
        edges::curve_bump_x_path(&pts(&HORIZONTAL)),
        "M50,100C85,100 85,100 120,100C155,100 155,100 190,100"
    );
    assert_eq!(
        edges::curve_bump_x_path(&pts(&DUPLICATE)),
        "M0,0C20,0 20,40 40,40C40,40 40,40 40,40C60,40 60,80 80,80"
    );
    assert_eq!(edges::curve_bump_x_path(&pts(&RIGHT_ANGLE[..1])), "M0,0");
    assert_eq!(edges::curve_bump_x_path(&[]), "");
}

#[test]
fn curve_bump_y_matches_d3() {
    assert_eq!(
        edges::curve_bump_y_path(&pts(&RIGHT_ANGLE)),
        "M0,0C0,0 60,0 60,0C60,30 60,30 60,60"
    );
    assert_eq!(
        edges::curve_bump_y_path(&pts(&FOUR_POINT)),
        "M0,0C0,0 40,0 40,0C40,20 40,20 40,40C40,40 80,40 80,40"
    );
    assert_eq!(
        edges::curve_bump_y_path(&pts(&UNEVEN)),
        "M0,0C0,0 10,0 10,0C10,15 40,15 40,30C40,32.5 90,32.5 90,35"
    );
    assert_ne!(
        edges::curve_bump_x_path(&pts(&UNEVEN)),
        edges::curve_bump_y_path(&pts(&UNEVEN)),
        "bumpX and bumpY must draw differently"
    );
    assert_eq!(
        edges::curve_bump_y_path(&pts(&RIGHT_ANGLE[..2])),
        "M0,0C0,0 60,0 60,0"
    );
    assert_eq!(
        edges::curve_bump_y_path(&pts(&VERTICAL)),
        "M100,50C100,85 100,85 100,120C100,155 100,155 100,190"
    );
    assert_eq!(
        edges::curve_bump_y_path(&pts(&HORIZONTAL)),
        "M50,100C50,100 120,100 120,100C120,100 190,100 190,100"
    );
    assert_eq!(
        edges::curve_bump_y_path(&pts(&DUPLICATE)),
        "M0,0C0,20 40,20 40,40C40,40 40,40 40,40C40,60 80,60 80,80"
    );
    assert_eq!(edges::curve_bump_y_path(&pts(&RIGHT_ANGLE[..1])), "M0,0");
    assert_eq!(edges::curve_bump_y_path(&[]), "");
}

/// `rounded` is mermaid's own curve, not d3-shape's — so what it is pinned against is mermaid's
/// own `edges.js::generateRoundedPath`, ported the same way (real function, run through Node,
/// same string convention) rather than hand-derived. `radius` is [`edges::CORNER_RADIUS`] (5),
/// the same value `Curve::path` calls this with — matching what `fix_corners` uses for every
/// other curve, per this module's own "why `radius` is a parameter" doc on `rounded_path`.
#[test]
fn rounded_curve_matches_mermaid() {
    let radius = edges::CORNER_RADIUS;
    assert_eq!(
        edges::rounded_path(&pts(&[(0.0, 0.0), (0.0, 40.0), (40.0, 40.0)]), radius),
        "M0,0L0,32.929Q0,40 7.071,40L40,40"
    );
    assert_eq!(
        edges::rounded_path(&pts(&UNEVEN), radius),
        "M0,0L5,0Q10,0 13.536,3.536L29.483,19.483Q40,30 54.799,31.48L90,35"
    );
    // A straight run (no bend at all — every point collinear) is left exactly straight: the
    // corner-detection angle is `π`, which trips `generateRoundedPath`'s own "too close to 180°"
    // guard, the same guard a real flowchart route can hit on an unbent middle waypoint.
    assert_eq!(
        edges::rounded_path(&pts(&[(0.0, 0.0), (50.0, 0.0), (100.0, 0.0)]), radius),
        "M0,0L50,0L100,0"
    );
    assert_eq!(
        edges::rounded_path(&pts(&RIGHT_ANGLE[..2]), radius),
        "M0,0L60,0",
        "two points is just a line, corner or not"
    );
    // `VERTICAL`/`HORIZONTAL` are themselves collinear runs (no bend), so — like the straight run
    // above — every corner-detection angle is `π` and nothing is rounded; `DUPLICATE` has a
    // zero-length segment at its middle point, which `generateRoundedPath`'s own `len1 < epsilon`
    // guard catches the same way.
    assert_eq!(
        edges::rounded_path(&pts(&VERTICAL), radius),
        "M100,50L100,120L100,190"
    );
    assert_eq!(
        edges::rounded_path(&pts(&HORIZONTAL), radius),
        "M50,100L120,100L190,100"
    );
    assert_eq!(
        edges::rounded_path(&pts(&DUPLICATE), radius),
        "M0,0L40,40L40,40L80,80"
    );
    assert_eq!(edges::rounded_path(&pts(&RIGHT_ANGLE[..1]), radius), "M0,0");
    assert_eq!(edges::rounded_path(&[], radius), "");
}

/// All 13 of mermaid's `flowchart.curve` spellings parse to their own, distinct [`Curve`] — the
/// mutation guard against `Curve::parse`'s match arms drifting out of sync with the enum (a copy-
/// pasted arm pointing at the wrong variant would make two names parse to the same curve and this
/// test would be the only thing that notices, since nothing else compares parse results against
/// each other).
#[test]
fn every_curve_name_parses_to_a_distinct_curve() {
    let names = [
        "basis",
        "linear",
        "step",
        "stepBefore",
        "stepAfter",
        "natural",
        "cardinal",
        "catmullRom",
        "monotoneX",
        "monotoneY",
        "bumpX",
        "bumpY",
        "rounded",
    ];
    let parsed: Vec<Curve> = names.iter().map(|s| Curve::parse(s)).collect();
    for (i, a) in parsed.iter().enumerate() {
        for (j, b) in parsed.iter().enumerate() {
            if i != j {
                assert_ne!(
                    a, b,
                    "{} and {} must not parse to the same Curve",
                    names[i], names[j]
                );
            }
        }
    }
}

/// The eight new curves draw eight different shapes for the same bent waypoints — the same
/// "must differ from each other" guard `step_variants_differ_from_each_other_and_match_d3` runs
/// for the first five, extended to the rest, over a point set with an actual bend (two points
/// would let several of these agree by accident: `curveLinear`-shaped coincidences already bit
/// this file once, `different_curves_draw_different_svg_path_data`'s own doc).
#[test]
fn the_eight_new_curves_draw_different_paths_from_each_other() {
    let p = pts(&UNEVEN);
    let named: Vec<(&str, String)> = vec![
        ("natural", edges::curve_natural_path(&p)),
        ("cardinal", edges::curve_cardinal_path(&p)),
        ("catmullRom", edges::curve_catmull_rom_path(&p)),
        ("monotoneX", edges::curve_monotone_x_path(&p)),
        ("monotoneY", edges::curve_monotone_y_path(&p)),
        ("bumpX", edges::curve_bump_x_path(&p)),
        ("bumpY", edges::curve_bump_y_path(&p)),
        ("rounded", edges::rounded_path(&p, edges::CORNER_RADIUS)),
    ];
    for i in 0..named.len() {
        for j in (i + 1)..named.len() {
            assert_ne!(
                named[i].1, named[j].1,
                "{} and {} must draw differently",
                named[i].0, named[j].0
            );
        }
    }
}

/// `Curve::parse(name).path(points)` reaches the exact free function this section's own
/// `_matches_d3`/`_matches_mermaid` tests already pinned against upstream — for every one of the
/// 13 names, not just the eight new ones.
///
/// This is not redundant with the `_matches_d3` tests above: those call `edges::curve_natural_path`
/// (etc.) **directly**, so a mistake in `Curve::parse`'s match arms or in `Curve::path`'s dispatch
/// — the wiring between a config string and the function that actually draws it — is invisible to
/// them. A mutation proved the gap real: swapping `Curve::path`'s `MonotoneX` arm to call
/// `curve_natural_path` instead of `curve_monotone_x_path` left every other test in this file
/// green, `curve_monotone_x_matches_d3` included, because that test never goes through
/// `Curve::path` at all. This test is the one built to catch exactly that swap.
type CurvePathFn = fn(&[Point]) -> String;

#[test]
fn curve_parse_and_path_reach_the_function_that_was_pinned_against_upstream() {
    let p = pts(&UNEVEN);
    let cases: &[(&str, CurvePathFn)] = &[
        ("basis", edges::curve_basis_path),
        ("linear", edges::polyline_path),
        ("natural", edges::curve_natural_path),
        ("cardinal", edges::curve_cardinal_path),
        ("catmullRom", edges::curve_catmull_rom_path),
        ("monotoneX", edges::curve_monotone_x_path),
        ("monotoneY", edges::curve_monotone_y_path),
        ("bumpX", edges::curve_bump_x_path),
        ("bumpY", edges::curve_bump_y_path),
    ];
    for (name, f) in cases {
        assert_eq!(
            Curve::parse(name).path(&p),
            f(&p),
            "Curve::parse(\"{name}\").path(...) must reach the same function {name} was pinned \
             against upstream with"
        );
    }
    // `rounded` takes a radius, so it is checked against `Curve::path`'s own hardcoded
    // `CORNER_RADIUS` argument rather than fitting the `fn(&[Point]) -> String` shape above.
    assert_eq!(
        Curve::parse("rounded").path(&p),
        edges::rounded_path(&p, edges::CORNER_RADIUS)
    );
    // `step`/`stepBefore`/`stepAfter` take a `t` argument `step_variants_differ_from_each_other_
    // and_match_d3` already pins by value; checked here for wiring completeness the same way.
    assert_eq!(Curve::parse("step").path(&p), edges::step_path(&p, 0.5));
    assert_eq!(
        Curve::parse("stepBefore").path(&p),
        edges::step_path(&p, 0.0)
    );
    assert_eq!(
        Curve::parse("stepAfter").path(&p),
        edges::step_path(&p, 1.0)
    );
}

/// The dispatch-completeness check above (`curve_parse_and_path_reach_the_function_that_was_
/// pinned_against_upstream`), but for the exact degenerate shape that shipped a real bug: a
/// vertical run of waypoints — `flowchart TD`'s ordinary look, konoma's default direction —
/// through `Curve::parse("monotoneX").path(...)`, the real config-to-render path, not the free
/// function directly. The bug this closes (`js_min`'s own doc has the mechanism) reached
/// production through exactly this call chain — `[ui] mermaid_curve = "monotoneX"` on a plain
/// `flowchart TD` — so the regression test for it has to go through the same chain, not just the
/// free function `curve_monotone_x_matches_d3` already pins directly.
type CurveDegenerateCase = (&'static str, &'static [(f64, f64)], &'static str);

#[test]
fn all_thirteen_curves_match_d3_or_mermaid_on_degenerate_points_through_curve_path() {
    let cases: &[CurveDegenerateCase] = &[
        (
            "basis",
            &VERTICAL,
            "M100,50L100,61.667C100,73.333 100,96.667 100,120\
         C100,143.333 100,166.667 100,178.333L100,190",
        ),
        ("linear", &VERTICAL, "M100,50 L100,120 L100,190"),
        // `step`/`stepBefore`/`stepAfter` genuinely diverge from raw d3 here — not a bug in
        // either implementation, and not related to the NaN fix this test exists for. d3's own
        // `Step.point` always emits two `lineTo` calls per point unconditionally (never checking
        // whether either has zero length), so a vertical run's raw d3 output carries redundant
        // same-coordinate legs (`edges::step_path`'s own module doc did not anticipate this —
        // it reasoned about the *deferred final leg* `curveStep` (t=0.5) leaves for `lineEnd`,
        // not about a *zero-dx segment* dropping a leg outright). konoma's own `step_path` skips
        // a leg whose two endpoints coincide (`EPS`-guarded), so it collapses straight to the
        // three real points instead. Both draw the *identical picture* — a zero-length line
        // segment is invisible in any renderer — so this is a cosmetic `d`-string difference,
        // not a rendering one, and out of scope for the NaN bug; flagged, not silently patched.
        ("step", &VERTICAL, "M100,50L100,120L100,190"),
        ("stepBefore", &VERTICAL, "M100,50L100,120L100,190"),
        ("stepAfter", &VERTICAL, "M100,50L100,120L100,190"),
        (
            "natural",
            &VERTICAL,
            "M100,50C100,73.333 100,96.667 100,120\
         C100,143.333 100,166.667 100,190",
        ),
        (
            "cardinal",
            &VERTICAL,
            "M100,50C100,50 100,96.667 100,120C100,143.333 100,190 100,190",
        ),
        (
            "catmullRom",
            &VERTICAL,
            "M100,50C100,50 100,96.667 100,120C100,143.333 100,190 100,190",
        ),
        (
            "monotoneX",
            &VERTICAL,
            "M100,50C100,50 100,120 100,120C100,120 100,190 100,190",
        ),
        (
            "monotoneY",
            &HORIZONTAL,
            "M50,100C50,100 120,100 120,100C120,100 190,100 190,100",
        ),
        (
            "bumpX",
            &VERTICAL,
            "M100,50C100,50 100,120 100,120C100,120 100,190 100,190",
        ),
        (
            "bumpY",
            &HORIZONTAL,
            "M50,100C50,100 120,100 120,100C120,100 190,100 190,100",
        ),
        ("rounded", &VERTICAL, "M100,50L100,120L100,190"),
    ];
    for (name, raw, expected) in cases {
        let p = pts(raw);
        assert_eq!(
            Curve::parse(name).path(&p),
            *expected,
            "Curve::parse(\"{name}\").path(...) on a degenerate point set"
        );
        assert!(
            !Curve::parse(name).path(&p).contains("NaN"),
            "{name} must not emit NaN through Curve::path"
        );
    }
}

/// **Mechanical**, not example-based: every one of the 13 curve names, over a battery of
/// degenerate point sets (vertical, horizontal, and every other axis-aligned-or-duplicate shape
/// below), must never emit `NaN` or `Infinity` into the `d` attribute it draws — regardless of
/// whether this crate happens to have a hand-written expected value for that exact combination.
/// The bug this exists to make impossible to reintroduce (`js_min`'s own doc has the mechanism)
/// was found by looking at a rendered picture, not by a test — this is the test that should have
/// caught it, and the reason it is swept over every curve rather than just `monotoneX`/`monotoneY`
/// is that the next curve to grow a `NaN` might not be either of those two.
#[test]
fn no_curve_ever_emits_nan_or_infinite_coordinates() {
    let names = [
        "basis",
        "linear",
        "step",
        "stepBefore",
        "stepAfter",
        "natural",
        "cardinal",
        "catmullRom",
        "monotoneX",
        "monotoneY",
        "bumpX",
        "bumpY",
        "rounded",
    ];
    // Every point set is deliberately degenerate in a different way: two axis-aligned runs (the
    // exact shape that broke `monotoneX`/`monotoneY`), a run with a duplicated interior point, a
    // single repeated point three times over, and a two-point vertical/horizontal edge (the
    // *most* common real shape of all — an ordinary two-node `flowchart TD` edge).
    let degenerate: &[&[(f64, f64)]] = &[
        &VERTICAL,
        &HORIZONTAL,
        &DUPLICATE,
        &[(50.0, 50.0), (50.0, 50.0), (50.0, 50.0)],
        &[(0.0, 0.0), (0.0, 100.0)],
        &[(0.0, 0.0), (100.0, 0.0)],
        &[(100.0, 100.0), (100.0, 100.0)],
    ];
    for name in names {
        for raw in degenerate {
            let p = pts(raw);
            let d = Curve::parse(name).path(&p);
            assert!(
                !d.contains("NaN") && !d.contains("inf") && !d.contains("Inf"),
                "{name} emitted a non-finite coordinate for {raw:?}: {d}"
            );
        }
    }
}

/// `Curve::rounds_corners` matches every released mermaid version's actual gate in `edges.js`:
/// `fixCorners` runs unconditionally, for **every** curve, `rounded` included on `develop` only
/// by an exemption added the same commit that introduced it (`Curve::rounds_corners`'s own doc
/// has the versioned evidence — `mermaid@11.4.1`/`mermaid@11.12.0` have no condition at all).
/// `Rounded` is the *only* curve this crate exempts, and only because it rounds every corner
/// itself at its own radius — running `fix_corners` first would double-round the same corner.
///
/// This test used to assert the opposite for `Linear`/`Step`/`StepBefore`/`StepAfter` — a rule
/// invented from how those curves *look* (chosen for a sharp angle, so surely nothing should
/// round it) rather than checked against upstream. It was wrong on both released versions
/// checked; the corrected list below is every curve *but* `Rounded`.
#[test]
fn rounds_corners_matches_mermaids_fix_corners_gate() {
    for curve in [
        Curve::Basis,
        Curve::Linear,
        Curve::Step,
        Curve::StepBefore,
        Curve::StepAfter,
        Curve::Natural,
        Curve::Cardinal,
        Curve::CatmullRom,
        Curve::MonotoneX,
        Curve::MonotoneY,
        Curve::BumpX,
        Curve::BumpY,
    ] {
        assert!(curve.rounds_corners(), "{curve:?} must run fix_corners");
    }
    assert!(
        !Curve::Rounded.rounds_corners(),
        "Curve::Rounded must not run fix_corners — it rounds corners itself"
    );
}

// ---------------------------------------------------------------------------------------------
// 10-c. `%%{init}%%`'s `flowchart.curve` — the priority order `spec_of`'s own doc states:
// `linkStyle <n> interpolate` > `linkStyle default interpolate` > `%%{init}%%`'s `flowchart.curve`
// > `[ui] mermaid_curve`.
// ---------------------------------------------------------------------------------------------

/// `%%{init: {"flowchart": {"curve": "linear"}}}%%` overrides `[ui] mermaid_curve` — the weakest
/// link in the priority chain beating the weaker-still config default.
#[test]
fn init_directive_curve_overrides_the_config_default() {
    let d = laid_out_curve(
        "%%{init: {\"flowchart\": {\"curve\": \"linear\"}}}%%\nflowchart TD\n  A --> B",
        "step",
    );
    assert_eq!(
        d.edges[0].curve,
        Curve::Linear,
        "the init directive must win over ui.mermaid_curve"
    );
}

/// An edge's own `linkStyle interpolate` still wins over `%%{init}%%` — the strongest link in the
/// chain beating the middle one, with a config default that agrees with neither so the test
/// cannot pass by two of the three coinciding.
#[test]
fn link_style_interpolate_overrides_init_directive_curve() {
    let d = laid_out_curve(
        "%%{init: {\"flowchart\": {\"curve\": \"linear\"}}}%%\n\
         flowchart TD\n  A --> B\n  linkStyle 0 interpolate step",
        "basis",
    );
    assert_eq!(
        d.edges[0].curve,
        Curve::Step,
        "linkStyle interpolate must win over both the init directive and the config default"
    );
}

/// `linkStyle default interpolate` sits between the two: it loses to this edge's own indexed
/// `linkStyle`, but beats the init directive for every edge that names no index of its own.
#[test]
fn link_style_default_interpolate_sits_between_init_and_indexed_link_style() {
    let d = laid_out_curve(
        "%%{init: {\"flowchart\": {\"curve\": \"linear\"}}}%%\n\
         flowchart TD\n  A --> B\n  C --> D\n  \
         linkStyle default interpolate step\n  linkStyle 1 interpolate stepBefore",
        "basis",
    );
    assert_eq!(
        d.edges[0].curve,
        Curve::Step,
        "linkStyle default interpolate must win over the init directive for an edge with no index override"
    );
    assert_eq!(
        d.edges[1].curve,
        Curve::StepBefore,
        "linkStyle 1's own interpolate must still win over linkStyle default"
    );
}

/// `%%{init: {"theme": "dark"}}%%` — a directive that names no `flowchart.curve` at all — is still
/// completely ignored for curve purposes, exactly as it always was (the corpus already has this
/// exact shape, and `docs/STATUS.md`'s "既定の出力は変わらない" invariant is precisely this: only
/// `flowchart.curve` reaching in is new, every other key must draw exactly what it drew before
/// this crate started reading `%%{init}%%` at all).
#[test]
fn init_directive_naming_only_theme_still_does_not_touch_curve() {
    let d = laid_out_curve(
        "%%{init: {\"theme\": \"dark\"}}%%\nflowchart TD\n  A --> B",
        "step",
    );
    assert_eq!(
        d.edges[0].curve,
        Curve::Step,
        "a theme-only init directive must leave the config default untouched"
    );
}

/// Quote style, whitespace, multi-line bodies and key order are all noise `init_flowchart_curve`
/// has to see through — the exact axes the task called out. Every source below sets the same
/// `flowchart.curve: linear` and must resolve to the same [`Curve::Linear`].
#[test]
fn init_directive_curve_survives_formatting_variation() {
    let sources = [
        // Double-quoted, single line — the canonical shape.
        "%%{init: {\"flowchart\": {\"curve\": \"linear\"}}}%%\nflowchart TD\n  A --> B",
        // Single-quoted — mermaid's own examples use this spelling just as often.
        "%%{init: {'flowchart': {'curve': 'linear'}}}%%\nflowchart TD\n  A --> B",
        // Multi-line, indented, mixed quoting.
        "%%{init: {\n  \"flowchart\": {\n    'curve': \"linear\"\n  }\n}}%%\n\
         flowchart TD\n  A --> B",
        // `flowchart` is not the first key in the init object.
        "%%{init: {\"theme\": \"dark\", \"flowchart\": {\"curve\": \"linear\"}}}%%\n\
         flowchart TD\n  A --> B",
        // `curve` is not the first key inside `flowchart`.
        "%%{init: {\"flowchart\": {\"htmlLabels\": false, \"curve\": \"linear\"}}}%%\n\
         flowchart TD\n  A --> B",
        // Bare (unquoted) keys and value — valid JSON5, which is what mermaid's directive body
        // actually is.
        "%%{init: {flowchart: {curve: linear}}}%%\nflowchart TD\n  A --> B",
        // No space after the colons.
        "%%{init:{\"flowchart\":{\"curve\":\"linear\"}}}%%\nflowchart TD\n  A --> B",
    ];
    for src in sources {
        let d = laid_out_curve(src, "basis");
        assert_eq!(
            d.edges[0].curve,
            Curve::Linear,
            "must resolve to Linear regardless of formatting: {src}"
        );
    }
}

/// A malformed `%%{init}%%` — bad JSON, a `curve` of the wrong type, `flowchart` misplaced — must
/// never crash, and must fall back all the way to the config default, exactly like an unrecognised
/// curve *name* already does (PRD design principle #3: an unknown or malformed value never blocks
/// a render).
#[test]
fn malformed_init_directive_falls_back_without_crashing() {
    let sources = [
        // `curve` is a number, not a string.
        "%%{init: {\"flowchart\": {\"curve\": 5}}}%%\nflowchart TD\n  A --> B",
        // `curve` is itself an object.
        "%%{init: {\"flowchart\": {\"curve\": {\"x\": 1}}}}%%\nflowchart TD\n  A --> B",
        // `curve` is an array.
        "%%{init: {\"flowchart\": {\"curve\": [\"linear\"]}}}%%\nflowchart TD\n  A --> B",
        // `flowchart` is a string, not an object — nothing to look a `curve` up inside.
        "%%{init: {\"flowchart\": \"oops\"}}%%\nflowchart TD\n  A --> B",
        // The directive's own JSON never closes.
        "%%{init: {\"flowchart\": {\"curve\": \"linear\"\nflowchart TD\n  A --> B",
        // `curve` at the top level, not nested under `flowchart` at all.
        "%%{init: {\"curve\": \"linear\"}}%%\nflowchart TD\n  A --> B",
        // Empty object.
        "%%{init: {}}%%\nflowchart TD\n  A --> B",
    ];
    for src in sources {
        let d = laid_out_curve(src, "step");
        assert_eq!(
            d.edges[0].curve,
            Curve::Step,
            "a malformed init directive must fall back to the config default: {src}"
        );
        let svg = render_curve(src, "dark", "step")
            .unwrap_or_else(|e| panic!("must still render despite a malformed directive: {e}"));
        assert!(svg.contains("<svg"));
    }
}

/// CJK labels alongside a real `%%{init}%%` `flowchart.curve` — the one combination this section's
/// other tests never exercise together, and the exact kind of "two features nobody tried at once"
/// gap `CLAUDE.md`'s testing policy calls out by name.
#[test]
fn cjk_labels_render_with_an_init_directive_curve() {
    let src = "%%{init: {\"flowchart\": {\"curve\": \"monotoneX\"}}}%%\n\
               flowchart TD\n  A[ツリー] -->|Enter| B{種別を解決}\n  B -->|画像| C[全画面プレビュー]";
    let d = laid_out_curve(src, "basis");
    assert_eq!(d.edges[0].curve, Curve::MonotoneX);
    let svg = render_curve(src, "dark", "basis").unwrap_or_else(|e| panic!("must render: {e}"));
    assert!(svg.contains("<svg"));
}

/// The wiring reaches all the way to the emitted `d=` attribute, over a corpus source with a real
/// bend (`long-edge`'s `A --> E` spans multiple ranks, so dagre gives it waypoints in between —
/// on two points `curve_basis_path` and `polyline_path` already agree, which would make this test
/// pass by accident).
#[test]
fn different_curves_draw_different_svg_path_data() {
    let (_, src) = CORPUS
        .iter()
        .find(|(name, _)| *name == "long-edge")
        .expect("corpus must still have `long-edge`");
    let basis = render_curve(src, "dark", "basis").expect("renders");
    let linear = render_curve(src, "dark", "linear").expect("renders");
    let step = render_curve(src, "dark", "step").expect("renders");
    assert_ne!(
        basis, linear,
        "a bent edge must draw differently under linear"
    );
    assert_ne!(basis, step, "a bent edge must draw differently under step");
    assert_ne!(
        linear, step,
        "linear and step must draw differently from each other too"
    );
}

/// CJK labels — konoma's standing gap in test coverage (`CLAUDE.md`'s own reminder) — must render
/// under every implemented curve without panicking.
#[test]
fn cjk_labels_render_under_every_curve() {
    let src = "flowchart TD\n  A[ツリー] -->|Enter| B{種別を解決}\n  \
               B -->|画像| C[全画面プレビュー]\n  B -->|テキスト| D[窓読み]\n  D --> A";
    for curve in [
        "basis",
        "linear",
        "step",
        "stepBefore",
        "stepAfter",
        "natural",
        "cardinal",
        "catmullRom",
        "monotoneX",
        "monotoneY",
        "bumpX",
        "bumpY",
        "rounded",
    ] {
        let svg = render_curve(src, "dark", curve).unwrap_or_else(|e| panic!("{curve}: {e}"));
        assert!(
            svg.contains("<svg"),
            "{curve}: CJK labels must still render"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 10-d. Coverage audit (2026-08-29): gaps found while auditing the curve feature's test coverage
// against a 35-point checklist. Each test below closes exactly one point the checklist named —
// see the doc comment for which.
// ---------------------------------------------------------------------------------------------

/// Checklist A4: an empty or whitespace-only curve name is exactly as unrecognised as any other
/// bad spelling — `Curve::parse` has no special case for either, so both fall through its
/// catch-all arm to `Curve::Basis`, the same fallback `unknown_curve_falls_back_to_basis_without_
/// crashing` already pins for `""`. That test never tried a *whitespace* string (as opposed to a
/// truly empty one), which is the shape checked here, both directly and through the full
/// `[ui] mermaid_curve` → render pipeline.
#[test]
fn whitespace_only_curve_name_falls_back_to_basis() {
    assert_eq!(Curve::parse("   "), Curve::Basis);
    assert_eq!(Curve::parse("\t"), Curve::Basis);
    assert_eq!(Curve::parse("\n"), Curve::Basis);

    let d = laid_out_curve("flowchart TD\n  A --> B", "   ");
    assert_eq!(d.edges[0].curve, Curve::Basis);

    let svg = render_curve("flowchart TD\n  A --> B", "dark", "  ")
        .expect("a blank curve name must still render, as Curve::Basis");
    assert!(svg.contains("<svg"));
}

/// Checklist B11: mermaid re-applies each `%%{init}%%` directive over its running config in
/// source order, so a *later* directive's `flowchart.curve` overrides an earlier one's —
/// `preprocess::strip_directives`'s own doc states this as the intended behaviour, but no
/// existing test actually put two init directives naming `flowchart.curve` in one source.
#[test]
fn a_second_init_directive_overrides_the_first_ones_curve() {
    let d = laid_out_curve(
        "%%{init: {\"flowchart\": {\"curve\": \"linear\"}}}%%\n\
         %%{init: {\"flowchart\": {\"curve\": \"step\"}}}%%\n\
         flowchart TD\n  A --> B",
        "basis",
    );
    assert_eq!(
        d.edges[0].curve,
        Curve::Step,
        "the second init directive must win over the first"
    );
}

/// Checklist B12: an `%%{init}%%` directive is not required to sit before the diagram's header —
/// `preprocess::strip_directives` scans the whole source rather than assuming a fixed position.
/// Every other init-directive test in this section puts the directive first; this one puts it
/// after the header and after an edge, which no existing test does.
#[test]
fn an_init_directive_after_the_header_still_sets_the_curve() {
    let d = laid_out_curve(
        "flowchart TD\n  A --> B\n  \
         %%{init: {\"flowchart\": {\"curve\": \"linear\"}}}%%\n  B --> C",
        "basis",
    );
    assert_eq!(
        d.edges[0].curve,
        Curve::Linear,
        "a mid-diagram init directive must still set the chart-wide curve"
    );
    assert_eq!(d.edges[1].curve, Curve::Linear);
}

/// Checklist C14 for `interpolate` specifically: `linkStyle 0,1,2 interpolate <curve>` — several
/// indices sharing one `interpolate` argument. `link_style_with_multiple_indices_reaches_every_
/// named_edge` already proves this shape for a plain style declaration; this is the same proof
/// for `interpolate`, which that test never exercised.
#[test]
fn link_style_interpolate_with_multiple_indices_reaches_every_named_edge() {
    let d = laid_out_curve(
        "flowchart TD\n  A --> B\n  B --> C\n  C --> D\n  D --> E\n  \
         linkStyle 0,1,2 interpolate linear",
        "basis",
    );
    assert_eq!(d.edges.len(), 4);
    for i in 0..3 {
        assert_eq!(
            d.edges[i].curve,
            Curve::Linear,
            "edge {i} is named by `linkStyle 0,1,2 interpolate linear`"
        );
    }
    assert_eq!(
        d.edges[3].curve,
        Curve::Basis,
        "edge 3 (D->E) was not named, and must keep the chart-wide default"
    );
}

/// Checklist C16: `linkStyle` naming an index past the last edge must not crash, and must leave
/// every real edge untouched. The adversarial sweep in `flowchart/tests.rs` already proves the
/// *parser* survives `linkStyle 99 stroke:red` on a source with no edges at all; this proves the
/// *render* pipeline does too, on a chart that has a real edge, with `interpolate` specifically,
/// and that the phantom index changes nothing real.
#[test]
fn link_style_interpolate_naming_an_index_past_the_last_edge_does_not_crash() {
    let d = laid_out_curve(
        "flowchart TD\n  A --> B\n  linkStyle 5 interpolate linear",
        "step",
    );
    assert_eq!(d.edges.len(), 1);
    assert_eq!(
        d.edges[0].curve,
        Curve::Step,
        "an out-of-range linkStyle index must touch no real edge"
    );
    let svg = render_curve(
        "flowchart TD\n  A --> B\n  linkStyle 5 interpolate linear",
        "dark",
        "step",
    )
    .expect("an out-of-range linkStyle index must not crash the render");
    assert!(svg.contains("<svg"));
}

/// Checklist C17: `linkStyle 0 interpolate <curve> <styles>` — an `interpolate` argument followed
/// by ordinary style declarations on the same statement — must apply *both*: the curve reaches
/// `PlacedEdge::curve` and the style reaches `PlacedEdge::style`, from one parse of one
/// statement. `flowchart::tests::link_style_reads_its_index_list_its_default_form_and_its_
/// interpolation` already proves the *parser* keeps both fields distinct; this is the render-level
/// proof that neither field's cascade clobbers the other's.
#[test]
fn link_style_interpolate_and_stroke_on_the_same_statement_both_apply() {
    let d = laid_out_curve(
        "flowchart TD\n  A --> B\n  linkStyle 0 interpolate linear stroke:#1f6feb",
        "basis",
    );
    assert_eq!(d.edges[0].curve, Curve::Linear, "the curve must apply");
    assert_eq!(
        d.edges[0]
            .style
            .as_ref()
            .expect("the style must apply too")
            .stroke
            .as_deref(),
        Some("#1f6feb")
    );
}

/// Checklist G31: `mermaid_curve` reaches only a flowchart's own edges. `spec_of`'s own doc states
/// that every other diagram kind's `SpecEdge::curve` is hardcoded to `Curve::Basis`, and the real
/// dispatcher (`preview::markdown::mermaid_to_svg_reason_flow`) does not even thread a `curve`
/// argument through to any other renderer's entry point. This is the end-to-end proof, through the
/// exact function the app calls (`mermaid_to_svg_curve`), for three diagram kinds.
#[test]
fn non_flowchart_diagrams_ignore_mermaid_curve() {
    use crate::preview::markdown::mermaid_to_svg_curve;
    let cases = [
        "stateDiagram-v2\n  [*] --> A\n  A --> B\n  B --> [*]",
        "classDiagram\n  A --|> B",
        "erDiagram\n  A ||--o{ B : has",
    ];
    for src in cases {
        let basis = mermaid_to_svg_curve(src, "dark", "basis").expect("must render");
        let step = mermaid_to_svg_curve(src, "dark", "step").expect("must render");
        assert_eq!(
            basis, step,
            "a non-flowchart diagram must draw identically regardless of mermaid_curve: {src}"
        );
    }
}

/// Checklist H34: `curve` still reaches an edge that names a subgraph as one of its own ends —
/// the `edges::End::Cluster` path through `route`, which no existing curve test exercises (every
/// other curve test in this section connects two ordinary nodes). `PlacedEdge::curve` is carried
/// unconditionally regardless of which `End` variant routed the edge (`lay_out_spec`'s own
/// `curve: edge.curve` push), so this is the proof at the level a reader would actually see: a
/// diagram with an edge crossing a subgraph's frame must still draw visibly differently under two
/// different curves, the same way `different_curves_draw_different_svg_path_data` proves it for
/// an ordinary bent edge.
#[test]
fn curve_reaches_an_edge_that_crosses_a_subgraph_frame() {
    let src = "flowchart LR\n  subgraph one [First]\n    A --> B\n  end\n  \
               subgraph two [Second]\n    C --> D\n  end\n  one --> two\n  E --> one";
    let d = laid_out_curve(src, "step");
    assert!(
        !d.clusters.is_empty(),
        "the source must actually produce a subgraph frame"
    );
    for e in &d.edges {
        assert_eq!(
            e.curve,
            Curve::Step,
            "every edge must resolve to the configured curve, cluster-bound or not"
        );
    }
    let basis = render_curve(src, "dark", "basis").expect("renders");
    let step = render_curve(src, "dark", "step").expect("renders");
    assert_ne!(
        basis, step,
        "a subgraph-bearing diagram must still draw visibly differently under two curves"
    );
}

/// Checklist H35: the legacy `graph` keyword is the same grammar as `flowchart`
/// (`flowchart/parser.rs`'s header parsing accepts both), so `curve` must resolve through it
/// identically — checked directly rather than assumed, since every other curve test in this file
/// spells the keyword `flowchart`. Both the config-default path and `linkStyle ... interpolate`
/// are checked, not just one.
#[test]
fn curve_resolves_identically_under_the_legacy_graph_keyword() {
    let flowchart = laid_out_curve("flowchart LR\n  A --> B", "monotoneX");
    let graph = laid_out_curve("graph LR\n  A --> B", "monotoneX");
    assert_eq!(flowchart.edges[0].curve, Curve::MonotoneX);
    assert_eq!(graph.edges[0].curve, Curve::MonotoneX);

    let flowchart2 = laid_out_curve(
        "flowchart LR\n  A --> B\n  linkStyle 0 interpolate step",
        "basis",
    );
    let graph2 = laid_out_curve(
        "graph LR\n  A --> B\n  linkStyle 0 interpolate step",
        "basis",
    );
    assert_eq!(flowchart2.edges[0].curve, Curve::Step);
    assert_eq!(graph2.edges[0].curve, Curve::Step);
}

// ---------------------------------------------------------------------------------------------
// 10-e. Independent second audit (2026-08-29): every existing degenerate-point/NaN-safety test
// above (`no_curve_ever_emits_nan_or_infinite_coordinates`,
// `all_thirteen_curves_match_d3_or_mermaid_on_degenerate_points_through_curve_path`) runs the 13
// curves over *hand-picked* point sets (`VERTICAL`, `HORIZONTAL`, `UNEVEN`, `DUPLICATE`, ...).
// That is exactly the shape of gap this feature has bitten konoma with once already: a synthetic
// point list proved the arithmetic safe while the *route dagre actually produces* for a real
// diagram — a self-loop, an edge crossing a subgraph frame, a parallel `A & B --> C & D` fan —
// never went through it, because those routes are built by `route`/`clip`'s own dedupe-and-insert
// logic (`edges.rs`'s module docs, steps 1-3) before a single coordinate ever reaches
// `Curve::path`. Nothing in this file had, until now, run the *specification corpus* (`CORPUS`,
// chosen from real diagram shapes — memory `corpus-from-spec-not-from-bugs`) across *every* curve
// name; every existing corpus-based curve test picks one corpus entry (`"long-edge"`) and at most
// three curves (`different_curves_draw_different_svg_path_data`), or one curve name across the
// whole corpus (`corpus_golden`, always `Curve::Basis`). This closes that cross product.
// ---------------------------------------------------------------------------------------------

/// Every entry in [`CORPUS`] — self-loop, edges crossing a subgraph frame both from inside and
/// from outside (`subgraph-bypass`, `subgraph-endpoint`), the `&`-group fan-out (`amp-chain`),
/// CJK labels, and the `classDef`/`linkStyle` cascade — run through **every** one of the 13 curve
/// names. Each combination must render, must contain no `NaN`/`Infinity` coordinate, must come
/// out a finite, positive size, and must actually rasterise — the same bar
/// `awkward_sources_produce_a_diagram_or_an_error_and_never_a_panic` sets for the default curve
/// alone, swept here over the full curve set instead.
///
/// Also covers the two structural cases no [`CORPUS`] entry has at all: a chart with nodes but
/// **zero** edges (so `Curve::path` is never even called — `spec_of`'s edge loop must still not
/// choke on an empty iterator, for every curve name it is handed) and a single node with no edges.
#[test]
fn every_corpus_source_survives_every_curve() {
    if !text_metrics::fonts_available() {
        return;
    }
    let curves = [
        "basis",
        "linear",
        "step",
        "stepBefore",
        "stepAfter",
        "natural",
        "cardinal",
        "catmullRom",
        "monotoneX",
        "monotoneY",
        "bumpX",
        "bumpY",
        "rounded",
    ];
    let extra: &[(&str, &str)] = &[
        ("zero-edges", "flowchart TD\n  A[one]\n  B[two]\n  C[three]"),
        ("single-node", "flowchart TD\n  A[Solo]"),
    ];
    for (name, src) in CORPUS.iter().chain(extra) {
        for curve in curves {
            let svg = render_curve(src, "dark", curve)
                .unwrap_or_else(|e| panic!("{name} under {curve}: must render: {e}"));
            assert!(
                !svg.contains("NaN") && !svg.contains("inf") && !svg.contains("Inf"),
                "{name} under {curve}: emitted a non-finite coordinate: {svg}"
            );
            let d = laid_out_curve(src, curve);
            assert!(
                d.width.is_finite() && d.height.is_finite() && d.width > 0.0 && d.height > 0.0,
                "{name} under {curve}: {}x{} is not a drawable size",
                num(d.width),
                num(d.height)
            );
            assert!(
                crate::preview::svg::rasterize_bytes(svg.as_bytes(), FsPath::new("m.svg"), 300)
                    .is_some(),
                "{name} under {curve}: the output did not rasterise"
            );
        }
    }
}

/// A far larger and more cyclic graph than anything in [`CORPUS`] (whose entries are all a
/// handful of edges) — a long rank chain closed into a cycle, and a chain of nested subgraphs —
/// run through every curve. The point sets a curve function sees here (many more waypoints per
/// edge from the extra ranks, more coincident-rank runs) are shaped differently from any
/// [`CORPUS`] entry's, which matters because the one real curve bug this crate has shipped
/// (`js_min`'s own doc, the `monotoneX`/vertical-run `NaN`) was found on a real rendered diagram,
/// not on a synthetic point list — so this is deliberately a *diagram*, built the way
/// `awkward_sources_produce_a_diagram_or_an_error_and_never_a_panic` builds its `wide`/`deep`
/// sources, rather than another hand-picked `&[Point]`.
#[test]
fn a_large_cyclic_and_deeply_nested_graph_survives_every_curve() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut wide = String::from("flowchart LR\n");
    for i in 0..40 {
        wide.push_str(&format!("  n{i} --> n{}\n", i + 1));
    }
    wide.push_str("  n0 --> n40\n  n40 --> n0\n");

    let mut nested = String::from("flowchart TD\n");
    for i in 0..8 {
        nested.push_str(&format!("  subgraph s{i} [Level {i}]\n"));
    }
    nested.push_str("  A --> B\n");
    for _ in 0..8 {
        nested.push_str("  end\n");
    }
    nested.push_str("  s0 --> C\n  C --> s7\n");

    let curves = [
        "basis",
        "linear",
        "step",
        "stepBefore",
        "stepAfter",
        "natural",
        "cardinal",
        "catmullRom",
        "monotoneX",
        "monotoneY",
        "bumpX",
        "bumpY",
        "rounded",
    ];
    for (label, src) in [
        ("wide-cycle", wide.as_str()),
        ("deep-nested", nested.as_str()),
    ] {
        for curve in curves {
            let svg = render_curve(src, "dark", curve)
                .unwrap_or_else(|e| panic!("{label} under {curve}: must render: {e}"));
            assert!(
                !svg.contains("NaN") && !svg.contains("inf") && !svg.contains("Inf"),
                "{label} under {curve}: emitted a non-finite coordinate"
            );
        }
    }
}

/// A second `linkStyle <n> interpolate` statement naming the **same** index as an earlier one:
/// mermaid re-applies each statement over its running config in source order (the same rule
/// `a_second_init_directive_overrides_the_first_ones_curve` already pins for `%%{init}%%`), so the
/// later one must win. `spec_of`'s `indexed_interpolate` picks this with `.filter_map(...)
/// .next_back()` over every matching `LinkStyle` — a real, distinct code path (it only does
/// anything different from `.next()` when more than one `LinkStyle` matches the same index) that
/// no existing test ever gives more than one matching statement to: every other `linkStyle
/// interpolate` test in this file names a given index exactly once.
#[test]
fn a_second_link_style_interpolate_on_the_same_index_overrides_the_first() {
    let d = laid_out_curve(
        "flowchart TD\n  A --> B\n  \
         linkStyle 0 interpolate step\n  linkStyle 0 interpolate linear",
        "basis",
    );
    assert_eq!(
        d.edges[0].curve,
        Curve::Linear,
        "a later linkStyle 0 interpolate must override an earlier one naming the same index"
    );

    // The same rule for `linkStyle default interpolate`, repeated.
    let d2 = laid_out_curve(
        "flowchart TD\n  A --> B\n  \
         linkStyle default interpolate step\n  linkStyle default interpolate linear",
        "basis",
    );
    assert_eq!(
        d2.edges[0].curve,
        Curve::Linear,
        "a later linkStyle default interpolate must override an earlier one"
    );
}

// ---------------------------------------------------------------------------------------------
// 10-f. Coverage audit follow-up (2026-08-29): two of the three items the audit flagged as
// unverified. `mermaid_curve`'s interaction with `mermaid_theme`/`mermaid_rows`/`svg_max_px` is
// finished here for `mermaid_theme` (both knobs are plain function parameters at this level) and
// in `app/tests.rs` for `mermaid_rows`/`svg_max_px` (which only exist as `App`/`[ui]` config, not
// as parameters `render_curve`/`mermaid_to_svg_curve` ever take). The third item — text mode never
// even reaching curve resolution — is also in `app/tests.rs`, next to the existing text-mode test.
// ---------------------------------------------------------------------------------------------

/// `[ui] mermaid_curve` and `[ui] mermaid_theme` sit on independent axes: changing one must never
/// move so much as a byte of what the other is responsible for. Checked both directions, so
/// neither can hide behind the other:
///
/// 1. holding theme fixed, every curve must draw the exact same colour-bearing attributes
///    (`fill="…"`/`stroke="…"`) — only the path/label geometry may move;
/// 2. holding curve fixed at something other than the default `basis`, every theme must draw the
///    exact same geometry — only colour may move.
///
/// Direction 2 is the gap `mermaid_themes_change_colours_but_never_font_metrics` (`markdown.rs`)
/// leaves open: that test always renders through `mermaid_to_svg`, which is `render_curve(...,
/// "basis")` by definition, so it has never actually exercised a non-default curve at all.
#[test]
fn mermaid_curve_and_mermaid_theme_are_independent_axes() {
    let (_, src) = CORPUS
        .iter()
        .find(|(name, _)| *name == "long-edge")
        .expect("corpus must still have `long-edge`");

    fn color_attrs(svg: &str) -> Vec<&str> {
        svg.split_whitespace()
            .filter(|w| w.starts_with("fill=\"") || w.starts_with("stroke=\""))
            .collect()
    }
    let geometry_of = |svg: &str| -> String {
        svg.split_whitespace()
            .filter(|w| !w.starts_with("fill=") && !w.starts_with("stroke=\""))
            .collect::<Vec<_>>()
            .join(" ")
    };

    // 1. Fixed theme, varying curve.
    let basis = render_curve(src, "dark", "basis").expect("renders");
    for curve in ["linear", "step", "monotoneX"] {
        let other = render_curve(src, "dark", curve).expect("renders");
        assert_eq!(
            color_attrs(&basis),
            color_attrs(&other),
            "curve {curve} must not move a single colour attribute away from the basis render"
        );
        assert_ne!(
            basis, other,
            "curve {curve} must still change something (the path data) — otherwise this \
             comparison is vacuous"
        );
    }

    // 2. Fixed curve (non-default), varying theme.
    let linear_dark = render_curve(src, "dark", "linear").expect("renders");
    let base_geometry = geometry_of(&linear_dark);
    for theme in ["light", "modern", "classic", "mermaid", "forest", "neutral"] {
        let other = render_curve(src, theme, "linear").expect("renders");
        assert_eq!(
            base_geometry,
            geometry_of(&other),
            "theme {theme} moved curve=\"linear\" geometry, not just colour"
        );
        assert_ne!(
            linear_dark, other,
            "theme {theme} must still change colour — otherwise this comparison is vacuous"
        );
    }
}

/// A hand-built two-point-or-more edge in `curve`, for the geometry tests above — the same shape
/// [`chart::rule`] builds for a chart's rule line, minus the theme/tip decisions this section
/// does not care about.
fn bent_edge(points: Vec<Point>, curve: Curve) -> PlacedEdge {
    PlacedEdge {
        from: "a".to_string(),
        to: "b".to_string(),
        points,
        gaps: Vec::new(),
        tip_start: Tip::None,
        tip_end: Tip::None,
        stroke: Stroke::Normal,
        label: None,
        start_label: None,
        end_label: None,
        badge: None,
        series: None,
        straight: false,
        overlay: false,
        style: None,
        curve,
        tip_matches_line: false,
    }
}

// ---------------------------------------------------------------------------------------------
// 10-g. Coverage audit follow-up (2026-08-29), item 2: `linkStyle <n>`'s index over **parallel**
// edges — more than one edge between the same node pair, self-loops included. `spec_of` resolves
// `linkStyle` purely from `chart.edges.iter().enumerate()` (`mod.rs`'s own comment: "`linkStyle`
// indices are positions in `Flowchart::edges`... exactly this iterator's index"), i.e. declaration
// order, before any layout ever runs — but nothing in this file had checked that against a source
// where two edges share both endpoints, the one shape where a bug that resolved the index against
// *layout* order instead (e.g. edges sorted by rank, or self-loops moved to the end of the list —
// both real things a routing pass could plausibly do) would be invisible on every existing test,
// since every existing test's edges are already distinguishable by their `(from, to)` pair alone.
//
// Each test below tells two same-pair edges apart by a distinct `linkStyle`-free discriminator
// (the edge's own label text), so it never has to *assume* `Diagram::edges` preserves declaration
// order — it looks each edge up by what it says, not by where it landed in the output `Vec`.
// ---------------------------------------------------------------------------------------------

/// The label text of an edge, or `""` if it carries none — the discriminator every test in this
/// section uses instead of trusting `d.edges`' own order.
fn edge_label_text(e: &PlacedEdge) -> String {
    e.label
        .as_ref()
        .map(|l| l.label.lines.join("\n"))
        .unwrap_or_default()
}

/// Two ordinary parallel `A --> B` edges, told apart by label: `linkStyle 0` must land on the
/// first-declared one (labelled `x`) and `linkStyle 1` on the second (labelled `y`) — never the
/// other way around, and never both on the same edge.
#[test]
fn link_style_index_targets_the_declared_edge_among_parallel_edges() {
    let d = laid_out_curve(
        "flowchart TD\n  A -->|x| B\n  A -->|y| B\n  \
         linkStyle 0 interpolate linear\n  linkStyle 1 interpolate step",
        "basis",
    );
    let x = d
        .edges
        .iter()
        .find(|e| edge_label_text(e) == "x")
        .expect("the \"x\"-labelled edge must exist");
    let y = d
        .edges
        .iter()
        .find(|e| edge_label_text(e) == "y")
        .expect("the \"y\"-labelled edge must exist");
    assert_eq!(
        x.curve,
        Curve::Linear,
        "linkStyle 0 must land on the first-declared A->B edge (labelled x)"
    );
    assert_eq!(
        y.curve,
        Curve::Step,
        "linkStyle 1 must land on the second-declared A->B edge (labelled y), not the first"
    );
}

/// The same proof over three parallel edges instead of two, so an off-by-one shift (an index
/// resolved one position early or late) cannot accidentally still land on the intended edge the
/// way it might by luck with only two.
#[test]
fn link_style_index_targets_the_declared_edge_among_three_parallel_edges() {
    let d = laid_out_curve(
        "flowchart TD\n  A -->|x| B\n  A -->|y| B\n  A -->|z| B\n  \
         linkStyle 0 interpolate linear\n  linkStyle 1 interpolate step\n  \
         linkStyle 2 interpolate monotoneX",
        "basis",
    );
    let find = |name: &str| {
        d.edges
            .iter()
            .find(|e| edge_label_text(e) == name)
            .unwrap_or_else(|| panic!("edge labelled {name} must exist"))
    };
    assert_eq!(
        find("x").curve,
        Curve::Linear,
        "linkStyle 0 -> first-declared (x)"
    );
    assert_eq!(
        find("y").curve,
        Curve::Step,
        "linkStyle 1 -> second-declared (y)"
    );
    assert_eq!(
        find("z").curve,
        Curve::MonotoneX,
        "linkStyle 2 -> third-declared (z)"
    );
}

/// A self-loop (`A --> A`) declared **before** two parallel `A --> B` edges must not shift the
/// indices those later edges' own `linkStyle` statements name: edge 0 is the loop, so `linkStyle
/// 1`/`linkStyle 2` must still land on `x`/`y`, not on the loop and not on each other.
#[test]
fn self_loop_before_parallel_edges_does_not_shift_link_style_indices() {
    let d = laid_out_curve(
        "flowchart TD\n  A --> A\n  A -->|x| B\n  A -->|y| B\n  \
         linkStyle 1 interpolate linear\n  linkStyle 2 interpolate step",
        "basis",
    );
    let x = d
        .edges
        .iter()
        .find(|e| edge_label_text(e) == "x")
        .expect("the \"x\"-labelled edge must exist");
    let y = d
        .edges
        .iter()
        .find(|e| edge_label_text(e) == "y")
        .expect("the \"y\"-labelled edge must exist");
    let loop_edge = d
        .edges
        .iter()
        .find(|e| e.from == "A" && e.to == "A")
        .expect("the self-loop must still be drawn");

    assert_eq!(
        loop_edge.curve,
        Curve::Basis,
        "the self-loop (index 0) names no linkStyle of its own; it must keep the chart-wide default"
    );
    assert_eq!(
        x.curve,
        Curve::Linear,
        "linkStyle 1 must land on the edge declared second (x), not on the self-loop"
    );
    assert_eq!(
        y.curve,
        Curve::Step,
        "linkStyle 2 must land on the edge declared third (y)"
    );
}

/// The same proof with the self-loop declared **between** the two parallel edges instead of
/// before them — the position most likely to confuse an implementation that (wrongly) keys edges
/// by `(from, to)` pair rather than by declaration order: `A --> A`'s pair never repeats, but it
/// sits between two entries that share the same pair as each other.
#[test]
fn self_loop_between_parallel_edges_does_not_shift_link_style_indices() {
    let d = laid_out_curve(
        "flowchart TD\n  A -->|x| B\n  A --> A\n  A -->|y| B\n  \
         linkStyle 0 interpolate linear\n  linkStyle 2 interpolate step",
        "basis",
    );
    let x = d
        .edges
        .iter()
        .find(|e| edge_label_text(e) == "x")
        .expect("the \"x\"-labelled edge must exist");
    let y = d
        .edges
        .iter()
        .find(|e| edge_label_text(e) == "y")
        .expect("the \"y\"-labelled edge must exist");
    let loop_edge = d
        .edges
        .iter()
        .find(|e| e.from == "A" && e.to == "A")
        .expect("the self-loop must still be drawn");

    assert_eq!(
        x.curve,
        Curve::Linear,
        "linkStyle 0 must land on the first-declared edge (x)"
    );
    assert_eq!(
        loop_edge.curve,
        Curve::Basis,
        "the self-loop (index 1) names no linkStyle; it must keep the chart-wide default"
    );
    assert_eq!(
        y.curve,
        Curve::Step,
        "linkStyle 2 must land on the third-declared edge (y), not shifted by the self-loop between them"
    );
}

// ---------------------------------------------------------------------------------------------
// `[ui] mermaid_routing = "konoma-orthogonal"` (docs/FEATURE-MERMAID-RENDERER.md §10, stage 1)
// ---------------------------------------------------------------------------------------------

/// Every `CORPUS` entry — a cluster-anchored edge (`one --> two`, `E --> one`, ...) is routed by
/// `orthogonal::route_flowchart` exactly like a node-to-node one now (§10-2's cluster-edge stage:
/// `orthogonal::cluster_as_node`/`build_by_id`), so `subgraph-*` sources no longer need excluding
/// from "every segment is axis-parallel" the way stage 1 through 5 had to exclude them.
fn orthogonal_corpus() -> Vec<(&'static str, &'static str)> {
    CORPUS.to_vec()
}

/// `samples/mermaid.ja.md`'s "大きさ" flowchart, byte-for-byte — deliberately **not** added to
/// `CORPUS` itself (`CORPUS` also drives the golden snapshot test, and this source is real-world
/// sized rather than hand-minimised for one property, so adding it there would both bloat the
/// golden and make failures on it harder to localise to one invariant). Kept as its own constant
/// because several standalone regression tests below (`orthogonal_settings_rules_sample_*`) each
/// need this exact source, and three hand-copies of the same ~30-line fixture already drifted from
/// "obviously the same string" into "trust me, diff it" before this was factored out.
const SETTINGS_RULES_SAMPLE: &str = "flowchart LR\n  F[ファイル] --> C{設定のルール}\n  \
               C -->|テキスト| T[窓読み]\n  C -->|コード| S[構文強調]\n  \
               C -->|Markdown| MD[ブロックモデル]\n  C -->|CSV / TSV| TB[表]\n  \
               C -->|画像| IM[デコード]\n  C -->|PDF| PD[ページ描画]\n  C -->|SVG| SV[usvg]\n  \
               C -->|動画| VD[キーフレーム]\n  C -->|書庫| AR[一覧]\n  \
               C -->|なし| NA[プレビュー不可]\n  MD --> MM[mermaid]\n  MD --> MA[数式]\n  \
               MM --> RS[ラスタライズ]\n  MA --> RS\n  SV --> RS\n  PD --> RS\n  \
               IM --> FIT[セルに合わせる]\n  RS --> FIT\n  VD --> FIT\n  FIT --> K{端末}\n  \
               K -->|kitty| KT[圧縮転送]\n  K -->|sixel / iTerm2| RI[画像プロトコル]\n  \
               K -->|それ以外| HB[ハーフブロック]\n  \
               classDef pix fill:#132a3a,stroke:#1f6feb,color:#c9d1d9\n  \
               classDef txt fill:#12291c,stroke:#2da44e,color:#c9d1d9\n  \
               class IM,PD,SV,VD,MM,MA,RS,FIT,KT,RI,HB pix\n  \
               class T,S,MD,TB,AR txt\n  \
               style NA fill:#2d2418,stroke:#d4a017,color:#c9d1d9";

/// [`orthogonal_corpus`], further filtered down to the sources
/// [`orthogonal_bend_count_never_exceeds_two`] can state a bend-count *cap* over. Two kinds of
/// edge are deliberately exempt from any cap, both drawn by the same mechanism
/// (`orthogonal::route_staircase_with_ports`: dagre's own waypoint chain, straightened, however
/// long it is) and both excluded here by source name rather than detected, since nothing on a
/// finished [`Diagram`] says which edge took which path any more:
///
/// * a **back edge** (§10-1 item 2's "補助・戻り辺は外周レーンのため回数制限なし" — a real
///   perimeter route is stage 5's job) — `branch`/`cjk`/`long-edge`/`left-right` each close a
///   cycle back onto an earlier node, and `self-loop` names one outright;
/// * a **collision-fallback forward edge** (§10-1 item 1, stage 3: a branch or merge whose route
///   crossed another node in *both* attempts falls back the same way). `amp-chain` is the one
///   corpus source that reaches this in practice — dumped and read by hand before being added
///   here, not guessed at: `A`'s branch to `D` crosses `B` (`A`'s own sibling source) on one
///   attempt and `C` (`D`'s own sibling target) on the other, so it falls back and comes out at 4
///   bends. `docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 2's "回数制限なし" is written about back
///   edges specifically, but the same reasoning applies here: a route that had to give up on both
///   direct right-angle shapes is in exactly the position stage 5's perimeter lanes are meant
///   for, not one this stage's ≤2 cap was ever meant to hold to. `strokes`' `A<-->F`/`C~~~E` and
///   `subgraph-bypass`'s `X->Y` join this list for the same reason (added 2026-09-01, once
///   `align_straight_lanes`'s `r + 1` adjacency bug was fixed and it started actually
///   straightening the chains these two edges skip across): `strokes`' `A-B-C-D-E-F` becomes one
///   straight column, and `A<-->F`/`C~~~E` skip clean across it, colliding on both attempts and
///   coming out at 6 bends each; `subgraph-bypass`'s `A->B` (inside the `one` frame `X->Y` has to
///   go around) straightens the same way, widening the detour `X->Y` needs above the frame to 4
///   bends. Both dumped and read by hand, not guessed at, the same as `amp-chain`.
/// * `subgraph-endpoint` and `subgraph-direction` joined this list for the same reason as
///   `amp-chain`, once `shape_crosses_a_node`'s own-endpoint check existed to find them — both
///   dumped and read by hand, not guessed at, and both the same two-step cascade:
///   `subgraph-endpoint`'s `E->one`: the original shape (`E` leaves `Top`, `one` receives `Left`)
///   held `E`'s own port coordinate across its first leg, which stayed inside `E`'s own real box
///   the whole way (the exact bug this check exists to catch), so `classify` swapped to the
///   alternate (`E` leaves `Right`, `one` receives `Bottom`) — which does not touch `E` at all, but
///   crosses `one`'s own member `A` instead (an ordinary *foreign*-node collision, real all along,
///   just never reached before the first swap started firing). Both fail, so it falls back to the
///   raw-derived staircase (3 bends). `subgraph-direction`'s `one->D` is the mirror image of the
///   same shape (`known_staircase_forward_edge`'s own doc has the numbers).
fn orthogonal_dag_corpus() -> Vec<(&'static str, &'static str)> {
    const UNCAPPED: &[&str] = &[
        "branch",
        "cjk",
        "long-edge",
        "left-right",
        "self-loop",
        "amp-chain",
        "strokes",
        "subgraph-bypass",
        "subgraph-endpoint",
        "subgraph-direction",
        // `C --> A` closes a cycle back onto the block's own member `A` — a back edge exactly
        // like the others in this list, just with one end inside a subgraph frame.
        "subgraph",
    ];
    orthogonal_corpus()
        .into_iter()
        .filter(|(name, _)| !UNCAPPED.contains(name))
        .collect()
}

/// Every segment of every routed edge in `d`, as `(a, b)` pairs.
///
/// `pub(super)` — reused by `state_tests`'s own orthogonal invariants (§10-5), which run this
/// same axis-parallel check over the state corpus rather than re-deriving it.
pub(super) fn edge_segments(d: &Diagram) -> Vec<(Point, Point)> {
    let mut out = Vec::new();
    for e in &d.edges {
        for w in e.points.windows(2) {
            out.push((w[0].clone(), w[1].clone()));
        }
    }
    out
}

pub(super) const AXIS_EPS: f64 = 1e-6;

/// `render_flow(..., curve, "splines")` is defined *as* `render_curve(..., curve)`'s own body
/// (`render_curve` in `mod.rs` is literally `render_flow(code, theme, curve, "splines")`), so this
/// is a decidability/non-panic check, not a regression guard: it states that routing a curve
/// through the `"splines"` string takes the same code path calling `render_curve` directly does,
/// and that path does not panic, across several curves and the whole corpus. **It cannot catch a
/// regression in what `"splines"` draws** — the moment the delegation above shipped, `a` and `b`
/// below became the same call with different names, i.e. `f(x) == f(x)`, the exact shape of
/// failure memory `markdown-block-walk-migration` records ("a live comparison goes silently
/// meaningless the instant B is B's own implementation"). See
/// [`splines_routing_signature_is_pinned`] for the actual regression guard.
#[test]
fn orthogonal_splines_routing_is_deterministic_and_does_not_panic() {
    for (name, src) in CORPUS {
        for curve in ["basis", "linear", "step", "monotoneX"] {
            let a = render_curve(src, "dark", curve)
                .unwrap_or_else(|e| panic!("{name}/{curve}: render_curve must render: {e}"));
            let b = render_flow(src, "dark", curve, "splines")
                .unwrap_or_else(|e| panic!("{name}/{curve}: render_flow must render: {e}"));
            assert_eq!(
                a, b,
                "{name}/{curve}: render_flow(..., \"splines\") must match render_curve(...) exactly \
                 (both calls take the same code path, so this can only fail on nondeterminism)"
            );
        }
    }
}

/// Re-dumps [`routing_signature`] for the five sources [`splines_routing_signature_is_pinned`]
/// pins, so a deliberate change to that test's expectations starts from real output rather than a
/// hand-edited guess. Run with `cargo test -- --ignored --nocapture` and copy the printed constants
/// in by hand — this does not write anywhere on disk, unlike `KONOMA_UPDATE_SNAPSHOTS=1`, which is
/// the point (see that test's doc comment).
#[test]
#[ignore]
fn dump_routing_signatures_for_pinning() {
    for name in [
        "branch",
        "left-right",
        "subgraph",
        "long-edge",
        "link-style-multi-index",
    ] {
        let (_, src) = CORPUS
            .iter()
            .find(|(n, _)| *n == name)
            .expect("known corpus name");
        let svg = render_flow(src, "dark", "basis", "splines").expect("must render");
        println!("=== {name} ===\n{}", routing_signature(&svg));
    }
}

const BRANCH_SIGNATURE: &str = "fill=none
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#333333
fill=#cccccc
fill=#333333
fill=#cccccc
";

const LEFT_RIGHT_SIGNATURE: &str = "fill=none
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#333333
fill=#cccccc
fill=#333333
fill=#cccccc
";

const SUBGRAPH_SIGNATURE: &str = "fill=none
fill=#2b2b38
stroke=#8a8a8a
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#cccccc
";

const LONG_EDGE_SIGNATURE: &str = "fill=none
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d3d3d3
fill=#d3d3d3
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
";

const LINK_STYLE_SIGNATURE: &str = "fill=none
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#1f6feb
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#1f6feb
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#1f6feb
fill=#d3d3d3
d=M#,#L#,#C#,# #,# #,#C#,# #,# #,#L#,#
fill=none
stroke=#d4a017
fill=#d3d3d3
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
fill=#1f2020
stroke=#cccccc
fill=#cccccc
";

/// The sentinel `orthogonal_splines_routing_is_deterministic_and_does_not_panic` cannot be, now
/// that `render_curve` is defined as `render_flow(..., "splines")`: a comparison between the two
/// is `f(x) == f(x)` (see that test's doc comment). This one does not call `render_curve` at all —
/// it pins `render_flow(..., "splines")` on its own, against a literal expected string baked into
/// *this source file*, not a `snapshots/` golden, so `KONOMA_UPDATE_SNAPSHOTS=1` cannot silently
/// carry a regression through. If `"splines"` ever stopped meaning splines routing — reached
/// `orthogonal::route_flowchart` by accident, or a future refactor broke the string match in
/// [`Routing::parse`](orthogonal) — this is the test that would actually notice, where the sentinel
/// above would not (both sides of its comparison would still be equally broken).
///
/// Five corpus sources stand in for the shapes a routing regression would plausibly move: a plain
/// forward chain with a back edge (`branch`), labelled edges (`left-right`), a subgraph frame
/// (`subgraph`), several back edges over a longer chain (`long-edge`), and `linkStyle`-coloured
/// edges (`link-style-multi-index`, the exact case `docs/STATUS.md`'s 2026-08-28 colour-cascade
/// regression came from). Expected values were obtained by dumping real output
/// ([`dump_routing_signatures_for_pinning`], `--ignored --nocapture`) and are
/// [`routing_signature`]'s masked `d`/`fill`/`stroke` columns rather than the whole SVG, for the
/// same reason `corpus_golden` masks numbers at all: an unmasked byte-exact pin would drift between
/// this machine's Helvetica and Linux CI's DejaVu Sans on nothing more than label width, for
/// reasons that have nothing to do with routing.
#[test]
fn splines_routing_signature_is_pinned() {
    const CASES: &[(&str, &str)] = &[
        ("branch", BRANCH_SIGNATURE),
        ("left-right", LEFT_RIGHT_SIGNATURE),
        ("subgraph", SUBGRAPH_SIGNATURE),
        ("long-edge", LONG_EDGE_SIGNATURE),
        ("link-style-multi-index", LINK_STYLE_SIGNATURE),
    ];
    for (name, expected) in CASES {
        let (_, src) = CORPUS
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name}: not in CORPUS"));
        let svg = render_flow(src, "dark", "basis", "splines")
            .unwrap_or_else(|e| panic!("{name}: must render: {e}"));
        assert_eq!(
            &routing_signature(&svg),
            expected,
            "{name}: splines routing signature drifted — re-dump with \
             dump_routing_signatures_for_pinning and inspect the diff before updating"
        );
    }
}

/// An unrecognised `routing` value falls back to `"splines"` — the same permissive contract every
/// unrecognised config value in this crate gets (mirrors `unknown_curve_falls_back_to_basis`
/// above), and specifically covers the bare word `"orthogonal"` (no `konoma-` prefix), which is
/// deliberately *not* the trigger — see `orthogonal::Routing::parse`'s own docs.
#[test]
fn unknown_routing_falls_back_to_splines_without_crashing() {
    let src = "flowchart TD\n  A --> B{cond}\n  B --> C\n  B --> D";
    let splines = render_flow(src, "dark", "basis", "splines").expect("must render");
    for unknown in ["xyz", "", "  ", "Orthogonal", "orthogonal", "ORTHOGONAL"] {
        let got = render_flow(src, "dark", "basis", unknown)
            .unwrap_or_else(|e| panic!("routing={unknown:?} must still render: {e}"));
        assert_eq!(
            splines, got,
            "routing={unknown:?} must resolve exactly like \"splines\""
        );
    }
}

/// Every routed edge's polyline, under `"konoma-orthogonal"`, is made of axis-parallel segments
/// only — no diagonal line — across the subgraph-free corpus (`orthogonal_corpus`): shapes,
/// strokes, CJK, all four directions, multiline labels, a self-loop, a long edge that skips ranks,
/// `A & B --> C & D` fan-out/fan-in, and `linkStyle`/`classDef` cascades.
#[test]
fn orthogonal_routing_draws_only_axis_parallel_segments() {
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        for (a, b) in edge_segments(&d) {
            let dx = (b.x - a.x).abs();
            let dy = (b.y - a.y).abs();
            assert!(
                dx < AXIS_EPS || dy < AXIS_EPS,
                "{name}: diagonal segment {a:?} -> {b:?}"
            );
        }
    }
}

/// An aligned pair (same coordinate across the flow, within half a pixel) draws as a bare
/// two-point straight run: zero bends, flow-centre to flow-centre.
#[test]
fn orthogonal_aligned_edge_is_a_straight_two_point_line() {
    let d = laid_out_flow("flowchart TD\n  A --> B", "basis", "konoma-orthogonal");
    let e = &d.edges[0];
    assert_eq!(e.points.len(), 2, "{:?}", e.points);
    assert!(
        (e.points[0].x - e.points[1].x).abs() < AXIS_EPS,
        "a straight-down TD edge must not drift sideways: {:?}",
        e.points
    );
    assert!(
        e.straight,
        "an orthogonal edge must draw as a polyline, not a curve"
    );
}

/// A decision node's two outgoing edges (branch) and a merge node's two incoming edges (merge).
///
/// The branch leg (`B->C`/`B->D`): `B` has only two out-edges, at or below
/// [`orthogonal::FAN_ELIGIBLE_MIN_BRANCHES`] (reimplemented 2026-09-02 — `classify`'s own
/// `fan_eligible` doc has the derivation, `docs/mermaid-theme/handoff/zz-design-sources.md`'s own
/// `2a` the reference), so 1b's own basic shape applies rather than the flow-axis retreat rule:
/// `B->C` is `classify`'s `aligned` (0-bend) shape (`C` sits directly downstream of `B`), and
/// `B->D` — not geometrically aligned — takes the *ordinary* cross-axis branch shape, one bend,
/// on `B`'s own Top face (`D` sits above `B`'s own row here), never sharing `B->C`'s own flow-axis
/// face at all. The two legs use two distinct physical faces, so there is no port-grid contest to
/// pin here (unlike the merge leg below, whose two edges *do* share one face — a real merge target
/// always accepts every incoming edge on its own flow-axis face, `merge_target_side`'s own doc,
/// regardless of how many there are).
///
/// The merge leg (`A->C`/`B->C`) is unaffected by that reimplementation — merge shapes were never
/// gated by `fan_eligible` (`classify`'s own `branching` ladder only ever consults it for a
/// *branching* source) — and keeps the original history this doc used to tell in full: `evict`'s
/// own port grid used to be centred on the claim *list's* own geometric mid-index (`(n-1)/2`),
/// which for an even claim count (`n = 2` here) has no slot at exactly `offset == 0.0` — so even
/// the aligned claim landed `PORT_SPACING/2 = 8px` off the face's true centre, and *neither* edge
/// drew as a straight line. §10-3's own "ファン列内の並び順"/"幹7区間の曲げ0" work
/// (`docs/FEATURE-MERMAID-RENDERER.md`) fixed the grid to anchor on *whichever slot the aligned/
/// trunk claim itself ends up in* instead (`evict`'s own doc on `anchor`) — required for `3a`'s
/// own even-count ten-way fan to draw its trunk edge with zero bends at all, and it applies to
/// *every* even-`n` face with an aligned/trunk claim, this two-claim one included. The aligned
/// claim now lands dead on the face's centre (a real 2-point line, not a 4-point one bent `±8px`
/// around it); its one competitor is pushed out to a full `PORT_SPACING = 16px` instead of half
/// of it, since the whole grid is now anchored one slot further out than the old symmetric one
/// was. Dumped and confirmed by hand, not assumed.
///
/// The merge case's own competitor (`B->C`) now lands on whichever side of `C`'s centre `B`
/// genuinely sits on (§10-3 item 12) rather than wherever an array-symmetric re-splice happened to
/// put it — see the assertion below for the concrete side.
#[test]
fn orthogonal_branch_and_merge_edges_share_a_face_one_aligned_one_bends_twice() {
    let branch = laid_out_flow(
        "flowchart LR\n  A --> B{cond}\n  B --> C\n  B --> D",
        "basis",
        "konoma-orthogonal",
    );
    let bc = branch
        .edges
        .iter()
        .find(|e| e.from == "B" && e.to == "C")
        .expect("edge B->C must exist");
    let bd = branch
        .edges
        .iter()
        .find(|e| e.from == "B" && e.to == "D")
        .expect("edge B->D must exist");
    let b_node = branch.node("B").expect("B must exist");
    assert_eq!(
        bc.points.len(),
        2,
        "B->C is the aligned leg — a straight 2-point line: {:?}",
        bc.points
    );
    assert_eq!(
        bd.points.len(),
        3,
        "B->D is a plain 3-way-or-fewer branch (1b's own basic shape, not the flow-axis retreat \
         rule) — one bend, on B's own cross-axis face: {:?}",
        bd.points
    );
    // B->C exits dead on B's own centre (its own flow-axis face, shared with nothing since B->D
    // now uses a different, cross-axis face entirely).
    let bc_exit_y = bc.points[0].y;
    assert!(
        (bc_exit_y - b_node.center.y).abs() < 1e-6,
        "B->C (aligned) exits exactly on B's centre: {bc_exit_y} vs {}",
        b_node.center.y
    );
    // B->D exits B's own Top face (D sits above B's own row), centred on it — B's only claim on
    // that face — [`orthogonal::PORT_INSET`] outside the node's own top edge.
    let (_, b_top, _, _) = b_node.bounds();
    let bd_exit = &bd.points[0];
    assert!(
        (bd_exit.x - b_node.center.x).abs() < 1e-6,
        "B->D exits centred on B's own Top face: {bd_exit:?} vs centre x {}",
        b_node.center.x
    );
    assert!(
        (bd_exit.y - (b_top - orthogonal::PORT_INSET)).abs() < 1e-6,
        "B->D exits PORT_INSET outside B's own top edge: {bd_exit:?} vs top {b_top}",
    );

    let merge = laid_out_flow(
        "flowchart LR\n  A --> C\n  B --> C\n  C --> D",
        "basis",
        "konoma-orthogonal",
    );
    let ac = merge
        .edges
        .iter()
        .find(|e| e.from == "A" && e.to == "C")
        .expect("edge A->C must exist");
    let bc2 = merge
        .edges
        .iter()
        .find(|e| e.from == "B" && e.to == "C")
        .expect("edge B->C must exist");
    let c_node = merge.node("C").expect("C must exist");
    assert_eq!(
        ac.points.len(),
        2,
        "A->C is the aligned leg — a straight 2-point line: {:?}",
        ac.points
    );
    assert_eq!(
        bc2.points.len(),
        4,
        "B->C must bend twice: {:?}",
        bc2.points
    );
    // A->C enters dead on C's own centre; B->C is pushed a full PORT_SPACING off it, on
    // *whichever side B genuinely sits on* (§10-3 item 12, `orthogonal::evict`'s own doc on
    // `anchor`): B sits below A/C here (dumped: B.center.y = 101.4 > C.center.y = 32.0), so B->C's
    // port belongs *below* C's centre, not above it. The pre-item-12 `evict` forced the aligned
    // claim into the claims array's own geometric middle index regardless of where it naturally
    // sorted, which could drag a genuinely-below sibling like this one above centre instead — this
    // assertion used to pin that wrong side; item 12's fix is what corrected it.
    let ac_entry_y = ac.points.last().unwrap().y;
    let bc2_entry_y = bc2.points.last().unwrap().y;
    assert!(
        (ac_entry_y - c_node.center.y).abs() < 1e-6,
        "A->C (aligned) enters exactly on C's centre: {ac_entry_y} vs {}",
        c_node.center.y
    );
    assert!(
        (bc2_entry_y - (c_node.center.y + orthogonal::PORT_SPACING)).abs() < 1e-6,
        "B->C enters PORT_SPACING below C's centre (B genuinely sits below C): {bc2_entry_y} vs {}",
        c_node.center.y
    );
}

/// A `{}` decision node draws as an eight-vertex chamfered rectangle when `[ui] mermaid_routing =
/// "konoma-orthogonal"`, sized exactly like an ordinary rectangle — and as the ordinary diamond,
/// unchanged, under `"splines"`. `Glyph::ChamferedRect`'s own docs explain why a diamond has no
/// flat run for a port to land on.
#[test]
fn decision_node_is_chamfered_under_orthogonal_and_a_diamond_under_splines() {
    let src = "flowchart TD\n  A --> B{cond}\n  B --> C";

    let splines = laid_out_curve(src, "basis");
    let b_splines = splines.node("B").expect("B must exist");
    assert_eq!(b_splines.shape, Glyph::Flow(Shape::Diamond));

    let ortho = laid_out_flow(src, "basis", "konoma-orthogonal");
    let b_ortho = ortho.node("B").expect("B must exist");
    assert_eq!(b_ortho.shape, Glyph::ChamferedRect);

    let polygon = shapes::polygon(b_ortho.shape, b_ortho.size);
    assert_eq!(
        polygon.len(),
        8,
        "a chamfered rectangle has eight vertices: {polygon:?}"
    );

    // Sized like an ordinary rectangle, not doubled the way a diamond is (§10-1: "菱形の2倍拡大を
    // しない").
    let rect_size = shapes::size(
        Glyph::Flow(Shape::Rect),
        Size::new(b_ortho.label.width, b_ortho.label.height),
    );
    assert_eq!(b_ortho.size.w, rect_size.w);
    assert_eq!(b_ortho.size.h, rect_size.h);

    // Reaches the actual SVG, not just the geometry model.
    let svg = render_flow(src, "dark", "basis", "konoma-orthogonal").expect("must render");
    assert!(
        svg.contains("<polygon"),
        "an orthogonal decision node must draw a <polygon>, not a diamond outline"
    );
}

/// A class/ER diagram draws identically whether `"splines"` or `"konoma-orthogonal"` is asked
/// for, because each of their own `spec_of` always sets `GraphSpec::routing` to
/// `Routing::Splines` regardless of the caller's setting (§10-5's own scope line: "他図種
/// （class/ER…）は今回対象外"). A state diagram is the one exception since
/// `docs/FEATURE-MERMAID-RENDERER.md` §10-5 (the flowchart was already one, from §10 itself) —
/// see `a_state_diagram_draws_differently_under_konoma_orthogonal`, just below, for its own half
/// of what used to be this same assertion. Same shape as `non_flowchart_diagrams_ignore_mermaid_
/// curve` above, through the real dispatcher.
#[test]
fn non_flowchart_diagrams_ignore_mermaid_routing() {
    use crate::preview::markdown::mermaid_to_svg_flow;
    let cases = ["classDiagram\n  A --|> B", "erDiagram\n  A ||--o{ B : has"];
    for src in cases {
        let splines = mermaid_to_svg_flow(src, "dark", "basis", "splines")
            .unwrap_or_else(|| panic!("{src}: must render under splines"));
        let ortho = mermaid_to_svg_flow(src, "dark", "basis", "konoma-orthogonal")
            .unwrap_or_else(|| panic!("{src}: must render under konoma-orthogonal"));
        assert_eq!(
            splines, ortho,
            "{src}: a non-flowchart diagram must draw identically regardless of mermaid_routing"
        );
    }
}

/// §10-5's own wiring point: a state diagram's edges *do* read `[ui] mermaid_routing` now, the
/// same as a flowchart's — `konoma-orthogonal` draws its transitions as right-angled polylines
/// (no cubic-Bezier `C` command) where `splines` draws mermaid's own curve, while every node's own
/// box/marker geometry — including the start dot's and end ring's fixed radii, S1's own token
/// list — stays byte-identical between the two (only the *edges* a routing mode governs at all,
/// `docs/FEATURE-MERMAID-RENDERER.md` §10-2's own "座標段の後処理と辺経路だけを足す").
#[test]
fn a_state_diagram_draws_differently_under_konoma_orthogonal() {
    use crate::preview::markdown::mermaid_to_svg_flow;
    let src = "stateDiagram-v2\n  [*] --> A\n  A --> B\n  B --> [*]";
    let splines = mermaid_to_svg_flow(src, "dark", "basis", "splines").expect("splines renders");
    let ortho =
        mermaid_to_svg_flow(src, "dark", "basis", "konoma-orthogonal").expect("orthogonal renders");
    assert_ne!(
        splines, ortho,
        "a state diagram's edges must actually change under konoma-orthogonal"
    );
    assert!(
        !ortho.contains('C'),
        "konoma-orthogonal must draw right angles, not a cubic-Bezier curve:\n{ortho}"
    );
    assert!(
        splines.contains('C'),
        "splines must still draw its own curve unchanged:\n{splines}"
    );
    // The start dot's and end ring's radii are pinned tokens (§10-5 S1), not layout — they must
    // not move a pixel just because the routing mode changed.
    assert!(
        ortho.contains("r=\"7\"") && splines.contains("r=\"7\""),
        "the start dot's 7px radius must be routing-independent"
    );
    assert!(
        ortho.contains("r=\"6.25\"") && splines.contains("r=\"6.25\""),
        "the end ring's outer 6.25px radius must be routing-independent"
    );
}

/// Every routed edge's two endpoints sit exactly [`orthogonal::PORT_INSET`] px **outside** the
/// node — or, for a cluster-anchored end (§10-2), the subgraph frame — each one meets, never
/// touching, let alone crossing into, the real geometric boundary — and the segment that reaches
/// each one is perpendicular to whichever face it lands on. §10-1 item 1's last bullet: "矢尻…の
/// 先端はノード枠の外縁から1px離す＝端点は枠座標の2px手前", and item 1's "出入りは常に辺へ垂直".
/// `orthogonal_corpus()` now includes every `subgraph-*` source, so this same loop is what checks
/// a cluster-anchored edge's own endpoint (`subgraph-endpoint`'s `one --> two` / `E --> one`)
/// against exactly the rule a node-anchored one is held to — no separate cluster-only test needed.
///
/// Pins the *direction* of the offset, not only its magnitude — a real regression once got this
/// backwards: an endpoint pulled *inward* (into the node) rather than outward drew an arrow tip
/// that pokes into the node's own interior, which stayed invisible in the finished picture only
/// because `svg::emit` paints nodes *after* edges, so the poking-in part vanished under the node's
/// opaque fill and the visible tip looked flush with the boundary instead of clear of it — a bug a
/// test that only checked the offset's *magnitude* would have missed entirely, since `t + inset`
/// and `t - inset` are both "some distance from `t`". See
/// `orthogonal_arrow_tip_has_a_visible_gap_from_the_node_in_real_pixels` for the pixel-level half
/// of this same proof (confirmed against a rasterised scan before either test was written).
#[test]
fn orthogonal_endpoints_sit_outside_the_node_and_arrive_perpendicular() {
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        assert_endpoints_sit_outside_and_perpendicular(name, &d);
    }
}

/// The per-diagram body of [`orthogonal_endpoints_sit_outside_the_node_and_arrive_perpendicular`],
/// factored out (matching [`assert_no_segment_crosses_a_foreign_node`]'s own shape below) so the
/// real-diagram regression tests — `samples/mermaid.ja.md`'s "大きさ" flowchart among them — can
/// state the exact same question a hand-built `CORPUS` fixture is held to, rather than only ever
/// running over `orthogonal_corpus()`. That gap is exactly how a real endpoint-dragged-into-its-
/// own-node regression (`route_with_ports`'s own doc: `MD->MM`'s bottom port on this very sample)
/// went unnoticed by this invariant before: it existed, but its only caller was `CORPUS`, which has
/// never included a diagram large enough for `align_straight_lanes` to move a node far enough to
/// stress this the way a real ~20-node flowchart does.
fn assert_endpoints_sit_outside_and_perpendicular(name: &str, d: &Diagram) {
    for e in &d.edges {
        if e.points.len() < 2 {
            continue;
        }
        let n = e.points.len();
        let ends = [
            (&e.from, &e.points[0], &e.points[1]),
            (&e.to, &e.points[n - 1], &e.points[n - 2]),
        ];
        for (node_id, endpoint, neighbour) in ends {
            // A cluster-anchored end (§10-2) meets a subgraph frame instead of a node — same
            // box shape (`PlacedCluster::bounds` reads exactly like `PlacedNode::bounds`), so
            // this checks whichever one `node_id` actually names rather than skipping it.
            let bounds = d
                .node(node_id)
                .map(|n| n.bounds())
                .or_else(|| d.cluster(node_id).map(|c| c.bounds()));
            let Some((l, t, r, b)) = bounds else {
                continue;
            };
            let on_top = (endpoint.y - (t - orthogonal::PORT_INSET)).abs() < AXIS_EPS;
            let on_bottom = (endpoint.y - (b + orthogonal::PORT_INSET)).abs() < AXIS_EPS;
            let on_left = (endpoint.x - (l - orthogonal::PORT_INSET)).abs() < AXIS_EPS;
            let on_right = (endpoint.x - (r + orthogonal::PORT_INSET)).abs() < AXIS_EPS;
            assert!(
                on_top || on_bottom || on_left || on_right,
                "{name}: edge {}->{} endpoint at {node_id} {endpoint:?} is not {}px \
                 OUTSIDE its bounds {:?}",
                e.from,
                e.to,
                orthogonal::PORT_INSET,
                (l, t, r, b)
            );

            let dx = (endpoint.x - neighbour.x).abs();
            let dy = (endpoint.y - neighbour.y).abs();
            assert!(
                dx < AXIS_EPS || dy < AXIS_EPS,
                "{name}: edge {}->{} segment into {node_id} is not axis-parallel: \
                 {neighbour:?} -> {endpoint:?}",
                e.from,
                e.to
            );
            if on_top || on_bottom {
                assert!(
                    dy > AXIS_EPS && dx < AXIS_EPS,
                    "{name}: edge {}->{} must meet {node_id} vertically at a top/bottom face: \
                     {neighbour:?} -> {endpoint:?}",
                    e.from,
                    e.to
                );
            } else {
                assert!(
                    dx > AXIS_EPS && dy < AXIS_EPS,
                    "{name}: edge {}->{} must meet {node_id} horizontally at a left/right \
                     face: {neighbour:?} -> {endpoint:?}",
                    e.from,
                    e.to
                );
            }
        }
    }
}

/// Every routed edge, checked over its **whole** length, must never re-enter its own two endpoint
/// nodes' real (unpadded) interior — the invariant [`assert_no_segment_crosses_a_foreign_node`]
/// structurally cannot state, because it excludes an edge's own two ends from the node list it
/// checks against (correctly so: a port sits only [`orthogonal::PORT_INSET`] outside its own face,
/// well inside [`orthogonal::COLLISION_MARGIN`]'s padding, so the *padded* test would flag every
/// edge's own harmless graze near its own port). [`orthogonal::staircase_punctures_its_own_endpoint`]
/// is the *unpadded* test that tells the two apart — the same one [`route_with_ports`] itself now
/// runs before ever handing a raw-derived staircase route to `clear_local_route`, so this test and
/// the router's own decision are stated in terms of the same function rather than two hand-rolled
/// copies that could quietly drift apart.
///
/// Reused by the corpus-wide loop below and by the real-diagram regression tests, the same split
/// [`assert_no_segment_crosses_a_foreign_node`] already has.
fn assert_no_edge_crosses_its_own_endpoint(name: &str, d: &Diagram) {
    for e in &d.edges {
        let (Some(source), Some(target)) = (d.node(&e.from), d.node(&e.to)) else {
            continue; // a cluster-anchored end has no real node box to puncture
        };
        assert!(
            !orthogonal::staircase_punctures_its_own_endpoint(
                &e.points,
                Some(source),
                Some(target)
            ),
            "{name}: edge {}->{} re-enters its own endpoint node's interior: {:?}, \
             source {:?}, target {:?}",
            e.from,
            e.to,
            e.points,
            source.bounds(),
            target.bounds()
        );
    }
}

/// [`assert_no_edge_crosses_its_own_endpoint`], run over the whole corpus — the counterpart to
/// [`orthogonal_no_segment_crosses_a_foreign_node_across_the_whole_corpus`] for an edge's own two
/// ends rather than every other node.
#[test]
fn orthogonal_no_edge_crosses_its_own_endpoint_across_the_whole_corpus() {
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        assert_no_edge_crosses_its_own_endpoint(name, &d);
    }
}

/// The pixel-level half of the proof above: rasterises `A --> B` under `"konoma-orthogonal"` and
/// scans the column under the arrow, from inside `A` down through the arrow and into `B`, looking
/// for the transparent gap `docs/FEATURE-MERMAID-RENDERER.md` §10-1 asks for between the arrow
/// tip's ink and `B`'s own stroke ink.
///
/// This is the test that actually would have caught the inward-vs-outward regression the sibling
/// vector-level test's own doc describes: with the endpoint pulled inward, `B`'s node (painted
/// after the edge) covers the part of the tip that pokes past the boundary and the scan below
/// finds ink touching ink with **no** transparent row in between — which is exactly what a
/// pre-fix run of this test showed before this fix landed (an SVG dump and a 4×-rasterised pixel
/// scan were both read by hand — `docs/STATUS.md`'s "検証してから言う" — not inferred from the
/// vector coordinates alone).
#[test]
fn orthogonal_arrow_tip_has_a_visible_gap_from_the_node_in_real_pixels() {
    let src = "flowchart TD\n  A --> B";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let a = d.node("A").expect("A must exist");
    let b = d.node("B").expect("B must exist");
    assert!(
        (a.center.x - b.center.x).abs() < AXIS_EPS,
        "the fixture must be a straight vertical drop so one column crosses both the arrow and B's top edge"
    );

    let svg = render_flow(src, "dark", "basis", "konoma-orthogonal").expect("must render");
    let scale = 4.0;
    let img = crate::preview::svg::rasterize_bytes(
        svg.as_bytes(),
        FsPath::new("orthogonal-tip-gap.svg"),
        (d.width.max(d.height) * scale).round() as u32,
    )
    .expect("must rasterize");
    let rgba = img.to_rgba8();

    let cx = (a.center.x * scale).round() as u32;
    let (_, b_top, _, _) = b.bounds();
    let scan_top = ((a.center.y) * scale).round() as u32;
    let scan_bottom = ((b_top + 2.0) * scale).round() as u32;

    // Column already inside A, at its own centre, must not be transparent (A's own fill).
    assert_ne!(
        rgba.get_pixel(cx, scan_top)[3],
        0,
        "the scan must start inside A's own fill, or it is not testing the right column"
    );
    // Somewhere between the arrow's ink and B's top edge, the column must go fully transparent —
    // the gap the spec asks for. A flush (zero-gap) tip, or one hidden under B's own paint,
    // leaves no such row.
    let saw_transparent_gap = (scan_top..=scan_bottom).any(|y| rgba.get_pixel(cx, y)[3] == 0);
    assert!(
        saw_transparent_gap,
        "no fully-transparent row between the arrow tip and B's top edge — the tip is touching \
         or crossing into B rather than clearing it by ~1px"
    );
    // And B's own stroke ink (its outline colour, not its fill) really is there, at or after its
    // geometric top edge — confirming the gap ends at B and is not some unrelated blank band.
    let stroke_hex = theme::Theme::named("dark").node_stroke;
    let (sr, sg, sb) = hex_to_rgb(stroke_hex);
    let saw_node_stroke = (scan_top..=scan_bottom).any(|y| {
        let p = rgba.get_pixel(cx, y);
        p[3] == 255 && close(p[0], sr) && close(p[1], sg) && close(p[2], sb)
    });
    assert!(
        saw_node_stroke,
        "B's own stroke colour ({stroke_hex}) must appear in the scanned column"
    );
}

/// `#rrggbb` → `(r, g, b)`, for comparing against a rasterised pixel.
fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap();
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap();
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap();
    (r, g, b)
}

/// Whether two 8-bit channel values are close enough to call the same colour — antialiasing at a
/// shape's own edge blends a pixel or two, so an exact match is the wrong bar here.
fn close(a: u8, b: u8) -> bool {
    (a as i16 - b as i16).abs() <= 6
}

/// §10-1 item 5, "辺・矢尻・ラベル文字・ノード枠を同色で揃える": under `"konoma-orthogonal"`, an
/// edge's arrow head is painted the line's own resolved stroke colour (`class`/`:::`/`linkStyle`,
/// falling back to the theme default) rather than mermaid's fixed `theme.arrowhead` — confirmed
/// both in the emitted markup (`svg::emit_tip`'s `color` parameter) and, for the theme-default
/// case, that it differs from `"splines"`'s fixed colour.
#[test]
fn orthogonal_arrow_head_matches_the_edges_resolved_stroke_colour() {
    let src = "flowchart LR\n  A --> B\n  linkStyle 0 stroke:#1f6feb";

    let ortho = render_flow(src, "dark", "basis", "konoma-orthogonal").expect("must render");
    assert!(
        ortho.contains("stroke=\"#1f6feb\""),
        "the edge's own path must carry its linkStyle colour: {ortho}"
    );
    let arrow_at = ortho
        .find("<polygon points=")
        .expect("an arrow head polygon must be emitted");
    let fill = attr(&ortho[arrow_at..], "fill").expect("the polygon must carry a fill");
    assert_eq!(
        fill, "#1f6feb",
        "an orthogonal arrow head must be painted the edge's resolved linkStyle colour: {ortho}"
    );

    // Under splines, the very same source keeps mermaid's own fixed arrowhead colour: the
    // linkStyle blue on the line, the theme's grey on the tip — the two are independent, unlike
    // under orthogonal routing.
    let splines = render_flow(src, "dark", "basis", "splines").expect("must render");
    assert!(splines.contains("stroke=\"#1f6feb\""));
    let arrow_at = splines
        .find("<polygon points=")
        .expect("an arrow head polygon must be emitted");
    let fill = attr(&splines[arrow_at..], "fill").expect("the polygon must carry a fill");
    let theme_arrowhead = theme::Theme::named("dark").arrowhead;
    assert_eq!(
        fill, theme_arrowhead,
        "splines must keep mermaid's own fixed arrowhead colour regardless of linkStyle: {splines}"
    );
    assert_ne!(
        fill, "#1f6feb",
        "this assertion is only meaningful if the theme default actually differs from the \
         linkStyle colour used above"
    );
}

/// A node with several colour-coded outgoing edges (`linkStyle <n> stroke:...`, the real-world
/// shape `docs/STATUS.md` records the 2026-08-28 `linkStyle` regression coming from) each get
/// their own arrow head colour under `"konoma-orthogonal"` — not all painted the same, and not
/// all left at the theme default.
#[test]
fn orthogonal_arrow_heads_differ_per_edge_when_their_linkstyles_differ() {
    let src = "flowchart TD\n  A --> B\n  A --> C\n  \
               linkStyle 0 stroke:#1f6feb\n  linkStyle 1 stroke:#d4a017";
    let svg = render_flow(src, "dark", "basis", "konoma-orthogonal").expect("must render");
    let mut fills = Vec::new();
    let mut rest = svg.as_str();
    while let Some(at) = rest.find("<polygon points=") {
        let slice = &rest[at..];
        fills.push(
            attr(slice, "fill")
                .expect("every polygon here carries a fill")
                .to_string(),
        );
        rest = &slice[1..];
    }
    assert!(
        fills.contains(&"#1f6feb".to_string()) && fills.contains(&"#d4a017".to_string()),
        "each edge's own linkStyle colour must reach its own arrow head: {fills:?}"
    );
}

/// The value of `attr` inside the first tag `haystack` starts with — a small hand-rolled reader
/// rather than an XML parser, since all this needs is one attribute out of one already-known tag.
fn attr<'a>(haystack: &'a str, attr: &str) -> Option<&'a str> {
    let needle = format!("{attr}=\"");
    let start = haystack.find(&needle)? + needle.len();
    let end = start + haystack[start..].find('"')?;
    Some(&haystack[start..end])
}

// ---------------------------------------------------------------------------------------------
// Stage 2: port eviction (docs/FEATURE-MERMAID-RENDERER.md §10-1 item 1, "退避則")
// ---------------------------------------------------------------------------------------------
//
// `orthogonal::tests` (in `orthogonal.rs`) states the eviction rules directly, against
// hand-built `PlacedNode`s that put an exact number of claims on an exact face without depending
// on dagre's own placement to happen to produce that shape. The tests below are the other half:
// real flowchart sources through the actual `render_flow`/`lay_out_flow` production entry
// points, so the *wiring* between `lay_out_spec`'s growth-retry loop and `orthogonal::evict` is
// what is being checked, not just the eviction formula in isolation. Every fixture and every
// pinned number below was found by dumping real output first (`laid_out_flow`/`laid_out_curve`
// printed by hand) and reading it, not by predicting what dagre would do — a 9-way fan-in was
// the first source that actually produced 4 merges on one face and forced real node growth; a
// naive guess (3 sources, one per face) split unevenly across dagre's own barycenter placement
// instead and grew nothing.

/// A real 9-way fan-in (`A`..`I` all merge into `Z`) is not evenly spread by dagre. Before
/// `align_straight_lanes`'s `r + 1` adjacency bug was fixed (2026-09-01), the function never
/// selected a lane at all, so this fixture's expected values were dagre's own unaligned placement.
/// Now that lane alignment actually runs, `A` (the smallest cross-coordinate candidate — every one
/// of `A`..`I` has exactly one out-edge to `Z`, so `Z`'s single incoming chain slot goes to
/// whichever wins "タイは上・左優先", `align_straight_lanes`'s own tie-break) wins the slot and
/// `Z` moves to sit under `A` instead — the leftmost node in the whole fan, not the centre.
///
/// §10-3 item 3's own correction (`classify`'s `merge_target_side` doc): a merge target now always
/// rides the *flow*-axis face, not the cross-axis one this fixture originally exercised — for `TD`
/// that is Top/Bottom, so `A->Z` (aligned) and every one of `B->Z`..`I->Z` (ordinary merges) all
/// land on `Z`'s single Top face together, dumped and confirmed (all nine share the exact same
/// `y`), not assumed — `Z`'s Left/Right faces go entirely unused.
///
/// §10-3 item 12's own "面のポートは相手の側で配る" (`docs/FEATURE-MERMAID-RENDERER.md`) decides how
/// wide: `A` is not the fan's own geometric centre here, it is dagre's own *leftmost* candidate
/// (this test's own doc, above — `align_straight_lanes`'s "タイは上・左優先" tie-break, not
/// `mod.rs::regroup_fan_lanes`, which only rearranges a *source's* own fan-out, never a merge
/// target's incoming claims). Dumped and confirmed: every one of `B`..`I` sits to `A`'s own right,
/// so all eight non-aligned claims land on the *same* side of `Z`'s centre — item 12's own "ノード
/// は大きい側に合わせて中心対称に拡大" still grows the box symmetrically around that centre even
/// though the near side carries nothing, `(8)*16 + 2*8 = 144px` of *reach* on the busy side alone
/// (the widest single offset any port sits at, `8 * PORT_SPACING = 128px`), doubled for the
/// symmetric grow: `2 * (128 + 8) = 272px`. Wider than a naïve "eight ports, `(9-1)*16 + 2*8`"
/// count would suggest, because that count implicitly assumes an even split across both sides —
/// item 12 does not force one when the geometry genuinely is not.
#[test]
fn orthogonal_growth_widens_a_node_whose_face_cannot_fit_its_ports() {
    let src = "flowchart TD\n  A --> Z\n  B --> Z\n  C --> Z\n  D --> Z\n  E --> Z\n  \
               F --> Z\n  G --> Z\n  H --> Z\n  I --> Z";

    let splines = laid_out_curve(src, "basis");
    let z_splines = splines.node("Z").expect("Z must exist");
    assert_eq!(
        z_splines.size.h, 45.400000000000006,
        "splines must not grow Z at all — dumped once and pinned"
    );

    let ortho = laid_out_flow(src, "basis", "konoma-orthogonal");
    let z_ortho = ortho.node("Z").expect("Z must exist");
    // §10-3 items 3 and 12 (this test's own doc): all nine edges land on Z's Top face, and `A`
    // (dagre's own leftmost pick, not the fan's centre) anchors at offset 0 with all eight
    // remaining claims genuinely on its right — `2 * (8 * PORT_SPACING + PORT_CLEARANCE) = 272px`.
    assert_eq!(
        z_ortho.size.w, 272.0,
        "Z's width must grow to exactly 2 * (8*PORT_SPACING + PORT_CLEARANCE) = 272px — item 12's \
         own symmetric grow around a genuinely one-sided fan"
    );
    // Height is untouched: nothing asked Z's Left/Right faces to grow at all any more.
    assert_eq!(
        z_ortho.size.h, z_splines.size.h,
        "no face asked Z's height to grow, so it must not have"
    );

    let (z_left, z_top, _, _) = z_ortho.bounds();
    let top_face_y = z_top - orthogonal::PORT_INSET;

    let mut top_xs = Vec::new();
    let mut aligned_count = 0;
    for e in &ortho.edges {
        if e.to != "Z" {
            continue;
        }
        let bends = e.points.len().saturating_sub(2);
        let last = e.points.last().unwrap();
        assert!(
            (last.y - top_face_y).abs() < 1e-6,
            "every edge into Z must land on its Top face now: {e:?}"
        );
        if bends == 0 {
            assert_eq!(
                e.from, "A",
                "A is the smallest-cross-coordinate source, so it wins Z's one aligned slot: {e:?}"
            );
            aligned_count += 1;
        } else {
            // Not pinned to exactly 2 any more (this assertion's own prior wording, "an ordinary
            // merge onto Z's Top face is... a two-bend shape"): nine single-width nodes sit in one
            // dense row above Z, each claiming its own 16px port along that row, and every port
            // *except* the two whose ordinary bend already lands below the row entirely (`B`, `G`)
            // sits directly under a *different* sibling than its own source — `C`'s own port sits
            // under `A`, `D`'s straight sweep passes clean through `B`'s box on the way to a gap
            // between `A` and `B`, and so on. A plain two-bend route from many of these sources
            // would run straight through that sibling's box; nothing before this test's own local
            // clearance fix (`clear_local_route`/`local_detour`, `docs/FEATURE-MERMAID-RENDERER.md`
            // §10-5) ever checked this fixture for crossings, so the exact "always 2" count this
            // assertion used to pin was never actually verified safe — dumping the real geometry
            // (`docs/STATUS.md`'s own ★未修正 history on `zz-design-2c`, the bug this fix targets)
            // shows most of `C`..`I` need a genuine extra jog around whichever sibling their own
            // straight run would otherwise cross. What still has to hold, regardless of how many
            // extra bends a crossing needs: an even number (every detour is a there-and-back pair)
            // and, the property that actually matters, no crossing at all.
            assert_eq!(
                bends % 2,
                0,
                "every bend count must be even (paired turns): {e:?}"
            );
            for w in e.points.windows(2) {
                for n in &ortho.nodes {
                    if n.id == e.from || n.id == e.to {
                        continue;
                    }
                    assert!(
                        !orthogonal::segment_crosses_node(&w[0], &w[1], n),
                        "{}->Z segment {:?}-{:?} must not cross {}: {e:?}",
                        e.from,
                        w[0],
                        w[1],
                        n.id
                    );
                }
            }
        }
        top_xs.push(last.x);
    }
    assert_eq!(
        aligned_count, 1,
        "exactly one edge must be the aligned A->Z"
    );
    assert_eq!(top_xs.len(), 9, "all nine edges must share Z's Top face");

    top_xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // 16px apart, but — §10-3 item 12 — one-sided, not symmetric about Z's own centre: `A`'s own
    // aligned claim (offset 0) sits *at* the centre, and every one of `B`..`I` genuinely sits to
    // its right, so the whole nine-port run occupies only the right half of the grown face.
    for w in top_xs.windows(2) {
        assert!(
            (w[1] - w[0] - orthogonal::PORT_SPACING).abs() < 1e-6,
            "ports on one face must be exactly 16px apart: {top_xs:?}"
        );
    }
    assert!(
        (top_xs[0] - z_ortho.center.x).abs() < 1e-6,
        "the aligned A->Z takes the exact centre slot, the run's own leftmost port: {top_xs:?} vs {}",
        z_ortho.center.x
    );
    assert!(
        (top_xs[8] - (z_ortho.center.x + 8.0 * orthogonal::PORT_SPACING)).abs() < 1e-6,
        "the run's own rightmost port sits 8*PORT_SPACING right of centre: {top_xs:?} vs {}",
        z_ortho.center.x
    );
    // The box itself still grows symmetrically about the centre (item 12's own "中心対称に拡大") —
    // clearance on the *unused* left side of the face matches the busy right side's own reach.
    let half_w = z_ortho.size.w / 2.0;
    for &x in top_xs.iter() {
        let from_center = (x - z_ortho.center.x).abs();
        assert!(
            half_w - from_center >= orthogonal::PORT_CLEARANCE - 1e-6,
            "port {from_center}px from centre must clear the corner by \
             {}px (half-width {half_w}): {top_xs:?}",
            orthogonal::PORT_CLEARANCE
        );
    }
    // And every port stays within Z's own (grown) flat run, never off past its left edge.
    assert!(
        top_xs
            .iter()
            .all(|&x| x >= z_left && x <= z_left + z_ortho.size.w),
        "every port must sit within Z's own grown width: {top_xs:?} vs [{z_left}, {}]",
        z_left + z_ortho.size.w
    );
}

/// A three-way merge whose own claims genuinely need more room than `Z`'s natural width — §10-3
/// items 3 and 12 (`docs/FEATURE-MERMAID-RENDERER.md`): `A --> Z` is aligned (`A.center.x ==
/// Z.center.x`, dumped) and lands on `Z`'s Top face at offset 0; `B` and `C` both genuinely sit to
/// `Z`'s right (`B.center.x = 141.3`, `C.center.x = 235.1`, both past `Z.center.x = 48.0`), so
/// item 12 puts both non-aligned claims on the same (right) side rather than splitting them across
/// the centre — `+16`/`+32`. `required_flat = 2 * (2*PORT_SPACING + PORT_CLEARANCE) = 80px`, wider
/// than `Z`'s own natural (splines) width of ~68.55px, so `Z` must grow. Before item 12's fix this
/// fixture's own name ("a face with room to spare must not grow") held — the old array-symmetric
/// grid split the two non-aligned claims onto opposite sides of `Z`'s centre (`-16`/`+16`), whose
/// narrower `max_abs_offset = 16` fit inside 68.55px without growing at all; that was the
/// side-crossing bug (§10-3 item 12's own "同じ辺にn本つく…下から来る辺が中心より上のポートへ回り込
/// む形は禁止"), not a genuinely roomy face.
#[test]
fn orthogonal_one_sided_merge_grows_to_fit_both_siblings_on_the_same_side() {
    let src = "flowchart TD\n  A --> Z\n  B --> Z\n  C --> Z";
    let splines = laid_out_curve(src, "basis");
    let ortho = laid_out_flow(src, "basis", "konoma-orthogonal");
    let z_splines = splines.node("Z").expect("Z must exist");
    let z_ortho = ortho.node("Z").expect("Z must exist");
    assert_eq!(
        z_ortho.size.w, 80.0,
        "B and C both genuinely sit right of Z, so Z must grow to 2*(2*PORT_SPACING+PORT_CLEARANCE) \
         = 80px to fit both on the same side: {:?} vs splines' natural {:?}",
        z_ortho.size, z_splines.size
    );
    assert_eq!(
        z_ortho.size.h, z_splines.size.h,
        "no face asked Z's height to grow, so it must not have"
    );
}

/// Across the capped corpus ([`orthogonal_dag_corpus`]), no orthogonal-routed edge ever exceeds
/// two bends — §10-1 item 1's "ポート分配で端点がずれた辺は曲げ2回まで許す" is a *cap* on the
/// aligned/branch/merge shapes, and this is the invariant stage 1's own bend-count tests could not
/// state, because eviction (and the growth it can trigger) did not exist yet: an aligned edge
/// whose two ends land on different eviction offsets needs exactly the second bend this rule
/// allows for. A back edge and a collision-fallback edge are both deliberately excluded from this
/// cap — see [`orthogonal_dag_corpus`]'s own doc — and both were caught by this test being run
/// over the wrong corpus before either exclusion existed: `branch`'s `D --> B` cycle came back
/// with 4 bends on the first run (correctly — §10-1 item 2 puts no limit on a back edge at all),
/// and stage 3's collision fix later did the same to `amp-chain`'s `A --> D`.
#[test]
fn orthogonal_bend_count_never_exceeds_two() {
    for (name, src) in orthogonal_dag_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        for e in &d.edges {
            let bends = e.points.len().saturating_sub(2);
            assert!(
                bends <= 2,
                "{name}: edge {}->{} has {bends} bends, more than the rule 4 cap: {:?}",
                e.from,
                e.to,
                e.points
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Stage 3: lane alignment and node-collision avoidance (docs/FEATURE-MERMAID-RENDERER.md §10-1
// item 2, "レーン揃え", plus item 1's collision fix)
// ---------------------------------------------------------------------------------------------

/// Every routed segment of every edge in `d`, other than the two the edge itself owns, must clear
/// every node's box — the "強い不変条件" stage 3's collision fix exists to guarantee. Asserts
/// through `orthogonal::segment_crosses_node`, the exact box test `classify` itself runs, so this
/// states the same question about the *finished* diagram that the routing decision already asked
/// about a candidate shape.
fn assert_no_segment_crosses_a_foreign_node(name: &str, d: &Diagram) {
    for e in &d.edges {
        for w in e.points.windows(2) {
            for n in &d.nodes {
                if n.id == e.from || n.id == e.to {
                    continue;
                }
                assert!(
                    !orthogonal::segment_crosses_node(&w[0], &w[1], n),
                    "{name}: edge {}->{} segment {:?}->{:?} crosses {} {:?}",
                    e.from,
                    e.to,
                    w[0],
                    w[1],
                    n.id,
                    n.bounds()
                );
            }
        }
    }
}

/// §10-1 item 1's collision fix, run over the **whole** corpus ([`orthogonal_corpus`]) — stage 3's
/// headline invariant, and stage 5's too: a back edge and `amp-chain`'s collision-fallback edge
/// used to be exempt here (see [`orthogonal_dag_corpus`]'s own doc for why), because stages 1-4's
/// back-edge route was still dagre's own waypoint chain, straightened, with no collision check of
/// its own. Stage 5's [`orthogonal::route_perimeter`] replaces that stopgap with a real
/// collision-avoiding route (`orthogonal::safe_ring_exit`), so the exemption no longer applies —
/// this now runs the invariant over every corpus source, self-loop included.
/// `orthogonal_dag_corpus` stays in use for [`orthogonal_bend_count_never_exceeds_two`], a
/// genuinely different invariant a perimeter or self-loop route is still exempt from by design
/// (many bends is the point of going around the outside).
#[test]
fn orthogonal_no_segment_crosses_a_foreign_node_across_the_whole_corpus() {
    for (name, src) in orthogonal_full_corpus() {
        // `zz-design-2c`'s own `API -> ID` used to be exempted here (§10-4's audit had named this
        // bug class and left it open — `docs/FEATURE-MERMAID-RENDERER.md` §10-4's own "一般化
        // チェックの結果"): a `staircase` edge whose raw dagre waypoints ran straight through the
        // sibling node `Q`, then `メタデータ DB`/`成果物保管`/`解析サンドボックス`, because
        // `clear_local_route`'s own remediation slid the *entire* straight run to one shared
        // coordinate — a single degree of freedom that cannot satisfy two obstacles at different
        // points along the run each demanding a different clearance (`Q`'s row only clears to its
        // left; the row below only clears to the right of where that pushed it), so the fix
        // oscillated between the two colliding coordinates every pass and silently gave up once
        // its budget ran out, still crossing `Q`. `local_detour` (`orthogonal.rs`) replaces the
        // whole-run slide with a local jog around each obstacle's own span, independent of every
        // other obstacle sharing the run — the exemption is gone; this now runs the invariant over
        // `zz-design-2c` too.
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        assert_no_segment_crosses_a_foreign_node(name, &d);
    }
}

/// The real diagram `docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 1 names as this stage's
/// motivating case: `samples/mermaid.ja.md`'s big flowchart, where `MD`（ブロックモデル）'s branch
/// to `MM`（mermaid）used to sweep straight through `NA`（プレビュー不可）'s box — both sitting in
/// `MD`'s own rank, directly in the path a branch's cross-axis sweep runs along. Confirms the
/// concrete bug is gone, not just the general mechanism: `MD`'s own two edges (`MD->MM`,
/// `MD->MA`) must clear `NA`, and the whole diagram must still hold the strong invariant.
#[test]
fn orthogonal_settings_rules_sample_no_longer_pierces_a_sibling_node() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");

    let na = d.node("NA").expect("NA must exist");
    for id in ["MM", "MA"] {
        let e = d
            .edges
            .iter()
            .find(|e| e.from == "MD" && e.to == *id)
            .unwrap_or_else(|| panic!("MD->{id} must exist"));
        for w in e.points.windows(2) {
            assert!(
                !orthogonal::segment_crosses_node(&w[0], &w[1], na),
                "MD->{id} must no longer pierce NA: segment {:?}->{:?}, NA bounds {:?}",
                w[0],
                w[1],
                na.bounds()
            );
        }
    }

    assert_no_segment_crosses_a_foreign_node("settings-rules", &d);
    // Neither of the two invariants below was ever run against this real diagram before — only
    // against `CORPUS`'s hand-built fixtures (`assert_endpoints_sit_outside_and_perpendicular`'s
    // own doc explains why that gap let a real regression through) — so both are pinned here too,
    // alongside the foreign-node check this test already ran.
    assert_endpoints_sit_outside_and_perpendicular("settings-rules", &d);
    assert_no_edge_crosses_its_own_endpoint("settings-rules", &d);
}

/// The coordinator's own third real-pixel finding on this exact diagram (2026-09-01, user report:
/// the mermaid node's own arrow tip was invisible, and two lines near it were impossible to trace to
/// where they connected): `MD->MM`'s raw-derived staircase route ran ~38px down into `MM`'s own
/// interior before reaching its (correctly placed) bottom port — invisible in the finished picture
/// only because `svg::emit` paints nodes after edges, the same masking `orthogonal_arrow_tip_has_a_
/// visible_gap_from_the_node_in_real_pixels`'s own doc describes for the inward-port bug — and
/// `MM->RS` left `MM`'s bottom port only to double straight back up through `MM`'s own box before
/// turning toward `RS`. Both are the *same* underlying staleness (`route_with_ports`'s own doc):
/// `align_straight_lanes` moved `MM` after dagre's raw waypoint chain was captured, and — unlike a
/// self-loop's `raw`, which `shift_cross` keeps in sync — nothing reprojected a forward staircase
/// edge's interior chain onto `MM`'s new position.
///
/// Pinned two ways: first, `MD->MM`'s own two-edge check the module doc's original NA-piercing case
/// used, generalised from `segment_crosses_node` (a foreign-node question) to
/// `staircase_punctures_its_own_endpoint` (an own-node one); second, calling both whole-diagram
/// invariants — this is what would have failed before the fix.
#[test]
fn orthogonal_settings_rules_sample_mermaid_node_edges_do_not_pierce_it() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");

    let mm = d.node("MM").expect("MM must exist");
    for (from, to) in [("MD", "MM"), ("MM", "RS")] {
        let e = d
            .edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap_or_else(|| panic!("{from}->{to} must exist"));
        assert!(
            !orthogonal::staircase_punctures_its_own_endpoint(&e.points, Some(mm), Some(mm)),
            "{from}->{to} must not re-enter MM's own interior: {:?}, MM bounds {:?}",
            e.points,
            mm.bounds()
        );
    }

    assert_endpoints_sit_outside_and_perpendicular("settings-rules", &d);
    assert_no_edge_crosses_its_own_endpoint("settings-rules", &d);
}

/// The coordinator's own second real-pixel finding on this exact diagram (2026-09-01, after
/// `align_straight_lanes`'s adjacency fix started actually moving nodes): once branch/merge
/// collisions against those relocated nodes became more common, every collision-fallback forward
/// edge here rode `route_perimeter` (stage 5's over-broad `shape.reverse || shape.staircase`
/// condition, reverted the same day — `route_with_ports`'s own doc) out to the diagram's outer
/// edge — `設定のルール->デコード`（`C->IM`, the "画像" label) along the very top, `ブロックモデル
/// ->mermaid`/`->数式`（`MD->MM`/`MD->MA`) out past the left edge and back, three parallel outer
/// runs stacked in the bottom-left corner. This diagram is a pure DAG (no cycle anywhere in its
/// source), so **every** edge in it is a forward edge — none of them has any business excursing by
/// a perimeter-lane amount at all, `PERIMETER_MARGIN` or more past the content box. Real output,
/// dumped and confirmed before this was written: `C->IM`'s own worst excursion is ~8.5px (clears
/// the row above it, nothing like a full perimeter loop), and `MD->MM`/`MD->MA` never leave `MD`'s
/// own local column at all — see `docs/STATUS.md` and this file's own git history for the numbers
/// from the regression itself.
#[test]
fn orthogonal_settings_rules_sample_has_no_perimeter_routed_forward_edges() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let bounds = diagram_content_bounds(&d);
    for e in &d.edges {
        let max_excursion = e
            .points
            .iter()
            .map(|p| excursion(bounds, p))
            .fold(0.0_f64, f64::max);
        assert!(
            max_excursion < orthogonal::PERIMETER_MARGIN,
            "{}->{}: excursion {max_excursion}px reaches perimeter-lane territory \
             (PERIMETER_MARGIN={}) in a diagram with no back edge at all: {:?}",
            e.from,
            e.to,
            orthogonal::PERIMETER_MARGIN,
            e.points
        );
    }
}

/// §10-3's "ファン列内の並び順" and "幹7区間の曲げ0" (`docs/FEATURE-MERMAID-RENDERER.md`,
/// `mod.rs`'s own `regroup_fan_lanes` doc), pinned against the real source the rule was reverse-
/// engineered from — `samples/mermaid.ja.md`'s 20-node "大きさ" fence (`SETTINGS_RULES_SAMPLE`),
/// laid out exactly as `docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`'s `3a`
/// section hand-authored it.
///
/// **Column order** — `設定のルール`'s ten-way fanout (`C`'s own children) matches `3a`'s own
/// top-to-bottom order *exactly*, all ten positions: the two `txt`-class members with no forward
/// edge of their own (`窓読み`/`T`, `構文強調`/`S`) lead, then `表`/`TB` and `一覧`/`AR` (same
/// class, "同色内は安定" keeping them in their declared order); `pix`-class `ページ描画`/`PD` and
/// `usvg`/`SV` sit closest to the trunk on either side (both converge one rank sooner, into
/// `ラスタライズ`, than `デコード`/`IM` and `キーフレーム`/`VD`, which converge into `セルに合わせ
/// る` a rank later — the "nearest downstream rank sits closer to the spine" tie-break this
/// module's own `regroup_fan_lanes` doc explains, needed here because `IM` is declared *before*
/// `PD` yet sits *farther* from the trunk); `MD`(`ブロックモデル`) — the trunk, the one `txt`
/// member that itself continues on to `mermaid`/`数式` — sits at the group's own centre index
/// (`10 / 2 = 5`); and `プレビュー不可`/`NA`, the one child with no `classDef`-backed class at all,
/// is last regardless of its own declared position.
///
/// **Seven-segment spine** — every rank-to-rank hop of `ファイル → 設定のルール → ブロックモデル →
/// mermaid → ラスタライズ → セルに合わせる → 端末 → 画像プロトコル` draws as `classify`'s `aligned`
/// shape, a straight 2-point line with no bend at all: `F->C` and `RS->FIT`/`FIT->K` were already
/// achievable before this session's work (single- or already-uncrowded faces), but `C->MD`,
/// `MD->MM`, `MM->RS`, and `K->RI` were not — each sits on an even-count face (`C`'s ten-way,
/// `MD`'s two-way, `MM`'s one-way-but-competing, `K`'s three-way) that `evict`'s pre-fix symmetric
/// port grid could never place exactly on the face's own centre (this module's own `orthogonal_
/// branch_and_merge_edges_share_a_face_one_aligned_one_bends_twice` is the general-purpose
/// regression for the same `evict` fix, on a minimal two-claim fixture).
#[test]
fn orthogonal_settings_rules_sample_fan_column_matches_3a_and_spine_is_all_zero_bend() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    assert_no_edge_crosses_its_own_endpoint("settings-rules-sample", &d);
    check_nodes_do_not_overlap("settings-rules-sample", &d);

    let mut fan: Vec<(&str, f64)> = d
        .edges
        .iter()
        .filter(|e| e.from == "C")
        .map(|e| (e.to.as_str(), d.node(&e.to).unwrap().center.y))
        .collect();
    fan.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let order: Vec<&str> = fan.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        order,
        vec!["T", "S", "TB", "AR", "PD", "MD", "SV", "IM", "VD", "NA"],
        "C's own ten-way fanout, top to bottom, matching 3a exactly: {order:?}"
    );

    for (from, to) in [
        ("F", "C"),
        ("C", "MD"),
        ("MD", "MM"),
        ("MM", "RS"),
        ("RS", "FIT"),
        ("FIT", "K"),
        ("K", "RI"),
    ] {
        let e = d
            .edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap_or_else(|| panic!("{from}->{to} must exist"));
        assert_eq!(
            e.points.len(),
            2,
            "{from}->{to} is a spine segment — a straight 2-point line: {:?}",
            e.points
        );
        assert!(
            (e.points[0].y - e.points[1].y).abs() < 1e-6,
            "{from}->{to} is horizontal (LR flow axis), no cross-axis drift: {:?}",
            e.points
        );
    }
}

/// The coordinator's own repro for the real bug `safe_ring_exit`'s L-shaped fallback used to have
/// (`orthogonal::tests::safe_ring_exit_l_shape_stops_at_the_ring_not_the_far_corner`'s own doc has
/// the full story): a decision node's retry loop (`C -->|再試行| B`) needed the L-shaped fallback
/// at both ends, on the same ring side, and the old third leg drew the return edge the *whole
/// height of the ring* instead of the short local jog its own two ports need. Not part of `CORPUS`
/// (adding it there would move the golden — `docs/FEATURE-MERMAID-RENDERER.md`'s own "ゴールデン
/// 13 本不変" rule), so this is a standalone regression test in the same shape
/// `orthogonal_settings_rules_sample_no_longer_pierces_a_sibling_node` already is.
#[test]
fn orthogonal_decision_retry_loop_does_not_span_the_whole_ring() {
    let src = "flowchart TB\n  A[入力] --> B[整形]\n  B --> C{検査}\n  C -->|合格| D[出力]\n  \
               C -->|再試行| B\n  D -.-> A";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");

    for (from, to) in [("C", "B"), ("D", "A")] {
        let e = d
            .edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap_or_else(|| panic!("{from}->{to} must exist"));
        let longest = e
            .points
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
            .fold(0.0_f64, f64::max);
        // An upper bound on what the two ports genuinely need: the two nodes' own vertical
        // separation, generous enough not to depend on exactly which face eviction picked, but
        // nowhere near the ring's own height (the bug's own symptom: `C->B`'s return edge drew
        // ~394px of ring — most of the diagram — for a ~46px stub before this was fixed).
        let a = d.node(from).expect("source node must exist");
        let b = d.node(to).expect("target node must exist");
        let needed = (a.center.y - b.center.y).abs();
        assert!(
            longest <= needed + 1.0,
            "{from}->{to}: longest segment is {longest}px, more than the ~{needed}px the two \
             ports actually need — the ring-spanning bug is back: {:?}",
            e.points
        );
    }
}

/// Every rank still holds no overlapping node after lane alignment moved some of them — §10-1
/// item 1's "整列でノードを動かした結果…重なった側を押し出して従来の最小間隔を維持する". Checked
/// with the exact same box-overlap assertion [`invariant_nodes_do_not_overlap`] states for the
/// splines path, now over the whole corpus under `"konoma-orthogonal"`.
#[test]
fn orthogonal_lane_alignment_never_leaves_nodes_overlapping() {
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        check_nodes_do_not_overlap(name, &d);
    }
}

/// §10-1 item 2's "直進辺を挟んで上下（左右）に分かれる辺は…曲げ位置を共有して対称に", read
/// through §10-3 item 3's own correction (`classify`'s `merge_target_side` doc): a merge target
/// now always rides the same flow-axis face a branch target already did, so `B->D`'s aligned
/// (0-bend) shape and `C->D`'s ordinary merge now compete for the exact same face on `D` — `D`'s
/// Top, not two different faces the way they did before the correction (aligned always used the
/// flow axis; the round-2 merge shape used the cross axis, so the two never used to overlap at
/// all).
///
/// [`orthogonal::evict`]'s own doc on `anchor` (§10-3's "ファン列内の並び順"/"幹7区間の曲げ0" work)
/// is what decides the split here: two claims on `D`'s Top face (`B->D`, aligned, and `C->D`, not).
/// `B->D`'s own natural sorted position already coincides with the face's own centre slot (its
/// other end, `B`, sits almost exactly on `D`'s own x — dumped: `B.center.x = 43.055`,
/// `D.center.x = 43.055`), so `B->D` is a genuine straight, absorbed lane: a 2-point line entering
/// dead on `D`'s own centre. `C->D`, its face-mate, is pushed a full `PORT_SPACING = 16px` off
/// it — §10-3 item 12's own "面のポートは相手の側で配る" decides *which* side: `C` sits well to `D`'s
/// own right (dumped: `C.center.x = 89.917`, past `D.center.x`), so `C->D`'s port belongs to the
/// right of `D`'s centre, not the left (a pre-item-12 `evict` used to force the aligned claim into
/// the claims array's own geometric middle index regardless of its own natural sort position, which
/// could drag `C->D` to the wrong side of centre purely as a side effect of that re-splice — the
/// exact same mechanism item 12 fixed on the real `セルに合わせる` merge, `docs/STATUS.md`'s own
/// ★未修正 entry). Still bends twice either way. Dumped and confirmed by hand (`B`'s and `C`'s own
/// x, and both edges' full point lists), not assumed.
#[test]
fn orthogonal_shared_merge_target_face_one_aligned_one_bends_twice() {
    let src = "flowchart TD\n  A ---> B\n  A --> C\n  B --> D\n  C --> D";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let bd = d
        .edges
        .iter()
        .find(|e| e.from == "B" && e.to == "D")
        .expect("B->D must exist");
    let cd = d
        .edges
        .iter()
        .find(|e| e.from == "C" && e.to == "D")
        .expect("C->D must exist");
    let d_node = d.node("D").expect("D must exist");

    // B->D is the aligned leg — a straight 2-point line; C->D still bends twice, since it loses
    // the face's one straight lane to B->D.
    assert_eq!(
        bd.points.len(),
        2,
        "B->D is the aligned leg — a straight 2-point line: {:?}",
        bd.points
    );
    assert_eq!(cd.points.len(), 4, "C->D must bend twice: {:?}", cd.points);

    // B->D enters dead on D's own centre; C->D is pushed a full PORT_SPACING off it, to the
    // *right* (C genuinely sits to D's right — see this test's own doc).
    let bd_entry_x = bd.points.last().unwrap().x;
    let cd_entry_x = cd.points.last().unwrap().x;
    assert!(
        (bd_entry_x - d_node.center.x).abs() < 1e-6,
        "B->D (aligned) enters exactly on D's centre: {bd_entry_x} vs {}",
        d_node.center.x
    );
    assert!(
        (cd_entry_x - (d_node.center.x + orthogonal::PORT_SPACING)).abs() < 1e-6,
        "C->D enters PORT_SPACING right of D's centre (C genuinely sits right of D): {cd_entry_x} \
         vs {}",
        d_node.center.x
    );

    // C->D's own bend still lands where it did before this fix — `A ---> B` is authored with an
    // extra dash (`SpecEdge::minlen` 2) while `A --> C` is the ordinary `minlen` 1, so under §10-3
    // item 8's own rank fix (`pull_back_fan_ranks`, `mod.rs`) `B` genuinely sits one rank further
    // from `A` than `C` does, and `C->D` spans the extra rank `B` occupies (`rank_lane_gap_bend`,
    // §10-3 item 4's own column-gap routing for an edge that skips a populated rank). `B->D` no
    // longer has a "bend row" to check at all — it is a straight line now (the assertion above
    // already pins its only two points). Dumped and confirmed by hand, not assumed.
    let cd_expected_row = 274.2;
    assert!(
        (cd.points[1].y - cd_expected_row).abs() < 1e-6,
        "C->D's bend row: {:?} vs {cd_expected_row}",
        cd.points
    );
}

/// §10-3 item 10's own two invariants (`docs/FEATURE-MERMAID-RENDERER.md` — "合流側はファン根元の
/// 鏡像"), pinned over `SETTINGS_RULES_SAMPLE`'s own `ラスタライズ`(`RS`): a real four-way merge
/// (`mermaid`/`数式`/`usvg`/`ページ描画` → `ラスタライズ`) from four simple, single-out-edge
/// sources — the exact real-world case the rule was fixed against (`usvg`'s own row is blocked by
/// `数式`'s sibling node, `MA`, forcing the `rank_lane_bend` best-effort path this fix added).
///
/// 1. **No top/bottom entry**: every edge into a multi-way merge target enters through the flow
///    axis face (`Left`, under `LR`) — never `Top`/`Bottom`, which is what the pre-fix `alt`
///    (cross-axis) fallback used to draw and what made the source ambiguous
///    ("面が曖昧" — `docs/STATUS.md`'s own ★未修正 entry this test closes).
/// 2. **No sibling crossing**: no two of those four edges' own segments cross each other —
///    the merge-side counterpart of [`orthogonal_settings_rules_sample_fan_lane_siblings_never_
///    cross`]'s fan-root claim, now for edges converging on a shared target rather than diverging
///    from a shared source.
#[test]
fn orthogonal_settings_rules_sample_merge_target_entries_are_flow_axis_and_never_cross() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let rs = d.node("RS").expect("RS (ラスタライズ) must exist");
    let (l, t, r, bo) = rs.bounds();

    let merge: Vec<&PlacedEdge> = d.edges.iter().filter(|e| e.to == "RS").collect();
    assert_eq!(
        merge.len(),
        4,
        "RS's own four-way merge (mermaid/数式/usvg/ページ描画): {:?}",
        merge.iter().map(|e| e.from.as_str()).collect::<Vec<_>>()
    );

    for e in &merge {
        let entry = e.points.last().expect("every edge has at least one point");
        let on_left_or_right_face = (entry.x - (l - orthogonal::PORT_INSET)).abs() < 1e-6
            || (entry.x - (r + orthogonal::PORT_INSET)).abs() < 1e-6;
        let on_top_or_bottom_face = (entry.y - (t - orthogonal::PORT_INSET)).abs() < 1e-6
            || (entry.y - (bo + orthogonal::PORT_INSET)).abs() < 1e-6;
        assert!(
            on_left_or_right_face && !on_top_or_bottom_face,
            "{}->RS must enter RS's own flow-axis (Left) face, never Top/Bottom: entry={entry:?}, \
             RS bounds=({l},{t},{r},{bo})",
            e.from
        );
        assert!(
            entry.y > t + 1e-6 && entry.y < bo - 1e-6,
            "{}->RS's own entry point must sit strictly inside RS's own vertical span, not flush \
             on a corner (§10-1 item 1's own 'ポートは角から8px以上'): entry={entry:?}, RS \
             bounds=({l},{t},{r},{bo})",
            e.from
        );
    }

    for i in 0..merge.len() {
        for j in (i + 1)..merge.len() {
            let (a, b) = (merge[i], merge[j]);
            for wa in a.points.windows(2) {
                for wb in b.points.windows(2) {
                    assert!(
                        orthogonal::segment_crossing(wa, wb).is_none(),
                        "{}->RS crosses {}->RS's own segment ({:?}-{:?} vs {:?}-{:?}) — sibling \
                         merge stubs into the same target must never cross: {} points={:?}, \
                         {} points={:?}",
                        a.from,
                        b.from,
                        wa[0],
                        wa[1],
                        wb[0],
                        wb[1],
                        a.from,
                        a.points,
                        b.from,
                        b.points
                    );
                }
            }
        }
    }
}

/// §10-3 item 12's own real motivating bug (`docs/FEATURE-MERMAID-RENDERER.md`, user report
/// "セルに合わせるの3合流でポートが中心より上へ回り込む"): `FIT`(`セルに合わせる`)'s own three-way
/// merge (`RS`(`ラスタライズ`, aligned — the trunk, offset 0), `IM`(`デコード`) and
/// `VD`(`キーフレーム`), both genuinely below `FIT`'s own centre in this `LR` layout). Before item
/// 12's fix, `evict`'s own array-symmetric re-splice moved the aligned claim from wherever it
/// naturally sorted to the claims array's own geometric middle index, which could drag a
/// same-side sibling across the centre line along the way — on this exact face, `RS` naturally
/// sorts *first* (its own other end, `MM`/`mermaid`, sits almost exactly on `FIT`'s own centre) and
/// `round((3-1)/2) = 1` used to force it to index 1, dragging `IM` down to index 0 and flipping its
/// offset from the `+16` its true side calls for to `-16` — landing it *above* `FIT`'s centre even
/// though `IM` genuinely sits below it, exactly the reported symptom. `3a`'s own reference geometry
/// (`docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`) confirms both non-trunk
/// members land below centre (`338`/`354` against a centre of `322`), matching this test's own
/// `+16`/`+32`.
#[test]
fn orthogonal_settings_rules_sample_fit_merge_keeps_same_side_siblings_on_the_same_side() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let fit = d.node("FIT").expect("FIT (セルに合わせる) must exist");

    let rs = d
        .edges
        .iter()
        .find(|e| e.from == "RS" && e.to == "FIT")
        .expect("RS->FIT (幹) must exist");
    let im = d
        .edges
        .iter()
        .find(|e| e.from == "IM" && e.to == "FIT")
        .expect("IM->FIT (デコード) must exist");
    let vd = d
        .edges
        .iter()
        .find(|e| e.from == "VD" && e.to == "FIT")
        .expect("VD->FIT (キーフレーム) must exist");

    assert_eq!(
        rs.points.len(),
        2,
        "RS->FIT is the trunk — a straight 2-point line entering exactly on FIT's own centre: {:?}",
        rs.points
    );
    let rs_entry = rs.points.last().unwrap().y;
    assert!(
        (rs_entry - fit.center.y).abs() < 1e-6,
        "RS->FIT (aligned) enters exactly on FIT's centre: {rs_entry} vs {}",
        fit.center.y
    );

    let im_entry = im.points.last().unwrap().y;
    let vd_entry = vd.points.last().unwrap().y;
    assert!(
        (im_entry - (fit.center.y + orthogonal::PORT_SPACING)).abs() < 1e-6,
        "IM->FIT (デコード) enters PORT_SPACING BELOW FIT's centre — never above it, the reported \
         side-crossing bug: {im_entry} vs {}",
        fit.center.y
    );
    assert!(
        (vd_entry - (fit.center.y + 2.0 * orthogonal::PORT_SPACING)).abs() < 1e-6,
        "VD->FIT (キーフレーム) enters 2*PORT_SPACING below FIT's centre, on the SAME side as \
         IM->FIT (both genuinely below centre): {vd_entry} vs {}",
        fit.center.y
    );

    // FIT's own height grows symmetric to the *larger* side (item 12's own "ノードは大きい側に
    // 合わせて中心対称に拡大") — 2 steps (IM, VD) reserved on both sides even though the near side
    // carries no claim at all: 2*(2*PORT_SPACING + PORT_CLEARANCE) = 80px, matching `3a`'s own
    // 高さ80 reference.
    let expected_h = 2.0 * (2.0 * orthogonal::PORT_SPACING + orthogonal::PORT_CLEARANCE);
    assert!(
        (fit.size.h - expected_h).abs() < 1e-6,
        "FIT's own height must be exactly 2*(2*PORT_SPACING+PORT_CLEARANCE) = {expected_h}px: got \
         {}",
        fit.size.h
    );
}

/// §10-3 item 13's own real motivating bug (`docs/FEATURE-MERMAID-RENDERER.md`, user report "線が
/// ページ描画のノードから出ていない・右上の角から生えて見える"): `PD`(`ページ描画`)'s own single
/// out-edge to `RS`(`ラスタライズ`) used to have its exit stub dragged off `PD`'s own assigned Right
/// face slot by `MA`(`数式`)'s box sitting in the same pass-through row — `clear_local_route`'s own
/// blind "every point sharing the flagged coordinate slides together" moved the port along with the
/// rest of the run. Pinned two ways: the port sits exactly on `PD`'s own vertical centre (its one
/// and only claim on that face, so `evict` gives it offset 0), and the route genuinely still clears
/// `MA`'s box (the bug this fix must not silently regress into "port stays put, line now crosses
/// 数式 instead").
#[test]
fn orthogonal_settings_rules_sample_page_render_exit_stays_on_its_own_port() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let pd = d.node("PD").expect("PD (ページ描画) must exist");
    let ma = d.node("MA").expect("MA (数式) must exist");
    let pd_rs = d
        .edges
        .iter()
        .find(|e| e.from == "PD" && e.to == "RS")
        .expect("PD->RS must exist");

    let exit = pd_rs.points.first().expect("PD->RS must have points");
    let (_, t, r, bo) = pd.bounds();
    assert!(
        (exit.x - (r + orthogonal::PORT_INSET)).abs() < 1e-6,
        "PD->RS must leave PD's own Right face, PORT_INSET outside it, not some other x: \
         {exit:?} vs PD bounds {:?}",
        pd.bounds()
    );
    assert!(
        (exit.y - (t + bo) / 2.0).abs() < 1e-6,
        "PD->RS's own exit port must sit exactly on PD's own vertical centre (PD's only claim on \
         this face): {exit:?} vs PD bounds {:?}",
        pd.bounds()
    );
    let second = &pd_rs.points[1];
    assert!(
        (exit.y - second.y).abs() < 1e-6 && (exit.x - second.x).abs() > 1e-6,
        "PD must still leave its own Right face horizontally (perpendicular exit, §10-1 item 1), \
         not turn immediately at the port: {:?}",
        pd_rs.points
    );

    for w in pd_rs.points.windows(2) {
        assert!(
            !orthogonal::segment_crosses_node(&w[0], &w[1], ma),
            "PD->RS must still clear MA (数式) even with its own port pinned: segment {w:?} in \
             {:?}, MA bounds {:?}",
            pd_rs.points,
            ma.bounds()
        );
    }
}

/// A minimal, hand-built counterpart to [`orthogonal_settings_rules_sample_merge_target_entries_
/// are_flow_axis_and_never_cross`] — three plain, single-out-edge sources at three different flow
/// coordinates merging into one target, deliberately shaped (an obstacle, `X`, sitting on `A`'s own
/// row directly ahead of it, `B`'s the "settings rules" sample's own `usvg`-vs-`数式` collision)
/// so the best-effort `rank_lane_bend` retry (`classify`'s own §10-3 item 10 doc: "全ての候補が
/// 交差してもcross-axisのaltへは決して落ちない") fires on a fixture small enough to read by hand
/// rather than only on the 20-node real-world source above.
#[test]
fn orthogonal_synthetic_blocked_merge_still_enters_the_flow_axis_face() {
    let src = "flowchart LR\n  A --> T[Target]\n  B --> T\n  C --> T\n  A --> X[Blocker]\n  \
               Z --> X";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let t = d.node("T").expect("T must exist");
    let (l, tt, r, bb) = t.bounds();
    let merge: Vec<&PlacedEdge> = d.edges.iter().filter(|e| e.to == "T").collect();
    assert_eq!(
        merge.len(),
        3,
        "{:?}",
        merge.iter().map(|e| e.from.as_str()).collect::<Vec<_>>()
    );
    for e in &merge {
        let entry = e.points.last().unwrap();
        let on_flow_axis_face = (entry.x - (l - orthogonal::PORT_INSET)).abs() < 1e-6
            || (entry.x - (r + orthogonal::PORT_INSET)).abs() < 1e-6;
        assert!(
            on_flow_axis_face,
            "{}->T must enter T's own flow-axis face even when its own row is blocked: \
             entry={entry:?}, T bounds=({l},{tt},{r},{bb})",
            e.from
        );
    }
}

/// §10-3 item 13's own two user-reported regressions (`docs/FEATURE-MERMAID-RENDERER.md`, this
/// module's own `reserve_pass_through_rows`/`nest_merge_target_hops` fix): `MA`(`数式`) used to sit
/// almost exactly on `PD`(`ページ描画`)'s own row, one column upstream of `RS`(`ラスタライズ`) —
/// directly inside `PD -> RS`'s own pass-through row — forcing `PD -> RS` into a four-bend detour
/// around it. `reserve_pass_through_rows` moves `MA` clear of that row instead of leaving the edge
/// to detour; pinned two ways, the *why* (the row itself is now clear) and the *what* (the route is
/// a plain two-bend hop again, not a detour).
#[test]
fn orthogonal_settings_rules_sample_math_node_is_not_on_page_renders_row() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let pd = d.node("PD").expect("PD (ページ描画) must exist");
    let ma = d.node("MA").expect("MA (数式) must exist");
    let (_, ma_top, _, ma_bottom) = ma.bounds();
    let pd_row = pd.center.y;
    assert!(
        pd_row < ma_top - 1e-6 || pd_row > ma_bottom + 1e-6,
        "MA (数式) must not occupy ページ描画's own pass-through row (its horizontal exit leg runs \
         straight through this row on its way to RS): pd_row={pd_row}, MA bounds=({ma_top}, \
         {ma_bottom})"
    );
}

#[test]
fn orthogonal_settings_rules_sample_page_render_to_rasterise_is_two_bends_not_a_detour() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let pd_rs = d
        .edges
        .iter()
        .find(|e| e.from == "PD" && e.to == "RS")
        .expect("PD->RS must exist");
    assert_eq!(
        pd_rs.points.len(),
        4,
        "PD->RS must be a plain two-bend hop route once 数式 is out of ページ描画's own \
         pass-through row — a longer point list means it is still detouring around something: {:?}",
        pd_rs.points
    );
}

/// A minimal `SpecEdge`, following `label_boosts_actually_widen_the_flow_axis_segment_dagre_lays_
/// out`'s own hand-built-`SpecEdge` convention — every field this module's own routing code never
/// reads (label, tips, stroke, start/end labels, style) set to its plainest value, since only `from`/
/// `to`/`minlen` matter to `reserve_pass_through_rows`.
fn plain_edge(id: &str, from: &str, to: &str, minlen: usize) -> super::SpecEdge {
    super::SpecEdge {
        id: id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        label: None,
        tip_start: Tip::None,
        tip_end: Tip::Arrow,
        stroke: Stroke::Normal,
        minlen,
        start_label: None,
        end_label: None,
        style: None,
        curve: Curve::Basis,
    }
}

/// §10-3 item 13's own rule (`docs/FEATURE-MERMAID-RENDERER.md`), pinned directly against
/// `super::reserve_pass_through_rows` rather than through a full flowchart source: a hand-built
/// four-node layout, `A`(rank 0) skipping straight to `B`(rank 2) — a rank-skipping `A -> B`
/// (`minlen` 2) — with two nodes hand-placed on `A`'s own exact row at the intermediate rank 1:
/// `C` (`A`'s own real child, `A -> C`, must be left exactly where it is) and `W` (no edge to or
/// from `A` at all, only its own unrelated `W -> D`, standing in for the coincidental-occupant case
/// this rule exists to fix). Going through the full flowchart pipeline (`laid_out_flow`) cannot
/// reliably reproduce this exact collision — dagre's own cross-axis packing spreads unrelated
/// same-rank nodes apart by construction, so no small hand-written source was found that actually
/// forces two unrelated nodes onto the identical row — hence the direct call, with full control over
/// the "before" geometry this function's own contract is stated against.
#[test]
fn reserve_pass_through_rows_evicts_an_unrelated_node_but_not_a_real_neighbour() {
    let direction = crate::preview::mermaid::flowchart::Direction::LeftToRight;
    let mut nodes = vec![
        placed_node("A", 0.0, 100.0, 40.0, 20.0),
        placed_node("C", 100.0, 100.0, 40.0, 20.0), // A's real child — on A's row on purpose.
        placed_node("W", 100.0, 100.0, 40.0, 20.0), // unrelated — also on A's row on purpose.
        placed_node("B", 200.0, 100.0, 40.0, 20.0),
    ];
    let node_rank: HashMap<String, i32> = [
        ("A".to_string(), 0),
        ("C".to_string(), 1),
        ("W".to_string(), 1),
        ("B".to_string(), 2),
    ]
    .into_iter()
    .collect();

    let edge_ac = plain_edge("e_ac", "A", "C", 1);
    let edge_ab = plain_edge("e_ab", "A", "B", 2);
    let edge_wd = plain_edge("e_wd", "W", "D", 1);
    let drawable = vec![
        super::Drawable {
            edge: &edge_ac,
            tail: "A".to_string(),
            head: "C".to_string(),
        },
        super::Drawable {
            edge: &edge_ab,
            tail: "A".to_string(),
            head: "B".to_string(),
        },
        super::Drawable {
            edge: &edge_wd,
            tail: "W".to_string(),
            head: "D".to_string(),
        },
    ];

    // `A -> B`'s own shape is the only thing this test needs eligible: a plain, non-branching,
    // adjacent-source-facing merge into a lone target — exactly what `route_flowchart`'s own trial
    // route would classify as `is_pass_through_shape` for this hand-built fixture.
    let eligible: HashSet<String> = ["e_ab".to_string()].into_iter().collect();
    let moved =
        super::reserve_pass_through_rows(direction, &mut nodes, &node_rank, &drawable, &eligible);
    assert!(
        moved,
        "must actually evict W for this test to mean anything"
    );

    let row = |id: &str| nodes.iter().find(|n| n.id == id).unwrap().center.y;
    let a_row = row("A");
    assert!(
        (row("C") - a_row).abs() < 1e-6,
        "C (A's own direct child, A -> C) must stay exactly on A's own row — the guard must not \
         evict a real neighbour: a_row={a_row}, c_row={}",
        row("C")
    );
    assert!(
        (row("W") - a_row).abs() >= 22.0 - 1e-6,
        "W (unrelated to A) must be evicted clear of A's own pass-through row (half its own height \
         + PASS_THROUGH_CLEARANCE = 10+12 = 22px): a_row={a_row}, w_row={}",
        row("W")
    );
}

/// §10-3 item 10's own trailing "ホップ x の入れ子" (`docs/FEATURE-MERMAID-RENDERER.md`) — the
/// perpendicular-crossing invariant `orthogonal_settings_rules_sample_merge_target_entries_are_
/// flow_axis_and_never_cross` already pins (`orthogonal::segment_crossing`) cannot see two siblings'
/// vertical hop legs simply *coinciding* (`セルに合わせる`'s own `デコード`/`キーフレーム`, this
/// fix's own motivating bug — both landed on the exact same independently-computed hop `x` before
/// `nest_merge_target_hops`), nor a sibling whose route never went through `EdgeShape::rank_lane_
/// bend` at all (`ラスタライズ`'s own `mermaid -> RS` and `数式 -> RS`, both ordinary adjacent-rank
/// merges — `数式 -> RS`'s hop crossed `ページ描画 -> RS`'s own horizontal source-leg once `数式`
/// moved out of `ページ描画`'s row, the second real bug this fix closes).
/// `orthogonal::polylines_cross` (the exact predicate `nest_merge_target_hops` itself uses to decide
/// whether a candidate slot is clear) is the general question; this test asks it of every pair of
/// siblings converging on each of the sample's two multi-way merges.
#[test]
fn orthogonal_settings_rules_sample_merge_siblings_never_cross_or_coincide() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    for target in ["RS", "FIT"] {
        let merge: Vec<&PlacedEdge> = d.edges.iter().filter(|e| e.to == target).collect();
        assert!(
            merge.len() >= 2,
            "{target} must have at least two incoming edges to be a meaningful test: {:?}",
            merge.iter().map(|e| e.from.as_str()).collect::<Vec<_>>()
        );
        for i in 0..merge.len() {
            for j in (i + 1)..merge.len() {
                assert!(
                    !orthogonal::polylines_cross(&merge[i].points, &merge[j].points),
                    "{}->{target} crosses or coincides with {}->{target} — sibling merge routes \
                     must never cross or overlap: {} points={:?}, {} points={:?}",
                    merge[i].from,
                    merge[j].from,
                    merge[i].from,
                    merge[i].points,
                    merge[j].from,
                    merge[j].points
                );
            }
        }
    }
}

/// §10-3 item 10's own "8px 刻み" (`docs/FEATURE-MERMAID-RENDERER.md`), restated precisely rather
/// than as a blanket "every pair of siblings differs by 8px regardless of geometry": two sibling
/// merge edges whose vertical hop legs would otherwise sit at overlapping rows (`IM`(`デコード`)
/// and `VD`(`キーフレーム`), both genuinely below `FIT`(`セルに合わせる`)'s own centre, `orthogonal_
/// settings_rules_sample_fit_merge_keeps_same_side_siblings_on_the_same_side`'s own doc has the
/// full row picture) must land at least [`orthogonal::PORT_CLEARANCE`] apart — never on the
/// identical `x` [`nest_merge_target_hops`]'s own doc reports as this fix's original motivating bug.
#[test]
fn orthogonal_settings_rules_sample_fit_merge_hops_are_spaced_at_least_port_clearance_apart() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let im = d
        .edges
        .iter()
        .find(|e| e.from == "IM" && e.to == "FIT")
        .expect("IM->FIT must exist");
    let vd = d
        .edges
        .iter()
        .find(|e| e.from == "VD" && e.to == "FIT")
        .expect("VD->FIT must exist");
    assert_eq!(
        im.points.len(),
        4,
        "IM->FIT must be a two-bend hop route: {:?}",
        im.points
    );
    assert_eq!(
        vd.points.len(),
        4,
        "VD->FIT must be a two-bend hop route: {:?}",
        vd.points
    );
    let im_hop = im.points[1].x;
    let vd_hop = vd.points[1].x;
    assert!(
        (im_hop - vd_hop).abs() >= orthogonal::PORT_CLEARANCE - 1e-6,
        "IM->FIT and VD->FIT's own rows overlap (both genuinely sit below FIT's centre) — their \
         hops must be nested at least PORT_CLEARANCE apart, never land on the same x: \
         im_hop={im_hop} vd_hop={vd_hop}"
    );
}

/// [`orthogonal::nest_merge_target_hops`]'s own two invariants, generalised past the one 20-node
/// sample above to the whole [`CORPUS`] (every direction, subgraphs included) plus
/// [`SETTINGS_RULES_SAMPLE`] — `nest_merge_target_hops` itself carries no `tree.is_empty()`/
/// direction guard (unlike `reserve_pass_through_rows`, checked separately below), so nothing
/// about this invariant is scoped to a subset of diagrams: no two *genuine merge siblings*
/// converging on the same target may ever cross or coincide, full stop.
///
/// Scoped to `orthogonal::classify`'s own definition of a merge sibling — `source_out_degree <= 1`
/// (`is_merge_hop_candidate`'s own guard, `orthogonal.rs`) — not to every edge that happens to share
/// a target: `subgraph-bypass`'s own `X -> Y` shares a target with `B -> Y`, but `X` itself branches
/// (`X --> Y`, `X --> A`), so `classify` draws `X -> Y` as a *branch* shape, an unrelated shape
/// family this fix never touches — its own route can cross a foreign sibling for reasons that
/// predate and are out of scope for this fix (a pre-existing, structurally different bug class:
/// a branching source's own `staircase` fallback detour, not a merge target's own port nesting).
#[test]
fn orthogonal_merge_sibling_hops_never_cross_or_coincide_across_corpus() {
    for (name, src) in orthogonal_full_corpus()
        .into_iter()
        .chain([("settings-rules-sample", SETTINGS_RULES_SAMPLE)])
    {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        let mut out_degree: HashMap<&str, usize> = HashMap::new();
        for e in &d.edges {
            *out_degree.entry(e.from.as_str()).or_insert(0) += 1;
        }
        let mut by_target: HashMap<&str, Vec<&PlacedEdge>> = HashMap::new();
        for e in &d.edges {
            // `Stroke::Invisible` (`~~~`) is a layout-only link mermaid itself never draws a line
            // for (`svg.rs`'s own `Stroke::Invisible => return`) — a "crossing" against one is not
            // a visible defect, so it is out of scope for this invariant the same way it is out of
            // scope for the renderer itself.
            if e.stroke == Stroke::Invisible {
                continue;
            }
            // A genuine merge sibling only, matching `is_merge_hop_candidate`'s own scope — a
            // branching source's own edge into this same target is a different shape family
            // (this test's own doc explains why `subgraph-bypass` needs this filter).
            if out_degree.get(e.from.as_str()).copied().unwrap_or(0) > 1 {
                continue;
            }
            by_target.entry(e.to.as_str()).or_default().push(e);
        }
        for (target, merge) in by_target {
            if merge.len() < 2 {
                continue;
            }
            for i in 0..merge.len() {
                for j in (i + 1)..merge.len() {
                    assert!(
                        !orthogonal::polylines_cross(&merge[i].points, &merge[j].points),
                        "{name}: {}->{target} crosses or coincides with {}->{target}: {} \
                         points={:?}, {} points={:?}",
                        merge[i].from,
                        merge[j].from,
                        merge[i].from,
                        merge[i].points,
                        merge[j].from,
                        merge[j].points
                    );
                }
            }
        }
    }
}

/// §10-3 item 13's own rule ("列 A の行 R から出て列 T（T > A+1）へ入る辺は、間の各列で行 R を占め
/// る"), stated purely from the finished layout's own geometry (never `reserve_pass_through_rows`'s
/// own internal rank machinery, so this cannot silently drift the way a second copy of the same
/// rank arithmetic could): for every forward edge whose source and target sit at least a whole
/// node-width apart on the flow axis with a real gap between them, no *unrelated* third node (no
/// edge to the source, in either direction) may sit strictly between them on the flow axis *and*
/// within [`super::PASS_THROUGH_CLEARANCE`] of the source's own cross-axis row.
///
/// Scoped to the same window [`reserve_pass_through_rows`] itself is (`tree.is_empty()`,
/// `mod.rs`'s own call site) — a subgraph diagram is out of scope for that pass, the same known
/// limit `pull_back_fan_ranks` already has (`docs/STATUS.md`'s own ★未修正 entry), so a `CORPUS`
/// name containing "subgraph" is excluded here rather than asserting a rule the production code
/// does not even attempt to keep for it. Also scoped to a non-branching source
/// (`source_out_degree <= 1`, `orthogonal::is_pass_through_shape`'s own doc): a branching source's
/// edge never draws rule 10/13's flow-axis "exit then hop" leg at all — it is a branch (cross-axis
/// exit) or a `fan_lane` member (a *different* corridor concept, §10-3 item 2's own nested lanes —
/// `strokes`'s own `A -> F`, `A` also branching via `A --- B`, is exactly this: real topology skips
/// a column `C` sits in, but the edge's own shape never threads a row through it, so reserving one
/// would repeat the `long-edge` regression this fix exists to close).
#[test]
fn orthogonal_pass_through_row_never_holds_an_unrelated_node_across_corpus() {
    use crate::preview::mermaid::flowchart::Direction;

    for (name, src) in orthogonal_corpus()
        .into_iter()
        .filter(|(name, _)| !name.contains("subgraph"))
        .chain([("settings-rules-sample", SETTINGS_RULES_SAMPLE)])
    {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        let direction = direction_of(src);
        let flow = |p: &Point| match direction {
            Direction::LeftToRight | Direction::RightToLeft => p.x,
            Direction::TopToBottom | Direction::BottomToTop => p.y,
        };
        let cross = |p: &Point| match direction {
            Direction::LeftToRight | Direction::RightToLeft => p.y,
            Direction::TopToBottom | Direction::BottomToTop => p.x,
        };
        let flow_extent = |n: &PlacedNode| match direction {
            Direction::LeftToRight | Direction::RightToLeft => n.size.w / 2.0,
            Direction::TopToBottom | Direction::BottomToTop => n.size.h / 2.0,
        };
        let cross_extent = |n: &PlacedNode| match direction {
            Direction::LeftToRight | Direction::RightToLeft => n.size.h / 2.0,
            Direction::TopToBottom | Direction::BottomToTop => n.size.w / 2.0,
        };

        let connected: HashSet<(String, String)> = d
            .edges
            .iter()
            .flat_map(|e| {
                [
                    (e.from.clone(), e.to.clone()),
                    (e.to.clone(), e.from.clone()),
                ]
            })
            .collect();
        let mut out_degree: HashMap<&str, usize> = HashMap::new();
        for e in &d.edges {
            *out_degree.entry(e.from.as_str()).or_insert(0) += 1;
        }

        for e in &d.edges {
            if e.from == e.to {
                continue; // self-loop
            }
            if out_degree.get(e.from.as_str()).copied().unwrap_or(0) > 1 {
                continue; // a branching source never draws this shape family — see this test's doc.
            }
            let (Some(source), Some(target)) = (d.node(&e.from), d.node(&e.to)) else {
                continue;
            };
            let (sf, tf) = (flow(&source.center), flow(&target.center));
            // Only a genuine forward, rank-skipping span has a pass-through row to reserve at
            // all — a same-column (`aligned`) or immediately-adjacent-rank edge has no
            // intermediate column to check.
            if tf <= sf + 2.0 * flow_extent(source) {
                continue;
            }
            let row = cross(&source.center);
            for n in &d.nodes {
                if n.id == e.from || n.id == e.to {
                    continue;
                }
                if connected.contains(&(e.from.clone(), n.id.clone())) {
                    continue; // a real neighbour of the edge's own source — left alone on purpose.
                }
                let nf = flow(&n.center);
                let n_flow_half = flow_extent(n);
                // Strictly inside the source-target span, with a whole node-width of margin on
                // each side, so a node sharing the source's or target's own rank column (an
                // ordinary sibling, not an intermediate-column occupant) is never flagged.
                if nf - n_flow_half <= sf + flow_extent(source)
                    || nf + n_flow_half >= tf - flow_extent(target)
                {
                    continue;
                }
                let n_cross_half = cross_extent(n);
                assert!(
                    (cross(&n.center) - row).abs()
                        >= n_cross_half + super::PASS_THROUGH_CLEARANCE - 1e-6,
                    "{name}: {}->{} skips an intermediate column that {} (unrelated to {}) still \
                     occupies on {}'s own pass-through row: source_row={row}, {}'s own \
                     center={:?}",
                    e.from,
                    e.to,
                    n.id,
                    e.from,
                    e.from,
                    n.id,
                    n.center
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Stage 4: label placement, minimum segment length, port-vs-plate avoidance
// ---------------------------------------------------------------------------------------------
//
// `docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 3. Every labelled corpus source's plate is checked
// against every axis-parallel invariant stage 1-3 already state over the same corpus
// (`orthogonal_corpus`), plus the three new ones this stage adds.
//
// Two of the three sub-rules turned out hard to coax out of a *real* `dagre` layout on purpose,
// dumped and read by hand while this was written (the same way
// `orthogonal_settings_rules_sample_no_longer_pierces_a_sibling_node` was), so each is instead
// covered by a direct, hand-built fixture rather than a diagram source:
//
// * A genuine two-flow-leg split (uneven eviction giving an aligned edge's two ends different
//   coordinates, `bridge`'s own doc) — every scratch diagram tried either kept one leg the full
//   rank-gap length or the two nodes drifted out of `classify`'s 0.5px aligned window first. Its
//   own selection behaviour is covered directly in `orthogonal.rs`'s test module instead
//   (`label_slot_picks_the_longer_of_two_flow_axis_segments`), by constructing the split polyline
//   by hand.
// * A port stub actually crossing a *foreign* plate (item 3's second rule). `dagre`'s own
//   edge-label reservation (the same mechanism `label_boosts_actually_widen_...` below proves)
//   turns out to keep every other same-rank node clear of a label's own footprint by construction
//   — pushing the label wider pushes its rank-mates out by roughly the same amount, several
//   diagrams and node-size combinations tried, so no corpus source or hand-built `GraphSpec` here
//   reaches a genuine crossing to have avoided. `orthogonal::avoid_label_plates` is instead proven
//   directly (`avoid_label_plates_pushes_only_the_port_that_actually_crosses_a_plate` below,
//   `avoid_label_plates_ignores_a_reverse_edge` in `orthogonal.rs`), and
//   `no_label_plate_is_crossed_by_a_foreign_edges_segment` stands as the regression guard for the
//   day some future routing change does reach one.

/// Every labelled edge's plate, across [`orthogonal_corpus`].
fn labelled_plates(d: &Diagram) -> Vec<(&PlacedEdge, &super::PlacedEdgeLabel)> {
    d.edges
        .iter()
        .filter_map(|e| e.label.as_ref().map(|l| (e, l)))
        .collect()
}

/// Every axis-parallel segment of `e`'s own polyline, as `(a, b)` pairs — what a plate is allowed
/// to sit on top of without that counting as "off the line".
fn own_segments(e: &PlacedEdge) -> impl Iterator<Item = (&Point, &Point)> {
    e.points.windows(2).map(|w| (&w[0], &w[1]))
}

/// Item 3's first rule, restated as a corpus invariant: every labelled edge's plate is centred
/// **on** one of its own edge's segments (within a fraction of a px — the two are built from the
/// same arithmetic, so this is really asking "did the wiring stay connected", not "is the geometry
/// approximately right").
#[test]
fn every_label_plate_centre_sits_on_its_own_edges_line() {
    for (name, src) in orthogonal_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        for (e, l) in labelled_plates(&d) {
            let on_a_segment = own_segments(e).any(|(a, b)| {
                let on_seg = |p: &Point| {
                    let within_x = p.x >= a.x.min(b.x) - AXIS_EPS && p.x <= a.x.max(b.x) + AXIS_EPS;
                    let within_y = p.y >= a.y.min(b.y) - AXIS_EPS && p.y <= a.y.max(b.y) + AXIS_EPS;
                    let collinear =
                        ((b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)).abs() < 1e-3;
                    within_x && within_y && collinear
                };
                on_seg(&l.center)
            });
            assert!(
                on_a_segment,
                "{name}: label {:?} centre {:?} is not on any segment of {:?}",
                l.label.lines, l.center, e.points
            );
        }
    }
}

/// Item 3's minimum-length rule, over every labelled edge whose chosen segment is the flow-axis
/// one — the only kind [`super::apply_label_growth`]'s retry loop can actually widen (a
/// cross-axis-only label, reachable only through a `reverse`/`staircase` edge, is a documented gap
/// — [`orthogonal::LabelSlot::is_flow_axis`]'s own doc). `orthogonal_dag_corpus`, not the full
/// corpus, for the same reason [`orthogonal_bend_count_never_exceeds_two`] uses it: a back edge's
/// label is drawn from dagre's own waypoint chain, which this stage does not reach.
#[test]
fn labelled_flow_axis_segments_meet_their_minimum_length() {
    for (name, src) in orthogonal_dag_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        for (e, l) in labelled_plates(&d) {
            let Some(slot) = orthogonal::label_slot(direction_of(src), &e.points) else {
                continue;
            };
            if !slot.is_flow_axis {
                continue;
            }
            let need = orthogonal::label_min_length(l.size, slot.horizontal);
            assert!(
                slot.length + 1e-6 >= need,
                "{name}: labelled segment {:?} is {}px, short of the {}px minimum for plate {:?}",
                e.points,
                slot.length,
                need,
                l.size
            );
        }
    }
}

/// Parses `src`'s own `direction` line without going through the whole `lay_out_flow` pipeline —
/// [`labelled_flow_axis_segments_meet_their_minimum_length`]'s own use of
/// [`orthogonal::label_slot`] needs it directly, the same value [`Flowchart::direction`] carries.
fn direction_of(src: &str) -> crate::preview::mermaid::flowchart::Direction {
    parse(src)
        .unwrap_or_else(|e| panic!("corpus source must parse: {e}"))
        .direction
}

/// Item 3's growth rule stated the way the task itself asks for it: a long label widens the rank
/// gap it sits in; a short one does not push the diagram wider than an unlabelled version would
/// be. CJK, because that is what the real report (`docs/STATUS.md`'s mermaid work) was about.
/// `LR`, not `TD`: this diagram's flow axis is horizontal there, so a *wide* single-line label —
/// the ordinary shape a long label takes — is what stretches the gap, matching how the rank gap
/// actually grows (`lay_out_spec_pass`'s own doc: `TD`/`BT` tracks a label's raw *height*,
/// `LR`/`RL` its raw *width*) rather than needing an author-written `<br>` to make the label tall.
#[test]
fn a_long_cjk_label_widens_the_rank_gap_and_a_short_one_does_not() {
    let bare = laid_out_flow(
        "flowchart LR\n  A[Start] --> B[End]",
        "basis",
        "konoma-orthogonal",
    );
    let short = laid_out_flow(
        "flowchart LR\n  A[Start] -->|x| B[End]",
        "basis",
        "konoma-orthogonal",
    );
    let long = laid_out_flow(
        "flowchart LR\n  A[Start] -->|とても長いラベルのテキストです、これはとても長い| B[End]",
        "basis",
        "konoma-orthogonal",
    );
    let gap = |d: &Diagram| {
        let e = &d.edges[0];
        (e.points.last().unwrap().x - e.points[0].x).abs()
    };
    let (bare_gap, short_gap, long_gap) = (gap(&bare), gap(&short), gap(&long));
    assert!(
        short_gap > bare_gap,
        "even a 1-char label must reserve some rank space: {short_gap} vs bare {bare_gap}"
    );
    assert!(
        long_gap > short_gap + 50.0,
        "a long multi-em CJK label must widen the gap far more than a 1-char label: \
         long={long_gap} short={short_gap}"
    );
}

/// A hand-built [`GraphSpec`] with 2 nodes almost touching (`ranksep`-scale gap dwarfed by the
/// label) proves `label_boosts` — [`lay_out_spec_pass`]'s own new parameter — actually reaches
/// dagre: the *same* pass, called once with an empty boost map and once with a large one for the
/// one labelled edge, must come back with a measurably longer flow-axis segment when boosted. This
/// is the wiring stage 4's growth loop depends on ([`apply_label_growth`]'s own doc proves the
/// *loop's* bookkeeping separately); real diagrams rarely ask the loop to do anything at all — see
/// this module's own note on why — so this is what actually exercises the boosted code path rather
/// than leaving it provably-correct-but-never-run.
#[test]
fn label_boosts_actually_widen_the_flow_axis_segment_dagre_lays_out() {
    let node = |id: &str| super::SpecNode {
        id: id.to_string(),
        glyph: Glyph::Flow(Shape::Rect),
        label: Label::measure(id),
        size: Size::new(30.0, 20.0),
        panel: None,
        style: None,
        has_class: true,
    };
    let edge = super::SpecEdge {
        id: "e1".to_string(),
        from: "A".to_string(),
        to: "B".to_string(),
        label: Some(Label::measure("a fairly long edge label")),
        tip_start: Tip::None,
        tip_end: Tip::Arrow,
        stroke: Stroke::Normal,
        minlen: 1,
        start_label: None,
        end_label: None,
        style: None,
        curve: Curve::Basis,
    };
    let spec = super::GraphSpec {
        direction: crate::preview::mermaid::flowchart::Direction::TopToBottom,
        nodes: vec![node("A"), node("B")],
        edges: vec![edge],
        blocks: Vec::new(),
        routing: orthogonal::Routing::Orthogonal,
    };

    let flow_gap = |boosts: &HashMap<String, f64>| {
        let (diagram, _required, _shortfall) =
            super::lay_out_spec_pass(&spec, &HashMap::new(), boosts)
                .unwrap_or_else(|e| panic!("hand-built spec must lay out: {e}"));
        let e = &diagram.edges[0];
        (e.points.last().unwrap().y - e.points[0].y).abs()
    };

    let unboosted = flow_gap(&HashMap::new());
    let mut boosts = HashMap::new();
    boosts.insert("e1".to_string(), 300.0);
    let boosted = flow_gap(&boosts);

    assert!(
        boosted > unboosted + 250.0,
        "a 300px label_boosts entry must reach dagre and widen the rank gap by roughly that much: \
         unboosted={unboosted} boosted={boosted}"
    );
}

/// [`apply_label_growth`]'s own contract, stated directly: it **adds** each pass's shortfall onto
/// the running total (not `max`, unlike [`apply_growth`]'s node sizes — that function's own doc
/// explains why the two need different combinators), only for a positive shortfall, and reports
/// whether anything grew.
#[test]
fn apply_label_growth_accumulates_positive_shortfalls_and_ignores_the_rest() {
    let mut boosts: HashMap<String, f64> = HashMap::new();
    boosts.insert("already".to_string(), 10.0);

    let mut shortfall: HashMap<String, f64> = HashMap::new();
    shortfall.insert("already".to_string(), 5.0); // must ADD to 15, not replace with 5
    shortfall.insert("fresh".to_string(), 20.0); // a brand new entry
    shortfall.insert("met".to_string(), 0.0); // not actually short — must not be recorded at all
    shortfall.insert("negative".to_string(), -1.0); // defensive: never subtracts

    let grew = super::apply_label_growth(&mut boosts, &shortfall);
    assert!(grew, "a positive shortfall must report growth");
    assert!((boosts["already"] - 15.0).abs() < 1e-9, "{:?}", boosts);
    assert!((boosts["fresh"] - 20.0).abs() < 1e-9, "{:?}", boosts);
    assert!(
        !boosts.contains_key("met"),
        "a zero shortfall adds no entry"
    );
    assert!(
        !boosts.contains_key("negative"),
        "a negative shortfall must never be applied"
    );

    // A second call with an all-zero shortfall must report no further growth and leave the totals
    // exactly where they were — the loop-stopping condition `lay_out_spec` relies on.
    let mut zero: HashMap<String, f64> = HashMap::new();
    zero.insert("already".to_string(), 0.0);
    let grew_again = super::apply_label_growth(&mut boosts, &zero);
    assert!(!grew_again);
    assert!((boosts["already"] - 15.0).abs() < 1e-9);
}

/// §10-1 item 3's second rule ("ポートの垂直レーンがラベルプレートと重なる場合はポートをさらに
/// 16px外へ"), over the whole corpus: no plate ever ends up crossed by a *foreign* edge's segment
/// — either it never would have (the common case) or `avoid_label_plates` pushed the port that
/// would have crossed it clear. An edge's own line is exempt (it is what the plate sits on, by
/// item 3's first rule) and so is its own arrow tip.
#[test]
fn no_label_plate_is_crossed_by_a_foreign_edges_segment() {
    for (name, src) in orthogonal_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        for (owner, l) in labelled_plates(&d) {
            let (pl, pt, pr, pb) = (
                l.center.x - l.size.w / 2.0,
                l.center.y - l.size.h / 2.0,
                l.center.x + l.size.w / 2.0,
                l.center.y + l.size.h / 2.0,
            );
            for other in &d.edges {
                if std::ptr::eq(other, owner) {
                    continue;
                }
                for (a, b) in own_segments(other) {
                    let crosses = if (a.y - b.y).abs() < AXIS_EPS {
                        let y = a.y;
                        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
                        y >= pt && y <= pb && x1 >= pl && x0 <= pr
                    } else if (a.x - b.x).abs() < AXIS_EPS {
                        let x = a.x;
                        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
                        x >= pl && x <= pr && y1 >= pt && y0 <= pb
                    } else {
                        false
                    };
                    assert!(
                        !crosses,
                        "{name}: {:?}->{:?}'s segment {a:?}->{b:?} runs through {:?}->{:?}'s \
                         label plate {:?}",
                        other.from, other.to, owner.from, owner.to, l.label.lines
                    );
                }
            }
        }
    }
}

/// [`orthogonal::avoid_label_plates`] itself, directly: a port whose stub runs through a foreign
/// plate must move exactly [`orthogonal::PORT_SPACING`] further from its own node, and an edge that
/// never crosses anything must come back byte-identical.
#[test]
fn avoid_label_plates_pushes_only_the_port_that_actually_crosses_a_plate() {
    use crate::preview::mermaid::flowchart::Direction;
    use orthogonal::{avoid_label_plates, EligibleEdge};

    let a = placed_node("A", 100.0, 0.0, 60.0, 40.0);
    let b = placed_node("B", 100.0, 200.0, 60.0, 40.0);
    let c = placed_node("C", 300.0, 0.0, 60.0, 40.0);
    let d = placed_node("D", 300.0, 200.0, 60.0, 40.0);
    let nodes = vec![a, b, c, d];
    let edges = vec![
        EligibleEdge {
            id: "ab",
            source: "A",
            target: "B",
            raw: &[],
            source_rank: Some(0),
            target_rank: Some(1),
            source_out_degree: 1,
            target_in_degree: 1,
        },
        EligibleEdge {
            id: "cd",
            source: "C",
            target: "D",
            raw: &[],
            source_rank: Some(0),
            target_rank: Some(1),
            source_out_degree: 1,
            target_in_degree: 1,
        },
    ];
    let routed = orthogonal::route_flowchart(
        Direction::TopToBottom,
        &nodes,
        &[],
        &edges,
        &std::collections::HashMap::new(),
    );
    let mut points = routed.points;
    let ab_before = points["ab"].clone();
    let cd_before = points["cd"].clone();

    // A plate sitting squarely on top of `ab`'s own stub near A's face — not `ab`'s own label
    // (different edge id), so it counts as foreign for `ab` and must push it; `cd`'s stub is far
    // away (x=300) and must be left untouched.
    let mut plates: HashMap<String, super::PlacedEdgeLabel> = HashMap::new();
    plates.insert(
        "someone-elses-label".to_string(),
        super::PlacedEdgeLabel {
            center: Point::new(ab_before[0].x, (ab_before[0].y + ab_before[1].y) / 2.0),
            size: Size::new(40.0, 14.0),
            label: Label::measure("x"),
        },
    );

    avoid_label_plates(
        Direction::TopToBottom,
        &nodes,
        &[],
        &edges,
        &mut points,
        &mut plates,
        &std::collections::HashMap::new(),
    );

    assert_ne!(points["ab"], ab_before, "ab's port must have been pushed");
    assert_eq!(
        (points["ab"][0].x - ab_before[0].x).abs(),
        orthogonal::PORT_SPACING,
        "the push must be exactly PORT_SPACING: {:?} vs {:?}",
        points["ab"][0],
        ab_before[0]
    );
    assert_eq!(
        points["cd"], cd_before,
        "cd shares no plate with anything and must be left exactly as routed"
    );
}

// ---------------------------------------------------------------------------------------------
// Stage 5: the perimeter lane (§10-1 item 4)
// ---------------------------------------------------------------------------------------------

/// The smallest box holding every node and every subgraph frame in `d` — the same thing
/// `orthogonal::content_bounds` computes (private to that module, so this is its own independent
/// arithmetic over the public `Diagram` fields, not a call into the implementation it is checking).
fn diagram_content_bounds(d: &Diagram) -> (f64, f64, f64, f64) {
    let mut l = f64::INFINITY;
    let mut t = f64::INFINITY;
    let mut r = f64::NEG_INFINITY;
    let mut b = f64::NEG_INFINITY;
    for n in &d.nodes {
        let (nl, nt, nr, nb) = n.bounds();
        l = l.min(nl);
        t = t.min(nt);
        r = r.max(nr);
        b = b.max(nb);
    }
    for c in &d.clusters {
        let (cl, ct, cr, cb) = c.bounds();
        l = l.min(cl);
        t = t.min(ct);
        r = r.max(cr);
        b = b.max(cb);
    }
    (l, t, r, b)
}

/// Every `(name, from, to)` corpus edge that excurses past the content box **without** being a
/// genuine back edge — a collision-fallback forward edge (`EdgeShape::staircase`) whose own local
/// route (`route_with_ports`'s own doc — reverted off the perimeter lane 2026-09-01) can stray
/// outside the content box by whatever amount dagre's own waypoint chain and
/// `orthogonal::clear_local_route` produce, with no `PERIMETER_MARGIN + k*PERIMETER_LANE_SPACING`
/// guarantee at all — that guarantee is item 4's own, and it is written about the perimeter lane
/// specifically, which only a true `reverse` (non-self-loop) edge draws from now.
///
/// `classify` (the function that actually decides `reverse` vs `staircase`) is private to
/// `orthogonal.rs`, and a finished [`Diagram`] carries no shape tag a test could read back — so
/// this list is reasoned from each source's own topology instead (verified against `classify`'s
/// own rule, "target rank ≤ source rank" for a genuine back edge, applied to each source's own
/// declared chain — the whole corpus was dumped for its own excursion amounts first, over
/// `scratch_dump_perimeter_excursions`, to confirm no entry outside this list needs it):
/// `strokes`' `A<-->F`/`C~~~E` (both forward, `A`/`C` before `F`/`E` in the declared `A---B-.->C
/// ==>D--o E--x F` chain), `long-edge`'s `A->E` (forward — the *other* two excursions on this same
/// source, `E->B` and `C->A`, both close a cycle backward and stay genuine back edges), and
/// `subgraph-bypass`'s `X->Y` (forward — spans the same ranks the `one` frame does without closing
/// any cycle, `orthogonal_dag_corpus`'s own doc on this exact source).
///
/// `subgraph-direction`'s `one->D` joined this list once `shape_crosses_a_node`'s own-endpoint
/// check was added (`route_with_ports`'s own doc on the settings-rules regression). Its own
/// two-step cascade, dumped and read by hand rather than guessed at: the *original* shape (`one`
/// leaves `Top`, `D` receives `Right`) genuinely re-entered `D`'s own real box on its final leg —
/// the same bug class that check exists to catch — so `classify` swapped to the alternate
/// (`one` leaves `Left`, `D` receives `Bottom`); that alternate does *not* touch `D` at all, but
/// its own straight run at `one`'s port height happens to run directly through the row `one`'s own
/// members (`A`/`B`, real, foreign nodes to this edge) sit on — a perfectly ordinary *foreign*-node
/// collision, the ordinary padded test already caught before this fix existed, just never reached
/// for this edge until the first swap started happening for real. Both attempts failing sends it to
/// the raw-derived staircase, whose own port then leaves through `one`'s own outermost (`Left`)
/// face — a small, genuine `PORT_INSET`-scale excursion past the content box, not a perimeter-lane
/// one, the same "still a local route, no margin guarantee" category every other entry here is in.
fn known_staircase_forward_edge(name: &str, from: &str, to: &str) -> bool {
    matches!(
        (name, from, to),
        ("strokes", "A", "F")
            | ("strokes", "C", "E")
            | ("long-edge", "A", "E")
            | ("subgraph-bypass", "X", "Y")
            | ("subgraph-direction", "one", "D")
    )
}

/// How far outside `bounds` a point sits — 0 for a point inside or on the edge, otherwise its
/// distance past whichever side it cleared. What [`orthogonal_perimeter_edges_clear_the_margin`]
/// measures a perimeter edge's route by.
fn excursion(bounds: (f64, f64, f64, f64), p: &Point) -> f64 {
    let (l, t, r, b) = bounds;
    let dx = (l - p.x).max(p.x - r).max(0.0);
    let dy = (t - p.y).max(p.y - b).max(0.0);
    dx.max(dy)
}

/// §10-1 item 4's own headline invariant: any edge whose route strays outside the content box at
/// all — a `branch`/`merge`/`aligned` shape never does, since its whole route stays within
/// coordinates existing nodes already occupy, so straying at all is itself the signal this is a
/// perimeter-routed edge — clears it by at least [`orthogonal::PERIMETER_MARGIN`].
///
/// **Test-sufficiency audit finding 4 (2026-09-01)**: run only over `orthogonal_corpus()` (every
/// `CORPUS` source), this invariant never actually exercises `content_bounds`'s own "a cluster's
/// bounds are folded in, not just its members' node boxes" contract — every `CORPUS` subgraph is
/// small enough that its frame never juts out past its members' own node boxes by more than the
/// ordinary member padding, so the check would pass identically even if `content_bounds` read
/// `nodes` alone. `orthogonal_only_corpus()`'s `orthogonal-frame-hugs-backedge` (a long title
/// forcing the frame wider than its one small member) is what actually makes the two readings
/// disagree — see [`orthogonal_frame_hugs_backedge_perimeter_lane_reads_the_frame_not_just_the_nodes`]
/// for the same fact pinned to real numbers, and this crate's own mutation check (temporarily
/// dropping `content_bounds`'s `clusters` loop) confirms this loop is what actually catches it.
#[test]
fn orthogonal_perimeter_edges_clear_the_margin() {
    for (name, src) in orthogonal_corpus()
        .into_iter()
        .chain(orthogonal_only_corpus())
    {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        let bounds = diagram_content_bounds(&d);
        for e in &d.edges {
            if e.from == e.to {
                continue; // a self-loop's small local bump is not a perimeter lane at all.
            }
            if known_staircase_forward_edge(name, &e.from, &e.to) {
                continue; // a local route, not a perimeter one — see that function's own doc.
            }
            let max_excursion = e
                .points
                .iter()
                .map(|p| excursion(bounds, p))
                .fold(0.0_f64, f64::max);
            if max_excursion < AXIS_EPS {
                continue; // not a perimeter edge — never left the content box at all.
            }
            assert!(
                max_excursion + AXIS_EPS >= orthogonal::PERIMETER_MARGIN,
                "{name}: edge {}->{} strays only {max_excursion}px outside the content box, \
                 short of the {}px margin: {:?}",
                e.from,
                e.to,
                orthogonal::PERIMETER_MARGIN,
                e.points
            );
        }
    }
}

/// Finding 4's own specific pin, dumped by hand: `orthogonal-frame-hugs-backedge`'s `one` frame
/// (widened by its own long title, `subgraph-long-title`'s same trick) sits *above* `D`, `X` and
/// `Y`'s own node boxes — `one`'s top edge (y=24) is the true minimum of the whole diagram, well
/// above every node's own top (y=49) — so `Y->X`'s ring lane can only land at `content.top -
/// PERIMETER_MARGIN` if `content_bounds` actually read `one`'s own bounds; a node-only reading
/// would put the ring 25px further down (`49 - 16 = 33`, inside `one`'s own frame by 9px — a
/// direct violation of "at least 16px outside the outermost frame").
#[test]
fn orthogonal_frame_hugs_backedge_perimeter_lane_reads_the_frame_not_just_the_nodes() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = orthogonal_only_corpus()
        .into_iter()
        .find(|(name, _)| *name == "orthogonal-frame-hugs-backedge")
        .unwrap()
        .1;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let frame = d.cluster("one").expect("cluster one must exist").bounds();
    let bounds = diagram_content_bounds(&d);
    // `one`'s frame really is the widest thing here — the fixture's whole point.
    assert!(
        (bounds.1 - frame.1).abs() < 0.01,
        "the fixture must make the frame's own top the diagram's minimum y: content.top={} \
         frame.top={}",
        bounds.1,
        frame.1
    );
    let yx = d
        .edges
        .iter()
        .find(|e| e.from == "Y" && e.to == "X")
        .expect("Y->X must exist");
    let topmost = yx.points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    assert!(
        (bounds.1 - topmost - orthogonal::PERIMETER_MARGIN).abs() < 1.0,
        "Y->X's ring lane must sit exactly PERIMETER_MARGIN above content's own top (which \
         already folds the frame in): topmost={topmost} content.top={} margin={}",
        bounds.1,
        orthogonal::PERIMETER_MARGIN
    );
    // And, directly against the frame itself rather than the whole content box: the ring's own
    // topmost excursion (the point this test just pinned) clears `one`'s own top edge by the
    // margin too — not just checked indirectly through the diagram's own bounding box, which
    // happens to be dominated by the very frame under test here.
    let out = frame.1 - topmost;
    assert!(
        out + AXIS_EPS >= orthogonal::PERIMETER_MARGIN,
        "Y->X's topmost point clears frame `one`'s own top edge by only {out}px, short of the \
         {}px margin",
        orthogonal::PERIMETER_MARGIN
    );
}

/// §10-1 item 4's 8px stagger: every perimeter edge's own maximum excursion past the content box
/// is exactly `PERIMETER_MARGIN + k * PERIMETER_LANE_SPACING` for some non-negative integer `k`
/// (its own lane index) — and, over a diagram with more than one perimeter edge, two *different*
/// edges never land on the same `k` (dumped and checked on `long-edge`'s two back edges before
/// this was written: 16px and 24px, `k = 0` and `k = 1`, never both 16px).
#[test]
fn orthogonal_perimeter_edges_stagger_8px_apart() {
    for (name, src) in orthogonal_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        let bounds = diagram_content_bounds(&d);
        let mut lanes: Vec<i64> = Vec::new();
        for e in &d.edges {
            if e.from == e.to {
                continue; // a self-loop's small local bump is not a perimeter lane at all.
            }
            if known_staircase_forward_edge(name, &e.from, &e.to) {
                continue; // a local route, not a perimeter one — see that function's own doc.
            }
            let max_excursion = e
                .points
                .iter()
                .map(|p| excursion(bounds, p))
                .fold(0.0_f64, f64::max);
            if max_excursion < AXIS_EPS {
                continue;
            }
            let steps =
                (max_excursion - orthogonal::PERIMETER_MARGIN) / orthogonal::PERIMETER_LANE_SPACING;
            assert!(
                (steps - steps.round()).abs() < 1e-6,
                "{name}: edge {}->{} excursion {max_excursion}px is not \
                 PERIMETER_MARGIN + k*PERIMETER_LANE_SPACING for an integer k (k={steps}): {:?}",
                e.from,
                e.to,
                e.points
            );
            let lane = steps.round() as i64;
            assert!(
                !lanes.contains(&lane),
                "{name}: edge {}->{} shares lane {lane} with another perimeter edge already seen",
                e.from,
                e.to
            );
            lanes.push(lane);
        }
    }
}

/// §10-1 item 4's third rule (段 5 item 3, the coordinator's own numbering): "別々の辺のcross走行
/// が偶然同じ座標に重なる場合の一般検出と8pxずらし" — two edges that share **no** node (a shared
/// node is stage 1-4's own territory: eviction already spaces those 16px apart on the shared face)
/// whose segments happen to land exactly on top of each other by coincidence.
///
/// Audited across the whole corpus before writing any fix: **zero** such overlaps exist, on any
/// axis, in any of the 27 sources here — checked by hand with `eprintln!` dumps of every pair
/// before this was turned into a real assertion, not assumed. A speculative detector-and-nudge
/// pass for a case with no reproducible instance would be exactly the kind of code this project's
/// own conventions warn against (untestable in any way but a synthetic fixture nobody's real
/// diagram reaches) — see `docs/STATUS.md` for the honest "not implemented, no known case" record
/// this leaves stage 5 item 3 with. This test is the regression guard in its place: it holds today
/// because [`align_straight_lanes`](super::align_straight_lanes)' overlap resolution (stage 3) and
/// the perimeter lane's 8px stagger (stage 5 item 1) already separate every case the corpus
/// exercises; if a future change ever lets two unrelated edges collide, this is what catches it.
#[test]
fn no_two_unrelated_edges_coincidentally_overlap_on_the_same_axis() {
    for (name, src) in orthogonal_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        for i in 0..d.edges.len() {
            for j in (i + 1)..d.edges.len() {
                let (e1, e2) = (&d.edges[i], &d.edges[j]);
                if e1.from == e2.from || e1.from == e2.to || e1.to == e2.from || e1.to == e2.to {
                    continue; // shares a node — stage 1-4 territory, not "unrelated"
                }
                for w1 in e1.points.windows(2) {
                    for w2 in e2.points.windows(2) {
                        let vert1 = (w1[0].x - w1[1].x).abs() < AXIS_EPS;
                        let vert2 = (w2[0].x - w2[1].x).abs() < AXIS_EPS;
                        if vert1 && vert2 && (w1[0].x - w2[0].x).abs() < AXIS_EPS {
                            let (y0a, y1a) = (w1[0].y.min(w1[1].y), w1[0].y.max(w1[1].y));
                            let (y0b, y1b) = (w2[0].y.min(w2[1].y), w2[0].y.max(w2[1].y));
                            assert!(
                                y0a >= y1b - AXIS_EPS || y0b >= y1a - AXIS_EPS,
                                "{name}: {}->{} and {}->{} overlap at x={} \
                                 (y=[{y0a},{y1a}] vs [{y0b},{y1b}])",
                                e1.from,
                                e1.to,
                                e2.from,
                                e2.to,
                                w1[0].x
                            );
                        }
                        let horiz1 = (w1[0].y - w1[1].y).abs() < AXIS_EPS;
                        let horiz2 = (w2[0].y - w2[1].y).abs() < AXIS_EPS;
                        if horiz1 && horiz2 && (w1[0].y - w2[0].y).abs() < AXIS_EPS {
                            let (x0a, x1a) = (w1[0].x.min(w1[1].x), w1[0].x.max(w1[1].x));
                            let (x0b, x1b) = (w2[0].x.min(w2[1].x), w2[0].x.max(w2[1].x));
                            assert!(
                                x0a >= x1b - AXIS_EPS || x0b >= x1a - AXIS_EPS,
                                "{name}: {}->{} and {}->{} overlap at y={} \
                                 (x=[{x0a},{x1a}] vs [{x0b},{x1b}])",
                                e1.from,
                                e1.to,
                                e2.from,
                                e2.to,
                                w1[0].y
                            );
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Stage 5 item 2: the 12px crossing gap (§10-1 item 4)
// ---------------------------------------------------------------------------------------------

/// [`edges::split_at_gaps`] with no gaps at all must hand back the original polyline as the one
/// and only piece — the fast path `svg::emit_edge` relies on for byte-identical output whenever an
/// edge never crosses another one.
#[test]
fn split_at_gaps_with_no_gaps_returns_the_whole_polyline_unchanged() {
    let pts = vec![
        Point::new(0.0, 0.0),
        Point::new(0.0, 100.0),
        Point::new(50.0, 100.0),
    ];
    let pieces = edges::split_at_gaps(&pts, &[]);
    assert_eq!(pieces, vec![pts]);
}

/// One gap in the middle of a segment splits the polyline into exactly two pieces, each ending
/// (or starting) precisely at the gap's own two points — the geometry
/// [`orthogonal::insert_crossing_gaps`] hands `split_at_gaps` in practice.
#[test]
fn split_at_gaps_cuts_one_gap_into_two_pieces() {
    let pts = vec![
        Point::new(0.0, 0.0),
        Point::new(0.0, 100.0),
        Point::new(50.0, 100.0),
    ];
    let gap = (Point::new(0.0, 44.0), Point::new(0.0, 56.0));
    let pieces = edges::split_at_gaps(&pts, &[gap]);
    assert_eq!(
        pieces,
        vec![
            vec![Point::new(0.0, 0.0), Point::new(0.0, 44.0)],
            vec![
                Point::new(0.0, 56.0),
                Point::new(0.0, 100.0),
                Point::new(50.0, 100.0)
            ],
        ]
    );
}

/// Two gaps on two different segments of the same polyline produce three pieces, each gap cut
/// independently of the other.
#[test]
fn split_at_gaps_cuts_more_than_one_gap() {
    let pts = vec![
        Point::new(0.0, 0.0),
        Point::new(0.0, 100.0),
        Point::new(80.0, 100.0),
    ];
    let gaps = [
        (Point::new(0.0, 44.0), Point::new(0.0, 56.0)),
        (Point::new(30.0, 100.0), Point::new(42.0, 100.0)),
    ];
    let pieces = edges::split_at_gaps(&pts, &gaps);
    assert_eq!(
        pieces,
        vec![
            vec![Point::new(0.0, 0.0), Point::new(0.0, 44.0)],
            vec![
                Point::new(0.0, 56.0),
                Point::new(0.0, 100.0),
                Point::new(30.0, 100.0)
            ],
            vec![Point::new(42.0, 100.0), Point::new(80.0, 100.0)],
        ]
    );
}

/// A gap whose own two points do not land on any segment of the polyline it is handed (should
/// never happen in practice — `insert_crossing_gaps` always builds a gap from the very polyline
/// it will be applied to — but defensively checked rather than assumed) leaves the polyline
/// untouched instead of panicking or silently corrupting it.
#[test]
fn split_at_gaps_ignores_a_gap_that_does_not_land_on_the_polyline() {
    let pts = vec![Point::new(0.0, 0.0), Point::new(0.0, 100.0)];
    let gap = (Point::new(500.0, 500.0), Point::new(500.0, 512.0));
    let pieces = edges::split_at_gaps(&pts, &[gap]);
    assert_eq!(pieces, vec![pts]);
}

/// §10-1 item 4's real motivating case (dumped and confirmed 2026-09-01, after `route_with_ports`
/// stopped routing a collision-fallback forward edge (`EdgeShape::staircase`) onto the perimeter
/// lane — see that function's own doc for why), **re-dumped again** once §10-3 item 5 ("主辺どうし
/// の交差は水平辺側に12pxの隙間") replaced this fixture's old "detour side always spans" tie-break
/// for a pair of ordinary (non-perimeter) edges with an orientation-based one: neither `A->C`/
/// `B->D` (the two aligned, 0-bend lanes, §10-3 item 3's own correction) nor `A->D`/`B->C` (both
/// `fan_lane` shapes riding the same flow-axis faces as their aligned siblings) is a genuine
/// perimeter edge (`reverse`/`staircase`) here, so every crossing among these four now goes to
/// whichever *segment* is horizontal, never to "whichever edge is the fancier shape".
///
/// `amp-chain`'s `A->D` and `B->C` are both collision-fallback edges and both route locally, which
/// uncoupled them from the perimeter lane's own 8px stagger — without
/// `orthogonal::separate_coincident_detours`'s own fix they would land on the exact same vertical
/// run; with it, `B->C`'s run is nudged 8px sideways, so the two now merely *cross* once (at one
/// point) rather than coincide.
///
/// **Re-dumped a second time (§10-3 item 12, "面のポートは相手の側で配る")**: `A->C` is `classify`'s
/// own `aligned` shape — a straight 2-point line, structurally incapable of crossing anything — so
/// it never carried the two gap requests this doc used to describe; the pre-item-12 `evict` was
/// putting `A->D`'s and `B->C`'s own ports on the *wrong* side of their shared faces (an
/// array-symmetric re-splice, not each edge's genuine side), which routed both of them back up
/// through `A`'s own row and manufactured two spurious crossings against `A->C`'s line that
/// shouldn't have existed at all. With ports on their true sides, only the one real crossing
/// remains: `A->D`'s own horizontal leg into `D` crosses `B->C`'s vertical run — `A->D` is the
/// horizontal side there (`segment_crossing`'s own contract), so it alone carries the gap; `A->C`,
/// `B->C`, `B->D`, and `C->E` all stay whole.
#[test]
fn orthogonal_crossing_gaps_cut_the_horizontal_side_of_each_crossing() {
    let src = "flowchart LR\n  A & B --> C & D\n  C --> E";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");

    let edge = |from: &str, to: &str| {
        d.edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap_or_else(|| panic!("{from}->{to} must exist"))
    };
    let (ac, ad, bc, bd, ce) = (
        edge("A", "C"),
        edge("A", "D"),
        edge("B", "C"),
        edge("B", "D"),
        edge("C", "E"),
    );

    assert!(bc.gaps.is_empty(), "B->C must stay whole: {:?}", bc.gaps);
    assert!(bd.gaps.is_empty(), "B->D must stay whole: {:?}", bd.gaps);
    assert!(ce.gaps.is_empty(), "C->E crosses nothing: {:?}", ce.gaps);
    assert_eq!(
        ac.points.len(),
        2,
        "A->C is the aligned leg — a straight 2-point line, structurally unable to cross anything: \
         {:?}",
        ac.points
    );
    assert!(
        ac.gaps.is_empty(),
        "A->C must stay whole — item 12's own side fix removed the two spurious crossings this \
         used to have against A->D/B->C's own (previously wrong-sided) ports: {:?}",
        ac.gaps
    );

    assert_eq!(
        ad.gaps.len(),
        1,
        "A->D must carry exactly one gap, from being the horizontal side of its crossing with \
         B->C's vertical run: {:?}",
        ad.gaps
    );

    // Every gap actually sits on its own edge's line, and removes exactly `CROSSING_GAP` px of
    // *arc length* along it (or clamped shorter only if the line itself is that short, which none
    // here is) — not necessarily `CROSSING_GAP` px of straight-line (Euclidean) distance between
    // its own two endpoints. Measuring by Euclidean distance is what this test used to do at one
    // point, and is exactly what let a real regression through: `orthogonal::insert_crossing_gaps`'s
    // own `gap_around` clamped the omitted stretch to the single segment its crossing point was
    // found on, so a crossing landing close enough to a corner (never on macOS's own font metrics
    // for this diagram, reliably on Linux's, whose metrics size `B`/`C` a few px differently — CI
    // caught it, 2026-09-01) got a gap only 6px wide instead of 12. The fix measures by arc length
    // along the whole polyline instead, letting the omitted stretch continue past a corner onto
    // the next segment when it has to — which is exactly the case a same-segment / Euclidean check
    // cannot tell apart from an under-sized gap, since a corner-straddling gap's own two endpoints
    // are, correctly, less than `CROSSING_GAP` px of *straight-line* distance apart.
    for e in [ad] {
        for (g0, g1) in &e.gaps {
            let arc = edges::arc_length_between(&e.points, g0, g1).unwrap_or_else(|| {
                panic!(
                    "{}->{}: gap {g0:?}-{g1:?} is not on any of its own segments {:?}",
                    e.from, e.to, e.points
                )
            });
            assert!(
                (arc - orthogonal::CROSSING_GAP).abs() < 1e-6,
                "{}->{}: gap {g0:?}-{g1:?} removes {arc}px of arc length, not CROSSING_GAP: {:?}",
                e.from,
                e.to,
                orthogonal::CROSSING_GAP
            );
        }
    }
}

/// The SVG itself draws the horizontal side of each crossing as more than one `<path>` element
/// (one per piece [`edges::split_at_gaps`] returns) and the vertical side as exactly one —
/// `svg::emit_edge`'s own fast path for an edge with no gaps.
#[test]
fn orthogonal_crossing_gap_splits_the_svg_path_of_the_horizontal_side_only() {
    let src = "flowchart LR\n  A & B --> C & D\n  C --> E";
    let svg =
        crate::preview::markdown::mermaid_to_svg_flow(src, "dark", "basis", "konoma-orthogonal")
            .expect("must render");
    let path_count = svg.matches("<path").count();
    // `A->C`, `B->C`, `B->D`, `C->E` draw 1 path each (never cut, §10-3 item 12's own fix —
    // `orthogonal_crossing_gaps_cut_the_horizontal_side_of_each_crossing`'s own doc); `A->D` alone
    // carries one real gap (2 pieces): 4 + 2 = 6.
    assert_eq!(
        path_count, 6,
        "expected 4 uncut edges (1 path each) + A->D (2 pieces) = 6: got {path_count}\n{svg}"
    );
}

/// The bug the coordinator's own verification pass found right after item 1's own fix: `normalise`
/// translates every `PlacedEdge::points` by `(dx, dy)` to keep the whole diagram non-negative, but
/// did not translate `PlacedEdge::gaps` — so a gap computed in the pre-normalise coordinate space
/// (`orthogonal::insert_crossing_gaps` reads straight from `route_flowchart`'s own output, before
/// `normalise` ever runs) silently pointed at the wrong location once `points` moved out from
/// under it. Reproduced directly at the `laid_out_flow` level (not just visually): every gap's
/// `x`/`y` must land inside the *normalised* diagram's own bounds, and — the stronger
/// check — still sit exactly on its own edge's line (already checked above, from a fresh call;
/// this test exists so a regression here fails immediately by name rather than as a puzzling
/// "gap not on segment" failure in the more general test).
#[test]
fn normalise_moves_edge_gaps_along_with_points() {
    let src = "flowchart LR\n  A & B --> C & D\n  C --> E";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let mut checked_at_least_one = false;
    for e in &d.edges {
        for (g0, g1) in &e.gaps {
            checked_at_least_one = true;
            // The precise check, not just "somewhere inside the diagram" (too weak to catch the
            // real bug — `dx`/`dy` can be small enough that an un-translated gap still happens to
            // fall inside the normalised bounds by coincidence, which is exactly how this test's
            // own first draft passed even with the translation missing): every gap endpoint must
            // still land, *independently*, on some segment of its own edge's (normalised) polyline
            // — independently, not "both on the same segment", because a gap that straddles a
            // corner (reachable on a platform whose font metrics move a crossing close enough to a
            // bend — this is what Linux CI's own `insert_crossing_gaps` regression looked like,
            // 2026-09-01) legitimately has its two endpoints on two different segments. A missing
            // translation is still caught exactly as before: if `normalise()` moved `points` but
            // not `gaps`, the untranslated `g0`/`g1` land nowhere on the translated `points` at
            // all, on any segment.
            let on_own_segments = edges::arc_length_between(&e.points, g0, g1).is_some();
            assert!(
                on_own_segments,
                "{}->{}: gap {g0:?}-{g1:?} does not land on any of its own (normalised) \
                 segments {:?} — normalise() must have moved `points` without moving `gaps`",
                e.from, e.to, e.points
            );
        }
    }
    assert!(
        checked_at_least_one,
        "fixture must actually produce at least one gap for this test to mean anything"
    );
}

/// The Linux-only reproduction of the same `normalise`/`gaps` translation this module's own
/// `normalise_moves_edge_gaps_along_with_points` pins, but with a gap that **straddles a corner** —
/// its two endpoints on two different segments of the edge's own polyline, the shape a real crossing
/// only takes when it lands within `CROSSING_GAP / 2` of a bend (`orthogonal::gap_around`'s own
/// arc-length fix, 2026-09-01 — reachable on macOS's own font metrics only by hand-building the
/// input, which is exactly what this test does instead of depending on a platform's measured text).
/// `normalise_moves_edge_gaps_along_with_points`'s own corpus run never happens to produce this
/// shape on this machine, which is exactly how the coordinator's own CI re-run found a second,
/// narrower hole in the exact same test family: both of that test's own on-segment checks (this one
/// and `orthogonal_frame_crossings_never_produce_a_gap`) required `g0` *and* `g1` on the *same*
/// segment, which a corner-straddling gap fails even when `normalise` translated it perfectly.
#[test]
fn normalise_translates_a_corner_straddling_gap_onto_its_own_two_segments() {
    let mut d = Diagram {
        width: 0.0,
        height: 0.0,
        ..Diagram::default()
    };
    // An L-shaped polyline — vertical (0,0)-(0,20), then horizontal (0,20)-(30,20), the same shape
    // `orthogonal::tests::crossing_gap_straddling_a_corner_still_totals_12px_by_arc_length` uses —
    // carrying a gap whose two endpoints already straddle the corner *before* `normalise` runs:
    // (0, 11) on the vertical leg, (3, 20) 3px past the corner onto the horizontal leg.
    d.edges.push(PlacedEdge {
        from: "a".to_string(),
        to: "b".to_string(),
        points: vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 20.0),
            Point::new(30.0, 20.0),
        ],
        gaps: vec![(Point::new(0.0, 11.0), Point::new(3.0, 20.0))],
        tip_start: Tip::None,
        tip_end: Tip::None,
        stroke: Stroke::Normal,
        label: None,
        start_label: None,
        end_label: None,
        badge: None,
        series: None,
        straight: false,
        overlay: false,
        style: None,
        curve: Curve::Basis,
        tip_matches_line: false,
    });

    super::normalise(&mut d);

    let e = &d.edges[0];
    // The whole diagram sat at the origin (min_x = min_y = 0), so `normalise` must have shifted
    // everything by exactly `(MARGIN, MARGIN)` — points and gap alike.
    assert_eq!(
        e.points,
        vec![
            Point::new(MARGIN, MARGIN),
            Point::new(MARGIN, 20.0 + MARGIN),
            Point::new(30.0 + MARGIN, 20.0 + MARGIN),
        ],
        "normalise must shift every point of the polyline by (MARGIN, MARGIN): {:?}",
        e.points
    );
    assert_eq!(e.gaps.len(), 1, "{:?}", e.gaps);
    let (g0, g1) = &e.gaps[0];
    assert_eq!(
        (g0.clone(), g1.clone()),
        (
            Point::new(MARGIN, 11.0 + MARGIN),
            Point::new(3.0 + MARGIN, 20.0 + MARGIN)
        ),
        "the gap must be shifted by the exact same (MARGIN, MARGIN) as the points — a gap left in \
         the pre-normalise coordinate space would still read (0,11)-(3,20) here"
    );
    // Each endpoint independently lands on *some* segment of the *translated* polyline — g0 on the
    // (now shifted) vertical leg, g1 on the (now shifted) horizontal leg, two different segments —
    // and the arc length between them is still the full `CROSSING_GAP`, unchanged by translation.
    let arc = edges::arc_length_between(&e.points, g0, g1).unwrap_or_else(|| {
        panic!(
            "gap {g0:?}-{g1:?} does not land on the translated polyline {:?}",
            e.points
        )
    });
    assert!(
        (arc - orthogonal::CROSSING_GAP).abs() < 1e-9,
        "arc length between the translated gap's own two endpoints must still be CROSSING_GAP: {arc}"
    );
}

/// §10-1 item 4's other half: "辺と枠の交差は隙間なし（直交して跨ぐだけ）" — a subgraph frame is
/// never consulted by `insert_crossing_gaps` at all (it only ever compares two edges' own
/// segments), so an edge that crosses a frame boundary — `subgraph-bypass`'s own `X->A`, which the
/// corpus already names for crossing a frame from outside it — must never carry a gap because of
/// that frame, whatever its edge-vs-edge gaps (if any) are.
#[test]
fn orthogonal_frame_crossings_never_produce_a_gap() {
    for (name, src) in orthogonal_corpus() {
        if !name.contains("subgraph") {
            continue;
        }
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        // Every gap that does exist must still sit on its own edge's own line (never on a frame's
        // own boundary) — reusing the same "each endpoint independently on some segment of its own
        // edge" shape the normalise test above states (not "both endpoints on the *same* segment",
        // which a gap straddling a corner legitimately fails), generalised to the whole subgraph
        // corpus rather than one hand-picked source.
        for e in &d.edges {
            for (g0, g1) in &e.gaps {
                let on_own_segments = edges::arc_length_between(&e.points, g0, g1).is_some();
                assert!(
                    on_own_segments,
                    "{name}: {}->{}'s gap {g0:?}-{g1:?} is not on its own line — a frame must \
                     never be the reason a gap exists: {:?}",
                    e.from, e.to, e.points
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// `[ui] mermaid_routing = "konoma-orthogonal"` — §10-2's cluster-anchored edge stage
// ---------------------------------------------------------------------------------------------

/// A cluster-anchored edge (`one --> two`, `E --> one`) is actually routed by
/// `orthogonal::route_flowchart` — `PlacedEdge::straight`/`tip_matches_line` are both true, the
/// same pair every node-to-node orthogonal edge sets — not silently left on the spline fallback
/// `route_with_ports`'s `unwrap_or_else` exists for. Checked directly against `subgraph-endpoint`'s
/// two cluster-anchored edges (a node→cluster end and a cluster→cluster end) rather than inferred
/// from the axis-parallel corpus sweep above, which cannot tell "genuinely orthogonal" apart from
/// "happened to fall back to a spline that came out axis-parallel by coincidence".
#[test]
fn cluster_anchored_edges_are_actually_routed_orthogonally_not_by_the_spline_fallback() {
    let src = "flowchart LR\n  subgraph one [First]\n    A --> B\n  end\n  \
               subgraph two [Second]\n    C --> D\n  end\n  one --> two\n  E --> one";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let one_to_two = d
        .edges
        .iter()
        .find(|e| e.from == "one" && e.to == "two")
        .expect("one->two must exist");
    let e_to_one = d
        .edges
        .iter()
        .find(|e| e.from == "E" && e.to == "one")
        .expect("E->one must exist");
    for (name, e) in [("one->two", one_to_two), ("E->one", e_to_one)] {
        assert!(
            e.straight,
            "{name}: must be drawn as a polyline, not a curve"
        );
        assert!(
            e.tip_matches_line,
            "{name}: the arrow tip must match the routed line, the same as every other \
             orthogonal edge"
        );
    }
}

/// §10-1 item 1's "退避則" (16px port spacing) applies to a subgraph frame's own faces exactly
/// like a node's — and, since [`orthogonal::build_by_id`] resolves a cluster-anchored edge's end
/// through the very same `evict` face-grouping a node-anchored one uses, a genuine merge of three
/// cluster-anchored edges spaces them 16px apart in one shared order, exactly as it would for
/// three ordinary node-to-node edges.
///
/// §10-3 item 3's own correction (`classify`'s `merge_target_side` doc) moved `X`/`Y`/`Z --> one`
/// off the cluster's cross-axis (Left/Right, for this `TD` diagram) face this test originally
/// exercised, onto its flow-axis Top face instead — the same face `one --> D`/`one --> E` used to
/// share with them until this correction separated the two groups onto Top (the 3-way merge) and
/// Left (the 2-way branch) respectively; dumped and confirmed by hand, not assumed. `one`'s own
/// two out-edges no longer share a face with the merge at all under this fixture, so the "mixed
/// node-endpoint and cluster-endpoint claims on one shared face" scenario the original test built
/// is not exercised by this specific source any more — `docs/STATUS.md`'s own ★未修正 records that
/// as a real, if narrow, coverage gap left by this correction rather than silently dropping it.
/// What this test still genuinely proves: `evict`'s 16px spacing groups `X`/`Y`/`Z`'s three
/// cluster-anchored claims on `one`'s own Top face by "もう一方の端点のcross座標順" (rule 1) —
/// `X`'s own centre x is smallest, then `Y`'s, then `Z`'s — landing at `center - 16, center,
/// center + 16` in that order.
#[test]
fn cluster_face_shares_16px_ports_across_a_three_way_merge() {
    let src = "flowchart TD\n  subgraph one [Group]\n    A --> B\n    A --> C\n  end\n  \
               X --> one\n  Y --> one\n  Z --> one\n  one --> D\n  one --> E";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let one = d.cluster("one").expect("cluster one must exist");
    let (_, t, _, _) = one.bounds();

    let top_port = |from: &str| -> f64 {
        let e = d
            .edges
            .iter()
            .find(|e| e.from == from && e.to == "one")
            .unwrap_or_else(|| panic!("{from}->one must exist"));
        let p = e.points.last().expect("must have at least one point");
        assert!(
            (p.y - (t - orthogonal::PORT_INSET)).abs() < AXIS_EPS,
            "{from}->one: expected this edge's own end at \"one\" to land on the Top face \
             (y={}), got {p:?}",
            t - orthogonal::PORT_INSET
        );
        p.x
    };

    let x_x = top_port("X");
    let y_x = top_port("Y");
    let z_x = top_port("Z");

    assert!(
        x_x < y_x && y_x < z_x,
        "expected X < Y < Z by \"other cross coordinate\" order (rule 1), got X={x_x} Y={y_x} \
         Z={z_x}"
    );
    assert!(
        (y_x - x_x - orthogonal::PORT_SPACING).abs() < AXIS_EPS,
        "X->Y gap must be exactly PORT_SPACING: X={x_x} Y={y_x}"
    );
    assert!(
        (z_x - y_x - orthogonal::PORT_SPACING).abs() < AXIS_EPS,
        "Y->Z gap must be exactly PORT_SPACING: Y={y_x} Z={z_x}"
    );
}

/// A reverse edge whose target is a subgraph (`D --> one`, looping back into a block the flow
/// already passed) draws from the perimeter lane exactly like a reverse node-to-node edge does —
/// `route_perimeter`, not the 1-2 bend shapes forward edges take — and its route still clears both
/// of the block's own member nodes (`A`, `B`), not only the frame itself. §10-1 item 2's "回数制限
/// なし" for a back edge applies here too: this is *not* added to `orthogonal_dag_corpus`'s
/// bend-count cap.
#[test]
fn cluster_anchored_reverse_edge_routes_through_the_perimeter_lane_and_clears_its_own_members() {
    let src = "flowchart TD\n  subgraph one [Group]\n    A --> B\n  end\n  one --> D\n  D --> one";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let back = d
        .edges
        .iter()
        .find(|e| e.from == "D" && e.to == "one")
        .expect("D->one must exist");
    assert!(
        back.points.len() > 3,
        "a perimeter route has more than one bend: {:?}",
        back.points
    );
    for w in back.points.windows(2) {
        let dx = (w[1].x - w[0].x).abs();
        let dy = (w[1].y - w[0].y).abs();
        assert!(
            dx < AXIS_EPS || dy < AXIS_EPS,
            "perimeter route must stay axis-parallel: {w:?}"
        );
    }
    for member_id in ["A", "B"] {
        let member = d.node(member_id).expect("member must exist");
        for w in back.points.windows(2) {
            assert!(
                !orthogonal::segment_crosses_node(&w[0], &w[1], member),
                "D->one's perimeter route crosses its own block's member {member_id}: {w:?}"
            );
        }
    }
}

/// A cluster-to-cluster edge (`two --> one`, both ends naming a subgraph) resolves *both* its ends
/// against the two frames themselves, not against whichever member node dagre anchored the
/// underlying layout edge to — `orthogonal::build_by_id` has to prefer the cluster box over the
/// anchor's own node box for both `source` and `target` at once, the one shape of cluster-anchored
/// edge no other test here exercises (every other cluster-edge test in this file has at least one
/// ordinary node end).
#[test]
fn cluster_to_cluster_edge_resolves_both_ends_against_the_frames_not_their_anchors() {
    let src = "flowchart LR\n  subgraph one [First]\n    A --> B\n  end\n  \
               subgraph two [Second]\n    C --> D\n  end\n  two --> one";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let e = d
        .edges
        .iter()
        .find(|e| e.from == "two" && e.to == "one")
        .expect("two->one must exist");
    assert!(
        e.straight && e.tip_matches_line,
        "must be orthogonal-routed"
    );

    let one = d.cluster("one").expect("cluster one must exist");
    let two = d.cluster("two").expect("cluster two must exist");
    let (start, end) = (
        e.points.first().expect("must have a start point"),
        e.points.last().expect("must have an end point"),
    );
    let on_bounds = |p: &Point, (l, t, r, b): (f64, f64, f64, f64)| {
        let on_v = (p.y - (t - orthogonal::PORT_INSET)).abs() < AXIS_EPS
            || (p.y - (b + orthogonal::PORT_INSET)).abs() < AXIS_EPS;
        let on_h = (p.x - (l - orthogonal::PORT_INSET)).abs() < AXIS_EPS
            || (p.x - (r + orthogonal::PORT_INSET)).abs() < AXIS_EPS;
        on_v || on_h
    };
    assert!(
        on_bounds(start, two.bounds()),
        "two->one's own start {start:?} must sit on cluster \"two\"'s own bounds {:?}, not on \
         one of its member nodes' bounds",
        two.bounds()
    );
    assert!(
        on_bounds(end, one.bounds()),
        "two->one's own end {end:?} must sit on cluster \"one\"'s own bounds {:?}, not on one \
         of its member nodes' bounds",
        one.bounds()
    );
}

/// Review finding 3's suspicion, confirmed and fixed 2026-09-01 (a second pass, after the
/// coordinator independently confirmed the `r + 1` adjacency bug this test's own history records
/// and asked for it to be fixed constructively rather than left as read): [`align_straight_lanes`]
/// moves [`PlacedNode::center`], but the raw dagre waypoints a self-loop still draws from
/// (`EligibleEdge::raw`, `mod.rs`'s own comment on why it is read *before* alignment runs) were
/// never re-derived after that move — `route_staircase_with_ports` is the one shape
/// (`route_with_ports`'s own doc) that still bridges through `raw`'s interior points rather than
/// only the two evicted ports.
///
/// This did not reproduce over the first round of adversarial constructions below, for a reason
/// this test's own git history records in detail: `align_straight_lanes`'s chain candidate filter
/// used to check `node_rank.get(t) == Some(&(r + 1))`, an exact-integer "next rank" test that
/// dagre's own `makeSpaceForEdgeLabels` (`mod.rs`'s own `minlen *= 2` comment) made impossible to
/// satisfy — real node ranks always land two apart, so the filter never matched any real edge, in
/// any diagram, and lane alignment never fired at all. Once that was fixed (`align_straight_lanes`'s
/// own doc, "next rank some *real* node actually occupies"), it started moving self-loop owners for
/// real — and the coupling above turned out genuine: `one-sided-huge-push` below, a self-loop owner
/// pushed ~137px by a one-sided run of wide siblings, drew with its ports correctly on the node's
/// current boundary but its interior bump landing ~176px away in open space, dumped and confirmed
/// before any fix. The fix is [`align_straight_lanes`]'s own return value — every node's cross-axis
/// delta — applied back onto a self-loop's `raw` before `route_with_ports` ever reads it
/// (`mod.rs`'s own `raw` construction, [`shift_cross`]).
///
/// What this test pins now that the fix is in: every self-loop across the whole adversarial set —
/// including the one that actually reproduced the bug — must draw axis-parallel and stay within
/// [`SELF_LOOP_BUMP_MARGIN`] of its owner's *current* box. That margin is deliberately not scaled
/// by node size (a stale-`raw` bug does not get harder to see on a wider node — `position_self_edges`
/// sizes a self-loop's own bump off `NODE_SEP`-scale spacing, not the owner's box, confirmed by the
/// same dumps: `wide-head`'s 387px-wide node still draws a ~40px bump, same as every other source
/// here), so it stays tight enough to actually catch a real rank-scale drift rather than merely not
/// flake — verified directly: reverting the fix (skip applying the delta to `raw`) makes
/// `one-sided-huge-push` fail this exact assertion with the point ~176px outside the margin, and
/// restoring it passes again.
#[test]
fn self_loop_routes_correctly_across_lane_alignment_stress_cases() {
    let mut dense = String::from("flowchart TD\n");
    for i in 0..12 {
        dense.push_str(&format!("  d{i} --> d{}\n", i + 1));
        dense.push_str(&format!("  d{i} --> e{i}\n  e{i} --> d{}\n", i + 2));
    }
    dense.push_str("  d6 --> d6\n"); // a self-loop on a middle node of a dense diamond lattice
    let dense_diamond_lattice_source = dense;
    let sources: Vec<(&str, &str)> =
        vec![
        (
            "wide-head",
            "flowchart TD\n  A[this label is deliberately very long to force a wide box] --> B\n  \
             B --> C\n  A --> A",
        ),
        (
            "wide-tail",
            "flowchart TD\n  A --> B\n  \
             B --> C[this label is deliberately very long to force a wide box]\n  C --> C",
        ),
        (
            "asymmetric-siblings",
            "flowchart TD\n  P --> A\n  Q --> A\n  R --> A\n  A --> B\n  B --> C\n  A --> A",
        ),
        (
            "wide-sibling-pushes-loop-owner",
            "flowchart TD\n  A0 --> A1\n  A1 --> A2\n  A1 --> A1\n  \
             W[a very very very long wide sibling label to force a nodesep push] --> A2\n  \
             A0 --> W",
        ),
        (
            "z-competes-for-b",
            "flowchart TD\n  A --> B\n  B --> C\n  A --> A\n  \
             Z[a very very very long sibling label to push things around] --> B",
        ),
        (
            "deep-fanout-both-sides",
            "flowchart TD\n  W1 --> A\n  W2 --> A\n  W3 --> A\n  A --> A\n  A --> B\n  \
             B --> X1\n  B --> X2\n  B --> X3\n  B --> C\n  C --> D",
        ),
        ("dense-diamond-lattice", dense_diamond_lattice_source.as_str()),
        (
            // The fixture that actually reproduced the bug: every wide sibling pushes `A` the
            // *same* direction (no opposing push to cancel it out the way the symmetric
            // constructions above coincidentally did), so the align delta (~137px, dumped) is far
            // larger than a self-loop's own ~40px bump and cannot be masked by it.
            "one-sided-huge-push",
            "flowchart TD\n  \
             W1[extremely long sibling label number one to push things far to the right] --> A\n  \
             W2[extremely long sibling label number two to push things even further right] --> A\n  \
             W3[extremely long sibling label number three to push things still further right] --> A\n  \
             A --> A\n  A --> B\n  B --> C",
        ),
    ];
    // Deliberately not scaled by node size — see this test's own doc for why a fixed bound is the
    // stronger check here.
    const SELF_LOOP_BUMP_MARGIN: f64 = 100.0;
    for (name, src) in &sources {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        for e in &d.edges {
            if e.from != e.to {
                continue;
            }
            let owner = d
                .nodes
                .iter()
                .find(|n| n.id == e.from)
                .unwrap_or_else(|| panic!("{name}: self-loop owner {} must be a node", e.from));
            for w in e.points.windows(2) {
                let dx = (w[1].x - w[0].x).abs();
                let dy = (w[1].y - w[0].y).abs();
                assert!(
                    dx < AXIS_EPS || dy < AXIS_EPS,
                    "{name}: self-loop {} must stay axis-parallel: {w:?}",
                    e.from
                );
            }
            let (l, t, r, b) = owner.bounds();
            for p in &e.points {
                assert!(
                    p.x >= l - SELF_LOOP_BUMP_MARGIN
                        && p.x <= r + SELF_LOOP_BUMP_MARGIN
                        && p.y >= t - SELF_LOOP_BUMP_MARGIN
                        && p.y <= b + SELF_LOOP_BUMP_MARGIN,
                    "{name}: self-loop {} point {p:?} sits far outside {}'s own current box \
                     ({l},{t})-({r},{b}) (margin {SELF_LOOP_BUMP_MARGIN}) — exactly the drift \
                     review finding 3 asked about",
                    e.from,
                    e.from
                );
            }
        }
    }
}

// =============================================================================================
// 2026-09-01 test-sufficiency audit (orthogonal wiring mode, `docs/FEATURE-MERMAID-RENDERER.md`
// §10) — two independent audits (a spec-axis review and a mutation-testing run) of the tests
// above. Findings are implemented below, grouped and commented by finding number so each one's own
// reasoning stays next to its test. Every new source here is deliberately kept OUT of `CORPUS`
// itself: `CORPUS` is `mermaid_render.snap`'s own golden input, and this audit's own ground rule
// (`docs/FEATURE-MERMAID-RENDERER.md` §10's own governing instruction) is that the golden must not
// move. New sources live in `orthogonal_only_corpus` instead — this module's own equivalent of
// `orthogonal_corpus`/`orthogonal_dag_corpus`'s existing "filtered/extended view of `CORPUS`"
// pattern, just built from scratch rather than filtered.
// =============================================================================================

/// Sources built specifically to exercise a `konoma-orthogonal` corner none of `CORPUS`'s
/// shape-driven entries reaches — never folded into `CORPUS` (see this section's own doc).
fn orthogonal_only_corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            // Finding 1: `T` receives four merge-shaped edges onto the same face (`W1->T` lands
            // on `T`'s own flow face instead, aligned) — `docs/FEATURE-MERMAID-RENDERER.md` §10-1
            // item 1's own "ノードをその軸方向に拡大" (eviction growth), dumped and confirmed to
            // actually fire here (`T`'s box grows from its label-only size to fit four 16px-spaced
            // ports), with every node a member of the same subgraph frame — the growth retry
            // (`lay_out_spec`'s own loop) re-lays the whole diagram out afterward, so the cluster
            // invariants have to hold against the *grown*, not the original, geometry.
            "orthogonal-subgraph-growth",
            "flowchart TD\n  subgraph one [Group]\n    W1 --> T\n    W2 --> T\n    W3 --> T\n    \
             W4 --> T\n    W5 --> T\n  end\n  T --> X",
        ),
        (
            // Finding 4: a long title forces the frame `one` wider/taller than its one small
            // member `D` — dumped and confirmed the frame is in fact the widest/tallest thing in
            // the diagram (`D`, `X`, `Y` are all small default-sized nodes). `X`/`Y` sit entirely
            // outside `one` and close a two-node cycle, so `Y->X` is a genuine back edge routed
            // through the perimeter lane (§10-1 item 4) — one that has no other reason to come
            // anywhere near `one` except that `one`'s own frame juts into the ring's own margin
            // unless `orthogonal::content_bounds` actually folds a cluster's bounds in (not just
            // its members' node boxes, `content_bounds`'s own doc).
            "orthogonal-frame-hugs-backedge",
            "flowchart TD\n  subgraph one [resolve the preview kind and delegate it]\n    D[d]\n  \
             end\n  X --> Y\n  Y --> X",
        ),
        (
            // Finding 8a: `C->B` is a collision-fallback `staircase` detour around `A` (dumped:
            // its route dips right through the self-loop `A->A`'s own bump). Both cross in real
            // pixels, so this is the one source that actually reaches `insert_crossing_gaps`'s
            // self-loop/detour interaction rather than assuming it from reading the code.
            "orthogonal-self-loop-crosses",
            "flowchart TD\n  A --> A\n  A --> B\n  C --> A\n  C --> B",
        ),
    ]
}

/// §10-4's own "一般化チェック" round 2's three sources, and round 4's `4a`/`4b`/`4c` (see
/// `state_tests`'s own `orthogonal_design_reference_corpus` for those), made a **permanent**
/// fixture list (coordinator instruction, 2026-09-02): every corpus-wide orthogonal invariant
/// this module states has to actually run against the same pictures the design references show,
/// on every test run — not only the one-off audits that originally produced them. Kept apart from
/// [`orthogonal_only_corpus`] rather than appended to it: that list is a grab-bag of narrow,
/// single-purpose regression fixtures each written for one specific finding, none of them vetted
/// against the *general* corpus-wide checks this list is chained into (`orthogonal_full_corpus`)
/// — folding them in surfaced a genuine, pre-existing, unrelated bug in `orthogonal-subgraph-
/// growth` (`docs/STATUS.md`'s own ★未修正 entry) that this task is not in scope to fix, and
/// widening scope by accident is worse than a narrower, correctly-scoped list.
fn orthogonal_design_reference_corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            // The exact source behind `docs/render-check/zz-design-2a-browser.png`
            // (`docs/render-check/zz-design-sources.md`'s "第 2 回" section) — a plain decision
            // node, a subgraph frame, and a dashed back edge.
            "zz-design-2a",
            r#"flowchart TB
  F[ファイル] --> R{ルールに一致?}
  R -->|画像| I[デコード]:::media
  R -->|Markdown| M[ブロックモデル]:::text
  R -->|なし| X[プレビュー不可]
  subgraph W[ワーカー]
    I
    M
  end
  I --> K[kitty 転送]:::media
  M --> K
  K --> D[再描画]
  D -.-> F
  classDef media fill:#0f2038,stroke:#58a6ff,color:#e6edf3
  classDef text fill:#0f2617,stroke:#3fb950,color:#e6edf3
  style X fill:#271d0b,stroke:#d29922,color:#e6edf3"#,
        ),
        (
            // `zz-design-2b-browser.png`'s own source: nested subgraphs (LR), the shape
            // `orthogonal_cross_subgraph_edge_never_draws_a_diagonal_segment` pins one bug fix
            // against — kept here too so the *rest* of the invariant suite (axis-parallel,
            // perpendicular entry, non-puncture, non-crossing…) also runs over it every time,
            // not just the one regression that source was originally added to catch.
            "zz-design-2b",
            r#"flowchart LR
  subgraph C[クライアント]
    CLI[CLI]
    UI[ブラウザ UI]
    EX[エディタ拡張]
  end
  subgraph G[クラウド]
    API[API ゲート]
    Q[ジョブキュー]
    W[ジョブ実行系]
    SB[解析サンドボックス]:::exec
    subgraph S[保存層]
      DB[メタデータ DB]:::data
      AR[成果物保管]:::data
    end
  end
  subgraph E[外部]
    ID[認証基盤]
    LLM[モデル API]:::model
    GIT[コード置き場]:::exec
    PAY[決済ページ]
  end
  CLI --> API
  UI -->|HTTPS| API
  EX --> API
  API --> ID
  API -->|投入| Q
  Q -->|取り出し| W
  API --> DB
  W --> DB
  W --> AR
  W --> LLM
  W -->|ツール実行| SB
  SB --> GIT
  CLI -.->|リンク| PAY
  classDef data fill:#161b22,stroke:#3fb950,color:#e6edf3
  classDef model fill:#161b22,stroke:#a371f7,color:#e6edf3
  classDef exec fill:#161b22,stroke:#f85149,color:#e6edf3
  linkStyle 0,1,2 stroke:#58a6ff
  linkStyle 4,5 stroke:#d29922"#,
        ),
        (
            // `zz-design-2c-browser.png`'s own source: `2b`'s identical graph laid out `TB`
            // instead of `LR` — the axis flip is the point (§10-4's own "2b/2c" pair), so keeping
            // both here catches anything that only shows up under one `direction`.
            "zz-design-2c",
            r#"flowchart TB
  subgraph C[クライアント]
    CLI[CLI]
    UI[ブラウザ UI]
    EX[エディタ拡張]
  end
  subgraph G[クラウド]
    API[API ゲート]
    Q[ジョブキュー]
    W[ジョブ実行系]
    SB[解析サンドボックス]:::exec
    subgraph S[保存層]
      DB[メタデータ DB]:::data
      AR[成果物保管]:::data
    end
  end
  subgraph E[外部]
    ID[認証基盤]
    LLM[モデル API]:::model
    GIT[コード置き場]:::exec
    PAY[決済ページ]
  end
  CLI --> API
  UI -->|HTTPS| API
  EX --> API
  API --> ID
  API -->|投入| Q
  Q -->|取り出し| W
  API --> DB
  W --> DB
  W --> AR
  W --> LLM
  W -->|ツール実行| SB
  SB --> GIT
  CLI -.->|リンク| PAY
  classDef data fill:#161b22,stroke:#3fb950,color:#e6edf3
  classDef model fill:#161b22,stroke:#a371f7,color:#e6edf3
  classDef exec fill:#161b22,stroke:#f85149,color:#e6edf3
  linkStyle 0,1,2 stroke:#58a6ff
  linkStyle 4,5 stroke:#d29922"#,
        ),
    ]
}

/// [`orthogonal_corpus`] (`CORPUS`, which also drives the golden snapshot) plus
/// [`orthogonal_design_reference_corpus`] (never touches the golden) — every corpus-wide
/// orthogonal invariant that has to see the design-reference sources (coordinator instruction,
/// 2026-09-02) iterates this rather than either half alone. Deliberately does **not** also chain
/// in [`orthogonal_only_corpus`] — see that function's own doc for why folding it in is scope
/// creep this list does not want.
fn orthogonal_full_corpus() -> Vec<(&'static str, &'static str)> {
    orthogonal_corpus()
        .into_iter()
        .chain(orthogonal_design_reference_corpus())
        .collect()
}

/// Redumps `2a`/`2b`/`2c` under `konoma-orthogonal` to `docs/render-check/zz-design-<name>-
/// ours.{svg,png}`, next to the existing `zz-design-<name>-browser.png` reference each was drawn
/// from (coordinator instruction, 2026-09-02: keep these regenerable rather than one-off).
///
/// `docs/render-check/` (local-only, `.gitignore`'s own `/docs/` line — never committed) is not
/// touched otherwise: existing `zz-design-*` files are the ones the top-level task's own §4
/// instruction says never to delete (`find docs/render-check -maxdepth 1 -type f !
/// -name 'zz-design-*' -delete`), and this only *adds* to that family, using its own naming
/// (`-ours` marks the half this renderer drew, as opposed to `-browser`, `zz-compare-*`, or the
/// bare `zz-design-<name>-wrap.html`/`.svg` staging files the original Claude Design handoff
/// left behind).
///
/// `#[ignore]`d like [`state_tests::gallery`] — run explicitly:
/// `cargo test --features git -- --ignored orthogonal_design_reference_dump`.
#[test]
#[ignore = "writes PNG/SVG files for a person to look at: cargo test -- --ignored orthogonal_design_reference_dump"]
fn orthogonal_design_reference_dump() {
    let dir = std::path::Path::new("docs/render-check");
    std::fs::create_dir_all(dir).expect("create docs/render-check");
    for (name, src) in orthogonal_design_reference_corpus() {
        let svg =
            crate::preview::mermaid::render::render_flow(src, "dark", "basis", "konoma-orthogonal")
                .unwrap_or_else(|e| panic!("{name}: must render under konoma-orthogonal: {e}"));
        let svg_path = dir.join(format!("{name}-ours.svg"));
        std::fs::write(&svg_path, &svg).unwrap_or_else(|e| panic!("{name}: write svg: {e}"));
        let img = crate::preview::svg::rasterize_bytes(svg.as_bytes(), &svg_path, 1600)
            .unwrap_or_else(|| panic!("{name}: must rasterise"));
        img.save(dir.join(format!("{name}-ours.png")))
            .unwrap_or_else(|e| panic!("{name}: write png: {e}"));
    }
}

/// Finding 1 (high): the cluster invariants stage 1b already states over `CORPUS` under
/// `"splines"` (`check_clusters_hold_their_members` and its three siblings, `pub(super)` above so
/// this section can reuse them rather than re-deriving the same geometry questions), run again
/// under `"konoma-orthogonal"` — routing an edge does not move a node or a frame, but §10-1 item 1's
/// own eviction growth *does* (a face too narrow for its ports grows the node, and `lay_out_spec`
/// re-lays the whole diagram out from the grown size), so nothing before this audit had actually
/// checked a frame still holds its members once that growth has happened.
#[test]
fn invariant_orthogonal_clusters_hold_their_members() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_corpus()
        .into_iter()
        .chain(orthogonal_only_corpus())
    {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        let tree = tree_of_src(src);
        check_clusters_hold_their_members(name, &d, &tree);
    }
}

/// §10-5 part-3 item 1's own converse of the check just above — run over the same corpus, plus the
/// permanent design-reference sources (§10-5's own "恒久テスト対象" instruction), since `zz-
/// design-4a`'s state-diagram sibling was the case that actually exposed the gap this pins for the
/// flowchart side of the shared `orthogonal.rs` machinery too.
#[test]
fn invariant_orthogonal_foreign_nodes_stay_out_of_clusters() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_corpus()
        .into_iter()
        .chain(orthogonal_only_corpus())
        .chain(orthogonal_design_reference_corpus())
    {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        let tree = tree_of_src(src);
        check_foreign_nodes_stay_out_of_clusters(name, &d, &tree);
    }
}

#[test]
fn invariant_orthogonal_nested_clusters_sit_inside_their_parent() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_corpus()
        .into_iter()
        .chain(orthogonal_only_corpus())
    {
        check_nested_clusters_sit_inside_their_parent(
            name,
            &laid_out_flow(src, "basis", "konoma-orthogonal"),
        );
    }
}

#[test]
fn invariant_orthogonal_unrelated_clusters_do_not_overlap() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_corpus()
        .into_iter()
        .chain(orthogonal_only_corpus())
    {
        check_unrelated_clusters_do_not_overlap(
            name,
            &laid_out_flow(src, "basis", "konoma-orthogonal"),
        );
    }
}

#[test]
fn invariant_orthogonal_cluster_titles_stay_inside_their_frame_and_off_every_node() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_corpus()
        .into_iter()
        .chain(orthogonal_only_corpus())
    {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        let tree = tree_of_src(src);
        check_cluster_titles(name, &d, &tree);
    }
}

/// Finding 1's own eviction-growth half, stated directly rather than only through the invariants
/// above: `orthogonal-subgraph-growth`'s `T` must actually grow past its label-only size (the
/// scenario those invariants are being run *for*), and its grown box must still fit inside its own
/// frame with the ordinary 16px cluster padding — not merely "the invariant above happened to pass
/// for unrelated reasons".
///
/// §10-3 item 3's own correction (`classify`'s `merge_target_side` doc) moved this fixture's five
/// merges (`W1`..`W5 --> T`) off `T`'s cross-axis face (Left/Right, for this `TD` diagram — a
/// height requirement) onto its flow-axis Top face instead (a WIDTH requirement) — the same axis
/// flip `orthogonal_growth_widens_a_node_whose_face_cannot_fit_its_ports`'s own doc explains for
/// its very similar `Z` fixture. Dumped and confirmed by hand, not assumed.
#[test]
fn orthogonal_eviction_growth_inside_a_subgraph_still_fits_the_frame() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = orthogonal_only_corpus()
        .into_iter()
        .find(|(name, _)| *name == "orthogonal-subgraph-growth")
        .unwrap()
        .1;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let t = d.node("T").expect("T must exist");
    let label_only = shapes::size(
        Glyph::Flow(Shape::Rect),
        Size::new(t.label.width, t.label.height),
    );
    assert!(
        t.size.w > label_only.w + 1.0,
        "T must actually grow past its label-only width to fit its five merge ports on its Top \
         face: grown={} label_only={}",
        t.size.w,
        label_only.w
    );
    assert!(
        (t.size.h - label_only.h).abs() < 1e-6,
        "T's height must be untouched — nothing asked its Left/Right faces to grow any more: \
         grown={} label_only={}",
        t.size.h,
        label_only.h
    );
    let cluster = d.cluster("one").expect("cluster one must exist");
    let out = escapes(t.bounds(), cluster.bounds());
    assert!(
        out <= 0.01,
        "T's grown box must still sit fully inside its own frame: pokes {out:.2}px out"
    );
}

/// Finding 2 (high): a `classDef`/`style`-painted decision node under `"konoma-orthogonal"` draws
/// as a chamfered rectangle (`decision_node_is_chamfered_under_orthogonal_and_a_diamond_under_splines`
/// already pins the shape), but nothing before this audit ever rendered one *with* a resolved paint
/// — every existing orthogonal SVG test uses an unstyled node, and `CORPUS`'s own
/// `classdef-and-style-cascade` source (which does exercise the cascade) has no decision node.
/// `svg::emit_node`'s own `fill`/`stroke`/`sw` locals feed every `Outline` arm identically
/// (`Outline::Polygon` included, `svg.rs` line ~543), so this is really checking that a chamfered
/// node was not accidentally left on some *other* code path — the v0.28.0 regression's own shape
/// ("parses but nobody reads it").
#[test]
fn orthogonal_classdef_paints_a_chamfered_decision_node() {
    let src =
        "flowchart TD\n  classDef hot fill:#f9f,stroke:#a00\n  A --> B{cond}:::hot\n  B --> C";

    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let b = d.node("B").expect("B must exist");
    assert_eq!(
        b.shape,
        Glyph::ChamferedRect,
        "B must chamfer under orthogonal routing"
    );
    let style = b
        .style
        .as_ref()
        .expect("B must carry the resolved `:::hot` paint");
    assert_eq!(style.fill.as_deref(), Some("#f9f"));
    assert_eq!(style.stroke.as_deref(), Some("#a00"));

    let svg = render_flow(src, "dark", "basis", "konoma-orthogonal").expect("must render");
    // `<polygon` also draws every arrowhead (a small 3-point triangle in the theme's own
    // arrowhead colour) — pick the *node's* polygon by its 8 vertices, `shapes::polygon`'s own
    // `Glyph::ChamferedRect` shape (already pinned by
    // `decision_node_is_chamfered_under_orthogonal_and_a_diamond_under_splines`), not the first
    // `<polygon` line in source order.
    let vertex_count = |l: &str| -> usize {
        l.split("points=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .map(|pts| pts.split_whitespace().count())
            .unwrap_or(0)
    };
    let polygon = svg
        .lines()
        .filter(|l| l.starts_with("<polygon"))
        .find(|l| vertex_count(l) == 8)
        .unwrap_or_else(|| panic!("no 8-vertex chamfered-rect <polygon> element in:\n{svg}"));
    assert!(
        polygon.contains("fill=\"#f9f\""),
        "the chamfered node's own polygon must carry the classDef fill: {polygon}"
    );
    assert!(
        polygon.contains("stroke=\"#a00\""),
        "the chamfered node's own polygon must carry the classDef stroke: {polygon}"
    );
}

// -------------------------------------------------------------------------------------------
// §10-3 item 6: an edge with no colour of its own takes a lightened version of the downstream
// (target) node's own resolved class colour, under `konoma-orthogonal` only
// -------------------------------------------------------------------------------------------

/// The plain, no-conflict case: `B` carries `:::hot` (`stroke:#1f6feb`, one of the three colours
/// `style::lighten`'s own reference set pins), `A->B` names neither `class` nor `linkStyle` of its
/// own, so it takes `style::lighten("#1f6feb")` — the exact value `style::tests::
/// lighten_approximates_the_three_reference_conversions_within_its_documented_error` already pins
/// for that input, reused here rather than re-derived so the two tests can never silently drift
/// apart about what `lighten` actually returns.
#[test]
fn orthogonal_edge_color_lightens_the_downstream_class_color() {
    let src = "flowchart LR\n  classDef hot stroke:#1f6feb\n  A --> B:::hot";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let ab = d
        .edges
        .iter()
        .find(|e| e.from == "A" && e.to == "B")
        .expect("A->B must exist");
    let style = ab
        .style
        .as_ref()
        .expect("A->B must carry a derived style once B resolves a class stroke");
    assert_eq!(
        style.stroke.as_deref(),
        Some("#508eef"),
        "must match style::lighten(\"#1f6feb\") exactly"
    );
    // The label colour follows the line's own, same as the arrowhead already does via
    // `tip_matches_line` — `emit_edge`'s own doc on that field, extended to the label by this
    // rule.
    assert_eq!(style.text.as_deref(), Some("#508eef"));

    let svg = render_flow(src, "dark", "basis", "konoma-orthogonal").expect("must render");
    assert!(
        svg.contains("stroke=\"#508eef\""),
        "the edge's own <path> must draw in the lightened colour:\n{svg}"
    );
    assert!(
        svg.contains("fill=\"#508eef\""),
        "the arrowhead (tip_matches_line) must draw in the same lightened colour:\n{svg}"
    );
}

/// Same rule, with a labelled edge: the label's own `<text>` must also carry the lightened colour
/// (`svg::emit_edge`'s `label_color`, wired from `edge.style.text` — previously never read for an
/// edge label at all, `theme.edge_label_text` hardcoded regardless of any `class`/`:::`/
/// `linkStyle` `color:` the source declared).
#[test]
fn orthogonal_edge_color_reaches_the_label_text_too() {
    let src = "flowchart LR\n  classDef hot stroke:#1f6feb\n  A -->|hi| B:::hot";
    let svg = render_flow(src, "dark", "basis", "konoma-orthogonal").expect("must render");
    assert!(
        svg.contains("fill=\"#508eef\">") && svg.contains("hi"),
        "the edge label's own <text> must draw in the lightened colour:\n{svg}"
    );
}

/// An edge's own explicit paint — `linkStyle`, `class`, `:::`, or `style` naming *the edge itself*
/// — is the most specific instruction the source gave, and must win over the automatically
/// derived colour, exactly the priority order `docs/FEATURE-MERMAID-RENDERER.md` §10-3 item 6
/// states ("linkStyle 明示 > 自動導出 > テーマ既定"): a lightened downstream colour is a fallback
/// for an edge that asked for nothing, not an override for one that did.
#[test]
fn orthogonal_edge_color_prefers_an_explicit_link_style_over_the_derived_one() {
    let src = "flowchart LR\n  classDef hot stroke:#1f6feb\n  A --> B:::hot\n  linkStyle 0 stroke:#00ff00";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let ab = d
        .edges
        .iter()
        .find(|e| e.from == "A" && e.to == "B")
        .expect("A->B must exist");
    assert_eq!(
        ab.style.as_ref().and_then(|s| s.stroke.as_deref()),
        Some("#00ff00"),
        "the edge's own linkStyle must win over B's lightened class colour"
    );
}

/// A downstream node with no resolved class colour at all leaves the edge exactly as it drew
/// before this rule existed: `[ui] mermaid_routing = "konoma-orthogonal"`'s own theme default
/// (`docs/FEATURE-MERMAID-RENDERER.md` §10-3 item 6's own "クラス無しの下流は従来のテーマ辺色").
#[test]
fn orthogonal_edge_color_leaves_an_unclassed_downstream_node_alone() {
    let src = "flowchart LR\n  A --> B";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let ab = d
        .edges
        .iter()
        .find(|e| e.from == "A" && e.to == "B")
        .expect("A->B must exist");
    assert!(
        ab.style.is_none(),
        "an edge whose downstream node has no class must draw with no override at all: {:?}",
        ab.style
    );
}

/// §10-3 item 6 is `konoma-orthogonal` only ("konoma-orthogonal のみ", the rule's own opening
/// words) — the default `"splines"` routing must draw byte-for-byte what it always drew, the same
/// "既定は1バイトも変えない" invariant every other round-3 rule keeps.
#[test]
fn splines_routing_never_derives_an_edge_color_from_the_downstream_node() {
    let src = "flowchart LR\n  classDef hot stroke:#1f6feb\n  A --> B:::hot";
    let d = laid_out_flow(src, "basis", "splines");
    let ab = d
        .edges
        .iter()
        .find(|e| e.from == "A" && e.to == "B")
        .expect("A->B must exist");
    assert!(
        ab.style.is_none(),
        "splines routing must never derive an edge colour from the target node: {:?}",
        ab.style
    );
}

// Finding 3's app-wiring pin lives in `e2e_tests.rs`
// (`e2e_ui_mermaid_routing_changes_rendered_pixels_standalone_mmd_fullscreen` and
// `..._mermaid_fence_fullscreen`) — the other two ways a flowchart's pixels reach the screen
// besides an inline fence, which is all `e2e_ui_mermaid_routing_changes_rendered_pixels` covers.

/// Finding 5 (medium): §10-1 item 4's 2026-09-01 clarification — a back edge's line style is
/// whatever the author wrote (`-->`/`-.->`/`==>`), never something `konoma-orthogonal` itself
/// decides — pinned directly against the emitted SVG's `stroke-dasharray`, the one place a reader
/// actually sees it. `strokes` (`CORPUS`) exercises every stroke spelling but not a *back* edge in
/// particular; these two sources are minimal 2-node cycles so the back edge is the only edge in the
/// diagram whose stroke matters.
#[test]
fn orthogonal_back_edge_keeps_the_authors_own_line_style() {
    // A solid back edge (the ordinary `-->`) must draw with no dash array at all.
    let solid = render_flow(
        "flowchart TD\n  A --> B\n  B --> A",
        "dark",
        "basis",
        "konoma-orthogonal",
    )
    .expect("must render");
    assert!(
        !solid.contains("stroke-dasharray"),
        "a solid back edge must not be drawn dashed just because it is routed on the perimeter \
         lane: {solid}"
    );

    // A dotted back edge (`-.->`) must keep its own dashes.
    let dotted = render_flow(
        "flowchart TD\n  A --> B\n  B -.-> A",
        "dark",
        "basis",
        "konoma-orthogonal",
    )
    .expect("must render");
    assert!(
        dotted.contains(&format!("stroke-dasharray=\"{}\"", super::svg::DOTTED_DASH)),
        "an author-dashed back edge must keep drawing dashed: {dotted}"
    );
}

/// Finding 6 (medium): `[ui] mermaid_curve` / `%%{init}%%`'s `flowchart.curve` only mean anything
/// under `"splines"` (`docs/FEATURE-MERMAID-RENDERER.md` §10-2's own "orthogonal は折れ線なので
/// curve の概念が無い"); under `"konoma-orthogonal"` a curve name must be a complete no-op on the
/// emitted bytes. `orthogonal_splines_routing_is_deterministic_and_does_not_panic` states the
/// analogous fact for `"splines"` itself (curve resolution is decidable and non-panicking); this is
/// the missing byte-identity half for `"konoma-orthogonal"`.
#[test]
fn orthogonal_routing_ignores_mermaid_curve_and_the_init_directive() {
    for (name, src) in CORPUS {
        let basis = render_flow(src, "dark", "basis", "konoma-orthogonal")
            .unwrap_or_else(|e| panic!("{name}: basis must render: {e}"));
        for curve in ["linear", "step", "monotoneX", "bumpX"] {
            let other = render_flow(src, "dark", curve, "konoma-orthogonal")
                .unwrap_or_else(|e| panic!("{name}/{curve}: must render: {e}"));
            assert_eq!(
                basis, other,
                "{name}: konoma-orthogonal must ignore mermaid_curve={curve} entirely"
            );
        }
    }
    // The `%%{init}%%` directive's own `flowchart.curve` must be equally inert.
    let plain = render_flow(
        "flowchart TD\n  A --> B --> C",
        "dark",
        "basis",
        "konoma-orthogonal",
    )
    .expect("must render");
    let with_init = render_flow(
        "%%{init: {\"flowchart\": {\"curve\": \"stepBefore\"}}}%%\nflowchart TD\n  A --> B --> C",
        "dark",
        "basis",
        "konoma-orthogonal",
    )
    .expect("must render");
    assert_eq!(
        plain, with_init,
        "konoma-orthogonal must ignore an `%%{{init}}%%` flowchart.curve directive too"
    );
}

/// Finding 8a (medium): a self-loop is never a "detour" edge for §10-1 item 4's 12px crossing gap
/// (`orthogonal::insert_crossing_gaps`'s own `is_detour` excludes it via `source.id != target.id`)
/// — so when a self-loop and a genuine detour edge cross in real pixels (`orthogonal-self-loop-
/// crosses`, dumped and confirmed to cross), the self-loop must never be the side that gets cut, and
/// the crossing detour edge must be.
#[test]
fn orthogonal_self_loop_never_gets_cut_by_a_crossing_gap() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = orthogonal_only_corpus()
        .into_iter()
        .find(|(name, _)| *name == "orthogonal-self-loop-crosses")
        .unwrap()
        .1;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let loop_edge = d
        .edges
        .iter()
        .find(|e| e.from == "A" && e.to == "A")
        .expect("the self-loop must exist");
    assert!(
        loop_edge.gaps.is_empty(),
        "a self-loop must never be the spanning side of a crossing gap: {:?}",
        loop_edge.gaps
    );
    let detour = d
        .edges
        .iter()
        .find(|e| e.from == "C" && e.to == "B")
        .expect("C->B must exist");
    assert!(
        !detour.gaps.is_empty(),
        "the fixture must actually reproduce a real crossing against the self-loop, so C->B must \
         be the one carrying the gap: {:?}",
        detour.gaps
    );
    // And, directly: the fixture's own premise — the self-loop's bump and C->B's detour actually
    // occupy overlapping space in real pixels — confirmed by their bounding boxes overlapping on
    // both axes (the two polylines' own extents, not just "a gap happened to appear somewhere").
    let bbox = |pts: &[Point]| -> (f64, f64, f64, f64) {
        let (mut l, mut t, mut r, mut b) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for p in pts {
            l = l.min(p.x);
            t = t.min(p.y);
            r = r.max(p.x);
            b = b.max(p.y);
        }
        (l, t, r, b)
    };
    let (dx, dy) = rect_overlap(bbox(&loop_edge.points), bbox(&detour.points));
    assert!(
        dx > 0.0 && dy > 0.0,
        "fixture must reproduce a real overlap in space between the self-loop and C->B, not just \
         a coincidental gap: loop_bbox={:?} detour_bbox={:?}",
        bbox(&loop_edge.points),
        bbox(&detour.points)
    );
}

/// Finding 8b (medium): a self-loop never occupies a perimeter lane index
/// (`orthogonal::perimeter_lanes`'s own filter: `s.reverse && source.id != target.id`) — so adding
/// or removing a self-loop on a node that is *also* part of a genuine back-edge cycle must not shift
/// the back edge's own lane (and therefore its ring distance) at all. Pinned by comparing two
/// otherwise-identical sources, one with the self-loop and one without.
#[test]
fn orthogonal_self_loop_does_not_consume_a_perimeter_lane() {
    if !text_metrics::fonts_available() {
        return;
    }
    let with_loop = laid_out_flow(
        "flowchart TD\n  A --> A\n  A --> B\n  B --> C\n  C --> A",
        "basis",
        "konoma-orthogonal",
    );
    let without_loop = laid_out_flow(
        "flowchart TD\n  A --> B\n  B --> C\n  C --> A",
        "basis",
        "konoma-orthogonal",
    );
    let ca_with = with_loop
        .edges
        .iter()
        .find(|e| e.from == "C" && e.to == "A")
        .expect("C->A must exist");
    let ca_without = without_loop
        .edges
        .iter()
        .find(|e| e.from == "C" && e.to == "A")
        .expect("C->A must exist");
    assert_eq!(
        ca_with.points, ca_without.points,
        "C->A's own route must be byte-identical whether or not A also carries a self-loop \
         (the self-loop must never consume a perimeter lane slot): with={:?} without={:?}",
        ca_with.points, ca_without.points
    );
}

/// Finding 10 (low, investigated): `MAX_GROWTH_PASSES = 3` (`mod.rs`'s own `lay_out_spec`) is a
/// defensive cap over a process the code's own doc states is monotonic and therefore always
/// converges on its own — `apply_growth` only ever grows toward an exact, one-shot-computable
/// minimum (`Eviction::required_size`), not an iterative approximation, so there is no obvious
/// adversarial input that forces three full passes rather than converging in one or two. A stress
/// source combining heavy fan-in (12 merge edges onto one face, forcing node growth) with long
/// labels (forcing `label_boosts` growth at the same time, `mod.rs`'s own doc: "a pass can ask for
/// both at once") was tried; `lay_out_spec` has no public hook that reports how many passes a given
/// input actually took, so whether this specific source reaches the cap could not be confirmed
/// directly. What is confirmed: the combined stress case renders without panicking and comes out
/// axis-parallel and collision-free, i.e. self-consistent regardless of how many passes it took —
/// the property finding 10 asked to be pinned "if reachable", stated unconditionally instead since
/// reachability itself could not be established.
#[test]
fn orthogonal_heavy_growth_stress_renders_self_consistently() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut src = String::from("flowchart TD\n");
    for i in 1..=12 {
        src.push_str(&format!(
            "  W{i}[extremely long sibling label number {i} to force both node and label growth at once] --> T\n"
        ));
    }
    src.push_str("  T -- also a long label on the way out --> X\n");
    let d = laid_out_flow(&src, "basis", "konoma-orthogonal");
    // Self-consistency: every segment axis-parallel, no edge crosses a foreign node's box.
    for e in &d.edges {
        for w in e.points.windows(2) {
            let dx = (w[1].x - w[0].x).abs();
            let dy = (w[1].y - w[0].y).abs();
            assert!(
                dx < AXIS_EPS || dy < AXIS_EPS,
                "heavy-growth stress: {}->{} must stay axis-parallel: {w:?}",
                e.from,
                e.to
            );
        }
        for n in &d.nodes {
            if n.id == e.from || n.id == e.to {
                continue;
            }
            for w in e.points.windows(2) {
                assert!(
                    !orthogonal::segment_crosses_node(&w[0], &w[1], n),
                    "heavy-growth stress: {}->{} crosses foreign node {}",
                    e.from,
                    e.to,
                    n.id
                );
            }
        }
    }
    // And render_flow (the SVG-emitting entry point) must not panic on the same source either.
    render_flow(&src, "dark", "basis", "konoma-orthogonal").expect("must render");
}

/// Finding 11 (low): degenerate flowcharts under `"konoma-orthogonal"` must not panic. A single
/// node with no edges at all exercises `lay_out_spec`'s growth loop with an empty `eligible` list
/// (`spec.routing == Routing::Orthogonal` but nothing to route); a self-loop on a lone node
/// exercises the one-node/one-edge case of every pass (`evict`, `align_straight_lanes`,
/// `insert_crossing_gaps`) at once.
#[test]
fn orthogonal_degenerate_diagrams_do_not_panic() {
    if !text_metrics::fonts_available() {
        return;
    }
    for src in ["flowchart TD\n  A[only]", "flowchart TD\n  A --> A"] {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        for e in &d.edges {
            for w in e.points.windows(2) {
                let dx = (w[1].x - w[0].x).abs();
                let dy = (w[1].y - w[0].y).abs();
                assert!(
                    dx < AXIS_EPS || dy < AXIS_EPS,
                    "{src:?}: {}->{} must stay axis-parallel: {w:?}",
                    e.from,
                    e.to
                );
            }
        }
        render_flow(src, "dark", "basis", "konoma-orthogonal")
            .unwrap_or_else(|e| panic!("{src:?} must render under konoma-orthogonal: {e}"));
    }
}

/// [`orthogonal_degenerate_diagrams_do_not_panic`]'s remaining case — two nodes placed at the exact
/// same centre — cannot be produced through a real mermaid source at all (dagre always separates
/// same-rank nodes by `nodesep`), so it is stated directly against [`orthogonal::route_edge`]
/// instead, the same private-access shortcut `orthogonal.rs`'s own unit tests use.
#[test]
fn orthogonal_route_edge_does_not_panic_on_coincident_centres() {
    let a = placed_node("A", 100.0, 100.0, 60.0, 40.0);
    let b = placed_node("B", 100.0, 100.0, 60.0, 40.0);
    // `route_edge` is `pub`; the coincident-centre case is exercised through it rather than the
    // private `route_flowchart`/`evict` pair this file has no direct access to.
    let pts = orthogonal::route_edge(
        crate::preview::mermaid::flowchart::Direction::TopToBottom,
        &a,
        &b,
        &[],
        Some(0),
        Some(1),
        1,
        1,
    );
    assert!(
        !pts.is_empty(),
        "must return a non-empty polyline, not panic"
    );
    for p in &pts {
        assert!(
            p.x.is_finite() && p.y.is_finite(),
            "must never emit a NaN/infinite point: {p:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// TEMP probe for docs/FEATURE-MERMAID-RENDERER.md §10-3 work — dumps a markdown file's own mermaid
// fences under `konoma-orthogonal` to `docs/render-check` for structural/visual comparison against
// hand-drawn reference SVGs. `#[ignore]`d so it never runs in the normal battery.

/// Dumps every mermaid fence in `md_path` to `docs/render-check/<stem>-NN-konoma-orthogonal.{svg,png}`
/// — `dump_mermaid_ja_corpus_under_orthogonal` and `dump_render_check_probes_under_orthogonal`'s
/// own shared body, factored out once a second source (`docs/render-check/zz-design-sources.md`,
/// `docs/render-check/zz-design-longedge-probe.md`) needed the identical treatment.
fn dump_fences_under_orthogonal(md_path: &str, stem: &str) {
    let md = std::fs::read_to_string(md_path).unwrap_or_else(|e| panic!("read {md_path}: {e}"));
    let mut fences: Vec<String> = Vec::new();
    let mut cur: Option<String> = None;
    for line in md.lines() {
        if let Some(buf) = &mut cur {
            if line.trim() == "```" {
                fences.push(std::mem::take(buf));
                cur = None;
            } else {
                buf.push_str(line);
                buf.push('\n');
            }
        } else if line.trim() == "```mermaid" {
            cur = Some(String::new());
        }
    }
    assert!(
        !fences.is_empty(),
        "{md_path}: must find at least one mermaid fence"
    );

    for (i, code) in fences.iter().enumerate() {
        let n = i + 1;
        let path = format!("docs/render-check/{stem}-{n:02}-konoma-orthogonal.svg");
        // `[ui] mermaid_routing` only ever means anything for a flowchart (`mod.rs`'s own doc) —
        // every other diagram kind renders through the ordinary `render`, unaffected by it.
        // `[ui] mermaid_routing` only ever means anything for a flowchart — this is the real
        // dispatcher every other diagram kind goes through in production
        // (`preview::markdown::mermaid_to_svg_reason_flow`), not `render_flow` alone (which only
        // ever knows how to parse a flowchart).
        let svg = crate::preview::markdown::mermaid_to_svg_flow(
            code,
            "dark",
            "basis",
            "konoma-orthogonal",
        );
        match svg {
            Some(svg) => {
                std::fs::write(&path, &svg).unwrap_or_else(|e| panic!("write {path}: {e}"));
                let png_path = format!("docs/render-check/{stem}-{n:02}-konoma-orthogonal.png");
                if let Some(img) = crate::preview::svg::rasterize_bytes(
                    svg.as_bytes(),
                    std::path::Path::new(&path),
                    2400,
                ) {
                    img.save(&png_path)
                        .unwrap_or_else(|e| panic!("save {png_path}: {e}"));
                }
            }
            None => eprintln!("{md_path} fence {n}: render failed"),
        }
    }
}

/// Dumps the "大きさ" flowchart (`samples/mermaid.ja.md` fence 3) to `docs/render-check` for
/// structural comparison against the round3 handoff's 3a SVG, and every other
/// `samples/mermaid.ja.md` fence for the same visual spot-check. Run explicitly with
/// `cargo test --features git dump_mermaid_ja_corpus_under_orthogonal -- --ignored --nocapture`.
#[test]
#[ignore]
fn dump_mermaid_ja_corpus_under_orthogonal() {
    dump_fences_under_orthogonal("samples/mermaid.ja.md", "mermaid.ja");
}

/// Dumps `docs/render-check/zz-design-sources.md` (2a/2b/2c-equivalent sources — a worker subgraph,
/// LR/TB nested-subgraph client/cloud/external graphs) and `docs/render-check/zz-design-longedge-
/// probe.md` (`CORPUS`'s own `long-edge` source, standalone, for the exact regression this session's
/// pass-through-reservation fix closes) to `docs/render-check`, for the general-corpus checks
/// `docs/FEATURE-MERMAID-RENDERER.md` §10-3's own implementation notes on this session's three
/// findings describe. Run explicitly with `cargo test --features git
/// dump_render_check_probes_under_orthogonal -- --ignored --nocapture`.
#[test]
#[ignore]
fn dump_render_check_probes_under_orthogonal() {
    dump_fences_under_orthogonal(
        "docs/render-check/zz-design-sources.md",
        "zz-design-sources",
    );
    dump_fences_under_orthogonal(
        "docs/render-check/zz-design-longedge-probe.md",
        "zz-design-longedge-probe",
    );
}

/// §10-3 item 13's own regression, found by generalising past `CORPUS` to real, larger sources
/// (`docs/render-check/zz-design-sources.md`'s `2b`): `API -> ID`, a cross-subgraph node-to-node
/// edge whose `classify`-decided shape is `staircase` (`raw` carries 13 dagre waypoints, several
/// collinear before ever reaching `ID`'s own port). `clear_local_route`'s own crossing search can
/// flag an *early* window of that collinear run, not the literal last one — an earlier draft of the
/// local-detour fix (§10-3 item 13) judged whether a port sat on the coordinate being moved by the
/// *literal window index* (`i + 2 == points.len()`) rather than by coordinate, so it never noticed
/// `ID`'s own port shared the exact coordinate an early window's fix slid out from under it: a
/// diagonal final segment, `orthogonal_routing_draws_only_axis_parallel_segments`'s own invariant
/// broken in a shape `CORPUS` had no fixture for. Pinned here directly, not only through the
/// corpus-wide invariant, because this is the exact real-world diagram the bug was found on
/// (`local_detour`'s own current implementation never slides a shared coordinate at all — it
/// detours only the interior span between two freshly-inserted boundary points either side of the
/// obstacle, so this specific failure mode cannot recur, but the fixture stays as a named regression
/// test for the general "port coordinate coincides with an obstacle boundary" shape).
#[test]
fn orthogonal_cross_subgraph_edge_never_draws_a_diagonal_segment() {
    let src = "flowchart LR\n  subgraph C[クライアント]\n    CLI[CLI]\n    UI[ブラウザ UI]\n    EX[エディタ拡張]\n  end\n  subgraph G[クラウド]\n    API[API ゲート]\n    Q[ジョブキュー]\n    W[ジョブ実行系]\n    SB[解析サンドボックス]:::exec\n    subgraph S[保存層]\n      DB[メタデータ DB]:::data\n      AR[成果物保管]:::data\n    end\n  end\n  subgraph E[外部]\n    ID[認証基盤]\n    LLM[モデル API]:::model\n    GIT[コード置き場]:::exec\n    PAY[決済ページ]\n  end\n  CLI --> API\n  UI -->|HTTPS| API\n  EX --> API\n  API --> ID\n  API -->|投入| Q\n  Q -->|取り出し| W\n  API --> DB\n  W --> DB\n  W --> AR\n  W --> LLM\n  W -->|ツール実行| SB\n  SB --> GIT\n  CLI -.->|リンク| PAY\n  classDef data fill:#161b22,stroke:#3fb950,color:#e6edf3\n  classDef model fill:#161b22,stroke:#a371f7,color:#e6edf3\n  classDef exec fill:#161b22,stroke:#f85149,color:#e6edf3\n  linkStyle 0,1,2 stroke:#58a6ff\n  linkStyle 4,5 stroke:#d29922\n";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let api_id = d
        .edges
        .iter()
        .find(|e| e.from == "API" && e.to == "ID")
        .expect("API->ID must exist");
    for w in api_id.points.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        assert!(
            (a.x - b.x).abs() < AXIS_EPS || (a.y - b.y).abs() < AXIS_EPS,
            "API->ID must never draw a diagonal segment: {:?}-{:?} in {:?}",
            a,
            b,
            api_id.points
        );
    }
}

#[test]
fn zzdebug_fence3_structural_table() {
    let src = "flowchart LR\n  F[ファイル] --> C{設定のルール}\n  C -->|テキスト| T[窓読み]\n  C -->|コード| S[構文強調]\n  C -->|Markdown| MD[ブロックモデル]\n  C -->|CSV / TSV| TB[表]\n  C -->|画像| IM[デコード]\n  C -->|PDF| PD[ページ描画]\n  C -->|SVG| SV[usvg]\n  C -->|動画| VD[キーフレーム]\n  C -->|書庫| AR[一覧]\n  C -->|なし| NA[プレビュー不可]\n  MD --> MM[mermaid]\n  MD --> MA[数式]\n  MM --> RS[ラスタライズ]\n  MA --> RS\n  SV --> RS\n  PD --> RS\n  IM --> FIT[セルに合わせる]\n  RS --> FIT\n  VD --> FIT\n  FIT --> K{端末}\n  K -->|kitty| KT[圧縮転送]\n  K -->|sixel / iTerm2| RI[画像プロトコル]\n  K -->|それ以外| HB[ハーフブロック]\n  classDef pix fill:#132a3a,stroke:#1f6feb,color:#c9d1d9\n  classDef txt fill:#12291c,stroke:#2da44e,color:#c9d1d9\n  class IM,PD,SV,VD,MM,MA,RS,FIT,KT,RI,HB pix\n  class T,S,MD,TB,AR txt\n  style NA fill:#2d2418,stroke:#d4a017,color:#c9d1d9\n";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    for n in &d.nodes {
        eprintln!(
            "NODE {:>4}: center=({:.1},{:.1}) size=({:.1}x{:.1}) box=[{:.1}..{:.1}]x[{:.1}..{:.1}]",
            n.id,
            n.center.x,
            n.center.y,
            n.size.w,
            n.size.h,
            n.center.x - n.size.w / 2.0,
            n.center.x + n.size.w / 2.0,
            n.center.y - n.size.h / 2.0,
            n.center.y + n.size.h / 2.0,
        );
    }
    for e in &d.edges {
        let bends = e.points.len().saturating_sub(2);
        eprintln!(
            "{:>3} -> {:<3} bends={} start={:?} end={:?} color={:?} points={:?}",
            e.from,
            e.to,
            bends,
            e.points.first(),
            e.points.last(),
            e.style.as_ref().and_then(|s| s.stroke.clone()),
            e.points,
        );
    }
}

/// §10-3's "ファン列内の並び順" (`docs/FEATURE-MERMAID-RENDERER.md`, `mod.rs`'s own `regroup_fan_
/// lanes` doc) — the mechanical rule reverse-engineered from `3a`'s reference geometry, pinned on
/// a small, hand-built fan deliberately shaped to make every clause of the rule independently
/// checkable, rather than only on `samples/mermaid.ja.md`'s 20-node fence (the real-world source
/// the rule was derived from, covered separately by `orthogonal_settings_rules_sample_*`):
///
/// `S` fans out to six children, in declaration order `A`(red), `P`(blue), `B`(red), `T`(red,
/// *and* the sole one with its own further out-edge — `T -> D` — so it is the unambiguous trunk
/// regardless of any geometric tie-break), `Q`(blue), `Z`(no `class` at all, only `classDef`
/// members get one). The rule (trunk → group index `len/2`; group by resolved colour, group order
/// by first declared member; classless always last) predicts, and this test pins, the exact order
/// `A, B, P, T, Q, Z`: the two same-coloured groups (`red` first, since `A` is declared before
/// `P`) supply `regroup_fan_lanes`'s own `rest` list `[A, B, P, Q, Z]` (classless `Z` pushed to
/// the tail regardless of its own declaration position, third of six), and inserting `T` at
/// `6 / 2 = 3` splits it `[A, B, P]` / `[T]` / `[Q,Z]`.
///
/// Each assertion below is aimed at one clause a mutation of the rule would silently break:
/// swapping the group-order key (first-declared member) would put `P` ahead of `A`; dropping the
/// "同色内は安定" clause (or resorting a group by id instead of declaration order) would put `B`
/// ahead of `A` or `Q` ahead of `P`; treating the classless rule as "first" instead of "last"
/// would move `Z` to the front; using `len/2` rounded the other way, or not special-casing the
/// trunk's own slot at all, would misplace `T`; and dropping [`orthogonal::evict`]'s own anchor
/// fix (this module's own `orthogonal_branch_and_merge_edges_share_a_face_one_aligned_one_bends_
/// twice`, the general-purpose regression for the same fix) would leave `S -> T` bent instead of
/// the straight, exactly-centred line the last two assertions pin.
#[test]
fn regroup_fan_lanes_groups_by_colour_centres_the_trunk_and_pushes_classless_outermost() {
    let src = "flowchart LR\n  S --> A\n  S --> P\n  S --> B\n  S --> T\n  S --> Q\n  S --> Z\n  \
               T --> D\n  classDef red stroke:#ff0000\n  classDef blu stroke:#0000ff\n  \
               class A,B red\n  class P,Q blu\n";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    assert_no_edge_crosses_its_own_endpoint("regroup-minimal", &d);
    check_nodes_do_not_overlap("regroup-minimal", &d);

    let mut kids: Vec<(&str, f64)> = d
        .edges
        .iter()
        .filter(|e| e.from == "S")
        .map(|e| (e.to.as_str(), d.node(&e.to).unwrap().center.y))
        .collect();
    kids.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let order: Vec<&str> = kids.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        order,
        vec!["A", "B", "P", "T", "Q", "Z"],
        "colour-grouped, trunk-centred, classless-outermost order: {order:?}"
    );

    // The trunk edge is `classify`'s `aligned` shape (a straight 2-point line), landing exactly
    // on S's own centre — `evict`'s anchor fix (§10-3's "幹7区間の曲げ0") is what makes this land
    // on 0 rather than `PORT_SPACING/2` off it despite six claims (an even count) on S's face.
    let st = d
        .edges
        .iter()
        .find(|e| e.from == "S" && e.to == "T")
        .expect("S->T must exist");
    let s_node = d.node("S").expect("S must exist");
    assert_eq!(
        st.points.len(),
        2,
        "S->T is the trunk — a straight 2-point line: {:?}",
        st.points
    );
    assert!(
        (st.points[0].y - s_node.center.y).abs() < 1e-6,
        "S->T exits exactly on S's own centre: {:?} vs {}",
        st.points[0],
        s_node.center.y
    );
}

/// §10-3 item 11 (`docs/FEATURE-MERMAID-RENDERER.md`) — `regroup_fan_lanes` widened from "at least
/// three members" to "at least two": `samples/mermaid.ja.md`'s own `ブロックモデル` (`MD`) fans out
/// to exactly two children, `mermaid`(`MM`, the trunk `MD->MM` selects — its own further edge
/// `MM->RS` is what makes it the unambiguous chain continuation) and `数式`(`MA`, the sole non-
/// trunk member), and `3a`'s own reference geometry puts `数式` *above* `mermaid`, not below —
/// pinned directly on the real corpus source rather than only inferred from the `SETTINGS_RULES_
/// SAMPLE` dump this fix was found against. Before this fix, a plain two-way fan was never
/// eligible for `regroup_fan_lanes` at all (`docs/STATUS.md`'s own ★未修正 entry — "`regroup_fan_
/// lanes` は要素3以上のファンのみ対象" — before this fix, `MA` fell wherever `align_straight_
/// lanes`'s own overlap sweep happened to leave it, landing *below* `MM` instead).
#[test]
fn orthogonal_settings_rules_sample_block_model_fan_puts_math_above_the_mermaid_trunk() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let mm = d.node("MM").expect("MM (mermaid) must exist");
    let ma = d.node("MA").expect("MA (数式) must exist");
    let mm_mm = d
        .edges
        .iter()
        .find(|e| e.from == "MD" && e.to == "MM")
        .expect("MD->MM must exist");
    assert_eq!(
        mm_mm.points.len(),
        2,
        "MD->MM is the trunk — a straight 2-point line: {:?}",
        mm_mm.points
    );
    assert!(
        ma.center.y < mm.center.y,
        "数式(MA) must sit above the mermaid(MM) trunk under LR (smaller y): MA.y={}, MM.y={}",
        ma.center.y,
        mm.center.y
    );
}

/// A minimal, hand-built counterpart to the corpus pin above: a two-way fan with no colour classes
/// at all (`3a`'s own `1b` case — a plain "image / none" split, `docs/FEATURE-MERMAID-RENDERER.md`
/// §10-3 item 11's own "1b: 画像=上・なし=下"), confirming the rule holds independent of the
/// colour-grouping machinery `regroup_fan_lanes_groups_by_colour_centres_the_trunk_and_pushes_
/// classless_outermost` already pins for larger fans: with exactly one non-trunk member and no
/// class on either child, `rest` still has exactly one entry, and `before = len / 2 = 1` still
/// puts it ahead of the trunk.
#[test]
fn orthogonal_synthetic_two_way_fan_puts_the_non_trunk_member_before_the_trunk() {
    let src = "flowchart LR\n  S --> A\n  S --> T\n  T --> D\n";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let a = d.node("A").expect("A must exist");
    let t = d.node("T").expect("T must exist");
    let st = d
        .edges
        .iter()
        .find(|e| e.from == "S" && e.to == "T")
        .expect("S->T must exist");
    assert_eq!(
        st.points.len(),
        2,
        "S->T is the trunk (T continues the chain via T->D) — a straight 2-point line: {:?}",
        st.points
    );
    assert!(
        a.center.y < t.center.y,
        "the sole non-trunk member (A) must sit before the trunk (T), same as the fan's own \
         `before = len / 2 = 1` split: A.y={}, T.y={}",
        a.center.y,
        t.center.y
    );
}

/// §10-3 item 2's own outside-in nesting (`3a`'s reference geometry, `orthogonal::route_fan_lane`'s
/// own doc), stated as the invariant it exists to buy: **no two fan-lane siblings on the same
/// source face ever cross each other's own segments** — not just "no node gets pierced" (the
/// pre-existing corpus invariants), a stronger, edge-vs-edge geometric claim only this fan region
/// can violate (an ordinary branch/merge pair never shares a face with more than one sibling).
///
/// Before the outside-in correction, the plain "step scales with distance from centre" formula drew
/// the *opposite* nesting: a sibling closer to the face's own centre got the *shallower* bend, so
/// its short stub sat inside a farther-out sibling's own bend lane and vice versa — on this exact
/// fixture (`設定のルール`'s ten-way fanout), `ページ描画`(`PD`, one step in from the centre on the
/// five-member half) and `一覧`(`AR`, two steps out) crossed: `AR`'s stub (`y=345.7`,
/// `x∈[319.75,359.75]`) sat inside the y-range of `PD`'s own vertical run (`y∈[308.3,361.7]`) at an
/// x (`327.75` under the old formula) still short of `PD`'s own bend depth — a real crossing, not a
/// hypothetical one, confirmed by hand against a dump of the pre-fix route before this test was
/// written. §10-3 item 5's "主辺どうしの交差は水平側が譲る" then fired on it, cutting a
/// [`orthogonal::CROSSING_GAP`]-px notch out of the horizontal stub — the "ファン根元のスタブが細切
/// れになる" symptom the user reported (`設定のルール` reads far busier than `3a`'s own reference).
/// The second half of this test (`gaps.is_empty()`) pins that the fix removes the crossing itself,
/// not just its visual symptom: once no two siblings cross, rule 5 never has anything to cut here.
#[test]
fn orthogonal_settings_rules_sample_fan_lane_siblings_never_cross() {
    let src = SETTINGS_RULES_SAMPLE;
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let fan: Vec<&PlacedEdge> = d.edges.iter().filter(|e| e.from == "C").collect();
    assert_eq!(
        fan.len(),
        10,
        "設定のルール's own ten-way fanout must still have all ten children: {:?}",
        fan.iter().map(|e| e.to.as_str()).collect::<Vec<_>>()
    );

    for e in &fan {
        assert!(
            e.gaps.is_empty(),
            "C->{}: a fan-lane sibling's own stub must never be cut by rule 5's crossing gap \
             (that only fires when two siblings actually cross) — gaps: {:?}, points: {:?}",
            e.to,
            e.gaps,
            e.points
        );
    }

    for i in 0..fan.len() {
        for j in (i + 1)..fan.len() {
            let (a, b) = (fan[i], fan[j]);
            for wa in a.points.windows(2) {
                for wb in b.points.windows(2) {
                    assert!(
                        orthogonal::segment_crossing(wa, wb).is_none(),
                        "C->{} crosses C->{}'s own segment ({:?}-{:?} vs {:?}-{:?}) — the two fan \
                         siblings' own routes must never cross: {} points={:?}, {} points={:?}",
                        a.to,
                        b.to,
                        wa[0],
                        wa[1],
                        wb[0],
                        wb[1],
                        a.to,
                        a.points,
                        b.to,
                        b.points
                    );
                }
            }
        }
    }
}

/// A minimal synthetic fixture with the same shape as [`orthogonal_settings_rules_sample_fan_lane_
/// siblings_never_cross`]'s own real-diagram pin, deliberately unbalanced (5 above the trunk, 4
/// below — `3a`'s own five-vs-four split, `MD` landing dead on `C`'s own centre line as the tenth,
/// aligned member) rather than a tidy symmetric fan, so this still exercises the "nest against the
/// *larger* half's own outer rank" rule (the per-face loop in `evict` builds this, `Eviction::
/// fan_step`'s own doc) a perfectly even fixture never would — the two closest-to-centre siblings on
/// *either* side of an unbalanced face (`MD` above, `B1` below) must bend to the identical depth,
/// the same "306 と 338 が同じ 348 を使う" behaviour `3a`'s own reference shows.
///
/// Every out-edge carries a label (`|ラベルA1|` etc): §10-1 item 3's own minimum-segment-length rule
/// is what gives real diagrams the flow-axis room a deep nested bend needs (`3a`'s own `設定のルー
/// ル`'s ten-way fanout has labels on every edge too) — without one, `C`'s ten unlabelled leaf
/// children all land at the bare `RANK_SEP` (50px) away, too close for the busiest half's deepest
/// bend (`PORT_CLEARANCE * (5 + 1)` = 48px) to clear before reaching the target, which trips
/// `route_fan_lane`'s own defensive "past the target" guard into its plain-midpoint fallback —
/// a real, if narrow, gap in that fallback's own no-crossing guarantee (`route_fan_lane`'s own doc:
/// the fallback is not built with §10-3's nesting in mind at all), but a different, already-
/// documented limitation from the one this test exists to pin, so the fixture is built to stay clear
/// of it rather than conflate the two.
#[test]
fn orthogonal_synthetic_ten_way_fan_lane_siblings_never_cross() {
    let src = "flowchart LR\n  Z --> C{cond}\n  \
               C -->|ラベルA1| A1\n  C -->|ラベルA2| A2\n  C -->|ラベルA3| A3\n  \
               C -->|ラベルA4| A4\n  C -->|ラベルA5| A5\n  \
               C -->|ラベルMD| MD\n  \
               C -->|ラベルB1| B1\n  C -->|ラベルB2| B2\n  C -->|ラベルB3| B3\n  C -->|ラベルB4| B4";
    let d = laid_out_flow(src, "basis", "konoma-orthogonal");
    let fan: Vec<&PlacedEdge> = d.edges.iter().filter(|e| e.from == "C").collect();
    assert_eq!(
        fan.len(),
        10,
        "C's own ten-way fanout: {:?}",
        fan.iter().map(|e| e.to.as_str()).collect::<Vec<_>>()
    );

    for e in &fan {
        assert!(
            e.gaps.is_empty(),
            "C->{}: no fan-lane sibling may carry a crossing gap: {:?}",
            e.to,
            e.gaps
        );
    }
    for i in 0..fan.len() {
        for j in (i + 1)..fan.len() {
            let (a, b) = (fan[i], fan[j]);
            for wa in a.points.windows(2) {
                for wb in b.points.windows(2) {
                    assert!(
                        orthogonal::segment_crossing(wa, wb).is_none(),
                        "C->{} crosses C->{}: {} points={:?}, {} points={:?}",
                        a.to,
                        b.to,
                        a.to,
                        a.points,
                        b.to,
                        b.points
                    );
                }
            }
        }
    }
}
