//! Linear commit rows; layout and matching remain host responsibilities.

use konoma_plugin::prelude::*;
use konoma_plugin::view::{ListRow, ListView};
use konoma_vcs_core::git::CommitInfo;

pub(super) fn copy_declarations(mut manifest: Manifest, jj: bool) -> Manifest {
    for screen in ["log", "detail"] {
        for (field, key, label) in [
            (
                "short_hash",
                "s",
                if jj {
                    t!(en: "change id", ja: "change id")
                } else {
                    t!(en: "short hash", ja: "短ハッシュ")
                },
            ),
            (
                "full_hash",
                "h",
                if jj {
                    t!(en: "commit id", ja: "commit id")
                } else {
                    t!(en: "full hash", ja: "完全ハッシュ")
                },
            ),
            ("subject", "t", t!(en: "subject", ja: "件名")),
            ("message", "m", t!(en: "message", ja: "メッセージ")),
            ("author", "a", t!(en: "author", ja: "著者")),
            ("date", "d", t!(en: "date", ja: "日付")),
        ] {
            manifest = manifest.action(
                Act::new(format!("{screen}_copy_{field}"))
                    .copy()
                    .key(key)
                    .label(label)
                    .project_context(ActContext::screen(
                        screen,
                        if screen == "log" {
                            vec!["commit"]
                        } else {
                            vec![]
                        },
                    )),
            );
        }
    }
    manifest
}

pub(super) fn view(commits: Vec<CommitInfo>, cx: &ViewCx) -> View {
    if commits.is_empty() {
        return View::List(ListView {
            rows: vec![ListRow::new(
                "empty",
                vec![Line::from(vec![Span::styled(
                    t!(en: "  (no commits)", ja: "  (コミットなし)").get(cx.lang),
                    Style::new().dim(),
                )])],
            )
            .selectable(false)],
        });
    }
    let inline = cx.settings.str("commit_meta_align", "right") == "inline";
    View::List(ListView {
        rows: commits
            .into_iter()
            .map(|commit| row(commit, inline))
            .collect(),
    })
}

fn row(commit: CommitInfo, inline: bool) -> ListRow {
    let date = konoma_base::epoch::format_epoch_short(commit.time_epoch.max(0) as u64);
    let filter = format!(
        "{} {} {} {date}",
        commit.short, commit.summary, commit.author
    );
    let mut spans = vec![
        Span::styled(format!("{} ", commit.short), Style::new().dim()),
        Span::raw(commit.summary),
    ];
    if inline {
        spans.push(Span::styled(
            format!("  · {}  · {date}", commit.author),
            Style::new().dim(),
        ));
    }
    let row = ListRow::new("commit", vec![Line::from(spans)])
        .value(commit.id)
        .name(commit.short)
        .filter_text(filter);
    if inline {
        row
    } else {
        row.right(Line::from(vec![Span::styled(
            format!("{} · {date}", commit.author),
            Style::new().fg(Color::DarkGray),
        )]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_negative_epoch_clamps_to_zero() {
        for inline in [false, true] {
            let row = row(
                CommitInfo {
                    id: "full".into(),
                    short: "short".into(),
                    summary: "subject".into(),
                    author: "author".into(),
                    time_epoch: -1,
                },
                inline,
            );
            assert_eq!(
                row.filter_text.as_deref(),
                Some("short subject author 1970-01-01 00:00")
            );
            let meta = if inline {
                row.cells[0].plain()
            } else {
                row.right.unwrap().plain()
            };
            assert!(meta.ends_with("1970-01-01 00:00"));
        }
    }
}
