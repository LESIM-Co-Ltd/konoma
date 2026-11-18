//! Reply-publication barriers around the real project loop, only for cross-host tests.
use super::*;
use crate::app::{
    diff_screen::{DiffScreenState, FileDiffRequest},
    AppError, AppEvent,
};
use konoma_host_core::project::DiffView;

/// Open or step the standalone diff through real input, settling only published worker replies.
pub fn drive_diff_representation_for_test<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    prepared: &mut PreparedProjects,
    path: Option<PathBuf>,
    key: Option<crate::Key>,
) -> Result<(DiffView, DiffView, PathBuf), AppError> {
    if let Some(path) = path {
        prepared.file_previews = Some(crate::host::file_preview::FilePreviewSource::new(
            prepared.registry.clone(),
            prepared.config.preview.rules.clone(),
            prepared.config.external.preview_commands,
        ));
        prepared.diff_return = prepared.screen.take();
        prepared.diff = Some(
            DiffScreenState::open_for_project(
                FileDiffRequest {
                    path,
                    root: prepared.open_dir.clone(),
                    info: prepared.service_info.values().next().unwrap().clone(),
                },
                prepared.services.clone(),
                &prepared.config,
                prepared.settings.diff_layout,
                None,
                prepared.file_previews.clone(),
            )
            .unwrap(),
        );
    }
    if prepared.file_preview.is_none() {
        prepared
            .diff
            .as_mut()
            .unwrap()
            .settle_representations_for_test();
        land_preview(prepared);
    }
    run_project_with(
        terminal,
        prepared,
        &mut key.map(AppEvent::Key).into_iter(),
        &ProjectLoopCx::default(),
        None,
    )?;
    if prepared.file_preview.is_none() {
        prepared
            .diff
            .as_mut()
            .unwrap()
            .settle_representations_for_test();
        land_preview(prepared);
    }
    let diff = prepared.diff.as_ref().unwrap();
    Ok((
        if prepared.file_preview.is_some() {
            DiffView::Preview
        } else {
            diff.effective
        },
        diff.choice,
        diff.request.path.clone(),
    ))
}

fn land_preview(prepared: &mut PreparedProjects) {
    if prepared.preview_open.is_none() {
        let diff = prepared.diff.as_mut().unwrap();
        if diff.take_command().is_some() {
            let source = diff.return_info();
            prepared.preview_open = preview::PendingPreview::start(prepared, source);
        }
    }
    if let Some(pending) = prepared.preview_open.take() {
        let opened = pending.wait_for_test().unwrap();
        let mut diff = prepared.diff.take().unwrap();
        prepared.file_preview =
            Some(pending.finish(opened, prepared, &ProjectLoopCx::default(), &mut diff));
        prepared.diff = Some(diff);
    }
}
