//! Standalone host adapter. Full diff screens pass their real state through `diff_presentation`.
use super::*;
use crate::host::project::diff_presentation::*;
use konoma_host_core::help::{default_help, default_label};
use konoma_host_core::help_msg::DiffMsg;
use konoma_host_core::menu::Item;

impl AppKeys {
    pub(crate) fn for_file_diff(config: &konoma_host_core::shared::SharedConfig) -> Self {
        let mut keys = Self::new(&config.keys, &config.ui.keys);
        keys.keys = crate::host::project::diff_keys::file_diff_keys(
            scheme_from_str(&config.ui.keys),
            &config.keys,
        );
        keys.with_lang(Lang::parse(&config.ui.lang))
            .with_wrap(config.ui.wrap)
    }
    /// Resolve shared keys for one frame of a project diff. Labels, availability and host
    /// capabilities come from the caller; the presentation owns every state-dependent choice.
    pub fn diff_presentation(&self, values: &DiffPresentationValues) -> DiffPresentation {
        let lookup = CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Diff,
        };
        DiffPresentation::build_with_lookup(
            &values.state,
            values.capabilities,
            &values.bindings,
            &values.words,
            &lookup,
        )
    }

    /// Compatibility for the generic context API, which represents only a source diff.
    /// Stateful DiffScreen callers use `diff_presentation` and retain its result for the frame.
    pub(super) fn source_diff(&self, paged: bool, scrolls: bool) -> DiffPresentation {
        let ctx = CoreContext::Diff;
        let launch = self.with_launch_facts(ctx, HelpFacts::default());
        let bindings = CoreAction::all().into_iter().filter_map(|action| {
            let op = DiffUiOp::from_core(HelpOp::Act(action))?;
            let mut label = launch
                .label
                .iter()
                .find(|(a, _)| *a == action)
                .map(|(_, l)| *l)
                .unwrap_or_else(|| default_label(ctx, action))
                .map(|m| m.tr(self.lang).to_string());
            let mut help = launch
                .help
                .iter()
                .find(|(a, _)| *a == action)
                .map(|(_, h)| *h)
                .unwrap_or_else(|| default_help(ctx, action))
                .map(|m| m.tr(self.lang).to_string());
            if op == DiffUiOp::Layout {
                label = Some(CoreMsg::FtDiffLayout.tr(self.lang).into());
                help = Some(DiffMsg::GitLayout.tr(self.lang).into());
            }
            if op == DiffUiOp::Back {
                label = Some(CoreMsg::GitBack.tr(self.lang).into());
            }
            let enabled = match op {
                DiffUiOp::Edit => self.launch.edit_acts(),
                DiffUiOp::OpenExternal => self.launch.open,
                _ => true,
            };
            Some(DiffBinding {
                op,
                keys: vec![],
                enabled,
                label,
                help,
                launch: matches!(op, DiffUiOp::Edit | DiffUiOp::OpenExternal),
                layers: vec![],
            })
        });
        let mut bindings: Vec<_> = bindings.collect();
        for op in [DiffUiOp::Cancel, DiffUiOp::Options] {
            bindings.push(DiffBinding {
                op,
                keys: vec![],
                enabled: true,
                label: None,
                help: None,
                launch: false,
                layers: vec![],
            });
        }
        let word = |m: CoreMsg| m.tr(self.lang).to_string();
        let words = DiffPresentationWords {
            view: word(CoreMsg::PreviewGitDiff),
            movement: word(CoreMsg::HelpMove),
            launch: word(CoreMsg::HelpLaunch),
            tabs: String::new(),
            global: word(CoreMsg::HelpGlobal),
            words: vec![
                (DiffWord::Scroll, word(CoreMsg::Scroll)),
                (DiffWord::Hscroll, word(CoreMsg::HintHscroll)),
                (DiffWord::Back, word(CoreMsg::GitBack)),
                (DiffWord::Options, word(CoreMsg::HintOptions)),
                (
                    DiffWord::HorizontalEnds,
                    DiffMsg::GitHScrollEnds.tr(self.lang).into(),
                ),
                (
                    DiffWord::BackHelp,
                    DiffMsg::BackToGitView.tr(self.lang).into(),
                ),
            ],
            dynamic: vec![],
        };
        self.diff_presentation(&DiffPresentationValues {
            state: DiffPresentationState {
                surface: if scrolls {
                    DiffSurface::Source
                } else {
                    DiffSurface::Summary
                },
                can_page: paged,
                ..Default::default()
            },
            capabilities: DiffCapabilities {
                project: false,
                view: false,
                blocks: false,
                discard: false,
                ..Default::default()
            },
            bindings,
            words,
        })
    }

    pub(super) fn diff_help_menus(&self, p: &DiffPresentation) -> Vec<DiffHelpMenu> {
        let lookup = CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Diff,
        };
        let mut out = Vec::new();
        for menu in [CoreMenu::Options, CoreMenu::Copy] {
            let Some(key) = lookup.keys_of(HelpOp::Menu(menu)).first().copied() else {
                continue;
            };
            let Some(view) = menu_view(&self.keys, CoreContext::Diff, menu, self.lang, false)
            else {
                continue;
            };
            let mut items: Vec<_> = view
                .items
                .into_iter()
                .enumerate()
                .map(|(index, item)| Item {
                    key: item.key,
                    action: DiffUiOp::from_core(HelpOp::Act(item.action))
                        .unwrap_or(DiffUiOp::Host(index)),
                    label: item.label,
                })
                .collect();
            if menu == CoreMenu::Options {
                let declaration: Vec<_> = items
                    .into_iter()
                    .map(|item| Item {
                        key: item.key,
                        action: item.action,
                        label: crate::host::acts::FixedMenuLabel {
                            declared: Some(item.label),
                            fallback: None,
                        },
                    })
                    .collect();
                items = p.options_menu(&declaration);
            }
            out.push((
                key,
                DiffHelpMenu {
                    title: view.title,
                    prefix: konoma_host_core::help::help_key(key),
                    items,
                },
            ));
        }
        out.sort_by_key(|(key, _)| konoma_host_core::help::display_order(*key));
        out.into_iter().map(|(_, menu)| menu).collect()
    }
}
