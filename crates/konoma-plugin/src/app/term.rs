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
    drop(leave_terminal(&mut std::io::stdout(), true));
}

/// The teardown of the terminal, written to `out`: the pictures first (they were placed on the
/// alternate screen, and a terminal deletes an image from the screen it is on), then raw mode
/// off (when `raw_mode`; it is a termios setting, not an escape), then bracketed paste off, the
/// alternate screen left and the cursor shown. The same order as konoma's `leave_for_good`.
///
/// Returns the right to write to the terminal it took (`None` when a stuck frame did not give it
/// within the wait: a stuck frame must not hold the exit). The signal path keeps it until the
/// process is gone, so a frame cannot be drawn onto the restored terminal.
#[must_use]
fn leave_terminal(
    out: &mut dyn std::io::Write,
    raw_mode: bool,
) -> Option<konoma_host_core::term::TerminalWrite> {
    use crossterm::{cursor::Show, execute, terminal};
    let writing =
        konoma_host_core::term::terminal_write_within(std::time::Duration::from_millis(200));
    super::pictures::delete_placed_pictures(out);
    if raw_mode {
        let _ = terminal::disable_raw_mode();
    }
    let mut out = out;
    let _ = execute!(
        &mut out,
        crossterm::event::DisableBracketedPaste,
        terminal::LeaveAlternateScreen,
        Show
    );
    writing
}

/// What the signal thread does for `sig` (`child_running`: a foreground child owns the terminal):
/// `None` when the signal is ignored (SIGINT while the child runs: Ctrl-C reached the child too
/// and it decides; leaving here would tear the screen down under it), otherwise `leave` (the
/// terminal teardown) and the conventional `128 + sig` to exit with, with the terminal guard
/// `leave` returned for the caller to keep until the process is gone.
fn exit_on_signal(
    sig: i32,
    child_running: bool,
    leave: impl FnOnce() -> Option<konoma_host_core::term::TerminalWrite>,
) -> Option<(i32, Option<konoma_host_core::term::TerminalWrite>)> {
    if konoma_host_core::term::ignore_signal(sig, child_running) {
        return None;
    }
    Some((128 + sig, leave()))
}

/// Restore the terminal when SIGTERM, SIGHUP or SIGINT ends the program, as konoma does. The
/// terminal is handed to a child (`hand_off`) with raw mode off, and Ctrl-C reaches the whole
/// process group: without this the default action would kill the program with the alternate
/// screen, bracketed paste and the pictures left on the terminal.
pub(super) fn install_exit_signals() {
    konoma_host_core::term::install_exit_signals(|sig, running| {
        let (code, writing) = exit_on_signal(sig, running, || {
            leave_terminal(&mut std::io::stdout(), true)
        })?;
        // The terminal stays ours until the process is gone.
        std::mem::forget(writing);
        Some(code)
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    // The signal numbers (the same on macOS and Linux).
    const SIGHUP: i32 = 1;
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;

    /// Runs `exit_on_signal` with a teardown that only counts its calls.
    fn answer(sig: i32, child_running: bool) -> (Option<i32>, usize) {
        let left = Cell::new(0);
        let code = exit_on_signal(sig, child_running, || {
            left.set(left.get() + 1);
            None
        })
        .map(|(code, _)| code);
        (code, left.get())
    }

    #[test]
    fn sigint_while_a_child_owns_the_terminal_is_ignored_and_leaves_nothing() {
        assert_eq!(answer(SIGINT, true), (None, 0));
    }

    #[test]
    fn sigint_without_a_child_restores_the_terminal_and_exits_130() {
        assert_eq!(answer(SIGINT, false), (Some(130), 1));
    }

    #[test]
    fn sigterm_and_sighup_end_the_program_even_while_a_child_runs() {
        assert_eq!(answer(SIGTERM, true), (Some(143), 1));
        assert_eq!(answer(SIGHUP, true), (Some(129), 1));
        assert_eq!(answer(SIGTERM, false), (Some(143), 1));
        assert_eq!(answer(SIGHUP, false), (Some(129), 1));
    }

    #[test]
    fn the_teardown_writes_paste_off_then_leaves_the_alternate_screen_then_shows_the_cursor() {
        let mut out: Vec<u8> = Vec::new();
        // No raw mode: that is a setting of the real terminal, which a test must not touch.
        drop(leave_terminal(&mut out, false));
        let got = String::from_utf8(out).unwrap();
        // Nothing was placed, so there are no picture deletes in front.
        assert_eq!(got, "\x1b[?2004l\x1b[?1049l\x1b[?25h");
    }
}
