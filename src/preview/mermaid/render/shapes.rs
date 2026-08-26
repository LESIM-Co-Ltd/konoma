//! How big a node is, what its outline looks like, and where an edge should meet it.
//!
//! # The sizes are mermaid's, taken from mermaid's source
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §0-1 says konoma does not chase a pixel match, but the
//! *proportions* are not a matter of taste: a diamond that is sized like a rectangle makes every
//! branch in the diagram look wrong, and mermaid's own formulas are the only description of those
//! proportions that is not a guess. Each `size_*` function below cites the file it came from in
//! `mermaid@11.17.2`, and the padding they all read is mermaid's `flowchart.padding`, whose schema
//! default is 15 (`flowDb.ts` reads `config.flowchart?.padding || 8`, and the schema always
//! supplies 15, so 8 is unreachable for a flowchart).
//!
//! The one that matters most is the diamond: mermaid gives it `s = w + h` where `w` and `h` are
//! the padded label, and draws a square of side `s` rotated 45°. Sizing it as `w × h` instead
//! makes the branch labels collide with the box on every decision node.
//!
//! # Two families, one interface
//!
//! [`Glyph`] is what a node is drawn as, and it has three kinds of member: the fifteen a
//! flowchart can ask for, wrapped in [`Glyph::Flow`]; the six a state diagram adds — the two
//! `[*]` markers, a choice diamond, a fork/join bar, a note, and a state box with a rule under
//! its first line; and the two compartmented boxes stage 3 adds, [`Glyph::ClassBox`] and
//! [`Glyph::ErBox`]. Everything downstream of this module (layout, clipping, emission, the
//! geometric invariants) is written against `Glyph` and knows nothing about which family a node
//! came from, which is what lets each stage reuse the pipeline whole rather than growing a
//! second one.
//!
//! # Shapes konoma folds together
//!
//! §2-4 settles that all 53 names must be *recognised* but need not be *drawn* apart. The parser
//! already folds the 53 onto the fifteen [`Shape`] variants; this module draws fourteen of them
//! and lets [`Shape::Text`] draw no outline at all. Nothing is folded further, so a shape a reader
//! wrote is a shape they see.
//!
//! # Intersections
//!
//! dagre routes edges between node *rectangles* — it knows nothing about outlines — so its first
//! and last waypoints sit on a bounding box. `edges::clip` throws those two away and asks this
//! module where the line really leaves the shape, which is what makes a branch come out of a
//! diamond's slanted side instead of its bounding box's flat one. The polygon case takes the
//! nearest of *all* the sides' crossings along the ray, which is
//! [`layout::intersect::intersect_polygon`]'s contract.

use std::f64::consts::PI;

use crate::preview::mermaid::flowchart::Shape;
use crate::preview::mermaid::layout::intersect::{
    intersect_ellipse, intersect_polygon, intersect_rect,
};
use crate::preview::mermaid::layout::Point;

/// mermaid's `flowchart.padding`. Schema default 15 (`config.schema.yaml`).
pub const PADDING: f64 = 15.0;

/// Corner radius of `A(text)`. mermaid's `themeVariables.radius`, default 5 (`roundedRect.ts`).
pub const CORNER_RADIUS: f64 = 5.0;

/// Half the gap between the two rings of `A(((text)))` (`doubleCircle.ts`).
pub const DOUBLE_CIRCLE_GAP: f64 = 5.0;

/// Width of the side frames of `A[[text]]` (`subroutine.ts`'s `FRAME_WIDTH`).
pub const SUBROUTINE_FRAME: f64 = 8.0;

/// How many segments approximate each half-circle cap of a stadium when intersecting it.
/// mermaid uses 50; the cap is only ever a few tens of px across, so 24 puts the worst-case
/// deviation well under a tenth of a pixel.
const CAP_SEGMENTS: usize = 24;

/// Radius of the dot `[*]` becomes. `stateStart.ts` draws a circle of `r = 7`.
pub const STATE_MARKER_RADIUS: f64 = 7.0;

/// Radius of the filled core inside the end marker (`stateEnd.ts`'s inner circle).
pub const STATE_END_INNER_RADIUS: f64 = 4.0;

/// Side of the square a `<<choice>>` diamond is cut from. `choice.ts` uses 40 and draws no text.
pub const CHOICE_SIZE: f64 = 40.0;

/// Long side of a `<<fork>>` / `<<join>>` bar (`forkJoin.ts`).
pub const BAR_LENGTH: f64 = 70.0;

/// Short side of a `<<fork>>` / `<<join>>` bar (`forkJoin.ts`).
pub const BAR_THICKNESS: f64 = 10.0;

/// How far a note's folded corner reaches in from its top-right corner.
///
/// konoma's own: mermaid draws a note as a plain rectangle. The fold is the one thing that says
/// "this is an aside, not a state" without a colour the terminal might be showing through
/// (§0-1 is what allows the difference).
pub const NOTE_FOLD: f64 = 10.0;

/// Space between a sequence participant's name and the sides of its box.
pub const PARTICIPANT_PAD_X: f64 = 14.0;

/// Space above and below a sequence participant's name.
pub const PARTICIPANT_PAD_Y: f64 = 10.0;

/// The narrowest a sequence participant's box may be.
///
/// Same reasoning as [`super::panel::ER_MIN_WIDTH`]: a row of boxes of wildly different widths
/// reads as noise, and a one-letter participant beside a sentence-long one is the common case.
/// mermaid pins its own at a flat `sequence.width` of 150, which is far too wide for a terminal.
pub const PARTICIPANT_MIN_WIDTH: f64 = 62.0;

/// Height of the band a sequence `actor`'s stick figure is drawn in, above its name.
pub const ACTOR_FIGURE_HEIGHT: f64 = 32.0;

/// Radius of that figure's head.
pub const ACTOR_HEAD_RADIUS: f64 = 6.0;

/// What a node is drawn as.
///
/// See the module docs: one enum over both diagram families, so that everything downstream is
/// written once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Glyph {
    /// One of a flowchart's fifteen outlines.
    Flow(Shape),
    /// The `[*]` an arrow leaves: a filled dot.
    StateStart,
    /// The `[*]` an arrow enters: a ring around a filled dot.
    StateEnd,
    /// `state x <<choice>>`: a diamond with no text.
    Choice,
    /// `state x <<fork>>` / `<<join>>`: a solid bar. `horizontal` is true when the bar lies
    /// across the page, which is what a top-to-bottom diagram wants.
    Bar {
        /// Whether the long axis is horizontal.
        horizontal: bool,
    },
    /// A `note left of` / `note right of`: a box with a folded corner.
    Note,
    /// A state box whose label arrived as more than one description, drawn with a rule under
    /// the first of them (mermaid's `SHAPE_STATE_WITH_DESC`).
    TitledBox,
    /// A class diagram's `classBox`: a square-cornered rectangle divided into compartments. What
    /// goes in them is the node's [`Panel`](super::panel::Panel), which is also what sized it.
    ClassBox,
    /// An ER diagram's `erBox`: a name band over a grid of attributes. Sized by its panel too.
    ErBox,
    /// A sequence diagram's `participant`: a rounded box with a minimum width, sitting at the head
    /// of a lifeline.
    Participant,
    /// A sequence diagram's `actor`: a stick figure with the name underneath it, and **no box**.
    ///
    /// mermaid draws the figure too; the crate konoma is replacing draws a second plain box, so
    /// `actor Alice` and `participant Alice` come out identical there. The name lives in the
    /// node's [`Panel`](super::panel::Panel), which is also what sized the node — the figure needs
    /// a band of its own above the text, and a centred label cannot express that.
    Actor,

    // --- stage 5: the marks a data chart is made of -------------------------------------------
    //
    // Seven kinds, and not one of them is a box with a word in it. What they have in common with
    // everything above is that they are *kinds*, with no continuous geometry of their own: an
    // angle, a ribbon's four corners and a graticule's ring count live in [`Mark`], for the same
    // reason a compartment's rows live in a `Panel` — a bounding box cannot say them, and putting
    // them here would cost `Glyph` its `Eq` and `Hash`.
    /// A pie slice. Its angles come from [`Mark::Wedge`] and its radius from the node's box.
    Wedge,
    /// A rectangle painted in a series colour: a bar, a treemap tile, a packet field, a Sankey
    /// node's column, a legend swatch.
    ChartBar,
    /// A small filled disc marking one datum. No outline — at four pixels across, a stroke is
    /// most of the mark.
    ChartPoint,
    /// A Sankey flow: a band whose two ends have different heights, sized by [`Mark::Ribbon`].
    Ribbon,
    /// Words and nothing else, in a box that is exactly the words.
    ///
    /// Not `Flow(Shape::Text)`, which pads its box by `PADDING` on every side: a tick label whose
    /// box is fifteen pixels wider than its ink cannot be checked for overlapping its neighbour,
    /// and overlapping tick labels are the commonest way a chart becomes unreadable.
    ChartLabel,
    /// The rectangle a plot is drawn inside — an outline in the axis colour, with nothing behind
    /// it, because §1 forbids painting over the terminal.
    PlotFrame,
    /// A radar chart's background: concentric rings and the spokes, from [`Mark::Graticule`].
    Graticule,
}

/// Continuous geometry a chart mark needs that its bounding box cannot express.
///
/// The same idea as [`super::panel::Panel`], and for the same reason: [`Glyph`] says *what kind of
/// mark*, the node's `size` says *how big*, and anything left over — an angle, a ribbon's four
/// corners — goes here, so that `Glyph` stays a plain enumeration that a `match` can be exhaustive
/// over and a `HashSet` can hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    /// A pie slice, in degrees clockwise from twelve o'clock.
    Wedge {
        /// Where the slice begins.
        start: f64,
        /// How far round it goes.
        sweep: f64,
    },
    /// A Sankey flow. All four are y offsets from the node's centre; the node's width is the
    /// horizontal span between the two ends.
    Ribbon {
        /// Top of the left end.
        left_top: f64,
        /// Bottom of the left end.
        left_bottom: f64,
        /// Top of the right end.
        right_top: f64,
        /// Bottom of the right end.
        right_bottom: f64,
    },
    /// A radar chart's rings and spokes.
    Graticule {
        /// How many rings, the outermost included.
        rings: usize,
        /// How many spokes radiate from the centre.
        spokes: usize,
        /// Whether a ring is a copy of the spoke polygon rather than a circle.
        polygon: bool,
    },
}

impl From<Shape> for Glyph {
    fn from(shape: Shape) -> Glyph {
        Glyph::Flow(shape)
    }
}

impl Default for Glyph {
    fn default() -> Glyph {
        Glyph::Flow(Shape::default())
    }
}

/// A width and a height, in px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Size {
    /// Convenience constructor.
    pub fn new(w: f64, h: f64) -> Size {
        Size { w, h }
    }
}

/// The bounding box a node of this shape needs for a label of size `label`.
///
/// This is the size handed to dagre, which lays out rectangles; the outline is drawn inside it.
/// For the slanted shapes that means the box is wider than mermaid's intermediate `w`, because
/// mermaid's own `updateNodeBounds` measures the drawn polygon, overhang included.
fn flow_size(shape: Shape, label: Size) -> Size {
    let (lw, lh) = (label.w, label.h);
    match shape {
        // squareRect.ts -> drawRect.ts: labelPaddingX = padding*2, labelPaddingY = padding,
        // and drawRect doubles each again.
        Shape::Rect => Size::new(lw + PADDING * 4.0, lh + PADDING * 2.0),
        // roundedRect.ts -> drawRect.ts: both paddings are padding*1, doubled by drawRect.
        Shape::RoundedRect => Size::new(lw + PADDING * 2.0, lh + PADDING * 2.0),
        // stadium.ts: h gets ONE padding (not two), and w carries a quarter of h for the caps.
        Shape::Stadium => {
            let h = lh + PADDING;
            Size::new(lw + h / 4.0 + PADDING, h)
        }
        // circle.ts. Two things to know about this line:
        //
        //  * Upstream reads `options?.padding ?? halfPadding`, and the flowchart renderer passes
        //    no `padding`, so mermaid effectively uses padding/2. konoma uses the whole padding,
        //    which is the only place in this file that knowingly leaves mermaid's number.
        //  * The radius follows the label's **width alone**, in konoma as in mermaid. A tall
        //    label — several `<br>`s — therefore overflows the ring at top and bottom. That is
        //    upstream's behaviour and it is recorded here (and in
        //    `tests::every_shape_holds_the_label_it_was_sized_for`, which exempts circles for
        //    exactly this reason) rather than quietly diverged from.
        Shape::Circle => {
            let r = lw / 2.0 + PADDING;
            Size::new(r * 2.0, r * 2.0)
        }
        // doubleCircle.ts: the outer ring already uses the whole padding upstream.
        Shape::DoubleCircle => {
            let r = lw / 2.0 + PADDING;
            Size::new(r * 2.0, r * 2.0)
        }
        // question.ts: w and h each get one padding, and the drawn square's side is their SUM.
        Shape::Diamond => {
            let s = (lw + PADDING) + (lh + PADDING);
            Size::new(s, s)
        }
        // hexagon.ts: h gets one padding, the slanted ends are h/4 wide each, and w gets one more.
        Shape::Hexagon => {
            let h = lh + PADDING;
            Size::new(lw + h / 2.0 + PADDING, h)
        }
        // subroutine.ts: one padding, plus a frame on each side.
        Shape::Subroutine => Size::new(lw + 2.0 * SUBROUTINE_FRAME + PADDING, lh + PADDING),
        // cylinder.ts: the ellipse's ry follows the width, and the drawn path stands 2*ry taller
        // than the `h` upstream computes.
        Shape::Cylinder => {
            let w = lw + PADDING;
            let ry = cylinder_ry(w);
            Size::new(w, lh + PADDING + 3.0 * ry)
        }
        // trapezoid.ts / leanRight.ts / leanLeft.ts: one padding each way; the slant adds h/2 of
        // overhang on each side, so the drawn box is `w + h` wide.
        Shape::Trapezoid | Shape::LeanRight | Shape::LeanLeft => {
            let h = lh + PADDING;
            Size::new(lw + PADDING + h, h)
        }
        // invertedTrapezoid.ts is the odd one out: it doubles BOTH paddings where trapezoid.ts
        // doubles neither. That asymmetry is upstream's, not a transcription slip.
        Shape::InvTrapezoid => {
            let h = lh + PADDING * 2.0;
            Size::new(lw + PADDING * 2.0 + h, h)
        }
        // rectLeftInvArrow.ts: the notch on the left is h/4 deep and sticks out of the label box.
        Shape::Odd => {
            let h = lh + PADDING;
            Size::new(lw + PADDING + h / 4.0, h)
        }
        // text.ts: one padding, no outline.
        Shape::Text => Size::new(lw + PADDING, lh + PADDING),
    }
}

/// `ry` of a cylinder's end ellipse for a body of width `w` (`cylinder.ts`).
fn cylinder_ry(w: f64) -> f64 {
    let rx = w / 2.0;
    rx / (2.5 + w / 50.0)
}

/// What to draw for a shape whose bounding box is `size`, in coordinates relative to the centre.
#[derive(Debug, Clone, PartialEq)]
pub enum Outline {
    /// Axis-aligned rectangle, corner radius `r` (0 for a square corner).
    Rect {
        /// Width.
        w: f64,
        /// Height.
        h: f64,
        /// Corner radius.
        r: f64,
    },
    /// A circle centred on the node.
    Circle {
        /// Radius.
        r: f64,
    },
    /// Two concentric circles.
    DoubleCircle {
        /// Outer radius.
        outer: f64,
        /// Inner radius.
        inner: f64,
    },
    /// A closed polygon.
    Polygon(Vec<Point>),
    /// A rectangle with a vertical rule `inset` from each side.
    Subroutine {
        /// Width.
        w: f64,
        /// Height.
        h: f64,
        /// Distance from each side to its rule.
        inset: f64,
    },
    /// A cylinder standing on end: a body `w` wide and `h` tall whose caps are ellipses of
    /// vertical radius `ry`.
    Cylinder {
        /// Width.
        w: f64,
        /// Total height, caps included.
        h: f64,
        /// Vertical radius of a cap.
        ry: f64,
    },
    /// A filled dot with no outline — the `[*]` an arrow leaves.
    Disc {
        /// Radius.
        r: f64,
    },
    /// A ring around a filled dot — the `[*]` an arrow enters.
    Target {
        /// Radius of the ring.
        outer: f64,
        /// Radius of the filled core.
        inner: f64,
    },
    /// A solid bar — `<<fork>>` and `<<join>>`.
    Bar {
        /// Width.
        w: f64,
        /// Height.
        h: f64,
    },
    /// A box with its top-right corner folded over — a note.
    Note {
        /// Width.
        w: f64,
        /// Height.
        h: f64,
        /// How far the fold reaches in from the corner, along both axes.
        fold: f64,
    },
    /// A stick figure standing in the top `figure_h` of the box, with the name below it.
    Actor {
        /// Width of the bounding box.
        w: f64,
        /// Height of the bounding box.
        h: f64,
        /// Height of the band the figure occupies, measured down from the top.
        figure_h: f64,
    },
    /// A pie slice: a sector of radius `r`, from `start` to `start + sweep` degrees clockwise
    /// from twelve o'clock.
    Wedge {
        /// Radius.
        r: f64,
        /// Where the slice begins, in degrees.
        start: f64,
        /// How far round it goes, in degrees.
        sweep: f64,
    },
    /// A Sankey flow, as four y offsets from the centre across a span of `w`.
    Ribbon {
        /// Horizontal span.
        w: f64,
        /// Top of the left end.
        left_top: f64,
        /// Bottom of the left end.
        left_bottom: f64,
        /// Top of the right end.
        right_top: f64,
        /// Bottom of the right end.
        right_bottom: f64,
    },
    /// A radar background: `rings` rings of radius `r * i / rings`, and `spokes` spokes.
    Graticule {
        /// Outer radius.
        r: f64,
        /// How many rings.
        rings: usize,
        /// How many spokes.
        spokes: usize,
        /// Whether the rings are polygons rather than circles.
        polygon: bool,
    },
    /// Nothing is drawn.
    None,
}

/// The outline of `shape` inside a bounding box of `size`.
fn flow_outline(shape: Shape, size: Size) -> Outline {
    let (w, h) = (size.w, size.h);
    match shape {
        Shape::Rect => Outline::Rect { w, h, r: 0.0 },
        Shape::RoundedRect => Outline::Rect {
            w,
            h,
            r: CORNER_RADIUS,
        },
        Shape::Stadium => Outline::Rect { w, h, r: h / 2.0 },
        Shape::Circle => Outline::Circle { r: w / 2.0 },
        Shape::DoubleCircle => Outline::DoubleCircle {
            outer: w / 2.0,
            inner: (w / 2.0 - DOUBLE_CIRCLE_GAP).max(0.0),
        },
        Shape::Subroutine => Outline::Subroutine {
            w,
            h,
            inset: SUBROUTINE_FRAME,
        },
        Shape::Cylinder => Outline::Cylinder {
            w,
            h,
            ry: cylinder_ry(w),
        },
        Shape::Text => Outline::None,
        _ => Outline::Polygon(flow_polygon(shape, size)),
    }
}

/// The vertices of a polygonal shape, relative to the node centre, in boundary order.
///
/// Shapes that are not polygons return their own boundary sampled densely enough to intersect
/// against (the stadium) or an empty list.
fn flow_polygon(shape: Shape, size: Size) -> Vec<Point> {
    let (w, h) = (size.w, size.h);
    let (hw, hh) = (w / 2.0, h / 2.0);
    match shape {
        // question.ts, recentred: a square of side `s` on its corner.
        Shape::Diamond => vec![
            Point::new(0.0, -hh),
            Point::new(hw, 0.0),
            Point::new(0.0, hh),
            Point::new(-hw, 0.0),
        ],
        // hexagon.ts with m = h/4.
        Shape::Hexagon => {
            let m = h / 4.0;
            vec![
                Point::new(-hw + m, -hh),
                Point::new(hw - m, -hh),
                Point::new(hw, 0.0),
                Point::new(hw - m, hh),
                Point::new(-hw + m, hh),
                Point::new(-hw, 0.0),
            ]
        }
        // trapezoid.ts: the wide side is the bottom. The drawn box is `inner + h` wide, so the
        // inner top edge spans +/- (w - h)/2.
        Shape::Trapezoid => {
            let t = (hw - hh).max(0.0);
            vec![
                Point::new(-hw, hh),
                Point::new(hw, hh),
                Point::new(t, -hh),
                Point::new(-t, -hh),
            ]
        }
        // invertedTrapezoid.ts: the wide side is the top.
        Shape::InvTrapezoid => {
            let t = (hw - hh).max(0.0);
            vec![
                Point::new(-t, hh),
                Point::new(t, hh),
                Point::new(hw, -hh),
                Point::new(-hw, -hh),
            ]
        }
        // leanRight.ts: a parallelogram leaning right.
        Shape::LeanRight => {
            let t = (hw - hh).max(0.0);
            vec![
                Point::new(-hw, hh),
                Point::new(t, hh),
                Point::new(hw, -hh),
                Point::new(-t, -hh),
            ]
        }
        // leanLeft.ts: the same, mirrored.
        Shape::LeanLeft => {
            let t = (hw - hh).max(0.0);
            vec![
                Point::new(-t, hh),
                Point::new(hw, hh),
                Point::new(t, -hh),
                Point::new(-hw, -hh),
            ]
        }
        // rectLeftInvArrow.ts: a rectangle with a V notched into its left side, h/4 deep.
        Shape::Odd => {
            let notch = h / 4.0;
            vec![
                Point::new(-hw, -hh),
                Point::new(-hw + notch, 0.0),
                Point::new(-hw, hh),
                Point::new(hw, hh),
                Point::new(hw, -hh),
            ]
        }
        // stadium.ts approximates its own caps the same way, with 50 segments.
        Shape::Stadium => {
            let r = hh;
            let a = (hw - r).max(0.0);
            let mut pts = vec![Point::new(-a, -r), Point::new(a, -r)];
            for i in 1..CAP_SEGMENTS {
                let t = -PI / 2.0 + PI * i as f64 / CAP_SEGMENTS as f64;
                pts.push(Point::new(a + r * t.cos(), r * t.sin()));
            }
            pts.push(Point::new(a, r));
            pts.push(Point::new(-a, r));
            for i in 1..CAP_SEGMENTS {
                let t = PI / 2.0 + PI * i as f64 / CAP_SEGMENTS as f64;
                pts.push(Point::new(-a + r * t.cos(), r * t.sin()));
            }
            pts
        }
        _ => Vec::new(),
    }
}

/// Where a line aimed at `target` leaves a `shape` of `size` centred on `center`.
///
/// This is the step that makes an edge come out of the shape the reader sees rather than out of
/// the bounding box dagre reasoned about.
fn flow_intersect(shape: Shape, center: Point, size: Size, target: &Point) -> Point {
    match shape {
        // circle.ts / doubleCircle.ts both clip against the outer ring.
        Shape::Circle | Shape::DoubleCircle => {
            intersect_ellipse(center.x, center.y, size.w / 2.0, size.h / 2.0, target)
        }
        // drawRect.ts, subroutine's frame and cylinder.ts all clip against the box. (Upstream's
        // cylinder nudges the result towards the cap; the difference is under a pixel and the
        // nudge can land a point inside the shape, which invariant (2) forbids.)
        Shape::Rect | Shape::RoundedRect | Shape::Subroutine | Shape::Cylinder | Shape::Text => {
            intersect_rect(center.x, center.y, size.w, size.h, target)
        }
        _ => {
            let verts: Vec<Point> = flow_polygon(shape, size)
                .into_iter()
                .map(|p| Point::new(center.x + p.x, center.y + p.y))
                .collect();
            if verts.len() < 3 {
                return intersect_rect(center.x, center.y, size.w, size.h, target);
            }
            intersect_polygon(&verts, &center, target)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The `Glyph` layer: one interface over both diagram families
// ---------------------------------------------------------------------------------------------

/// The bounding box a node drawn as `glyph` needs for a label of size `label`.
///
/// The state markers ignore the label entirely, because mermaid draws no text inside them —
/// see [`Glyph`].
pub fn size(glyph: Glyph, label: Size) -> Size {
    match glyph {
        Glyph::Flow(shape) => flow_size(shape, label),
        // stateStart.ts / stateEnd.ts: a fixed dot, whatever the state was called.
        Glyph::StateStart | Glyph::StateEnd => {
            Size::new(STATE_MARKER_RADIUS * 2.0, STATE_MARKER_RADIUS * 2.0)
        }
        // choice.ts: a fixed 40x40 square on its corner, with no text.
        Glyph::Choice => Size::new(CHOICE_SIZE, CHOICE_SIZE),
        // forkJoin.ts: a bar whose long axis follows the diagram's own axis.
        Glyph::Bar { horizontal } => {
            if horizontal {
                Size::new(BAR_LENGTH, BAR_THICKNESS)
            } else {
                Size::new(BAR_THICKNESS, BAR_LENGTH)
            }
        }
        // A note is a rounded-rect-shaped box with room for the fold at its corner.
        Glyph::Note => Size::new(
            (label.w + PADDING * 2.0).max(NOTE_FOLD * 3.0),
            (label.h + PADDING * 2.0).max(NOTE_FOLD * 2.0),
        ),
        // The rule sits between the label's first and second line, so no extra height is needed.
        Glyph::TitledBox => Size::new(label.w + PADDING * 2.0, label.h + PADDING * 2.0),
        // A box that holds a table is measured by its table. `panel::class_panel` /
        // `panel::er_panel` decide the width from the widest row and the height from how many
        // rows there are, and the caller passes that size straight through here — a second
        // opinion about it would be a second thing to keep in step (see `panel`'s module docs).
        Glyph::ClassBox | Glyph::ErBox => label,
        // A participant's box is its name plus padding, floored at a width that keeps a row of
        // them even.
        Glyph::Participant => Size::new(
            (label.w + PARTICIPANT_PAD_X * 2.0).max(PARTICIPANT_MIN_WIDTH),
            label.h + PARTICIPANT_PAD_Y * 2.0,
        ),
        // Sized by its panel, like the two compartmented boxes, and for the same reason: the
        // figure's band and the name's band are two rows, not one centred label.
        Glyph::Actor => label,
        // Every chart mark is sized by the chart, not by a label: a bar's height is its value
        // against the axis scale, a tile's area is its share of the whole, a wedge's radius is the
        // room the pie was given. Passing the size straight through is the same contract the two
        // compartmented boxes have — one place decides, and nothing here second-guesses it.
        Glyph::Wedge
        | Glyph::ChartBar
        | Glyph::ChartPoint
        | Glyph::Ribbon
        | Glyph::PlotFrame
        | Glyph::Graticule => label,
        // …with one exception, and it is the point of the glyph: a chart label's box **is** its
        // ink, with no padding at all, so that "two tick labels do not overlap" is a statement
        // about what a reader sees.
        Glyph::ChartLabel => label,
    }
}

/// The outline of `glyph` inside a bounding box of `size`.
///
/// `mark` carries the continuous geometry the box cannot say — see [`Mark`]. Every glyph that
/// needs one draws **nothing** without it, rather than guessing: a wedge with no angles is not a
/// full circle, it is a bug, and a silent full circle would be a lying picture.
pub fn outline(glyph: Glyph, size: Size, mark: Option<Mark>) -> Outline {
    match glyph {
        Glyph::Wedge => match mark {
            Some(Mark::Wedge { start, sweep }) => Outline::Wedge {
                r: size.w / 2.0,
                start,
                sweep,
            },
            _ => Outline::None,
        },
        Glyph::Ribbon => match mark {
            Some(Mark::Ribbon {
                left_top,
                left_bottom,
                right_top,
                right_bottom,
            }) => Outline::Ribbon {
                w: size.w,
                left_top,
                left_bottom,
                right_top,
                right_bottom,
            },
            _ => Outline::None,
        },
        Glyph::Graticule => match mark {
            Some(Mark::Graticule {
                rings,
                spokes,
                polygon,
            }) => Outline::Graticule {
                r: size.w / 2.0,
                rings,
                spokes,
                polygon,
            },
            _ => Outline::None,
        },
        Glyph::ChartBar | Glyph::PlotFrame => Outline::Rect {
            w: size.w,
            h: size.h,
            r: 0.0,
        },
        Glyph::ChartPoint => Outline::Disc {
            r: size.w.min(size.h) / 2.0,
        },
        Glyph::ChartLabel => Outline::None,
        _ => flow_outline_or_glyph(glyph, size),
    }
}

fn flow_outline_or_glyph(glyph: Glyph, size: Size) -> Outline {
    match glyph {
        Glyph::Flow(shape) => flow_outline(shape, size),
        Glyph::StateStart => Outline::Disc { r: size.w / 2.0 },
        Glyph::StateEnd => Outline::Target {
            outer: size.w / 2.0,
            inner: STATE_END_INNER_RADIUS.min(size.w / 2.0),
        },
        Glyph::Choice => Outline::Polygon(flow_polygon(Shape::Diamond, size)),
        Glyph::Bar { .. } => Outline::Bar {
            w: size.w,
            h: size.h,
        },
        Glyph::Note => Outline::Note {
            w: size.w,
            h: size.h,
            fold: NOTE_FOLD.min(size.w / 2.0).min(size.h / 2.0),
        },
        Glyph::TitledBox => Outline::Rect {
            w: size.w,
            h: size.h,
            r: CORNER_RADIUS,
        },
        // Square corners, unlike everything else konoma rounds: a UML class and an ER entity are
        // drawn square everywhere they are drawn at all, and the corner is the only thing that
        // tells a compartmented box from a subgraph frame at a glance.
        Glyph::ClassBox | Glyph::ErBox => Outline::Rect {
            w: size.w,
            h: size.h,
            r: 0.0,
        },
        Glyph::Participant => Outline::Rect {
            w: size.w,
            h: size.h,
            r: CORNER_RADIUS,
        },
        Glyph::Actor => Outline::Actor {
            w: size.w,
            h: size.h,
            figure_h: ACTOR_FIGURE_HEIGHT.min(size.h),
        },
        // Handled by `outline` before it delegates here.
        Glyph::Wedge
        | Glyph::ChartBar
        | Glyph::ChartPoint
        | Glyph::Ribbon
        | Glyph::ChartLabel
        | Glyph::PlotFrame
        | Glyph::Graticule => Outline::None,
    }
}

/// The vertices of a polygonal glyph, relative to the node centre, in boundary order. Empty for
/// a glyph whose boundary is not a polygon.
pub fn polygon(glyph: Glyph, size: Size) -> Vec<Point> {
    match glyph {
        Glyph::Flow(shape) => flow_polygon(shape, size),
        Glyph::Choice => flow_polygon(Shape::Diamond, size),
        _ => Vec::new(),
    }
}

/// Where a line aimed at `target` leaves a `glyph` of `size` centred on `center`.
pub fn intersect(glyph: Glyph, center: Point, size: Size, target: &Point) -> Point {
    match glyph {
        Glyph::Flow(shape) => flow_intersect(shape, center, size, target),
        Glyph::StateStart | Glyph::StateEnd => {
            intersect_ellipse(center.x, center.y, size.w / 2.0, size.h / 2.0, target)
        }
        Glyph::Choice => flow_intersect(Shape::Diamond, center, size, target),
        // A bar, a note and a titled box all clip against their box. The note's folded corner is
        // a detail of the drawing: cutting the line there would leave a visible gap where the
        // fold is, and the fold is at most ten pixels of a corner an edge rarely meets.
        Glyph::Bar { .. }
        | Glyph::Note
        | Glyph::TitledBox
        | Glyph::ClassBox
        | Glyph::ErBox
        | Glyph::Participant
        | Glyph::Actor
        // A chart mark is never an edge's endpoint: a grid line runs the width of the plot, a
        // line plot passes *through* its own points, and a radar curve closes on itself. Clipping
        // against the box is the harmless answer for a question nothing asks.
        | Glyph::Wedge
        | Glyph::ChartBar
        | Glyph::ChartPoint
        | Glyph::Ribbon
        | Glyph::ChartLabel
        | Glyph::PlotFrame
        | Glyph::Graticule => intersect_rect(center.x, center.y, size.w, size.h, target),
    }
}
