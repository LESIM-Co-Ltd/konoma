//! The pictures of a document (inline images, mermaid fences, math), host-neutral: where an image
//! URL points, the download of a remote one, and how a file is decoded. Both hosts (konoma and the
//! standalone plugin app) use this one implementation, so a document looks the same in either.
//!
//! The host owns the cache the pixels go into, the threads, the picker and the drawing.

pub mod files;
pub mod remote;
pub mod requests;
pub mod resources;
pub mod slots;

pub use files::*;
pub use remote::*;
pub use requests::*;
pub use resources::*;
pub use slots::*;

#[cfg(test)]
mod tests;
