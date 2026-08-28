//! Drawing a `gantt`.
//!
//! # The defect this is measured against
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §6-A records the crate drawing a **sixty-day chart on an
//! axis that runs to 2058**. That is what an unread date looks like once it has been turned into
//! a number, and it is why the parser refuses a date it cannot read rather than substituting one
//! (`gantt::compile`). Here the axis is derived from the tasks' own extent, so the picture cannot
//! disagree with the dates in it — and [`super::tests`] states exactly that as an invariant.
//!
//! # One place maps a date onto the page
//!
//! [`Scale::x`], the same discipline `xychart`'s `value_to_y` follows: every bar's left edge,
//! every bar's width, every grid line and every tick comes out of one function, so "a bar's width
//! is its duration" is a property of one line rather than a coincidence between four.
//!
//! # A milestone is a moment
//!
//! A `milestone` task has no duration, so it is drawn as a diamond *on* its date rather than as a
//! bar of some minimum width. A minimum-width bar would say the milestone took time.

use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::gantt::time::{self, Instant};
use crate::preview::mermaid::gantt::{Gantt, Task};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

use super::band;
use super::chart::{label_node, rule, TICK_GAP, TICK_LEN};
use super::{
    normalise, svg, Diagram, Glyph, Label, PlacedCluster, PlacedNode, RenderError, Size, Theme,
};

/// How wide the plotting area is before the tick labels ask for more.
pub const PLOT_WIDTH: f64 = 520.0;

/// How tall one task's row is.
pub const ROW_HEIGHT: f64 = 24.0;

/// Blank space between two rows.
pub const ROW_GAP: f64 = 6.0;

/// Blank space between a section frame and the rows in it.
///
/// **Less than half [`ROW_GAP`]**, because the sections here are stacked: a frame that reaches
/// further than half the gap meets the frame of the section below it.
pub const SECTION_PAD: f64 = 2.0;

/// Blank space between the axis and the first row.
pub const AXIS_GAP: f64 = 10.0;

/// Side of a milestone's diamond.
pub const MILESTONE: f64 = 16.0;

/// How many ticks to aim for along the axis.
pub const TICKS: usize = 6;

/// Reads a mermaid gantt chart and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let gantt = crate::preview::mermaid::gantt::parse(code)?;
    let diagram = lay_out(&gantt)?;
    Ok(svg::emit(&diagram, &Theme::named(theme)))
}

/// Maps a date onto the page. **The only place the time scale is applied.**
pub struct Scale {
    /// Left edge of the plot.
    pub left: f64,
    /// Right edge.
    pub right: f64,
    /// The earliest instant on the axis.
    pub from: Instant,
    /// The latest.
    pub to: Instant,
}

impl Scale {
    /// Where an instant sits, in px.
    pub fn x(&self, at: Instant) -> f64 {
        let span = (self.to.millis() - self.from.millis()) as f64;
        if span <= 0.0 {
            return self.left;
        }
        self.left + (at.millis() - self.from.millis()) as f64 / span * (self.right - self.left)
    }
}

/// Works out where every bar, milestone, tick and section frame goes.
pub fn lay_out(gantt: &Gantt) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    let Some((from, to)) = gantt.extent() else {
        return Err(RenderError::NothingToDraw);
    };
    if to.millis() <= from.millis() {
        // Every task on one instant. There is no axis to draw, and a chart drawn on a
        // zero-width one puts every bar on the same pixel.
        return Err(RenderError::ChartHasNoExtent {
            what: "every task starts and ends at the same moment",
        });
    }

    // The row labels decide where the plot starts.
    let names: Vec<Label> = gantt
        .tasks
        .iter()
        .map(|t| Label::measure(&t.name))
        .collect();
    let widest = names.iter().map(|l| l.width).fold(0.0_f64, f64::max);
    let left = widest + TICK_GAP * 2.0;

    // The ticks decide how wide the plot has to be: dates are long words, and two of them
    // touching is the commonest way a time axis becomes unreadable.
    let ticks = tick_dates(from, to, &gantt.tick_interval);
    let tick_labels: Vec<Label> = ticks
        .iter()
        .map(|t| Label::measure(&time::format(*t, &gantt.axis_format)))
        .collect();
    let widest_tick = tick_labels.iter().map(|l| l.width).fold(0.0_f64, f64::max);
    let want = (widest_tick + TICK_GAP * 2.0) * ticks.len().max(1) as f64;
    let scale = Scale {
        left,
        right: left + PLOT_WIDTH.max(want),
        from,
        to,
    };

    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut edges = Vec::new();

    // --- the axis -----------------------------------------------------------------------------
    let axis_h = tick_labels.iter().map(|l| l.height).fold(0.0_f64, f64::max) + TICK_LEN + TICK_GAP;
    let top = axis_h + AXIS_GAP;
    // How far the grid lines reach: every row, plus a title band for every section after the
    // first. Computed here because the ticks are drawn before the rows are placed.
    let extra: f64 = section_bands(gantt);
    let bottom = top + gantt.tasks.len() as f64 * (ROW_HEIGHT + ROW_GAP) + extra;

    for (t, label) in ticks.iter().zip(tick_labels.iter()) {
        let x = scale.x(*t);
        edges.push(rule(
            Point::new(x, axis_h - TICK_LEN),
            Point::new(x, bottom),
            Stroke::Dotted,
            None,
        ));
        if let Some(n) = label_node(
            format!("tick#{}", time::format(*t, "%Y-%m-%dT%H:%M")),
            label.clone(),
            Point::new(x, axis_h - TICK_LEN - TICK_GAP - label.height / 2.0),
            None,
        ) {
            nodes.push(n);
        }
    }

    // --- the rows -----------------------------------------------------------------------------
    let mut spans: Vec<(usize, f64, f64)> = Vec::new();
    let mut y = top;
    let mut open_section: Option<&str> = None;
    for (i, task) in gantt.tasks.iter().enumerate() {
        // The sections are stacked, so the next one's title band is reserved before its first row
        // — otherwise it grows up into the section above it (`band::title_room`).
        if open_section != Some(task.section.as_str()) {
            if open_section.is_some() {
                y += band::title_room(&task.section) + SECTION_PAD * 2.0;
            }
            open_section = Some(task.section.as_str());
        }
        let cy = y + ROW_HEIGHT / 2.0;
        if let Some(n) = label_node(
            format!("task#{i}#name"),
            names[i].clone(),
            Point::new(widest - names[i].width / 2.0, cy),
            None,
        ) {
            nodes.push(n);
        }
        if task.tags.milestone {
            nodes.push(PlacedNode {
                id: format!("task#{i}"),
                shape: Glyph::Flow(Shape::Diamond),
                center: Point::new(scale.x(task.start), cy),
                size: Size::new(MILESTONE, MILESTONE),
                label: Label::measure(""),
                panel: None,
                series: Some(series_of(task)),
                mark: None,
                style: None,
            });
        } else {
            let x0 = scale.x(task.start);
            let x1 = scale.x(task.end);
            // A task of no width would be invisible; one pixel of bar is still a bar, and the
            // *milestone* is what a zero-length task is supposed to be drawn as.
            let w = (x1 - x0).max(1.0);
            nodes.push(PlacedNode {
                id: format!("task#{i}"),
                shape: Glyph::ChartBar,
                center: Point::new(x0 + w / 2.0, cy),
                size: Size::new(w, ROW_HEIGHT * 0.7),
                label: Label::measure(""),
                panel: None,
                series: Some(series_of(task)),
                mark: None,
                style: None,
            });
        }
        spans.push((i, y, y + ROW_HEIGHT));
        y += ROW_HEIGHT + ROW_GAP;
    }

    // --- the section frames -------------------------------------------------------------------
    //
    // As wide as what is actually on the rows, not as wide as the *plot*: a milestone straddles
    // its own date, so a milestone on the last day of the chart reaches half a diamond past the
    // plot's right edge, and a frame drawn to the plot would cut it off.
    let content_right = nodes
        .iter()
        .map(|n| n.bounds().2)
        .fold(scale.right, f64::max);
    let mut sections: Vec<PlacedCluster> = Vec::new();
    let mut run: Option<(String, f64, f64)> = None;
    // The band's **ordinal**, counted over every run including the blank-titled ones dropped
    // below, is what makes its id unique. A section name may be written twice with another
    // section between the two, which is two bands — and two frames carrying one id is not a
    // cosmetic problem: `Diagram::cluster` answers with the first of them, so a lookup for the
    // second silently reads the first and a test that used it reported a correct chart as wrong.
    let mut ordinal = 0usize;
    for (i, start, end) in &spans {
        let name = gantt.tasks[*i].section.clone();
        match &mut run {
            Some((open, _, last)) if *open == name => *last = *end,
            _ => {
                if let Some((open, t0, t1)) = run.take() {
                    sections.push(section(ordinal, &open, t0, t1, 0.0, content_right));
                    ordinal += 1;
                }
                run = Some((name, *start, *end));
            }
        }
    }
    if let Some((open, t0, t1)) = run.take() {
        sections.push(section(ordinal, &open, t0, t1, 0.0, content_right));
    }
    sections.retain(|s| !s.title.is_blank());

    let mut diagram = Diagram {
        nodes,
        edges,
        clusters: sections,
        ..Diagram::default()
    };
    band::add_title(&mut diagram, gantt.preamble.title.as_deref());
    normalise(&mut diagram);
    Ok(diagram)
}

/// How much extra height the section title bands add, all together.
fn section_bands(gantt: &Gantt) -> f64 {
    let mut total = 0.0;
    let mut open: Option<&str> = None;
    for task in &gantt.tasks {
        if open != Some(task.section.as_str()) {
            if open.is_some() {
                total += band::title_room(&task.section) + SECTION_PAD * 2.0;
            }
            open = Some(task.section.as_str());
        }
    }
    total
}

/// A section's frame across the rows in it.
fn section(
    ordinal: usize,
    title: &str,
    top: f64,
    bottom: f64,
    left: f64,
    right: f64,
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

/// Which palette entry a bar takes. The four tags mermaid gives a task are four states of the
/// same thing, and a reader wants to tell them apart at a glance.
fn series_of(task: &Task) -> usize {
    match (task.tags.crit, task.tags.done, task.tags.active) {
        (true, _, _) => 3,
        (_, true, _) => 1,
        (_, _, true) => 2,
        _ => 0,
    }
}

/// The dates the axis is ticked at.
///
/// `tickInterval` when the source gave one, and otherwise the largest of day / week / month /
/// year that fits about [`TICKS`] ticks into the range — which is what a reader wants and what d3
/// does with a time scale.
pub fn tick_dates(from: Instant, to: Instant, interval: &str) -> Vec<Instant> {
    let span = (to.millis() - from.millis()).max(1);
    let (count, unit) = read_interval(interval).unwrap_or_else(|| {
        let day = time::MS_PER_DAY;
        let days = span / day;
        if days <= 1 {
            (6, time::Unit::Hour)
        } else if days <= TICKS as i64 * 2 {
            (1, time::Unit::Day)
        } else if days <= TICKS as i64 * 14 {
            (1, time::Unit::Week)
        } else if days <= TICKS as i64 * 90 {
            (1, time::Unit::Month)
        } else {
            (1, time::Unit::Year)
        }
    });
    let mut out = Vec::new();
    let mut at = floor_to(from, unit);
    if at < from {
        at = at.add(count.max(1), unit);
    }
    // The guard is on the count, so a unit that cannot advance cannot spin.
    while at <= to && out.len() < 64 {
        if at >= from {
            out.push(at);
        }
        let next = at.add(count.max(1), unit);
        if next <= at {
            break;
        }
        at = next;
    }
    if out.is_empty() {
        out.push(from);
        out.push(to);
    }
    out
}

/// `tickInterval 1week` — `/^([1-9][0-9]*)(millisecond|second|minute|hour|day|week|month)$/`.
fn read_interval(text: &str) -> Option<(i64, time::Unit)> {
    let t = text.trim();
    let digits = t.find(|c: char| !c.is_ascii_digit())?;
    if digits == 0 {
        return None;
    }
    let n: i64 = t[..digits].parse().ok()?;
    let unit = match t[digits..].trim().to_ascii_lowercase().as_str() {
        "millisecond" => time::Unit::Millisecond,
        "second" => time::Unit::Second,
        "minute" => time::Unit::Minute,
        "hour" => time::Unit::Hour,
        "day" => time::Unit::Day,
        "week" => time::Unit::Week,
        "month" => time::Unit::Month,
        _ => return None,
    };
    Some((n, unit))
}

/// The start of the `unit` `at` falls in, so the ticks land on round dates rather than on the
/// first task's start time.
fn floor_to(at: Instant, unit: time::Unit) -> Instant {
    let (y, m, _, _, _, _, _) = at.civil();
    match unit {
        time::Unit::Year => Instant::from_civil(y, 1, 1, 0, 0, 0, 0),
        time::Unit::Month => Instant::from_civil(y, m, 1, 0, 0, 0, 0),
        // A week starts on the day the source named; `weekday` defaults to Sunday upstream.
        time::Unit::Week | time::Unit::Day => at.start_of_day(),
        _ => at,
    }
}
