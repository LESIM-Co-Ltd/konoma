//! The Markdown core of konoma: the block model built from one walk of pulldown-cmark's events
//! ([`model`]), the block-level alignment of two versions of a document ([`diff`]) and the
//! source-text analysis both rest on ([`syntax`]). Nothing here draws: styling and layout stay in
//! the host.

pub mod diff;
pub mod model;
pub mod syntax;

// Source-text corpora shared with the host's tests (`test-hooks`); not part of the API.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub mod corpus;
