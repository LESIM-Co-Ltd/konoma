//! Leaving a file preview retains its owner and distinguishes navigation from quitting.

use super::*;
use crate::view::{Grid, TableView, TextSource, TextView};
use ratatui::backend::TestBackend;

fn key(c: char) -> AppEvent {
    AppEvent::Key(Key::char(c))
}

struct Page {
    table: bool,
}

struct Cells;
impl Grid for Cells {
    fn nrows(&self) -> usize {
        3
    }
    fn ncols(&self) -> usize {
        3
    }
    fn header(&self, col: usize) -> Option<String> {
        Some(format!("col {col}"))
    }
    fn cell(&self, row: usize, col: usize) -> String {
        format!("{row},{col}")
    }
}

impl Session for Page {
    fn layout(&self) -> Layout {
        paged_layout()
    }
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
        if self.table {
            Ok(View::Table(TableView {
                grid: std::sync::Arc::new(Cells),
                title: None,
                rainbow: false,
                addressed: false,
            }))
        } else {
            Ok(View::Text(TextView {
                source: TextSource::Owned("body\n".repeat(40)),
                styler: None,
                file_lines: true,
                syntax_name: None,
            }))
        }
    }
}

#[test]
fn preview_exit_distinguishes_back_quit_and_end() {
    let cx = AppCx::default();
    let keys = AppKeys::defaults();
    let esc = AppEvent::Key(Key::new(KeyCode::Esc));
    let ctrl_c = AppEvent::Key(Key::ctrl('c'));
    let cases = [
        (false, vec![key('q')], FileLoopExit::Back),
        (false, vec![esc.clone()], FileLoopExit::Back),
        (false, vec![key('Q')], FileLoopExit::Quit),
        (false, vec![ctrl_c.clone()], FileLoopExit::Quit),
        (false, vec![key('?'), key('Q')], FileLoopExit::Quit),
        (false, vec![key('?'), ctrl_c.clone()], FileLoopExit::Quit),
        (false, vec![key('?'), key('q')], FileLoopExit::End),
        (false, vec![key('?'), esc.clone()], FileLoopExit::End),
        (
            true,
            vec![AppEvent::Key(Key::new(KeyCode::Enter)), key('q')],
            FileLoopExit::End,
        ),
        (
            true,
            vec![AppEvent::Key(Key::new(KeyCode::Enter)), esc],
            FileLoopExit::End,
        ),
        (
            true,
            vec![AppEvent::Key(Key::new(KeyCode::Enter)), key('Q')],
            FileLoopExit::Quit,
        ),
        (
            true,
            vec![AppEvent::Key(Key::new(KeyCode::Enter)), ctrl_c],
            FileLoopExit::Quit,
        ),
        (
            true,
            vec![
                AppEvent::Key(Key::new(KeyCode::Enter)),
                key('?'),
                key('q'),
                key('q'),
            ],
            FileLoopExit::End,
        ),
        (false, vec![], FileLoopExit::End),
    ];
    for (table, events, want) in cases {
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut session = Page { table };
        let mut state = FileLoopState::new(&cx, &keys);
        let mut events = events.into_iter();
        assert_eq!(
            run_file_with(&mut term, &mut session, &mut events, &cx, &mut state, None).unwrap(),
            want
        );
        assert_eq!(events.count(), 0, "the overlay closes before leaving");
        if want != FileLoopExit::Quit {
            assert!(state.st.help.is_none());
            if let Frame::Table(t) = &state.base {
                assert!(t.popup.is_none());
            }
        }
    }

    let config: KeysConfig = toml::from_str("[text]\nx = \"preview_back\"\n").unwrap();
    let keys = AppKeys::new(&config, "vim");
    let mut state = FileLoopState::new(&cx, &keys);
    let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
    assert_eq!(
        run_file_with(
            &mut term,
            &mut Page { table: false },
            &mut [key('x')].into_iter(),
            &cx,
            &mut state,
            None
        )
        .unwrap(),
        FileLoopExit::Back
    );

    let config: KeysConfig = toml::from_str("[global]\nX = \"quit\"\n").unwrap();
    let keys = AppKeys::new(&config, "vim");
    for events in [vec![key('X')], vec![key('?'), key('X')]] {
        let mut state = FileLoopState::new(&cx, &keys);
        assert_eq!(
            drive(&mut Page { table: false }, &mut state, &cx, events, None).0,
            FileLoopExit::Quit
        );
    }
}

use crate::input::Origin;
use crate::view::{CanvasView, DocumentView, ImageData, ImageView, Layout, Reopen};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn drive(
    session: &mut dyn Session,
    state: &mut FileLoopState,
    cx: &AppCx,
    events: Vec<AppEvent>,
    pictures: Option<&mut Pictures>,
) -> (FileLoopExit, Terminal<TestBackend>) {
    let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
    let exit = run_file_with(
        &mut term,
        session,
        &mut events.into_iter(),
        cx,
        state,
        pictures,
    )
    .unwrap();
    (exit, term)
}

struct Typed {
    line: String,
    input: bool,
    views: usize,
}

impl Session for Typed {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
        self.views += 1;
        let mut c = CanvasView::new(req.cols, req.rows);
        c.text_input = self.input;
        c.set_string(0, 0, &self.line, crate::Style::new());
        c.redraw_after = Some(Duration::from_secs(2));
        Ok(View::Canvas(c))
    }
    fn key(&mut self, key: &Key, _: &ActCx) -> Result<Outcome, crate::Failure> {
        match key.code {
            KeyCode::Char(c) => self.line.push(c),
            KeyCode::Esc => self.input = false,
            _ => {}
        }
        Ok(Outcome::Redraw)
    }
}

struct ImagePages {
    image: ImageView,
    asked: Vec<usize>,
}

fn paged_layout() -> Layout {
    Layout {
        pages: Some(PageLayout::Separate {
            count: Some(3),
            titles: Vec::new(),
        }),
        ..Layout::single()
    }
}

impl Session for ImagePages {
    fn layout(&self) -> Layout {
        paged_layout()
    }
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
        self.asked.push(req.page.unwrap_or(0));
        Ok(View::Image(self.image.clone()))
    }
}

fn document(text: &str) -> DocumentView {
    DocumentView {
        markdown: text.into(),
        origin: Origin::Converted {
            from: "input.conv".into(),
        },
        resources: Default::default(),
        options: Default::default(),
        title: None,
        truncated: false,
    }
}

struct Converter(usize);
impl Session for Converter {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
        self.0 += 1;
        Ok(View::Reopen(Reopen {
            content_type: "markdown".into(),
            text: "converted".into(),
            resources: Default::default(),
            origin: Origin::Converted {
                from: "input.conv".into(),
            },
            truncated: false,
        }))
    }
}

struct Destination {
    opens: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<(usize, usize)>>>,
    fail: bool,
}

impl crate::Plugin for Destination {
    fn manifest(&self) -> crate::Manifest {
        crate::Manifest::new("destination", "0.0.1").shows(ViewKind::Document)
    }
    fn open(&self, _: crate::Input, _: &crate::OpenCx) -> Result<Box<dyn Session>, crate::Failure> {
        let id = self.opens.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail {
            return Err(crate::Failure::unsupported(crate::t!(en: "failed")));
        }
        Ok(Box::new(DestinationSession {
            id,
            views: 0,
            seen: self.seen.clone(),
        }))
    }
}

struct DestinationSession {
    id: usize,
    views: usize,
    seen: Arc<Mutex<Vec<(usize, usize)>>>,
}

impl Session for DestinationSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
        self.views += 1;
        self.seen.lock().unwrap().push((self.id, self.views));
        Ok(View::Document(document("destination")))
    }
}

#[test]
fn file_loop_resume_keeps_session_page_pose_and_typing() {
    let cx = AppCx::default();
    let keys = AppKeys::defaults();
    let mut state = FileLoopState::new(&cx, &keys);
    let mut typed = Typed {
        line: String::new(),
        input: true,
        views: 0,
    };
    let esc = AppEvent::Key(Key::new(KeyCode::Esc));
    assert_eq!(
        drive(
            &mut typed,
            &mut state,
            &cx,
            vec![key('a'), key('b'), esc, key('q')],
            None
        )
        .0,
        FileLoopExit::Back
    );
    assert_eq!(typed.line, "ab");
    assert!(state.st.input_left);
    let before = typed.views;
    assert!(state.view_due.is_some(), "the canvas armed its view timer");
    // A fixed future deadline makes the preservation assertion independent of test speed.
    let due = Instant::now() + MAX_REDRAW;
    state.view_due = Some(due);
    let (exit, term) = drive(&mut typed, &mut state, &cx, vec![], None);
    assert_eq!(exit, FileLoopExit::End);
    assert_eq!(
        typed.views,
        before + 1,
        "resume refreshes this same session once"
    );
    assert_eq!(typed.line, "ab");
    assert!(state.st.input_left, "resume keeps the input latch");
    assert_eq!(term.backend().buffer()[(0, 0)].symbol(), "a");
    assert_eq!(term.backend().buffer()[(1, 0)].symbol(), "b");
    assert_eq!(
        state.view_due,
        Some(due),
        "resume keeps the pending view deadline"
    );

    // The caller keeps the picture owner; real decode completion is fenced, without sleeps.
    let image = ImageView::new(ImageData::Rgba {
        width: 4,
        height: 4,
        pixels: Arc::new(vec![255; 64]),
    });
    let mut pictures = Pictures::new(
        ratatui_image::picker::Picker::halfblocks(),
        "test",
        std::path::Path::new("/fixture.pic"),
        800,
    )
    .with_out(Box::new(std::io::sink()));
    pictures.submit_image(&image, false);
    pictures.wait_still_decode_for_test();
    let mut pages = ImagePages {
        image,
        asked: Vec::new(),
    };
    let mut state = FileLoopState::new(&cx, &keys);
    assert_eq!(
        drive(
            &mut pages,
            &mut state,
            &cx,
            vec![key('J'), key('+'), key('l'), key('q')],
            Some(&mut pictures)
        )
        .0,
        FileLoopExit::Back
    );
    assert_eq!(state.st.at_page, 1);
    let pose = (pictures.still_zoom(), pictures.still_center());
    assert!(pose.0 > 1.0, "the image was zoomed");
    assert_ne!(pose.1, (0.5, 0.5), "the image was panned");
    let before = pages.asked.len();
    assert_eq!(
        drive(&mut pages, &mut state, &cx, vec![], Some(&mut pictures)).0,
        FileLoopExit::End
    );
    assert_eq!(
        &pages.asked[before..],
        &[1],
        "resume asks the retained page"
    );
    assert_eq!(
        (pictures.still_zoom(), pictures.still_center()),
        pose,
        "resume keeps image pose"
    );

    // A destination session is constructed only once, and keeps its own view count.
    for fail in [false, true] {
        let opens = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let plugin = Arc::new(Destination {
            opens: opens.clone(),
            seen: seen.clone(),
            fail,
        });
        let cx = AppCx {
            reopen: Some(super::super::ReopenSource::new(move |_| {
                Some(crate::reopen::ContentTarget {
                    plugin: plugin.clone(),
                    settings: Settings::empty().clone(),
                })
            })),
            ..AppCx::default()
        };
        let mut source = Converter(0);
        let mut state = FileLoopState::new(&cx, &keys);
        assert_eq!(
            drive(&mut source, &mut state, &cx, vec![key('q')], None).0,
            FileLoopExit::Back
        );
        assert_eq!(
            drive(&mut source, &mut state, &cx, vec![AppEvent::Resize], None).0,
            FileLoopExit::End
        );
        assert_eq!(source.0, 1, "resume must not return to the converter");
        assert_eq!(
            opens.load(Ordering::SeqCst),
            1,
            "resume never reopens the destination"
        );
        if fail {
            assert!(
                state.reopen_failed.is_some(),
                "the reopen failure stays remembered"
            );
        } else {
            assert_eq!(
                *seen.lock().unwrap(),
                vec![(1, 1), (1, 2), (1, 3)],
                "the swapped session retains its identity and state"
            );
        }
    }
}

#[test]
fn file_loop_resume_keeps_table_cursor_edit_line_options_and_launch_notice() {
    let cx = AppCx::default();
    let keys = AppKeys::defaults();
    let mut state = FileLoopState::new(&cx, &keys);
    let mut text = Page { table: false };
    drive(
        &mut text,
        &mut state,
        &cx,
        vec![key(','), key('w'), key('j'), key('l'), key('q')],
        None,
    );
    assert!(!state.st.display.wrap);
    assert!(!state.own.wrap);
    assert_eq!((state.st.caret_line, state.st.caret_col), (1, 1));
    assert_eq!(
        super::super::edit_line::edit_line(),
        Some(2),
        "leaving preserves the edit line"
    );
    drive(&mut text, &mut state, &cx, vec![], None);
    assert!(
        !state.own.wrap,
        "the loop's own keys keep the changed options"
    );
    assert_eq!((state.st.caret_line, state.st.caret_col), (1, 1));
    assert_eq!(super::super::edit_line::edit_line(), Some(2));

    // A worker can answer while the file screen is away: its original receiver stays alive.
    let tx = state.launch_tx.clone();
    state.launching = 1;
    tx.send(Err("test launch notice".into())).unwrap();
    drive(&mut text, &mut state, &cx, vec![], None);
    assert_eq!(state.launching, 0);
    assert!(state
        .st
        .flash
        .as_deref()
        .is_some_and(|s| s.contains("test launch notice")));

    let mut state = FileLoopState::new(&cx, &keys);
    let mut table = Page { table: true };
    drive(
        &mut table,
        &mut state,
        &cx,
        vec![key('J'), key('j'), key('l'), key('q')],
        None,
    );
    let Frame::Table(before) = &state.base else {
        panic!("table on screen")
    };
    assert_eq!(state.st.at_page, 1);
    let cursor = before.state;
    assert_eq!((cursor.cursor_row, cursor.cursor_col), (1, 1));
    drive(&mut table, &mut state, &cx, vec![], None);
    let Frame::Table(after) = &state.base else {
        panic!("table on screen")
    };
    assert_eq!(state.st.at_page, 1);
    assert_eq!(
        after.state, cursor,
        "resume keeps the table cursor and scroll"
    );
}
