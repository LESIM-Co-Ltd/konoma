//! Format-neutral drawing of one slide: a [`SlideScene`] (built by the PowerPoint and
//! OpenDocument readers) becomes an SVG.
//!
//! * [`model`] -- the scene: shapes, pictures, groups, fills, lines, text; all geometry in EMU.
//! * [`color`] -- the DrawingML colour transforms and named colours the readers need.
//! * [`text`] -- text layout (measuring with the faces resvg will really use, wrapping, bullets,
//!   alignment, spacing, autofit, vertical text).
//! * [`fonts`] -- the Office-font substitution table and measurement.
//! * [`metafile`] -- EMF / WMF pictures as SVG.
//! * [`path`] -- `arcTo` and path-space scaling.
//! * [`geom`] / [`odf_geom`] -- preset and custom geometry of the two formats as paths.
//! * [`chart`] -- a chart model lowered to shapes; [`pic_fx`] -- picture colour effects;
//!   [`symbol_font`] -- Symbol / Wingdings characters.
//! * [`underlay`] -- what the slides of a deck share (a master's drawing, the item budget).
//! * `svg` -- the writer behind [`render_svg`].
//!
//! The renderer is deliberately dumb about file formats and *never trusts* its input: numbers
//! are sanitised, everything textual is escaped, and the output is bounded (see
//! [`svg::MAX_SVG_TEXT_BYTES`] and [`svg::MAX_EMBEDDED_IMAGE_BYTES`]). The SVG still embeds
//! pictures that came from a file, so it is to be drawn by the supervised drawing process like any
//! untrusted SVG.

pub mod chart;
pub mod color;
pub mod cost;
pub mod fonts;
pub mod footprint;
pub mod geom;
mod geom_xml;
pub mod metafile;
pub mod model;
pub mod odf_geom;
pub mod path;
mod patterns;
pub mod pic_fx;
pub mod strings;
pub mod svg;
pub mod symbol_font;
pub mod text;
pub mod underlay;

#[cfg(test)]
mod cost_tests;
#[cfg(test)]
mod effects_tests;
#[cfg(test)]
pub(crate) mod hardening_tests;
#[cfg(test)]
pub(crate) mod tests;

pub use model::*;

use std::sync::Arc;

/// The result of [`render_svg`].
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The slide as an SVG document (px at 96 dpi; `viewBox` = slide size).
    pub svg: String,
    /// Something is missing: the scene itself was truncated by its reader, or the renderer hit a
    /// size budget, a group nesting limit or a path-command limit.
    pub truncated: bool,
    /// The caller said the render was not wanted any more (see [`render_svg_cancellable`]) and it
    /// stopped early: `svg` is incomplete and must not be shown or remembered.
    pub cancelled: bool,
    /// The filter and mask work of the slide's effects, counted the way the drawing process counts
    /// it, with the slide rasterised at [`Rendered::model_px`] on its longer side.
    pub filter_work: f64,
    /// The size, on the longer side in px, `filter_work` was measured at: [`svg::MODEL_RASTER_PX`],
    /// or the slide's own size when that is bigger (the drawing process never draws below 1:1).
    pub model_px: f64,
    /// What the slide costs the drawing process, as the writer counted it (see [`cost`]).
    pub features: cost::Features,
}

impl Rendered {
    /// The longest side, in px, this slide can be rasterised at and still be inside the drawing
    /// process's work and time budgets. Two limits apply, the smaller wins:
    ///
    /// * The process counts filter work times the square of the scale, so a slide whose effects
    ///   add up to `W` at `model_px` stays within the writer's allowance
    ///   ([`svg::MAX_FILTER_WORK`], the process's limit with a margin) up to
    ///   `model_px * sqrt(MAX_FILTER_WORK / W)`.
    /// * The time the writer predicted for the slide ([`cost::Features::predict`]) has a part that
    ///   grows with the raster's side (outlines) and one that grows with its area (fills,
    ///   pictures, filters, tiles, the area SVG pictures paint); the raster is kept to where the
    ///   prediction reaches [`cost::MAX_CHILD_MS`] (the same budget the writer held the slide to
    ///   at `model_px`).
    ///
    /// Never below `model_px` (the writer already kept the slide inside both at that size), the
    /// bigger the work the smaller the result (monotonic). Memory of pixel layers needs no cap:
    /// the writer nests nothing that isolates deeper than a few levels, far under the process's
    /// limit even at the largest raster; node counts do not depend on the raster size.
    pub fn max_raster_px(&self) -> u32 {
        let model = self.model_px.max(1.0);
        let work = self.filter_work;
        let by_filters = if work.is_finite() && work > 0.0 {
            model * (svg::MAX_FILTER_WORK / work).sqrt()
        } else {
            f64::INFINITY
        };
        // The time the writer predicted for the drawing grows with the raster too (fills, strokes,
        // pictures and outlines in proportion to its area or side): the raster is kept to what
        // fits the same time budget the writer held the slide to at the model size.
        let by_time = model * self.features.predict().max_ratio(cost::MAX_CHILD_MS);
        let px = by_filters.min(by_time);
        if px.is_infinite() || px.is_nan() {
            return u32::MAX;
        }
        // Floor of the model size: below it the drawing process would draw at 1:1 anyway.
        px.max(model).min(f64::from(u32::MAX)) as u32
    }
}

/// Draws a scene as SVG. `media` resolves an image key (as used in [`ImageFill::key`]) to the
/// bytes of the image; `None` (or bytes that are not a usable image) draws a placeholder.
///
/// Implemented: backgrounds, shapes with solid / gradient / pattern / image fills, lines (dash,
/// cap, join, compound, arrow heads), pictures with crop and clip, groups with rotation and
/// flips, text, and the effects (outer and inner shadow, glow, soft edge, reflection). EMF / WMF
/// pictures are converted to SVG. See the `svg` module for how each
/// approximation is made.
#[cfg(test)]
pub fn render_svg(scene: &SlideScene, media: &dyn Fn(&str) -> Option<Arc<Vec<u8>>>) -> Rendered {
    svg::render(scene, media, &|| false)
}

/// [`render_svg`] that polls `cancel` between items and inside its long loops (picture effects,
/// metafile conversion, the writing of a long path) and stops as soon as it says yes; the result
/// then has `cancelled` set. The slides of a presentation are drawn on a worker that the preview
/// leaves behind when the user moves on, and this is what lets it stop.
pub fn render_svg_cancellable(
    scene: &SlideScene,
    media: &dyn Fn(&str) -> Option<Arc<Vec<u8>>>,
    cancel: &dyn Fn() -> bool,
) -> Rendered {
    svg::render(scene, media, cancel)
}
