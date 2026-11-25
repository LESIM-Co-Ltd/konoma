// Shared VCS data types. Production reads and writes go through `crate::vcs`.
pub use konoma_vcs_core::git::{DiffLine, DiffLineKind, FileStatus};

// Legacy fixture helpers are available only to tests, never to host features. Without the `git`
// feature no test reads them any more, so the glob would be an unused import there.
#[cfg(all(test, feature = "git"))]
pub use konoma_vcs_core::git::*;
