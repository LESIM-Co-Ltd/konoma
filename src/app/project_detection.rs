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
        let previously_ready = !self.tab.projects.pending();
        let detection = self.tab.projects.poll();
        if previously_ready && detection == Some(false) {
            for project in &self.tab.projects.projects {
                if let Some(runtime) = self
                    .project_runtimes
                    .get_mut(&super::project_screens::ProjectKey::from(project))
                {
                    runtime.dirty.extend(runtime.views.keys().cloned());
                    runtime.error = None;
                    runtime.chip_dirty = true;
                    runtime.invalidated |= runtime.busy;
                }
            }
        }
        let changed = match detection {
            Some(false) => true,
            Some(true) => {
                self.flash = Some(
                    crate::i18n::tr(self.lang, crate::i18n::Msg::ProjectDetectionFailed).into(),
                );
                true
            }
            None => false,
        };
        let changed = self.poll_project_jobs() || changed;
        self.replay_project_keys() || changed
    }
}
