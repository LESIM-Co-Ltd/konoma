//! Git and Jujutsu project screens.

use konoma_plugin::prelude::*;
use konoma_plugin::view::{ListRow, ListView};
use konoma_vcs_core::style::VcsColor;
use konoma_vcs_core::vcs::Vcs;
use std::path::PathBuf;
mod actions;
mod branches;
mod detail;
mod log;
mod paths;
mod worktrees;

pub struct Git;
pub struct Jj;

macro_rules! plugin {
    ($plugin:ident, $id:literal, $marker:literal, $kind:expr, $name:literal) => {
        impl Plugin for $plugin {
            fn manifest(&self) -> Manifest {
                let manifest = Manifest::new($id, env!("CARGO_PKG_VERSION"))
                    .name(t!(en: $name, ja: $name))
                    .project(
                        declaration($id)
                            .marker(DetectionMarker::new($marker, $kind))
                            .version_control()
                            .screen(ProjectScreen::new("changes", t!(en: "Changes", ja: "変更"))
                                .key('p').row_kind("file").row_kind("empty"))
                            .screen(ProjectScreen::new("log", t!(en: "Log", ja: "ログ"))
                                .row_kind("commit").row_kind("empty"))
                            .screen(ProjectScreen::new("branches", if $id == "git" {
                                t!(en: "Branches", ja: "ブランチ")
                            } else { t!(en: "Bookmarks", ja: "ブックマーク") })
                                .key('b').row_kind("branch").row_kind("current").row_kind("empty"))
                            .screen(ProjectScreen::new("detail", t!(en: "Detail", ja: "詳細")).pushed()),
                    )
                    .shows(ViewKind::List)
                    .setting(SettingDecl::new("path_style", Value::Str("relative".into()),
                        t!(en: "Path display: relative, home or full", ja: "パス表示: relative・home・full")))
                    .setting(SettingDecl::new("commit_meta_align", Value::Str("right".into()),
                        t!(en: "Commit author and date: right or inline", ja: "コミットの作者と日付: right・inline")));
                let manifest = actions::declare(worktrees::declare(manifest, $id == "git"), $id == "git")
                    .action(Act::new("diff").key("Enter")
                        .label(t!(en: "diff", ja: "差分"))
                        .help(t!(en: "Open the file diff", ja: "ファイルの差分を開く"))
                        .footer(4)
                        .project_context(ActContext::screen("changes", ["file"])))
                    .action(Act::new("detail").key("Enter")
                        .label(t!(en: "detail", ja: "詳細"))
                        .project_context(ActContext::screen("log", ["commit"])));
                branches::declare(log::copy_declarations(manifest, $id == "jj"), $id == "git")
            }
            fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
                let Input::Project { root, .. } = input else {
                    return Err(Failure::unsupported(t!(en: "Project required", ja: "プロジェクトが必要です")));
                };
                Ok(Box::new(Changes { root, backend: Backend::$plugin, files: Vec::new(), worktree_home: None, worktree_diff: None, detail_meta: Default::default() }))
            }
        }
    };
}
plugin!(Git, "git", ".git", MarkerKind::Any, "Git");
plugin!(Jj, "jj", ".jj", MarkerKind::Directory, "jj");

fn declaration(id: &str) -> ProjectDecl {
    // Like the legacy watcher, changes anywhere in the working tree invalidate status.
    // Watching `.git` itself also covers replacing a linked worktree's gitdir file.
    let paths: &[&str] = if id == "git" {
        &[
            ".",
            ".git",
            ".git/HEAD",
            ".git/index",
            ".git/info/exclude",
            ".git/refs",
            ".git/packed-refs",
            ".gitignore",
        ]
    } else {
        &[
            ".",
            ".jj/repo/op_heads",
            ".jj/repo/op_store",
            ".jj/working_copy",
            ".gitignore",
        ]
    };
    paths
        .iter()
        .fold(ProjectDecl::new(), |decl, path| decl.watch(path))
}

enum Backend {
    Git,
    Jj,
}

impl Backend {
    fn commit_meta(
        &self,
        root: &std::path::Path,
        id: &str,
    ) -> Option<konoma_vcs_core::git::CommitMeta> {
        match self {
            Self::Git => konoma_vcs_core::vcs::Git.commit_meta(root, id),
            #[cfg(feature = "git")]
            Self::Jj => konoma_vcs_core::vcs::Jj.commit_meta(root, id),
            #[cfg(not(feature = "git"))]
            Self::Jj => None,
        }
    }
    fn log(&self, root: &std::path::Path) -> Vec<konoma_vcs_core::git::CommitInfo> {
        match self {
            Self::Git => konoma_vcs_core::vcs::Git.log(root, 200),
            #[cfg(feature = "git")]
            Self::Jj => konoma_vcs_core::vcs::Jj.log(root, 200),
            #[cfg(not(feature = "git"))]
            Self::Jj => Vec::new(),
        }
    }

    fn changed_files(&self, root: &std::path::Path) -> Vec<konoma_vcs_core::git::ChangeEntry> {
        match self {
            Self::Git => konoma_vcs_core::vcs::Git.changed_files(root),
            #[cfg(feature = "git")]
            Self::Jj => konoma_vcs_core::vcs::Jj.changed_files(root),
            #[cfg(not(feature = "git"))]
            Self::Jj => Vec::new(),
        }
    }
}

struct Changes {
    // The host can reopen cached detail views without calling view(), so each cached header
    // needs its original metadata when that older detail becomes active again.
    detail_meta: std::collections::HashMap<String, konoma_vcs_core::git::CommitMeta>,
    root: PathBuf,
    backend: Backend,
    files: Vec<DisplayedFile>,
    worktree_home: Option<PathBuf>,
    worktree_diff: worktrees::PreparedDiff,
}

struct DisplayedFile {
    path: PathBuf,
    label: String,
    staged: bool,
}

impl Session for Changes {
    fn watch_paths(&mut self, _: &ViewCx) -> Result<Option<Vec<PathBuf>>, Failure> {
        let mut paths: Vec<_> = declaration(match self.backend {
            Backend::Git => "git",
            Backend::Jj => "jj",
        })
        .watch_paths
        .iter()
        .map(|p| self.root.join(p))
        .collect();
        if matches!(self.backend, Backend::Git) {
            if let Some(gitdir) = konoma_vcs_core::git::git_dir(&self.root) {
                if let Ok(common) = std::fs::read_to_string(gitdir.join("commondir")) {
                    let path = gitdir.join(common.trim());
                    if let Ok(path) = path.canonicalize() {
                        paths.push(path);
                    }
                }
                paths.push(gitdir);
            }
        }
        Ok(Some(paths))
    }
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        if let Some(project) = req.project.as_ref().filter(|p| p.screen == "worktree_diff") {
            if matches!(self.backend, Backend::Git) {
                return Ok(worktrees::diff_view(
                    &self.root,
                    project,
                    cx,
                    &mut self.worktree_diff,
                ));
            }
        }
        if let Some(project) = req.project.as_ref().filter(|p| p.screen == "worktrees") {
            if matches!(self.backend, Backend::Git) {
                self.worktree_home = project.home_dir.clone();
                return Ok(worktrees::view(&self.root, project, cx));
            }
        }
        if let Some(project) = req.project.as_ref().filter(|p| p.screen == "detail") {
            let (view, meta) =
                detail::read(&self.backend, &self.root, project.selection.as_deref());
            if let Some(id) = &project.selection {
                if let Some(meta) = meta {
                    self.detail_meta.insert(id.clone(), meta);
                } else {
                    self.detail_meta.remove(id);
                }
            }
            return Ok(View::Diff(view));
        }
        if req.project.as_ref().is_some_and(|p| p.screen == "log") {
            return Ok(log::view(self.backend.log(&self.root), cx));
        }
        if req.project.as_ref().is_some_and(|p| p.screen == "branches") {
            return Ok(branches::view(konoma_vcs_core::vcs::refs(&self.root), cx));
        }
        if req.project.as_ref().is_none_or(|p| p.screen != "changes") {
            return Err(Failure::unsupported(
                t!(en: "Unknown screen", ja: "不明な画面です"),
            ));
        }
        let entries = self.backend.changed_files(&self.root);
        self.files = entries
            .iter()
            .map(|entry| DisplayedFile {
                path: entry.path.clone(),
                label: paths::display(
                    &entry.path,
                    &self.root,
                    req.project.as_ref(),
                    cx.settings.str("path_style", "relative"),
                ),
                staged: entry.staged,
            })
            .collect();
        if !entries.is_empty() {
            let rows = entries
                .into_iter()
                .map(|entry| {
                    let name = paths::display(
                        &entry.path,
                        &self.root,
                        req.project.as_ref(),
                        cx.settings.str("path_style", "relative"),
                    );
                    let line = Line::from(vec![
                        Span::raw(if entry.staged { "● " } else { "  " }),
                        Span::styled(
                            format!("{} ", entry.status.marker()),
                            Style::new().fg(vcs_color(entry.status.color())),
                        ),
                        Span::raw(name),
                    ]);
                    ListRow::new("file", vec![line])
                        .name(
                            entry
                                .path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("?"),
                        )
                        .value(entry.path.to_string_lossy())
                        .target(entry.path)
                })
                .collect();
            return Ok(View::List(ListView { rows }));
        }
        Ok(View::List(ListView {
            rows: vec![ListRow::new(
                "empty",
                vec![Line {
                    spans: vec![Span::raw(
                        t!(en: "  (no changes)", ja: "  (変更なし)").get(cx.lang),
                    )],
                    style: Style::new().dim(),
                }],
            )
            .selectable(false)],
        }))
    }

    fn act(&mut self, action: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        if self.enabled(action) {
            if let Some(context) = cx
                .project
                .as_ref()
                .filter(|p| p.screen.as_deref() == Some("branches"))
            {
                return actions::branch_act(&self.root, action, context);
            }
        }
        if let Some((screen, field)) = copy_action(action).filter(|_| self.enabled(action)) {
            let Some(context) = cx
                .project
                .as_ref()
                .filter(|p| p.screen.as_deref() == Some(screen))
            else {
                return Ok(Outcome::Unhandled);
            };
            let Some(row) = context
                .selected
                .as_ref()
                .filter(|r| screen == "detail" || r.kind == "commit")
            else {
                return Ok(Outcome::Unhandled);
            };
            let meta = if screen == "detail" {
                self.detail_meta.get(&row.value).cloned()
            } else {
                self.backend.commit_meta(&self.root, &row.value)
            };
            let Some(meta) = meta else {
                return Ok(Outcome::Flash(
                    t!(en: "no commit to copy", ja: "コピーするコミットがありません"),
                ));
            };
            let text = match field {
                "short_hash" => meta.short,
                "full_hash" => meta.id,
                "subject" => meta.message.lines().next().unwrap_or("").to_string(),
                "message" => meta.message,
                "author" => meta.author,
                "date" => meta.date,
                _ => return Ok(Outcome::Unhandled),
            };
            return Ok(Outcome::Copy { text });
        }
        if action == "detail" {
            return Ok(cx
                .project
                .as_ref()
                .filter(|p| p.screen.as_deref() == Some("log"))
                .and_then(|p| p.selected.as_ref())
                .filter(|r| r.kind == "commit")
                .map(|r| Outcome::PushScreen {
                    screen: "detail".into(),
                    selection: Some(r.value.clone()),
                })
                .unwrap_or(Outcome::Unhandled));
        }
        if !self.enabled(action) {
            return Ok(Outcome::Unhandled);
        }
        if action.starts_with("worktree_") {
            return worktrees::act(
                &self.root,
                action,
                cx,
                self.worktree_home.as_deref(),
                &mut self.worktree_diff,
            );
        }
        let Some(context) = cx
            .project
            .as_ref()
            .filter(|p| p.screen.as_deref() == Some("changes"))
        else {
            return Ok(Outcome::Unhandled);
        };
        if action == "sync" {
            #[cfg(feature = "git")]
            let ok = konoma_vcs_core::vcs::jj::snapshot(&self.root);
            #[cfg(not(feature = "git"))]
            let ok = false;
            return Ok(Outcome::Flash(if ok {
                t!(en: "jj snapshotted the working copy", ja: "jj が作業コピーを取り込みました")
            } else {
                t!(en: "jj could not snapshot the working copy", ja: "jj が作業コピーを取り込めませんでした")
            }));
        }
        if action == "stage_all" {
            if self.files.is_empty() {
                return Ok(Outcome::Flash(t!(en: "no changes", ja: "変更なし")));
            }
            konoma_vcs_core::git::stage_all(&self.root)
                .map_err(|e| actions::failed(&e.to_string()))?;
            return Ok(Outcome::Flash(
                t!(en: format!("Staged all ({})", self.files.len()),
                ja: format!("全てステージ ({})", self.files.len())),
            ));
        }
        if action == "unstage_all" {
            if !self.files.iter().any(|file| file.staged) {
                return Ok(Outcome::Flash(t!(en: "nothing staged", ja: "ステージ無し")));
            }
            konoma_vcs_core::git::unstage_all(&self.root)
                .map_err(|e| actions::failed(&e.to_string()))?;
            return Ok(Outcome::Flash(
                t!(en: "Unstaged all", ja: "全てアンステージ"),
            ));
        }
        if action == "commit" {
            let Some(message) = context.text.as_deref() else {
                return Ok(Outcome::Unhandled);
            };
            let message = message.trim();
            if message.is_empty() {
                return Ok(Outcome::Flash(
                    t!(en: "message is empty", ja: "メッセージが空です"),
                ));
            }
            // The legacy commit handler flashes git's message without the `failed:` prefix.
            konoma_vcs_core::git::commit(&self.root, message)
                .map_err(|e| Failure::external(e.to_string()))?;
            return Ok(Outcome::Flash(t!(en: "Committed", ja: "コミットしました")));
        }
        let Some(row) = context.selected.as_ref().filter(|row| row.kind == "file") else {
            return Ok(Outcome::Unhandled);
        };
        let Some(file) = self
            .files
            .iter()
            .find(|file| file.path.to_string_lossy() == row.value)
        else {
            return Ok(Outcome::Unhandled);
        };
        let (result, message) = match action {
            // The host owns the baseline: HEAD for git, the working-copy parent for jj.
            "diff" => {
                return Ok(Outcome::OpenDiff {
                    path: file.path.clone(),
                })
            }
            "stage" => (
                konoma_vcs_core::git::stage(&self.root, &file.path),
                t!(en: "Staged", ja: "ステージ"),
            ),
            "unstage" => (
                konoma_vcs_core::git::unstage(&self.root, &file.path),
                t!(en: "Unstaged", ja: "アンステージ"),
            ),
            "discard" if context.confirmation.as_deref() == Some("yes") => (
                konoma_vcs_core::git::discard(&self.root, &file.path),
                t!(en: "Discarded", ja: "破棄"),
            ),
            _ => return Ok(Outcome::Unhandled),
        };
        result.map_err(|e| actions::failed(&e.to_string()))?;
        Ok(Outcome::Flash(
            t!(en: format!("{}: {}", message.get("en"), file.label),
            ja: format!("{}: {}", message.get("ja"), file.label)),
        ))
    }

    fn enabled(&self, action: &str) -> bool {
        if !cfg!(feature = "git") {
            return false;
        }
        if copy_action(action).is_some() {
            return true;
        }
        match action {
            "branch_copy_name" => true,
            "checkout"
            | "create_branch"
            | "delete_branch"
            | "delete_current_branch"
            | "worktree_switch"
            | "worktree_create"
            | "worktree_changes" => {
                matches!(self.backend, Backend::Git)
            }
            "detail" => true,
            "diff" => !self.files.is_empty(),
            "stage" | "unstage" | "discard" => {
                matches!(self.backend, Backend::Git) && !self.files.is_empty()
            }
            "stage_all" | "unstage_all" | "commit" => matches!(self.backend, Backend::Git),
            "sync" => matches!(self.backend, Backend::Jj),
            _ => false,
        }
    }
}

fn copy_action(action: &str) -> Option<(&str, &str)> {
    let (screen, field) = action.split_once("_copy_")?;
    (matches!(screen, "log" | "detail")
        && matches!(
            field,
            "short_hash" | "full_hash" | "subject" | "message" | "author" | "date"
        ))
    .then_some((screen, field))
}

fn vcs_color(color: VcsColor) -> Color {
    match color {
        VcsColor::Reset => Color::Reset,
        VcsColor::Red => Color::Red,
        VcsColor::Green => Color::Green,
        VcsColor::Yellow => Color::Yellow,
        VcsColor::Blue => Color::Blue,
        VcsColor::Magenta => Color::Magenta,
        VcsColor::Cyan => Color::Cyan,
        VcsColor::White => Color::White,
        VcsColor::LightRed => Color::LightRed,
        VcsColor::LightGreen => Color::LightGreen,
        VcsColor::LightYellow => Color::LightYellow,
    }
}
