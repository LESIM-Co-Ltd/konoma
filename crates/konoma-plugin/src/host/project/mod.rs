//! Project discovery and watching, written once for every host (konoma and the standalone app).
//!
//! A *project* is a directory that carries one of the markers a plugin declares
//! ([`ProjectDecl`](crate::project::ProjectDecl)): `.git`, `.jj`, `*.xcodeproj`, ... Which plugins
//! declare projects is the host's knowledge (its registry, and which plugins run as child
//! processes), so the host hands the declarations over as a value; everything after that is shared:
//!
//! - [`detection`]: the upward search from a directory, the worker that watches the markers and the
//!   declared watch paths (bounded event queue, overflow turns into a rescan, watch failures are
//!   reported but keep the last answer), and [`ProjectDetection`], the per-tab handle the UI polls.

pub mod detection;

#[cfg(any(test, feature = "test-hooks"))]
pub use detection::assert_native_watch_delivery;
pub use detection::{
    affected, detect, detect_directories, watch_targets, MatchedProject, ProjectDeclaration,
    ProjectDeclarationSet, ProjectDetection,
};
