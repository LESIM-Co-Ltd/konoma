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

use konoma_widgets::table::{popup_page_step, popup_scroll_by, popup_scroll_to, TableStep};

use super::draw::{self, Frame, PopupState, TableFrame};
use super::pictures::Pictures;
use super::window;
use super::AppError;
use crate::key::{Key, KeyCode, KeyMods};
use crate::session::{Session, TermCaps, ViewCx};
use crate::view::{PageLayout, View, ViewReq};
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
///
/// A table is moved by the keys of the shared table context (`h j k l`, `g G`, `0 $`, paging,
/// `J K` between sheets) and the fixed arrows, `Home`, `End`; `Enter` opens the full-cell popup.
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
        lines_of(&self.help_sections())
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

    // ---- Tables ----

    /// What the help and the footer say about a table: what the app does not act on is off.
    fn table_facts(&self, mode: TableMode) -> HelpFacts {
        let acted = |op: &HelpOp| match op {
            HelpOp::Act(CoreAction::Navigate(_)) => true,
            HelpOp::Act(CoreAction::PageNext | CoreAction::PagePrev) => mode.paged,
            HelpOp::Act(CoreAction::Quit | CoreAction::PreviewBack | CoreAction::ToggleHelp) => {
                true
            }
            HelpOp::Cancel => true,
            _ => false,
        };
        let mut off: Vec<HelpOp> = Vec::new();
        for row in konoma_host_core::help::rows_of(CoreContext::Table) {
            for op in row.ops() {
                if !acted(op) && !off.contains(op) {
                    off.push(*op);
                }
            }
        }
        off.push(HelpOp::Menu(konoma_host_core::keymap::CoreMenu::Options));
        HelpFacts {
            off,
            label: {
                let mut v = vec![(CoreAction::PreviewBack, Some(CoreMsg::HintQuit))];
                if mode.sheets {
                    v.push((CoreAction::PageNext, Some(CoreMsg::HintSheet)));
                    v.push((CoreAction::PagePrev, Some(CoreMsg::HintSheet)));
                }
                v
            },
            help: {
                let mut v = vec![
                    (CoreAction::PreviewBack, Some(CoreMsg::QuitApp)),
                    (CoreAction::Quit, Some(CoreMsg::QuitApp)),
                ];
                if mode.sheets {
                    v.push((CoreAction::PageNext, Some(CoreMsg::SheetSwitchHelp)));
                    v.push((CoreAction::PagePrev, Some(CoreMsg::SheetSwitchHelp)));
                }
                v
            },
        }
    }

    fn table_lookup(&self) -> CoreLookup<'_> {
        CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Table,
        }
    }

    /// The footer's key hints of a table: the cell cursor, the cell popup (`Enter`), the sheets,
    /// the ends, what leaves and what opens the help. A hint is there only when its key acts
    /// (`J` `K` only with several sheets, `Enter` only when there is a cell).
    pub fn table_footer_hints(&self, mode: TableMode) -> Vec<CoreToken> {
        let facts = self.table_facts(mode);
        let look = self.table_lookup();
        let tok = |f: CoreFoot| render_foot(f, CoreContext::Table, &facts, &look, self.lang);
        let mut out: Vec<CoreToken> = Vec::new();
        if mode.popup {
            out.extend(self.popup_tokens());
            return out;
        }
        out.extend(tok(CoreFoot::Cell));
        if mode.cells {
            out.push(self.enter_token());
        }
        out.extend(tok(CoreFoot::PageTurn));
        out.extend(tok(CoreFoot::Ends));
        out.push(
            tok(CoreFoot::Back)
                .or_else(|| tok(CoreFoot::Quit))
                .unwrap_or_else(|| ctrl_c_token(self.lang)),
        );
        out.extend(tok(CoreFoot::Help));
        out
    }

    /// [`AppKeys::table_footer_hints`] as the text the footer shows.
    pub fn table_footer_tokens(&self, mode: TableMode) -> Vec<String> {
        self.table_footer_hints(mode)
            .into_iter()
            .map(|t| t.text)
            .collect()
    }

    /// `Enter:view cell`: the fixed key that opens the popup.
    fn enter_token(&self) -> CoreToken {
        let key = KeyPress::key(crossterm::event::KeyCode::Enter);
        CoreToken {
            text: format!(
                "{}:{}",
                konoma_host_core::help::write_keys(&[key], true),
                CoreMsg::HintViewCell.tr(self.lang)
            ),
            keys: vec![key],
            ops: Vec::new(),
        }
    }

    /// The hints of the full-cell popup: scrolling, the ends, closing (the leave keys, `Enter` and `Esc`).
    pub fn popup_tokens(&self) -> Vec<CoreToken> {
        use konoma_host_core::help::write_keys;
        let facts = self.table_facts(TableMode {
            popup: true,
            ..TableMode::default()
        });
        let look = self.table_lookup();
        let mut out: Vec<CoreToken> = Vec::new();
        out.extend(render_foot(
            CoreFoot::Scroll,
            CoreContext::Table,
            &facts,
            &look,
            self.lang,
        ));
        if let Some(t) = render_foot(CoreFoot::Ends, CoreContext::Table, &facts, &look, self.lang) {
            out.push(CoreToken {
                text: format!(
                    "{}:{}",
                    write_keys(&t.keys, true),
                    CoreMsg::FtTopBottom.tr(self.lang)
                ),
                keys: t.keys,
                ops: t.ops,
            });
        }
        // What closes the popup: the keys that leave (`q`), and the fixed `Enter` and `Esc`.
        let mut close = look.keys_of(HelpOp::Act(CoreAction::PreviewBack));
        for k in [
            KeyPress::key(crossterm::event::KeyCode::Enter),
            KeyPress::key(crossterm::event::KeyCode::Esc),
        ] {
            if !close.contains(&k) {
                close.push(k);
            }
        }
        out.push(CoreToken {
            text: format!(
                "{}:{}",
                write_keys(&close, false),
                CoreMsg::FtClose.tr(self.lang)
            ),
            keys: close,
            ops: Vec::new(),
        });
        out
    }

    /// The sections of the `?` help of a table, with the fixed `Enter` row.
    ///
    /// The help describes the table under it: with the popup open it still lists the keys of the
    /// table (the host's does), so the rows are the ones of the table with no popup.
    pub fn table_help_sections(&self, mode: TableMode) -> Vec<CoreSection> {
        let mode = TableMode {
            popup: false,
            ..mode
        };
        let look = self.table_lookup();
        let mut secs = core_sections(
            CoreContext::Table,
            &self.table_facts(mode),
            &look,
            self.lang,
        );
        if mode.cells {
            let row = (
                konoma_host_core::help::help_key(KeyPress::key(crossterm::event::KeyCode::Enter)),
                CoreMsg::TableCellViewHelp.tr(self.lang).to_string(),
            );
            let view_title = konoma_host_core::help::view_title(CoreContext::Table).tr(self.lang);
            match secs.iter_mut().find(|s| s.title == view_title) {
                Some(s) => s.rows.push(row),
                None => secs.insert(
                    0,
                    CoreSection {
                        title: view_title.to_string(),
                        rows: vec![row],
                    },
                ),
            }
        }
        secs
    }

    /// The `?` help of a table as lines.
    fn table_help_lines(&self, mode: TableMode) -> Vec<Line> {
        lines_of(&self.table_help_sections(mode))
    }

    /// The title of the `?` help over a table (see [`AppKeys::help_title`]).
    pub fn table_help_title(&self, mode: TableMode) -> String {
        let facts = self.table_facts(mode);
        let look = self.table_lookup();
        let close = look.keys_of(HelpOp::Act(CoreAction::ToggleHelp));
        help_title(&close, &scroll_keys(&look, &facts), self.lang)
    }

    /// What the app does for `key` over a table (`None` = nothing). `popup`: the full-cell popup
    /// is open, where the keys are the popup's own: the vertical motions scroll it, `PageUp`
    /// and `PageDown` page it, `Enter` / `Esc` / `q` close it; the others do nothing.
    fn table_act(&self, key: &Key, popup: bool) -> Option<Act> {
        if key.mods == KeyMods::CTRL && matches!(key.code, KeyCode::Char('c')) {
            return Some(Act::Quit);
        }
        let allowed = |m: Motion, literal_page: bool| match m {
            Motion::Down | Motion::Up | Motion::Top | Motion::Bottom => true,
            Motion::PageDown | Motion::PageUp => !popup || literal_page,
            _ => !popup,
        };
        if key.mods.is_empty() {
            let fixed = match key.code {
                KeyCode::Down => Some(Motion::Down),
                KeyCode::Up => Some(Motion::Up),
                KeyCode::Left => Some(Motion::Left),
                KeyCode::Right => Some(Motion::Right),
                KeyCode::Home => Some(Motion::Top),
                KeyCode::End => Some(Motion::Bottom),
                _ => None,
            };
            if let Some(m) = fixed {
                return allowed(m, false).then_some(Act::Move(m));
            }
            match key.code {
                KeyCode::Esc => return Some(Act::Back),
                KeyCode::Enter => return Some(Act::Cell),
                _ => {}
            }
        }
        let kp = key_press(key)?;
        let literal_page =
            key.mods.is_empty() && matches!(key.code, KeyCode::PageDown | KeyCode::PageUp);
        match self.keys.resolve(CoreContext::Table, kp) {
            CoreResolution::Action(CoreAction::Quit) => Some(Act::Quit),
            CoreResolution::Action(CoreAction::PreviewBack) => Some(Act::Back),
            CoreResolution::Action(CoreAction::ToggleHelp) => Some(Act::Help),
            CoreResolution::Action(CoreAction::Navigate(m)) => {
                allowed(m, literal_page).then_some(Act::Move(m))
            }
            CoreResolution::Action(CoreAction::PageNext) if !popup => Some(Act::Page(1)),
            CoreResolution::Action(CoreAction::PagePrev) if !popup => Some(Act::Page(-1)),
            _ => None,
        }
    }

    /// Whether `key` does something over a table (in the `?` help when `in_help`). What the hints
    /// and the help rows name must pass this.
    pub fn table_acts_on(&self, key: &Key, in_help: bool, mode: TableMode) -> bool {
        match self.table_act(key, mode.popup && !in_help) {
            None => false,
            // The help scrolls on the vertical motions; the others do nothing while it is open.
            Some(Act::Move(m)) => {
                !in_help
                    || matches!(
                        m,
                        Motion::Down
                            | Motion::Up
                            | Motion::Top
                            | Motion::Bottom
                            | Motion::PageDown
                            | Motion::PageUp
                            | Motion::HalfDown
                            | Motion::HalfUp
                    )
            }
            Some(Act::Page(_)) => !in_help && mode.paged,
            Some(Act::Cell) => !in_help && mode.cells,
            Some(_) => true,
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

/// A help as lines: a heading and the aligned rows of each section.
fn lines_of(secs: &[CoreSection]) -> Vec<Line> {
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

/// What a table on screen lets the keys do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableMode {
    /// The full-cell popup is open.
    pub popup: bool,
    /// The file has several sheets (pages): `J` `K` turn them.
    pub paged: bool,
    /// The table has a cell to show: `Enter` opens the popup.
    pub cells: bool,
    /// The cells have spreadsheet addresses: the pages are sheets (the footer and the help say
    /// `sheet`, as the host does for a workbook).
    pub sheets: bool,
}

/// What a key does in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// `Enter` over a table: opens the full-cell popup, or closes it.
    Cell,
    /// A page turn over a table: 1 next, -1 previous.
    Page(i32),
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
    /// The page (sheet) of a table, from 0.
    table_page: usize,
    /// How many pages the session declares (`None` = it does not say, or has none).
    pages: Option<usize>,
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
            let mut req = ViewReq::new(size.width, body_rows);
            if st.table_page > 0 {
                req = req.page(st.table_page);
            }
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
            let (mut frame, redraw_after) = frame_of(
                view,
                &mut st,
                usize::from(body_rows),
                (&cx.lang, keys.lang()),
                &cx.title,
            );
            if let Frame::Table(new) = &mut frame {
                // A table read again (a resize, a page turn) keeps the cursor and the popup; a
                // page turn has reset them before asking.
                if let Frame::Table(old) = &mut base {
                    new.state = old.state;
                    new.popup = old.popup.take();
                }
                new.clamp_cursor();
                let layout = session.layout().clipped();
                st.pages = match layout.pages {
                    Some(PageLayout::Separate { count, titles }) => {
                        count.or((!titles.is_empty()).then_some(titles.len()))
                    }
                    _ => None,
                };
            }
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
        let mode = table_mode(&base, st.pages);
        let mut help_frame: Frame;
        let footer;
        let frame: &mut Frame = match st.help {
            Some(top) => {
                let (lines, title) = match mode {
                    Some(m) => (keys.table_help_lines(m), keys.table_help_title(m)),
                    None => (keys.help_lines(), keys.help_title()),
                };
                let rows = usize::from(body_rows);
                let top = top.min(lines.len().saturating_sub(rows));
                st.help = Some(top);
                help_frame = Frame::Lines(lines.into_iter().skip(top).take(rows).collect());
                footer = format!(" {}", title.trim());
                &mut help_frame
            }
            None => {
                footer = match mode {
                    Some(m) => {
                        if let Frame::Table(t) = &mut base {
                            if let Some(p) = t.popup.as_mut() {
                                p.footer = keys
                                    .popup_tokens()
                                    .into_iter()
                                    .map(|t| t.text)
                                    .collect::<Vec<_>>()
                                    .join("  ");
                            }
                        }
                        format!(" {}", keys.table_footer_tokens(m).join("  "))
                    }
                    None => draw::footer(
                        &keys.footer_tokens(st.scrolls),
                        &cx.title,
                        &base,
                        st.scroll,
                        st.scrolls,
                    ),
                };
                &mut base
            }
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
            Next::Event(AppEvent::Key(key)) => {
                let table_mode = table_mode(&base, st.pages);
                let popup_open = table_mode.is_some_and(|m| m.popup);
                let act = match table_mode {
                    Some(_) => keys.table_act(&key, popup_open && st.help.is_none()),
                    None => keys.act(&key),
                };
                // A key over a table never asks the session again, but a page turn: a
                // spreadsheet is read again on every `view`.
                if table_mode.is_some() {
                    need_view = false;
                }
                match (act, st.help) {
                    // `Q` and Ctrl-C end the app even from the help (the host's `Q` does); `q`
                    // and `Esc` close it.
                    (Some(Act::Back), Some(_)) => st.help = None,
                    // The popup is closed first; the app is left from the table.
                    (Some(Act::Back), None) if popup_open => {
                        if let Frame::Table(t) = &mut base {
                            t.popup = None;
                        }
                    }
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
                    (Some(Act::Move(m)), None) => match &mut base {
                        Frame::Table(t) => table_move(t, m),
                        _ if st.scrolls => scroll_by(m, &mut st),
                        _ => {}
                    },
                    (Some(Act::Cell), None) => {
                        if let Frame::Table(t) = &mut base {
                            t.popup = match t.popup {
                                Some(_) => None,
                                None if t.grid.ncols() > 0 => Some(PopupState::default()),
                                None => None,
                            };
                        }
                    }
                    (Some(Act::Page(d)), None) => {
                        if let (Frame::Table(t), Some(n)) = (&mut base, st.pages) {
                            let to = (st.table_page as i64 + i64::from(d))
                                .clamp(0, n.saturating_sub(1) as i64)
                                as usize;
                            if to != st.table_page {
                                st.table_page = to;
                                // The new sheet starts at its top left with no popup.
                                t.state = Default::default();
                                t.popup = None;
                                need_view = true;
                            }
                        }
                    }
                    (Some(Act::Cell | Act::Page(_)), Some(_)) => {}
                    (None, _) => {}
                }
            }
        }
    }
}

/// The mode of the table on screen (`None` = the frame is not a table).
fn table_mode(base: &Frame, pages: Option<usize>) -> Option<TableMode> {
    match base {
        Frame::Table(t) => Some(TableMode {
            popup: t.popup.is_some(),
            paged: pages.is_some_and(|n| n > 1),
            cells: t.grid.ncols() > 0,
            sheets: t.addressed,
        }),
        _ => None,
    }
}

/// Move the cell cursor, or scroll the popup while it is open.
fn table_move(t: &mut TableFrame, m: Motion) {
    if let Some(p) = t.popup.as_mut() {
        match m {
            Motion::Down => p.scroll = popup_scroll_by(p.scroll, 1),
            Motion::Up => p.scroll = popup_scroll_by(p.scroll, -1),
            Motion::Top => p.scroll = popup_scroll_to(false),
            Motion::Bottom => p.scroll = popup_scroll_to(true),
            Motion::PageDown => p.scroll = popup_scroll_by(p.scroll, popup_page_step(p.viewport)),
            Motion::PageUp => p.scroll = popup_scroll_by(p.scroll, -popup_page_step(p.viewport)),
            _ => {}
        }
        return;
    }
    let step = match m {
        Motion::Up => TableStep::Up,
        Motion::Down => TableStep::Down,
        Motion::Left => TableStep::Left,
        Motion::Right => TableStep::Right,
        Motion::Top => TableStep::FirstRow,
        Motion::Bottom => TableStep::LastRow,
        Motion::LineHome => TableStep::FirstCol,
        Motion::LineEnd => TableStep::LastCol,
        Motion::PageDown => TableStep::PageDown,
        Motion::PageUp => TableStep::PageUp,
        Motion::HalfDown => TableStep::HalfDown,
        Motion::HalfUp => TableStep::HalfUp,
    };
    t.state.step(step, t.grid.nrows(), t.grid.ncols());
}

/// Draw `frame` and `footer`, then the pictures over it. A frame on which a picture moved or went
/// is cleared and drawn again (the cells of its old place would stay on screen otherwise), and the
/// deletes of the kitty images that went are written after it. The right to write to the terminal
/// is held for all of it, so an exit's cleanup never lands between the chunks of a transmit.
fn draw_frame<B: Backend>(
    terminal: &mut Terminal<B>,
    frame: &mut Frame,
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
fn frame_of(
    view: View,
    st: &mut State,
    rows: usize,
    (lang_code, lang): (&str, Lang),
    name: &str,
) -> (Frame, Option<Duration>) {
    use crate::view::TextSource;
    st.scrolls = false;
    match view {
        View::Table(t) => (
            Frame::Table(Box::new(TableFrame {
                grid: t.grid,
                name: name.to_string(),
                note: t.title,
                rainbow: t.rainbow,
                addressed: t.addressed,
                lang,
                lang_code: lang_code.to_string(),
                state: Default::default(),
                popup: None,
            })),
            None,
        ),
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
