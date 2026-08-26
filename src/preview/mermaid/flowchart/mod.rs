//! Reading mermaid's flowchart language — stage 1b of konoma's own renderer.
//!
//! The whole of this module answers one question: *what did the author write?* It produces a
//! [`Flowchart`] — nodes, edges, subgraphs — and stops there. No text is measured, no box is
//! sized, no SVG is emitted; those are later stages and they read this model.
//!
//! ```text
//! let chart = parse("flowchart LR\n  A[Start] --> B{Ok?}\n  B -->|yes| C((Done))").unwrap();
//! chart.direction        == Direction::LeftToRight
//! chart.nodes.len()      == 3
//! chart.edges[1].label   == Some("yes")
//! ```
//! (konoma has no library target, so this is an illustration rather than a doctest; the same
//! case is asserted for real in [`tests`].)
//!
//! # Layout of the module
//!
//! | file | what it owns |
//! |---|---|
//! | [`preprocess`] | the five rewrites mermaid applies before its parser ever runs |
//! | [`parser`] | the grammar: statements, vertices, links, subgraphs |
//! | [`shapes`] | the 53 shape names of `@{ shape: … }`, and the small YAML reader for it |
//! | [`text`] | quotes, `<br>` in four spellings, `#nn;` entities |
//! | [`model`] | the intermediate representation everything above produces |
//!
//! # What it does not do
//!
//! Per `docs/FEATURE-MERMAID-RENDERER.md` §2-5: no curve names, no pixel spacing options, no
//! ELK, no animation, no `icon`/`img` shapes, no tooltips, no collapsible subgraphs, no
//! hand-drawn look, no legacy `graph >` direction symbols, and neither of the two undocumented
//! vertex forms (`(-text-)`, `[|k:v|text]`). Everything in §2-3 that must be *recognised* so it
//! cannot turn into a phantom node — `classDef`, `class`, `style`, `linkStyle`, `:::`, `click`,
//! `%%` comments, `%%{init}%%`, front matter, `accTitle`, `accDescr` in both forms, and edge
//! metadata — is recognised.

// Stage 1b is the parser only: nothing calls it until the geometry and SVG stages land
// (`docs/FEATURE-MERMAID-RENDERER.md` §7). konoma is a binary crate, so `pub` marks nothing as
// used and every entry point — and every re-export below — looks unreachable to rustc until
// then. Same reasoning, and same pair of lints, as the vendored `layout` subtree.
#![allow(dead_code, unused_imports)]

pub mod model;
pub mod parser;
pub mod preprocess;
pub mod shapes;
pub mod text;

pub use model::{
    Arrow, ClassDef, Direction, Edge, Flowchart, LinkStyle, LinkStyleTarget, Node, ParseError,
    Shape, Stroke, Subgraph,
};
pub use parser::parse;

#[cfg(test)]
mod tests;
