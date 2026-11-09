//! Project controls and structured footer tokens, derived from the host's resolved facts.

use crossterm::event::KeyCode;
use konoma_host_core::help::{footer_key, help_key};
use konoma_host_core::keymap::KeyPress;

use crate::host::acts::{self, ActBinding};
use konoma_host_core::help::CoreSection;
use konoma_host_core::menu::{self, Item};

#[path = "project_help.rs"]
mod project_help;
pub use project_help::{remaining_project_open_control, ProjectHelpWords};

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static BUILD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The current test thread's construction count, independent of other test frames.
#[cfg(any(test, feature = "test-hooks"))]
pub fn project_presentation_build_count() -> usize {
    BUILD_COUNT.with(std::cell::Cell::get)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectControlId {
    Screens,
    Move,
    Mark,
    Filter,
    Back,
    Help,
    Open,
    NewTab,
    Copy,
    Actions,
    Scroll,
    Hscroll,
    Ends,
    DiffLayout,
    Act,
}

#[derive(Clone, Debug)]
pub struct ProjectControl {
    pub id: ProjectControlId,
    pub keys: Vec<KeyPress>,
    pub label: String,
    shared_notation: bool,
}

impl ProjectControl {
    pub fn new(id: ProjectControlId, keys: Vec<KeyPress>, label: impl Into<String>) -> Self {
        Self {
            id,
            keys,
            label: label.into(),
            shared_notation: false,
        }
    }

    fn shared(mut self) -> Self {
        self.shared_notation = true;
        self
    }

    pub fn rendered_keys(&self) -> String {
        self.keys
            .iter()
            .map(|key| {
                if self.shared_notation {
                    if key.code == KeyCode::BackTab && !key.ctrl {
                        "Shift-Tab".into()
                    } else {
                        help_key(*key)
                    }
                } else {
                    footer_key(*key, true)
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    pub fn render(&self) -> String {
        format!("{}:{}", self.rendered_keys(), self.label)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectPresentationContent {
    List,
    Lines,
    Diff,
}

/// Enter's enabled action and the aliases the host has already verified as List Right.
#[derive(Clone, Debug)]
pub struct ProjectOpenFacts {
    pub label: String,
    pub right_aliases: Vec<KeyPress>,
}

/// Resolved keys, in host order. These are never looked up again by the presentation.
#[derive(Clone, Debug, Default)]
pub struct ProjectMotionKeys {
    pub scroll: Vec<KeyPress>,
    pub hscroll: Vec<KeyPress>,
    pub ends: Vec<KeyPress>,
    pub diff_layout: Vec<KeyPress>,
    pub half_page: Vec<KeyPress>,
    pub page: Vec<KeyPress>,
    pub line_ends: Vec<KeyPress>,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectPresentationFacts {
    /// None retains the loading screen's existing list-like controls.
    pub content: Option<ProjectPresentationContent>,
    pub marks: bool,
    pub filter: bool,
    pub pushed: bool,
    pub picker: bool,
    pub graph_jj: Option<bool>,
    pub row_target: bool,
    pub open: Option<ProjectOpenFacts>,
    pub motions: ProjectMotionKeys,
    /// The graph bridge uses the host's zipped, enabled movement keys.
    pub graph_scroll: Vec<KeyPress>,
    /// Empty when the host says that Copy does not open.
    pub copy_keys: Vec<KeyPress>,
    /// A screen's translated filter wording, used only in its footer.
    pub filter_label: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct ProjectCapabilities {
    pub can_open_tab: bool,
}

/// An index into the host's action table, never an owned project action identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationOp {
    ProjectAction(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentationToken {
    pub text: String,
    pub keys: Vec<KeyPress>,
    pub ops: Vec<PresentationOp>,
}

/// Converted bindings plus execution and declaration facts supplied by the host.
#[derive(Clone, Debug)]
pub struct ProjectPresentationBinding {
    pub binding: ActBinding<usize>,
    pub enabled: bool,
    /// The picker/graph bridge hides the originally declared Enter hint even after a remap.
    pub declared_enter: bool,
}

/// Already translated host words. Plugin labels retain their own translations in the bindings.
pub struct ProjectPresentationWords<'a> {
    pub language: &'a str,
    pub screens: &'a str,
    pub move_list: &'a str,
    pub scroll: &'a str,
    pub mark: &'a str,
    pub filter: &'a str,
    pub back: &'a str,
    pub help: &'a str,
    pub new_tab: &'a str,
    pub copy: &'a str,
    pub actions: &'a str,
    pub hscroll: &'a str,
    pub ends: &'a str,
    pub diff_layout: &'a str,
    pub cycle: &'a str,
    pub pop: &'a str,
    pub apply: &'a str,
    pub cancel: &'a str,
    pub nav: &'a str,
    pub search: &'a str,
    pub half_page: &'a str,
    pub page: &'a str,
    pub line_ends: &'a str,
}

/// One redraw's derived values. No project state or execution policy is owned here.
pub struct ProjectPresentation {
    facts: ProjectPresentationFacts,
    plugin_rows: Vec<(String, String)>,
    covered: Vec<KeyPress>,
    copy_menu: Vec<Item<usize, String>>,
    actions_menu: Vec<Item<usize, String>>,
    declared_actions_menu: Vec<Item<usize, String>>,
    controls: Vec<ProjectControl>,
    text_controls: Option<Vec<ProjectControl>>,
    text_help: Option<Vec<(String, String)>>,
    footer_tokens: Vec<PresentationToken>,
    footer_priority: Vec<PresentationOp>,
}

impl ProjectPresentation {
    pub fn build(
        facts: &ProjectPresentationFacts,
        capabilities: ProjectCapabilities,
        bindings: &[ProjectPresentationBinding],
        words: &ProjectPresentationWords<'_>,
    ) -> Self {
        #[cfg(any(test, feature = "test-hooks"))]
        BUILD_COUNT.with(|count| count.set(count.get() + 1));
        use ProjectControlId as Id;
        let text_controls = text_controls(facts, words);
        let text_help = text_controls.as_ref().map(|controls| {
            let mut hints: Vec<_> = controls
                .iter()
                .map(|c| (c.rendered_keys(), c.label.clone()))
                .collect();
            for (keys, label) in [
                (&facts.motions.half_page, words.half_page),
                (&facts.motions.page, words.page),
                (&facts.motions.line_ends, words.line_ends),
            ] {
                if !keys.is_empty() {
                    hints.push((
                        keys.iter()
                            .map(|k| help_key(*k))
                            .collect::<Vec<_>>()
                            .join(" / "),
                        label.into(),
                    ));
                }
            }
            hints
        });
        let mut controls = text_controls.clone().unwrap_or_else(|| {
            let list = facts.content == Some(ProjectPresentationContent::List);
            let mut controls = vec![
                ProjectControl::new(Id::Screens, tabs(), words.screens).shared(),
                ProjectControl::new(
                    Id::Move,
                    vec![KeyPress::ch('j'), KeyPress::ch('k')],
                    if list { words.move_list } else { words.scroll },
                )
                .shared(),
            ];
            if list && facts.marks {
                controls.push(
                    ProjectControl::new(Id::Mark, vec![KeyPress::ch('V')], words.mark).shared(),
                );
            }
            if list && facts.filter {
                controls.push(
                    ProjectControl::new(Id::Filter, vec![KeyPress::ch('/')], words.filter).shared(),
                );
            }
            controls
                .push(ProjectControl::new(Id::Back, vec![KeyPress::ch('q')], words.back).shared());
            controls
                .push(ProjectControl::new(Id::Help, vec![KeyPress::ch('?')], words.help).shared());
            controls
        });
        if let Some(open) = facts
            .open
            .as_ref()
            .filter(|_| facts.content == Some(ProjectPresentationContent::List))
        {
            let mut keys = vec![KeyPress::key(KeyCode::Enter)];
            for key in &open.right_aliases {
                if !keys.contains(key) {
                    keys.push(*key);
                }
            }
            controls.push(ProjectControl::new(Id::Open, keys, &open.label));
        }
        if facts.row_target && capabilities.can_open_tab {
            controls.push(
                ProjectControl::new(Id::NewTab, vec![KeyPress::ctrl_ch('t')], words.new_tab)
                    .shared(),
            );
        }
        if !facts.copy_keys.is_empty() {
            controls.push(ProjectControl::new(
                Id::Copy,
                facts.copy_keys.clone(),
                words.copy,
            ));
        }
        let acts: Vec<_> = bindings.iter().map(|b| &b.binding).collect();
        let enabled = |binding: &ActBinding<usize>| {
            bindings
                .iter()
                .any(|b| b.binding.act == binding.act && b.enabled)
        };
        if acts::menu_candidates(&acts).iter().any(|(index, _)| {
            bindings
                .iter()
                .any(|b| b.binding.act == *index && b.enabled)
        }) {
            controls.push(
                ProjectControl::new(Id::Actions, vec![KeyPress::ch(' ')], words.actions.trim())
                    .shared(),
            );
        }
        let mut hints = acts::footer_hints(&acts);
        let mut footer_priority = Vec::new();
        let footer_tokens = if facts.picker || facts.graph_jj.is_some() {
            // Preserve the legacy VCS wording and priority until the old hub is removed.
            let mut tokens = Vec::new();
            if !facts.picker {
                tokens.push(dynamic(format!(
                    "{}:{}",
                    write_keys(&facts.graph_scroll),
                    words.nav
                )));
                tokens.extend(
                    controls
                        .iter()
                        .find(|c| c.id == Id::Open)
                        .map(|c| dynamic(c.render())),
                );
            }
            for hint in hints {
                if bindings
                    .iter()
                    .any(|b| b.binding.act == hint.members[0].act && b.declared_enter)
                {
                    continue;
                }
                // The old bridge planned the first member even while its group was disabled.
                footer_priority.push(PresentationOp::ProjectAction(hint.members[0].act));
                let keys = hint.keys(enabled);
                if !keys.is_empty() {
                    tokens.push(PresentationToken {
                        text: format!("{}:{}", write_keys(&keys), hint.label(words.language)),
                        keys,
                        ops: hint
                            .members
                            .iter()
                            .map(|b| PresentationOp::ProjectAction(b.act))
                            .collect(),
                    });
                }
            }
            if facts.picker {
                if controls.iter().any(|c| c.id == Id::Open) {
                    tokens.push(dynamic(format!("l/Enter:{}", words.apply)));
                }
                tokens.push(dynamic(format!("h/q:{}", words.cancel)));
            } else {
                tokens.extend([
                    dynamic(format!("/:{}", words.search)),
                    dynamic(format!("Tab/⇧Tab:{}", words.screens)),
                    dynamic(format!("h/q/Esc:{}", words.back)),
                ]);
            }
            tokens
        } else {
            // Unlike the legacy bridge, ordinary project hints follow declaration order.
            hints.sort_by_key(|hint| acts.iter().position(|b| b.act == hint.members[0].act));
            let mut entries = Vec::new();
            for mut control in controls.iter().cloned() {
                if let Some(label) = facts
                    .filter_label
                    .as_ref()
                    .filter(|_| control.id == Id::Filter)
                {
                    control.label.clone_from(label);
                }
                project_entry(
                    &mut entries,
                    ProjectFooterEntry {
                        control,
                        ops: vec![],
                    },
                );
            }
            let shared = entries.len();
            for hint in hints {
                let keys = hint.keys(enabled);
                if keys.is_empty() {
                    continue;
                }
                project_entry(
                    &mut entries,
                    ProjectFooterEntry {
                        control: ProjectControl::new(Id::Act, keys, hint.label(words.language)),
                        ops: hint
                            .members
                            .iter()
                            .map(|b| PresentationOp::ProjectAction(b.act))
                            .collect(),
                    },
                );
            }
            let mut entries = entries.into_iter();
            let mut controls = dynamic(String::new());
            for entry in entries.by_ref().take(shared) {
                if !controls.text.is_empty() {
                    controls.text.push_str("  ");
                }
                controls.text.push_str(&entry.control.render());
                controls.keys.extend(entry.control.keys);
                controls.ops.extend(entry.ops);
            }
            std::iter::once(controls)
                .chain(entries.map(|entry| PresentationToken {
                    text: entry.control.render(),
                    keys: entry.control.keys,
                    ops: entry.ops,
                }))
                .collect()
        };
        if !facts.picker && facts.graph_jj.is_none() {
            footer_priority = footer_tokens
                .iter()
                .flat_map(|t| t.ops.iter().copied())
                .collect();
        }
        let rows = acts::help_rows(&acts, words.language, enabled);
        let covered = rows
            .iter()
            .flat_map(|(keys, _, _)| keys.iter().copied())
            .collect();
        let plugin_rows = rows
            .into_iter()
            .map(|(keys, description, _)| {
                let (columns, zipped) = konoma_host_core::help::zip_columns(&keys, |key| vec![key]);
                (
                    project_help::write_columns(&columns, zipped),
                    description.to_string(),
                )
            })
            .collect();
        let copy_menu = bindings
            .iter()
            .filter_map(|b| {
                Some(Item {
                    key: b.binding.copy_key.filter(|_| b.enabled)?,
                    action: b.binding.act,
                    label: b.binding.label.get(words.language).to_string(),
                })
            })
            .collect();
        let priority: Vec<_> = footer_priority
            .iter()
            .map(|op| match op {
                PresentationOp::ProjectAction(index) => *index,
            })
            .collect();
        let build_menu = |gated: bool| {
            menu::build_items(
                acts::menu_candidates(&acts),
                &priority,
                &[],
                &[],
                |_| None,
                |index| !gated || bindings.iter().any(|b| b.binding.act == index && b.enabled),
                |index| acts::menu_label(&acts, &index, words.language).map(str::to_string),
            )
        };
        let actions_menu = build_menu(true);
        let declared_actions_menu = build_menu(false);
        Self {
            facts: facts.clone(),
            plugin_rows,
            covered,
            copy_menu,
            actions_menu,
            declared_actions_menu,
            controls,
            text_controls,
            text_help,
            footer_tokens,
            footer_priority,
        }
    }

    pub fn controls(&self) -> &[ProjectControl] {
        &self.controls
    }
    pub fn copy_menu(&self) -> &[Item<usize, String>] {
        &self.copy_menu
    }
    pub fn copy_action(&self, key: KeyPress) -> Option<usize> {
        if crate::keys::parse_key(&key.label()).is_ok_and(crate::keys::closes_menu) {
            return None;
        }
        self.copy_menu
            .iter()
            .find(|item| item.key == key)
            .map(|item| item.action)
    }
    pub fn actions_menu(&self) -> &[Item<usize, String>] {
        &self.actions_menu
    }
    pub fn declared_actions_menu(&self) -> &[Item<usize, String>] {
        &self.declared_actions_menu
    }
    pub fn text_controls(&self) -> Option<&[ProjectControl]> {
        self.text_controls.as_deref()
    }
    pub fn text_help(&self) -> Option<&[(String, String)]> {
        self.text_help.as_deref()
    }
    /// The help-only motions appended by an existing host's text-hints wrapper.
    pub fn additional_text_help(&self) -> Option<&[(String, String)]> {
        Some(&self.text_help.as_ref()?[self.text_controls.as_ref()?.len()..])
    }
    pub fn open_control(&self) -> Option<&ProjectControl> {
        self.controls
            .iter()
            .find(|c| c.id == ProjectControlId::Open)
    }
    pub fn new_tab_control(&self) -> Option<&ProjectControl> {
        self.controls
            .iter()
            .find(|c| c.id == ProjectControlId::NewTab)
    }
    pub fn footer_tokens(&self) -> &[PresentationToken] {
        &self.footer_tokens
    }
    pub fn footer_priority(&self) -> &[PresentationOp] {
        &self.footer_priority
    }
}

fn tabs() -> Vec<KeyPress> {
    vec![KeyPress::key(KeyCode::Tab), KeyPress::key(KeyCode::BackTab)]
}

fn text_controls(
    facts: &ProjectPresentationFacts,
    words: &ProjectPresentationWords<'_>,
) -> Option<Vec<ProjectControl>> {
    use ProjectControlId as Id;
    if !matches!(
        facts.content,
        Some(ProjectPresentationContent::Lines | ProjectPresentationContent::Diff)
    ) {
        return None;
    }
    let mut hints = Vec::new();
    for (id, keys, label) in [
        (Id::Scroll, &facts.motions.scroll, words.scroll),
        (Id::Hscroll, &facts.motions.hscroll, words.hscroll),
        (Id::Ends, &facts.motions.ends, words.ends),
    ] {
        if !keys.is_empty() {
            hints.push(ProjectControl::new(id, keys.clone(), label));
        }
    }
    if facts.content == Some(ProjectPresentationContent::Diff)
        && !facts.motions.diff_layout.is_empty()
    {
        hints.insert(
            hints.len().min(2),
            ProjectControl::new(
                Id::DiffLayout,
                facts.motions.diff_layout.clone(),
                words.diff_layout,
            ),
        );
    }
    if !facts.pushed {
        hints.push(ProjectControl::new(Id::Screens, tabs(), words.cycle).shared());
    }
    hints.push(
        ProjectControl::new(
            Id::Back,
            vec![KeyPress::ch('q'), KeyPress::key(KeyCode::Esc)],
            words.pop,
        )
        .shared(),
    );
    hints.push(ProjectControl::new(Id::Help, vec![KeyPress::ch('?')], words.help).shared());
    Some(hints)
}

fn dynamic(text: String) -> PresentationToken {
    PresentationToken {
        text,
        keys: vec![],
        ops: vec![],
    }
}

fn write_keys(keys: &[KeyPress]) -> String {
    let mut parts = Vec::new();
    for key in keys {
        let text = footer_key(*key, true);
        if !parts.contains(&text) {
            parts.push(text);
        }
    }
    parts.join("/")
}

struct ProjectFooterEntry {
    control: ProjectControl,
    ops: Vec<PresentationOp>,
}

fn project_entry(entries: &mut Vec<ProjectFooterEntry>, mut entry: ProjectFooterEntry) {
    let mut kept = Vec::new();
    for key in entry.control.keys.iter().copied() {
        if let Some(index) = entries
            .iter()
            .position(|previous| previous.control.keys.contains(&key))
        {
            for op in &entry.ops {
                if !entries[index].ops.contains(op) {
                    entries[index].ops.push(*op);
                }
            }
        } else if !kept.contains(&key) {
            kept.push(key);
        }
    }
    if kept.is_empty() {
        return;
    }
    entry.control.keys = kept;
    entries.push(entry);
}
