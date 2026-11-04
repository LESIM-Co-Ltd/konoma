//! Project declarations. Detection and screen navigation are performed by the host.

use std::path::PathBuf;

use crate::text::Tr;

/// The project screen and opaque selection to draw in this view request.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProjectView {
    pub screen: String,
    pub selection: Option<String>,
}

impl ProjectView {
    pub fn new(screen: impl Into<String>) -> Self {
        Self {
            screen: screen.into(),
            selection: None,
        }
    }
    pub fn selection(mut self, selection: impl Into<String>) -> Self {
        self.selection = Some(selection.into());
        self
    }
}

/// A row's declared kind and opaque stable value, without its display cells.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProjectRow {
    pub kind: String,
    pub value: String,
}

impl ProjectRow {
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            value: value.into(),
        }
    }
}

impl From<&crate::view::ListRow> for ProjectRow {
    fn from(row: &crate::view::ListRow) -> Self {
        Self::new(&row.kind, &row.value)
    }
}

/// Project-only action data. A tree or diff action may have no project screen.
/// Answers contain the declared choice values, never labels or pressed keys.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProjectAct {
    pub screen: Option<String>,
    pub selected: Option<ProjectRow>,
    pub marked: Vec<ProjectRow>,
    pub confirmation: Option<String>,
    pub text: Option<String>,
    pub choice: Option<String>,
}

impl ProjectAct {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn screen(mut self, screen: impl Into<String>) -> Self {
        self.screen = Some(screen.into());
        self
    }
    pub fn selected(mut self, row: ProjectRow) -> Self {
        self.selected = Some(row);
        self
    }
    pub fn marked(mut self, rows: impl IntoIterator<Item = ProjectRow>) -> Self {
        self.marked = rows.into_iter().collect();
        self
    }
    pub fn confirmation(mut self, value: impl Into<String>) -> Self {
        self.confirmation = Some(value.into());
        self
    }
    pub fn text(mut self, value: impl Into<String>) -> Self {
        self.text = Some(value.into());
        self
    }
    pub fn choice(mut self, value: impl Into<String>) -> Self {
        self.choice = Some(value.into());
        self
    }
}

/// What the host refreshes after the action completes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefreshTarget {
    CurrentScreen,
    Screen { screen: String },
    AllScreens,
    TreeStatus,
    Chip,
}

impl RefreshTarget {
    pub fn screen(screen: impl Into<String>) -> Self {
        Self::Screen {
            screen: screen.into(),
        }
    }
}

/// Which directory entry a detection marker matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum MarkerKind {
    #[default]
    Any,
    File,
    Directory,
}

/// A case-sensitive file-name glob, searched at the tree root and each ancestor.
/// All matching markers may produce projects; a marker is not a path to traverse.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DetectionMarker {
    pub name: String,
    pub kind: MarkerKind,
}

impl DetectionMarker {
    pub fn new(name: impl Into<String>, kind: MarkerKind) -> Self {
        Self {
            name: name.into(),
            kind,
        }
    }
}

/// One project screen. Row kinds are local to this screen.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProjectScreen {
    pub id: String,
    pub title: Tr,
    /// An optional single character that jumps to this screen.
    pub key: Option<char>,
    pub row_kinds: Vec<String>,
}

impl ProjectScreen {
    pub fn new(id: impl Into<String>, title: impl Into<Tr>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            key: None,
            row_kinds: Vec::new(),
        }
    }

    pub fn key(mut self, key: char) -> Self {
        self.key = Some(key);
        self
    }
    pub fn row_kind(mut self, kind: impl Into<String>) -> Self {
        self.row_kinds.push(kind.into());
        self
    }
}

/// A project offered alongside the plugin's existing file bindings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProjectDecl {
    pub markers: Vec<DetectionMarker>,
    pub screens: Vec<ProjectScreen>,
    /// The short summary displayed beside the tree title.
    pub chip: Option<Tr>,
    /// Paths relative to the project root. A change invalidates the project's data.
    pub watch_paths: Vec<PathBuf>,
    /// An advertisement only: the capability itself is available to static plugins only.
    pub version_control: bool,
}

impl ProjectDecl {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn marker(mut self, marker: DetectionMarker) -> Self {
        self.markers.push(marker);
        self
    }
    pub fn screen(mut self, screen: ProjectScreen) -> Self {
        self.screens.push(screen);
        self
    }
    pub fn chip(mut self, chip: impl Into<Tr>) -> Self {
        self.chip = Some(chip.into());
        self
    }
    pub fn watch(mut self, path: impl Into<PathBuf>) -> Self {
        self.watch_paths.push(path.into());
        self
    }
    pub fn version_control(mut self) -> Self {
        self.version_control = true;
        self
    }
}

/// Where a project action applies. `Enter` is an ordinary action key scoped to row kinds.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActContext {
    Screen {
        screen: String,
        row_kinds: Vec<String>,
    },
    TreeSelection,
    Diff,
}

impl ActContext {
    pub fn screen(
        screen: impl Into<String>,
        row_kinds: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::Screen {
            screen: screen.into(),
            row_kinds: row_kinds.into_iter().map(Into::into).collect(),
        }
    }
}

/// A value returned by a host choice dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Choice {
    pub value: String,
    pub label: Tr,
}

impl Choice {
    pub fn new(value: impl Into<String>, label: impl Into<Tr>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// A confirmation key and its value (for example `y` = `-d`, `!` = `-D`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConfirmChoice {
    pub key: char,
    pub choice: Choice,
}

impl ConfirmChoice {
    pub fn new(key: char, value: impl Into<String>, label: impl Into<Tr>) -> Self {
        Self {
            key,
            choice: Choice::new(value, label),
        }
    }
}

/// A destructive action must declare a confirmation with at least one choice.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Confirmation {
    pub text: Tr,
    pub choices: Vec<ConfirmChoice>,
}

impl Confirmation {
    pub fn new(text: impl Into<Tr>, choices: Vec<ConfirmChoice>) -> Self {
        Self {
            text: text.into(),
            choices,
        }
    }
}

/// A text prompt preceding an action.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TextInput {
    pub label: Tr,
    pub initial: String,
}

impl TextInput {
    pub fn new(label: impl Into<Tr>, initial: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            initial: initial.into(),
        }
    }
}

/// A list prompt preceding an action.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChoiceInput {
    pub label: Tr,
    pub choices: Vec<Choice>,
}

impl ChoiceInput {
    pub fn new(label: impl Into<Tr>, choices: Vec<Choice>) -> Self {
        Self {
            label: label.into(),
            choices,
        }
    }
}
