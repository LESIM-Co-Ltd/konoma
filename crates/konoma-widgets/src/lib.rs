//! Display parts for ratatui that do not depend on the application's state: windowed file
//! reading, kitty graphics transmit, image layout, diff rows, icon glyphs and the Markdown
//! document renderer. The host (konoma) and single-purpose apps use the same parts.

// The renderer's doc comments quote Markdown fences (```mermaid …) in prose; rustdoc would read them
// as code blocks with odd attributes. They are prose, not doctests.
#![allow(rustdoc::invalid_codeblock_attributes)]

pub mod colors;
pub mod gitdiff;
pub mod icons;
pub mod kitty;
pub mod layout;
pub mod markdown;
pub mod text;
pub mod window;
