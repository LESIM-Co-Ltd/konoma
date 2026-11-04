//! The rules that settle which plugin shows a file.
//!
//! Order for a file: the user's rules, then user-registered plugins' `Default` bindings, then
//! standard plugins' `Default` bindings (registration order within a layer), then plain text if
//! the file reads as text, then "can not preview". `Option` bindings never take part.

use std::cell::OnceCell;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use globset::{GlobBuilder, GlobMatcher};

use crate::keys::{sanitize, KeyIssue};
use crate::manifest::{ApiVersion, Manifest, Priority, Target};
use crate::session::Plugin;
use crate::settings::Value;

mod project_values;

/// Who registered a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginOrigin {
    /// Shipped with konoma.
    Standard,
    /// Added by the user.
    User,
    /// Carried inside a standalone program as the destination of another plugin's output (a
    /// document reopened as Markdown, an embedded diagram). It is looked up by content type and
    /// embedded kind ([`Registry::resolve_content`], [`Registry::resolve_embed`]) but never takes
    /// a file: [`Registry::resolve_file`] skips it, so the program shows only its own plugins'
    /// files.
    Bundled,
}

/// The longest id, action id, setting name, screen name or work group a manifest may declare.
pub const MAX_NAME_CHARS: usize = 64;
/// The most actions a manifest may declare.
pub const MAX_ACTIONS: usize = 256;
/// The most settings a manifest may declare.
pub const MAX_SETTINGS: usize = 256;
/// The most binds a manifest may declare.
pub const MAX_BINDS: usize = 1024;
/// The most screens a manifest may declare.
pub const MAX_SCREENS: usize = 64;
/// The most targets one bind may list.
pub const MAX_TARGETS_PER_BIND: usize = 512;
/// The longest target (a glob, a file name, a MIME type, a content or embed name), in **bytes**.
/// Generous: a brace glob of dozens of extensions fits.
pub const MAX_TARGET_BYTES: usize = 1024;
/// The most values a setting's default may hold in all (a list or a table counts every entry,
/// nested ones included).
pub const MAX_DEFAULT_NODES: usize = 1024;
/// The longest key of a table inside a setting's default, in bytes.
pub const MAX_DEFAULT_KEY_BYTES: usize = 256;
/// The deepest a setting's default may nest (lists and tables inside lists and tables).
pub const MAX_DEFAULT_DEPTH: usize = 8;
/// The longest version string kept (longer is cut).
pub const MAX_VERSION_CHARS: usize = 64;
/// The most paths a project may watch.
pub const MAX_WATCH_PATHS: usize = 512;
/// The longest detection marker, in bytes.
pub const MAX_MARKER_BYTES: usize = 1024;
/// The most choices in one prompt.
pub const MAX_CHOICES: usize = 64;
/// The longest value returned by a choice, in bytes.
pub const MAX_CHOICE_VALUE_BYTES: usize = 1024;
/// The most refresh targets an action may declare.
pub const MAX_REFRESH_TARGETS: usize = 64;

/// Whether `s` is a usable name: 1 to [`MAX_NAME_CHARS`] characters of `[a-z0-9._-]`. A name is
/// printed by the host (`konoma --plugins`, the startup message) and used in config tables and
/// key contexts, so it may not carry a control character, a space or a long text.
pub fn is_valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME_CHARS
        && s.bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

/// `s` as it may be shown in a message about a refusal: what is not allowed in a name becomes `?`
/// and the length is bounded.
fn display_name(s: &str) -> String {
    s.chars()
        .take(MAX_NAME_CHARS)
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// Whether `c` may not appear in a pattern or an identifier: any control character, or the kitty
/// placeholder U+10EEEE.
fn is_unclean(c: char) -> bool {
    c.is_control() || c == '\u{10EEEE}'
}

/// The declaration whose name is invalid. No unsanitized identifier is carried here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManifestName {
    PluginId,
    ActionId,
    ActionScreen,
    SettingName,
    ScreenName,
    WorkGroup,
    RowKind,
}

impl ManifestName {
    fn english(self) -> &'static str {
        match self {
            Self::PluginId => "the plugin id",
            Self::ActionId => "an action id",
            Self::ActionScreen => "an action's screen",
            Self::SettingName => "a setting name",
            Self::ScreenName => "a screen name",
            Self::WorkGroup => "the work group",
            Self::RowKind => "a row kind",
        }
    }
}

/// The declaration count that exceeded its limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManifestCount {
    Actions,
    Settings,
    Binds,
    Screens,
    Targets,
    Markers,
    RowKinds,
    WatchPaths,
    Choices,
    RefreshTargets,
}

impl ManifestCount {
    fn english(self) -> &'static str {
        match self {
            Self::Actions => "actions",
            Self::Settings => "settings",
            Self::Binds => "binds",
            Self::Screens => "screens",
            Self::Targets => "targets in a bind",
            Self::Markers => "project detection markers",
            Self::RowKinds => "row kinds in a screen",
            Self::WatchPaths => "project watch paths",
            Self::Choices => "choices in a prompt",
            Self::RefreshTargets => "refresh targets in an action",
        }
    }
}

/// Structured data for a manifest refusal, so hosts can translate it without parsing prose.
/// Its Display is the existing English `RegistryError::InvalidManifest::why` diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManifestReason {
    UnknownRefreshScreen {
        action: String,
        screen: String,
    },
    InvalidWatchPath {
        index: usize,
    },
    InvalidMarker {
        index: usize,
        limit: usize,
    },
    InvalidScreenKey {
        key: String,
    },
    InvalidConfirmationKey {
        action: String,
        key: String,
    },
    DuplicateConfirmationKey {
        action: String,
        key: String,
    },
    EmptyConfirmation {
        action: String,
    },
    ChoiceValueTooLong {
        action: String,
        limit: usize,
    },
    ChoiceLabelTooLong {
        action: String,
        limit: usize,
    },
    DestructiveWithoutConfirmation {
        action: String,
    },
    DuplicateScreen {
        screen: String,
    },
    DuplicateScreenKey {
        key: String,
    },
    UnknownScreen {
        action: String,
        screen: String,
    },
    UnknownRowKind {
        action: String,
        screen: String,
        kind: String,
    },
    NoDetectionMarkers,
    InvalidName {
        kind: ManifestName,
        name: String,
        limit: usize,
    },
    TooMany {
        kind: ManifestCount,
        count: usize,
        limit: usize,
    },
    TargetTooLong {
        bind: usize,
        bytes: usize,
        limit: usize,
    },
    TargetUnclean {
        bind: usize,
    },
    DefaultTooMany {
        limit: usize,
    },
    DefaultKeyTooLong {
        limit: usize,
    },
    DefaultKeyUnclean {
        limit: usize,
    },
    DefaultTooDeep {
        limit: usize,
    },
}

impl fmt::Display for ManifestReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRefreshScreen { action, screen } => write!(f, "action `{action}` refreshes unknown screen `{screen}`"),
            Self::InvalidWatchPath { index } => write!(f, "project watch path {index} must be nonempty and relative without parent components"),
            Self::InvalidMarker { index, limit } => write!(f, "project detection marker {index} must be one nonempty file name other than . or .. (at most {limit} bytes)"),
            Self::InvalidScreenKey { key } => write!(f, "screen key `{key}` is a control character or reserved by the host"),
            Self::InvalidConfirmationKey { action, key } => write!(f, "confirmation key `{key}` of action `{action}` is a control character or reserved by the host"),
            Self::DuplicateConfirmationKey { action, key } => write!(f, "confirmation key `{key}` of action `{action}` is declared twice"),
            Self::EmptyConfirmation { action } => write!(f, "confirmation of action `{action}` must declare at least one choice"),
            Self::ChoiceValueTooLong { action, limit } => write!(f, "a choice value of action `{action}` exceeds {limit} bytes"),
            Self::ChoiceLabelTooLong { action, limit } => write!(f, "a choice label of action `{action}` exceeds {limit} characters"),
            Self::DestructiveWithoutConfirmation { action } => write!(f, "action `{action}` is destructive but has no confirmation choices"),
            Self::DuplicateScreen { screen } => write!(f, "screen `{screen}` is declared twice"),
            Self::DuplicateScreenKey { key } => write!(f, "screen key `{key}` is declared twice"),
            Self::UnknownScreen { action, screen } => write!(f, "action `{action}` names unknown screen `{screen}`"),
            Self::UnknownRowKind { action, screen, kind } => write!(f, "action `{action}` names unknown row kind `{kind}` in screen `{screen}`"),
            Self::NoDetectionMarkers => write!(f, "a project must declare at least one detection marker"),
            Self::InvalidName { kind, name, limit } => write!(f, "{} `{name}` is not a name ([a-z0-9._-], 1 to {limit} characters)", kind.english()),
            Self::TooMany { kind, count, limit } => write!(f, "{count} {} (at most {limit})", kind.english()),
            Self::TargetTooLong { bind, bytes, limit } => write!(f, "bind {bind} has a target that is too long ({bytes} bytes, at most {limit})"),
            Self::TargetUnclean { bind } => write!(f, "bind {bind} has a target that contains a control character"),
            Self::DefaultTooMany { limit } => write!(f, "a setting default of more than {limit} values"),
            Self::DefaultKeyTooLong { limit } => write!(f, "a setting default has a key that is too long (at most {limit} bytes, no control character)"),
            Self::DefaultKeyUnclean { limit } => write!(f, "a setting default has a key that is unclean (at most {limit} bytes, no control character)"),
            Self::DefaultTooDeep { limit } => write!(f, "a setting default nested deeper than {limit}"),
        }
    }
}

/// Never carry a target's text in a refusal: it may contain terminal control characters.
fn target_problem(bind: usize, t: &Target) -> Option<ManifestReason> {
    let (Target::Glob(s)
    | Target::FileName(s)
    | Target::Mime(s)
    | Target::Content(s)
    | Target::Embed(s)) = t;
    if s.len() > MAX_TARGET_BYTES {
        return Some(ManifestReason::TargetTooLong {
            bind,
            bytes: s.len(),
            limit: MAX_TARGET_BYTES,
        });
    }
    s.chars()
        .any(is_unclean)
        .then_some(ManifestReason::TargetUnclean { bind })
}

fn default_problem(v: &Value) -> Option<ManifestReason> {
    fn walk(v: &Value, depth: usize, nodes: &mut usize) -> Option<ManifestReason> {
        *nodes += 1;
        if *nodes > MAX_DEFAULT_NODES {
            return Some(ManifestReason::DefaultTooMany {
                limit: MAX_DEFAULT_NODES,
            });
        }
        let children: Box<dyn Iterator<Item = &Value>> = match v {
            Value::List(l) => Box::new(l.iter()),
            Value::Table(t) => {
                if let Some(k) = t
                    .keys()
                    .find(|k| k.len() > MAX_DEFAULT_KEY_BYTES || k.chars().any(is_unclean))
                {
                    return Some(if k.len() > MAX_DEFAULT_KEY_BYTES {
                        ManifestReason::DefaultKeyTooLong {
                            limit: MAX_DEFAULT_KEY_BYTES,
                        }
                    } else {
                        ManifestReason::DefaultKeyUnclean {
                            limit: MAX_DEFAULT_KEY_BYTES,
                        }
                    });
                }
                Box::new(t.values())
            }
            _ => return None,
        };
        if depth > MAX_DEFAULT_DEPTH {
            return Some(ManifestReason::DefaultTooDeep {
                limit: MAX_DEFAULT_DEPTH,
            });
        }
        children.into_iter().find_map(|c| walk(c, depth + 1, nodes))
    }
    walk(v, 1, &mut 0)
}

/// The single validation used by the registry and every host/checker that registers a manifest.
fn manifest_problem(m: &Manifest) -> Option<ManifestReason> {
    let name = |kind: ManifestName, s: &str| {
        (!is_valid_name(s)).then(|| ManifestReason::InvalidName {
            kind,
            name: display_name(s),
            limit: MAX_NAME_CHARS,
        })
    };
    let count = |kind, count, limit| {
        (count > limit).then_some(ManifestReason::TooMany { kind, count, limit })
    };
    name(ManifestName::PluginId, &m.id)
        .or_else(|| count(ManifestCount::Actions, m.actions.len(), MAX_ACTIONS))
        .or_else(|| count(ManifestCount::Settings, m.settings.len(), MAX_SETTINGS))
        .or_else(|| count(ManifestCount::Binds, m.binds.len(), MAX_BINDS))
        .or_else(|| count(ManifestCount::Screens, m.screens.len(), MAX_SCREENS))
        .or_else(|| {
            m.binds.iter().enumerate().find_map(|(i, b)| {
                count(
                    ManifestCount::Targets,
                    b.targets.len(),
                    MAX_TARGETS_PER_BIND,
                )
                .or_else(|| b.targets.iter().find_map(|t| target_problem(i, t)))
            })
        })
        .or_else(|| m.settings.iter().find_map(|s| default_problem(&s.default)))
        .or_else(|| {
            m.actions.iter().find_map(|a| {
                name(ManifestName::ActionId, &a.id).or_else(|| {
                    a.screen
                        .as_deref()
                        .and_then(|s| name(ManifestName::ActionScreen, s))
                })
            })
        })
        .or_else(|| {
            m.settings
                .iter()
                .find_map(|s| name(ManifestName::SettingName, &s.name))
        })
        .or_else(|| {
            m.screens
                .iter()
                .find_map(|s| name(ManifestName::ScreenName, s))
        })
        .or_else(|| {
            m.work_group
                .as_deref()
                .and_then(|g| name(ManifestName::WorkGroup, g))
        })
        .or_else(|| project_problem(m))
}

fn project_problem(m: &Manifest) -> Option<ManifestReason> {
    use crate::project::ActContext;
    let project = m.project.as_ref();
    if let Some(p) = project {
        if p.markers.is_empty() {
            return Some(ManifestReason::NoDetectionMarkers);
        }
        for (kind, count, limit) in [
            (
                ManifestCount::Screens,
                m.screens.len() + p.screens.len(),
                MAX_SCREENS,
            ),
            (
                ManifestCount::Markers,
                p.markers.len(),
                MAX_TARGETS_PER_BIND,
            ),
        ] {
            if count > limit {
                return Some(ManifestReason::TooMany { kind, count, limit });
            }
        }
        for s in &p.screens {
            for (kind, name) in std::iter::once((ManifestName::ScreenName, &s.id))
                .chain(s.row_kinds.iter().map(|r| (ManifestName::RowKind, r)))
            {
                if !is_valid_name(name) {
                    return Some(ManifestReason::InvalidName {
                        kind,
                        name: display_name(name),
                        limit: MAX_NAME_CHARS,
                    });
                }
            }
            if s.row_kinds.len() > MAX_TARGETS_PER_BIND {
                return Some(ManifestReason::TooMany {
                    kind: ManifestCount::RowKinds,
                    count: s.row_kinds.len(),
                    limit: MAX_TARGETS_PER_BIND,
                });
            }
        }
    }
    let mut screens = std::collections::HashSet::new();
    let mut keys = std::collections::HashSet::new();
    for id in m.screens.iter().chain(
        project
            .into_iter()
            .flat_map(|p| p.screens.iter().map(|s| &s.id)),
    ) {
        if !screens.insert(id) {
            return Some(ManifestReason::DuplicateScreen {
                screen: display_name(id),
            });
        }
    }
    for s in project.into_iter().flat_map(|p| &p.screens) {
        if let Some(key) = s.key {
            if !keys.insert(key) {
                return Some(ManifestReason::DuplicateScreenKey {
                    key: key.escape_default().to_string(),
                });
            }
        }
    }
    for a in &m.actions {
        if a.destructive && a.confirmation.as_ref().is_none_or(|c| c.choices.is_empty()) {
            return Some(ManifestReason::DestructiveWithoutConfirmation {
                action: display_name(&a.id),
            });
        }
        // The legacy key context remains permissive. Only the new typed context names projects.
        if let Some(ActContext::Screen { screen, row_kinds }) = &a.project_context {
            let declared = project.and_then(|p| p.screens.iter().find(|s| &s.id == screen));
            let Some(declared) = declared else {
                return Some(ManifestReason::UnknownScreen {
                    action: display_name(&a.id),
                    screen: display_name(screen),
                });
            };
            for kind in row_kinds {
                if !declared.row_kinds.contains(kind) {
                    return Some(ManifestReason::UnknownRowKind {
                        action: display_name(&a.id),
                        screen: display_name(screen),
                        kind: display_name(kind),
                    });
                }
            }
        }
    }
    project_values::problem(m)
}

/// Why a registration was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryError {
    /// The manifest declares a name that is not usable or more than the host takes (see
    /// [`is_valid_name`], [`MAX_ACTIONS`]...). `id` is safe to print.
    InvalidManifest {
        id: String,
        why: String,
        reason: ManifestReason,
    },
    /// A plugin with this id is already registered; the later one is refused.
    DuplicateId(String),
    /// A glob in the manifest does not compile.
    InvalidGlob { id: String, glob: String },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::DuplicateId(id) => write!(f, "plugin id `{id}` is already registered"),
            RegistryError::InvalidGlob { id, glob } => {
                write!(f, "plugin `{id}` declares an invalid glob `{glob}`")
            }
            RegistryError::InvalidManifest { id, why, .. } => {
                write!(f, "plugin `{id}` is refused: {why}")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// Something found about a plugin when it was registered, for the startup message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryNote {
    pub plugin: String,
    pub kind: NoteKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteKind {
    /// An action lost its key or was refused (see [`KeyIssue`]); the rest of the plugin works.
    Key(KeyIssue),
    /// The plugin was built against an API version this host does not speak; it is switched off.
    ApiMismatch {
        declared: ApiVersion,
        host: ApiVersion,
    },
    /// The plugin is given files (`binds`) but declares no `shows`, so the host attaches no
    /// screen to them.
    ShowsNotDeclared,
    /// The registration was refused because the manifest declares a name that is not usable or
    /// more than the host takes. The plugin is not registered; `RegistryNote::plugin` is a safe
    /// rendering of its id.
    Refused { why: String },
}

/// A user binding (a `[[preview.rules]]` entry read as "this target goes to that plugin").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRule {
    pub target: Target,
    /// The plugin id.
    pub plugin: String,
    pub hint: Option<String>,
}

/// What is known about a file. The content checks run only when a rule needs them, and each runs
/// at most once per resolution.
pub struct FileFacts<'a> {
    pub path: &'a Path,
    /// The MIME type sniffed from the content.
    pub sniff_mime: &'a dyn Fn() -> Option<String>,
    /// Whether the content reads as text.
    pub is_text: &'a dyn Fn() -> bool,
}

/// The decision for a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Plugin { id: String, hint: Option<String> },
    Text,
    CanNotPreview { ext: String },
}

/// One line of `konoma --plugins`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingRow {
    pub target: Target,
    pub priority: Priority,
    pub plugin: String,
    pub origin: PluginOrigin,
    pub enabled: bool,
}

enum Matcher {
    Glob(GlobMatcher),
    FileName(String),
    Mime(String),
    Content(String),
    Embed(String),
}

struct CompiledBind {
    priority: Priority,
    matchers: Vec<(Target, Matcher)>,
}

struct Entry {
    id: String,
    manifest: Manifest,
    plugin: Arc<dyn Plugin>,
    origin: PluginOrigin,
    enabled: bool,
    /// Set when the plugin cannot run at all (an API version this host does not speak). It stays
    /// listed, but switched off for good: `set_enabled` cannot lift it.
    blocked: bool,
    binds: Vec<CompiledBind>,
    notes: Vec<NoteKind>,
}

/// The registered plugins.
#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
    /// The plugins whose registration was refused (the notes for the startup message).
    refused: Vec<RegistryNote>,
}

struct Lazy<'a> {
    facts: &'a FileFacts<'a>,
    mime: OnceCell<Option<String>>,
    is_text: OnceCell<bool>,
}

impl Lazy<'_> {
    fn mime(&self) -> Option<&str> {
        self.mime
            .get_or_init(|| (self.facts.sniff_mime)())
            .as_deref()
    }

    fn is_text(&self) -> bool {
        *self.is_text.get_or_init(|| (self.facts.is_text)())
    }

    fn file_name(&self) -> &str {
        self.facts
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
    }
}

fn mime_glob_match(pattern: &str, mime: &str) -> bool {
    match pattern.strip_suffix("/*") {
        Some(prefix) => mime.starts_with(prefix),
        None => pattern == mime,
    }
}

fn compile(target: &Target) -> Option<Matcher> {
    Some(match target {
        Target::Glob(g) => Matcher::Glob(
            GlobBuilder::new(g)
                .case_insensitive(true)
                .build()
                .ok()?
                .compile_matcher(),
        ),
        Target::FileName(n) => Matcher::FileName(n.to_lowercase()),
        Target::Mime(m) => Matcher::Mime(m.clone()),
        Target::Content(c) => Matcher::Content(c.clone()),
        Target::Embed(e) => Matcher::Embed(e.clone()),
    })
}

fn matches_file(m: &Matcher, lazy: &Lazy) -> bool {
    match m {
        Matcher::Glob(g) => g.is_match(lazy.file_name()),
        Matcher::FileName(n) => lazy.file_name().to_lowercase() == *n,
        Matcher::Mime(p) => lazy.mime().is_some_and(|m| mime_glob_match(p, m)),
        Matcher::Content(_) | Matcher::Embed(_) => false,
    }
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Register a plugin. The manifest is read once. A second plugin with the same id is refused.
    pub fn register(
        &mut self,
        plugin: Arc<dyn Plugin>,
        origin: PluginOrigin,
    ) -> Result<(), RegistryError> {
        let manifest = plugin.manifest();
        if let Some(reason) = manifest_problem(&manifest) {
            let why = reason.to_string();
            let id = display_name(&manifest.id);
            self.refused.push(RegistryNote {
                plugin: id.clone(),
                kind: NoteKind::Refused { why: why.clone() },
            });
            return Err(RegistryError::InvalidManifest { id, why, reason });
        }
        // What the host registers is the sanitized manifest: an action whose key is reserved
        // (layers 0 to 3) or taken keeps the action and loses the key; one that cannot be
        // registered is dropped. Both are noted for the startup message.
        let (mut manifest, key_issues) = sanitize(&manifest);
        // The version is shown (`konoma --plugins`): cut, and without control characters.
        manifest.version = manifest
            .version
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_VERSION_CHARS)
            .collect();
        // The strings of a setting's default (and the keys of a table in it) are texts too.
        for st in &mut manifest.settings {
            st.default = st.default.cleaned();
        }
        let mut notes: Vec<NoteKind> = key_issues.into_iter().map(NoteKind::Key).collect();
        let blocked = !manifest.api.works_with(ApiVersion::CURRENT);
        if blocked {
            notes.push(NoteKind::ApiMismatch {
                declared: manifest.api,
                host: ApiVersion::CURRENT,
            });
        }
        if manifest.assigns_files_without_shows() {
            notes.push(NoteKind::ShowsNotDeclared);
        }
        if self.entries.iter().any(|e| e.id == manifest.id) {
            return Err(RegistryError::DuplicateId(manifest.id));
        }
        let mut binds = Vec::new();
        for b in &manifest.binds {
            let mut matchers = Vec::new();
            for t in &b.targets {
                let m = compile(t).ok_or_else(|| RegistryError::InvalidGlob {
                    id: manifest.id.clone(),
                    glob: match t {
                        Target::Glob(g) => g.clone(),
                        _ => String::new(),
                    },
                })?;
                matchers.push((t.clone(), m));
            }
            binds.push(CompiledBind {
                priority: b.priority,
                matchers,
            });
        }
        self.entries.push(Entry {
            id: manifest.id.clone(),
            manifest,
            plugin,
            origin,
            enabled: !blocked,
            blocked,
            binds,
            notes,
        });
        Ok(())
    }

    /// Enable or disable a plugin. Returns false when the id is unknown.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> bool {
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(e) => {
                e.enabled = enabled && !e.blocked;
                true
            }
            None => false,
        }
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.entry(id).is_some_and(|e| e.enabled)
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Who registered the plugin.
    pub fn origin(&self, id: &str) -> Option<PluginOrigin> {
        self.entry(id).map(|e| e.origin)
    }

    /// What registering found out, for every plugin, in registration order.
    pub fn notes(&self) -> Vec<RegistryNote> {
        self.entries
            .iter()
            .flat_map(|e| {
                e.notes.iter().map(|k| RegistryNote {
                    plugin: e.id.clone(),
                    kind: k.clone(),
                })
            })
            .chain(self.refused.iter().cloned())
            .collect()
    }

    /// The manifest as registered (keys that could not be kept are removed).
    pub fn manifest(&self, id: &str) -> Option<&Manifest> {
        self.entry(id).map(|e| &e.manifest)
    }

    pub fn plugin(&self, id: &str) -> Option<Arc<dyn Plugin>> {
        self.entry(id).map(|e| e.plugin.clone())
    }

    /// The ids in registration order.
    pub fn ids(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.id.as_str()).collect()
    }

    /// Decide who shows a file.
    pub fn resolve_file(&self, rules: &[UserRule], facts: &FileFacts) -> Resolution {
        let lazy = Lazy {
            facts,
            mime: OnceCell::new(),
            is_text: OnceCell::new(),
        };
        // 1. The user's rules, first match; a rule aimed at a missing or disabled plugin is skipped.
        for rule in rules {
            if !self.is_enabled(&rule.plugin)
                || self.origin(&rule.plugin) == Some(PluginOrigin::Bundled)
            {
                continue;
            }
            if let Some(m) = compile(&rule.target) {
                if matches_file(&m, &lazy) {
                    return Resolution::Plugin {
                        id: rule.plugin.clone(),
                        hint: rule.hint.clone(),
                    };
                }
            }
        }
        // 2. and 3. Default bindings: user plugins, then standard ones.
        for origin in [PluginOrigin::User, PluginOrigin::Standard] {
            for e in self
                .entries
                .iter()
                .filter(|e| e.enabled && e.origin == origin)
            {
                let hit = e
                    .binds
                    .iter()
                    .filter(|b| b.priority == Priority::Default)
                    .any(|b| b.matchers.iter().any(|(_, m)| matches_file(m, &lazy)));
                if hit {
                    return Resolution::Plugin {
                        id: e.id.clone(),
                        hint: None,
                    };
                }
            }
        }
        // 4. Text.
        if lazy.is_text() {
            return Resolution::Text;
        }
        // 5. Give up, safely.
        Resolution::CanNotPreview {
            ext: facts
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string(),
        }
    }

    fn resolve_by(&self, hit: impl Fn(&Matcher) -> bool) -> Option<String> {
        // A bundled plugin comes last: the program's own plugins answer first.
        for origin in [
            PluginOrigin::User,
            PluginOrigin::Standard,
            PluginOrigin::Bundled,
        ] {
            for e in self
                .entries
                .iter()
                .filter(|e| e.enabled && e.origin == origin)
            {
                let found = e
                    .binds
                    .iter()
                    .filter(|b| b.priority == Priority::Default)
                    .any(|b| b.matchers.iter().any(|(_, m)| hit(m)));
                if found {
                    return Some(e.id.clone());
                }
            }
        }
        None
    }

    /// The plugin that takes a content type (a user plugin before a standard one).
    pub fn resolve_content(&self, content_type: &str) -> Option<String> {
        self.resolve_by(|m| matches!(m, Matcher::Content(c) if c == content_type))
    }

    /// The plugin that takes an embedded kind (`fence:mermaid`, `math`).
    pub fn resolve_embed(&self, kind: &str) -> Option<String> {
        self.resolve_by(|m| matches!(m, Matcher::Embed(k) if k == kind))
    }

    /// Every binding with its owner, for `konoma --plugins`.
    pub fn describe(&self) -> Vec<BindingRow> {
        let mut rows = Vec::new();
        for e in &self.entries {
            for (b, c) in e.manifest.binds.iter().zip(&e.binds) {
                for (target, _) in &c.matchers {
                    rows.push(BindingRow {
                        target: target.clone(),
                        priority: b.priority,
                        plugin: e.id.clone(),
                        origin: e.origin,
                        enabled: e.enabled,
                    });
                }
            }
        }
        rows
    }
}
