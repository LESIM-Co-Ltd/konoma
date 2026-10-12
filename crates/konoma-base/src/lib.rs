//! Process-wide basics of konoma, below every other crate of the workspace.
//!
//! - [`panic`]: catching a panic silently (background workers must always report back).
//! - [`epoch`]: formatting Unix epoch seconds as UTC dates.
//! - [`process`]: whether this process is the application itself (gates the `test-hooks` seams).
//! - [`private_dir`]: the per-process private temp directories (`konoma-<kind>-<pid>`).

pub mod epoch;
pub mod panic;
pub mod private_dir;
pub mod process;
