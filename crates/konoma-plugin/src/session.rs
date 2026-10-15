//! The entry traits and the contexts the host passes in.

use std::fmt;
use std::sync::Arc;

use crate::input::Input;
use crate::key::Key;
use crate::manifest::Manifest;
use crate::settings::Settings;
use crate::text::Tr;
use crate::view::{Layout, View, ViewReq};

/// The entry point of a plugin. The host reads `manifest` at startup and calls nothing else until
/// a file is opened.
pub trait Plugin: Send + Sync + 'static {
    /// The declarations a, b and c.
    fn manifest(&self) -> Manifest;
    /// Open a file (or a content-type input). Heavy work is fine here: the host calls it from a
    /// worker.
    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure>;
}

/// One opened input.
pub trait Session: Send {
    /// The representations and pages this content has.
    fn layout(&self) -> Layout {
        Layout::single()
    }
    /// The current display.
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure>;
    /// A declared action. Return [`Outcome::Unhandled`] for ids you do not know.
    fn act(&mut self, action: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        let _ = (action, cx);
        Ok(Outcome::Unhandled)
    }
    /// A key press, while the view asked for text input ([`crate::view::CanvasView::text_input`]).
    ///
    /// - Every key but `Esc` comes here, layer-0 keys (`?`, `Q`, `t`...) and `Ctrl-C` included,
    ///   as ordinary keys; a paste is one [`crate::key::KeyCode::Paste`]. Keys the API has no name
    ///   for (`Insert`, media keys) are dropped.
    /// - `Esc` is the host's: it leaves the input state whatever the plugin answers, and arrives
    ///   here too as a notice that cannot be refused (it gets through even when the backlog below
    ///   is full: the last place is kept for it). After it the host does not give the typing
    ///   back because a later view asks for it again: only an answer to something the user did
    ///   does (a declared action they pressed, or opening the file again). See
    ///   [`crate::view::CanvasView::text_input`].
    /// - [`Outcome::Unhandled`] does nothing: the key does not fall back to the host's keymap or to
    ///   the plugin's declared actions. Return `Unhandled` for keys you do not use, for example
    ///   those with `CTRL` or `ALT` ([`crate::key::KeyMods`]).
    /// - Return [`Outcome::Redraw`] to have the view asked for again. Keys are never dropped:
    ///   those that came while a call was running arrive one by one in order. The host holds a
    ///   bounded backlog (4096 keys, a paste cut at 1 MiB, 4 MiB of waiting pastes in total) and refuses new input past it, so a
    ///   call that never returns does not grow memory without bound.
    fn key(&mut self, key: &Key, cx: &ActCx) -> Result<Outcome, Failure> {
        let _ = (key, cx);
        Ok(Outcome::Unhandled)
    }
    /// Whether a declared action works now. The host uses one answer for the key handler, the
    /// footer and the help, so a hint is shown only while the key acts.
    fn enabled(&self, action: &str) -> bool {
        let _ = action;
        true
    }
}

/// How many colors the terminal shows. The host does not convert a plugin's colors: a plugin
/// that draws its own cells ([`crate::view::CanvasView`]) picks colors by this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ColorDepth {
    /// The 16 named colors. The default: when nothing is known, promise the least.
    #[default]
    Ansi16,
    /// The 256-color palette ([`crate::Color::Indexed`]).
    Ansi256,
    /// 24-bit color ([`crate::Color::Rgb`]).
    TrueColor,
}

/// What the terminal can do.
///
/// Build one with [`TermCaps::new`] and the builders (more fields may be added later).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TermCaps {
    /// Whether real pixels can be drawn (a graphics protocol).
    pub pixels: bool,
    /// How many colors can be shown.
    pub colors: ColorDepth,
}

impl TermCaps {
    /// The least a terminal promises: no pixels, 16 colors.
    pub fn new() -> TermCaps {
        TermCaps::default()
    }

    /// The same with `pixels` set.
    pub fn pixels(mut self, on: bool) -> TermCaps {
        self.pixels = on;
        self
    }

    /// The same with `colors` set.
    pub fn colors(mut self, depth: ColorDepth) -> TermCaps {
        self.colors = depth;
        self
    }
}

/// A cancellation check the host passes to long work.
#[derive(Clone)]
pub struct Cancel(Arc<dyn Fn() -> bool + Send + Sync>);

impl Cancel {
    /// Never cancelled.
    pub fn never() -> Cancel {
        Cancel(Arc::new(|| false))
    }

    pub fn from_fn(f: impl Fn() -> bool + Send + Sync + 'static) -> Cancel {
        Cancel(Arc::new(f))
    }

    pub fn is_cancelled(&self) -> bool {
        (self.0)()
    }
}

impl fmt::Debug for Cancel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cancel(cancelled: {})", self.is_cancelled())
    }
}

macro_rules! context {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug)]
        pub struct $name<'a> {
            /// The plugin's `[plugins.<id>]` settings.
            pub settings: &'a Settings,
            /// The user's language (`"en"`, `"ja"`).
            pub lang: &'a str,
            pub cancel: Cancel,
            pub term: TermCaps,
            /// Whether the plugin may start an external program (the user's `[external]` switch
            /// for what the plugin shows). Off by default: a plugin that has an external fallback
            /// must stay silent unless the host says yes.
            pub external: bool,
        }

        impl<'a> $name<'a> {
            pub fn new(settings: &'a Settings, lang: &'a str) -> $name<'a> {
                $name {
                    settings,
                    lang,
                    cancel: Cancel::never(),
                    term: TermCaps::default(),
                    external: false,
                }
            }

            /// English, no settings, never cancelled, no pixels: for tests.
            pub fn bare() -> $name<'static> {
                $name::new(Settings::empty(), "en")
            }

            pub fn with_cancel(mut self, cancel: Cancel) -> $name<'a> {
                self.cancel = cancel;
                self
            }

            pub fn with_term(mut self, term: TermCaps) -> $name<'a> {
                self.term = term;
                self
            }

            pub fn with_external(mut self, external: bool) -> $name<'a> {
                self.external = external;
                self
            }
        }
    };
}

context!(
    /// What `Plugin::open` receives besides the input.
    OpenCx
);
context!(
    /// What `Session::view` receives besides the request.
    ViewCx
);
context!(
    /// What `Session::act` receives besides the action id.
    ActCx
);

/// The result of an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not an action of this session.
    Unhandled,
    /// Redraw the view.
    Redraw,
    /// Show a short message.
    Flash(Tr),
    /// Read the file again.
    Reload,
}

/// Why something failed. The host decides what to do from the kind: for example a
/// [`FailureKind::TooHeavy`] or [`FailureKind::Timeout`] request may be asked again smaller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Unsupported,
    /// The file is password-protected.
    Encrypted,
    Corrupt,
    TooLarge,
    TooHeavy,
    Timeout,
    Cancelled,
    Io,
    External,
}

/// A failure with a message for the user and optional detail for logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: Tr,
    pub detail: Option<String>,
}

macro_rules! failure_ctor {
    ($($(#[$doc:meta])* $fn:ident => $kind:ident,)*) => {
        $(
            $(#[$doc])*
            pub fn $fn(message: impl Into<Tr>) -> Failure {
                Failure::new(FailureKind::$kind, message)
            }
        )*
    };
}

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<Tr>) -> Failure {
        Failure {
            kind,
            message: message.into(),
            detail: None,
        }
    }

    failure_ctor! {
        unsupported => Unsupported,
        encrypted => Encrypted,
        corrupt => Corrupt,
        too_large => TooLarge,
        too_heavy => TooHeavy,
        timeout => Timeout,
        cancelled => Cancelled,
        io => Io,
        external => External,
    }

    /// Add detail text for logs.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Failure {
        self.detail = Some(detail.into());
        self
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message.en())?;
        if let Some(d) = &self.detail {
            write!(f, " ({d})")?;
        }
        Ok(())
    }
}

impl std::error::Error for Failure {}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Failure {
        Failure::io(crate::t!(en: "I/O error", ja: "入出力エラー")).with_detail(e.to_string())
    }
}
