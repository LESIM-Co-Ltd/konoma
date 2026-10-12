//! Process-wide basics of konoma, below every other crate of the workspace.
//!
//! - [`panic`]: catching a panic silently (background workers must always report back).
//! - [`private_dir`]: the per-process private temp directories (`konoma-<kind>-<pid>`).

pub mod panic;
pub mod private_dir;
