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
pub struct CrosstermEvents;

impl EventSource for CrosstermEvents {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        loop {
            if let Some(d) = wait {
                match crossterm::event::poll(d) {
                    Ok(true) => {}
                    Ok(false) => return Next::Timeout,
                    Err(_) => return Next::End,
                }
            }
            match crossterm::event::read() {
                Ok(Event::Key(k)) => {
                    if let Some(key) = key_from_crossterm(&k) {
                        return Next::Event(AppEvent::Key(key));
                    }
                }
                Ok(Event::Resize(..)) => return Next::Event(AppEvent::Resize),
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
        restore();
        previous(info);
    }));
}

/// Leave the alternate screen and raw mode, show the cursor. Best effort: it runs from a panic
/// hook and from `Drop`, so it never fails or panics.
fn restore_terminal() {
    use crossterm::{cursor::Show, execute, terminal};
    let _ = terminal::disable_raw_mode();
    let _ = execute!(std::io::stdout(), terminal::LeaveAlternateScreen, Show);
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
        execute!(std::io::stdout(), terminal::EnterAlternateScreen, Hide)
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
