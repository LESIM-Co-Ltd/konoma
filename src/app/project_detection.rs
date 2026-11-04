//! Per-tab project discovery. The UI only starts workers and takes their cached answers.

use super::App;

impl App {
    pub(super) fn ensure_project_detection(&mut self) {
        self.tab
            .projects
            .ensure(&self.tab.root, &self.cfg.plugin_disabled());
    }

    pub(crate) fn poll_projects(&mut self) -> bool {
        self.ensure_project_detection();
        match self.tab.projects.poll() {
            Some(false) => true,
            Some(true) => {
                self.flash = Some(
                    crate::i18n::tr(self.lang, crate::i18n::Msg::ProjectDetectionFailed).into(),
                );
                true
            }
            None => false,
        }
    }
}
