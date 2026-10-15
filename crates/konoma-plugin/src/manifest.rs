//! The declarations of a plugin: a (display bindings), b (launch verbs), c (actions and help).

use crate::settings::Value;
use crate::text::Tr;

/// The version of this API. A plugin declares the one it was built against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl ApiVersion {
    /// The API version of this crate.
    pub const CURRENT: ApiVersion = ApiVersion { major: 0, minor: 1 };

    /// Whether a host that speaks `host` can run a plugin built against `self`: the same major
    /// version, and a minor version the host already has (a newer minor may use declarations the
    /// host does not know). While the major version is 0 every minor step may break, so the
    /// minor must match exactly.
    pub fn works_with(self, host: ApiVersion) -> bool {
        if self.major != host.major {
            return false;
        }
        if self.major == 0 {
            self.minor == host.minor
        } else {
            self.minor <= host.minor
        }
    }
}

impl std::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// What kind of thing a plugin's main view shows. It decides which keys the host keeps for itself
/// in that view (the keys of the display type: see [`crate::keys::reserved`]), so a plugin that
/// shows only text is not refused the keys a table would need.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewKind {
    /// Windowed text (source, hex dumps, logs).
    Text,
    /// A decorated document.
    Document,
    Table,
    Image,
    Diff,
    /// A list of rows.
    List,
    /// Cells and pictures the plugin draws itself (see [`crate::view::CanvasView`]). The host
    /// keeps only a few keys for itself in this context.
    Canvas,
}

impl ViewKind {
    /// The name of the key context of this kind (`text`, `document`, `table`, `image`, `diff`,
    /// `list`, `canvas`).
    pub fn context(self) -> &'static str {
        match self {
            ViewKind::Text => "text",
            ViewKind::Document => "document",
            ViewKind::Table => "table",
            ViewKind::Image => "image",
            ViewKind::Diff => "diff",
            ViewKind::List => "list",
            ViewKind::Canvas => "canvas",
        }
    }
}

/// What the host shows in place of the content when the plugin shows nothing for a file: it
/// failed, the terminal cannot draw what it returned, or nothing has come back. A plugin picks one
/// in its manifest ([`Manifest::unavailable`]); the host does the drawing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Unavailable {
    /// The failure's message (a general "can not preview" without one), then the file name.
    #[default]
    Message,
    /// The file as plain text: a table or a diagram that cannot be drawn is still text.
    /// `with_reason` puts the reason the picture failed (a vector file that is too heavy, say)
    /// above the text.
    RawText { with_reason: bool },
    /// Always this sentence, then the file name, whatever the failure said.
    Notice(Tr),
    /// The host's own frame for a picture that is missing: the reason in the frame when there is
    /// one, otherwise the general "cannot display the image" line.
    Picture,
}

/// Everything a plugin declares. The host reads it once at startup; the implementation is not
/// called until a file is opened.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    pub api: ApiVersion,
    pub name: Tr,
    pub binds: Vec<Bind>,
    pub actions: Vec<Act>,
    pub verbs: Vec<VerbDecl>,
    pub settings: Vec<SettingDecl>,
    /// Names of the plugin's own screens (each is a key **context** of its own: `<id>.<screen>`).
    pub screens: Vec<String>,
    /// What the plugin's main view shows (see [`Manifest::shows`]). Empty means "anything" for
    /// the check of the keys (an action that names no context is checked against every display
    /// type) but **no screen is attached**: a plugin that is given files with `binds` and shows
    /// nothing is reported at startup (`NoteKind::ShowsNotDeclared`, `testing::check_manifest`).
    pub shows: Vec<ViewKind>,
    /// The exclusive work group the plugin's jobs belong to, if any. See [`Manifest::work_group`].
    pub work_group: Option<String>,
    /// Whether the plugin shows a converted form of the file. See [`Manifest::converts`].
    pub converts: bool,
    /// Whether a text view is the file itself. See [`Manifest::file_text`].
    pub file_text: bool,
    /// What the host shows when the plugin shows nothing. See [`Unavailable`].
    pub unavailable: Unavailable,
}

impl Manifest {
    /// A manifest with no declarations. The name defaults to the id.
    pub fn new(id: impl Into<String>, version: impl Into<String>) -> Manifest {
        let id = id.into();
        Manifest {
            name: Tr::new(id.clone()),
            id,
            version: version.into(),
            api: ApiVersion::CURRENT,
            binds: Vec::new(),
            actions: Vec::new(),
            verbs: Vec::new(),
            settings: Vec::new(),
            screens: Vec::new(),
            shows: Vec::new(),
            work_group: None,
            converts: false,
            file_text: false,
            unavailable: Unavailable::Message,
        }
    }

    pub fn name(mut self, name: impl Into<Tr>) -> Manifest {
        self.name = name.into();
        self
    }

    /// Declare that the main view shows this kind of thing (call it once per kind). The host
    /// chooses the plugin's screen (the text window, the table, the picture pipeline, the
    /// document widget) from these declarations, so a plugin that is given files by
    /// [`Manifest::bind`] **must** declare at least one: with none the host attaches no screen,
    /// and says so at startup. The keys of the plugin's actions are checked against the keys
    /// reserved for the declared kinds only (when none is declared, against every kind). A view
    /// of a kind that was not declared is not drawn: the host shows a message naming it.
    pub fn shows(mut self, kind: ViewKind) -> Manifest {
        if !self.shows.contains(&kind) {
            self.shows.push(kind);
        }
        self
    }

    /// Declare that the plugin shows a **converted** form of the file (a workbook as a table, a Word
    /// document as Markdown): there is no file with the lines it shows, so what it shows is never
    /// written back, its source view is a generated text, and the host names its pages after what
    /// they are (sheets, slides). The host decides which screen a plugin gets from
    /// [`Manifest::shows`] and this, never from its id.
    pub fn converts(mut self) -> Manifest {
        self.converts = true;
        self
    }

    /// Declare that the text view of the plugin is the file itself (a [`TextSource::File`] of the
    /// file it was opened on): the host starts reading the file in windows at once, before the
    /// plugin has answered, and colors it with the plugin's styler when that arrives. Without this
    /// the host waits ("Loading...") for the plugin's text, which may be made from the file (a hex
    /// dump).
    ///
    /// [`TextSource::File`]: crate::view::TextSource::File
    pub fn file_text(mut self) -> Manifest {
        self.file_text = true;
        self
    }

    /// What the host shows when the plugin shows nothing for a file. The default is the failure's
    /// message and the file name.
    pub fn unavailable(mut self, how: Unavailable) -> Manifest {
        self.unavailable = how;
        self
    }

    /// Whether the plugin is given files (a name, a glob or a content type sniffed from the file)
    /// but declares no kind of view: the host would attach no screen to them.
    pub fn assigns_files_without_shows(&self) -> bool {
        self.shows.is_empty()
            && self.binds.iter().any(|b| {
                b.targets
                    .iter()
                    .any(|t| matches!(t, Target::Glob(_) | Target::FileName(_) | Target::Mime(_)))
            })
    }

    pub fn bind(mut self, bind: Bind) -> Manifest {
        self.binds.push(bind);
        self
    }

    pub fn action(mut self, act: Act) -> Manifest {
        self.actions.push(act);
        self
    }

    pub fn verb(mut self, verb: VerbDecl) -> Manifest {
        self.verbs.push(verb);
        self
    }

    pub fn setting(mut self, setting: SettingDecl) -> Manifest {
        self.settings.push(setting);
        self
    }

    /// Declare a screen of the plugin's own: a key context named `<id>.<name>`.
    pub fn screen(mut self, name: impl Into<String>) -> Manifest {
        self.screens.push(name.into());
        self
    }

    /// Put the plugin in an **exclusive work group** (for plugins whose `open` and `view` can
    /// hold gigabytes: spreadsheets, Word documents, slide decks).
    ///
    /// The host runs the jobs of all plugins that name the same group **one at a time**. A job
    /// asked for while another of the group runs waits, and only the **newest** waiting job is
    /// kept (a newer one replaces it), so holding a page key cannot pile up loads. The running job
    /// is told to stop the usual way: its [`Cancel`](crate::Cancel) turns true as soon as a newer
    /// job is asked for, so a plugin should check it often (at least once per row or part it
    /// reads).
    pub fn work_group(mut self, group: impl Into<String>) -> Manifest {
        self.work_group = Some(group.into());
        self
    }
}

/// What a binding matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A glob on the **file name only**, case-insensitive (`*.{png,jpg}` is allowed).
    Glob(String),
    /// A file name, as is (`Makefile`), case-insensitive.
    FileName(String),
    /// A MIME type sniffed from the content: `image/*` matches by prefix, otherwise exact.
    Mime(String),
    /// A content type handed over by another plugin (`markdown`).
    Content(String),
    /// An embedded thing: `fence:mermaid`, `math`.
    Embed(String),
}

/// Whether a binding takes part in the decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// Used whenever it matches.
    Default,
    /// Used only when chosen explicitly (`R` or a setting).
    Option,
}

/// One display binding: some targets with a priority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bind {
    pub targets: Vec<Target>,
    pub priority: Priority,
}

impl Bind {
    fn of(targets: Vec<Target>) -> Bind {
        Bind {
            targets,
            priority: Priority::Default,
        }
    }

    /// File-name globs.
    pub fn files<I, S>(globs: I) -> Bind
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Bind::of(globs.into_iter().map(|g| Target::Glob(g.into())).collect())
    }

    /// Exact file names.
    pub fn file_names<I, S>(names: I) -> Bind
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Bind::of(
            names
                .into_iter()
                .map(|n| Target::FileName(n.into()))
                .collect(),
        )
    }

    /// A MIME type or prefix such as `image/*`.
    pub fn mime(pattern: impl Into<String>) -> Bind {
        Bind::of(vec![Target::Mime(pattern.into())])
    }

    /// A content type.
    pub fn content(ty: impl Into<String>) -> Bind {
        Bind::of(vec![Target::Content(ty.into())])
    }

    /// An embedded kind.
    pub fn embed(kind: impl Into<String>) -> Bind {
        Bind::of(vec![Target::Embed(kind.into())])
    }

    /// Make the binding an option (not part of the decision).
    pub fn option(mut self) -> Bind {
        self.priority = Priority::Option;
        self
    }
}

/// An action the plugin handles, with its key, help and footer entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Act {
    pub id: String,
    /// The default key (see [`crate::keys::parse_key`]). `None` means menu-only.
    pub key: Option<String>,
    /// Short label for the footer.
    pub label: Tr,
    /// Description for the help screen.
    pub help: Tr,
    /// Footer priority (smaller shows first). `None` keeps it out of the footer.
    pub footer: Option<u8>,
    /// The **context** of the key: a host context (`text`, `table`, ...) or one of the plugin's own
    /// screens. `None` means the plugin's main view (the kinds in [`Manifest::shows`]). Set it
    /// with [`Act::context`]; [`Act::screen`] is the older name of the same thing.
    pub screen: Option<String>,
    /// Whether the action writes files.
    pub writes: bool,
    /// Whether the action destroys data (needs a confirmation declaration, which this API version
    /// does not have yet).
    pub destructive: bool,
}

impl Act {
    pub fn new(id: impl Into<String>) -> Act {
        let id = id.into();
        Act {
            label: Tr::new(id.clone()),
            help: Tr::new(""),
            id,
            key: None,
            footer: None,
            screen: None,
            writes: false,
            destructive: false,
        }
    }

    pub fn key(mut self, key: impl Into<String>) -> Act {
        self.key = Some(key.into());
        self
    }

    /// The short label for the footer. A `&str` is the English text only; use `t!` for several
    /// languages.
    pub fn label(mut self, label: impl Into<Tr>) -> Act {
        self.label = label.into();
        self
    }

    /// The description for the help screen (same forms as [`Act::label`]).
    pub fn help(mut self, help: impl Into<Tr>) -> Act {
        self.help = help.into();
        self
    }

    pub fn footer(mut self, priority: u8) -> Act {
        self.footer = Some(priority);
        self
    }

    /// The context the key lives in: a host context name (`text`, `table`, `image`, `document`)
    /// or a screen of the plugin.
    pub fn context(mut self, context: impl Into<String>) -> Act {
        self.screen = Some(context.into());
        self
    }

    /// The older name of [`Act::context`].
    pub fn screen(self, screen: impl Into<String>) -> Act {
        self.context(screen)
    }

    pub fn writes(mut self) -> Act {
        self.writes = true;
        self
    }

    pub fn destructive(mut self) -> Act {
        self.destructive = true;
        self
    }
}

/// A launch verb: `e` edits, `o` opens in another app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Edit,
    Open,
}

/// One way to launch something; the chain is tried in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launcher {
    /// The user's editor.
    Editor,
    /// A macOS app by bundle id.
    MacApp { bundle_id: String },
    /// A command line (the first element is the program).
    Command(Vec<String>),
    /// The OS default application.
    OsDefault,
}

/// How a launcher runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    /// Hand the terminal over (vim and other TUIs).
    Terminal,
    /// Start a GUI app from a worker without blocking the UI.
    Gui,
}

/// The declaration of one verb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerbDecl {
    pub verb: Verb,
    pub chain: Vec<Launcher>,
    pub mode: LaunchMode,
    /// Whether the line position token `{line}` is used.
    pub line_token: bool,
    /// The footer label, when it differs from the host's.
    pub label: Option<Tr>,
}

impl VerbDecl {
    /// A verb with an empty chain, terminal mode, no line token.
    pub fn new(verb: Verb) -> VerbDecl {
        VerbDecl {
            verb,
            chain: Vec::new(),
            mode: LaunchMode::Terminal,
            line_token: false,
            label: None,
        }
    }

    pub fn launcher(mut self, l: Launcher) -> VerbDecl {
        self.chain.push(l);
        self
    }

    pub fn mode(mut self, mode: LaunchMode) -> VerbDecl {
        self.mode = mode;
        self
    }

    pub fn line_token(mut self) -> VerbDecl {
        self.line_token = true;
        self
    }

    pub fn label(mut self, label: impl Into<Tr>) -> VerbDecl {
        self.label = Some(label.into());
        self
    }
}

/// A setting the plugin reads from `[plugins.<id>]`.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingDecl {
    pub name: String,
    pub default: Value,
    pub help: Tr,
}

impl SettingDecl {
    pub fn new(name: impl Into<String>, default: Value, help: impl Into<Tr>) -> SettingDecl {
        SettingDecl {
            name: name.into(),
            default,
            help: help.into(),
        }
    }
}
