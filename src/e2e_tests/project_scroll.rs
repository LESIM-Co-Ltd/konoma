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
                    .screen(ProjectScreen::new("lines", "Lines").key('z'))
                    .screen(ProjectScreen::new("diff", "Diff").key('d'))
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
            "lines" => View::text((0..ROWS).map(|i| Screen::Lines.label(i))),
            "diff" => View::Diff(PluginDiffView {
                hunks: vec![DiffHunk {
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
        let dir = sandbox(name);
        std::fs::write(dir.join("fixture.scroll-project"), "project").unwrap();
        let marker = test_plugins::register(name, Arc::new(ScrollPlugin));
        let mut cfg = test_plugins::marker_config(&marker);
        cfg.ui.lang = "en".into();
        cfg.ui.statusbar = "split".into();
        cfg.ui.tabbar = "hidden".into();
        cfg.external.git = false;
        let mut sim = Sim::with_config_sized(&canon(&dir), cfg, WIDTH, BODY_ROWS + CHROME_ROWS);
        sim.app.tab.projects.wait_for_result();
        assert!(sim.app.tab.projects.uses_injected_events());
        sim.app.poll_projects();
        sim.key('p');
        drain(&mut sim);
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
fn e2e_project_lines_scroll_follows_hidden_cursor_and_page_keys() {
    let mut f = ScrollFixture::new("project_scroll_lines", Screen::Lines);
    f.expect_window(0, 0);
    for _ in 0..=BODY_ROWS {
        f.press(KeyCode::Char('j'));
    }
    f.expect_window(2, 5);
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
fn e2e_project_diff_scroll_follows_hidden_cursor_and_last_row() {
    let mut f = ScrollFixture::new("project_scroll_diff", Screen::Diff);
    f.expect_window(0, 0);
    for _ in 0..=BODY_ROWS {
        f.press(KeyCode::Char('j'));
    }
    f.expect_window(2, 5);
    assert!(!f.body().iter().any(|row| row.text.contains("diff-000")));
    f.press(KeyCode::Char('G'));
    f.expect_window(ROWS - usize::from(BODY_ROWS), ROWS - 1);
    f.press(KeyCode::Char('g'));
    f.expect_window(0, 0);
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
            f.expect_window(usize::from(body_rows == 1), 1);
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
