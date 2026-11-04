//! Project declarations. Detection and screen navigation are performed by the host.

use std::path::PathBuf;

use crate::text::Tr;

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
