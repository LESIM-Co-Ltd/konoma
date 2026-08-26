//! Reading mermaid's entity-relationship language — the other half of stage 3.
//!
//! The whole of this module answers one question: *what did the author write?* It produces an
//! [`ErDiagram`] — entities with their attributes, relationships with a cardinality at each end,
//! and the subgraphs that group them — and stops there. [`render::er`] turns that into a picture,
//! with stage 1's layout rather than one of its own.
//!
//! [`render::er`]: crate::preview::mermaid::render::er
//!
//! ```text
//! let d = parse("erDiagram\n  CUSTOMER ||--o{ ORDER : places").unwrap();
//! d.entities.len()                        == 2
//! d.relationships[0].to_cardinality       == Cardinality::ZeroOrMore
//! ```
//! (konoma has no library target, so this is an illustration rather than a doctest; the same case
//! is asserted for real in [`tests`].)
//!
//! # Layout of the module
//!
//! | file | what it owns |
//! |---|---|
//! | [`parser`] | the grammar: entities, attribute blocks, cardinalities, subgraphs |
//! | [`model`] | the intermediate representation the parser produces |
//!
//! # What it does not do
//!
//! `classDef`, `class`, `style`, `%%` comments, front matter, `accTitle` and `accDescr` in both
//! forms are *recognised* and dropped (§2-3). Neither a `direction` written inside a `subgraph`
//! nor a `layout: elk` in front matter changes anything — §0-4 settles that ELK is not
//! implemented and that not falling over is the whole requirement.
//!
//! Two things mermaid draws that konoma does not, each pinned by a test:
//!
//! * **striped attribute rows.** Upstream fills alternate rows with `rowEven`/`rowOdd` to
//!   separate them. konoma composites onto the terminal (§1), where an opaque stripe is a grey
//!   band rather than a subtle tint, so the rows are separated by a rule instead.
//! * **`markdown` in an entity name.** The label type is `markdown` upstream, so
//!   `"This **is** _Markdown_"` comes out bold and italic. konoma has no markdown renderer inside
//!   a diagram label (§4-4 says the same about `<b>` in a flowchart), so the text is drawn as
//!   written.

// konoma is a binary crate, so `pub` marks nothing as used. Same reasoning, and the same pair of
// lints, as the sibling parsers.
#![allow(dead_code, unused_imports)]

pub mod model;
pub mod parser;

pub use model::{
    Attribute, Cardinality, Direction, Entity, ErDiagram, Identification, ParseError, Relationship,
    SubGraph,
};
pub use parser::{is_er_diagram, parse};

#[cfg(test)]
mod tests;
