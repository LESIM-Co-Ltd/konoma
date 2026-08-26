//! Drawing a parsed state diagram — stage 2 of konoma's own mermaid renderer.
//!
//! ```text
//! StateDiagram ──▶ measure ──▶ size the glyphs ──▶ GraphSpec ──▶ (stage 1's layout) ──▶ SVG
//! ```
//!
//! # What this module is, and what it is not
//!
//! It is **not** a layout. `docs/FEATURE-MERMAID-RENDERER.md` §7 puts stage 2 at a cost of
//! "cheap" precisely because a state diagram is the same kind of picture as a flowchart — boxes,
//! arrows and frames — so the whole of [`super::lay_out_spec`] is reused unchanged: ranking,
//! ordering, coordinates, the compound frames, the clipping, the label placement, the viewBox.
//! Everything this file adds is the translation from one language's model to the
//! language-neutral [`GraphSpec`], plus the six glyphs a state diagram needs that a flowchart has
//! no word for (`shapes::Glyph`).
//!
//! That is deliberate and it is checkable: [`super::state_tests`] runs the same geometric
//! invariants over state sources that [`super::tests`] runs over flowchart sources, and they are
//! literally the same properties because they are properties of the same [`Diagram`].
//!
//! # The four decisions this file does make
//!
//! 1. **A composite state is a frame, not a box.** It is the only thing in the model that can be
//!    both an edge endpoint *and* a container, and stage 1d already solved that pair: an edge
//!    naming a block is laid out against a member and cut back to the frame.
//! 2. **A `--` region is an untitled dashed frame.** mermaid gives each region a group with no
//!    label; without a title there is nothing but the outline to say the regions are concurrent
//!    rather than nested, so konoma dashes it (§0-1 allows the difference).
//! 3. **A fork bar turns with the diagram.** `forkJoin.ts` sizes the bar from the layout
//!    direction, and it has to: a bar drawn along the flow is a line, not a bar.
//! 4. **A note is placed by hand, after the layout, on the side the author asked for.** This is
//!    the one place stage 2 does *not* follow mermaid. Upstream makes the note a node and hangs
//!    it off a link, which leaves dagre to decide where it goes: `left of` and `right of` only
//!    reverse the link's direction, so on a top-to-bottom diagram a `right of` note lands
//!    *below* its state and a `left of` note lands *above* it — and konoma's first attempt,
//!    which copied that, drew a note reading "this is the note to the left" on the right.
//!    §0-1's acceptance criterion is "the same source read correctly", and a picture that
//!    contradicts its own words fails it.
//!
//!    So notes never reach dagre at all (a note orders nothing, so it has no ranking meaning to
//!    give up), and [`place_notes`] puts each one beside its state afterwards. That is
//!    deterministic — no search, no bias, the side is always the one that was written — and it
//!    keeps notes from perturbing the state layout. It also stays checkable: the note boxes and
//!    their connectors are ordinary [`Diagram`] geometry by the time [`lay_out`] returns, so the
//!    same invariants run over them, plus one that is about the side specifically.
//!
//! `note over …` is **not** part of this grammar — `stateDiagram.jison` has `left of`,
//! `right of` and the floating `note "text" as id`, and nothing else — so a `note over A` is
//! dropped by the parser rather than guessed at.

use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::state::{self, Direction, Kind, NotePosition, StateDiagram};
use crate::preview::mermaid::text_metrics;

use super::edges::Tip;
use super::shapes::{self, Glyph, Size};
use super::svg;
use super::{
    lay_out_spec, Diagram, GraphSpec, Label, PlacedEdge, PlacedNode, RenderError, SpecBlock,
    SpecEdge, SpecNode, Theme,
};

/// Blank space between a note and the state it belongs to, in px. Also the step a note is pushed
/// by when the place it wanted is taken.
pub const NOTE_GAP: f64 = 24.0;

/// How many times a note may be pushed further out before konoma gives up and leaves it where it
/// is. Every push clears at least one box, so a diagram would have to hold this many boxes in one
/// row to reach the bound.
const NOTE_PUSH_LIMIT: usize = 200;

/// Reads a mermaid state-diagram source and draws it.
///
/// **This is the entry point the golden tests go through**, so that what they pin is what a
/// caller gets — `docs/FEATURE-MERMAID-RENDERER.md` §6, after konoma was once caught pinning a
/// function that was not the one in production.
///
/// `theme` is `ui.mermaid_theme`'s raw string; an unknown value silently means `dark`.
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let diagram = state::parse(code)?;
    let laid = lay_out(&diagram)?;
    Ok(svg::emit(&laid, &Theme::named(theme)))
}

/// Measures, sizes, lays out and routes a parsed state diagram.
pub fn lay_out(diagram: &StateDiagram) -> Result<Diagram, RenderError> {
    // The gate. usvg does not fail on a missing font, it just drops the glyphs, so the only place
    // this can be caught is before anything is built (PRD design principle #3).
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if diagram.states.is_empty() {
        return Err(RenderError::NothingToDraw);
    }
    let mut out = lay_out_spec(&spec_of(diagram))?;
    place_notes(&mut out, diagram);
    Ok(out)
}

/// The whole of what is specific to the state-diagram language.
pub fn spec_of(diagram: &StateDiagram) -> GraphSpec {
    let horizontal_bars = matches!(
        diagram.direction,
        Direction::TopToBottom | Direction::BottomToTop
    );

    let mut nodes: Vec<SpecNode> = Vec::new();
    let mut blocks: Vec<SpecBlock> = Vec::new();
    for s in &diagram.states {
        // A note is not laid out. See the module docs: it carries no ordering meaning, and
        // leaving it to dagre is what put it on the wrong side.
        if s.kind == Kind::Note {
            continue;
        }
        if s.kind.is_block() {
            blocks.push(SpecBlock {
                id: s.id.clone(),
                title: s.label.clone(),
                members: s.members.clone(),
                // A `--` region has no title, so the outline is the only thing that can say what
                // it is; a composite state has one and does not need the hint.
                dashed: s.kind == Kind::Concurrent,
            });
            continue;
        }
        let glyph = glyph_of(s.kind, s.titled, horizontal_bars);
        let label = Label::measure(&s.label);
        let size = shapes::size(glyph, shapes::Size::new(label.width, label.height));
        nodes.push(SpecNode {
            id: s.id.clone(),
            glyph,
            label,
            size,
            panel: None,
        });
    }

    // dagre lays a compound graph out over its *leaves*, and a frame's rectangle is read back
    // from the border nodes hung off them. A block listed before its members would still work —
    // `Tree::from_blocks` resolves the nesting by id — but keeping the parser's order means the
    // frames come out innermost-first, which is the order `read_clusters` sorts by depth anyway.
    let edges = diagram
        .transitions
        .iter()
        .filter(|t| !t.is_note_link)
        .map(|t| SpecEdge {
            id: t.id.clone(),
            from: t.from.clone(),
            to: t.to.clone(),
            label: t
                .label
                .as_deref()
                .map(Label::measure)
                .filter(|l| !l.is_blank()),
            tip_start: Tip::None,
            tip_end: Tip::Arrow,
            stroke: Stroke::Normal,
            minlen: 1,
            start_label: None,
            end_label: None,
        })
        .collect();

    GraphSpec {
        direction: diagram.direction,
        nodes,
        edges,
        blocks,
    }
}

/// Puts every note beside the state it belongs to, on the side the source asked for.
///
/// Runs on the finished geometry, so it can be stated as a rule rather than as a hope:
///
/// 1. the note's inner edge sits one [`NOTE_GAP`] clear of its state's box, on the requested
///    side, and its centre starts on the state's own centre line;
/// 2. if that place is taken — by a state, or by a note already placed — the note **slides down**
///    until it is clear. It never moves sideways, so the side the reader was told about is always
///    the side they see, and it stays next to its state instead of being pushed past the whole
///    row. (Sliding *out* was the first attempt: on a left-to-right diagram it sent a note the
///    width of the picture away and dragged its connector across every box in between.)
///    What this does *not* promise is that the connector stays clear of every other box; on a
///    left-to-right diagram "to the right of" is the next rank's column, and a line reaching it
///    can pass a state on the way. That is the price of honouring the word the author wrote, and
///    it is recorded rather than hidden — see
///    [`super::state_tests::a_notes_connector_can_cross_a_state_in_a_crowded_diagram`].
/// 3. the connector is drawn from the note's boundary to the state's, dotted and headless, so it
///    cannot be read as a transition.
///
/// Notes are placed in the order they were written, which is what makes two notes on one state
/// come out in a stable order.
fn place_notes(out: &mut Diagram, model: &StateDiagram) {
    for note in model.states.iter().filter(|s| s.kind == Kind::Note) {
        let Some(position) = note.note_position else {
            continue;
        };
        // Which state it belongs to, and which way its connector was written: the parser makes
        // the tie-line point at the state for `left of` and away from it for `right of`, and the
        // drawn line keeps that so the model and the picture say the same thing.
        let Some(link) = model
            .transitions
            .iter()
            .find(|t| t.is_note_link && (t.from == note.id || t.to == note.id))
        else {
            continue;
        };
        let anchor_id = if link.from == note.id {
            link.to.clone()
        } else {
            link.from.clone()
        };
        // A note may name a composite state, which is a frame rather than a box.
        let Some((anchor_center, anchor_size, anchor_glyph)) = out
            .node(&anchor_id)
            .map(|n| (n.center.clone(), n.size, n.shape))
            .or_else(|| {
                out.cluster(&anchor_id)
                    .map(|c| (c.center.clone(), c.size, Glyph::Flow(Shape::Rect)))
            })
        else {
            continue;
        };

        let label = Label::measure(&note.label);
        let size = shapes::size(Glyph::Note, Size::new(label.width, label.height));
        let sign = match position {
            NotePosition::Right => 1.0,
            NotePosition::Left => -1.0,
        };
        let mut center = Point::new(
            anchor_center.x + sign * (anchor_size.w / 2.0 + NOTE_GAP + size.w / 2.0),
            anchor_center.y,
        );
        center = Point::new(center.x, free_row(out, &center, size));
        let note_point = shapes::intersect(Glyph::Note, center.clone(), size, &anchor_center);
        let anchor_point = shapes::intersect(anchor_glyph, anchor_center, anchor_size, &center);
        let points = if link.from == note.id {
            vec![note_point, anchor_point]
        } else {
            vec![anchor_point, note_point]
        };

        out.nodes.push(PlacedNode {
            id: note.id.clone(),
            shape: Glyph::Note,
            center,
            size,
            label,
            panel: None,
            series: None,
            mark: None,
        });
        out.edges.push(PlacedEdge {
            from: link.from.clone(),
            to: link.to.clone(),
            points,
            tip_start: Tip::None,
            tip_end: Tip::None,
            stroke: Stroke::Dotted,
            label: None,
            start_label: None,
            end_label: None,
            badge: None,
            series: None,
            straight: false,
        });
    }
    // The drawing grew sideways, so the extent has to be recomputed over what is now on it.
    super::normalise(out);
}

/// How many rows the slide may look at. Every step passes at least one box, so a diagram would
/// need this many boxes stacked in one column to reach the bound.
const SLIDE_STEPS: usize = 200;

/// The `y` to put a note at: its state's own centre line, or — if a box is already there — the
/// first row below it that is free.
///
/// This is a guarantee, not a preference. Sliding downward past one box at a time is monotone and
/// eventually leaves the drawing behind, where nothing is in the way, so a note's box **never**
/// lands on a state's. What it does not guarantee is the *connector*: see
/// [`super::state_tests::a_notes_connector_can_cross_a_state_in_a_crowded_diagram`].
///
/// Two things were tried here and taken back out, both because they never once changed an answer
/// and a check that decides nothing is worse than no check — it reads as a guarantee:
///
/// * scanning **upward** as well and taking whichever row is nearer. The nearer row was always
///   the one below, over every source in [`super::state_tests::CASES`] and a dozen more.
/// * preferring a row whose **connector** crosses nothing. Measured the same way: the first free
///   row already satisfied it wherever any row did, so the preference never chose differently.
fn free_row(out: &Diagram, center: &Point, size: Size) -> f64 {
    let mut y = center.y;
    for _ in 0..SLIDE_STEPS {
        let Some(clear_by) = box_overlap(out, &Point::new(center.x, y), size) else {
            break;
        };
        y += clear_by.max(0.0) + NOTE_GAP;
    }
    y
}

/// How far a note at `center` would have to move down for the first thing it lands on to be clear
/// of it, or `None` when it lands on nothing.
///
/// Two things count. A **state's box**, obviously. And a **transition's line**: a note is opaque
/// and is drawn over the edges, so one dropped on a line hides part of it — and an arrow whose
/// shaft disappears halfway is worse than a note further from its state.
///
/// A *frame* does not count. A note drawn over one is still legible, and treating a frame as an
/// obstruction would eject a note from the block its state lives in, which puts it further from
/// what it is about than a frame's fill ever puts it out of reach.
fn box_overlap(out: &Diagram, center: &Point, size: Size) -> Option<f64> {
    let (l, t, r, b) = (
        center.x - size.w / 2.0,
        center.y - size.h / 2.0,
        center.x + size.w / 2.0,
        center.y + size.h / 2.0,
    );
    let on_a_box = out.nodes.iter().find_map(|n| {
        let (nl, nt, nr, nb) = n.bounds();
        let dx = nr.min(r) - nl.max(l);
        let dy = nb.min(b) - nt.max(t);
        (dx > 0.01 && dy > 0.01).then_some(dy)
    });
    if on_a_box.is_some() {
        return on_a_box;
    }
    // The lowest point at which a line passes through the box is how far down the note has to go
    // to be under it — the same shape of answer a box gives, so the caller's slide stays monotone.
    let mut lowest: Option<f64> = None;
    for e in &out.edges {
        for w in e.drawn_points().windows(2) {
            let len = (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
            let steps = ((len * 2.0).ceil() as usize).clamp(1, 4000);
            for i in 0..=steps {
                let t_i = i as f64 / steps as f64;
                let (x, y) = (
                    w[0].x + t_i * (w[1].x - w[0].x),
                    w[0].y + t_i * (w[1].y - w[0].y),
                );
                if x > l + 0.01 && x < r - 0.01 && y > t + 0.01 && y < b - 0.01 {
                    lowest = Some(lowest.map_or(y, |cur: f64| cur.max(y)));
                }
            }
        }
    }
    lowest.map(|y| y - t)
}

/// What each kind of state is drawn as.
fn glyph_of(kind: Kind, titled: bool, horizontal_bars: bool) -> Glyph {
    match kind {
        Kind::Start => Glyph::StateStart,
        Kind::End => Glyph::StateEnd,
        Kind::Choice => Glyph::Choice,
        // `forkJoin.ts` reads the layout direction: the bar lies across the flow, so a
        // top-to-bottom diagram wants a wide one and a left-to-right diagram a tall one.
        Kind::Fork | Kind::Join => Glyph::Bar {
            horizontal: horizontal_bars,
        },
        Kind::Note => Glyph::Note,
        // A state written with two or more descriptions keeps the first apart from the rest with
        // a rule; one description is an ordinary box (mermaid's own fix-up at the end of
        // `extract` turns a single-entry `rectWithTitle` back into a `rect`).
        Kind::Simple if titled => Glyph::TitledBox,
        // The rounded rectangle every state diagram is mostly made of. konoma draws it with the
        // radius its flowchart uses for `A(text)` rather than mermaid's own 10, so the two
        // diagram kinds look like they came from one hand.
        Kind::Simple => Glyph::Flow(Shape::RoundedRect),
        // Unreachable: a block never reaches here (`spec_of` takes it out first). Drawing it as
        // an ordinary box rather than panicking is the honest answer if that ever changes.
        Kind::Composite | Kind::Concurrent => Glyph::Flow(Shape::RoundedRect),
    }
}
