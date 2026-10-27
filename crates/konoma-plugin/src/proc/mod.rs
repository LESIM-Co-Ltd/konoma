//! The child-process transport of the plugin API.
//!
//! A plugin built as its own program serves the host over its standard input and output; the host
//! drives it through [`client::ProcPlugin`], which implements [`Plugin`](crate::Plugin) like an
//! in-process plugin. Nothing here knows the host application.
//!
//! - [`wire`]: the wire types (the in-memory API types have no serde) and their conversions. The
//!   receiving side treats the sender as hostile.
//! - [`frame`]: a 4-byte big-endian length and a MessagePack body; at most 64 MiB.
//! - [`proto`]: the handshake and the messages.
//! - [`server`]: the child side ([`serve`], [`serve_io`], the `export!` macro, [`cli_main`]).
//! - [`client`]: the host side ([`client::ProcPlugin`]).
//!
//! # What does not cross
//!
//! - `Arc` sharing is lost: two `Arc`s of one buffer arrive as two buffers.
//! - A [`Resource::Lazy`](crate::Resource) is not sent; the child sees the resource as absent.
//!   (A lazy resource a child returns in a document is dropped, and a lazy picture is refused.)
//! - A table's per-cell `detail` and a text view's `styler` do not cross. The child sends the
//!   table's cells (at most [`wire::TABLE_MAX_CELLS`]) and the text already styled (a file over
//!   [`wire::STYLE_MAX_BYTES`] is sent as the file, unstyled).
//! - A child's [`ImageData::File`](crate::view::ImageData) and text file must be absolute paths.
//! - A canvas travels per row as style runs; the sender's `CanvasView::check` must pass.
//!
//! # Deadlines and failure
//!
//! A call that does not answer in time, a child that dies, a broken or oversized frame, and a child
//! over its memory limit end the whole child (its process group is killed). The calls in flight
//! fail with a [`Failure`](crate::Failure) whose detail is the tail of the child's stderr, every
//! other session of that child fails afterwards, and the next `open` starts a new child.
//! [`client::Limits`] names the deadlines and says why each has its value.

#![allow(clippy::module_name_repetitions)]

pub mod client;
pub mod frame;
mod os;
pub mod proto;
mod server;
pub mod wire;

pub use server::{cli_main, serve, serve_io, ServeOptions};

/// Emit `main` for a plugin program: `--serve` serves the host on stdin and stdout, `--manifest`
/// prints the declaration, `--version` and `--help` print what they say. Given a file, the program
/// is not yet a standalone app and says so.
///
/// ```ignore
/// konoma_plugin::export!(MyPlugin);
/// ```
#[macro_export]
macro_rules! export {
    ($plugin:expr) => {
        fn main() -> ::std::process::ExitCode {
            $crate::proc::cli_main($plugin)
        }
    };
}

/// The manifest as pretty JSON: what `--manifest` prints and what the host keeps as its cache of a
/// plugin it has not launched yet.
pub fn manifest_to_json(m: &crate::Manifest) -> Result<String, wire::WireError> {
    let w = wire::manifest_to_wire(m)?;
    serde_json::to_string_pretty(&w).map_err(|e| wire::WireError::bad(e.to_string()))
}

/// Read a manifest written by [`manifest_to_json`]. The file is the host's own cache, but it is read
/// as untrusted: a bad file is an error, never a panic.
pub fn manifest_from_json(s: &str) -> Result<crate::Manifest, wire::WireError> {
    let w: wire::WManifest =
        serde_json::from_str(s).map_err(|e| wire::WireError::bad(e.to_string()))?;
    wire::manifest_from_wire(w)
}
