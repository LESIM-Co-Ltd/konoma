//! Drawing a `gitGraph`.
//!
//! # Lanes, not ranks
//!
//! A git graph is a *history*: the x axis is the order the commits were made and the y axis is
//! which branch each was made on. Both are given by the source, so there is nothing for a layered
//! layout to decide — handing it to dagre would let it reorder the branches to reduce crossings,
//! and a branch that moves is a branch a reader stops being able to follow.
//!
//! So this is the third language that computes its own geometry (after the sequence diagram and
//! the seven charts), and everything below layout is still shared.
//!
//! # The lane order is the source's, not the drawing's
//!
//! [`GitGraph::lanes`] resolves `order:` against the order branches were created in. A commit's
//! lane is its branch's, and a merge sits on the branch it was made on — which is what makes the
//! two lines into a merge dot come from two different lanes.
//!
//! # The labels are measured before anything is placed
//!
//! A commit carries an id under it and any number of tags beside it, and those are the widest
//! things in the drawing — `cherry-pick:MERGE` is four commits wide. Both steps are therefore
//! **derived from the measured labels**, the way a flowchart's ranks come from its boxes and a
//! chart's slots from its category names; a constant step cannot know that two adjacent captions
//! read `ReverseHighlight` when they are drawn 46px apart. Nothing is ever clipped, truncated or
//! wrapped to make it fit — konoma does not do that anywhere.

use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::gitgraph::{Commit, CommitKind, Direction, GitGraph};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

use super::band;
use super::chart::label_node;
use super::{
    normalise, shapes, svg, Diagram, Glyph, Label, PlacedEdge, PlacedNode, RenderError, Size,
    Theme, Tip,
};

/// The closest two commits are ever drawn along the time axis. Two whose labels want more get it.
pub const COMMIT_STEP: f64 = 46.0;

/// The closest two branch lanes are ever drawn. Two whose labels want more get it.
pub const LANE_STEP: f64 = 44.0;

/// Blank space between a branch's name and the first commit on its lane.
pub const NAME_GAP: f64 = 10.0;

/// Blank space between a commit and the id beside it.
pub const CAPTION_GAP: f64 = 6.0;

/// Blank space between a commit and the first tag beside it.
pub const TAG_GAP: f64 = 5.0;

/// Blank space between one tag and the next one stacked beyond it.
pub const TAG_STACK: f64 = 3.0;

/// Blank space left between one commit's label and another's.
///
/// Both axes use it: along the lane it separates two neighbouring commits' captions, and across
/// the lanes it is what a label stops short of the midline between two lanes by. It is also the
/// width of the corridor a line changes lanes in, so half of it is the clearance between that line
/// and the nearest word.
///
/// Three spaces' worth at 14px. Two — the obvious "one word gap" — leaves `Reverse` and
/// `Highlight` looking like one hyphenless compound, because they are two *different* commits'
/// captions and a reader has to see the boundary before reading either.
pub const LABEL_GAP: f64 = 12.0;

/// Everything one commit hangs off itself, measured.
///
/// Measured up front because both steps are derived from these sizes — see the module docs.
struct Marks {
    /// The caption: the commit's id when the author wrote one, and blank otherwise.
    caption: Label,
    /// Each tag's words and the box its glyph needs, in the order they were written.
    tags: Vec<(Label, Size)>,
}

impl Marks {
    /// Measures what one commit carries.
    ///
    /// The caption is the id **only if the author wrote one**. A generated id is a random string
    /// that names nothing, and captioning every commit with one buries the two or three a reader
    /// is actually meant to follow.
    ///
    /// A tag does **not** stand in for it. mermaid draws both — `drawCommitLabel` under the dot
    /// and `drawCommitTags` over it — and it has to: a tag is a name somebody pinned onto a
    /// commit, and a commit can have an id of its own as well. Replacing one with the other threw
    /// away whichever the author wrote first.
    fn of(commit: &Commit) -> Marks {
        let caption = Label::measure(if commit.explicit_id { &commit.id } else { "" });
        let tags = commit
            .tags
            .iter()
            .map(|tag| {
                let label = Label::measure(tag);
                let size = shapes::size(Glyph::Tag, Size::new(label.width, label.height));
                (label, size)
            })
            .collect();
        Marks { caption, tags }
    }

    /// The caption's box, which is empty when there is no caption to draw — so a commit without
    /// one asks for no room at all rather than for a blank line's worth.
    fn caption_box(&self) -> Size {
        if self.caption.is_blank() {
            Size::new(0.0, 0.0)
        } else {
            Size::new(self.caption.width, self.caption.height)
        }
    }
}

/// Reads a mermaid git graph and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let graph = crate::preview::mermaid::gitgraph::parse(code)?;
    let diagram = lay_out(&graph)?;
    Ok(svg::emit(&diagram, &Theme::named(theme)))
}

/// Works out where every commit, line, lane and tag goes.
pub fn lay_out(graph: &GitGraph) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if graph.commits.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    let lanes: Vec<&str> = graph.lanes().iter().map(|b| b.name.as_str()).collect();
    let lane_of = |branch: &str| lanes.iter().position(|b| *b == branch).unwrap_or(0);

    // In `LR:` the branch names sit before the lanes, so their width decides where time starts.
    //
    // `TB:`/`BT:` use the same origin and should not: there the names sit *above* the lanes, and
    // the room they need is made by where they are placed. The width of the widest name is added
    // to the top of the drawing as blank page — see the ignored
    // `a_top_to_bottom_git_graph_does_not_open_with_a_band_of_empty_page`. Left alone here because
    // it is a defect of its own with a cause of its own, not part of the label work below.
    let names: Vec<Label> = lanes.iter().map(|b| Label::measure(b)).collect();
    let widest = names.iter().map(|l| l.width).fold(0.0_f64, f64::max);
    let x0 = widest + NAME_GAP;

    // --- measure, then lay out ------------------------------------------------------------------
    //
    // A commit's labels are offset **across** the lanes and never along them, because along the
    // lane is where the lane's own line runs (see the tags below). So a label's *width* is what
    // has to fit along the time axis in `LR:` and across the lanes in `TB:`/`BT:`, and its height
    // is the other way round. Every step below is written in those two projections rather than in
    // x and y, so one statement covers all three directions.
    let lr = matches!(graph.direction, Direction::LeftToRight);
    let along_of = move |s: Size| if lr { s.w } else { s.h };
    let across_of = move |s: Size| if lr { s.h } else { s.w };

    let marks: Vec<Marks> = graph.commits.iter().map(Marks::of).collect();
    // How far a commit's labels reach out on each side of its lane. The caption goes one side and
    // the tags the other, so the two are measured apart and never add up.
    let caption_reach = |m: &Marks| {
        if m.caption.is_blank() {
            0.0
        } else {
            shapes::COMMIT_RADIUS + CAPTION_GAP + across_of(m.caption_box())
        }
    };
    let tags_reach = |m: &Marks| {
        if m.tags.is_empty() {
            0.0
        } else {
            shapes::COMMIT_RADIUS
                + TAG_GAP
                + m.tags.iter().map(|(_, s)| across_of(*s)).sum::<f64>()
                + (m.tags.len() - 1) as f64 * TAG_STACK
        }
    };
    // …and how far they reach either way *along* it, from the commit they belong to.
    let caption_half = |m: &Marks| along_of(m.caption_box()) / 2.0;
    let tags_half = |m: &Marks| {
        m.tags
            .iter()
            .map(|(_, s)| along_of(*s) / 2.0)
            .fold(0.0_f64, f64::max)
    };

    // --- how far apart the lanes are --------------------------------------------------------
    //
    // A caption reaches toward the *next* lane and a tag toward the *previous* one, and each has
    // to stay inside **its own lane's column** — its own half of the gap, on its own side of the
    // midline between the two lanes. Anything past the midline is nearer the wrong lane's line
    // than its own and reads as belonging to it: at a fixed pitch the `cherry-pick:MERGE` ticket
    // on `release` came within 6px of `develop`'s line and 38 from its own, which is why it looked
    // like `develop`'s.
    //
    // Half the gap and not the sum of the two facing reaches, which is the tighter rule and the
    // wrong one: it lets a tall stack take three quarters of a gap whose other side happens to be
    // empty, and the stack then sits nearer its neighbour than its own lane while nothing
    // overlaps anything.
    //
    // The step stays uniform: a lane is a branch, and a lane drawn at a different pitch from its
    // neighbour reads as something the history does not say.
    let mut captions_out = vec![0.0_f64; lanes.len()];
    let mut tags_out = vec![0.0_f64; lanes.len()];
    for (commit, m) in graph.commits.iter().zip(&marks) {
        let k = lane_of(&commit.branch);
        captions_out[k] = captions_out[k].max(caption_reach(m));
        tags_out[k] = tags_out[k].max(tags_reach(m));
    }
    // A branch's name is a label too. In `LR:` it sits **on** its own lane — it is what comes
    // before the first commit — so it takes up room in both of that lane's rows; in `TB:`/`BT:` it
    // sits above the whole drawing instead, where the `widest + NAME_GAP` below is what makes room
    // for it.
    if lr {
        for (k, name) in names.iter().enumerate() {
            let half = across_of(Size::new(name.width, name.height)) / 2.0;
            captions_out[k] = captions_out[k].max(half);
            tags_out[k] = tags_out[k].max(half);
        }
    }
    let mut lane_step = LANE_STEP;
    if !lr {
        // Across the time axis the branch names sit *above* the lanes, side by side, so the lanes
        // have to be at least as far apart as the names are wide or two names run together into
        // one word. (Along it — `LR:` — they sit before the first commit instead, which is `x0`.)
        lane_step = lane_step.max(widest + NAME_GAP);
    }
    // Only where there **is** a neighbour on that side. The outermost lane's tickets hang off the
    // edge of the drawing into space nothing else can want, and asking the pitch to hold a 200px
    // `cherry-pick:A` ticket that intrudes on nothing would put the two lanes 434px apart.
    for k in 0..lanes.len() {
        if k + 1 < lanes.len() {
            lane_step = lane_step.max(captions_out[k] * 2.0 + LABEL_GAP);
        }
        if k > 0 {
            lane_step = lane_step.max(tags_out[k] * 2.0 + LABEL_GAP);
        }
    }

    // --- where each commit sits along the time axis -------------------------------------------
    //
    // How far one commit's labels reach either way along the axis: the caption's row and the
    // tags' row are on opposite sides of the lane, so what matters is whichever reaches further.
    let ext = |m: &Marks| caption_half(m).max(tags_half(m));

    // Two neighbours in time are pushed apart until their labels clear each other by `LABEL_GAP`.
    // Because **every** consecutive pair gets that gap, the label rows come out in `seq` order
    // with no two of them touching — so a caption two commits away cannot reach in either, and
    // between any two commits there is a strip of the drawing with no label in it at all. The
    // lines use that strip to change lanes in; see below.
    //
    // Stated over neighbours in *time* rather than over neighbours on a *lane*, even though only
    // two on the same lane can collide (the lane step above keeps one lane's rows clear of the
    // next lane's). The lane-neighbour rule is enough to stop labels overlapping and is **not**
    // enough to leave that strip, and one rule that gives both is worth the extra room it asks
    // for.
    //
    // Accumulating rather than scaling makes the widening **local**: one long id pushes its own
    // two neighbours apart and leaves the rest of the history at [`COMMIT_STEP`], where a uniform
    // step taken from the widest label in the graph would spread every commit in it.
    let mut at = vec![0.0_f64; graph.commits.len()];
    for i in 1..graph.commits.len() {
        let need = ext(&marks[i - 1]) + ext(&marks[i]) + LABEL_GAP;
        at[i] = at[i - 1] + need.max(COMMIT_STEP);
    }

    let along = |seq: usize| x0 + at[seq] + shapes::COMMIT_RADIUS;
    let across = move |lane: usize| lane as f64 * lane_step + shapes::COMMIT_RADIUS;
    // `gitGraph TB:` / `BT:` run time down or up the page; `LR:` is the default. The two axes are
    // swapped rather than recomputed here, in one place, so everything below is written in the
    // picture's own two axes and a mutation to either step affects all three directions the same
    // way.
    let axes = |along_v: f64, across_v: f64| match graph.direction {
        Direction::LeftToRight => Point::new(along_v, across_v),
        Direction::TopToBottom => Point::new(across_v, along_v),
        Direction::BottomToTop => Point::new(across_v, -along_v),
    };
    let place = |seq: usize, lane: usize| -> Point { axes(along(seq), across(lane)) };
    // The middle of the label-free strip that runs between commit `seq` and the one before it —
    // where a line that changes lanes is allowed to cross. `seq` is never 0 here: a parent always
    // comes before its child.
    let corridor = |seq: usize| {
        let opens = along(seq - 1) + ext(&marks[seq - 1]);
        let closes = along(seq) - ext(&marks[seq]);
        (opens + closes) / 2.0
    };
    // Where a label goes: `out` across the lanes from its commit, positive being the caption's
    // side and negative the tags'.
    let beside = |center: &Point, out: f64| match graph.direction {
        Direction::LeftToRight => Point::new(center.x, center.y + out),
        _ => Point::new(center.x + out, center.y),
    };

    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut edges: Vec<PlacedEdge> = Vec::new();
    let d = shapes::COMMIT_RADIUS * 2.0;

    // --- the lane names -------------------------------------------------------------------------
    for (i, name) in names.iter().enumerate() {
        let at = match graph.direction {
            Direction::LeftToRight => Point::new(widest - name.width / 2.0, across(i)),
            _ => Point::new(across(i), -NAME_GAP - name.height / 2.0),
        };
        // The name is drawn on the terminal's own background, so it takes the ordinary text
        // colour rather than the one a *fill* was chosen to carry.
        if let Some(n) = label_node(format!("branch#{}", lanes[i]), name.clone(), at, None) {
            nodes.push(n);
        }
    }

    // --- the commits ----------------------------------------------------------------------------
    for (i, commit) in graph.commits.iter().enumerate() {
        let lane = lane_of(&commit.branch);
        let center = place(commit.seq, lane);
        let m = &marks[i];
        let drawn = commit.custom_type.unwrap_or(commit.kind);
        let (glyph, size) = match drawn {
            CommitKind::Reverse => (Glyph::Reverted, Size::new(d, d)),
            // A highlight is meant to stand out, and a bigger dot of the same shape does not at
            // this scale. A square does.
            CommitKind::Highlight => (Glyph::Flow(Shape::Rect), Size::new(d, d)),
            CommitKind::Merge => (Glyph::Flow(Shape::DoubleCircle), Size::new(d, d)),
            CommitKind::CherryPick | CommitKind::Normal => {
                (Glyph::Flow(Shape::Circle), Size::new(d, d))
            }
        };
        nodes.push(PlacedNode {
            id: commit.id.clone(),
            shape: glyph,
            center: center.clone(),
            size,
            label: Label::measure(""),
            panel: None,
            series: Some(lane),
            mark: None,
        });

        // The caption beside a commit — see [`Marks::of`] for which commits get one and why a tag
        // is not allowed to stand in for it.
        let at = beside(
            &center,
            shapes::COMMIT_RADIUS + CAPTION_GAP + across_of(m.caption_box()) / 2.0,
        );
        if let Some(n) = label_node(
            format!("{}#caption", commit.id),
            m.caption.clone(),
            at,
            None,
        ) {
            nodes.push(n);
        }

        // --- the tags, on the other side of the dot from the caption ---------------------------
        //
        // Two things put a tag there. It has to be **out of the caption's row**, because a tag
        // drawn in the same band as the id is one more word in an undifferentiated row — that is
        // mermaid's arrangement too (`drawCommitLabel` under the dot, `drawCommitTags` at
        // `commitPosition.y - 16 - yOffset` over it). And it has to be **across the lanes rather
        // than up and down the page**, which is where konoma differs and why: in `TB:`/`BT:` up
        // and down *is* the time axis, so a tag hung above its commit sits on its own lane's line
        // — the line runs straight through the ticket, and through the id under it — and takes up
        // the room the next commit along needs. Across is the one direction the lane line is not.
        //
        // Several tags on one commit stack **outward in source order**, so the first one written
        // is nearest its commit and the column reads the way the source does.
        let mut out = shapes::COMMIT_RADIUS + TAG_GAP;
        for (k, (label, size)) in m.tags.iter().enumerate() {
            nodes.push(PlacedNode {
                id: format!("{}#tag{k}", commit.id),
                shape: Glyph::Tag,
                center: beside(&center, -(out + across_of(*size) / 2.0)),
                size: *size,
                label: label.clone(),
                panel: None,
                // A tag takes its commit's lane colour, which is what says *which* commit it
                // belongs to when it is sitting a couple of rows out from the dot.
                series: Some(lane),
                mark: None,
            });
            out += across_of(*size) + TAG_STACK;
        }
    }

    // --- the lines ------------------------------------------------------------------------------
    //
    // One per parent, so a merge draws two. The line is routed as a staircase: along the parent's
    // lane, across into the child's, then along that one to the child — which is how a branch
    // reads as a branch rather than as a diagonal.
    //
    // **It crosses in the corridor, not over the child.** Turning at the child's own position is
    // the obvious route and it puts the crossing squarely through the child's labels: the merge
    // into `MERGE` was drawn straight down the middle of the word `MERGE`. The column over a
    // commit belongs to that commit's caption and tags; the strip between two commits belongs to
    // nothing, which is what makes it the place to cross.
    for commit in &graph.commits {
        let lane = lane_of(&commit.branch);
        let head = place(commit.seq, lane);
        for parent_id in &commit.parents {
            let Some(parent) = graph.commit(parent_id) else {
                continue;
            };
            let plane = lane_of(&parent.branch);
            let tail = place(parent.seq, plane);
            let mut points = vec![tail.clone()];
            if plane != lane {
                let cross = corridor(commit.seq);
                points.push(axes(cross, across(plane)));
                points.push(axes(cross, across(lane)));
            }
            points.push(head.clone());
            edges.push(PlacedEdge {
                from: parent.id.clone(),
                to: commit.id.clone(),
                points,
                tip_start: Tip::None,
                tip_end: Tip::None,
                stroke: Stroke::Normal,
                label: None,
                start_label: None,
                end_label: None,
                badge: None,
                series: Some(plane),
                // A branch line is a right angle: along the lane, then across into the next one.
                straight: true,
                overlay: false,
            });
        }
    }

    let mut diagram = Diagram {
        nodes,
        edges,
        ..Diagram::default()
    };
    band::add_title(&mut diagram, graph.preamble.title.as_deref());
    normalise(&mut diagram);
    Ok(diagram)
}
