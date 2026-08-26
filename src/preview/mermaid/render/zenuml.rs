//! Drawing a `zenuml`.
//!
//! There is nothing here but the seam. [`crate::preview::mermaid::zenuml`] reads a ZenUML source
//! into the **sequence model**, and stage 4's renderer draws it — which is the whole design: a
//! ZenUML source and a `sequenceDiagram` source say the same kinds of thing, so a second layout
//! would be a second way to be wrong about the same geometry.
//!
//! What that buys, for free: the sequence diagram's own invariants (y runs strictly down, a
//! message's ends are on its two lifelines, an activation bar spans from its opening message to
//! its closing one, a frame contains what is inside it) all hold for a ZenUML diagram, because
//! they are statements about the geometry rather than about the language it came from.

use super::{Diagram, RenderError, Theme};

/// Reads a ZenUML source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let diagram = crate::preview::mermaid::zenuml::parse(code)?;
    let laid = super::sequence::lay_out(&diagram)?;
    Ok(super::svg::emit(&laid, &Theme::named(theme)))
}

/// The geometry, for the tests that state things about it.
pub fn lay_out(code: &str) -> Result<Diagram, RenderError> {
    let diagram = crate::preview::mermaid::zenuml::parse(code)?;
    super::sequence::lay_out(&diagram)
}
