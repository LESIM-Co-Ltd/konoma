// Text measurement for konoma's mermaid renderer: lives in the `konoma-svg` crate (it shares the
// font database with the rasterizer); re-exported so `crate::preview::mermaid::text_metrics::...`
// paths are unchanged.

pub use konoma_svg::text_metrics::*;
