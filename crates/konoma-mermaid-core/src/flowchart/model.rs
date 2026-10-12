//! The intermediate representation the flowchart parser produces.
//!
//! This is the whole output of stage 1b: structure, no geometry. Nothing here knows about
//! pixels, fonts or SVG — those arrive in later stages and read this model. Keeping the two
//! apart is what makes the parser testable on its own: every rule in
//! `docs/FEATURE-MERMAID-RENDERER.md` §2 can be checked by asserting on these values, without
//! a font database or a layout run.
//!
//! The shape of the model deliberately mirrors mermaid's own `FlowDB`, because that is the
//! only description of the language that is not a guess: node list in declaration order, edge
//! list in declaration order, and a flat list of subgraphs whose membership lists name their
//! *direct* children (nested subgraphs appear in their parent by id).

use std::collections::HashMap;
use std::fmt;

/// Layout direction of a chart or of one subgraph.
///
/// mermaid accepts five spellings; `TD` ("top down") is an alias of `TB` and is folded into it
/// on the way in, exactly as `FlowDB::setDirection` does, so downstream code only ever sees
/// four values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Direction {
    /// `TB` / `TD` — the default when the header names no direction.
    #[default]
    TopToBottom,
    /// `BT`
    BottomToTop,
    /// `LR`
    LeftToRight,
    /// `RL`
    RightToLeft,
}

impl Direction {
    /// The canonical mermaid spelling (`TD` normalises to `TB`).
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::TopToBottom => "TB",
            Direction::BottomToTop => "BT",
            Direction::LeftToRight => "LR",
            Direction::RightToLeft => "RL",
        }
    }

    /// Parses one of `TB` `TD` `BT` `RL` `LR`. Anything else is not a direction.
    pub fn parse(s: &str) -> Option<Direction> {
        match s {
            "TB" | "TD" => Some(Direction::TopToBottom),
            "BT" => Some(Direction::BottomToTop),
            "LR" => Some(Direction::LeftToRight),
            "RL" => Some(Direction::RightToLeft),
            _ => None,
        }
    }
}

/// The outline a node is drawn with.
///
/// mermaid has 53 named shapes as of v11.17; konoma draws a small family and folds the rest
/// onto the nearest member (`docs/FEATURE-MERMAID-RENDERER.md` §2-4: "53 形状すべての描き分けは
/// 不要"). The *names* are still all recognised — an unknown name is an error, because that is
/// the difference between a diagram konoma understood and one it guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Shape {
    /// `A[text]`, and every boxy member of the 53. Also the shape of a node that was only ever
    /// mentioned, which is why it is the default.
    #[default]
    Rect,
    /// `A(text)`
    RoundedRect,
    /// `A([text])`
    Stadium,
    /// `A[[text]]`
    Subroutine,
    /// `A[(text)]`
    Cylinder,
    /// `A((text))`
    Circle,
    /// `A(((text)))`
    DoubleCircle,
    /// `A{text}`
    Diamond,
    /// `A{{text}}`
    Hexagon,
    /// `A>text]`
    Odd,
    /// `A[/text\]` — base at the bottom.
    Trapezoid,
    /// `A[\text/]` — base at the top.
    InvTrapezoid,
    /// `A[/text/]` — parallelogram leaning right.
    LeanRight,
    /// `A[\text\]` — parallelogram leaning left.
    LeanLeft,
    /// `A@{ shape: text }` — a label with no outline at all.
    Text,
}

/// What is drawn at the end(s) of an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arrow {
    /// `---` — a plain line.
    None,
    /// `-->`
    Point,
    /// `--x`
    Cross,
    /// `--o`
    Circle,
    /// `<-->`
    DoublePoint,
    /// `x--x`
    DoubleCross,
    /// `o--o`
    DoubleCircle,
    /// The two halves of a `-- text -->` link disagreed (`x-- text -->`), which mermaid reports
    /// as `INVALID`. The edge still exists; it simply has no defensible arrow.
    Invalid,
}

/// How an edge's line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stroke {
    /// `---`
    Normal,
    /// `===`
    Thick,
    /// `-.-`
    Dotted,
    /// `~~~`
    Invisible,
    /// Start and end of a `-- text -->` link named different strokes.
    Invalid,
}

/// One node of the chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The identifier written in the source. Unique within a chart.
    pub id: String,
    /// The text drawn inside the shape, already decoded: `<br>` variants and literal `\n` have
    /// become real newlines and `#nn;` entities have become their characters. When the source
    /// never gave the node a label this is the id, which is what mermaid draws.
    pub label: String,
    /// The outline. Defaults to [`Shape::Rect`] for a node that was only ever mentioned.
    pub shape: Shape,
    /// Class names attached by `:::name` or by a `class` statement, in the order they were
    /// applied. konoma does not colour nodes yet; the names are carried so that it can.
    pub classes: Vec<String>,
    /// Declarations from a `style` statement naming this node, e.g. `fill:#f9f`.
    pub styles: Vec<String>,
}

/// One edge of the chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// `e1` for `A e1@--> B`, otherwise mermaid's generated `L_<from>_<to>_<n>`.
    pub id: String,
    /// True when the id came from the source rather than from the generator.
    pub user_defined_id: bool,
    /// Id of the source node (or of a subgraph used as an endpoint).
    pub from: String,
    /// Id of the target node (or of a subgraph used as an endpoint).
    pub to: String,
    /// Arrow heads.
    pub arrow: Arrow,
    /// Line style.
    pub stroke: Stroke,
    /// mermaid's "link length": the rank distance the layout should try to span. Derived from
    /// the number of dashes (`line.length - 1`), except for dotted links where it is the number
    /// of dots. Clamped to 10, as `FlowDB::addSingleLink` does.
    pub length: usize,
    /// The text on the edge, from either `-->|text|` or `-- text -->`, decoded like a node
    /// label. `None` when the link carried no text (which is not the same as an empty one).
    pub label: Option<String>,
    /// Class names attached by a `class` statement naming this edge's id.
    pub classes: Vec<String>,
}

/// One `subgraph` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subgraph {
    /// The identifier. Either written in the source (`subgraph one [Title]`, or `subgraph one`)
    /// or generated as `subGraph<n>`; see [`Subgraph::auto_id`].
    pub id: String,
    /// The text drawn on the block's frame. Empty for `subgraph` with no title at all.
    pub title: String,
    /// Ids of the *direct* members, in declaration order. A nested subgraph appears here by its
    /// own id, so the tree is read by following ids. A node that already belonged to an earlier
    /// subgraph is not repeated here (mermaid's `makeUniq`), which is what keeps a nested block's
    /// nodes out of its parent's list.
    pub members: Vec<String>,
    /// A `direction` statement inside the block, which overrides the chart direction for it.
    pub direction: Option<Direction>,
    /// Class names attached by a `class` statement naming this subgraph.
    pub classes: Vec<String>,
    /// True when [`Subgraph::id`] was generated because the source gave only a title.
    pub auto_id: bool,
}

/// One `classDef` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassDef {
    /// The class name; `default` is a legal name and means "every node".
    pub name: String,
    /// The declarations, split on `,`, e.g. `["fill:#f9f", "stroke:#333"]`.
    pub styles: Vec<String>,
}

/// Which links a `linkStyle` statement applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkStyleTarget {
    /// `linkStyle default ...`
    Default,
    /// `linkStyle 0,2 ...` — indices into [`Flowchart::edges`].
    Indices(Vec<usize>),
}

/// One `linkStyle` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkStyle {
    /// Which links it applies to.
    pub target: LinkStyleTarget,
    /// The declarations, split on `,`.
    pub styles: Vec<String>,
    /// The `interpolate <curve>` argument, if the statement carried one — the strongest link in
    /// the curve priority chain `render::spec_of`'s own doc states (`linkStyle <n> interpolate` >
    /// `linkStyle default interpolate` > `%%{init}%%`'s `flowchart.curve` > `[ui] mermaid_curve`).
    pub interpolate: Option<String>,
}

/// A parsed `flowchart` / `graph` diagram.
#[derive(Debug, Clone, Default)]
pub struct Flowchart {
    /// Chart direction; `TB` when the header named none.
    pub direction: Direction,
    /// Nodes in declaration order.
    pub nodes: Vec<Node>,
    /// Edges in declaration order. `linkStyle` indices refer to positions in this list.
    pub edges: Vec<Edge>,
    /// Subgraphs in *completion* order — a nested block is registered before the block that
    /// contains it, because that is when its `end` is reached. mermaid numbers its generated
    /// `subGraph<n>` ids off the same counter, so the order is part of the observable output.
    pub subgraphs: Vec<Subgraph>,
    /// `classDef` statements, in source order.
    pub class_defs: Vec<ClassDef>,
    /// `linkStyle` statements, in source order.
    pub link_styles: Vec<LinkStyle>,
    /// `title:` from a YAML front matter block, if there was one.
    pub title: Option<String>,
    /// `flowchart.curve` from an `%%{init}%%` directive, if the source had one and it named that
    /// key (`preprocess::init_flowchart_curve`). The chart-wide default `[ui] mermaid_curve`
    /// falls back to when this is `None`; a per-edge `linkStyle ... interpolate` still wins over
    /// both (`render::spec_of`'s own doc has the full priority order).
    pub curve: Option<String>,
    /// `accTitle:` — an accessibility title. Not drawn.
    pub acc_title: Option<String>,
    /// `accDescr:` or `accDescr { ... }` — an accessibility description. Not drawn.
    pub acc_descr: Option<String>,
    /// id -> index into [`Flowchart::nodes`].
    index: HashMap<String, usize>,
}

impl Flowchart {
    /// An empty chart with a direction and (optionally) a front matter title and an `%%{init}%%`
    /// `flowchart.curve`. Used by the parser; the id index is private, so a struct literal cannot
    /// be written from outside.
    pub(super) fn new(
        direction: Direction,
        title: Option<String>,
        curve: Option<String>,
    ) -> Flowchart {
        Flowchart {
            direction,
            title,
            curve,
            ..Flowchart::default()
        }
    }

    /// Looks a node up by id.
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.index.get(id).and_then(|i| self.nodes.get(*i))
    }

    /// Looks an edge up by id (generated or user-defined).
    pub fn edge(&self, id: &str) -> Option<&Edge> {
        self.edges.iter().find(|e| e.id == id)
    }

    /// Looks a subgraph up by id.
    pub fn subgraph(&self, id: &str) -> Option<&Subgraph> {
        self.subgraphs.iter().find(|s| s.id == id)
    }

    /// The ids of every node, in declaration order. Handy in tests and for the layout stage,
    /// which needs a stable iteration order to keep its output reproducible.
    pub fn node_ids(&self) -> Vec<&str> {
        self.nodes.iter().map(|n| n.id.as_str()).collect()
    }

    /// Registers a node if it is new and returns its index. Used by the parser only.
    pub(super) fn intern_node(&mut self, id: &str) -> usize {
        if let Some(i) = self.index.get(id) {
            return *i;
        }
        let i = self.nodes.len();
        self.nodes.push(Node {
            id: id.to_string(),
            label: id.to_string(),
            shape: Shape::Rect,
            classes: Vec::new(),
            styles: Vec::new(),
        });
        self.index.insert(id.to_string(), i);
        i
    }

    /// Rebuilds the id index after nodes have been removed. Used by the parser only.
    pub(super) fn reindex(&mut self) {
        self.index.clear();
        for (i, n) in self.nodes.iter().enumerate() {
            self.index.insert(n.id.clone(), i);
        }
    }

    pub(super) fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        let i = *self.index.get(id)?;
        self.nodes.get_mut(i)
    }

    pub(super) fn edge_mut(&mut self, id: &str) -> Option<&mut Edge> {
        self.edges.iter_mut().find(|e| e.id == id)
    }

    pub(super) fn subgraph_mut(&mut self, id: &str) -> Option<&mut Subgraph> {
        self.subgraphs.iter_mut().find(|s| s.id == id)
    }
}

/// Why a source could not be read as a flowchart.
///
/// The renderer's contract (`docs/FEATURE-MERMAID-RENDERER.md` §1) is that a failure means "do
/// not draw" — konoma falls back to the text diagram. So every variant here has to be a case
/// where drawing *something* would be worse than drawing nothing: a diagram of another kind, a
/// diagram with no nodes, an unclosed construct whose extent we would have to guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The source held no statements at all.
    Empty,
    /// The first keyword was not `flowchart` or `graph`. The renderer must not draw this: the
    /// current crate turns `venn-beta` into three boxes labelled "venn", "beta" and "sets".
    NotAFlowchart {
        /// The keyword that was found, for the message.
        header: String,
    },
    /// The header was a flowchart but the body declared no node. The current crate answers a
    /// bare `graph TD` with a 16x16 transparent SVG that gets blown up to full width.
    NoNodes,
    /// A shape bracket was opened and never closed, so the label's extent is a guess.
    UnclosedShape {
        /// 1-based line in the original source.
        line: usize,
    },
    /// A `"` was opened and never closed. In mermaid this silently swallows the rest of the
    /// diagram, which is exactly the kind of quiet wrong answer that must not be drawn.
    UnclosedString {
        /// 1-based line in the original source.
        line: usize,
    },
    /// `A@{ ... }` named a shape mermaid does not have, or spelled a real one with capitals or
    /// an underscore, both of which mermaid rejects outright.
    UnknownShape {
        /// The name as written.
        name: String,
        /// 1-based line in the original source.
        line: usize,
    },
    /// `A@{ ... }` was opened and never closed.
    UnclosedShapeData {
        /// 1-based line in the original source.
        line: usize,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "empty diagram"),
            ParseError::NotAFlowchart { header } => {
                write!(f, "not a flowchart: diagram starts with `{header}`")
            }
            ParseError::NoNodes => write!(f, "flowchart declares no nodes"),
            ParseError::UnclosedShape { line } => {
                write!(f, "unclosed node shape at line {line}")
            }
            ParseError::UnclosedString { line } => {
                write!(f, "unclosed `\"` at line {line}")
            }
            ParseError::UnknownShape { name, line } => write!(
                f,
                "no such shape: `{name}` at line {line} (shape names are lowercase and use `-`)"
            ),
            ParseError::UnclosedShapeData { line } => {
                write!(f, "unclosed `@{{` at line {line}")
            }
        }
    }
}

impl std::error::Error for ParseError {}
