//! Commit metadata and per-file diff data; layout belongs to the host.

use konoma_plugin::style::{Color, Line, Span, Style};
use konoma_plugin::view::{DiffHunk, DiffKind, DiffLine, DiffView};
use konoma_vcs_core::git::{CommitMeta, DiffLine as CoreLine, DiffLineKind};
use konoma_vcs_core::vcs::Vcs;
use std::path::Path;

pub(super) fn read(
    backend: &super::Backend,
    root: &Path,
    id: Option<&str>,
) -> (DiffView, Option<CommitMeta>) {
    let Some(id) = id else {
        return (DiffView::default(), None);
    };
    if id == "worktree" && matches!(backend, super::Backend::Git) {
        return (
            build(None, konoma_vcs_core::git::worktree_diff(root), false),
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
