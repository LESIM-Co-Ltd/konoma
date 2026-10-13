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

/// Who registered a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginOrigin {
    /// Shipped with konoma.
    Standard,
    /// Added by the user.
    User,
}

/// Why a registration was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
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
        // What the host registers is the sanitized manifest: an action whose key is reserved
        // (layers 0 to 3) or taken keeps the action and loses the key; one that cannot be
        // registered is dropped. Both are noted for the startup message.
        let (manifest, key_issues) = sanitize(&manifest);
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
            if !self.is_enabled(&rule.plugin) {
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
        for origin in [PluginOrigin::User, PluginOrigin::Standard] {
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
