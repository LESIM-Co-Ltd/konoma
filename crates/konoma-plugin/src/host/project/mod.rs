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
//! - [`state`]: per-context frames, row identity, filters, marks and resolved screen motions.
//! - [`prompt`]: project choosers, prompt answers and failed-act text retries.
//! - [`input`] and [`actions`]: ordered keys, incremental replay, action scopes and guarded jobs.
//! - [`effects`]: origin-bearing outcomes, reply adoption and the context/view lifecycle.

pub mod actions;
pub mod content;
pub mod detection;
pub mod diff;
pub mod diff_blocks;
pub mod diff_frame;
pub mod diff_keys;
pub mod diff_presentation;
pub mod draw;
pub mod effects;
pub mod input;
pub mod presentation;
pub mod prompt;
pub mod runtime;
pub mod state;
pub use actions::{
    ProjectActFacts, ProjectAction, ProjectActionInvocation, ProjectActionScope, ProjectCommand,
    ProjectDispatch, ProjectHost, ProjectNotice, ProjectUpdate, ProjectWritePolicy,
};
pub use effects::{ProjectContextInfo, ProjectEffect, ProjectEvent, ProjectInvalidation};
pub use input::{ProjectContextMut, ProjectKeyFacts, ProjectReplay, ProjectReplayState};

#[cfg(test)]
mod actions_tests;
#[cfg(test)]
mod effects_tests;
#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod presentation_help_tests;
#[cfg(test)]
mod presentation_tests;

pub use prompt::{
    ProjectActionId, ProjectDialog, ProjectInvocation, ProjectOrigin, ProjectPrompt,
    ProjectPromptBlockers, ProjectPromptUpdate, PromptInvocation,
};

#[cfg(test)]
mod prompt_tests;

pub use state::{
    ProjectFrame, ProjectInput, ProjectKey, ProjectMotionResult, ProjectScreenState,
    ProjectViewport,
};

#[cfg(test)]
mod state_tests;

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

pub mod md_diff;
#[cfg(test)]
mod md_diff_tests;
#[cfg(test)]
mod media_diff_worker_tests;

pub mod media_diff;
