//! Drawing a `sankey-beta`.
//!
//! # The claim a Sankey makes, and what has to be true for it to hold
//!
//! Every band's thickness is its value at the same scale, and the bands meeting a node stack up to
//! exactly that node's own height. So the picture says "this much came in, this much went out"
//! without a single number on it — and it says it only if the *scale is one scale*. [`Scale`] is
//! that one scale, and everything with a thickness goes through it.
//!
//! # Columns
//!
//! A node's column is the longest chain of links reaching it, which is d3-sankey's
//! `computeNodeDepths`; a node with nothing leaving it is then pushed to the last column, which is
//! d3-sankey's `justify` alignment and mermaid's default. Both are transcribed rather than
//! invented, because "how far right does this node sit" is the only part of the layout that is not
//! forced by the values.
//!
//! A cycle has no such order at all, and [`crate::preview::mermaid::chart::sankey::parse`] refuses
//! one before this module sees it.

use crate::preview::mermaid::chart::sankey::Sankey;
use crate::preview::mermaid::layout::Point;

use super::super::shapes::Mark;
use super::super::{normalise, Diagram, Glyph, Label, PlacedNode, RenderError, Size, Theme};
use super::{add_title, bar_node, label_node};

/// How wide the whole diagram is.
pub const WIDTH: f64 = 520.0;

/// How tall it is.
pub const HEIGHT: f64 = 300.0;

/// Width of the bar standing for a node.
pub const NODE_WIDTH: f64 = 12.0;

/// Blank space between two nodes stacked in the same column.
pub const NODE_PAD: f64 = 10.0;

/// Blank space between a node's bar and its name.
pub const LABEL_GAP: f64 = 5.0;

/// The thinnest a band may be drawn, so a small flow is a hairline rather than nothing.
pub const MIN_BAND: f64 = 1.0;

/// Value-to-pixels, computed once for the whole diagram.
///
/// A Sankey with two scales is not a Sankey with a rounding error, it is a Sankey that lies, so
/// this is a type rather than a number multiplied in three places.
#[derive(Debug, Clone, Copy)]
pub struct Scale(f64);

impl Scale {
    /// How tall `value` is, in px.
    pub fn height(&self, value: f64) -> f64 {
        value * self.0
    }
}

/// Reads a mermaid Sankey source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let sankey = crate::preview::mermaid::chart::sankey::parse(code)?;
    let diagram = lay_out(&sankey)?;
    Ok(super::super::svg::emit(&diagram, &Theme::named(theme)))
}

/// Works out where every node bar and every ribbon goes.
pub fn lay_out(sankey: &Sankey) -> Result<Diagram, RenderError> {
    if !crate::preview::mermaid::text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    let n = sankey.nodes.len();
    if n == 0 {
        return Err(RenderError::ChartHasNoExtent {
            what: "the diagram has no nodes",
        });
    }
    // A node's value is the larger of what enters it and what leaves it — d3-sankey's
    // `computeNodeValues`. For a node in the middle of a balanced flow the two are equal; for a
    // source or a sink only one of them exists.
    let values: Vec<f64> = (0..n)
        .map(|i| {
            let into: f64 = sankey
                .links
                .iter()
                .filter(|l| l.target == i)
                .map(|l| l.value)
                .sum();
            let out: f64 = sankey
                .links
                .iter()
                .filter(|l| l.source == i)
                .map(|l| l.value)
                .sum();
            into.max(out)
        })
        .collect();
    if values.iter().sum::<f64>() <= 0.0 {
        return Err(RenderError::ChartHasNoExtent {
            what: "every flow is zero, so no band has any thickness",
        });
    }

    let column = columns(sankey, n);
    let last = column.iter().copied().max().unwrap_or(0);

    // The scale: the fullest column decides it, because that is the column that has to fit.
    let mut column_total = vec![0.0_f64; last + 1];
    let mut column_count = vec![0usize; last + 1];
    for i in 0..n {
        column_total[column[i]] += values[i];
        column_count[column[i]] += 1;
    }
    let mut k = f64::INFINITY;
    for c in 0..=last {
        if column_count[c] == 0 || column_total[c] <= 0.0 {
            continue;
        }
        let room = HEIGHT - (column_count[c] as f64 - 1.0) * NODE_PAD;
        if room <= 0.0 {
            continue;
        }
        k = k.min(room / column_total[c]);
    }
    if !k.is_finite() || k <= 0.0 {
        return Err(RenderError::ChartHasNoExtent {
            what: "the flows do not fit in any height",
        });
    }
    let scale = Scale(k);

    // --- how wide the diagram has to be ----------------------------------------------------------
    //
    // A name is drawn beside its bar, in the gap between that column and the next, so **the names
    // decide the width** — the same rule `xychart` follows for its categories, and for the same
    // reason: a name that reaches past the middle of the gap is nearer to the *next* column's bar
    // than to its own, and a reader has no way to tell which bar it belongs to. The label's centre
    // sits `LABEL_GAP + w/2` from its own bar's centre and `gap + NODE_WIDTH/2 - LABEL_GAP - w/2`
    // from the next one's, so the gap has to be more than `w + 2 * LABEL_GAP`; the extra pixel
    // keeps a name that fits exactly from landing on the tie.
    //
    // **The gap before the last column holds two sets of names**, because every column but the
    // last writes its names to the right and the last writes its own to the left — so that one
    // gap has to fit the widest of both. Without it, `B` and the long name to its left ran
    // together into one string with no space between them.
    let widest_in = |want: usize| -> f64 {
        sankey
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, _)| column[*i] == want)
            .map(|(_, n)| Label::measure(n).width)
            .fold(0.0_f64, f64::max)
    };
    let mut needed_gap = 0.0_f64;
    for c in 0..last {
        let mut want = widest_in(c) + LABEL_GAP * 2.0 + 1.0;
        if c + 1 == last {
            want += widest_in(last) + LABEL_GAP;
        }
        needed_gap = needed_gap.max(want);
    }
    let width = if last == 0 {
        WIDTH
    } else {
        WIDTH.max(NODE_WIDTH + last as f64 * (NODE_WIDTH + needed_gap))
    };

    // --- the node bars -------------------------------------------------------------------------
    let column_x = |c: usize| -> f64 {
        if last == 0 {
            0.0
        } else {
            c as f64 * (width - NODE_WIDTH) / last as f64
        }
    };
    let mut top = vec![0.0_f64; n];
    let mut cursor = vec![0.0_f64; last + 1];
    for c in 0..=last {
        let used: f64 = (0..n)
            .filter(|i| column[*i] == c)
            .map(|i| scale.height(values[i]).max(MIN_BAND))
            .sum::<f64>()
            + (column_count[c].saturating_sub(1)) as f64 * NODE_PAD;
        cursor[c] = (HEIGHT - used) / 2.0;
    }
    // Declaration order inside a column. It is the author's order, and reordering it to reduce
    // crossings would move a node the reader put somewhere on purpose.
    for i in 0..n {
        let c = column[i];
        top[i] = cursor[c];
        cursor[c] += scale.height(values[i]).max(MIN_BAND) + NODE_PAD;
    }

    let mut nodes: Vec<PlacedNode> = Vec::new();
    for i in 0..n {
        let h = scale.height(values[i]).max(MIN_BAND);
        let x = column_x(column[i]);
        nodes.push(bar_node(
            format!("node#{i}"),
            Point::new(x + NODE_WIDTH / 2.0, top[i] + h / 2.0),
            Size::new(NODE_WIDTH, h),
            Some(i),
        ));
    }

    // --- the ribbons ---------------------------------------------------------------------------
    //
    // The bands on each side of a node are stacked in the order of the *other* end's position, so
    // two flows crossing between the same pair of columns cross once rather than twice. That is
    // d3-sankey's `reorderLinks`, and it is the only ordering decision here.
    let mut out_cursor = top.clone();
    let mut in_cursor = top.clone();
    let mut order: Vec<usize> = (0..sankey.links.len()).collect();
    order.sort_by(|a, b| {
        let (la, lb) = (&sankey.links[*a], &sankey.links[*b]);
        (column[la.source], top[la.source], top[la.target])
            .partial_cmp(&(column[lb.source], top[lb.source], top[lb.target]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut left_of: Vec<f64> = vec![0.0; sankey.links.len()];
    for &li in &order {
        let l = &sankey.links[li];
        left_of[li] = out_cursor[l.source];
        out_cursor[l.source] += scale.height(l.value).max(MIN_BAND);
    }
    let mut order_in: Vec<usize> = (0..sankey.links.len()).collect();
    order_in.sort_by(|a, b| {
        let (la, lb) = (&sankey.links[*a], &sankey.links[*b]);
        (column[la.target], top[la.target], top[la.source])
            .partial_cmp(&(column[lb.target], top[lb.target], top[lb.source]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for &li in &order_in {
        let l = &sankey.links[li];
        let band = scale.height(l.value).max(MIN_BAND);
        let l0 = left_of[li];
        let r0 = in_cursor[l.target];
        in_cursor[l.target] += band;

        let x0 = column_x(column[l.source]) + NODE_WIDTH;
        let x1 = column_x(column[l.target]);
        let cx = (x0 + x1) / 2.0;
        let cy = (l0 + l0 + band + r0 + r0 + band) / 4.0;
        nodes.push(PlacedNode {
            id: format!("flow#{li}"),
            shape: Glyph::Ribbon,
            center: Point::new(cx, cy),
            size: Size::new((x1 - x0).max(0.0), (l0 + band).max(r0 + band) - l0.min(r0)),
            label: Label::measure(""),
            panel: None,
            series: Some(l.source),
            mark: Some(Mark::Ribbon {
                left_top: l0 - cy,
                left_bottom: l0 + band - cy,
                right_top: r0 - cy,
                right_bottom: r0 + band - cy,
            }),
            style: None,
        });
    }

    // --- the names ------------------------------------------------------------------------------
    //
    // Outside the bar, on the side that has room: to the right for every column but the last, to
    // the left for that one, so a name never runs off the edge of the drawing.
    for i in 0..n {
        let h = scale.height(values[i]).max(MIN_BAND);
        let label = Label::measure(&sankey.nodes[i]);
        let x = column_x(column[i]);
        let center = if column[i] == last && last > 0 {
            Point::new(x - LABEL_GAP - label.width / 2.0, top[i] + h / 2.0)
        } else {
            Point::new(
                x + NODE_WIDTH + LABEL_GAP + label.width / 2.0,
                top[i] + h / 2.0,
            )
        };
        if let Some(node) = label_node(format!("node#{i}#label"), label, center, None) {
            nodes.push(node);
        }
    }

    let mut diagram = Diagram {
        nodes,
        ..Diagram::default()
    };
    add_title(&mut diagram, &sankey.preamble);
    normalise(&mut diagram);
    Ok(diagram)
}

/// Which column each node sits in.
///
/// `computeNodeDepths` followed by `justify`: a node's depth is the longest chain of links that
/// reaches it, and a node with nothing leaving it is then moved to the last column, so every sink
/// lines up on the right. The walk is breadth-first over the links, and it terminates because
/// [`crate::preview::mermaid::chart::sankey::parse`] has already refused a cycle.
fn columns(sankey: &Sankey, n: usize) -> Vec<usize> {
    let mut depth = vec![0usize; n];
    // n rounds is enough for the longest possible path, and the bound is what makes this safe
    // against a cycle slipping past the parser.
    for _ in 0..n {
        let mut moved = false;
        for l in &sankey.links {
            if depth[l.target] < depth[l.source] + 1 {
                depth[l.target] = depth[l.source] + 1;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let last = depth.iter().copied().max().unwrap_or(0);
    (0..n)
        .map(|i| {
            if sankey.links.iter().any(|l| l.source == i) {
                depth[i]
            } else {
                last
            }
        })
        .collect()
}
