//! The shared part of konoma's configuration: display language, `[editor]`, `[keys]` overrides
//! and `SharedConfig`, the slice of `config.toml` that konoma and the plugin SDK's small host
//! read the same way. No terminal library, no plugin API, no dependency on the host.

pub mod editor;
pub mod keys;
pub mod lang;
pub mod shared;
