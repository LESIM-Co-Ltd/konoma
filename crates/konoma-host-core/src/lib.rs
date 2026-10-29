//! The shared part of konoma's configuration: display language, `[editor]`, `[keys]` overrides
//! and `SharedConfig`, the slice of `config.toml` that konoma and the plugin SDK's small host
//! read the same way. No renderer (ratatui), no plugin API, no dependency on the host. [`term`] holds the terminal hand-off and its process-wide lock.

#[cfg(feature = "clipboard")]
pub mod clipboard;
pub mod copy;
pub mod editor;
pub mod help;
pub mod help_msg;
pub mod keymap;
pub mod keys;
pub mod lang;
pub mod menu;
pub mod paths;
pub mod shared;
pub mod sniff;
pub mod term;
