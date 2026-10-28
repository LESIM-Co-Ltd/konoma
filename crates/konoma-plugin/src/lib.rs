//! The plugin API of konoma.
//!
//! A plugin implements [`Plugin`] (declarations plus `open`) and [`Session`] (one opened file).
//! It never draws: it returns a [`View`] (a display type) and the host draws it with its own
//! widgets. (A plugin that needs free drawing returns a [`CanvasView`](view::CanvasView): cells and
//! picture placements as plain data that the host copies to the screen. See "Canvas" below.)
//! This crate does not depend on any terminal library; colors and modifiers are the small types
//! in [`style`], which the host maps onto its renderer.
//!
//! # Canvas (free drawing)
//!
//! Prefer a display type (text, document, table, image, list, diff) when one fits: it gets
//! scrolling, search, selection and copy from the host for free. A plugin whose picture is not
//! one of them (a chart, a game, a form) shows [`ViewKind::Canvas`]
//! and answers `View::Canvas(CanvasView)`.
//!
//! - **Cells.** A [`CanvasView`](view::CanvasView) is `cols x rows` cells (a grapheme and a
//!   [`Style`]); size it to the request (`CanvasView::new(req.cols, req.rows)`) and write with
//!   `set_string` / `fill`. [`CanvasView::check`](view::CanvasView::check) is the one contract of
//!   "plain data is safe", and the host refuses a canvas that fails it.
//!   `Session::view` is called at times the plugin does not choose, so it must not advance state.
//! - **Colors** are the plugin's to choose: read `cx.term.colors` ([`ColorDepth`]); the host does
//!   not convert.
//! - **Keys.** Declared actions work in the `canvas` key context; the host keeps a few keys for
//!   itself ([`keys::reserved`]). A plugin that needs raw keys returns `text_input: true` and
//!   receives them in [`Session::key`] until the user presses `Esc`.
//! - **Time.** `redraw_after` asks to be drawn again (33 ms to 24 h), only while the canvas is
//!   fully visible.
//! - **Pictures.** [`CanvasImage`] places a picture over the cells; its `id` is the picture's
//!   identity. See its documentation for the limits.
//!
//! ```
//! use konoma_plugin::prelude::*;
//! use konoma_plugin::testing::{open_bytes, view_canvas, CanvasText};
//!
//! struct Banner;
//! impl Plugin for Banner {
//!     fn manifest(&self) -> Manifest {
//!         Manifest::new("banner", "0.1.0")
//!             .bind(Bind::files(["*.banner"]))
//!             .shows(ViewKind::Canvas)
//!     }
//!     fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
//!         let title = match &input {
//!             Input::File { path, .. } => std::fs::read_to_string(path)?.trim().to_string(),
//!             _ => String::new(),
//!         };
//!         Ok(Box::new(BannerSession { title }))
//!     }
//! }
//!
//! struct BannerSession {
//!     title: String,
//! }
//! impl Session for BannerSession {
//!     fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
//!         let mut canvas = CanvasView::new(req.cols, req.rows);
//!         canvas.fill(CellRect::new(0, 0, req.cols, 1), Style::new().reversed());
//!         canvas.set_string(1, 0, &self.title, Style::new().reversed().bold());
//!         Ok(View::Canvas(canvas))
//!     }
//! }
//!
//! let (mut session, _file) = open_bytes(&Banner, "a.banner", b"hello\n").unwrap();
//! let canvas = view_canvas(&mut *session, &ViewReq::new(12, 2)).unwrap();
//! assert_eq!(CanvasText::of(&canvas).rows[0], " hello");
//! ```
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

#[cfg(feature = "app")]
pub mod app;
#[cfg(feature = "pictures")]
pub mod canvas_pictures;
#[cfg(feature = "pictures")]
pub mod table_ui;
#[cfg(feature = "config")]
pub mod config;
pub mod hints;
pub mod input;
pub mod key;
pub mod keys;
pub mod launch;
pub mod manifest;
pub mod prelude;
#[cfg(feature = "proc")]
pub mod proc;
#[cfg(feature = "ratatui")]
pub mod ratatui_draw;
pub mod registry;
pub mod scope;
pub mod session;
pub mod settings;
pub mod style;
pub mod testing;
pub mod text;
pub mod view;

pub use input::{Input, LazyResource, Origin, Resource, ResourceReq, Resources};
pub use key::{Key, KeyCode, KeyMods};
pub use manifest::{
    Act, ApiVersion, Bind, LaunchMode, Launcher, Manifest, Priority, SettingDecl, Target,
    Unavailable, Verb, VerbDecl, ViewKind,
};
pub use scope::{ReadScope, ScopeError};
pub use session::{
    ActCx, Cancel, ColorDepth, Failure, FailureKind, OpenCx, Outcome, Plugin, Session, TermCaps,
    ViewCx,
};
pub use settings::{Settings, Value};
pub use style::{Color, Line, Modifier, Span, Style};
pub use text::Tr;
pub use view::{
    symbol_width, CanvasCell, CanvasImage, CanvasIssue, CanvasView, CellRect, View, ViewReq,
};
