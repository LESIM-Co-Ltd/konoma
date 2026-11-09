//! Pure per-context project screen state, independent of a host's surfaces and actions.

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_host_core::keymap::Motion;
use konoma_host_core::project::DiffLayout;

use super::{MatchedProject, ProjectContent, ScreenKey};
use crate::project::{ProjectAct, ProjectDecl, ProjectRow, ProjectScreen};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectKey {
    pub plugin: String,
    pub root: PathBuf,
    pub marker: String,
}

impl From<&MatchedProject> for ProjectKey {
    fn from(project: &MatchedProject) -> Self {
        Self {
            plugin: project.plugin.clone(),
            root: project.root.clone(),
            marker: project.marker.clone(),
        }
    }
}

#[derive(Clone)]
pub enum ProjectInput<I> {
    Key(KeyEvent),
    Act(Box<I>),
}

#[derive(Clone)]
pub struct ProjectFrame<I> {
    pub key: ScreenKey,
    /// Position in the filtered visible rows.
    pub cursor: usize,
    pub scroll: u16,
    pub hscroll: u16,
    /// Indices in the original list, including rows hidden by a filter.
    pub marked: BTreeSet<usize>,
    pub query: String,
    pub filtering: bool,
    filter_anchor: usize,
    initialized: bool,
    pub pending: VecDeque<ProjectInput<I>>,
}

impl<I> ProjectFrame<I> {
    pub fn start_filter(&mut self) {
        self.filtering = true;
        self.query.clear();
    }

    pub fn new(key: ScreenKey) -> Self {
        Self {
            key,
            cursor: 0,
            scroll: 0,
            hscroll: 0,
            marked: BTreeSet::new(),
            query: String::new(),
            filtering: false,
            filter_anchor: 0,
            initialized: false,
            pending: VecDeque::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectViewport {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectMotionResult {
    Unhandled,
    Handled,
    Back,
    LayoutChanged(DiffLayout),
}

#[derive(Clone)]
pub struct ProjectScreenState<I> {
    pub project: ProjectKey,
    pub stack: Vec<ProjectFrame<I>>,
}

impl<I> ProjectScreenState<I> {
    pub fn new(project: ProjectKey, screen: ScreenKey) -> Self {
        Self {
            project,
            stack: vec![ProjectFrame::new(screen)],
        }
    }

    pub fn frame(&self) -> Option<&ProjectFrame<I>> {
        self.stack.last()
    }

    pub fn frame_mut(&mut self) -> Option<&mut ProjectFrame<I>> {
        self.stack.last_mut()
    }

    fn screen<'a>(&self, declaration: Option<&'a ProjectDecl>) -> Option<&'a ProjectScreen> {
        let frame = self.frame()?;
        declaration?
            .screens
            .iter()
            .find(|screen| screen.id == frame.key.screen)
    }

    pub fn marks_enabled(&self, declaration: Option<&ProjectDecl>) -> bool {
        self.frame().is_some_and(|frame| {
            declaration.is_some_and(|declaration| {
                declaration
                    .screens
                    .iter()
                    .any(|screen| screen.id == frame.key.screen && screen.marks)
            })
        })
    }

    pub fn filter_enabled(&self, declaration: Option<&ProjectDecl>) -> bool {
        self.screen(declaration).is_none_or(|screen| screen.filter)
    }

    pub fn pushed(&self, declaration: Option<&ProjectDecl>) -> bool {
        self.frame().is_some_and(|frame| {
            declaration.is_some_and(|declaration| {
                declaration
                    .screens
                    .iter()
                    .any(|screen| screen.id == frame.key.screen && screen.pushed)
            })
        })
    }

    /// Legacy VCS picker behavior, shared until the old hub is removed.
    pub fn picker(&self) -> bool {
        self.frame()
            .is_some_and(|frame| frame.key.screen == "graph_picker")
            && matches!(self.project.plugin.as_str(), "git" | "jj")
    }

    /// Whether the graph belongs to jj; other screens and providers have no legacy graph.
    pub fn graph_jj(&self) -> Option<bool> {
        if self.frame()?.key.screen != "graph" {
            return None;
        }
        match self.project.plugin.as_str() {
            "git" => Some(false),
            "jj" => Some(true),
            _ => None,
        }
    }

    pub fn initialize_frame(&mut self, content: &ProjectContent, declaration: &ProjectDecl) {
        let marks = self.marks_enabled(Some(declaration));
        let Some(frame) = self.frame_mut().filter(|frame| !frame.initialized) else {
            return;
        };
        if let ProjectContent::List(list) = content {
            if let Some(cursor) = list.visible_rows(&frame.query).iter().position(|index| {
                let row = &list.rows[*index];
                row.initial_selection && row.selectable && !row.filter_fallback
            }) {
                frame.cursor = cursor;
            }
            frame.marked.extend(
                list.rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| marks && row.marked && row.selectable)
                    .map(|(index, _)| index),
            );
        }
        frame.initialized = true;
    }

    pub fn toggle_mark(&mut self, content: &ProjectContent, declaration: &ProjectDecl) {
        if !self.marks_enabled(Some(declaration)) || !matches!(content, ProjectContent::List(_)) {
            return;
        }
        if let Some(index) =
            self.selected_index(content, ProjectViewport::default(), DiffLayout::Unified)
        {
            let marks = &mut self.frame_mut().unwrap().marked;
            if !marks.remove(&index) {
                marks.insert(index);
            }
        }
    }

    /// The host resolves a Diff layout key before resolving Text motions.
    pub fn cycle_layout(
        &mut self,
        content: &ProjectContent,
        layout: DiffLayout,
    ) -> ProjectMotionResult {
        if !matches!(content, ProjectContent::Diff(_)) {
            return ProjectMotionResult::Unhandled;
        }
        let Some(frame) = self.frame_mut() else {
            return ProjectMotionResult::Unhandled;
        };
        frame.hscroll = 0;
        ProjectMotionResult::LayoutChanged(layout.next())
    }

    /// Apply a resolved Text motion. The physical key distinguishes list Page from mapped Page.
    pub fn apply_motion(
        &mut self,
        content: &ProjectContent,
        motion: Motion,
        physical_key: KeyCode,
        viewport: ProjectViewport,
        layout: DiffLayout,
        declaration: Option<&ProjectDecl>,
    ) -> ProjectMotionResult {
        if let ProjectContent::List(list) = content {
            if motion == Motion::Right {
                return ProjectMotionResult::Unhandled;
            }
            // The legacy branch panel consumes page motions without moving its cursor.
            if self.picker()
                && matches!(
                    motion,
                    Motion::HalfUp | Motion::HalfDown | Motion::PageUp | Motion::PageDown
                )
            {
                return ProjectMotionResult::Handled;
            }
            if motion == Motion::Left {
                return if self.graph_jj().is_some() || self.pushed(declaration) {
                    ProjectMotionResult::Back
                } else {
                    ProjectMotionResult::Unhandled
                };
            }
            let selectable: Vec<_> = self
                .visible_rows(content, viewport, layout)
                .iter()
                .enumerate()
                .filter(|(_, index)| list.rows[**index].selectable)
                .map(|(cursor, _)| cursor)
                .collect();
            let Some(frame) = self.frame_mut() else {
                return ProjectMotionResult::Unhandled;
            };
            let at = selectable
                .iter()
                .position(|cursor| *cursor == frame.cursor)
                .unwrap_or(0);
            let last = selectable.len().saturating_sub(1);
            let next = match motion {
                Motion::Up => at.saturating_sub(1),
                Motion::Down => at.saturating_add(1).min(last),
                Motion::HalfUp => at.saturating_sub(10),
                Motion::HalfDown => at.saturating_add(10).min(last),
                Motion::PageUp => at.saturating_sub(if physical_key == KeyCode::PageUp {
                    10
                } else {
                    20
                }),
                Motion::PageDown => at
                    .saturating_add(if physical_key == KeyCode::PageDown {
                        10
                    } else {
                        20
                    })
                    .min(last),
                Motion::Top => 0,
                Motion::Bottom => last,
                _ => at,
            };
            frame.cursor = selectable.get(next).copied().unwrap_or(0);
        } else {
            let (total, max_h) = match content {
                ProjectContent::Lines(lines) => (
                    lines.len(),
                    lines
                        .iter()
                        .map(ratatui::text::Line::width)
                        .max()
                        .unwrap_or(0)
                        .saturating_sub(viewport.cols as usize)
                        .min(u16::MAX as usize) as u16,
                ),
                ProjectContent::Diff(diff) => {
                    diff.metrics(viewport.cols, layout.is_split(viewport.cols))
                }
                ProjectContent::List(_) => unreachable!(),
            };
            let max_v = total
                .saturating_sub(viewport.rows as usize)
                .min(u16::MAX as usize) as u16;
            let Some(frame) = self.frame_mut() else {
                return ProjectMotionResult::Unhandled;
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
        }
        ProjectMotionResult::Handled
    }

    pub fn visible_rows(
        &self,
        content: &ProjectContent,
        viewport: ProjectViewport,
        layout: DiffLayout,
    ) -> Vec<usize> {
        let Some(frame) = self.frame() else {
            return Vec::new();
        };
        match content {
            ProjectContent::List(list) => list.visible_rows(&frame.query),
            ProjectContent::Lines(lines) => (0..lines.len()).collect(),
            ProjectContent::Diff(diff) => {
                (0..diff.row_count(layout.is_split(viewport.cols))).collect()
            }
        }
    }

    pub fn selected_index(
        &self,
        content: &ProjectContent,
        viewport: ProjectViewport,
        layout: DiffLayout,
    ) -> Option<usize> {
        let index = self
            .visible_rows(content, viewport, layout)
            .get(self.frame()?.cursor)
            .copied()?;
        if let ProjectContent::List(list) = content {
            let row = list.rows.get(index)?;
            if !row.selectable || row.filter_fallback {
                return None;
            }
        }
        Some(index)
    }

    /// The real target under the cursor, accounting for filters and non-selectable rows.
    pub fn selected_target(
        &self,
        content: &ProjectContent,
        viewport: ProjectViewport,
        layout: DiffLayout,
    ) -> Option<PathBuf> {
        self.row_target(content, self.selected_index(content, viewport, layout)?)
    }

    /// Resolve a selected original row index, as returned by `selected_index`.
    pub fn row_target(&self, content: &ProjectContent, index: usize) -> Option<PathBuf> {
        match content {
            ProjectContent::List(list) => {
                let row = list.rows.get(index)?;
                if !row.selectable || row.filter_fallback {
                    return None;
                }
                row.target.clone()
            }
            _ => None,
        }
    }

    pub fn act_context(&self, content: Option<&ProjectContent>) -> ProjectAct {
        let Some(frame) = self.frame() else {
            return ProjectAct::new();
        };
        let mut context = ProjectAct::new()
            .screen(&frame.key.screen)
            .tab(frame.key.tab);
        if let Some(content @ ProjectContent::List(list)) = content {
            if let Some(row) = self
                .selected_index(content, ProjectViewport::default(), DiffLayout::Unified)
                .and_then(|index| list.rows.get(index))
            {
                context = context.selected(ProjectRow::from(row));
            }
            context = context.marked(
                frame
                    .marked
                    .iter()
                    .filter_map(|index| list.rows.get(*index))
                    .map(ProjectRow::from),
            );
        }
        context
    }

    pub fn filter_anchor(&self, content: Option<&ProjectContent>) -> usize {
        self.frame()
            .map(|frame| {
                content
                    .and_then(|content| {
                        self.visible_rows(content, ProjectViewport::default(), DiffLayout::Unified)
                            .get(frame.cursor)
                            .copied()
                    })
                    .unwrap_or(frame.filter_anchor)
            })
            .unwrap_or(0)
    }

    pub fn filter_changed(&mut self, content: Option<&ProjectContent>, anchor: usize) {
        let visible = content
            .map(|content| {
                self.visible_rows(content, ProjectViewport::default(), DiffLayout::Unified)
            })
            .unwrap_or_default();
        let cursor = visible
            .iter()
            .position(|index| *index >= anchor)
            .unwrap_or(visible.len().saturating_sub(1));
        if let Some(frame) = self.frame_mut() {
            frame.cursor = cursor;
            frame.filter_anchor = visible.get(cursor).copied().unwrap_or(anchor);
        }
    }

    pub fn start_filter(&mut self, content: Option<&ProjectContent>) {
        let anchor = self.filter_anchor(content);
        if let Some(frame) = self.frame_mut() {
            frame.start_filter();
        }
        self.filter_changed(content, anchor);
    }

    pub fn clear_filter(&mut self, content: Option<&ProjectContent>) {
        let anchor = self.filter_anchor(content);
        if let Some(frame) = self.frame_mut() {
            frame.query.clear();
        }
        self.filter_changed(content, anchor);
    }

    pub fn handle_filter_key(&mut self, content: Option<&ProjectContent>, key: KeyEvent) -> bool {
        if !self.frame().is_some_and(|frame| frame.filtering) {
            return false;
        }
        let anchor = self.filter_anchor(content);
        let frame = self.frame_mut().unwrap();
        match key.code {
            KeyCode::Esc => {
                frame.query.clear();
                frame.filtering = false;
            }
            KeyCode::Enter => frame.filtering = false,
            KeyCode::Backspace => {
                frame.query.pop();
            }
            KeyCode::Char(c)
                if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() && !c.is_control() =>
            {
                frame.query.push(c);
            }
            _ => {}
        }
        self.filter_changed(content, anchor);
        true
    }

    /// Preserve whole ProjectRow identities across replacement, rather than clamping indices.
    pub fn remap_rows(
        &mut self,
        screen: &ScreenKey,
        old: Option<&ProjectContent>,
        new: &ProjectContent,
    ) {
        let (Some(ProjectContent::List(old)), ProjectContent::List(new)) = (old, new) else {
            return;
        };
        for frame in self.stack.iter_mut().filter(|frame| &frame.key == screen) {
            let selected = old
                .visible_rows(&frame.query)
                .get(frame.cursor)
                .and_then(|index| old.rows.get(*index))
                .map(ProjectRow::from);
            let marked: Vec<_> = frame
                .marked
                .iter()
                .filter_map(|index| old.rows.get(*index))
                .map(ProjectRow::from)
                .collect();
            if let Some(selected) = selected {
                if let Some(cursor) = new
                    .visible_rows(&frame.query)
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
