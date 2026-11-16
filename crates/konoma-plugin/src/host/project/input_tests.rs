use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, VecDeque};

type Invocation = ProjectInvocation<()>;

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn jump(screen: &str, key: Option<char>) -> crate::host::bind::JumpBinding {
    crate::host::bind::JumpBinding {
        op: crate::host::bind::jump_op_id(screen),
        screen: screen.into(),
        key: key.map(konoma_host_core::keymap::KeyPress::ch),
    }
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
                jumps: vec![jump("other", Some('o'))],
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
        // `o` jumps to `other` (declared below): the leader must still keep it from jumping.
        jumps: vec![jump("other", Some('o'))],
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

/// Presses `c` on the list screen of a project that declares `other` (default key `o`) with the
/// given jump bindings; returns the dispatch and the screen shown afterwards.
fn press_with_jumps(
    c: char,
    jumps: Vec<crate::host::bind::JumpBinding>,
) -> (super::actions::ProjectDispatch, String) {
    let mut f = super::actions_tests::Fixture::new();
    f.runtimes.get_mut(&project()).unwrap().project.declaration =
        crate::ProjectDecl::new().screen(crate::ProjectScreen::new("other", "Other").key('o'));
    let facts = ProjectKeyFacts {
        jumps,
        ..Default::default()
    };
    let (mut host, mut context) = f.parts();
    let dispatch = host.handle_key(&mut context, key(c), &facts).dispatch;
    let screen = context
        .active
        .as_ref()
        .unwrap()
        .frame()
        .unwrap()
        .key
        .screen
        .clone();
    (dispatch, screen)
}

#[test]
fn a_rebound_jump_key_navigates_and_the_declared_key_does_not() {
    use super::actions::ProjectDispatch;
    let rebound = || vec![jump("other", Some('x'))];
    assert_eq!(
        press_with_jumps('x', rebound()),
        (ProjectDispatch::Handled, "other".into()),
        "the key the user gave jumps"
    );
    assert_eq!(
        press_with_jumps('o', rebound()),
        (ProjectDispatch::Unhandled, "list".into()),
        "the declared default is inert once rebound"
    );
}

#[test]
fn the_default_jump_key_navigates_and_a_removed_one_does_not() {
    use super::actions::ProjectDispatch;
    assert_eq!(
        press_with_jumps('o', vec![jump("other", Some('o'))]),
        (ProjectDispatch::Handled, "other".into())
    );
    assert_eq!(
        press_with_jumps('o', vec![jump("other", None)]),
        (ProjectDispatch::Unhandled, "list".into()),
        "a removed jump has no key at all"
    );
    assert_eq!(
        press_with_jumps('o', Vec::new()),
        (ProjectDispatch::Unhandled, "list".into()),
        "no jump bindings, no jump: the declared key alone is not read"
    );
}

#[test]
fn a_screen_action_with_the_rebound_key_wins_over_the_jump() {
    use super::actions::ProjectDispatch;
    let mut f = super::actions_tests::Fixture::new();
    f.runtimes.get_mut(&project()).unwrap().project.declaration =
        crate::ProjectDecl::new().screen(crate::ProjectScreen::new("other", "Other").key('o'));
    let facts = ProjectKeyFacts {
        jumps: vec![jump("other", Some('x'))],
        // `facts.action`: an action of the screen owns `x`.
        action: Some(super::actions_tests::action().id),
        ..Default::default()
    };
    let (mut host, mut context) = f.parts();
    assert_eq!(
        host.handle_key(&mut context, key('x'), &facts).dispatch,
        ProjectDispatch::Unhandled,
        "left to the action"
    );
    assert_eq!(
        context.active.as_ref().unwrap().frame().unwrap().key.screen,
        "list"
    );
}

fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
}

/// A key the project layer ignores (an Alt chord has no form in it) does nothing at all: with a
/// `c` waiting behind a read, it neither joins the queue nor cancels it. Plain `q` still leaves
/// the project, as before.
#[test]
fn an_ignored_key_neither_queues_nor_cancels_what_waits() {
    use super::actions::ProjectDispatch;
    for c in ['h', 'j', 'l', 'q', 'Q'] {
        let mut f = super::actions_tests::Fixture::new();
        f.runtimes.get_mut(&project()).unwrap().busy = true;
        let (mut host, mut context) = f.parts();
        assert!(host.queue_input(&mut context, ProjectInput::Key(key('c'))));
        let update = host.handle_key(&mut context, alt(c), &ProjectKeyFacts::of_no_key());
        assert_eq!(update.dispatch, ProjectDispatch::Unhandled, "Alt-{c}");
        let frame = context.active.as_ref().unwrap().frame().unwrap();
        assert_eq!(frame.pending.len(), 1, "Alt-{c} changed the queue");
        assert!(
            matches!(frame.pending.front(), Some(ProjectInput::Key(k)) if k == &key('c')),
            "Alt-{c} displaced the waiting key"
        );
    }
    let mut f = super::actions_tests::Fixture::new();
    f.runtimes.get_mut(&project()).unwrap().busy = true;
    let (mut host, mut context) = f.parts();
    assert!(host.queue_input(&mut context, ProjectInput::Key(key('c'))));
    let update = host.handle_key(&mut context, key('q'), &ProjectKeyFacts::default());
    assert_eq!(update.dispatch, ProjectDispatch::LeaveProject);
}

/// The direct refresh goes through the same gate as the key `r`: behind a read, behind input that
/// is still pending after the read, and into the busy notice when the queue is full.
#[test]
fn the_direct_refresh_goes_through_the_gate_of_the_key_r() {
    use super::actions::ProjectCommand;
    // (busy, pending before, expected queue length after, expected busy notice)
    for (busy, before, after, notice) in [
        (true, 0, 1, false),
        (false, 1, 2, false),
        (false, 4096, 4096, true),
        (true, 4096, 4096, true),
        (false, 0, 0, false),
    ] {
        let mut outcomes = Vec::new();
        for direct in [false, true] {
            let mut f = super::actions_tests::Fixture::new();
            f.runtimes.get_mut(&project()).unwrap().busy = busy;
            let (mut host, mut context) = f.parts();
            for _ in 0..before {
                assert!(host.queue_input(&mut context, ProjectInput::Key(key('x'))));
            }
            let update = if direct {
                host.refresh(&mut context)
            } else {
                host.handle_key(&mut context, key('r'), &ProjectKeyFacts::default())
            };
            let frame = context.active.as_ref().unwrap().frame().unwrap();
            let busy_notice = update.commands.iter().any(|c| {
                matches!(
                    c,
                    ProjectCommand::Notice(super::actions::ProjectNotice::Busy)
                )
            });
            let ran = update
                .commands
                .iter()
                .any(|c| matches!(c, ProjectCommand::EnsureView));
            assert_eq!(
                frame.pending.len(),
                after,
                "direct={direct} busy={busy} before={before}: queue length"
            );
            assert_eq!(
                busy_notice, notice,
                "direct={direct} busy={busy} before={before}"
            );
            assert_eq!(
                ran,
                after == 0 && !notice,
                "direct={direct} busy={busy} before={before}: ran"
            );
            outcomes.push((frame.pending.len(), busy_notice, ran));
        }
        assert_eq!(
            outcomes[0], outcomes[1],
            "busy={busy} before={before}: r and refresh differ"
        );
    }
}
