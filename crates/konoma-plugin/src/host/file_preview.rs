//! One file-preview decision for every host. Reading belongs to a worker; resolution is pure.
use std::cell::OnceCell;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::registry::{FileFacts, PluginOrigin, Registry, Resolution, UserRule};
use crate::{Target, ViewKind};
use konoma_host_core::project::{DiffTarget, MediaDiffKind};
use konoma_host_core::shared::Rule;

const TEXT: &str = "text";
const COMMAND: &str = "command";
const UNAVAILABLE: &str = "can-not-preview";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePreviewPlan {
    Text,
    Plugin {
        id: String,
        hint: Option<String>,
    },
    Command {
        template: String,
        output: Option<String>,
        detached: bool,
    },
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFilePreview {
    pub plan: FilePreviewPlan,
    pub diff_target: DiffTarget,
    /// The registry selection also owns launch verbs when it has no preview display.
    pub selected_plugin: Option<String>,
    /// The precise picture format, already settled by the worker for media predicates.
    pub media_kind: Option<MediaDiffKind>,
}

/// Already-read facts. `None` means unavailable content, rather than an empty text file.
pub struct FilePreviewFacts<'a> {
    pub path: &'a Path,
    pub bytes: Option<&'a [u8]>,
}

/// Registry and config captured together, before dispatching work.
#[derive(Clone)]
pub struct FilePreviewSource {
    registry: Arc<Registry>,
    rules: Vec<Rule>,
    commands: bool,
}

impl FilePreviewSource {
    pub fn new(registry: Arc<Registry>, rules: Vec<Rule>, commands: bool) -> Self {
        Self {
            registry,
            rules,
            commands,
        }
    }

    /// Resolve on a worker. Baseline bytes take precedence over the possibly deleted path.
    /// Without baseline bytes, special files are rejected before any opener or glob match.
    pub fn resolve(&self, path: &Path, sniff: Option<&[u8]>) -> ResolvedFilePreview {
        resolve_path(&self.registry, &self.rules, self.commands, path, sniff)
    }

    /// Select only the registry plugin. Launch lookup never needs a display or diff format.
    pub fn selected_plugin(&self, path: &Path) -> Option<String> {
        selected_plugin_of_path(&self.registry, &self.rules, path)
    }

    /// Ordinary preview selection does not classify a diff representation.
    pub fn plan(&self, path: &Path, sniff: Option<&[u8]>) -> FilePreviewPlan {
        preview_plan_of_path(&self.registry, &self.rules, self.commands, path, sniff).0
    }

    pub fn verbs(&self, resolved: &ResolvedFilePreview) -> Vec<crate::VerbDecl> {
        match &resolved.selected_plugin {
            Some(id) => self
                .registry
                .manifest(id)
                .map(|m| m.verbs.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

pub(crate) fn preview_plan_of_path(
    registry: &Registry,
    rules: &[Rule],
    commands: bool,
    path: &Path,
    sniff: Option<&[u8]>,
) -> (FilePreviewPlan, Option<String>) {
    PathContent::new(path, sniff).with_facts(|facts| resolve_plan(registry, rules, commands, facts))
}

pub(crate) fn resolve_path(
    registry: &Registry,
    rules: &[Rule],
    commands: bool,
    path: &Path,
    sniff: Option<&[u8]>,
) -> ResolvedFilePreview {
    let content = PathContent::new(path, sniff);
    let (plan, selected_plugin) =
        content.with_facts(|facts| resolve_plan(registry, rules, commands, facts));
    let media_kind = media_kind_with(registry, &plan, path, || content.bytes());
    let diff_target = target_from_format(registry, &plan, media_kind);
    ResolvedFilePreview {
        plan,
        diff_target,
        selected_plugin,
        media_kind,
    }
}

pub(crate) fn selected_plugin_of_path(
    registry: &Registry,
    rules: &[Rule],
    path: &Path,
) -> Option<String> {
    PathContent::new(path, None).with_facts(|facts| {
        match registry.resolve_file_with(&user_rules(rules, path), facts, is_internal) {
            Resolution::Plugin { id, .. } => Some(id),
            _ => None,
        }
    })
}

fn is_internal(id: &str) -> bool {
    matches!(id, TEXT | COMMAND | UNAVAILABLE)
}

struct PathContent<'a> {
    path: &'a Path,
    sniff: Option<&'a [u8]>,
    head: OnceCell<Option<Vec<u8>>>,
}

impl<'a> PathContent<'a> {
    fn new(path: &'a Path, sniff: Option<&'a [u8]>) -> Self {
        Self {
            path,
            sniff,
            head: OnceCell::new(),
        }
    }

    fn bytes(&self) -> Option<&[u8]> {
        self.sniff.or_else(|| {
            self.head
                .get_or_init(|| {
                    // A FIFO must never reach open, even when content is needed by a MIME rule.
                    if !path_metadata(self.path).ok()?.is_file() {
                        return None;
                    }
                    #[cfg(any(test, feature = "test-hooks"))]
                    test_hooks::note_open();
                    let mut file = std::fs::File::open(self.path).ok()?;
                    let mut bytes = vec![0; 8192];
                    #[cfg(any(test, feature = "test-hooks"))]
                    test_hooks::note_read();
                    let n = file.read(&mut bytes).ok()?;
                    bytes.truncate(n);
                    Some(bytes)
                })
                .as_deref()
        })
    }

    fn with_facts<T>(&self, f: impl FnOnce(&FileFacts<'_>) -> T) -> T {
        let mime = || {
            self.bytes()
                .and_then(konoma_host_core::sniff::mime_of_bytes)
        };
        let text = || {
            self.bytes()
                .is_some_and(konoma_widgets::text::is_probably_text_bytes)
        };
        f(&FileFacts {
            path: self.path,
            sniff_mime: &mime,
            is_text: &text,
        })
    }
}

fn plugin_of_builtin(name: &str, path: &Path) -> (&'static str, Option<&'static str>) {
    match name {
        "markdown" => ("markdown", None),
        "mermaid" => ("mermaid", None),
        "image" => ("image", None),
        "svg" => ("svg", None),
        "video" => ("video", None),
        "pdf" => ("pdf", None),
        "code" => ("code", None),
        "archive" => ("archive", None),
        "spreadsheet" => ("spreadsheet", None),
        "csv" => ("csv", Some("csv")),
        "tsv" => ("csv", Some("tsv")),
        "document" if is_presentation_path(path) => ("presentation", None),
        "document" => ("document", None),
        "text" => (TEXT, None),
        _ => (UNAVAILABLE, None),
    }
}

/// The legacy document builtin covers both Word and presentation files.
fn is_presentation_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "pptx" | "pptm" | "ppsx" | "ppsm" | "potx" | "potm" | "ppt" | "odp" | "otp"
        )
    })
}

/// Expand OR predicates without changing first-match order. Command hints index the original rules.
pub fn user_rules(rules: &[Rule], path: &Path) -> Vec<UserRule> {
    let mut out = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        let (plugin, hint) = match (&rule.builtin, &rule.command) {
            (Some(name), _) => {
                let (p, h) = plugin_of_builtin(name, path);
                (p, h.map(str::to_owned))
            }
            (None, Some(_)) => (COMMAND, Some(i.to_string())),
            _ => (UNAVAILABLE, None),
        };
        for target in [
            rule.glob.clone().map(Target::Glob),
            rule.mime.clone().map(Target::Mime),
        ]
        .into_iter()
        .flatten()
        {
            out.push(UserRule {
                target,
                plugin: plugin.into(),
                hint: hint.clone(),
            });
        }
    }
    out
}

/// Pure resolution from the same declarations and config, with no filesystem fallback.
pub fn resolve_file_preview(
    registry: &Registry,
    rules: &[Rule],
    commands: bool,
    facts: FilePreviewFacts<'_>,
) -> ResolvedFilePreview {
    let mime = || facts.bytes.and_then(konoma_host_core::sniff::mime_of_bytes);
    let text = || {
        facts
            .bytes
            .is_some_and(konoma_widgets::text::is_probably_text_bytes)
    };
    let file = FileFacts {
        path: facts.path,
        sniff_mime: &mime,
        is_text: &text,
    };
    let (plan, selected_plugin) = resolve_plan(registry, rules, commands, &file);
    let media_kind = media_kind_with(registry, &plan, facts.path, || facts.bytes);
    let diff_target = target_from_format(registry, &plan, media_kind);
    ResolvedFilePreview {
        plan,
        diff_target,
        selected_plugin,
        media_kind,
    }
}

fn resolve_plan(
    registry: &Registry,
    rules: &[Rule],
    commands: bool,
    facts: &FileFacts<'_>,
) -> (FilePreviewPlan, Option<String>) {
    let resolution = registry.resolve_file_with(&user_rules(rules, facts.path), facts, |id| {
        matches!(id, TEXT | COMMAND | UNAVAILABLE)
    });
    let selected_plugin = match &resolution {
        Resolution::Plugin { id, .. } => Some(id.clone()),
        _ => None,
    };
    let plan = match resolution {
        Resolution::Text => FilePreviewPlan::Text,
        Resolution::CanNotPreview { .. } => FilePreviewPlan::Unavailable,
        Resolution::Plugin { id, hint } => match id.as_str() {
            TEXT => FilePreviewPlan::Text,
            COMMAND => match hint
                .as_deref()
                .and_then(|h| h.parse::<usize>().ok())
                .and_then(|i| rules.get(i))
            {
                Some(r) if commands && r.command.is_some() => FilePreviewPlan::Command {
                    template: r.command.clone().unwrap(),
                    output: r.render_as.clone(),
                    detached: r.detached,
                },
                _ => FilePreviewPlan::Unavailable,
            },
            "csv" => FilePreviewPlan::Plugin {
                hint: Some(
                    if crate::table_sep::delimiter(hint.as_deref(), facts.path) == b'\t' {
                        "tsv"
                    } else {
                        "csv"
                    }
                    .into(),
                ),
                id,
            },
            "archive" if !archive_path(facts.path) => FilePreviewPlan::Unavailable,
            _ if registry.origin(&id) == Some(PluginOrigin::User)
                || registry.manifest(&id).is_some_and(|m| !m.shows.is_empty()) =>
            {
                FilePreviewPlan::Plugin { id, hint }
            }
            _ => FilePreviewPlan::Unavailable,
        },
    };
    (plan, selected_plugin)
}

fn archive_path(path: &Path) -> bool {
    path.file_name().and_then(|s| s.to_str()).is_some_and(|s| {
        let s = s.to_ascii_lowercase();
        [".zip", ".tar", ".tgz", ".tar.gz"]
            .iter()
            .any(|ext| s.ends_with(ext))
    })
}

/// Classify an already settled plan. Hosts also map their non-file states to Unavailable.
#[cfg(any(test, feature = "test-hooks"))]
fn plan_diff_target(
    registry: &Registry,
    plan: &FilePreviewPlan,
    facts: FilePreviewFacts<'_>,
) -> DiffTarget {
    plan_diff_target_with(registry, plan, facts.path, || facts.bytes)
}

#[cfg(any(test, feature = "test-hooks"))]
fn plan_diff_target_with<'a>(
    registry: &Registry,
    plan: &FilePreviewPlan,
    path: &Path,
    bytes: impl FnOnce() -> Option<&'a [u8]>,
) -> DiffTarget {
    target_from_format(registry, plan, media_kind_with(registry, plan, path, bytes))
}

fn media_kind_with<'a>(
    registry: &Registry,
    plan: &FilePreviewPlan,
    path: &Path,
    bytes: impl FnOnce() -> Option<&'a [u8]>,
) -> Option<MediaDiffKind> {
    let FilePreviewPlan::Plugin { id, .. } = plan else {
        return None;
    };
    if !registry
        .manifest(id)
        .is_some_and(|m| m.shows.contains(&ViewKind::Image))
    {
        return None;
    }
    super::project::media_diff::media_format_from_extension(path).unwrap_or_else(|| {
        super::project::media_diff::media_format(path, Some(bytes().unwrap_or_default()))
    })
}

fn target_from_format(
    registry: &Registry,
    plan: &FilePreviewPlan,
    media_kind: Option<MediaDiffKind>,
) -> DiffTarget {
    match plan {
        FilePreviewPlan::Text => DiffTarget::Text,
        FilePreviewPlan::Command { output, .. } if output.as_deref() != Some("image") => {
            DiffTarget::Text
        }
        FilePreviewPlan::Plugin { id, .. } => {
            let Some(m) = registry.manifest(id) else {
                return DiffTarget::Other;
            };
            if m.shows.contains(&ViewKind::Document) && !m.converts {
                DiffTarget::Markdown
            } else if (m.shows.contains(&ViewKind::Text) && m.file_text) || id == "mermaid" {
                DiffTarget::Text
            } else if m.shows.contains(&ViewKind::Image) {
                match media_kind {
                    Some(MediaDiffKind::Svg) => DiffTarget::Svg,
                    Some(MediaDiffKind::Image | MediaDiffKind::Pdf) => DiffTarget::Picture,
                    None => DiffTarget::Other,
                }
            } else {
                DiffTarget::Other
            }
        }
        _ => DiffTarget::Other,
    }
}

/// Compatibility probes for host tests of already constructed display states.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub mod test_hooks {
    use super::*;
    use std::cell::Cell;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct PathIo {
        pub metadata: usize,
        pub opens: usize,
        pub reads: usize,
    }

    thread_local! { static IO: Cell<PathIo> = Cell::new(PathIo::default()); }

    pub(super) fn note_metadata() {
        IO.with(|io| {
            let mut n = io.get();
            n.metadata += 1;
            io.set(n);
        });
    }

    pub(super) fn note_open() {
        IO.with(|io| {
            let mut n = io.get();
            n.opens += 1;
            io.set(n);
        });
    }

    pub(super) fn note_read() {
        IO.with(|io| {
            let mut n = io.get();
            n.reads += 1;
            io.set(n);
        });
    }

    pub fn count_path_io<T>(f: impl FnOnce() -> T) -> (T, PathIo) {
        let before = IO.with(Cell::get);
        let value = f();
        let after = IO.with(Cell::get);
        (
            value,
            PathIo {
                metadata: after.metadata - before.metadata,
                opens: after.opens - before.opens,
                reads: after.reads - before.reads,
            },
        )
    }

    pub fn is_presentation_path(path: &Path) -> bool {
        super::is_presentation_path(path)
    }

    pub fn plan_diff_target(
        registry: &Registry,
        plan: &FilePreviewPlan,
        facts: FilePreviewFacts<'_>,
    ) -> DiffTarget {
        super::plan_diff_target(registry, plan, facts)
    }
}

fn path_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    #[cfg(any(test, feature = "test-hooks"))]
    test_hooks::note_metadata();
    std::fs::metadata(path)
}
