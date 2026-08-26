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
pub fn size(shape: Shape, label: Size) -> Size {
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
    /// Nothing is drawn.
    None,
}

/// The outline of `shape` inside a bounding box of `size`.
pub fn outline(shape: Shape, size: Size) -> Outline {
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
        _ => Outline::Polygon(polygon(shape, size)),
    }
}

/// The vertices of a polygonal shape, relative to the node centre, in boundary order.
///
/// Shapes that are not polygons return their own boundary sampled densely enough to intersect
/// against (the stadium) or an empty list.
pub fn polygon(shape: Shape, size: Size) -> Vec<Point> {
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
pub fn intersect(shape: Shape, center: Point, size: Size, target: &Point) -> Point {
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
            let verts: Vec<Point> = polygon(shape, size)
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
