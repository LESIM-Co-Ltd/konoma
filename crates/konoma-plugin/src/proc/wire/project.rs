//! Protocol 2 project declarations and prompts.

use super::*;
use crate::project::*;

/// The minimum protocol needed to preserve this declaration's meaning.
pub fn manifest_protocol(m: &WManifest) -> u32 {
    if m.read_only
        || m.project.is_some()
        || m.actions.iter().any(|a| {
            a.copy
                || !a.refresh.is_empty()
                || a.project_context.is_some()
                || a.confirmation.is_some()
                || a.text_input.is_some()
                || a.choice_input.is_some()
        })
    {
        2
    } else {
        1
    }
}

pub fn input_protocol(i: &WInput) -> u32 {
    if matches!(i, WInput::Project { .. }) {
        2
    } else {
        1
    }
}
pub fn outcome_protocol(o: &WOutcome) -> u32 {
    if matches!(
        o,
        WOutcome::Unhandled | WOutcome::Redraw | WOutcome::Flash(_) | WOutcome::Reload
    ) {
        1
    } else {
        2
    }
}
pub fn view_protocol(v: &WView) -> u32 {
    if matches!(v, WView::List(rows) if rows.iter().any(|r| !r.selectable || r.marked || r.initially_selected || r.filter_fallback || !r.value.is_empty()))
    {
        2
    } else {
        1
    }
}

pub fn require_protocol(selected: u32, required: u32) -> Result<(), WireError> {
    if selected < required {
        Err(WireError::bad(format!(
            "project data requires protocol {required}, negotiated {selected}"
        )))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WMarkerKind {
    Any,
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WDetectionMarker {
    pub name: String,
    pub kind: WMarkerKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WProjectScreen {
    pub id: String,
    pub title: WTr,
    pub key: Option<char>,
    pub row_kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub marks: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub checks: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub pushed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WProjectDecl {
    pub markers: Vec<WDetectionMarker>,
    pub screens: Vec<WProjectScreen>,
    pub chip: Option<WTr>,
    pub watch_paths: Vec<WPath>,
    pub version_control: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WActContext {
    Screen {
        screen: String,
        row_kinds: Vec<String>,
    },
    TreeSelection,
    Diff,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WChoice {
    pub value: String,
    pub label: WTr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WConfirmChoice {
    pub key: char,
    pub choice: WChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WTr>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WConfirmation {
    pub text: WTr,
    pub choices: Vec<WConfirmChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WTextInput {
    pub label: WTr,
    pub initial: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub keep_on_failure: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WChoiceInput {
    pub label: WTr,
    pub choices: Vec<WChoice>,
}

pub fn marker_kind_to_wire(kind: MarkerKind) -> WMarkerKind {
    match kind {
        MarkerKind::Any => WMarkerKind::Any,
        MarkerKind::File => WMarkerKind::File,
        MarkerKind::Directory => WMarkerKind::Directory,
    }
}

pub fn marker_kind_from_wire(kind: WMarkerKind) -> MarkerKind {
    match kind {
        WMarkerKind::Any => MarkerKind::Any,
        WMarkerKind::File => MarkerKind::File,
        WMarkerKind::Directory => MarkerKind::Directory,
    }
}

pub fn detection_marker_to_wire(marker: &DetectionMarker) -> WDetectionMarker {
    WDetectionMarker {
        name: marker.name.clone(),
        kind: marker_kind_to_wire(marker.kind),
    }
}

pub fn detection_marker_from_wire(marker: WDetectionMarker) -> DetectionMarker {
    DetectionMarker {
        name: marker.name,
        kind: marker_kind_from_wire(marker.kind),
    }
}

pub fn project_screen_to_wire(screen: &ProjectScreen) -> WProjectScreen {
    WProjectScreen {
        id: screen.id.clone(),
        title: tr_to_wire(&screen.title),
        key: screen.key,
        row_kinds: screen.row_kinds.clone(),
        marks: screen.marks,
        checks: screen.checks,
        pushed: screen.pushed,
    }
}

pub fn project_screen_from_wire(screen: WProjectScreen) -> ProjectScreen {
    ProjectScreen {
        id: screen.id,
        title: tr_from_wire(screen.title),
        key: screen.key,
        row_kinds: screen.row_kinds,
        marks: screen.marks,
        checks: screen.checks,
        pushed: screen.pushed,
    }
}

pub fn project_to_wire(p: &ProjectDecl) -> WProjectDecl {
    WProjectDecl {
        markers: p.markers.iter().map(detection_marker_to_wire).collect(),
        screens: p.screens.iter().map(project_screen_to_wire).collect(),
        chip: p.chip.as_ref().map(tr_to_wire),
        watch_paths: p.watch_paths.iter().map(|p| path_to_wire(p)).collect(),
        version_control: p.version_control,
    }
}

pub fn project_from_wire(p: WProjectDecl) -> Result<ProjectDecl, WireError> {
    Ok(ProjectDecl {
        markers: p
            .markers
            .into_iter()
            .map(detection_marker_from_wire)
            .collect(),
        screens: p
            .screens
            .into_iter()
            .map(project_screen_from_wire)
            .collect(),
        chip: p.chip.map(tr_from_wire),
        watch_paths: p
            .watch_paths
            .into_iter()
            .map(path_from_wire)
            .collect::<Result<_, _>>()?,
        version_control: p.version_control,
    })
}

pub fn act_context_to_wire(c: &ActContext) -> WActContext {
    match c {
        ActContext::Screen { screen, row_kinds } => WActContext::Screen {
            screen: screen.clone(),
            row_kinds: row_kinds.clone(),
        },
        ActContext::TreeSelection => WActContext::TreeSelection,
        ActContext::Diff => WActContext::Diff,
    }
}

pub fn act_context_from_wire(c: WActContext) -> ActContext {
    match c {
        WActContext::Screen { screen, row_kinds } => ActContext::Screen { screen, row_kinds },
        WActContext::TreeSelection => ActContext::TreeSelection,
        WActContext::Diff => ActContext::Diff,
    }
}

pub fn choice_to_wire(c: &Choice) -> WChoice {
    WChoice {
        value: c.value.clone(),
        label: tr_to_wire(&c.label),
    }
}
pub fn choice_from_wire(c: WChoice) -> Choice {
    Choice {
        value: c.value,
        label: tr_from_wire(c.label),
    }
}

pub fn confirm_choice_to_wire(choice: &ConfirmChoice) -> WConfirmChoice {
    WConfirmChoice {
        key: choice.key,
        choice: choice_to_wire(&choice.choice),
        status: choice.status.as_ref().map(tr_to_wire),
    }
}

pub fn confirm_choice_from_wire(choice: WConfirmChoice) -> ConfirmChoice {
    ConfirmChoice {
        key: choice.key,
        choice: choice_from_wire(choice.choice),
        status: choice.status.map(tr_from_wire),
    }
}

pub fn confirmation_to_wire(c: &Confirmation) -> WConfirmation {
    WConfirmation {
        setting: c.setting.clone(),
        text: tr_to_wire(&c.text),
        choices: c.choices.iter().map(confirm_choice_to_wire).collect(),
    }
}
pub fn confirmation_from_wire(c: WConfirmation) -> Confirmation {
    Confirmation {
        setting: c.setting,
        text: tr_from_wire(c.text),
        choices: c
            .choices
            .into_iter()
            .map(confirm_choice_from_wire)
            .collect(),
    }
}
pub fn text_input_to_wire(i: &TextInput) -> WTextInput {
    WTextInput {
        keep_on_failure: i.keep_on_failure,
        label: tr_to_wire(&i.label),
        initial: i.initial.clone(),
    }
}
pub fn text_input_from_wire(i: WTextInput) -> TextInput {
    TextInput {
        keep_on_failure: i.keep_on_failure,
        label: tr_from_wire(i.label),
        initial: i.initial,
    }
}
pub fn choice_input_to_wire(i: &ChoiceInput) -> WChoiceInput {
    WChoiceInput {
        label: tr_to_wire(&i.label),
        choices: i.choices.iter().map(choice_to_wire).collect(),
    }
}
pub fn choice_input_from_wire(i: WChoiceInput) -> ChoiceInput {
    ChoiceInput {
        label: tr_from_wire(i.label),
        choices: i.choices.into_iter().map(choice_from_wire).collect(),
    }
}
