use super::actions_tests::{project, Fixture};
use super::*;
use crate::project::ProjectView;
use crate::view::{ListRow, ListView};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

fn info(id: u64) -> ProjectContextInfo<()> {
    ProjectContextInfo {
        id,
        root: format!("/context/{id}").into(),
        open_dir: format!("/context/{id}/open").into(),
        home: Some("/home".into()),
        icons: true,
        viewport: ProjectViewport {
            cols: 80 + id as u16,
            rows: 20,
        },
        surface: (),
    }
}

fn reply() -> ProjectReply<()> {
    ProjectReply {
        watch_paths: None,
        views: vec![],
        enabled: HashMap::new(),
        chip: None,
        chip_error: None,
        effect: None,
        error: None,
        act_failed: false,
        contexts: HashMap::new(),
    }
}

#[test]
fn reply_remaps_active_return_and_inactive_contexts() {
    let mut f = Fixture::new();
    let mut returned = f.state.clone().unwrap();
    let mut inactive = ProjectScreenState::new(project(), ScreenKey::new("list", 19));
    let mut inactive_return = inactive.clone();
    let mut pending = VecDeque::new();
    let mut r = reply();
    let runtime = f.runtimes.get_mut(&project()).unwrap();
    runtime.views.insert(
        ScreenKey::new("list", 19),
        runtime.views[&ScreenKey::new("list", 7)].clone(),
    );
    for id in [7, 19] {
        r.views.push((
            ScreenKey::new("list", id),
            Arc::new(ProjectContent::List(ListView {
                rows: vec![
                    ListRow::new("file", vec![]).value("b"),
                    ListRow::new("file", vec![]).value("a"),
                ],
                ..Default::default()
            })),
        ));
        r.contexts
            .insert(ScreenKey::new("list", id), ProjectView::new("list").tab(id));
    }
    runtime.pending = Some(r);
    let (mut host, mut active) = f.parts();
    active.returned = Some(&mut returned);
    let mut contexts = [
        active,
        ProjectContextMut {
            id: 19,
            active: Some(&mut inactive),
            returned: Some(&mut inactive_return),
            pending_acts: &mut pending,
            request: None,
        },
    ];
    assert!(
        host.poll(&mut contexts, &[info(7), info(19)], &[project()])
            .changed
    );
    for state in [
        contexts[0].active.as_ref().unwrap(),
        contexts[0].returned.as_ref().unwrap(),
        contexts[1].active.as_ref().unwrap(),
        contexts[1].returned.as_ref().unwrap(),
    ] {
        assert_eq!(
            state.frame().unwrap().cursor,
            1,
            "selected row a follows its identity"
        );
    }
}

#[test]
fn closed_context_does_not_reenter_cache_from_inflight_reply() {
    let mut f = Fixture::new();
    let invocation = f.request();
    let key = ScreenKey::new("list", 7);
    let runtime = f.runtimes.get_mut(&project()).unwrap();
    runtime.busy = true;
    runtime.pending_retry = Some(Box::new(invocation));
    runtime.dirty.insert(key.clone());
    let mut late = reply();
    late.views.push((key.clone(), runtime.views[&key].clone()));
    late.contexts
        .insert(key.clone(), ProjectView::new("list").tab(7));
    let (released, replies) = std::sync::mpsc::sync_channel(1);
    runtime.rx = replies;
    let (mut host, _) = f.parts();
    host.close_context(7);
    let runtime = &host.runtimes[&project()];
    assert!(runtime.views.is_empty());
    assert!(runtime.contexts.is_empty());
    assert!(runtime.dirty.is_empty());
    assert!(runtime.pending_retry.is_none());
    assert_eq!(runtime.closed_tabs.iter().copied().collect::<Vec<_>>(), [7]);
    released.send(late).unwrap();
    host.poll(&mut [], &[info(19)], &[project()]);
    let runtime = &host.runtimes[&project()];
    assert!(
        runtime.views.is_empty(),
        "late reply cannot revive a closed context"
    );
    assert!(runtime.contexts.is_empty());
    assert!(matches!(f.jobs.try_recv().unwrap(), ProjectJob::TabsClosed { tabs } if tabs == [7]));
}

#[test]
fn invalidated_reply_does_not_replace_view_or_context() {
    let mut f = Fixture::new();
    let key = ScreenKey::new("list", 7);
    let runtime = f.runtimes.get_mut(&project()).unwrap();
    let old = runtime.views[&key].clone();
    let old_context = runtime.contexts[&key].clone();
    runtime.busy = true;
    runtime.chip = Some(crate::Chip::new("old"));
    let mut r = reply();
    r.views
        .push((key.clone(), Arc::new(ProjectContent::Lines(vec![]))));
    r.contexts.insert(
        key.clone(),
        ProjectView::new("list")
            .tab(7)
            .paths("/stale", "/stale/open"),
    );
    r.chip = Some(Some(crate::Chip::new("stale")));
    r.watch_paths = Some(vec!["/watch".into()]);
    r.enabled.insert("new".into(), true);
    r.effect = Some(());
    r.error = Some(crate::Failure::external(crate::Tr::new("reply error")));
    runtime.pending = Some(r);
    let (mut host, mut context) = f.parts();
    host.invalidate(&project(), ProjectInvalidation::Detection);
    assert!(host.runtimes[&project()].invalidated);
    let update = host.poll(std::slice::from_mut(&mut context), &[info(7)], &[project()]);
    let runtime = &host.runtimes[&project()];
    assert!(Arc::ptr_eq(&runtime.views[&key], &old));
    assert_eq!(runtime.contexts[&key], old_context);
    assert_eq!(
        runtime.chip.as_ref().unwrap().text,
        crate::Chip::new("old").text
    );
    assert!(runtime.dirty.contains(&key));
    assert!(!runtime.invalidated);
    assert!(runtime.enabled["new"]);
    assert!(runtime.error.is_some());
    assert!(
        matches!(update.events.as_slice(), [ProjectEvent::Watch(_, paths), ProjectEvent::Effect(()), ProjectEvent::EnsureView, ProjectEvent::Error(_)] if paths == &[std::path::PathBuf::from("/watch")])
    );
    assert!(matches!(f.jobs.try_recv().unwrap(), ProjectJob::Watch));
}

fn effect() -> ProjectEffect<()> {
    ProjectEffect {
        origin: ProjectOrigin {
            context: 7,
            project: project(),
            root: info(7).root,
            screen: Some(ScreenKey::new("list", 7)),
            surface: (),
        },
        outcome: Some(crate::Outcome::PushScreen {
            screen: "detail".into(),
            selection: Some("a".into()),
        }),
        tree_status: true,
        label: "Run".into(),
    }
}

#[test]
fn navigation_effects_require_matching_origin() {
    for field in [
        "matching",
        "root",
        "context",
        "project",
        "screen",
        "selection",
    ] {
        let mut f = Fixture::new();
        f.runtimes.get_mut(&project()).unwrap().project.declaration = crate::ProjectDecl::new()
            .screen(crate::ProjectScreen::new("detail", "Detail").pushed());
        let mut e = effect();
        match field {
            "root" => e.origin.root = "/other".into(),
            "context" => e.origin.context = 19,
            "project" => e.origin.project.marker = "other".into(),
            "screen" => e.origin.screen.as_mut().unwrap().screen = "other".into(),
            "selection" => e.origin.screen.as_mut().unwrap().selection = Some("other".into()),
            _ => {}
        }
        let (mut host, mut context) = f.parts();
        let mut open = e.clone();
        open.outcome = Some(crate::Outcome::OpenFile {
            path: "/file".into(),
            line: Some(4),
        });
        let update = host.apply_effect(&mut context, &info(7), open);
        assert_eq!(
            update
                .commands
                .iter()
                .any(|command| matches!(command, ProjectCommand::Navigation { .. })),
            field == "matching",
            "navigation {field}"
        );
        host.apply_effect(&mut context, &info(7), e);
        assert_eq!(
            context.active.as_ref().unwrap().stack.len(),
            if field == "matching" { 2 } else { 1 },
            "{field}"
        );
    }
    let mut f = Fixture::new();
    f.state = None;
    f.runtimes.get_mut(&project()).unwrap().project.declaration =
        crate::ProjectDecl::new().screen(crate::ProjectScreen::new("detail", "Detail").pushed());
    let mut e = effect();
    e.origin.screen = None;
    let (mut host, mut context) = f.parts();
    let update = host.apply_effect(&mut context, &info(7), e);
    assert!(
        matches!(update.commands.as_slice(), [ProjectCommand::TreeStatus, ProjectCommand::StartScreen { project: p, screen }] if p == &project() && screen.screen == "detail" && screen.selection.as_deref() == Some("a") && screen.tab == 7)
    );
}

#[test]
fn non_navigation_effects_keep_existing_acceptance_rules() {
    for outcome in [
        Some(crate::Outcome::Copy {
            text: "copied".into(),
        }),
        Some(crate::Outcome::Flash(crate::Tr::new("message"))),
        Some(crate::Outcome::OpenFile {
            path: "/file".into(),
            line: Some(4),
        }),
        None,
    ] {
        let mut f = Fixture::new();
        let mut e = effect();
        e.origin.screen = Some(ScreenKey::new("old", 7));
        e.outcome = outcome.clone();
        let (mut host, mut context) = f.parts();
        let update = host.apply_effect(&mut context, &info(7), e.clone());
        assert!(matches!(
            update.commands.first(),
            Some(ProjectCommand::TreeStatus)
        ));
        assert!(matches!(&update.commands[1], ProjectCommand::Label(label) if label == "Run"));
        match outcome {
            Some(crate::Outcome::Copy { .. }) => assert!(
                matches!(&update.commands[2], ProjectCommand::Copy(text) if text == "copied")
            ),
            Some(crate::Outcome::Flash(_)) => assert!(
                matches!(&update.commands[2], ProjectCommand::Flash(message) if message.get("en") == "message")
            ),
            _ => assert_eq!(update.commands.len(), 2),
        }
        e.origin.root = "/other".into();
        assert!(host
            .apply_effect(&mut context, &info(7), e.clone())
            .commands
            .is_empty());
        e.origin.root = info(7).root;
        e.outcome = Some(crate::Outcome::Unhandled);
        assert!(host
            .apply_effect(&mut context, &info(7), e)
            .commands
            .is_empty());
    }
}

#[test]
fn effect_order_uses_state_after_previous_effect() {
    let mut f = Fixture::new();
    f.runtimes.get_mut(&project()).unwrap().project.declaration =
        crate::ProjectDecl::new().screen(crate::ProjectScreen::new("detail", "Detail").pushed());
    let (mut host, mut context) = f.parts();
    host.apply_effect(&mut context, &info(7), effect());
    let mut pop = effect();
    pop.outcome = Some(crate::Outcome::PopScreen);
    host.apply_effect(&mut context, &info(7), pop.clone());
    assert_eq!(
        context.active.as_ref().unwrap().stack.len(),
        2,
        "old-screen pop is judged after push"
    );
    pop.origin.screen = Some(ScreenKey {
        screen: "detail".into(),
        tab: 7,
        selection: Some("a".into()),
    });
    host.apply_effect(&mut context, &info(7), pop);
    assert_eq!(context.active.as_ref().unwrap().stack.len(), 1);
    let mut pop_root = effect();
    pop_root.outcome = Some(crate::Outcome::PopScreen);
    host.apply_effect(&mut context, &info(7), pop_root);
    assert_eq!(
        context.active.as_ref().unwrap().stack.len(),
        1,
        "root pop is a no-op"
    );
    for outcome in [
        crate::Outcome::OpenFile {
            path: "/file".into(),
            line: Some(4),
        },
        crate::Outcome::OpenDiff {
            path: "/diff".into(),
        },
        crate::Outcome::SwitchRoot {
            root: "/new".into(),
            message: None,
        },
        crate::Outcome::OpenTab {
            path: "/tab".into(),
        },
    ] {
        let save = matches!(
            outcome,
            crate::Outcome::OpenFile { .. } | crate::Outcome::OpenDiff { .. }
        );
        let mut e = effect();
        e.outcome = Some(outcome.clone());
        let update = host.apply_effect(&mut context, &info(7), e);
        assert!(
            matches!(update.commands.last(), Some(ProjectCommand::Navigation { outcome: actual, save_return }) if actual == &outcome && *save_return == save)
        );
    }
}

struct RetryPlugin(&'static str);
struct RetrySession {
    failure: &'static str,
    acted: bool,
}
impl crate::Plugin for RetryPlugin {
    fn manifest(&self) -> crate::Manifest {
        crate::Manifest::new("test", "1")
    }
    fn open(
        &self,
        _: crate::Input,
        _: &crate::OpenCx,
    ) -> Result<Box<dyn crate::Session>, crate::Failure> {
        Ok(Box::new(RetrySession {
            failure: self.0,
            acted: false,
        }))
    }
}
impl crate::Session for RetrySession {
    fn view(
        &mut self,
        _: &crate::view::ViewReq,
        _: &crate::ViewCx,
    ) -> Result<crate::View, crate::Failure> {
        Err(crate::Failure::external(crate::Tr::new("view error")))
    }
    fn act(&mut self, _: &str, _: &crate::ActCx) -> Result<crate::Outcome, crate::Failure> {
        self.acted = true;
        if self.failure == "act" {
            Err(crate::Failure::external(crate::Tr::new("act error")))
        } else {
            Ok(crate::Outcome::Redraw)
        }
    }
    fn watch_paths(
        &mut self,
        _: &crate::ViewCx,
    ) -> Result<Option<Vec<std::path::PathBuf>>, crate::Failure> {
        if self.acted && self.failure == "watch" {
            Err(crate::Failure::external(crate::Tr::new("watch error")))
        } else {
            Ok(None)
        }
    }
    fn chip(&mut self, _: &crate::ViewCx) -> Result<Option<crate::Chip>, crate::Failure> {
        if self.acted && self.failure == "chip" {
            Err(crate::Failure::external(crate::Tr::new("chip error")))
        } else {
            Ok(None)
        }
    }
}

#[test]
fn retry_is_only_for_act_failure_and_current_unblocked_origin() {
    for failure in ["act", "view", "watch", "chip"] {
        let mut f = Fixture::new();
        let mut invocation = f.request();
        invocation.declaration.text_input =
            Some(crate::TextInput::new("Input", "").keep_on_failure());
        invocation.context.text = Some("answer".into());
        let mut runtime = ProjectRuntime::start(
            f.runtimes[&project()].project.clone(),
            Arc::new(RetryPlugin(failure)),
            vec![],
            None,
            || ProjectEnv {
                settings: crate::Settings::empty().clone(),
                lang: "en".into(),
                theme: "TwoDark".into(),
                term: crate::TermCaps::new(),
                read_scope: crate::ReadScope::unrestricted(),
                messages: ProjectMessages {
                    worker_failed: "failed".into(),
                    no_changes: "none".into(),
                    mermaid_caption: "diagram".into(),
                    view_unsupported: "unsupported".into(),
                    outside_read_dirs: "outside".into(),
                    invalid_watch_paths: crate::Tr::new("invalid"),
                },
            },
            |_: &mut (), _| {},
        );
        let timeout = std::time::Duration::from_secs(20);
        runtime.rx.recv_timeout(timeout).unwrap();
        runtime.ready = true;
        runtime.busy = true;
        runtime.acting = true;
        runtime.pending_retry = Some(Box::new(invocation.clone()));
        runtime
            .tx
            .send(ProjectJob::Act {
                action: "run".into(),
                context: invocation.context.clone(),
                refresh: if failure == "view" {
                    vec![(ScreenKey::new("list", 7), crate::ViewReq::new(80, 20))]
                } else {
                    vec![]
                },
                chip: failure == "chip",
                watch: failure == "watch",
                effect: Box::new(()),
            })
            .unwrap();
        runtime.pending = Some(runtime.rx.recv_timeout(timeout).unwrap());
        f.runtimes.insert(project(), runtime);
        let (mut host, mut context) = f.parts();
        let update = host.poll(std::slice::from_mut(&mut context), &[info(7)], &[project()]);
        assert!(host.runtimes[&project()].pending_retry.is_none());
        let order: Vec<_> = update
            .events
            .iter()
            .map(|event| match event {
                ProjectEvent::Watch(..) => "watch",
                ProjectEvent::Effect(..) => "effect",
                ProjectEvent::EnsureView => "ensure",
                ProjectEvent::Error(..) => "error",
                ProjectEvent::ChipError(..) => "chip_error",
                ProjectEvent::Retry(..) => "retry",
            })
            .collect();
        assert_eq!(
            order,
            match failure {
                "act" => vec!["ensure", "error", "retry"],
                "chip" => vec!["effect", "ensure", "chip_error"],
                _ => vec!["effect", "ensure", "error"],
            }
        );
        let mut retries = update.events.into_iter().filter_map(|event| match event {
            ProjectEvent::Retry(i) => Some(i),
            _ => None,
        });
        if failure != "act" {
            assert!(retries.next().is_none(), "{failure} is a follow-up failure");
            continue;
        }
        let retry = retries
            .next()
            .expect("act failure offers its retry after errors");
        assert!(retries.next().is_none());
        for condition in [
            "context", "project", "root", "screen", "dialog", "text", "filter", "matching",
        ] {
            let mut current = retry.origin.clone();
            let mut blockers = ProjectPromptBlockers::default();
            match condition {
                "context" => current.context = 19,
                "project" => current.project.marker = "other".into(),
                "root" => current.root = "/other".into(),
                "screen" => current.screen = Some(ScreenKey::new("other", 7)),
                "dialog" => blockers.dialog = true,
                "text" => blockers.text_input = true,
                "filter" => blockers.filtering = true,
                _ => {}
            }
            *host.dialog = None;
            assert_eq!(
                host.reopen_retry(retry.clone(), &current, blockers),
                condition == "matching",
                "{condition}"
            );
        }
    }
}

#[test]
fn refresh_named_and_all_screens_keep_each_context_paths() {
    for target in [
        crate::RefreshTarget::screen("list"),
        crate::RefreshTarget::AllScreens,
    ] {
        let mut f = Fixture::new();
        let runtime = f.runtimes.get_mut(&project()).unwrap();
        runtime.views.insert(
            ScreenKey::new("list", 19),
            Arc::new(ProjectContent::Lines(vec![])),
        );
        runtime.views.insert(
            ScreenKey::new("list", 99),
            Arc::new(ProjectContent::Lines(vec![])),
        );
        let mut invocation = f.request();
        invocation.declaration.refresh = vec![target];
        let facts = f.facts();
        let live = [info(7), info(19)];
        let (mut host, mut context) = f.parts();
        host.execute_act(&mut context, invocation, &facts, |key| {
            effects::request_for(&live, key)
        });
        match f.jobs.try_recv().unwrap() {
            ProjectJob::Act { refresh, .. } => {
                assert_eq!(refresh.len(), 2, "closed context has no request");
                for (index, id) in [7, 19].into_iter().enumerate() {
                    let (key, req) = &refresh[index];
                    assert_eq!(key.tab, id);
                    assert_eq!(req.cols, 80 + id as u16);
                    assert_eq!(req.rows, 20);
                    let view = req.project.as_ref().unwrap();
                    assert_eq!(view.tree_root, Some(format!("/context/{id}").into()));
                    assert_eq!(view.open_dir, Some(format!("/context/{id}/open").into()));
                    assert_eq!(view.home_dir, Some("/home".into()));
                    assert!(view.icons);
                }
            }
            _ => panic!("act"),
        }
    }
    assert!(
        effects::request(&info(7), &ScreenKey::new("list", 19)).is_none(),
        "context-ID mismatch must not make a request"
    );
    let key = ScreenKey {
        tab: 19,
        screen: "detail".into(),
        selection: Some("selected".into()),
    };
    let req = effects::request(&info(19), &key).unwrap();
    assert_eq!(req.project.unwrap().selection.as_deref(), Some("selected"));
}
