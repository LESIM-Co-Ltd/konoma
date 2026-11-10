//! Read-only inspection for integration tests, including the app-only feature matrix.
use super::{DiffPresentation, DiffScreenState};

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
    pub(in crate::app) fn settle_representations_for_test(&mut self) {
        self.settle_representations_with_pictures_for_test(None);
    }

    pub(in crate::app) fn settle_representations_with_pictures_for_test(
        &mut self,
        pictures: Option<&mut crate::app::DocPics>,
    ) {
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
