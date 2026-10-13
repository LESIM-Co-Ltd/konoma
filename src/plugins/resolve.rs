//! Settling which plugin shows a file, and mapping the answer onto today's `PreviewKind`.

use std::path::Path;

use konoma_plugin::registry::{FileFacts, Resolution, UserRule};
use konoma_plugin::Target;

use super::{with_registry, COMMAND_ID, TEXT_ID, UNSUPPORTED_ID};
use crate::config::Rule;
use crate::preview::{archive, PreviewKind};

/// Extensions of the presentation formats (the rest of the old `builtin = "document"` is Word).
fn is_presentation(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| {
            matches!(
                e.as_str(),
                "pptx" | "pptm" | "ppsx" | "ppsm" | "potx" | "potm" | "odp" | "otp"
            )
        })
}

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
        "document" if is_presentation(path) => ("presentation", None),
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

/// The field separator of a table plugin's input: the note wins, otherwise the extension.
fn delimiter(hint: Option<&str>, path: &Path) -> u8 {
    match hint {
        Some("tsv") => b'\t',
        Some(_) => b',',
        None => {
            let tsv = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
            if tsv {
                b'\t'
            } else {
                b','
            }
        }
    }
}

/// Map the registry's answer onto the `PreviewKind` that still carries the display.
fn kind_of(res: Resolution, rules: &[Rule], preview_commands: bool, path: &Path) -> PreviewKind {
    let p = path.to_path_buf();
    match res {
        Resolution::Text => PreviewKind::Text(p),
        Resolution::CanNotPreview { .. } => PreviewKind::can_not_preview(path),
        Resolution::Plugin { id, hint } => match id.as_str() {
            "markdown" => PreviewKind::Markdown(p),
            "mermaid" => PreviewKind::Mermaid(p),
            "image" => PreviewKind::Image(p),
            "svg" => PreviewKind::Svg(p),
            "video" => PreviewKind::Video(p),
            "pdf" => PreviewKind::Pdf(p),
            "code" => PreviewKind::Code(p),
            "csv" => PreviewKind::Table {
                delimiter: delimiter(hint.as_deref(), path),
                path: p,
            },
            "spreadsheet" => PreviewKind::Spreadsheet(p),
            // Word and presentations are one display until the plugins are wired in.
            "document" | "presentation" => PreviewKind::Document(p),
            "archive" => match archive::ArchiveKind::from_path(path) {
                Some(kind) => PreviewKind::Archive { path: p, kind },
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
            // `can-not-preview`, a plugin that has no display here yet, or a user plugin.
            _ => PreviewKind::can_not_preview(path),
        },
    }
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
    let mime = || {
        let kind = match sniff {
            Some(bytes) => infer::get(bytes),
            None => infer::get_from_path(path).ok().flatten(),
        };
        kind.map(|k| k.mime_type().to_string())
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
    kind_of(res, rules, preview_commands, path)
}
