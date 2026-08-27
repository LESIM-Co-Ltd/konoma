//! Drawing a `timeline`.
//!
//! # The defect this is measured against
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §6-A records the crate drawing a timeline in **white text
//! on pastel** — a diagram whose words cannot be read, which is the worst kind of failure because
//! the picture still looks finished. konoma paints the event boxes in the palette's series
//! colours and writes on them in [`Theme::series_text`], the one colour every series colour was
//! chosen to carry, and [`super::tests`] states that as a property rather than as a hope.
//!
//! [`Theme::series_text`]: super::Theme::series_text
//!
//! # Both directions are drawn
//!
//! `timeline TD` is a documented keyword with a renderer of its own upstream
//! (`timelineRendererVertical`), so reading it and drawing the other way round would be a
//! statement that does nothing — the shape §2-3 warns about. The two branches share everything
//! except which axis the periods run along.

use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;
use crate::preview::mermaid::timeline::{Direction, Timeline};

use super::band;
use super::chart::rule;
use super::{
    normalise, shapes, svg, Diagram, Glyph, Label, PlacedCluster, PlacedNode, RenderError, Size,
    Theme,
};

/// Blank space between two periods.
pub const PERIOD_GAP: f64 = 18.0;

/// Blank space between the spine and the first event under it.
pub const SPINE_GAP: f64 = 14.0;

/// Blank space between two events of the same period.
pub const EVENT_GAP: f64 = 6.0;

/// Blank space between a section frame and what is inside it.
///
/// **Less than half [`PERIOD_GAP`]**, for the reason `journey::SECTION_PAD` gives: two frames that
/// meet in the gap between their periods read as one section.
pub const SECTION_PAD: f64 = 8.0;

/// The narrowest an event box may be.
pub const MIN_EVENT_WIDTH: f64 = 90.0;

/// Reads a mermaid timeline and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let timeline = crate::preview::mermaid::timeline::parse(code)?;
    let diagram = lay_out(&timeline)?;
    Ok(svg::emit(&diagram, &Theme::named(theme)))
}

/// Works out where every period, event and section frame goes.
pub fn lay_out(timeline: &Timeline) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if timeline.periods.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut edges = Vec::new();
    let mut spans: Vec<(usize, f64, f64)> = Vec::new();

    // A period's column is as wide as the widest thing in it, so nothing overlaps its neighbour.
    let periods: Vec<Label> = timeline
        .periods
        .iter()
        .map(|p| Label::measure(&p.name))
        .collect();
    let events: Vec<Vec<Label>> = timeline
        .periods
        .iter()
        .map(|p| p.events.iter().map(|e| Label::measure(e)).collect())
        .collect();

    let horizontal = timeline.direction == Direction::LeftToRight;
    let mut cursor = 0.0_f64;
    let mut extent = 0.0_f64;

    let mut open_section: Option<&str> = None;
    for i in 0..timeline.periods.len() {
        // A top-to-bottom timeline stacks its section frames, so the next one's title band has to
        // be reserved before its first period is placed — see `band::title_room`.
        if !horizontal {
            let section = timeline.periods[i].section.as_str();
            if open_section != Some(section) {
                if open_section.is_some() {
                    cursor += band::title_room(section) + SECTION_PAD * 2.0;
                }
                open_section = Some(section);
            }
        }
        let widest = events[i]
            .iter()
            .map(|l| l.width)
            .fold(periods[i].width, f64::max);
        let head = shapes::size(
            Glyph::Flow(Shape::RoundedRect),
            Size::new(periods[i].width, periods[i].height),
        );
        let event_w = (widest + shapes::PADDING * 2.0).max(MIN_EVENT_WIDTH);
        let event_sizes: Vec<Size> = events[i]
            .iter()
            .map(|l| Size::new(event_w, l.height + shapes::PADDING))
            .collect();

        if horizontal {
            let column = event_w.max(head.w);
            let cx = cursor + column / 2.0;
            nodes.push(period_node(
                i,
                periods[i].clone(),
                Point::new(cx, head.h / 2.0),
                head,
            ));
            let mut y = head.h + SPINE_GAP;
            for (k, size) in event_sizes.iter().enumerate() {
                nodes.push(event_node(
                    i,
                    k,
                    events[i][k].clone(),
                    Point::new(cx, y + size.h / 2.0),
                    *size,
                ));
                y += size.h + EVENT_GAP;
            }
            // The tie between a period and the events under it: one line down its own column, so
            // an event can never read as belonging to the period beside it.
            if !event_sizes.is_empty() {
                edges.push(rule(
                    Point::new(cx, head.h),
                    Point::new(cx, head.h + SPINE_GAP),
                    Stroke::Normal,
                    None,
                ));
            }
            spans.push((i, cursor, cursor + column));
            extent = extent.max(y - EVENT_GAP);
            cursor += column + PERIOD_GAP;
        } else {
            let row = head.h.max(
                event_sizes.iter().map(|s| s.h).sum::<f64>()
                    + EVENT_GAP * event_sizes.len().saturating_sub(1) as f64,
            );
            let cy = cursor + row / 2.0;
            nodes.push(period_node(
                i,
                periods[i].clone(),
                Point::new(head.w / 2.0, cy),
                head,
            ));
            let mut y = cursor;
            let x = head.w + SPINE_GAP;
            for (k, size) in event_sizes.iter().enumerate() {
                nodes.push(event_node(
                    i,
                    k,
                    events[i][k].clone(),
                    Point::new(x + size.w / 2.0, y + size.h / 2.0),
                    *size,
                ));
                y += size.h + EVENT_GAP;
            }
            if !event_sizes.is_empty() {
                edges.push(rule(
                    Point::new(head.w, cy),
                    Point::new(x, cy),
                    Stroke::Normal,
                    None,
                ));
            }
            spans.push((i, cursor, cursor + row));
            extent = extent.max(x + event_w);
            cursor += row + PERIOD_GAP;
        }
    }

    // --- the section frames -------------------------------------------------------------------
    let mut sections: Vec<PlacedCluster> = Vec::new();
    let mut run: Option<(String, f64, f64)> = None;
    // The band's **ordinal**, counted over every run including the blank-titled ones dropped
    // below, is what makes its id unique: a section name written twice with another between the
    // two is two bands, and two frames carrying one id make `Diagram::cluster` answer with the
    // first of them whichever was asked for.
    let mut ordinal = 0usize;
    for (i, start, end) in &spans {
        let name = timeline.periods[*i].section.clone();
        match &mut run {
            Some((open, _, last)) if *open == name => *last = *end,
            _ => {
                if let Some((open, from, to)) = run.take() {
                    sections.push(frame(ordinal, &open, from, to, extent, horizontal));
                    ordinal += 1;
                }
                run = Some((name, *start, *end));
            }
        }
    }
    if let Some((open, from, to)) = run.take() {
        sections.push(frame(ordinal, &open, from, to, extent, horizontal));
    }
    sections.retain(|s| !s.title.is_blank());

    let mut diagram = Diagram {
        nodes,
        edges,
        clusters: sections,
        ..Diagram::default()
    };
    band::add_title(&mut diagram, timeline.preamble.title.as_deref());
    normalise(&mut diagram);
    Ok(diagram)
}

/// A period's own box, on the spine.
fn period_node(i: usize, label: Label, center: Point, size: Size) -> PlacedNode {
    PlacedNode {
        id: format!("period#{i}"),
        shape: Glyph::Flow(Shape::RoundedRect),
        center,
        size,
        label,
        panel: None,
        series: None,
        mark: None,
    }
}

/// One event, painted in its period's series colour so a column reads as a column.
fn event_node(i: usize, k: usize, label: Label, center: Point, size: Size) -> PlacedNode {
    PlacedNode {
        id: format!("period#{i}#event#{k}"),
        shape: Glyph::Flow(Shape::Rect),
        center,
        size,
        label,
        panel: None,
        series: Some(i),
        mark: None,
    }
}

/// A section's frame across the periods in it.
fn frame(
    ordinal: usize,
    title: &str,
    from: f64,
    to: f64,
    extent: f64,
    horizontal: bool,
) -> PlacedCluster {
    let (l, t, r, b) = if horizontal {
        (
            from - SECTION_PAD,
            -SECTION_PAD,
            to + SECTION_PAD,
            extent + SECTION_PAD,
        )
    } else {
        (
            -SECTION_PAD,
            from - SECTION_PAD,
            extent + SECTION_PAD,
            to + SECTION_PAD,
        )
    };
    band::frame(format!("section#{ordinal}#{title}"), title, l, t, r, b)
}
