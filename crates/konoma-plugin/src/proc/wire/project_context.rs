//! Protocol 2 project requests, action selections and refresh declarations.

use super::*;
use crate::project::{ProjectAct, ProjectRow, ProjectView, RefreshTarget};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WProjectView {
    pub screen: String,
    pub selection: Option<String>,
}
impl WProjectView {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WProjectRow {
    pub kind: String,
    pub value: String,
}
impl WProjectRow {
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WProjectAct {
    pub screen: Option<String>,
    pub selected: Option<WProjectRow>,
    pub marked: Vec<WProjectRow>,
    pub confirmation: Option<String>,
    pub text: Option<String>,
    pub choice: Option<String>,
}
impl WProjectAct {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn screen(mut self, screen: impl Into<String>) -> Self {
        self.screen = Some(screen.into());
        self
    }
    pub fn selected(mut self, row: WProjectRow) -> Self {
        self.selected = Some(row);
        self
    }
    pub fn marked(mut self, rows: impl IntoIterator<Item = WProjectRow>) -> Self {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WRefreshTarget {
    CurrentScreen,
    Screen { screen: String },
    AllScreens,
    TreeStatus,
    Chip,
}
impl WRefreshTarget {
    pub fn screen(screen: impl Into<String>) -> Self {
        Self::Screen {
            screen: screen.into(),
        }
    }
}

pub fn project_view_to_wire(p: &ProjectView) -> WProjectView {
    WProjectView {
        screen: p.screen.clone(),
        selection: p.selection.clone(),
    }
}
pub fn project_view_from_wire(p: WProjectView) -> ProjectView {
    ProjectView {
        screen: p.screen,
        selection: p.selection,
    }
}
pub fn project_row_to_wire(p: &ProjectRow) -> WProjectRow {
    WProjectRow {
        kind: p.kind.clone(),
        value: p.value.clone(),
    }
}
pub fn project_row_from_wire(p: WProjectRow) -> ProjectRow {
    ProjectRow {
        kind: p.kind,
        value: p.value,
    }
}
pub fn project_act_to_wire(p: &ProjectAct) -> WProjectAct {
    WProjectAct {
        screen: p.screen.clone(),
        selected: p.selected.as_ref().map(project_row_to_wire),
        marked: p.marked.iter().map(project_row_to_wire).collect(),
        confirmation: p.confirmation.clone(),
        text: p.text.clone(),
        choice: p.choice.clone(),
    }
}
pub fn project_act_from_wire(p: WProjectAct) -> ProjectAct {
    ProjectAct {
        screen: p.screen,
        selected: p.selected.map(project_row_from_wire),
        marked: p.marked.into_iter().map(project_row_from_wire).collect(),
        confirmation: p.confirmation,
        text: p.text,
        choice: p.choice,
    }
}
pub fn refresh_target_to_wire(t: &RefreshTarget) -> WRefreshTarget {
    match t {
        RefreshTarget::CurrentScreen => WRefreshTarget::CurrentScreen,
        RefreshTarget::Screen { screen } => WRefreshTarget::Screen {
            screen: screen.clone(),
        },
        RefreshTarget::AllScreens => WRefreshTarget::AllScreens,
        RefreshTarget::TreeStatus => WRefreshTarget::TreeStatus,
        RefreshTarget::Chip => WRefreshTarget::Chip,
    }
}
pub fn refresh_target_from_wire(t: WRefreshTarget) -> RefreshTarget {
    match t {
        WRefreshTarget::CurrentScreen => RefreshTarget::CurrentScreen,
        WRefreshTarget::Screen { screen } => RefreshTarget::Screen { screen },
        WRefreshTarget::AllScreens => RefreshTarget::AllScreens,
        WRefreshTarget::TreeStatus => RefreshTarget::TreeStatus,
        WRefreshTarget::Chip => RefreshTarget::Chip,
    }
}

pub fn act_cx_to_wire(cx: &crate::ActCx<'_>) -> Result<WCx, WireError> {
    let mut out = cx_to_wire(&CxSource {
        settings: cx.settings,
        lang: cx.lang,
        term: cx.term,
        external: cx.external,
        read_scope: &cx.read_scope,
    })?;
    out.project = cx.project.as_ref().map(project_act_to_wire);
    Ok(out)
}

pub fn req_protocol(req: &WViewReq) -> u32 {
    if req.project.is_some() {
        2
    } else {
        1
    }
}
pub fn cx_protocol(cx: &WCx) -> u32 {
    if cx.project.is_some() {
        2
    } else {
        1
    }
}
