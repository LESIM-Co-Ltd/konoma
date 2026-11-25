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

use crate::test_support::top_edge;

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
    let declaration = ProjectDecl::new()
        .marker(konoma_plugin::project::DetectionMarker::new(
            "marker",
            konoma_plugin::project::MarkerKind::Any,
        ))
        .screen(
            ProjectScreen::new("items", konoma_plugin::t!(en: "List", ja: "一覧"))
                .key('1')
                .marks(true)
                .checks(true),
        )
        .screen(ProjectScreen::new("other", "Other"))
        .screen(ProjectScreen::new("hidden", "Hidden").pushed());
    // The fixture plugin gets its jump keys from the same binding function every plugin does. The
    // registry itself would refuse the golden's digit key `1`, so the keys are built from the
    // manifest without its checks.
    app.plugin_keys = crate::plugins::PluginKeys::from_unchecked_manifest(
        &konoma_plugin::Manifest::new("fixture", "0.1.0").project(declaration.clone()),
    );
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
        origin_tab: None,
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
                declaration,
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
            acting_writes: false,
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
        heading: None,
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
) -> (Buffer, Value, Option<Frame>) {
    // A list and a diff are drawn inside the outer frame; the other contents are not.
    let frame = match &content {
        Some(ProjectContent::List(list)) => Some(Frame {
            area,
            title: list
                .title
                .as_ref()
                .map(|t| t.spans.iter().map(|s| s.text.as_str()).collect())
                .unwrap_or_default(),
        }),
        Some(ProjectContent::Diff(_)) => Some(Frame {
            area,
            title: String::new(),
        }),
        _ => None,
    };
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
    let jumps = app.plugin_keys.jumps_of(&nav.project.plugin).to_vec();
    let update = draw::draw_project(
        &mut small,
        area,
        ProjectDrawValues {
            jumps: &jumps,
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
            query: &nav.stack.last().unwrap().query,
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
        frame,
    )
}

/// The draw area and title of a framed content (a list or a diff).
struct Frame {
    area: Rect,
    title: String,
}

/// The screen tabs of the fixture plugin's declaration, as `draw_project` words them.
const TABS: &str = " 1:List  Other ";

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
    cells_framed(
        name,
        outputs.into_iter().map(|(b, s)| (b, s, None)).collect(),
    );
}

/// The frozen cell at (x, y) of record `expected`, as `[symbol, fg, bg, modifier]`.
fn frozen_cell(reference: &Value, expected: &Value, width: u16, x: u16, y: u16) -> Value {
    let mut count = 0usize;
    let rows = expected["rows"].as_array().unwrap();
    let row = rows[usize::from(y)].as_u64().unwrap() as usize;
    for run in reference["rows"][row].as_array().unwrap() {
        let n = run[0].as_u64().unwrap() as usize;
        if usize::from(x) < count + n {
            return reference["cells"][run[1].as_u64().unwrap() as usize].clone();
        }
        count += n;
    }
    panic!("no frozen cell at {x},{y} (width {width})");
}

fn cell_json(c: &ratatui::buffer::Cell) -> Value {
    json!([c.symbol(), color(c.fg), color(c.bg), c.modifier.bits()])
}

/// Compares a framed output with the pre-extraction fixture, which has no frame. The frame moved
/// the body one column right (derived from the old and new body geometry): every frozen body cell
/// is compared with the new cell at the body's new place. The cells of the draw area outside the
/// old body (the frozen tab/title row, the last row, the two right columns) are checked against
/// the rule instead: frame glyphs on the left, right and bottom edges; the top edge `┌`, the
/// title at the left, the tabs right-aligned (dropped when title and tabs do not both fit), `┐`.
/// Cells outside the draw area stay one-to-one.
fn cells_framed(name: &str, outputs: Vec<(Buffer, Value, Option<Frame>)>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots")
        .join(format!("{name}.json"));
    let reference: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("pre-extraction cell fixture"))
            .unwrap();
    let records = reference["outputs"].as_array().unwrap();
    assert_eq!(records.len(), outputs.len());
    for ((buffer, state, frame), expected) in outputs.iter().zip(records) {
        let a = buffer.area;
        assert_eq!(json!([a.x, a.y, a.width, a.height]), expected["rect"]);
        assert_eq!(*state, expected["state"]);
        if let Some(frame) = frame {
            framed_record(name, &reference, expected, buffer, frame);
            continue;
        }
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

fn within(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

fn framed_record(name: &str, reference: &Value, expected: &Value, buffer: &Buffer, frame: &Frame) {
    let area = frame.area;
    let old = draw::unframed_body_area(area);
    let new = draw::project_body_area(area);
    assert_eq!((new.width, new.height), (old.width, old.height));
    let (dx, dy) = (new.x - old.x, new.y - old.y);
    assert_eq!(
        (dx, dy),
        (1, 0),
        "the frame moves the body one column right"
    );
    let a = buffer.area;
    let (mut compared, mut outside) = (0usize, 0usize);
    for y in 0..a.height {
        for x in 0..a.width {
            let frozen = frozen_cell(reference, expected, a.width, x, y);
            if within(old, x, y) {
                assert_eq!(
                    cell_json(&buffer[(x + dx, y + dy)]),
                    frozen,
                    "{name}, body cell {x},{y}"
                );
                compared += 1;
            } else if !within(area, x, y) {
                assert_eq!(cell_json(&buffer[(x, y)]), frozen, "{name}, cell {x},{y}");
                outside += 1;
            }
        }
    }
    assert_eq!(compared, usize::from(old.width) * usize::from(old.height));
    assert_eq!(
        outside,
        usize::from(a.width) * usize::from(a.height)
            - usize::from(area.width) * usize::from(area.height)
    );
    // The cells of the area outside the body follow the rule.
    // The whole top edge as text (corners, title, filler, tabs, or no tabs when they do not fit)
    // and the columns the tabs take, both from the display widths of title and tabs.
    top_edge::check(buffer, area, &frame.title, TABS).unwrap_or_else(|e| panic!("{name}: {e}"));
    let tab_columns = top_edge::tabs_columns(area, &frame.title, TABS);
    let title_cells = top_edge::width(&frame.title).min(area.width.saturating_sub(2));
    let default_cell = |x: u16, y: u16| {
        let c = &buffer[(x, y)];
        (c.fg, c.bg, c.modifier)
            == (
                Color::Reset,
                Color::Reset,
                ratatui::style::Modifier::empty(),
            )
    };
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if within(new, x, y) {
                continue;
            }
            let c = &buffer[(x, y)];
            let (left, right) = (x == area.x, x == area.right() - 1);
            if y == area.y {
                // The glyphs of the row were checked as text; here the style of the plain cells.
                let in_title = !left && !right && x - area.x - 1 < title_cells;
                let in_tabs = tab_columns.as_ref().is_some_and(|r| r.contains(&x));
                if left || right || !(in_title || in_tabs) {
                    assert!(default_cell(x, y), "{name}, frame glyph style {x},{y}");
                }
                continue;
            }
            let want = if y == area.bottom() - 1 {
                (if left {
                    "└"
                } else if right {
                    "┘"
                } else {
                    "─"
                })
                .to_string()
            } else {
                "│".to_string()
            };
            assert_eq!(c.symbol(), want, "{name}, frame cell {x},{y}");
            assert!(default_cell(x, y), "{name}, frame glyph style {x},{y}");
        }
    }
    // The frozen tab row: the tabs now sit at the right of the top edge. Their text, order and
    // style are compared with the frozen tab cells; the frozen title (beside the tabs) with the
    // title at the left, where the old row showed it at all.
    let frozen_row = |i: u16| frozen_cell(reference, expected, a.width, area.x + i, area.y);
    let tab_chars: Vec<char> = TABS.chars().collect();
    if let Some(columns) = &tab_columns {
        assert_eq!(usize::from(columns.end - columns.start), tab_chars.len());
        for (i, ch) in tab_chars.iter().enumerate() {
            let f = frozen_row(i as u16);
            assert_eq!(f[0], json!(ch.to_string()), "{name}, frozen tab text");
            assert_eq!(
                cell_json(&buffer[(columns.start + i as u16, area.y)]),
                f,
                "{name}, tab {i}"
            );
        }
    }
    let title: Vec<char> = frame.title.chars().collect();
    let room = usize::from(area.width.saturating_sub(2));
    let old_tab_width = tab_chars.len().min(usize::from(area.width));
    for (j, ch) in title.iter().enumerate() {
        if old_tab_width + j < usize::from(area.width) && j < room {
            let f = frozen_row((old_tab_width + j) as u16);
            assert_eq!(f[0], json!(ch.to_string()), "{name}, frozen title text");
            assert_eq!(
                cell_json(&buffer[(area.x + 1 + j as u16, area.y)]),
                f,
                "{name}, title {j}"
            );
        }
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
    cells_framed("project_body", outputs);
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
    cells_framed("project_list", outputs);
}

#[test]
fn project_dialog_cells_match_all_stages() {
    let (mut app, dir) = app();
    // The project list shows each root relative to the open directory, and the frozen fixture
    // holds six `..` steps. Build that depth under the temp dir instead of inheriting the depth of
    // whatever `TMPDIR` the machine has (a shallow `/tmp/x` on Linux CI shows fewer steps).
    let deep = dir
        .join("a")
        .join("b")
        .join("c")
        .join("d")
        .join("e")
        .join("f");
    std::fs::create_dir_all(&deep).unwrap();
    app.tab.open_dir = deep;
    let project_root = dir.join("project");
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
                                root: project_root.clone(),
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

// ---- the project screen is a layer of the walk ----

/// The layers that belong to the screen under a project screen, each turned on in turn: the project
/// screen is still the frontmost layer, and the projections follow it (the keys are a canvas's, the
/// chip is the screen's own).
#[test]
fn project_layer_masks_the_layers_of_the_screen_under_it() {
    use crate::keymap::Surface;
    type Setup = fn(&mut App);
    let masked: Vec<(&str, Setup)> = vec![
        ("file diff", |a| {
            a.tab.preview_kind = Some(PreviewKind::GitDiff(PathBuf::from("x.txt")));
            a.tab.mode = Mode::Preview;
        }),
        ("tree filter input", |a| {
            a.tab.filter_input = Some(String::new())
        }),
        ("preview search input", |a| {
            a.tab.search_input = Some(String::new())
        }),
        ("mark wait", |a| a.mark_set_pending = true),
        ("changed-only filter", |a| a.tab.changed_filter = true),
        ("tree visual", |a| a.tab.visual_anchor = Some(0)),
        ("outline", |a| a.outline_open = true),
        ("outline filter", |a| {
            a.outline_open = true;
            a.outline_filtering = true;
        }),
        ("table cell", |a| a.table_cell_open = true),
    ];
    for (name, setup) in masked {
        let (mut app, _dir) = app();
        assert!(app.project_active());
        assert_eq!(app.frontmost_layer(|_| true), UiLayer::Project, "none");
        setup(&mut app);
        assert_eq!(
            app.frontmost_layer(|_| true),
            UiLayer::Project,
            "{name}: the screen under the project screen must not be in front"
        );
        assert_eq!(app.internal_mode(), None, "{name}: no chip of its own");
        assert_eq!(app.surface(), Surface::PreviewCanvas, "{name}");
    }

    // What is drawn over the project screen stays in front of it.
    let in_front: Vec<(&str, Setup, UiLayer)> = vec![
        ("help", |a| a.show_help = true, UiLayer::Help),
        (
            "bookmark list",
            |a| a.bookmark_list = true,
            UiLayer::Bookmarks,
        ),
        (
            "bookmark list filter",
            |a| {
                a.bookmark_list = true;
                a.bookmark_list_filtering = true;
            },
            UiLayer::BookmarksFilter,
        ),
        ("tab list", |a| a.tab_list = true, UiLayer::Tabs),
        (
            "tab list filter",
            |a| {
                a.tab_list = true;
                a.tab_list_filtering = true;
            },
            UiLayer::TabsFilter,
        ),
        ("info", |a| a.show_info = true, UiLayer::Info),
        (
            "confirm dialog",
            |a| {
                a.dialog = Some(Dialog {
                    op: PendingOp::Quit,
                    kind: DialogKind::Confirm {
                        message: "message".into(),
                        allow_permanent: false,
                    },
                })
            },
            UiLayer::ConfirmQuit,
        ),
    ];
    for (name, setup, layer) in in_front {
        let (mut app, _dir) = app();
        // The masked state is on as well: the overlay still wins.
        app.tab.filter_input = Some(String::new());
        app.tab.visual_anchor = Some(0);
        setup(&mut app);
        assert_eq!(app.frontmost_layer(|_| true), layer, "{name}");
        assert_eq!(app.internal_mode(), layer.internal_mode(), "{name}");
    }

    // Without a project screen the same states are in front again.
    let (mut app, _dir) = app();
    app.tab.filter_input = Some(String::new());
    app.tab.project_navigation = None;
    assert_eq!(app.frontmost_layer(|_| true), UiLayer::Filter);
}

/// The keys that turn the pages of the preview under a project screen are not available on it, and
/// a state the screen under left open (the outline, the cell popup) is not drawn over the screen.
#[test]
fn project_screen_does_not_page_or_draw_the_masked_overlays_of_the_screen_under() {
    use crate::keymap::{Action, Context};
    let (mut app, _dir) = app();
    app.tab.page_count = Some(3);
    for a in [Action::PageNext, Action::PagePrev] {
        assert!(
            !crate::ops::action_enabled(&app, Context::Canvas, a),
            "{a:?}: not on a project canvas"
        );
    }
    // The same preview, with no project screen over it, turns pages.
    let covered = app.tab.project_navigation.take();
    assert!(!app.project_screen_covers());
    for a in [Action::PageNext, Action::PagePrev] {
        assert!(
            crate::ops::action_enabled(&app, Context::Canvas, a),
            "{a:?}: acts on the preview itself"
        );
    }
    app.tab.project_navigation = covered;
    assert!(app.project_screen_covers());

    // A state left open under the screen is not drawn over it.
    app.outline_open = true;
    let mut term = Terminal::new(TestBackend::new(60, 14)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let text = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    let outline_title = crate::i18n::tr(app.lang, crate::i18n::Msg::PageListTitle);
    assert!(
        !text.contains(outline_title),
        "the outline popup is drawn over the screen"
    );
}
