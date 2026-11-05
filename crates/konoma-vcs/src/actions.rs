use konoma_plugin::prelude::*;

fn action(id: &str, key: &str, label: Tr, help: Tr, priority: u8, file: bool) -> Act {
    Act::new(id)
        .key(key)
        .label(label)
        .help(help)
        .footer(priority)
        .writes()
        .project_context(ActContext::screen(
            "changes",
            if file { vec!["file"] } else { vec![] },
        ))
        .refresh(RefreshTarget::CurrentScreen)
        .refresh(RefreshTarget::TreeStatus)
}

fn yes() -> Vec<ConfirmChoice> {
    vec![ConfirmChoice::new('y', "yes", t!(en: "yes", ja: "はい"))]
}

pub(super) fn declare(manifest: Manifest, git: bool) -> Manifest {
    if !cfg!(feature = "git") {
        return manifest;
    }
    if !git {
        return manifest
            .setting(SettingDecl::new(
                "confirm_sync",
                Value::Bool(true),
                t!(en: "Ask before letting jj snapshot the working copy", ja: "jj に作業コピーを取り込ませる前に確認する"),
            ))
            .action(
                action(
                    "sync", "s", t!(en: "sync", ja: "同期"),
                    t!(en: "let jj snapshot the working copy (confirm)", ja: "jj に作業コピーを取り込ませる (確認)"),
                    0, false,
                )
                .confirmation(Confirmation::new(
                    t!(en: "Let jj snapshot the working copy? (jj undo reverses it)", ja: "jj に作業コピーを取り込ませますか?(jj undo で戻せます)"),
                    yes(),
                ).setting("confirm_sync"))
                .refresh(RefreshTarget::Chip),
            );
    }
    let stage = t!(en: "stage(all)", ja: "ステージ(全)");
    let stage_help = t!(en: "stage / stage all", ja: "ステージ / 全ステージ");
    let unstage = t!(en: "unstage(all)", ja: "解除(全)");
    let unstage_help = t!(en: "unstage / unstage all", ja: "アンステージ / 全アンステージ");
    manifest
        .hint_group(HintGroup::new("stage", stage.clone()).help(stage_help.clone()))
        .hint_group(HintGroup::new("unstage", unstage.clone()).help(unstage_help.clone()))
        .action(
            action("stage", "s", stage.clone(), stage_help.clone(), 0, true).hint_group("stage"),
        )
        .action(action("stage_all", "S", stage, stage_help, 0, false).hint_group("stage"))
        .action(
            action(
                "unstage",
                "u",
                unstage.clone(),
                unstage_help.clone(),
                1,
                true,
            )
            .hint_group("unstage"),
        )
        .action(action("unstage_all", "U", unstage, unstage_help, 1, false).hint_group("unstage"))
        .action(
            action(
                "discard",
                "x",
                t!(en: "discard", ja: "破棄"),
                t!(en: "discard file (confirm)", ja: "ファイル破棄 (確認)"),
                2,
                true,
            )
            .destructive()
            .confirmation(Confirmation::new(
                t!(en: "Discard changes to {row} ?", ja: "変更を破棄しますか: {row} ?"),
                yes(),
            )),
        )
        .action(
            action(
                "commit",
                "c",
                t!(en: "commit", ja: "コミット"),
                t!(en: "commit (staged)", ja: "コミット (ステージ済み)"),
                3,
                false,
            )
            .text_input(
                TextInput::new(t!(en: "Commit message", ja: "コミットメッセージ"), "")
                    .keep_on_failure(),
            )
            .refresh(RefreshTarget::Chip),
        )
}

pub(super) fn failed(message: &str) -> Failure {
    // Project action failures are flashed verbatim by the host.
    Failure::external(t!(en: format!("failed: {message}"), ja: format!("失敗: {message}")))
}

pub(super) fn branch_act(
    root: &std::path::Path,
    action: &str,
    context: &ProjectAct,
) -> Result<Outcome, Failure> {
    if action == "create_branch" {
        let Some(name) = context.text.as_deref() else {
            return Ok(Outcome::Unhandled);
        };
        let name = name.trim();
        if name.is_empty() {
            return Ok(Outcome::Flash(t!(en: "name is empty", ja: "名前が空です")));
        }
        konoma_vcs_core::git::create_branch(root, name)
            .map_err(|e| Failure::external(e.to_string()))?;
        return Ok(Outcome::Flash(
            t!(en: format!("Created branch: {name}"), ja: format!("ブランチ作成: {name}")),
        ));
    }
    let Some(row) = context
        .selected
        .as_ref()
        .filter(|r| matches!(r.kind.as_str(), "branch" | "current"))
    else {
        return Ok(Outcome::Unhandled);
    };
    let name = &row.value;
    match action {
        "branch_copy_name" => Ok(Outcome::Copy { text: name.clone() }),
        "checkout" => {
            konoma_vcs_core::git::checkout(root, name)
                .map_err(|e| Failure::external(e.to_string()))?;
            Ok(Outcome::Flash(
                t!(en: format!("Switched to: {name}"), ja: format!("切替: {name}")),
            ))
        }
        "delete_current_branch" | "delete_branch" if row.kind == "current" => Ok(Outcome::Flash(
            t!(en: "cannot delete the current branch", ja: "現在のブランチは削除できません"),
        )),
        "delete_branch" => {
            let force = match context.confirmation.as_deref() {
                Some("safe") => false,
                Some("force") => true,
                _ => return Ok(Outcome::Unhandled),
            };
            konoma_vcs_core::git::delete_branch(root, name, force)
                .map_err(|e| Failure::external(e.to_string()))?;
            Ok(Outcome::Flash(
                t!(en: format!("Deleted branch: {name}"), ja: format!("ブランチ削除: {name}")),
            ))
        }
        _ => Ok(Outcome::Unhandled),
    }
}
