//! Read-only Git and Jujutsu project screens.

use konoma_plugin::prelude::*;
use konoma_plugin::view::{ListRow, ListView};
use konoma_vcs_core::style::VcsColor;
use konoma_vcs_core::vcs::Vcs;
use std::path::PathBuf;
mod paths;

pub struct Git;
pub struct Jj;

macro_rules! plugin {
    ($plugin:ident, $id:literal, $marker:literal, $kind:expr, $name:literal) => {
        impl Plugin for $plugin {
            fn manifest(&self) -> Manifest {
                Manifest::new($id, env!("CARGO_PKG_VERSION"))
                    .name(t!(en: $name, ja: $name))
                    .project(
                        declaration($id)
                            .marker(DetectionMarker::new($marker, $kind))
                            .version_control()
                            .screen(ProjectScreen::new("changes", t!(en: "Changes", ja: "変更"))
                                .key('p').row_kind("file").row_kind("empty")),
                    )
                    .shows(ViewKind::List)
                    .setting(SettingDecl::new("path_style", Value::Str("relative".into()),
                        t!(en: "Path display: relative, home or full", ja: "パス表示: relative・home・full")))
                    .action(Act::new("diff").key("Enter")
                        .label(t!(en: "diff", ja: "差分"))
                        .help(t!(en: "Open the file diff", ja: "ファイルの差分を開く"))
                        .footer(1)
                        .project_context(ActContext::screen("changes", ["file"])))
            }
            fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
                let Input::Project { root, .. } = input else {
                    return Err(Failure::unsupported(t!(en: "Project required", ja: "プロジェクトが必要です")));
                };
                Ok(Box::new(Changes { root, backend: Backend::$plugin, files: Vec::new() }))
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
    root: PathBuf,
    backend: Backend,
    files: Vec<PathBuf>,
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
        if req.project.as_ref().is_none_or(|p| p.screen != "changes") {
            return Err(Failure::unsupported(
                t!(en: "Unknown screen", ja: "不明な画面です"),
            ));
        }
        let entries = self.backend.changed_files(&self.root);
        self.files = entries.iter().map(|entry| entry.path.clone()).collect();
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
        if action != "diff" {
            return Ok(Outcome::Unhandled);
        }
        let Some(context) = cx
            .project
            .as_ref()
            .filter(|p| p.screen.as_deref() == Some("changes"))
        else {
            return Ok(Outcome::Unhandled);
        };
        let Some(row) = context.selected.as_ref().filter(|row| row.kind == "file") else {
            return Ok(Outcome::Unhandled);
        };
        // The host owns the baseline: HEAD for git, the working-copy parent for jj.
        // Staging is only a row indicator, just as in the legacy changes hub.
        Ok(self
            .files
            .iter()
            .find(|path| path.to_string_lossy() == row.value)
            .map(|path| Outcome::OpenDiff { path: path.clone() })
            .unwrap_or(Outcome::Unhandled))
    }
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
