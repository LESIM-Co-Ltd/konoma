//! The plugin API of konoma.
//!
//! A plugin implements [`Plugin`] (declarations plus `open`) and [`Session`] (one opened file).
//! It never draws: it returns a [`View`] (a display type) and the host draws it with its own
//! widgets. This crate does not depend on any terminal library; colors and modifiers are the small
//! types in [`style`], which the host maps onto its renderer.
//!
//! Modules:
//! - [`text`]: per-language strings ([`Tr`], [`t!`])
//! - [`style`]: colors, modifiers, spans and lines
//! - [`settings`]: the `[plugins.<id>]` values
//! - [`manifest`]: declarations a (display bindings), b (launch verbs) and c (actions and help)
//! - [`input`]: what a plugin is asked to open
//! - [`view`]: the display types a plugin returns: text, document, table, image, list, diff, and
//!   the free-drawing [`CanvasView`](view::CanvasView) (cells and picture placements as plain data)
//! - [`key`]: a key press as data, for [`Session::key`] (text input in a canvas)
//! - [`session`]: the entry traits and their contexts
//! - [`registry`]: the rules that settle which plugin shows a file
//! - [`hints`]: which declared actions are shown (one rule for the host and for tests)
//! - [`keys`]: key notation, the layered reserved keys, and manifest validation
//! - [`testing`]: checks that do not need the host

pub mod hints;
pub mod input;
pub mod key;
pub mod keys;
pub mod manifest;
pub mod prelude;
pub mod registry;
pub mod session;
pub mod settings;
pub mod style;
pub mod testing;
pub mod text;
pub mod view;

pub use input::{Input, LazyResource, Origin, Resource, ResourceReq, Resources};
pub use key::{Key, KeyMods};
pub use manifest::{
    Act, ApiVersion, Bind, LaunchMode, Launcher, Manifest, Priority, SettingDecl, Target,
    Unavailable, Verb, VerbDecl, ViewKind,
};
pub use session::{
    ActCx, Cancel, ColorDepth, Failure, FailureKind, OpenCx, Outcome, Plugin, Session, TermCaps,
    ViewCx,
};
pub use settings::{Settings, Value};
pub use style::{Color, Line, Modifier, Span, Style};
pub use text::Tr;
pub use view::{CanvasCell, CanvasImage, CanvasIssue, CanvasView, CellRect, View, ViewReq};
