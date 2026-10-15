//! Stage 2.5: what a plugin's canvas can ask of the host besides cells (`docs/FEATURE-PLUGIN-
//! ARCHITECTURE.md` section 7-4, items 4 and 5).
//!
//! **Text input (`CanvasView::text_input`).** While the canvas on screen asks for it, the host is in
//! the `Surface::PluginInput` state: `handle_key` hands every key to [`App::plugin_input_key`]
//! before any keymap or fixed key, so the keys of layer 0 (`?`, `1`..`9`, `t`, `q`...) are text.
//! - Every key but `Esc` becomes a `konoma_plugin::Key` and goes to `Session::key`; a paste is one
//!   `KeyCode::Paste`. `Ctrl-C` is a key like the others (konoma's text inputs do not treat it
//!   specially either: the terminal is in raw mode, so it is never a signal).
//! - `Esc` is the host's: it leaves the state and the plugin is told with an `Esc` key. The host
//!   takes the typing again only after the plugin has shown a canvas that does not ask for it
//!   (`plugin_input_left`), so a plugin cannot hold the user.
//! - Keys are never dropped. A key pressed while a job holds the session waits in a queue and
//!   goes, in order, in the next job (`PluginJob::Keys`). The queue belongs to the show: opening
//!   another file or tab drops it with the session.
//!
//! **Redraw timer (`CanvasView::redraw_after`).** The answer arms a deadline (`now + max(d,
//! 33ms)`); when it passes, the run loop asks the plugin for the view again, once, and only while the
//! canvas is on screen (preview mode, this file) -- the same condition as the GIF timers. A job
//! still out when it passes means the tick is skipped (no requests pile up); the next answer arms
//! the deadline again. The deadline is part of the run loop's wait (`canvas_poll_timeout`), so
//! an idle screen still wakes up for nothing else.

use std::time::{Duration, Instant};

use konoma_plugin::key::{KeyCode as PKey, KeyMods};
use konoma_plugin::Key;

use super::*;

/// The shortest time between two timer redraws. A plugin may ask for less (even zero); the host
/// draws at most about 30 frames per second.
pub const MIN_REDRAW_AFTER: Duration = Duration::from_millis(33);

/// The longest the run loop sleeps for the deadline: the idle wait of the loop (so file events and
/// the like are still picked up on time).
const IDLE_WAIT: Duration = Duration::from_millis(100);

/// A terminal key as the plugin receives it. `None` for the keys a plugin has no name for (the
/// media keys, `Insert`, the lock keys); those are ignored while the plugin takes the typing.
pub(crate) fn plugin_key_of(key: &crossterm::event::KeyEvent) -> Option<Key> {
    use crossterm::event::{KeyCode as C, KeyModifiers as M};
    let code = match key.code {
        C::Char(c) => PKey::Char(c),
        C::Enter => PKey::Enter,
        C::Backspace => PKey::Backspace,
        C::Delete => PKey::Delete,
        C::Tab => PKey::Tab,
        C::BackTab => PKey::BackTab,
        C::Left => PKey::Left,
        C::Right => PKey::Right,
        C::Up => PKey::Up,
        C::Down => PKey::Down,
        C::Home => PKey::Home,
        C::End => PKey::End,
        C::PageUp => PKey::PageUp,
        C::PageDown => PKey::PageDown,
        C::Esc => PKey::Esc,
        C::F(n) => PKey::F(n),
        _ => return None,
    };
    let mut mods = KeyMods::NONE;
    if key.modifiers.contains(M::CONTROL) {
        mods |= KeyMods::CTRL;
    }
    if key.modifiers.contains(M::ALT) {
        mods |= KeyMods::ALT;
    }
    if key.modifiers.contains(M::SHIFT) {
        mods |= KeyMods::SHIFT;
    }
    Some(Key { code, mods })
}

impl App {
    /// Whether the host is taking the typing for the plugin: the canvas on screen (preview mode)
    /// asked for text input and `Esc` has not left it since. This is the `UiLayer::PluginInput`
    /// condition; it sits just above the base screen, so every dialog and overlay is in front.
    pub fn is_plugin_input(&self) -> bool {
        !self.plugin_input_left
            && matches!(self.tab.mode, Mode::Preview)
            && self.canvas_view().is_some_and(|c| c.text_input)
    }

    /// A key pressed while the plugin takes the typing. `Esc` leaves the state (and is told to the
    /// plugin); any other key is queued for the plugin. A key with no plugin name is ignored.
    pub fn plugin_input_key(&mut self, key: &crossterm::event::KeyEvent) {
        // A menu opened before the canvas asked for input would otherwise swallow the next key.
        self.pending_leader = None;
        let Some(key) = plugin_key_of(key) else {
            return;
        };
        if key.code == PKey::Esc {
            self.plugin_input_left = true;
        }
        self.plugin_key_queue.push(key);
        self.flush_plugin_keys();
    }

    /// A paste while the plugin takes the typing: one key holding all the text.
    pub fn plugin_input_paste(&mut self, text: String) {
        self.pending_leader = None;
        self.plugin_key_queue.push(Key::paste(text));
        self.flush_plugin_keys();
    }

    /// Hand the queued keys to the plugin, in one job, if nothing holds the session. Called when a
    /// key is queued and whenever an answer gave the session back. Does nothing without keys, while
    /// a job holds the session, or while the plugin's work group is busy (a job parked in the
    /// group's slot could be replaced, and keys are never replaced).
    pub(super) fn flush_plugin_keys(&mut self) {
        if self.plugin_key_queue.is_empty() {
            return;
        }
        let Some(show) = self.plugin.as_ref() else {
            return;
        };
        if show.session.is_none() || !matches!(show.phase, plugin_host::PluginPhase::Ready) {
            return;
        }
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let group_busy = registry
            .manifest(&show.id)
            .and_then(|m| m.work_group.as_ref())
            .and_then(|g| self.plugin_groups.get(g))
            .is_some_and(|slot| slot.busy);
        if group_busy {
            return;
        }
        let Some(show) = self.plugin.as_mut() else {
            return;
        };
        let Some(session) = show.session.take() else {
            return;
        };
        let (id, path, req) = (show.id.clone(), show.path.clone(), show.req.clone());
        let keys = std::mem::take(&mut self.plugin_key_queue);
        let work = self.plugin_work(
            id,
            path,
            plugin_host::PluginJob::Keys { session, keys },
            req,
        );
        self.dispatch_plugin_work(work, plugin_host::Wait::Result(Duration::ZERO));
    }

    /// Forget the keys waiting, the `Esc` latch and the redraw deadline (the show is going).
    pub(super) fn forget_canvas_state(&mut self) {
        self.plugin_key_queue.clear();
        self.plugin_input_left = false;
        self.canvas_redraw_at = None;
    }

    /// A result was installed. `now` is what it did to the canvas: `Some(None)` = the view is gone
    /// or is not a canvas, `Some(Some((text_input, redraw_after)))` = a new canvas, `None` = the
    /// view is as it was.
    pub(super) fn canvas_landed(&mut self, now: Option<Option<(bool, Option<Duration>)>>) {
        match now {
            Some(Some((text_input, redraw_after))) => {
                // The plugin shows a canvas that does not ask for the typing: the next one that
                // does is a new request, and the host takes it.
                if !text_input {
                    self.plugin_input_left = false;
                }
                self.canvas_redraw_at =
                    redraw_after.map(|d| Instant::now() + d.max(MIN_REDRAW_AFTER));
            }
            Some(None) => {
                self.plugin_input_left = false;
                self.canvas_redraw_at = None;
            }
            // A result without a view (an action or a key that changed nothing): the canvas is as
            // it was, and so is a deadline that is still waiting. One that is not -- it passed
            // while a job was out, so the tick was skipped -- is armed again from the view on screen.
            None => {
                if self.canvas_redraw_at.is_none() {
                    let after = self.canvas_view().and_then(|c| c.redraw_after);
                    self.canvas_redraw_at = after.map(|d| Instant::now() + d.max(MIN_REDRAW_AFTER));
                }
            }
        }
    }

    /// Whether the canvas that wants the timer is on screen: preview mode, and the plugin show is
    /// for the file this tab shows (`is_canvas_preview`). The condition of the GIF timers.
    fn canvas_redraw_visible(&self) -> bool {
        matches!(self.tab.mode, Mode::Preview) && self.is_canvas_preview()
    }

    /// When the redraw is due (for the tests).
    #[cfg(test)]
    pub fn canvas_redraw_deadline(&self) -> Option<Instant> {
        self.canvas_redraw_at
    }

    /// The run loop's tick: if the deadline has passed (`now` is given, so tests step the clock
    /// without sleeping) ask the plugin for the view again. Returns whether a request was started.
    /// A deadline that is set while the canvas is not on screen is dropped. When a job still holds
    /// the session the tick is skipped: the next answer arms the deadline again.
    pub fn canvas_redraw_if_due(&mut self, now: Instant) -> bool {
        let Some(at) = self.canvas_redraw_at else {
            return false;
        };
        if !self.canvas_redraw_visible() {
            self.canvas_redraw_at = None;
            return false;
        }
        if now < at {
            return false;
        }
        self.canvas_redraw_at = None;
        let req = self.plugin_view_req();
        self.plugin_request_view(req)
    }

    /// How long the run loop may sleep for the redraw: until the deadline, at most the idle wait.
    /// `None` when there is no deadline or the canvas is not on screen.
    pub fn canvas_poll_timeout(&self, now: Instant) -> Option<Duration> {
        let at = self.canvas_redraw_at?;
        if !self.canvas_redraw_visible() {
            return None;
        }
        Some(at.saturating_duration_since(now).min(IDLE_WAIT))
    }

    /// Test-only: put the deadline where a test wants it.
    #[cfg(test)]
    pub(crate) fn set_canvas_redraw_at_for_test(&mut self, at: Option<Instant>) {
        self.canvas_redraw_at = at;
    }

    /// Test-only: whether a job holds the plugin's session now.
    #[cfg(test)]
    pub(crate) fn plugin_session_is_out_for_test(&self) -> bool {
        self.plugin.as_ref().is_some_and(|p| p.session.is_none())
    }

    /// Test-only: the keys waiting for a free session.
    #[cfg(test)]
    pub(crate) fn plugin_key_queue_len_for_test(&self) -> usize {
        self.plugin_key_queue.len()
    }
}
