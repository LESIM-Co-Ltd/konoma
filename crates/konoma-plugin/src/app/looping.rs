//! The loop: ask the session for its view, draw it, wait for a key.

use std::time::{Duration, Instant};

use konoma_host_core::help::{
    core_sections, ctrl_c_token, help_title, pad_cells, render_foot, scroll_keys, CoreFoot,
    CoreLookup, CoreSection, CoreToken, HelpFacts, HelpOp, KeyLookup,
};
use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::keymap::{
    scheme_from_str, CoreAction, CoreContext, CoreKeys, CoreResolution, KeyPress, Motion,
};
use konoma_host_core::keys::KeysConfig;
use konoma_host_core::lang::Lang;
use ratatui::backend::Backend;
use ratatui::Terminal;

use super::draw::{self, Frame};
use super::pictures::Pictures;
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
    lang: Lang,
}

impl AppKeys {
    /// The keys for the `[keys]` table `keys` and the `ui.keys` paging style (`"vim"` | `"less"`).
    pub fn new(keys: &KeysConfig, ui_keys: &str) -> AppKeys {
        AppKeys {
            keys: CoreKeys::from_keys_config(scheme_from_str(ui_keys), keys),
            lang: Lang::En,
        }
    }

    /// The same keys with the help and the footer in `lang`.
    pub fn with_lang(mut self, lang: Lang) -> AppKeys {
        self.lang = lang;
        self
    }

    /// The language of the help and the footer.
    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// What the help and the footer say about the keys of a text: every operation of the shared
    /// layers the app does not act on is off (its row and token are left out), and the two
    /// leaving keys are named as the app leaves (it has no tree to go back to).
    pub fn facts(&self) -> HelpFacts {
        let acted = |op: &HelpOp| match op {
            HelpOp::Act(CoreAction::Navigate(m)) => matches!(
                m,
                Motion::Down
                    | Motion::Up
                    | Motion::Top
                    | Motion::Bottom
                    | Motion::PageDown
                    | Motion::PageUp
                    | Motion::HalfDown
                    | Motion::HalfUp
            ),
            HelpOp::Act(CoreAction::Quit | CoreAction::PreviewBack | CoreAction::ToggleHelp) => {
                true
            }
            HelpOp::Cancel => true,
            _ => false,
        };
        let mut off: Vec<HelpOp> = Vec::new();
        for row in konoma_host_core::help::rows_of(CoreContext::Text) {
            for op in row.ops() {
                if !acted(op) && !off.contains(op) {
                    off.push(*op);
                }
            }
        }
        off.push(HelpOp::Menu(konoma_host_core::keymap::CoreMenu::Options));
        HelpFacts {
            off,
            label: vec![(CoreAction::PreviewBack, Some(CoreMsg::HintQuit))],
            help: vec![
                (CoreAction::PreviewBack, Some(CoreMsg::QuitApp)),
                (CoreAction::Quit, Some(CoreMsg::QuitApp)),
            ],
        }
    }

    /// The footer's key hints: what leaves (`q` or else `Q` or else `Ctrl-C`, whichever really
    /// quits), what scrolls (only when the frame on screen scrolls: `scrolls`), what opens the
    /// help. A hint is there only when its key acts.
    pub fn footer_hints(&self, scrolls: bool) -> Vec<CoreToken> {
        let facts = self.facts();
        let look = CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Text,
        };
        let tok = |f: CoreFoot| render_foot(f, CoreContext::Text, &facts, &look, self.lang);
        let leave = tok(CoreFoot::Back)
            .or_else(|| tok(CoreFoot::Quit))
            .unwrap_or_else(|| ctrl_c_token(self.lang));
        let mut out = vec![leave];
        if scrolls {
            out.extend(tok(CoreFoot::Scroll));
        }
        out.extend(tok(CoreFoot::Help));
        out
    }

    /// [`AppKeys::footer_hints`] as the text the footer shows.
    pub fn footer_tokens(&self, scrolls: bool) -> Vec<String> {
        self.footer_hints(scrolls)
            .into_iter()
            .map(|t| t.text)
            .collect()
    }

    /// Whether `key` does something: in the `?` help (`in_help`) or on the screen of a text
    /// (`scrolls`: the frame scrolls) or of anything else (a message, a canvas: the motions do
    /// nothing there). What the hints and the help rows name must pass this.
    pub fn acts_on(&self, key: &Key, in_help: bool, scrolls: bool) -> bool {
        match self.act(key) {
            None => false,
            Some(Act::Move(_)) => in_help || scrolls,
            Some(_) => true,
        }
    }

    /// The sections of the `?` help.
    pub fn help_sections(&self) -> Vec<CoreSection> {
        let look = CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Text,
        };
        core_sections(CoreContext::Text, &self.facts(), &look, self.lang)
    }

    /// The `?` help as lines: a heading and the aligned rows of each section.
    fn help_lines(&self) -> Vec<Line> {
        let secs = self.help_sections();
        let w = secs
            .iter()
            .flat_map(|s| s.rows.iter())
            .map(|(k, _)| unicode_width::UnicodeWidthStr::width(k.as_str()))
            .max()
            .unwrap_or(0);
        let mut out = Vec::new();
        for (i, s) in secs.iter().enumerate() {
            if i > 0 {
                out.push(Line::raw(""));
            }
            out.push(Line::raw(s.title.clone()));
            for (k, d) in &s.rows {
                out.push(Line::raw(format!("  {}  {d}", pad_cells(k, w))));
            }
        }
        out
    }

    /// The title of the `?` help, shown in the footer while it is open.
    ///
    /// Named from the actual bindings: the keys that toggle the help (the fixed `Esc` is added),
    /// and the keys that scroll it, left out when none does. The leaving key `q` closes it too
    /// and is not named, as in the host.
    pub fn help_title(&self) -> String {
        let facts = self.facts();
        let look = CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Text,
        };
        let close = look.keys_of(HelpOp::Act(CoreAction::ToggleHelp));
        help_title(&close, &scroll_keys(&look, &facts), self.lang)
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
                // `Esc` is fixed (not in the keymap) and the help says `q / Esc` leaves, so it
                // does: it closes the help, and out of the help it ends the app.
                KeyCode::Esc => return Some(Act::Back),
                _ => {}
            }
        }
        let kp = key_press(key)?;
        match self.keys.resolve(CoreContext::Text, kp) {
            CoreResolution::Action(CoreAction::Quit) => Some(Act::Quit),
            CoreResolution::Action(CoreAction::PreviewBack) => Some(Act::Back),
            CoreResolution::Action(CoreAction::Navigate(m)) => Some(Act::Move(m)),
            CoreResolution::Action(CoreAction::ToggleHelp) => Some(Act::Help),
            _ => None,
        }
    }
}

/// What a key does in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// Ends the app, even from the help (`Q`, `Ctrl-C`): the host's `Q` quits with its help open
    /// too.
    Quit,
    /// Leaves: closes the help when it is open, else ends the app (`q`, `Esc`); in the host `q`
    /// and `Esc` close the help.
    Back,
    Move(Motion),
    Help,
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
    /// The `?` help while it is open: the first line shown.
    help: Option<usize>,
    /// Whether the frame on screen scrolls (a text does; a message and a canvas do not).
    scrolls: bool,
}

/// Run the app on `terminal` until the key of leaving (`q` by default, or `Ctrl-C`, or the end of
/// `events`). `keys` says what each key does. A canvas shows its cells only: see
/// [`run_with_pictures`] for the pictures a canvas places.
pub fn run_with<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    session: &mut dyn Session,
    events: &mut E,
    cx: &AppCx,
    keys: &AppKeys,
) -> Result<(), AppError> {
    run_with_pictures(terminal, session, events, cx, keys, None)
}

/// [`run_with`] with the pictures a canvas places (`pictures`: `None` is a terminal that cannot
/// show any, where the canvas shows its cells and nothing is decoded).
///
/// Every wait is bounded: while a decode or an encode is out, or a GIF plays, the loop wakes
/// every few milliseconds to draw what the workers finished. Such a wake does not ask the
/// session for a view again (it would not have changed): only a key, a resize and the view's
/// own redraw timer do. Leaving drops `pictures`, which cancels the decode in flight and
/// deletes every kitty image placed.
pub fn run_with_pictures<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    session: &mut dyn Session,
    events: &mut E,
    cx: &AppCx,
    keys: &AppKeys,
    mut pictures: Option<Pictures>,
) -> Result<(), AppError> {
    let mut st = State::default();
    let terr = |e: &dyn std::fmt::Display| AppError::Terminal(e.to_string());
    // The frame of the last view, and when that view is due again (a wake for the pictures
    // draws this frame again).
    let mut base = Frame::Lines(Vec::new());
    let mut view_due: Option<Instant> = None;
    let mut need_view = true;
    loop {
        let size = terminal.size().map_err(|e| terr(&e))?;
        let body_rows = size.height.saturating_sub(1);
        st.page = usize::from(body_rows).max(1);
        if view_due.is_some_and(|d| Instant::now() >= d) {
            need_view = true;
        }
        let mut view_wait = None;
        if need_view {
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
            if let Some(p) = pictures.as_mut() {
                match &frame {
                    Frame::Canvas(c) => p.submit(c),
                    _ => p.forget(),
                }
            }
            view_wait = redraw_after.map(|d| d.clamp(MIN_REDRAW, MAX_REDRAW));
            view_due = view_wait.map(|d| Instant::now() + d);
            base = frame;
        }
        if let Some(p) = pictures.as_mut() {
            p.poll();
        }
        let base_frame = &base;
        let help_frame;
        let (frame, footer) = match st.help {
            Some(top) => {
                let lines = keys.help_lines();
                let rows = usize::from(body_rows);
                let top = top.min(lines.len().saturating_sub(rows));
                st.help = Some(top);
                help_frame = Frame::Lines(lines.into_iter().skip(top).take(rows).collect());
                (&help_frame, format!(" {}", keys.help_title().trim()))
            }
            None => (
                base_frame,
                draw::footer(
                    &keys.footer_tokens(st.scrolls),
                    &cx.title,
                    base_frame,
                    st.scroll,
                    st.scrolls,
                ),
            ),
        };
        draw_frame(terminal, frame, &footer, pictures.as_mut())?;

        // How long to wait: the view's own timer, or sooner for the pictures. A timeout that
        // came from the pictures' shorter wake leaves the view as it is.
        let view_left = match view_wait {
            Some(w) => Some(w),
            None => view_due.map(|d| d.saturating_duration_since(Instant::now())),
        };
        let pic_wait = pictures.as_ref().and_then(Pictures::wake_in);
        let picture_first = match (view_left, pic_wait) {
            (Some(v), Some(p)) => p < v,
            (None, Some(_)) => true,
            _ => false,
        };
        let wait = if picture_first { pic_wait } else { view_left };
        // A key and a resize ask for a new view; a timeout does unless the pictures woke the loop.
        need_view = true;
        match events.next(wait) {
            Next::End => return Ok(()),
            Next::Timeout => need_view = !picture_first,
            Next::Event(AppEvent::Resize) => need_view = true,
            Next::Event(AppEvent::Key(key)) => match (keys.act(&key), st.help) {
                // `Q` and Ctrl-C end the app even from the help (the host's `Q` does); `q` and
                // `Esc` close it.
                (Some(Act::Back), Some(_)) => st.help = None,
                (Some(Act::Quit | Act::Back), _) => return Ok(()),
                (Some(Act::Help), Some(_)) => st.help = None,
                (Some(Act::Help), None) => st.help = Some(0),
                (Some(Act::Move(m)), Some(top)) => {
                    st.help = Some(match m {
                        Motion::Down => top.saturating_add(1),
                        Motion::Up => top.saturating_sub(1),
                        Motion::Top => 0,
                        Motion::Bottom => usize::MAX / 2,
                        Motion::PageDown => top.saturating_add(st.page),
                        Motion::PageUp => top.saturating_sub(st.page),
                        Motion::HalfDown => top.saturating_add((st.page / 2).max(1)),
                        Motion::HalfUp => top.saturating_sub((st.page / 2).max(1)),
                        _ => top,
                    })
                }
                (Some(Act::Move(m)), None) if st.scrolls => scroll_by(m, &mut st),
                (Some(Act::Move(_)), None) => {}
                (None, _) => {}
            },
        }
    }
}

/// Draw `frame` and `footer`, then the pictures over it. A frame on which a picture moved or went
/// is cleared and drawn again (the cells of its old place would stay on screen otherwise), and the
/// deletes of the kitty images that went are written after it. The right to write to the terminal
/// is held for all of it, so an exit's cleanup never lands between the chunks of a transmit.
fn draw_frame<B: Backend>(
    terminal: &mut Terminal<B>,
    frame: &Frame,
    footer: &str,
    mut pictures: Option<&mut Pictures>,
) -> Result<(), AppError> {
    let terr = |e: &dyn std::fmt::Display| AppError::Terminal(e.to_string());
    let on_canvas = matches!(frame, Frame::Canvas(_));
    let _writing = konoma_host_core::term::terminal_write();
    for pass in 0..2 {
        terminal
            .draw(|f| {
                draw::draw(f, frame, footer);
                if let Some(p) = pictures.as_deref_mut() {
                    let body = draw::body_of(f.area());
                    p.draw(f.buffer_mut(), body, on_canvas);
                }
            })
            .map_err(|e| terr(&e))?;
        let moved = pictures.as_deref_mut().is_some_and(Pictures::take_moved);
        if pass == 0 && moved {
            terminal.clear().map_err(|e| terr(&e))?;
        } else {
            break;
        }
    }
    if let Some(p) = pictures {
        p.flush_deletes();
    }
    Ok(())
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
    st.scrolls = false;
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
            st.scrolls = true;
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
