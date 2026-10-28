//! The standalone app: a plugin program that is given a file shows it in a terminal of its own.
//!
//! This is a small host. It picks the plugin with the registry's rules (the same
//! [`Registry::resolve_file`] the konoma host uses), opens a session, and draws the view with
//! ratatui. Only the display types that need no shared widget code are drawn here:
//! [`View::Message`](crate::View), [`View::Text`](crate::View) (a file read in windows, a string or
//! styled lines) and [`View::Canvas`](crate::View). Any other view says it cannot be shown yet.
//!
//! The loop ([`run_with`]) takes its terminal and its events from outside, so it runs on a
//! ratatui `TestBackend` in tests; [`run`] is the thin entry that owns the real terminal.

mod draw;
mod looping;
mod pictures;
mod term;
mod window;

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::registry::{FileFacts, PluginOrigin, Registry, Resolution};
use crate::session::{ColorDepth, Failure, OpenCx, Plugin, Session, TermCaps};
use crate::{Input, Settings};

pub use looping::{run_with, run_with_pictures, AppCx, AppEvent, AppKeys, EventSource, Next};
pub use pictures::{delete_placed_pictures, InProcessSvg, Pictures};
pub use term::{install_panic_hook_with, key_from_crossterm};

/// Why the app did not run (or stopped). Every case has a plain message for standard error.
#[derive(Debug)]
pub enum AppError {
    /// The file does not exist.
    NoFile(PathBuf),
    /// The path is not a file (a directory, say).
    NotAFile(PathBuf),
    /// The file cannot be read.
    Unreadable(PathBuf, std::io::Error),
    /// No plugin of this program shows the file.
    NotHandled { path: PathBuf, why: String },
    /// A plugin of the program was refused at registration (its manifest is not usable).
    Plugin(String),
    /// The plugin could not open the file.
    Open(String),
    /// The program is not run in a terminal (standard input or output is a pipe or a file).
    NoTerminal,
    /// The terminal failed.
    Terminal(String),
}

impl AppError {
    /// The exit code of the program: 2 when the file argument is refused (it does not exist, is
    /// not a file, cannot be read, or no plugin takes it: the same as the refusal of an unknown
    /// option), 1 when the run fails (a plugin, the open, the terminal).
    pub fn exit_code(&self) -> u8 {
        match self {
            AppError::NoFile(_)
            | AppError::NotAFile(_)
            | AppError::Unreadable(..)
            | AppError::NotHandled { .. } => 2,
            AppError::Plugin(_)
            | AppError::Open(_)
            | AppError::NoTerminal
            | AppError::Terminal(_) => 1,
        }
    }
}

/// `s` without control characters (it reaches a terminal).
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::NoFile(p) => write!(f, "{}: no such file", clean(&p.to_string_lossy())),
            AppError::NotAFile(p) => write!(f, "{}: not a file", clean(&p.to_string_lossy())),
            AppError::Unreadable(p, e) => {
                write!(f, "{}: cannot be read: {e}", clean(&p.to_string_lossy()))
            }
            AppError::NotHandled { path, why } => write!(
                f,
                "{}: this program does not show this file ({why})",
                clean(&path.to_string_lossy())
            ),
            AppError::Plugin(s) => write!(f, "{}", clean(s)),
            AppError::Open(s) => write!(f, "cannot open the file: {}", clean(s)),
            AppError::NoTerminal => write!(f, "standard input and output must be a terminal"),
            AppError::Terminal(s) => write!(f, "terminal: {}", clean(s)),
        }
    }
}

impl std::error::Error for AppError {}

/// The locale of the environment (`LC_ALL`, `LC_MESSAGES`, `LANG`; empty when none is set).
fn os_locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

/// `"ja"` when the locale names Japanese, `"en"` otherwise.
pub fn lang_of(locale: &str) -> &'static str {
    if locale.starts_with("ja") {
        "ja"
    } else {
        "en"
    }
}

/// The colors a terminal shows, from `$COLORTERM` and `$TERM`.
pub fn colors_of(colorterm: &str, term: &str) -> ColorDepth {
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        ColorDepth::TrueColor
    } else if term.contains("256color") {
        ColorDepth::Ansi256
    } else {
        ColorDepth::Ansi16
    }
}

/// Whether the start of a file reads as text: no NUL byte in its first 8 KiB.
fn reads_as_text(path: &Path) -> bool {
    let mut buf = Vec::new();
    match std::fs::File::open(path) {
        Ok(f) => {
            let _ = f.take(8192).read_to_end(&mut buf);
            !buf.contains(&0)
        }
        Err(_) => false,
    }
}

/// What opening a file settled on: the session, and the name for the title line.
pub struct Opened {
    pub session: Box<dyn Session>,
    pub title: String,
    pub plugin_id: String,
    /// The absolute path of the file (where a picture's relative references resolve from).
    pub path: PathBuf,
}

/// Check the file, pick the plugin by the registry's rules and open a session. Nothing here
/// touches the terminal, so every refusal is an ordinary message.
pub fn open_file(
    plugins: Vec<Arc<dyn Plugin>>,
    path: &Path,
    lang: &str,
    term: TermCaps,
) -> Result<Opened, AppError> {
    let path =
        std::path::absolute(path).map_err(|e| AppError::Unreadable(path.to_path_buf(), e))?;
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(AppError::NoFile(path)),
        Err(e) => return Err(AppError::Unreadable(path, e)),
    };
    if !meta.is_file() {
        return Err(AppError::NotAFile(path));
    }
    if let Err(e) = std::fs::File::open(&path) {
        return Err(AppError::Unreadable(path, e));
    }

    let mut registry = Registry::new();
    for p in plugins {
        registry
            .register(p, PluginOrigin::Standard)
            .map_err(|e| AppError::Plugin(e.to_string()))?;
    }
    let sniff = || None;
    let is_text = || reads_as_text(&path);
    let facts = FileFacts {
        path: &path,
        sniff_mime: &sniff,
        is_text: &is_text,
    };
    let (id, hint) = match registry.resolve_file(&[], &facts) {
        Resolution::Plugin { id, hint } => (id, hint),
        Resolution::Text => {
            return Err(AppError::NotHandled {
                path,
                why: "no plugin of this program takes it; it reads as text".to_string(),
            })
        }
        Resolution::CanNotPreview { ext } => {
            return Err(AppError::NotHandled {
                path,
                why: if ext.is_empty() {
                    "no plugin of this program takes it".to_string()
                } else {
                    format!("no plugin of this program takes `.{}`", clean(&ext))
                },
            })
        }
    };
    let plugin = registry
        .plugin(&id)
        .ok_or_else(|| AppError::Plugin(format!("plugin `{id}` is not registered")))?;
    let cx = OpenCx::new(Settings::empty(), lang).with_term(term);
    let session = plugin
        .open(
            Input::File {
                path: path.clone(),
                hint,
            },
            &cx,
        )
        .map_err(|f: Failure| AppError::Open(f.message.get(lang).to_string()))?;
    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Opened {
        session,
        title,
        plugin_id: id,
        path,
    })
}

/// `<program> <file>`: open the file in the terminal. Returns when the user quits.
pub fn run(plugins: Vec<Arc<dyn Plugin>>, path: &Path) -> Result<(), AppError> {
    use std::io::IsTerminal;
    let colors = colors_of(
        &std::env::var("COLORTERM").unwrap_or_default(),
        &std::env::var("TERM").unwrap_or_default(),
    );
    let term = TermCaps::new().colors(colors);
    // The config comes first: `ui.lang` says the language of the session too. A missing or broken
    // config file gives the defaults; a broken one is reported on standard error once the
    // terminal is back (the screen is the alternate one meanwhile).
    let loaded = konoma_host_core::shared::SharedConfig::load_default();
    let svg_max_px = loaded.0.ui.svg_max_px;
    let (keys, load_error) = keys_from_load(loaded);
    let lang = match keys.lang() {
        konoma_host_core::lang::Lang::Jp => "ja",
        konoma_host_core::lang::Lang::En => "en",
    };
    // The file is checked and opened before the terminal is taken, so a refusal is a message on
    // the ordinary screen.
    let mut opened = open_file(plugins, path, lang, term)?;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(AppError::NoTerminal);
    }
    let cx = AppCx {
        lang: lang.to_string(),
        term,
        title: opened.title.clone(),
    };
    let mut guard = term::TerminalGuard::enter()?;
    // The terminal is asked what it can show once, on the alternate screen and before events are
    // read (konoma does the same); what was typed meanwhile is replayed, and a late reply is
    // dropped from the keys. Without an answer there are no pictures: the cells stay.
    let konoma_widgets::term_query::Startup {
        picker,
        typed,
        late_reply,
    } = konoma_widgets::term_query::picker_from_terminal();
    let window_size = crossterm::terminal::window_size()
        .ok()
        .map(|w| (w.columns, w.rows, w.width, w.height));
    let pictures = picker
        .map(|p| konoma_widgets::term_query::fix_picker_font_size(p, window_size))
        .map(|p| Pictures::new(p, &opened.plugin_id, &opened.path, svg_max_px));
    let result = run_with_pictures(
        guard.terminal(),
        &mut *opened.session,
        &mut term::CrosstermEvents::after_query(&typed, late_reply),
        &cx,
        &keys,
        pictures,
    );
    guard.leave();
    // A failure to write to a closed standard error changes nothing about the run.
    let _ = write_load_error(&mut std::io::stderr(), load_error.as_deref());
    result
}

/// The keys of a loaded config, and the load error (if any) passed on for [`write_load_error`].
pub fn keys_from_load(
    (shared, error): (konoma_host_core::shared::SharedConfig, Option<String>),
) -> (AppKeys, Option<String>) {
    let lang = konoma_host_core::lang::Lang::resolve_with(&shared.ui.lang, || {
        konoma_host_core::lang::Lang::parse(lang_of(&os_locale()))
    });
    (
        AppKeys::new(&shared.keys, &shared.ui.keys).with_lang(lang),
        error,
    )
}

/// Write the config load error as one line (nothing when there is none). The message is the one
/// the shared loader made with `format_load_error`, the wording konoma shows too.
pub fn write_load_error(w: &mut dyn std::io::Write, error: Option<&str>) -> std::io::Result<()> {
    match error {
        Some(e) => writeln!(w, "{}", clean(e)),
        None => Ok(()),
    }
}
