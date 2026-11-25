//! Konoma's conversion of the shared file decision into its display state.
use crate::config::Rule;
use crate::preview::PreviewKind;
use konoma_plugin::host::file_preview::{FilePreviewPlan, FilePreviewSource};
use std::path::Path;

pub(crate) fn user_rules(rules: &[Rule], path: &Path) -> Vec<konoma_plugin::registry::UserRule> {
    konoma_plugin::host::file_preview::user_rules(
        &rules.iter().map(Into::into).collect::<Vec<_>>(),
        path,
    )
}

pub(crate) fn kind_of(plan: FilePreviewPlan, path: &Path) -> PreviewKind {
    match plan {
        FilePreviewPlan::Text => PreviewKind::Text(path.into()),
        FilePreviewPlan::Plugin { id, hint } => PreviewKind::Plugin {
            id,
            hint,
            path: path.into(),
        },
        FilePreviewPlan::Command {
            template,
            output,
            detached,
        } => PreviewKind::Command {
            path: path.into(),
            template,
            render_as: output,
            detached,
        },
        FilePreviewPlan::Unavailable => PreviewKind::can_not_preview(path),
    }
}

pub(crate) fn resolve_kind(
    rules: &[Rule],
    disabled: &[String],
    commands: bool,
    path: &Path,
    sniff: Option<&[u8]>,
) -> PreviewKind {
    #[cfg(test)]
    crate::test_support::note_resolve_preview_call();
    let source = FilePreviewSource::new(
        super::shared_registry(disabled),
        rules.iter().map(Into::into).collect(),
        commands,
    );
    kind_of(source.plan(path, sniff), path)
}

pub(crate) fn resolve_file(
    rules: &[Rule],
    disabled: &[String],
    commands: bool,
    path: &Path,
    sniff: Option<&[u8]>,
) -> konoma_plugin::host::file_preview::ResolvedFilePreview {
    #[cfg(test)]
    crate::test_support::note_resolve_preview_call();
    let source = FilePreviewSource::new(
        super::shared_registry(disabled),
        rules.iter().map(Into::into).collect(),
        commands,
    );
    source.resolve(path, sniff)
}

pub(crate) fn plugin_id_of(rules: &[Rule], disabled: &[String], path: &Path) -> Option<String> {
    if !crate::preview::is_previewable(path) {
        return None;
    }
    #[cfg(test)]
    crate::test_support::note_plugin_id_read();
    FilePreviewSource::new(
        super::shared_registry(disabled),
        rules.iter().map(Into::into).collect(),
        true,
    )
    .selected_plugin(path)
}
