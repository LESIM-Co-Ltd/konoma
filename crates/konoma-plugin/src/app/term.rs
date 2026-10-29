//! The real terminal: raw mode, the alternate screen, the panic hook and crossterm's events.

use std::io::Stdout;
use std::time::Duration;

use crossterm::event::{Event, KeyCode as CKey, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use super::looping::{AppEvent, EventSource, Next};
use super::AppError;
use crate::key::{Key, KeyCode, KeyMods};

/// A key press of crossterm as the plugin API's [`Key`]. `None` for a release or repeat of
/// nothing, and for a key the API has no name for. The case of a character carries Shift
/// (`SHIFT` is never set with a `Char` or a `BackTab`, whatever the terminal reports).
pub fn key_from_crossterm(ev: &KeyEvent) -> Option<Key> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let code = match ev.code {
        CKey::Char(c) => KeyCode::Char(c),
        CKey::Enter => KeyCode::Enter,
        CKey::Backspace => KeyCode::Backspace,
        CKey::Delete => KeyCode::Delete,
        CKey::Tab => KeyCode::Tab,
        CKey::BackTab => KeyCode::BackTab,
        CKey::Left => KeyCode::Left,
        CKey::Right => KeyCode::Right,
        CKey::Up => KeyCode::Up,
        CKey::Down => KeyCode::Down,
        CKey::Home => KeyCode::Home,
        CKey::End => KeyCode::End,
        CKey::PageUp => KeyCode::PageUp,
        CKey::PageDown => KeyCode::PageDown,
        CKey::Esc => KeyCode::Esc,
        CKey::F(n) => KeyCode::F(n),
        _ => return None,
    };
    let mut mods = KeyMods::NONE;
    if ev.modifiers.contains(KeyModifiers::CONTROL) {
        mods |= KeyMods::CTRL;
    }
    if ev.modifiers.contains(KeyModifiers::ALT) {
        mods |= KeyMods::ALT;
    }
    if ev.modifiers.contains(KeyModifiers::SHIFT)
        && !matches!(code, KeyCode::Char(_) | KeyCode::BackTab)
    {
        mods |= KeyMods::SHIFT;
    }
    Some(Key::new(code).with_mods(mods))
}

/// Events read from the terminal.
///
/// The keys typed while the startup terminal query was in flight come first (replayed in order),
/// and a kitty graphics reply that arrives after the query gave up is dropped from the keys
/// (`crossterm` cannot parse it, so it would arrive as the keys `G`, `i`, `=`...): both as konoma
/// does (`konoma_widgets::term_query`).
pub struct CrosstermEvents {
    pending: std::collections::VecDeque<Key>,
    late: konoma_widgets::term_query::LateKittyReply,
}

impl Default for CrosstermEvents {
    fn default() -> Self {
        CrosstermEvents {
            pending: std::collections::VecDeque::new(),
            late: konoma_widgets::term_query::LateKittyReply::off(),
        }
    }
}

impl CrosstermEvents {
    /// The events after a startup query: `typed` is what was typed during it (bytes), `late` drops
    /// the reply that may still come.
    pub fn after_query(typed: &[u8], late: konoma_widgets::term_query::LateKittyReply) -> Self {
        CrosstermEvents {
            pending: konoma_widgets::term_query::replay_keys(typed)
                .iter()
                .filter_map(key_from_crossterm)
                .collect(),
            late,
        }
    }
}

impl EventSource for CrosstermEvents {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        loop {
            if let Some(key) = self.pending.pop_front() {
                return Next::Event(AppEvent::Key(key));
            }
            if let Some(d) = wait {
                match crossterm::event::poll(d) {
                    Ok(true) => {}
                    Ok(false) => return Next::Timeout,
                    Err(_) => return Next::End,
                }
            }
            match crossterm::event::read() {
                Ok(Event::Key(k)) => {
                    self.pending
                        .extend(self.late.feed(k).iter().filter_map(key_from_crossterm));
                }
                Ok(Event::Resize(..)) => return Next::Event(AppEvent::Resize),
                Ok(Event::Paste(text)) => return Next::Event(AppEvent::Paste(text)),
                Ok(_) => {}
                Err(_) => return Next::End,
            }
        }
    }
}

/// Run `restore` when any thread panics, before the previous hook prints its message (the
/// message would be written into the alternate screen and lost, and the terminal would stay in
/// raw mode).
pub fn install_panic_hook_with(restore: fn()) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A panic a worker catches (and reports as a failure) is not the end of the program.
        if !konoma_base::panic::panic_is_caught_here() {
            restore();
        }
        previous(info);
    }));
}

/// Leave the alternate screen and raw mode, show the cursor. Best effort: it runs from a panic
/// hook and from `Drop`, so it never fails or panics.
fn restore_terminal() {
    use crossterm::{cursor::Show, execute, terminal};
    // The pictures first: they were placed on the alternate screen, and a terminal deletes an
    // image from the screen it is on. (Not waited for long: a stuck frame must not hold the exit.)
    {
        let _writing =
            konoma_host_core::term::terminal_write_within(std::time::Duration::from_millis(200));
        super::pictures::delete_placed_pictures(&mut std::io::stdout());
    }
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        crossterm::event::DisableBracketedPaste,
        terminal::LeaveAlternateScreen,
        Show
    );
}

/// The terminal in raw mode on the alternate screen; restored on `leave` and on `Drop`.
pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
}

impl TerminalGuard {
    pub fn enter() -> Result<TerminalGuard, AppError> {
        use crossterm::{cursor::Hide, execute, terminal};
        install_panic_hook_with(restore_terminal);
        terminal::enable_raw_mode().map_err(|e| AppError::Terminal(e.to_string()))?;
        // From here the terminal is changed: a failure restores it.
        let mut guard = TerminalGuard {
            terminal: Terminal::new(CrosstermBackend::new(std::io::stdout())).map_err(|e| {
                restore_terminal();
                AppError::Terminal(e.to_string())
            })?,
            active: true,
        };
        execute!(
            std::io::stdout(),
            terminal::EnterAlternateScreen,
            Hide,
            crossterm::event::EnableBracketedPaste
        )
        .map_err(|e| AppError::Terminal(e.to_string()))?;
        guard
            .terminal
            .clear()
            .map_err(|e| AppError::Terminal(e.to_string()))?;
        Ok(guard)
    }

    pub fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    /// Give the terminal back.
    pub fn leave(&mut self) {
        if self.active {
            self.active = false;
            restore_terminal();
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.leave();
    }
}
