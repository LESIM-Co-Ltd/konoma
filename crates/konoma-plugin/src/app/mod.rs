//! The standalone app: a plugin program that is given a file shows it in a terminal of its own.
//!
//! This is a small host. It picks the plugin with the registry's rules (the same
//! [`Registry::resolve_file`] the konoma host uses), opens a session, and draws the view with
//! ratatui. It draws [`View::Message`](crate::View), [`View::Text`](crate::View) (a file read in
//! windows, a string or styled lines), [`View::Canvas`](crate::View) and
//! [`View::Table`](crate::View) (the grid drawn with the shared `konoma-widgets` table, the cell
//! cursor, the sheets as pages, the full-cell popup) and [`View::Image`](crate::View) (a still
//! picture decoded off the loop and drawn through the `Pictures`, with zoom and pan; with no
//! `Pictures` the line that names the terminal) and [`View::Document`](crate::View) (Markdown, laid
//! out by the shared `document_render` and drawn in a bordered frame; every picture of it is shown
//! as text). Any other view says it cannot be shown yet.
//!
//! The loop ([`run_with`]) takes its terminal and its events from outside, so it runs on a
//! ratatui `TestBackend` in tests; [`run`] is the thin entry that owns the real terminal.

mod copy;
mod display;
mod doc_pictures;
mod document;
mod draw;
mod looping;
mod pictures;
mod still;
mod term;
mod window;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::registry::{FileFacts, PluginOrigin, Registry, Resolution};
use crate::reopen::ContentTarget;
use crate::session::{ColorDepth, Failure, OpenCx, Plugin, Session, TermCaps};
use crate::{Input, Settings};

pub use display::Display;
pub use doc_pictures::DocEnv;
pub use looping::{
    run_with, run_with_pictures, AppCx, AppEvent, AppKeys, EventSource, ImageMode, Next, TableMode,
};
pub use pictures::{delete_placed_pictures, InProcessSvg, Pictures};
pub use still::{failure_line, StillShape};
pub use term::{install_panic_hook_with, key_from_crossterm};

/// Finds the plugin that takes a content type (what a [`View::Reopen`](crate::View) names), with
/// the settings it is opened with. **The one place the app looks a content type up**: it is built
/// from the registry of the program ([`ReopenSource::from_registry`]).
#[derive(Clone)]
pub struct ReopenSource(Arc<ContentLookup>);

type ContentLookup = dyn Fn(&str) -> Option<ContentTarget>;

impl ReopenSource {
    /// A source from any lookup.
    pub fn new(lookup: impl Fn(&str) -> Option<ContentTarget> + 'static) -> ReopenSource {
        ReopenSource(Arc::new(lookup))
    }

    /// The plugin `registry` names for a content type ([`Registry::resolve_content`]: the rule
    /// konoma follows), with the settings `settings_of` gives for it.
    pub fn from_registry(
        registry: Arc<Registry>,
        settings_of: Box<dyn Fn(&str) -> Settings>,
    ) -> ReopenSource {
        ReopenSource::new(move |content_type| {
            let id = registry.resolve_content(content_type)?;
            let plugin = registry.plugin(&id)?;
            Some(ContentTarget {
                settings: settings_of(&id),
                plugin,
            })
        })
    }

    /// The plugin that takes `content_type`, if any.
    pub fn target(&self, content_type: &str) -> Option<ContentTarget> {
        (self.0)(content_type)
    }
}

impl fmt::Debug for ReopenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReopenSource")
    }
}

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

/// A registry holding `plugins` as the standard ones of the program and `bundled` as the plugins
/// it carries only as destinations ([`PluginOrigin::Bundled`]: they are looked up for a reopened
/// content type and an embedded kind, and never take a file).
fn register_all(
    plugins: Vec<Arc<dyn Plugin>>,
    bundled: Vec<Arc<dyn Plugin>>,
) -> Result<Registry, AppError> {
    let mut registry = Registry::new();
    for (list, origin) in [
        (plugins, PluginOrigin::Standard),
        (bundled, PluginOrigin::Bundled),
    ] {
        for p in list {
            registry
                .register(p, origin)
                .map_err(|e| AppError::Plugin(e.to_string()))?;
        }
    }
    Ok(registry)
}

/// The plugin `registry` picks for the file at `path`, and the hint of the binding. The facts are
/// read the way konoma reads them: the MIME type from the content
/// ([`konoma_host_core::sniff`]) and the text judgment of `konoma-widgets`.
fn pick_in(registry: &Registry, path: &Path) -> Result<(String, Option<String>), AppError> {
    let sniff = || konoma_host_core::sniff::mime_of_path(path);
    let is_text = || konoma_widgets::text::is_probably_text(path);
    let facts = FileFacts {
        path,
        sniff_mime: &sniff,
        is_text: &is_text,
    };
    match registry.resolve_file(&[], &facts) {
        Resolution::Plugin { id, hint } => Ok((id, hint)),
        Resolution::Text => Err(AppError::NotHandled {
            path: path.to_path_buf(),
            why: "no plugin of this program takes it; it reads as text".to_string(),
        }),
        Resolution::CanNotPreview { ext } => Err(AppError::NotHandled {
            path: path.to_path_buf(),
            why: if ext.is_empty() {
                "no plugin of this program takes it".to_string()
            } else {
                format!("no plugin of this program takes `.{}`", clean(&ext))
            },
        }),
    }
}

/// The id of the plugin among `plugins` that shows the file at `path`, or the refusal
/// [`open_file`] gives. Nothing is opened.
pub fn pick_plugin(plugins: Vec<Arc<dyn Plugin>>, path: &Path) -> Result<String, AppError> {
    let path =
        std::path::absolute(path).map_err(|e| AppError::Unreadable(path.to_path_buf(), e))?;
    let registry = register_all(plugins, Vec::new())?;
    pick_in(&registry, &path).map(|(id, _)| id)
}

/// What opening a file settled on: the session, and the name for the title line.
pub struct Opened {
    pub session: Box<dyn Session>,
    pub title: String,
    pub plugin_id: String,
    /// The absolute path of the file (where a picture's relative references resolve from).
    pub path: PathBuf,
    /// The note of the binding that picked the plugin (a CSV opened as `tsv`, say): it reaches the
    /// copy menu as it reached the plugin.
    pub hint: Option<String>,
    /// What the plugin declared it shows when it shows nothing ([`Manifest::unavailable`]).
    pub unavailable: crate::manifest::Unavailable,
    /// The display types the plugin declared ([`Manifest::shows`]).
    pub shows: Vec<crate::manifest::ViewKind>,
    /// The plugins of the program (who draws the mermaid and math a document embeds).
    pub registry: Arc<Registry>,
}

/// Check the file, pick the plugin by the registry's rules and open a session. Nothing here
/// touches the terminal, so every refusal is an ordinary message.
pub fn open_file(
    plugins: Vec<Arc<dyn Plugin>>,
    path: &Path,
    lang: &str,
    term: TermCaps,
) -> Result<Opened, AppError> {
    open_file_with(plugins, path, lang, term, &|_| Settings::new())
}

/// [`open_file`] with the settings of the chosen plugin: `settings_of(id)` is asked once, for the
/// plugin the registry picked, and is what its `OpenCx::settings` holds.
pub fn open_file_with(
    plugins: Vec<Arc<dyn Plugin>>,
    path: &Path,
    lang: &str,
    term: TermCaps,
    settings_of: &dyn Fn(&str) -> Settings,
) -> Result<Opened, AppError> {
    open_file_bundled(plugins, Vec::new(), path, lang, term, settings_of)
}

/// [`open_file_with`] for a program that also carries `bundled` plugins: destinations of a
/// reopened content type and of an embedded kind, registered in the same registry but never
/// picked for the file ([`PluginOrigin::Bundled`]).
pub fn open_file_bundled(
    plugins: Vec<Arc<dyn Plugin>>,
    bundled: Vec<Arc<dyn Plugin>>,
    path: &Path,
    lang: &str,
    term: TermCaps,
    settings_of: &dyn Fn(&str) -> Settings,
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

    let registry = register_all(plugins, bundled)?;
    let (id, hint) = pick_in(&registry, &path)?;
    let plugin = registry
        .plugin(&id)
        .ok_or_else(|| AppError::Plugin(format!("plugin `{id}` is not registered")))?;
    let unavailable = registry
        .manifest(&id)
        .map_or_else(|| plugin.manifest().unavailable, |m| m.unavailable.clone());
    let shows = registry
        .manifest(&id)
        .map_or_else(|| plugin.manifest().shows, |m| m.shows.clone());
    let settings = settings_of(&id);
    let cx = OpenCx::new(&settings, lang).with_term(term);
    let session = plugin
        .open(
            Input::File {
                path: path.clone(),
                hint: hint.clone(),
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
        hint,
        unavailable,
        shows,
        registry: Arc::new(registry),
    })
}

/// Everything `run` settles before the terminal is taken.
pub struct Prepared {
    pub opened: Opened,
    pub keys: AppKeys,
    /// The language of the session: `"ja"` or `"en"`.
    pub lang: &'static str,
    pub svg_max_px: u32,
    /// What a document's pictures need (who draws mermaid and math, `external.remote_images`).
    pub doc_env: doc_pictures::DocEnv,
    /// Where a `Reopen` is opened again (put it in [`AppCx::reopen`]).
    pub reopen: ReopenSource,
    /// How the text and the documents look (put it in [`AppCx::display`]).
    pub display: Display,
    /// The config load error, still to be reported ([`write_load_error`]).
    pub load_error: Option<String>,
}

/// A refusal before the terminal is taken, with the config load error that is still unreported.
pub struct PrepareFailure {
    pub error: AppError,
    pub load_error: Option<String>,
}

impl PrepareFailure {
    /// Write the config load error (one line, before the refusal itself, which the caller prints)
    /// and hand the refusal on.
    pub fn report(self, w: &mut dyn std::io::Write) -> AppError {
        // A failure to write to a closed standard error changes nothing about the run.
        let _ = write_load_error(w, self.load_error.as_deref());
        self.error
    }
}

/// The part of `run` before the terminal: from a loaded config, the language, the keys, and the
/// file opened with the settings of the plugin that takes it (`[plugins.<id>]` and the old `[ui]`
/// names, [`crate::config::plugin_settings`]). The file is checked and opened here, so a refusal
/// is a message on the ordinary screen.
pub fn prepare(
    plugins: Vec<Arc<dyn Plugin>>,
    path: &Path,
    term: TermCaps,
    loaded: (konoma_host_core::shared::SharedConfig, Option<String>),
) -> Result<Prepared, PrepareFailure> {
    prepare_bundled(plugins, Vec::new(), path, term, loaded)
}

/// [`prepare`] for a program that also carries `bundled` plugins ([`open_file_bundled`]). Their
/// settings (`[plugins.<id>]`) reach them by the same rule as the program's own.
pub fn prepare_bundled(
    plugins: Vec<Arc<dyn Plugin>>,
    bundled: Vec<Arc<dyn Plugin>>,
    path: &Path,
    term: TermCaps,
    loaded: (konoma_host_core::shared::SharedConfig, Option<String>),
) -> Result<Prepared, PrepareFailure> {
    let svg_max_px = loaded.0.ui.svg_max_px;
    let remote_images = loaded.0.external.remote_images;
    let display = Display::of(&loaded.0);
    let (plugin_tables, moved_ui) = (loaded.0.plugins.clone(), loaded.0.ui.moved());
    let (keys, load_error) = keys_from_load(loaded);
    let lang = match keys.lang() {
        konoma_host_core::lang::Lang::Jp => "ja",
        konoma_host_core::lang::Lang::En => "en",
    };
    let (tables, moved) = (plugin_tables.clone(), moved_ui.clone());
    let (tables2, moved2) = (plugin_tables.clone(), moved_ui.clone());
    let settings_of = |id: &str| crate::config::plugin_settings(&plugin_tables, &moved_ui, id);
    match open_file_bundled(plugins, bundled, path, lang, term, &settings_of) {
        Ok(opened) => Ok(Prepared {
            doc_env: doc_pictures::DocEnv::from_registry(
                Arc::clone(&opened.registry),
                lang,
                Box::new(move |id| crate::config::plugin_settings(&tables, &moved, id)),
                remote_images,
            ),
            reopen: ReopenSource::from_registry(
                Arc::clone(&opened.registry),
                Box::new(move |id| crate::config::plugin_settings(&tables2, &moved2, id)),
            ),
            opened,
            keys,
            lang,
            svg_max_px,
            display,
            load_error,
        }),
        Err(error) => Err(PrepareFailure { error, load_error }),
    }
}

/// `<program> <file>`: open the file in the terminal. Returns when the user quits.
pub fn run(plugins: Vec<Arc<dyn Plugin>>, path: &Path) -> Result<(), AppError> {
    run_bundled(plugins, Vec::new(), path)
}

/// [`run`] for a program that also carries `bundled` plugins: destinations of a reopened content
/// type and of an embedded kind. The file is still picked among `plugins` only.
pub fn run_bundled(
    plugins: Vec<Arc<dyn Plugin>>,
    bundled: Vec<Arc<dyn Plugin>>,
    path: &Path,
) -> Result<(), AppError> {
    use std::io::IsTerminal;
    let colors = colors_of(
        &std::env::var("COLORTERM").unwrap_or_default(),
        &std::env::var("TERM").unwrap_or_default(),
    );
    let term = TermCaps::new().colors(colors);
    // The config comes first: `ui.lang` says the language of the session too. A missing or broken
    // config file gives the defaults; a broken one is reported on standard error once the
    // terminal is back (the screen is the alternate one meanwhile), or at once when the run ends
    // before the terminal is taken.
    let prepared = prepare_bundled(
        plugins,
        bundled,
        path,
        term,
        konoma_host_core::shared::SharedConfig::load_default(),
    );
    let Prepared {
        mut opened,
        keys,
        lang,
        svg_max_px,
        doc_env,
        reopen,
        display,
        load_error,
    } = match prepared {
        Ok(p) => p,
        Err(f) => return Err(f.report(&mut std::io::stderr())),
    };
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(PrepareFailure {
            error: AppError::NoTerminal,
            load_error,
        }
        .report(&mut std::io::stderr()));
    }
    let cx = AppCx {
        lang: lang.to_string(),
        term,
        title: opened.title.clone(),
        unavailable: opened.unavailable.clone(),
        plugin_id: opened.plugin_id.clone(),
        path: opened.path.clone(),
        hint: opened.hint.clone(),
        shows: opened.shows.clone(),
        reopen: Some(reopen),
        display,
        open_dir: std::env::current_dir().ok(),
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
        .map(|p| {
            Pictures::new(p, &opened.plugin_id, &opened.path, svg_max_px).with_documents(doc_env)
        });
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
        AppKeys::new(&shared.keys, &shared.ui.keys)
            .with_lang(lang)
            .with_wrap(shared.ui.wrap),
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
