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
    /// Names of the plugin's own screens (the contexts of its keys: `<id>.<screen>`).
    pub screens: Vec<String>,
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
        }
    }

    pub fn name(mut self, name: Tr) -> Manifest {
        self.name = name;
        self
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

    pub fn screen(mut self, name: impl Into<String>) -> Manifest {
        self.screens.push(name.into());
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
    /// The context of the key: a host context (`text`, `table`, ...) or one of the plugin's own
    /// screens. `None` means the plugin's main view.
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

    pub fn label(mut self, label: Tr) -> Act {
        self.label = label;
        self
    }

    pub fn help(mut self, help: Tr) -> Act {
        self.help = help;
        self
    }

    pub fn footer(mut self, priority: u8) -> Act {
        self.footer = Some(priority);
        self
    }

    pub fn screen(mut self, screen: impl Into<String>) -> Act {
        self.screen = Some(screen.into());
        self
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

    pub fn label(mut self, label: Tr) -> VerbDecl {
        self.label = Some(label);
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
    pub fn new(name: impl Into<String>, default: Value, help: Tr) -> SettingDecl {
        SettingDecl {
            name: name.into(),
            default,
            help,
        }
    }
}
