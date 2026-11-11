//! Value-based chrome for a file preview owned by a project diff.
use konoma_host_core::{
    help::{self, CoreRow, CoreSec, CoreSection, HelpFacts, HelpOp, KeyLayer, KeyLookup},
    keymap::{CoreContext, HostOp, KeyPress},
    lang::Lang,
    preview_plan::{footer_plan, PreviewOp, PreviewState, PvDyn, PvFoot},
};

/// Resolved bindings and availability supplied by a host, without exposing its action types.
pub struct PreviewBinding {
    pub op: PreviewOp,
    pub keys: Vec<KeyPress>,
    pub enabled: bool,
    pub provided: bool,
    pub label: Option<String>,
    pub help: Option<String>,
    pub launch: bool,
    pub layers: Vec<(KeyPress, KeyLayer)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PreviewSection {
    View,
    Move,
    Launch,
    Tabs,
    Global,
}

pub struct PreviewHelpRow {
    pub keys_text: String,
    pub description: String,
    pub section: PreviewSection,
}

/// Catalogue words needed by reference rows. A host with no corresponding capability supplies
/// no binding; those rows never consume its word.
#[derive(Default)]
pub struct PreviewHelpWords {
    pub search: String,
    pub table_search: String,
    pub bookmarks: String,
    pub zoom: String,
    pub tasks: String,
    pub details: String,
    pub tool: String,
    pub tabs: String,
}

struct Row {
    groups: Vec<Vec<PreviewOp>>,
    core: Option<CoreRow>,
    description: Option<String>,
    ungated: bool,
}

fn row(ops: &[PreviewOp]) -> Row {
    Row {
        groups: vec![ops.to_vec()],
        core: None,
        description: None,
        ungated: false,
    }
}

fn core_row(core: CoreRow) -> Row {
    Row {
        core: Some(core),
        ..row(&[])
    }
}

fn rows(context: CoreContext, state: &PreviewState, words: &PreviewHelpWords) -> Vec<Row> {
    use konoma_host_core::{
        keymap::{CoreAction as A, HostOp as H},
        preview_plan::PreviewOp as O,
    };
    let host = |op| O::Host(op);
    let core = |op| O::Core(op);
    let described = |ops: &[O], word: &str| Row {
        description: Some(word.into()),
        ..row(ops)
    };
    let file_jump = || row(&[host(H::PreviewFileNext), host(H::PreviewFilePrev)]);
    let bookmarks = || described(&[host(H::MarkSet), host(H::MarkJump)], &words.bookmarks);
    let mut rows = Vec::new();
    if state.text_missing {
        rows.extend([file_jump(), bookmarks()]);
    } else {
        match context {
            CoreContext::Text | CoreContext::Document => {
                rows.extend([
                    core_row(CoreRow::Scroll),
                    core_row(CoreRow::Ends),
                    core_row(CoreRow::HScroll),
                    core_row(CoreRow::LineEnds),
                ]);
                rows.push(Row {
                    groups: vec![
                        vec![host(H::SearchStart)],
                        vec![host(H::SearchNext), host(H::SearchPrev)],
                    ],
                    description: Some(words.search.clone()),
                    ..row(&[])
                });
                rows.extend([
                    row(&[host(H::PreviewEnterVisual), host(H::PreviewEnterVisualLine)]),
                    row(&[host(H::PreviewCopySelectionRef)]),
                    core_row(CoreRow::PageTurn),
                    row(&[host(H::CycleRepresentation)]),
                    core_row(CoreRow::Outline),
                ]);
                if context == CoreContext::Document {
                    rows.push(row(&[host(H::LinkFocusNext), host(H::LinkFocusPrev)]));
                    for op in [O::LinkOpen, host(H::OpenLinkNewTab)] {
                        rows.push(Row {
                            ungated: true,
                            ..row(&[op])
                        });
                    }
                    if !state.office_document {
                        rows.push(Row {
                            ungated: true,
                            ..described(
                                &[
                                    core(A::ImageZoomIn),
                                    core(A::ImageZoomOut),
                                    core(A::ImageZoomReset),
                                ],
                                &words.zoom,
                            )
                        });
                        for word in [&words.tasks, &words.details] {
                            rows.push(Row {
                                ungated: true,
                                ..described(&[host(H::ToggleFocusedItem), O::LinkOpen], word)
                            });
                        }
                    }
                }
            }
            CoreContext::Image => {
                rows.extend([
                    core_row(CoreRow::Zoom),
                    core_row(CoreRow::ZoomReset),
                    core_row(CoreRow::Pan),
                    core_row(CoreRow::PageTurn),
                    core_row(CoreRow::Outline),
                    row(&[host(H::ImageReturnToDiff)]),
                ]);
            }
            CoreContext::Table => {
                rows.extend([
                    core_row(CoreRow::TableMove),
                    core_row(CoreRow::Ends),
                    core_row(CoreRow::TableCols),
                    core_row(CoreRow::PageTurn),
                    core_row(CoreRow::Outline),
                    row(&[host(H::CycleRepresentation)]),
                ]);
                rows.push(Row {
                    groups: vec![
                        vec![host(H::SearchStart)],
                        vec![host(H::SearchNext), host(H::SearchPrev)],
                    ],
                    description: Some(words.table_search.clone()),
                    ..row(&[])
                });
                rows.extend([
                    row(&[O::ToggleTableCell]),
                    core_row(CoreRow::PageKeys),
                    core_row(CoreRow::HalfKeys),
                ]);
            }
            CoreContext::Canvas => {
                rows.extend([
                    core_row(CoreRow::PageTurn),
                    core_row(CoreRow::Outline),
                    row(&[host(H::CycleRepresentation)]),
                ]);
            }
            _ => return rows,
        }
        rows.extend([file_jump(), bookmarks()]);
    }
    rows.extend([core_row(CoreRow::Edit), core_row(CoreRow::Open)]);
    if !state.text_missing {
        rows.extend([
            row(&[host(H::OpenGitView)]),
            described(&[host(H::GitLaunchTool)], &words.tool),
            row(&[host(H::Refresh)]),
        ]);
        if matches!(context, CoreContext::Text | CoreContext::Document) {
            rows.extend([core_row(CoreRow::PageKeys), core_row(CoreRow::HalfKeys)]);
        }
        rows.push(row(&[core(A::CyclePathStyle)]));
        if context == CoreContext::Canvas {
            rows.push(row(&[host(H::PreviewCopySelectionRef)]));
        }
    }
    rows.push(core_row(CoreRow::Back));
    rows
}

pub struct PreviewValues {
    pub context: CoreContext,
    pub state: PreviewState,
    pub bindings: Vec<PreviewBinding>,
    pub core: HelpFacts,
    pub search_status: Option<String>,
    pub change_status: Option<String>,
    pub copy_code: Option<String>,
}

pub struct PreviewToken {
    pub text: String,
    pub keys: Vec<KeyPress>,
    pub ops: Vec<PreviewOp>,
}

impl PreviewValues {
    pub fn options_menu<A: Copy>(
        &self,
        declaration: &[konoma_host_core::menu::Item<A, crate::host::acts::FixedMenuLabel>],
        op_of: impl Fn(A) -> Option<PreviewOp>,
        available: impl Fn(A) -> bool,
    ) -> Vec<konoma_host_core::menu::Item<A, String>> {
        use konoma_host_core::keymap::CoreAction;
        crate::host::acts::fixed_menu_items(
            declaration,
            |action| {
                available(action)
                    && match op_of(action) {
                        Some(PreviewOp::Core(CoreAction::ToggleWrap)) => {
                            matches!(self.context, CoreContext::Text | CoreContext::Document)
                        }
                        Some(PreviewOp::Core(CoreAction::CyclePathStyle)) => true,
                        Some(op) => self.binding(op).is_none_or(|b| b.provided && b.enabled),
                        None => true,
                    }
            },
            |label, _| label.clone(),
        )
    }

    pub fn help_rows(
        &self,
        words: &PreviewHelpWords,
        lookup: &dyn KeyLookup,
        lang: Lang,
    ) -> Vec<PreviewHelpRow> {
        rows(self.context, &self.state, words)
            .into_iter()
            .filter_map(|row| {
                if let Some(core) = row.core {
                    let output = help::render_row(core, self.context, &self.core, lookup, lang)?;
                    return Some(PreviewHelpRow {
                        keys_text: output.keys_text,
                        description: output.desc,
                        section: match output.sec {
                            CoreSec::View => PreviewSection::View,
                            CoreSec::Move => PreviewSection::Move,
                            CoreSec::Launch => PreviewSection::Launch,
                            CoreSec::Global => PreviewSection::Global,
                        },
                    });
                }
                let mut parts = Vec::new();
                let mut lead = None;
                let mut first_key = None;
                for group in row.groups {
                    let on: Vec<_> = group
                        .into_iter()
                        .filter(|op| {
                            self.binding(*op)
                                .is_some_and(|b| b.provided && (row.ungated || b.enabled))
                        })
                        .collect();
                    let (columns, zipped) = help::zip_columns(&on, |op| self.keys(op));
                    if columns.is_empty() {
                        continue;
                    }
                    if lead.is_none() {
                        lead = on.first().copied();
                        first_key = columns.first().and_then(|col| col.first()).copied();
                    }
                    let mut seen = Vec::new();
                    parts.push(
                        columns
                            .into_iter()
                            .map(|col| {
                                col.into_iter()
                                    .filter_map(|key| {
                                        let text = help::help_key(key);
                                        if seen.contains(&text) {
                                            None
                                        } else {
                                            seen.push(text.clone());
                                            Some(text)
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" / ")
                            })
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(if zipped { "  ·  " } else { " / " }),
                    );
                }
                let lead = lead?;
                let binding = self.binding(lead)?;
                let description = row.description.or_else(|| {
                    if self.state.returns_to_diff
                        && matches!(
                            lead,
                            PreviewOp::Host(
                                HostOp::CycleRepresentation | HostOp::ImageReturnToDiff
                            )
                        )
                    {
                        self.label(lead, lang)
                    } else {
                        binding.help.clone()
                    }
                })?;
                Some(PreviewHelpRow {
                    keys_text: parts.join("  "),
                    description,
                    section: if binding.launch {
                        PreviewSection::Launch
                    } else {
                        match first_key.and_then(|key| {
                            binding
                                .layers
                                .iter()
                                .find(|(k, _)| *k == key)
                                .map(|(_, layer)| *layer)
                        }) {
                            Some(KeyLayer::Global) => PreviewSection::Global,
                            Some(KeyLayer::Basic) => PreviewSection::Move,
                            _ => PreviewSection::View,
                        }
                    },
                })
            })
            .collect()
    }

    pub fn help_sections(
        &self,
        words: &PreviewHelpWords,
        lookup: &dyn KeyLookup,
        lang: Lang,
        extra: Vec<PreviewHelpRow>,
        plugin: Vec<CoreSection>,
        menus: Vec<CoreSection>,
    ) -> Vec<CoreSection> {
        use konoma_host_core::help_msg::CoreMsg;
        let mut rows = self.help_rows(words, lookup, lang);
        rows.extend(extra);
        let section = |kind, title: String| {
            let rows: Vec<_> = rows
                .iter()
                .filter(|r| r.section == kind)
                .map(|r| (r.keys_text.clone(), r.description.clone()))
                .collect();
            (!rows.is_empty()).then_some(CoreSection { title, rows })
        };
        let mut out = Vec::new();
        out.extend(section(
            PreviewSection::View,
            help::view_title(self.context).tr(lang).into(),
        ));
        out.extend(plugin);
        out.extend(section(
            PreviewSection::Move,
            CoreMsg::HelpMove.tr(lang).into(),
        ));
        out.extend(section(
            PreviewSection::Launch,
            CoreMsg::HelpLaunch.tr(lang).into(),
        ));
        out.extend(menus);
        out.extend(section(PreviewSection::Tabs, words.tabs.clone()));
        out.extend(section(
            PreviewSection::Global,
            CoreMsg::HelpGlobal.tr(lang).into(),
        ));
        out.retain(|section| !section.rows.is_empty());
        out
    }
    fn binding(&self, op: PreviewOp) -> Option<&PreviewBinding> {
        self.bindings.iter().find(|b| b.op == op)
    }

    fn enabled(&self, op: PreviewOp) -> bool {
        self.binding(op).is_some_and(|b| b.enabled)
    }

    fn keys(&self, op: PreviewOp) -> Vec<KeyPress> {
        self.binding(op).map_or_else(Vec::new, |b| b.keys.clone())
    }

    pub(crate) fn label(&self, op: PreviewOp, lang: Lang) -> Option<String> {
        // A diff's Preview always steps back into the diff. Both hosts use this decision,
        // independently of the plugin's ordinary representation cycle.
        if self.state.returns_to_diff
            && matches!(
                op,
                PreviewOp::Host(HostOp::CycleRepresentation | HostOp::ImageReturnToDiff)
            )
        {
            return Some(
                if lang == Lang::Jp {
                    "diff に戻る"
                } else {
                    "back to diff"
                }
                .into(),
            );
        }
        self.binding(op).and_then(|b| b.label.clone())
    }

    pub(crate) fn provided_op(&self, op: PreviewOp) -> bool {
        self.binding(op).is_some_and(|b| b.provided && b.enabled)
    }

    pub fn footer(&self, lookup: &dyn KeyLookup, lang: Lang) -> Vec<PreviewToken> {
        let mut out = Vec::new();
        for entry in footer_plan(self.context, &self.state) {
            let (ops, label, union) = match entry {
                PvFoot::Core(foot) => {
                    if let Some(t) = help::render_foot(foot, self.context, &self.core, lookup, lang)
                    {
                        out.push(PreviewToken {
                            text: t.text,
                            keys: t.keys,
                            ops: t
                                .ops
                                .into_iter()
                                .filter_map(|op| match op {
                                    HelpOp::Act(a) => Some(PreviewOp::Core(a)),
                                    HelpOp::Cancel => Some(PreviewOp::Cancel),
                                    HelpOp::Menu(_) => None,
                                })
                                .collect(),
                        });
                    }
                    continue;
                }
                PvFoot::Dyn(dynamic) => {
                    let text = match dynamic {
                        PvDyn::SearchStatus => &self.search_status,
                        PvDyn::ChangeStatus => &self.change_status,
                        PvDyn::CopyCode => &self.copy_code,
                    };
                    if let Some(text) = text {
                        out.push(PreviewToken {
                            text: text.clone(),
                            keys: vec![],
                            ops: vec![],
                        });
                    }
                    continue;
                }
                PvFoot::Op(op) => (vec![op], self.label(op, lang), false),
                PvFoot::As(op, word) => (vec![op], Some(word.tr(lang).into()), false),
                PvFoot::Group(group, word) => {
                    (group.ops().to_vec(), Some(word.tr(lang).into()), false)
                }
                PvFoot::GroupOp(group) => (
                    group.ops().to_vec(),
                    self.label(group.ops()[0], lang),
                    false,
                ),
                PvFoot::Union(group, word) => {
                    (group.ops().to_vec(), Some(word.tr(lang).into()), true)
                }
            };
            let Some(label) = label else {
                continue;
            };
            let on: Vec<_> = ops.iter().copied().filter(|op| self.enabled(*op)).collect();
            let keys: Vec<_> = if union {
                on.iter().flat_map(|op| self.keys(*op)).collect()
            } else {
                help::zip_columns(&on, |op| self.keys(op))
                    .0
                    .into_iter()
                    .flatten()
                    .collect()
            };
            let mut unique = Vec::new();
            for key in keys {
                if !unique.contains(&key) {
                    unique.push(key);
                }
            }
            if !unique.is_empty() {
                out.push(PreviewToken {
                    text: format!(
                        "{}:{label}",
                        help::write_keys(&unique, self.context == CoreContext::Table)
                    ),
                    keys: unique,
                    ops,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_preview_help_omits_empty_sections_from_every_input() {
        let keys = konoma_host_core::keymap::CoreKeys::defaults(
            konoma_host_core::keymap::scheme_from_str("vim"),
        );
        let values = PreviewValues {
            context: CoreContext::Document,
            state: PreviewState::default(),
            bindings: vec![],
            core: HelpFacts::default(),
            search_status: None,
            change_status: None,
            copy_code: None,
        };
        let empty = || CoreSection {
            title: "Movement".into(),
            rows: vec![],
        };
        let sections = values.help_sections(
            &PreviewHelpWords::default(),
            &help::CoreLookup {
                keys: &keys,
                ctx: CoreContext::Document,
            },
            Lang::En,
            vec![],
            vec![empty()],
            vec![empty()],
        );
        assert!(
            sections.iter().all(|s| !s.rows.is_empty()),
            "a shared Preview plan never emits an empty section"
        );
    }
}
