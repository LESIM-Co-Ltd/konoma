//! Reading mermaid's sequence-diagram language — stage 4.
//!
//! The whole of this module answers one question: *what did the author write?* It produces a
//! [`SequenceDiagram`] — participants in the order they first appeared, the boxes that group them,
//! and one flat stream of events — and stops there. [`render::sequence`] turns that into a
//! picture.
//!
//! [`render::sequence`]: crate::preview::mermaid::render::sequence
//!
//! ```text
//! let d = parse("sequenceDiagram\n  Alice->>+John: Hi").unwrap();
//! d.participants.len()   == 2
//! d.events.len()         == 2     // the message, then the `+`'s activation
//! ```
//! (konoma has no library target, so this is an illustration rather than a doctest; the same case
//! is asserted for real in [`tests`].)
//!
//! # Layout of the module
//!
//! | file | what it owns |
//! |---|---|
//! | [`parser`] | the grammar: participants, messages, notes, blocks, activations, `box` |
//! | [`model`] | the intermediate representation the parser produces |
//!
//! # This is the one stage that does not use dagre, and that is the design
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §7 says so. A sequence diagram's layout is not a search: the
//! participants go across the top in the order they appeared, the lifelines hang down, and the
//! messages stack in source order. There is no ranking to do and no crossing to minimise, so
//! [`render::sequence::lay_out`] builds a `Diagram` in one pass instead of going through
//! `lay_out_spec` — which is the layered-graph pipeline and would have to be lied to about what
//! the nodes and edges are. **Everything below layout is shared**: the same `Diagram`, the same
//! `Glyph`, the same `Tip`, the same `svg::emit`, the same palettes.

// konoma is a binary crate, so `pub` marks nothing as used. Same reasoning, and the same pair of
// lints, as the sibling parsers.
#![allow(dead_code, unused_imports)]

pub mod model;
pub mod parser;

pub use model::{
    AutoNumber, BlockKind, Event, Head, Line, Message, Note, ParseError, Participant,
    ParticipantBox, ParticipantKind, Placement, SectionKind, SequenceDiagram, Signal,
};
pub use parser::{is_sequence_diagram, parse};

#[cfg(test)]
mod tests;
