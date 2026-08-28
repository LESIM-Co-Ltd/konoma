//! Drawing a `journey`.
//!
//! # Not a graph, so not `lay_out_spec`
//!
//! A user journey is a row of tasks in source order, banded by section. There is no ranking to do
//! and no crossings to reduce, so — as with the seven charts and the sequence diagram — the
//! geometry is worked out in one pass and everything below layout is shared.
//!
//! # The face is the diagram
//!
//! A journey's score is the whole point of the diagram: it is what makes it a journey rather than
//! a list of steps. So the score is drawn as a face ([`Glyph::Face`]) whose **mouth curve** is the
//! number — a smile at 5, a straight line at 3, a frown at 1 — rather than as a colour or a digit.
//! A reader takes the shape of a curve in at a glance; a colour needs a legend, and a digit needs
//! reading.
//!
//! A task with an unreadable score (`journeyDb` gets `NaN` there) draws **no face at all** rather
//! than a neutral one, because a neutral face is a claim about how the step felt.
//!
//! # A section is a frame
//!
//! Sections become [`PlacedCluster`]s, which means the six cluster invariants apply to them
//! without a line being written here: a section holds its own tasks, and two sections do not
//! overlap.

use crate::preview::mermaid::journey::Journey;
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

use super::band;
use super::chart::label_node;
use super::shapes::Mark;
use super::{
    normalise, shapes, svg, Diagram, Glyph, Label, PlacedCluster, PlacedNode, RenderError, Size,
    Theme,
};

/// Blank space between two tasks.
pub const TASK_GAP: f64 = 14.0;

/// Blank space between a section frame and the tasks in it.
///
/// **Less than half [`TASK_GAP`]**, or two neighbouring frames would meet in the gap between the
/// last task of one section and the first of the next — which reads as one section and which
/// `check_unrelated_clusters_do_not_overlap` catches.
pub const SECTION_PAD: f64 = 6.0;

/// Blank space between the face and the task box under it.
pub const FACE_GAP: f64 = 6.0;

/// Blank space between the task box and the names under it.
pub const PEOPLE_GAP: f64 = 4.0;

/// The narrowest a task's box may be, so a row of one-word tasks stays even.
pub const MIN_TASK_WIDTH: f64 = 64.0;

/// Reads a mermaid user journey and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let journey = crate::preview::mermaid::journey::parse(code)?;
    let diagram = lay_out(&journey)?;
    Ok(svg::emit(&diagram, &Theme::named(theme)))
}

/// Works out where every task, face, name and section frame goes.
pub fn lay_out(journey: &Journey) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if journey.tasks.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    let face = shapes::FACE_RADIUS * 2.0;
    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut boxes: Vec<(usize, f64, f64)> = Vec::new(); // (task index, left, right)

    // Every task box is the same height, so the row reads as a row; the width is the widest of
    // what has to fit inside it.
    let names: Vec<Label> = journey
        .tasks
        .iter()
        .map(|t| Label::measure(&t.name))
        .collect();
    let people: Vec<Label> = journey
        .tasks
        .iter()
        .map(|t| Label::measure(&t.people.join(", ")))
        .collect();
    let box_h = shapes::size(
        Glyph::Flow(crate::preview::mermaid::flowchart::Shape::RoundedRect),
        Size::new(0.0, names.iter().map(|l| l.height).fold(0.0_f64, f64::max)),
    )
    .h;

    let face_top = 0.0_f64;
    let box_top = face_top + face + FACE_GAP;
    let people_top = box_top + box_h + PEOPLE_GAP;

    let mut x = 0.0_f64;
    for (i, task) in journey.tasks.iter().enumerate() {
        let w = names[i]
            .width
            .max(people[i].width)
            .max(face)
            .max(MIN_TASK_WIDTH)
            + shapes::PADDING * 2.0;
        let cx = x + w / 2.0;

        if let Some(score) = task.score {
            nodes.push(PlacedNode {
                id: format!("task#{i}#face"),
                shape: Glyph::Face,
                center: Point::new(cx, face_top + face / 2.0),
                size: Size::new(face, face),
                label: Label::measure(""),
                panel: None,
                // **No series.** The palette is *categorical* — five colours chosen to be told
                // apart, not to be ordered — and a score is an ordered quantity. Painting the
                // faces from it would invite a reader to take "teal" for good and "blue" for bad,
                // which the palette does not mean. The mouth carries the number; the colour would
                // only add a second, false, reading of it.
                series: None,
                mark: Some(Mark::Face { score }),
                style: None,
            });
        }
        nodes.push(PlacedNode {
            id: format!("task#{i}"),
            shape: Glyph::Flow(crate::preview::mermaid::flowchart::Shape::RoundedRect),
            center: Point::new(cx, box_top + box_h / 2.0),
            size: Size::new(w, box_h),
            label: names[i].clone(),
            panel: None,
            series: None,
            mark: None,
            style: None,
        });
        if let Some(n) = label_node(
            format!("task#{i}#people"),
            people[i].clone(),
            Point::new(cx, people_top + people[i].height / 2.0),
            None,
        ) {
            nodes.push(n);
        }
        boxes.push((i, x, x + w));
        x += w + TASK_GAP;
    }

    // --- the section frames -------------------------------------------------------------------
    //
    // One per run of tasks sharing a section, so a section written twice draws twice — which is
    // what the source says happened.
    let mut sections: Vec<PlacedCluster> = Vec::new();
    let people_h = people.iter().map(|l| l.height).fold(0.0_f64, f64::max);
    let bottom = people_top + people_h;
    let mut run: Option<(String, f64, f64)> = None;
    // The band's **ordinal**, counted over every run including the blank-titled ones dropped
    // below, is what makes its id unique: a section name written twice with another between the
    // two is two bands, and two frames carrying one id make `Diagram::cluster` answer with the
    // first of them whichever was asked for.
    let mut ordinal = 0usize;
    for (i, left, right) in &boxes {
        let name = journey.tasks[*i].section.clone();
        match &mut run {
            Some((open, _, end)) if *open == name => *end = *right,
            _ => {
                if let Some((open, start, end)) = run.take() {
                    sections.push(section_frame(ordinal, &open, start, end, face_top, bottom));
                    ordinal += 1;
                }
                run = Some((name, *left, *right));
            }
        }
    }
    if let Some((open, start, end)) = run.take() {
        sections.push(section_frame(ordinal, &open, start, end, face_top, bottom));
    }
    sections.retain(|s| !s.title.is_blank());

    let mut diagram = Diagram {
        nodes,
        clusters: sections,
        ..Diagram::default()
    };
    // The frames grew upwards to hold their titles, so the title above them is placed last.
    band::add_title(&mut diagram, journey.preamble.title.as_deref());
    normalise(&mut diagram);
    Ok(diagram)
}

/// A section's frame, grown to hold its title above the tasks in it.
fn section_frame(
    ordinal: usize,
    title: &str,
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
) -> PlacedCluster {
    band::frame(
        format!("section#{ordinal}#{title}"),
        title,
        left - SECTION_PAD,
        top - SECTION_PAD,
        right + SECTION_PAD,
        bottom + SECTION_PAD,
    )
}
