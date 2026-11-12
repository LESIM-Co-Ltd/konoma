//! Commit metadata and per-file diff data; layout belongs to the host.

use konoma_plugin::style::{Color, Line, Span, Style};
use konoma_plugin::view::{DiffHunk, DiffKind, DiffLine, DiffView};
use konoma_vcs_core::git::{CommitMeta, DiffLine as CoreLine, DiffLineKind};
use konoma_vcs_core::vcs::Vcs;
use std::path::Path;

/// The detail selection that stands for "all uncommitted changes" (the graph's working-copy row
/// and the changes screen's diff-all).
pub(super) const UNCOMMITTED: &str = "worktree";

/// The uncommitted changes as one diff: git's HEAD to the index and working tree, jj's `@`
/// against its parent (`@-`).
pub(super) fn uncommitted(backend: &super::Backend, root: &Path) -> Vec<CoreLine> {
    match backend {
        super::Backend::Git => konoma_vcs_core::git::worktree_diff(root),
        #[cfg(feature = "git")]
        super::Backend::Jj => konoma_vcs_core::vcs::Jj.commit_diff(root, "@"),
        #[cfg(not(feature = "git"))]
        super::Backend::Jj => Vec::new(),
    }
}

pub(super) fn read(
    backend: &super::Backend,
    root: &Path,
    id: Option<&str>,
    lang: &str,
) -> (DiffView, Option<CommitMeta>) {
    let Some(id) = id else {
        return (DiffView::default(), None);
    };
    if id == UNCOMMITTED {
        // The old hub titled this diff "Uncommitted changes" (it has no commit header).
        return (
            build(
                None,
                uncommitted(backend, root),
                matches!(backend, super::Backend::Jj),
            )
            .title(Line::raw(format!(
                " {} ",
                konoma_plugin::t!(en: "Uncommitted changes", ja: "未コミットの変更").get(lang)
            ))),
            None,
        );
    }
    let vcs: &dyn Vcs = match backend {
        super::Backend::Git => &konoma_vcs_core::vcs::Git,
        #[cfg(feature = "git")]
        super::Backend::Jj => &konoma_vcs_core::vcs::Jj,
        #[cfg(not(feature = "git"))]
        super::Backend::Jj => return (DiffView::default(), None),
    };
    let meta = vcs.commit_meta(root, id);
    let view = build(
        meta.clone(),
        vcs.commit_diff(root, id),
        matches!(backend, super::Backend::Jj),
    );
    (view, meta)
}

fn build(meta: Option<CommitMeta>, lines: Vec<CoreLine>, jj: bool) -> DiffView {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut current = None;
    for line in lines {
        if line.old_no.is_none() && line.new_no.is_none() {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            current = Some(DiffHunk::new(Vec::new()).path(line.text));
            continue;
        }
        current
            .get_or_insert_with(DiffHunk::default)
            .lines
            .push(DiffLine {
                kind: match line.kind {
                    DiffLineKind::Added => DiffKind::Added,
                    DiffLineKind::Removed => DiffKind::Removed,
                    DiffLineKind::Context => DiffKind::Context,
                },
                old_no: line.old_no.map(|n| n as usize),
                new_no: line.new_no.map(|n| n as usize),
                text: line.text,
            });
    }
    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    let header = meta
        .map(|meta| {
            let mut header = vec![
                Line::from(vec![
                    Span::styled(
                        format!("{} {}", if jj { "change" } else { "commit" }, meta.short),
                        Style::new().fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!("   {} · {}", meta.author, meta.date),
                        Style::new().fg(Color::DarkGray),
                    ),
                ]),
                Line::raw(""),
            ];
            let mut message = meta.message.lines();
            if let Some(subject) = message.next() {
                header.push(Line::from(vec![Span::styled(subject, Style::new().bold())]));
            }
            header.extend(message.map(Line::raw));
            header.push(Line::raw(""));
            header
        })
        .unwrap_or_default();
    DiffView::new(hunks).header(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t.invalid"])
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    #[test]
    fn the_uncommitted_selection_is_titled_in_both_languages_and_a_commit_is_not() {
        let dir = konoma_test_support::unique_tmp("vcs_detail_title");
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.as_path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "seed"]);
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        for (lang, want) in [
            ("en", " Uncommitted changes "),
            ("ja", " 未コミットの変更 "),
        ] {
            let (view, meta) = read(&super::super::Backend::Git, root, Some(UNCOMMITTED), lang);
            assert_eq!(view.title.as_ref().map(Line::plain).as_deref(), Some(want));
            assert!(meta.is_none() && view.header.is_empty());
            assert!(!view.hunks.is_empty(), "{lang}: the diff is there");
        }
        let (view, _) = read(&super::super::Backend::Git, root, Some("HEAD"), "en");
        assert!(view.title.is_none(), "a commit keeps its own header");
    }

    #[cfg(feature = "git")]
    #[test]
    fn jj_uncommitted_in_a_non_colocated_repo_is_the_working_copy_against_its_parent() {
        fn jj(root: &Path, args: &[&str]) -> bool {
            std::process::Command::new("jj")
                .args(args)
                .current_dir(root)
                .env("JJ_CONFIG", "")
                .env("JJ_USER", "t")
                .env("JJ_EMAIL", "t@t.invalid")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        let dir = konoma_test_support::unique_tmp("vcs_detail_jj");
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.as_path();
        if !jj(root, &["git", "init", "--no-colocate", "."]) {
            return; // no jj on this machine
        }
        assert!(!root.join(".git").exists());
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        assert!(jj(root, &["commit", "-m", "seed"]));
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        // konoma itself never snapshots (every call carries --ignore-working-copy), so the edit
        // reaches `@` only once some jj command has run, as it does in real use.
        assert!(jj(root, &["status"]));
        let (view, meta) = read(&super::super::Backend::Jj, root, Some(UNCOMMITTED), "en");
        assert!(meta.is_none());
        assert_eq!(
            view.title.as_ref().map(Line::plain).as_deref(),
            Some(" Uncommitted changes ")
        );
        let texts: Vec<_> = view
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| (l.kind, l.text.as_str())))
            .collect();
        assert!(texts.contains(&(DiffKind::Removed, "one")), "{texts:?}");
        assert!(texts.contains(&(DiffKind::Added, "two")), "{texts:?}");
    }

    #[test]
    fn detail_core_metadata_keeps_jj_word_styles_blank_lines_and_file_paths() {
        for jj in [false, true] {
            let meta = CommitMeta {
                id: "full".into(),
                short: "short".into(),
                author: "Author".into(),
                date: "Date".into(),
                message: "Subject\n\nBody\n\n日本語".into(),
            };
            let lines = ["main.rs", "data.json"]
                .into_iter()
                .flat_map(|path| {
                    [
                        CoreLine {
                            kind: DiffLineKind::Context,
                            old_no: None,
                            new_no: None,
                            text: path.into(),
                        },
                        CoreLine {
                            kind: DiffLineKind::Added,
                            old_no: None,
                            new_no: Some(1),
                            text: "body".into(),
                        },
                    ]
                })
                .collect();
            let view = build(Some(meta), lines, jj);
            assert_eq!(
                view.header.iter().map(Line::plain).collect::<Vec<_>>(),
                [
                    format!(
                        "{} short   Author · Date",
                        if jj { "change" } else { "commit" }
                    ),
                    "".into(),
                    "Subject".into(),
                    "".into(),
                    "Body".into(),
                    "".into(),
                    "日本語".into(),
                    "".into()
                ]
            );
            assert_eq!(view.header[0].spans[0].style.fg, Some(Color::Yellow));
            assert_eq!(view.header[0].spans[1].style.fg, Some(Color::DarkGray));
            assert!(view.header[2].spans[0]
                .style
                .add
                .contains(konoma_plugin::style::Modifier::BOLD));
            assert_eq!(
                view.hunks
                    .iter()
                    .map(|h| h.path.as_deref())
                    .collect::<Vec<_>>(),
                [Some("main.rs"), Some("data.json")]
            );
            assert!(view
                .hunks
                .iter()
                .all(|h| h.lines.len() == 1 && h.lines[0].text == "body"));
        }
    }
}
