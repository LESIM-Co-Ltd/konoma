//! Linked worktrees; the host owns selection, filtering and diff layout.

use konoma_plugin::prelude::*;
use konoma_plugin::view::{DiffHunk, DiffKind, DiffLine, DiffView, ListRow, ListView};
use konoma_vcs_core::git::{self, WorktreeInfo};
use std::path::Path;

pub(super) fn kind(worktree: &WorktreeInfo) -> &'static str {
    if worktree.is_bare {
        "bare"
    } else if worktree.prunable || !worktree.path.is_dir() {
        "unavailable"
    } else if worktree.is_current {
        "current"
    } else {
        "worktree"
    }
}

pub(super) fn view(root: &Path, project: &ProjectView, cx: &ViewCx) -> View {
    let mut rows: Vec<_> = git::worktrees(root)
        .into_iter()
        .map(|w| {
            let name = if w.is_bare {
                "(bare)"
            } else {
                w.branch.as_deref().unwrap_or("(detached)")
            };
            let path = super::paths::display(&w.path, root, Some(project), "home");
            let mut spans = vec![
                Span::styled(
                    if w.is_current { "* " } else { "  " },
                    Style::new().fg(Color::Green),
                ),
                Span::styled(
                    name,
                    if w.is_current {
                        Style::new().fg(Color::Green).bold()
                    } else {
                        Style::new()
                    },
                ),
                Span::raw(format!(
                    "  {}  {path}",
                    w.head.as_deref().unwrap_or("-------")
                )),
            ];
            let mut suffix = |text: String| spans.push(Span::styled(text, Style::new().dim()));
            if w.is_main {
                suffix("  main".into());
            }
            if w.is_bare {
                suffix("  bare".into());
            }
            if let Some(reason) = &w.locked {
                suffix(if reason.is_empty() {
                    "  locked".into()
                } else {
                    format!("  locked: {reason}")
                });
            }
            if w.prunable {
                suffix("  prunable".into());
            }
            let row = ListRow::new(kind(&w), vec![Line::from(spans)])
                .initial_selection(w.is_current)
                .name(
                    w.branch
                        .as_deref()
                        .map(str::to_owned)
                        .unwrap_or_else(|| w.path.to_string_lossy().into_owned()),
                )
                .value(w.path.to_string_lossy())
                .filter_text(format!(
                    "{} {}",
                    w.branch.as_deref().unwrap_or(""),
                    w.path.display()
                ));
            if kind(&w) == "worktree" {
                row.target(w.path)
            } else {
                row
            }
        })
        .collect();
    rows.push(
        ListRow::new(
            "empty",
            vec![Line::from(vec![Span::styled(
                t!(en: "  (no worktrees)", ja: "  (ワークツリーなし)").get(cx.lang),
                Style::new().dim(),
            )])],
        )
        .filter_fallback(true),
    );
    View::List(ListView { rows })
}

fn refusal(kind: &str, switching: bool) -> Option<Tr> {
    match kind {
        "bare" => Some(
            t!(en: "bare repository — it has no checkout", ja: "ベアリポジトリ（実体のチェックアウトがありません）"),
        ),
        "unavailable" => Some(
            t!(en: "worktree is missing (run `git worktree prune`)", ja: "ワークツリーが見つかりません（`git worktree prune` を実行してください）"),
        ),
        "current" if switching => {
            Some(t!(en: "already in this worktree", ja: "既にこのワークツリーにいます"))
        }
        _ => None,
    }
}

pub(super) fn display_path(path: &Path, home: Option<&Path>) -> String {
    home.and_then(|home| path.strip_prefix(home).ok())
        .map(|relative| {
            if relative.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", relative.display())
            }
        })
        .unwrap_or_else(|| path.display().to_string())
}

pub(super) fn act(
    root: &Path,
    action: &str,
    cx: &ActCx,
    _home: Option<&Path>,
) -> Result<Outcome, Failure> {
    let Some(context) = cx
        .project
        .as_ref()
        .filter(|p| p.screen.as_deref() == Some("worktrees"))
    else {
        return Ok(Outcome::Unhandled);
    };
    if action == "worktree_create" {
        #[cfg(feature = "git")]
        return super::actions::create_worktree(root, context.text.as_deref(), cx.settings, _home);
        #[cfg(not(feature = "git"))]
        return Ok(Outcome::Unhandled);
    }
    let Some(row) = context.selected.as_ref().filter(|r| r.kind != "empty") else {
        return Ok(Outcome::Unhandled);
    };
    let worktrees = git::worktrees(root);
    let Some(worktree) = worktrees
        .iter()
        .find(|w| w.path.to_string_lossy() == row.value)
    else {
        return Ok(Outcome::Unhandled);
    };
    if action == "worktree_switch" {
        return Ok(if let Some(message) = refusal(kind(worktree), true) {
            Outcome::Flash(message)
        } else {
            Outcome::SwitchRoot {
                root: worktree.path.clone(),
                message: None,
            }
        });
    }
    if action == "worktree_changes" {
        if let Some(message) = refusal(kind(worktree), false) {
            return Ok(Outcome::Flash(message));
        }
        let diff = changes(root, worktree, &worktrees, cx.settings, cx.lang, _home);
        return Ok(if diff.hunks.is_empty() {
            Outcome::Flash(t!(en: "no changes", ja: "変更なし"))
        } else {
            Outcome::PushScreen {
                screen: "worktree_diff".into(),
                selection: Some(worktree.path.to_string_lossy().into_owned()),
            }
        });
    }
    Ok(Outcome::Unhandled)
}

pub(super) fn diff_view(root: &Path, project: &ProjectView, cx: &ViewCx) -> View {
    let worktrees = git::worktrees(root);
    let diff = project
        .selection
        .as_deref()
        .and_then(|path| worktrees.iter().find(|w| w.path.to_string_lossy() == path))
        .filter(|w| refusal(kind(w), false).is_none())
        .map(|w| {
            changes(
                root,
                w,
                &worktrees,
                cx.settings,
                cx.lang,
                project.home_dir.as_deref(),
            )
        })
        .unwrap_or_default();
    View::Diff(diff)
}

fn base(
    root: &Path,
    worktree: &WorktreeInfo,
    worktrees: &[WorktreeInfo],
    settings: &Settings,
) -> Option<String> {
    let mut candidates = Vec::new();
    for name in settings.list_str("graph_base_branches") {
        if !candidates.contains(&name) && git::branch_tip(root, &name).is_some() {
            candidates.push(name);
        }
    }
    if let Some(main) = worktrees
        .iter()
        .find(|w| w.is_main)
        .and_then(|w| w.branch.as_ref())
    {
        if !candidates.contains(main) {
            candidates.push(main.clone());
        }
    }
    let mut best: Option<(String, i64)> = None;
    for name in candidates {
        if worktree.branch.as_ref() == Some(&name) {
            continue;
        }
        let Some(time) = git::merge_base_time(&worktree.path, &name) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, previous)| time > *previous) {
            best = Some((name, time));
        }
    }
    best.map(|(name, _)| name)
}

fn changes(
    root: &Path,
    worktree: &WorktreeInfo,
    worktrees: &[WorktreeInfo],
    settings: &Settings,
    lang: &str,
    home: Option<&Path>,
) -> DiffView {
    let branch = worktree
        .branch
        .as_deref()
        .or(worktree.head.as_deref())
        .unwrap_or("?");
    let path = display_path(&worktree.path, home);
    let base = base(root, worktree, worktrees, settings);
    let since = base
        .as_deref()
        .map(|base| git::diff_since(&worktree.path, base))
        .unwrap_or_default();
    let (lines, title) = if let Some(base) = base.filter(|_| !since.is_empty()) {
        (since, format!("{branch} ← {base}  {path}"))
    } else {
        (
            git::worktree_diff(&worktree.path),
            format!(
                "{branch}  {path}  ({})",
                t!(en: "Uncommitted changes", ja: "未コミットの変更").get(lang)
            ),
        )
    };
    if lines.is_empty() {
        return DiffView::default();
    }
    let mut hunks: Vec<DiffHunk> = Vec::new();
    for line in lines {
        if line.old_no.is_none() && line.new_no.is_none() {
            hunks.push(DiffHunk::default().path(line.text));
            continue;
        }
        if hunks.is_empty() {
            hunks.push(DiffHunk::default());
        }
        if let Some(hunk) = hunks.last_mut() {
            hunk.lines.push(DiffLine {
                kind: match line.kind {
                    git::DiffLineKind::Added => DiffKind::Added,
                    git::DiffLineKind::Removed => DiffKind::Removed,
                    git::DiffLineKind::Context => DiffKind::Context,
                },
                old_no: line.old_no.map(|n| n as usize),
                new_no: line.new_no.map(|n| n as usize),
                text: line.text,
            });
        }
    }
    DiffView::new(hunks).header(vec![Line::raw(title)])
}

pub(super) fn declare(mut manifest: Manifest, git: bool) -> Manifest {
    if !git {
        return manifest;
    }
    if let Some(project) = manifest.project.as_mut() {
        let after = project
            .screens
            .iter()
            .rposition(|s| s.id == "branches")
            .or_else(|| project.screens.iter().rposition(|s| s.id == "log"))
            .map_or(0, |i| i + 1);
        project.screens.insert(
            after,
            ProjectScreen::new("worktrees", t!(en: "Worktrees", ja: "ワークツリー"))
                .filter_label(t!(en: "search", ja: "検索"))
                .key('w')
                .row_kind("worktree")
                .row_kind("current")
                .row_kind("bare")
                .row_kind("unavailable")
                .row_kind("empty"),
        );
        project
            .screens
            .push(ProjectScreen::new("worktree_diff", t!(en: "Changes", ja: "変更")).pushed());
    }
    manifest
        .setting(SettingDecl::new("worktree_dir", Value::Str("../".into()),
            t!(en: "New worktrees: directory relative to the main worktree", ja: "新規ワークツリーの配置先: メインワークツリーからの相対ディレクトリ")))
        .setting(SettingDecl::new("graph_base_branches", Value::List(vec![]),
            t!(en: "Base branch candidates; the newest merge-base wins, ties keep list order", ja: "base ブランチの候補: 最新の merge-base を選択し、同時刻ならリスト順")))
        .action(Act::new("worktree_switch").key("Enter")
            .label(t!(en: "switch", ja: "切替"))
            .help(t!(en: "switch to it", ja: "切り替える"))
            .footer(0).project_context(ActContext::screen("worktrees", ["worktree", "current", "bare", "unavailable"])))
}
