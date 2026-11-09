//! Raw-cache injection only; the bridge still dispatches and draws through the real host.
use super::*;

impl App {
    pub(crate) fn seed_source_diff_for_test(&mut self, lines: Vec<crate::git::DiffLine>) {
        self.diff_cache = self
            .tab
            .preview_path
            .clone()
            .map(|path| DiffCache { path, lines });
    }
}
