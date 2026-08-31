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

/// The `CORPUS` entries orthogonal routing's stage-1 invariants can be checked over: every case
/// that draws no subgraph.
///
/// A cluster-boundary edge is explicitly out of scope for stage 1 — §10-2's later stages own it —
/// so it keeps drawing through the ordinary spline path even under `"konoma-orthogonal"`
/// (`lay_out_spec`'s own comment on the `matches!(tail, edges::End::Node(..))` guard). Including a
/// `subgraph-*` entry here would make "every segment is axis-parallel" state something the
/// renderer does not actually promise yet, so this filters them out by name rather than by
/// guessing which ones happen to pass.
fn orthogonal_corpus() -> Vec<(&'static str, &'static str)> {
    CORPUS
        .iter()
        .copied()
        .filter(|(name, _)| !name.starts_with("subgraph"))
        .collect()
}

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
///   for, not one this stage's ≤2 cap was ever meant to hold to.
fn orthogonal_dag_corpus() -> Vec<(&'static str, &'static str)> {
    const UNCAPPED: &[&str] = &[
        "branch",
        "cjk",
        "long-edge",
        "left-right",
        "self-loop",
        "amp-chain",
    ];
    orthogonal_corpus()
        .into_iter()
        .filter(|(name, _)| !UNCAPPED.contains(name))
        .collect()
}

/// Every segment of every routed edge in `d`, as `(a, b)` pairs.
fn edge_segments(d: &Diagram) -> Vec<(Point, Point)> {
    let mut out = Vec::new();
    for e in &d.edges {
        for w in e.points.windows(2) {
            out.push((w[0].clone(), w[1].clone()));
        }
    }
    out
}

const AXIS_EPS: f64 = 1e-6;

/// The default path (`"splines"`) must not move by even one byte — `[ui] mermaid_routing` is a
/// brand new opt-in axis, and `docs/FEATURE-MERMAID-RENDERER.md` §10-2 states the same "既定の
/// 見え方は1バイトも変えない" rule `[ui] mermaid_curve` was held to. `render_flow(..., curve,
/// "splines")` is required to reproduce `render_curve(..., curve)` exactly, across several curves
/// and the whole corpus.
#[test]
fn orthogonal_splines_routing_reproduces_render_curve_byte_for_byte() {
    for (name, src) in CORPUS {
        for curve in ["basis", "linear", "step", "monotoneX"] {
            let a = render_curve(src, "dark", curve)
                .unwrap_or_else(|e| panic!("{name}/{curve}: render_curve must render: {e}"));
            let b = render_flow(src, "dark", curve, "splines")
                .unwrap_or_else(|e| panic!("{name}/{curve}: render_flow must render: {e}"));
            assert_eq!(
                a, b,
                "{name}/{curve}: render_flow(..., \"splines\") must match render_curve(...) exactly"
            );
        }
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
    for (name, src) in orthogonal_corpus() {
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

/// A decision node's two outgoing edges (branch) and a merge node's two incoming edges (merge)
/// each bend exactly once — a 3-point polyline, per §10-1 item 1's "分岐/合流は曲げ1回".
#[test]
fn orthogonal_branch_and_merge_edges_bend_exactly_once() {
    let branch = laid_out_flow(
        "flowchart LR\n  A --> B{cond}\n  B --> C\n  B --> D",
        "basis",
        "konoma-orthogonal",
    );
    for target in ["C", "D"] {
        let e = branch
            .edges
            .iter()
            .find(|e| e.from == "B" && e.to == target)
            .unwrap_or_else(|| panic!("edge B->{target} must exist"));
        assert_eq!(e.points.len(), 3, "branch edge B->{target}: {:?}", e.points);
    }

    let merge = laid_out_flow(
        "flowchart LR\n  A --> C\n  B --> C\n  C --> D",
        "basis",
        "konoma-orthogonal",
    );
    for source in ["A", "B"] {
        let e = merge
            .edges
            .iter()
            .find(|e| e.from == source && e.to == "C")
            .unwrap_or_else(|| panic!("edge {source}->C must exist"));
        assert_eq!(e.points.len(), 3, "merge edge {source}->C: {:?}", e.points);
    }
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

/// Only a flowchart's own edges read `[ui] mermaid_routing` — a state/class/ER diagram draws
/// identically whether `"splines"` or `"konoma-orthogonal"` is asked for, because each of their
/// own `spec_of` always sets `GraphSpec::routing` to `Routing::Splines` regardless of the caller's
/// setting (`docs/FEATURE-MERMAID-RENDERER.md` §10-2: "影響は curve と同じく flowchart のみ").
/// Same shape as `non_flowchart_diagrams_ignore_mermaid_curve` above, through the real dispatcher.
#[test]
fn non_flowchart_diagrams_ignore_mermaid_routing() {
    use crate::preview::markdown::mermaid_to_svg_flow;
    let cases = [
        "stateDiagram-v2\n  [*] --> A\n  A --> B\n  B --> [*]",
        "classDiagram\n  A --|> B",
        "erDiagram\n  A ||--o{ B : has",
    ];
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

/// Every routed edge's two endpoints sit exactly [`orthogonal::PORT_INSET`] px **outside** the
/// node each one meets — never touching, let alone crossing into, the real geometric boundary —
/// and the segment that reaches each one is perpendicular to whichever face it lands on. §10-1
/// item 1's last bullet: "矢尻…の先端はノード枠の外縁から1px離す＝端点は枠座標の2px手前", and item
/// 1's "出入りは常に辺へ垂直".
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
    for (name, src) in orthogonal_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
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
                let Some(node) = d.node(node_id) else {
                    continue;
                };
                let (l, t, r, b) = node.bounds();
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

/// A real 9-way fan-in (`A`..`I` all merge into `Z`) is not evenly spread by dagre: `E`, whose x
/// dagre happened to place exactly under `Z`, is the one **aligned** edge (0 bends, `Z`'s Top
/// face); the other eight split 4-and-4 across `Z`'s Left and Right faces by which side of `Z`
/// their own x falls on. Both four-edge faces need `(4-1)*16 + 2*8 = 64px` of flat run — more
/// than `Z`'s natural single-line height of 45.4px — so `Z` grows to fit them, on the axis
/// (height, since Left/Right faces run along it) that demand actually asks for.
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
    assert_eq!(
        z_ortho.size.h, 64.0,
        "Z's height must grow to exactly (4-1)*16 + 2*8 = 64px — the busiest face's requirement"
    );
    // Growth is per-axis: nothing asked Z's width to grow (only its Left/Right faces were
    // crowded, which is a height requirement), so it must be unchanged from the splines pass.
    assert_eq!(
        z_ortho.size.w, z_splines.size.w,
        "no face asked Z's width to grow, so it must not have"
    );

    let (_, z_top, _, _) = z_ortho.bounds();
    let left_face_x = z_ortho.bounds().0 - orthogonal::PORT_INSET;
    let right_face_x = z_ortho.bounds().2 + orthogonal::PORT_INSET;

    let mut left_ys = Vec::new();
    let mut right_ys = Vec::new();
    let mut aligned_count = 0;
    for e in &ortho.edges {
        if e.to != "Z" {
            continue;
        }
        let bends = e.points.len().saturating_sub(2);
        let last = e.points.last().unwrap();
        if (last.y - (z_top - orthogonal::PORT_INSET)).abs() < 1e-6 {
            // Landed on Z's Top face: must be the one aligned (E), zero bends.
            assert_eq!(bends, 0, "the Top-face edge must be the aligned one: {e:?}");
            assert_eq!(
                e.from, "E",
                "E is the source dagre happened to align under Z: {e:?}"
            );
            aligned_count += 1;
        } else if (last.x - left_face_x).abs() < 1e-6 {
            assert_eq!(
                bends, 1,
                "a merge onto Z's Left face bends exactly once: {e:?}"
            );
            left_ys.push(last.y);
        } else if (last.x - right_face_x).abs() < 1e-6 {
            assert_eq!(
                bends, 1,
                "a merge onto Z's Right face bends exactly once: {e:?}"
            );
            right_ys.push(last.y);
        } else {
            panic!(
                "edge {}->Z landed on none of Z's three occupied faces: {e:?}",
                e.from
            );
        }
    }
    assert_eq!(
        aligned_count, 1,
        "exactly one edge must be the aligned E->Z"
    );
    assert_eq!(left_ys.len(), 4, "four merges must share Z's Left face");
    assert_eq!(right_ys.len(), 4, "four merges must share Z's Right face");

    for ys in [&mut left_ys, &mut right_ys] {
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        // 16px apart, symmetric about Z's own centre.
        for w in ys.windows(2) {
            assert!(
                (w[1] - w[0] - orthogonal::PORT_SPACING).abs() < 1e-6,
                "ports on one face must be exactly 16px apart: {ys:?}"
            );
        }
        let mid = (ys[0] + ys[3]) / 2.0;
        assert!(
            (mid - z_ortho.center.y).abs() < 1e-6,
            "four ports must be symmetric about the face's own centre: {ys:?} vs {}",
            z_ortho.center.y
        );
        // Every port at least PORT_CLEARANCE from a corner.
        let half_h = z_ortho.size.h / 2.0;
        for &y in ys.iter() {
            let from_center = (y - z_ortho.center.y).abs();
            assert!(
                half_h - from_center >= orthogonal::PORT_CLEARANCE - 1e-6,
                "port {from_center}px from centre must clear the corner by \
                 {}px (half-height {half_h}): {ys:?}",
                orthogonal::PORT_CLEARANCE
            );
        }
    }
}

/// A face with room to spare must not grow — the other half of the growth test above. `Z` here
/// receives only two edges, both merges landing on its Left face (`B` and `C`, both left of `Z`);
/// two ports need `(2-1)*16 + 2*8 = 32px`, well inside `Z`'s natural 45.4px height, so nothing
/// about `Z`'s size may differ between `"splines"` and `"konoma-orthogonal"`.
#[test]
fn orthogonal_roomy_face_does_not_grow() {
    let src = "flowchart TD\n  A --> Z\n  B --> Z\n  C --> Z";
    let splines = laid_out_curve(src, "basis");
    let ortho = laid_out_flow(src, "basis", "konoma-orthogonal");
    let z_splines = splines.node("Z").expect("Z must exist");
    let z_ortho = ortho.node("Z").expect("Z must exist");
    assert_eq!(
        z_ortho.size, z_splines.size,
        "a face that already fits its ports must leave the node exactly as it was"
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

/// §10-1 item 1's collision fix, run over the whole cycle-free corpus ([`orthogonal_dag_corpus`]
/// — see its own doc for why a back edge and `amp-chain`'s known collision-fallback edge are
/// excluded: both are explicitly exempt from any bend cap *and* from any collision guarantee,
/// since a real perimeter route is still stage 5's job). This is stage 3's headline invariant.
#[test]
fn orthogonal_no_segment_crosses_a_foreign_node_across_the_dag_corpus() {
    for (name, src) in orthogonal_dag_corpus() {
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
    let src = "flowchart LR\n  F[ファイル] --> C{設定のルール}\n  \
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
}

/// Every rank still holds no overlapping node after lane alignment moved some of them — §10-1
/// item 1's "整列でノードを動かした結果…重なった側を押し出して従来の最小間隔を維持する". Checked
/// with the exact same box-overlap assertion [`invariant_nodes_do_not_overlap`] states for the
/// splines path, now over the whole corpus under `"konoma-orthogonal"`.
#[test]
fn orthogonal_lane_alignment_never_leaves_nodes_overlapping() {
    for (name, src) in orthogonal_corpus() {
        let d = laid_out_flow(src, "basis", "konoma-orthogonal");
        check_nodes_do_not_overlap(name, &d);
    }
}

/// §10-1 item 2's "直進レーンを挟んで上下（左右）に分かれる辺は…曲げ位置を共有して対称に". Two
/// merges into the same target, one from each side, already share a bend column by construction
/// (a merge's bend sits at the target's own flow coordinate — `orthogonal::bridge`'s `Flow, Cross`
/// case), which is exactly the "現状の合流形が自然に対称になるケース" the rule says to leave
/// alone: this states that directly, on a real diagram (`length`'s `B --> D`, `C --> D`, dumped
/// and confirmed before being pinned), rather than re-deriving it from the formula alone.
#[test]
fn orthogonal_merges_into_the_same_target_share_a_symmetric_bend_column() {
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
    assert_eq!(bd.points.len(), 3, "{:?}", bd.points);
    assert_eq!(cd.points.len(), 3, "{:?}", cd.points);
    // The bend (middle point) of each sits at the same flow coordinate (y, for TD) — D's own —
    // and the two are symmetric about D's own centre on the cross axis (x).
    assert!(
        (bd.points[1].y - cd.points[1].y).abs() < 1e-6,
        "the two merges must bend at the same row: {:?} vs {:?}",
        bd.points[1],
        cd.points[1]
    );
    let d_node = d.node("D").expect("D must exist");
    let b_offset = bd.points[1].x - d_node.center.x;
    let c_offset = cd.points[1].x - d_node.center.x;
    assert!(
        (b_offset + c_offset).abs() < 1e-6,
        "the two bends must be symmetric about D's own centre: {b_offset} vs {c_offset}"
    );
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
    let routed = orthogonal::route_flowchart(Direction::TopToBottom, &nodes, &edges);
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
        &edges,
        &mut points,
        &mut plates,
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
