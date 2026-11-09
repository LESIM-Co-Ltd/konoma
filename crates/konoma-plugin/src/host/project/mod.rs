//! Project discovery, session ownership and prepared views, shared by project hosts.
//!
//! A *project* is a directory that carries one of the markers a plugin declares
//! ([`ProjectDecl`](crate::project::ProjectDecl)): `.git`, `.jj`, `*.xcodeproj`, ... Which plugins
//! declare projects is the host's knowledge (its registry, and which plugins run as child
//! processes), so the host hands the declarations over as a value; everything after that is shared:
//!
//! - [`detection`]: the upward search from a directory, the worker that watches the markers and the
//!   declared watch paths (bounded event queue, overflow turns into a rescan, watch failures are
//!   reported but keep the last answer), and [`ProjectDetection`], the per-tab handle the UI polls.
//! - [`runtime`]: one serial session owner per project, taking host policy and localized messages
//!   as values, with opaque action effects and pending invocations.
//! - [`content`] and [`diff`]: the shared widgets' prepared content and windowed diff layout cache.

pub mod content;
pub mod detection;
pub mod diff;
pub mod runtime;

pub use content::{content, ProjectContent};
pub use diff::{ProjectDiff, ProjectDiffLayout};
pub use runtime::{
    ProjectEnv, ProjectJob, ProjectMessages, ProjectReply, ProjectRuntime, ScreenKey,
};

#[cfg(test)]
#[path = "diff_tests.rs"]
mod diff_tests;

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod runtime_tests;

#[cfg(any(test, feature = "test-hooks"))]
pub use detection::assert_native_watch_delivery;
pub use detection::{
    affected, detect, detect_directories, watch_targets, MatchedProject, ProjectDeclaration,
    ProjectDeclarationSet, ProjectDetection,
};
