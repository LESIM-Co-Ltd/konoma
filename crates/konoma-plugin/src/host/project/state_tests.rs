use super::ProjectInput;
use super::ProjectMotionResult;
use super::{ProjectContent, ProjectKey, ProjectScreenState, ProjectViewport, ScreenKey};
use crate::project::{ProjectDecl, ProjectRow, ProjectScreen};
use crate::view::{ListRow, ListView};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_host_core::keymap::Motion;
use konoma_host_core::project::DiffLayout;

fn state() -> ProjectScreenState<&'static str> {
    ProjectScreenState::new(
        ProjectKey {
            plugin: "git".into(),
            root: "/project".into(),
            marker: ".git".into(),
        },
        ScreenKey::new("changes", 7),
    )
}

fn row(value: &str) -> ListRow {
    ListRow::new("file", vec![]).value(value).filter_text(value)
}

fn list(rows: Vec<ListRow>) -> ProjectContent {
    ProjectContent::List(ListView {
        rows,
        ..Default::default()
    })
}

#[test]
fn rows_remap_by_project_row_identity() {
    let mut state = state();
    let key = state.stack[0].key.clone();
    let old = list(vec![row("a"), row("b").name("Bee"), row("c")]);
    state.stack[0].cursor = 1;
    state.stack[0].marked.extend([0, 1]);
    let new = list(vec![row("c"), row("a"), row("b").name("Bee")]);
    state.remap_rows(&key, Some(&old), &new);
    assert_eq!(state.stack[0].cursor, 2);
    assert_eq!(
        state.stack[0].marked.iter().copied().collect::<Vec<_>>(),
        vec![1, 2]
    );
    let renamed = list(vec![row("a"), row("b").name("Other")]);
    state.remap_rows(&key, Some(&new), &renamed);
    assert_eq!(state.stack[0].cursor, 0);
    assert_eq!(
        state.stack[0].marked.iter().copied().collect::<Vec<_>>(),
        vec![0]
    );
    let changed_kind = list(vec![
        row("b").name("Other"),
        ListRow::new("directory", vec![]).value("a"),
    ]);
    state.remap_rows(&key, Some(&renamed), &changed_kind);
    assert_eq!(state.stack[0].cursor, 0);
    assert!(state.stack[0].marked.is_empty());
}

#[test]
fn top_on_next_reload_starts_the_list_over_once() {
    let mut state = state();
    let key = state.stack[0].key.clone();
    let old = list(vec![row("a"), row("b")]);
    state.stack[0].cursor = 1;
    state.top_on_next_reload();
    // A row sorted above the selection would move the cursor down; the flag wins, once.
    let new = list(vec![row("0"), row("a"), row("b")]);
    state.remap_rows(&key, Some(&old), &new);
    assert_eq!(state.stack[0].cursor, 0);
    state.stack[0].cursor = 2;
    state.remap_rows(&key, Some(&new), &new);
    assert_eq!(
        state.stack[0].cursor, 2,
        "the flag is taken by the first replacement"
    );
}

#[test]
fn removed_selection_discards_keys_but_keeps_invocations() {
    let mut state = state();
    let key = state.stack[0].key.clone();
    let old = list(vec![row("a"), row("b")]);
    state.stack[0].cursor = 1;
    state.stack[0].pending.extend([
        ProjectInput::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ProjectInput::Act(Box::new("first snapshot")),
        ProjectInput::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        ProjectInput::Act(Box::new("second snapshot")),
    ]);
    state.remap_rows(&key, Some(&old), &list(vec![row("a")]));
    assert_eq!(state.stack[0].cursor, 0);
    let pending = &state.stack[0].pending;
    assert_eq!(pending.len(), 2);
    assert!(matches!(&pending[0], ProjectInput::Act(value) if **value == "first snapshot"));
    assert!(matches!(&pending[1], ProjectInput::Act(value) if **value == "second snapshot"));
}

#[test]
fn filter_anchor_and_fallback_are_not_action_selection() {
    let mut state = state();
    let viewport = ProjectViewport { cols: 80, rows: 3 };
    let layout = DiffLayout::Unified;
    let content = list(vec![
        row("alpha"),
        row("beta").target("/project/b"),
        row("bravo"),
        row("message").selectable(false),
        row("fallback one").filter_fallback(true).selectable(true),
        row("fallback two").filter_fallback(true),
    ]);
    state.stack[0].cursor = 1;
    state.start_filter(Some(&content));
    state.handle_filter_key(
        Some(&content),
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
    );
    assert_eq!(state.visible_rows(&content, viewport, layout), vec![1, 2]);
    assert_eq!(state.stack[0].cursor, 0);
    assert_eq!(state.selected_index(&content, viewport, layout), Some(1));
    assert_eq!(state.row_target(&content, 1), Some("/project/b".into()));
    state.stack[0].marked.extend([0, 2]);
    let act = state.act_context(Some(&content));
    assert_eq!(act.tab, 7);
    assert_eq!(act.screen.as_deref(), Some("changes"));
    assert_eq!(act.selected, Some(ProjectRow::from(&row("beta"))));
    assert_eq!(
        act.marked,
        vec![
            ProjectRow::from(&row("alpha")),
            ProjectRow::from(&row("bravo"))
        ]
    );
    state.handle_filter_key(
        Some(&content),
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
    );
    assert_eq!(state.visible_rows(&content, viewport, layout), vec![4, 5]);
    assert_eq!(state.selected_index(&content, viewport, layout), None);
    assert_eq!(state.act_context(Some(&content)).selected, None);
    state.handle_filter_key(
        Some(&content),
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
    );
    assert_eq!(state.selected_index(&content, viewport, layout), Some(2));
    state.handle_filter_key(
        Some(&content),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert_eq!(state.stack[0].cursor, 2);
    assert!(!state.stack[0].filtering);
    state.stack[0].cursor = 3;
    assert_eq!(state.selected_index(&content, viewport, layout), None);
    state.start_filter(Some(&content));
    state.handle_filter_key(
        Some(&content),
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
    );
    assert_eq!(
        state.selected_index(&list(vec![row("a")]), viewport, layout),
        None
    );
    assert_eq!(state.stack[0].cursor, 0);
}

#[test]
fn initial_selection_and_marks_are_applied_once() {
    let mut state = state();
    let declaration =
        ProjectDecl::new().screen(ProjectScreen::new("changes", "Changes").marks(true));
    let content = list(vec![
        row("heading")
            .selectable(false)
            .marked(true)
            .initial_selection(true),
        row("a").marked(true),
        row("b").initial_selection(true),
        row("fallback")
            .filter_fallback(true)
            .initial_selection(true),
    ]);
    state.initialize_frame(&content, &declaration);
    assert_eq!(state.stack[0].cursor, 2);
    assert_eq!(
        state.stack[0].marked.iter().copied().collect::<Vec<_>>(),
        vec![1]
    );
    state.stack[0].cursor = 1;
    state.toggle_mark(&content, &declaration);
    assert!(state.stack[0].marked.is_empty());
    state.initialize_frame(&content, &declaration);
    assert_eq!(state.stack[0].cursor, 1);
    assert!(state.stack[0].marked.is_empty());
    let disabled = ProjectDecl::new().screen(ProjectScreen::new("changes", "Changes"));
    let mut unmarked =
        super::ProjectScreenState::<()>::new(state.project.clone(), ScreenKey::new("changes", 8));
    unmarked.initialize_frame(&content, &disabled);
    assert!(!unmarked.marks_enabled(Some(&disabled)));
    assert!(unmarked.stack[0].marked.is_empty());
    unmarked.toggle_mark(&content, &disabled);
    assert!(unmarked.stack[0].marked.is_empty());
    state.stack[0].query = "b".into();
    state.stack[0].cursor = 0;
    state.toggle_mark(&content, &declaration);
    assert_eq!(
        state.stack[0].marked.iter().copied().collect::<Vec<_>>(),
        vec![2]
    );
    state.stack[0].query = "missing".into();
    state.toggle_mark(&content, &declaration);
    assert_eq!(
        state.stack[0].marked.iter().copied().collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn motion_steps_and_caps_match_each_content_kind() {
    use ProjectMotionResult::{Back, Handled, LayoutChanged, Unhandled};
    let mut state = state();
    let viewport = ProjectViewport { cols: 10, rows: 5 };
    let layout = DiffLayout::Unified;
    let content = list((0..50).map(|i| row(&i.to_string())).collect());
    for (motion, key, expected) in [
        (Motion::Up, KeyCode::Up, 0),
        (Motion::PageDown, KeyCode::PageDown, 10),
        (Motion::PageDown, KeyCode::Char('f'), 30),
        (Motion::HalfDown, KeyCode::Char('d'), 40),
        (Motion::PageDown, KeyCode::PageDown, 49),
        (Motion::PageUp, KeyCode::PageUp, 39),
        (Motion::PageUp, KeyCode::Char('b'), 19),
        (Motion::HalfUp, KeyCode::Char('u'), 9),
        (Motion::Top, KeyCode::Home, 0),
        (Motion::Bottom, KeyCode::End, 49),
    ] {
        assert_eq!(
            state.apply_motion(&content, motion, key, viewport, layout, None),
            Handled
        );
        assert_eq!(state.stack[0].cursor, expected);
    }
    assert_eq!(
        state.apply_motion(
            &content,
            Motion::Right,
            KeyCode::Right,
            viewport,
            layout,
            None
        ),
        Unhandled
    );
    assert_eq!(
        state.apply_motion(
            &content,
            Motion::Left,
            KeyCode::Left,
            viewport,
            layout,
            None
        ),
        Unhandled
    );
    let pushed = ProjectDecl::new().screen(ProjectScreen::new("changes", "Changes").pushed());
    assert_eq!(
        state.apply_motion(
            &content,
            Motion::Left,
            KeyCode::Left,
            viewport,
            layout,
            Some(&pushed)
        ),
        Back
    );
    state.stack[0].key.screen = "graph".into();
    assert_eq!(
        state.apply_motion(
            &content,
            Motion::Left,
            KeyCode::Char('h'),
            viewport,
            layout,
            None
        ),
        Back
    );
    state.stack[0].key.screen = "graph_picker".into();
    for motion in [
        Motion::HalfUp,
        Motion::HalfDown,
        Motion::PageUp,
        Motion::PageDown,
    ] {
        assert_eq!(
            state.apply_motion(&content, motion, KeyCode::PageDown, viewport, layout, None),
            Handled
        );
        assert_eq!(state.stack[0].cursor, 49);
    }
    state.stack[0].key.screen = "changes".into();
    let sparse = list(vec![
        row("heading").selectable(false),
        row("a"),
        row("gap").selectable(false),
        row("b"),
    ]);
    state.stack[0].cursor = 1;
    state.apply_motion(&sparse, Motion::Down, KeyCode::Down, viewport, layout, None);
    assert_eq!(state.stack[0].cursor, 3);
    state.apply_motion(
        &list(vec![]),
        Motion::Bottom,
        KeyCode::End,
        viewport,
        layout,
        None,
    );
    assert_eq!(state.stack[0].cursor, 0);

    let lines = ProjectContent::Lines(vec!["x".repeat(40).into(); 50]);
    let diff = ProjectContent::Diff(super::ProjectDiff::new(
        (0..50)
            .map(|i| konoma_vcs_core::git::DiffLine {
                kind: konoma_vcs_core::git::DiffLineKind::Context,
                old_no: Some(i + 1),
                new_no: Some(i + 1),
                text: "x".repeat(40),
            })
            .collect(),
        vec![],
        String::new(),
        "empty".into(),
    ));
    for content in [&lines, &diff] {
        state.stack[0].scroll = 0;
        state.stack[0].hscroll = 0;
        for (motion, expected) in [
            (Motion::Up, 0),
            (Motion::Down, 1),
            (Motion::HalfDown, 11),
            (Motion::PageDown, 31),
            (Motion::Bottom, 45),
            (Motion::Down, 45),
            (Motion::PageUp, 25),
            (Motion::HalfUp, 15),
            (Motion::Top, 0),
        ] {
            assert_eq!(
                state.apply_motion(content, motion, KeyCode::PageDown, viewport, layout, None),
                Handled
            );
            assert_eq!(state.stack[0].scroll, expected);
        }
        for (motion, expected) in [
            (Motion::Left, 0),
            (Motion::Right, 4),
            (Motion::Right, 8),
            (Motion::Left, 4),
            (Motion::LineHome, 0),
        ] {
            state.apply_motion(content, motion, KeyCode::Right, viewport, layout, None);
            assert_eq!(state.stack[0].hscroll, expected);
        }
    }
    state.apply_motion(
        &lines,
        Motion::LineEnd,
        KeyCode::End,
        viewport,
        layout,
        None,
    );
    assert_eq!(state.stack[0].hscroll, 30);
    state.apply_motion(
        &lines,
        Motion::Right,
        KeyCode::Right,
        viewport,
        layout,
        None,
    );
    assert_eq!(state.stack[0].hscroll, 30);
    assert_eq!(state.cycle_layout(&lines, layout), Unhandled);
    assert_eq!(
        state.cycle_layout(&diff, layout),
        LayoutChanged(DiffLayout::Split)
    );
    assert_eq!(state.stack[0].hscroll, 0);
    assert_eq!(
        state.visible_rows(&diff, viewport, DiffLayout::Auto).len(),
        50
    );
    let empty = ProjectContent::Lines(vec![]);
    state.stack[0].scroll = u16::MAX;
    state.stack[0].hscroll = u16::MAX;
    state.apply_motion(&empty, Motion::Bottom, KeyCode::End, viewport, layout, None);
    assert_eq!((state.stack[0].scroll, state.stack[0].hscroll), (0, 0));
    let mut huge_lines = vec!["x".into(); 65_540];
    huge_lines[0] = "x".repeat(65_540).into();
    let huge = ProjectContent::Lines(huge_lines);
    state.apply_motion(&huge, Motion::Bottom, KeyCode::End, viewport, layout, None);
    assert_eq!(state.stack[0].scroll, 65_535);
    state.apply_motion(&huge, Motion::Down, KeyCode::Down, viewport, layout, None);
    assert_eq!(state.stack[0].scroll, 65_535);
    state.apply_motion(
        &huge,
        Motion::LineEnd,
        KeyCode::End,
        ProjectViewport { cols: 0, rows: 0 },
        layout,
        None,
    );
    assert_eq!(state.stack[0].hscroll, 65_535);
    state.apply_motion(
        &huge,
        Motion::Right,
        KeyCode::Right,
        ProjectViewport { cols: 0, rows: 0 },
        layout,
        None,
    );
    assert_eq!(state.stack[0].hscroll, 65_535);
}
