//! Stage 2.5: what a plugin's canvas can ask of the host besides cells (`docs/FEATURE-PLUGIN-
//! ARCHITECTURE.md` section 7-4, items 4 and 5).
//!
//! **Text input (`CanvasView::text_input`).** While the canvas on screen asks for it, the host is in
//! the `Surface::PluginInput` state: `handle_key` hands every key to [`App::plugin_input_key`]
//! before any keymap or fixed key, so the keys of layer 0 (`?`, `1`..`9`, `t`, `q`...) are text.
//! - Every key but `Esc` becomes a `konoma_plugin::Key` and goes to `Session::key`; a paste is one
//!   `KeyCode::Paste`. `Ctrl-C` is a key like the others (konoma's text inputs do not treat it
//!   specially either: the terminal is in raw mode, so it is never a signal).
//! - `Esc` is the host's: it leaves the state and the plugin is told with an `Esc` key. **Only the
//!   user takes the typing back**: after `Esc` the host captures again only when the canvas that
//!   asks for it is the answer to something the user did -- a declared action they pressed
//!   (`App::plugin_user_act`), or opening the file again (a new show starts clean). A timer tick,
//!   a refit, the answer to a key or a message in between never do, so a plugin cannot hold the
//!   user by asking again (`plugin_input_left`).
//! - Keys are never dropped. A key pressed while a job holds the session waits in a queue and
//!   goes, in order, in the next job (`PluginJob::Keys`). The queue belongs to the show: opening
//!   another file or tab drops it with the session. It is bounded ([`MAX_QUEUED_KEYS`], and a
//!   paste is cut at [`MAX_PASTE_BYTES`]): a plugin that stops answering does not make the host
//!   hold an unbounded backlog; past the bound new input is refused with a message, never
//!   reordered.
//! - *Stage 3 (a plugin in a child process):* a `key` or `view` that never returns keeps its
//!   worker thread (and the session) for good; in-process nothing can stop it. The child-process
//!   transport must kill on a timeout and allow one outstanding job per plugin id.
//!
//! **Redraw timer (`CanvasView::redraw_after`).** The answer arms a deadline (`now + d` with `d`
//! kept between 33 ms and 24 h); when it passes, the run loop asks the plugin for the view again,
//! once, and only while the canvas is on screen (preview mode, this file, and no overlay or dialog
//! in front of it). A deadline that passes under an overlay waits: the plugin is not called while
//! the canvas is hidden, and it is asked once when the overlay closes. A job still out when it
//! passes means the tick is skipped (no requests pile up); the next answer arms the deadline
//! again. The deadline is part of the run loop's wait (`canvas_poll_timeout`), so an idle screen
//! still wakes up for nothing else.

use std::time::{Duration, Instant};

use konoma_plugin::key::{KeyCode as PKey, KeyMods};
use konoma_plugin::Key;

use super::*;
use crate::i18n::{tr, Msg};

/// The shortest time between two timer redraws. A plugin may ask for less (even zero); the host
/// draws at most about 30 frames per second.
pub const MIN_REDRAW_AFTER: Duration = Duration::from_millis(33);

/// The longest time a plugin can ask to wait between two redraws; a longer (or overflowing) one is
/// this. Keeps `Instant + Duration` from overflowing on a value a plugin chose.
pub const MAX_REDRAW_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// How many keys wait for a session that a job holds, at most.
pub const MAX_QUEUED_KEYS: usize = 4096;

/// The longest paste handed to a plugin, in bytes; a longer one is cut (at a character boundary).
pub const MAX_PASTE_BYTES: usize = 1 << 20;

/// When a plugin that asked for `d` is asked again: `d` kept within the bounds, from now.
fn redraw_deadline(d: Duration) -> Instant {
    let d = d.clamp(MIN_REDRAW_AFTER, MAX_REDRAW_AFTER);
    Instant::now()
        .checked_add(d)
        .unwrap_or_else(|| Instant::now() + MIN_REDRAW_AFTER)
}

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
    // Shift is in the character itself (`'A'`) and in the key (`BackTab`): terminals differ in
    // whether they also report the bit, so the plugin never sees it with these two.
    if key.modifiers.contains(M::SHIFT) && !matches!(code, PKey::Char(_) | PKey::BackTab) {
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
        // Leaving is the host's, whatever is waiting: the latch is set even when the queue is full.
        if key.code == PKey::Esc {
            self.plugin_input_left = true;
        }
        if self.plugin_queue_is_full() {
            return;
        }
        self.plugin_key_queue.push(key);
        self.flush_plugin_keys();
    }

    /// A paste while the plugin takes the typing: one key holding all the text, cut at
    /// [`MAX_PASTE_BYTES`] (with a message).
    pub fn plugin_input_paste(&mut self, mut text: String) {
        self.pending_leader = None;
        if self.plugin_queue_is_full() {
            return;
        }
        if text.len() > MAX_PASTE_BYTES {
            let mut end = MAX_PASTE_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            self.flash = Some(tr(self.lang, Msg::PluginPasteCut).to_string());
        }
        self.plugin_key_queue.push(Key::paste(text));
        self.flush_plugin_keys();
    }

    /// Whether no more input is taken for the plugin: [`MAX_QUEUED_KEYS`] wait for a session that
    /// does not come back. The new input is refused with a message (the older is kept, in order).
    fn plugin_queue_is_full(&mut self) -> bool {
        if self.plugin_key_queue.len() < MAX_QUEUED_KEYS {
            return false;
        }
        self.flash = Some(tr(self.lang, Msg::PluginInputFull).to_string());
        true
    }

    /// Hand the queued keys to the plugin, in one job, if nothing holds the session. Called when a
    /// key is queued and whenever an answer gave the session back. Does nothing without keys, while
    /// a job holds the session, or while the plugin's work group is busy (a job parked in the
    /// group's slot could be replaced, and keys are never replaced).
    pub(super) fn flush_plugin_keys(&mut self) {
        if self.plugin_key_queue.is_empty() || !self.plugin_session_is_free() {
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
        self.plugin_user_act = false;
        self.canvas_redraw_at = None;
    }

    /// A result was installed. `now` is what it did to the canvas: `Some(None)` = the view is gone
    /// or is not a canvas, `Some(Some((text_input, redraw_after)))` = a new canvas, `None` = the
    /// view is as it was. `by_user` says the canvas answers something the user did (a declared
    /// action they pressed): only such an answer takes the typing back after `Esc`.
    pub(super) fn canvas_landed(
        &mut self,
        now: Option<Option<(bool, Option<Duration>)>>,
        by_user: bool,
    ) {
        match now {
            Some(Some((text_input, redraw_after))) => {
                // `Esc` left the typing: a canvas that asks for it again does not take it back by
                // itself (the plugin flapping `text_input` must not trap the user); the user does,
                // by an action of theirs the plugin answers with such a canvas.
                if text_input && by_user {
                    self.plugin_input_left = false;
                }
                self.canvas_redraw_at = redraw_after.map(redraw_deadline);
            }
            // A view that is not a canvas (a message, a failure) does not clear the latch either:
            // a message followed by a canvas asking for the typing is not the user's doing.
            Some(None) => {
                self.canvas_redraw_at = None;
            }
            // A result without a view (an action or a key that changed nothing): the canvas is as
            // it was, and so is a deadline that is still waiting. One that is not -- it passed
            // while a job was out, so the tick was skipped -- is armed again from the view on screen.
            None => {
                if self.canvas_redraw_at.is_none() {
                    let after = self.canvas_view().and_then(|c| c.redraw_after);
                    self.canvas_redraw_at = after.map(redraw_deadline);
                }
            }
        }
    }

    /// Whether the canvas that wants the timer is on screen: preview mode, and the plugin show is
    /// for the file this tab shows (`is_canvas_preview`). The condition of the GIF timers.
    fn canvas_redraw_visible(&self) -> bool {
        matches!(self.tab.mode, Mode::Preview) && self.is_canvas_preview()
    }

    /// Whether an overlay or a dialog (help, a menu, a list, a confirm...) is in front of the
    /// canvas: it is not fully visible, so the plugin is not asked to redraw it. The text input
    /// layer is the canvas's own and does not cover it.
    fn canvas_is_covered(&self) -> bool {
        !matches!(
            self.frontmost_layer(|_| true),
            UiLayer::Base(_) | UiLayer::PluginInput
        )
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
        // Under an overlay the deadline waits (it is not dropped): when the overlay closes it is
        // long past, and the plugin is asked once.
        if now < at || self.canvas_is_covered() {
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
        if !self.canvas_redraw_visible() || self.canvas_is_covered() {
            return None;
        }
        Some(at.saturating_duration_since(now).min(IDLE_WAIT))
    }

    /// Test-only: put the deadline where a test wants it.
    #[cfg(test)]
    pub(crate) fn set_canvas_redraw_at_for_test(&mut self, at: Option<Instant>) {
        self.canvas_redraw_at = at;
    }

    /// Test-only: the effect of a result on the canvas state, without a result.
    #[cfg(test)]
    pub(crate) fn canvas_landed_for_test(
        &mut self,
        now: Option<Option<(bool, Option<Duration>)>>,
        by_user: bool,
    ) {
        self.canvas_landed(now, by_user);
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
