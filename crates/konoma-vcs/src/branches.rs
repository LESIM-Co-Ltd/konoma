//! Branch and bookmark lists, with Git writes guarded by the project host.

use konoma_plugin::prelude::*;
use konoma_plugin::view::{ListRow, ListView};

pub(super) fn view(refs: Vec<konoma_vcs_core::git::BranchInfo>, cx: &ViewCx) -> View {
    let mut rows: Vec<_> = refs
        .into_iter()
        .map(|branch| {
            ListRow::new(
                if branch.is_current {
                    "current"
                } else {
                    "branch"
                },
                vec![Line::from(vec![
                    Span::styled(
                        if branch.is_current { "* " } else { "  " },
                        Style::new().fg(Color::Green),
                    ),
                    Span::styled(
                        &branch.name,
                        if branch.is_current {
                            Style::new().fg(Color::Green).bold()
                        } else {
                            Style::new()
                        },
                    ),
                ])],
            )
            .initially_selected(branch.is_current)
            .name(&branch.name)
            .filter_text(&branch.name)
            .value(branch.name)
        })
        .collect();
    rows.push(
        ListRow::new(
            "empty",
            vec![Line::from(vec![Span::styled(
                t!(en: "  (no branches)", ja: "  (ブランチなし)").get(cx.lang),
                Style::new().dim(),
            )])],
        )
        .filter_fallback(true),
    );
    View::List(ListView { rows })
}

pub(super) fn declare(mut manifest: Manifest, git: bool) -> Manifest {
    manifest = manifest.action(
        Act::new("branch_copy_name")
            .copy()
            .key("n")
            .label(t!(en: "branch name", ja: "ブランチ名"))
            .project_context(ActContext::screen("branches", ["branch", "current"])),
    );
    if !git {
        return manifest;
    }
    manifest
        .action(write(
            "checkout",
            "Enter",
            t!(en: "checkout", ja: "切替"),
            t!(en: "checkout", ja: "切替"),
            0,
            &["branch", "current"],
        ))
        .action(
            write(
                "create_branch",
                "c",
                t!(en: "new", ja: "新規"),
                t!(en: "new branch", ja: "新規ブランチ"),
                1,
                &[],
            )
            .text_input(
                TextInput::new(t!(en: "New branch", ja: "新規ブランチ"), "").keep_on_failure(),
            ),
        )
        .action(
            write(
                "delete_branch",
                "d",
                t!(en: "delete", ja: "削除"),
                t!(en: "delete (y=-d / !=-D)", ja: "削除 (y=-d / !=-D)"),
                2,
                &["branch"],
            )
            .destructive()
            .confirmation(Confirmation::new(
                t!(en: "Delete branch: {row}", ja: "ブランチ削除: {row}"),
                vec![
                    ConfirmChoice::new(
                        'y',
                        "safe",
                        t!(en: "delete (safe, -d)", ja: "削除 (安全 -d)"),
                    )
                    .status(t!(en: "delete(-d)", ja: "削除(-d)")),
                    ConfirmChoice::new(
                        '!',
                        "force",
                        t!(en: "force delete (-D)", ja: "強制削除 (-D)"),
                    )
                    .status(t!(en: "force(-D)", ja: "強制(-D)")),
                ],
            )),
        )
        .action(write(
            "delete_current_branch",
            "d",
            t!(en: "delete", ja: "削除"),
            t!(en: "delete (y=-d / !=-D)", ja: "削除 (y=-d / !=-D)"),
            2,
            &["current"],
        ))
}

fn write(id: &str, key: &str, label: Tr, help: Tr, priority: u8, kinds: &[&str]) -> Act {
    Act::new(id)
        .key(key)
        .label(label)
        .help(help)
        .footer(priority)
        .writes()
        .project_context(ActContext::screen("branches", kinds.iter().copied()))
        .refresh(RefreshTarget::CurrentScreen)
        .refresh(RefreshTarget::TreeStatus)
}
