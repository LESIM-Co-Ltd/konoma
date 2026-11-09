//! Collect project draw values, resize the worker viewport and adopt SDK corrections.

use konoma_plugin::host::project::draw::{draw_project, project_body_area, ProjectDrawValues};
use ratatui::{layout::Rect, Frame};

use crate::app::App;
use crate::i18n::{tr, Msg};

pub(crate) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let body = project_body_area(area);
    app.resize_project(body.width, body.height);
    let Some(nav) = &app.tab.project_navigation else {
        return;
    };
    let Some(runtime) = app.project_runtimes.get(&nav.project) else {
        return;
    };
    let Some(active) = nav.stack.last() else {
        return;
    };
    let content = app.project_content();
    let visible = if matches!(
        content.as_deref(),
        Some(konoma_plugin::host::project::ProjectContent::List(_))
    ) {
        app.project_visible_rows()
    } else {
        Vec::new()
    };
    let unavailable = runtime
        .error
        .as_ref()
        .map(|e| e.message.get(app.plugin_lang()))
        .unwrap_or(tr(app.lang, Msg::Loading));
    let update = draw_project(
        frame.buffer_mut(),
        area,
        ProjectDrawValues {
            declaration: &runtime.project.declaration,
            active_screen: &active.key.screen,
            cursor: active.cursor,
            scroll: active.scroll,
            hscroll: active.hscroll,
            marked: &active.marked,
            content: content.as_deref(),
            visible_rows: &visible,
            layout: if app.diff_is_split(body.width) {
                konoma_host_core::project::DiffLayout::Split
            } else {
                konoma_host_core::project::DiffLayout::Unified
            },
            lang: app.plugin_lang(),
            unavailable,
        },
    );
    if let Some(active) = app
        .tab
        .project_navigation
        .as_mut()
        .and_then(|nav| nav.stack.last_mut())
    {
        if let Some(cursor) = update.cursor {
            active.cursor = cursor;
        }
        if let Some(scroll) = update.scroll {
            active.scroll = scroll;
        }
        if let Some(hscroll) = update.hscroll {
            active.hscroll = hscroll;
        }
    }
}
