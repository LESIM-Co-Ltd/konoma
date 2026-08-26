//! konoma's own mermaid renderer.
//!
//! Being built in stages; see `docs/FEATURE-MERMAID-RENDERER.md` §7.
//!
//! * **Stage 0** laid down the shared groundwork: [`text_metrics`], which measures text the
//!   way resvg will actually draw it, plus the instrument that proves the two agree.
//! * **Stage 1a** vendored the layout engine into [`layout`] — a copy of the `dagre` crate,
//!   carried in `src/` rather than taken as a dependency so konoma can fix it in a published
//!   build. It arrives with the 30-graph cross-validation baseline recorded from the real
//!   dagre.js, which is this renderer's only external source of truth.
//!
//! Parsing, shape geometry and SVG emission are still to come. Until they land,
//! `preview::markdown::mermaid_to_svg` is untouched and every diagram still goes through
//! `mermaid-rs-renderer`; nothing in this module is on the drawing path yet.

pub mod layout;
pub mod text_metrics;
