//! Per-tab project discovery. The UI only starts workers and takes their cached answers.

use super::App;

impl App {
    pub(super) fn ensure_project_detection(&mut self) {
        crate::plugins::projects::ensure(
            &mut self.tab.projects,
            &self.tab.root,
            &self.cfg.plugin_disabled(),
        );
    }

    pub(crate) fn poll_projects(&mut self) -> bool {
        self.ensure_project_detection();
        let previously_ready = !self.tab.projects.pending();
        let keys = |projects: &[konoma_plugin::host::project::detection::MatchedProject]| {
            projects
                .iter()
                .map(super::project_screens::ProjectKey::from)
                .collect::<Vec<_>>()
        };
        let before = keys(&self.tab.projects.projects);
        let detection = self.tab.projects.poll();
        let after = keys(&self.tab.projects.projects);
        // An adopted answer retires the projects it no longer lists, failed or not (a watch failure
        // after a successful scan still carries the new list). A failed scan keeps the previous
        // projects, so nothing differs then.
        if previously_ready
            && (detection == Some(false) || (detection.is_some() && before != after))
        {
            // A project that is no longer listed is retired, whatever else changed: its screens
            // close and its runtime goes. Only projects still listed are invalidated.
            for gone in before.iter().filter(|key| !after.contains(key)) {
                self.retire_project(gone);
            }
            for project in &self.tab.projects.projects {
                konoma_plugin::host::project::ProjectHost::new(
                    &mut self.project_runtimes,
                    &mut self.project_dialog,
                    &mut self.project_replaying,
                )
                .invalidate(
                    &super::project_screens::ProjectKey::from(project),
                    konoma_plugin::host::project::ProjectInvalidation::Detection,
                );
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
