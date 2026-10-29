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
//! - Every path a child sends (a text file, a picture file, an SVG's base directory, the origin of
//!   a document, a list row's target) must be absolute: the side that puts the view on the wire
//!   refuses a relative one with a failure, because the child's working directory means nothing to
//!   the host. (The host's own input is made absolute before it is sent.)
//! - An SVG from a child is never trusted (`trusted` is false on arrival, whatever the child said):
//!   its external references are not read.
//! - A canvas travels per row as style runs that refer to a table of styles by number (one table
//!   per answer, so a style is written once, not per run); the sender's `CanvasView::check` must
//!   pass. Coloured text whose encoding would pass [`wire::STYLED_MAX_ENCODED`] is sent plain
//!   (with the reason on the child's stderr).
//! - Side files: the total a child hands over in one answer is at most
//!   [`wire::SIDE_TOTAL_MAX_FROM_CHILD`] (256 MiB, the amount the host holds of canvases), the
//!   total the host hands over in one message [`wire::SIDE_TOTAL_MAX`]. The ones of an answer that
//!   fails to convert are deleted unread; [`client::sweep_stale_side_dirs`] removes the directories
//!   a host that died left behind.
//!
//! # Deadlines and failure
//!
//! A call that does not answer in time, a child that dies, a broken or oversized frame, and a child
//! over its memory limit end the whole child (its process group is killed). The calls in flight
//! fail with a [`Failure`](crate::Failure) whose detail is the tail of the child's stderr, every
//! other session of that child fails afterwards, and the next `open` starts a new child.
//! [`client::Limits`] names the deadlines and says why each has its value.
//!
//! The child works through the calls of all its sessions in order, so a call's deadline (and the
//! grace after a cancel) counts only from when the child starts on it, not from when the call was
//! sent. A call cancelled while it waits its turn is answered "cancelled" without running.
//!
//! A host that stops a child on purpose (`stop`, the idle stop, a drop) sends `Shutdown` first: the
//! child drops its sessions in order and exits; only a child that has not left within
//! `Limits::shutdown_grace` is killed. A child whose host has gone (its input ends) raises every
//! cancel flag and exits after [`ServeOptions::exit_after_host_gone`].
//! A start that fails is remembered for `Limits::start_retry_after`: the opens in that window fail
//! with the same reason without starting the program again.

#![allow(clippy::module_name_repetitions)]

pub mod client;
pub mod frame;
mod os;
pub mod proto;
mod server;
pub mod wire;

pub use server::{cli_main, cli_main_bundled, cli_main_many, serve, serve_io, ServeOptions};

/// Emit `main` for a plugin program: `--serve` serves the host on stdin and stdout, `--manifest`
/// prints the declaration, `--version` and `--help` print what they say. Given a file, the program
/// opens it in a terminal of its own when konoma-plugin is built with the `app` feature (and says
/// it cannot otherwise).
///
/// More than one plugin may be given: `export!(A, B)`. The protocol serves one plugin per
/// process, so `--serve`, `--manifest` and `--version` then take `--plugin <id>`; a file goes to
/// the plugin the registry's rules pick.
///
/// ```ignore
/// konoma_plugin::export!(MyPlugin);
/// konoma_plugin::export!(MyPlugin, MyOtherPlugin);
/// konoma_plugin::export!(@program my_crate::program());
/// // With plugins carried only as destinations of a reopened or embedded content (the standalone
/// // app looks them up; the file, `--serve` and `--manifest` never reach them):
/// konoma_plugin::export!(@program my_crate::program(), @bundled my_crate::bundled());
/// ```
#[macro_export]
macro_rules! export {
    // A program whose plugins come from one function of its library (`fn() -> Vec<Box<dyn
    // Plugin>>`), so a test can read the same list.
    (@program $plugins:expr) => {
        fn main() -> ::std::process::ExitCode {
            $crate::proc::cli_main_many($plugins)
        }
    };
    (@program $plugins:expr, @bundled $bundled:expr) => {
        fn main() -> ::std::process::ExitCode {
            $crate::proc::cli_main_bundled($plugins, $bundled)
        }
    };
    ($plugin:expr $(,)?) => {
        fn main() -> ::std::process::ExitCode {
            $crate::proc::cli_main($plugin)
        }
    };
    ($first:expr, $($rest:expr),+ $(,)?) => {
        fn main() -> ::std::process::ExitCode {
            $crate::proc::cli_main_many(::std::vec![
                ::std::boxed::Box::new($first) as ::std::boxed::Box<dyn $crate::Plugin>,
                $(::std::boxed::Box::new($rest) as ::std::boxed::Box<dyn $crate::Plugin>),+
            ])
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
