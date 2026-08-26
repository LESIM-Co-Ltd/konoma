//! konoma's own mermaid renderer.
//!
//! Stage 0 (see `docs/FEATURE-MERMAID-RENDERER.md` §7) only puts down the shared groundwork:
//! **text measurement** and the instrument that proves it agrees with what resvg actually draws.
//! Parsing, layout and SVG emission arrive in stage 1; until then `preview::markdown::mermaid_to_svg`
//! is untouched and every diagram still goes through `mermaid-rs-renderer`.

pub mod text_metrics;
