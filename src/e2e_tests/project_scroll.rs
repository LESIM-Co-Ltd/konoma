//! Project scrolling through key dispatch and the real TestBackend rendering path.

use super::super::*;
use super::drain;
use crate::app::project_screens::ProjectContent;
use crate::plugins::test_plugins;
use konoma_plugin::view::{
    DiffHunk, DiffKind, DiffLine, DiffView as PluginDiffView, ListRow, ListView, View,
};
use konoma_plugin::{
    DetectionMarker, Failure, Input, Line, Manifest, MarkerKind, OpenCx, Plugin, ProjectDecl,
    ProjectScreen, Session, ViewCx, ViewKind, ViewReq,
};
use ratatui::style::Modifier;
use std::sync::Arc;

const ROWS: usize = 40;
const WIDTH: u16 = 96;
const BODY_ROWS: u16 = 4;
// Split header, project tabs, the bottom content margin, and the footer.
const CHROME_ROWS: u16 = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Lines,
    Diff,
    List,
}

impl Screen {
    fn id(self) -> &'static str {
        match self {
            Self::Lines => "lines",
            Self::Diff => "diff",
            Self::List => "list",
        }
    }

    fn key(self) -> char {
        match self {
            Self::Lines => 'z',
            Self::Diff => 'd',
            Self::List => 'i',
        }
    }

    fn label(self, index: usize) -> String {
        format!("{}-{index:03}", self.id())
    }
}

struct ScrollPlugin;

impl Plugin for ScrollPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("project-scroll", "1")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("*.scroll-project", MarkerKind::File))
                    .watch("metadata/index")
                    .screen(ProjectScreen::new("hidden", "Hidden").pushed())
                    .screen(ProjectScreen::new("lines", "Lines").key('z'))
                    .screen(ProjectScreen::new("diff", "Diff").key('d'))
                    .screen(ProjectScreen::new("wide", "Wide").key('w'))
                    .screen(ProjectScreen::new("list", "List").key('i').row_kind("item")),
            )
            .shows(ViewKind::Text)
            .shows(ViewKind::Diff)
            .shows(ViewKind::List)
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        assert!(matches!(input, Input::Project { .. }));
        Ok(Box::new(ScrollPlugin))
    }
}

impl Session for ScrollPlugin {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        // Return the whole content, independent of the viewport: scrolling belongs to the host.
        Ok(match req.project.as_ref().unwrap().screen.as_str() {
            "hidden" => View::text(["hidden"]),
            "wide" => View::text((0..ROWS).map(|_| "日本語".repeat(80))),
            "lines" => View::text((0..ROWS).map(|i| Screen::Lines.label(i))),
            "diff" => View::Diff(PluginDiffView {
                header: vec![],
                hunks: vec![DiffHunk {
                    path: None,
                    lines: (0..ROWS)
                        .map(|i| DiffLine {
                            kind: DiffKind::Added,
                            old_no: None,
                            new_no: Some(i + 1),
                            text: Screen::Diff.label(i),
                        })
                        .collect(),
                }],
            }),
            "list" => View::List(ListView {
                rows: (0..ROWS)
                    .map(|i| {
                        ListRow::new("item", vec![Line::raw(Screen::List.label(i))])
                            .value(i.to_string())
                    })
                    .collect(),
            }),
            other => panic!("unexpected screen: {other}"),
        })
    }
}

struct BodyRow {
    text: String,
    reversed: bool,
}

struct ScrollFixture {
    sim: Sim,
    screen: Screen,
    _dir: crate::test_support::TmpDir,
}

impl ScrollFixture {
    fn new(name: &str, screen: Screen) -> Self {
        Self::with_config(name, screen, |_| {})
    }

    fn with_config(name: &str, screen: Screen, configure: impl FnOnce(&mut Config)) -> Self {
        let dir = sandbox(name);
        std::fs::write(dir.join("fixture.scroll-project"), "project").unwrap();
        let marker = test_plugins::register(name, Arc::new(ScrollPlugin));
        let mut cfg = test_plugins::marker_config(&marker);
        cfg.ui.lang = "en".into();
        cfg.ui.statusbar = "split".into();
        cfg.ui.tabbar = "hidden".into();
        cfg.external.git = false;
        configure(&mut cfg);
        let mut sim = Sim::with_config_sized(&canon(&dir), cfg, WIDTH, BODY_ROWS + CHROME_ROWS);
        sim.app.tab.projects.wait_for_result();
        assert!(sim.app.tab.projects.uses_injected_events());
        sim.app.poll_projects();
        sim.key('p');
        drain(&mut sim);
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "lines");
        sim.key(screen.key());
        drain(&mut sim);
        assert!(sim.app.project_active());
        let content = sim.app.project_content().unwrap();
        assert!(matches!(
            (screen, content.as_ref()),
            (Screen::Lines, ProjectContent::Lines(_))
                | (Screen::Diff, ProjectContent::Diff(_))
                | (Screen::List, ProjectContent::List(_))
        ));
        Self {
            sim,
            screen,
            _dir: dir,
        }
    }

    fn press(&mut self, code: KeyCode) {
        self.sim.press(code, KeyModifiers::NONE);
        drain(&mut self.sim);
    }

    fn body(&mut self) -> Vec<BodyRow> {
        let app = &mut self.sim.app;
        let buffer = self
            .sim
            .term
            .draw(|frame| ui::render(frame, app))
            .unwrap()
            .buffer
            .clone();
        let height = buffer.area.height.saturating_sub(CHROME_ROWS);
        assert_eq!(self.sim.app.tab.preview_viewport, height);
        (0..height)
            .map(|row| {
                let cells: Vec<_> = (0..buffer.area.width.saturating_sub(2))
                    .map(|x| &buffer[(x, row + 2)])
                    .collect();
                BodyRow {
                    text: cells.iter().map(|cell| cell.symbol()).collect(),
                    reversed: cells
                        .iter()
                        .any(|cell| cell.modifier.contains(Modifier::REVERSED)),
                }
            })
            .collect()
    }

    #[track_caller]
    fn expect_window(&mut self, first: usize, cursor: usize) {
        let rows = self.body();
        for (offset, row) in rows.iter().enumerate() {
            let index = first + offset;
            assert!(
                row.text.trim_end().ends_with(&self.screen.label(index)),
                "body row {offset}: expected {}, got {:?}",
                self.screen.label(index),
                row.text
            );
            assert_eq!(
                row.reversed,
                self.screen == Screen::List && index == cursor,
                "body row {offset}: only the selected List row is reversed"
            );
        }
    }
}

#[test]
fn e2e_project_lines_scroll_keeps_top_line_and_page_keys() {
    let mut f = ScrollFixture::new("project_scroll_lines", Screen::Lines);
    f.expect_window(0, 0);
    for _ in 0..=BODY_ROWS {
        f.press(KeyCode::Char('j'));
    }
    f.expect_window(5, 0);
    assert!(!f.body().iter().any(|row| row.text.contains("lines-000")));
    f.press(KeyCode::Char('G'));
    f.expect_window(ROWS - usize::from(BODY_ROWS), ROWS - 1);
    f.press(KeyCode::Char('g'));
    f.expect_window(0, 0);
    f.press(KeyCode::PageDown);
    assert!(!f.body().iter().any(|row| row.text.contains("lines-000")));
    f.press(KeyCode::PageUp);
    f.expect_window(0, 0);
}

#[test]
fn e2e_project_diff_scroll_keeps_top_line_and_last_page_full() {
    let mut f = ScrollFixture::new("project_scroll_diff", Screen::Diff);
    f.expect_window(0, 0);
    for _ in 0..=BODY_ROWS {
        f.press(KeyCode::Char('j'));
    }
    f.expect_window(5, 0);
    assert!(!f.body().iter().any(|row| row.text.contains("diff-000")));
    f.press(KeyCode::Char('G'));
    f.expect_window(ROWS - usize::from(BODY_ROWS), ROWS - 1);
    f.press(KeyCode::Char('g'));
    f.expect_window(0, 0);
}

#[test]
fn e2e_project_diff_highlighter_reuses_unchanged_layout_and_vertical_scroll() {
    use crate::preview::gitdiff::highlight_calls_for_test;
    let mut f = ScrollFixture::new("project_diff_cache", Screen::Diff);
    let before = highlight_calls_for_test();
    f.body();
    f.body();
    f.press(KeyCode::Char('j'));
    assert_eq!(highlight_calls_for_test(), before);
    f.press(KeyCode::Char('s'));
    assert!(highlight_calls_for_test() > before);
}

#[test]
fn e2e_project_diff_key_after_hscroll_clamp_reuses_layout() {
    let mut f = ScrollFixture::new("project_diff_clamp_cache", Screen::Diff);
    let content = f.sim.app.project_content().unwrap();
    let ProjectContent::Diff(diff) = content.as_ref() else {
        panic!("diff fixture")
    };
    let before = diff.layout_count();
    f.body();
    let draw_alone = diff.layout_count() - before;
    assert_eq!(draw_alone, 0);
    f.sim
        .app
        .tab
        .project_navigation
        .as_mut()
        .unwrap()
        .stack
        .last_mut()
        .unwrap()
        .hscroll = u16::MAX;
    let before = diff.layout_count();
    f.press(KeyCode::Char('j'));
    f.body();
    assert!(diff.layout_count() - before <= draw_alone + 1);
    assert_eq!(f.sim.app.project_frame().unwrap().hscroll, 0);
    let after = diff.layout_count();
    f.body();
    assert_eq!(diff.layout_count(), after);
}

#[test]
fn e2e_project_diff_and_text_hints_follow_their_actual_motions() {
    for screen in [Screen::Lines, Screen::Diff] {
        let f = ScrollFixture::new("project_scroll_hints", screen);
        let hints = f.sim.app.project_controls().unwrap();
        assert_eq!(hints.contains("s:"), screen == Screen::Diff, "{hints}");
        assert!(hints.contains("h/l") && hints.contains("g/G"), "{hints}");
        let help = crate::ops::help::sections(&f.sim.app);
        assert_eq!(
            help.iter().flat_map(|s| &s.rows).any(|(k, _)| k == "s"),
            screen == Screen::Diff
        );
    }
}

#[cfg(feature = "git")]
#[test]
fn e2e_project_diff_layout_is_independent_of_rendered_and_picture_origin() {
    for picture in [false, true] {
        let mut f = ScrollFixture::new(&format!("project_diff_origin_{picture}"), Screen::Diff);
        f.press(KeyCode::Char('q'));
        init_git_repo(&f._dir);
        let path = f._dir.join(if picture { "picture.png" } else { "doc.md" });
        if picture {
            write_solid_png(&path, 8, 8, [10, 20, 30]);
        } else {
            std::fs::write(&path, "# Title\n\nOriginal.\n").unwrap();
        }
        run_git(&f._dir, &["add", "-A"]);
        run_git(&f._dir, &["commit", "-q", "-m", "base"]);
        if picture {
            write_solid_png(&path, 8, 8, [30, 20, 10]);
        } else {
            std::fs::write(&path, "# Title\n\nChanged.\n").unwrap();
        }
        f.sim = f.sim.with_picker();
        f.sim.app.open_git_diff(&path);
        f.sim.draw();
        assert!(f.sim.app.diff_rendered_active());
        assert_eq!(f.sim.app.media_diff_showing_pictures(), picture);
        let media_layout = f.sim.app.media_diff_layout();
        let view = f.sim.app.diff_view_for_test();
        f.press(KeyCode::Char('p'));
        f.press(KeyCode::Char('d'));
        assert!(f.sim.app.project_active());
        assert!(f.sim.app.diff_rendered_active());
        assert_eq!(f.sim.app.media_diff_showing_pictures(), picture);
        assert!(!f.sim.app.diff_is_split(WIDTH - 2));
        f.press(KeyCode::Char('s'));
        assert!(f.sim.app.diff_is_split(WIDTH - 2));
        assert_eq!(f.sim.app.media_diff_layout(), media_layout);
        assert_eq!(f.sim.app.diff_view_for_test(), view);
        assert!(f.sim.app.project_controls().unwrap().contains("s:"));
        f.press(KeyCode::Char('q'));
        assert!(!f.sim.app.project_active());
        assert!(f.sim.app.is_git_diff_preview());
        assert_eq!(f.sim.app.tab.preview_path.as_ref(), Some(&path));
        assert_eq!(f.sim.app.diff_view_for_test(), view);
        assert_eq!(f.sim.app.media_diff_layout(), media_layout);
    }
}

#[test]
fn e2e_project_diff_layout_remap_drives_keys_footer_and_help() {
    for remapped in [false, true] {
        let mut f = ScrollFixture::with_config(
            &format!("project_layout_structural_{remapped}"),
            Screen::Diff,
            |cfg| {
                // Layout belongs to the diff layer, independently of the text motion table.
                cfg.keys.surfaces.insert(
                    "text".into(),
                    [("S".into(), "noop".into()), ("s".into(), "noop".into())].into(),
                );
                if remapped {
                    cfg.keys.surfaces.insert(
                        "diff".into(),
                        [
                            ("S".into(), "cycle_diff_layout".into()),
                            ("s".into(), "noop".into()),
                        ]
                        .into(),
                    );
                }
            },
        );
        let (active, inactive) = if remapped { ('S', 's') } else { ('s', 'S') };
        assert!(!f.sim.app.diff_is_split(WIDTH - 2));
        f.press(KeyCode::Char(inactive));
        assert!(!f.sim.app.diff_is_split(WIDTH - 2));
        f.press(KeyCode::Char(active));
        assert!(f.sim.app.diff_is_split(WIDTH - 2));
        let hints = f.sim.app.project_controls().unwrap();
        assert!(hints.contains(&format!("{active}:")), "{hints}");
        assert!(!hints.contains(&format!("{inactive}:")), "{hints}");
        let help = crate::ops::help::sections(&f.sim.app);
        assert!(help
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(key, _)| key == &active.to_string()));
        assert!(!help
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(key, _)| key == &inactive.to_string()));
    }
}

#[test]
fn e2e_project_list_right_without_enter_act_does_nothing_and_has_no_hint() {
    let mut f = ScrollFixture::new("project_list_no_enter", Screen::List);
    for code in [KeyCode::Char('l'), KeyCode::Right, KeyCode::Enter] {
        f.press(code);
        assert_eq!(f.sim.app.project_frame().unwrap().key.screen, "list");
        assert_eq!(f.sim.app.project_frame().unwrap().cursor, 0);
    }
    assert!(!f.sim.app.project_controls().unwrap().contains("l:"));
}

#[test]
fn e2e_project_text_motion_steps_horizontal_caps_less_and_remapped_keys() {
    for less in [false, true] {
        let mut f = ScrollFixture::with_config(
            &format!("project_text_motions_{less}"),
            Screen::Lines,
            |cfg| {
                cfg.ui.keys = if less { "less" } else { "vim" }.into();
                cfg.keys.surfaces.insert(
                    "text".into(),
                    [
                        ("Z".into(), "navigate:down".into()),
                        ("j".into(), "noop".into()),
                    ]
                    .into(),
                );
            },
        );
        f.press(KeyCode::Char('w'));
        f.press(KeyCode::Char('j'));
        assert_eq!(f.sim.app.project_frame().unwrap().scroll, 0);
        f.press(KeyCode::Char('Z'));
        assert_eq!(f.sim.app.project_frame().unwrap().scroll, 1);
        let modifiers = if less {
            KeyModifiers::NONE
        } else {
            KeyModifiers::CONTROL
        };
        f.sim.press(KeyCode::Char('d'), modifiers);
        drain(&mut f.sim);
        assert_eq!(f.sim.app.project_frame().unwrap().scroll, 11);
        f.sim.press(KeyCode::Char('f'), modifiers);
        drain(&mut f.sim);
        assert_eq!(f.sim.app.project_frame().unwrap().scroll, 31);
        f.press(KeyCode::Char('G'));
        assert_eq!(f.sim.app.project_frame().unwrap().scroll, 36);
        f.press(KeyCode::Char('g'));
        assert_eq!(f.sim.app.project_frame().unwrap().scroll, 0);
        f.press(KeyCode::Char('l'));
        assert_eq!(f.sim.app.project_frame().unwrap().hscroll, 4);
        f.press(KeyCode::Char('$'));
        assert_eq!(
            f.sim.app.project_frame().unwrap().hscroll,
            480 - (WIDTH - 2)
        );
        f.press(KeyCode::Char('h'));
        assert_eq!(
            f.sim.app.project_frame().unwrap().hscroll,
            480 - (WIDTH - 2) - 4
        );
        f.press(KeyCode::Char('0'));
        assert_eq!(f.sim.app.project_frame().unwrap().hscroll, 0);
        let split = f.sim.app.diff_is_split(WIDTH - 2);
        f.press(KeyCode::Char('s'));
        assert_eq!(f.sim.app.diff_is_split(WIDTH - 2), split);
        assert!(!f.sim.app.project_controls().unwrap().contains("j/"));
    }
}

#[test]
fn e2e_project_text_motion_remap_moves_lines_and_diffs() {
    for screen in [Screen::Lines, Screen::Diff] {
        let mut f = ScrollFixture::with_config("project_motion_layer", screen, |cfg| {
            cfg.keys.surfaces.insert(
                "text".into(),
                [
                    ("Z".into(), "navigate:down".into()),
                    ("j".into(), "noop".into()),
                ]
                .into(),
            );
        });
        f.press(KeyCode::Char('j'));
        assert_eq!(f.sim.app.project_frame().unwrap().scroll, 0);
        f.press(KeyCode::Char('Z'));
        assert_eq!(f.sim.app.project_frame().unwrap().scroll, 1);
    }
}

#[test]
fn e2e_project_list_scroll_follows_cursor_back_up() {
    let mut f = ScrollFixture::new("project_scroll_list", Screen::List);
    f.expect_window(0, 0);
    for (cursor, first) in [(1, 0), (2, 0), (3, 0), (4, 1), (5, 2), (6, 3), (7, 4)] {
        f.press(KeyCode::Char('j'));
        f.expect_window(first, cursor);
    }
    for (cursor, first) in [(6, 3), (5, 2), (4, 1), (3, 0), (2, 0), (1, 0), (0, 0)] {
        f.press(KeyCode::Char('k'));
        f.expect_window(first, cursor);
    }
}

#[test]
fn e2e_project_scroll_zero_one_and_two_body_rows() {
    for screen in [Screen::Lines, Screen::Diff, Screen::List] {
        let mut f = ScrollFixture::new(&format!("project_scroll_tiny_{}", screen.id()), screen);
        for body_rows in [0, 1, 2] {
            f.sim.resize(WIDTH, body_rows + CHROME_ROWS);
            drain(&mut f.sim);
            assert_eq!(f.body().len(), usize::from(body_rows));
            f.press(KeyCode::Char('g'));
            f.expect_window(0, 0);
            f.press(KeyCode::Char('j'));
            f.expect_window(
                if screen == Screen::List {
                    usize::from(body_rows == 1)
                } else {
                    1
                },
                1,
            );
            f.press(KeyCode::Char('G'));
            if body_rows > 0 {
                f.expect_window(ROWS - usize::from(body_rows), ROWS - 1);
            } else {
                assert!(f.body().is_empty());
            }
            for code in [
                KeyCode::PageDown,
                KeyCode::PageUp,
                KeyCode::Char('k'),
                KeyCode::Char('g'),
            ] {
                f.press(code);
            }
            assert!(f.sim.app.project_active());
            f.expect_window(0, 0);
        }
    }
}
