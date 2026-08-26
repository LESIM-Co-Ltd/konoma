//! Reading mermaid's class-diagram language — half of stage 3 of konoma's own renderer.
//!
//! The whole of this module answers one question: *what did the author write?* It produces a
//! [`ClassDiagram`] — classes with their members, relationships, notes and namespaces — and stops
//! there. No text is measured, no box is sized, no SVG is emitted; [`render::class`] does that,
//! and it does it with stage 1's layout rather than one of its own
//! (`docs/FEATURE-MERMAID-RENDERER.md` §7).
//!
//! [`render::class`]: crate::preview::mermaid::render::class
//!
//! ```text
//! let d = parse("classDiagram\n  Animal <|-- Duck\n  Animal : +int age").unwrap();
//! d.classes.len()   == 2
//! d.relations.len() == 1
//! d.class("Animal").unwrap().attributes[0].display() == "+int age"
//! ```
//! (konoma has no library target, so this is an illustration rather than a doctest; the same case
//! is asserted for real in [`tests`].)
//!
//! # Layout of the module
//!
//! | file | what it owns |
//! |---|---|
//! | [`parser`] | the grammar, the member decomposition, and the generic-type rewriting |
//! | [`model`] | the intermediate representation the parser produces |
//!
//! Preprocessing is *not* here: a class diagram goes through exactly the same five rewrites a
//! flowchart does (`flowchart::preprocess`), because those are mermaid's `preprocessDiagram()`
//! and it runs before any diagram's parser.
//!
//! # What it does not do
//!
//! `classDef`, `cssClass`, `style`, `click`, `callback`, `link`, `%%` comments, front matter,
//! `accTitle` and `accDescr` in both forms are all *recognised* — which is the point, since a
//! line-based parser that did not recognise them would turn `classDef someclass fill:#f96` into a
//! box (§2-3). None of them is drawn, and neither is a `direction` written inside a `namespace`.
//!
//! Two constructs are recognised and deliberately reduced, each pinned by a test that asserts the
//! reduction rather than hoping for it:
//!
//! * **`hierarchicalNamespaces: false`** is a front-matter config, not syntax, and konoma reads no
//!   config out of front matter. A dotted `namespace A.B.C` therefore always nests.
//! * **a lollipop's phantom class**. mermaid turns `bar ()-- foo` into an *interface node* with
//!   `opacity: 0` plus a real class — an invisible box that still takes up room in the layout.
//!   konoma draws the ring on the line between two ordinary classes instead, which says the same
//!   thing without a box nobody can see.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this model that only the
// tests read look unreachable to rustc even though [`parse`] and [`is_class_diagram`] are both on
// the drawing path. Same reasoning, and the same pair of lints, as the sibling parsers.
#![allow(dead_code, unused_imports)]

pub mod model;
pub mod parser;

pub use model::{
    Class, ClassDiagram, Direction, Line, Marker, Member, MemberKind, Namespace, Note, ParseError,
    Relation,
};
pub use parser::{is_class_diagram, parse};

#[cfg(test)]
mod tests;
