//! The loop: ask the session for its view, draw it, wait for a key.

use std::time::Duration;

use ratatui::backend::Backend;
use ratatui::Terminal;

use super::draw::{self, Frame};
use super::window;
use super::AppError;
use crate::key::{Key, KeyCode, KeyMods};
use crate::session::{Session, TermCaps, ViewCx};
use crate::view::{View, ViewReq};
use crate::{Line, Settings};

/// What the app tells the loop about its surroundings.
#[derive(Clone, Debug)]
pub struct AppCx {
    pub lang: String,
    pub term: TermCaps,
    /// Shown in the footer.
    pub title: String,
}

/// An event the loop reacts to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEvent {
    Key(Key),
    /// The terminal was resized: draw again at the new size.
    Resize,
}

/// What an [`EventSource`] answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    Event(AppEvent),
    /// Nothing came within the wait.
    Timeout,
    /// No more events: the loop ends (the input is closed, or a test's keys are used up).
    End,
}

/// Where the loop gets its events. Any iterator of [`AppEvent`] is one (it never times out), so
/// a test passes a list of keys.
pub trait EventSource {
    /// The next event; `wait` is how long to wait for it (`None` = as long as it takes).
    fn next(&mut self, wait: Option<Duration>) -> Next;
}

impl<I: Iterator<Item = AppEvent>> EventSource for I {
    fn next(&mut self, _wait: Option<Duration>) -> Next {
        match Iterator::next(self) {
            Some(e) => Next::Event(e),
            None => Next::End,
        }
    }
}

/// The shortest wait the loop asks for between two draws of an animation (33 ms, the same floor
/// the host applies to `redraw_after`).
const MIN_REDRAW: Duration = Duration::from_millis(33);
/// The longest (24 hours, as the host).
const MAX_REDRAW: Duration = Duration::from_secs(24 * 60 * 60);

/// What the loop keeps between two frames.
#[derive(Default)]
struct State {
    /// The first line shown of a text.
    scroll: usize,
    /// The number of lines of a text file, counted once.
    counted: Option<(std::path::PathBuf, usize)>,
    /// The height of the body at the last draw (the page of `PageUp` / `PageDown`).
    page: usize,
    /// The last number of lines a text had (the bound of `End`).
    total: usize,
}

/// Run the app on `terminal` until `q` (or `Ctrl-C`, or the end of `events`).
pub fn run_with<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    session: &mut dyn Session,
    events: &mut E,
    cx: &AppCx,
) -> Result<(), AppError> {
    let mut st = State::default();
    let terr = |e: &dyn std::fmt::Display| AppError::Terminal(e.to_string());
    loop {
        let size = terminal.size().map_err(|e| terr(&e))?;
        let body_rows = size.height.saturating_sub(1);
        st.page = usize::from(body_rows).max(1);
        let req = ViewReq::new(size.width, body_rows);
        let vcx = ViewCx::new(Settings::empty(), &cx.lang).with_term(cx.term);
        let view = match session.view(&req, &vcx) {
            Ok(v) => v,
            Err(f) => View::Message(crate::view::MessageView {
                lines: vec![Line::raw(format!(
                    "[can not preview: {}]",
                    f.message.get(&cx.lang)
                ))],
            }),
        };
        let (frame, redraw_after) = frame_of(view, &mut st, usize::from(body_rows));
        let footer = draw::footer(&cx.title, &frame, st.scroll);
        terminal
            .draw(|f| draw::draw(f, &frame, &footer))
            .map_err(|e| terr(&e))?;

        let wait = redraw_after.map(|d| d.clamp(MIN_REDRAW, MAX_REDRAW));
        match events.next(wait) {
            Next::End => return Ok(()),
            Next::Timeout | Next::Event(AppEvent::Resize) => {}
            Next::Event(AppEvent::Key(key)) => {
                if is_quit(&key) {
                    return Ok(());
                }
                scroll_by_key(&key, &mut st);
            }
        }
    }
}

fn is_quit(key: &Key) -> bool {
    match (&key.code, key.mods) {
        (KeyCode::Char('q'), m) => m.is_empty(),
        (KeyCode::Char('c'), m) => m == KeyMods::CTRL,
        _ => false,
    }
}

fn scroll_by_key(key: &Key, st: &mut State) {
    if !key.mods.is_empty() {
        return;
    }
    let page = st.page;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => st.scroll = st.scroll.saturating_add(1),
        KeyCode::Char('k') | KeyCode::Up => st.scroll = st.scroll.saturating_sub(1),
        KeyCode::PageDown | KeyCode::Char(' ') => st.scroll = st.scroll.saturating_add(page),
        KeyCode::PageUp => st.scroll = st.scroll.saturating_sub(page),
        KeyCode::Home | KeyCode::Char('g') => st.scroll = 0,
        KeyCode::End | KeyCode::Char('G') => st.scroll = st.total,
        _ => {}
    }
}

/// Turn a view into what is drawn, clamping the scroll to the text's length.
fn frame_of(view: View, st: &mut State, rows: usize) -> (Frame, Option<Duration>) {
    use crate::view::TextSource;
    match view {
        View::Message(m) => (Frame::Lines(m.lines), None),
        View::Canvas(c) => {
            let after = c.redraw_after;
            (Frame::Canvas(c), after)
        }
        View::Text(t) => {
            let frame = match t.source {
                TextSource::Owned(s) => {
                    let lines: Vec<&str> = s.lines().collect();
                    clamp(st, lines.len(), rows);
                    Frame::Lines(
                        lines
                            .iter()
                            .skip(st.scroll)
                            .take(rows)
                            .map(|l| Line::raw(window::clean_line(l.as_bytes())))
                            .collect(),
                    )
                }
                TextSource::Lines(all) => {
                    clamp(st, all.len(), rows);
                    Frame::Lines(all.into_iter().skip(st.scroll).take(rows).collect())
                }
                TextSource::File(path) => {
                    let total = match &st.counted {
                        Some((p, n)) if *p == path => *n,
                        _ => match window::count_lines(&path) {
                            Ok(n) => {
                                st.counted = Some((path.clone(), n));
                                n
                            }
                            Err(e) => {
                                return (
                                    Frame::Lines(vec![Line::raw(format!(
                                        "[can not read {}: {e}]",
                                        path.display()
                                    ))]),
                                    None,
                                )
                            }
                        },
                    };
                    clamp(st, total, rows);
                    match window::read_window(&path, st.scroll, rows) {
                        Ok(w) => Frame::Lines(w.into_iter().map(Line::raw).collect()),
                        Err(e) => Frame::Lines(vec![Line::raw(format!(
                            "[can not read {}: {e}]",
                            path.display()
                        ))]),
                    }
                }
            };
            (frame, None)
        }
        other => (
            Frame::Lines(vec![Line::raw(format!(
                "[not displayable yet: {}]",
                kind_name(&other)
            ))]),
            None,
        ),
    }
}

/// Keep the last page full: the scroll is at most `total - rows`.
fn clamp(st: &mut State, total: usize, rows: usize) {
    st.total = total.saturating_sub(rows);
    st.scroll = st.scroll.min(st.total);
}

fn kind_name(v: &View) -> &'static str {
    match v {
        View::Text(_) => "text",
        View::Document(_) => "document",
        View::Table(_) => "table",
        View::Image(_) => "image",
        View::List(_) => "list",
        View::Diff(_) => "diff",
        View::Canvas(_) => "canvas",
        View::Reopen(_) => "document",
        View::Message(_) => "message",
    }
}
