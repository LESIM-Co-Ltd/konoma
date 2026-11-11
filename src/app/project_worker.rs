//! Host inputs and App glue for the SDK's serial project owner.

use konoma_plugin::host::project::{ProjectEnv, ProjectMessages};
use konoma_plugin::project::Chip;
#[cfg(test)]
use konoma_plugin::project::ProjectView;
use konoma_plugin::view::ViewReq;
use konoma_plugin::TermCaps;

use super::project_screens::{ProjectEffect, ProjectInvocation, ProjectKey, ScreenKey};
use super::PathStyle;

#[cfg(test)]
pub(super) type ProjectJob = konoma_plugin::host::project::ProjectJob<ProjectEffect>;
pub(crate) type ProjectRuntime =
    konoma_plugin::host::project::ProjectRuntime<ProjectEffect, ProjectInvocation>;

fn declaration_chip(
    project: &crate::plugins::projects::MatchedProject,
    lang: &str,
) -> Option<Chip> {
    project
        .declaration
        .chip
        .as_ref()
        .map(|chip| Chip::new(chip.get(lang)))
}

fn project_messages(lang: crate::i18n::Lang) -> ProjectMessages {
    use crate::i18n::{tr, Msg};
    ProjectMessages {
        worker_failed: tr(lang, Msg::ProjectWorkerFailed).into(),
        no_changes: tr(lang, Msg::GitNoChanges).into(),
        mermaid_caption: tr(lang, Msg::MermaidCaption).into(),
        view_unsupported: tr(lang, Msg::ProjectViewUnsupported).into(),
        outside_read_dirs: tr(lang, Msg::ImageReasonOutsideReadDirs).into(),
        invalid_watch_paths: konoma_plugin::t!(en: "Invalid project watch paths", ja: "プロジェクトの監視パスが不正です"),
    }
}

impl super::App {
    pub(super) fn project_context_info(
        &self,
        tab: &super::PerTab,
    ) -> konoma_plugin::host::project::ProjectContextInfo<super::project_screens::ProjectSurface>
    {
        konoma_plugin::host::project::ProjectContextInfo {
            id: tab.id,
            root: tab.root.clone(),
            open_dir: tab.open_dir.clone(),
            home: crate::plugins::read_scope::home_dir(),
            icons: self.cfg.ui.icons,
            viewport: konoma_plugin::host::project::ProjectViewport {
                cols: tab.preview_cols,
                rows: tab.preview_viewport,
            },
            surface: super::project_screens::ProjectSurface::from_tab(tab),
        }
    }

    pub(super) fn project_request(&self, key: &ScreenKey) -> Option<ViewReq> {
        let tab = std::iter::once(&self.tab)
            .chain(self.tabs.iter())
            .find(|tab| tab.id == key.tab)?;
        konoma_plugin::host::project::effects::request(&self.project_context_info(tab), key)
    }

    pub(super) fn start_project_runtime(
        &mut self,
        project: crate::plugins::projects::MatchedProject,
    ) {
        if !self.project_is_enabled(&project.plugin) {
            return;
        }
        let key = ProjectKey::from(&project);
        if self.project_runtimes.contains_key(&key) {
            return;
        }
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let Some(plugin) = registry.plugin(&project.plugin) else {
            return;
        };
        let mut settings = crate::plugins::effective_settings(&self.cfg, &project.plugin);
        if settings.get("path_style").is_none() {
            settings.set(
                "path_style",
                konoma_plugin::Value::Str(
                    match self.path_style {
                        PathStyle::Relative => "relative",
                        PathStyle::Home => "home",
                        PathStyle::Full => "full",
                    }
                    .into(),
                ),
            );
        }
        let lang = self.plugin_lang().to_owned();
        let chip = declaration_chip(&project, &lang);
        let theme = self.cfg.ui.theme.code_theme.clone();
        let term = TermCaps::new().colors(self.plugin_colors);
        let table = self.cfg.plugins.get(&project.plugin).cloned();
        let standard = crate::plugins::read_scope::is_host_owned(&registry, &project.plugin);
        let home = crate::plugins::read_scope::home_dir();
        let messages = project_messages(self.lang);
        let root = project.root.clone();
        let shown = root.join(&project.marker);
        let runtime = ProjectRuntime::start(
            project.clone(),
            plugin,
            registry
                .manifest(&project.plugin)
                .map(|m| m.actions.iter().map(|a| a.id.clone()).collect())
                .unwrap_or_default(),
            chip,
            move || ProjectEnv {
                settings,
                lang,
                theme,
                term,
                read_scope: crate::plugins::read_scope::scope_for(
                    standard,
                    table.as_ref(),
                    &root,
                    &shown,
                    home.as_deref(),
                ),
                messages,
            },
            |effect, outcome| effect.outcome = Some(outcome),
        );
        konoma_plugin::host::project::ProjectHost::new(
            &mut self.project_runtimes,
            &mut self.project_dialog,
            &mut self.project_replaying,
        )
        .ensure_project(project, runtime);
    }
}

#[cfg(test)]
#[path = "project_worker_tests.rs"]
mod tests;
