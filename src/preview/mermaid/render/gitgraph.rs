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

use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::gitgraph::{CommitKind, Direction, GitGraph};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

use super::band;
use super::chart::label_node;
use super::{
    normalise, shapes, svg, Diagram, Glyph, Label, PlacedEdge, PlacedNode, RenderError, Size,
    Theme, Tip,
};

/// Distance between two commits along the time axis.
pub const COMMIT_STEP: f64 = 46.0;

/// Distance between two branch lanes.
pub const LANE_STEP: f64 = 44.0;

/// Blank space between a branch's name and the first commit on its lane.
pub const NAME_GAP: f64 = 10.0;

/// Blank space between a commit and the tag or id under it.
pub const CAPTION_GAP: f64 = 6.0;

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

    // The branch names sit before the lanes, so their width decides where time starts.
    let names: Vec<Label> = lanes.iter().map(|b| Label::measure(b)).collect();
    let widest = names.iter().map(|l| l.width).fold(0.0_f64, f64::max);
    let x0 = widest + NAME_GAP;

    // Along the time axis the branch names sit *before* the first commit; across it they sit
    // *above* the lanes, side by side, so the lanes have to be at least as far apart as the names
    // are wide or two names run together into one word.
    let lane_step = match graph.direction {
        Direction::LeftToRight => LANE_STEP,
        _ => LANE_STEP.max(widest + NAME_GAP),
    };
    let along = move |seq: usize| x0 + seq as f64 * COMMIT_STEP + shapes::COMMIT_RADIUS;
    let across = move |lane: usize| lane as f64 * lane_step + shapes::COMMIT_RADIUS;
    // `gitGraph TB:` / `BT:` run time down or up the page; `LR:` is the default. The two axes are
    // swapped rather than recomputed, so a mutation to the step affects both the same way.
    let place = |seq: usize, lane: usize| -> Point {
        match graph.direction {
            Direction::LeftToRight => Point::new(along(seq), across(lane)),
            Direction::TopToBottom => Point::new(across(lane), along(seq)),
            Direction::BottomToTop => Point::new(across(lane), -along(seq)),
        }
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
    for commit in &graph.commits {
        let lane = lane_of(&commit.branch);
        let center = place(commit.seq, lane);
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

        // The caption under a commit: its tags, or the id **if the author wrote one**. A
        // generated id is a random string that names nothing, and captioning every commit with
        // one buries the two or three a reader is actually meant to follow.
        let caption = if !commit.tags.is_empty() {
            commit.tags.join(", ")
        } else if commit.explicit_id {
            commit.id.clone()
        } else {
            String::new()
        };
        let label = Label::measure(&caption);
        let at = Point::new(
            center.x,
            center.y + shapes::COMMIT_RADIUS + CAPTION_GAP + label.height / 2.0,
        );
        if let Some(n) = label_node(format!("{}#caption", commit.id), label, at, None) {
            nodes.push(n);
        }
    }

    // --- the lines ------------------------------------------------------------------------------
    //
    // One per parent, so a merge draws two. The line is routed as an elbow: along the parent's
    // lane to the child's position, then across into it — which is how a branch reads as a branch
    // rather than as a diagonal.
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
                points.push(match graph.direction {
                    Direction::LeftToRight => Point::new(head.x, tail.y),
                    _ => Point::new(tail.x, head.y),
                });
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
