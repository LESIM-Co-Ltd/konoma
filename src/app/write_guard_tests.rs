//! Execution boundaries remain guarded when a caller bypasses key dispatch.

use super::*;
use crate::i18n::{tr, Msg};
use crate::plugins::test_plugins::{self, ProjectFixture};
use crate::test_support::unique_tmp;
use konoma_plugin::project::{DetectionMarker, MarkerKind, ProjectDecl, ProjectScreen};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn project_guard_rechecks_immediate_and_queued_acts_without_taking_the_session() {
    let dir = unique_tmp("konoma_project_guard_executor");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("view.project-test");
    std::fs::write(&path, "fixture").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let marker = test_plugins::register(
        "project_guard_executor",
        Arc::new(ProjectFixture {
            id: "project-guard-executor",
            project: ProjectDecl::new()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .screen(ProjectScreen::new("items", "Items").row_kind("item")),
            calls: calls.clone(),
            navigation: None,
        }),
    );
    let cfg = test_plugins::marker_config(&marker);
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.enter_preview(&path);
    assert!(app.plugin.as_ref().unwrap().session.is_some());
    app.cfg.plugins.insert(
        "project-guard-executor".into(),
        toml::Table::from_iter([("read_only".into(), true.into())]),
    );
    assert!(!app.plugin_act("write"));
    app.send_queued_act("write".into());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(app.plugin.as_ref().unwrap().session.is_some());
    assert_eq!(app.flash.as_deref(), Some(tr(app.lang, Msg::PluginReadOnly)));
    app.cfg.plugins.remove("project-guard-executor");
    assert!(app.plugin_act("write"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "git")]
#[test]
fn guard_refuses_every_git_job_even_without_an_action_declaration_or_a_repository() {
    let dir = unique_tmp("konoma_guard_git_executor");
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::default();
    cfg.plugins.insert(
        "git".into(),
        toml::Table::from_iter([("read_only".into(), true.into())]),
    );
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    for kind in [
        GitOpKind::Stage,
        GitOpKind::Unstage,
        GitOpKind::StageAll,
        GitOpKind::UnstageAll,
        GitOpKind::Discard,
        GitOpKind::Commit,
        GitOpKind::Checkout,
        GitOpKind::CreateBranch,
        GitOpKind::DeleteBranch,
        GitOpKind::WorktreeAdd,
    ] {
        assert!(
            !app.start_git_op(GitOpJob {
                kind,
                root: PathBuf::new(),
                tab_id: 0,
                path: dir.join("untouched"),
                text: "never executed".into(),
                flag: false,
                label: "fixture".into(),
                count: 0,
            }),
            "{kind:?}"
        );
        assert_eq!(app.flash.as_deref(), Some(tr(app.lang, Msg::PluginReadOnly)));
        assert!(!app.git_op_running());
    }
    assert!(!dir.join("untouched").exists());
}

#[cfg(feature = "git")]
#[test]
fn dialog_submit_rechecks_the_guard_and_keeps_the_unexecuted_commit_message() {
    let dir = unique_tmp("konoma_guard_dialog_submit");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.dialog = Some(Dialog {
        op: PendingOp::GitCommit,
        kind: DialogKind::Input {
            title: tr(app.lang, Msg::GitCommit).into(),
            buffer: "keep this message".into(),
            cursor: 17,
        },
    });
    app.cfg.plugins.insert(
        "git".into(),
        toml::Table::from_iter([("read_only".into(), true.into())]),
    );
    app.dialog_submit().unwrap();
    assert_eq!(app.flash.as_deref(), Some(tr(app.lang, Msg::PluginReadOnly)));
    assert!(!app.git_op_running());
    assert!(
        matches!(app.dialog, Some(Dialog { kind: DialogKind::Input { ref buffer, .. }, .. }) if buffer == "keep this message")
    );
}
