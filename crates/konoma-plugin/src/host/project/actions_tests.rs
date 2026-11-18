use super::actions::action_key;
use super::*;
use crate::project::{ActContext, ProjectAct, ProjectRow, ProjectView};
use crate::view::{ListRow, ListView, ViewReq};
use std::collections::{HashMap, VecDeque};
use std::sync::{mpsc, Arc};

type Invocation = ProjectInvocation<()>;

pub(super) struct Fixture {
    pub runtimes: HashMap<ProjectKey, ProjectRuntime<(), Invocation>>,
    pub dialog: Option<ProjectDialog<Invocation>>,
    pub replay: ProjectReplayState,
    pub state: Option<ProjectScreenState<Invocation>>,
    pub pending: VecDeque<Invocation>,
    pub jobs: mpsc::Receiver<ProjectJob<()>>,
}

pub(super) fn project() -> ProjectKey {
    ProjectKey {
        plugin: "test".into(),
        root: "/project".into(),
        marker: "marker".into(),
    }
}

pub(super) fn action() -> ProjectAction {
    ProjectAction {
        id: ProjectActionId {
            plugin: "test".into(),
            action: "run".into(),
        },
        declaration: crate::Act::new("run")
            .project_context(ActContext::screen("list", ["file"]))
            .writes(),
        key: Some(konoma_host_core::keymap::KeyPress::key(
            crossterm::event::KeyCode::Enter,
        )),
    }
}

impl Fixture {
    pub fn new() -> Self {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (_, rx) = mpsc::sync_channel(1);
        let key = ScreenKey::new("list", 7);
        let view = ProjectView::new("list").tab(7);
        let runtime = ProjectRuntime {
            top: Default::default(),
            closed_tabs: Default::default(),
            watch_paths: None,
            refresh_watch: false,
            project: MatchedProject {
                plugin: "test".into(),
                root: "/project".into(),
                marker: "marker".into(),
                declaration: crate::ProjectDecl::new(),
                session_watch_paths: None,
            },
            views: HashMap::from([(
                key.clone(),
                Arc::new(ProjectContent::List(ListView {
                    rows: vec![
                        ListRow::new("file", vec![]).value("a"),
                        ListRow::new("file", vec![]).value("b"),
                    ],
                    ..Default::default()
                })),
            )]),
            contexts: HashMap::from([(key.clone(), view)]),
            launch_targets: Default::default(),
            enabled: HashMap::from([("run".into(), true)]),
            chip: None,
            chip_dirty: false,
            busy: false,
            acting: false,
            acting_writes: false,
            pending_retry: None,
            view_request: None,
            dirty: Default::default(),
            ready: true,
            error: None,
            tx,
            rx,
            pending: None,
            invalidated: false,
        };
        Self {
            runtimes: HashMap::from([(project(), runtime)]),
            dialog: None,
            replay: Default::default(),
            state: Some(ProjectScreenState::new(project(), key)),
            pending: VecDeque::new(),
            jobs,
        }
    }

    pub fn parts(
        &mut self,
    ) -> (
        ProjectHost<'_, Invocation>,
        ProjectContextMut<'_, Invocation>,
    ) {
        (
            ProjectHost::new(&mut self.runtimes, &mut self.dialog, &mut self.replay),
            ProjectContextMut {
                id: 7,
                active: self.state.as_mut(),
                returned: None,
                pending_acts: &mut self.pending,
                request: Some(ProjectView::new("list").tab(7)),
            },
        )
    }

    pub fn facts(&self) -> ProjectActFacts<()> {
        ProjectActFacts {
            origin: ProjectOrigin {
                context: 7,
                project: project(),
                root: "/project".into(),
                screen: self
                    .state
                    .as_ref()
                    .and_then(|s| s.frame().map(|f| f.key.clone())),
                surface: (),
            },
            binding: true,
            policy: ProjectWritePolicy::default(),
            context: ProjectAct::new(),
            confirmation: false,
        }
    }

    pub fn request(&mut self) -> Invocation {
        let facts = self.facts();
        let (mut host, mut context) = self.parts();
        let update = host.request_act(&mut context, &action(), &facts, |i| i);
        match update.prompt {
            Some(ProjectPromptUpdate::InvocationReady(i)) => i,
            _ => panic!("request must be ready"),
        }
    }
}

#[test]
fn execute_rechecks_policy_binding_enabled_origin_and_row() {
    for condition in [
        "policy", "manifest", "binding", "enabled", "origin", "context", "project", "screen", "row",
    ] {
        let mut f = Fixture::new();
        let invocation = f.request();
        assert_eq!(
            invocation.context.selected,
            Some(ProjectRow::new("file", "a"))
        );
        let mut facts = f.facts();
        match condition {
            "policy" => facts.policy.configured = Some(toml::Value::Boolean(true)),
            "manifest" => facts.policy.declared_read_only = true,
            "binding" => facts.binding = false,
            "enabled" => {
                f.runtimes
                    .get_mut(&project())
                    .unwrap()
                    .enabled
                    .insert("run".into(), false);
            }
            "origin" => facts.origin.root = "/other".into(),
            "context" => facts.origin.context = 19,
            "project" => facts.origin.project.marker = "other".into(),
            "screen" => facts.origin.screen = Some(ScreenKey::new("other", 7)),
            "row" => {
                f.runtimes.get_mut(&project()).unwrap().views.insert(
                    ScreenKey::new("list", 7),
                    Arc::new(ProjectContent::List(ListView {
                        rows: vec![ListRow::new("file", vec![]).value("b")],
                        ..Default::default()
                    })),
                );
            }
            _ => unreachable!(),
        }
        let (mut host, mut context) = f.parts();
        host.execute_act(&mut context, invocation, &facts, |_| {
            Some(ViewReq::new(80, 20))
        });
        assert!(
            f.jobs.try_recv().is_err(),
            "must reject changed {condition}"
        );
    }
    let mut f = Fixture::new();
    let invocation = f.request();
    f.state.as_mut().unwrap().frame_mut().unwrap().cursor = 1;
    let facts = f.facts();
    let (mut host, mut context) = f.parts();
    host.execute_act(&mut context, invocation.clone(), &facts, |_| {
        Some(ViewReq::new(80, 20))
    });
    match f.jobs.try_recv().unwrap() {
        ProjectJob::Act { context, .. } => {
            assert_eq!(context.selected, Some(ProjectRow::new("file", "a")))
        }
        _ => panic!("act job"),
    }
    let (mut host, mut context) = f.parts();
    host.execute_act(&mut context, invocation, &facts, |_| {
        Some(ViewReq::new(80, 20))
    });
    assert!(f.jobs.try_recv().is_err(), "acting must never run twice");
    assert!(f
        .state
        .as_ref()
        .unwrap()
        .frame()
        .unwrap()
        .pending
        .is_empty());
    for offscreen in [false, true] {
        let mut f = Fixture::new();
        if offscreen {
            f.state = None;
        }
        let invocation = f.request();
        f.runtimes.get_mut(&project()).unwrap().busy = true;
        let facts = f.facts();
        let (mut host, mut context) = f.parts();
        host.execute_act(&mut context, invocation, &facts, |_| None);
        assert!(f.jobs.try_recv().is_err());
        if offscreen {
            assert_eq!(f.pending.len(), 1);
        } else {
            assert_eq!(f.state.as_ref().unwrap().frame().unwrap().pending.len(), 1);
        }
    }
    for enabled in [false, true] {
        let mut f = Fixture::new();
        f.runtimes
            .get_mut(&project())
            .unwrap()
            .enabled
            .insert("run".into(), enabled);
        let mut facts = f.facts();
        facts.policy.declared_read_only = enabled;
        let (mut host, mut context) = f.parts();
        assert!(host
            .request_act(&mut context, &action(), &facts, |i| i)
            .prompt
            .is_none());
        assert!(host.dialog.is_none());
    }
}

#[test]
fn refresh_targets_preserve_context_and_watch_rules() {
    for (targets, want_chip, want_watch) in [
        (vec![crate::RefreshTarget::Chip], true, false),
        (vec![], false, true),
        (vec![crate::RefreshTarget::CurrentScreen], false, true),
        (vec![crate::RefreshTarget::AllScreens], false, true),
        (vec![crate::RefreshTarget::screen("list")], false, true),
    ] {
        let mut f = Fixture::new();
        let other = ScreenKey::new("list", 19);
        let runtime = f.runtimes.get_mut(&project()).unwrap();
        runtime
            .views
            .insert(other.clone(), Arc::new(ProjectContent::Lines(vec![])));
        let mut invocation = f.request();
        invocation.declaration.refresh = targets.clone();
        let facts = f.facts();
        let (mut host, mut context) = f.parts();
        let mut requested = Vec::new();
        host.execute_act(&mut context, invocation, &facts, |key| {
            requested.push(key.clone());
            Some(
                ViewReq::new(if key.tab == 19 { 119 } else { 87 }, 20).project(
                    ProjectView::new(&key.screen)
                        .tab(key.tab)
                        .paths(format!("/context/{}", key.tab), "/open"),
                ),
            )
        });
        match f.jobs.try_recv().unwrap() {
            ProjectJob::Act {
                chip,
                watch,
                refresh,
                ..
            } => {
                assert_eq!(chip, want_chip);
                assert_eq!(watch, want_watch);
                if targets.iter().any(|t| {
                    matches!(
                        t,
                        crate::RefreshTarget::AllScreens | crate::RefreshTarget::Screen { .. }
                    )
                }) {
                    assert_eq!(requested, vec![ScreenKey::new("list", 7), other]);
                    assert_eq!(refresh[1].1.cols, 119);
                    assert_eq!(
                        refresh[1].1.project.as_ref().unwrap().tree_root,
                        Some(std::path::PathBuf::from("/context/19"))
                    );
                }
            }
            _ => panic!("act"),
        }
    }
    let mut f = Fixture::new();
    let facts = f.facts();
    let a = action();
    let (mut host, mut context) = f.parts();
    assert_eq!(
        host.action_project(&context, &a.id, &[project()]),
        Some(project())
    );
    let other = ProjectKey {
        plugin: "other".into(),
        ..project()
    };
    context.active = None;
    assert_eq!(
        host.action_project(&context, &a.id, &[other, project()]),
        Some(project())
    );
    context.id = 19;
    let mut facts = facts;
    facts.origin.context = 19;
    facts.origin.screen = None;
    facts.context = ProjectAct::new()
        .tab(19)
        .selected(ProjectRow::new("file", "/context/19/file"));
    let update = host.request_act(&mut context, &a, &facts, |i| i);
    match update.prompt {
        Some(ProjectPromptUpdate::InvocationReady(i)) => {
            assert_eq!(i.origin.context, 19);
            assert_eq!(i.context.tab, 19);
            assert_eq!(
                i.context.selected,
                Some(ProjectRow::new("file", "/context/19/file"))
            );
        }
        _ => panic!("other context request"),
    }
}

#[test]
fn screen_action_wins_shared_jump_and_right_uses_enter() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use konoma_host_core::keymap::{KeyPress, Motion};
    let mut f = Fixture::new();
    f.runtimes.get_mut(&project()).unwrap().project.declaration = crate::ProjectDecl::new()
        .screen(crate::ProjectScreen::new("list", "List").key('x'))
        .screen(crate::ProjectScreen::new("other", "Other").key('o'));
    let mut own = action();
    own.key = Some(KeyPress::ch('x'));
    let mut tree = own.clone();
    tree.id.action = "tree".into();
    tree.declaration.project_context = Some(ActContext::TreeSelection);
    let scope = ProjectActionScope::Screen {
        plugin: "test".into(),
        context: ProjectAct::new()
            .screen("list")
            .selected(ProjectRow::new("file", "a")),
        diff: false,
        jumps: vec![KeyPress::ch('x'), KeyPress::ch('o')],
    };
    let actions = vec![own.clone(), tree.clone()];
    let (mut host, mut context) = f.parts();
    assert_eq!(host.acts_in(&scope, &actions).len(), 1);
    assert_eq!(
        action_key(&scope, &own, KeyPress::ch('x')),
        Some(KeyPress::ch('x'))
    );
    assert_eq!(action_key(&scope, &tree, KeyPress::ch('x')), None);
    assert_eq!(
        tree.key,
        Some(KeyPress::ch('x')),
        "tree declaration is immutable"
    );
    let mut facts = super::input::ProjectKeyFacts {
        action: Some(own.id.clone()),
        actions: vec![own.clone()],
        ..Default::default()
    };
    assert_eq!(
        host.handle_key(
            &mut context,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &facts
        )
        .dispatch,
        ProjectDispatch::Unhandled
    );
    assert_eq!(
        context.active.as_ref().unwrap().frame().unwrap().key.screen,
        "list"
    );
    facts.action = None;
    facts.enter_action = Some(own.id.clone());
    facts.motion = Some(Motion::Right);
    facts.text_right = true;
    let update = host.handle_key(
        &mut context,
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        &facts,
    );
    assert!(matches!(update.commands.as_slice(), [ProjectCommand::Dispatch(id)] if id == &own.id));
    facts.enter_action = None;
    assert!(host
        .handle_key(
            &mut context,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &facts
        )
        .commands
        .is_empty());
    facts.enter_action = Some(own.id.clone());
    host.runtimes
        .get_mut(&project())
        .unwrap()
        .enabled
        .insert("run".into(), false);
    assert!(host
        .handle_key(
            &mut context,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &facts
        )
        .commands
        .is_empty());
    for (physical, list_override, text_right, expected) in [
        (KeyCode::Right, Some(false), false, true),
        (KeyCode::Char('l'), Some(true), false, true),
        (KeyCode::Char('l'), Some(false), true, false),
        (KeyCode::Char('l'), None, true, true),
        (KeyCode::Char('l'), None, false, false),
    ] {
        assert_eq!(
            super::input::list_right(true, physical, list_override, text_right),
            expected
        );
    }
}

fn text_invocation(f: &mut Fixture, keep: bool) -> Invocation {
    let mut invocation = f.request();
    let input = crate::TextInput::new("Message", "seed");
    invocation.declaration = invocation.declaration.clone().text_input(if keep {
        input.keep_on_failure()
    } else {
        input
    });
    invocation.context.text = Some("typed message".into());
    invocation
}

fn notice_of(update: &ProjectUpdate<Invocation>) -> Option<ProjectNotice> {
    update.commands.iter().find_map(|command| match command {
        ProjectCommand::Notice(notice) => Some(*notice),
        _ => None,
    })
}

fn retried_text(update: &ProjectUpdate<Invocation>) -> Vec<Option<String>> {
    update
        .events
        .iter()
        .filter_map(|event| match event {
            ProjectEvent::Retry(invocation) => Some(invocation.context.text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn act_refused_before_it_runs_returns_the_typed_text_as_a_retry() {
    // Every refusal that happens before the job is sent, each with a kept and an unkept input.
    for refusal in [
        "read_only",
        "busy_acting",
        "busy_not_acting_queue_full",
        "not_ready",
        "send_failed",
    ] {
        for keep in [true, false] {
            let mut f = Fixture::new();
            let invocation = text_invocation(&mut f, keep);
            let mut facts = f.facts();
            match refusal {
                "read_only" => facts.policy.declared_read_only = true,
                "busy_acting" => {
                    let runtime = f.runtimes.get_mut(&project()).unwrap();
                    runtime.busy = true;
                    runtime.acting = true;
                }
                "busy_not_acting_queue_full" => {
                    f.runtimes.get_mut(&project()).unwrap().busy = true;
                    let frame = f.state.as_mut().unwrap().frame_mut().unwrap();
                    for _ in 0..4096 {
                        frame
                            .pending
                            .push_back(ProjectInput::Act(Box::new(invocation.clone())));
                    }
                }
                "not_ready" => f.runtimes.get_mut(&project()).unwrap().ready = false,
                "send_failed" => {
                    // The channel holds one job and nothing drains it: a first act fills it.
                    let (mut host, mut context) = f.parts();
                    host.execute_act(&mut context, invocation.clone(), &facts, |_| None);
                    let runtime = f.runtimes.get_mut(&project()).unwrap();
                    runtime.busy = false;
                    runtime.acting = false;
                    runtime.pending_retry = None;
                }
                _ => unreachable!(),
            }
            let (mut host, mut context) = f.parts();
            let update = host.execute_act(&mut context, invocation, &facts, |_| None);
            let notice = if refusal == "read_only" {
                ProjectNotice::ReadOnly
            } else {
                ProjectNotice::Busy
            };
            assert_eq!(notice_of(&update), Some(notice), "{refusal} keep={keep}");
            let want = if keep {
                vec![Some("typed message".to_string())]
            } else {
                vec![]
            };
            assert_eq!(retried_text(&update), want, "{refusal} keep={keep}");
        }
    }
    // A refusal that is not about running now (queued for later) keeps the invocation queued.
    let mut f = Fixture::new();
    let invocation = text_invocation(&mut f, true);
    f.runtimes.get_mut(&project()).unwrap().busy = true;
    let facts = f.facts();
    let (mut host, mut context) = f.parts();
    let update = host.execute_act(&mut context, invocation, &facts, |_| None);
    assert!(notice_of(&update).is_none() && update.events.is_empty());
    // Origin, selection and disabled refusals never reopen the input.
    for case in ["origin", "disabled", "selection"] {
        let mut f = Fixture::new();
        let invocation = text_invocation(&mut f, true);
        let mut facts = f.facts();
        match case {
            "origin" => facts.origin.root = "/other".into(),
            "disabled" => {
                f.runtimes
                    .get_mut(&project())
                    .unwrap()
                    .enabled
                    .insert("run".into(), false);
            }
            _ => {
                f.runtimes.get_mut(&project()).unwrap().views.insert(
                    ScreenKey::new("list", 7),
                    Arc::new(ProjectContent::List(ListView {
                        rows: vec![ListRow::new("file", vec![]).value("b")],
                        ..Default::default()
                    })),
                );
            }
        }
        let (mut host, mut context) = f.parts();
        let update = host.execute_act(&mut context, invocation, &facts, |_| None);
        assert!(update.events.is_empty(), "{case}");
    }
}
