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
//! * **Stage 1b** added [`flowchart`], which reads mermaid's flowchart language — three quarters
//!   of the mermaid people actually write — into a structural model. It parses and stops: no
//!   measuring, no layout, no SVG.
//! * **Stage 1c** added [`render`], which is where the three of them meet and a picture comes
//!   out: labels measured by [`text_metrics`], boxes sized by mermaid's own shape formulas,
//!   positions from [`layout`], and then the part that is konoma's alone — clipping each edge to
//!   the shape it touches, curving it, and putting its label back on the middle of the line.
//!
//! Subgraph frames are still to come. Until stage 1e swaps it,
//! `preview::markdown::mermaid_to_svg` is untouched and every diagram konoma actually shows still
//! goes through `mermaid-rs-renderer`; nothing in this module is on the drawing path yet.

pub mod flowchart;
pub mod layout;
pub mod render;
pub mod text_metrics;
