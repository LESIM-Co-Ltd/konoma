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
pub mod fonts;
pub mod geom;
mod geom_xml;
pub mod metafile;
pub mod model;
pub mod odf_geom;
pub mod path;
mod patterns;
pub mod pic_fx;
pub mod svg;
pub mod symbol_font;
pub mod text;
pub mod underlay;

#[cfg(test)]
mod effects_tests;
#[cfg(test)]
mod tests;

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
}

/// Draws a scene as SVG. `media` resolves an image key (as used in [`ImageFill::key`]) to the
/// bytes of the image; `None` (or bytes that are not a usable image) draws a placeholder.
///
/// Implemented: backgrounds, shapes with solid / gradient / pattern / image fills, lines (dash,
/// cap, join, compound, arrow heads), pictures with crop and clip, groups with rotation and
/// flips, text, and the effects (outer and inner shadow, glow, soft edge, reflection). EMF / WMF
/// pictures are converted to SVG. See the `svg` module for how each
/// approximation is made.
pub fn render_svg(scene: &SlideScene, media: &dyn Fn(&str) -> Option<Arc<Vec<u8>>>) -> Rendered {
    svg::render(scene, media)
}
