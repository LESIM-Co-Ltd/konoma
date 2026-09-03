//! Subgraph frames: the tree behind them, the room their titles need, and the cut that stops a
//! line that names a block from being drawn inside it.
//!
//! # Why a cluster is not a node
//!
//! Everything else this renderer draws is sized first and placed second. A frame is the other way
//! round: dagre decides where the members go, and the frame is whatever rectangle holds them.
//! [`layout`](crate::preview::mermaid::layout) already does that part — its compound support
//! (`set_parent`, the nesting graph, the border segments) puts a box around each parent and
//! reports it back as the parent node's `x`/`y`/`width`/`height`. So this module does not lay
//! anything out. It does three things dagre has no opinion about:
//!
//! 1. **Builds the tree.** The parser hands over a flat list of blocks whose `members` name their
//!    direct children by id, so who is inside whom has to be resolved before `set_parent` can be
//!    called. Blocks that turn out to contain no nodes at all are dropped here rather than
//!    reaching dagre, where a childless parent gets no border nodes and so no rectangle.
//! 2. **Makes the title fit.** mermaid's `clusters.js` widens the box to `bbox.width + padding`
//!    when the title is wider than the frame; konoma also checks the *height*, because the band
//!    dagre leaves above the topmost member is a fixed consequence of `ranksep` and a two-line
//!    title does not fit in it. Growing is done deepest-first so a block that grew is still inside
//!    its parent when the parent's turn comes.
//! 3. **Cuts the line at the frame.** mermaid's `cutPathAtIntersect`.
//!
//! # Edges that name a block
//!
//! `one --> two` is legal (§2-2) and dagre cannot route it: a compound parent has no rank, and
//! the ranking phase only ever sees leaves. mermaid solves this by *re-anchoring* — the edge is
//! laid out against a representative descendant (`findNonClusterChild`) and then cut back to the
//! frame's boundary at drawing time (`cutPathAtIntersect`), so the reader sees a line that starts
//! on the box. Under `[ui] mermaid_routing = "konoma-orthogonal"`, konoma's choice of
//! representative is directional (`docs/FEATURE-MERMAID-RENDERER.md` §10-5 S2/S4) — `mod.rs`'s
//! own caller passes [`AnchorRole::Declared`] instead for `Routing::Splines` (the pre-§10-5,
//! direction-blind "first descendant" rule, every diagram's default rendering), so this section
//! describes `Exit`/`Entry` only: a block that is an edge's own **source** is
//! anchored at a **sink** among its own descendants (no outgoing edge to another descendant,
//! `AnchorRole::Exit`) so the line reads as leaving from the block's actual exit, not from
//! whichever member happened to be declared first; a block that is an edge's own **target** is
//! anchored at a **source** (`AnchorRole::Entry`) for the mirror reason — which is exactly an
//! internal `[*]` start marker whenever the source language wrote one, since a state's own entry
//! has no in-block predecessor by construction (§2's own "開始マーカーの無い複合状態は流れ方向の
//! 先頭ノードを初期状態とみなす" falls out of the same rule with no marker at all: the flow's own
//! first member is the one with no in-block predecessor either way). A block with no qualifying
//! descendant at all — every one of them has an in-block successor or predecessor, which only an
//! internal cycle can produce — falls back to the first descendant in declaration order, the
//! pre-§10-5 behaviour. Upstream's extra rule (prefer a descendant that does not already share an
//! edge with the block) exists to keep its *own* recursive extraction from losing the anchor;
//! konoma never extracts, so the anchor cannot disappear.

use std::collections::{HashMap, HashSet};

use crate::preview::mermaid::flowchart::Flowchart;
use crate::preview::mermaid::layout::Point;

use super::shapes::Size;
use super::SpecBlock;

/// Gap between the frame's top edge and the top of the title's line block.
///
/// mermaid's is zero (`flowchart.subGraphTitleMargin.top`, schema default 0), which puts the
/// title's ink on the stroke. Two pixels is konoma's own; §0-1 is what allows the difference.
pub const TITLE_PAD_Y: f64 = 2.0;

/// Extra width a frame must have beyond its title, total across both sides.
///
/// `clusters.js`: `width = node.width <= bbox.width + node.padding ? bbox.width + node.padding :
/// node.width`, and a flowchart cluster's `padding` is `flowchart.padding` — the same 15 the
/// shapes use, which is why it is read from there rather than declared again.
pub const TITLE_PAD_X: f64 = super::shapes::PADDING;

/// Blank space between a frame's outline and whatever it holds, on every side.
///
/// `docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 4 ("枠の入れ子、枠とノード・外周レーンの余白は最低
/// 16px") and §10-5 S2 ("内部＝ストリップ下＋周囲 16px パディング") ask for the same number, so the
/// frame [`super::rebuild_frames`] derives under `Routing::Orthogonal` uses one constant for both.
/// The splines path never reads it: there a frame is dagre's own border-node rectangle, padded by
/// whatever half a `nodesep`/`ranksep` happens to be, and moving it would move every golden file.
pub const PAD: f64 = 16.0;

/// Stroke width of a frame. mermaid's `.cluster rect { stroke-width: 1px }` — deliberately
/// thinner than a node's outline so the frame reads as background rather than as another box.
pub const STROKE_WIDTH: f64 = 1.0;

/// Corner radius of a frame. mermaid leaves `rx` unset (square corners); konoma rounds it by the
/// same radius `A(text)` uses, so the two kinds of box look like they come from one drawing.
pub const CORNER_RADIUS: f64 = super::shapes::CORNER_RADIUS;

/// Which end of an edge [`Tree::anchor`] is resolving, for an `id` that names a block — the
/// module's own doc on "Edges that name a block" has the rule each variant picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorRole {
    /// `id` is an edge's own **source** (`id --> other`): anchor at a descendant with no
    /// outgoing edge to another descendant of the same block.
    Exit,
    /// `id` is an edge's own **target** (`other --> id`): anchor at a descendant with no
    /// incoming edge from another descendant of the same block.
    Entry,
    /// The pre-§10-5 rule, direction-blind: the first descendant in declaration order, full stop
    /// — `role`/`internal_edges` never disqualify anything. `mod.rs`'s own caller passes this for
    /// `Routing::Splines` (every diagram kind's default rendering, and every non-flowchart/
    /// non-state kind under any routing) so that path's own drawn geometry — the exact member a
    /// cluster-anchored edge lays out against — is untouched by `Exit`/`Entry`'s own directional
    /// search: `docs/FEATURE-MERMAID-RENDERER.md` §10-5's own acceptance criterion ("既定 splines
    /// の状態図は1バイト不変") and `snapshots/`'s own golden files hold the anchor fixed here to
    /// keep it.
    Declared,
}

/// One block of the subgraph tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    /// The subgraph id, written or generated.
    pub id: String,
    /// The title text, already decoded by the parser. Empty for a block with no title.
    pub title: String,
    /// Id of the enclosing block, if this one is nested.
    pub parent: Option<String>,
    /// 0 for a top-level block.
    pub depth: usize,
    /// Direct members that are nodes, in declaration order.
    pub member_nodes: Vec<String>,
    /// Direct members that are blocks, in declaration order.
    pub child_clusters: Vec<String>,
    /// Whether the frame is drawn with a dashed outline. A `subgraph` never is; one concurrent
    /// region of a state diagram's `--` always is, because it has no title to say what it is.
    pub dashed: bool,
}

/// The subgraph tree of one chart.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    clusters: Vec<Cluster>,
    index: HashMap<String, usize>,
}

impl Tree {
    /// Resolves the parser's flat list into a tree, dropping blocks that hold no nodes.
    ///
    /// `is_node` answers whether an id names a node the renderer is going to draw; the parser has
    /// already removed subgraph ids from the node list, so the two sets do not overlap.
    ///
    /// **A block with no nodes anywhere under it is dropped.** dagre gives a parent its rectangle
    /// through border nodes, and a parent with no children never gets any — the box would come
    /// back with no coordinates at all. Dropping it here means an edge that names it is dropped
    /// too (there is nothing to point at), which is the same honest degradation the renderer
    /// already applies to a node it could not place.
    pub fn build(chart: &Flowchart, is_node: impl Fn(&str) -> bool) -> Tree {
        let blocks: Vec<SpecBlock> = chart
            .subgraphs
            .iter()
            .map(|s| SpecBlock {
                id: s.id.clone(),
                title: s.title.clone(),
                members: s.members.clone(),
                dashed: false,
            })
            .collect();
        Tree::from_blocks(&blocks, is_node)
    }

    /// [`Tree::build`] over the language-neutral block list, which is what the renderer's own
    /// spec carries. Both diagram families reach the same tree through this.
    pub fn from_blocks(blocks: &[SpecBlock], is_node: impl Fn(&str) -> bool) -> Tree {
        let ids: HashSet<&str> = blocks.iter().map(|s| s.id.as_str()).collect();

        // Parent of each block: the block that lists it as a member.
        let mut parent_of: HashMap<&str, &str> = HashMap::new();
        for s in blocks {
            for m in &s.members {
                if ids.contains(m.as_str()) {
                    parent_of.insert(m.as_str(), s.id.as_str());
                }
            }
        }

        // Does this block hold a node, anywhere below it? Memoised over the tree.
        let by_id: HashMap<&str, &SpecBlock> = blocks.iter().map(|s| (s.id.as_str(), s)).collect();
        let mut holds: HashMap<&str, bool> = HashMap::new();
        for s in blocks {
            holds_a_node(s, &by_id, &is_node, &mut holds, 0);
        }

        let mut clusters: Vec<Cluster> = Vec::new();
        for s in blocks {
            if !holds.get(s.id.as_str()).copied().unwrap_or(false) {
                continue;
            }
            let parent = parent_of.get(s.id.as_str()).map(|p| p.to_string());
            let mut member_nodes = Vec::new();
            let mut child_clusters = Vec::new();
            for m in &s.members {
                if ids.contains(m.as_str()) {
                    if holds.get(m.as_str()).copied().unwrap_or(false) {
                        child_clusters.push(m.clone());
                    }
                } else if is_node(m) {
                    member_nodes.push(m.clone());
                }
            }
            clusters.push(Cluster {
                id: s.id.clone(),
                title: s.title.clone(),
                parent,
                depth: 0,
                member_nodes,
                child_clusters,
                dashed: s.dashed,
            });
        }

        let mut index: HashMap<String, usize> = HashMap::new();
        for (i, c) in clusters.iter().enumerate() {
            index.insert(c.id.clone(), i);
        }
        // A dropped parent leaves its survivors parentless. That cannot happen — a block that
        // holds a node has a parent that holds one too — but the tree is walked by following
        // `parent`, so a dangling id would loop forever rather than fail loudly.
        for c in &mut clusters {
            if let Some(p) = &c.parent {
                if !index.contains_key(p) {
                    c.parent = None;
                }
            }
        }
        let mut tree = Tree { clusters, index };
        for i in 0..tree.clusters.len() {
            tree.clusters[i].depth = tree.depth_of(i);
        }
        tree
    }

    /// Walks up from `i`, counting hops. Bounded by the number of blocks, so a cycle the parser
    /// could not produce cannot hang the renderer either.
    fn depth_of(&self, i: usize) -> usize {
        let mut depth = 0;
        let mut cur = self.clusters[i].parent.clone();
        while let Some(p) = cur {
            depth += 1;
            if depth > self.clusters.len() {
                return depth;
            }
            cur = self
                .index
                .get(&p)
                .and_then(|j| self.clusters[*j].parent.clone());
        }
        depth
    }

    /// Whether the chart has any frame to draw.
    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    /// The blocks, in the parser's order (a nested block before the one that contains it).
    pub fn iter(&self) -> std::slice::Iter<'_, Cluster> {
        self.clusters.iter()
    }

    /// Looks a block up by id.
    pub fn get(&self, id: &str) -> Option<&Cluster> {
        self.index.get(id).map(|i| &self.clusters[*i])
    }

    /// Whether `id` names a block of this tree.
    pub fn contains(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }

    /// Every node id under `id`, flattened depth-first through nested blocks —
    /// [`Cluster::member_nodes`] first, then each [`Cluster::child_clusters`] entry's own
    /// descendants in turn (mermaid's own `findNonClusterChild` traversal order, tie-break
    /// dropped — see the module docs). Borrowed straight out of `self`'s own [`Cluster`]s, not
    /// rebuilt, so [`Tree::anchor`] can hand one straight back to its caller.
    fn flatten_descendants<'a>(&'a self, id: &str) -> Vec<&'a str> {
        let mut out = Vec::new();
        if let Some(c) = self.get(id) {
            out.extend(c.member_nodes.iter().map(String::as_str));
            for child in &c.child_clusters {
                out.extend(self.flatten_descendants(child));
            }
        }
        out
    }

    /// The node an edge that names `id` should be laid out against.
    ///
    /// `None` when `id` is neither a node nor a block that holds one.
    ///
    /// For a plain node, `id` itself — `role`/`internal_edges` are never consulted. For a block,
    /// the module's own doc on "Edges that name a block": `role` picks which directional
    /// property (`Exit`'s sink, `Entry`'s source) the anchor must have, checked over
    /// `internal_edges` restricted to the pairs that are *both* [`flatten_descendants`] of this
    /// same block — an edge to or from outside the block says nothing about its own internal
    /// flow, so it is never allowed to disqualify a candidate. Falls back to the first descendant
    /// in declaration order when nothing qualifies (an internal cycle, where every descendant has
    /// both an in-block successor and predecessor).
    ///
    /// An `internal_edges` pair that itself names a *nested* block (`zz-design-4c`'s own
    /// `整形 --> 解析`, `解析` a block two levels under `処理`) is resolved through this same
    /// function, recursively, before it is tested against `members` — `解析 --> join_state` would
    /// otherwise disqualify nobody at all (`解析` is not itself a member-level node id, so it never
    /// matches anything in `members`), silently hiding the whole nested chain from the sink/source
    /// search and leaving `整形` a false, undisqualified "sink" one level up. Resolving `整形 -->
    /// 解析` to `整形 --> 解析`'s own `Entry` anchor first (`解析`'s own internal source, one level
    /// down) is what lets the disqualification propagate correctly through arbitrarily many levels
    /// of nesting — a written edge `u --> v` is never itself the thing that has to be a member,
    /// only wherever it actually resolves to. Bounded the same way [`flatten_descendants`] already
    /// is: a block can never nest inside itself (`Tree::build`'s own construction), so this cannot
    /// recurse forever.
    ///
    /// `is_node` is taken as a trait object rather than by `impl Fn` on purpose: [`flatten_
    /// descendants`] recurses, and a generic parameter would make each level of nesting a fresh
    /// instantiation wrapping the last, which rustc refuses at depth ("reached the recursion
    /// limit while instantiating").
    ///
    /// [`flatten_descendants`]: Tree::flatten_descendants
    pub fn anchor<'a>(
        &'a self,
        id: &'a str,
        is_node: &dyn Fn(&str) -> bool,
        role: AnchorRole,
        internal_edges: &[(String, String)],
    ) -> Option<&'a str> {
        self.anchor_bounded(id, is_node, role, internal_edges, 0)
    }

    /// [`Tree::anchor`]'s own body, with a recursion budget: every level [`anchor`]'s own nested
    /// resolution takes moves *toward* a leaf (a real node has no further recursion at all), so an
    /// ordinary, non-cyclic diagram never comes close to this bound. It exists only for an edge
    /// that (directly, or through several hops) names the very block currently being resolved —
    /// `[*] --> Active`, resolving `Active`'s own anchor, tries to resolve `Active` again as that
    /// edge's own target — or a genuine cross-reference cycle between two sibling blocks, neither
    /// of which any real `state`/`subgraph` nesting can produce (a block's members are always
    /// syntactically *inside* it — the module's own doc), but which a hand-built [`GraphSpec`]
    /// (this function has no way to tell those apart from a parsed one) is not prevented from
    /// constructing. Past the bound, an edge naming a block is simply left unresolved (`None`,
    /// filtered out below the same way an edge to/from outside `id` already is) rather than
    /// recursing forever — the same "give up cleanly rather than hang" contract
    /// [`holds_a_node`]'s own `depth > by_id.len()` bound keeps.
    ///
    /// [`GraphSpec`]: super::GraphSpec
    fn anchor_bounded<'a>(
        &'a self,
        id: &'a str,
        is_node: &dyn Fn(&str) -> bool,
        role: AnchorRole,
        internal_edges: &[(String, String)],
        depth: usize,
    ) -> Option<&'a str> {
        if is_node(id) {
            return Some(id);
        }
        if depth > self.clusters.len() {
            return None;
        }
        let descendants = self.flatten_descendants(id);
        let first = *descendants.first()?;
        if role == AnchorRole::Declared {
            return Some(first);
        }
        let members: HashSet<&str> = descendants.iter().copied().collect();
        let mut disqualified: HashSet<&str> = HashSet::new();
        for (u, v) in internal_edges {
            // An edge naming `id` itself is never internal to `id` (an external entry/exit edge,
            // resolved instead by whichever *other* block's own `anchor` call is looking at it) —
            // skipped without recursing, both as the common-case fast path and to keep the only
            // way `anchor_bounded` ever calls itself with the same `id` again from being this one,
            // most frequent shape of self-reference.
            if u == id || v == id {
                continue;
            }
            let u = if self.contains(u) {
                self.anchor_bounded(u, is_node, AnchorRole::Exit, internal_edges, depth + 1)
            } else {
                Some(u.as_str())
            };
            let v = if self.contains(v) {
                self.anchor_bounded(v, is_node, AnchorRole::Entry, internal_edges, depth + 1)
            } else {
                Some(v.as_str())
            };
            let (Some(u), Some(v)) = (u, v) else {
                continue;
            };
            if u == v || !members.contains(u) || !members.contains(v) {
                continue;
            }
            // A sink has no outgoing edge: `u` (this internal edge's own source, resolved) is
            // disqualified from being one. A source has no incoming edge: `v` (this internal
            // edge's own target, resolved) is disqualified. `Declared` already returned above —
            // matched defensively (never a crash, principle #3) rather than with `unreachable!`.
            if role == AnchorRole::Exit {
                disqualified.insert(u);
            } else {
                disqualified.insert(v);
            }
        }
        Some(
            descendants
                .into_iter()
                .find(|d| !disqualified.contains(d))
                .unwrap_or(first),
        )
    }

    /// [`Tree::flatten_descendants`], public: every node id under `id`, nested blocks flattened in.
    ///
    /// `mod.rs`'s own round-4 cluster passes (`docs/FEATURE-MERMAID-RENDERER.md` §10-5) need this to
    /// treat a block as one atomic unit — the whole set of nodes an ASAP re-rank or a lane shift has
    /// to move together — which is the same set [`Tree::anchor`] already searches for one member.
    pub fn descendants(&self, id: &str) -> Vec<&str> {
        self.flatten_descendants(id)
    }

    /// The **outermost** block `id` belongs to — for a node, the depth-0 ancestor of the block that
    /// lists it; for a block, its own depth-0 ancestor (itself, when it is already top level).
    /// `None` for a node no block holds, and for an id this tree knows nothing about.
    ///
    /// This is the "unit" the round-4 passes move as one body: a nested block never moves
    /// independently of the block that holds it, so the unit is always the depth-0 ancestor, not
    /// whichever block lists `id` directly. Bounded the same way [`Tree::depth_of`] is, so a
    /// dangling parent (which `Tree::from_blocks` already repairs) could not hang the walk.
    pub fn outermost(&self, id: &str) -> Option<&str> {
        // Every candidate is borrowed out of `self`, never out of `id`: the answer outlives the
        // caller's own string.
        let mut cur = self
            .get(id)
            .or_else(|| {
                self.clusters
                    .iter()
                    .find(|c| c.member_nodes.iter().any(|m| m == id))
            })
            .map(|c| c.id.as_str())?;
        for _ in 0..=self.clusters.len() {
            match self.get(cur).and_then(|c| c.parent.as_deref()) {
                Some(p) => cur = p,
                None => return Some(cur),
            }
        }
        Some(cur)
    }

    /// Whether an edge endpoint written as `id` has anything to do with the block `cluster_id`:
    /// it *is* the block, or it is a node or block somewhere under it.
    ///
    /// This is what separates "the line legitimately leaves this frame" from "the line has no
    /// business being in this frame at all", which is the invariant the tests state.
    pub fn touches(&self, id: &str, cluster_id: &str) -> bool {
        if id == cluster_id {
            return true;
        }
        // A node: find the block that lists it, then walk up.
        let mut owner: Option<&str> = None;
        for c in &self.clusters {
            if c.member_nodes.iter().any(|m| m == id) {
                owner = Some(c.id.as_str());
                break;
            }
        }
        let start = match owner {
            Some(o) => o,
            None if self.contains(id) => id,
            None => return false,
        };
        let mut cur = Some(start);
        let mut hops = 0;
        while let Some(v) = cur {
            if v == cluster_id {
                return true;
            }
            hops += 1;
            if hops > self.clusters.len() + 1 {
                return false;
            }
            cur = self.get(v).and_then(|c| c.parent.as_deref());
        }
        false
    }
}

/// Recursive half of [`Tree::build`]'s "does this block hold a node" question.
fn holds_a_node<'a>(
    s: &'a SpecBlock,
    by_id: &HashMap<&'a str, &'a SpecBlock>,
    is_node: &impl Fn(&str) -> bool,
    memo: &mut HashMap<&'a str, bool>,
    depth: usize,
) -> bool {
    if let Some(v) = memo.get(s.id.as_str()) {
        return *v;
    }
    // The parser cannot nest a block inside itself, but the walk is guarded anyway: this runs on
    // whatever a `.md` file contains.
    if depth > by_id.len() {
        return false;
    }
    let mut found = false;
    for m in &s.members {
        if let Some(child) = by_id.get(m.as_str()) {
            if holds_a_node(child, by_id, is_node, memo, depth + 1) {
                found = true;
            }
        } else if is_node(m) {
            found = true;
        }
    }
    memo.insert(s.id.as_str(), found);
    found
}

/// A frame's rectangle while it is still being adjusted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub left: f64,
    /// Top edge.
    pub top: f64,
    /// Right edge.
    pub right: f64,
    /// Bottom edge.
    pub bottom: f64,
}

impl Rect {
    /// From a centre and a size.
    pub fn new(center: &Point, size: Size) -> Rect {
        Rect {
            left: center.x - size.w / 2.0,
            top: center.y - size.h / 2.0,
            right: center.x + size.w / 2.0,
            bottom: center.y + size.h / 2.0,
        }
    }

    /// Centre of the rectangle.
    pub fn center(&self) -> Point {
        Point::new(
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        )
    }

    /// Width and height.
    pub fn size(&self) -> Size {
        Size::new(self.right - self.left, self.bottom - self.top)
    }

    /// Grows to contain `other`.
    pub fn absorb(&mut self, other: &Rect) {
        self.left = self.left.min(other.left);
        self.top = self.top.min(other.top);
        self.right = self.right.max(other.right);
        self.bottom = self.bottom.max(other.bottom);
    }

    /// Whether `p` is strictly inside.
    ///
    /// mermaid's `outsideNode` is `dx >= w || dy >= h`, so a point exactly on the boundary counts
    /// as outside — which is what makes a cut point stable: cutting a path and asking again
    /// whether it is inside gives "no", not "yes, by a rounding error".
    pub fn contains(&self, p: &Point) -> bool {
        p.x > self.left && p.x < self.right && p.y > self.top && p.y < self.bottom
    }
}

/// Where the segment `a -> b` leaves the rectangle, given that `a` is inside it and `b` is not.
///
/// Solved per edge of the rectangle rather than by mermaid's `intersection()`, which reconstructs
/// the point from four absolute differences and needs three special cases for its own division by
/// zero. The parametric form has one: a segment of zero length, which cannot occur here because
/// one end is inside and the other is not.
pub fn exit_point(a: &Point, b: &Point, rect: &Rect) -> Point {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let mut best: Option<f64> = None;
    let mut consider = |t: f64, on_axis: bool| {
        if (0.0..=1.0).contains(&t) && on_axis && best.is_none_or(|cur| t < cur) {
            best = Some(t);
        }
    };
    // Slack for the "is the crossing within the other axis's span" test: the crossing of a
    // corner is on both edges, and a bit of tolerance keeps it from being on neither.
    const EPS: f64 = 1e-9;
    if dx.abs() > 0.0 {
        for x in [rect.left, rect.right] {
            let t = (x - a.x) / dx;
            let y = a.y + t * dy;
            consider(t, y >= rect.top - EPS && y <= rect.bottom + EPS);
        }
    }
    if dy.abs() > 0.0 {
        for y in [rect.top, rect.bottom] {
            let t = (y - a.y) / dy;
            let x = a.x + t * dx;
            consider(t, x >= rect.left - EPS && x <= rect.right + EPS);
        }
    }
    match best {
        Some(t) => Point::new(a.x + t * dx, a.y + t * dy),
        // Unreachable for an inside/outside pair; falling back to the outside end keeps the
        // polyline monotone rather than inventing a point in the middle of nowhere.
        None => b.clone(),
    }
}

/// Replaces the run of `points` that lies inside `rect` at the *start* with the crossing itself.
///
/// mermaid's `cutPathAtIntersect`, restated: it walks from the outside in and stops at the first
/// point that is inside; this walks from the inside out and stops at the first point that is not,
/// which is the same cut and does not depend on which end of the array upstream happened to pass.
pub fn cut_start(points: &[Point], rect: &Rect) -> Vec<Point> {
    if points.is_empty() {
        return Vec::new();
    }
    let first_outside = points.iter().position(|p| !rect.contains(p));
    match first_outside {
        // Already starts outside the frame: there is nothing inside to remove.
        Some(0) => points.to_vec(),
        Some(i) => {
            let mut out = Vec::with_capacity(points.len() - i + 1);
            out.push(exit_point(&points[i - 1], &points[i], rect));
            out.extend_from_slice(&points[i..]);
            out
        }
        // The whole line is inside the frame. That means the *other* end of the edge is inside
        // this block too — `one --> A` with `A` a member of `one`, which is a diagram that asks
        // for a line from a box to something already in it. There is no crossing to find, so the
        // start is put on the boundary nearest the far end and the run inside is dropped, exactly
        // as it would be if a crossing had been found one point later.
        None => {
            let center = rect.center();
            let far = points.last().cloned().unwrap_or_else(|| center.clone());
            vec![exit_point(&center, &push_out(&far, rect), rect)]
        }
    }
}

/// [`cut_start`] from the other end.
pub fn cut_end(points: &[Point], rect: &Rect) -> Vec<Point> {
    let reversed: Vec<Point> = points.iter().rev().cloned().collect();
    let mut out = cut_start(&reversed, rect);
    out.reverse();
    out
}

/// `p`, moved far enough along the ray from the rectangle's centre to be outside it.
///
/// Used only by [`cut_start`]'s degenerate branch, where every point is inside and [`exit_point`]
/// needs an outside end to aim at.
fn push_out(p: &Point, rect: &Rect) -> Point {
    let c = rect.center();
    let (dx, dy) = (p.x - c.x, p.y - c.y);
    let len = dx.hypot(dy);
    let span = rect.size().w + rect.size().h + 1.0;
    if len < 1e-9 {
        return Point::new(c.x + span, c.y);
    }
    Point::new(c.x + dx / len * span, c.y + dy / len * span)
}
