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

use std::collections::HashSet;
use std::path::{Path as FsPath, PathBuf};

use resvg::usvg;

use super::clusters;
use super::edges;
use super::labels::Label;
use super::shapes::{self, Glyph, Size};
use super::svg::num;
use super::theme::{self, Theme};
use super::{lay_out, render, Diagram, PlacedCluster, PlacedEdge, PlacedNode, RenderError, MARGIN};
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
const CORPUS: &[(&str, &str)] = &[
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
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn laid_out(src: &str) -> Diagram {
    let chart = parse(src).unwrap_or_else(|e| panic!("corpus source must parse: {e}"));
    lay_out(&chart).unwrap_or_else(|e| panic!("corpus source must lay out: {e}"))
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
        shapes::outline(Glyph::Flow(Shape::Stadium), s),
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
        },
        PlacedCluster {
            id: "inner".to_string(),
            title: label("", 0.0),
            center: Point::new(320.0, 645.0),
            size: Size::new(200.0, 90.0),
            parent: Some("outer".to_string()),
            depth: 1,
            dashed: false,
        },
    ];

    Diagram {
        width: 1040.0,
        height: 720.0,
        nodes,
        edges,
        clusters,
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
