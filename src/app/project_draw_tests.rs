//! Literal cell fixtures captured from 1131c68d before moving the host drawing.

use super::*;
use konoma_plugin::host::project::draw::{
    self, ProjectDialogLabel, ProjectDialogPresentation, ProjectDialogWords, ProjectDrawValues,
};
use konoma_plugin::host::project::{MatchedProject, ProjectRuntime};
use konoma_plugin::project::{
    Choice, ChoiceInput, ConfirmChoice, Confirmation, ProjectDecl, ProjectScreen, TextInput,
};
use konoma_plugin::view::{ListRow, ListStrip, ListView};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Stylize},
    text::Line,
    widgets::{Paragraph, Widget},
    Terminal,
};
use serde_json::{json, Value};

fn same_cells(a: &Buffer, b: &Buffer) {
    assert_eq!(a.area, b.area);
    let mut count = 0;
    for y in a.area.y..a.area.bottom() {
        for x in a.area.x..a.area.right() {
            let (a, b) = (&a[(x, y)], &b[(x, y)]);
            assert_eq!(
                (a.symbol(), a.fg, a.bg, a.modifier),
                (b.symbol(), b.fg, b.bg, b.modifier),
                "{x},{y}"
            );
            count += 1;
        }
    }
    assert_eq!(
        count,
        usize::from(a.area.width) * usize::from(a.area.height)
    );
    assert!(a.content.iter().any(|c| c.symbol() != " "));
}

/// A small host has no konoma surface or effects wrapper.
fn small_dialog(app: &App) -> ProjectDialogPresentation {
    use konoma_plugin::host::project::prompt::{ProjectDialog as D, ProjectInvocation as I};
    let dialog = match app.project_dialog.as_ref().unwrap() {
        D::Projects { projects, selected } => D::Projects {
            projects: projects.clone(),
            selected: *selected,
        },
        D::Act {
            invocation,
            prompts,
            stage,
            buffer,
            cursor,
            selected,
        } => {
            let shared = invocation.prompt_invocation();
            D::Act {
                invocation: Box::new(I {
                    action: shared.action.clone(),
                    declaration: shared.declaration.clone(),
                    context: shared.context.clone(),
                    origin: ProjectOrigin {
                        context: shared.origin.context,
                        project: shared.origin.project.clone(),
                        root: shared.origin.root.clone(),
                        screen: shared.origin.screen.clone(),
                        surface: (),
                    },
                }),
                prompts: prompts.clone(),
                stage: *stage,
                buffer: buffer.clone(),
                cursor: *cursor,
                selected: *selected,
            }
        }
    };
    let labels = match &dialog {
        D::Projects { projects, .. } => projects
            .iter()
            .map(|p| ProjectDialogLabel {
                name: p.plugin.clone(),
                path: app.format_path(&p.root),
            })
            .collect(),
        _ => vec![],
    };
    draw::present_project_dialog(
        &dialog,
        &labels,
        &ProjectDialogWords {
            lang: app.plugin_lang(),
            choose: tr(app.lang, Msg::ProjectChoose),
            confirm_title: tr(app.lang, Msg::DlgConfirmTitle),
            input_title: tr(app.lang, Msg::DlgInputTitle),
            keys: tr(app.lang, Msg::ProjectDialogKeys),
            confirm_cancel: tr(app.lang, Msg::ProjectConfirmCancel),
            confirm_cancel_status: tr(app.lang, Msg::ProjectConfirmCancelStatus),
        },
    )
}

fn app() -> (App, crate::test_support::TmpDir) {
    let dir = crate::test_support::unique_tmp("project_draw_values");
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = crate::config::Config::default();
    cfg.ui.lang = "en".into();
    cfg.external.git = false;
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.flash = None;
    let project = ProjectKey {
        plugin: "fixture".into(),
        root: dir.to_path_buf(),
        marker: "marker".into(),
    };
    let screen = ScreenKey::new("items", app.tab.id);
    app.tab.project_navigation = Some(ProjectNavigation {
        state: ProjectScreenState::new(project.clone(), screen.clone()),
        origin_mode: Mode::Tree,
        origin_path: None,
        origin_kind: None,
        origin_scroll: 0,
        origin_hscroll: 0,
    });
    let (tx, _jobs) = std::sync::mpsc::sync_channel(1);
    let (_results, rx) = std::sync::mpsc::sync_channel(1);
    app.project_runtimes.insert(
        project,
        ProjectRuntime {
            project: MatchedProject {
                plugin: "fixture".into(),
                root: dir.to_path_buf(),
                marker: "marker".into(),
                session_watch_paths: None,
                declaration: ProjectDecl::new()
                    .screen(
                        ProjectScreen::new("items", konoma_plugin::t!(en: "List", ja: "一覧"))
                            .key('1')
                            .marks(true)
                            .checks(true),
                    )
                    .screen(ProjectScreen::new("other", "Other"))
                    .screen(ProjectScreen::new("hidden", "Hidden").pushed()),
            },
            top: Default::default(),
            closed_tabs: Default::default(),
            watch_paths: None,
            refresh_watch: false,
            views: Default::default(),
            contexts: Default::default(),
            launch_targets: Default::default(),
            enabled: Default::default(),
            chip: None,
            chip_dirty: false,
            busy: true,
            acting: false,
            pending_retry: None,
            view_request: None,
            dirty: Default::default(),
            ready: true,
            error: None,
            tx,
            rx,
            pending: None,
            invalidated: false,
        },
    );
    (app, dir)
}

fn list(chips: bool) -> ProjectContent {
    use konoma_plugin::style::{Color as C, Line as L, Span, Style};
    ProjectContent::List(ListView {
        title: Some(L::from(vec![Span::styled(
            " title",
            Style::new().fg(C::Yellow),
        )])),
        rows: vec![
            ListRow::new(
                "item",
                vec![
                    L::from(vec![Span::styled("alpha日本", Style::new().fg(C::Green))]),
                    L::raw("extra"),
                ],
            )
            .checked(true)
            .right(L::from(vec![Span::styled(
                "META",
                Style::new().fg(C::Cyan),
            )])),
            ListRow::new("item", vec![L::raw("beta")]),
            ListRow::new("item", vec![L::raw("gamma")]).checked(false),
            ListRow::new("item", vec![L::raw("delta")]),
            ListRow::new("item", vec![L::raw("epsilon")]),
        ],
        bottom: chips.then(|| {
            ListStrip::new(
                vec![
                    L {
                        spans: vec![
                            Span::raw("chip"),
                            Span::styled("色", Style::new().fg(C::Red)),
                        ],
                        style: Style::new().fg(C::Yellow).bold(),
                    },
                    L::raw("second"),
                ],
                18,
            )
        }),
    })
}

fn diff(empty: bool) -> ProjectContent {
    use konoma_vcs_core::git::{DiffLine, DiffLineKind};
    ProjectContent::Diff(ProjectDiff::new(
        if empty {
            vec![]
        } else {
            (0..18)
                .map(|i| DiffLine {
                    kind: if i % 2 == 0 {
                        DiffLineKind::Removed
                    } else {
                        DiffLineKind::Added
                    },
                    old_no: (i % 2 == 0).then_some(i + 1),
                    new_no: (i % 2 != 0).then_some(i + 1),
                    text: format!("{i}: abcdefghijklmnopqrstuvwxyz日本"),
                })
                .collect()
        },
        vec![],
        "base16-ocean.dark".into(),
        "No changes".into(),
    ))
}

fn body_output(
    app: &mut App,
    content: Option<ProjectContent>,
    area: Rect,
    cursor: usize,
    scroll: u16,
    hscroll: u16,
) -> (Buffer, Value) {
    let nav = app.tab.project_navigation.as_ref().unwrap();
    let key = nav.project.clone();
    let screen = nav.stack.last().unwrap().key.clone();
    let context = app.project_request(&screen).unwrap().project.unwrap();
    let runtime = app.project_runtimes.get_mut(&key).unwrap();
    runtime.views.clear();
    if let Some(content) = content {
        runtime.views.insert(screen.clone(), Arc::new(content));
    }
    runtime.contexts.insert(screen, context);
    let active = app
        .tab
        .project_navigation
        .as_mut()
        .unwrap()
        .stack
        .last_mut()
        .unwrap();
    active.cursor = cursor;
    active.scroll = scroll;
    active.hscroll = hscroll;
    active.marked.insert(0);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let buffer = terminal
        .draw(|f| crate::ui::project::render(f, app, area))
        .unwrap()
        .buffer
        .clone();
    let mut small = Buffer::empty(buffer.area);
    let content = app.project_content();
    let visible = app.project_visible_rows();
    let nav = app.tab.project_navigation.as_ref().unwrap();
    let runtime = &app.project_runtimes[&nav.project];
    let update = draw::draw_project(
        &mut small,
        area,
        ProjectDrawValues {
            declaration: &runtime.project.declaration,
            active_screen: "items",
            cursor,
            scroll,
            hscroll,
            marked: &nav.stack.last().unwrap().marked,
            content: content.as_deref(),
            visible_rows: &visible,
            layout: app.diff_layout,
            lang: app.plugin_lang(),
            unavailable: tr(app.lang, Msg::Loading),
        },
    );
    same_cells(&buffer, &small);
    let active = app.project_frame().unwrap();
    assert_eq!(
        (active.cursor, active.scroll, active.hscroll),
        (
            update.cursor.unwrap_or(cursor),
            update.scroll.unwrap_or(scroll),
            update.hscroll.unwrap_or(hscroll)
        )
    );
    (
        buffer,
        json!([
            active.cursor,
            active.scroll,
            active.hscroll,
            app.tab.preview_cols,
            app.tab.preview_viewport
        ]),
    )
}

fn dialog(app: &App, prompt: ProjectPrompt, selected: usize, cursor: usize) -> ProjectDialog {
    let origin = ProjectOrigin {
        context: app.tab.id,
        project: app.tab.project_navigation.as_ref().unwrap().project.clone(),
        root: app.tab.root.clone(),
        screen: None,
        surface: ProjectSurface::from_tab(&app.tab),
    };
    let shared = SharedInvocation {
        action: ProjectActionId {
            plugin: "fixture".into(),
            action: "act".into(),
        },
        declaration: konoma_plugin::Act::new("act").destructive(),
        context: ProjectAct::new().selected(ProjectRow::new("item", "opaque").name("日本")),
        origin: origin.clone(),
    };
    ProjectDialog::Act {
        invocation: Box::new(ProjectInvocation {
            shared,
            effect: ProjectEffect::new(origin, "Act"),
        }),
        prompts: vec![prompt],
        stage: 0,
        buffer: "初éabc".into(),
        cursor,
        selected,
    }
}

fn confirmation() -> ProjectPrompt {
    ProjectPrompt::Confirmation(Confirmation::new(
        konoma_plugin::t!(en: "Delete {row}?", ja: "{row}を削除？"),
        vec![
            ConfirmChoice::new('!', "safe", "Safe"),
            ConfirmChoice::new('x', "danger", "Danger").destructive(),
            ConfirmChoice::new('a', "cjk", konoma_plugin::t!(en: "日本語", ja: "日本語")),
        ],
    ))
}

fn color(c: Color) -> u32 {
    match c {
        Color::Reset => 0,
        Color::Black => 1,
        Color::Red => 2,
        Color::Green => 3,
        Color::Yellow => 4,
        Color::Blue => 5,
        Color::Magenta => 6,
        Color::Cyan => 7,
        Color::Gray => 8,
        Color::DarkGray => 9,
        Color::LightRed => 10,
        Color::LightGreen => 11,
        Color::LightYellow => 12,
        Color::LightBlue => 13,
        Color::LightMagenta => 14,
        Color::LightCyan => 15,
        Color::White => 16,
        Color::Indexed(i) => 0x100 + u32::from(i),
        Color::Rgb(r, g, b) => {
            0x1000000 | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
        }
    }
}

fn cells(name: &str, outputs: Vec<(Buffer, Value)>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots")
        .join(format!("{name}.json"));
    let reference: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("pre-extraction cell fixture"))
            .unwrap();
    let records = reference["outputs"].as_array().unwrap();
    assert_eq!(records.len(), outputs.len());
    for ((buffer, state), expected) in outputs.iter().zip(records) {
        let a = buffer.area;
        assert_eq!(json!([a.x, a.y, a.width, a.height]), expected["rect"]);
        assert_eq!(*state, expected["state"]);
        let mut count = 0;
        let row_ids = expected["rows"].as_array().unwrap();
        assert_eq!(row_ids.len(), usize::from(a.height));
        for row in row_ids {
            let before = count;
            for run in reference["rows"][row.as_u64().unwrap() as usize]
                .as_array()
                .unwrap()
            {
                for _ in 0..run[0].as_u64().unwrap() {
                    let c = &buffer.content[count];
                    assert_eq!(
                        json!([c.symbol(), color(c.fg), color(c.bg), c.modifier.bits()]),
                        reference["cells"][run[1].as_u64().unwrap() as usize],
                        "{name}, cell {count}"
                    );
                    count += 1;
                }
            }
            assert_eq!(count - before, usize::from(a.width));
        }
        assert_eq!(count, usize::from(a.width) * usize::from(a.height));
        assert!(buffer.content.iter().any(|c| c.symbol() != " "));
    }
}

#[test]
fn project_body_cells_match_each_content_and_small_viewport() {
    let (mut app, _dir) = app();
    let mut outputs = vec![];
    for height in [2, 3, 4, 8] {
        for layout in [DiffLayout::Unified, DiffLayout::Split] {
            app.diff_layout = layout;
            for kind in 0..5 {
                let content = match kind {
                    0 => Some(list(false)),
                    1 => Some(ProjectContent::Lines(
                        (0..12)
                            .map(|i| {
                                Line::raw(format!("line {i} abcdefghijklmnopqrstuvwxyz")).green()
                            })
                            .collect(),
                    )),
                    2 => Some(diff(false)),
                    3 => Some(diff(true)),
                    _ => None,
                };
                outputs.push(body_output(
                    &mut app,
                    content,
                    Rect::new(3, 2, 26, height),
                    3,
                    4,
                    3,
                ));
            }
        }
    }
    cells("project_body", outputs);
}

#[test]
fn project_list_cells_preserve_columns_right_meta_and_chips() {
    let (mut app, _dir) = app();
    let mut outputs = vec![];
    for width in [6, 12, 30] {
        for height in [5, 8, 10] {
            for cursor in [4, 0] {
                outputs.push(body_output(
                    &mut app,
                    Some(list(true)),
                    Rect::new(3, 2, width, height),
                    cursor,
                    4,
                    0,
                ));
            }
        }
    }
    cells("project_list", outputs);
}

#[test]
fn project_dialog_cells_match_all_stages() {
    let (mut app, _dir) = app();
    let mut outputs = vec![];
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
        app.lang = lang;
        for area in [
            Rect::new(3, 2, 90, 24),
            Rect::new(3, 2, 36, 7),
            Rect::new(3, 2, 10, 3),
        ] {
            for kind in 0..5 {
                let prompt = match kind {
                    0 => confirmation(),
                    1 => ProjectPrompt::Text(TextInput::new("Message", "")),
                    _ => ProjectPrompt::Choice(ChoiceInput::new(
                        "Choose",
                        (0..12)
                            .map(|i| Choice::new(i.to_string(), format!("choice {i}")))
                            .collect(),
                    )),
                };
                app.project_dialog = Some(dialog(&app, prompt, 8, 2));
                if kind == 3 {
                    if let Some(ProjectDialog::Act { stage, .. }) = &mut app.project_dialog {
                        *stage = usize::MAX;
                    }
                }
                if kind == 4 {
                    app.project_dialog = Some(ProjectDialog::Projects {
                        projects: vec![
                            ProjectKey {
                                plugin: "fixture".into(),
                                root: "/project".into(),
                                marker: "marker".into()
                            };
                            12
                        ],
                        selected: 8,
                    });
                }
                let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
                let buffer = terminal
                    .draw(|f| {
                        f.render_widget(
                            Paragraph::new(vec![Line::raw("界".repeat(50)); 30])
                                .blue()
                                .on_yellow(),
                            f.area(),
                        );
                        crate::ui::dialog::render(f, &app, area);
                    })
                    .unwrap()
                    .buffer
                    .clone();
                let mut small = Buffer::empty(buffer.area);
                Paragraph::new(vec![Line::raw("界".repeat(50)); 30])
                    .blue()
                    .on_yellow()
                    .render(small.area, &mut small);
                draw::draw_project_dialog(&mut small, area, &small_dialog(&app));
                same_cells(&buffer, &small);
                outputs.push((buffer, Value::Null));
            }
        }
    }
    cells("project_dialog", outputs);
}

#[test]
fn dialog_footer_cells_reserve_cancel_before_choices() {
    let (mut app, _dir) = app();
    let mut outputs = vec![];
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
        app.lang = lang;
        app.project_dialog = Some(dialog(&app, confirmation(), 0, 0));
        for width in [1, 2, 3, 8, 12, 16, 24, 32, 80] {
            let area = Rect::new(3, 2, width, 1);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 4));
            Paragraph::new(Line::from(crate::ui::status::footer_spans(&app, width)))
                .render(area, &mut buffer);
            // Keep the comparison nonempty even when no footer token fits.
            buffer[(0, 0)].set_symbol("·");
            let mut small = Buffer::empty(buffer.area);
            Paragraph::new(Line::from(draw::project_dialog_footer(
                &small_dialog(&app),
                width,
            )))
            .render(area, &mut small);
            small[(0, 0)].set_symbol("·");
            same_cells(&buffer, &small);
            outputs.push((buffer, Value::Null));
        }
    }
    cells("project_dialog_footer", outputs);
}

#[test]
fn project_dialog_presentation_is_built_once_per_frame() {
    let (mut app, _dir) = app();
    app.project_dialog = Some(dialog(&app, confirmation(), 0, 0));
    for statusbar in ["split", "bottom"] {
        app.cfg.ui.statusbar = statusbar.into();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let before = draw::project_dialog_presentation_build_count();
        terminal.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        assert_eq!(draw::project_dialog_presentation_build_count() - before, 1);
    }
}

#[test]
fn project_resize_request_precedes_chip_reservation() {
    let (mut app, _dir) = app();
    let project = app.tab.project_navigation.as_ref().unwrap().project.clone();
    let (tx, jobs) = std::sync::mpsc::sync_channel(1);
    let runtime = app.project_runtimes.get_mut(&project).unwrap();
    runtime.tx = tx;
    runtime.busy = false;
    body_output(&mut app, Some(list(true)), Rect::new(3, 2, 30, 10), 0, 0, 0);
    let job = jobs
        .try_recv()
        .expect("resize must schedule a worker request");
    let konoma_plugin::host::project::ProjectJob::View { req, .. } = job else {
        panic!("view request required");
    };
    assert_eq!((req.cols, req.rows), (28, 8));
}
