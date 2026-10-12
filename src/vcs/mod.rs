// Which version-control system backs a directory, and the one read surface every backend
// implements, live in the `konoma-vcs-core` crate; re-exported here so every `crate::vcs::...` path
// in the host (including `crate::vcs::jj`) is unchanged.

pub use konoma_vcs_core::vcs::*;
