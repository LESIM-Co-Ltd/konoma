//! Drawing a parsed sequence diagram — stage 4.
//!
//! ```text
//! SequenceDiagram ──▶ measure ──▶ columns across ──▶ walk the events downwards ──▶ SVG
//! ```
//!
//! # This is the one stage that does not go through `lay_out_spec`, and that is the design
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §7 settles it. [`super::lay_out_spec`] is the pipeline for a
//! **layered graph**: measure, rank, order to reduce crossings, assign coordinates, route. A
//! sequence diagram has none of those questions. Its participants go across the top in the order
//! they first appeared — that order is the author's, and reordering it would be a bug, not an
//! improvement — its lifelines hang straight down, and its messages stack at increasing y in
//! source order. Feeding it to dagre would mean inventing nodes and edges that mean something
//! different from what they are, and then hoping the answer came back in the order it went in.
//!
//! So this module builds a [`Diagram`] in one pass. **Everything below layout is shared**: the
//! same `Diagram`, the same [`Glyph`] and [`Tip`] enums, the same [`svg::emit`], the same five
//! palettes. Stage 4 added exactly one new piece of geometry — [`PlacedLifeline`], because a
//! lifeline is neither a node nor an edge — and three new drawn things that are variants of enums
//! that already existed ([`Glyph::Participant`], [`Glyph::Actor`], [`Tip::Async`]).
//!
//! # What the walk maintains
//!
//! One y cursor and three stacks:
//!
//! * **open frames** — a `loop` / `alt` / … that has not met its `end`. Each one absorbs the
//!   bounding box of everything placed while it is open, which is what makes "the frame encloses
//!   its contents" true by construction rather than by a guess about how wide to draw it.
//! * **open activations**, per participant. `activate` pushes and `deactivate` pops, and the depth
//!   at the moment of pushing is what steps the bar one width to the right — so nested bars form a
//!   staircase and never overlap.
//! * **the last message's y**, which is where an activation opens and closes. mermaid does the
//!   same (`bounds.getVerticalPos()`), and it is what makes a bar start at the arrow that caused
//!   it rather than somewhere below it.
//!
//! # Where konoma leaves mermaid, and why
//!
//! * **`actor` is a stick figure.** mermaid draws one too; `mermaid-rs-renderer`, which konoma is
//!   replacing here, draws a second plain box, so `actor Alice` and `participant Alice` come out
//!   identical there.
//! * **A group frame hugs its contents** instead of spanning every participant. On a terminal a
//!   frame that reaches across a diagram nobody is talking to says less than one that reaches
//!   across the two participants that are.
//! * **A `rect`'s colour and a `box`'s colour are not painted.** konoma composites onto the
//!   terminal (§1), where an opaque highlight is a grey slab rather than a tint. A `rect` is drawn
//!   as a frame like the others so the region it marks is still visible.
//! * **A lifeline is dashed and quiet.** See [`svg::emit_lifeline`]'s note.
//! * **`create` and `destroy` are drawn**: a created participant's box appears at the message that
//!   creates it, and a destroyed one's lifeline stops at a cross. The crate ignores both.

use std::collections::HashMap;

use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::sequence::{
    self, AutoNumber, BlockKind, Event, Head, Line, Note, Placement, SequenceDiagram,
};
use crate::preview::mermaid::text_metrics;

use super::edges::Tip;
use super::panel::{Panel, PanelCell, PanelRow};
use super::shapes::{self, Glyph, Size};
use super::{clusters, labels, svg};
use super::{
    ClusterSection, Diagram, Label, PlacedActivation, PlacedCluster, PlacedEdge, PlacedEdgeLabel,
    PlacedLifeline, PlacedNode, RenderError, Theme, ACTIVATION_WIDTH,
};

/// Gap between two participants' boxes. mermaid's `sequence.actorMargin`, schema default 50.
pub const PARTICIPANT_GAP: f64 = 50.0;

/// Blank space between a `box` frame and the participants inside it.
pub const BOX_PAD_X: f64 = 12.0;

/// Blank space under the diagram's title, before the participants.
pub const TITLE_GAP: f64 = 10.0;

/// Blank space above a message's label and below its line.
pub const MESSAGE_PAD: f64 = 12.0;

/// Blank space between a message's label and the line it belongs to.
pub const LABEL_GAP: f64 = 4.0;

/// How far a self-message's loop reaches to the right of its own lifeline.
pub const SELF_WIDTH: f64 = 42.0;

/// How far it reaches down.
pub const SELF_HEIGHT: f64 = 26.0;

/// Blank space above and below a note. mermaid's `sequence.noteMargin`, schema default 10.
pub const NOTE_GAP: f64 = 10.0;

/// Blank space between a `left of` / `right of` note and the lifeline it belongs to.
pub const NOTE_SIDE_GAP: f64 = 12.0;

/// How far a `note over A,B` reaches past the two lifelines it spans.
pub const NOTE_OVERHANG: f64 = 14.0;

/// Blank space between a group frame and the messages inside it.
pub const GROUP_PAD_X: f64 = 12.0;

/// Blank space above a group frame.
pub const GROUP_GAP: f64 = 8.0;

/// Blank space between the last thing inside a group frame and its bottom edge.
pub const GROUP_PAD_BOTTOM: f64 = 10.0;

/// Blank space above an `else` / `and` / `option` rule.
pub const SECTION_GAP: f64 = 6.0;

/// Blank space between a sequence number's digits and the edge of its disc.
pub const BADGE_PAD: f64 = 5.0;

/// Blank space between the last message and the mirrored participants at the foot.
pub const FOOT_GAP: f64 = 14.0;

/// Reads a mermaid sequence source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let parsed = sequence::parse(code)?;
    let laid = lay_out(&parsed)?;
    Ok(svg::emit(&laid, &Theme::named(theme)))
}

/// One participant's column: everything about it that is decided before the walk begins.
struct Column {
    id: String,
    glyph: Glyph,
    label: Label,
    panel: Option<Panel>,
    size: Size,
    /// The lifeline's x, which is also the centre of the head and foot boxes.
    x: f64,
    /// Centre y of the head box. `None` until the creating message is reached, for a participant
    /// declared with `create`.
    head_y: Option<f64>,
    /// Set when a `destroy` is honoured by a following message.
    destroyed_y: Option<f64>,
    created: bool,
}

impl Column {
    fn lifeline_top(&self, default_top: f64) -> f64 {
        match self.head_y {
            Some(y) => y + self.size.h / 2.0,
            None => default_top,
        }
    }
}

/// A frame that has been opened and is waiting for its `end`.
struct OpenFrame {
    id: String,
    title: Label,
    top: f64,
    depth: usize,
    parent: Option<String>,
    /// Union of everything placed while this frame has been open, as `(l, t, r, b)`.
    contents: Option<(f64, f64, f64, f64)>,
    sections: Vec<ClusterSection>,
}

/// Which side of a lifeline a line attaches to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// Measures, places and routes a parsed sequence diagram.
///
/// Stops at geometry: no colour, no markup. That is the seam every invariant test is stated
/// across (§6), and it is why the tests never have to parse konoma's own SVG.
pub fn lay_out(diagram: &SequenceDiagram) -> Result<Diagram, RenderError> {
    // The gate. usvg does not fail on a missing font, it just drops the glyphs, so the only place
    // this can be caught is before anything is built (PRD design principle #3).
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if diagram.participants.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    let mut columns = measure_columns(diagram);
    place_columns(diagram, &mut columns);
    let index: HashMap<String, usize> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.clone(), i))
        .collect();

    // --- the head band -----------------------------------------------------------------------
    let title = diagram.title.as_deref().map(Label::measure);
    let title_size = title
        .as_ref()
        .filter(|l| !l.is_blank())
        .map(|l| shapes::size(Glyph::Flow(Shape::Text), Size::new(l.width, l.height)));
    let top = title_size.map_or(0.0, |s| s.h + TITLE_GAP);
    // The band a `box` frame's title lives in, above the participants. Measured from the titles
    // that are actually there, so a two-line one gets two lines of room rather than covering the
    // boxes underneath it.
    let box_band = diagram
        .boxes
        .iter()
        .map(|b| band(&Label::measure(&b.title)))
        .fold(0.0_f64, f64::max);
    let head_top = top + box_band;
    let head_h = columns.iter().map(|c| c.size.h).fold(0.0_f64, f64::max);
    for c in columns.iter_mut() {
        // Bottom-aligned, so every lifeline starts at the same y even when a stick figure stands
        // taller than the box beside it.
        if !c.created {
            c.head_y = Some(head_top + head_h - c.size.h / 2.0);
        }
    }
    let body_top = head_top + head_h;

    let (mut nodes, edges, mut frames, mut bars, open_bars, mut y, head_y, destroyed_y) = {
        let mut walk = Walk::new(&columns, index, body_top);
        walk.run(diagram);
        (
            walk.nodes,
            walk.edges,
            walk.frames,
            walk.bars,
            walk.open_bars,
            walk.y,
            walk.head_y,
            walk.destroyed_y,
        )
    };
    // A `create` is honoured by the message that follows it, so where a created participant's box
    // goes is only known once the walk is done. One that was never honoured — mermaid errors on
    // that case — falls back to the head band rather than being left undrawn.
    for (i, c) in columns.iter_mut().enumerate() {
        if head_y[i].is_some() {
            c.head_y = head_y[i];
        } else if c.head_y.is_none() {
            c.head_y = Some(head_top + head_h - c.size.h / 2.0);
        }
        c.destroyed_y = destroyed_y[i];
    }

    // --- the foot band -----------------------------------------------------------------------
    y += FOOT_GAP;
    let body_bottom = y;
    let mut bottom = body_bottom;
    for c in &columns {
        if c.destroyed_y.is_some() {
            continue;
        }
        nodes.push(PlacedNode {
            id: format!("{}#foot", c.id),
            shape: c.glyph,
            center: Point::new(c.x, body_bottom + c.size.h / 2.0),
            size: c.size,
            label: c.label.clone(),
            panel: c.panel.clone(),
            series: None,
            mark: None,
        });
        bottom = bottom.max(body_bottom + c.size.h);
    }

    // --- lifelines, and the bars that are still open at the foot -------------------------------
    let mut lifelines: Vec<PlacedLifeline> = Vec::with_capacity(columns.len());
    for (i, c) in columns.iter().enumerate() {
        let top_y = c.lifeline_top(body_top);
        let bottom_y = c.destroyed_y.unwrap_or(body_bottom).max(top_y);
        for open in &open_bars[i] {
            bars[i][*open].bottom = bottom_y;
        }
        for bar in bars[i].iter_mut() {
            bar.top = bar.top.clamp(top_y, bottom_y);
            bar.bottom = bar.bottom.clamp(bar.top, bottom_y);
        }
        lifelines.push(PlacedLifeline {
            id: c.id.clone(),
            x: c.x,
            top: top_y,
            bottom: bottom_y,
            destroyed: c.destroyed_y.is_some(),
            activations: std::mem::take(&mut bars[i]),
        });
    }

    // --- the head boxes, and the diagram's own title -------------------------------------------
    for c in &columns {
        let Some(head_y) = c.head_y else { continue };
        nodes.push(PlacedNode {
            id: c.id.clone(),
            shape: c.glyph,
            center: Point::new(c.x, head_y),
            size: c.size,
            label: c.label.clone(),
            panel: c.panel.clone(),
            series: None,
            mark: None,
        });
    }
    let span_left = columns
        .iter()
        .map(|c| c.x - c.size.w / 2.0)
        .fold(f64::INFINITY, f64::min);
    let span_right = columns
        .iter()
        .map(|c| c.x + c.size.w / 2.0)
        .fold(f64::NEG_INFINITY, f64::max);
    if let (Some(label), Some(size)) = (title, title_size) {
        nodes.push(PlacedNode {
            id: "title#".to_string(),
            shape: Glyph::Flow(Shape::Text),
            center: Point::new((span_left + span_right) / 2.0, size.h / 2.0),
            size,
            label,
            panel: None,
            series: None,
            mark: None,
        });
    }

    // --- the `box` frames, which are the other kind of grouping --------------------------------
    let mut placed_frames: Vec<PlacedCluster> = Vec::new();
    for b in &diagram.boxes {
        let members: Vec<usize> = b
            .members
            .iter()
            .filter_map(|m| columns.iter().position(|c| &c.id == m))
            .collect();
        if members.is_empty() {
            continue;
        }
        let left = members
            .iter()
            .map(|i| columns[*i].x - columns[*i].size.w / 2.0)
            .fold(f64::INFINITY, f64::min)
            - BOX_PAD_X;
        let right = members
            .iter()
            .map(|i| columns[*i].x + columns[*i].size.w / 2.0)
            .fold(f64::NEG_INFINITY, f64::max)
            + BOX_PAD_X;
        let title = Label::measure(&b.title);
        let (mut left, mut right) = (left, right);
        let need = title.width + clusters::TITLE_PAD_X;
        let short = need - (right - left);
        if short > 0.0 {
            left -= short / 2.0;
            right += short / 2.0;
        }
        placed_frames.push(PlacedCluster {
            id: b.id.clone(),
            title,
            center: Point::new((left + right) / 2.0, (head_top - box_band + bottom) / 2.0),
            size: Size::new(right - left, bottom - (head_top - box_band)),
            parent: None,
            depth: 0,
            dashed: false,
            filled: true,
            sections: Vec::new(),
        });
    }
    // A group frame is drawn over a `box` frame, so the boxes go in first and the stable sort by
    // depth below leaves them there.
    placed_frames.append(&mut frames);
    placed_frames.sort_by_key(|c| c.depth);

    let mut d = Diagram {
        width: 0.0,
        height: 0.0,
        nodes,
        edges,
        clusters: placed_frames,
        lifelines,
    };
    super::normalise(&mut d);
    Ok(d)
}

/// The height a title of this size needs above whatever it labels.
///
/// [`PlacedCluster::title_center`] puts the text at `top + TITLE_PAD_Y + height/2`, so the band has
/// to be at least that plus the same padding under it — which is what keeps the frame's title off
/// the first message inside it without anything having to check afterwards.
fn band(title: &Label) -> f64 {
    clusters::TITLE_PAD_Y * 2.0 + title.height + 4.0
}

/// Measures each participant and sizes its glyph.
fn measure_columns(diagram: &SequenceDiagram) -> Vec<Column> {
    diagram
        .participants
        .iter()
        .map(|p| {
            let label = Label::measure(&p.label);
            match p.kind {
                sequence::ParticipantKind::Actor => {
                    let panel = actor_panel(&label);
                    Column {
                        id: p.id.clone(),
                        glyph: Glyph::Actor,
                        label: Label::measure(""),
                        size: shapes::size(Glyph::Actor, panel.size),
                        panel: Some(panel),
                        x: 0.0,
                        head_y: None,
                        destroyed_y: None,
                        created: p.created_at.is_some(),
                    }
                }
                sequence::ParticipantKind::Participant => {
                    let size =
                        shapes::size(Glyph::Participant, Size::new(label.width, label.height));
                    Column {
                        id: p.id.clone(),
                        glyph: Glyph::Participant,
                        label,
                        panel: None,
                        size,
                        x: 0.0,
                        head_y: None,
                        destroyed_y: None,
                        created: p.created_at.is_some(),
                    }
                }
            }
        })
        .collect()
}

/// An `actor`'s contents: a band for the stick figure, then the name under it.
///
/// A [`Panel`] and not a centred label, because the two bands are two rows — and because a panel
/// is geometry the shared invariant
/// [`super::tests::check_panels_stay_inside_their_box`] already knows how to check.
fn actor_panel(label: &Label) -> Panel {
    let w = (label.width + shapes::PARTICIPANT_PAD_X * 2.0).max(shapes::PARTICIPANT_MIN_WIDTH);
    let text_band = label.height + shapes::PARTICIPANT_PAD_Y;
    Panel {
        rows: vec![PanelRow {
            cells: vec![PanelCell {
                label: label.clone(),
                x: w / 2.0,
                centered: true,
            }],
            y: shapes::ACTOR_FIGURE_HEIGHT + text_band / 2.0,
        }],
        rules: Vec::new(),
        columns: Vec::new(),
        size: Size::new(w, shapes::ACTOR_FIGURE_HEIGHT + text_band),
    }
}

/// Lays the columns out left to right, in **first-appearance order**, leaving room for the `box`
/// frames that wrap some of them and for the text that has to fit between them.
fn place_columns(diagram: &SequenceDiagram, columns: &mut [Column]) {
    let n = columns.len();
    if n == 0 {
        return;
    }
    let mut first_of_box: Vec<bool> = vec![false; n];
    let mut last_of_box: Vec<bool> = vec![false; n];
    for b in &diagram.boxes {
        let members: Vec<usize> = b
            .members
            .iter()
            .filter_map(|m| columns.iter().position(|c| &c.id == m))
            .collect();
        if let (Some(lo), Some(hi)) = (members.iter().min(), members.iter().max()) {
            first_of_box[*lo] = true;
            last_of_box[*hi] = true;
        }
    }
    let mut gaps: Vec<f64> = (0..n.saturating_sub(1))
        .map(|i| {
            PARTICIPANT_GAP
                + if last_of_box[i] { BOX_PAD_X } else { 0.0 }
                + if first_of_box[i + 1] { BOX_PAD_X } else { 0.0 }
        })
        .collect();
    widen_for_text(diagram, columns, &mut gaps);

    let mut x = if first_of_box[0] { BOX_PAD_X } else { 0.0 };
    for i in 0..n {
        columns[i].x = x + columns[i].size.w / 2.0;
        x += columns[i].size.w;
        if i + 1 < n {
            x += gaps[i];
        }
    }
}

/// Widens the gaps between columns until every label has somewhere to go.
///
/// **This is the difference between a diagram a person can read and one where the words run over
/// the lifelines.** konoma does not wrap text — §2-5 puts `wrappingWidth` in the not-implemented
/// list, because it is a px-space idea and mermaid's own `sequence.wrap` is `false` by default —
/// so the only way a long message label can be legible is for its two participants to stand far
/// enough apart. mermaid grows `actorMargin` for the same reason.
///
/// Each demand is "these two axes must be at least this far apart"; a demand that spans several
/// columns shares the shortfall equally between the gaps it crosses. They are applied
/// shortest-span first, so a tight local demand is satisfied before a wide one that would
/// otherwise absorb it — and a wide one then measures the span the local ones already produced.
fn widen_for_text(diagram: &SequenceDiagram, columns: &[Column], gaps: &mut [f64]) {
    let n = columns.len();
    let index: HashMap<&str, usize> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    let half = |i: usize| columns[i].size.w / 2.0;

    // `(left column, right column, how far apart their axes must be)`.
    let mut wants: Vec<(usize, usize, f64)> = Vec::new();
    let mut want = |lo: usize, hi: usize, need: f64| {
        if lo < hi && hi < n {
            wants.push((lo, hi, need));
        }
    };
    for event in &diagram.events {
        match event {
            Event::Message(m) => {
                let (Some(&i), Some(&j)) = (index.get(m.from.as_str()), index.get(m.to.as_str()))
                else {
                    continue;
                };
                let label = Label::measure(&m.text);
                if label.is_blank() {
                    continue;
                }
                if i == j {
                    // A self-message's loop and its label both sit to the right of the axis.
                    if i + 1 < n {
                        want(
                            i,
                            i + 1,
                            SELF_WIDTH
                                + LABEL_GAP
                                + super::LABEL_PAD_X * 2.0
                                + label.width
                                + half(i + 1),
                        );
                    }
                } else {
                    want(i.min(j), i.max(j), label.width + super::LABEL_PAD_X * 2.0);
                }
            }
            Event::Note(note) => {
                let cols: Vec<usize> = note
                    .actors
                    .iter()
                    .filter_map(|a| index.get(a.as_str()).copied())
                    .collect();
                let Some(&first) = cols.first() else { continue };
                let label = Label::measure(&note.text);
                let w = shapes::size(Glyph::Note, Size::new(label.width, label.height)).w;
                match note.placement {
                    Placement::Over if cols.len() >= 2 => {
                        let (lo, hi) = (cols[0].min(cols[1]), cols[0].max(cols[1]));
                        want(lo, hi, w - NOTE_OVERHANG * 2.0);
                    }
                    // A note centred on one axis reaches half its width to each side.
                    Placement::Over => {
                        if first > 0 {
                            want(first - 1, first, w / 2.0 + half(first - 1) + NOTE_GAP);
                        }
                        if first + 1 < n {
                            want(first, first + 1, w / 2.0 + half(first + 1) + NOTE_GAP);
                        }
                    }
                    Placement::RightOf => {
                        if first + 1 < n {
                            want(
                                first,
                                first + 1,
                                NOTE_SIDE_GAP + w + half(first + 1) + NOTE_GAP,
                            );
                        }
                    }
                    Placement::LeftOf => {
                        if first > 0 {
                            want(
                                first - 1,
                                first,
                                NOTE_SIDE_GAP + w + half(first - 1) + NOTE_GAP,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    wants.sort_by_key(|(lo, hi, _)| hi - lo);
    for (lo, hi, need) in wants {
        let span: f64 = half(lo)
            + half(hi)
            + (lo..hi).map(|g| gaps[g]).sum::<f64>()
            + ((lo + 1)..hi).map(|c| columns[c].size.w).sum::<f64>();
        if need > span {
            let extra = (need - span) / (hi - lo) as f64;
            for gap in gaps.iter_mut().take(hi).skip(lo) {
                *gap += extra;
            }
        }
    }
}

/// The walk down the event stream.
struct Walk<'a> {
    columns: &'a [Column],
    index: HashMap<String, usize>,
    nodes: Vec<PlacedNode>,
    edges: Vec<PlacedEdge>,
    frames: Vec<PlacedCluster>,
    open: Vec<OpenFrame>,
    bars: Vec<Vec<PlacedActivation>>,
    open_bars: Vec<Vec<usize>>,
    y: f64,
    body_top: f64,
    last_message_y: f64,
    auto: AutoNumber,
    next_number: f64,
    group_n: usize,
    note_n: usize,
    pending_create: Option<String>,
    pending_destroy: Option<String>,
    /// Assigned during the walk and read back by `lay_out`, because a created participant's box
    /// and a destroyed one's cross are both decided by a message rather than by a declaration.
    head_y: Vec<Option<f64>>,
    destroyed_y: Vec<Option<f64>>,
}

impl<'a> Walk<'a> {
    fn new(columns: &'a [Column], index: HashMap<String, usize>, body_top: f64) -> Walk<'a> {
        let n = columns.len();
        Walk {
            columns,
            index,
            nodes: Vec::new(),
            edges: Vec::new(),
            frames: Vec::new(),
            open: Vec::new(),
            bars: vec![Vec::new(); n],
            open_bars: vec![Vec::new(); n],
            y: body_top,
            body_top,
            last_message_y: body_top,
            auto: AutoNumber::Off,
            next_number: 1.0,
            group_n: 0,
            note_n: 0,
            pending_create: None,
            pending_destroy: None,
            head_y: vec![None; n],
            destroyed_y: vec![None; n],
        }
    }

    fn run(&mut self, diagram: &SequenceDiagram) {
        for event in &diagram.events {
            match event {
                Event::Message(m) => self.message(m),
                Event::Note(n) => self.note(n),
                Event::Activate(id) => self.activate(id),
                Event::Deactivate(id) => self.deactivate(id),
                Event::BlockStart { kind, title } => self.open_frame(*kind, title),
                Event::Section { kind, title } => self.section(kind.keyword(), title),
                Event::BlockEnd => self.close_frame(),
                Event::AutoNumber(cmd) => {
                    self.auto = *cmd;
                    if let AutoNumber::On { start, .. } = cmd {
                        self.next_number = *start;
                    }
                }
                Event::Create(id) => self.pending_create = Some(id.clone()),
                Event::Destroy(id) => self.pending_destroy = Some(id.clone()),
            }
        }
        // A block the parser let through unclosed cannot happen (it is a `ParseError`), but a
        // frame left open here would simply never be drawn, so close them rather than drop them.
        while !self.open.is_empty() {
            self.close_frame();
        }
    }

    /// Absorbs a rectangle into every frame that is currently open.
    fn absorb(&mut self, r: (f64, f64, f64, f64)) {
        for f in self.open.iter_mut() {
            f.contents = Some(match f.contents {
                None => r,
                Some((l, t, rr, b)) => (l.min(r.0), t.min(r.1), rr.max(r.2), b.max(r.3)),
            });
        }
    }

    /// Where a line attaches on a participant's axis, allowing for the bars open on it.
    fn attach(&self, col: usize, side: Side) -> f64 {
        let x = self.columns[col].x;
        let open = self.open_bars[col].len();
        if open == 0 {
            return x;
        }
        let left = x - ACTIVATION_WIDTH / 2.0;
        match side {
            Side::Left => left,
            Side::Right => left + open as f64 * ACTIVATION_WIDTH,
        }
    }

    fn message(&mut self, m: &sequence::Message) {
        let (Some(&from), Some(&to)) = (
            self.index.get(m.from.as_str()),
            self.index.get(m.to.as_str()),
        ) else {
            return;
        };
        self.y += MESSAGE_PAD;

        // A `create` is honoured by the next message, which is where the new participant's box is
        // drawn — so the room it needs has to be taken out of the cursor on both sides of the line.
        let creating = self
            .pending_create
            .take()
            .and_then(|id| self.index.get(id.as_str()).copied())
            .filter(|i| {
                self.columns[*i].created && self.head_y[*i].is_none() && (*i == to || *i == from)
            });
        let create_room = creating.map_or(0.0, |i| self.columns[i].size.h / 2.0 + 6.0);
        self.y += create_room;

        let label = Label::measure(&m.text);
        let has_label = !label.is_blank();
        let label_h = if has_label { label.height } else { 0.0 };
        let badge = self.next_badge().map(|text| {
            let l = Label::measure(&text);
            let r = (l.width.max(labels::line_height()) / 2.0) + BADGE_PAD;
            (l, r)
        });
        // A sequence number sits on the shaft, so the text above the shaft has to clear the top of
        // its disc. Without this the badge and the label share the same band and overlap on every
        // numbered message — which is what the crate this replaces does.
        let lift = match (&badge, has_label) {
            (Some((_, r)), true) => *r + 2.0,
            _ => 0.0,
        };

        let (points, label_center, bottom) = if from == to {
            let x = self.attach(from, Side::Right);
            let line_y = self.y;
            let foot = line_y + SELF_HEIGHT;
            let points = vec![
                Point::new(x, line_y),
                Point::new(x + SELF_WIDTH, line_y),
                Point::new(x + SELF_WIDTH, foot),
                Point::new(x, foot),
            ];
            // A self-message's label cannot go above its own line — the line is a loop, and there
            // is nothing above it but the message before. It goes beside the loop instead.
            // Clear of the loop by a whole `LABEL_PAD_X` more than the gap, so the label's own
            // box does not touch the shaft — `svg::label_meets_its_line` would otherwise decide
            // the words need a patch behind them and paint one.
            let center = Point::new(
                x + SELF_WIDTH + LABEL_GAP + super::LABEL_PAD_X + label.width / 2.0,
                line_y + SELF_HEIGHT / 2.0,
            );
            (points, center, foot)
        } else {
            let line_y = self.y + label_h + if has_label { LABEL_GAP + lift } else { 0.0 };
            let rightwards = self.columns[to].x > self.columns[from].x;
            let (side_from, side_to) = if rightwards {
                (Side::Right, Side::Left)
            } else {
                (Side::Left, Side::Right)
            };
            let x_from = self.attach(from, side_from);
            // A message that creates its target lands on the new box's edge, not on an axis that
            // does not exist above it yet.
            let x_to = match creating {
                Some(i) if i == to => {
                    let half = self.columns[to].size.w / 2.0;
                    if rightwards {
                        self.columns[to].x - half
                    } else {
                        self.columns[to].x + half
                    }
                }
                _ => self.attach(to, side_to),
            };
            let points = vec![Point::new(x_from, line_y), Point::new(x_to, line_y)];
            let center = Point::new(
                (x_from + x_to) / 2.0,
                line_y - LABEL_GAP - lift - label_h / 2.0,
            );
            (points, center, line_y)
        };

        let line_y = points[0].y;
        if let Some(i) = creating {
            self.head_y[i] = Some(line_y);
        }
        if let Some(id) = self.pending_destroy.take() {
            if let Some(&i) = self.index.get(id.as_str()) {
                if i == from || i == to {
                    self.destroyed_y[i] = Some(bottom);
                } else {
                    self.pending_destroy = Some(id);
                }
            }
        }

        let placed_label = has_label.then(|| PlacedEdgeLabel {
            center: label_center.clone(),
            size: Size::new(
                label.width + super::LABEL_PAD_X * 2.0,
                label.height + super::LABEL_PAD_Y * 2.0,
            ),
            label: label.clone(),
        });
        let placed_badge = badge.map(|(l, r)| {
            let toward = if points.len() > 1 && points[1].x >= points[0].x {
                1.0
            } else {
                -1.0
            };
            PlacedEdgeLabel {
                center: Point::new(points[0].x + toward * (r + 1.0), points[0].y),
                size: Size::new(r * 2.0, r * 2.0),
                label: l,
            }
        });

        let mut rect = bounds_of_points(&points);
        if let Some(l) = &placed_label {
            rect = union(rect, label_rect(l));
        }
        if let Some(b) = &placed_badge {
            rect = union(rect, label_rect(b));
        }
        self.absorb(rect);

        self.edges.push(PlacedEdge {
            from: m.from.clone(),
            to: m.to.clone(),
            points,
            tip_start: tip_of(m.signal.start),
            tip_end: tip_of(m.signal.end),
            stroke: match m.signal.line {
                Line::Solid => Stroke::Normal,
                Line::Dotted => Stroke::Dotted,
            },
            label: placed_label,
            start_label: None,
            end_label: None,
            badge: placed_badge,
            series: None,
        });

        self.last_message_y = bottom;
        self.y = bottom + MESSAGE_PAD + create_room;
    }

    /// The next sequence number, if `autonumber` is on.
    fn next_badge(&mut self) -> Option<String> {
        let AutoNumber::On { step, .. } = self.auto else {
            return None;
        };
        let n = self.next_number;
        self.next_number += step;
        Some(format_number(n))
    }

    fn note(&mut self, n: &Note) {
        let cols: Vec<usize> = n
            .actors
            .iter()
            .filter_map(|a| self.index.get(a.as_str()).copied())
            .collect();
        let Some(&first) = cols.first() else { return };
        self.y += NOTE_GAP;
        let label = Label::measure(&n.text);
        let natural = shapes::size(Glyph::Note, Size::new(label.width, label.height));
        let (center_x, width) = match n.placement {
            Placement::Over if cols.len() >= 2 => {
                let a = self.columns[cols[0]].x;
                let b = self.columns[cols[1]].x;
                let (lo, hi) = (a.min(b), a.max(b));
                let span = (hi - lo) + NOTE_OVERHANG * 2.0;
                ((lo + hi) / 2.0, natural.w.max(span))
            }
            Placement::Over => (self.columns[first].x, natural.w),
            Placement::LeftOf => (
                self.columns[first].x - NOTE_SIDE_GAP - natural.w / 2.0,
                natural.w,
            ),
            Placement::RightOf => (
                self.columns[first].x + NOTE_SIDE_GAP + natural.w / 2.0,
                natural.w,
            ),
        };
        let size = Size::new(width, natural.h);
        let center = Point::new(center_x, self.y + size.h / 2.0);
        self.absorb((
            center.x - size.w / 2.0,
            center.y - size.h / 2.0,
            center.x + size.w / 2.0,
            center.y + size.h / 2.0,
        ));
        self.nodes.push(PlacedNode {
            id: format!("note#{}", self.note_n),
            shape: Glyph::Note,
            center,
            size,
            label,
            panel: None,
            series: None,
            mark: None,
        });
        self.note_n += 1;
        self.y += size.h + NOTE_GAP;
        // A note does not move `last_message_y`: an `activate` after a note still opens its bar at
        // the message that caused it, which is what upstream's `getVerticalPos` gives too.
    }

    fn activate(&mut self, id: &str) {
        let Some(&i) = self.index.get(id) else { return };
        let depth = self.open_bars[i].len();
        self.bars[i].push(PlacedActivation {
            depth,
            top: self.last_message_y,
            bottom: self.last_message_y,
        });
        let at = self.bars[i].len() - 1;
        self.open_bars[i].push(at);
    }

    fn deactivate(&mut self, id: &str) {
        let Some(&i) = self.index.get(id) else { return };
        if let Some(at) = self.open_bars[i].pop() {
            self.bars[i][at].bottom = self.last_message_y;
        }
    }

    fn open_frame(&mut self, kind: BlockKind, title: &str) {
        self.y += GROUP_GAP;
        let text = frame_text(kind.keyword(), title);
        let label = Label::measure(&text);
        let parent = self.open.last().map(|f| f.id.clone());
        let depth = self.open.len();
        self.open.push(OpenFrame {
            id: format!("g#{}", self.group_n),
            title: label.clone(),
            top: self.y,
            depth,
            parent,
            contents: None,
            sections: Vec::new(),
        });
        self.group_n += 1;
        self.y += band(&label);
    }

    fn section(&mut self, keyword: &str, title: &str) {
        if self.open.is_empty() {
            return;
        }
        self.y += SECTION_GAP;
        let label = Label::measure(&frame_text(keyword, title));
        let at = self.y;
        let height = band(&label);
        if let Some(f) = self.open.last_mut() {
            f.sections.push(ClusterSection {
                y: at,
                title: label,
            });
        }
        self.y += height;
    }

    fn close_frame(&mut self) {
        let Some(f) = self.open.pop() else { return };
        self.y += GROUP_PAD_BOTTOM;
        let bottom = self.y;
        let (mut left, mut right) = match f.contents {
            Some((l, _, r, _)) => (l - GROUP_PAD_X, r + GROUP_PAD_X),
            // A block with nothing in it still means something ("this happens zero times"), so it
            // is drawn across the participants rather than collapsed to a sliver.
            None => (
                self.columns
                    .iter()
                    .map(|c| c.x)
                    .fold(f64::INFINITY, f64::min)
                    - GROUP_PAD_X,
                self.columns
                    .iter()
                    .map(|c| c.x)
                    .fold(f64::NEG_INFINITY, f64::max)
                    + GROUP_PAD_X,
            ),
        };
        // Wide enough for the frame's own title and for every section title inside it, the same
        // way `read_clusters::fit_titles` grows a subgraph.
        let need = std::iter::once(&f.title)
            .chain(f.sections.iter().map(|s| &s.title))
            .map(|l| l.width)
            .fold(0.0_f64, f64::max)
            + clusters::TITLE_PAD_X;
        let short = need - (right - left);
        if short > 0.0 {
            left -= short / 2.0;
            right += short / 2.0;
        }
        let rect = (left, f.top, right, bottom);
        self.absorb(rect);
        self.frames.push(PlacedCluster {
            id: f.id,
            title: f.title,
            center: Point::new((left + right) / 2.0, (f.top + bottom) / 2.0),
            size: Size::new(right - left, bottom - f.top),
            parent: f.parent,
            depth: f.depth,
            dashed: true,
            filled: false,
            sections: f.sections,
        });
    }
}

/// `loop Every minute`, or just `loop` when the author gave no text.
///
/// **One space, not two.** usvg collapses a run of whitespace inside a `<text>` before it shapes
/// it, so a double space would be measured at a width resvg then refuses to draw — which the
/// measuring instrument catches, and which is exactly the class of mismatch §6 exists to find.
fn frame_text(keyword: &str, title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        keyword.to_string()
    } else {
        format!("{keyword} {t}")
    }
}

/// A sequence number as it is drawn: an integer with no point, and at most two decimals otherwise
/// (mermaid's `NUM` token allows `autonumber 1.5 0.25`).
fn format_number(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        return format!("{}", v.round() as i64);
    }
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// What each message head is drawn as.
fn tip_of(head: Head) -> Tip {
    match head {
        Head::None => Tip::None,
        Head::Arrow => Tip::Arrow,
        Head::Cross => Tip::Cross,
        Head::Async => Tip::Async,
    }
}

fn bounds_of_points(points: &[Point]) -> (f64, f64, f64, f64) {
    let mut r = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for p in points {
        r = (r.0.min(p.x), r.1.min(p.y), r.2.max(p.x), r.3.max(p.y));
    }
    r
}

fn label_rect(l: &PlacedEdgeLabel) -> (f64, f64, f64, f64) {
    (
        l.center.x - l.size.w / 2.0,
        l.center.y - l.size.h / 2.0,
        l.center.x + l.size.w / 2.0,
        l.center.y + l.size.h / 2.0,
    )
}

fn union(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}
