use super::prompt::*;
use super::{ProjectKey, ScreenKey};
use crate::project::{
    Choice, ChoiceInput, ConfirmChoice, Confirmation, ProjectAct, ProjectRow, TextInput,
};
use crate::{Act, Value};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn invocation() -> ProjectInvocation<u8> {
    ProjectInvocation {
        action: ProjectActionId {
            plugin: "git".into(),
            action: "save".into(),
        },
        declaration: Act::new("save")
            .confirmation(Confirmation::new(
                "Save {row}?",
                vec![ConfirmChoice::new('y', "yes", "Yes")],
            ))
            .text_input(TextInput::new("Message", "初é").keep_on_failure())
            .choice_input(ChoiceInput::new(
                "Kind",
                vec![
                    Choice::new("first", "First"),
                    Choice::new("second", "Second"),
                ],
            )),
        context: ProjectAct::default().selected(ProjectRow::new("file", "saved").name("Snapshot")),
        origin: ProjectOrigin {
            context: 7,
            project: ProjectKey {
                plugin: "git".into(),
                root: "/project".into(),
                marker: ".git".into(),
            },
            root: "/project/sub".into(),
            screen: Some(ScreenKey::new("changes", 7)),
            surface: 1,
        },
    }
}

fn key(
    dialog: &mut Option<ProjectDialog<ProjectInvocation<u8>>>,
    code: KeyCode,
) -> ProjectPromptUpdate<ProjectInvocation<u8>> {
    handle_key(dialog, KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn prompt_chain_preserves_answers_and_resets_stage_local_state() {
    let mut dialog = None;
    assert!(matches!(
        begin_act(&mut dialog, invocation(), true),
        ProjectPromptUpdate::DialogChanged
    ));
    assert!(
        matches!(dialog, Some(ProjectDialog::Act { ref prompts, stage: 0, ref buffer, cursor: 0, selected: 0, .. })
        if matches!(prompts.as_slice(), [ProjectPrompt::Confirmation(_), ProjectPrompt::Text(_), ProjectPrompt::Choice(_)]) && buffer.is_empty())
    );
    if let Some(ProjectDialog::Act { selected, .. }) = &mut dialog {
        *selected = 1;
    }
    key(&mut dialog, KeyCode::Char('y'));
    assert!(
        matches!(dialog, Some(ProjectDialog::Act { ref invocation, stage: 1, ref buffer, cursor: 2, selected: 0, .. })
        if buffer == "初é" && invocation.context.confirmation.as_deref() == Some("yes"))
    );
    if let Some(ProjectDialog::Act { selected, .. }) = &mut dialog {
        *selected = 1;
    }
    key(&mut dialog, KeyCode::Enter);
    assert!(
        matches!(dialog, Some(ProjectDialog::Act { ref invocation, stage: 2, ref buffer, cursor: 0, selected: 0, .. })
        if buffer.is_empty() && invocation.context.text.as_deref() == Some("初é") && invocation.context.confirmation.as_deref() == Some("yes"))
    );
    key(&mut dialog, KeyCode::Down);
    let ProjectPromptUpdate::InvocationReady(done) = key(&mut dialog, KeyCode::Enter) else {
        panic!("ready invocation")
    };
    assert_eq!(done.context.confirmation.as_deref(), Some("yes"));
    assert_eq!(done.context.text.as_deref(), Some("初é"));
    assert_eq!(done.context.choice.as_deref(), Some("second"));
    assert!(dialog.is_none());

    let prompt = invocation()
        .declaration
        .confirmation
        .unwrap()
        .setting("ask");
    assert!(!confirmation_enabled(
        &prompt,
        Some(&Value::Bool(false)),
        Some(&Value::Bool(true))
    ));
    assert!(confirmation_enabled(
        &prompt,
        Some(&Value::Str("false".into())),
        Some(&Value::Bool(false))
    ));
    assert!(!confirmation_enabled(
        &prompt,
        None,
        Some(&Value::Bool(false))
    ));
    assert!(confirmation_enabled(&prompt, None, None));
    assert!(confirmation_enabled(
        &Confirmation::new("Ask", vec![]),
        Some(&Value::Bool(false)),
        None
    ));
    begin_act(&mut dialog, invocation(), false);
    assert!(
        matches!(dialog, Some(ProjectDialog::Act { ref prompts, ref buffer, cursor: 2, .. })
        if matches!(prompts.as_slice(), [ProjectPrompt::Text(_), ProjectPrompt::Choice(_)]) && buffer == "初é")
    );
}

#[test]
fn confirmation_cancel_and_case_matching() {
    for code in [KeyCode::Char('n'), KeyCode::Char('N'), KeyCode::Esc] {
        let mut inv = invocation();
        inv.declaration.confirmation = Some(Confirmation::new(
            "Ask",
            vec![
                ConfirmChoice::new('n', "bad", "Bad"),
                ConfirmChoice::new('N', "bad-upper", "Bad"),
            ],
        ));
        let mut dialog = None;
        begin_act(&mut dialog, inv, true);
        assert!(matches!(
            key(&mut dialog, code),
            ProjectPromptUpdate::Cancelled { notice: true }
        ));
        assert!(dialog.is_none());
    }
    for (choices, code, expected) in [
        (
            vec![
                ConfirmChoice::new('y', "lower", "Lower"),
                ConfirmChoice::new('Y', "upper", "Upper"),
            ],
            KeyCode::Char('Y'),
            "upper",
        ),
        (
            vec![ConfirmChoice::new('y', "lower", "Lower")],
            KeyCode::Char('Y'),
            "lower",
        ),
    ] {
        let mut inv = invocation();
        inv.declaration.confirmation = Some(Confirmation::new("Ask", choices));
        inv.declaration.text_input = None;
        inv.declaration.choice_input = None;
        let mut dialog = None;
        begin_act(&mut dialog, inv, true);
        for ignored in [KeyCode::Enter, KeyCode::Char('z'), KeyCode::Down] {
            assert!(matches!(
                key(&mut dialog, ignored),
                ProjectPromptUpdate::DialogChanged
            ));
            assert!(matches!(dialog, Some(ProjectDialog::Act { stage: 0, .. })));
        }
        let ProjectPromptUpdate::InvocationReady(done) = key(&mut dialog, code) else {
            panic!("declared choice")
        };
        assert_eq!(done.context.confirmation.as_deref(), Some(expected));
    }
}

#[test]
fn text_cursor_counts_unicode_characters() {
    let mut dialog = None;
    begin_act(&mut dialog, invocation(), false);
    for (code, text, position) in [
        (KeyCode::Left, "初é", 1),
        (KeyCode::Char('猫'), "初猫é", 2),
        (KeyCode::Backspace, "初é", 1),
        (KeyCode::Delete, "初", 1),
        (KeyCode::Home, "初", 0),
        (KeyCode::Left, "初", 0),
        (KeyCode::Char('😀'), "😀初", 1),
        (KeyCode::End, "😀初", 2),
        (KeyCode::Right, "😀初", 2),
    ] {
        key(&mut dialog, code);
        assert!(
            matches!(dialog, Some(ProjectDialog::Act { ref buffer, cursor, .. }) if buffer == text && cursor == position)
        );
    }
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        handle_key(&mut dialog, KeyEvent::new(KeyCode::Char('x'), modifiers));
        assert!(
            matches!(dialog, Some(ProjectDialog::Act { ref buffer, cursor: 2, .. }) if buffer == "😀初")
        );
    }
    assert!(matches!(
        key(&mut dialog, KeyCode::Esc),
        ProjectPromptUpdate::Cancelled { notice: false }
    ));
    assert!(dialog.is_none());
}

#[test]
fn row_substitution_uses_invocation_snapshot() {
    let mut snapshot = invocation();
    let prompt = snapshot.declaration.confirmation.as_ref().unwrap();
    assert_eq!(snapshot.confirmation_text(prompt, "en"), "Save Snapshot?");
    snapshot.context.selected.as_mut().unwrap().name = None;
    assert_eq!(snapshot.confirmation_text(prompt, "en"), "Save saved?");
    snapshot.context.selected = None;
    assert_eq!(snapshot.confirmation_text(prompt, "en"), "Save ?");
    let localized = Confirmation::new(crate::t!(en: "Delete {row}", ja: "{row} を削除"), vec![]);
    assert_eq!(snapshot.confirmation_text(&localized, "ja"), " を削除");
}

#[test]
fn retry_reopens_only_text_without_reconfirmation() {
    let mut inv = invocation();
    inv.context.confirmation = Some("yes".into());
    inv.context.text = Some("再入力é".into());
    inv.context.choice = Some("second".into());
    let current = inv.origin.clone();
    let mut dialog = None;
    assert!(reopen_text(
        &mut dialog,
        inv.clone(),
        &current,
        ProjectPromptBlockers::default()
    ));
    assert!(
        matches!(dialog, Some(ProjectDialog::Act { ref prompts, stage: 0, ref buffer, cursor: 4, selected: 0, .. })
        if matches!(prompts.as_slice(), [ProjectPrompt::Text(_)]) && buffer == "再入力é")
    );
    assert!(!reopen_text(
        &mut dialog,
        inv.clone(),
        &current,
        ProjectPromptBlockers::default()
    ));
    let ProjectPromptUpdate::InvocationReady(done) = key(&mut dialog, KeyCode::Enter) else {
        panic!("retry submits directly")
    };
    assert_eq!(
        done.context.selected,
        Some(ProjectRow::new("file", "saved").name("Snapshot"))
    );
    assert_eq!(done.context.confirmation.as_deref(), Some("yes"));
    assert_eq!(done.context.choice.as_deref(), Some("second"));
    assert_eq!(done.context.text.as_deref(), Some("再入力é"));
    dialog = Some(ProjectDialog::Projects {
        projects: vec![current.project.clone()],
        selected: 0,
    });
    assert!(!reopen_text(
        &mut dialog,
        inv.clone(),
        &current,
        ProjectPromptBlockers::default()
    ));
    assert!(matches!(
        dialog,
        Some(ProjectDialog::Projects { selected: 0, .. })
    ));
    dialog = None;
    for blockers in [
        ProjectPromptBlockers {
            dialog: true,
            ..Default::default()
        },
        ProjectPromptBlockers {
            text_input: true,
            ..Default::default()
        },
        ProjectPromptBlockers {
            filtering: true,
            ..Default::default()
        },
    ] {
        assert!(!reopen_text(&mut dialog, inv.clone(), &current, blockers));
    }
    for field in 0..4 {
        let mut changed = current.clone();
        match field {
            0 => changed.context += 1,
            1 => changed.project.marker = "other".into(),
            2 => changed.root = "/other".into(),
            _ => changed.screen = None,
        }
        assert!(!reopen_text(
            &mut dialog,
            inv.clone(),
            &changed,
            ProjectPromptBlockers::default()
        ));
    }
    let mut other_surface = current.clone();
    other_surface.surface = 2;
    assert!(reopen_text(
        &mut dialog,
        inv.clone(),
        &other_surface,
        ProjectPromptBlockers::default()
    ));
    dialog = None;
    inv.declaration.text_input.as_mut().unwrap().keep_on_failure = false;
    assert!(!reopen_text(
        &mut dialog,
        inv.clone(),
        &current,
        ProjectPromptBlockers::default()
    ));
    inv.declaration.text_input.as_mut().unwrap().keep_on_failure = true;
    inv.context.text = None;
    assert!(!reopen_text(
        &mut dialog,
        inv.clone(),
        &current,
        ProjectPromptBlockers::default()
    ));
    inv.declaration.text_input = None;
    assert!(!reopen_text(
        &mut dialog,
        inv,
        &current,
        ProjectPromptBlockers::default()
    ));
}

#[test]
fn malformed_stage_cancels_safely() {
    for stage in [3, usize::MAX] {
        for code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('y')] {
            let mut dialog = None;
            begin_act(&mut dialog, invocation(), true);
            if let Some(ProjectDialog::Act { stage: at, .. }) = &mut dialog {
                *at = stage;
            }
            assert!(matches!(
                key(&mut dialog, code),
                ProjectPromptUpdate::Cancelled { notice: true }
            ));
            assert!(dialog.is_none());
        }
    }
}

#[test]
fn chooser_handles_zero_one_many_and_cancel() {
    let project = invocation().origin.project;
    let mut other = project.clone();
    other.plugin = "jj".into();
    let mut dialog: Option<ProjectDialog<ProjectInvocation<u8>>> = None;
    assert!(matches!(
        choose_projects(&mut dialog, vec![]),
        ProjectPromptUpdate::Unchanged
    ));
    assert!(dialog.is_none());
    assert!(
        matches!(choose_projects(&mut dialog, vec![project.clone()]), ProjectPromptUpdate::ChosenProject(key) if key == project)
    );
    assert!(dialog.is_none());
    choose_projects(&mut dialog, vec![project.clone(), other.clone()]);
    key(&mut dialog, KeyCode::Up);
    key(&mut dialog, KeyCode::Down);
    key(&mut dialog, KeyCode::Down);
    assert!(
        matches!(key(&mut dialog, KeyCode::Enter), ProjectPromptUpdate::ChosenProject(key) if key == other)
    );
    assert!(dialog.is_none());
    choose_projects(&mut dialog, vec![project, other]);
    assert!(matches!(
        key(&mut dialog, KeyCode::Esc),
        ProjectPromptUpdate::Cancelled { notice: false }
    ));
    assert!(dialog.is_none());
    let mut inv = invocation();
    assert!(!inv.destructive());
    inv.declaration.destructive = true;
    assert!(inv.destructive());
    inv.declaration.confirmation = None;
    inv.declaration.text_input = None;
    inv.declaration.choice_input = None;
    assert!(matches!(
        begin_act(&mut dialog, inv, true),
        ProjectPromptUpdate::InvocationReady(_)
    ));
    assert!(dialog.is_none());
}
