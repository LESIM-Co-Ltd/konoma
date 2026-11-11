//! Read-only inspection for integration tests, including the app-only feature matrix.
use super::{DiffPresentation, DiffScreenState};
use crate::host::project::md_diff::{MdDiffKind, MdDiffOutcome};

/// Import explicitly in tests; presentation itself is internal to the app.
pub trait DiffScreenTestHooks {
    fn presentation(&self) -> DiffPresentation;
}

impl DiffScreenTestHooks for DiffScreenState {
    fn presentation(&self) -> DiffPresentation {
        DiffScreenState::presentation(self)
    }
}

impl DiffScreenState {
    pub(in crate::app) fn actions_for_test(&self) -> Vec<(String, String)> {
        self.actions_menu()
            .into_iter()
            .map(|i| (konoma_host_core::help::help_key(i.key), i.label))
            .collect()
    }
    pub(in crate::app) fn menu_for_test(&self) -> Option<String> {
        self.extra_menu
            .map(|m| match m {
                konoma_host_core::keymap::CoreMenu::Actions => "actions".into(),
                _ => "copy".into(),
            })
            .or_else(|| self.menu.then(|| "options".into()))
    }
    pub(in crate::app) fn snapshot_for_test(
        &self,
        return_to: Vec<String>,
        width: u16,
    ) -> crate::app::project::ProjectDiffSnapshot {
        crate::app::project::ProjectDiffSnapshot {
            help: self
                .help
                .map(|top| crate::app::project::ProjectHelpSnapshot {
                    sections: self
                        .help_sections()
                        .into_iter()
                        .map(|s| (s.title, s.rows))
                        .collect(),
                    scroll: usize::from(top),
                    title: self.app_keys.help_title(),
                }),
            path: self.request.path.clone(),
            baseline: match self.request.info.backend {
                crate::app::project::ProjectBackend::Git => "HEAD",
                crate::app::project::ProjectBackend::Jj => "@-",
            }
            .into(),
            choice: self.choice,
            effective: self.effective,
            next_representation: crate::host::project::diff_keys::next_diff_view(
                self.effective,
                &self.representations(),
            ),
            layout: format!("{:?}", super::source_diff_layout(self.layout)),
            resolved_split: self.layout.is_split(width.saturating_sub(2)),
            split_drawn: (self.effective == super::DiffView::Source).then_some(self.split_drawn),
            wrap: self.display.wrap,
            scroll: usize::from(self.scroll),
            hscroll: self.hscroll,
            first_change: self.blocks().rows.first().copied(),
            editor_line: self.edit_line(),
            return_to,
            baseline_text: if self.effective == super::DiffView::Rendered {
                match self
                    .markdown
                    .landed_for(&self.request.path, MdDiffKind::Rendered)
                {
                    Some(MdDiffOutcome::Ready { old_pre, .. }) => Some(old_pre),
                    _ => None,
                }
            } else {
                None
            },
            preview_context: None,
            source_lines: self.data.as_ref().map_or_else(Vec::new, |data| {
                data.lines
                    .iter()
                    .map(|l| (format!("{:?}", l.kind), l.old_no, l.new_no, l.text.clone()))
                    .collect()
            }),
        }
    }
    pub(in crate::app) fn settle_representations_for_test(&mut self) {
        self.settle_representations_with_pictures_for_test(None);
    }

    pub(in crate::app) fn settle_representations_with_pictures_for_test(
        &mut self,
        pictures: Option<&mut crate::app::DocPics>,
    ) {
        self.settle_discard_for_test();
        if let Some(rx) = self.preview_reply.take() {
            self.resolved_preview = Some(
                rx.recv_timeout(std::time::Duration::from_secs(30))
                    .expect("project idle fence: diff Preview resolution"),
            );
            self.choose_effective();
        }
        if let Some(rx) = self.pending.take() {
            self.data = Some(
                rx.recv_timeout(std::time::Duration::from_secs(30))
                    .expect("project idle fence: diff raw document")
                    .unwrap(),
            );
        }
        while self.markdown.pending_for(&self.request.path) {
            let result = self
                .markdown_rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("project idle fence: diff Markdown representation");
            self.adopt_markdown(result);
        }
        self.settle_media_for_test(pictures);
    }
}
