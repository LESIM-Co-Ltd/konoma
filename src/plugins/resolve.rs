//! Settling which plugin shows a file, and mapping the answer onto today's `PreviewKind`.

use std::path::Path;

use konoma_plugin::registry::{FileFacts, PluginOrigin, Resolution, UserRule};
use konoma_plugin::Target;

use super::{with_registry, COMMAND_ID, TEXT_ID, UNSUPPORTED_ID};
use crate::config::Rule;
use crate::preview::{archive, PreviewKind};

/// The plugin (and the note handed with it) an old `builtin = "<name>"` stands for. `path` decides
/// the one name that covered two plugins: `document` is Word or a presentation by extension.
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
        "document" if crate::app::is_presentation_path(path) => ("presentation", None),
        "document" => ("document", None),
        "text" => (TEXT_ID, None),
        _ => (UNSUPPORTED_ID, None),
    }
}

/// `[[preview.rules]]` read as the user's assignments, in order. A rule with both a glob and a mime
/// becomes two assignments (it matches when either does), so first-match order is unchanged.
/// The note of a `command` rule is the index of the rule in `rules`.
pub(crate) fn user_rules(rules: &[Rule], path: &Path) -> Vec<UserRule> {
    let mut out = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        let (plugin, hint) = match (&rule.builtin, &rule.command) {
            (Some(name), _) => {
                let (p, h) = plugin_of_builtin(name, path);
                (p, h.map(str::to_string))
            }
            (None, Some(_)) => (COMMAND_ID, Some(i.to_string())),
            (None, None) => (UNSUPPORTED_ID, None),
        };
        let mut push = |target: Target| {
            out.push(UserRule {
                target,
                plugin: plugin.to_string(),
                hint: hint.clone(),
            });
        };
        if let Some(g) = &rule.glob {
            push(Target::Glob(g.clone()));
        }
        if let Some(m) = &rule.mime {
            push(Target::Mime(m.clone()));
        }
    }
    out
}

/// The old `builtin` name of a CSV/TSV input: the note wins, otherwise the extension decides (the
/// same rule the plugin applies to the byte, `konoma_table::delimiter`).
fn csv_note(hint: Option<&str>, path: &Path) -> &'static str {
    if konoma_table::delimiter(hint, path) == b'\t' {
        "tsv"
    } else {
        "csv"
    }
}

/// Map the registry's answer onto the `PreviewKind` that still carries the display.
fn kind_of(
    res: Resolution,
    rules: &[Rule],
    disabled: &[String],
    preview_commands: bool,
    path: &Path,
) -> PreviewKind {
    let p = path.to_path_buf();
    match res {
        Resolution::Text => PreviewKind::Text(p),
        Resolution::CanNotPreview { .. } => PreviewKind::can_not_preview(path),
        Resolution::Plugin { id, hint } => match id.as_str() {
            // The separator is settled here, as it always was, and handed over as the note.
            "csv" => PreviewKind::Plugin {
                hint: Some(csv_note(hint.as_deref(), path).to_string()),
                id,
                path: p,
            },
            // A name that is not a zip / tar / tar.gz (a user rule can aim any glob at the
            // archive plugin) is the safe "can not preview", as before.
            "archive" => match archive::ArchiveKind::from_path(path) {
                Some(_) => PreviewKind::Plugin {
                    path: p,
                    id,
                    hint: None,
                },
                None => PreviewKind::can_not_preview(path),
            },
            TEXT_ID => PreviewKind::Text(p),
            COMMAND_ID => {
                let rule = hint
                    .as_deref()
                    .and_then(|h| h.parse::<usize>().ok())
                    .and_then(|i| rules.get(i));
                match rule.and_then(|r| r.command.as_deref().map(|c| (r, c))) {
                    // `[external] preview_commands = false`: the rule behaves as the safe
                    // "can not preview" instead of launching anything.
                    Some(_) if !preview_commands => PreviewKind::can_not_preview(path),
                    Some((r, template)) => PreviewKind::Command {
                        path: p,
                        template: template.to_string(),
                        render_as: r.render_as.clone(),
                        detached: r.detached,
                    },
                    None => PreviewKind::can_not_preview(path),
                }
            }
            // Any other plugin that shows files, standard or the user's, is opened through the same
            // Session road and drawn by the host's own widget for the view it returns. What it is
            // told apart by is what it declares (`Manifest::shows`), never its id.
            _ if shows_files(&id, disabled) => PreviewKind::Plugin { path: p, id, hint },
            // `can-not-preview`, or a standard plugin that has no display of its own (it only
            // takes embedded things).
            _ => PreviewKind::can_not_preview(path),
        },
    }
}

/// Whether the plugin `id` shows files: it declares what its main view shows, or it is one the user
/// registered (a plugin that declares nothing is still given the files it is bound to). A plugin
/// that only takes embedded things or content types (`math`) does not.
fn shows_files(id: &str, disabled: &[String]) -> bool {
    with_registry(disabled, |r| {
        r.origin(id) == Some(PluginOrigin::User)
            || r.manifest(id).is_some_and(|m| !m.shows.is_empty())
    })
}

/// Decide the `PreviewKind` of `path`. `sniff` is the content of a file that may no longer exist
/// (the old bytes of a deleted file); without it the file itself is read, and only when a rule
/// needs it, at most once.
pub(crate) fn resolve_kind(
    rules: &[Rule],
    disabled: &[String],
    preview_commands: bool,
    path: &Path,
    sniff: Option<&[u8]>,
) -> PreviewKind {
    let user = user_rules(rules, path);
    let mime = || match sniff {
        Some(bytes) => konoma_host_core::sniff::mime_of_bytes(bytes),
        None => konoma_host_core::sniff::mime_of_path(path),
    };
    let text = || match sniff {
        Some(bytes) => crate::preview::text::is_probably_text_bytes(bytes),
        None => crate::preview::text::is_probably_text(path),
    };
    let facts = FileFacts {
        path,
        sniff_mime: &mime,
        is_text: &text,
    };
    let res = with_registry(disabled, |r| r.resolve_file(&user, &facts));
    kind_of(res, rules, disabled, preview_commands, path)
}

/// [`plugin_id_of`] decided from the path alone: no file is opened (no content sniff, no text
/// probe), so a rule or binding that needs the content does not match. The footer asks this every
/// frame, and `e` acts on the same answer so the key and its hint cannot disagree.
pub(crate) fn plugin_id_by_path(
    rules: &[Rule],
    disabled: &[String],
    path: &Path,
) -> Option<String> {
    let user = user_rules(rules, path);
    let mime = || None;
    let text = || false;
    let facts = FileFacts {
        path,
        sniff_mime: &mime,
        is_text: &text,
    };
    match with_registry(disabled, |r| r.resolve_file(&user, &facts)) {
        Resolution::Plugin { id, .. } => Some(id),
        _ => None,
    }
}

/// The id of the plugin that shows `path` (a user rule first, then the standard plugins), or
/// `None` when no plugin takes it (plain text, or "can not preview"). The same decision
/// `resolve_kind` makes, read for the verbs a plugin declares (`o` opens through them).
pub(crate) fn plugin_id_of(rules: &[Rule], disabled: &[String], path: &Path) -> Option<String> {
    let user = user_rules(rules, path);
    let mime = || konoma_host_core::sniff::mime_of_path(path);
    let text = || crate::preview::text::is_probably_text(path);
    let facts = FileFacts {
        path,
        sniff_mime: &mime,
        is_text: &text,
    };
    match with_registry(disabled, |r| r.resolve_file(&user, &facts)) {
        Resolution::Plugin { id, .. } => Some(id),
        _ => None,
    }
}
