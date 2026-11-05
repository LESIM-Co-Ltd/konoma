//! Commit graph state and styled rows; navigation and layout belong to the host.

use konoma_plugin::prelude::*;
use konoma_plugin::view::{ListRow, ListStrip, ListView};
use konoma_vcs_core::git::{self, GraphRow, LegendEntry, WorktreeLabels};
use konoma_vcs_core::vcs::Vcs;
use std::collections::HashSet;
use std::path::Path;

pub(super) fn declare(mut manifest: Manifest, jj: bool) -> Manifest {
    manifest = manifest
        .hint_group(HintGroup::new("picker_reorder", t!(en: "reorder", ja: "並替")))
        .action(Act::new("graph_picker").key("b")
            .label(if jj { t!(en: "bookmarks", ja: "ブックマーク") } else { t!(en: "branches", ja: "ブランチ") })
            .help(if jj { t!(en: "bookmarks", ja: "ブックマーク") } else { t!(en: "choose which branches are shown", ja: "表示するブランチを選ぶ") })
            .footer(7).refresh(RefreshTarget::screen("graph_picker"))
            .project_context(ActContext::screen("graph", [] as [&str; 0])))
        .action(picker_action("picker_toggle", "x", t!(en: "toggle", ja: "トグル"), 0))
        .action(picker_action("picker_down", "J", t!(en: "move down", ja: "下へ移動"), 1).hint_group("picker_reorder"))
        .action(picker_action("picker_up", "K", t!(en: "move up", ja: "上へ移動"), 1).hint_group("picker_reorder"))
        .action(picker_action("picker_all", "a", t!(en: "all", ja: "全部"), 2))
        .action(picker_action("picker_current", "c", t!(en: "current only", ja: "現在のみ"), 3))
        .action(picker_action("picker_apply", "Enter", t!(en: "apply", ja: "適用"), 4)
            .refresh(RefreshTarget::screen("graph")))
        .setting(SettingDecl::new("graph_max_branches", Value::Int(12),
            t!(en: "Maximum visible graph branches (0: unlimited)", ja: "グラフの表示ブランチ数 (0: 無制限)")))
        .setting(SettingDecl::new("graph_base_branches", Value::List(vec![]),
            t!(en: "Priority branches for the graph base", ja: "グラフの基準ブランチの優先順")))
        .action(Act::new("graph_detail").key("Enter")
            .label(t!(en: "detail", ja: "詳細"))
            .help(t!(en: "detail (commit diff / worktree diff)", ja: "詳細 (コミット差分 / 作業ツリー差分)"))
            .footer(4)
            .project_context(ActContext::screen("graph", ["commit", "worktree"])));
    if jj {
        manifest.action(
            Act::new("graph_all")
                .key("a")
                .label(t!(en: "all revisions", ja: "全リビジョン"))
                .help(t!(en: "show every revision", ja: "全リビジョンを表示"))
                .footer(5)
                .refresh(RefreshTarget::CurrentScreen)
                .project_context(ActContext::screen("graph", [] as [&str; 0])),
        )
    } else {
        manifest
            .action(Act::new("graph_set_base").key("s")
                .label(t!(en: "base", ja: "基準"))
                .help(t!(en: "pin the selected commit's branch as the base (leftmost lane)", ja: "選択コミットの枝を基準(左端レーン)に固定"))
                .footer(5).refresh(RefreshTarget::CurrentScreen).project_context(ActContext::screen("graph", [] as [&str; 0])))
            .action(Act::new("graph_clear_base").key("x")
                .label(t!(en: "base off", ja: "基準解除"))
                .help(t!(en: "clear the pinned base", ja: "基準の固定を解除"))
                .footer(6).refresh(RefreshTarget::CurrentScreen).project_context(ActContext::screen("graph", [] as [&str; 0])))
    }
}

fn picker_action(id: &str, key: &str, label: Tr, priority: u8) -> Act {
    Act::new(id)
        .key(key)
        .label(label)
        .footer(priority)
        .refresh(RefreshTarget::CurrentScreen)
        .project_context(ActContext::screen("graph_picker", [] as [&str; 0]))
}

#[derive(Default)]
pub(super) struct Graph {
    base: Option<String>,
    label: Option<String>,
    visible: HashSet<String>,
    order: Vec<String>,
    all: bool,
    initialized: bool,
    rows: Vec<GraphRow>,
    tentative: HashSet<String>,
    reordered: bool,
}

impl Graph {
    pub(super) fn act(
        &mut self,
        action: &str,
        project: &ProjectAct,
        root: &Path,
        settings: &Settings,
    ) -> Outcome {
        match action {
            "graph_picker" => {
                self.ensure_order(&git::branches_by_recency(root), settings);
                if self.visible.is_empty() {
                    self.visible.extend(self.order.iter().cloned());
                }
                self.tentative.clone_from(&self.visible);
                self.reordered = false;
                Outcome::PushScreen {
                    screen: "graph_picker".into(),
                    selection: None,
                }
            }
            "graph_set_base" => {
                let Some(row) = project
                    .selected
                    .as_ref()
                    .filter(|r| r.kind == "commit")
                    .and_then(|r| {
                        self.rows
                            .iter()
                            .find(|row| row.commit.as_deref() == Some(&r.value))
                    })
                else {
                    return Outcome::Flash(
                        t!(en: "select a commit to set as base", ja: "基準にするコミットを選んでください"),
                    );
                };
                let label = row
                    .refs
                    .split(',')
                    .map(str::trim)
                    .map(|part| part.strip_prefix("HEAD -> ").unwrap_or(part))
                    .find(|part| !part.is_empty() && !part.starts_with("tag:"))
                    .unwrap_or(&row.short)
                    .to_string();
                self.base.clone_from(&row.commit);
                self.label = Some(label.clone());
                Outcome::Flash(t!(en: format!("base: {label}"), ja: format!("基準: {label}")))
            }
            "graph_clear_base" if self.base.is_some() => {
                self.base = None;
                self.label = None;
                Outcome::Flash(t!(en: "base cleared", ja: "基準を解除しました"))
            }
            "graph_all" => {
                self.all = !self.all;
                Outcome::Flash(if self.all {
                    t!(en: "showing every revision", ja: "全リビジョンを表示")
                } else {
                    t!(en: "showing the default range", ja: "既定の範囲を表示")
                })
            }
            _ => Outcome::Unhandled,
        }
    }
    pub(super) fn picker_act(
        &mut self,
        action: &str,
        project: &ProjectAct,
        root: &Path,
    ) -> Outcome {
        let selected = project.selected.as_ref();
        match action {
            "picker_apply" => {
                self.visible.clone_from(&self.tentative);
                if self.reordered {
                    let base = self
                        .order
                        .iter()
                        .filter(|name| self.visible.contains(*name))
                        .find_map(|name| {
                            konoma_vcs_core::vcs::ref_tip(root, name).map(|oid| (oid, name.clone()))
                        });
                    (self.base, self.label) =
                        base.map_or((None, None), |(oid, name)| (Some(oid), Some(name)));
                }
                self.reordered = false;
                return Outcome::PopScreen;
            }
            "picker_toggle" => {
                let Some(row) = selected else {
                    return Outcome::Unhandled;
                };
                if row.kind == "current" {
                    return Outcome::Flash(
                        t!(en: "current branch (HEAD) is always shown", ja: "現在ブランチ(HEAD)は常に表示されます"),
                    );
                }
                if !self.tentative.remove(&row.value) {
                    self.tentative.insert(row.value.clone());
                }
            }
            "picker_all" => self.tentative = self.order.iter().cloned().collect(),
            "picker_current" => {
                self.tentative.clear();
                self.tentative.extend(
                    git::branches_by_recency(root)
                        .into_iter()
                        .filter(|(_, current, _)| *current)
                        .map(|(name, _, _)| name),
                );
                self.tentative.extend(self.label.iter().cloned());
            }
            "picker_down" | "picker_up" => {
                if let Some(i) =
                    selected.and_then(|row| self.order.iter().position(|name| name == &row.value))
                {
                    let j = if action == "picker_down" {
                        i.checked_add(1)
                    } else {
                        i.checked_sub(1)
                    };
                    if let Some(j) = j.filter(|j| *j < self.order.len()) {
                        self.order.swap(i, j);
                        self.reordered = true;
                    }
                }
            }
            _ => return Outcome::Unhandled,
        }
        Outcome::Redraw
    }
    pub(super) fn picker_view(&self, root: &Path, icons: bool, lang: &str) -> View {
        let head = git::branches_by_recency(root)
            .into_iter()
            .find(|(_, head, _)| *head)
            .map(|(name, _, _)| name);
        let rows = if self.order.is_empty() {
            vec![ListRow::new(
                "empty",
                vec![Line::from(Span::styled(
                    t!(en: "  (no branches)", ja: "  (ブランチなし)").get(lang),
                    Style::new().dim(),
                ))],
            )
            .selectable(false)]
        } else {
            self.order
                .iter()
                .map(|name| {
                    let current = head.as_ref() == Some(name);
                    let on = self.tentative.contains(name);
                    ListRow::new(
                        if current { "current" } else { "branch" },
                        vec![Line::from(vec![
                            Span::styled(
                                if on { "[x] " } else { "[ ] " },
                                if on {
                                    Style::new().fg(Color::Green)
                                } else {
                                    Style::new().dim()
                                },
                            ),
                            Span::raw(if current {
                                if icons {
                                    "⎇ "
                                } else {
                                    "* "
                                }
                            } else {
                                "  "
                            }),
                            Span::styled(
                                name,
                                if current {
                                    Style::new().fg(Color::Green).bold()
                                } else if on {
                                    Style::new()
                                } else {
                                    Style::new().dim()
                                },
                            ),
                        ])],
                    )
                    .value(name)
                    .name(name)
                })
                .collect()
        };
        let shown = self
            .order
            .iter()
            .filter(|name| self.tentative.contains(*name))
            .count();
        View::List(ListView {
            rows,
            title: Some(Line::raw(format!(
                "{}  ({shown}/{})",
                t!(en: "Graph branches", ja: "グラフのブランチ").get(lang),
                self.order.len()
            ))),
            bottom: None,
        })
    }
    fn ensure_order(&mut self, branches: &[(String, bool, i64)], settings: &Settings) {
        let exists: HashSet<_> = branches.iter().map(|(name, _, _)| name.as_str()).collect();
        self.order.retain(|name| exists.contains(name.as_str()));
        if self.order.is_empty() {
            for name in settings
                .list_str("graph_base_branches")
                .into_iter()
                .chain(
                    branches
                        .iter()
                        .filter(|(_, head, _)| *head)
                        .map(|(name, _, _)| name.clone()),
                )
                .chain(branches.iter().map(|(name, _, _)| name.clone()))
            {
                if exists.contains(name.as_str()) && !self.order.contains(&name) {
                    self.order.push(name);
                }
            }
        } else {
            for (name, _, _) in branches {
                if !self.order.contains(name) {
                    self.order.push(name.clone());
                }
            }
        }
    }

    pub(super) fn view(
        &mut self,
        backend: &super::Backend,
        root: &Path,
        icons: bool,
        cx: &ViewCx,
    ) -> View {
        let branches = git::branches_by_recency(root);
        self.ensure_order(&branches, cx.settings);
        if !self.initialized {
            let cap = usize::try_from(cx.settings.int("graph_max_branches", 12)).unwrap_or(12);
            self.visible.extend(
                branches
                    .iter()
                    .filter(|(_, head, _)| *head)
                    .map(|(name, _, _)| name.clone()),
            );
            for name in &self.order {
                if cap != 0 && self.visible.len() >= cap {
                    break;
                }
                self.visible.insert(name.clone());
            }
            if matches!(backend, super::Backend::Git)
                && !cx.settings.list_str("graph_base_branches").is_empty()
            {
                for name in &self.order {
                    if self.visible.contains(name) {
                        if let Some(oid) = konoma_vcs_core::vcs::ref_tip(root, name) {
                            self.base = Some(oid);
                            self.label = Some(name.clone());
                            break;
                        }
                    }
                }
            }
            self.initialized = true;
        }
        let visible: Vec<_> = self.visible.iter().cloned().collect();
        let use_all = visible.is_empty() || visible.len() >= branches.len();
        let labels = worktree_labels(cx.lang);
        let refs = (!use_all).then_some(visible.as_slice());
        self.rows = match backend {
            super::Backend::Git => {
                konoma_vcs_core::vcs::Git.graph(self.all, root, self.base.as_deref(), &labels, refs)
            }
            #[cfg(feature = "git")]
            super::Backend::Jj => {
                konoma_vcs_core::vcs::Jj.graph(self.all, root, None, &labels, refs)
            }
            #[cfg(not(feature = "git"))]
            super::Backend::Jj => Vec::new(),
        };
        if !use_all {
            for row in &mut self.rows {
                row.refs = row
                    .refs
                    .split(',')
                    .map(str::trim)
                    .filter(|name| {
                        name.starts_with("HEAD -> ")
                            || *name == "HEAD"
                            || name.starts_with("tag:")
                            || self.visible.contains(*name)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
            }
        }
        let mut legend = git::legend_from_rows(&self.rows, root, self.label.as_deref());
        legend.sort_by_key(|entry| {
            self.order
                .iter()
                .position(|name| name == &entry.name)
                .unwrap_or(usize::MAX)
        });
        let hidden = if use_all {
            0
        } else {
            branches.len().saturating_sub(visible.len())
        };
        let n = self.rows.iter().filter(|row| row.commit.is_some()).count();
        let mut title = format!(
            "{} graph  ({n})",
            if matches!(backend, super::Backend::Git) {
                "Git"
            } else {
                "jj"
            }
        );
        if let Some(label) = &self.label {
            if matches!(backend, super::Backend::Git) {
                title.push_str(&format!("  {}base: {label}", if icons { "⌖ " } else { "" }));
            }
        }
        let rows = if self
            .rows
            .iter()
            .all(|row| row.commit.is_none() && !row.worktree)
        {
            vec![ListRow::new(
                "empty",
                vec![Line::from(Span::styled(
                    t!(en: "  (no commits)", ja: "  (コミットなし)").get(cx.lang),
                    Style::new().dim(),
                ))],
            )
            .selectable(false)]
        } else {
            self.rows
                .iter()
                .map(|row| {
                    row_view(
                        row,
                        cx.settings.str("commit_meta_align", "right") == "inline",
                    )
                })
                .collect()
        };
        View::List(ListView {
            rows,
            title: Some(Line::raw(title)),
            bottom: (!legend.is_empty()).then(|| legend_strip(&legend, hidden, icons, cx.lang)),
        })
    }
}

fn worktree_labels(lang: &str) -> WorktreeLabels {
    WorktreeLabels {
        uncommitted: t!(en: "Uncommitted changes", ja: "未コミットの変更")
            .get(lang)
            .into(),
        staged: t!(en: "staged", ja: "ステージ済").get(lang).into(),
        unstaged: t!(en: "unstaged", ja: "未ステージ").get(lang).into(),
        untracked: t!(en: "untracked", ja: "未追跡").get(lang).into(),
    }
}

fn row_view(row: &GraphRow, inline: bool) -> ListRow {
    let mut spans: Vec<_> = row
        .graph
        .iter()
        .map(|(text, style)| {
            let mut mapped = Style::new();
            if let Some(color) = style.fg {
                mapped = mapped.fg(super::vcs_color(color));
            }
            if style.bold {
                mapped = mapped.bold();
            }
            Span::styled(text, mapped)
        })
        .collect();
    let (kind, filter, meta) = if row.worktree {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            &row.subject,
            Style::new().fg(Color::Yellow).bold(),
        ));
        (
            "worktree",
            format!("{} {}", row.subject, row.date),
            row.date.clone(),
        )
    } else if row.commit.is_some() {
        spans.push(Span::raw(" "));
        if !row.refs.is_empty() {
            spans.push(Span::styled(
                format!("({}) ", row.refs),
                Style::new().fg(Color::Yellow),
            ));
        }
        spans.push(Span::raw(&row.subject));
        (
            "commit",
            format!(
                "{} {} {} {} {}",
                row.refs, row.subject, row.short, row.author, row.date
            ),
            [&row.short, &row.author, &row.date]
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" · "),
        )
    } else {
        ("connector", String::new(), String::new())
    };
    if inline && !meta.is_empty() {
        spans.push(Span::styled(format!("  {meta}"), Style::new().dim()));
    }
    let result = ListRow::new(kind, vec![Line::from(spans)])
        .value(
            row.commit
                .as_deref()
                .unwrap_or(if row.worktree { "worktree" } else { "" }),
        )
        .name(&row.short)
        .filter_text(filter)
        .selectable(row.commit.is_some() || row.worktree);
    if !inline && !meta.is_empty() {
        result.right(super::log::right_meta(meta))
    } else {
        result
    }
}

fn legend_strip(legend: &[LegendEntry], hidden: usize, icons: bool, lang: &str) -> ListStrip {
    let mut chips: Vec<_> = legend
        .iter()
        .map(|entry| {
            let color = super::vcs_color(entry.color);
            let mut spans = Vec::new();
            if entry.is_head {
                spans.extend([
                    Span::styled(if icons { "⎇" } else { "br:" }, Style::new().bold()),
                    Span::raw(" "),
                    Span::styled("●", Style::new().fg(color).bold()),
                    Span::styled(format!(" {}", entry.name), Style::new().bold()),
                ]);
            } else if entry.is_base {
                spans.extend([
                    Span::styled(if icons { "⌖" } else { "base:" }, Style::new().fg(color)),
                    Span::raw(" "),
                    Span::styled("●", Style::new().fg(color)),
                    Span::raw(format!(" {}", entry.name)),
                ]);
            } else {
                spans.extend([
                    Span::styled("●", Style::new().fg(color)),
                    Span::raw(format!(" {}", entry.name)),
                ]);
            }
            Line::from(spans)
        })
        .collect();
    if hidden > 0 {
        chips.push(Line::from(Span::styled(
            format!("(+{hidden} {})", t!(en: "hidden", ja: "非表示").get(lang)),
            Style::new().fg(Color::DarkGray),
        )));
    }
    ListStrip::new(
        chips,
        legend
            .iter()
            .map(|entry| entry.name.chars().count() + 5)
            .sum::<usize>()
            + if hidden > 0 { 12 } else { 0 },
    )
}
