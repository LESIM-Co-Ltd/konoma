//! The loop: ask the session for its view, draw it, wait for a key.

use std::time::Duration;

use konoma_host_core::keymap::{
    scheme_from_str, CoreAction, CoreContext, CoreKeys, CoreResolution, KeyPress, Motion,
};
use konoma_host_core::keys::KeysConfig;
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

/// The keys of the app: layers 0 to 3 of the key design, shared with the host
/// (`konoma_host_core::keymap`), with the user's `[keys]` overrides applied.
///
/// The app shows one text at a time, so it answers only what a text needs: moving and paging
/// (`j` `k` `g` `G`, `Ctrl-f` `Ctrl-b` `Ctrl-d` `Ctrl-u` or `f` `b` `d` `u` by `ui.keys`,
/// `PageUp` `PageDown`) and leaving (`q`, `Q`). The arrows, `Home` and `End` are fixed keys.
/// Every other command of the shared tables is ignored here until the app can do it.
#[derive(Clone, Debug)]
pub struct AppKeys {
    keys: CoreKeys,
}

impl AppKeys {
    /// The keys for the `[keys]` table `keys` and the `ui.keys` paging style (`"vim"` | `"less"`).
    pub fn new(keys: &KeysConfig, ui_keys: &str) -> AppKeys {
        AppKeys {
            keys: CoreKeys::from_keys_config(scheme_from_str(ui_keys), keys),
        }
    }

    /// The built-in keys (no overrides, vim paging).
    pub fn defaults() -> AppKeys {
        AppKeys::new(&KeysConfig::default(), "vim")
    }

    /// What the app does for `key` (`None` = nothing).
    fn act(&self, key: &Key) -> Option<Act> {
        if key.mods == KeyMods::CTRL && matches!(key.code, KeyCode::Char('c')) {
            return Some(Act::Quit);
        }
        // The fixed keys: not in the keymap, so not rebindable.
        if key.mods.is_empty() {
            match key.code {
                KeyCode::Down => return Some(Act::Move(Motion::Down)),
                KeyCode::Up => return Some(Act::Move(Motion::Up)),
                KeyCode::Home => return Some(Act::Move(Motion::Top)),
                KeyCode::End => return Some(Act::Move(Motion::Bottom)),
                _ => {}
            }
        }
        let kp = key_press(key)?;
        match self.keys.resolve(CoreContext::Text, kp) {
            CoreResolution::Action(CoreAction::Quit | CoreAction::PreviewBack) => Some(Act::Quit),
            CoreResolution::Action(CoreAction::Navigate(m)) => Some(Act::Move(m)),
            _ => None,
        }
    }
}

/// What a key does in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Quit,
    Move(Motion),
}

/// The key as the shared tables name it. Alt and the keys that are not part of the shared
/// tables (function keys, a paste) have none; Shift is already in the character.
fn key_press(key: &Key) -> Option<KeyPress> {
    use konoma_host_core::keymap::KeyPress as P;
    if key.mods.contains(KeyMods::ALT) {
        return None;
    }
    let ctrl = key.mods.contains(KeyMods::CTRL);
    let code = match &key.code {
        KeyCode::Char(c) => crossterm::event::KeyCode::Char(*c),
        KeyCode::Enter => crossterm::event::KeyCode::Enter,
        KeyCode::Backspace => crossterm::event::KeyCode::Backspace,
        KeyCode::Delete => crossterm::event::KeyCode::Delete,
        KeyCode::Tab => crossterm::event::KeyCode::Tab,
        KeyCode::BackTab => crossterm::event::KeyCode::BackTab,
        KeyCode::Left => crossterm::event::KeyCode::Left,
        KeyCode::Right => crossterm::event::KeyCode::Right,
        KeyCode::Up => crossterm::event::KeyCode::Up,
        KeyCode::Down => crossterm::event::KeyCode::Down,
        KeyCode::Home => crossterm::event::KeyCode::Home,
        KeyCode::End => crossterm::event::KeyCode::End,
        KeyCode::PageUp => crossterm::event::KeyCode::PageUp,
        KeyCode::PageDown => crossterm::event::KeyCode::PageDown,
        KeyCode::Esc => crossterm::event::KeyCode::Esc,
        _ => return None,
    };
    Some(P { code, ctrl })
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

/// Run the app on `terminal` until the key of leaving (`q` by default, or `Ctrl-C`, or the end of
/// `events`). `keys` says what each key does.
pub fn run_with<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    session: &mut dyn Session,
    events: &mut E,
    cx: &AppCx,
    keys: &AppKeys,
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
            Next::Event(AppEvent::Key(key)) => match keys.act(&key) {
                Some(Act::Quit) => return Ok(()),
                Some(Act::Move(m)) => scroll_by(m, &mut st),
                None => {}
            },
        }
    }
}

fn scroll_by(m: Motion, st: &mut State) {
    let page = st.page;
    let half = (page / 2).max(1);
    match m {
        Motion::Down => st.scroll = st.scroll.saturating_add(1),
        Motion::Up => st.scroll = st.scroll.saturating_sub(1),
        Motion::PageDown => st.scroll = st.scroll.saturating_add(page),
        Motion::PageUp => st.scroll = st.scroll.saturating_sub(page),
        Motion::HalfDown => st.scroll = st.scroll.saturating_add(half),
        Motion::HalfUp => st.scroll = st.scroll.saturating_sub(half),
        Motion::Top => st.scroll = 0,
        Motion::Bottom => st.scroll = st.total,
        // The app does not scroll sideways yet.
        Motion::Left | Motion::Right | Motion::LineHome | Motion::LineEnd => {}
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
