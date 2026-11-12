//! Standalone facts and indexed bindings for the one shared project presentation.

use super::*;
use crate::host::acts::ActBinding;
use crate::host::project::presentation::*;
use konoma_host_core::help::CoreSection;
use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::keymap::CoreMenu;

pub(super) struct Presentation {
    pub project: ProjectPresentation,
    pub actions: Vec<ProjectActionId>,
    pub help: Vec<CoreSection>,
}

impl ProjectLoopState {
    pub(super) fn presentation(
        &self,
        prepared: &PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
    ) -> Option<Presentation> {
        let nav = self.screen.as_ref()?;
        let runtime = self.runtimes.get(&nav.project)?;
        let manifest = prepared.registry.manifest(&nav.project.plugin)?;
        let (bound, _) = AppActs::from_config(
            &nav.project.plugin,
            manifest,
            &prepared.config.keys,
            &prepared.config.plugins,
        );
        let actions = self.actions(prepared, info);
        let mut ids = Vec::new();
        let bindings: Vec<_> = bound
            .bindings
            .into_iter()
            .filter_map(|b| {
                let action = actions.iter().find(|a| a.id.action == b.act)?;
                if !b.context.as_deref().is_none_or(|c| c == "canvas") {
                    return None;
                }
                let index = ids.len();
                ids.push(action.id.clone());
                Some(ProjectPresentationBinding {
                    enabled: crate::host::project::actions::act_enabled(Some(runtime), &b.act)
                        && self
                            .act_facts(prepared, info, &action.id)?
                            .policy
                            .allow_write(b.writes),
                    declared_enter: action.declaration.key.as_deref() == Some("Enter"),
                    binding: ActBinding {
                        act: index,
                        key: action.key,
                        copy_key: b.copy_key,
                        hint_group: b.hint_group,
                        context: b.context,
                        label: b.label,
                        help: b.help,
                        footer: b.footer,
                        writes: b.writes,
                    },
                })
            })
            .collect();
        let content = self.content(info);
        let declaration = &runtime.project.declaration;
        let motion_keys = |motions: &[Motion]| {
            motions
                .iter()
                .flat_map(|m| {
                    self.keys
                        .keys_of(CoreContext::Text, CoreAction::Navigate(*m))
                })
                .collect::<Vec<_>>()
        };
        let mut candidates = motion_keys(&[Motion::Right]);
        if let Some(table) = self.keys.types.get(&CoreViewType::List) {
            candidates.extend(table.keys().copied());
        }
        candidates.push(KeyPress::key(KeyCode::Right));
        let mut right_aliases = Vec::new();
        for key in candidates {
            if crate::host::project::input::list_right(
                true,
                key.code,
                self.keys
                    .types
                    .get(&CoreViewType::List)
                    .and_then(|t| t.get(&key))
                    .map(|b| matches!(b, CoreBinding::Run(CoreAction::Navigate(Motion::Right)))),
                self.keys.resolve(CoreContext::Text, key)
                    == CoreResolution::Action(CoreAction::Navigate(Motion::Right)),
            ) && !right_aliases.contains(&key)
            {
                right_aliases.push(key);
            }
        }
        let open = bindings
            .iter()
            .find(|b| b.enabled && b.binding.key == Some(KeyPress::key(KeyCode::Enter)))
            .map(|b| ProjectOpenFacts {
                label: b.binding.label.get(prepared.lang).into(),
                right_aliases,
            });
        let mut facts = ProjectPresentationFacts {
            content: content.as_deref().map(|c| match c {
                ProjectContent::List(_) => ProjectPresentationContent::List,
                ProjectContent::Lines(_) => ProjectPresentationContent::Lines,
                ProjectContent::Diff(_) => ProjectPresentationContent::Diff,
            }),
            marks: nav.marks_enabled(Some(declaration)),
            filter: nav.filter_enabled(Some(declaration)),
            pushed: nav.pushed(Some(declaration)),
            picker: nav.picker(),
            graph_jj: nav.graph_jj(),
            vcs_jj: nav.vcs_jj(),
            row_target: content
                .as_deref()
                .and_then(|c| {
                    nav.selected_index(c, self.viewport, prepared.settings.diff_layout)
                        .and_then(|i| nav.row_target(c, i))
                })
                .is_some(),
            open,
            motions: ProjectMotionKeys {
                scroll: motion_keys(&[Motion::Down, Motion::Up]),
                hscroll: motion_keys(&[Motion::Left, Motion::Right]),
                ends: motion_keys(&[Motion::Top, Motion::Bottom]),
                diff_layout: self
                    .keys
                    .keys_of(CoreContext::Diff, CoreAction::CycleDiffLayout),
                half_page: motion_keys(&[Motion::HalfDown, Motion::HalfUp]),
                page: motion_keys(&[Motion::PageDown, Motion::PageUp]),
                line_ends: motion_keys(&[Motion::LineHome, Motion::LineEnd]),
                left: motion_keys(&[Motion::Left]),
                // The standalone app has no per-screen keys: `q` and `/` are always bound.
                back: vec![KeyPress::ch('q')],
                filter: vec![KeyPress::ch('/')],
                help: self
                    .keys
                    .keys_of(CoreContext::Canvas, CoreAction::ToggleHelp),
            },
            graph_scroll: {
                let (columns, _) =
                    konoma_host_core::help::zip_columns(&[Motion::Down, Motion::Up], |m| {
                        let mut keys = motion_keys(&[m]);
                        for binding in fixed_keys(CoreContext::Text) {
                            if binding.op == PreviewOp::Core(CoreAction::Navigate(m))
                                && !binding.alias
                                && !keys.contains(&binding.key)
                            {
                                keys.push(binding.key);
                            }
                        }
                        keys
                    });
                columns.into_iter().flatten().collect()
            },
            copy_keys: if bindings
                .iter()
                .any(|b| b.enabled && b.binding.copy_key.is_some())
            {
                self.keys.menu_keys_of(CoreContext::Canvas, CoreMenu::Copy)
            } else {
                Vec::new()
            },
            filter_label: declaration
                .screens
                .iter()
                .find(|s| s.id == nav.frame().unwrap().key.screen)
                .and_then(|s| s.filter_label.as_ref())
                .map(|l| l.get(prepared.lang).into()),
            ..Default::default()
        }
        .with_declared_screens(
            &declaration.screens,
            &nav.frame().unwrap().key.screen,
            prepared.lang,
        );
        // `!` launches the tool on every standalone screen; the footer shows it where the host
        // does (the version-control changes screen only — not the home screen of another
        // project kind or of a contract plugin), and only when this program can launch at all.
        if self.launch_enabled
            && declaration.version_control
            && facts.home
            && nav.frame().is_some_and(|f| f.key.screen == "changes")
        {
            use konoma_host_core::keymap::{CoreBinding, HostOp};
            facts.tool_keys = crate::host::project::diff_keys::file_diff_keys(
                scheme_from_str(&prepared.config.ui.keys),
                &prepared.config.keys,
            )
            .bound(CoreContext::Diff)
            .into_iter()
            .filter(|(_, b)| matches!(b, CoreBinding::Host(HostOp::GitLaunchTool)))
            .map(|(key, _)| key)
            .collect();
        }
        let t = |en, ja| if prepared.lang == "ja" { ja } else { en };
        let msg = |m: CoreMsg| m.tr(prepared.keys.lang());
        // `q` (and `h`) on a root screen leaves the project, which ends this program: only a
        // pushed screen or a project opened over a preview goes back.
        let back = if nav.stack.len() > 1 || self.preview_project {
            t("back", "戻る")
        } else {
            t("quit", "終了")
        };
        let words = ProjectPresentationWords {
            language: prepared.lang,
            screens: t("screens", "画面"),
            move_list: t("move", "移動"),
            scroll: msg(CoreMsg::Scroll),
            mark: t("mark", "マーク"),
            filter: t("filter", "絞り込み"),
            back,
            help: msg(CoreMsg::HintHelp),
            new_tab: t("new tab", "別タブ"),
            copy: msg(CoreMsg::HintCopy),
            actions: msg(CoreMsg::WkActionsTitle),
            hscroll: msg(CoreMsg::HintHscroll),
            ends: msg(CoreMsg::HintEnds),
            diff_layout: msg(CoreMsg::FtDiffLayout),
            cycle: t("Cycle project screens", "プロジェクトの画面を切り替える"),
            // Host rule: src/app/project_screens.rs supplies Msg::ProjectBack unconditionally.
            // §7-5:967 says "root の画面の `q` は終了、重ねた画面の `q` は戻る。"
            // It does not specify a VCS-only wording rule. U8b fix-2 item 3 requires retaining
            // this compatibility restriction when unconditional wording breaks the existing
            // project_open_aliases_and_root_pushed_wording_match_actions expectation.
            // Non-VCS pushed screens therefore retain "back" / "戻る", unlike the host below.
            pop: if declaration.version_control && (facts.pushed || self.preview_project) {
                t(
                    "Pop this screen or return to the previous view",
                    "画面を一つ戻るか元の表示に戻る",
                )
            } else {
                back
            },
            apply: t("apply", "適用"),
            cancel: t("cancel", "取消"),
            nav: t("nav", "移動"),
            search: t("search", "検索"),
            half_page: msg(CoreMsg::HelpHalfPage),
            page: msg(CoreMsg::HintPage),
            line_ends: msg(CoreMsg::HintLineEnds),
            close: msg(CoreMsg::FtClose),
            tool: t("tool", "ツール"),
        };
        let project = ProjectPresentation::build(
            &facts,
            ProjectCapabilities {
                can_open_tab: false,
            },
            &bindings,
            &words,
        );
        let screens = declaration
            .screens
            .iter()
            .filter_map(|s| Some((s.key?.to_string(), s.title.get(prepared.lang).into())))
            .collect::<Vec<_>>();
        let help = project.help_sections(
            &ProjectHelpWords {
                plugin: manifest.name.get(prepared.lang),
                project: msg(CoreMsg::ProjectTitle),
                picker: t("Graph branches", "グラフのブランチ"),
                graph: if facts.graph_jj == Some(true) {
                    t("jj graph", "jj グラフ")
                } else {
                    t("Git graph", "Git グラフ")
                },
                new_tab: t("Open in a new tab", "別タブで開く"),
                move_list: t("move", "移動"),
                move_commit: t(
                    "move (commit / uncommitted)",
                    "移動 (コミット / 未コミット)",
                ),
                ends: msg(CoreMsg::TopBottom),
                apply: words.apply,
                cancel: words.cancel,
                filter: t("filter by name", "名前で絞り込み"),
                mark: t(
                    "Toggle the selected row's mark",
                    "選択行のマークを切り替える",
                ),
                search: t(
                    "filter the rows by the text they show",
                    "行の表示文字列で絞り込み",
                ),
                refresh: t("refresh listing & git status", "一覧と git status を再読込"),
                movement: msg(CoreMsg::HelpMove),
                screens: if facts.vcs_jj == Some(true) {
                    t(
                        "cycle changes/log/graph/bookmarks",
                        "画面を巡回: 変更/ログ/グラフ/ブックマーク",
                    )
                } else {
                    t(
                        "cycle changes/log/graph/branches/worktrees",
                        "画面を巡回: 変更/ログ/グラフ/ブランチ/worktree",
                    )
                },
                back_to_changes: back,
                cycle: words.cycle,
                back,
                scroll: words.scroll,
                copy: words.copy,
                actions: words.actions,
            },
            &screens,
        );
        Some(Presentation {
            project,
            actions: ids,
            help,
        })
    }
}
