//! Project navigation, action contexts and the shared prompts. Session ownership is on workers.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_plugin::project::{
    ActContext, ChoiceInput, Confirmation, ProjectAct, ProjectRow, RefreshTarget, TextInput,
};
use konoma_plugin::{Act, Outcome};
use ratatui::text::Line;

use super::project_worker::ProjectJob;
use super::*;
use crate::i18n::Msg;
use crate::keymap::Context;
use crate::plugins::{ActBinding, PluginAct};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProjectKey {
    pub plugin: String,
    pub root: PathBuf,
    pub marker: String,
}

impl From<&crate::plugins::projects::MatchedProject> for ProjectKey {
    fn from(p: &crate::plugins::projects::MatchedProject) -> Self {
        Self {
            plugin: p.plugin.clone(),
            root: p.root.clone(),
            marker: p.marker.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ScreenKey {
    pub screen: String,
    pub selection: Option<String>,
}

impl ScreenKey {
    fn new(screen: impl Into<String>) -> Self {
        Self {
            screen: screen.into(),
            selection: None,
        }
    }
}

pub(crate) enum ProjectContent {
    List(konoma_plugin::view::ListView),
    Lines(Vec<Line<'static>>),
    Diff(ProjectDiff),
}

/// Immutable diff data owns its layout cache, so a replacement view always invalidates it.
pub(crate) struct ProjectDiff {
    pub lines: Vec<crate::git::DiffLine>,
    pub header: Vec<Line<'static>>,
    pub theme: String,
    pub no_changes: String,
    split_rows: usize,
    unified_width: usize,
    cache: std::sync::Mutex<Option<Arc<ProjectDiffLayout>>>,
    #[cfg(test)]
    layouts: std::sync::atomic::AtomicUsize,
}

pub(crate) struct ProjectDiffLayout {
    pub lines: Vec<Line<'static>>,
    hscroll: u16,
    width: u16,
    split: bool,
    start: usize,
    count: usize,
}

impl ProjectDiff {
    pub fn new(
        lines: Vec<crate::git::DiffLine>,
        header: Vec<Line<'static>>,
        theme: String,
        no_changes: String,
    ) -> Self {
        // This constructor runs on the project worker. Keep totals separate from highlighted
        // rows so key handling and row queries never need to render the document.
        let split_rows = crate::preview::gitdiff::side_by_side_row_count(&lines, "");
        let unified_width = crate::preview::gitdiff::unified_max_hscroll(&lines, 0)
            .max(header.iter().map(Line::width).max().unwrap_or(0));
        Self {
            lines,
            header,
            theme,
            no_changes,
            split_rows,
            unified_width,
            cache: Default::default(),
            #[cfg(test)]
            layouts: Default::default(),
        }
    }

    fn header_rows(&self) -> usize {
        self.header.len() + usize::from(!self.header.is_empty())
    }

    pub fn row_count(&self, split: bool) -> usize {
        self.header_rows()
            + if self.lines.is_empty() {
                usize::from(!self.header.is_empty())
            } else if split {
                self.split_rows
            } else {
                self.lines.len()
            }
    }

    pub fn metrics(&self, width: u16, split: bool) -> (usize, u16) {
        let max_h = if self.lines.is_empty() {
            0
        } else if split {
            crate::preview::gitdiff::side_by_side_max_hscroll(&self.lines, usize::from(width))
        } else {
            self.unified_width
                .max(usize::from(!self.header.is_empty()))
                .saturating_sub(usize::from(width))
        };
        (self.row_count(split), max_h.min(u16::MAX as usize) as u16)
    }

    pub fn layout(
        &self,
        width: u16,
        split: bool,
        offset: u16,
        start: usize,
        count: usize,
    ) -> Arc<ProjectDiffLayout> {
        use crate::preview::gitdiff::{diff_lines_range, diff_lines_side_by_side_range};
        use ratatui::style::{Color, Style};
        // Stacked panning only moves the Paragraph; it does not change its highlighted lines.
        let offset = if split { offset } else { 0 };
        let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(cached) = cache.as_ref().filter(|c| {
            c.width == width
                && c.split == split
                && c.hscroll == offset
                && c.start == start
                && c.count == count
        }) {
            return cached.clone();
        }
        #[cfg(test)]
        self.layouts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let iw = usize::from(width);
        let end = start.saturating_add(count);
        let mut lines: Vec<_> = self
            .header
            .iter()
            .skip(start)
            .take(count)
            .cloned()
            .collect();
        if !self.header.is_empty() && start <= self.header.len() && self.header.len() < end {
            lines.push(Line::styled(
                "─".repeat(iw.max(1)),
                Style::new().fg(Color::DarkGray),
            ));
        }
        let header_rows = self.header_rows();
        let diff_start = start.saturating_sub(header_rows);
        let diff_count = end.saturating_sub(header_rows.max(start));
        if self.lines.is_empty() {
            if !self.header.is_empty() && start <= header_rows && header_rows < end {
                lines.push(Line::styled(
                    self.no_changes.clone(),
                    Style::new().fg(Color::DarkGray),
                ));
            }
        } else if diff_count > 0 {
            if split {
                lines.extend(diff_lines_side_by_side_range(
                    &self.lines,
                    "",
                    &self.theme,
                    iw,
                    usize::from(offset),
                    diff_start,
                    diff_count,
                ));
            } else {
                lines.extend(diff_lines_range(
                    &self.lines,
                    "",
                    &self.theme,
                    iw,
                    diff_start,
                    diff_count,
                ));
            }
        }
        let drawn = Arc::new(ProjectDiffLayout {
            lines,
            hscroll: offset,
            width,
            split,
            start,
            count,
        });
        *cache = Some(drawn.clone());
        drawn
    }

    #[cfg(test)]
    pub fn layout_count(&self) -> usize {
        self.layouts.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Clone)]
pub(crate) struct ProjectFrame {
    pub key: ScreenKey,
    pub cursor: usize,
    pub scroll: u16,
    pub hscroll: u16,
    pub marked: BTreeSet<usize>,
    pub query: String,
    pub filtering: bool,
    initialized: bool,
    pending: std::collections::VecDeque<ProjectInput>,
}

#[derive(Clone)]
enum ProjectInput {
    Key(KeyEvent),
    Act(Box<ProjectInvocation>),
}

impl ProjectFrame {
    fn new(key: ScreenKey) -> Self {
        Self {
            key,
            cursor: 0,
            scroll: 0,
            hscroll: 0,
            marked: BTreeSet::new(),
            query: String::new(),
            filtering: false,
            initialized: false,
            pending: Default::default(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProjectNavigation {
    pub project: ProjectKey,
    pub stack: Vec<ProjectFrame>,
    origin_mode: Mode,
    origin_path: Option<PathBuf>,
    origin_kind: Option<PreviewKind>,
    origin_scroll: u16,
    origin_hscroll: u16,
}

#[derive(Clone)]
pub(crate) struct ProjectEffect {
    pub project: ProjectKey,
    pub tab: u64,
    pub tree_root: PathBuf,
    pub view: Option<ScreenKey>,
    pub outcome: Option<Outcome>,
    pub tree_status: bool,
    pub label: String,
}

#[derive(Clone)]
pub(crate) struct ProjectInvocation {
    project: ProjectKey,
    action: PluginAct,
    declaration: Act,
    pub(super) context: ProjectAct,
    effect: ProjectEffect,
    origin_mode: Mode,
    origin_path: Option<PathBuf>,
}

impl ProjectInvocation {
    pub(crate) fn destructive(&self) -> bool {
        self.declaration.destructive
    }

    pub(crate) fn confirmation_text(&self, prompt: &Confirmation, lang: &str) -> String {
        let row = self.context.selected.as_ref();
        let name = row
            .map(|row| row.name.as_deref().unwrap_or(&row.value))
            .unwrap_or("");
        prompt.text.get(lang).replace("{row}", name)
    }
}

#[derive(Clone)]
pub(crate) enum ProjectPrompt {
    Confirmation(Confirmation),
    Text(TextInput),
    Choice(ChoiceInput),
}

pub(crate) enum ProjectDialog {
    Projects {
        projects: Vec<ProjectKey>,
        selected: usize,
    },
    Act {
        invocation: Box<ProjectInvocation>,
        prompts: Vec<ProjectPrompt>,
        stage: usize,
        buffer: String,
        cursor: usize,
        selected: usize,
    },
}

/// Matching excludes fallback messages until all ordinary rows have been filtered out.
fn visible_list_rows(list: &konoma_plugin::view::ListView, query: &str) -> Vec<usize> {
    let rows: Vec<_> = list
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| !row.filter_fallback && row.matches_filter(query))
        .map(|(index, _)| index)
        .collect();
    if rows.is_empty() {
        list.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.filter_fallback)
            .map(|(index, _)| index)
            .collect()
    } else {
        rows
    }
}

/// Refreshes keep the identity the user selected, even when the provider reorders its rows.
fn remap_project_rows(
    tab: &mut PerTab,
    project: &ProjectKey,
    screen: &ScreenKey,
    old: Option<&ProjectContent>,
    new: &ProjectContent,
) {
    let (Some(ProjectContent::List(old)), ProjectContent::List(new)) = (old, new) else {
        return;
    };
    for nav in [&mut tab.project_navigation, &mut tab.project_return]
        .into_iter()
        .flatten()
        .filter(|nav| &nav.project == project)
    {
        for frame in nav.stack.iter_mut().filter(|frame| &frame.key == screen) {
            let selected = visible_list_rows(old, &frame.query)
                .get(frame.cursor)
                .map(|index| ProjectRow::from(&old.rows[*index]));
            let marked: Vec<_> = frame
                .marked
                .iter()
                .filter_map(|index| old.rows.get(*index))
                .map(ProjectRow::from)
                .collect();
            if let Some(selected) = selected {
                if let Some(cursor) = visible_list_rows(new, &frame.query)
                    .iter()
                    .position(|index| ProjectRow::from(&new.rows[*index]) == selected)
                {
                    frame.cursor = cursor;
                } else {
                    frame.cursor = 0;
                    frame
                        .pending
                        .retain(|input| matches!(input, ProjectInput::Act(_)));
                }
            }
            frame.marked = new
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| marked.contains(&ProjectRow::from(*row)))
                .map(|(index, _)| index)
                .collect();
        }
    }
}

impl App {
    fn project_frame_mut(&mut self) -> Option<&mut ProjectFrame> {
        self.tab.project_navigation.as_mut()?.stack.last_mut()
    }

    fn project_list_right(&self, key: KeyEvent) -> bool {
        use crate::keymap::{Action, Binding, KeyPress, Motion, Resolution};
        if !self
            .project_content()
            .is_some_and(|v| matches!(v.as_ref(), ProjectContent::List(_)))
        {
            return false;
        }
        if key.code == KeyCode::Right {
            return true;
        }
        let kp = KeyPress::norm(&key);
        #[cfg(feature = "git")]
        let binding = self
            .keymaps
            .types
            .get(&crate::keymap::ViewType::List)
            .and_then(|t| t.get(&kp));
        #[cfg(not(feature = "git"))]
        let binding = None;
        match binding {
            Some(Binding::Run(Action::Navigate(Motion::Right))) => true,
            Some(_) => false,
            None => {
                self.keymaps.resolve(Context::Text, None, kp)
                    == Resolution::Action(Action::Navigate(Motion::Right))
            }
        }
    }

    fn project_enter_action(&self) -> Option<crate::keymap::Action> {
        self.plugin_action_for_key(
            Context::Canvas,
            crate::keymap::KeyPress::key(KeyCode::Enter),
        )
    }

    pub(crate) fn project_open_hint(&self) -> Option<(String, String)> {
        use crate::keymap::{Action, KeyPress, Motion};
        if !self
            .project_content()
            .is_some_and(|v| matches!(v.as_ref(), ProjectContent::List(_)))
        {
            return None;
        }
        let action = self.project_enter_action()?;
        if !crate::ops::action_enabled(self, Context::Canvas, action) {
            return None;
        }
        let Action::Plugin(act) = action else {
            return None;
        };
        let label = self
            .plugin_keys
            .declaration(act)?
            .label
            .get(self.plugin_lang())
            .to_string();
        let mut candidates = self
            .keymaps
            .keys_of(Context::Text, Action::Navigate(Motion::Right));
        #[cfg(feature = "git")]
        if let Some(table) = self.keymaps.types.get(&crate::keymap::ViewType::List) {
            candidates.extend(table.keys().copied());
        }
        candidates.push(KeyPress::key(KeyCode::Right));
        let mut keys = vec![crate::ops::keys::footer_key(
            KeyPress::key(KeyCode::Enter),
            true,
        )];
        for key in candidates {
            if self.project_list_right(KeyEvent::new(
                key.code,
                if key.ctrl {
                    KeyModifiers::CONTROL
                } else {
                    KeyModifiers::NONE
                },
            )) {
                let key = crate::ops::keys::footer_key(key, true);
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        Some((keys.join("/"), label))
    }

    fn handle_project_text_key(&mut self, key: KeyEvent) -> bool {
        use crate::keymap::{Action, KeyPress, Motion, Resolution};
        let Some(content) = self.project_content() else {
            return false;
        };
        let is_diff = matches!(content.as_ref(), ProjectContent::Diff(_));
        if matches!(content.as_ref(), ProjectContent::List(_)) {
            return false;
        }
        if is_diff
            && self
                .keymaps
                .resolve(Context::Diff, None, KeyPress::norm(&key))
                == Resolution::Action(Action::CycleDiffLayout)
        {
            let Some(frame) = self.project_frame_mut() else {
                return false;
            };
            frame.hscroll = 0;
            // Project diffs always show raw lines, independent of the origin preview's presentation.
            self.diff_layout = self.diff_layout.next();
            self.tab.preview_hscroll = 0;
            self.tab.git_detail_hscroll = 0;
            let label = match self.diff_layout {
                DiffLayout::Unified => Msg::DiffUnified,
                DiffLayout::Split => Msg::DiffSideBySide,
                DiffLayout::Auto => Msg::DiffAuto,
            };
            self.flash = Some(tr(self.lang, label).into());
            return true;
        }
        let action = crate::ops::keys::fixed_action(Context::Text, key.code).or_else(|| match self
            .keymaps
            .resolve(Context::Text, None, KeyPress::norm(&key))
        {
            Resolution::Action(action) => Some(action),
            _ => None,
        });
        let Some(Action::Navigate(motion)) = action else {
            return false;
        };
        let width = self.tab.preview_cols;
        let (total, max_h) = match content.as_ref() {
            ProjectContent::Lines(lines) => (
                lines.len(),
                lines
                    .iter()
                    .map(Line::width)
                    .max()
                    .unwrap_or(0)
                    .saturating_sub(width as usize)
                    .min(u16::MAX as usize) as u16,
            ),
            ProjectContent::Diff(diff) => diff.metrics(width, self.diff_is_split(width)),
            ProjectContent::List(_) => return false,
        };
        let max_v = total
            .saturating_sub(self.tab.preview_viewport as usize)
            .min(u16::MAX as usize) as u16;
        let Some(frame) = self.project_frame_mut() else {
            return false;
        };
        frame.scroll = frame.scroll.min(max_v);
        frame.hscroll = frame.hscroll.min(max_h);
        match motion {
            Motion::Up => frame.scroll = frame.scroll.saturating_sub(1),
            Motion::Down => frame.scroll = frame.scroll.saturating_add(1).min(max_v),
            Motion::HalfUp => frame.scroll = frame.scroll.saturating_sub(10),
            Motion::HalfDown => frame.scroll = frame.scroll.saturating_add(10).min(max_v),
            Motion::PageUp => frame.scroll = frame.scroll.saturating_sub(20),
            Motion::PageDown => frame.scroll = frame.scroll.saturating_add(20).min(max_v),
            Motion::Top => frame.scroll = 0,
            Motion::Bottom => frame.scroll = max_v,
            Motion::Left => frame.hscroll = frame.hscroll.saturating_sub(4),
            Motion::Right => frame.hscroll = frame.hscroll.saturating_add(4).min(max_h),
            Motion::LineHome => frame.hscroll = 0,
            Motion::LineEnd => frame.hscroll = max_h,
        }
        true
    }

    pub(crate) fn project_active(&self) -> bool {
        self.tab.project_navigation.is_some()
    }

    fn project_waiting_for_view(&self) -> bool {
        let Some(nav) = &self.tab.project_navigation else {
            return false;
        };
        let Some(frame) = nav.stack.last() else {
            return false;
        };
        self.project_runtimes
            .get(&nav.project)
            .is_some_and(|runtime| {
                !runtime.acting
                    && runtime.error.is_none()
                    && (!runtime.ready
                        || runtime.busy
                        || runtime.contexts.get(&frame.key)
                            != self.project_request(&frame.key).project.as_ref()
                        || !runtime.views.contains_key(&frame.key)
                        || runtime.dirty.contains(&frame.key))
            })
    }

    fn queue_project_input(&mut self, input: ProjectInput) {
        if !self.project_active() {
            if let ProjectInput::Act(invocation) = input {
                if self.tab.project_pending_acts.len() < 4096 {
                    self.tab.project_pending_acts.push_back(*invocation);
                } else {
                    self.flash = Some(tr(self.lang, Msg::ProjectBusy).into());
                }
            }
            return;
        }
        if let Some(frame) = self
            .tab
            .project_navigation
            .as_mut()
            .and_then(|nav| nav.stack.last_mut())
        {
            if frame.pending.len() < 4096 {
                if self.project_replaying {
                    frame.pending.push_front(input);
                } else {
                    frame.pending.push_back(input);
                }
            } else {
                self.flash = Some(tr(self.lang, Msg::ProjectBusy).into());
            }
        }
    }

    pub(crate) fn project_input_replaying(&self) -> bool {
        self.project_replaying
    }

    pub(super) fn cancel_project_input(&mut self) {
        self.tab.project_pending_acts.clear();
        if let Some(nav) = &mut self.tab.project_navigation {
            for frame in &mut nav.stack {
                frame.pending.clear();
            }
        }
    }

    fn project_pending_interaction(&self) -> bool {
        let Some(nav) = &self.tab.project_navigation else {
            return false;
        };
        let Some(frame) = nav.stack.last() else {
            return false;
        };
        let Some(ProjectInput::Key(key)) = frame.pending.front() else {
            return false;
        };
        if key.code == KeyCode::Char(' ') {
            return true;
        }
        self.plugin_keys
            .acts_of(&nav.project.plugin)
            .iter()
            .any(|binding| {
                binding.key == Some(crate::keymap::KeyPress::norm(key))
                    && self
                        .plugin_keys
                        .declaration(binding.act)
                        .is_some_and(|decl| {
                            decl.confirmation.is_some()
                                || decl.text_input.is_some()
                                || decl.choice_input.is_some()
                        })
            })
    }

    /// Loading never consumes an operation key. Replay through the ordinary dispatcher so the
    /// current view, context and write guard are consulted after the reply, including dialogs.
    pub(super) fn replay_project_keys(&mut self) -> bool {
        if self.project_replaying {
            return false;
        }
        self.project_replaying = true;
        let mut changed = false;
        for _ in 0..64 {
            if self.project_waiting_for_view() {
                break;
            }
            let input = self
                .tab
                .project_navigation
                .as_mut()
                .and_then(|nav| nav.stack.last_mut())
                .and_then(|frame| frame.pending.pop_front());
            let input = input.or_else(|| {
                let invocation = self.tab.project_pending_acts.front()?;
                if self
                    .project_runtimes
                    .get(&invocation.project)
                    .is_some_and(|runtime| runtime.busy)
                {
                    return None;
                }
                self.tab
                    .project_pending_acts
                    .pop_front()
                    .map(|invocation| ProjectInput::Act(Box::new(invocation)))
            });
            let Some(input) = input else {
                break;
            };
            changed = true;
            match input {
                ProjectInput::Key(key) => {
                    let result = crate::handle_key(self, key);
                    crate::resolve_key_result(self, result);
                }
                ProjectInput::Act(invocation) => self.execute_project_act(*invocation),
            }
        }
        self.project_replaying = false;
        changed
    }

    pub(crate) fn project_frame(&self) -> Option<&ProjectFrame> {
        self.tab.project_navigation.as_ref()?.stack.last()
    }

    pub(crate) fn project_content(&self) -> Option<Arc<ProjectContent>> {
        let nav = self.tab.project_navigation.as_ref()?;
        let key = &nav.stack.last()?.key;
        let runtime = self.project_runtimes.get(&nav.project)?;
        if runtime.contexts.get(key) != self.project_request(key).project.as_ref() {
            return None;
        }
        runtime.views.get(key).cloned()
    }

    pub(crate) fn project_marks_enabled(&self) -> bool {
        self.tab
            .project_navigation
            .as_ref()
            .and_then(|nav| {
                let frame = nav.stack.last()?;
                Some(
                    self.project_runtimes
                        .get(&nav.project)?
                        .project
                        .declaration
                        .screens
                        .iter()
                        .any(|s| s.id == frame.key.screen && s.marks),
                )
            })
            .unwrap_or(false)
    }

    pub(crate) fn project_visible_rows(&self) -> Vec<usize> {
        let Some(frame) = self.project_frame() else {
            return Vec::new();
        };
        let Some(content) = self.project_content() else {
            return Vec::new();
        };
        match content.as_ref() {
            ProjectContent::List(list) => visible_list_rows(list, &frame.query),
            ProjectContent::Lines(lines) => (0..lines.len()).collect(),
            ProjectContent::Diff(diff) => {
                (0..diff.row_count(self.diff_is_split(self.tab.preview_cols))).collect()
            }
        }
    }

    fn project_selected_index(&self) -> Option<usize> {
        let index = self
            .project_visible_rows()
            .get(self.project_frame()?.cursor)
            .copied()?;
        if let Some(content) = self.project_content() {
            if let ProjectContent::List(list) = content.as_ref() {
                if !list.rows.get(index)?.selectable {
                    return None;
                }
            }
        }
        Some(index)
    }

    pub(crate) fn project_row_target(&self) -> Option<PathBuf> {
        let index = self.project_selected_index()?;
        match self.project_content()?.as_ref() {
            ProjectContent::List(list) => list.rows.get(index)?.target.clone(),
            _ => None,
        }
    }

    fn action_project(&self, p: PluginAct) -> Option<&ProjectKey> {
        if let Some(nav) = &self.tab.project_navigation {
            return (nav.project.plugin == p.plugin()).then_some(&nav.project);
        }
        if let Some(nav) = &self.tab.project_return {
            if nav.project.plugin == p.plugin() {
                return Some(&nav.project);
            }
        }
        self.tab
            .projects
            .projects
            .iter()
            .filter(|project| project.plugin == p.plugin())
            .find_map(|project| {
                self.project_runtimes
                    .get_key_value(&ProjectKey::from(project))
                    .map(|(key, _)| key)
            })
    }

    pub(crate) fn project_act_context(&self) -> ProjectAct {
        if let Some(frame) = self.project_frame() {
            let mut context = ProjectAct::new().screen(&frame.key.screen);
            if let Some(content) = self.project_content() {
                if let ProjectContent::List(list) = content.as_ref() {
                    if let Some(row) = self.project_selected_index().and_then(|i| list.rows.get(i))
                    {
                        context = context.selected(ProjectRow::from(row));
                    }
                    context = context.marked(
                        frame
                            .marked
                            .iter()
                            .filter_map(|i| list.rows.get(*i))
                            .map(ProjectRow::from),
                    );
                }
            }
            return context;
        }
        if self.is_git_diff_preview() {
            return self
                .tab
                .preview_path
                .as_ref()
                .map(|p| ProjectAct::new().selected(ProjectRow::new("file", p.to_string_lossy())))
                .unwrap_or_default();
        }
        let row = |path: &PathBuf| {
            let kind = if self.tab.entries.iter().any(|e| &e.path == path && e.is_dir) {
                "directory"
            } else {
                "file"
            };
            ProjectRow::new(kind, path.to_string_lossy())
        };
        let mut context = ProjectAct::new().marked(self.tab.selection.iter().map(row));
        if let Some(entry) = self.tab.entries.get(self.tab.selected) {
            context = context.selected(row(&entry.path));
        }
        context
    }

    pub(crate) fn project_acts_in(&self, ctx: Context) -> Vec<&ActBinding> {
        #[cfg(feature = "git")]
        let host_diff = ctx == Context::Diff;
        #[cfg(not(feature = "git"))]
        let host_diff = false;
        let context_matches = if self.project_active() {
            ctx == Context::Canvas
        } else {
            matches!(ctx, Context::Tree | Context::TreeVisual) || host_diff
        };
        if !context_matches {
            return Vec::new();
        }
        let context = self.project_act_context();
        let ids: BTreeSet<_> = if let Some(nav) = &self.tab.project_navigation {
            [nav.project.plugin.as_str()].into_iter().collect()
        } else if matches!(ctx, Context::Tree | Context::TreeVisual) || self.is_git_diff_preview() {
            self.tab
                .projects
                .projects
                .iter()
                .map(|p| p.plugin.as_str())
                .collect()
        } else {
            return Vec::new();
        };
        ids.into_iter()
            .flat_map(|id| self.plugin_keys.acts_of(id))
            .filter(|binding| {
                let Some(decl) = self.plugin_keys.declaration(binding.act) else {
                    return false;
                };
                match &decl.project_context {
                    Some(ActContext::Screen { screen, row_kinds }) => {
                        self.project_active()
                            && context.screen.as_ref() == Some(screen)
                            && (row_kinds.is_empty()
                                || context
                                    .selected
                                    .as_ref()
                                    .is_some_and(|row| row_kinds.contains(&row.kind)))
                    }
                    Some(ActContext::TreeSelection) => {
                        !self.project_active()
                            && matches!(ctx, Context::Tree | Context::TreeVisual)
                            && context.selected.is_some()
                    }
                    Some(ActContext::Diff) => {
                        if self.project_active() {
                            self.project_content().is_some_and(|view| {
                                matches!(view.as_ref(), ProjectContent::Diff(_))
                            })
                        } else {
                            self.is_git_diff_preview()
                        }
                    }
                    _ => false,
                }
            })
            .collect()
    }

    pub(crate) fn project_act_enabled(&self, p: PluginAct) -> Option<bool> {
        self.plugin_keys.declaration(p)?.project_context.as_ref()?;
        let key = self.action_project(p)?;
        Some(self.project_runtimes.get(key).is_some_and(|r| {
            r.ready && !r.acting && r.enabled.get(p.act()).copied().unwrap_or(false)
        }))
    }

    /// `p` already belongs to OpenGitView. The legacy hub remains the fallback.
    pub(crate) fn enter_project(&mut self) -> bool {
        self.ensure_project_detection();
        if self.tab.projects.pending() {
            self.tab.project_enter_pending = true;
            return true;
        }
        let projects: Vec<_> = self
            .tab
            .projects
            .projects
            .iter()
            .filter(|p| !p.declaration.screens.is_empty())
            .map(ProjectKey::from)
            .collect();
        match projects.as_slice() {
            [] => false,
            [project] => {
                self.open_project(project.clone());
                true
            }
            _ => {
                self.project_dialog = Some(ProjectDialog::Projects {
                    projects,
                    selected: 0,
                });
                true
            }
        }
    }

    pub(super) fn open_project(&mut self, key: ProjectKey) {
        if let Some(project) = self
            .tab
            .projects
            .projects
            .iter()
            .find(|p| ProjectKey::from(*p) == key)
            .cloned()
        {
            self.start_project_runtime(project);
        }
        let Some(runtime) = self.project_runtimes.get(&key) else {
            return;
        };
        let Some(screen) = runtime
            .project
            .declaration
            .screens
            .iter()
            .find(|s| !s.pushed)
        else {
            return;
        };
        let screen = ScreenKey::new(&screen.id);
        self.show_project_screen(key, screen);
    }

    fn show_project_screen(&mut self, key: ProjectKey, screen: ScreenKey) {
        self.tab.project_navigation = Some(ProjectNavigation {
            project: key,
            stack: vec![ProjectFrame::new(screen)],
            origin_mode: self.tab.mode,
            origin_path: self.tab.preview_path.clone(),
            origin_kind: self.tab.preview_kind.clone(),
            origin_scroll: self.tab.preview_scroll,
            origin_hscroll: self.tab.preview_hscroll,
        });
        self.ensure_project_view();
    }

    fn ensure_project_view(&mut self) {
        self.initialize_project_frame();
        let Some(nav) = &self.tab.project_navigation else {
            return;
        };
        let Some(frame) = nav.stack.last() else {
            return;
        };
        let req = self.project_request(&frame.key);
        let Some(runtime) = self.project_runtimes.get_mut(&nav.project) else {
            return;
        };
        if runtime.busy
            || !runtime.ready
            || runtime.error.is_some()
            || (runtime.views.contains_key(&frame.key)
                && !runtime.dirty.contains(&frame.key)
                && runtime.contexts.get(&frame.key) == req.project.as_ref())
        {
            return;
        }
        if runtime
            .tx
            .try_send(ProjectJob::View {
                key: frame.key.clone(),
                req,
            })
            .is_ok()
        {
            runtime.busy = true;
            #[cfg(test)]
            {
                runtime.view_request = Some(frame.key.clone());
            }
        }
    }

    fn project_back(&mut self) {
        let Some(mut nav) = self.tab.project_navigation.take() else {
            return;
        };
        if nav.stack.len() > 1 {
            nav.stack.pop();
            self.tab.project_navigation = Some(nav);
            self.ensure_project_view();
            return;
        }
        match (nav.origin_mode, nav.origin_path) {
            (Mode::Preview, Some(path)) => {
                if matches!(nav.origin_kind, Some(PreviewKind::GitDiff(_))) {
                    self.open_git_diff(&path);
                } else {
                    self.enter_preview(&path);
                }
                self.tab.preview_scroll = nav.origin_scroll;
                self.tab.preview_hscroll = nav.origin_hscroll;
            }
            _ => self.back_to_tree(),
        }
    }

    pub(crate) fn handle_project_key(&mut self, key: KeyEvent) -> bool {
        if self.project_dialog.is_some() {
            self.project_dialog_key(key);
            return true;
        }
        if !self.project_active()
            || self.show_help
            || self.dialog.is_some()
            || self.is_tab_list()
            || self.is_bookmark_list()
            || self.is_info()
        {
            return false;
        }
        let interaction = self.project_pending_interaction();
        let navigation = !interaction
            && (self.pending_leader.is_none()
                && matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Tab | KeyCode::BackTab
                )
                || self.pending_leader.is_none()
                    && self.tab.project_navigation.as_ref().is_some_and(|nav| {
                        self.project_runtimes
                            .get(&nav.project)
                            .is_some_and(|runtime| {
                                runtime.project.declaration.screens.iter().any(|screen| {
                                    screen.key.is_some_and(|c| key.code == KeyCode::Char(c))
                                })
                            })
                    }));
        let global = !interaction
            && (key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                || self
                    .keymaps
                    .layer_of_key(Context::Canvas, crate::keymap::KeyPress::norm(&key))
                    == Some(crate::keymap::KeyLayer::Global));
        let local_movement = !interaction
            && self.pending_leader.is_none()
            && self.project_content().is_some()
            && matches!(
                key.code,
                KeyCode::Char('j' | 'k' | 'g' | 'G' | 'V' | '/')
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::PageUp
                    | KeyCode::PageDown
            );
        if !navigation
            && !global
            && !local_movement
            && (self.project_waiting_for_view()
                || !self.project_replaying
                    && self
                        .project_frame()
                        .is_some_and(|frame| !frame.pending.is_empty()))
        {
            if let Some(crate::keymap::Action::Plugin(action)) =
                self.plugin_action_for_key(Context::Canvas, crate::keymap::KeyPress::norm(&key))
            {
                let writes = self
                    .plugin_keys
                    .declaration(action)
                    .is_some_and(|decl| decl.writes);
                if !self.allow_write(action.plugin(), writes, false) {
                    return true;
                }
            }
            self.queue_project_input(ProjectInput::Key(key));
            return true;
        }
        if global {
            if let Some(frame) = self
                .tab
                .project_navigation
                .as_mut()
                .and_then(|nav| nav.stack.last_mut())
            {
                frame.pending.clear();
            }
        }
        if self.pending_leader.is_some() {
            return false;
        }
        if self.project_frame().is_some_and(|f| f.filtering) {
            let frame = self
                .tab
                .project_navigation
                .as_mut()
                .unwrap()
                .stack
                .last_mut()
                .unwrap();
            match key.code {
                KeyCode::Esc => {
                    frame.query.clear();
                    frame.filtering = false;
                }
                KeyCode::Enter => frame.filtering = false,
                KeyCode::Backspace => {
                    frame.query.pop();
                    frame.cursor = 0;
                }
                KeyCode::Char(c) => {
                    frame.query.push(c);
                    frame.cursor = 0;
                }
                _ => {}
            }
            return true;
        }
        if self.handle_project_text_key(key) {
            return true;
        }
        if self.project_list_right(key) {
            if let Some(action) = self.project_enter_action() {
                let result = crate::dispatch_action(self, action, self.surface());
                crate::resolve_key_result(self, result);
            }
            return true;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        match key.code {
            KeyCode::Char('r') => {
                let nav = self.tab.project_navigation.as_ref().unwrap();
                let key = nav.stack.last().unwrap().key.clone();
                if let Some(runtime) = self.project_runtimes.get_mut(&nav.project) {
                    runtime.dirty.insert(key);
                    runtime.invalidated |= runtime.busy;
                    runtime.error = None;
                }
                self.ensure_project_view();
                return true;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.project_back();
                return true;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let Some(nav) = self.tab.project_navigation.as_mut() else {
                    return false;
                };
                let Some(runtime) = self.project_runtimes.get(&nav.project) else {
                    return false;
                };
                let screens = &runtime.project.declaration.screens;
                let Some(frame) = nav.stack.last_mut() else {
                    return false;
                };
                if screens.iter().any(|s| s.id == frame.key.screen && s.pushed) {
                    return true;
                }
                let screens: Vec<_> = screens.iter().filter(|s| !s.pushed).collect();
                if screens.is_empty() {
                    return true;
                }
                let at = screens
                    .iter()
                    .position(|s| s.id == frame.key.screen)
                    .unwrap_or(0);
                let next = if key.code == KeyCode::BackTab {
                    (at + screens.len() - 1) % screens.len()
                } else {
                    (at + 1) % screens.len()
                };
                *frame = ProjectFrame::new(ScreenKey::new(&screens[next].id));
                self.ensure_project_view();
                return true;
            }
            KeyCode::Char('/')
                if self
                    .project_content()
                    .is_some_and(|view| matches!(view.as_ref(), ProjectContent::List(_))) =>
            {
                self.tab
                    .project_navigation
                    .as_mut()
                    .unwrap()
                    .stack
                    .last_mut()
                    .unwrap()
                    .filtering = true;
                return true;
            }
            KeyCode::Char('V')
                if self
                    .project_content()
                    .is_some_and(|view| matches!(view.as_ref(), ProjectContent::List(_))) =>
            {
                if !self.project_marks_enabled() {
                    return true;
                }
                if let Some(index) = self.project_selected_index() {
                    let marks = &mut self
                        .tab
                        .project_navigation
                        .as_mut()
                        .unwrap()
                        .stack
                        .last_mut()
                        .unwrap()
                        .marked;
                    if !marks.remove(&index) {
                        marks.insert(index);
                    }
                }
                return true;
            }
            KeyCode::Char('j')
            | KeyCode::Down
            | KeyCode::Char('k')
            | KeyCode::Up
            | KeyCode::Char('g')
            | KeyCode::Home
            | KeyCode::Char('G')
            | KeyCode::End
            | KeyCode::PageDown
            | KeyCode::PageUp
                if self
                    .project_content()
                    .is_some_and(|v| matches!(v.as_ref(), ProjectContent::List(_))) =>
            {
                let len = self.project_visible_rows().len();
                let frame = self
                    .tab
                    .project_navigation
                    .as_mut()
                    .unwrap()
                    .stack
                    .last_mut()
                    .unwrap();
                frame.cursor = match key.code {
                    KeyCode::Char('g') | KeyCode::Home => 0,
                    KeyCode::Char('G') | KeyCode::End => len.saturating_sub(1),
                    KeyCode::Char('k') | KeyCode::Up => frame.cursor.saturating_sub(1),
                    KeyCode::PageUp => frame.cursor.saturating_sub(10),
                    KeyCode::PageDown => frame.cursor.saturating_add(10).min(len.saturating_sub(1)),
                    _ => frame.cursor.saturating_add(1).min(len.saturating_sub(1)),
                };
                return true;
            }
            KeyCode::Char(c) => {
                let nav = self.tab.project_navigation.as_mut().unwrap();
                if let Some(screen) = self.project_runtimes[&nav.project]
                    .project
                    .declaration
                    .screens
                    .iter()
                    .find(|s| s.key == Some(c))
                {
                    *nav.stack.last_mut().unwrap() = ProjectFrame::new(ScreenKey::new(&screen.id));
                    self.ensure_project_view();
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn project_confirmation_enabled(&self, plugin: &str, prompt: &Confirmation) -> bool {
        let Some(key) = &prompt.setting else {
            return true;
        };
        let settings = crate::plugins::effective_settings(&self.cfg, plugin);
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let value = settings.get(key).or_else(|| {
            registry
                .manifest(plugin)?
                .settings
                .iter()
                .find(|setting| &setting.name == key)
                .map(|setting| &setting.default)
        });
        !matches!(value, Some(konoma_plugin::Value::Bool(false)))
    }

    pub(crate) fn request_project_act(&mut self, action: PluginAct) -> bool {
        let Some(decl) = self.plugin_keys.declaration(action).cloned() else {
            return false;
        };
        if decl.project_context.is_none() {
            return false;
        }
        let Some(project) = self.action_project(action).cloned() else {
            return true;
        };
        if !self.allow_write(action.plugin(), decl.writes, false) {
            return true;
        }
        if !self.project_act_enabled(action).unwrap_or(false) {
            return true;
        }
        let mut context = self.project_act_context();
        if decl.copy && context.selected.is_none() {
            if let Some(selection) = self
                .project_frame()
                .and_then(|frame| frame.key.selection.as_ref())
            {
                context = context.selected(ProjectRow::new("selection", selection));
            }
        }
        let effect = ProjectEffect {
            project: project.clone(),
            tab: self.tab.id,
            tree_root: self.tab.root.clone(),
            view: self.project_frame().map(|f| f.key.clone()),
            outcome: None,
            tree_status: false,
            label: decl.label.get(self.plugin_lang()).into(),
        };
        let mut prompts = Vec::new();
        if let Some(prompt) = decl
            .confirmation
            .as_ref()
            .filter(|prompt| self.project_confirmation_enabled(action.plugin(), prompt))
        {
            prompts.push(ProjectPrompt::Confirmation(prompt.clone()));
        }
        if let Some(prompt) = &decl.text_input {
            prompts.push(ProjectPrompt::Text(prompt.clone()));
        }
        if let Some(prompt) = &decl.choice_input {
            prompts.push(ProjectPrompt::Choice(prompt.clone()));
        }
        let invocation = ProjectInvocation {
            project,
            action,
            declaration: decl,
            context,
            effect,
            origin_mode: self.tab.mode,
            origin_path: self.tab.preview_path.clone(),
        };
        if prompts.is_empty() {
            self.execute_project_act(invocation);
        } else {
            let buffer = match &prompts[0] {
                ProjectPrompt::Text(input) => input.initial.clone(),
                _ => String::new(),
            };
            let cursor = buffer.chars().count();
            self.project_dialog = Some(ProjectDialog::Act {
                invocation: Box::new(invocation),
                prompts,
                stage: 0,
                buffer,
                cursor,
                selected: 0,
            });
        }
        true
    }

    pub(super) fn execute_project_act(&mut self, mut invocation: ProjectInvocation) {
        if invocation.effect.tree_root != self.tab.root
            || invocation.origin_mode != self.tab.mode
            || invocation.origin_path != self.tab.preview_path
            || invocation.effect.view.as_ref() != self.project_frame().map(|frame| &frame.key)
        {
            return;
        }
        if !self.allow_write(
            invocation.action.plugin(),
            invocation.declaration.writes,
            false,
        ) {
            return;
        }
        let Some(runtime) = self.project_runtimes.get(&invocation.project) else {
            return;
        };
        if runtime.busy && !runtime.acting {
            self.queue_project_input(ProjectInput::Act(Box::new(invocation)));
            return;
        }
        if runtime.busy || !runtime.ready {
            self.flash = Some(tr(self.lang, Msg::ProjectBusy).into());
            return;
        }
        if !runtime
            .enabled
            .get(invocation.action.act())
            .copied()
            .unwrap_or(false)
        {
            return;
        }
        if invocation.context.screen.is_some() && invocation.context.selected.as_ref().is_some_and(|selected| self.project_content().is_some_and(|view| matches!(view.as_ref(), ProjectContent::List(list) if !list.rows.iter().any(|row| &ProjectRow::from(row) == selected)))) {
            self.flash = Some(tr(self.lang, Msg::ProjectSelectionChanged).into());
            return;
        }
        if self
            .plugin_binding_in(
                self.key_context().unwrap_or_else(|| self.base_context()),
                invocation.action,
            )
            .is_none()
        {
            return;
        }
        let mut targets = BTreeSet::new();
        let mut chip = false;
        // Actions can change the gitdir; only a chip-only refresh skips watch discovery.
        let mut watch = invocation.declaration.refresh.is_empty();
        for target in &invocation.declaration.refresh {
            watch |= !matches!(target, RefreshTarget::Chip);
            match target {
                RefreshTarget::CurrentScreen => {
                    if let Some(key) = &invocation.effect.view {
                        targets.insert(key.clone());
                    }
                }
                RefreshTarget::Screen { screen } => {
                    let cached: Vec<_> = runtime
                        .views
                        .keys()
                        .filter(|key| &key.screen == screen)
                        .cloned()
                        .collect();
                    if cached.is_empty() {
                        targets.insert(ScreenKey::new(screen));
                    } else {
                        targets.extend(cached);
                    }
                }
                RefreshTarget::AllScreens => {
                    targets.extend(runtime.views.keys().cloned());
                    for screen in &runtime.project.declaration.screens {
                        if !targets.iter().any(|key| key.screen == screen.id) {
                            targets.insert(ScreenKey::new(&screen.id));
                        }
                    }
                }
                RefreshTarget::TreeStatus => invocation.effect.tree_status = true,
                RefreshTarget::Chip => chip = true,
                _ => {}
            }
        }
        let refresh = targets
            .into_iter()
            .map(|key| {
                let req = self.project_request(&key);
                (key, req)
            })
            .collect();
        let retry = invocation
            .declaration
            .text_input
            .as_ref()
            .filter(|input| input.keep_on_failure)
            .map(|_| Box::new(invocation.clone()));
        let runtime = self.project_runtimes.get_mut(&invocation.project).unwrap();
        let job = ProjectJob::Act {
            action: invocation.action.act().into(),
            context: invocation.context,
            refresh,
            chip,
            watch,
            effect: Box::new(invocation.effect),
        };
        if runtime.tx.try_send(job).is_ok() {
            runtime.busy = true;
            runtime.acting = true;
            runtime.pending_retry = retry;
            if let Some(frame) = self
                .tab
                .project_navigation
                .as_mut()
                .and_then(|nav| nav.stack.last_mut())
            {
                frame.pending.clear();
            }
        }
    }

    fn reopen_project_input(&mut self, invocation: ProjectInvocation) {
        if invocation.effect.tab != self.tab.id
            || invocation.effect.tree_root != self.tab.root
            || !self.tab.project_navigation.as_ref().is_some_and(|nav| {
                nav.project == invocation.project
                    && nav
                        .stack
                        .last()
                        .is_some_and(|frame| Some(&frame.key) == invocation.effect.view.as_ref())
            })
            || self.dialog.is_some()
            || self.project_dialog.is_some()
            || self.is_text_input_active()
            || self.project_frame().is_some_and(|frame| frame.filtering)
        {
            return;
        }
        let Some(prompt) = invocation.declaration.text_input.clone() else {
            return;
        };
        let Some(buffer) = invocation.context.text.clone() else {
            return;
        };
        let cursor = buffer.chars().count();
        self.project_dialog = Some(ProjectDialog::Act {
            invocation: Box::new(invocation),
            prompts: vec![ProjectPrompt::Text(prompt)],
            stage: 0,
            buffer,
            cursor,
            selected: 0,
        });
    }

    pub(crate) fn poll_project_jobs(&mut self) -> bool {
        let retained: std::collections::HashSet<_> = std::iter::once(&self.tab)
            .chain(self.tabs.iter())
            .flat_map(|tab| tab.projects.projects.iter().map(ProjectKey::from))
            .collect();
        self.project_runtimes
            .retain(|key, _| retained.contains(key));
        for project in self.tab.projects.projects.clone() {
            self.start_project_runtime(project);
        }
        let mut changed = false;
        let mut effects = Vec::new();
        let mut watches = Vec::new();
        let mut chip_errors = Vec::new();
        let mut errors = Vec::new();
        let mut retries = Vec::new();
        for (key, runtime) in &mut self.project_runtimes {
            while let Some(reply) = runtime
                .pending
                .take()
                .or_else(|| runtime.rx.try_recv().ok())
            {
                changed = true;
                runtime.busy = false;
                runtime.acting = false;
                if let Some(invocation) = runtime.pending_retry.take().filter(|_| reply.act_failed)
                {
                    retries.push(*invocation);
                }
                #[cfg(test)]
                {
                    runtime.view_request = None;
                }
                runtime.ready |= reply.error.is_none();
                runtime.error = reply.error;
                if let Some(paths) = reply.watch_paths {
                    runtime.watch_paths = Some(paths);
                }
                if !reply.enabled.is_empty() {
                    runtime.enabled = reply.enabled;
                }
                if !runtime.invalidated {
                    runtime.contexts.extend(reply.contexts);
                    for (screen, view) in reply.views {
                        let old = runtime.views.get(&screen).map(Arc::as_ref);
                        remap_project_rows(&mut self.tab, key, &screen, old, &view);
                        for tab in &mut self.tabs {
                            remap_project_rows(tab, key, &screen, old, &view);
                        }
                        runtime.dirty.remove(&screen);
                        runtime.views.insert(screen, view);
                    }
                }
                if let Some(chip) = reply.chip {
                    if !runtime.chip_dirty {
                        runtime.chip = chip;
                    }
                }
                if let Some(error) = reply.chip_error {
                    chip_errors.push((key.clone(), error));
                }
                if runtime.invalidated {
                    runtime.dirty.extend(runtime.views.keys().cloned());
                    runtime.invalidated = false;
                }
                if let Some(effect) = reply.effect {
                    effects.push(effect);
                }
                if let Some(error) = &runtime.error {
                    if self
                        .tab
                        .project_navigation
                        .as_ref()
                        .is_some_and(|nav| &nav.project == key)
                    {
                        errors.push(error.clone());
                    }
                }
            }
            if let Some(paths) = &runtime.watch_paths {
                watches.push((key.clone(), paths.clone()));
            }
            if runtime.refresh_watch
                && !runtime.busy
                && runtime.ready
                && runtime.tx.try_send(ProjectJob::Watch).is_ok()
            {
                runtime.refresh_watch = false;
                runtime.chip_dirty = false;
                runtime.busy = true;
            }
            if runtime.chip_dirty
                && runtime.ready
                && !runtime.busy
                && runtime.tx.try_send(ProjectJob::Chip).is_ok()
            {
                runtime.chip_dirty = false;
                runtime.busy = true;
            }
        }
        for (key, paths) in watches {
            for tab in std::iter::once(&mut self.tab).chain(self.tabs.iter_mut()) {
                tab.projects
                    .set_watch_paths(&key.plugin, &key.root, &key.marker, paths.clone());
            }
        }
        for effect in effects {
            if effect.tree_status && self.tab.root.starts_with(&effect.project.root) {
                self.git_status_dirty = true;
            }
            if effect.tab == self.tab.id {
                self.apply_project_effect(effect);
            } else if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == effect.tab) {
                tab.project_effects.push(effect);
            }
        }
        for effect in std::mem::take(&mut self.tab.project_effects) {
            self.apply_project_effect(effect);
            changed = true;
        }
        if self.tab.project_enter_pending && !self.tab.projects.pending() {
            self.tab.project_enter_pending = false;
            if !self.enter_project() {
                self.open_git_view();
            }
            changed = true;
        }
        self.ensure_project_view();
        for error in errors {
            self.flash = Some(error.message.get(self.plugin_lang()).into());
        }
        for (key, error) in chip_errors {
            if self
                .tab
                .projects
                .projects
                .iter()
                .any(|p| ProjectKey::from(p) == key)
            {
                self.flash = Some(error.message.get(self.plugin_lang()).into());
            }
        }
        for invocation in retries {
            self.reopen_project_input(invocation);
        }
        changed
    }

    fn initialize_project_frame(&mut self) {
        if let Some(nav) = self.tab.project_navigation.as_mut() {
            if let Some(frame) = nav.stack.last_mut() {
                if !frame.initialized {
                    if let Some(view) = self
                        .project_runtimes
                        .get(&nav.project)
                        .and_then(|r| r.views.get(&frame.key))
                    {
                        if let ProjectContent::List(list) = view.as_ref() {
                            frame.cursor = visible_list_rows(list, &frame.query)
                                .iter()
                                .position(|index| {
                                    list.rows[*index].selectable
                                        && list.rows[*index].initially_selected
                                })
                                .unwrap_or(0);
                            let marks =
                                self.project_runtimes
                                    .get(&nav.project)
                                    .is_some_and(|runtime| {
                                        runtime
                                            .project
                                            .declaration
                                            .screens
                                            .iter()
                                            .any(|s| s.id == frame.key.screen && s.marks)
                                    });
                            frame.marked.extend(
                                list.rows
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, row)| marks && row.marked && row.selectable)
                                    .map(|(i, _)| i),
                            );
                        }
                        frame.initialized = true;
                    }
                }
            }
        }
    }

    fn apply_project_effect(&mut self, effect: ProjectEffect) {
        if self.tab.root != effect.tree_root {
            return;
        }
        if effect.tree_status {
            self.git_status_dirty = true;
        }
        self.flash = Some(effect.label);
        if matches!(
            &effect.outcome,
            Some(
                Outcome::PushScreen { .. }
                    | Outcome::OpenFile { .. }
                    | Outcome::OpenDiff { .. }
                    | Outcome::SwitchRoot { .. }
                    | Outcome::OpenTab { .. }
            )
        ) && effect.view.is_some()
            && !self.tab.project_navigation.as_ref().is_some_and(|nav| {
                nav.project == effect.project
                    && nav.stack.last().map(|frame| &frame.key) == effect.view.as_ref()
            })
        {
            return;
        }
        match effect.outcome {
            Some(Outcome::Copy { text }) => self.set_clipboard_flash(&text),
            Some(Outcome::Flash(message)) => {
                self.flash = Some(message.get(self.plugin_lang()).into())
            }
            Some(Outcome::PushScreen { screen, selection }) => {
                if !self
                    .project_runtimes
                    .get(&effect.project)
                    .is_some_and(|runtime| {
                        runtime
                            .project
                            .declaration
                            .screens
                            .iter()
                            .any(|s| s.id == screen)
                    })
                {
                    self.flash = Some(tr(self.lang, Msg::ProjectUnknownScreen).into());
                    return;
                }
                let target = ScreenKey { screen, selection };
                if self.tab.project_navigation.is_none() && effect.view.is_none() {
                    self.show_project_screen(effect.project, target);
                    return;
                }
                let Some(nav) = self.tab.project_navigation.as_mut().filter(|nav| {
                    nav.project == effect.project
                        && nav.stack.last().map(|f| &f.key) == effect.view.as_ref()
                }) else {
                    return;
                };
                nav.stack.push(ProjectFrame::new(target));
            }
            Some(outcome @ (Outcome::OpenFile { .. } | Outcome::OpenDiff { .. })) => {
                self.tab.project_return = self.tab.project_navigation.take();
                let result = self.apply_plugin_navigation(outcome);
                crate::resolve_key_result(self, result.map(|()| false));
            }
            Some(outcome @ Outcome::SwitchRoot { .. }) => {
                self.tab.project_navigation = None;
                let result = self.apply_plugin_navigation(outcome);
                crate::resolve_key_result(self, result.map(|()| false));
            }
            Some(outcome @ Outcome::OpenTab { .. }) => {
                let result = self.apply_plugin_navigation(outcome);
                crate::resolve_key_result(self, result.map(|()| false));
            }
            _ => {}
        }
    }

    pub(crate) fn project_controls(&self) -> Option<String> {
        self.project_active().then(|| {
            let mut controls = if let Some(hints) = self.project_text_hints() {
                hints.into_iter().map(|(keys, label)| format!("{keys}:{label}")).collect::<Vec<_>>().join("  ")
            } else { tr(
                self.lang,
                if self
                    .project_content()
                    .is_some_and(|view| matches!(view.as_ref(), ProjectContent::List(_)))
                {
                    Msg::ProjectControls
                } else {
                    Msg::ProjectTextControls
                },
            )
            .to_string() };
            if self.project_content().is_some_and(|view| matches!(view.as_ref(), ProjectContent::List(_))) && !self.project_marks_enabled() {
                controls = konoma_plugin::t!(en: "Tab/Shift-Tab:screens  j/k:move  /:filter  q:back  ?:help", ja: "Tab/Shift-Tab:画面  j/k:移動  /:絞り込み  q:戻る  ?:ヘルプ").get(self.plugin_lang()).to_string();
            }
            if let Some((keys, label)) = self.project_open_hint() {
                controls.push_str(&format!("  {keys}:{label}"));
            }
            if crate::ops::menu::opens(self, Context::Canvas, crate::keymap::LeaderId::Copy) {
                let keys = self.keymaps.leader_keys_of(Context::Canvas).into_iter()
                    .filter(|(_, id)| *id == crate::keymap::LeaderId::Copy)
                    .map(|(key, _)| crate::ops::keys::footer_key(key, true))
                    .collect::<Vec<_>>().join("/");
                if !keys.is_empty() {
                    controls.push_str(&format!("  {keys}:{}", tr(self.lang, Msg::CopyHint)));
                }
            }
            if crate::ops::menu::opens(self, Context::Canvas, crate::keymap::LeaderId::Actions) {
                controls.push_str(&format!(
                    "  Space:{}",
                    tr(self.lang, Msg::WkActionsTitle).trim()
                ));
            }
            controls
        })
    }

    pub(crate) fn project_pushed(&self) -> bool {
        self.tab.project_navigation.as_ref().is_some_and(|nav| {
            self.project_runtimes
                .get(&nav.project)
                .is_some_and(|runtime| {
                    nav.stack.last().is_some_and(|f| {
                        runtime
                            .project
                            .declaration
                            .screens
                            .iter()
                            .any(|s| s.id == f.key.screen && s.pushed)
                    })
                })
        })
    }

    pub(crate) fn project_text_hints(&self) -> Option<Vec<(String, String)>> {
        use crate::keymap::{Action, Motion};
        let content = self.project_content()?;
        if matches!(content.as_ref(), ProjectContent::List(_)) {
            return None;
        }
        let groups = [
            (vec![Motion::Down, Motion::Up], Msg::Scroll),
            (vec![Motion::Left, Motion::Right], Msg::HintHscroll),
            (vec![Motion::Top, Motion::Bottom], Msg::HintEnds),
        ];
        let mut hints = Vec::new();
        for (motions, label) in groups {
            let keys = motions
                .into_iter()
                .flat_map(|m| self.keymaps.keys_of(Context::Text, Action::Navigate(m)))
                .map(|key| crate::ops::keys::footer_key(key, true))
                .collect::<Vec<_>>()
                .join("/");
            if !keys.is_empty() {
                hints.push((keys, tr(self.lang, label).into()));
            }
        }
        if matches!(content.as_ref(), ProjectContent::Diff(_)) {
            let keys = self
                .keymaps
                .keys_of(Context::Diff, Action::CycleDiffLayout)
                .into_iter()
                .map(|key| crate::ops::keys::footer_key(key, true))
                .collect::<Vec<_>>()
                .join("/");
            if !keys.is_empty() {
                hints.insert(
                    hints.len().min(2),
                    (keys, tr(self.lang, Msg::FtDiffLayout).into()),
                );
            }
        }
        if !self.project_pushed() {
            hints.push((
                "Tab/Shift-Tab".into(),
                tr(self.lang, Msg::ProjectCycle).into(),
            ));
        }
        hints.push(("q/Esc".into(), tr(self.lang, Msg::ProjectBack).into()));
        hints.push(("?".into(), tr(self.lang, Msg::HintHelp).into()));
        Some(hints)
    }

    pub(crate) fn project_text_help(&self) -> Option<Vec<(String, String)>> {
        use crate::keymap::{Action, Motion};
        let mut hints = self.project_text_hints()?;
        for (motions, label) in [
            ([Motion::HalfDown, Motion::HalfUp], Msg::HelpHalfPage),
            ([Motion::PageDown, Motion::PageUp], Msg::HintPage),
            ([Motion::LineHome, Motion::LineEnd], Msg::HintLineEnds),
        ] {
            let keys = motions
                .into_iter()
                .flat_map(|m| self.keymaps.keys_of(Context::Text, Action::Navigate(m)))
                .map(crate::ops::keys::help_key)
                .collect::<Vec<_>>()
                .join(" / ");
            if !keys.is_empty() {
                hints.push((keys, tr(self.lang, label).into()));
            }
        }
        Some(hints)
    }

    pub(crate) fn project_chips(&self) -> Vec<Line<'static>> {
        self.tab
            .projects
            .projects
            .iter()
            .filter_map(|project| {
                self.project_runtimes
                    .get(&ProjectKey::from(project))
                    .and_then(|runtime| runtime.chip.as_ref())
                    .map(|chip| konoma_plugin::ratatui_draw::into_line(chip.text.clone()))
                    .filter(|line| line.width() > 0)
            })
            .collect()
    }

    pub(crate) fn return_to_project(&mut self) -> bool {
        let Some(nav) = self.tab.project_return.take() else {
            return false;
        };
        self.tab.project_navigation = Some(nav);
        self.ensure_project_view();
        true
    }

    pub(crate) fn resize_project(&mut self, cols: u16, rows: u16) {
        if self.tab.preview_cols != cols || self.tab.preview_viewport != rows {
            self.tab.preview_cols = cols;
            self.tab.preview_viewport = rows;
            if let Some(nav) = &self.tab.project_navigation {
                if let Some(runtime) = self.project_runtimes.get_mut(&nav.project) {
                    runtime.dirty.extend(runtime.views.keys().cloned());
                    runtime.invalidated |= runtime.busy;
                }
            }
        }
        self.ensure_project_view();
    }

    #[cfg(test)]
    pub(crate) fn project_jobs_pending(&self) -> bool {
        self.project_runtimes.values().any(|r| r.busy)
    }

    #[cfg(test)]
    pub(crate) fn project_tree_dirty(&self) -> bool {
        self.git_status_dirty
    }

    #[cfg(test)]
    pub(crate) fn clear_project_tree_dirty(&mut self) {
        self.git_status_dirty = false;
    }

    #[cfg(test)]
    pub(crate) fn wait_project_view_request(&mut self, screen: &str) {
        for _ in 0..32 {
            let nav = self
                .tab
                .project_navigation
                .as_ref()
                .expect("project navigation");
            let runtime = self
                .project_runtimes
                .get(&nav.project)
                .expect("project runtime");
            if runtime
                .view_request
                .as_ref()
                .is_some_and(|key| key.screen == screen)
            {
                return;
            }
            assert!(
                runtime.busy && !runtime.acting,
                "the requested view must be scheduled"
            );
            self.wait_project_job();
        }
        panic!("project view was not requested");
    }

    #[cfg(test)]
    pub(crate) fn wait_project_view(&mut self) {
        for _ in 0..32 {
            if !self.project_waiting_for_view() {
                return;
            }
            self.wait_project_job();
        }
        panic!("project view did not complete");
    }

    #[cfg(test)]
    pub(crate) fn wait_project_job(&mut self) {
        // Tests wait for a concrete result, never a sleep or a timing assertion.
        if let Some(runtime) = self.project_runtimes.values_mut().find(|r| r.busy) {
            let reply = runtime
                .rx
                .recv_timeout(std::time::Duration::from_secs(120))
                .expect("project worker result");
            runtime.pending = Some(reply);
            self.poll_projects();
        }
    }
}
