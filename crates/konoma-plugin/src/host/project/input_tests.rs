use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, VecDeque};

type Invocation = ProjectInvocation<()>;

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn project() -> ProjectKey {
    ProjectKey {
        plugin: "test".into(),
        root: "/project".into(),
        marker: "marker".into(),
    }
}

fn invocation() -> Invocation {
    ProjectInvocation {
        action: ProjectActionId {
            plugin: "test".into(),
            action: "run".into(),
        },
        declaration: crate::Act::new("run"),
        context: crate::ProjectAct::new(),
        origin: ProjectOrigin {
            context: 7,
            project: project(),
            root: "/project".into(),
            screen: None,
            surface: (),
        },
    }
}

#[test]
fn queue_limits_are_4096_and_replay_budget_is_64() {
    let mut runtimes = HashMap::new();
    let mut dialog = None;
    let mut replay = ProjectReplayState::default();
    let mut host = ProjectHost::new(&mut runtimes, &mut dialog, &mut replay);
    let mut state = ProjectScreenState::new(project(), ScreenKey::new("list", 7));
    let mut pending = VecDeque::new();
    let mut context = ProjectContextMut {
        id: 7,
        active: Some(&mut state),
        returned: None,
        pending_acts: &mut pending,
        request: None,
    };
    for _ in 0..4096 {
        assert!(host.queue_input(&mut context, ProjectInput::Key(key('j'))));
    }
    assert!(!host.queue_input(&mut context, ProjectInput::Key(key('x'))));
    assert_eq!(
        context
            .active
            .as_ref()
            .unwrap()
            .frame()
            .unwrap()
            .pending
            .len(),
        4096
    );
    assert!(host.begin_replay());
    for _ in 0..64 {
        assert!(matches!(
            host.next_replay(&mut context),
            Some(ProjectInput::Key(_))
        ));
    }
    assert!(host.next_replay(&mut context).is_none());
    assert_eq!(
        context
            .active
            .as_ref()
            .unwrap()
            .frame()
            .unwrap()
            .pending
            .len(),
        4032
    );
    host.end_replay();
    context.active = None;
    for _ in 0..4096 {
        assert!(host.queue_input(&mut context, ProjectInput::Act(Box::new(invocation()))));
    }
    assert!(!host.queue_input(&mut context, ProjectInput::Act(Box::new(invocation()))));
    assert_eq!(context.pending_acts.len(), 4096);
    assert!(host.begin_replay());
    for _ in 0..64 {
        assert!(matches!(
            host.next_replay(&mut context),
            Some(ProjectInput::Act(_))
        ));
    }
    assert!(host.next_replay(&mut context).is_none());
    assert_eq!(context.pending_acts.len(), 4032);
    host.end_replay();
    let mut f = super::actions_tests::Fixture::new();
    f.runtimes.get_mut(&project()).unwrap().project.declaration = crate::ProjectDecl::new()
        .screen(crate::ProjectScreen::new("retained", "Retained").pushed())
        .screen(
            crate::ProjectScreen::new("modal", "Modal")
                .pushed()
                .transient(),
        );
    let state = f.state.as_mut().unwrap();
    for screen in ["retained", "modal", "modal"] {
        state
            .stack
            .push(ProjectFrame::new(ScreenKey::new(screen, 7)));
    }
    for frame in &mut state.stack {
        frame.pending.push_back(ProjectInput::Key(key('x')));
    }
    f.pending.push_back(invocation());
    let (mut host, mut context) = f.parts();
    host.cancel_input(&mut context);
    let state = context.active.as_ref().unwrap();
    assert_eq!(
        state
            .stack
            .iter()
            .map(|f| f.key.screen.as_str())
            .collect::<Vec<_>>(),
        ["list", "retained"]
    );
    assert!(state.stack.iter().all(|f| f.pending.is_empty()));
    assert!(context.pending_acts.is_empty());
    context
        .active
        .as_mut()
        .unwrap()
        .frame_mut()
        .unwrap()
        .pending
        .push_back(ProjectInput::Key(key('x')));
    let facts = ProjectKeyFacts {
        global: true,
        ..Default::default()
    };
    host.handle_key(&mut context, key('?'), &facts);
    assert!(context
        .active
        .as_ref()
        .unwrap()
        .frame()
        .unwrap()
        .pending
        .is_empty());
}

#[test]
fn requeued_replay_item_returns_to_front() {
    let mut f = super::actions_tests::Fixture::new();
    f.state
        .as_mut()
        .unwrap()
        .frame_mut()
        .unwrap()
        .pending
        .extend([ProjectInput::Key(key('a')), ProjectInput::Key(key('b'))]);
    let (mut host, mut context) = f.parts();
    assert!(host.begin_replay());
    let item = host.next_replay(&mut context).unwrap();
    assert!(host.queue_input(&mut context, item));
    assert!(matches!(host.next_replay(&mut context), Some(ProjectInput::Key(k)) if k == key('a')));
    assert!(matches!(host.next_replay(&mut context), Some(ProjectInput::Key(k)) if k == key('b')));
    host.end_replay();
}

#[test]
fn pending_prompt_or_space_prevents_navigation_bypass() {
    use super::actions::ProjectDispatch;
    use super::input::ProjectKeyFacts;
    use konoma_host_core::keymap::KeyPress;
    for first in [' ', 'c'] {
        for later in ['q', 'o', 'j', 'r', '?'] {
            let mut f = super::actions_tests::Fixture::new();
            f.runtimes.get_mut(&project()).unwrap().busy = true;
            f.runtimes.get_mut(&project()).unwrap().project.declaration = crate::ProjectDecl::new()
                .screen(crate::ProjectScreen::new("other", "Other").key('o'));
            let mut prompt = super::actions_tests::action();
            prompt.key = Some(KeyPress::ch('c'));
            prompt.declaration.text_input = Some(crate::TextInput::new("Text", ""));
            let facts = ProjectKeyFacts {
                actions: vec![prompt],
                global: later == '?',
                ..Default::default()
            };
            let (mut host, mut context) = f.parts();
            assert!(host.queue_input(&mut context, ProjectInput::Key(key(first))));
            assert_eq!(
                host.handle_key(&mut context, key(later), &facts).dispatch,
                ProjectDispatch::Handled
            );
            let frame = context.active.as_ref().unwrap().frame().unwrap();
            assert_eq!(frame.key.screen, "list");
            assert_eq!(
                frame.pending.len(),
                2,
                "{first} then {later} must remain ordered"
            );
            assert!(matches!(frame.pending.back(), Some(ProjectInput::Key(k)) if k == &key(later)));
        }
    }
}

#[test]
fn replay_observes_each_intermediate_prompt_and_leader() {
    use super::actions::ProjectDispatch;
    use super::input::ProjectKeyFacts;
    use konoma_host_core::keymap::KeyPress;
    let mut f = super::actions_tests::Fixture::new();
    let mut action = super::actions_tests::action();
    action.key = Some(KeyPress::ch('c'));
    action.declaration.text_input = Some(crate::TextInput::new("Text", ""));
    let mut facts = ProjectKeyFacts {
        action: Some(action.id.clone()),
        actions: vec![action.clone()],
        ..Default::default()
    };
    let mut act_facts = f.facts();
    act_facts.confirmation = true;
    f.state
        .as_mut()
        .unwrap()
        .frame_mut()
        .unwrap()
        .pending
        .extend(['c', 'q', 'o', '\n'].map(|c| {
            ProjectInput::Key(if c == '\n' {
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            } else {
                key(c)
            })
        }));
    let (mut host, mut context) = f.parts();
    assert!(host.pending_interaction(&context, &facts.actions));
    assert!(host.begin_replay());
    assert!(!host.begin_replay());
    assert!(matches!(host.next_replay(&mut context), Some(ProjectInput::Key(k)) if k == key('c')));
    assert_eq!(
        host.handle_key(&mut context, key('c'), &facts).dispatch,
        ProjectDispatch::Unhandled
    );
    host.request_act(&mut context, &action, &act_facts, |i| i);
    for c in ['q', 'o'] {
        let Some(ProjectInput::Key(k)) = host.next_replay(&mut context) else {
            panic!("raw key")
        };
        assert_eq!(k, key(c));
        host.handle_key(&mut context, k, &facts);
    }
    let Some(ProjectInput::Key(k)) = host.next_replay(&mut context) else {
        panic!("submit")
    };
    match host.handle_key(&mut context, k, &facts).prompt {
        Some(ProjectPromptUpdate::InvocationReady(i)) => {
            assert_eq!(i.context.text.as_deref(), Some("qo"))
        }
        _ => panic!("replay must use the intermediate prompt"),
    }
    host.runtimes
        .get_mut(&project())
        .unwrap()
        .project
        .declaration =
        crate::ProjectDecl::new().screen(crate::ProjectScreen::new("other", "Other").key('o'));
    host.queue_input(&mut context, ProjectInput::Key(key('o')));
    facts.leader = true;
    facts.action = None;
    let Some(ProjectInput::Key(k)) = host.next_replay(&mut context) else {
        panic!("leader continuation")
    };
    assert_eq!(
        host.handle_key(&mut context, k, &facts).dispatch,
        ProjectDispatch::Unhandled
    );
    assert_eq!(
        context.active.as_ref().unwrap().frame().unwrap().key.screen,
        "list"
    );
    host.end_replay();
}
