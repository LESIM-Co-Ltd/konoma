//! Drawing basics shared by konoma and its renderer crates: the system font database, text
//! measurement that agrees with what resvg draws, the guard rails for SVG that konoma did not
//! write, image decoding with its memory gate, and the rasterization of SVG.
//!
//! The supervised child process that draws *untrusted* SVG lives in the `konoma` package
//! (`svg_proc`); the `*_via` functions of [`svg`] take it as a parameter.

pub mod image;
pub mod svg;
pub mod svg_guard;
pub mod svg_size;
pub mod text_metrics;
