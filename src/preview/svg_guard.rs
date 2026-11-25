// Guard rails for SVG that konoma did not write: they live in the `konoma-svg` crate. This module
// keeps the paths `crate::preview::svg_guard::...` working. The words for a refused SVG are
// `konoma_plugin::canvas_pictures::svg_fail_msg` (shared with the standalone app).

pub use konoma_svg::svg_guard::*;
