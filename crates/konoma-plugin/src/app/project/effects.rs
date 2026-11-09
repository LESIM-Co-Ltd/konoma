//! Ordered effect delivery; tabs/tree and host navigation conversion stay on konoma's host.

use super::*;
use crate::Outcome;

/// The two later project executors have distinct, typed entry points.
enum ProjectNavigation {
    OpenDiff {
        path: PathBuf,
    },
    SwitchRoot {
        root: PathBuf,
        message: Option<crate::Tr>,
    },
}

impl ProjectNavigation {
    fn dispatch(self, prepared: &mut PreparedProjects) {
        match self {
            Self::OpenDiff { path } => {
                let _ = path;
            }
            Self::SwitchRoot { root, message } => {
                let _ = (root, message);
            }
        }
        unsupported(prepared);
    }
}

fn unsupported(prepared: &mut PreparedProjects) {
    prepared.notice = Some(
        crate::t!(
            en: "this action is not supported in the standalone app",
            ja: "この操作は単体アプリでは使えません",
        )
        .get(prepared.lang)
        .into(),
    );
}

impl ProjectLoopState {
    pub(super) fn apply(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        update: ProjectUpdate<StandaloneInvocation>,
    ) -> bool {
        for event in update.events {
            match event {
                ProjectEvent::Watch(key, paths) => {
                    prepared
                        .detection
                        .set_watch_paths(&key.plugin, &key.root, &key.marker, paths)
                }
                ProjectEvent::Effect(effect) => {
                    let update = self.with_host(info, |host, context| {
                        host.apply_effect(context, info, effect)
                    });
                    if self.apply(prepared, info, update) {
                        return true;
                    }
                }
                ProjectEvent::EnsureView => self.ensure(info),
                ProjectEvent::Error(error) | ProjectEvent::ChipError(_, error) => {
                    prepared.notice = Some(error.message.get(prepared.lang).into());
                }
                ProjectEvent::Retry(invocation) => {
                    let Some(nav) = &self.screen else { continue };
                    let current = ProjectOrigin {
                        context: info.id,
                        project: nav.project.clone(),
                        root: info.root.clone(),
                        screen: nav.frame().map(|f| f.key.clone()),
                        surface: info.surface.clone(),
                    };
                    let blockers = ProjectPromptBlockers {
                        filtering: nav.frame().is_some_and(|f| f.filtering),
                        ..Default::default()
                    };
                    self.with_host(info, |host, _| {
                        host.reopen_retry(invocation, &current, blockers)
                    });
                }
            }
        }
        if update.dispatch == ProjectDispatch::LeaveProject {
            if !self.with_host(info, |host, context| host.pop_screen(context)) {
                return true;
            }
            self.ensure(info);
        }
        for command in update.commands {
            match command {
                ProjectCommand::EnsureView => self.ensure(info),
                ProjectCommand::LayoutChanged(layout) => prepared.settings.diff_layout = layout,
                ProjectCommand::Dispatch(id) => self.dispatch_act(prepared, info, id),
                ProjectCommand::Label(label) => prepared.notice = Some(label),
                ProjectCommand::Flash(message) => prepared.notice = Some(message.get(prepared.lang).into()),
                ProjectCommand::Copy(text) => prepared.notice = Some(match konoma_host_core::clipboard::set_text(&text) {
                    Ok(()) => konoma_host_core::copy::copied_preview_notice(prepared.keys.lang(), &text),
                    Err(error) => konoma_host_core::copy::copy_failed_notice(prepared.keys.lang(), &error),
                }),
                ProjectCommand::UnknownScreen => prepared.notice = Some(crate::t!(en: "project screen is not declared", ja: "プロジェクトの画面が宣言されていません").get(prepared.lang).into()),
                ProjectCommand::StartScreen { project, screen } => {
                    self.screen = Some(ProjectHost::start_screen(project, screen));
                    self.ensure(info);
                }
                ProjectCommand::Navigation { outcome, .. } => match outcome {
                    Outcome::OpenFile { .. } | Outcome::OpenTab { .. } => unsupported(prepared),
                    Outcome::OpenDiff { path } => ProjectNavigation::OpenDiff { path }.dispatch(prepared),
                    Outcome::SwitchRoot { root, message } => ProjectNavigation::SwitchRoot { root, message }.dispatch(prepared),
                    // The shared effect adapter only emits these four navigation outcomes.
                    _ => unreachable!("shared project navigation command"),
                },
                ProjectCommand::Notice(notice) => prepared.notice = Some(match notice {
                    ProjectNotice::Busy => crate::t!(en: "project action is still running", ja: "プロジェクトの操作を実行中です"),
                    ProjectNotice::ReadOnly => crate::t!(en: "this plugin is set to read-only ([plugins.<id>] read_only = true)", ja: "このプラグインは読み取り専用です([plugins.<id>] read_only = true)"),
                    ProjectNotice::SelectionChanged => crate::t!(en: "selected project row changed; action cancelled", ja: "選択した行が変わったため操作を中止しました"),
                }.get(prepared.lang).into()),
                // No tree-status owner or tab/menu executor exists on this surface.
                ProjectCommand::TreeStatus | ProjectCommand::NewTab => {}
            }
        }
        if let Some(prompt) = update.prompt {
            match prompt {
                ProjectPromptUpdate::InvocationReady(invocation) => {
                    self.execute_act(prepared, info, invocation)
                }
                ProjectPromptUpdate::ChosenProject(project) => {
                    if let Some(screen) =
                        self.with_host(info, |host, _| host.first_screen(&project, info.id))
                    {
                        self.screen = Some(ProjectHost::start_screen(project, screen));
                        self.ensure(info);
                    }
                }
                ProjectPromptUpdate::Cancelled { notice: true } => {
                    prepared.notice = Some(
                        crate::t!(en: "canceled", ja: "取消")
                            .get(prepared.lang)
                            .into(),
                    )
                }
                _ => {}
            }
        }
        false
    }
}
