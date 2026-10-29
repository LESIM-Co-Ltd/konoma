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
//!   paste is cut at [`MAX_PASTE_BYTES`], and the pastes together at [`MAX_QUEUED_PASTE_BYTES`]):
//!   a plugin that stops answering does not make the host hold an unbounded backlog; past the
//!   bound new input is refused with a message, never reordered. The last place of the queue is
//!   kept for `Esc`, so the way out is always told to the plugin.
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

use konoma_plugin::host::typing::{self, Admitted, NextJob, Notice};

use super::*;
use crate::i18n::{tr, Msg};

/// The shortest time between two timer redraws. A plugin may ask for less (even zero); the host
/// draws at most about 30 frames per second.
pub const MIN_REDRAW_AFTER: Duration = Duration::from_millis(33);

/// The longest time a plugin can ask to wait between two redraws; a longer (or overflowing) one is
/// this. Keeps `Instant + Duration` from overflowing on a value a plugin chose.
pub const MAX_REDRAW_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

// The bounds of the queue are the shared ones (`konoma_plugin::host::typing`).
#[cfg(test)]
pub use typing::{MAX_PASTE_BYTES, MAX_QUEUED_KEYS, MAX_QUEUED_PASTE_BYTES};

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

/// A terminal key as the plugin receives it (the shared conversion).
pub(crate) use typing::key_of as plugin_key_of;

impl App {
    /// Whether the host is taking the typing for the plugin: the canvas on screen (preview mode)
    /// asked for text input and `Esc` has not left it since. This is the `UiLayer::PluginInput`
    /// condition; it sits just above the base screen, so every dialog and overlay is in front.
    pub fn is_plugin_input(&self) -> bool {
        typing::is_capturing(
            self.plugin_input_left,
            matches!(self.tab.mode, Mode::Preview),
            self.canvas_view().is_some_and(|c| c.text_input),
        )
    }

    /// A key pressed while the plugin takes the typing. `Esc` leaves the state (and is told to the
    /// plugin); any other key is queued for the plugin. A key with no plugin name is ignored.
    pub fn plugin_input_key(&mut self, key: &crossterm::event::KeyEvent) {
        // A menu opened before the canvas asked for input would otherwise swallow the next key.
        self.pending_leader = None;
        let Some(key) = plugin_key_of(key) else {
            return;
        };
        // Leaving is the host's, whatever is waiting: the latch is set even when the queue is
        // full, and the plugin is told: the last place of the queue is kept for `Esc`.
        let admitted =
            typing::admit_key(&mut self.plugin_key_queue, &mut self.plugin_input_left, key);
        self.after_admit(admitted);
    }

    /// A paste while the plugin takes the typing: one key holding all the text, cut at
    /// [`MAX_PASTE_BYTES`] and at what is left of [`MAX_QUEUED_PASTE_BYTES`] (with a message).
    pub fn plugin_input_paste(&mut self, text: String) {
        self.pending_leader = None;
        let admitted = typing::admit_paste(&mut self.plugin_key_queue, text);
        self.after_admit(admitted);
    }

    /// Say what the shared queue said, and send what is queued.
    pub(super) fn after_admit(&mut self, admitted: Admitted) {
        if let Some(n) = admitted.notice {
            self.flash = Some(self.plugin_notice(n));
        }
        if admitted.queued {
            self.flush_plugin_keys();
        }
    }

    /// The words of a notice of the shared queue.
    pub(super) fn plugin_notice(&self, n: Notice) -> String {
        let msg = match n {
            Notice::InputFull => Msg::PluginInputFull,
            Notice::PasteCut => Msg::PluginPasteCut,
        };
        tr(self.lang, msg).to_string()
    }

    /// Hand what waits for the session to the plugin, **in the order it was pressed**: the queued
    /// keys and the queued actions are one line. The next thing is the oldest queued action when
    /// no key was queued before it (it goes as a job of its own), else the keys queued ahead of
    /// that action (one job, in order; all of them when no action waits). One job at a time: the
    /// answer gives the session back and this runs again. Called when a key or an action is
    /// queued and whenever an answer gave the session back. Does nothing while a job holds the
    /// session or the plugin's work group is busy (a job parked in the group's slot could be
    /// replaced, and keys and actions are never replaced).
    pub(super) fn flush_plugin_keys(&mut self) {
        if !self.plugin_session_is_free() {
            return;
        }
        let Some(show) = self.plugin.as_mut() else {
            return;
        };
        match typing::next_job(&mut self.plugin_key_queue, &mut show.flow.acts) {
            None => {}
            Some(NextJob::Act(action)) => self.send_queued_act(action),
            Some(NextJob::Keys(keys)) => {
                let Some(session) = show.session.take() else {
                    return;
                };
                let (id, path, req) = (show.id.clone(), show.path.clone(), show.req.clone());
                let work = self.plugin_work(
                    id,
                    path,
                    plugin_host::PluginJob::Keys { session, keys },
                    req,
                );
                self.dispatch_plugin_work(work, plugin_host::Wait::Result(Duration::ZERO));
            }
        }
    }

    /// Forget the keys waiting, the `Esc` latch and the redraw deadline (the show is going).
    pub(super) fn forget_canvas_state(&mut self) {
        typing::forget(
            &mut self.plugin_key_queue,
            &mut self.plugin_input_left,
            &mut self.plugin_user_act,
        );
        self.canvas_redraw_at = None;
        self.canvas_more_at = None;
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
                typing::retake(&mut self.plugin_input_left, text_input, by_user);
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

    /// Pictures of the canvas were left for later and the last answer made progress: ask the plugin
    /// again (its answer decodes the next ones from the sources the host kept), **at most once per
    /// [`MIN_REDRAW_AFTER`]** and only while the canvas is on screen. When the last ask is too
    /// recent, or the canvas is hidden or covered, the redraw deadline is set to the first moment
    /// an ask is allowed instead (the timer asks, and drops the deadline when the canvas is not
    /// on screen). `now` is given so tests step the clock.
    pub(super) fn canvas_ask_more(&mut self, now: Instant) {
        let due = self
            .canvas_more_at
            .and_then(|t| t.checked_add(MIN_REDRAW_AFTER))
            .unwrap_or(now);
        if due <= now && self.canvas_redraw_visible() && !self.canvas_is_covered() {
            self.canvas_more_at = Some(now);
            if let Some(show) = self.plugin.as_mut() {
                show.flow.wants_view = true;
            }
            return;
        }
        let at = due.max(now);
        self.canvas_redraw_at = Some(self.canvas_redraw_at.map_or(at, |old| old.min(at)));
    }

    /// [`Self::canvas_ask_more`] for an answer that has just landed (`now`). The floor between two
    /// asks counts from this landing, the end of the job that took the time, when the answer was
    /// itself the answer to an ask (a follow-up or the timer's: `canvas_more_at` is set): a job
    /// longer than the floor must not make the next ask due at once, or the asking would run back
    /// to back for as long as pictures keep coming. The answer to the user's own action (no ask
    /// before it) is followed up at once.
    pub(crate) fn canvas_ask_more_after_landing(&mut self, now: Instant) {
        if self.canvas_more_at.is_some() {
            self.canvas_more_at = Some(now);
        }
        self.canvas_ask_more(now);
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
        // Any ask counts for the floor between two asks (the follow-up for pictures included).
        self.canvas_more_at = Some(now);
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

    /// Test-only: the bytes of pasted text waiting for a free session.
    #[cfg(test)]
    pub(crate) fn plugin_queued_paste_bytes_for_test(&self) -> usize {
        typing::queued_paste_bytes(&self.plugin_key_queue)
    }

    /// Test-only: whether a view is wanted as soon as the session is at hand (the follow-up ask
    /// for pictures, `R`, a page turn).
    #[cfg(test)]
    pub(crate) fn plugin_wants_view_for_test(&self) -> bool {
        self.plugin.as_ref().is_some_and(|p| p.flow.wants_view)
    }

    /// Test-only: forget the wish for a view.
    #[cfg(test)]
    pub(crate) fn clear_plugin_wants_view_for_test(&mut self) {
        if let Some(p) = self.plugin.as_mut() {
            p.flow.wants_view = false;
        }
    }

    /// Test-only: the follow-up ask for pictures left for later, at the given time.
    #[cfg(test)]
    pub(crate) fn canvas_ask_more_for_test(&mut self, now: Instant) {
        self.canvas_ask_more(now);
    }

    /// Test-only: the keys waiting for a free session.
    #[cfg(test)]
    pub(crate) fn plugin_key_queue_len_for_test(&self) -> usize {
        self.plugin_key_queue.len()
    }
}
