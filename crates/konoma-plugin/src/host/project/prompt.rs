//! Pure project prompt transitions. Hosts execute returned invocations and translate notices.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{ProjectKey, ScreenKey};
use crate::project::{ChoiceInput, Confirmation, ProjectAct, TextInput};
use crate::{Act, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
/// Owned identifiers, independent of a host's key binding handles.
pub struct ProjectActionId {
    pub plugin: String,
    pub action: String,
}

/// The surface is opaque: only the host knows its mode and preview path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectOrigin<H> {
    pub context: u64,
    pub project: ProjectKey,
    pub root: PathBuf,
    pub screen: Option<ScreenKey>,
    pub surface: H,
}

#[derive(Clone)]
/// The request-time selection and answers travel together through every prompt and retry.
pub struct ProjectInvocation<H> {
    pub action: ProjectActionId,
    pub declaration: Act,
    pub context: ProjectAct,
    pub origin: ProjectOrigin<H>,
}

/// Allows a host to retain its effects envelope around the single shared invocation.
pub trait PromptInvocation {
    type Surface;
    fn prompt_invocation(&self) -> &ProjectInvocation<Self::Surface>;
    fn prompt_invocation_mut(&mut self) -> &mut ProjectInvocation<Self::Surface>;
}

impl<H> PromptInvocation for ProjectInvocation<H> {
    type Surface = H;
    fn prompt_invocation(&self) -> &Self {
        self
    }
    fn prompt_invocation_mut(&mut self) -> &mut Self {
        self
    }
}

impl<H> ProjectInvocation<H> {
    pub fn destructive(&self) -> bool {
        self.declaration.destructive
    }
    pub fn confirmation_text(&self, prompt: &Confirmation, lang: &str) -> String {
        let name = self
            .context
            .selected
            .as_ref()
            .map(|row| row.name.as_deref().unwrap_or(&row.value))
            .unwrap_or("");
        prompt.text.get(lang).replace("{row}", name)
    }
}

#[derive(Clone)]
pub enum ProjectPrompt {
    Confirmation(Confirmation),
    Text(TextInput),
    Choice(ChoiceInput),
}

pub enum ProjectDialog<I> {
    Projects {
        projects: Vec<ProjectKey>,
        selected: usize,
    },
    Act {
        invocation: Box<I>,
        prompts: Vec<ProjectPrompt>,
        stage: usize,
        buffer: String,
        /// Character position in `buffer`, not a byte offset.
        cursor: usize,
        selected: usize,
    },
}

pub enum ProjectPromptUpdate<I> {
    Unchanged,
    DialogChanged,
    /// Confirmation and malformed-stage cancellation request a localized notice.
    Cancelled {
        notice: bool,
    },
    ChosenProject(ProjectKey),
    InvocationReady(I),
}

#[derive(Clone, Copy, Default)]
pub struct ProjectPromptBlockers {
    pub dialog: bool,
    pub text_input: bool,
    pub filtering: bool,
}

/// Call only for a failed act, never for a failed follow-up watch or refresh.
pub fn reopen_text<I: PromptInvocation>(
    dialog: &mut Option<ProjectDialog<I>>,
    invocation: I,
    current: &ProjectOrigin<I::Surface>,
    blockers: ProjectPromptBlockers,
) -> bool {
    let shared = invocation.prompt_invocation();
    // Retry follows the project frame. The host rechecks its surface before executing the act.
    if shared.origin.context != current.context
        || shared.origin.project != current.project
        || shared.origin.root != current.root
        || shared.origin.screen != current.screen
        || current.screen.is_none()
        || dialog.is_some()
        || blockers.dialog
        || blockers.text_input
        || blockers.filtering
    {
        return false;
    }
    let Some(prompt) = shared
        .declaration
        .text_input
        .as_ref()
        .filter(|prompt| prompt.keep_on_failure)
        .cloned()
    else {
        return false;
    };
    let Some(buffer) = shared.context.text.clone() else {
        return false;
    };
    let cursor = buffer.chars().count();
    *dialog = Some(ProjectDialog::Act {
        invocation: Box::new(invocation),
        prompts: vec![ProjectPrompt::Text(prompt)],
        stage: 0,
        buffer,
        cursor,
        selected: 0,
    });
    true
}

pub fn confirmation_enabled(
    prompt: &Confirmation,
    configured: Option<&Value>,
    default: Option<&Value>,
) -> bool {
    prompt.setting.is_none() || !matches!(configured.or(default), Some(Value::Bool(false)))
}

/// Opens confirmation, text and choice in that order, or returns an invocation to execute.
pub fn begin_act<I: PromptInvocation>(
    dialog: &mut Option<ProjectDialog<I>>,
    invocation: I,
    confirmation: bool,
) -> ProjectPromptUpdate<I> {
    let declaration = &invocation.prompt_invocation().declaration;
    let mut prompts = Vec::new();
    if let Some(prompt) = declaration.confirmation.as_ref().filter(|_| confirmation) {
        prompts.push(ProjectPrompt::Confirmation(prompt.clone()));
    }
    if let Some(prompt) = &declaration.text_input {
        prompts.push(ProjectPrompt::Text(prompt.clone()));
    }
    if let Some(prompt) = &declaration.choice_input {
        prompts.push(ProjectPrompt::Choice(prompt.clone()));
    }
    if prompts.is_empty() {
        return ProjectPromptUpdate::InvocationReady(invocation);
    }
    let buffer = initial_buffer(prompts.first());
    let cursor = buffer.chars().count();
    *dialog = Some(ProjectDialog::Act {
        invocation: Box::new(invocation),
        prompts,
        stage: 0,
        buffer,
        cursor,
        selected: 0,
    });
    ProjectPromptUpdate::DialogChanged
}

fn initial_buffer(prompt: Option<&ProjectPrompt>) -> String {
    match prompt {
        Some(ProjectPrompt::Text(input)) => input.initial.clone(),
        _ => String::new(),
    }
}

/// Edits the single dialog slot without executing actions or opening a host surface.
pub fn handle_key<I: PromptInvocation>(
    dialog: &mut Option<ProjectDialog<I>>,
    key: KeyEvent,
) -> ProjectPromptUpdate<I> {
    let Some(current) = dialog.take() else {
        return ProjectPromptUpdate::Unchanged;
    };
    if matches!(&current, ProjectDialog::Act { prompts, stage, .. } if prompts.get(*stage).is_none())
    {
        return ProjectPromptUpdate::Cancelled { notice: true };
    }
    if matches!(&current, ProjectDialog::Act { prompts, stage, .. } if matches!(prompts.get(*stage), Some(ProjectPrompt::Confirmation(_))))
        && matches!(key.code, KeyCode::Char('n' | 'N') | KeyCode::Esc)
    {
        return ProjectPromptUpdate::Cancelled { notice: true };
    }
    if key.code == KeyCode::Esc {
        return ProjectPromptUpdate::Cancelled { notice: false };
    }
    match current {
        ProjectDialog::Projects {
            projects,
            mut selected,
        } => {
            match key.code {
                KeyCode::Enter => {
                    return projects
                        .get(selected)
                        .cloned()
                        .map(ProjectPromptUpdate::ChosenProject)
                        .unwrap_or(ProjectPromptUpdate::DialogChanged)
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    selected = selected
                        .saturating_add(1)
                        .min(projects.len().saturating_sub(1))
                }
                KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
                _ => {}
            }
            *dialog = Some(ProjectDialog::Projects { projects, selected });
        }
        ProjectDialog::Act {
            mut invocation,
            prompts,
            mut stage,
            mut buffer,
            mut cursor,
            mut selected,
        } => {
            let context = &mut invocation.prompt_invocation_mut().context;
            let mut answered = false;
            let Some(prompt) = prompts.get(stage) else {
                return ProjectPromptUpdate::Cancelled { notice: true };
            };
            match prompt {
                ProjectPrompt::Confirmation(prompt) => {
                    if let KeyCode::Char(c) = key.code {
                        if let Some(choice) = prompt
                            .choices
                            .iter()
                            .find(|choice| choice.key == c)
                            .or_else(|| {
                                if c.is_ascii_uppercase() {
                                    prompt
                                        .choices
                                        .iter()
                                        .find(|choice| choice.key == c.to_ascii_lowercase())
                                } else {
                                    None
                                }
                            })
                        {
                            context.confirmation = Some(choice.choice.value.clone());
                            answered = true;
                        }
                    }
                }
                ProjectPrompt::Text(_) => match key.code {
                    KeyCode::Enter => {
                        context.text = Some(buffer.clone());
                        answered = true;
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        buffer.insert(char_byte(&buffer, cursor), c);
                        cursor += 1;
                    }
                    KeyCode::Backspace if cursor > 0 => {
                        let start = char_byte(&buffer, cursor - 1);
                        let end = char_byte(&buffer, cursor);
                        buffer.replace_range(start..end, "");
                        cursor -= 1;
                    }
                    KeyCode::Delete if cursor < buffer.chars().count() => {
                        let start = char_byte(&buffer, cursor);
                        let end = char_byte(&buffer, cursor + 1);
                        buffer.replace_range(start..end, "");
                    }
                    KeyCode::Left => cursor = cursor.saturating_sub(1),
                    KeyCode::Right => cursor = (cursor + 1).min(buffer.chars().count()),
                    KeyCode::Home => cursor = 0,
                    KeyCode::End => cursor = buffer.chars().count(),
                    _ => {}
                },
                ProjectPrompt::Choice(prompt) => match key.code {
                    KeyCode::Char('j') | KeyCode::Down => {
                        selected = (selected + 1).min(prompt.choices.len().saturating_sub(1))
                    }
                    KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Enter => {
                        if let Some(choice) = prompt.choices.get(selected) {
                            context.choice = Some(choice.value.clone());
                            answered = true;
                        }
                    }
                    _ => {}
                },
            }
            if answered {
                stage += 1;
                if stage == prompts.len() {
                    return ProjectPromptUpdate::InvocationReady(*invocation);
                }
                buffer = initial_buffer(prompts.get(stage));
                cursor = buffer.chars().count();
                selected = 0;
            }
            *dialog = Some(ProjectDialog::Act {
                invocation,
                prompts,
                stage,
                buffer,
                cursor,
                selected,
            });
        }
    }
    ProjectPromptUpdate::DialogChanged
}

fn char_byte(text: &str, cursor: usize) -> usize {
    text.char_indices()
        .nth(cursor)
        .map_or(text.len(), |(index, _)| index)
}

/// Drops the chooser's candidates that `keep` refuses (projects the latest detection no longer
/// lists), keeping the selection on the same project where it survives. Returns whether a chooser
/// was open and had no candidate left; it is then closed. Any other dialog is left alone.
pub fn retain_projects<I>(
    dialog: &mut Option<ProjectDialog<I>>,
    keep: impl Fn(&ProjectKey) -> bool,
) -> bool {
    let Some(ProjectDialog::Projects { projects, selected }) = dialog else {
        return false;
    };
    let current = projects.get(*selected).cloned();
    projects.retain(|key| keep(key));
    if projects.is_empty() {
        *dialog = None;
        return true;
    }
    *selected = current
        .and_then(|key| projects.iter().position(|p| *p == key))
        .unwrap_or((*selected).min(projects.len() - 1));
    false
}

/// Closes an act dialog (confirmation, text or choice input) whose invocation belongs to
/// `project`, for a project that is no longer detected: its keys must not stay with a dialog that
/// can no longer act. Returns whether one was closed. The chooser and the dialog of another
/// project stay.
pub fn close_act_dialog_of<I: PromptInvocation>(
    dialog: &mut Option<ProjectDialog<I>>,
    project: &ProjectKey,
) -> bool {
    let belongs = matches!(
        dialog,
        Some(ProjectDialog::Act { invocation, .. })
            if invocation.prompt_invocation().origin.project == *project
    );
    if belongs {
        *dialog = None;
    }
    belongs
}

/// No candidates are unhandled; one opens directly; several open the chooser.
pub fn choose_projects<I>(
    dialog: &mut Option<ProjectDialog<I>>,
    projects: Vec<ProjectKey>,
) -> ProjectPromptUpdate<I> {
    match projects.as_slice() {
        [] => ProjectPromptUpdate::Unchanged,
        [project] => ProjectPromptUpdate::ChosenProject(project.clone()),
        _ => {
            *dialog = Some(ProjectDialog::Projects {
                projects,
                selected: 0,
            });
            ProjectPromptUpdate::DialogChanged
        }
    }
}
