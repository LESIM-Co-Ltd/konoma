//! Regression checks for the shared policy and its execution boundaries.

use super::write_guard::write_guard;
use super::Gate;
use crate::app::App;
use crate::config::Config;
use crate::i18n::Msg;
use crate::test_support::unique_tmp;

#[test]
fn guard_checks_the_writing_plugins_table_in_both_languages() {
    let dir = unique_tmp("konoma_write_guard");
    std::fs::create_dir_all(&dir).unwrap();
    for lang in ["en", "ja"] {
        for value in [
            None,
            Some(toml::Value::Boolean(false)),
            Some(toml::Value::Boolean(true)),
            Some(toml::Value::String("true".into())),
        ] {
            let mut cfg = Config::default();
            cfg.ui.lang = lang.into();
            let mut table = toml::Table::new();
            if let Some(value) = value.clone() {
                table.insert("read_only".into(), value);
            }
            cfg.plugins.insert("project-guard".into(), table);
            let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
            let blocked = value == Some(toml::Value::Boolean(true));
            let want = if blocked {
                Gate::Off(Some(Msg::PluginReadOnly))
            } else {
                Gate::On
            };
            assert_eq!(write_guard(&app, "project-guard", true, false), want);
            assert_eq!(write_guard(&app, "other-project", true, false), Gate::On);
            assert_eq!(write_guard(&app, "project-guard", false, false), Gate::On);
            assert_eq!(app.allow_write("project-guard", true, false), !blocked);
            if blocked {
                assert_eq!(
                    app.flash.as_deref(),
                    Some(crate::i18n::tr(app.lang, Msg::PluginReadOnly))
                );
            } else {
                assert!(app.flash.is_none());
            }
        }
    }
}

#[test]
fn guard_plugin_declaration_cannot_be_overridden_by_configuration() {
    use crate::plugins::test_plugins;
    use crate::plugins::test_projects::{ProjectLog, Screens};
    use std::sync::{Arc, Mutex};
    let dir = unique_tmp("project_declared_read_only_guard");
    std::fs::create_dir_all(&dir).unwrap();
    for read_only in [false, true] {
        let marker = test_plugins::register(
            if read_only {
                "guard-declared-read-only"
            } else {
                "guard-declared-writable"
            },
            Arc::new(Screens {
                log: Arc::new(Mutex::new(ProjectLog::default())),
                read_only,
                hold: None,
                started: None,
                outcome: None,
            }),
        );
        for configured in [false, true] {
            let mut cfg = test_plugins::marker_config(&marker);
            cfg.external.git = false;
            cfg.plugins.insert(
                "project-screens".into(),
                toml::Table::from_iter([("read_only".into(), configured.into())]),
            );
            let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
            assert_eq!(
                write_guard(&app, "project-screens", true, false).is_on(),
                !(read_only || configured)
            );
            assert_eq!(write_guard(&app, "project-screens", false, false), Gate::On);
            assert_eq!(
                app.allow_write("project-screens", true, false),
                !(read_only || configured)
            );
        }
    }
}

#[cfg(feature = "git")]
#[test]
fn guard_legacy_git_and_jj_read_only_settings_are_accepted_and_type_checked() {
    for id in ["git", "jj"] {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
            let mut tables = crate::plugins::PluginTables::new();
            tables.insert(
                id.into(),
                toml::Table::from_iter([("read_only".into(), true.into())]),
            );
            assert!(crate::plugins::table_problems(&tables, lang).is_empty());
            tables
                .get_mut(id)
                .unwrap()
                .insert("read_only".into(), "true".into());
            let problems = crate::plugins::table_problems(&tables, lang);
            assert_eq!(problems.len(), 1);
            assert!(problems[0].contains("read_only"));
        }
    }
}

fn method<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source.find(&format!("fn {name}(")).unwrap();
    let tail = &source[start..];
    let end = tail.find("\n    }\n").unwrap();
    &tail[..end]
}

#[test]
fn guard_is_before_each_write_executor_and_before_a_session_is_taken() {
    let git = include_str!("../app/git_view.rs");
    let start = method(git, "start_git_op");
    assert!(
        start.find("allow_write(\"git\", true, true)").unwrap()
            < start.find("gitop_pending").unwrap()
    );
    assert_eq!(
        git.matches("run_git_op(").count(),
        start.matches("run_git_op(").count() + 1,
        "only start_git_op may call the executor (the extra occurrence is its definition)"
    );
    let sync = method(git, "jj_run_sync");
    assert!(
        sync.find("allow_write(\"jj\", true, false)").unwrap()
            < sync.find("vcs::jj::snapshot(").unwrap()
    );
    let plugin = include_str!("../app/plugin_host.rs");
    for name in ["plugin_act", "send_queued_act"] {
        let body = method(plugin, name);
        assert!(
            body.find("allow_plugin_write(").unwrap() < body.find("session.take()").unwrap(),
            "{name}"
        );
    }
    assert_eq!(
        plugin.matches("PluginJob::Act {").count(),
        method(plugin, "plugin_act")
            .matches("PluginJob::Act {")
            .count()
            + method(plugin, "send_queued_act")
                .matches("PluginJob::Act {")
                .count()
            + 1,
        "only the guarded methods construct act jobs (the extra occurrence is the worker match)"
    );
    let policy = include_str!("../app/plugin_acts.rs");
    let body = method(policy, "allow_plugin_write");
    assert!(body.contains("b.act.act() == action && b.writes"));
    assert!(body.contains("self.allow_write(&id, writes, false)"));
    let project = include_str!("../app/project_screens.rs");
    let execute = method(project, "execute_project_act");
    assert!(execute.contains("invocation.declaration.writes"));
    assert!(execute.find("allow_write(").unwrap() < execute.find("ProjectJob::Act {").unwrap());
    assert!(method(project, "request_project_act").contains("allow_write("));
    assert_eq!(
        project.matches("ProjectJob::Act {").count(),
        1,
        "one guarded project action dispatcher"
    );
    let worker = include_str!("../app/project_worker.rs");
    assert_eq!(
        worker.matches("ProjectJob::Act {").count(),
        1,
        "only the serial owner consumes project actions"
    );
    assert_eq!(worker.matches("plugin_host::act_session(").count(), 1);
    assert_eq!(
        plugin.matches("act_session(").count(),
        2,
        "definition and legacy guarded dispatcher"
    );
}

fn sources(directory: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        // Test fixtures may deliberately call write APIs to build their repositories.
        if name.contains("test") {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn guard_source_scan_rejects_repository_writes_outside_the_known_executor() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let names = [
        "stage",
        "unstage",
        "stage_all",
        "unstage_all",
        "discard",
        "commit",
        "checkout",
        "create_branch",
        "delete_branch",
        "worktree_add",
    ];
    let mut git_calls = 0;
    let mut jj_calls = 0;
    let mut plugin_calls = 0;
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        for name in names {
            let call = format!("git::{name}(");
            if text.contains(&call) {
                git_calls += text.matches(&call).count();
                assert_eq!(
                    path,
                    src.join("app/git_view.rs"),
                    "{call} bypasses the guarded executor"
                );
                assert_eq!(
                    text.matches(&call).count(),
                    method(&text, "run_git_op").matches(&call).count()
                );
            }
        }
        if text.contains("vcs::jj::snapshot(") {
            jj_calls += text.matches("vcs::jj::snapshot(").count();
            assert_eq!(path, src.join("app/git_view.rs"));
            assert_eq!(
                text.matches("vcs::jj::snapshot(").count(),
                method(&text, "jj_run_sync")
                    .matches("vcs::jj::snapshot(")
                    .count()
            );
        }
        if text.contains("session.act(") {
            plugin_calls += text.matches("session.act(").count();
            assert_eq!(
                path,
                src.join("app/plugin_host.rs"),
                "plugin acts must use the guarded dispatcher"
            );
            assert_eq!(text.matches("session.act(").count(), 1);
        }
    }
    assert!(
        git_calls >= names.len(),
        "each of the ten known git write APIs must be scanned"
    );
    assert!(jj_calls >= 1, "the jj snapshot write must be scanned");
    assert!(plugin_calls >= 1, "the plugin act executor must be scanned");
    let project = include_str!("../app/project_screens.rs");
    assert_eq!(project.matches("ProjectJob::Act {").count(), 1);
    assert!(method(project, "execute_project_act").contains("allow_write("));
    eprintln!("guarded write paths: git={git_calls}, jj={jj_calls}, plugin={plugin_calls}, project=1; lower bound=13");
}
