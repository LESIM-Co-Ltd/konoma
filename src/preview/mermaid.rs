// konoma's own mermaid renderer lives in the `konoma-mermaid-core` crate; re-exported so the
// `crate::preview::mermaid::...` paths (`render`, `flowchart`, `chart`, `text_metrics`, ...) are
// unchanged.

pub use konoma_mermaid_core::*;
